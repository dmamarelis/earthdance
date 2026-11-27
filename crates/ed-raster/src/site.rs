//! Water from optical scenes for any site on Earth: no per-site constants.
//!
//! The Koronia pipeline (`koroneia.rs`) reproduces the legacy script and its hand-tuned numbers
//! (−18 dB, NDWI > 0.1, a drawn circle). Anywhere else those numbers are wrong, so here they come from the
//! data:
//!
//! * clouds, shadows and no-data are removed with the Sentinel-2 scene classification (SCL);
//! * the water threshold on NDWI = (green − NIR)/(green + NIR) is found by **Otsu's method** (the split that
//!   maximises the between-class variance of the histogram), pooled over the clear scenes;
//! * the lake is the **largest connected water body** of the maximum extent, its zone that body widened;
//! * a scene counts towards the area series only if the zone is ≥ 90 % cloud-free.

use ed_core::{Bounds, Grid, Rgba};

use crate::filters::{dilate_cross, label4};

/// The target grid: a lat/lon raster around a place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SiteGrid {
    pub bounds: Bounds,
    pub w: usize,
    pub h: usize,
}

impl SiteGrid {
    /// A grid of about `half_km` around (lon, lat), `max_px` pixels across at most, never finer than 10 m.
    pub fn around(lon: f64, lat: f64, half_km: f64, max_px: usize) -> Self {
        let dlat = half_km / 110.57;
        let dlon = half_km / (111.32 * lat.to_radians().cos().max(0.05));
        let res_km = (2.0 * half_km / max_px as f64).max(0.010);
        let w = ((2.0 * half_km) / res_km).round().max(16.0) as usize;
        let h = ((2.0 * half_km) / res_km).round().max(16.0) as usize;
        Self { bounds: Bounds([lon - dlon, lat - dlat, lon + dlon, lat + dlat]), w, h }
    }

    pub fn from_bounds(bounds: Bounds, max_px: usize) -> Self {
        let lat = (bounds.south() + bounds.north()) / 2.0;
        let wk = (bounds.east() - bounds.west()) * 111.32 * lat.to_radians().cos();
        let hk = (bounds.north() - bounds.south()) * 110.57;
        let res_km = (wk.max(hk) / max_px as f64).max(0.010);
        Self { bounds, w: (wk / res_km).round().max(16.0) as usize, h: (hk / res_km).round().max(16.0) as usize }
    }

    /// Area of one pixel (km²) at the grid's centre latitude.
    pub fn px_km2(&self) -> f64 {
        let lat = (self.bounds.south() + self.bounds.north()) / 2.0;
        let dx = (self.bounds.east() - self.bounds.west()) / self.w as f64 * 111.32 * lat.to_radians().cos();
        let dy = (self.bounds.north() - self.bounds.south()) / self.h as f64 * 110.57;
        dx * dy
    }

    /// Pixel size in metres (for choosing a COG overview).
    pub fn res_m(&self) -> f64 {
        self.px_km2().sqrt() * 1000.0
    }
}

/// One optical acquisition on the site grid (reflectance × 10 000; SCL class numbers).
#[derive(Clone, Debug)]
pub struct OpticalScene {
    pub date: ed_core::Date,
    pub source: String,
    pub green: Grid<f32>,
    pub red: Grid<f32>,
    pub nir: Grid<f32>,
    pub scl: Grid<f32>,
}

/// SCL classes that are not a clear view of the ground: no data, saturated, cloud shadow, clouds, cirrus.
pub fn scl_clear(c: f32) -> bool {
    c.is_finite() && !matches!(c as i32, 0 | 1 | 3 | 8..=10)
}
pub fn scl_cloud(c: f32) -> bool {
    matches!(c as i32, 8..=10)
}

pub fn ndwi(green: f32, nir: f32) -> f32 {
    let s = green + nir;
    if s > 0.0 { (green - nir) / s } else { f32::NAN }
}

/// Otsu's threshold of `values` within `[lo, hi]` (256 bins).
pub fn otsu(values: impl Iterator<Item = f32>, lo: f32, hi: f32) -> Option<f32> {
    const B: usize = 256;
    let mut hist = [0u64; B];
    let mut n = 0u64;
    for v in values {
        if v.is_finite() && v >= lo && v <= hi {
            let i = (((v - lo) / (hi - lo)) * (B - 1) as f32).round() as usize;
            hist[i.min(B - 1)] += 1;
            n += 1;
        }
    }
    if n < 100 {
        return None;
    }
    let total: f64 = hist.iter().enumerate().map(|(i, c)| i as f64 * *c as f64).sum();
    let (mut w0, mut sum0, mut best, mut arg) = (0.0, 0.0, -1.0, 0);
    for (i, c) in hist.iter().enumerate() {
        w0 += *c as f64;
        sum0 += i as f64 * *c as f64;
        let w1 = n as f64 - w0;
        if w0 == 0.0 || w1 == 0.0 {
            continue;
        }
        let (m0, m1) = (sum0 / w0, (total - sum0) / w1);
        let between = w0 * w1 * (m0 - m1).powi(2);
        if between > best {
            best = between;
            arg = i;
        }
    }
    Some(lo + (hi - lo) * (arg as f32 + 0.5) / (B - 1) as f32)
}

/// What one scene says about the site.
#[derive(Clone, Debug)]
pub struct SceneResult {
    pub date: ed_core::Date,
    pub source: String,
    /// Share of the whole grid seen clearly.
    pub clear: f64,
    /// Share of the lake zone seen clearly.
    pub zone_clear: f64,
    /// Water inside the lake zone (km²), when the zone was ≥ 90 % clear.
    pub water_km2: Option<f64>,
    pub ndwi: Grid<f32>,
    /// The picture of the "Optical" view: real colour approximation, clouds as clouds.
    pub image: Rgba,
    pub water: Grid<bool>,
}

#[derive(Clone, Debug)]
pub struct SiteAnalysis {
    pub threshold: f32,
    pub lake: Grid<bool>,
    pub zone: Grid<bool>,
    pub lake_km2: f64,
    /// Centroid of the lake [lon, lat] and the radius of a circle of the same area (km).
    pub center: [f64; 2],
    pub radius_km: f64,
    pub scenes: Vec<SceneResult>,
    /// Lake edge on the last usable scene (closed ring, 72 rays).
    pub outline: Vec<[f64; 2]>,
}

/// The "Optical" picture of a date, from green + NIR only (no red/blue fetched: fewer bytes). Colours are
/// tone-mapped to sit naturally on a satellite basemap: soil tan, vegetation olive, water dark blue-grey
/// tinted by its turbidity, clouds white.
fn image(s: &OpticalScene) -> Rgba {
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    let px = (0..s.green.data.len())
        .map(|i| {
            let (g, n, c) = (s.green.data[i], s.nir.data[i], s.scl.data[i]);
            if !(g.is_finite() && n.is_finite()) || c as i32 == 0 {
                return [0, 0, 0, 0];
            }
            if scl_cloud(c) {
                return [232, 236, 240, 235];
            }
            let v = (n - g) / (n + g).max(1.0); // > 0 land (more so with vegetation), < 0 water
            let bright = (g / 1400.0).clamp(0.0, 1.4).powf(0.9);
            let land = lerp([138.0, 118.0, 92.0], [72.0, 94.0, 54.0], (v * 1.6).clamp(0.0, 1.0));
            let land = land.map(|x| x * (0.55 + 0.6 * bright));
            let water = lerp([22.0, 36.0, 50.0], [70.0, 88.0, 82.0], (g / 1300.0).clamp(0.0, 1.0));
            let t = ((v + 0.06) / 0.12).clamp(0.0, 1.0); // soft shoreline
            let c = lerp(water, land, t);
            [c[0].min(255.0) as u8, c[1].min(255.0) as u8, c[2].min(255.0) as u8, 255]
        })
        .collect();
    Rgba { w: s.green.w, h: s.green.h, bounds: s.green.bounds, px }
}

fn largest(mask: &Grid<bool>) -> Grid<bool> {
    let (labels, n) = label4(mask);
    if n == 0 {
        return mask.map(|_| false);
    }
    let mut count = vec![0usize; n as usize + 1];
    for l in &labels {
        count[*l as usize] += 1;
    }
    let big = (1..=n as usize).max_by_key(|&i| count[i]).unwrap_or(1) as u32;
    Grid::from_vec(mask.w, mask.h, mask.bounds, labels.iter().map(|l| *l == big).collect())
}

fn outline(g: &Grid<bool>) -> Vec<[f64; 2]> {
    let pts: Vec<(usize, usize)> = (0..g.h).flat_map(|y| (0..g.w).map(move |x| (x, y))).filter(|&(x, y)| *g.at(x, y)).collect();
    if pts.is_empty() {
        return Vec::new();
    }
    let lat0 = g.lat(g.h / 2);
    let (kx, ky) = (111.32 * lat0.to_radians().cos(), 110.57);
    let cx = pts.iter().map(|p| g.lon(p.0)).sum::<f64>() / pts.len() as f64;
    let cy = pts.iter().map(|p| g.lat(p.1)).sum::<f64>() / pts.len() as f64;
    let mut best = vec![None::<(f64, [f64; 2])>; 72];
    for &(x, y) in &pts {
        let (dx, dy) = ((g.lon(x) - cx) * kx, (g.lat(y) - cy) * ky);
        let a = dy.atan2(dx);
        let k = (((a + std::f64::consts::PI) / (2.0 * std::f64::consts::PI) * 72.0).floor() as usize).min(71);
        let d = dx.hypot(dy);
        if best[k].is_none_or(|b| d > b.0) {
            best[k] = Some((d, [g.lon(x), g.lat(y)]));
        }
    }
    let mut ring: Vec<[f64; 2]> = best.into_iter().flatten().map(|b| b.1).collect();
    if let Some(f) = ring.first().copied() {
        ring.push(f);
    }
    ring
}

/// Water, lake and area series from the scenes (any order; the result is sorted by date).
pub fn analyse(grid: &SiteGrid, scenes: &[OpticalScene]) -> Option<SiteAnalysis> {
    let px_km2 = grid.px_km2();
    let n = grid.w * grid.h;
    let clear_of = |s: &OpticalScene| -> Vec<bool> { (0..n).map(|i| scl_clear(s.scl.data[i]) && s.green.data[i].is_finite() && s.nir.data[i].is_finite()).collect() };
    let clears: Vec<Vec<bool>> = scenes.iter().map(clear_of).collect();
    let share = |c: &[bool]| c.iter().filter(|v| **v).count() as f64 / n as f64;

    // one threshold for the site, from the clearest scenes pooled
    let mut order: Vec<usize> = (0..scenes.len()).collect();
    order.sort_by(|a, b| share(&clears[*b]).total_cmp(&share(&clears[*a])));
    let pool: Vec<usize> = order.iter().copied().filter(|&i| share(&clears[i]) > 0.6).take(8).collect();
    let cl = &clears;
    let threshold = otsu(
        pool.iter().flat_map(|&i| (0..n).filter(move |&k| cl[i][k]).map(move |k| ndwi(scenes[i].green.data[k], scenes[i].nir.data[k]))),
        -0.8,
        0.8,
    )
    .unwrap_or(0.0)
    .clamp(-0.3, 0.4);

    let ndwis: Vec<Grid<f32>> = scenes.iter().map(|s| Grid::from_vec(grid.w, grid.h, grid.bounds, (0..n).map(|i| ndwi(s.green.data[i], s.nir.data[i])).collect())).collect();
    let waters: Vec<Grid<bool>> = ndwis.iter().zip(&clears).map(|(g, c)| Grid::from_vec(grid.w, grid.h, grid.bounds, (0..n).map(|i| c[i] && g.data[i] > threshold).collect())).collect();

    // maximum extent over the clear scenes → the largest connected water body = the lake
    let mut max_ext = Grid::filled(grid.w, grid.h, grid.bounds, false);
    for (i, w) in waters.iter().enumerate() {
        if share(&clears[i]) > 0.6 {
            for (m, v) in max_ext.data.iter_mut().zip(&w.data) {
                *m |= *v;
            }
        }
    }
    let lake = largest(&max_ext);
    let lake_px = lake.data.iter().filter(|v| **v).count();
    if lake_px < 20 {
        return None;
    }
    let lake_km2 = lake_px as f64 * px_km2;
    // the zone: the lake widened by ~400 m, where its shoreline can move
    let margin = ((0.4 / px_km2.sqrt()).round() as usize).clamp(2, 40);
    let zone = dilate_cross(&lake, margin);
    let zone_idx: Vec<usize> = (0..n).filter(|&i| zone.data[i]).collect();
    let pts: Vec<(f64, f64)> = (0..n).filter(|&i| lake.data[i]).map(|i| (lake.lon(i % grid.w), lake.lat(i / grid.w))).collect();
    let center = [pts.iter().map(|p| p.0).sum::<f64>() / pts.len() as f64, pts.iter().map(|p| p.1).sum::<f64>() / pts.len() as f64];

    let mut results: Vec<SceneResult> = scenes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let zc = zone_idx.iter().filter(|&&k| clears[i][k]).count() as f64 / zone_idx.len().max(1) as f64;
            let water_km2 = (zc >= 0.9).then(|| zone_idx.iter().filter(|&&k| waters[i].data[k]).count() as f64 * px_km2);
            SceneResult { date: s.date, source: s.source.clone(), clear: share(&clears[i]), zone_clear: zc, water_km2, ndwi: ndwis[i].clone(), image: image(s), water: waters[i].clone() }
        })
        .collect();
    results.sort_by_key(|r| r.date);
    let last = results.iter().rev().find(|r| r.water_km2.is_some());
    let outline = last.map_or_else(Vec::new, |r| {
        let mut wz = r.water.clone();
        for (v, z) in wz.data.iter_mut().zip(&zone.data) {
            *v &= *z;
        }
        outline(&largest(&wz))
    });
    Some(SiteAnalysis { threshold, lake, zone, lake_km2, center, radius_km: (lake_km2 / std::f64::consts::PI).sqrt(), scenes: results, outline })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otsu_splits_a_bimodal_histogram() {
        // land around −0.4, water around +0.3
        let v = (0..2000).map(|i| if i % 3 == 0 { 0.3 + (i % 7) as f32 * 0.01 } else { -0.4 + (i % 11) as f32 * 0.01 });
        let t = otsu(v, -1.0, 1.0).unwrap();
        assert!(t > -0.3 && t < 0.3, "{t}");
    }

    #[test]
    fn finds_a_synthetic_lake_through_a_cloud() {
        let g = SiteGrid::around(23.15, 40.68, 5.0, 100);
        let n = g.w * g.h;
        let (cx, cy) = (g.w / 2, g.h / 2);
        let mk = |date: &str, r: f64, cloud: bool| {
            let mut green = vec![800.0f32; n];
            let mut nir = vec![2500.0f32; n];
            let mut scl = vec![4.0f32; n];
            for y in 0..g.h {
                for x in 0..g.w {
                    let d = ((x as f64 - cx as f64).powi(2) + (y as f64 - cy as f64).powi(2)).sqrt();
                    if d < r {
                        green[y * g.w + x] = 900.0;
                        nir[y * g.w + x] = 300.0;
                    }
                    if cloud && x < g.w / 2 {
                        scl[y * g.w + x] = 9.0;
                    }
                }
            }
            let gr = |v: Vec<f32>| Grid::from_vec(g.w, g.h, g.bounds, v);
            OpticalScene { date: ed_core::Date::parse(date).unwrap(), source: date.into(), green: gr(green), red: gr(vec![700.0; n]), nir: gr(nir), scl: gr(scl) }
        };
        let a = analyse(&g, &[mk("2026-01-01", 30.0, false), mk("2026-06-01", 20.0, false), mk("2026-07-01", 15.0, true)]).unwrap();
        assert_eq!(a.scenes.len(), 3);
        let (w0, w1) = (a.scenes[0].water_km2.unwrap(), a.scenes[1].water_km2.unwrap());
        assert!(w0 > w1 * 2.0, "{w0} vs {w1}");
        assert!(a.scenes[2].water_km2.is_none(), "half the lake under cloud must not count");
        assert!((a.center[0] - 23.15).abs() < 0.01 && (a.center[1] - 40.68).abs() < 0.01);
        assert!(a.outline.len() > 60);
    }
}
