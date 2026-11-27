//! The satellite basemap under everything (and place names on top), Google-Earth style: tiles stream in
//! in the background, blurry ancestors stand in until the sharp tiles arrive, and texture uploads are
//! spread over frames so panning never stutters.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Rect};
use ed_sources::tiles::{Basemap, TileKey, Tiles};

use crate::map::{merc, Map};

/// New tile textures per frame at most (each upload is a GPU copy on the UI thread).
const UPLOADS_PER_FRAME: usize = 6;
/// Tile textures kept on the GPU.
const MAX_TEXTURES: usize = 700;

pub struct Basemaps {
    /// Imagery and labels have their own workers, so the (later-drawn) labels never starve the imagery.
    tiles: Arc<Tiles>,
    labels: Arc<Tiles>,
    tex: HashMap<TileKey, egui::TextureHandle>,
    order: VecDeque<TileKey>,
    uploads: usize,
}

impl Basemaps {
    pub fn new(ctx: &egui::Context) -> Self {
        let (c1, c2) = (ctx.clone(), ctx.clone());
        Self { tiles: Tiles::new(8, move || c1.request_repaint()), labels: Tiles::new(3, move || c2.request_repaint()), tex: HashMap::new(), order: VecDeque::new(), uploads: 0 }
    }

    pub fn begin_frame(&mut self) {
        self.uploads = 0;
    }

    pub fn loading(&self) -> usize {
        self.tiles.pending() + self.labels.pending()
    }

    fn pool(&self, key: TileKey) -> &Arc<Tiles> {
        if key.map == Basemap::EsriLabels { &self.labels } else { &self.tiles }
    }

    /// The texture of a tile if it can be had this frame (already uploaded, or ready and within budget).
    fn texture(&mut self, ctx: &egui::Context, key: TileKey, request: bool) -> Option<egui::TextureId> {
        if let Some(t) = self.tex.get(&key) {
            return Some(t.id());
        }
        let img = if request { self.pool(key).get(key) } else { self.pool(key).peek(key) }?;
        if self.uploads >= UPLOADS_PER_FRAME {
            ctx.request_repaint();
            return None;
        }
        self.uploads += 1;
        let ci = egui::ColorImage::from_rgba_unmultiplied([img.w, img.h], &img.rgba);
        let h = ctx.load_texture(format!("tile{:?}", key), ci, egui::TextureOptions::LINEAR);
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
        let z = ((map.view.zoom + 1.0).round() as i32).clamp(1, i32::from(layer.max_zoom())) as u8;
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
        // centre tiles first: they are requested (and so fetched) first
        let mut cells: Vec<(i64, i64)> = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y))).collect();
        let (mx, my) = ((cx * f64::from(n)) as i64, (cy * f64::from(n)) as i64);
        cells.sort_by_key(|(x, y)| (x - mx).pow(2) + (y - my).pow(2));
        cells.reverse(); // get() pushes to the front: the last requested is fetched first
        for (x, y) in cells {
            let wx = x.rem_euclid(i64::from(n)) as u32;
            let left = rect.center().x + ((x as f64 * tile) - cx * world) as f32;
            let top = rect.center().y + ((y as f64 * tile) - cy * world) as f32;
            let r = Rect::from_min_size(Pos2::new(left, top), egui::vec2(tile as f32, tile as f32));
            let key = TileKey { map: layer, z, x: wx, y: y as u32 };
            if let Some(t) = self.texture(ctx, key, true) {
                painter.image(t, r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
                continue;
            }
            // stand-in: the nearest ancestor that is ready, its matching quarter scaled up
            let mut k = key;
            for depth in 1..=6u32 {
                let Some(p) = k.parent() else { break };
                k = p;
                if let Some(t) = self.texture(ctx, k, false) {
                    let s = 1.0 / (1u32 << depth) as f32;
                    let (ox, oy) = ((key.x % (1 << depth)) as f32 * s, (key.y % (1 << depth)) as f32 * s);
                    painter.image(t, r, Rect::from_min_size(Pos2::new(ox, oy), egui::vec2(s, s)), tint);
                    break;
                }
            }
        }
    }
}
