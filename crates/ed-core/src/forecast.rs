//! The lake water balance: fitted to the radar areas and run forward with real and past weather.
//!
//! ```text
//!   S    = how full the lake is, 0 (dry) .. 1 (fullest seen)
//!   dS   = a · rain − k · ET0 · irrigation factor        a = a_wet (Nov–Mar) or a_dry (Apr–Oct)
//!   area = A_max · S^q                                  (shallow, flat lake)
//! ```
//!
//! [`run`] is the port of `js/forecast.js` (the forecast and its what-ifs); [`fit`] is the port of
//! `tools/fit_forecast.py` (finding the parameters). Both use the same [`step`], which in the legacy code
//! existed once in Python and once in JS.
//!
//! From the last radar pass to the last day with real weather (history + 7-day forecast) the real weather is
//! used. After that, one scenario per past year ("what if the coming months have the weather of 2015, …"),
//! each run with the rain coefficient at 50 %, 100 % and 150 % of the fitted value: the summer passes say
//! little about how fast rain refills the lake, so that stays wide on purpose. The band is the 10th–90th
//! percentile of all runs widened by the model's typical error. Nothing here is guessed: every number comes
//! from the measurements and the weather.

use std::collections::HashMap;

use serde::Serialize;

use crate::model::{Backtest, Climate, ClimateYear, DateSpan, FitSeries, ForecastModel, Params, Reservoir, Weather};
use crate::stats::{brent, least_squares, mean, percentile, pinv_sym};
use crate::Date;

pub const RAIN_FACTORS: [f64; 3] = [0.5, 1.0, 1.5];

/// How many simulated years the weather generator supplies (× the 3 rain factors = members).
pub const SYNTHETIC_YEARS: usize = 150;

/// November to March: rain also runs off the catchment into the lake.
pub fn is_wet(d: Date) -> bool {
    let m = d.month();
    m >= 11 || m <= 3
}

/// 15 April to 30 September.
pub fn is_irrigation(d: Date) -> bool {
    let md = d.md();
    ((4, 15)..=(9, 30)).contains(&md)
}

/// One day of the water balance. `rain_factor` scales the rain coefficient, `loss_factor` the ET0 loss.
#[inline]
pub fn step(s: f64, p: &Params, d: Date, rain: f64, et0: f64, rain_factor: f64, loss_factor: f64) -> f64 {
    let a = if is_wet(d) { p.a_wet } else { p.a_dry };
    (s + a * rain_factor * rain - p.k * et0 * loss_factor).clamp(0.0, 1.0)
}

/// Fill level for an area: the inverse of `area = A_max · S^q`.
pub fn level_of(area: f64, p: &Params) -> f64 {
    (area.min(p.amax) / p.amax).powf(1.0 / p.q)
}

pub fn area_of(s: f64, p: &Params) -> f64 {
    p.amax * s.powf(p.q)
}

// ───────────────────────────────────────────────────────── forecast run

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// % change of future rain.
    pub rain: f64,
    /// % change of irrigation water (15 Apr – 30 Sep).
    pub irrigation: f64,
    /// Days after today.
    pub horizon: i32,
}

impl Options {
    pub fn what_if(rain: f64, irrigation: f64) -> Self {
        Self { rain, irrigation, horizon: 365 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Band {
    pub p50: f64,
    pub p10: f64,
    pub p90: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NextFall {
    pub early: Date,
    pub mid: Date,
    pub late: Date,
    /// Share of the runs that fall below the limit again.
    pub share: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// Last radar pass: the run starts the day after.
    pub start: Date,
    pub today: Date,
    pub dates: Vec<Date>,
    pub p10: Vec<f64>,
    pub p50: Vec<f64>,
    pub p90: Vec<f64>,
    pub members: usize,
    pub limit: f64,
    pub amax: f64,
    pub now: Band,
    pub below_today: f64,
    /// When the band crosses below the limit (earliest on the dry side).
    pub cross: Option<DateSpan>,
    /// When it comes back above, if it is below today.
    pub refill: Option<DateSpan>,
    /// Next summer's fall below the limit.
    pub next: Option<NextFall>,
    pub low_next: Option<f64>,
    pub days_below_next: usize,
    pub weather_until: Option<Date>,
    pub rmse: f64,
}

impl Run {
    pub fn index_of(&self, d: Date) -> Option<usize> {
        self.dates.binary_search(&d).ok()
    }
}

fn first_from(dates: &[Date], from: usize, pred: impl Fn(Date) -> bool) -> Option<usize> {
    (from..dates.len()).find(|&i| pred(dates[i]))
}

/// The forecast from the last pass to `today + horizon`.
pub fn run(fc: &ForecastModel, res: &Reservoir, wx: Option<&Weather>, today: Date, opt: Options) -> Run {
    let p = fc.params;
    let horizon = if opt.horizon == 0 { 365 } else { opt.horizon };
    let limit = res.limit;
    let start = res.dates[res.dates.len() - 1];
    let area0 = *res.area_km2.as_ref().and_then(|a| a.last()).unwrap_or(&res.volume[res.volume.len() - 1]);
    let end = today.add_days(horizon);

    // real weather by day (history + 7-day forecast)
    let mut known: HashMap<Date, (f64, f64)> = HashMap::new();
    let mut last_known = None;
    if let Some(wx) = wx {
        for (i, &d) in wx.time.iter().enumerate() {
            if let (Some(r), Some(e)) = (wx.rain[i], wx.et0[i]) {
                known.insert(d, (r, e));
                last_known = Some(d);
            }
        }
    }

    let days: Vec<Date> = (start.days() + 1..=end.days()).map(Date::from_days).collect();
    let fr = 1.0 + opt.rain / 100.0;
    let fi = 1.0 + opt.irrigation / 100.0;
    let real = |d: Date| last_known.filter(|&lk| d <= lk).and_then(|_| known.get(&d).copied());

    // the futures: one weather sequence per scenario (real weather first, then the scenario's)
    let scenarios: Vec<Vec<(f64, f64)>> = match &fc.generator {
        Some(g) => {
            // simulated years from the Markov-chain generator, from the day after the last real weather
            let from = days.iter().position(|d| real(*d).is_none()).unwrap_or(days.len());
            let wet = last_known.and_then(|d| known.get(&d)).is_some_and(|w| w.0 >= g.threshold);
            // the regime the simulation starts from: the month under way, judged on its real rain so far
            // (or the previous month when the simulation starts on the 1st)
            let regime = days.get(from).map_or(1, |&d0| {
                let probe = if d0.day() == 1 { d0.add_days(-1) } else { d0 };
                let first = Date::ymd(probe.year(), probe.month(), 1);
                let seen: Vec<f64> = (first.days()..d0.days()).filter_map(|n| known.get(&Date::from_days(n)).map(|w| w.0)).collect();
                g.regime_of(probe.month(), seen.iter().sum(), seen.len() as u32, crate::date::days_in_month(probe.year(), probe.month()))
            });
            (0..SYNTHETIC_YEARS)
                .map(|k| {
                    let mut rng = crate::weathergen::Rng::new(0x5EED_0000 + k as u64);
                    let sim = days.get(from).map_or_else(Vec::new, |&d0| g.simulate(d0, days.len() - from, wet, regime, &mut rng));
                    days[..from].iter().map(|d| real(*d).unwrap_or((0.0, 0.0))).chain(sim).collect()
                })
                .collect()
        }
        None => {
            // a scenario year y gives Oct–Dec from y and Jan–Sep from y+1 (one hydrological year)
            let empty = ClimateYear::default();
            fc.climate
                .years
                .iter()
                .copied()
                .filter(|&y| fc.climate.year(y + 1).is_some())
                .map(|y| {
                    days.iter()
                        .map(|&d| {
                            real(d).unwrap_or_else(|| {
                                let yy = if d.month() >= 10 { y } else { y + 1 };
                                let c = fc.climate.year(yy).or_else(|| fc.climate.year(y)).unwrap_or(&empty);
                                let i = d.climate_slot();
                                (c.rain.get(i).copied().flatten().unwrap_or(0.0), c.et0.get(i).copied().flatten().unwrap_or(0.0))
                            })
                        })
                        .collect()
                })
                .collect()
        }
    };

    let members: Vec<Vec<f64>> = scenarios
        .iter()
        .flat_map(|w| RAIN_FACTORS.iter().map(move |&af| (w, af)))
        .map(|(w, af)| {
            let mut s = level_of(area0, &p);
            days.iter()
                .zip(w)
                .map(|(&d, &(rain, et0))| {
                    let future = d >= today; // the what-if changes apply from today on
                    let r = rain * if future { fr } else { 1.0 };
                    // the fitted loss k·ET0 is taken as 30 % evaporation + 70 % irrigation in the irrigation
                    // season; only the what-if uses that split (with no change the factor is 1)
                    let kf = if future && is_irrigation(d) { 0.3 + 0.7 * fi } else { 1.0 };
                    s = step(s, &p, d, r, et0, af, kf);
                    area_of(s, &p)
                })
                .collect()
        })
        .collect();

    let err = fc.rmse;
    let n = days.len();
    let (mut p10, mut p50, mut p90) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    let mut col = vec![0.0; members.len()];
    for i in 0..n {
        for (c, m) in col.iter_mut().zip(&members) {
            *c = m[i];
        }
        col.sort_by(f64::total_cmp);
        p50.push(crate::stats::percentile_sorted(&col, 0.5));
        p10.push((crate::stats::percentile_sorted(&col, 0.1) - err).max(0.0));
        p90.push((crate::stats::percentile_sorted(&col, 0.9) + err).min(p.amax));
    }

    let ti = days.iter().position(|&d| d == today).unwrap_or(0);
    let down = |a: &[f64], from: usize| (from..a.len()).find(|&i| a[i] < limit);
    let up = |a: &[f64], from: usize| (from..a.len()).find(|&i| a[i] >= limit);
    let at = |i: Option<usize>| i.map(|i| days[i]);
    // band edges: p10 is the dry side (falls first, refills last), p90 the wet side
    let span = |f: &dyn Fn(&[f64]) -> Option<usize>| {
        let (e, m, l) = (f(&p10), f(&p50), f(&p90));
        (e.is_some() || m.is_some() || l.is_some()).then(|| DateSpan { early: at(e), mid: at(m), late: at(l) })
    };
    let cross = if n == 0 { None } else { span(&|a| down(a, 0)) };
    let refill = (n > 0 && p50[ti] < limit)
        .then(|| span(&|a| if a[ti] < limit { up(a, ti) } else { Some(ti) }))
        .flatten()
        .map(|r| DateSpan { early: r.late, mid: r.mid, late: r.early });
    // next summer's fall below the limit: the range is the middle half (25th–75th) of the runs' own dates
    let next = (n > 0)
        .then(|| {
            let mut ds: Vec<usize> = members
                .iter()
                .filter_map(|m| {
                    let b = if m[ti] >= limit { Some(ti) } else { up(m, ti) };
                    b.and_then(|b| down(m, b))
                })
                .collect();
            ds.sort_unstable();
            if ds.is_empty() {
                return None;
            }
            let q = |f: f64| days[ds[(ds.len() - 1).min((f * ds.len() as f64).floor() as usize)]];
            // the middle date is where the median curve crosses (stable when a what-if moves runs out of the set)
            let b50 = if p50[ti] >= limit { Some(ti) } else { up(&p50, ti) };
            let m50 = b50.and_then(|b| down(&p50, b));
            Some(NextFall { early: q(0.25), mid: m50.map_or_else(|| q(0.5), |i| days[i]), late: q(0.75), share: ds.len() as f64 / members.len() as f64 })
        })
        .flatten();
    let below_today = if n == 0 { 0.0 } else { members.iter().filter(|m| m[ti] < limit).count() as f64 / members.len() as f64 };
    let june = first_from(&days, 0, |d| d > today && d.md() == (6, 1));
    let nov = june.and_then(|j| first_from(&days, j + 1, |d| d.md() == (11, 1)));
    let summer = june.map(|j| &p50[j..nov.unwrap_or(n)]);
    let low_next = summer.map(crate::stats::min);
    let days_below_next = summer.map_or(0, |s| s.iter().filter(|&&v| v < limit).count());

    Run {
        start,
        today,
        now: if n == 0 { Band { p50: area0, p10: area0, p90: area0 } } else { Band { p50: p50[ti], p10: p10[ti], p90: p90[ti] } },
        dates: days,
        p10,
        p50,
        p90,
        members: members.len(),
        limit,
        amax: p.amax,
        below_today,
        cross,
        refill,
        next,
        low_next,
        days_below_next,
        weather_until: last_known,
        rmse: err,
    }
}

// ───────────────────────────────────────────────────────── fitting

/// Daily rain and ET0 by date (mm); missing days count as dry and still.
pub type DailyWeather = HashMap<Date, (f64, f64)>;

fn wx_at(w: &DailyWeather, d: Date) -> (f64, f64) {
    w.get(&d).copied().unwrap_or((0.0, 0.0))
}

/// Daily lake area from `start` for `days` days (`days + 1` values: the area at the start of each day).
pub fn simulate(p: &Params, start: Date, area0: f64, days: i32, wx: impl Fn(Date) -> (f64, f64)) -> Vec<f64> {
    let mut s = level_of(area0, p);
    (0..=days)
        .map(|i| {
            let d = start.add_days(i);
            let a = area_of(s, p);
            let (rain, et0) = wx(d);
            s = step(s, p, d, rain, et0, 1.0, 1.0);
            a
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct FitConfig {
    /// First weather year used as a forecast scenario.
    pub climate_from: i32,
    /// Passes up to this date are used for the back-test; `None` = the last pass at least 30 days before the last.
    pub backtest_cutoff: Option<Date>,
    /// Day the winter refill assumption starts; `None` = 1 October before the first measurement.
    pub refill_from: Option<Date>,
    /// Fit a Markov-chain weather generator and run the forecast on simulated years.
    pub generator: bool,
    /// Fit the generator to the last this-many years of `wx` only (the climate is shifting);
    /// `None` = all of it.
    pub generator_years: Option<i32>,
}

/// Window the generator is fitted on (years before the end), chosen by the held-out evaluation in
/// `crates/ed-sources/examples/weathergen_eval.rs`.
pub const GENERATOR_YEARS: Option<i32> = Some(30);

/// Wet-day threshold for the generator (mm): reanalysis drizzle below this is not a rain day.
pub const WET_DAY_MM: f64 = 1.0;

impl Default for FitConfig {
    fn default() -> Self {
        Self { climate_from: 2015, backtest_cutoff: None, refill_from: None, generator: true, generator_years: GENERATOR_YEARS }
    }
}

#[derive(Clone, Debug)]
pub struct FitResult {
    pub params: Params,
    pub sd: [f64; 3],
    pub rmse: f64,
    pub rmse_step1: f64,
    pub fitted: Vec<f64>,
    pub refill_low: f64,
}

/// Two steps: (1) `a_dry, k, q` by least squares between the simulated and the measured areas; (2) `a_wet`
/// so that a lake at this record's lowest area on `refill_from` refills to the first measured area with the
/// real weather in between (the measurements have no winter yet).
pub fn fit_params(dates: &[Date], area: &[f64], amax: f64, refill_from: Date, wx: &DailyWeather) -> FitResult {
    let d0 = dates[0];
    let idx: Vec<usize> = dates.iter().map(|d| d.since(d0) as usize).collect();
    let span = idx[idx.len() - 1] as i32;
    let resid = |x: &[f64]| -> Vec<f64> {
        let p = Params { a_wet: x[0], a_dry: x[0], k: x[1], q: x[2], amax };
        let sim = simulate(&p, d0, area[0], span, |d| wx_at(wx, d));
        idx.iter().zip(area).map(|(&i, a)| sim[i] - a).collect()
    };
    let f = least_squares(resid, &[0.001, 0.001, 0.3], &[0.0, 1e-5, 0.05], &[0.05, 0.05, 2.0], 500);
    let (a_dry, k, q) = (f.x[0], f.x[1], f.x[2]);
    let n = area.len();
    let ss: f64 = f.fun.iter().map(|v| v * v).sum();
    let rmse_step1 = (ss / n as f64).sqrt();
    let mut jtj = [0.0; 9];
    for row in 0..n {
        for a in 0..3 {
            for b in 0..3 {
                jtj[a * 3 + b] += f.jac[row * 3 + a] * f.jac[row * 3 + b];
            }
        }
    }
    let cov = pinv_sym(&jtj, 3);
    let scale = ss / (n.saturating_sub(3).max(1)) as f64;
    let sd = [0, 4, 8].map(|i: usize| (cov[i] * scale).max(0.0).sqrt());

    let low = crate::stats::min(area);
    let days = d0.since(refill_from);
    let miss = |a_wet: f64| {
        let p = Params { a_wet, a_dry, k, q, amax };
        simulate(&p, refill_from, low, days, |d| wx_at(wx, d))[days as usize] - area[0]
    };
    let a_wet = if miss(a_dry) < 0.0 && 0.0 < miss(0.05) { brent(miss, a_dry, 0.05, 2e-12, 100).unwrap_or(a_dry) } else { a_dry };
    let params = Params { a_wet, a_dry, k, q, amax };
    let sim = simulate(&params, d0, area[0], span, |d| wx_at(wx, d));
    let fitted: Vec<f64> = idx.iter().map(|&i| sim[i]).collect();
    let rmse = mean(&fitted.iter().zip(area).map(|(s, a)| (s - a).powi(2)).collect::<Vec<_>>()).sqrt();
    FitResult { params, sd, rmse, rmse_step1, fitted, refill_low: low }
}

fn round_to(v: f64, d: usize) -> f64 {
    // Python's round(): correctly rounded, ties to even, which is what format! does too
    format!("{v:.d$}").parse().unwrap_or(v)
}

/// The same date in another year (29 Feb → 28 Feb).
fn in_year(d: Date, y: i32) -> Date {
    Date::try_ymd(y, d.month(), d.day()).unwrap_or_else(|| Date::ymd(y, d.month(), 28))
}

/// Fits the lake model to a reservoir's measured areas and packages it with the back-test and the climate
/// years the forecast runs on. `wx` must cover `climate_from-01-01 ..= end`.
pub fn fit(region_id: &str, res: &Reservoir, wx: &DailyWeather, end: Date, cfg: &FitConfig) -> ForecastModel {
    let dates = &res.dates;
    let area: Vec<f64> = res.area_km2.clone().unwrap_or_else(|| res.volume.clone());
    let amax = res.capacity.max(crate::stats::max(&area));
    let refill_from = cfg.refill_from.unwrap_or_else(|| Date::ymd(dates[0].year() - 1, 10, 1));
    let full = fit_params(dates, &area, amax, refill_from, wx);

    // back-test: fit on the passes up to the cutoff, forecast the last pass with the real weather
    let last = dates[dates.len() - 1];
    let cutoff = cfg.backtest_cutoff.unwrap_or_else(|| last.add_days(-30));
    let cut: Vec<usize> = (0..dates.len()).filter(|&i| dates[i] <= cutoff).collect();
    let backtest = (cut.len() >= 4).then(|| {
        let cd: Vec<Date> = cut.iter().map(|&i| dates[i]).collect();
        let ca: Vec<f64> = cut.iter().map(|&i| area[i]).collect();
        let bt = fit_params(&cd, &ca, amax, refill_from, wx);
        let c0 = cd[cd.len() - 1];
        let a0 = ca[ca.len() - 1];
        let pred = *simulate(&bt.params, c0, a0, last.since(c0), |d| wx_at(wx, d)).last().expect("non-empty");
        // the same back-test model crossing the limit with every past year's weather (what it could have said)
        let mut cross: Vec<Date> = (cfg.climate_from..end.year())
            .filter_map(|y| {
                let tr = simulate(&bt.params, c0, a0, 120, |d| wx_at(wx, in_year(d, y + (d.year() - c0.year()))));
                tr.iter().position(|&v| v < res.limit).map(|i| c0.add_days(i as i32))
            })
            .collect();
        cross.sort();
        Backtest {
            cutoff: c0,
            passes: cd.len(),
            target: last,
            predicted: round_to(pred, 2),
            observed: round_to(area[area.len() - 1], 2),
            cross: (!cross.is_empty()).then(|| DateSpan { early: Some(cross[0]), mid: Some(cross[cross.len() / 2]), late: cross.last().copied() }),
            members: cross.len(),
        }
    });

    // weather scenarios: one per past year, day by day (slot 0 = 1 Jan, 365 = 31 Dec, 29 Feb copies 28 Feb)
    let years: Vec<i32> = (cfg.climate_from..=end.year()).collect();
    let mut days = std::collections::BTreeMap::new();
    for &y in &years {
        let mut cy = ClimateYear { rain: Vec::with_capacity(366), et0: Vec::with_capacity(366) };
        for m in 1..=12u32 {
            for dd in 1..=31u32 {
                let d = match Date::try_ymd(y, m, dd) {
                    Some(d) => d,
                    None if m == 2 && dd == 29 => Date::ymd(y, 2, 28),
                    None => continue,
                };
                let v = wx.get(&d);
                cy.rain.push(v.map(|v| round_to(v.0, 1)));
                cy.et0.push(v.map(|v| round_to(v.1, 2)));
            }
        }
        days.insert(y.to_string(), cy);
    }
    let sd = [("a_dry", full.sd[0]), ("k", full.sd[1]), ("q", full.sd[2])].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    ForecastModel {
        region: region_id.to_string(),
        params: full.params,
        sd,
        rmse: round_to(full.rmse, 2),
        fit: FitSeries { dates: dates.clone(), measured: area.iter().map(|&v| round_to(v, 2)).collect(), model: full.fitted.iter().map(|&v| round_to(v, 2)).collect() },
        backtest,
        climate: Climate { years, until: end, days },
        generator: cfg.generator.then(|| fit_generator(wx, end, cfg.generator_years)).flatten(),
    }
}

/// The weather generator for the lake: monthly regimes on, fitted to the last `years` years up to `end`.
pub fn fit_generator(wx: &DailyWeather, end: Date, years: Option<i32>) -> Option<crate::weathergen::WeatherGen> {
    let from = years.map(|y| Date::ymd(end.year() - y, end.month(), 1));
    let window: DailyWeather = wx.iter().filter(|(d, _)| from.is_none_or(|f| **d >= f) && **d <= end).map(|(d, v)| (*d, *v)).collect();
    crate::weathergen::WeatherGen::fit(&window, WET_DAY_MM, true)
}

/// The daily weather a fitted model's climate block holds (rounded to 0.1 mm / 0.01 mm): enough to refit
/// offline when the original series is not at hand.
pub fn weather_from_climate(c: &Climate) -> DailyWeather {
    let mut out = DailyWeather::new();
    for &y in &c.years {
        let Some(cy) = c.year(y) else { continue };
        let mut d = Date::ymd(y, 1, 1);
        while d.year() == y && d <= c.until {
            let i = d.climate_slot();
            if let (Some(Some(r)), Some(Some(e))) = (cy.rain.get(i), cy.et0.get(i)) {
                out.insert(d, (*r, *e));
            }
            d = d.add_days(1);
        }
    }
    out
}

/// Percentiles helper kept public for the app's charts.
pub fn band_at(members: &[f64]) -> Band {
    Band { p50: percentile(members, 0.5), p10: percentile(members, 0.1), p90: percentile(members, 0.9) }
}

// ───────────────────────────────────────────────────────── between passes

/// How a day between two passes was estimated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Fill {
    /// The water balance run on the real weather, nudged to land on the next pass.
    Model,
    /// Monotone cubic (PCHIP) through the passes: no weather for that stretch.
    Pchip,
}

/// Daily lake area between the measured passes: the model bridge.
///
/// From each pass the water balance runs forward on the real weather of the days in between; the run's
/// miss at the next pass is spread linearly over the interval (a two-point bridge), so the curve passes
/// through every measurement and follows the weather in between. Where weather is missing the stretch falls
/// back to PCHIP. Values stay within `[0, A_max]`. Every value returned is an estimate, never a measurement.
pub fn bridge(fc: &ForecastModel, res: &Reservoir, wx: Option<&Weather>) -> Vec<(Date, f64, Fill)> {
    let p = fc.params;
    let area = res.area_km2.as_ref().unwrap_or(&res.volume);
    let n = res.dates.len().min(area.len());
    if n < 2 {
        return Vec::new();
    }
    let mut daily: HashMap<Date, (f64, f64)> = HashMap::new();
    if let Some(wx) = wx {
        for (i, d) in wx.time.iter().enumerate() {
            if let (Some(r), Some(e)) = (wx.rain[i], wx.et0[i]) {
                daily.insert(*d, (r, e));
            }
        }
    }
    let xs: Vec<f64> = res.dates[..n].iter().map(|d| f64::from(d.days())).collect();
    let pchip = crate::interp::Pchip::new(&xs, &area[..n]);
    let mut out = Vec::new();
    for i in 0..n - 1 {
        let (d0, d1) = (res.dates[i], res.dates[i + 1]);
        let span = d1.since(d0);
        let have = (0..span).all(|k| daily.contains_key(&d0.add_days(k)));
        if have {
            let sim = simulate(&p, d0, area[i], span, |d| daily[&d]);
            let miss = area[i + 1] - sim[span as usize];
            for k in 1..span {
                let v = sim[k as usize] + miss * f64::from(k) / f64::from(span);
                out.push((d0.add_days(k), v.clamp(0.0, p.amax), Fill::Model));
            }
        } else {
            for k in 1..span {
                let d = d0.add_days(k);
                out.push((d, crate::interp::Interpolant::eval(&pchip, f64::from(d.days())), Fill::Pchip));
            }
        }
    }
    out
}
