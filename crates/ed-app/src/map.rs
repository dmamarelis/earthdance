//! The map: a Web-Mercator canvas with pan, zoom and eased "fly to", drawing georeferenced images, areas,
//! points and lines. Zoom levels follow MapLibre's (512-px world at zoom 0), so the story chapters' zooms
//! carry over.
//!
//! No basemap yet: which tiles to draw (and how) is decision D2. The overlays are positioned in real
//! coordinates, so a tile layer can slot in underneath later.

use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Shape, Stroke, Vec2};
use ed_core::Bounds;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub lon: f64,
    pub lat: f64,
    pub zoom: f64,
}

/// Web Mercator, normalised to [0, 1] on both axes.
pub fn merc(lon: f64, lat: f64) -> (f64, f64) {
    let x = (lon + 180.0) / 360.0;
    let s = lat.to_radians().sin().clamp(-0.9999, 0.9999);
    let y = 0.5 - ((1.0 + s) / (1.0 - s)).ln() / (4.0 * PI);
    (x, y)
}

pub fn unmerc(x: f64, y: f64) -> (f64, f64) {
    let lon = x * 360.0 - 180.0;
    let lat = (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    (lon, lat)
}

pub struct Map {
    pub view: View,
    flight: Option<(View, View, Instant, Duration)>,
}

fn ease(t: f64) -> f64 {
    // ease-in-out cubic
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

impl Map {
    pub fn new(view: View) -> Self {
        Self { view, flight: None }
    }

    fn world(&self) -> f64 {
        512.0 * 2f64.powf(self.view.zoom)
    }

    pub fn to_screen(&self, rect: Rect, lon: f64, lat: f64) -> Pos2 {
        let (cx, cy) = merc(self.view.lon, self.view.lat);
        let (x, y) = merc(lon, lat);
        let w = self.world();
        rect.center() + Vec2::new(((x - cx) * w) as f32, ((y - cy) * w) as f32)
    }

    pub fn to_geo(&self, rect: Rect, p: Pos2) -> (f64, f64) {
        let (cx, cy) = merc(self.view.lon, self.view.lat);
        let w = self.world();
        let d = p - rect.center();
        unmerc(cx + f64::from(d.x) / w, cy + f64::from(d.y) / w)
    }

    pub fn bounds_rect(&self, rect: Rect, b: Bounds) -> Rect {
        Rect::from_two_pos(self.to_screen(rect, b.west(), b.north()), self.to_screen(rect, b.east(), b.south()))
    }

    /// Kilometres per screen pixel at the view centre.
    pub fn km_per_px(&self) -> f64 {
        40_075.0 * self.view.lat.to_radians().cos() / self.world()
    }

    pub fn fly_to(&mut self, to: View, secs: f32) {
        if secs <= 0.0 {
            self.view = to;
            self.flight = None;
        } else {
            self.flight = Some((self.view, to, Instant::now(), Duration::from_secs_f32(secs)));
        }
    }

    pub fn zoom_by(&mut self, dz: f64) {
        let to = View { zoom: (self.view.zoom + dz).clamp(3.0, 18.0), ..self.view };
        self.fly_to(to, 0.3);
    }

    /// Animation, drag-to-pan and scroll-to-zoom (around the cursor).
    pub fn interact(&mut self, ui: &egui::Ui, rect: Rect, resp: &egui::Response) {
        if let Some((from, to, t0, dur)) = self.flight {
            let t = (t0.elapsed().as_secs_f64() / dur.as_secs_f64()).min(1.0);
            let k = ease(t);
            // interpolate in Mercator so the path is straight on screen
            let (fx, fy) = merc(from.lon, from.lat);
            let (tx, ty) = merc(to.lon, to.lat);
            let (lon, lat) = unmerc(fx + (tx - fx) * k, fy + (ty - fy) * k);
            self.view = View { lon, lat, zoom: from.zoom + (to.zoom - from.zoom) * k };
            if t >= 1.0 {
                self.flight = None;
            }
            ui.ctx().request_repaint();
        }
        if resp.dragged() {
            self.flight = None;
            let d = resp.drag_delta();
            let (cx, cy) = merc(self.view.lon, self.view.lat);
            let w = self.world();
            let (lon, lat) = unmerc(cx - f64::from(d.x) / w, cy - f64::from(d.y) / w);
            self.view.lon = lon;
            self.view.lat = lat.clamp(-80.0, 80.0);
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.flight = None;
                let pointer = ui.input(|i| i.pointer.hover_pos()).unwrap_or(rect.center());
                let before = self.to_geo(rect, pointer);
                self.view.zoom = (self.view.zoom + f64::from(scroll) / 240.0).clamp(3.0, 18.0);
                // keep the point under the cursor where it was
                let after = self.to_geo(rect, pointer);
                self.view.lon += before.0 - after.0;
                self.view.lat += before.1 - after.1;
            }
        }
    }

    /// A light graticule, so the empty map still reads as a map until a basemap exists (D2).
    pub fn graticule(&self, painter: &egui::Painter, rect: Rect) {
        let (w, n) = self.to_geo(rect, rect.left_top());
        let (e, s) = self.to_geo(rect, rect.right_bottom());
        let span = (e - w).max(1e-6);
        let step = [0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0].into_iter().find(|s| span / s <= 12.0).unwrap_or(10.0);
        let stroke = Stroke::new(1.0, Color32::from_white_alpha(10));
        let mut x = (w / step).ceil() * step;
        while x <= e {
            let a = self.to_screen(rect, x, n);
            let b = self.to_screen(rect, x, s);
            painter.line_segment([a, b], stroke);
            x += step;
        }
        let mut y = (s / step).ceil() * step;
        while y <= n {
            let a = self.to_screen(rect, w, y);
            let b = self.to_screen(rect, e, y);
            painter.line_segment([a, b], stroke);
            y += step;
        }
    }
}

/// A circle of `km` around `[lon, lat]` as a closed ring (72 steps, like the legacy `ring()`).
pub fn ring(center: [f64; 2], km: f64) -> Vec<[f64; 2]> {
    (0..=72)
        .map(|i| {
            let a = f64::from(i) / 72.0 * 2.0 * PI;
            [center[0] + km / (111.32 * center[1].to_radians().cos()) * a.cos(), center[1] + km / 110.57 * a.sin()]
        })
        .collect()
}

/// A filled star-shaped polygon (fan from its centroid: right for circles and for the 72-ray lake outline,
/// which egui's convex fill cannot draw).
pub fn star_fill(points: &[Pos2], fill: Color32) -> Shape {
    let n = points.len();
    if n < 3 {
        return Shape::Noop;
    }
    let c = points.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2()) / n as f32;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(c.to_pos2(), fill);
    for p in points {
        mesh.colored_vertex(*p, fill);
    }
    for i in 0..n as u32 {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n as u32);
    }
    Shape::mesh(mesh)
}

/// A dashed polyline.
pub fn dashed(points: &[Pos2], stroke: Stroke, dash: f32, gap: f32) -> Vec<Shape> {
    Shape::dashed_line(points, stroke, dash, gap)
}
