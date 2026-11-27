//! Georeferenced rasters on a plain lat/lon (EPSG:4326) grid.
//!
//! Row 0 is the northern edge, column 0 the western edge; pixel (x, y) covers
//! `[west + x·dx, west + (x+1)·dx] × [north − (y+1)·dy, north − y·dy]`.

use serde::{Deserialize, Serialize};

/// [west, south, east, north] in degrees.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bounds(pub [f64; 4]);

impl Bounds {
    pub const fn west(&self) -> f64 {
        self.0[0]
    }
    pub const fn south(&self) -> f64 {
        self.0[1]
    }
    pub const fn east(&self) -> f64 {
        self.0[2]
    }
    pub const fn north(&self) -> f64 {
        self.0[3]
    }
    pub fn contains(&self, lon: f64, lat: f64) -> bool {
        lon >= self.west() && lon <= self.east() && lat >= self.south() && lat <= self.north()
    }
}

/// A row-major 2D raster. `T = f32` holds physical values with NaN as "no data".
#[derive(Clone, Debug, PartialEq)]
pub struct Grid<T> {
    pub w: usize,
    pub h: usize,
    pub bounds: Bounds,
    pub data: Vec<T>,
}

impl<T: Clone> Grid<T> {
    pub fn filled(w: usize, h: usize, bounds: Bounds, v: T) -> Self {
        Self { w, h, bounds, data: vec![v; w * h] }
    }
}

impl<T> Grid<T> {
    pub fn from_vec(w: usize, h: usize, bounds: Bounds, data: Vec<T>) -> Self {
        assert_eq!(data.len(), w * h, "grid data length");
        Self { w, h, bounds, data }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> &T {
        &self.data[y * self.w + x]
    }

    #[inline]
    pub fn at_mut(&mut self, x: usize, y: usize) -> &mut T {
        &mut self.data[y * self.w + x]
    }

    pub fn dx(&self) -> f64 {
        (self.bounds.east() - self.bounds.west()) / self.w as f64
    }
    pub fn dy(&self) -> f64 {
        (self.bounds.north() - self.bounds.south()) / self.h as f64
    }

    /// Longitude of the centre of column x.
    pub fn lon(&self, x: usize) -> f64 {
        self.bounds.west() + (x as f64 + 0.5) * self.dx()
    }
    /// Latitude of the centre of row y.
    pub fn lat(&self, y: usize) -> f64 {
        self.bounds.north() - (y as f64 + 0.5) * self.dy()
    }

    /// The pixel containing (lon, lat), as `probe()` in the legacy app (floor of the linear position).
    pub fn pixel_of(&self, lon: f64, lat: f64) -> Option<(usize, usize)> {
        let fx = ((lon - self.bounds.west()) / (self.bounds.east() - self.bounds.west()) * self.w as f64).floor();
        let fy = ((self.bounds.north() - lat) / (self.bounds.north() - self.bounds.south()) * self.h as f64).floor();
        (fx >= 0.0 && fy >= 0.0 && (fx as usize) < self.w && (fy as usize) < self.h).then_some((fx as usize, fy as usize))
    }

    pub fn map<U>(&self, f: impl FnMut(&T) -> U) -> Grid<U> {
        Grid { w: self.w, h: self.h, bounds: self.bounds, data: self.data.iter().map(f).collect() }
    }
}

impl Grid<f32> {
    /// Value at (lon, lat), `None` outside the grid or on a no-data pixel.
    pub fn sample(&self, lon: f64, lat: f64) -> Option<f32> {
        let (x, y) = self.pixel_of(lon, lat)?;
        let v = *self.at(x, y);
        v.is_finite().then_some(v)
    }
}

/// An 8-bit RGBA image (premultiplied: no), row-major, row 0 = north. What the map draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Rgba {
    pub w: usize,
    pub h: usize,
    pub bounds: Bounds,
    pub px: Vec<[u8; 4]>,
}
