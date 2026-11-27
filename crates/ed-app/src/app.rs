//! Application state, background loading, the actions every view can trigger, and the top-level layout.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Color32, RichText};
use ed_core::ask::Question;
use ed_core::insights::WhatIf;
use ed_core::model::{Dataset, WeatherOrigin, WeatherSet};
use ed_core::status::{self, Status};
use ed_core::text::{fmt_date, t, DateStyle};
use ed_core::{Date, Lang, Rgba};
use serde::{Deserialize, Serialize};

use crate::map::{Map, View};
use crate::sound::Player;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Real,
    Demo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Explore,
    Story,
    Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sensor {
    Optical,
    Radar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointTab {
    Ground,
    Radar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionTab {
    Wx,
    Fx,
    H2o,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WxState {
    Loading,
    Live,
    Snapshot,
    None,
}

impl WxState {
    pub fn key(self) -> &'static str {
        match self {
            WxState::Loading => "loading",
            WxState::Live => "live",
            WxState::Snapshot => "snapshot",
            WxState::None => "none",
        }
    }
}

/// What is drawn on the map (the "Show" menu).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ShowOpts {
    pub areas: bool,
    pub area_labels: bool,
    pub points: bool,
    pub point_labels: bool,
    pub shoreline: bool,
    pub legend: bool,
    pub grid: bool,
    pub estimates: bool,
}

impl Default for ShowOpts {
    fn default() -> Self {
        Self { areas: false, area_labels: true, points: true, point_labels: false, shoreline: true, legend: true, grid: true, estimates: true }
    }
}

/// What the user is looking at (the legacy app's `state` object).
pub struct UiState {
    pub lang: Lang,
    pub mode: Mode,
    pub layer: usize,
    pub frame: usize,
    pub sensor: Sensor,
    pub compare: bool,
    pub compare_frame: usize,
    /// Swipe position as a fraction of the map width.
    pub swipe: f32,
    pub point: Option<String>,
    pub tab: PointTab,
    pub region: Option<String>,
    pub rtab: RegionTab,
    pub story: usize,
    pub what_if: WhatIf,
    pub asked: Option<(Question, String, Instant)>,
    pub ask_input: String,
    pub show_validation: bool,
    pub show_about: bool,
    pub left_open: bool,
    pub timeline_open: bool,
    pub show: ShowOpts,
}

/// Remembered between runs.
#[derive(Serialize, Deserialize)]
struct Prefs {
    lang: Lang,
    source: Source,
    left_open: bool,
    timeline_open: bool,
    #[serde(default)]
    show: ShowOpts,
}

/// A data set ready to show.
pub struct Loaded {
    pub ds: Arc<Dataset>,
    pub status: Arc<Status>,
    pub wx: Option<Arc<WeatherSet>>,
    pub wx_state: WxState,
    pub today: Date,
    /// When the Story was opened from the real data: leaving it goes back there.
    pub story_from_real: bool,
}

enum Msg {
    Progress(String),
    Data(Box<Dataset>, Option<WeatherSet>),
    Failed(String),
}

pub struct App {
    pub st: UiState,
    pub source: Source,
    pub data: Option<Loaded>,
    pub map: Map,
    pub sound: Player,
    pub loading_msg: String,
    pub error: Option<String>,
    rx: Option<Receiver<Msg>>,
    wx_rx: Option<Receiver<Option<WeatherSet>>>,
    wx_fallback: Option<WeatherSet>,
    textures: HashMap<usize, egui::TextureHandle>,
    pending_mode: Option<Mode>,
    /// Kept so actions can start background work that repaints the UI.
    map_ctx: Option<egui::Context>,
    pub shot: crate::shot::Shot,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let prefs: Option<Prefs> = ed_sources::cache::get("prefs");
        let source = std::env::args()
            .find_map(|a| match a.as_str() {
                "--demo" => Some(Source::Demo),
                "--real" => Some(Source::Real),
                _ => None,
            })
            .or(prefs.as_ref().map(|p| p.source))
            .unwrap_or(Source::Real);
        let st = UiState {
            lang: prefs.as_ref().map_or(Lang::El, |p| p.lang),
            mode: Mode::Explore,
            layer: 0,
            frame: 0,
            sensor: Sensor::Optical,
            compare: false,
            compare_frame: 0,
            swipe: 0.5,
            point: None,
            tab: PointTab::Ground,
            region: None,
            rtab: RegionTab::Wx,
            story: 0,
            what_if: WhatIf::default(),
            asked: None,
            ask_input: String::new(),
            show_validation: false,
            show_about: false,
            left_open: prefs.as_ref().is_none_or(|p| p.left_open),
            timeline_open: prefs.as_ref().is_none_or(|p| p.timeline_open),
            show: prefs.as_ref().map_or_else(ShowOpts::default, |p| p.show),
        };
        let mut app = Self {
            st,
            source,
            data: None,
            map: Map::new(View { lon: 23.15, lat: 40.68, zoom: 10.6 }),
            sound: Player::default(),
            loading_msg: String::new(),
            error: None,
            rx: None,
            wx_rx: None,
            wx_fallback: None,
            textures: HashMap::new(),
            pending_mode: None,
            map_ctx: Some(cc.egui_ctx.clone()),
            shot: Default::default(),
        };
        app.load(source, &cc.egui_ctx);
        app
    }

    pub fn save_prefs(&self) {
        ed_sources::cache::put("prefs", &Prefs { lang: self.st.lang, source: self.source, left_open: self.st.left_open, timeline_open: self.st.timeline_open, show: self.st.show });
    }

    // ───────────────────────────────────────────────────────── loading

    /// Starts building a data set in the background.
    pub fn load(&mut self, source: Source, ctx: &egui::Context) {
        self.source = source;
        self.data = None;
        self.error = None;
        self.textures.clear();
        self.sound.stop();
        self.st.point = None;
        self.st.region = None;
        self.st.compare = false;
        self.loading_msg = t("loader.data", self.st.lang).to_string();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let root = ed_sources::data_root();
            let progress = |s: &str| {
                let _ = tx.send(Msg::Progress(s.to_string()));
                ctx.request_repaint();
            };
            let res = match source {
                Source::Demo => ed_sources::demo::load(&root),
                Source::Real => ed_sources::real::load(&root, &progress),
            };
            let _ = tx.send(match res {
                Ok((ds, wx)) => Msg::Data(Box::new(ds), wx),
                Err(e) => Msg::Failed(e.to_string()),
            });
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        self.save_prefs();
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.rx {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Progress(s) => self.loading_msg = s,
                    Msg::Failed(e) => {
                        self.error = Some(e);
                        self.rx = None;
                        return;
                    }
                    Msg::Data(ds, wx) => {
                        self.rx = None;
                        self.on_data(*ds, wx, ctx);
                        return;
                    }
                }
            }
        }
        if let Some(rx) = &self.wx_rx
            && let Ok(w) = rx.try_recv()
        {
            self.wx_rx = None;
            if let Some(d) = &mut self.data {
                let w = w.or_else(|| self.wx_fallback.take());
                d.wx_state = match w.as_ref().map(|w| w.source) {
                    Some(WeatherOrigin::Live) => WxState::Live,
                    Some(WeatherOrigin::Snapshot) => WxState::Snapshot,
                    None => WxState::None,
                };
                d.wx = w.map(Arc::new);
                self.recompute();
            }
        }
    }

    fn on_data(&mut self, ds: Dataset, wx_fallback: Option<WeatherSet>, ctx: &egui::Context) {
        let today = ed_sources::today_local();
        let story_from_real = self.pending_mode == Some(Mode::Story) && ds.demo;
        let ds = Arc::new(ds);
        let status = Arc::new(status::compute(&ds.regions, None, false, ds.reference_name(), ds.forecast.as_ref(), today));
        self.map.fly_to(View { lon: ds.site.center[0], lat: ds.site.center[1], zoom: ds.site.zoom }, 0.0);
        self.st.layer = 0;
        self.st.frame = 0;
        self.st.compare_frame = 0;
        if ds.forecast.is_some() {
            self.st.rtab = RegionTab::H2o;
        }
        // weather: a month before the first pass, up to a week ahead
        let first = ds.regions.regions.iter().filter_map(|r| r.series.dates.first()).min().copied();
        if let Some(first) = first {
            let (tx, rx) = channel();
            let regions = ds.regions.regions.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(ed_sources::weather::load(&regions, first.add_days(-30), today, None));
                ctx.request_repaint();
            });
            self.wx_rx = Some(rx);
        }
        self.wx_fallback = wx_fallback;
        self.data = Some(Loaded { ds, status, wx: None, wx_state: if first.is_some() { WxState::Loading } else { WxState::None }, today, story_from_real });
        if let Some(m) = self.pending_mode.take() {
            self.set_mode(m);
        }
        self.set_layer(0);
    }

    /// Re-runs every rule (after the weather arrives).
    pub fn recompute(&mut self) {
        if let Some(d) = &mut self.data {
            let loaded = d.wx_state != WxState::Loading;
            d.status = Arc::new(status::compute(&d.ds.regions, d.wx.as_deref(), loaded, d.ds.reference_name(), d.ds.forecast.as_ref(), d.today));
        }
    }

    // ───────────────────────────────────────────────────────── textures

    pub fn texture(&mut self, ctx: &egui::Context, img: &Arc<Rgba>) -> egui::TextureId {
        let key = Arc::as_ptr(img) as usize;
        self.textures
            .entry(key)
            .or_insert_with(|| {
                let flat: Vec<u8> = img.px.iter().flatten().copied().collect();
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.w, img.h], &flat);
                ctx.load_texture(format!("img{key}"), ci, egui::TextureOptions::LINEAR)
            })
            .id()
    }

    // ───────────────────────────────────────────────────────── actions

    pub fn ds(&self) -> Option<Arc<Dataset>> {
        self.data.as_ref().map(|d| d.ds.clone())
    }

    pub fn set_layer(&mut self, i: usize) {
        let Some(ds) = self.ds() else { return };
        if ds.layers.is_empty() {
            return;
        }
        let date = ds.layers.get(self.st.layer).and_then(|l| l.frames.get(self.st.frame)).map(|f| f.date);
        self.st.layer = i.min(ds.layers.len() - 1);
        let fr = &ds.layers[self.st.layer].frames;
        self.st.frame = date.map_or(0, |d| nearest(fr.iter().map(|f| f.date), d));
        self.st.compare_frame = self.st.compare_frame.min(fr.len().saturating_sub(1));
    }

    pub fn set_frame(&mut self, i: isize) {
        let Some(ds) = self.ds() else { return };
        let n = ds.layers.get(self.st.layer).map_or(0, |l| l.frames.len());
        if n > 0 {
            self.st.frame = i.clamp(0, n as isize - 1) as usize;
        }
    }

    pub fn frame_date(&self) -> Option<Date> {
        let ds = self.data.as_ref()?.ds.clone();
        ds.layers.get(self.st.layer)?.frames.get(self.st.frame).map(|f| f.date)
    }

    pub fn set_frame_to_date(&mut self, d: Date) {
        let Some(ds) = self.ds() else { return };
        if let Some(l) = ds.layers.get(self.st.layer) {
            self.st.frame = nearest(l.frames.iter().map(|f| f.date), d);
        }
    }

    pub fn select_point(&mut self, id: &str, fly: bool) {
        let Some(ds) = self.ds() else { return };
        let Some(p) = ds.point(id) else { return };
        if self.st.point.as_deref() != Some(id) {
            self.sound.stop();
        }
        self.st.point = Some(id.to_string());
        self.st.region = None;
        if fly {
            self.map.fly_to(View { lon: p.lon, lat: p.lat, zoom: self.map.view.zoom }, 0.9);
        }
    }

    pub fn select_region(&mut self, id: &str, fly: bool) {
        let Some(ds) = self.ds() else { return };
        let Some(r) = ds.region(id) else { return };
        self.sound.stop();
        self.st.point = None;
        self.st.region = Some(id.to_string());
        if fly {
            self.map.fly_to(View { lon: r.center[0], lat: r.center[1], zoom: self.map.view.zoom.max(9.8) }, 0.9);
        }
    }

    pub fn close_right(&mut self) {
        self.st.point = None;
        self.st.region = None;
        self.sound.stop();
    }

    pub fn home(&mut self) {
        if let Some(ds) = self.ds() {
            self.map.fly_to(View { lon: ds.site.center[0], lat: ds.site.center[1], zoom: ds.site.zoom }, 1.2);
        }
    }

    /// The Story runs on the demo data; opening it from the real data is a visit, and leaving it goes back.
    pub fn set_mode(&mut self, mode: Mode) {
        let (demo, from_real) = self.data.as_ref().map_or((false, false), |d| (d.ds.demo, d.story_from_real));
        if mode == Mode::Story && !demo {
            self.pending_mode = Some(Mode::Story);
            let ctx = self.map_ctx.clone();
            if let Some(ctx) = ctx {
                self.load(Source::Demo, &ctx);
            }
            return;
        }
        if mode != Mode::Story && demo && from_real {
            self.pending_mode = Some(mode);
            if let Some(ctx) = self.map_ctx.clone() {
                self.load(Source::Real, &ctx);
            }
            return;
        }
        self.st.mode = mode;
        match mode {
            Mode::Story => self.go_story(0),
            Mode::Status => {
                self.close_right();
                self.home();
            }
            Mode::Explore => {
                if self.st.region.is_some() {
                    self.close_right();
                }
            }
        }
    }

    pub fn go_story(&mut self, i: usize) {
        let Some(ds) = self.ds() else { return };
        if i >= ds.content.story.len() {
            self.set_mode(Mode::Status);
            return;
        }
        self.st.story = i;
        let ch = ds.content.story[i].clone();
        self.st.compare = false;
        if let Some(s) = &ch.sensor {
            self.st.sensor = if s == "radar" { Sensor::Radar } else { Sensor::Optical };
        }
        if let Some(l) = ch.layer.as_ref().and_then(|l| ds.layers.iter().position(|x| &x.info.id == l))
            && l != self.st.layer
        {
            self.set_layer(l);
        }
        if let Some(f) = ch.frame {
            self.set_frame(f as isize);
        }
        match &ch.point {
            Some(p) => self.select_point(p, false),
            None => self.close_right(),
        }
        let c = ch.center.unwrap_or(ds.site.center);
        self.map.fly_to(View { lon: c[0], lat: c[1], zoom: ch.zoom.unwrap_or(ds.site.zoom) }, 2.6);
    }

    pub fn toggle_listen(&mut self) {
        if self.sound.is_playing() {
            self.sound.stop();
            return;
        }
        let Some(ds) = self.ds() else { return };
        let Some(p) = self.st.point.as_ref().and_then(|id| ds.point(id)) else { return };
        let s = &p.series;
        let lo = ed_core::stats::min(&s.hv_db);
        let hi = ed_core::stats::max(&s.hv_db);
        let span = if hi - lo == 0.0 { 1.0 } else { hi - lo };
        let bright: Vec<f64> = s.hv_db.iter().map(|v| (v - lo) / span).collect();
        let vals = if self.st.tab == PointTab::Ground { &s.disp_mm } else { &s.hv_db };
        self.sound.play(vals, &bright);
    }

    pub fn ask(&mut self, q: Question, text: String) {
        self.st.asked = Some((q, text, Instant::now()));
    }
}

/// Index of the date nearest to `d`.
pub fn nearest(dates: impl Iterator<Item = Date>, d: Date) -> usize {
    dates.enumerate().min_by_key(|(_, x)| (x.since(d)).abs()).map_or(0, |(i, _)| i)
}

pub fn short(d: Date, l: Lang) -> String {
    fmt_date(d, l, DateStyle::DayMon)
}

pub fn chip(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text.into()).color(color).small().strong());
        });
}

pub fn section(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text.to_uppercase()).small().color(theme::MUTED).strong());
    ui.add_space(2.0);
}

pub fn card<R>(ui: &mut egui::Ui, accent: Option<Color32>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let stroke = accent.map_or(egui::Stroke::new(1.0, Color32::from_white_alpha(18)), |c| egui::Stroke::new(1.0, c.gamma_multiply(0.7)));
    egui::Frame::new()
        .fill(accent.map_or(theme::CARD, |c| c.gamma_multiply(0.08)))
        .stroke(stroke)
        .corner_radius(12)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn tile(ui: &mut egui::Ui, label: &str, value: &str, unit: &str, color: Option<Color32>) {
    egui::Frame::new().fill(theme::CARD).corner_radius(10).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_min_width(96.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(label).small().color(theme::MUTED));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.label(RichText::new(value).size(20.0).strong().color(color.unwrap_or(theme::TEXT)));
                ui.label(RichText::new(unit).small().color(theme::MUTED));
            });
        });
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.map_ctx = Some(ctx.clone());
        self.poll(&ctx);
        self.shot(&ctx);
        for i in self.sound.tick() {
            if let (Some(ds), Some(pid)) = (self.ds(), self.st.point.clone())
                && let Some(p) = ds.point(&pid)
                && let Some(d) = p.series.dates.get(i)
            {
                self.set_frame_to_date(*d);
            }
        }
        if self.sound.is_playing() {
            ctx.request_repaint();
        }
        if self.data.is_none() {
            crate::panels::loader(self, ui);
            return;
        }
        self.keys(&ctx);
        crate::panels::top_bar(self, ui);
        if self.st.timeline_open && self.st.mode != Mode::Status {
            crate::panels::timeline(self, ui);
        }
        // drawers: the briefing / story card on the left (not in Explore), the selection on the right
        if self.st.mode != Mode::Explore {
            let mut open = self.st.left_open;
            egui::Panel::left("left").resizable(false).exact_size(330.0).show_collapsible(ui, &mut open, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match self.st.mode {
                    Mode::Story => crate::panels::story_left(self, ui),
                    _ => crate::status_ui::brief(self, ui),
                });
            });
            if open != self.st.left_open {
                self.st.left_open = open;
                self.save_prefs();
            }
        }
        let mut right = self.st.point.is_some() || self.st.region.is_some();
        egui::Panel::right("right").resizable(false).exact_size(380.0).show_collapsible(ui, &mut right, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                if self.st.region.is_some() {
                    crate::status_ui::region(self, ui);
                } else {
                    crate::panels::inspector(self, ui);
                }
            });
        });
        if !right && (self.st.point.is_some() || self.st.region.is_some()) {
            self.close_right();
        }
        egui::CentralPanel::no_frame().show(ui, |ui| crate::panels::map_view(self, ui));
        crate::sheets::show(self, &ctx);
        if self.st.asked.as_ref().is_some_and(|a| a.2.elapsed().as_secs_f32() < 6.0) {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_prefs();
    }
}

impl App {
    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (left, right, r, esc) = ctx.input(|i| {
            (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight), i.key_pressed(egui::Key::R), i.key_pressed(egui::Key::Escape))
        });
        if esc {
            self.st.show_validation = false;
            self.st.show_about = false;
        }
        if r {
            self.st.sensor = if self.st.sensor == Sensor::Radar { Sensor::Optical } else { Sensor::Radar };
        }
        if self.st.mode == Mode::Story {
            if right {
                self.go_story(self.st.story + 1);
            }
            if left && self.st.story > 0 {
                self.go_story(self.st.story - 1);
            }
            return;
        }
        if right {
            self.set_frame(self.st.frame as isize + 1);
        }
        if left {
            self.set_frame(self.st.frame as isize - 1);
        }
    }

    pub fn weather_pending(&self) -> bool {
        self.wx_rx.is_some() || self.rx.is_some()
    }

    pub fn toggle_lang(&mut self) {
        self.st.lang = self.st.lang.other();
        self.save_prefs();
    }

    pub fn toggle_left(&mut self) {
        self.st.left_open = !self.st.left_open;
        self.save_prefs();
    }

    pub fn toggle_timeline(&mut self) {
        self.st.timeline_open = !self.st.timeline_open;
        self.save_prefs();
    }
}
