//! Headless-ish verification: `EARTHDANCE_SHOT=out.png` makes the app set itself up from environment
//! variables, render, take a screenshot of its own window, save it and quit.
//!
//!   EARTHDANCE_SHOT_MODE   explore | story | status
//!   EARTHDANCE_SHOT_SENSOR optical | radar
//!   EARTHDANCE_SHOT_SELECT p:<point id> | r:<region id>
//!   EARTHDANCE_SHOT_LANG   el | en
//!   EARTHDANCE_SHOT_FRAME  frame index
//!   EARTHDANCE_SHOT_WHATIF rain,irrigation (%)
//!   EARTHDANCE_SHOT_COMPARE compare-frame index (turns on the swipe)
//!   EARTHDANCE_SHOT_SHEET  validation | about
//!   EARTHDANCE_SHOT_ZOOM   zoom change after the data is shown (reports how long the tiles took)

use eframe::egui;

use crate::app::{App, Mode, Sensor};

#[derive(Default)]
pub struct Shot {
    frames: u32,
    configured: bool,
    requested: bool,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    zoomed: Option<std::time::Instant>,
}

fn var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// A screenshot or scripted run (the app then leaves the saved preferences alone).
pub fn active() -> bool {
    var("EARTHDANCE_SHOT").is_some() || var("EARTHDANCE_SCRIPT").is_some() || var("EARTHDANCE_TOUR").is_some()
}

impl App {
    pub fn shot(&mut self, ctx: &egui::Context) {
        let Some(path) = var("EARTHDANCE_SHOT") else { return };
        ctx.request_repaint();
        if self.data.is_none() || self.weather_pending() {
            return;
        }
        self.shot.frames += 1;
        if !self.shot.configured {
            self.shot.configured = true;
            match var("EARTHDANCE_SHOT_LANG").as_deref() {
                Some("en") => self.st.lang = ed_core::Lang::En,
                Some("el") => self.st.lang = ed_core::Lang::El,
                _ => {}
            }
            match var("EARTHDANCE_SHOT_MODE").as_deref() {
                Some("status") => self.set_mode(Mode::Status),
                Some("story") => self.set_mode(Mode::Story),
                _ => {}
            }
            if var("EARTHDANCE_SHOT_SENSOR").as_deref() == Some("radar") {
                self.st.sensor = Sensor::Radar;
            }
            if let Some(f) = var("EARTHDANCE_SHOT_FRAME").and_then(|v| v.parse().ok()) {
                self.set_frame(f);
            }
            if let Some(w) = var("EARTHDANCE_SHOT_WHATIF") {
                let v: Vec<f64> = w.split(',').filter_map(|x| x.parse().ok()).collect();
                if v.len() == 2 {
                    self.st.what_if = ed_core::insights::WhatIf { rain: v[0], irrigation: v[1] };
                }
            }
            if let Some(dz) = var("EARTHDANCE_SHOT_ZOOM").and_then(|v| v.parse::<f64>().ok()) {
                self.map.zoom_by(dz);
                self.shot.zoomed = Some(std::time::Instant::now());
            }
            if let Some(c) = var("EARTHDANCE_SHOT_COMPARE").and_then(|v| v.parse().ok()) {
                self.st.compare = true;
                self.st.compare_frame = c;
            }
            match var("EARTHDANCE_SHOT_SHEET").as_deref() {
                Some("validation") => self.st.show_validation = true,
                Some("about") => self.st.show_about = true,
                _ => {}
            }
            match var("EARTHDANCE_SHOT_SELECT") {
                Some(s) if s.starts_with("p:") => self.select_point(&s[2..], false),
                Some(s) if s.starts_with("r:") => self.select_region(&s[2..], false),
                _ => {}
            }
            self.shot.frames = 0;
            return;
        }
        // let flights and story animations settle
        if let Some(t0) = self.shot.zoomed
            && self.basemaps.loading() == 0
            && t0.elapsed().as_secs_f32() > 0.4
        {
            eprintln!("tiles complete {:.2} s after the zoom", t0.elapsed().as_secs_f64());
            self.shot.zoomed = None;
        }
        // let flights and story animations settle, and the basemap tiles arrive (at most ~60 s)
        if self.shot.frames >= 150 && (self.basemaps.loading() == 0 || self.shot.frames > 3600) && !self.shot.requested {
            self.shot.requested = true;
            let done = self.shot.done.clone();
            let ctx2 = ctx.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                match image::RgbaImage::from_raw(w as u32, h as u32, bytes).map(|b| b.save(&path)) {
                    Some(Ok(())) => eprintln!("screenshot saved to {path}"),
                    other => eprintln!("screenshot failed: {other:?}"),
                }
                done.store(true, std::sync::atomic::Ordering::Release);
                ctx2.request_repaint();
            })));
        }
        if self.shot.done.load(std::sync::atomic::Ordering::Acquire) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
