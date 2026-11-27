//! The Status view: the daily briefing (left), one area's card (right), "Ask EarthDance" and the what-if.

use eframe::egui::{self, Color32, RichText};
use ed_core::ask::{self, Question};
use ed_core::insights::{EventKind, Finding};
use ed_core::model::{Dataset, WeatherSet};
use ed_core::status::{self, Status, WhatIfResult};
use ed_core::text::{fd, fmt_date, signed, t, tf, to_fixed, DateStyle, Lang};
use ed_core::{Date, Level};

use crate::app::{card, chip, section, short, App, RegionTab, WxState};
use crate::panels::strip_tags;
use crate::theme;

fn level_key(l: Level) -> String {
    format!("st.{}", l.key())
}

/// (status identity, sliders, result)
type WhatIfCache = Option<(usize, ed_core::insights::WhatIf, Option<(String, WhatIfResult)>)>;

thread_local! {
    static WHAT_IF: std::cell::RefCell<WhatIfCache> = const { std::cell::RefCell::new(None) };
}

/// The what-if for the current sliders, recomputed only when they (or the status) change.
pub fn what_if(app: &App, ds: &Dataset, st: &std::sync::Arc<Status>, wx: Option<&WeatherSet>) -> Option<(String, WhatIfResult)> {
    let key = std::sync::Arc::as_ptr(st) as usize;
    let w = app.st.what_if;
    WHAT_IF.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((k, cw, r)) = c.as_ref()
            && *k == key
            && *cw == w
        {
            return r.clone();
        }
        let r = status::what_if(ds, st, wx, w);
        *c = Some((key, w, r.clone()));
        r
    })
}

// ───────────────────────────────────────────────────────── left: the daily briefing

pub fn brief(app: &mut App, ui: &mut egui::Ui) {
    let l = app.st.lang;
    let Some(d) = &app.data else { return };
    let (ds, st, wx, wx_state, today) = (d.ds.clone(), d.status.clone(), d.wx.clone(), d.wx_state, d.today);
    let [ok, watch, alert] = st.counts();
    let n = |v: usize| v.to_string();
    let title = if alert > 0 {
        tf(if alert == 1 { "brief.h.alert1" } else { "brief.h.alertN" }, l, &[("n", &n(alert))])
    } else if watch > 0 {
        tf(if watch == 1 { "brief.h.watch1" } else { "brief.h.watchN" }, l, &[("n", &n(watch))])
    } else {
        t("brief.h.ok", l).to_string()
    };
    let last_pass = ds.regions.regions.iter().filter_map(|r| r.series.dates.last()).max().copied();

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} · {}", t("brief.kicker", l), fmt_date(today, l, DateStyle::WeekdayDayMonth))).small().color(theme::ACCENT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("◂").on_hover_text(t("ui.hide", l)).clicked() {
                app.toggle_left();
            }
            if ui.small_button(format!("⬇ {}", t("report.btn", l))).clicked() {
                let wi = what_if(app, &ds, &st, wx.as_deref());
                match crate::report::export(&ds, &st, wx.as_deref(), wx_state, today, l, app.st.what_if, wi.map(|x| x.1)) {
                    Ok(p) => {
                        let _ = std::process::Command::new("xdg-open").arg(&p).spawn();
                    }
                    Err(e) => eprintln!("report: {e}"),
                }
            }
        });
    });
    ui.label(RichText::new(strip_tags(&title)).size(24.0).strong());
    ui.horizontal(|ui| {
        for (lv, c) in [(Level::Alert, alert), (Level::Watch, watch), (Level::Ok, ok)] {
            chip(ui, format!("{c} {}", t(&level_key(lv), l)), theme::level(lv));
        }
    });
    ui.add_space(4.0);
    egui::Grid::new("fresh").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
        ui.label(RichText::new(t("brief.radar", l)).small().color(theme::MUTED));
        let radar = last_pass.map_or(String::new(), |lp| tf("brief.radarText", l, &[("d", &fd(Some(lp), l)), ("n", &today.since(lp).to_string())]));
        ui.label(RichText::new(format!("{}{}", strip_tags(&radar), if ds.demo { format!(" · {}", t("demo.short", l)) } else { String::new() })).small());
        ui.end_row();
        ui.label(RichText::new(t("brief.wx", l)).small().color(theme::MUTED));
        let wxl = if wx_state == WxState::Snapshot {
            tf("brief.wx.snapshot", l, &[("d", &fd(wx.as_ref().and_then(|w| w.saved), l))])
        } else {
            t(&format!("brief.wx.{}", wx_state.key()), l).to_string()
        };
        ui.label(RichText::new(strip_tags(&wxl)).small());
        ui.end_row();
    });

    section(ui, t("brief.todo", l));
    let mut todo: Vec<(&ed_core::model::Region, &Finding)> = Vec::new();
    for r in st.sorted(&ds.regions) {
        for f in &st.get(&r.id).findings {
            if f.level != Level::Ok {
                todo.push((r, f));
            }
        }
    }
    todo.sort_by_key(|(_, f)| std::cmp::Reverse(f.level));
    if todo.is_empty() {
        ui.label(RichText::new(t("brief.none", l)).color(theme::MUTED));
    }
    for (r, f) in todo {
        let resp = card(ui, Some(theme::level(f.level)), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("●").color(theme::level(f.level)));
                ui.vertical(|ui| {
                    ui.label(RichText::new(f.title.get(l)).strong());
                    ui.label(RichText::new(r.name.get(l)).small().color(theme::MUTED));
                });
            });
            ui.interact(ui.min_rect(), ui.scope_id().with(("todo", &r.id, f.key)), egui::Sense::click())
        });
        if resp.clicked() {
            app.select_region(&r.id, true);
        }
    }

    if st.brief.is_some() {
        ask_box(app, ui, &st, l);
    }

    section(ui, t("brief.areas", l));
    for r in st.sorted(&ds.regions) {
        let a = st.get(&r.id);
        let sel = app.st.region.as_deref() == Some(&r.id);
        let resp = ui
            .horizontal(|ui| {
                ui.label(RichText::new("●").color(theme::level(a.status)));
                ui.label(RichText::new(r.name.get(l)).strong().color(if sel { theme::ACCENT } else { theme::TEXT }));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(w) = &a.weather {
                        ui.label(RichText::new(format!("{} mm", to_fixed(w.rain7, 0))).small().color(theme::MUTED)).on_hover_text(t("rg.rain7", l));
                    }
                    if let Some(rf) = &a.reference {
                        ui.label(RichText::new(format!("{} {}", ds.reference_name(), if rf.agrees { "✓" } else { "✗" })).small().color(if rf.agrees { theme::OK } else { theme::WATCH }));
                    }
                    if let Some(rate) = a.ground.rate {
                        ui.label(RichText::new(format!("{} {}", signed(rate, 0), t("unit.mmyr", l))).small()).on_hover_text(t("rg.tile.rate", l));
                    }
                });
            })
            .response
            .interact(egui::Sense::click());
        if resp.clicked() {
            app.select_region(&r.id, true);
        }
    }
    ui.label(RichText::new(t("brief.foot", l)).small().color(theme::MUTED));

    scenario_box(app, ui, &ds, &st, wx.as_deref(), today, l);
}

// ───────────────────────────────────────────────────────── ask

fn ask_box(app: &mut App, ui: &mut egui::Ui, st: &Status, l: Lang) {
    let Some(b) = &st.brief else { return };
    section(ui, t("ask.title", l));
    card(ui, None, |ui| {
        ui.horizontal_wrapped(|ui| {
            for q in Question::LISTED {
                let on = app.st.asked.as_ref().is_some_and(|a| a.0 == q);
                if ui.selectable_label(on, q.label(l)).clicked() {
                    app.ask(q, q.label(l).to_string());
                }
            }
        });
        ui.horizontal(|ui| {
            let edit = ui.add(egui::TextEdit::singleline(&mut app.st.ask_input).hint_text(t("ask.ph", l)).desired_width(ui.available_width() - 40.0));
            let send = ui.button("→").on_hover_text(t("ask.send", l)).clicked();
            if (send || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))) && !app.st.ask_input.trim().is_empty() {
                let text = app.st.ask_input.trim().to_string();
                app.ask(ask::route(&text).unwrap_or(Question::None), text);
                app.st.ask_input.clear();
            }
        });
        if let Some((q, text, t0)) = &app.st.asked {
            let a = ask::answer(*q, b, l);
            ui.label(RichText::new(text).strong().color(theme::ACCENT));
            // revealed word by word: the answer is ready at once, this only makes it readable on video
            let elapsed = t0.elapsed().as_secs_f32();
            if elapsed < 0.45 {
                ui.label(RichText::new(t("ask.thinking", l)).italics().color(theme::MUTED));
            } else {
                let mut budget = ((elapsed - 0.45) / 0.028 * 3.0) as usize;
                for p in &a.text {
                    let words: Vec<&str> = p.split(' ').collect();
                    let k = budget.min(words.len());
                    ui.label(words[..k].join(" "));
                    budget -= k;
                    if budget == 0 {
                        break;
                    }
                }
                if budget > 0 {
                    ui.label(RichText::new(&a.sources).small().color(theme::MUTED));
                }
            }
        }
        ui.label(RichText::new(t("ask.note", l)).small().color(theme::MUTED));
    });
}

// ───────────────────────────────────────────────────────── what if

fn scenario_box(app: &mut App, ui: &mut egui::Ui, ds: &Dataset, st: &std::sync::Arc<Status>, wx: Option<&WeatherSet>, today: Date, l: Lang) {
    let Some(r) = status::lake_region(&ds.regions) else { return };
    let res = r.reservoir.as_ref().expect("lake");
    let model = st.brief.is_some() && ds.forecast.as_ref().is_some_and(|f| f.region == r.id);
    section(ui, t("scn.title", l));
    card(ui, None, |ui| {
        ui.label(strip_tags(&tf(if model { "scn.q.model" } else { "scn.q" }, l, &[("lake", res.name.get(l))])));
        let pct = |v: f64| format!("{}{}%", if v > 0.0 { "+" } else { "" }, v).replacen('-', "−", 1);
        ui.horizontal(|ui| {
            ui.label(t("scn.rain", l));
            ui.add(egui::Slider::new(&mut app.st.what_if.rain, -50.0..=30.0).step_by(5.0).show_value(false));
            ui.label(pct(app.st.what_if.rain));
        });
        ui.horizontal(|ui| {
            ui.label(t("scn.irr", l));
            ui.add(egui::Slider::new(&mut app.st.what_if.irrigation, -50.0..=50.0).step_by(5.0).show_value(false));
            ui.label(pct(app.st.what_if.irrigation));
        });
        if let Some((_, s)) = what_if(app, ds, st, wx) {
            let bad = s.low() < s.limit();
            let kpi = |ui: &mut egui::Ui, label: &str, val: String, sub: String, badc: bool| {
                ui.add_space(2.0);
                ui.label(RichText::new(label).small().color(theme::MUTED));
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(val).size(16.0).strong().color(if badc { theme::ALERT } else { theme::OK }));
                    ui.label(RichText::new(sub).small().color(theme::MUTED));
                });
            };
            let nd = |d: Option<Date>| d.map_or_else(|| "–".to_string(), |d| fmt_date(d, l, DateStyle::DayMonYear));
            match &s {
                WhatIfResult::Model(m) => {
                    let b = &st.brief.as_ref().expect("model").base;
                    let sep_base = b.dates.iter().position(|d| *d > today && d.md() == (6, 1)).map(|i| b.p50[i]);
                    kpi(ui, t("scn.next", l), nd(m.next), format!("{} {}", t("scn.base", l), nd(b.next.map(|n| n.mid))), m.next.is_some());
                    kpi(ui, t("scn.sep", l), format!("{} km²", to_fixed(m.low, 0)), format!("{} {} · {} {}", t("rc.limit", l), ed_core::text::js_number(m.limit), t("scn.base", l), sep_base.map_or("–".into(), |v| to_fixed(v, 0))), bad);
                    kpi(ui, t("scn.days", l), m.days_below.to_string(), format!("{} · {} {}", t(if m.days_below > 0 { "scn.fine" } else { "scn.nofine" }, l), t("scn.base", l), b.days_below_next), m.days_below > 0);
                }
                WhatIfResult::Simple(x) => {
                    kpi(ui, t("scn.low", l), format!("{} {}", to_fixed(x.low, 0), x.unit), format!("{} {} · {} {}", t("rc.limit", l), ed_core::text::js_number(x.limit), t("scn.same", l), to_fixed(x.base, 0)), bad);
                    if let (Some(a), Some(n)) = (x.area, x.area_now) {
                        kpi(ui, t("scn.area", l), format!("{} km²", to_fixed(a, 0)), format!("{} {}", t("scn.now", l), to_fixed(n, 0)), false);
                    }
                    kpi(ui, t("scn.days", l), ed_core::text::js_number(x.days_below), t(if x.days_below > 0.0 { "scn.fine" } else { "scn.nofine" }, l).to_string(), x.days_below > 0.0);
                    if let Some(sink) = x.sink {
                        kpi(ui, &strip_tags(&tf("scn.sink", l, &[("area", r.name.get(l))])), format!("{} {}", signed(sink, 0), t("unit.mmyr", l)), String::new(), false);
                    }
                }
            }
        }
        if ui.small_button(t("scn.reset", l)).clicked() {
            app.st.what_if = Default::default();
        }
        let note = format!("{}{}", t(if model { "scn.note.model" } else { "scn.note" }, l), if ds.demo { format!(" {}", t("demo.warn", l)) } else { String::new() });
        ui.label(RichText::new(note).small().color(theme::MUTED));
    });
}

// ───────────────────────────────────────────────────────── right: one area

fn alert_card(ui: &mut egui::Ui, ds: &Dataset, st: &Status, r: &ed_core::model::Region, l: Lang) {
    let a = st.get(&r.id);
    let Some(rs) = &a.reservoir else { return };
    let res = r.reservoir.as_ref().expect("reservoir");
    let is_model = st.brief.is_some() && ds.forecast.as_ref().is_some_and(|f| f.region == r.id);
    if !is_model && rs.level == Level::Ok {
        return;
    }
    let lv = if is_model && rs.level == Level::Ok { Level::Watch } else { rs.level };
    card(ui, Some(theme::level(lv)), |ui| {
        ui.label(RichText::new(t("al.title", l).to_uppercase()).small().color(theme::level(lv)).strong());
        ui.label(RichText::new(rs.finding.title.get(l)).size(16.0).strong());
        if let (true, Some(b)) = (is_model, &st.brief) {
            let lines = ask::lines(b, l);
            for (k, line) in ["al3.seen", "al3.coming", "al3.do"].iter().zip(lines) {
                ui.label(RichText::new(t(k, l)).small().color(theme::MUTED));
                ui.label(line);
            }
            ui.label(RichText::new(ask::backtest_line(b, l)).small().color(theme::MUTED));
        } else {
            let frac = (rs.pct / 100.0).clamp(0.0, 1.0) as f32;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 12.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 6, Color32::from_white_alpha(20));
            ui.painter().rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * frac, rect.height())), 6, theme::WATER);
            let lx = rect.left() + rect.width() * (res.limit / res.capacity) as f32;
            ui.painter().line_segment([egui::pos2(lx, rect.top() - 3.0), egui::pos2(lx, rect.bottom() + 3.0)], egui::Stroke::new(2.0, theme::ALERT));
            ui.label(RichText::new(format!("0 · {} {} {} · {}", t("rc.limit", l), ed_core::text::js_number(res.limit), res.unit, ed_core::text::js_number(res.capacity))).small().color(theme::MUTED));
            if let Some(w) = rs.warned_on {
                let warn = ed_core::text::js_number(ds.regions.thresholds.reservoir_warn_days);
                ui.label(strip_tags(&tf("al.warned", l, &[("w", &warn), ("d", &fd(Some(w), l))])));
            }
        }
        ui.label(RichText::new(format!("{}: {}", t("al.rule", l), res.rule.get(l))).small().color(theme::MUTED));
    });
}

#[allow(clippy::too_many_arguments)]
fn charts(app: &mut App, ui: &mut egui::Ui, ds: &Dataset, st: &Status, r: &ed_core::model::Region, wx: Option<&WeatherSet>, today: Date, l: Lang) {
    ui.horizontal(|ui| {
        for (tab, key) in [(RegionTab::Wx, "rtab.wx"), (RegionTab::Fx, "rtab.fx"), (RegionTab::H2o, "rtab.h2o")] {
            if ui.selectable_label(app.st.rtab == tab, t(key, l)).clicked() {
                app.st.rtab = tab;
            }
        }
    });
    let brief = st.brief.as_ref().filter(|_| ds.forecast.as_ref().is_some_and(|f| f.region == r.id));
    let w = wx.and_then(|x| x.by_region.get(&r.id));
    let note = crate::charts::region_chart(ui, r, st.get(&r.id), w, brief, app.st.rtab, today, ds.demo, l);
    ui.label(RichText::new(note).small().color(theme::MUTED));
}

pub fn region(app: &mut App, ui: &mut egui::Ui) {
    let l = app.st.lang;
    let Some(d) = &app.data else { return };
    let (ds, st, wx, wx_state, today) = (d.ds.clone(), d.status.clone(), d.wx.clone(), d.wx_state, d.today);
    let Some(r) = app.st.region.as_ref().and_then(|id| ds.region(id)).cloned() else { return };
    let a = st.get(&r.id).clone();
    let th = &ds.regions.thresholds;
    let lead = st.brief.is_some() && ds.forecast.as_ref().is_some_and(|f| f.region == r.id);

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            chip(ui, t(&level_key(a.status), l), theme::level(a.status));
            ui.heading(r.name.get(l));
            ui.label(RichText::new(strip_tags(&tf("rg.pass", l, &[("d", &fd(Some(a.last_pass), l)), ("n", &today.since(a.last_pass).to_string())]))).small().color(theme::MUTED));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            if ui.button("✕").clicked() {
                app.close_right();
            }
        });
    });

    if lead {
        alert_card(ui, &ds, &st, &r, l);
        charts(app, ui, &ds, &st, &r, wx.as_deref(), today, l);
    }

    // 7 days of weather from today
    match wx.as_ref().and_then(|w| w.by_region.get(&r.id)) {
        Some(w) => {
            if let Some(i) = w.index_of(today) {
                let n = (w.time.len() - i).min(7);
                let mx = (0..n).map(|k| w.rain[i + k].unwrap_or(0.0)).fold(5.0, f64::max);
                ui.columns(n.max(1), |cols| {
                    for (k, ui) in cols.iter_mut().enumerate().take(n) {
                        let rain = w.rain[i + k].unwrap_or(0.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(if k == 0 { t("fc.today", l).to_string() } else { fmt_date(w.time[i + k], l, DateStyle::WeekdayShort) }).small().color(if k == 0 { theme::ACCENT } else { theme::MUTED }));
                            ui.label(w.tmax[i + k].map_or("–".into(), |v| format!("{}°", ed_core::text::js_number(ed_core::text::js_round(v)))));
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 36.0), egui::Sense::hover());
                            let h = rect.height() * (rain / mx) as f32;
                            ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - h), rect.max), 2, theme::WATER);
                            ui.label(RichText::new(if rain >= 0.1 { to_fixed(rain, if rain < 10.0 { 1 } else { 0 }) } else { "0".into() }).small());
                        });
                    }
                });
            }
        }
        None => {
            ui.label(RichText::new(t(if wx_state == WxState::Loading { "brief.wx.loading" } else { "rnote.nowx" }, l)).color(theme::MUTED));
        }
    }

    ui.horizontal_wrapped(|ui| {
        if let Some(s) = &a.soil {
            crate::app::tile(ui, t("rg.tile.soil", l), &to_fixed(s.now, 0), "%", (s.finding.level != Level::Ok).then_some(theme::WATCH));
        }
        if let Some(w) = &a.wetland {
            crate::app::tile(ui, t("rg.tile.cleared", l), &to_fixed(w.ha, 0), "ha", (w.level != Level::Ok).then_some(theme::ALERT));
        }
        if let (Some(rs), Some(res)) = (&a.reservoir, &r.reservoir) {
            let (v, u) = res.area_km2.as_ref().and_then(|x| x.last()).map_or((rs.now, res.unit.as_str()), |v| (*v, "km²"));
            crate::app::tile(ui, t("rg.tile.lake", l), &to_fixed(v, 0), u, (rs.level != Level::Ok).then_some(theme::ALERT));
        }
        if let Some(rate) = a.ground.rate {
            crate::app::tile(ui, t("rg.tile.rate", l), &signed(rate, 0), t("unit.mmyr", l), (a.ground.level != Level::Ok).then_some(theme::ALERT));
            let rf = &a.reference;
            crate::app::tile(
                ui,
                &strip_tags(&tf("rg.tile.egms", l, &[("ref", ds.reference_name())])),
                &rf.as_ref().map_or("–".into(), |x| signed(x.diff, 1)),
                t("unit.mmyr", l),
                rf.as_ref().filter(|x| !x.agrees).map(|_| theme::WATCH),
            );
        }
        crate::app::tile(ui, t("rg.tile.rain", l), &a.weather.as_ref().map_or("–".into(), |w| to_fixed(w.rain30, 0)), "mm", None);
    });

    section(ui, t("rg.findings", l));
    for f in a.findings.iter().filter(|f| !(lead && f.key == "reservoir")) {
        card(ui, (f.level != Level::Ok).then_some(theme::level(f.level)), |ui| {
            ui.label(RichText::new(f.title.get(l)).strong().color(if f.level == Level::Ok { theme::TEXT } else { theme::level(f.level) }));
            ui.label(f.text.get(l));
            if let Some(act) = &f.action {
                egui::Frame::new().fill(Color32::from_white_alpha(8)).corner_radius(8).inner_margin(8).show(ui, |ui| {
                    ui.label(RichText::new(t("action.title", l)).small().strong().color(theme::ACCENT));
                    ui.label(act.get(l));
                });
            }
            ui.label(RichText::new(f.basis.get(l)).small().color(theme::MUTED));
        });
    }

    if !lead {
        alert_card(ui, &ds, &st, &r, l);
        charts(app, ui, &ds, &st, &r, wx.as_deref(), today, l);
    }

    if let Some(e) = &a.effects {
        section(ui, t("ev.title", l));
        if e.events.is_empty() {
            ui.label(RichText::new(t("ev.none", l)).color(theme::MUTED));
        }
        for ev in e.events.iter().take(5) {
            let what = match ev.kind {
                EventKind::Rain => tf("ev.rain", l, &[("v", &to_fixed(ev.value, 0))]),
                EventKind::Heat => tf("ev.heat", l, &[("n", &ev.days.to_string()), ("v", &to_fixed(ev.value, 0))]),
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(fd(Some(ev.date), l)).small().color(theme::MUTED));
                ui.label(RichText::new(strip_tags(&what)).strong().color(if ev.kind == EventKind::Rain { theme::WATER } else { theme::ALERT }));
            });
            if let Some(p) = ev.pass {
                let mut after = Vec::new();
                if let Some(v) = ev.d_hh {
                    after.push(format!("HH {} dB", signed(v, 1)));
                }
                if let Some(v) = ev.d_water.filter(|v| *v != 0.0) {
                    after.push(format!("{} {} km²", t("ev.water", l), signed(v, 0)));
                }
                if let Some(v) = ev.d_disp.filter(|v| v.abs() >= 1.0) {
                    after.push(format!("{} {} mm", t("ev.ground", l), signed(v, 0)));
                }
                let joined = if after.is_empty() { "—".to_string() } else { after.join(" · ") };
                ui.label(RichText::new(format!("{} {joined}", strip_tags(&tf("ev.after", l, &[("d", &short(p, l))])))).small());
            }
        }
    }

    egui::CollapsingHeader::new(t("how.title", l)).show(ui, |ui| {
        let n = ed_core::text::js_number;
        ui.label(strip_tags(t("how.text", l)));
        if a.ground.rate.is_some() {
            ui.label(strip_tags(&tf("how.sink", l, &[("a", &n(th.sink_watch_mm_yr)), ("b", &n(th.sink_alert_mm_yr))])));
            ui.label(strip_tags(&tf("how.egms", l, &[("a", &n(th.egms_tolerance_mm_yr)), ("ref", ds.reference_name())])));
        }
        ui.label(strip_tags(&tf("how.res", l, &[("a", &n(th.reservoir_warn_days))])));
        ui.label(strip_tags(&tf("how.wx", l, &[("a", &n(th.heavy_rain_mm_day)), ("b", &n(th.heat_c)), ("c", &n(th.dry_spell_days))])));
        if ds.demo {
            ui.label(RichText::new(t("how.demo", l)).color(theme::WATCH));
        }
    });
}
