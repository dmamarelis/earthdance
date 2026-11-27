//! The two sheets: "Is it real?" (validation against an independent reference) and About.

use eframe::egui::{self, RichText};
use ed_core::stats::agreement;
use ed_core::text::{fmt_num, t, to_fixed};

use crate::app::{tile, App};
use crate::theme;

pub fn show(app: &mut App, ctx: &egui::Context) {
    let l = app.st.lang;
    let Some(ds) = app.ds() else { return };
    if app.st.show_validation
        && let Some(v) = &ds.validation
    {
        let mut open = true;
        egui::Window::new(t("val.title", l)).open(&mut open).collapsible(false).default_width(820.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label(RichText::new(t("val.kicker", l).to_uppercase()).small().color(theme::ACCENT));
            let x: Vec<f64> = v.pairs.iter().map(|p| p.reference).collect();
            let y: Vec<f64> = v.pairs.iter().map(|p| p.nisar).collect();
            let a = agreement(&x, &y);
            ui.columns(2, |c| {
                crate::charts::validation(&mut c[0], v, ds.reference_name());
                let ui = &mut c[1];
                ui.horizontal_wrapped(|ui| {
                    tile(ui, t("val.n", l), &a.n.to_string(), "", None);
                    tile(ui, t("val.r2", l), &to_fixed(a.r2, 2), "", None);
                    tile(ui, t("val.rmse", l), &to_fixed(a.rmse, 1), &v.unit, None);
                    tile(ui, t("val.bias", l), &fmt_num(a.bias, 1), &v.unit, None);
                });
                ui.label(t("val.text", l));
                let label = ds.reference.as_ref().map_or(String::new(), |r| r.label.get(l).to_string());
                ui.label(RichText::new(format!("{} {}: {label}.{}", v.note.get(l), ds.reference_name(), if ds.demo { format!(" {}", t("demo.warn", l)) } else { String::new() })).small().color(theme::MUTED));
            });
        });
        app.st.show_validation = open;
    }
    if app.st.show_about {
        let mut open = true;
        let a = &ds.content.about;
        egui::Window::new("EarthDance").open(&mut open).collapsible(false).default_width(520.0).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label(RichText::new("NASA Space Apps Challenge 2026 · Dancing with the SARs").small().color(theme::ACCENT));
            ui.label(RichText::new(a.tagline.get(l)).size(20.0).strong());
            ui.add_space(6.0);
            ui.label(RichText::new(t("about.sources", l)).strong());
            for s in &a.sources {
                ui.horizontal(|ui| {
                    ui.hyperlink_to(format!("{} ↗", s.name), &s.url);
                    ui.label(RichText::new(&s.by).small().color(theme::MUTED));
                });
            }
            ui.label(RichText::new(t("about.team", l)).strong());
            ui.label(a.team.get(l));
            ui.label(RichText::new(t("about.ai", l)).strong());
            ui.label(a.ai.get(l));
        });
        app.st.show_about = open;
    }
}
