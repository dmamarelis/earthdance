//! Explore and Story views: loader, top bar, timeline, layer list + legend, point inspector, story card,
//! and the map canvas with its overlays.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, UiBuilder, Vec2};
use ed_core::model::{Frame, LayerInfo};
use ed_core::stats::{max, min, slope_per_year};
use ed_core::text::{fill, fmt_date, fmt_num, t, tf, to_fixed, DateStyle};
use ed_core::Level;

use crate::app::{card, chip, nearest, short, tile, App, Mode, PointTab, Sensor, Source};
use crate::map::{dashed, ring, star_fill};
use crate::theme;

// ───────────────────────────────────────────────────────── loader

pub fn loader(app: &mut App, ui: &mut egui::Ui) {
    egui::CentralPanel::no_frame().show(ui, |ui| {
        let r = ui.max_rect();
        ui.painter().rect_filled(r, 0, theme::BG);
        // warm-up: the basemap of the coming view loads (and shows, dimmed) behind the loader
        if let Some(b) = app.basemap() {
            let ctx = ui.ctx().clone();
            app.basemaps.warm(&app.map, r, b, app.st.show.labels);
            app.basemaps.draw(&ctx, ui.painter(), &app.map, r, b, Color32::from_gray(60));
        }
        let c = r.center();
        let time = ui.input(|i| i.time) as f32;
        for k in 0..3 {
            let ph = ((time * 0.6 + k as f32 / 3.0) % 1.0) * 60.0 + 10.0;
            ui.painter().circle_stroke(c - Vec2::new(0.0, 40.0), ph, Stroke::new(1.5, theme::ACCENT.gamma_multiply(1.0 - ph / 70.0)));
        }
        ui.painter().text(c + Vec2::new(0.0, 40.0), Align2::CENTER_CENTER, "EarthDance", FontId::proportional(28.0), theme::TEXT);
        let msg = app.error.as_ref().map_or(app.loading_msg.clone(), |e| format!("{} {e}", t("err.load", app.st.lang)));
        ui.painter().text(c + Vec2::new(0.0, 72.0), Align2::CENTER_CENTER, msg, FontId::proportional(14.0), theme::MUTED);
        if app.error.is_some() {
            let b = Rect::from_center_size(c + Vec2::new(0.0, 110.0), Vec2::new(200.0, 28.0));
            let other = if app.source == Source::Demo { Source::Real } else { Source::Demo };
            if ui.put(b, egui::Button::new(t(if other == Source::Demo { "src.demo" } else { "src.real" }, app.st.lang))).clicked() {
                let ctx = ui.ctx().clone();
                app.load(other, &ctx);
            }
        } else {
            ui.ctx().request_repaint();
        }
    });
}

// ───────────────────────────────────────────────────────── top bar

pub fn top_bar(app: &mut App, ui: &mut egui::Ui) {
    let l = app.st.lang;
    let Some(d) = &app.data else { return };
    let (ds, status) = (d.ds.clone(), d.status.clone());
    let alerts = status.counts()[Level::Alert as usize];
    egui::Panel::top("top").exact_size(40.0).show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            if app.st.mode != Mode::Explore && ui.selectable_label(app.st.left_open, "☰").on_hover_text(t(if app.st.left_open { "ui.hide" } else { "ui.show" }, l)).clicked() {
                app.toggle_left();
            }
            ui.label(RichText::new("EarthDance").strong().color(theme::ACCENT));
            ui.label(RichText::new(ds.site.name.get(l)).small().color(theme::MUTED));
            ui.separator();
            for (m, key) in [(Mode::Explore, "mode.explore"), (Mode::Story, "mode.story"), (Mode::Status, "mode.status")] {
                let mut label = t(key, l).to_string();
                if m == Mode::Status && alerts > 0 {
                    label = format!("{label} ·{alerts}");
                }
                let mut b = ui.selectable_label(app.st.mode == m, label);
                if m == Mode::Story && !ds.demo {
                    b = b.on_hover_text(t("src.story", l));
                }
                if b.clicked() && app.st.mode != m {
                    app.set_mode(m);
                }
            }
            ui.separator();
            if app.st.mode != Mode::Status && !ds.layers.is_empty() {
                let cur = ds.layers.get(app.st.layer).map_or(String::new(), |ly| ly.info.name.get(l).to_string());
                ui.menu_button(format!("{cur} ▾"), |ui| layers_menu(app, ui));
            }
            ui.menu_button(format!("{} ▾", if l == ed_core::Lang::El { "Εμφάνιση" } else { "Show" }), |ui| show_menu(app, ui));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button(if l == ed_core::Lang::El { "EN" } else { "ΕΛ" }).clicked() {
                    app.toggle_lang();
                }
                ui.menu_button("⋯", |ui| {
                    if ds.validation.is_some() && ui.button(format!("✓ {}", t("nav.validation", l))).clicked() {
                        app.st.show_validation = true;
                        ui.close();
                    }
                    if ui.button("ℹ EarthDance").clicked() {
                        app.st.show_about = true;
                        ui.close();
                    }
                });
                if ds.demo {
                    ui.label(RichText::new(t("demo.badge", l)).small().color(theme::WATCH));
                }
                let anywhere = if l == ed_core::Lang::El { "🌍 Οπουδήποτε…" } else { "🌍 Anywhere…" };
                let current = match &app.source {
                    Source::Real => t("src.real", l).to_string(),
                    Source::Demo => t("src.demo", l).to_string(),
                    Source::Live(p) => format!("🌍 {}", p.name),
                };
                egui::ComboBox::from_id_salt("source").width(150.0).selected_text(current).show_ui(ui, |ui| {
                    for (s, key) in [(Source::Real, "src.real"), (Source::Demo, "src.demo")] {
                        if ui.selectable_label(app.source == s, t(key, l)).clicked() && app.source != s {
                            let ctx = ui.ctx().clone();
                            app.st.mode = Mode::Explore;
                            app.load(s, &ctx);
                        }
                    }
                    if ui.selectable_label(matches!(app.source, Source::Live(_)), anywhere).clicked() {
                        app.places.open = true;
                    }
                });
                if app.streaming {
                    ui.label(RichText::new(format!("⟳ {}", app.loading_msg)).small().color(theme::MUTED));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                }
            });
        });
    });
}

/// The layer picker (a dropdown instead of a permanent panel).
fn layers_menu(app: &mut App, ui: &mut egui::Ui) {
    let l = app.st.lang;
    let Some(ds) = app.ds() else { return };
    ui.set_min_width(300.0);
    for (i, ly) in ds.layers.iter().enumerate() {
        let r = ui
            .horizontal(|ui| {
                let (sw, _) = ui.allocate_exact_size(Vec2::new(6.0, 28.0), Sense::hover());
                paint_gradient(ui.painter(), sw, &ly.info);
                ui.vertical(|ui| {
                    ui.label(RichText::new(ly.info.name.get(l)).strong().color(if i == app.st.layer { theme::ACCENT } else { theme::TEXT }));
                    ui.label(RichText::new(format!("{} · {}-band", ly.info.short.get(l), ly.info.band)).small().color(theme::MUTED));
                });
            })
            .response
            .interact(Sense::click());
        if r.clicked() {
            app.set_layer(i);
            ui.close();
        }
    }
    if !ds.missing.is_empty() {
        ui.separator();
        let list = ds.missing.iter().map(|m| m.name.get(l)).collect::<Vec<_>>().join(", ");
        ui.label(RichText::new(strip_tags(&tf("src.missing", l, &[("list", &list)]))).small().color(theme::MUTED));
    }
}

/// What to draw on the map.
fn show_menu(app: &mut App, ui: &mut egui::Ui) {
    let el = app.st.lang == ed_core::Lang::El;
    let s = &mut app.st.show;
    let before = format!("{s:?}");
    ui.label(RichText::new(if el { "Υπόβαθρο" } else { "Basemap" }).small().color(theme::MUTED));
    for (v, a, b) in [(0u8, "Δορυφορική εικόνα (Esri)", "Satellite imagery (Esri)"), (1, "Sentinel-2 χωρίς σύννεφα (EOX)", "Sentinel-2 cloudless (EOX)"), (2, "Κανένα", "None")] {
        ui.radio_value(&mut s.basemap, v, if el { a } else { b });
    }
    ui.checkbox(&mut s.labels, if el { "Τοπωνύμια" } else { "Place names" });
    ui.horizontal(|ui| {
        ui.label(if el { "Διαφάνεια επιπέδων" } else { "Layer opacity" });
        ui.add(egui::Slider::new(&mut s.opacity, 0.2..=1.0).show_value(false));
    });
    ui.separator();
    ui.checkbox(&mut s.legend, if el { "Υπόμνημα" } else { "Legend" });
    ui.checkbox(&mut s.points, if el { "Σημεία" } else { "Points" });
    ui.checkbox(&mut s.point_labels, if el { "Ονόματα σημείων" } else { "Point names" });
    ui.checkbox(&mut s.areas, if el { "Περιοχές (κύκλοι)" } else { "Areas (circles)" });
    ui.checkbox(&mut s.area_labels, if el { "Ονόματα περιοχών" } else { "Area names" });
    ui.checkbox(&mut s.shoreline, if el { "Ακτογραμμή σεναρίου" } else { "What-if shoreline" });
    ui.checkbox(&mut s.estimates, if el { "Εκτιμήσεις ανάμεσα στις λήψεις" } else { "Estimates between passes" });
    ui.checkbox(&mut s.grid, if el { "Κάναβος" } else { "Grid" });
    ui.separator();
    ui.checkbox(&mut app.st.timeline_open, if el { "Χρονολόγιο" } else { "Timeline" });
    if format!("{:?}", app.st.show) != before {
        app.save_prefs();
    }
}

// ───────────────────────────────────────────────────────── timeline

pub fn timeline(app: &mut App, ui: &mut egui::Ui) {
    let l = app.st.lang;
    let Some(ds) = app.ds() else { return };
    let Some(layer) = ds.layers.get(app.st.layer) else { return };
    let fr = &layer.frames;
    if fr.is_empty() {
        return;
    }
    egui::Panel::bottom("timeline").exact_size(70.0).show(ui, |ui| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let f = &fr[app.st.frame];
            ui.label(RichText::new(fmt_date(f.date, l, DateStyle::DayMonthYear)).strong());
            let meta = format!(
                "{} {} {} {} · {}{}",
                t("tl.frame", l),
                app.st.frame + 1,
                t("tl.of", l),
                fr.len(),
                layer.info.name.get(l),
                if f.empty { format!(" · {}", t("tl.nodata", l)) } else { String::new() }
            );
            ui.label(RichText::new(meta).color(theme::MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("▾").on_hover_text(t("ui.hide", l)).clicked() {
                    app.toggle_timeline();
                }
                let on = app.st.compare;
                if ui.selectable_label(on, format!("⇹ {}", t("tl.compare", l))).clicked() {
                    app.st.compare = !on;
                    if app.st.compare && app.st.compare_frame == app.st.frame {
                        app.st.compare_frame = 0;
                        if app.st.frame == 0 {
                            app.st.frame = fr.len() - 1;
                        }
                    }
                }
                if app.st.compare {
                    egui::ComboBox::from_id_salt("cmp").selected_text(fmt_date(fr[app.st.compare_frame].date, l, DateStyle::DayMonYear)).show_ui(ui, |ui| {
                        for (i, f) in fr.iter().enumerate() {
                            ui.selectable_value(&mut app.st.compare_frame, i, fmt_date(f.date, l, DateStyle::DayMonYear));
                        }
                    });
                }
            });
        });
        // the track: click or drag anywhere
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click_and_drag());
        let rail = Rect::from_min_max(Pos2::new(rect.left() + 20.0, rect.top() + 8.0), Pos2::new(rect.right() - 20.0, rect.top() + 12.0));
        let n = fr.len();
        let x_of = |i: usize| if n > 1 { rail.left() + rail.width() * i as f32 / (n - 1) as f32 } else { rail.center().x };
        let p = ui.painter();
        p.rect_filled(rail, 2, Color32::from_white_alpha(20));
        p.rect_filled(Rect::from_min_max(rail.left_top(), Pos2::new(x_of(app.st.frame), rail.bottom())), 2, theme::ACCENT);
        let every = n.div_ceil(10).max(1);
        for (i, f) in fr.iter().enumerate() {
            let x = x_of(i);
            let now = i == app.st.frame;
            let col = if f.empty {
                Color32::from_white_alpha(40)
            } else if now {
                theme::ACCENT
            } else if app.st.compare && i == app.st.compare_frame {
                theme::WATER
            } else if i < app.st.frame {
                theme::ACCENT.gamma_multiply(0.7)
            } else {
                Color32::from_white_alpha(90)
            };
            p.circle_filled(Pos2::new(x, rail.center().y), if now { 5.0 } else { 2.5 }, col);
            if i % every == 0 || now {
                p.text(Pos2::new(x, rail.bottom() + 14.0), Align2::CENTER_CENTER, short(f.date, l), FontId::proportional(10.5), if now { theme::TEXT } else { theme::MUTED });
            }
        }
        if let Some(pos) = resp.interact_pointer_pos()
            && (resp.clicked() || resp.dragged())
        {
            let k = ((pos.x - rail.left()) / rail.width() * (n - 1) as f32).round().clamp(0.0, (n - 1) as f32) as usize;
            app.st.frame = k;
        }
    });
}

// ───────────────────────────────────────────────────────── explore: layers + legend

/// Horizontal colour ramp of a legend.
pub fn gradient(ui: &mut egui::Ui, info: &LayerInfo, size: Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    paint_gradient(ui.painter(), rect, info);
}

fn paint_gradient(p: &egui::Painter, rect: Rect, info: &LayerInfo) {
    let stops = info.legend.rgb_stops();
    let (lo, hi) = (stops[0].0, stops[stops.len() - 1].0);
    let mut mesh = egui::Mesh::default();
    for (k, (v, c)) in stops.iter().enumerate() {
        let x = rect.left() + rect.width() * ((v - lo) / (hi - lo)) as f32;
        mesh.colored_vertex(Pos2::new(x, rect.top()), theme::rgb(*c));
        mesh.colored_vertex(Pos2::new(x, rect.bottom()), theme::rgb(*c));
        if k > 0 {
            let b = 2 * k as u32;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b, b + 1);
        }
    }
    p.add(egui::Shape::mesh(mesh));
}

pub fn legend(ui: &mut egui::Ui, info: &LayerInfo, l: ed_core::Lang) {
    ui.label(RichText::new(info.description.get(l)).color(theme::MUTED));
    ui.add_space(4.0);
    gradient(ui, info, Vec2::new(ui.available_width(), 10.0));
    let (lo, hi) = info.legend.range();
    ui.horizontal(|ui| {
        ui.label(RichText::new(fmt_num(lo, 0)).small());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(fmt_num(hi, 0)).small());
            ui.add_space(ui.available_width() / 2.0 - 20.0);
            ui.label(RichText::new(&info.unit).small().color(theme::MUTED));
        });
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new(info.legend.low.get(l)).small().color(theme::MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(info.legend.high.get(l)).small().color(theme::MUTED));
        });
    });
}

/// The legacy strings carry a little HTML (`<b>`, `<em>`); the native UI shows plain text.
pub fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

// ───────────────────────────────────────────────────────── story

pub fn story_left(app: &mut App, ui: &mut egui::Ui) {
    let l = app.st.lang;
    let Some(ds) = app.ds() else { return };
    let n = ds.content.story.len();
    if n == 0 {
        return;
    }
    let i = app.st.story.min(n - 1);
    let ch = &ds.content.story[i];
    ui.horizontal(|ui| {
        for k in 0..n {
            let col = if k < i {
                theme::ACCENT.gamma_multiply(0.6)
            } else if k == i {
                theme::ACCENT
            } else {
                Color32::from_white_alpha(40)
            };
            let (r, resp) = ui.allocate_exact_size(Vec2::new(36.0, 6.0), Sense::click());
            ui.painter().rect_filled(r, 3, col);
            if resp.clicked() {
                app.go_story(k);
            }
        }
    });
    ui.add_space(10.0);
    ui.label(RichText::new(ch.kicker.get(l).to_uppercase()).small().color(theme::ACCENT).strong());
    ui.label(RichText::new(ch.title.get(l)).size(26.0).strong());
    ui.add_space(6.0);
    ui.label(RichText::new(ch.body.get(l)).size(15.0));
    ui.add_space(10.0);
    if let Some(ly) = ds.layers.get(app.st.layer) {
        legend(ui, &ly.info, l);
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(i > 0, egui::Button::new("◀")).clicked() {
            app.go_story(i - 1);
        }
        let label = t(if i == 0 { "story.start" } else if i == n - 1 { "story.finish" } else { "story.next" }, l);
        if ui.add(egui::Button::new(RichText::new(format!("{label}  ▶")).strong()).fill(theme::ACCENT.gamma_multiply(0.35))).clicked() {
            app.go_story(i + 1);
        }
    });
}

// ───────────────────────────────────────────────────────── point inspector

pub fn inspector(app: &mut App, ui: &mut egui::Ui) {
    let l = app.st.lang;
    let Some(d) = &app.data else { return };
    let (ds, status) = (d.ds.clone(), d.status.clone());
    let Some(p) = app.st.point.as_ref().and_then(|id| ds.point(id)).cloned() else { return };
    let s = &p.series;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            chip(ui, t(&format!("kind.{}", p.kind), l), theme::kind(&p.kind));
            ui.heading(p.name.get(l));
            ui.label(RichText::new(format!("{:.3}°N  {:.3}°E", p.lat, p.lon)).small().color(theme::MUTED));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            if ui.button("✕").clicked() {
                app.close_right();
            }
        });
    });
    let total = s.disp_mm[s.disp_mm.len() - 1] - s.disp_mm[0];
    let rate = slope_per_year(&s.dates, &s.disp_mm);
    let hv_range = max(&s.hv_db) - min(&s.hv_db);
    ui.horizontal_wrapped(|ui| {
        tile(ui, t("tile.total", l), &fmt_num(total, 0), "mm", (total < -10.0).then_some(theme::ALERT));
        tile(ui, t("tile.rate", l), &fmt_num(rate, 0), t("unit.mmyr", l), (rate < -10.0).then_some(theme::ALERT));
        tile(ui, t("tile.hv", l), &to_fixed(hv_range, 1), "dB", None);
    });
    // does the independent reference tell the same story here?
    if let Some(r) = &s.ref_mm {
        let diff = rate - slope_per_year(&s.dates, r);
        let ok = diff.abs() <= ds.regions.thresholds.egms_tolerance_mm_yr;
        let txt = fill(t(if ok { "agree.yes" } else { "agree.no" }, l), &[("ref", ds.reference_name())]);
        card(ui, Some(if ok { theme::OK } else { theme::WATCH }), |ui| {
            ui.label(format!("{} {}   Δ {} {}", if ok { "✓" } else { "!" }, strip_tags(&txt), fmt_num(diff, 1), t("unit.mmyr", l)));
        });
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (tab, key) in [(PointTab::Ground, "tab.ground"), (PointTab::Radar, "tab.radar")] {
            if ui.selectable_label(app.st.tab == tab, t(key, l)).clicked() {
                app.st.tab = tab;
                app.sound.stop();
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let playing = app.sound.is_playing();
            if ui.selectable_label(playing, format!("♫ {}", t(if playing { "listen.stop" } else { "listen.play" }, l))).clicked() {
                app.toggle_listen();
            }
        });
    });
    let cursor = app.frame_date();
    if let Some(clicked) = crate::charts::point_chart(ui, &p, app.st.tab, cursor, ds.reference_name(), app.st.show.estimates, l) {
        app.set_frame_to_date(clicked);
    }
    ui.label(RichText::new(t(if app.st.tab == PointTab::Ground { "chart.note.ground" } else { "chart.note.radar" }, l)).small().color(theme::MUTED));
    if let Some(why) = ds.content.explanations.get(&p.kind) {
        card(ui, None, |ui| {
            ui.label(RichText::new(why.title.get(l)).strong());
            ui.label(why.text.get(l));
        });
    }
    if let Some(v) = ds.content.voices.get(&p.id) {
        card(ui, Some(theme::ACCENT), |ui| {
            ui.label(RichText::new(t("voice.tag", l).to_uppercase()).small().color(theme::ACCENT));
            ui.label(RichText::new(v.quote.get(l)).italics());
            ui.label(RichText::new(format!("— {}", v.who.get(l))).small().color(theme::MUTED));
        });
    }
    egui::CollapsingHeader::new(t("unknowns.title", l)).show(ui, |ui| {
        for u in &ds.content.unknowns {
            ui.label(format!("• {}", u.get(l)));
        }
    });
    let _ = status;
}

// ───────────────────────────────────────────────────────── the map

fn optical_for(app: &App, ds: &ed_core::model::Dataset, frame: &Frame) -> Option<usize> {
    let _ = app;
    (!ds.optical.is_empty()).then(|| nearest(ds.optical.iter().map(|o| o.date), frame.date))
}

pub fn map_view(app: &mut App, ui: &mut egui::Ui) {
    let l = app.st.lang;
    let Some(d) = &app.data else { return };
    let (ds, status) = (d.ds.clone(), d.status.clone());
    let wx = d.wx.clone();
    let rect = ui.max_rect();
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    app.map.interact(ui, rect, &resp);
    let ctx = ui.ctx().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0, theme::BG);
    // the satellite basemap (slightly dimmed so the data on top reads clearly)
    let base = app.basemap();
    if let Some(b) = base {
        app.basemaps.draw(&ctx, &painter, &app.map, rect, b, Color32::from_gray(205));
    }
    if app.st.show.grid || base.is_none() {
        app.map.graticule(&painter, rect);
    }
    if base.is_none() {
        let site = app.map.bounds_rect(rect, ds.site.bounds);
        painter.rect_stroke(site, 0, Stroke::new(1.0, Color32::from_white_alpha(18)), egui::StrokeKind::Middle);
    }

    // ── imagery: the current frame, and with compare the other frame left of the swipe line
    let layer = ds.layers.get(app.st.layer);
    let swipe_x = rect.left() + rect.width() * app.st.swipe;
    let draw = |app: &mut App, frame: &Frame, clip: Rect| {
        let p = painter.with_clip_rect(clip);
        let full = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        match app.st.sensor {
            // demo clouds: generated over the whole view, a new sky for each date, as cloudy as the date says
            Sensor::Optical if ds.demo => {
                let Some(i) = optical_for(app, &ds, frame) else { return };
                let o = &ds.optical[i];
                let seed = { use std::hash::{Hash, Hasher}; let mut h = std::hash::DefaultHasher::new(); o.date.hash(&mut h); h.finish() };
                let b = ds.site.bounds;
                let ((x0, y0), (x1, y1)) = (crate::map::merc(b.west(), b.north()), crate::map::merc(b.east(), b.south()));
                app.clouds.draw(&ctx, &p, &app.map, rect, crate::clouds::Sky { seed, cover: o.cover.unwrap_or(50.0), site: [x0, y0, x1, y1] });
            }
            Sensor::Optical => {
                let Some(i) = optical_for(app, &ds, frame) else { return };
                let img = ds.optical[i].image.clone();
                let tex = app.texture(&ctx, &img, crate::app::Edge::Feather);
                p.image(tex, app.map.bounds_rect(rect, img.bounds), full, Color32::WHITE);
            }
            Sensor::Radar => {
                let Some(img) = frame.image.clone().filter(|_| !frame.empty) else { return };
                // data layers sit on the imagery at the chosen opacity, fading out in a soft oval
                let tex = app.texture(&ctx, &img, crate::app::Edge::Organic);
                let a = if base.is_some() { app.st.show.opacity } else { 1.0 };
                // demo data is made up anyway: wash the whole view in the layer's own average colour, so
                // the layer fades into a matching tint instead of into the imagery
                if ds.demo {
                    let [r, g, b] = mean_colour(&img.px);
                    p.rect_filled(clip, 0, Color32::from_rgba_unmultiplied(r, g, b, (a * 200.0) as u8));
                }
                p.image(tex, app.map.bounds_rect(rect, img.bounds), full, Color32::from_white_alpha((a * 255.0) as u8));
            }
        }
    };
    if let Some(ly) = layer
        && let Some(f) = ly.frames.get(app.st.frame)
    {
        if app.st.compare {
            let right = Rect::from_min_max(Pos2::new(swipe_x, rect.top()), rect.max);
            let left = Rect::from_min_max(rect.min, Pos2::new(swipe_x, rect.bottom()));
            draw(app, f, right);
            if let Some(cf) = ly.frames.get(app.st.compare_frame) {
                draw(app, cf, left);
            }
        } else {
            draw(app, f, rect);
        }
    }

    // ── place names over the imagery
    if app.st.show.labels && base.is_some() {
        app.basemaps.draw(&ctx, &painter, &app.map, rect, ed_sources::tiles::Basemap::EsriLabels, Color32::from_white_alpha(225));
    }

    // ── Status: areas (drawn on the demo set; on the real set the lake picture is enough) and the what-if shoreline
    let mut hovered_region = None;
    if app.st.mode == Mode::Status {
        // names are placed right, left, above or below their dot, wherever they do not cover another name;
        // the selected area is placed first, and a name with no free spot is left out (the dot stays)
        let mut placed: Vec<Rect> = Vec::new();
        let mut order: Vec<&ed_core::model::Region> = ds.regions.regions.iter().collect();
        order.sort_by_key(|r| app.st.region.as_deref() != Some(&r.id));
        for r in order {
            let a = status.get(&r.id);
            let col = theme::level(a.status);
            let sel = app.st.region.as_deref() == Some(&r.id);
            let c = app.map.to_screen(rect, r.center[0], r.center[1]);
            let rad = (r.radius_km / app.map.km_per_px()) as f32;
            if app.st.show.areas || sel {
                let pts: Vec<Pos2> = ring(r.center, r.radius_km).iter().map(|q| app.map.to_screen(rect, q[0], q[1])).collect();
                painter.add(star_fill(&pts[..pts.len() - 1], col.gamma_multiply(if sel { 0.16 } else { 0.07 })));
                painter.add(egui::Shape::line(pts, Stroke::new(if sel { 1.6 } else { 1.0 }, col.gamma_multiply(0.8))));
            }
            if resp.hover_pos().is_some_and(|h| h.distance(c) <= rad) {
                hovered_region = Some(r.id.clone());
            }
            // a small status dot + name at the centre, instead of the big tag
            if app.st.show.area_labels || sel || hovered_region.as_deref() == Some(&r.id) {
                painter.circle_filled(c, 4.0, col);
                let g = painter.layout_no_wrap(r.name.get(l).to_string(), FontId::proportional(11.0), theme::TEXT);
                let size = g.size() + Vec2::new(8.0, 4.0);
                let spots = [
                    c + Vec2::new(8.0, -size.y / 2.0),
                    c + Vec2::new(-8.0 - size.x, -size.y / 2.0),
                    c + Vec2::new(-size.x / 2.0, -8.0 - size.y),
                    c + Vec2::new(-size.x / 2.0, 8.0),
                ];
                let free = spots.iter().map(|p| Rect::from_min_size(*p, size)).find(|t| placed.iter().all(|o| !o.expand(2.0).intersects(*t)));
                let Some(tag) = free.or_else(|| (sel || hovered_region.as_deref() == Some(&r.id)).then(|| Rect::from_min_size(spots[0], size))) else { continue };
                placed.push(tag);
                painter.rect_filled(tag, 4, Color32::from_rgba_unmultiplied(11, 14, 19, 200));
                painter.galley(tag.min + Vec2::new(4.0, 2.0), g, if sel { theme::ACCENT } else { theme::TEXT });
                if resp.hover_pos().is_some_and(|h| tag.contains(h)) {
                    hovered_region = Some(r.id.clone());
                }
            }
        }
        if let Some((rid, res)) = crate::status_ui::what_if(app, &ds, &status, wx.as_deref())
            && app.st.what_if.is_change()
            && app.st.show.shoreline
            && let Some(outline) = ds.region(&rid).and_then(|r| r.reservoir.as_ref()).and_then(|x| x.outline.as_ref())
            && outline.len() > 3
        {
            let n = outline.len() - 1;
            let cx = outline[..n].iter().map(|p| p[0]).sum::<f64>() / n as f64;
            let cy = outline[..n].iter().map(|p| p[1]).sum::<f64>() / n as f64;
            let sc = res.scale();
            let now: Vec<Pos2> = outline.iter().map(|q| app.map.to_screen(rect, q[0], q[1])).collect();
            let scn: Vec<Pos2> = outline[..n].iter().map(|q| app.map.to_screen(rect, cx + (q[0] - cx) * sc, cy + (q[1] - cy) * sc)).collect();
            painter.add(star_fill(&scn, theme::WATER.gamma_multiply(0.28)));
            let mut closed = scn.clone();
            closed.push(scn[0]);
            painter.add(egui::Shape::line(closed, Stroke::new(2.5, theme::WATER)));
            painter.extend(dashed(&now, Stroke::new(1.5, Color32::WHITE), 6.0, 4.0));
        }
    }

    // ── points
    let mut hovered_point = None;
    if app.st.mode != Mode::Status && app.st.show.points {
        let beat = app.sound.is_playing();
        for p in &ds.points {
            let c = app.map.to_screen(rect, p.lon, p.lat);
            if !rect.contains(c) {
                continue;
            }
            let sel = app.st.point.as_deref() == Some(&p.id);
            let hov = resp.hover_pos().is_some_and(|h| h.distance(c) <= 8.0);
            let col = theme::kind(&p.kind);
            let rad = if sel { 5.5 } else { 4.0 } + if sel && beat { 2.0 * ((ui.input(|i| i.time) * 6.0).sin().abs() as f32) } else { 0.0 };
            painter.circle_filled(c, rad + 1.5, Color32::from_rgba_unmultiplied(11, 14, 19, 200));
            painter.circle_filled(c, rad, col);
            if sel {
                painter.circle_stroke(c, rad + 3.0, Stroke::new(1.2, theme::TEXT));
            }
            if app.st.show.point_labels || sel || hov {
                let g = painter.layout_no_wrap(p.name.get(l).to_string(), FontId::proportional(11.5), theme::TEXT);
                let lr = Rect::from_min_size(c + Vec2::new(rad + 4.0, -g.size().y / 2.0 - 2.0), g.size() + Vec2::new(8.0, 4.0));
                painter.rect_filled(lr, 4, Color32::from_rgba_unmultiplied(11, 14, 19, 210));
                painter.galley(lr.min + Vec2::new(4.0, 2.0), g, theme::TEXT);
            }
            if hov {
                hovered_point = Some(p.id.clone());
            }
        }
    }
    if hovered_point.is_some() || hovered_region.is_some() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if resp.clicked() {
        if let Some(id) = hovered_point {
            app.select_point(&id, true);
        } else if let Some(id) = hovered_region {
            app.select_region(&id, true);
        }
    }

    // ── swipe handle
    if app.st.compare {
        painter.line_segment([Pos2::new(swipe_x, rect.top()), Pos2::new(swipe_x, rect.bottom())], Stroke::new(2.0, Color32::WHITE));
        let h = Rect::from_center_size(Pos2::new(swipe_x, rect.center().y), Vec2::splat(36.0));
        let hr = ui.interact(h, ui.scope_id().with("swipe"), Sense::drag());
        painter.circle_filled(h.center(), 18.0, Color32::WHITE);
        painter.text(h.center(), Align2::CENTER_CENTER, "⇔", FontId::proportional(18.0), Color32::BLACK);
        if hr.dragged() {
            app.st.swipe = ((swipe_x + hr.drag_delta().x - rect.left()) / rect.width()).clamp(0.05, 0.95);
        }
        if let Some(ly) = layer {
            let tag = |x: f32, s: String, a: Align2| {
                let g = painter.layout_no_wrap(s, FontId::proportional(12.0), theme::TEXT);
                let r = a.anchor_size(Pos2::new(x, rect.top() + 60.0), g.size() + Vec2::new(14.0, 6.0));
                painter.rect_filled(r, 8, Color32::from_rgba_unmultiplied(13, 18, 27, 220));
                painter.galley(r.center() - g.size() / 2.0, g, theme::TEXT);
            };
            tag(swipe_x - 10.0, fmt_date(ly.frames[app.st.compare_frame].date, l, DateStyle::DayMonYear), Align2::RIGHT_TOP);
            tag(swipe_x + 10.0, fmt_date(ly.frames[app.st.frame].date, l, DateStyle::DayMonYear), Align2::LEFT_TOP);
        }
    }

    // ── hover probe: the value under the cursor and what it means
    if app.st.sensor == Sensor::Radar
        && let (Some(h), Some(ly)) = (resp.hover_pos(), layer)
        && let Some(f) = ly.frames.get(app.st.frame)
        && let Some(vals) = &f.values
    {
        let (lon, lat) = app.map.to_geo(rect, h);
        if let Some(v) = vals.sample(lon, lat) {
            let v = f64::from(v);
            let info = &ly.info;
            let sub = if info.id == "sm" { t("probe.sm", l).to_string() } else { format!("{}-band · {}", info.band, info.short.get(l)) };
            let cls = info.class_of(v).map(|c| c.get(l).to_string());
            let r = Rect::from_min_size(h + Vec2::new(16.0, 16.0), Vec2::new(250.0, if cls.is_some() { 84.0 } else { 46.0 }));
            painter.rect_filled(r, 10, Color32::from_rgba_unmultiplied(13, 18, 27, 240));
            painter.rect_stroke(r, 10, Stroke::new(1.0, Color32::from_white_alpha(40)), egui::StrokeKind::Middle);
            painter.text(r.left_top() + Vec2::new(10.0, 8.0), Align2::LEFT_TOP, format!("{} {}", fmt_num(v, 1), info.unit), FontId::proportional(17.0), theme::TEXT);
            painter.text(r.left_top() + Vec2::new(10.0, 30.0), Align2::LEFT_TOP, sub, FontId::proportional(11.0), theme::MUTED);
            if let Some(c) = cls {
                let g = painter.layout(c, FontId::proportional(11.5), theme::TEXT, 230.0);
                painter.galley(r.left_top() + Vec2::new(10.0, 48.0), g, theme::TEXT);
            }
        }
    }

    if let Some(b) = base {
        let mut text = b.attribution().to_string();
        if app.st.show.labels {
            text.push_str(" · Labels © Esri");
        }
        let loading = app.basemaps.loading();
        if loading > 0 {
            text = format!("⟳ {loading} · {text}");
        }
        let g = painter.layout_no_wrap(text, FontId::proportional(9.5), Color32::from_white_alpha(170));
        let r = Rect::from_min_size(rect.right_bottom() - g.size() - Vec2::new(12.0, 6.0), g.size() + Vec2::new(8.0, 4.0));
        painter.rect_filled(r, 3, Color32::from_rgba_unmultiplied(11, 14, 19, 170));
        painter.galley(r.min + Vec2::new(4.0, 2.0), g, Color32::WHITE);
    }
    overlays(app, ui, rect, &ds, &status);
}

/// Sensor switch + note (top centre), map tools (right), hint (bottom), collapsed-panel pills.
fn overlays(app: &mut App, ui: &mut egui::Ui, rect: Rect, ds: &ed_core::model::Dataset, status: &ed_core::status::Status) {
    let l = app.st.lang;
    let bar = Rect::from_center_size(Pos2::new(rect.center().x, rect.top() + 34.0), Vec2::new(520.0, 52.0));
    ui.scope_builder(UiBuilder::new().max_rect(bar), |ui| {
        ui.vertical_centered(|ui| {
            egui::Frame::new().fill(Color32::from_rgba_unmultiplied(13, 18, 27, 235)).corner_radius(18).inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    // the second switch is the radar on NISAR data, the water index on optical-only (live) data
                    let radar = ds.layers.get(app.st.layer).is_none_or(|ly| ly.info.band == "L" || ly.info.band == "C");
                    let second = if radar { t("sensor.radar", l).to_string() } else { ds.layers.get(app.st.layer).map_or(String::new(), |ly| ly.info.name.get(l).to_string()) };
                    for (s, label, icon) in [(Sensor::Optical, t("sensor.optical", l).to_string(), "☁"), (Sensor::Radar, second, if radar { "◎" } else { "≈" })] {
                        if ui.selectable_label(app.st.sensor == s, format!("{icon} {label}")).clicked() {
                            app.st.sensor = s;
                        }
                    }
                });
            });
            let note = sensor_note(app, ds);
            let empty = app.st.sensor == Sensor::Radar && ds.layers.get(app.st.layer).and_then(|ly| ly.frames.get(app.st.frame)).is_some_and(|f| f.empty);
            if !note.is_empty() {
                // a backing exactly as wide as the text
                let g = ui.painter().layout_no_wrap(note, FontId::proportional(11.0), if empty { theme::WATCH } else { theme::TEXT });
                let (r, _) = ui.allocate_exact_size(g.size() + Vec2::new(16.0, 4.0), Sense::hover());
                ui.painter().rect_filled(r, 6, Color32::from_rgba_unmultiplied(11, 14, 19, 210));
                ui.painter().galley(r.min + Vec2::new(8.0, 2.0), g, theme::TEXT);
            }
        });
    });

    // map tools, painted by hand so they stay readable over any imagery
    let mut y = rect.top() + 90.0;
    let mut tool = |ui: &mut egui::Ui, id: &str, glyph: &str, color: Color32, on: bool, hint: &str| -> bool {
        let r = Rect::from_min_size(Pos2::new(rect.right() - 52.0, y), Vec2::new(40.0, 34.0));
        y += 40.0;
        let resp = ui.interact(r, ui.scope_id().with(id), Sense::click()).on_hover_text(hint);
        let fill = if on { theme::ACCENT.gamma_multiply(0.35) } else if resp.hovered() { Color32::from_rgb(30, 38, 52) } else { Color32::from_rgba_unmultiplied(13, 18, 27, 240) };
        ui.painter().rect_filled(r, 10, fill);
        ui.painter().rect_stroke(r, 10, Stroke::new(1.0, Color32::from_white_alpha(30)), egui::StrokeKind::Inside);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(17.0), color);
        resp.clicked()
    };
    if tool(ui, "zin", "+", theme::TEXT, false, "Zoom in") {
        app.map.zoom_by(1.0);
    }
    if tool(ui, "zout", "−", theme::TEXT, false, "Zoom out") {
        app.map.zoom_by(-1.0);
    }
    if tool(ui, "home", "⌂", theme::TEXT, false, "Home") {
        app.home();
    }
    // the lake alert opens only when asked for (Status, with a lake forecast)
    if app.st.mode == Mode::Status
        && let Some(lake) = status.lake_id(ds.forecast.as_ref())
    {
        let lv = ed_core::status::lake_level(status, ds.forecast.as_ref()).unwrap_or(Level::Watch);
        let on = app.st.region.as_deref() == Some(&lake);
        if tool(ui, "alert", "⚠", theme::level(lv), on, t("al.open", l)) {
            if on {
                app.close_right();
            } else {
                app.select_region(&lake, true);
            }
        }
    }

    // a compact legend, bottom left
    if app.st.show.legend
        && app.st.mode != Mode::Status
        && app.st.sensor == Sensor::Radar
        && let Some(ly) = ds.layers.get(app.st.layer)
    {
        let r = Rect::from_min_size(Pos2::new(rect.left() + 12.0, rect.bottom() - 92.0), Vec2::new(250.0, 80.0));
        ui.scope_builder(UiBuilder::new().max_rect(r), |ui| {
            egui::Frame::new().fill(Color32::from_rgba_unmultiplied(17, 21, 28, 235)).corner_radius(8).inner_margin(8).show(ui, |ui| {
                ui.set_width(234.0);
                ui.label(RichText::new(format!("{} · {}", ly.info.name.get(l), ly.info.unit)).small().strong());
                gradient(ui, &ly.info, Vec2::new(234.0, 7.0));
                let (lo, hi) = ly.info.legend.range();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} {}", fmt_num(lo, 0), ly.info.legend.low.get(l))).small().color(theme::MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format!("{} {}", ly.info.legend.high.get(l), fmt_num(hi, 0))).small().color(theme::MUTED));
                    });
                });
            })
            .response
            .on_hover_text(ly.info.description.get(l));
        });
    }
    // collapsed panels come back from small pills
    if !app.st.left_open && app.st.mode != Mode::Explore {
        let pill = Rect::from_min_size(rect.left_top() + Vec2::new(12.0, 12.0), Vec2::new(200.0, 30.0));
        let label = match app.st.mode {
            Mode::Story => ds.content.story.get(app.st.story).map_or(String::new(), |c| c.title.get(l).to_string()),
            Mode::Status => t("brief.kicker", l).to_string(),
            Mode::Explore => String::new(),
        };
        let button = egui::Button::new(RichText::new(format!("▸ {label}")).color(theme::TEXT)).fill(theme::PANEL).stroke(Stroke::new(1.0, Color32::from_white_alpha(28))).corner_radius(15);
        if ui.put(pill, button).on_hover_text(t("ui.show", l)).clicked() {
            app.toggle_left();
        }
    }
    if !app.st.timeline_open && app.st.mode != Mode::Status {
        let pill = Rect::from_center_size(Pos2::new(rect.center().x, rect.bottom() - 24.0), Vec2::new(140.0, 28.0));
        if ui.put(pill, egui::Button::new(format!("▴ {}", app.frame_date().map_or(String::new(), |d| short(d, l))))).clicked() {
            app.toggle_timeline();
        }
    } else if app.st.mode == Mode::Explore && app.st.show.points && !ds.points.is_empty() && app.st.point.is_none() && !app.st.show.legend {
        let g = egui::RichText::new(format!("● {}", t("hint.click", l))).small();
        let r = Rect::from_center_size(Pos2::new(rect.center().x, rect.bottom() - 22.0), Vec2::new(340.0, 24.0));
        ui.put(r, egui::Label::new(g));
    }
}

fn sensor_note(app: &App, ds: &ed_core::model::Dataset) -> String {
    let l = app.st.lang;
    let Some(ly) = ds.layers.get(app.st.layer) else { return String::new() };
    let Some(f) = ly.frames.get(app.st.frame) else { return String::new() };
    match app.st.sensor {
        Sensor::Optical => {
            let Some(i) = optical_for(app, ds, f) else { return String::new() };
            let c = &ds.optical[i];
            if let Some(cover) = c.cover {
                format!("{} · {}", fill(t("sensor.cover", l), &[("n", &ed_core::text::js_number(cover))]), short(c.date, l))
            } else {
                let gap = c.date.since(f.date).abs();
                let key = if gap > 0 { "sensor.s2gap" } else { "sensor.s2" };
                fill(t(key, l), &[("n", &gap.to_string()), ("d", &short(c.date, l))])
            }
        }
        Sensor::Radar if f.empty => format!("{} · {}", t("sensor.nodata", l), short(f.date, l)),
        Sensor::Radar if ly.info.band == "L" || ly.info.band == "C" => format!("NISAR L-band · {} · {}", ly.info.name.get(l), t("sensor.through", l)),
        Sensor::Radar => format!("{} {} · {}", ly.info.sensor, ly.info.product, if l == ed_core::Lang::El { "τα σύννεφα αφαιρέθηκαν (γκρι)" } else { "clouds removed (grey)" }),
    }
}


/// Average colour of the opaque pixels of an RGBA image (sampled; for tinting).
fn mean_colour(px: &[[u8; 4]]) -> [u8; 3] {
    let (mut sum, mut n) = ([0u64; 3], 0u64);
    for px in px.iter().step_by(7) {
        if px[3] > 200 {
            for c in 0..3 {
                sum[c] += u64::from(px[c]);
            }
            n += 1;
        }
    }
    if n == 0 { [128; 3] } else { sum.map(|v| (v / n) as u8) }
}
