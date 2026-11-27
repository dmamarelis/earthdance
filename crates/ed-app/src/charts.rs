//! Charts (egui_plot). The x axis is always "days since 1970-01-01" and is labelled as dates.

use std::ops::RangeInclusive;

use eframe::egui::{self, Color32};
use egui_plot::{Bar, BarChart, FilledArea, GridInput, GridMark, HLine, Legend, Line, LineStyle, Plot, PlotPoints, Points, VLine};
use ed_core::ask::Brief;
use ed_core::insights::Assessment;
use ed_core::model::{Point, Region, Validation, Weather};
use ed_core::text::{fmt_date, t, DateStyle};
use ed_core::{Date, Lang};

use crate::app::{PointTab, RegionTab};
use crate::theme;

fn x(d: Date) -> f64 {
    f64::from(d.days())
}

fn date_axis(l: Lang, with_year: bool) -> impl Fn(GridMark, &RangeInclusive<f64>) -> String {
    move |m, _| {
        let d = Date::from_days(m.value.round() as i32);
        fmt_date(d, l, if with_year { DateStyle::DayMonYY } else { DateStyle::DayMon })
    }
}

/// Grid marks on the first of the month (every 1, 2 or 3 months depending on the span), so the axis
/// labels are calendar dates instead of round day numbers.
fn month_marks(input: GridInput) -> Vec<GridMark> {
    let (lo, hi) = input.bounds;
    // egui_plot only labels marks at least ~60–80 px apart; base_step_size is 8 px in days
    let days_per_px = input.base_step_size / 8.0;
    let need = 90.0 * days_per_px;
    let every = [1u32, 2, 3, 6, 12].into_iter().find(|m| f64::from(*m) * 30.0 >= need).unwrap_or(12);
    let mut d = Date::from_days(lo.floor() as i32);
    d = Date::ymd(d.year(), d.month(), 1);
    let mut out = Vec::new();
    while f64::from(d.days()) <= hi {
        if f64::from(d.days()) >= lo && (d.month() - 1).is_multiple_of(every) {
            out.push(GridMark { value: f64::from(d.days()), step_size: 30.0 * f64::from(every) });
        }
        d = if d.month() == 12 { Date::ymd(d.year() + 1, 1, 1) } else { Date::ymd(d.year(), d.month() + 1, 1) };
    }
    out
}

/// A row of coloured legend entries under a chart (egui_plot's legend sits on top of the data).
pub fn legend_row(ui: &mut egui::Ui, items: &[(&str, Color32)]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (name, c) in items {
            ui.label(egui::RichText::new(format!("● {name}")).small().color(*c));
        }
    });
}

fn base(id: &str, l: Lang, height: f32, with_year: bool) -> Plot<'static> {
    Plot::new(id.to_string())
        .height(height)
        .x_grid_spacer(month_marks)
        .x_axis_formatter(date_axis(l, with_year))
        .legend(Legend::default().position(egui_plot::Corner::LeftTop).background_alpha(0.6))
        .allow_scroll(false)
        .allow_zoom(false)
        .allow_drag(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .show_grid([false, true])
        .label_formatter(move |hp| {
            let (name, p) = match hp {
                egui_plot::HoverPosition::NearDataPoint { plot_name, position, .. } => (*plot_name, *position),
                egui_plot::HoverPosition::Elsewhere { position } => ("", *position),
            };
            let d = Date::from_days(p.x.round() as i32);
            Some(format!("{name}\n{}\n{:.2}", fmt_date(d, l, DateStyle::DayMonYear), p.y).trim_start().to_string())
        })
}

fn series(dates: &[Date], v: &[f64]) -> PlotPoints<'static> {
    dates.iter().zip(v).map(|(d, y)| [x(*d), *y]).collect()
}

/// Splits a series with gaps into contiguous pieces (Chart.js `spanGaps: false` would break the line).
fn pieces(dates: &[Date], v: &[Option<f64>]) -> Vec<Vec<[f64; 2]>> {
    let mut out: Vec<Vec<[f64; 2]>> = vec![Vec::new()];
    for (d, y) in dates.iter().zip(v) {
        match y {
            Some(y) => out.last_mut().expect("non-empty").push([x(*d), *y]),
            None if !out.last().expect("non-empty").is_empty() => out.push(Vec::new()),
            None => {}
        }
    }
    out.retain(|p| !p.is_empty());
    out
}

/// A point's time series. Returns the date of the pass the user clicked, if any.
/// A smooth daily estimate between passes (Floater–Hormann, small blending degree), drawn dotted.
fn estimated(dates: &[Date], v: &[f64]) -> PlotPoints<'static> {
    use ed_core::interp::{FloaterHormann, Interpolant, SMOOTH_D};
    if dates.len() < 3 {
        return PlotPoints::from(Vec::<[f64; 2]>::new());
    }
    let xs: Vec<f64> = dates.iter().map(|d| x(*d)).collect();
    let fh = FloaterHormann::new(&xs, v, SMOOTH_D);
    (dates[0].days()..=dates[dates.len() - 1].days()).map(|n| [f64::from(n), fh.eval(f64::from(n))]).collect()
}

#[allow(clippy::too_many_arguments)]
pub fn point_chart(ui: &mut egui::Ui, p: &Point, tab: PointTab, cursor: Option<Date>, ref_name: &str, show_estimates: bool, l: Lang) -> Option<Date> {
    let s = &p.series;
    let resp = base(&format!("pt-{}", p.id), l, 220.0, false).show(ui, |pu| {
        match tab {
            PointTab::Ground => {
                if show_estimates {
                    pu.line(Line::new("", estimated(&s.dates, &s.disp_mm)).color(theme::ALERT.gamma_multiply(0.7)).width(1.2).style(LineStyle::Dotted { spacing: 4.0 }));
                }
                pu.points(Points::new(t("chart.disp", l), series(&s.dates, &s.disp_mm)).color(theme::ALERT).radius(3.0));
                pu.line(Line::new(t("chart.disp", l), series(&s.dates, &s.disp_mm)).color(theme::ALERT).width(if show_estimates { 0.0 } else { 2.2 }).fill(0.0).fill_alpha(0.15));
                if let Some(r) = &s.ref_mm {
                    pu.line(Line::new(format!("{ref_name} (mm)"), series(&s.dates, r)).color(theme::GREY).width(1.5).style(LineStyle::Dashed { length: 6.0 }));
                }
            }
            PointTab::Radar => {
                pu.line(Line::new("HH · L (dB)", series(&s.dates, &s.hh_db)).color(theme::TEXT).width(2.2));
                pu.line(Line::new("HV · L (dB)", series(&s.dates, &s.hv_db)).color(theme::OK).width(2.2));
                if let Some(v) = &s.vv_db {
                    pu.line(Line::new("VV · C (dB)", series(&s.dates, v)).color(theme::VIOLET).width(1.5).style(LineStyle::Dashed { length: 6.0 }));
                }
            }
        }
        if let Some(c) = cursor {
            pu.vline(VLine::new("", x(c)).color(theme::ACCENT).style(LineStyle::Dashed { length: 4.0 }).width(1.5));
        }
        pu.pointer_coordinate()
    });
    if resp.response.clicked()
        && let Some(pc) = resp.inner
    {
        let target = Date::from_days(pc.x.round() as i32);
        return s.dates.iter().min_by_key(|d| d.since(target).abs()).copied();
    }
    None
}

#[allow(clippy::too_many_arguments)]
/// The region panel's chart for one tab. Returns the note shown under it.
pub fn region_chart(ui: &mut egui::Ui, r: &Region, a: &Assessment, w: Option<&Weather>, brief: Option<&Brief>, estimates: &[(Date, f64, ed_core::forecast::Fill)], tab: RegionTab, today: Date, demo: bool, l: Lang) -> String {
    let s = &r.series;
    let id = format!("rg-{}-{tab:?}", r.id);
    let mut note = match tab {
        RegionTab::Wx => {
            let Some(w) = w else { return t("rnote.nowx", l).to_string() };
            let mut bars = Vec::new();
            let mut bal = Vec::new();
            let mut acc = 0.0;
            for i in (0..w.time.len()).step_by(7) {
                let end = (i + 7).min(w.time.len());
                let rain: f64 = w.rain[i..end].iter().map(|v| v.unwrap_or(0.0)).sum();
                let et0: f64 = w.et0[i..end].iter().map(|v| v.unwrap_or(0.0)).sum();
                acc += rain - et0;
                let future = w.time[i] > today;
                bars.push(Bar::new(x(w.time[i]) + 3.5, rain).width(5.5).fill(if future { theme::WATER.gamma_multiply(0.3) } else { theme::WATER.gamma_multiply(0.85) }));
                bal.push([x(w.time[i]) + 3.5, acc]);
            }
            base(&id, l, 220.0, false).show(ui, |pu| {
                pu.bar_chart(BarChart::new(t("rc.rainWeek", l), bars).color(theme::WATER));
                pu.line(Line::new(t("rc.balance", l), PlotPoints::from(bal)).color(theme::WATCH).width(2.0));
                pu.vline(VLine::new(t("rc.today", l), x(today)).color(Color32::from_white_alpha(170)).style(LineStyle::Dashed { length: 3.0 }));
            });
            t("rnote.wx", l).to_string()
        }
        RegionTab::Fx => {
            let hh = s.hh_db.clone().unwrap_or_default();
            base(&id, l, 180.0, false).show(ui, |pu| {
                for piece in pieces(&s.dates, &hh) {
                    pu.line(Line::new("HH · NISAR (dB)", PlotPoints::from(piece.clone())).color(theme::TEXT).width(2.0));
                    pu.points(Points::new("HH · NISAR (dB)", PlotPoints::from(piece)).color(theme::TEXT).radius(2.5));
                }
                if let Some(v) = &s.vv_db {
                    pu.line(Line::new("VV · Sentinel-1 (dB)", series(&s.dates, v)).color(theme::VIOLET).width(1.5).style(LineStyle::Dashed { length: 6.0 }));
                }
            });
            if let Some(e) = &a.effects {
                let bars = e.passes.iter().map(|p| Bar::new(x(p.date), p.rain).width(4.0)).collect();
                base(&format!("{id}-rain"), l, 90.0, false).show(ui, |pu| {
                    pu.bar_chart(BarChart::new(t("rc.rainPass", l), bars).color(theme::WATER.gamma_multiply(0.7)));
                });
            }
            t(if a.effects.is_some() { "rnote.fx" } else { "rnote.nowx" }, l).to_string()
        }
        RegionTab::H2o => match (&r.reservoir, brief) {
            (Some(res), Some(b)) => {
                let f = &b.base;
                // up to 1 October after next summer: far enough to see the next fall below the limit
                let until = f.dates.iter().copied().find(|d| *d > today && d.md() == (10, 1) && d.year() > today.year()).or(f.dates.last().copied()).unwrap_or(today);
                let k = f.dates.iter().position(|d| *d > until).unwrap_or(f.dates.len());
                let xs: Vec<f64> = f.dates[..k].iter().map(|d| x(*d)).collect();
                let area = res.area_km2.as_ref().unwrap_or(&res.volume);
                let start = res.dates.first().copied().unwrap_or(today);
                base(&id, l, 240.0, true).include_y(0.0).include_y((f.amax / 5.0).ceil() * 5.0).include_x(x(start)).show(ui, |pu| {
                    pu.add(FilledArea::new("", &xs, &f.p10[..k], &f.p90[..k]).fill_color(theme::WATER.gamma_multiply(0.2)));
                    pu.line(Line::new("", PlotPoints::from(xs.iter().zip(&f.p50[..k]).map(|(a, b)| [*a, *b]).collect::<Vec<_>>())).color(theme::WATER).width(1.8).style(LineStyle::Dashed { length: 6.0 }));
                    if !estimates.is_empty() {
                        pu.line(Line::new("", estimates.iter().map(|(d, v, _)| [x(*d), *v]).collect::<PlotPoints>()).color(Color32::from_white_alpha(140)).width(1.0).style(LineStyle::Dotted { spacing: 3.0 }));
                    }
                    pu.points(Points::new("", series(&res.dates, area)).color(Color32::WHITE).radius(3.5));
                    pu.hline(HLine::new("", res.limit).color(theme::ALERT).style(LineStyle::Dashed { length: 8.0 }).width(1.5));
                    pu.vline(VLine::new("", x(today)).color(Color32::from_white_alpha(180)).style(LineStyle::Dotted { spacing: 4.0 }));
                });
                let est_label = if l == Lang::El { "εκτίμηση ανάμεσα στις λήψεις" } else { "estimated between passes" };
                let mut items = vec![(t("rc.measured", l), Color32::WHITE)];
                if !estimates.is_empty() {
                    items.push((est_label, Color32::from_white_alpha(140)));
                }
                legend_row(ui, &items);
                legend_row(ui, &[(t("rc.forecast", l), theme::WATER), (t("rc.band", l), theme::WATER.gamma_multiply(0.5)), (t("rc.limit", l), theme::ALERT), (t("rc.today", l), Color32::from_white_alpha(180))]);
                match b.generator {
                    Some((g0, g1)) => {
                        let (n, m) = (ed_core::forecast::SYNTHETIC_YEARS, f.members);
                        if l == Lang::El {
                            format!("Τελείες: μετρημένη έκταση. Διακεκομμένη: η πρόβλεψη του μοντέλου ισοζυγίου νερού. Σκίαση: εύρος 10–90 % από {m} σενάρια ({n} προσομοιωμένες χρονιές καιρού από αλυσίδες Markov, ERA5 {g0}–{g1}, × 3 τιμές του συντελεστή βροχής) και το σφάλμα του μοντέλου.")
                        } else {
                            format!("Dots: measured water extent. Dashed: the water-balance model forecast. Shading: 10–90 % range of {m} runs ({n} simulated years of weather from Markov chains fitted to ERA5 {g0}–{g1}, × 3 rain coefficients) plus the model error.")
                        }
                    }
                    None => t("rnote.h2o.fc", l).to_string(),
                }
            }
            (Some(res), None) => {
                let proj = a.reservoir.as_ref().and_then(|x| x.cross_date);
                base(&id, l, 220.0, false).include_y(0.0).show(ui, |pu| {
                    pu.line(Line::new(format!("{} ({})", t("rc.volume", l), res.unit), series(&res.dates, &res.volume)).color(theme::WATER).width(2.0).fill(0.0).fill_alpha(0.16));
                    if let (Some(p), Some(last), Some(v)) = (proj, res.dates.last(), res.volume.last()) {
                        pu.line(Line::new(t("rc.projection", l), PlotPoints::from(vec![[x(*last), *v], [x(p), res.limit]])).color(theme::WATER).style(LineStyle::Dashed { length: 5.0 }));
                    }
                    pu.hline(HLine::new(format!("{} ({})", t("rc.limit", l), res.unit), res.limit).color(theme::ALERT).style(LineStyle::Dashed { length: 8.0 }));
                });
                t("rnote.h2o.res", l).to_string()
            }
            (None, _) => {
                base(&id, l, 220.0, false).include_y(0.0).show(ui, |pu| {
                    if let Some(v) = &s.vol_loss_hm3 {
                        pu.line(Line::new(t("rc.loss", l), series(&s.dates, v)).color(theme::ALERT).width(2.0).fill(0.0).fill_alpha(0.16));
                    }
                    if let Some(w) = s.water_km2.as_ref().filter(|w| w.iter().any(|v| *v > 0.0)) {
                        let bars = s.dates.iter().zip(w).map(|(d, v)| Bar::new(x(*d), *v).width(6.0)).collect();
                        pu.bar_chart(BarChart::new(t("rc.water", l), bars).color(theme::WATER.gamma_multiply(0.55)));
                    }
                });
                t("rnote.h2o.aq", l).to_string()
            }
        },
    };
    if demo && tab != RegionTab::Wx {
        note.push(' ');
        note.push_str(t("demo.warn", l));
    }
    note
}

/// NISAR rates against the independent reference, with the 1:1 line.
pub fn validation(ui: &mut egui::Ui, v: &Validation, ref_name: &str) {
    let xs: Vec<f64> = v.pairs.iter().map(|p| p.reference).collect();
    let ys: Vec<f64> = v.pairs.iter().map(|p| p.nisar).collect();
    let lo = ((ed_core::stats::min(&xs).min(ed_core::stats::min(&ys)) - 2.0) / 10.0).floor() * 10.0;
    let hi = ((ed_core::stats::max(&xs).max(ed_core::stats::max(&ys)).max(0.0) + 2.0) / 10.0).ceil() * 10.0;
    Plot::new("validation")
        .height(340.0)
        .data_aspect(1.0)
        .legend(Legend::default())
        .x_axis_label(format!("{ref_name} ({})", v.unit))
        .y_axis_label(format!("NISAR ({})", v.unit))
        // the same fixed steps on both axes (the automatic x labels thinned out to just "0")
        .x_grid_spacer(egui_plot::uniform_grid_spacer(|_| [50.0, 10.0, 5.0]))
        .y_grid_spacer(egui_plot::uniform_grid_spacer(|_| [50.0, 10.0, 5.0]))
        .include_x(lo)
        .include_x(hi)
        .include_y(lo)
        .include_y(hi)
        .show(ui, |pu| {
            pu.line(Line::new("1:1", PlotPoints::from(vec![[lo, lo], [hi, hi]])).color(Color32::from_white_alpha(100)).style(LineStyle::Dashed { length: 6.0 }));
            pu.points(Points::new(format!("NISAR vs {ref_name}"), xs.iter().zip(&ys).map(|(a, b)| [*a, *b]).collect::<PlotPoints>()).color(theme::ACCENT).radius(4.0));
        });
}
