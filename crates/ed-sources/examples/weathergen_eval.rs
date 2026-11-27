//! Is the Markov-chain weather generator better than replaying past years?
//!
//! ERA5 daily rain + ET0 at Lake Koronia, 1950 → now (Open-Meteo, cached). For every held-out hydrological
//! year Y (Oct Y-1 → Sep Y) from 2005 on, two forecasts of the quantities that move the lake:
//!   * winter rain (Nov–Mar), the refill;
//!   * summer deficit (Apr–Sep ET0 − rain), the drain.
//!
//! The forecasts: replaying the 11 hydrological years before Y (what the legacy forecast did), and 500
//! simulated years from four generator variants (daily chain alone or with monthly regimes; fitted to all
//! years before Oct Y-1 or to the last 30). Scored with CRPS (lower = better) and the share of years falling
//! inside the ensemble's 10–90 % band (ideal 80 %).
//!
//!     cargo run --release -p ed-sources --example weathergen_eval

use ed_core::forecast::{DailyWeather, WET_DAY_MM};
use ed_core::weathergen::{stats, Rng, WeatherGen};
use ed_core::Date;

/// CRPS of an ensemble against an observation: E|X − y| − ½ E|X − X'| (sorted-sample formula).
fn crps(ens: &[f64], y: f64) -> f64 {
    let mut s = ens.to_vec();
    s.sort_by(f64::total_cmp);
    let n = s.len() as f64;
    let a: f64 = s.iter().map(|x| (x - y).abs()).sum::<f64>() / n;
    let b: f64 = s.iter().enumerate().map(|(i, x)| x * (2.0 * i as f64 + 1.0 - n)).sum::<f64>() / (n * n);
    a - b
}

fn pct(ens: &[f64], p: f64) -> f64 {
    ed_core::stats::percentile(ens, p)
}

/// (winter rain, summer deficit) of the hydrological year ending in September `y`, from a lookup.
fn season(y: i32, at: impl Fn(Date) -> Option<(f64, f64)>) -> Option<(f64, f64)> {
    let mut w = 0.0;
    let mut d = Date::ymd(y - 1, 11, 1);
    while d <= Date::ymd(y, 3, 31) {
        w += at(d)?.0;
        d = d.add_days(1);
    }
    let mut s = 0.0;
    let mut d = Date::ymd(y, 4, 1);
    while d <= Date::ymd(y, 9, 30) {
        let (r, e) = at(d)?;
        s += e - r;
        d = d.add_days(1);
    }
    Some((w, s))
}

fn main() {
    let (lon, lat) = (23.145, 40.682);
    let to = ed_sources::today_local().add_days(-3);
    let wx: DailyWeather = ed_sources::weather::daily_rain_et0(lon, lat, Date::ymd(1950, 1, 1), to).expect("ERA5 history");
    println!("ERA5 days: {}", wx.len());

    // 1. how well the generator reproduces the climate it was fitted to (and out of sample)
    let split = Date::ymd(2014, 12, 31);
    let train: DailyWeather = wx.iter().filter(|(d, _)| **d <= split).map(|(d, v)| (*d, *v)).collect();
    let g = WeatherGen::fit(&train, WET_DAY_MM, true).expect("fit");
    let mut rows: Vec<(Date, f64, f64)> = wx.iter().map(|(d, v)| (*d, v.0, v.1)).collect();
    rows.sort_by_key(|r| r.0);
    let obs_in = stats(&rows.iter().copied().filter(|r| r.0 <= split).collect::<Vec<_>>(), WET_DAY_MM);
    let obs_out = stats(&rows.iter().copied().filter(|r| r.0 > split).collect::<Vec<_>>(), WET_DAY_MM);
    let mut rng = Rng::new(42);
    let start = Date::ymd(3000, 1, 1);
    let sim: Vec<(Date, f64, f64)> = g.simulate(start, 365 * 400, false, 1, &mut rng).into_iter().enumerate().map(|(i, (r, e))| (start.add_days(i as i32), r, e)).collect();
    let s = stats(&sim, WET_DAY_MM);
    println!("\n                     ERA5 1950–2014   ERA5 2015–now   gen+regimes (400 yr)");
    let row = |name: &str, a: f64, b: f64, c: f64| println!("{name:<20} {a:>14.2} {b:>15.2} {c:>18.2}");
    for (m, name) in [(0, "Jan"), (3, "Apr"), (6, "Jul"), (9, "Oct")] {
        row(&format!("wet-day freq {name}"), obs_in.wet_freq[m], obs_out.wet_freq[m], s.wet_freq[m]);
        row(&format!("rain {name} (mm)"), obs_in.monthly_rain[m], obs_out.monthly_rain[m], s.monthly_rain[m]);
    }
    row("ET0 Jul (mm)", obs_in.monthly_et0[6], obs_out.monthly_et0[6], s.monthly_et0[6]);
    row("mean dry spell (d)", obs_in.dry_spell, obs_out.dry_spell, s.dry_spell);
    row("winter rain mean", obs_in.winter_mean, obs_out.winter_mean, s.winter_mean);
    row("winter rain sd", obs_in.winter_sd, obs_out.winter_sd, s.winter_sd);

    // 2. forecast skill on held-out years, for several variants
    let at = |d: Date| wx.get(&d).copied();
    // (name, regimes, window in years)
    let variants: [(&str, bool, Option<i32>); 4] = [
        ("daily chain, all yrs", false, None),
        ("+ regimes, all yrs", true, None),
        ("daily chain, 30 yrs", false, Some(30)),
        ("+ regimes, 30 yrs", true, Some(30)),
    ];
    let nm = variants.len() + 1;
    let mut score = vec![[0.0f64; 2]; nm];
    let mut cover = vec![[0usize; 2]; nm];
    let mut spread = vec![[0.0f64; 2]; nm];
    let mut n = 0;
    for y in 2005..=to.year() {
        let Some(obs) = season(y, at) else { continue };
        let cut = Date::ymd(y - 1, 9, 30);
        let mut ens: Vec<Vec<(f64, f64)>> = vec![(y - 11..y).filter_map(|yy| season(yy, at)).collect()];
        for (_, regimes, window) in variants {
            let from = window.map_or(Date::ymd(1900, 1, 1), |w| Date::ymd(y - 1 - w, 10, 1));
            let hist: DailyWeather = wx.iter().filter(|(d, _)| **d <= cut && **d >= from).map(|(d, v)| (*d, *v)).collect();
            let g = WeatherGen::fit(&hist, WET_DAY_MM, regimes).expect("fit");
            let sept: f64 = (0..30).filter_map(|k| at(Date::ymd(y - 1, 9, 1).add_days(k))).map(|v| v.0).sum();
            let regime = g.regime_of(9, sept, 30, 30);
            let wet = at(cut).is_some_and(|v| v.0 >= WET_DAY_MM);
            let mut rng = Rng::new(y as u64);
            let d0 = Date::ymd(y - 1, 10, 1);
            ens.push((0..500).filter_map(|_| { let seq = g.simulate(d0, 366, wet, regime, &mut rng); season(y, |d| seq.get(d.since(d0) as usize).copied()) }).collect());
        }
        n += 1;
        for (mi, e) in ens.iter().enumerate() {
            for (qi, o) in [obs.0, obs.1].into_iter().enumerate() {
                let q: Vec<f64> = e.iter().map(|v| if qi == 0 { v.0 } else { v.1 }).collect();
                score[mi][qi] += crps(&q, o);
                spread[mi][qi] += pct(&q, 0.9) - pct(&q, 0.1);
                if o >= pct(&q, 0.1) && o <= pct(&q, 0.9) {
                    cover[mi][qi] += 1;
                }
            }
        }
    }
    let n = n as f64;
    println!("\nheld-out hydrological years 2005–{}: {n}   (CRPS: lower is better; coverage of the 10–90 % band: ideal 80 %)", to.year());
    println!("{:<24} {:>11} {:>12} {:>10} {:>11} {:>12} {:>13}", "", "CRPS winter", "CRPS deficit", "cov winter", "cov deficit", "band winter", "band deficit");
    let names: Vec<&str> = std::iter::once("replay last 11 years").chain(variants.iter().map(|v| v.0)).collect();
    for (mi, name) in names.iter().enumerate() {
        println!(
            "{name:<24} {:>11.1} {:>12.1} {:>9.0}% {:>10.0}% {:>12.0} {:>13.0}",
            score[mi][0] / n, score[mi][1] / n, cover[mi][0] as f64 / n * 100.0, cover[mi][1] as f64 / n * 100.0, spread[mi][0] / n, spread[mi][1] / n
        );
    }
}
