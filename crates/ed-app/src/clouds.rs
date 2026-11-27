//! Made-up clouds for the demo set, over the whole view (Show ▾ → Clouds turns them off).
//!
//! The field is fractal Perlin noise at map coordinates (Web Mercator): anchored to the ground, never
//! repeating, with finer octaves fading in as you zoom. Each date gets its own sky (`seed`), and the
//! threshold is set so the site is as cloudy as the date's cloud cover says.
//!
//! Cost model (the UI thread never computes noise):
//! * A field is computed **in the background** for an area twice the view in each direction, at a zoom
//!   rounded to half levels. Panning and zooming then only move and scale that texture; a new field is
//!   asked for when the view leaves it or the zoom moves half a level, and the old one stays on screen
//!   until the new one is ready.
//! * Octaves much larger than the area are smooth across it, so they are computed on a coarse grid and
//!   interpolated; only the fine ones are evaluated per cell. Gradients come from a table (no trig).

use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, Color32, Pos2, Rect};
use rayon::prelude::*;

use crate::map::{merc, Map};

/// Screen pixels per computed cell (at the field's own zoom).
const CELL: f64 = 4.0;
/// Wavelength of the coarsest octave (weather systems), in Mercator units (1 = the equator, 40 075 km).
const BASE: f64 = 3000.0 / 40_075.0;
const MAX_OCTAVES: usize = 15;
const PERSIST: f64 = 0.58;
/// Octaves fade in between these wavelengths in screen pixels.
const FADE_PX: (f64, f64) = (8.0, 20.0);
/// Octaves longer than this many cells are computed on the coarse grid.
const COARSE_CELLS: f64 = 96.0;
/// Coarse grid step, in cells.
const STEP: usize = 12;

/// One date's sky: its random seed, cloud cover in %, and the site (Mercator [x0, y0, x1, y1]) the cover
/// describes.
#[derive(Clone, Copy, PartialEq)]
pub struct Sky {
    pub seed: u64,
    pub cover: f64,
    pub site: [f64; 4],
}

/// What a computed field is for: the sky, the half-level zoom, and the Mercator area it covers.
#[derive(Clone, Copy, PartialEq)]
struct Job {
    sky: Sky,
    zq: f64,
    area: [f64; 4],
    /// Size of the view it was made for, in screen px (sets which octaves are "visible").
    view: [f64; 2],
}

#[derive(Default)]
pub struct Clouds {
    shown: Option<(Job, egui::TextureHandle)>,
    pending: Option<(Job, Receiver<egui::ColorImage>)>,
}

fn hash(x: i64, y: i64, o: u64, seed: u64) -> u64 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ o.wrapping_mul(0x1656_67B1_9E37_79F9) ^ seed;
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h
}

/// 64 unit gradients, so a lattice corner costs a hash and a table read.
static GRAD: std::sync::LazyLock<[(f64, f64); 64]> = std::sync::LazyLock::new(|| std::array::from_fn(|i| (i as f64 / 64.0 * std::f64::consts::TAU).sin_cos()));

/// 2-D Perlin (gradient) noise at lattice coordinates (x, y), roughly in [-0.7, 0.7].
fn perlin(x: f64, y: f64, o: u64, seed: u64) -> f64 {
    let (fx, fy) = (x.floor(), y.floor());
    let (ix, iy) = (fx as i64, fy as i64);
    let (dx, dy) = (x - fx, y - fy);
    let g = |gx: i64, gy: i64, px: f64, py: f64| {
        let (s, c) = GRAD[(hash(gx, gy, o, seed) >> 58) as usize];
        c * px + s * py
    };
    let q = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v) = (q(dx), q(dy));
    let a = g(ix, iy, dx, dy);
    let b = g(ix + 1, iy, dx - 1.0, dy);
    let c = g(ix, iy + 1, dx, dy - 1.0);
    let d = g(ix + 1, iy + 1, dx - 1.0, dy - 1.0);
    a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
}

/// Octave indices with their weights.
type Octaves = Vec<(usize, f64)>;

fn amp(k: usize) -> f64 {
    PERSIST.powi(k as i32)
}

fn norm() -> f64 {
    (0..MAX_OCTAVES).map(amp).sum()
}

/// The octaves `ks` (with weights) of the field at Mercator (mx, my), not normalised.
fn octaves(mx: f64, my: f64, seed: u64, ks: &[(usize, f64)]) -> f64 {
    ks.iter()
        .map(|&(k, w)| {
            let f = f64::from(1u32 << k) / BASE;
            // every octave rotated so no lattice line runs along the screen axes
            let (s, c) = (0.45 + k as f64 * 0.93).sin_cos();
            w * amp(k) * perlin((mx * c - my * s) * f, (mx * s + my * c) * f, k as u64, seed)
        })
        .sum()
}

fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Computes the cloud image for a job (runs on a worker thread).
fn compute(job: Job) -> egui::ColorImage {
    let Sky { seed, cover, site } = job.sky;
    let all: Vec<(usize, f64)> = (0..MAX_OCTAVES).map(|k| (k, 1.0)).collect();
    let n = norm();
    // threshold: `cover` % of the site above it
    let s = 64;
    let mut v: Vec<f64> = (0..s * s).map(|i| octaves(site[0] + (site[2] - site[0]) * ((i % s) as f64 + 0.5) / s as f64, site[1] + (site[3] - site[1]) * ((i / s) as f64 + 0.5) / s as f64, seed, &all) / n).collect();
    v.sort_by(f64::total_cmp);
    let t = v[(((1.0 - cover / 100.0) * v.len() as f64) as usize).min(v.len() - 1)];
    // spread of the field over a wide area, then the part of it visible at this zoom (sets edge widths)
    let wide: Vec<f64> = (0..32 * 32).map(|i| octaves(site[0] + (i % 32) as f64 * BASE * 0.21, site[1] + (i / 32) as f64 * BASE * 0.21, seed, &all) / n).collect();
    let mean = wide.iter().sum::<f64>() / wide.len() as f64;
    let sd_wide = (wide.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / wide.len() as f64).sqrt();

    let world = 512.0 * 2f64.powf(job.zq);
    let cell = CELL / world;
    let weights: Vec<(usize, f64)> = (0..MAX_OCTAVES)
        .map(|k| (k, if k == 0 { 1.0 } else { smoothstep(FADE_PX.0, FADE_PX.1, BASE / f64::from(1u32 << k) * world) }))
        .filter(|(_, w)| *w > 0.0)
        .collect();
    let view = job.view[0].max(job.view[1]) / world;
    let seen: f64 = weights.iter().map(|&(k, w)| (amp(k) * w * (1.0 - smoothstep(view, 4.0 * view, BASE / f64::from(1u32 << k)))).powi(2)).sum();
    let total: f64 = (0..MAX_OCTAVES).map(|k| amp(k).powi(2)).sum();
    let sd = (sd_wide * (seen / total).sqrt()).max(sd_wide * 0.05);

    // grid: the area plus a margin up-left for the shadows (clouds ~2 km up, sun from the north-west)
    let [x0, y0, x1, y1] = job.area;
    let (gw, gh) = (((x1 - x0) / cell).ceil() as usize, ((y1 - y0) / cell).ceil() as usize);
    let m = ((2.0 / 40_075.0 / cell).round() as usize).clamp(1, 12);
    let (fw, fh) = (gw + m, gh + m);
    let at = |i: usize, j: usize| (x0 + (i as f64 - m as f64 + 0.5) * cell, y0 + (j as f64 - m as f64 + 0.5) * cell);
    let (coarse, fine): (Octaves, Octaves) = weights.iter().partition(|&&(k, _)| BASE / f64::from(1u32 << k) > COARSE_CELLS * cell);
    // coarse octaves on a grid every STEP cells, interpolated
    let (cw, ch) = (fw / STEP + 2, fh / STEP + 2);
    let cgrid: Vec<f64> = (0..cw * ch)
        .into_par_iter()
        .map(|i| {
            let (x, y) = at((i % cw) * STEP, (i / cw) * STEP);
            octaves(x, y, seed, &coarse)
        })
        .collect();
    let field: Vec<f64> = (0..fh)
        .into_par_iter()
        .flat_map_iter(|j| {
            let (cgrid, fine) = (&cgrid, &fine);
            (0..fw).map(move |i| {
                let (ci, cj) = (i / STEP, j / STEP);
                let (u, v) = ((i % STEP) as f64 / STEP as f64, (j % STEP) as f64 / STEP as f64);
                let c = |a: usize, b: usize| cgrid[b * cw + a];
                let base = c(ci, cj) * (1.0 - u) * (1.0 - v) + c(ci + 1, cj) * u * (1.0 - v) + c(ci, cj + 1) * (1.0 - u) * v + c(ci + 1, cj + 1) * u * v;
                let (x, y) = at(i, j);
                (base + octaves(x, y, seed, fine)) / n
            })
        })
        .collect();
    // crisp-ish edges; thin cloud lets some ground through; bright thick tops; a soft shadow
    let alpha = |v: f64| smoothstep(t - 0.04 * sd, t + 0.18 * sd, v) * (0.6 + 0.38 * smoothstep(t + 0.15 * sd, t + 1.4 * sd, v));
    let px: Vec<u8> = (0..gh)
        .into_par_iter()
        .flat_map_iter(|j| {
            let field = &field;
            (0..gw).flat_map(move |i| {
                let v = field[(j + m) * fw + i + m];
                let ac = alpha(v);
                let as_ = 0.32 * alpha(field[j * fw + i]);
                let g = 212.0 + 43.0 * smoothstep(t, t + 1.3 * sd, v);
                let a = ac + as_ * (1.0 - ac);
                let c = if a > 0.0 { g * ac / a } else { 0.0 };
                [c as u8, c as u8, (c * 1.015).min(255.0) as u8, (a * 255.0) as u8]
            })
        })
        .collect();
    egui::ColorImage::from_rgba_unmultiplied([gw, gh], &px)
}

impl Clouds {
    pub fn draw(&mut self, ctx: &egui::Context, p: &egui::Painter, map: &Map, rect: Rect, sky: Sky) {
        if sky.cover <= 0.5 {
            return;
        }
        let world = 512.0 * 2f64.powf(map.view.zoom);
        let (cx, cy) = merc(map.view.lon, map.view.lat);
        let (hw, hh) = (f64::from(rect.width()) / 2.0 / world, f64::from(rect.height()) / 2.0 / world);
        let view = [cx - hw, cy - hh, cx + hw, cy + hh];
        let zq = (map.view.zoom * 2.0).round() / 2.0;
        let fits = |j: &Job| j.sky == sky && j.zq == zq && j.area[0] <= view[0] && j.area[1] <= view[1] && j.area[2] >= view[2] && j.area[3] >= view[3];

        // a finished field replaces the shown one
        if let Some((job, rx)) = &self.pending
            && let Ok(img) = rx.try_recv()
        {
            let job = *job;
            match &mut self.shown {
                Some((j, t)) => {
                    t.set(img, egui::TextureOptions::LINEAR);
                    *j = job;
                }
                None => self.shown = Some((job, ctx.load_texture("clouds", img, egui::TextureOptions::LINEAR))),
            }
            self.pending = None;
        }
        // ask for a new field when the shown one no longer fits (one job at a time: a stale one finishes,
        // is shown, and the next frame asks again)
        let shown_fits = self.shown.as_ref().is_some_and(|(j, _)| fits(j));
        if !shown_fits && self.pending.is_none() {
            let job = Job { sky, zq, area: [cx - 2.0 * hw, cy - 2.0 * hh, cx + 2.0 * hw, cy + 2.0 * hh], view: [f64::from(rect.width()), f64::from(rect.height())] };
            let (tx, rx) = channel();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(compute(job));
                ctx2.request_repaint();
            });
            self.pending = Some((job, rx));
        }
        // draw what we have, mapped to where it belongs on the ground (it moves and scales with the map);
        // a field for another date is not shown
        if let Some((j, t)) = &self.shown
            && j.sky == sky
        {
            let to = |x: f64, y: f64| Pos2::new(rect.center().x + ((x - cx) * world) as f32, rect.center().y + ((y - cy) * world) as f32);
            let r = Rect::from_min_max(to(j.area[0], j.area[1]), to(j.area[2], j.area[3]));
            p.image(t.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
    }
}
