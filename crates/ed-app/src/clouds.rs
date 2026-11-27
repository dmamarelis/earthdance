//! Made-up clouds for the demo set, over the whole view.
//!
//! The field is fractal Perlin noise evaluated at map coordinates (Web Mercator), so it is anchored to the
//! ground, never repeats, and gains detail as you zoom in (finer octaves fade in as they become larger
//! than a few screen pixels). Each date gets its own field (`seed`), and the threshold is set so the share
//! of sky covered matches the date's cloud cover.
//!
//! The field is computed at a quarter of the screen resolution on all cores and redrawn only when the
//! view, date or cover changes; the GPU's linear filtering smooths it back up.

use eframe::egui::{self, Color32, Pos2, Rect};
use rayon::prelude::*;

use crate::map::{merc, Map};

/// Screen pixels per computed cell.
const CELL: f32 = 3.0;
/// Wavelength of the coarsest octave (weather systems, so a zoomed-out view has large-scale structure), in Mercator units (1 = the equator, 40 075 km).
const BASE: f64 = 3000.0 / 40_075.0;
const MAX_OCTAVES: usize = 15;
/// Octaves fade in between these wavelengths in screen pixels.
const FADE_PX: (f64, f64) = (6.0, 16.0);

/// One date's sky: its random seed, cloud cover in %, and the site (Mercator [x0, y0, x1, y1]) the cover
/// describes.
pub struct Sky {
    pub seed: u64,
    pub cover: f64,
    pub site: [f64; 4],
}

#[derive(Default)]
pub struct Clouds {
    tex: Option<egui::TextureHandle>,
    key: Option<(i64, i64, i64, u64, i64, u32, u32)>,
    /// (seed, cover in tenths of %, site) → noise threshold and the noise's spread.
    threshold: Option<(u64, i64, [i64; 4], f64, f64)>,
}

fn hash(x: i64, y: i64, o: u64, seed: u64) -> f64 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ o.wrapping_mul(0x1656_67B1_9E37_79F9) ^ seed;
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// 2-D Perlin (gradient) noise at lattice coordinates (x, y), roughly in [-0.7, 0.7]. Each lattice point gets a
/// random unit gradient; unlike value noise, features do not line up with the lattice.
fn perlin(x: f64, y: f64, o: u64, seed: u64) -> f64 {
    let (fx, fy) = (x.floor(), y.floor());
    let (ix, iy) = (fx as i64, fy as i64);
    let (dx, dy) = (x - fx, y - fy);
    let grad = |gx: i64, gy: i64, px: f64, py: f64| {
        let (s, c) = (hash(gx, gy, o, seed) * std::f64::consts::PI).sin_cos();
        c * px + s * py
    };
    let q = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v) = (q(dx), q(dy));
    let a = grad(ix, iy, dx, dy);
    let b = grad(ix + 1, iy, dx - 1.0, dy);
    let c = grad(ix, iy + 1, dx, dy - 1.0);
    let d = grad(ix + 1, iy + 1, dx - 1.0, dy - 1.0);
    a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
}

/// Fractal noise at Mercator (mx, my); `weights[k]` scales octave k (wavelength BASE / 2^k).
fn fbm(mx: f64, my: f64, seed: u64, weights: &[f64]) -> f64 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0f64);
    for (k, w) in weights.iter().enumerate() {
        let f = f64::from(1u32 << k) / BASE;
        // rotate every octave (the first too) so no lattice line runs along the screen axes
        let (s, c) = (0.45 + k as f64 * 0.93).sin_cos();
        let (x, y) = (mx * c - my * s, mx * s + my * c);
        // warp the coordinates by the coarser octaves' result: billowy shapes instead of grid-ish blobs
        let warp = 0.35 * sum / norm.max(1.0);
        sum += w * amp * perlin(x * f + warp, y * f - warp, k as u64, seed);
        norm += amp;
        amp *= 0.58;
    }
    sum / norm
}

fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Clouds {
    /// Noise level above which the sky is cloudy, set so that `cover` % of the site (Mercator
    /// [x0, y0, x1, y1]) is covered: the date's cloud cover describes the site, the rest of the sky varies
    /// around it. Also returns the noise's spread over a wide area (for edge widths).
    fn threshold(&mut self, seed: u64, cover: f64, site: [f64; 4]) -> (f64, f64) {
        let ck = (cover * 10.0).round() as i64;
        let sk = site.map(|v| (v * 1e7).round() as i64);
        if let Some((s, c, k, t, sd)) = self.threshold
            && s == seed
            && c == ck
            && k == sk
        {
            return (t, sd);
        }
        let w = [1.0; MAX_OCTAVES];
        let n = 96;
        let mut v: Vec<f64> = (0..n * n)
            .into_par_iter()
            .map(|i| fbm(site[0] + (site[2] - site[0]) * ((i % n) as f64 + 0.5) / n as f64, site[1] + (site[3] - site[1]) * ((i / n) as f64 + 0.5) / n as f64, seed, &w))
            .collect();
        v.sort_by(f64::total_cmp);
        let i = (((1.0 - cover / 100.0) * v.len() as f64) as usize).min(v.len() - 1);
        let t = v[i];
        let wide: Vec<f64> = (0..64 * 64).map(|i| fbm(site[0] + (i % 64) as f64 * BASE * 0.11, site[1] + (i / 64) as f64 * BASE * 0.11, seed, &w)).collect();
        let mean = wide.iter().sum::<f64>() / wide.len() as f64;
        let sd = (wide.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / wide.len() as f64).sqrt();
        self.threshold = Some((seed, ck, sk, t, sd));
        (t, sd)
    }

    pub fn draw(&mut self, ctx: &egui::Context, p: &egui::Painter, map: &Map, rect: Rect, sky: Sky) {
        let Sky { seed, cover, site } = sky;
        if cover <= 0.5 {
            return;
        }
        let world = 512.0 * 2f64.powf(map.view.zoom);
        let cell = f64::from(CELL) / world;
        let (cx, cy) = merc(map.view.lon, map.view.lat);
        // the grid is snapped to whole cells of the ground, so panning only moves it (no shimmer)
        let (gw, gh) = ((rect.width() / CELL).ceil() as u32 + 2, (rect.height() / CELL).ceil() as u32 + 2);
        let ox = ((cx - f64::from(rect.width()) / 2.0 / world) / cell).floor() as i64;
        let oy = ((cy - f64::from(rect.height()) / 2.0 / world) / cell).floor() as i64;
        let key = (ox, oy, (map.view.zoom * 1e6).round() as i64, seed, (cover * 10.0).round() as i64, gw, gh);
        if self.key != Some(key) || self.tex.is_none() {
            let (t, sd) = self.threshold(seed, cover, site);
            let weights: Vec<f64> = (0..MAX_OCTAVES)
                .map(|k| if k == 0 { 1.0 } else { smoothstep(FADE_PX.0, FADE_PX.1, BASE / f64::from(1u32 << k) * world) })
                .take_while(|w| *w > 0.0)
                .collect();
            // the noise on the grid plus a margin up-left, where the shadows come from
            // (clouds ~2 km up, sun from the north-west)
            let sh = ((2.0 / 40_075.0 * world / f64::from(CELL)).round() as i64).clamp(1, 12);
            let m = sh as u32;
            let (fw, fh) = (gw + m, gh + m);
            let field: Vec<f64> = (0..fh)
                .into_par_iter()
                .flat_map_iter(|j| {
                    let weights = &weights;
                    (0..fw).map(move |i| fbm((ox as f64 - f64::from(m) + f64::from(i) + 0.5) * cell, (oy as f64 - f64::from(m) + f64::from(j) + 0.5) * cell, seed, weights))
                })
                .collect();
            // edges are sized by how much the field varies within this view, so clouds keep defined shapes
            // at every zoom (fine octaves are weak, so close up the field is nearly flat around the threshold)
            // (computed from the zoom, not from the pixels, so the edges do not flicker while panning)
            let view = f64::from(rect.width().max(rect.height())) / world;
            let amp = |k: usize| 0.58f64.powi(k as i32);
            let all: f64 = (0..MAX_OCTAVES).map(|k| amp(k).powi(2)).sum();
            let seen: f64 = weights.iter().enumerate().map(|(k, w)| (amp(k) * w * (1.0 - smoothstep(view, 4.0 * view, BASE / f64::from(1u32 << k)))).powi(2)).sum();
            let sd = (sd * (seen / all).sqrt()).max(sd * 0.05);
            // crisp-ish edges; thin cloud lets some ground through, so even an overcast date keeps texture
            let alpha = |n: f64| smoothstep(t - 0.04 * sd, t + 0.18 * sd, n) * (0.6 + 0.38 * smoothstep(t + 0.15 * sd, t + 1.4 * sd, n));
            let px: Vec<u8> = (0..gh)
                .into_par_iter()
                .flat_map_iter(|j| {
                    let (field, alpha) = (&field, &alpha);
                    (0..gw).flat_map(move |i| {
                        let n = field[((j + m) * fw + i + m) as usize];
                        let ac = alpha(n);
                        let as_ = 0.32 * alpha(field[(j * fw + i) as usize]);
                        // thick tops are bright white, thin edges greyer (seen from above)
                        let g = 212.0 + 43.0 * smoothstep(t, t + 1.3 * sd, n);
                        let a = ac + as_ * (1.0 - ac);
                        let c = if a > 0.0 { g * ac / a } else { 0.0 };
                        [c as u8, c as u8, (c * 1.015).min(255.0) as u8, (a * 255.0) as u8]
                    })
                })
                .collect();
            let img = egui::ColorImage::from_rgba_unmultiplied([gw as usize, gh as usize], &px);
            match &mut self.tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.tex = Some(ctx.load_texture("clouds", img, egui::TextureOptions::LINEAR)),
            }
            self.key = Some(key);
        }
        if let Some(t) = &self.tex {
            let left = rect.center().x + ((ox as f64 * cell - cx) * world) as f32;
            let top = rect.center().y + ((oy as f64 * cell - cy) * world) as f32;
            let r = Rect::from_min_size(Pos2::new(left, top), egui::vec2(gw as f32 * CELL, gh as f32 * CELL));
            p.image(t.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
    }
}
