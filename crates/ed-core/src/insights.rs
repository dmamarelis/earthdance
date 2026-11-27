//! The conclusions engine (port of `js/insights.js`).
//!
//! Plain, readable rules: every sentence the app shows about an area is produced here from the numbers in
//! the area's series and the weather. No machine learning, nothing hidden: change a threshold and the
//! conclusions change with it.

use serde::Serialize;

use crate::model::{Region, Reservoir, Series, Thresholds, Weather};
use crate::stats::{corr, max, mean, median, min, slope_per_year, sum};
use crate::text::{fmt_date, js_number, js_round, signed as num, to_fixed, DateStyle, Lang, Text};
use crate::Date;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    #[default]
    Ok,
    Watch,
    Alert,
}

impl Level {
    pub const fn key(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Watch => "watch",
            Level::Alert => "alert",
        }
    }
}

/// One conclusion about an area.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    pub key: &'static str,
    pub level: Level,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<&'static str>,
    pub title: Text,
    pub text: Text,
    pub basis: Text,
    /// What the region could do about it (only for findings that are not "ok").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<Text>,
}

fn fd(d: Date, l: Lang) -> String {
    fmt_date(d, l, DateStyle::DayMonYear)
}
fn fds(d: Date, l: Lang, s: DateStyle) -> String {
    fmt_date(d, l, s)
}
fn el(l: Lang) -> bool {
    l == Lang::El
}

/// Sum of a daily weather variable over `[from, to]`, both inclusive; missing days count as 0.
pub fn wx_sum(wx: &Weather, key: &[Option<f64>], from: Date, to: Date) -> f64 {
    wx.time.iter().zip(key).filter(|(d, _)| **d >= from && **d <= to).map(|(_, v)| v.unwrap_or(0.0)).sum()
}

// ───────────────────────────────────────────────────────── ground motion

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ground {
    pub rate: Option<f64>,
    pub total: Option<f64>,
    pub level: Level,
    pub ratio: Option<f64>,
    pub finding: Option<Finding>,
}

fn ground(s: &Series, th: &Thresholds) -> Ground {
    // no displacement series (e.g. the real set has no GUNW yet): no ground conclusion at all, not a fake "stable"
    let Some(disp) = s.disp_mm.as_deref() else {
        return Ground { rate: None, total: None, level: Level::Ok, ratio: None, finding: None };
    };
    let rate = slope_per_year(&s.dates, disp);
    let total = disp[disp.len() - 1] - disp[0];
    let level = if rate <= th.sink_alert_mm_yr {
        Level::Alert
    } else if rate <= th.sink_watch_mm_yr {
        Level::Watch
    } else {
        Level::Ok
    };

    // does it sink faster while the fields are irrigated (May–September)?
    let (mut d_irr, mut t_irr, mut d_rest, mut t_rest) = (0.0, 0.0, 0.0, 0.0);
    for i in 1..s.dates.len() {
        let span = s.dates[i].since(s.dates[i - 1]);
        let mid = s.dates[i - 1].add_days(span.div_euclid(2)).month();
        let dd = disp[i] - disp[i - 1];
        if (5..=9).contains(&mid) {
            d_irr += dd;
            t_irr += f64::from(span);
        } else {
            d_rest += dd;
            t_rest += f64::from(span);
        }
    }
    let ratio = (t_irr != 0.0 && t_rest != 0.0 && d_rest < 0.0).then(|| (d_irr / t_irr) / (d_rest / t_rest));
    let seasonal = level != Level::Ok && ratio.is_some_and(|r| r >= 1.3);
    let since0 = s.dates[0];

    let finding = Finding {
        key: "ground",
        level,
        sub: None,
        title: match level {
            Level::Ok => Text::new("Έδαφος σταθερό", "Ground is stable"),
            Level::Watch => Text::new("Το έδαφος υποχωρεί", "The ground is sinking"),
            Level::Alert => Text::new("Έντονη καθίζηση", "Strong subsidence"),
        },
        text: Text::both(|l| {
            let since = fd(since0, l);
            let base = match (el(l), level == Level::Ok) {
                (true, true) => format!("Καμία αξιόλογη μετακίνηση: {} mm/έτος.", num(rate, 0)),
                (true, false) => format!("Υποχωρεί με {} mm/έτος (σύνολο {} mm από {since}).", num(rate, 0), num(total, 0)),
                (false, true) => format!("No significant motion: {} mm/yr.", num(rate, 0)),
                (false, false) => format!("Sinking at {} mm/yr ({} mm in total since {since}).", num(rate, 0), num(total, 0)),
            };
            if !seasonal {
                return base;
            }
            let r = to_fixed(ratio.unwrap_or(0.0), 1);
            base + &if el(l) {
                format!(" Την αρδευτική περίοδο (Μάιος–Σεπτέμβριος) βυθίζεται {r}× πιο γρήγορα: ένδειξη ότι φταίει η άντληση υπόγειου νερού.")
            } else {
                format!(" During the irrigation season (May–September) it sinks {r}× faster: a sign that groundwater pumping is the cause.")
            }
        }),
        basis: Text::new(
            "NISAR L2 GUNW · κλίση ευθείας στη χρονοσειρά μετατόπισης",
            "NISAR L2 GUNW · linear trend of the displacement series",
        ),
        action: None,
    };
    Ground { rate: Some(rate), total: Some(total), level, ratio: if seasonal { ratio } else { None }, finding: Some(finding) }
}

// ───────────────────────────────────────────────────────── independent reference (EGMS)

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefCheck {
    pub ref_rate: f64,
    pub diff: f64,
    pub rmse: f64,
    pub agrees: bool,
    pub finding: Finding,
}

fn reference(s: &Series, th: &Thresholds, rate: Option<f64>, ref_name: &str) -> Option<RefCheck> {
    let r = s.ref_mm.as_deref()?;
    let disp = s.disp_mm.as_deref()?;
    let rate = rate?;
    let ref_rate = slope_per_year(&s.dates, r);
    let diff = rate - ref_rate;
    let se: Vec<f64> = disp.iter().zip(r).map(|(v, w)| (v - w).powi(2)).collect();
    let rmse = mean(&se).sqrt();
    let agrees = diff.abs() <= th.egms_tolerance_mm_yr;
    let tol = js_number(th.egms_tolerance_mm_yr);
    let finding = Finding {
        key: "egms",
        level: if agrees { Level::Ok } else { Level::Watch },
        sub: None,
        title: if agrees {
            Text::new(format!("Συμφωνεί με {ref_name}"), format!("Agrees with {ref_name}"))
        } else {
            Text::new(format!("Απόκλιση από {ref_name}"), format!("Differs from {ref_name}"))
        },
        text: Text::both(|l| {
            let head = if el(l) {
                format!(
                    "NISAR {} έναντι {ref_name} {} mm/έτος (διαφορά {}, RMSE {} mm).",
                    num(rate, 0),
                    num(ref_rate, 0),
                    num(diff, 1),
                    to_fixed(rmse, 1)
                )
            } else {
                format!(
                    "NISAR {} vs {ref_name} {} mm/yr (difference {}, RMSE {} mm).",
                    num(rate, 0),
                    num(ref_rate, 0),
                    num(diff, 1),
                    to_fixed(rmse, 1)
                )
            };
            match (agrees, el(l)) {
                (true, true) => head + " Δύο ανεξάρτητα συστήματα δίνουν την ίδια εικόνα.",
                (true, false) => head + " Two independent systems give the same picture.",
                (false, true) => head + &format!(" Ξεπερνά την ανοχή των ±{tol} mm/έτος: το συμπέρασμα για το έδαφος θέλει έλεγχο πριν χρησιμοποιηθεί."),
                (false, false) => head + &format!(" Beyond the ±{tol} mm/yr tolerance: check the ground conclusion before acting on it."),
            }
        }),
        basis: Text::new(format!("{ref_name} · σύγκριση ρυθμών στην ίδια περιοχή"), format!("{ref_name} · rates compared over the same area")),
        action: None,
    };
    Some(RefCheck { ref_rate, diff, rmse, agrees, finding })
}

// ───────────────────────────────────────────────────────── L-band vs C-band

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sensors {
    pub r: f64,
    pub hv_range: f64,
    pub finding: Finding,
}

fn sensors(s: &Series) -> Option<Sensors> {
    let hh: Vec<f64> = s.hh_db.as_ref()?.iter().map(|v| v.unwrap_or(0.0)).collect();
    let vv = s.vv_db.as_deref()?;
    let r = corr(&hh, vv);
    let (hv_range, peak) = match s.hv_db.as_deref() {
        Some(hv) => {
            let m = max(hv);
            (m - min(hv), hv.iter().position(|&v| v == m).map(|i| s.dates[i]))
        }
        None => (0.0, None),
    };
    let finding = Finding {
        key: "sensors",
        level: Level::Ok,
        sub: None,
        title: if r >= 0.6 {
            Text::new("NISAR και Sentinel-1 συμφωνούν", "NISAR and Sentinel-1 agree")
        } else {
            Text::new("Οι δύο δορυφόροι βλέπουν διαφορετικά", "The two satellites see differently")
        },
        text: Text::both(|l| {
            let rs = to_fixed(r, 2);
            let mut out = if el(l) {
                format!("Οπισθοσκέδαση L-band (HH) και C-band (VV): συσχέτιση r = {rs}.")
            } else {
                format!("L-band (HH) and C-band (VV) backscatter: correlation r = {rs}.")
            };
            if r < 0.6 {
                out += if el(l) {
                    " Αναμενόμενο όπου υπάρχει βλάστηση: το L-band τη διαπερνά, το C-band όχι."
                } else {
                    " Expected over vegetation: L-band penetrates it, C-band does not."
                };
            }
            if hv_range >= 4.0 {
                let m = peak.map_or_else(|| "undefined".into(), |p| fds(p, l, DateStyle::MonthLong));
                let hr = to_fixed(hv_range, 1);
                out += &if el(l) {
                    format!(" Ο κύκλος της καλλιέργειας φαίνεται στο HV: κορύφωση τον {m} (εύρος {hr} dB).")
                } else {
                    format!(" The crop cycle shows in HV: peak in {m} (range {hr} dB).")
                };
            }
            out
        }),
        basis: Text::new("NISAR L2 GCOV + Sentinel-1 RTC · μέσος όρος περιοχής", "NISAR L2 GCOV + Sentinel-1 RTC · area mean"),
        action: None,
    };
    Some(Sensors { r, hv_range, finding })
}

// ───────────────────────────────────────────────────────── floods

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Flood {
    pub date: Date,
    pub extra: f64,
    pub rain: Option<f64>,
    pub finding: Finding,
}

fn surface_water(s: &Series, wx: Option<&Weather>) -> Option<Flood> {
    let w = s.water_km2.as_deref()?;
    let base = median(w);
    let peak = max(w);
    if peak - base < 5f64.max(base) {
        return None; // a lake's normal seasonal swing is not a flood
    }
    let i = w.iter().position(|&v| v == peak)?;
    let recent = i + 2 >= w.len();
    let di = s.dates[i];
    let rain = wx.map(|wx| wx_sum(wx, &wx.rain, di.add_days(-14), di));
    let next = (i + 1 < w.len()).then(|| (s.dates[i + 1], w[i + 1]));
    let finding = Finding {
        key: "flood",
        level: if recent { Level::Alert } else { Level::Ok },
        sub: None,
        title: if recent {
            Text::new("Πλημμύρα σε εξέλιξη", "Flooding in progress")
        } else {
            Text::new("Καταγράφηκε πλημμύρα", "A flood was recorded")
        },
        text: Text::both(|l| {
            let (p, b) = (to_fixed(peak, 0), to_fixed(base, 0));
            let mut out = if el(l) {
                format!("Στις {} το ραντάρ είδε {p} km² νερού (συνήθως {b}).", fd(di, l))
            } else {
                format!("On {} the radar saw {p} km² of water (normally {b}).", fd(di, l))
            };
            if let Some(rain) = rain {
                let r = to_fixed(rain, 0);
                out += &if el(l) {
                    format!(" Τις 14 προηγούμενες ημέρες είχαν πέσει {r} mm βροχής.")
                } else {
                    format!(" {r} mm of rain had fallen in the 14 days before.")
                };
            }
            if let Some((nd, nw)) = next {
                let (d, v) = (fds(nd, l, DateStyle::DayMon), to_fixed(nw, 0));
                out += &if el(l) {
                    format!(" Στην επόμενη λήψη ({d}) είχαν μείνει {v} km².")
                } else {
                    format!(" By the next pass ({d}) {v} km² remained.")
                };
            }
            out
        }),
        basis: Text::new(
            "NISAR HH: το ήρεμο νερό είναι σκοτεινό στο ραντάρ, ακόμη και με σύννεφα",
            "NISAR HH: calm water is dark to radar, even through cloud",
        ),
        action: None,
    };
    Some(Flood { date: di, extra: peak - base, rain, finding })
}

// ───────────────────────────────────────────────────────── reservoir / lake

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReservoirState {
    pub now: f64,
    pub pct: f64,
    pub per_day: f64,
    /// Days until the limit at the last pace; `f64::INFINITY` when not falling.
    pub days_left: f64,
    pub cross_date: Option<Date>,
    pub overdue: bool,
    pub warned_on: Option<Date>,
    pub below: bool,
    pub level: Level,
    pub rain_since: Option<f64>,
    pub finding: Finding,
}

/// Pace and days left, using the drop between the measurements `i-1` and `i`.
fn project(res: &Reservoir, i: usize) -> (f64, f64) {
    let per_day = if i == 0 {
        f64::NAN
    } else {
        (res.volume[i] - res.volume[i - 1]) / f64::from(res.dates[i].since(res.dates[i - 1]))
    };
    let room = res.volume[i] - res.limit;
    let days_left = if room <= 0.0 {
        0.0
    } else if per_day < 0.0 {
        room / -per_day
    } else {
        f64::INFINITY
    };
    (per_day, days_left)
}

fn reservoir(res: Option<&Reservoir>, th: &Thresholds, wx: Option<&Weather>, today: Date) -> Option<ReservoirState> {
    let res = res?;
    if res.volume.is_empty() {
        return None;
    }
    let n = res.volume.len() - 1;
    let now = res.volume[n];
    let (per_day, days_left) = project(res, n);
    let below = now <= res.limit;
    let warn = th.reservoir_warn_days;
    let level = if below || days_left <= warn {
        Level::Alert
    } else if days_left <= 2.0 * warn {
        Level::Watch
    } else {
        Level::Ok
    };
    let cross_date = (days_left.is_finite() && !below).then(|| res.dates[n].add_days(js_round(days_left) as i32));
    let overdue = cross_date.is_some_and(|c| c <= today); // the projected date passed with no new measurement
    let days_from_today = cross_date.map(|c| c.since(today));
    // the first measurement at which this warning would already have fired
    let warned_on = (1..=n).find(|&i| project(res, i).1 <= warn).map(|i| res.dates[i]);
    let rain_since = wx.filter(|_| today > res.dates[n]).map(|wx| wx_sum(wx, &wx.rain, res.dates[n].add_days(1), today));
    let pct = now / res.capacity * 100.0;
    let dft = days_from_today.map_or_else(|| "null".into(), |d| d.to_string());

    let finding = Finding {
        key: "reservoir",
        level,
        sub: None,
        title: if below {
            Text::new("Νερό κάτω από το ελάχιστο όριο", "Water below the minimum limit")
        } else if overdue {
            Text::new("Το νερό ίσως πέρασε ήδη το όριο", "Water may already be past the limit")
        } else if level == Level::Alert {
            Text::new(format!("Το νερό φτάνει στο όριο σε ~{dft} ημέρες"), format!("Water reaches the limit in ~{dft} days"))
        } else if level == Level::Watch {
            Text::new("Το απόθεμα νερού πέφτει", "Water storage is falling")
        } else {
            Text::new("Απόθεμα νερού εντός ορίων", "Water storage within limits")
        },
        text: Text::both(|l| {
            let (nw, p, lim, u) = (to_fixed(now, 0), to_fixed(pct, 0), to_fixed(res.limit, 0), &res.unit);
            let mut out = if el(l) {
                format!("{}: {nw} {u} ({p}% της χωρητικότητας), όριο {lim} {u}.", res.name.el)
            } else {
                format!("{}: {nw} {u} ({p}% of capacity), limit {lim} {u}.", res.name.en)
            };
            if let Some(c) = cross_date.filter(|_| level != Level::Ok) {
                let pd = to_fixed(per_day.abs(), 2);
                out += &if el(l) {
                    format!(
                        " Στην τελευταία λήψη έχανε {pd} {u} την ημέρα· με αυτόν τον ρυθμό {} το όριο γύρω στις {}.",
                        if overdue { "πέρασε" } else { "περνά" },
                        fd(c, l)
                    )
                } else {
                    format!(
                        " At the last pass it was losing {pd} {u} a day; at that pace it {} the limit around {}.",
                        if overdue { "crossed" } else { "crosses" },
                        fd(c, l)
                    )
                };
            }
            if let Some(r) = rain_since.filter(|_| level != Level::Ok) {
                let r = to_fixed(r, 0);
                out += &if el(l) {
                    format!(" Από τότε έχουν πέσει {r} mm βροχής· μόνο η επόμενη λήψη θα δείξει αν ανέκοψαν την πτώση.")
                } else {
                    format!(" {r} mm of rain have fallen since; only the next pass will show whether that stopped the drop.")
                };
            }
            out
        }),
        basis: res.method.clone(),
        action: None,
    };
    Some(ReservoirState { now, pct, per_day, days_left, cross_date, overdue, warned_on, below, level, rain_since, finding })
}

// ───────────────────────────────────────────────────────── groundwater

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Aquifer {
    pub loss: f64,
    pub finding: Finding,
}

fn aquifer(s: &Series) -> Option<Aquifer> {
    let v = s.vol_loss_hm3.as_deref()?;
    let loss = v[v.len() - 1] - v[0];
    if loss < 0.5 {
        return None;
    }
    let d0 = s.dates[0];
    Some(Aquifer {
        loss,
        finding: Finding {
            key: "aquifer",
            level: Level::Ok, // explains the ground finding; it does not raise a second alarm
            sub: None,
            title: Text::new("Χάνεται υπόγειο νερό", "Groundwater is being lost"),
            text: Text::both(|l| {
                let x = to_fixed(loss, 1);
                if el(l) {
                    format!("Η καθίζηση αντιστοιχεί σε όγκο συμπίεσης {x} hm³ από {}: τόσο νερό, τουλάχιστον, βγήκε από τα στρώματα που συμπιέστηκαν και δεν αναπληρώθηκε.", fd(d0, l))
                } else {
                    format!("The subsidence amounts to a compaction volume of {x} hm³ since {}: at least that much water left the compacted layers and was not replaced.", fd(d0, l))
                }
            }),
            basis: Text::new(
                "Βύθιση × έκταση. Κατώτατη ένδειξη, όχι μέτρηση στάθμης γεώτρησης.",
                "Subsidence × area. A lower bound, not a borehole level measurement.",
            ),
            action: None,
        },
    })
}

// ───────────────────────────────────────────────────────── weather now

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeatherNow {
    pub rain7: f64,
    pub rain30: f64,
    pub balance90: f64,
    pub forecast: f64,
    pub dry: usize,
    pub tmax: Option<f64>,
    pub rain_today: f64,
    pub level: Level,
    pub finding: Finding,
}

fn weather_now(wx: &Weather, th: &Thresholds, today: Date) -> Option<WeatherNow> {
    let i = wx.time.iter().position(|&d| d == today)?;
    let back = |n: usize, key: &[Option<f64>]| -> f64 { key[(i + 1).saturating_sub(n)..=i].iter().map(|v| v.unwrap_or(0.0)).sum() };
    let rain7 = back(7, &wx.rain);
    let rain30 = back(30, &wx.rain);
    let balance90 = back(90, &wx.rain) - back(90, &wx.et0);
    let ahead: Vec<(Date, f64, Option<f64>)> =
        (i + 1..wx.time.len()).map(|k| (wx.time[k], wx.rain[k].unwrap_or(0.0), wx.tmax[k])).collect();
    let forecast: f64 = ahead.iter().map(|a| a.1).sum();
    let heavy = ahead.iter().find(|a| a.1 >= th.heavy_rain_mm_day).copied();
    let hot = ahead.iter().find(|a| a.2.is_some_and(|t| t >= th.heat_c)).copied();
    let dry = (0..=i).rev().take_while(|&k| wx.rain[k].unwrap_or(0.0) < 1.0).count();
    let is_dry = dry as f64 >= th.dry_spell_days;
    let level = if heavy.is_some() || hot.is_some() || is_dry { Level::Watch } else { Level::Ok };
    let sub = if heavy.is_some() {
        Some("heavy")
    } else if hot.is_some() {
        Some("heat")
    } else if is_dry {
        Some("dry")
    } else {
        None
    };
    let tmax = wx.tmax[i];
    let rain_today = wx.rain[i].unwrap_or(0.0);

    let finding = Finding {
        key: "weather",
        level,
        sub,
        title: if heavy.is_some() {
            Text::new("Έρχεται ισχυρή βροχή", "Heavy rain is coming")
        } else if hot.is_some() {
            Text::new("Έρχεται καύσωνας", "A heatwave is coming")
        } else if is_dry {
            Text::new(format!("{dry} ημέρες χωρίς βροχή"), format!("{dry} days without rain"))
        } else {
            Text::new("Καιρός χωρίς ακραία φαινόμενα", "No extreme weather")
        },
        text: Text::both(|l| {
            let tm = tmax.map_or_else(|| "undefined".into(), |t| to_fixed(t, 0));
            let (rt, r30, b90, fc) = (to_fixed(rain_today, 1), to_fixed(rain30, 0), num(balance90, 0), to_fixed(forecast, 0));
            let mut out = if el(l) {
                format!("Σήμερα έως {tm}°C, βροχή {rt} mm. Τελευταίες 30 ημέρες: {r30} mm. Ισοζύγιο 90 ημερών (βροχή − εξατμισοδιαπνοή): {b90} mm. Πρόγνωση 7 ημερών: {fc} mm.")
            } else {
                format!("Today up to {tm}°C, rain {rt} mm. Last 30 days: {r30} mm. 90-day balance (rain − evapotranspiration): {b90} mm. 7-day forecast: {fc} mm.")
            };
            if let Some((d, r, _)) = heavy {
                let (ds, rs) = (fds(d, l, DateStyle::DayMon), to_fixed(r, 0));
                out += &if el(l) { format!(" Στις {ds} αναμένονται {rs} mm σε μία ημέρα.") } else { format!(" {rs} mm expected in one day on {ds}.") };
            }
            out
        }),
        basis: Text::new("Open-Meteo · ημερήσια δεδομένα επανανάλυσης και πρόγνωση", "Open-Meteo · daily reanalysis and forecast"),
        action: None,
    };
    Some(WeatherNow { rain7, rain30, balance90, forecast, dry, tmax, rain_today, level, finding })
}

// ───────────────────────────────────────────────────────── weather vs radar

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Rain,
    Heat,
}

/// A notable weather day (or heatwave) inside the satellite record, with what the next pass saw.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WxEvent {
    #[serde(rename = "type")]
    pub kind: EventKind,
    pub date: Date,
    /// Last day of a heatwave.
    pub end: Option<Date>,
    pub value: f64,
    pub days: u32,
    pub pass: Option<Date>,
    #[serde(rename = "dHH")]
    pub d_hh: Option<f64>,
    pub d_water: Option<f64>,
    pub d_disp: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PassRain {
    pub date: Date,
    /// Rain since the previous pass (30 days before the first one).
    pub rain: f64,
    /// Rain in the 10 days up to the pass.
    pub rain10: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Effects {
    pub passes: Vec<PassRain>,
    pub r: f64,
    pub deficit: f64,
    pub events: Vec<WxEvent>,
    pub finding: Finding,
}

fn weather_effects(s: &Series, wx: &Weather, th: &Thresholds) -> Effects {
    let passes: Vec<PassRain> = s
        .dates
        .iter()
        .enumerate()
        .map(|(i, &d)| PassRain {
            date: d,
            rain: wx_sum(wx, &wx.rain, if i > 0 { s.dates[i - 1].add_days(1) } else { d.add_days(-30) }, d),
            rain10: wx_sum(wx, &wx.rain, d.add_days(-9), d),
        })
        .collect();
    let ok: Vec<usize> = s.hh_db.as_ref().map_or(Vec::new(), |hh| (0..hh.len()).filter(|&i| hh[i].is_some()).collect());
    let r = if ok.len() > 2 {
        let hh = s.hh_db.as_ref().expect("checked");
        corr(&ok.iter().map(|&i| passes[i].rain10).collect::<Vec<_>>(), &ok.iter().map(|&i| hh[i].unwrap_or(0.0)).collect::<Vec<_>>())
    } else {
        0.0
    };
    let last = s.dates[s.dates.len() - 1];
    let first = s.dates[0];
    let (may, sep) = (Date::ymd(last.year(), 5, 1), Date::ymd(last.year(), 9, 30));
    let deficit = -(wx_sum(wx, &wx.rain, may, sep) - wx_sum(wx, &wx.et0, may, sep));

    // notable days inside the satellite record, each paired with the first pass after it
    let mut events: Vec<WxEvent> = Vec::new();
    for (i, &d) in wx.time.iter().enumerate() {
        if d < first || d > last {
            continue;
        }
        if let Some(rv) = wx.rain[i].filter(|&v| v >= th.heavy_rain_mm_day) {
            events.push(WxEvent { kind: EventKind::Rain, date: d, end: None, value: rv, days: 1, pass: None, d_hh: None, d_water: None, d_disp: None });
        }
        let t = wx.tmax[i].unwrap_or(-99.0);
        if t >= th.heat_c {
            match events.last_mut() {
                Some(prev) if prev.kind == EventKind::Heat && d.since(prev.end.unwrap_or(prev.date)) <= 1 => {
                    prev.end = Some(d);
                    prev.value = prev.value.max(t);
                    prev.days += 1;
                }
                _ => events.push(WxEvent { kind: EventKind::Heat, date: d, end: Some(d), value: t, days: 1, pass: None, d_hh: None, d_water: None, d_disp: None }),
            }
        }
    }
    for e in &mut events {
        let from = e.end.unwrap_or(e.date);
        if let Some(k) = s.dates.iter().position(|&d| d >= from).filter(|&k| k > 0) {
            e.pass = Some(s.dates[k]);
            e.d_hh = s.hh_db.as_ref().and_then(|hh| Some(hh[k]? - hh[k - 1]?));
            e.d_water = s.water_km2.as_ref().map(|w| w[k] - w[k - 1]);
            e.d_disp = s.disp_mm.as_ref().map(|w| w[k] - w[k - 1]);
        }
    }
    events.sort_by_key(|e| std::cmp::Reverse(e.date));

    let clear = r.abs() >= 0.5;
    let n_ok = ok.len();
    let finding = Finding {
        key: "effects",
        level: Level::Ok,
        sub: None,
        title: if clear {
            Text::new("Η βροχή φαίνεται στο ραντάρ", "Rain shows up in the radar")
        } else {
            Text::new("Βροχή και ραντάρ: καμία καθαρή σχέση", "Rain and radar: no clear link")
        },
        text: Text::both(|l| {
            let rs = to_fixed(r, 2);
            let mut out = if el(l) {
                format!("Βροχή 10 ημερών πριν από κάθε λήψη έναντι οπισθοσκέδασης HH: r = {rs} σε {n_ok} λήψεις.")
            } else {
                format!("10-day rain before each pass against HH backscatter: r = {rs} over {n_ok} passes.")
            };
            if clear && r > 0.0 {
                out += if el(l) { " Το βρεγμένο έδαφος επιστρέφει περισσότερο σήμα." } else { " Wet soil returns more signal." };
            }
            if deficit > 50.0 {
                let d = to_fixed(deficit, 0);
                out += &if el(l) {
                    format!(" Μάιος–Σεπτέμβριος: έλλειμμα {d} mm, που καλύφθηκε με άρδευση.")
                } else {
                    format!(" May–September: a {d} mm deficit, made up by irrigation.")
                };
            }
            out
        }),
        basis: Text::same("Open-Meteo + NISAR L2 GCOV"),
        action: None,
    };
    Effects { passes, r, deficit, events, finding }
}

// ───────────────────────────────────────────────────────── soil moisture

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Soil {
    pub now: f64,
    pub avg: f64,
    pub drop: f64,
    pub finding: Finding,
}

/// Compared with the other areas on the same date, not with this area's own year: moisture is seasonal.
fn soil(s: &Series, th: &Thresholds, soil_ref: Option<f64>) -> Option<Soil> {
    let v = s.soil_moist_pct.as_deref()?;
    let now = v[v.len() - 1];
    let avg = soil_ref.unwrap_or_else(|| mean(v));
    let drop = (avg - now) / avg * 100.0;
    let level = if soil_ref.is_some() && drop >= th.soil_dry_drop_pct { Level::Watch } else { Level::Ok };
    Some(Soil {
        now,
        avg,
        drop,
        finding: Finding {
            key: "soil",
            level,
            sub: None,
            title: if level == Level::Watch {
                Text::new("Το έδαφος ξεραίνεται", "The soil is drying out")
            } else {
                Text::new("Υγρασία εδάφους κανονική", "Soil moisture normal")
            },
            text: Text::both(|l| {
                let (n, a, dr) = (to_fixed(now, 0), to_fixed(avg, 0), to_fixed(drop, 0));
                if el(l) {
                    let tail = if drop > 0.0 { format!("{dr}% πιο ξερό εδώ") } else { "εδώ πιο υγρό".into() };
                    format!("Υγρασία εδάφους {n}% στην τελευταία λήψη· ο μέσος όρος όλων των περιοχών την ίδια μέρα είναι {a}% ({tail}).")
                } else {
                    let tail = if drop > 0.0 { format!("{dr}% drier here") } else { "wetter here".into() };
                    format!("Soil moisture {n}% at the last pass; the average of all areas that day is {a}% ({tail}).")
                }
            }),
            basis: Text::new(
                "NISAR L-band · το νερό αυξάνει τη διηλεκτρική σταθερά του εδάφους και άρα το σήμα",
                "NISAR L-band · water raises the soil dielectric constant and therefore the signal",
            ),
            action: None,
        },
    })
}

// ───────────────────────────────────────────────────────── wetland

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Wetland {
    pub loss: f64,
    pub ha: f64,
    pub level: Level,
    pub finding: Finding,
}

fn wetland(s: &Series, th: &Thresholds) -> Option<Wetland> {
    let r = s.reed_km2.as_deref()?;
    if max(r) < 0.05 {
        return None;
    }
    let last = r[r.len() - 1];
    let loss = (r[0] - last) / r[0] * 100.0;
    let ha = s.cleared_ha.as_deref().map_or(0.0, |c| c[c.len() - 1] - c[0]);
    let level = if ha >= th.cleared_alert_ha {
        Level::Alert
    } else if ha >= th.cleared_watch_ha || loss >= th.reed_loss_pct {
        Level::Watch
    } else {
        Level::Ok
    };
    let first = s.cleared_ha.as_deref().and_then(|c| c.iter().position(|&x| x > 0.0)).map(|i| s.dates[i]);
    Some(Wetland {
        loss,
        ha,
        level,
        finding: Finding {
            key: "wetland",
            level,
            sub: None,
            title: if level == Level::Ok {
                Text::new("Καλαμιώνες σταθεροί", "Reed beds stable")
            } else {
                Text::new("Ο υγρότοπος γίνεται χωράφι", "Wetland is turning into fields")
            },
            text: Text::both(|l| {
                if level == Level::Ok {
                    let x = to_fixed(last, 1);
                    return if el(l) { format!("Καλαμιώνες {x} km², χωρίς αξιόλογη αλλαγή.") } else { format!("Reed beds {x} km², no notable change.") };
                }
                let (h, lo) = (to_fixed(ha, 0), to_fixed(loss, 0));
                if el(l) {
                    let since = first.map_or_else(String::new, |f| format!(" από {}", fd(f, l)));
                    format!("{h} ha καλαμιώνα άλλαξαν σε γυμνό, οργωμένο έδαφος{since} (−{lo}% έκτασης). Το HV έπεσε (χάθηκε ο όγκος της βλάστησης) και το HH άλλαξε από διπλή ανάκλαση σε τραχιά επιφάνεια: η υπογραφή του οργώματος.")
                } else {
                    let since = first.map_or_else(String::new, |f| format!(" since {}", fd(f, l)));
                    format!("{h} ha of reeds turned into bare, ploughed ground{since} (−{lo}% of the area). HV dropped (the vegetation volume is gone) and HH went from double bounce to a rough surface: the signature of ploughing.")
                }
            }),
            basis: Text::new("NISAR HV (όγκος βλάστησης) + HH (τραχύτητα, διπλή ανάκλαση)", "NISAR HV (vegetation volume) + HH (roughness, double bounce)"),
            action: None,
        },
    })
}

/// What the region could do about each finding. Suggestions for the competent authority, not decisions.
pub fn action_for(key: &str, sub: Option<&str>) -> Option<Text> {
    let k = match sub {
        Some(s) => format!("{key}.{s}"),
        None => key.to_string(),
    };
    let (el, en) = match k.as_str() {
        "reservoir" => ("Ενημέρωση Διεύθυνσης Υδάτων και ΤΟΕΒ· σταδιακός περιορισμός των αρδευτικών απολήψεων από τη λίμνη πριν από το όριο, όχι μετά.", "Notify the Water Directorate and irrigation boards; reduce irrigation withdrawals from the lake gradually before the limit, not after."),
        "wetland" => ("Αυτοψία από τον Φορέα Διαχείρισης (ΟΦΥΠΕΚΑ) στα σημεία που άλλαξαν· έλεγχος χρήσεων γης στη ζώνη προστασίας.", "Field inspection by the protected-area authority at the changed spots; check land use inside the protection zone."),
        "ground" => ("Έλεγχος αδειών και μετρητών στις γεωτρήσεις της ζώνης, περιορισμός αντλήσεων· σταθμός GNSS για επιβεβαίωση.", "Check borehole permits and meters in the zone, limit pumping; a GNSS station to confirm."),
        "soil" => ("Άρδευση με βάση τη μετρημένη υγρασία αντί για σταθερό ωράριο: λιγότερο νερό από λίμνη και γεωτρήσεις.", "Irrigate by measured moisture instead of a fixed schedule: less water from the lake and the boreholes."),
        "egms" => ("Να μη ληφθεί απόφαση μόνο με αυτό: επιβεβαίωση με επόμενες λήψεις ή επιτόπια μέτρηση.", "Do not decide on this alone: confirm with the next passes or a field measurement."),
        "flood" => ("Ενημέρωση Πολιτικής Προστασίας· έλεγχος αναχωμάτων και αγροτικών δρόμων στη ζώνη που πλημμύρισε.", "Notify Civil Protection; check embankments and farm roads in the flooded zone."),
        "weather.heavy" => ("Προειδοποίηση Πολιτικής Προστασίας, καθαρισμός τάφρων και φρεατίων πριν από τη βροχή.", "Civil Protection warning; clear ditches and drains before the rain."),
        "weather.heat" | "weather.dry" => ("Ενημέρωση αγροτών και ΤΟΕΒ για αυξημένη ζήτηση νερού· αυστηρότερη τήρηση ωραρίων.", "Warn farmers and irrigation boards about higher water demand; enforce schedules."),
        _ => return None,
    };
    Some(Text::new(el, en))
}

// ───────────────────────────────────────────────────────── "what if" (no fitted model)

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhatIf {
    /// % change of rain next season.
    pub rain: f64,
    /// % change of irrigation water next season.
    pub irrigation: f64,
}

impl WhatIf {
    pub fn is_change(&self) -> bool {
        self.rain != 0.0 || self.irrigation != 0.0
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scenario {
    pub peak: f64,
    pub low: f64,
    pub base: f64,
    pub limit: f64,
    pub unit: String,
    pub area: Option<f64>,
    pub area_now: Option<f64>,
    pub area_base: Option<f64>,
    /// Linear scale of the shoreline (√ of the area ratio).
    pub scale: f64,
    pub days_below: f64,
    pub sink: Option<f64>,
}

/// "What if" for the lake: rain and irrigation changed by a percentage next season.
/// A deliberately simple water balance built only from the measured series: winter gain (last low → high)
/// scales with rain; summer loss (high → latest low) is 70 % irrigation, 30 % evaporation. Lake area follows
/// storage through a power law fitted to the radar areas; ground sinking scales with pumping.
pub fn scenario(region: &Region, a: &Assessment, w: WhatIf) -> Option<Scenario> {
    let res = region.reservoir.as_ref()?;
    let v = &res.volume;
    if v.is_empty() {
        return None;
    }
    let last = v[v.len() - 1];
    let hi = crate::stats::argmax(v);
    let low_before = min(&v[..=hi]);
    // winter refill: the observed rise to the peak; if the record starts at the peak (no refill seen yet),
    // assume next winter refills the lake to that peak again
    let gain = if v[hi] > low_before { v[hi] - low_before } else { v[hi] - last };
    let loss = v[hi] - last;
    let (r, i) = (w.rain / 100.0, w.irrigation / 100.0);
    let peak = last + gain * (1.0 + r);
    let low = (peak - loss * (0.7 * (1.0 + i) + 0.3)).max(0.0);
    let base = (last + gain - loss).max(0.0);

    let area_of: Option<Box<dyn Fn(f64) -> f64>> = match res.area_km2.as_deref() {
        Some(ar) if ar.len() == v.len() => {
            let xs: Vec<f64> = v.iter().map(|x| x.ln()).collect();
            let ys: Vec<f64> = ar.iter().map(|x| x.max(0.01).ln()).collect();
            let (mx, my) = (mean(&xs), mean(&ys));
            let b = corr(&xs, &ys) * (sum(&ys.iter().map(|y| (y - my).powi(2)).collect::<Vec<_>>()) / sum(&xs.iter().map(|x| (x - mx).powi(2)).collect::<Vec<_>>())).sqrt();
            let c = (my - b * mx).exp();
            Some(Box::new(move |vol: f64| c * vol.max(0.01).powf(b)))
        }
        _ => None,
    };
    let below = (res.limit - low).max(0.0);
    let days_below = if below > 0.0 { js_round(150.0 * below / (loss * (0.7 * (1.0 + i) + 0.3)).max(1.0)) } else { 0.0 };
    Some(Scenario {
        peak,
        low,
        base,
        limit: res.limit,
        unit: res.unit.clone(),
        area: area_of.as_ref().map(|f| f(low)),
        area_now: res.area_km2.as_ref().and_then(|a| a.last().copied()),
        area_base: area_of.as_ref().map(|f| f(base)),
        scale: match (&res.area_km2, &area_of) {
            (Some(_), Some(f)) => (f(low) / f(last)).sqrt(),
            (Some(_), None) => f64::NAN,
            (None, _) => 1.0,
        },
        days_below,
        sink: a.ground.rate.map(|r| r * (1.0 + i)),
    })
}

// ───────────────────────────────────────────────────────── one area

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Assessment {
    pub id: String,
    pub last_pass: Date,
    pub ground: Ground,
    pub reference: Option<RefCheck>,
    pub sensors: Option<Sensors>,
    pub flood: Option<Flood>,
    pub reservoir: Option<ReservoirState>,
    pub aquifer: Option<Aquifer>,
    pub soil: Option<Soil>,
    pub wetland: Option<Wetland>,
    pub weather: Option<WeatherNow>,
    pub effects: Option<Effects>,
    /// Most severe first.
    pub findings: Vec<Finding>,
    pub status: Level,
}

impl Assessment {
    pub fn headline(&self) -> Option<&Finding> {
        self.findings.first()
    }

    /// Re-sorts the findings by severity (stable) and refreshes the status.
    pub fn resort(&mut self) {
        self.findings.sort_by_key(|f| std::cmp::Reverse(f.level));
        self.status = self.findings.first().map_or(Level::Ok, |f| f.level);
    }
}

/// Every conclusion about one area. `soil_ref` = mean soil moisture of all areas at the last pass.
pub fn assess(region: &Region, wx: Option<&Weather>, th: &Thresholds, today: Date, ref_name: &str, soil_ref: Option<f64>) -> Assessment {
    let s = &region.series;
    let g = ground(s, th);
    let reference = reference(s, th, g.rate, ref_name);
    let mut out = Assessment {
        id: region.id.clone(),
        last_pass: s.dates[s.dates.len() - 1],
        reference,
        sensors: sensors(s),
        flood: surface_water(s, wx),
        reservoir: reservoir(region.reservoir.as_ref(), th, wx, today),
        aquifer: aquifer(s),
        soil: soil(s, th, soil_ref),
        wetland: wetland(s, th),
        weather: wx.and_then(|w| weather_now(w, th, today)),
        effects: wx.map(|w| weather_effects(s, w, th)),
        ground: g,
        findings: Vec::new(),
        status: Level::Ok,
    };
    // the legacy code set the action on the shared finding objects, so the per-rule results carry it too
    for x in [
        out.reservoir.as_mut().map(|x| &mut x.finding),
        out.wetland.as_mut().map(|x| &mut x.finding),
        out.ground.finding.as_mut(),
        out.soil.as_mut().map(|x| &mut x.finding),
        out.aquifer.as_mut().map(|x| &mut x.finding),
        out.reference.as_mut().map(|x| &mut x.finding),
        out.flood.as_mut().map(|x| &mut x.finding),
        out.weather.as_mut().map(|x| &mut x.finding),
        out.effects.as_mut().map(|x| &mut x.finding),
        out.sensors.as_mut().map(|x| &mut x.finding),
    ]
    .into_iter()
    .flatten()
    {
        if x.level != Level::Ok {
            x.action = action_for(x.key, x.sub);
        }
    }
    // order = the order the findings are shown in (before sorting by severity)
    let f: Vec<Finding> = [
        out.reservoir.as_ref().map(|x| &x.finding),
        out.wetland.as_ref().map(|x| &x.finding),
        out.ground.finding.as_ref(),
        out.soil.as_ref().map(|x| &x.finding),
        out.aquifer.as_ref().map(|x| &x.finding),
        out.reference.as_ref().map(|x| &x.finding),
        out.flood.as_ref().map(|x| &x.finding),
        out.weather.as_ref().map(|x| &x.finding),
        out.effects.as_ref().map(|x| &x.finding),
        out.sensors.as_ref().map(|x| &x.finding),
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect();
    out.findings = f;
    out.resort();
    out
}
