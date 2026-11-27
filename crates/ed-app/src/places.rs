//! "Anywhere…": find a place by name (or take the map's centre) and build its data set live.

use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, RichText};
use ed_sources::live::{Found, Place};

use crate::app::{App, Mode, Source};
use crate::theme;

pub struct Places {
    pub open: bool,
    query: String,
    results: Vec<Found>,
    searching: Option<Receiver<Result<Vec<Found>, String>>>,
    error: Option<String>,
    half_km: f64,
}

impl Default for Places {
    fn default() -> Self {
        Self { open: false, query: String::new(), results: Vec::new(), searching: None, error: None, half_km: 8.0 }
    }
}

/// Detail of the live download: automatic (from the link speed), fast (coarse pixels) or detailed.
pub static QUALITY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn slack() -> Option<f64> {
    match QUALITY.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Some(3.5),
        2 => Some(1.5),
        _ => None,
    }
}

pub fn window(app: &mut App, ctx: &egui::Context) {
    if !app.places.open {
        return;
    }
    let el = app.st.lang == ed_core::Lang::El;
    // results of a search running in the background
    if let Some(rx) = &app.places.searching
        && let Ok(r) = rx.try_recv()
    {
        app.places.searching = None;
        match r {
            Ok(v) => {
                app.places.error = None;
                app.places.results = v;
            }
            Err(e) => app.places.error = Some(e),
        }
    }
    let mut open = true;
    let mut go: Option<Place> = None;
    egui::Window::new(if el { "Οπουδήποτε στη Γη" } else { "Anywhere on Earth" }).open(&mut open).collapsible(false).default_width(380.0).show(ctx, |ui| {
        ui.label(RichText::new(if el { "Sentinel-2 (AWS, χωρίς λογαριασμό) για τους τελευταίους 18 μήνες: νερό, σύννεφα, έκταση λίμνης, πρόβλεψη." } else { "Sentinel-2 (AWS, no account) for the last 18 months: water, clouds, lake extent, forecast." }).small().color(theme::MUTED));
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut app.places.query).hint_text(if el { "π.χ. Λίμνη Βόλβη" } else { "e.g. Lake Volvi" }).desired_width(260.0));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("🔍").clicked() || enter) && !app.places.query.trim().is_empty() && app.places.searching.is_none() {
                let (tx, rx) = channel();
                let (q, lang, ctx2) = (app.places.query.clone(), app.st.lang.code(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(ed_sources::live::geocode(&q, lang).map_err(|e| e.to_string()));
                    ctx2.request_repaint();
                });
                app.places.searching = Some(rx);
            }
        });
        ui.horizontal(|ui| {
            ui.label(if el { "Ακτίνα" } else { "Radius" });
            ui.add(egui::Slider::new(&mut app.places.half_km, 3.0..=25.0).step_by(1.0).suffix(" km"));
        });
        ui.horizontal(|ui| {
            ui.label(if el { "Λεπτομέρεια" } else { "Detail" });
            let mut q = QUALITY.load(std::sync::atomic::Ordering::Relaxed);
            for (v, el_s, en_s) in [(0u8, "Αυτόματα", "Auto"), (1, "Γρήγορα (~80 m)", "Fast (~80 m)"), (2, "Λεπτομερώς (~40 m)", "Detailed (~40 m)")] {
                ui.radio_value(&mut q, v, if el { el_s } else { en_s });
            }
            QUALITY.store(q, std::sync::atomic::Ordering::Relaxed);
        });
        if app.places.searching.is_some() {
            ui.label(RichText::new("…").color(theme::MUTED));
        }
        if let Some(e) = &app.places.error {
            ui.label(RichText::new(e).color(theme::ALERT));
        }
        for f in &app.places.results {
            if ui.selectable_label(false, format!("{} · {}  ({:.3}, {:.3})", f.name, f.detail, f.lat, f.lon)).clicked() {
                go = Some(Place { name: f.name.clone(), lon: f.lon, lat: f.lat, half_km: app.places.half_km });
            }
        }
        ui.separator();
        if ui.button(if el { "📍 Εδώ (κέντρο του χάρτη)" } else { "📍 Here (map centre)" }).clicked() {
            let v = app.map.view;
            go = Some(Place { name: format!("{:.3}°, {:.3}°", v.lat, v.lon), lon: v.lon, lat: v.lat, half_km: app.places.half_km });
        }
    });
    app.places.open = open;
    if let Some(p) = go {
        app.places.open = false;
        app.st.mode = Mode::Explore;
        app.load(Source::Live(p), ctx);
    }
}
