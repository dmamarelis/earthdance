//! UI tour: `EARTHDANCE_TOUR=dir` walks the app through every view it can show (each mode, layer, point,
//! area and tab, story chapter, sheet, panel state, in both languages), screenshots each one to `dir`, and
//! reports any panel whose contents are wider than the panel (`OVERFLOW <state> <panel> +N px`), which is
//! how text and tiles end up cut off or pushed off-screen. It ends with a summary and closes.
//!
//! States are set directly (not clicked), so the tour does not depend on the window size. Menus and
//! popups are checked with the click-driven script (`crate::script`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;
use ed_core::Lang;

use crate::app::{App, Mode, PointTab, RegionTab, Sensor};

/// Panels measured this frame: (panel, how far the contents stick out, px).
static OVERFLOW: Mutex<Vec<(String, f32)>> = Mutex::new(Vec::new());

/// Call at the end of a panel's contents: records it if they are wider than the panel.
pub fn check(name: &str, ui: &egui::Ui) {
    let over = ui.min_rect().right() - ui.clip_rect().right();
    if over > 1.0 {
        OVERFLOW.lock().expect("overflow").push((name.to_string(), over));
    }
}

type Setup = Box<dyn Fn(&mut App)>;

#[derive(Default)]
pub struct Tour {
    states: Vec<(String, Setup)>,
    next: usize,
    frames: u32,
    out: String,
    shot: Option<Arc<AtomicBool>>,
    found: Vec<String>,
    pub active: bool,
    built: bool,
}

impl Tour {
    pub fn from_env() -> Self {
        let Ok(out) = std::env::var("EARTHDANCE_TOUR") else { return Self::default() };
        let _ = std::fs::create_dir_all(&out);
        Self { out, active: true, ..Self::default() }
    }
}

fn reset(app: &mut App) {
    app.close_right();
    app.st.compare = false;
    app.st.show_validation = false;
    app.st.show_about = false;
    app.st.left_open = true;
    app.st.timeline_open = true;
    app.st.what_if = Default::default();
    if app.st.mode != Mode::Explore {
        app.set_mode(Mode::Explore);
    }
}

fn states(app: &App) -> Vec<(String, Setup)> {
    let Some(ds) = app.ds() else { return Vec::new() };
    let mut v: Vec<(String, Setup)> = Vec::new();
    for lang in [Lang::En, Lang::El] {
        let lg = if lang == Lang::En { "en" } else { "el" };
        let add = |v: &mut Vec<(String, Setup)>, name: String, f: Box<dyn Fn(&mut App)>| {
            v.push((
                format!("{lg}_{name}"),
                Box::new(move |app: &mut App| {
                    reset(app);
                    app.st.lang = lang;
                    f(app);
                }),
            ));
        };
        add(&mut v, "explore_optical".into(), Box::new(|a| a.st.sensor = Sensor::Optical));
        for (i, ly) in ds.layers.iter().enumerate() {
            add(&mut v, format!("explore_radar_{}", ly.info.id), Box::new(move |a| {
                a.st.sensor = Sensor::Radar;
                a.set_layer(i);
            }));
        }
        add(&mut v, "explore_compare".into(), Box::new(|a| {
            a.st.sensor = Sensor::Radar;
            a.st.compare = true;
            a.st.compare_frame = 0;
            a.set_frame(isize::MAX);
        }));
        add(&mut v, "explore_no_timeline".into(), Box::new(|a| a.st.timeline_open = false));
        for p in &ds.points {
            for (tab, tn) in [(PointTab::Ground, "ground"), (PointTab::Radar, "radar")] {
                let id = p.id.clone();
                add(&mut v, format!("point_{}_{tn}", p.id), Box::new(move |a| {
                    a.select_point(&id, false);
                    a.st.tab = tab;
                }));
            }
        }
        for i in 0..ds.content.story.len() {
            add(&mut v, format!("story_{i}"), Box::new(move |a| {
                a.set_mode(Mode::Story);
                a.go_story(i);
            }));
        }
        add(&mut v, "story_closed".into(), Box::new(|a| {
            a.set_mode(Mode::Story);
            a.st.left_open = false;
        }));
        add(&mut v, "status".into(), Box::new(|a| a.set_mode(Mode::Status)));
        add(&mut v, "status_closed".into(), Box::new(|a| {
            a.set_mode(Mode::Status);
            a.st.left_open = false;
        }));
        add(&mut v, "status_whatif".into(), Box::new(|a| {
            a.set_mode(Mode::Status);
            a.st.what_if = ed_core::insights::WhatIf { rain: -30.0, irrigation: 20.0 };
        }));
        for r in &ds.regions.regions {
            for (tab, tn) in [(RegionTab::Wx, "wx"), (RegionTab::Fx, "fx"), (RegionTab::H2o, "h2o")] {
                let id = r.id.clone();
                add(&mut v, format!("region_{}_{tn}", r.id), Box::new(move |a| {
                    a.set_mode(Mode::Status);
                    a.select_region(&id, false);
                    a.st.rtab = tab;
                }));
            }
        }
        add(&mut v, "sheet_validation".into(), Box::new(|a| a.st.show_validation = true));
        add(&mut v, "sheet_about".into(), Box::new(|a| a.st.show_about = true));
    }
    v
}

impl App {
    pub fn tour_tick(&mut self, ctx: &egui::Context) {
        if !self.tour.active {
            return;
        }
        ctx.request_repaint();
        if self.data.is_none() || self.weather_pending() {
            return;
        }
        if !self.tour.built {
            self.tour.states = states(self);
            self.tour.built = true;
            self.tour.frames = 0;
            eprintln!("tour: {} states", self.tour.states.len());
        }
        self.tour.frames += 1;
        // the frame just drawn belongs to the current state: collect its overflow reports
        let over: Vec<(String, f32)> = std::mem::take(&mut *OVERFLOW.lock().expect("overflow"));
        if let Some(done) = &self.tour.shot {
            if !done.load(Ordering::Acquire) {
                return;
            }
            self.tour.shot = None;
            self.tour.next += 1;
            self.tour.frames = 0;
        }
        let Some(i) = (self.tour.next < self.tour.states.len()).then_some(self.tour.next) else {
            eprintln!("tour: done, {} problem(s)", self.tour.found.len());
            for f in &self.tour.found {
                eprintln!("  {f}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        if self.tour.frames == 1 {
            let tour = std::mem::take(&mut self.tour.states);
            (tour[i].1)(self);
            self.tour.states = tour;
            return;
        }
        // settle: animations and flights, then tiles (at most ~3 s)
        if self.tour.frames < 40 || (self.basemaps.loading() > 0 && self.tour.frames < 180) {
            return;
        }
        let name = self.tour.states[i].0.clone();
        for (panel, px) in over {
            let line = format!("OVERFLOW {name} {panel} +{px:.0} px");
            eprintln!("{line}");
            if !self.tour.found.contains(&line) {
                self.tour.found.push(line);
            }
        }
        let path = format!("{}/{:03}_{name}.png", self.tour.out, i);
        let done = Arc::new(AtomicBool::new(false));
        let (d2, c2) = (done.clone(), ctx.clone());
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(b) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                let _ = b.save(&path);
            }
            d2.store(true, Ordering::Release);
            c2.request_repaint();
        })));
        self.tour.shot = Some(done);
    }
}
