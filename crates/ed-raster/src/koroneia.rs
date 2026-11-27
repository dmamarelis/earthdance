//! The "Real data" build of the legacy `tools/build_real_data.py`, as a pure function: decoded rasters in,
//! grids, images and series out. No IO except the optional [`read_inputs_from_dirs`] helper.
//!
//! Method (unchanged from the legacy script):
//! * Every band is resampled bilinearly onto the site grid ([`crate::warp`]).
//! * **Optical water**: `(B04 − B08)/(B04 + B08) > 0.1 && B08 < 1000`. Optical dates are used up to the first
//!   NISAR pass; a date more than 20 % below the previous accepted one is dropped (turbid water / cloud).
//!   The lake = union of the used optical water masks; the drawn zone = lake dilated by 22 px.
//! * **Radar**: dB = 10·log10(power), 3×3 median (invalid pixels filled with the median of the valid ones
//!   first, then set back to nodata). Water = `dB <= −18` inside the lake. An orbit (date ordinal mod 12) is
//!   used only if its first pass sees ≥ 50 % of the lake as water; a pass below 0.6 × min(previous, next) of
//!   the orbit-filtered list is dropped (wind). Change = dB − first pass of the same orbit.
//! * Passes whose file is empty (or all nodata) are kept on the timeline as "empty" frames; dropped passes
//!   are removed from it.
//!
//! Numerics follow numpy/scipy where the result is observable: f32 arithmetic for the raster maths the
//! script did in float32, f64 for blurs and areas, numpy's pairwise sums for means, half-to-even rounding
//! of the hover values.

use std::io;
use std::path::Path;

use ed_core::{Bounds, Date, Grid, Rgba};
use rayon::prelude::*;

use crate::filters::{dilate_cross, gaussian, largest_component, median3};
use crate::geotiff::{self, SourceRaster};
use crate::render::{Stop, colorize, edge_alpha, optical_image};
use crate::warp::{UnsupportedCrs, warp_bilinear};

/// A circular region of interest (km radius around a lon/lat centre, using the site's km/degree factors).
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub id: String,
    /// [lon, lat]
    pub center: [f64; 2],
    pub radius_km: f64,
    /// The lake zone: its water series is the whole-lake count (not clipped to the circle) and it carries
    /// the reservoir block. (`id == "koroneia_core"` in the legacy script.)
    pub core: bool,
}

/// Everything site-specific about the build. [`SiteConfig::koronia`] holds the legacy constants.
#[derive(Clone, Debug, PartialEq)]
pub struct SiteConfig {
    /// [west, south, east, north], EPSG:4326.
    pub bounds: Bounds,
    /// Pixel size in degrees (both axes).
    pub res: f64,
    pub w: usize,
    pub h: usize,
    /// km per degree of longitude / latitude at the site (flat-earth areas and distances).
    pub km_lon: f64,
    pub km_lat: f64,
    /// Hover grid = image / `hover_block` (block means).
    pub hover_block: usize,
    /// Drawn area = lake dilated by this many pixels.
    pub lake_margin_px: usize,
    pub sar_water_db: f32,
    pub ndwi_min: f32,
    pub nir_max: f32,
    /// An optical date below `optical_drop ×` the previous accepted one is not used.
    pub optical_drop: f64,
    /// A pass below `wind_dip × min(prev, next)` is dropped.
    pub wind_dip: f64,
    /// An orbit's first pass must see this share of the lake as water.
    pub orbit_min_lake: f64,
    /// Indicative reservoir limit as a share of the capacity.
    pub limit_share: f64,
    /// Repeat cycle in days: orbit = proleptic ordinal of the date mod this.
    pub orbit_repeat_days: i32,
    pub zones: Vec<Zone>,
}

impl SiteConfig {
    /// Lake Koronia, as in `tools/build_real_data.py`.
    pub fn koronia() -> Self {
        let bounds = [23.03, 40.62, 23.27, 40.74];
        let res = 0.0002;
        Self {
            bounds: Bounds(bounds),
            res,
            // round((e − w)/res): 1200 × 600.
            w: ((bounds[2] - bounds[0]) / res).round_ties_even() as usize,
            h: ((bounds[3] - bounds[1]) / res).round_ties_even() as usize,
            km_lon: 111.13 * 40.68f64.to_radians().cos(),
            km_lat: 110.57,
            hover_block: 4,
            lake_margin_px: 22,
            sar_water_db: -18.0,
            ndwi_min: 0.1,
            nir_max: 1000.0,
            optical_drop: 0.8,
            wind_dip: 0.6,
            orbit_min_lake: 0.5,
            limit_share: 0.25,
            orbit_repeat_days: 12,
            zones: vec![
                Zone { id: "koroneia_core".into(), center: [23.145, 40.682], radius_km: 4.0, core: true },
                Zone { id: "koroneia_shore".into(), center: [23.11, 40.66], radius_km: 2.5, core: false },
            ],
        }
    }

    /// Area of one pixel in km².
    pub fn px_km2(&self) -> f64 {
        (self.res * self.km_lon) * (self.res * self.km_lat)
    }

    /// Longitude of column x's centre, computed as the script did (`west + (x + 0.5)·res`).
    pub fn lon(&self, x: usize) -> f64 {
        self.bounds.west() + (x as f64 + 0.5) * self.res
    }
    /// Latitude of row y's centre (`north − (y + 0.5)·res`).
    pub fn lat(&self, y: usize) -> f64 {
        self.bounds.north() - (y as f64 + 0.5) * self.res
    }

    /// Orbit class of a pass: Python's `date.toordinal() % repeat` (ordinal 1 = 0001-01-01).
    pub fn orbit(&self, d: Date) -> i32 {
        (d.days() + 719_163).rem_euclid(self.orbit_repeat_days)
    }

    fn empty_grid<T: Clone>(&self, v: T) -> Grid<T> {
        Grid::filled(self.w, self.h, self.bounds, v)
    }

    /// The circle mask of a zone.
    pub fn zone_mask(&self, z: &Zone) -> Grid<bool> {
        let r2 = z.radius_km * z.radius_km;
        let mut g = self.empty_grid(false);
        for y in 0..self.h {
            let dy = (self.lat(y) - z.center[1]) * self.km_lat;
            for x in 0..self.w {
                let dx = (self.lon(x) - z.center[0]) * self.km_lon;
                *g.at_mut(x, y) = dx * dx + dy * dy <= r2;
            }
        }
        g
    }
}

/// One NISAR GCOV file. `raster: None` = a zero-byte file.
#[derive(Clone, Debug)]
pub struct NisarInput {
    pub date: Date,
    /// File name, e.g. `koroneia_nisar_2026-06-18.tif` (goes into the source label).
    pub name: String,
    pub raster: Option<SourceRaster>,
}

/// One Sentinel-2 (HLS) date: red (B04) and NIR (B08). `None` = missing or zero-byte file.
#[derive(Clone, Debug)]
pub struct OpticalInput {
    pub date: Date,
    /// File stem without the band, e.g. `koroneia_optical_2026-02-16`.
    pub name: String,
    pub red: Option<SourceRaster>,
    pub nir: Option<SourceRaster>,
}

#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub nisar: Vec<NisarInput>,
    pub optical: Vec<OpticalInput>,
}

/// Legend ramps of the two radar layers (`layers[].legend.stops` of the manifest).
#[derive(Clone, Debug, PartialEq)]
pub struct Legends {
    pub hh: Vec<Stop>,
    pub change: Vec<Stop>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sensor {
    Sentinel2,
    Nisar,
}

impl Sensor {
    /// The label used in `regions.json` (`water_source`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sentinel2 => "Sentinel-2",
            Self::Nisar => "NISAR",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropReason {
    /// The orbit's first pass saw less than half of the lake as water.
    SteepOrbit,
    /// Far below the passes before and after.
    Wind,
}

impl DropReason {
    /// The legacy log wording.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SteepOrbit => "steep orbit: calm water not dark enough",
            Self::Wind => "far below the passes before and after: wind-roughened water",
        }
    }
}

/// The radar layers of one pass on the timeline.
#[derive(Clone, Debug)]
pub struct SarLayers {
    /// Median-filtered HH backscatter in dB (NaN = no data).
    pub hh_db: Grid<f32>,
    /// dB minus the first pass of the same orbit.
    pub change_db: Grid<f32>,
    /// Display opacity: footprint edge × zone edge, 0..1.
    pub alpha: Grid<f32>,
    pub hh_rgba: Rgba,
    pub change_rgba: Rgba,
}

/// A frame of the hh/change timeline.
#[derive(Clone, Debug)]
pub struct SarFrame {
    pub date: Date,
    /// "NISAR L2 GCOV HHHH · <file>"
    pub source: String,
    /// `None` = empty pass, shown as "no data".
    pub layers: Option<SarLayers>,
}

impl SarFrame {
    pub const fn is_empty(&self) -> bool {
        self.layers.is_none()
    }
}

/// An image of the optical view.
#[derive(Clone, Debug)]
pub struct OpticalFrame {
    pub date: Date,
    /// "HLS S30 · <stem>"
    pub source: String,
    pub rgba: Rgba,
}

/// Per-date diagnostics of the optical water count.
#[derive(Clone, Debug, PartialEq)]
pub struct OpticalStat {
    pub date: Date,
    /// Water anywhere on the grid.
    pub water_km2: f64,
    pub used: bool,
}

/// Per-pass diagnostics of the radar water count (non-empty passes only).
#[derive(Clone, Debug, PartialEq)]
pub struct PassStat {
    pub date: Date,
    pub orbit: i32,
    /// Water inside the lake.
    pub water_km2: f64,
    /// `None` = used.
    pub dropped: Option<DropReason>,
}

/// A pass left off the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct DroppedPass {
    pub date: Date,
    pub water_km2: f64,
    pub reason: DropReason,
}

/// The lake-area series of a zone (`regions[].series`).
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneSeries {
    pub id: String,
    pub dates: Vec<Date>,
    /// Rounded to 2 decimals.
    pub water_km2: Vec<f64>,
    /// Mean HH dB over the zone's valid pixels, 2 decimals; `None` on optical dates (or no valid pixel).
    pub hh_db: Vec<Option<f64>>,
    pub water_source: Vec<Sensor>,
}

/// The reservoir block of the core zone.
#[derive(Clone, Debug, PartialEq)]
pub struct Reservoir {
    pub zone_id: String,
    /// Largest surface of the series (km², 2 decimals).
    pub capacity: f64,
    /// `limit_share × capacity` (2 decimals).
    pub limit: f64,
    pub dates: Vec<Date>,
    pub area_km2: Vec<f64>,
    /// Lake edge on the last date: 72-direction polygon, closed (first point repeated), [lon, lat] to 5 decimals.
    pub outline: Vec<[f64; 2]>,
}

/// Everything the build produces.
#[derive(Clone, Debug)]
pub struct Build {
    /// hh/change timeline: kept passes (with layers) and empty passes, in date order.
    pub sar_frames: Vec<SarFrame>,
    pub dropped: Vec<DroppedPass>,
    pub optical_frames: Vec<OpticalFrame>,
    pub optical_stats: Vec<OpticalStat>,
    pub pass_stats: Vec<PassStat>,
    /// Union of the used optical water masks.
    pub lake: Grid<bool>,
    pub lake_km2: f64,
    /// Drawn area: the lake + margin.
    pub zone: Grid<bool>,
    /// Soft edge of the drawn area, 0..1.
    pub zone_alpha: Grid<f32>,
    pub series: Vec<ZoneSeries>,
    pub reservoir: Option<Reservoir>,
    pub hover_block: usize,
}

impl Build {
    /// The map-hover grid of a layer (`save_values`): block means inside the drawn zone.
    pub fn hover(&self, values: &Grid<f32>) -> Grid<i16> {
        block_values(values, Some(&self.zone), self.hover_block)
    }
}

/// `save_values`: `block × block` NaN-aware means, ×10, rounded half to even, clamped to ±32767, as i16;
/// −32768 where a block has no data. Pixels outside `mask` count as no data. Trailing rows/columns that do
/// not fill a block are ignored; the result covers `(w / block) × (h / block)` cells over the same bounds
/// (shrunk accordingly).
pub fn block_values(values: &Grid<f32>, mask: Option<&Grid<bool>>, block: usize) -> Grid<i16> {
    let (w, h) = (values.w / block, values.h / block);
    let mut out = Vec::with_capacity(w * h);
    for by in 0..h {
        for bx in 0..w {
            let mut sum = 0f32;
            let mut n = 0u32;
            for y in by * block..(by + 1) * block {
                for x in bx * block..(bx + 1) * block {
                    let v = *values.at(x, y);
                    if v.is_finite() && mask.is_none_or(|m| *m.at(x, y)) {
                        sum += v;
                        n += 1;
                    }
                }
            }
            out.push(if n == 0 {
                i16::MIN
            } else {
                let mean = f64::from(sum) / f64::from(n);
                (mean * 10.0).round_ties_even().clamp(-32767.0, 32767.0) as i16
            });
        }
    }
    let b = values.bounds;
    let bounds = Bounds([
        b.west(),
        b.north() - (h * block) as f64 * values.dy(),
        b.west() + (w * block) as f64 * values.dx(),
        b.north(),
    ]);
    Grid::from_vec(w, h, bounds, out)
}

/// numpy's pairwise summation (`np.sum` over a contiguous 1-D array, accumulating in `T`).
fn pairwise_sum<T: Copy + Default + std::ops::Add<Output = T>>(a: &[T]) -> T {
    let n = a.len();
    if n < 8 {
        return a.iter().fold(T::default(), |s, &v| s + v);
    }
    if n <= 128 {
        let mut r = [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
        let mut i = 8;
        while i + 8 <= n {
            for (k, rk) in r.iter_mut().enumerate() {
                *rk = *rk + a[i + k];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        for &v in &a[i..] {
            res = res + v;
        }
        return res;
    }
    let m = n / 2 - (n / 2) % 8;
    pairwise_sum(&a[..m]) + pairwise_sum(&a[m..])
}

/// `round(x, 2)` / `round(x, 5)` as Python does (correctly rounded decimal).
fn round_dp(x: f64, dp: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.dp$}").parse().unwrap_or(x)
}

/// Median of a non-empty f32 slice as `np.median` (mean of the two middle values in f32 for even n).
fn median_f32(mut v: Vec<f32>) -> f32 {
    let n = v.len();
    let mid = n / 2;
    let (_, &mut hi, _) = v.select_nth_unstable_by(mid, f32::total_cmp);
    if n % 2 == 1 {
        return hi;
    }
    let lo = v[..mid].iter().copied().fold(f32::NEG_INFINITY, f32::max);
    (lo + hi) / 2.0
}

/// The raster if it holds any data (the script treated all-nodata files like empty ones).
fn usable(r: &Option<SourceRaster>) -> Option<&SourceRaster> {
    r.as_ref().filter(|r| !r.is_all_nodata())
}

fn count(m: &[bool]) -> usize {
    m.iter().filter(|&&b| b).count()
}

/// A radar pass after the per-pass maths (before the orbit/wind filters).
struct Pass {
    idx: usize,
    orbit: i32,
    valid: Vec<bool>,
    db: Grid<f32>,
}

/// Lake edge as a 72-direction polygon (`outline` in the legacy script): per direction, the farthest pixel of
/// the largest 4-connected water body from its centroid. Closed: the first point is repeated.
pub fn outline(cfg: &SiteConfig, mask: &Grid<bool>) -> Vec<[f64; 2]> {
    let Some(big) = largest_component(mask) else {
        return Vec::new();
    };
    let (mut lons, mut lats) = (Vec::new(), Vec::new());
    for y in 0..big.h {
        for x in 0..big.w {
            if *big.at(x, y) {
                lons.push(cfg.lon(x));
                lats.push(cfg.lat(y));
            }
        }
    }
    let n = lons.len() as f64;
    let cx = pairwise_sum(&lons) / n;
    let cy = pairwise_sum(&lats) / n;
    let pts: Vec<(f64, f64)> = lons
        .iter()
        .zip(&lats)
        .map(|(&lo, &la)| {
            let (px, py) = ((lo - cx) * cfg.km_lon, (la - cy) * cfg.km_lat);
            (py.atan2(px), px.hypot(py))
        })
        .collect();
    let pi = std::f64::consts::PI;
    let mut ring = Vec::with_capacity(73);
    for i in 0..72 {
        let a = -pi + f64::from(i) * 2.0 * pi / 72.0;
        let mut best: Option<(usize, f64)> = None;
        for (k, &(ang, dist)) in pts.iter().enumerate() {
            let d = ang - a;
            // |angle(exp(i·d))| < π/72: the wrapped angular difference.
            if d.sin().atan2(d.cos()).abs() < pi / 72.0 && best.is_none_or(|(_, bd)| dist > bd) {
                best = Some((k, dist));
            }
        }
        if let Some((k, _)) = best {
            ring.push([round_dp(lons[k], 5), round_dp(lats[k], 5)]);
        }
    }
    if let Some(&first) = ring.first() {
        ring.push(first);
    }
    ring
}

/// Runs the whole build. Inputs need not be sorted (they are processed in date order).
///
/// # Errors
/// If an input raster is in a CRS that cannot be reprojected.
pub fn build(cfg: &SiteConfig, inputs: &Inputs, legends: &Legends) -> Result<Build, UnsupportedCrs> {
    let px_km2 = cfg.px_km2();
    let (w, h, bounds) = (cfg.w, cfg.h, cfg.bounds);
    let warp = |r: &SourceRaster| warp_bilinear(r, bounds, w, h);
    let zone_masks: Vec<Grid<bool>> = cfg.zones.iter().map(|z| cfg.zone_mask(z)).collect();

    let mut optical_in: Vec<&OpticalInput> = inputs.optical.iter().collect();
    optical_in.sort_by_key(|o| o.date);
    let mut nisar_in: Vec<&NisarInput> = inputs.nisar.iter().collect();
    nisar_in.sort_by_key(|p| p.date);

    // ── Sentinel-2: water masks and pseudo-colour images.
    struct Optical {
        date: Date,
        source: String,
        rgba: Rgba,
        water: Vec<bool>,
        water_km2: f64,
    }
    let optical: Vec<Optical> = optical_in
        .par_iter()
        .filter_map(|o| Some((o, usable(&o.red)?, usable(&o.nir)?)))
        .map(|(o, red, nir)| -> Result<Optical, UnsupportedCrs> {
            let (red, nir) = (warp(red)?, warp(nir)?);
            let water: Vec<bool> = red
                .data
                .iter()
                .zip(&nir.data)
                .map(|(&r, &n)| (r - n) / (r + n) > cfg.ndwi_min && n < cfg.nir_max)
                .collect();
            let water_km2 = count(&water) as f64 * px_km2;
            Ok(Optical { date: o.date, source: format!("HLS S30 · {}", o.name), rgba: optical_image(&red, &nir), water, water_km2 })
        })
        .collect::<Result<_, _>>()?;

    let first_sar = nisar_in.first().map(|p| p.date);
    let mut last_ok: Option<f64> = None;
    let optical_stats: Vec<OpticalStat> = optical
        .iter()
        .map(|p| {
            let used = first_sar.is_none_or(|f| p.date < f) && last_ok.is_none_or(|l| p.water_km2 >= cfg.optical_drop * l);
            if used {
                last_ok = Some(p.water_km2);
            }
            OpticalStat { date: p.date, water_km2: p.water_km2, used }
        })
        .collect();

    let mut lake = vec![false; w * h];
    for (p, s) in optical.iter().zip(&optical_stats) {
        if s.used {
            for (l, &m) in lake.iter_mut().zip(&p.water) {
                *l |= m;
            }
        }
    }
    let lake = Grid::from_vec(w, h, bounds, lake);
    let lake_km2 = count(&lake.data) as f64 * px_km2;
    let zone = dilate_cross(&lake, cfg.lake_margin_px);
    let zone_alpha = gaussian(&zone.map(|&b| f64::from(u8::from(b))), 4.0).map(|&v| v.clamp(0.0, 1.0));

    let optical_frames: Vec<OpticalFrame> = optical
        .iter()
        .map(|o| {
            let mut rgba = o.rgba.clone();
            for (p, &za) in rgba.px.iter_mut().zip(&zone_alpha.data) {
                p[3] = (f64::from(p[3]) * za) as u8;
            }
            OpticalFrame { date: o.date, source: o.source.clone(), rgba }
        })
        .collect();

    // ── NISAR: dB per pass.
    let passes: Vec<Pass> = nisar_in
        .par_iter()
        .enumerate()
        .filter_map(|(idx, p)| Some((idx, usable(&p.raster)?)))
        .map(|(idx, r)| -> Result<Pass, UnsupportedCrs> {
            let power = warp(r)?;
            let valid: Vec<bool> = power.data.iter().map(|v| v.is_finite()).collect();
            let db = power.map(|&p| 10.0 * (if p.is_finite() { p } else { 1.0 }).max(1e-6).log10());
            let valid_db: Vec<f32> = db.data.iter().zip(&valid).filter(|(_, v)| **v).map(|(d, _)| *d).collect();
            let fill = if valid_db.is_empty() { f32::NAN } else { median_f32(valid_db) };
            let filled = Grid::from_vec(w, h, bounds, db.data.iter().zip(&valid).map(|(&d, &v)| if v { d } else { fill }).collect());
            let med = median3(&filled);
            let db = Grid::from_vec(w, h, bounds, med.data.iter().zip(&valid).map(|(&d, &v)| if v { d } else { f32::NAN }).collect());
            Ok(Pass { idx, orbit: cfg.orbit(nisar_in[idx].date), valid, db })
        })
        .collect::<Result<_, _>>()?;

    // Change reference: the first non-empty pass of each orbit.
    let base_of = |orbit: i32| passes.iter().find(|p| p.orbit == orbit).map(|p| &p.db);

    struct Radar {
        water: Vec<bool>,
        water_km2: f64,
        hh_db: Vec<Option<f64>>,
        layers: SarLayers,
    }
    let radar: Vec<Radar> = passes
        .par_iter()
        .map(|p| {
            let base = base_of(p.orbit).unwrap_or(&p.db);
            let change = Grid::from_vec(w, h, bounds, p.db.data.iter().zip(&base.data).map(|(a, b)| a - b).collect());
            let valid = Grid::from_vec(w, h, bounds, p.valid.clone());
            let alpha = Grid::from_vec(
                w,
                h,
                bounds,
                edge_alpha(&valid).data.iter().zip(&zone_alpha.data).map(|(e, z)| e * z).collect(),
            );
            let water: Vec<bool> = p.db.data.iter().zip(&lake.data).map(|(&d, &l)| d <= cfg.sar_water_db && l).collect();
            let water_km2 = count(&water) as f64 * px_km2;
            let hh_db = zone_masks
                .iter()
                .map(|m| {
                    let v: Vec<f32> = p.db.data.iter().zip(&m.data).filter(|(d, m)| **m && d.is_finite()).map(|(d, _)| *d).collect();
                    // np.nanmean on float32: pairwise f32 sum, divided in f32.
                    (!v.is_empty()).then(|| f64::from(pairwise_sum(&v) / v.len() as f32))
                })
                .collect();
            let layers = SarLayers {
                hh_rgba: colorize(&p.db, &legends.hh, Some(&alpha)),
                change_rgba: colorize(&change, &legends.change, Some(&alpha)),
                alpha: alpha.map(|&a| a as f32),
                hh_db: p.db.clone(),
                change_db: change,
            };
            Radar { water, water_km2, hh_db, layers }
        })
        .collect();

    // Orbit filter: the first pass of the orbit must see enough of the lake.
    let good_orbit = |o: i32| {
        let first = passes.iter().position(|p| p.orbit == o).expect("orbit of an existing pass");
        radar[first].water_km2 >= cfg.orbit_min_lake * lake_km2
    };
    let mut dropped: Vec<Option<DropReason>> =
        passes.iter().map(|p| (!good_orbit(p.orbit)).then_some(DropReason::SteepOrbit)).collect();
    // Wind filter over the orbit-filtered list (neighbours taken from that list even if dropped here).
    let kept: Vec<usize> = (0..passes.len()).filter(|&i| dropped[i].is_none()).collect();
    for t in kept.windows(3) {
        let (prev, cur, next) = (radar[t[0]].water_km2, radar[t[1]].water_km2, radar[t[2]].water_km2);
        if cur < cfg.wind_dip * prev.min(next) {
            dropped[t[1]] = Some(DropReason::Wind);
        }
    }
    let pass_stats: Vec<PassStat> = passes
        .iter()
        .zip(&radar)
        .zip(&dropped)
        .map(|((p, r), d)| PassStat { date: nisar_in[p.idx].date, orbit: p.orbit, water_km2: r.water_km2, dropped: *d })
        .collect();
    let dropped_list: Vec<DroppedPass> = pass_stats
        .iter()
        .filter_map(|s| Some(DroppedPass { date: s.date, water_km2: s.water_km2, reason: s.dropped? }))
        .collect();

    // ── Timeline: every input pass in order, empty ones as "no data", dropped ones left out.
    let mut layers_of: Vec<Option<SarLayers>> = (0..nisar_in.len()).map(|_| None).collect();
    let mut stats = Vec::with_capacity(radar.len());
    for (p, r) in passes.iter().zip(radar) {
        layers_of[p.idx] = Some(r.layers);
        stats.push((r.water, r.water_km2, r.hh_db));
    }
    let sar_frames: Vec<SarFrame> = nisar_in
        .iter()
        .zip(layers_of)
        .filter(|(p, _)| !dropped_list.iter().any(|d| d.date == p.date))
        .map(|(p, layers)| SarFrame { date: p.date, source: format!("NISAR L2 GCOV HHHH · {}", p.name), layers })
        .collect();

    // ── Series: used optical dates, then used radar passes.
    struct Row<'a> {
        date: Date,
        sensor: Sensor,
        mask: &'a [bool],
        water_km2: f64,
        hh_db: Option<&'a [Option<f64>]>,
    }
    let mut rows: Vec<Row<'_>> = optical
        .iter()
        .zip(&optical_stats)
        .filter(|(_, s)| s.used)
        .map(|(o, _)| Row { date: o.date, sensor: Sensor::Sentinel2, mask: &o.water, water_km2: o.water_km2, hh_db: None })
        .collect();
    for ((p, (mask, km2, hh)), d) in passes.iter().zip(&stats).zip(&dropped) {
        if d.is_none() {
            rows.push(Row { date: nisar_in[p.idx].date, sensor: Sensor::Nisar, mask, water_km2: *km2, hh_db: Some(hh) });
        }
    }
    let dates: Vec<Date> = rows.iter().map(|r| r.date).collect();
    let mut series = Vec::new();
    let mut reservoir = None;
    for (zi, (z, zm)) in cfg.zones.iter().zip(&zone_masks).enumerate() {
        let water: Vec<f64> = rows
            .iter()
            .map(|r| {
                let km2 = if z.core {
                    r.water_km2
                } else {
                    r.mask.iter().zip(&lake.data).zip(&zm.data).filter(|((m, l), c)| **m && **l && **c).count() as f64 * px_km2
                };
                round_dp(km2, 2)
            })
            .collect();
        let hh_db = rows.iter().map(|r| r.hh_db.and_then(|h| h[zi]).map(|v| round_dp(v, 2))).collect();
        if z.core && !rows.is_empty() {
            let cap = water.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let last = rows.last().expect("non-empty rows");
            reservoir = Some(Reservoir {
                zone_id: z.id.clone(),
                capacity: round_dp(cap, 2),
                limit: round_dp(cfg.limit_share * cap, 2),
                dates: dates.clone(),
                area_km2: water.clone(),
                outline: outline(cfg, &Grid::from_vec(w, h, bounds, last.mask.to_vec())),
            });
        }
        series.push(ZoneSeries {
            id: z.id.clone(),
            dates: dates.clone(),
            water_km2: water,
            hh_db,
            water_source: rows.iter().map(|r| r.sensor).collect(),
        });
    }

    Ok(Build {
        sar_frames,
        dropped: dropped_list,
        optical_frames,
        optical_stats,
        pass_stats,
        lake,
        lake_km2,
        zone,
        zone_alpha: zone_alpha.map(|&v| v as f32),
        series,
        reservoir,
        hover_block: cfg.hover_block,
    })
}

/// The ISO date inside a file name (`…_2026-06-18…`).
fn date_in(name: &str) -> Option<Date> {
    (0..name.len().saturating_sub(9)).find_map(|i| name.get(i..i + 10).and_then(Date::parse))
}

fn read_raster(path: &Path) -> io::Result<Option<SourceRaster>> {
    let bytes = std::fs::read(path)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    geotiff::decode(&bytes).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))
}

/// Reads the legacy input folders: `<nisar_dir>/koroneia_nisar_<date>.tif` and
/// `<s2_dir>/koroneia_optical_<date>_B04.tif` (+ `_B08.tif`). Zero-byte files become `None`; a B04 without
/// its B08 gets `nir: None` (skipped by [`build`], as the script did).
///
/// # Errors
/// IO errors, and `InvalidData` for a file that is not a readable GeoTIFF.
pub fn read_inputs_from_dirs(nisar_dir: impl AsRef<Path>, s2_dir: impl AsRef<Path>) -> io::Result<Inputs> {
    let list = |dir: &Path, prefix: &str, suffix: &str| -> io::Result<Vec<(Date, String)>> {
        let mut v = Vec::new();
        for e in std::fs::read_dir(dir)? {
            let name = e?.file_name().to_string_lossy().into_owned();
            if name.starts_with(prefix)
                && name.ends_with(suffix)
                && let Some(d) = date_in(&name)
            {
                v.push((d, name));
            }
        }
        v.sort();
        Ok(v)
    };
    let (nd, sd) = (nisar_dir.as_ref(), s2_dir.as_ref());
    let nisar_files = list(nd, "koroneia_nisar_", ".tif")?;
    let optical_files = list(sd, "koroneia_optical_", "_B04.tif")?;
    let nisar = nisar_files
        .par_iter()
        .map(|(date, name)| Ok(NisarInput { date: *date, name: name.clone(), raster: read_raster(&nd.join(name))? }))
        .collect::<io::Result<Vec<_>>>()?;
    let optical = optical_files
        .par_iter()
        .map(|(date, name)| {
            let stem = name.trim_end_matches("_B04.tif").to_owned();
            let nir_path = sd.join(format!("{stem}_B08.tif"));
            let nir = if nir_path.exists() { read_raster(&nir_path)? } else { None };
            Ok(OpticalInput { date: *date, name: stem, red: read_raster(&sd.join(name))?, nir })
        })
        .collect::<io::Result<Vec<_>>>()?;
    Ok(Inputs { nisar, optical })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn koronia_constants() {
        let c = SiteConfig::koronia();
        assert_eq!((c.w, c.h), (1200, 600));
        assert!((c.km_lon - 84.3).abs() < 0.05);
        // 2026-06-18: ordinal 739785 → 739785 % 12 = 9.
        assert_eq!(c.orbit(Date::parse("2026-06-18").unwrap()), 739_785 % 12);
        assert_eq!(c.orbit(Date::parse("0001-01-01").unwrap()), 1);
        // Two passes 12 days apart share the orbit.
        let d = Date::parse("2026-07-07").unwrap();
        assert_eq!(c.orbit(d), c.orbit(d.add_days(12)));
    }

    #[test]
    fn numpy_like_helpers() {
        assert_eq!(round_dp(27.255_000_1, 2), 27.26);
        assert_eq!(round_dp(23.152_349_9, 5), 23.15235);
        assert_eq!(median_f32(vec![3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median_f32(vec![4.0, 1.0, 3.0, 2.0]), 2.5);
        let v: Vec<f64> = (0..1000).map(f64::from).collect();
        assert_eq!(pairwise_sum(&v), 499_500.0);
        assert_eq!(date_in("koroneia_optical_2026-02-16_B04.tif"), Date::parse("2026-02-16"));
        assert_eq!(date_in("x.tif"), None);
    }

    #[test]
    fn block_values_rounding_and_nodata() {
        let b = Bounds([0.0, 0.0, 5.0, 2.0]);
        // 5×2 with block 2 → 2×1 (the last column is ignored).
        let g = Grid::from_vec(5, 2, b, vec![1.0, 1.1, f32::NAN, f32::NAN, 9.0, 1.0, 1.0, f32::NAN, f32::NAN, 9.0]);
        let v = block_values(&g, None, 2);
        assert_eq!((v.w, v.h), (2, 1));
        assert_eq!(v.data, vec![10, i16::MIN]); // mean 1.025 → 10.25 → 10
        assert_eq!(v.bounds, Bounds([0.0, 0.0, 4.0, 2.0]));
        // Half to even: 0.25 × 10 = 2.5 → 2; a mask excludes pixels.
        let g = Grid::from_vec(2, 1, Bounds([0.0, 0.0, 2.0, 1.0]), vec![0.25, 100.0]);
        let m = Grid::from_vec(2, 1, g.bounds, vec![true, false]);
        assert_eq!(block_values(&g, Some(&m), 1).data, vec![2, i16::MIN]);
    }

    #[test]
    fn outline_of_a_disc() {
        let mut c = SiteConfig::koronia();
        c.w = 60;
        c.h = 60;
        c.bounds = Bounds([23.0, 40.0, 23.0 + 60.0 * c.res, 40.0 + 60.0 * c.res]);
        let mut m = Grid::filled(60, 60, c.bounds, false);
        for y in 0..60 {
            for x in 0..60 {
                let (dx, dy) = (x as f64 - 30.0, y as f64 - 30.0);
                *m.at_mut(x, y) = dx * dx + dy * dy <= 15.0 * 15.0;
            }
        }
        *m.at_mut(2, 2) = true; // a separate speck: not the largest component
        let ring = outline(&c, &m);
        assert_eq!(ring.len(), 73);
        assert_eq!(ring[0], ring[72]);
        assert!(ring.iter().all(|p| p[0] > c.lon(10) && p[1] < c.lat(10)));
        assert!(outline(&c, &Grid::filled(60, 60, c.bounds, false)).is_empty());
    }
}
