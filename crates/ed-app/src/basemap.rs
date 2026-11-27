//! The satellite basemap under everything (and place names on top), Google-Earth style.
//!
//! * Tiles stream in on background workers; what is on screen is fetched first, the zoom levels just above
//!   and below are prefetched, and requests for views already left are dropped (see `ed_sources::tiles`).
//! * While a tile loads, the best stand-in is drawn: its sharper children when zooming out, a scaled-up
//!   ancestor when zooming in. The view sharpens instead of showing holes.
//! * At most a few texture uploads happen per frame, so panning and zooming never stutter.
//! * [`Basemaps::warm`] starts all this while the app is still loading its data.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use ed_sources::tiles::{Basemap, TileKey, Tiles};

use crate::map::{merc, Map};

/// New tile textures per frame at most (each upload is a GPU copy on the UI thread).
const UPLOADS_PER_FRAME: usize = 8;
/// Tile textures kept on the GPU.
const MAX_TEXTURES: usize = 900;

pub struct Basemaps {
    /// Imagery and labels have their own workers, so the (later-drawn) labels never starve the imagery.
    tiles: Arc<Tiles>,
    labels: Arc<Tiles>,
    tex: HashMap<TileKey, egui::TextureHandle>,
    order: VecDeque<TileKey>,
    uploads: usize,
}

/// The tiles of zoom `z` covering `rect`, with their screen rectangles, centre first.
fn cells(map: &Map, rect: Rect, z: u8) -> Vec<(u32, u32, Rect)> {
    let n = 1u32 << z;
    let world = 512.0 * 2f64.powf(map.view.zoom);
    let (cx, cy) = merc(map.view.lon, map.view.lat);
    let half_w = f64::from(rect.width()) / 2.0 / world;
    let half_h = f64::from(rect.height()) / 2.0 / world;
    let x0 = ((cx - half_w) * f64::from(n)).floor() as i64;
    let x1 = ((cx + half_w) * f64::from(n)).floor() as i64;
    let y0 = (((cy - half_h) * f64::from(n)).floor() as i64).max(0);
    let y1 = (((cy + half_h) * f64::from(n)).floor() as i64).min(i64::from(n) - 1);
    let tile = world / f64::from(n);
    let (mx, my) = ((cx * f64::from(n)) as i64, (cy * f64::from(n)) as i64);
    let mut out: Vec<(i64, i64)> = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y))).collect();
    out.sort_by_key(|(x, y)| (x - mx).pow(2) + (y - my).pow(2));
    out.into_iter()
        .map(|(x, y)| {
            let left = rect.center().x + ((x as f64 * tile) - cx * world) as f32;
            let top = rect.center().y + ((y as f64 * tile) - cy * world) as f32;
            (x.rem_euclid(i64::from(n)) as u32, y as u32, Rect::from_min_size(Pos2::new(left, top), Vec2::splat(tile as f32)))
        })
        .collect()
}

fn zoom_level(map: &Map, layer: Basemap) -> u8 {
    ((map.view.zoom + 1.0).round() as i32).clamp(1, i32::from(layer.max_zoom())) as u8
}

impl Basemaps {
    pub fn new(ctx: &egui::Context) -> Self {
        let (c1, c2) = (ctx.clone(), ctx.clone());
        Self { tiles: Tiles::new(10, move || c1.request_repaint()), labels: Tiles::new(3, move || c2.request_repaint()), tex: HashMap::new(), order: VecDeque::new(), uploads: 0 }
    }

    pub fn begin_frame(&mut self) {
        self.uploads = 0;
        self.tiles.tick();
        self.labels.tick();
    }

    pub fn loading(&self) -> usize {
        self.tiles.pending() + self.labels.pending()
    }

    fn pool(&self, key: TileKey) -> &Arc<Tiles> {
        if key.map == Basemap::EsriLabels { &self.labels } else { &self.tiles }
    }

    /// Requests the tiles for a view without drawing (warm-up during loading, prefetch of other levels).
    fn request(&self, map: &Map, rect: Rect, layer: Basemap, z: u8, prio: u8) {
        for (x, y, _) in cells(map, rect, z) {
            let _ = self.pool(TileKey { map: layer, z, x, y }).get(TileKey { map: layer, z, x, y }, prio);
        }
    }

    /// Starts fetching (and decoding) the tiles a view will need, before it is shown.
    pub fn warm(&self, map: &Map, rect: Rect, layer: Basemap, labels: bool) {
        let z = zoom_level(map, layer);
        self.request(map, rect, layer, z, 0);
        self.request(map, rect, layer, z.saturating_sub(1).max(1), 1);
        if labels {
            self.request(map, rect, Basemap::EsriLabels, zoom_level(map, Basemap::EsriLabels), 1);
        }
    }

    /// The texture of a tile if it can be had this frame (already uploaded, or ready and within budget).
    fn texture(&mut self, ctx: &egui::Context, key: TileKey, request: Option<u8>) -> Option<egui::TextureId> {
        if let Some(t) = self.tex.get(&key) {
            return Some(t.id());
        }
        let img = match request {
            Some(prio) => self.pool(key).get(key, prio),
            None => self.pool(key).peek(key),
        }?;
        if self.uploads >= UPLOADS_PER_FRAME {
            ctx.request_repaint();
            return None;
        }
        self.uploads += 1;
        let ci = egui::ColorImage::from_rgba_unmultiplied([img.w, img.h], &img.rgba);
        let h = ctx.load_texture(format!("tile{key:?}"), ci, egui::TextureOptions::LINEAR);
        let id = h.id();
        self.tex.insert(key, h);
        self.order.push_back(key);
        while self.order.len() > MAX_TEXTURES {
            if let Some(old) = self.order.pop_front() {
                self.tex.remove(&old);
            }
        }
        Some(id)
    }

    /// Draws one tiled layer over `rect`.
    pub fn draw(&mut self, ctx: &egui::Context, painter: &egui::Painter, map: &Map, rect: Rect, layer: Basemap, tint: Color32) {
        let z = zoom_level(map, layer);
        let full = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let mut missing = 0;
        for (x, y, r) in cells(map, rect, z) {
            let key = TileKey { map: layer, z, x, y };
            if let Some(t) = self.texture(ctx, key, Some(0)) {
                painter.image(t, r, full, tint);
                continue;
            }
            missing += 1;
            // stand-in 1: the nearest ancestor that is ready, its matching part scaled up
            let mut k = key;
            for depth in 1..=8u32 {
                let Some(p) = k.parent() else { break };
                k = p;
                if let Some(t) = self.texture(ctx, k, None) {
                    let s = 1.0 / (1u32 << depth) as f32;
                    let (ox, oy) = ((key.x % (1 << depth)) as f32 * s, (key.y % (1 << depth)) as f32 * s);
                    painter.image(t, r, Rect::from_min_size(Pos2::new(ox, oy), Vec2::splat(s)), tint);
                    break;
                }
            }
            // stand-in 2 (zooming out): the four sharper children that are already here, on top
            if z < layer.max_zoom() {
                for (i, j) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
                    let child = TileKey { map: layer, z: z + 1, x: x * 2 + i, y: y * 2 + j };
                    if let Some(t) = self.texture(ctx, child, None) {
                        let q = Rect::from_min_size(r.min + Vec2::new(i as f32, j as f32) * r.width() / 2.0, r.size() / 2.0);
                        painter.image(t, q, full, tint);
                    }
                }
            }
        }
        // prefetch: the level above always (cheap, and it is the stand-in for zooming in), the level below
        // when the screen itself is complete
        self.request(map, rect, layer, z.saturating_sub(1).max(1), 1);
        if missing == 0 && z < layer.max_zoom() && self.pool(TileKey { map: layer, z, x: 0, y: 0 }).pending() < 4 {
            let inner = rect.shrink2(rect.size() * 0.25);
            self.request(map, inner, layer, z + 1, 2);
        }
    }
}
