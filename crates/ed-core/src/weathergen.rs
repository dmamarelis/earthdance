//! A stochastic daily weather generator (Richardson 1981 / WGEN family), fitted to a place's history.
//!
//! * **Occurrence** — a first-order two-state Markov chain per calendar month:
//!   `p01 = P(wet today | dry yesterday)`, `p11 = P(wet today | wet yesterday)`.
//! * **Amount** on wet days — a gamma distribution per month, fitted by maximum likelihood
//!   (Thom's 1958 approximation of the shape).
//! * **ET0** — normal per month *and* per wet/dry state (wet days are cloudier, so evaporate less), with an
//!   AR(1) memory of the standardised anomaly, truncated at 0.
//!
//! The lake forecast used to replay the 11 past years 2015–2025 as its futures; with the generator it can
//! run hundreds of plausible years fitted to many more (ERA5 goes back to 1940), so the tails (a dry winter,
//! a long dry spell) are sampled instead of being whatever happened to occur in a decade.
//!
//! Known limit, stated rather than hidden: a first-order chain with independent amounts underestimates the
//! year-to-year variance of totals ("overdispersion"); [`Stats`] measures it so the app can say by how much.

use serde::{Deserialize, Serialize};

use crate::forecast::DailyWeather;
use crate::Date;

// ───────────────────────────────────────────────────────── random numbers

/// xoshiro256++ (Blackman & Vigna), seeded through SplitMix64. Small, fast, reproducible across platforms.
#[derive(Clone, Debug)]
pub struct Rng([u64; 4]);

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut z = seed;
        let mut next = || {
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            x ^ (x >> 31)
        };
        Self([next(), next(), next(), next()])
    }

    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.0;
        let out = (s[0].wrapping_add(s[3])).rotate_left(23).wrapping_add(s[0]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        out
    }

    /// Uniform in [0, 1) with 53 random bits.
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Standard normal (Box–Muller; one of the pair is discarded for simplicity).
    pub fn normal(&mut self) -> f64 {
        let u1 = 1.0 - self.uniform(); // (0, 1]
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    /// Gamma(shape, 1) by Marsaglia & Tsang (2000); shape < 1 via the `U^(1/k)` boost.
    pub fn gamma(&mut self, shape: f64) -> f64 {
        if shape < 1.0 {
            let u = 1.0 - self.uniform();
            return self.gamma(shape + 1.0) * u.powf(1.0 / shape);
        }
        let d = shape - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();
        loop {
            let (x, v) = loop {
                let x = self.normal();
                let v = 1.0 + c * x;
                if v > 0.0 {
                    break (x, v * v * v);
                }
            };
            let u = self.uniform();
            if u < 1.0 - 0.0331 * x.powi(4) || u.ln() < 0.5 * x * x + d * (1.0 - v + v.ln()) {
                return d * v;
            }
        }
    }
}

// ───────────────────────────────────────────────────────── the model

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Month {
    /// P(wet | dry yesterday)
    pub p01: f64,
    /// P(wet | wet yesterday)
    pub p11: f64,
    /// Gamma shape and scale of the rain on wet days (mm).
    pub shape: f64,
    pub scale: f64,
    /// ET0 mean and standard deviation on dry days and on wet days (mm).
    pub et0_dry: (f64, f64),
    pub et0_wet: (f64, f64),
}

/// Regimes of a month by its rain total: below the lower tercile, between, above the upper tercile.
pub const REGIMES: usize = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WeatherGen {
    /// A day is wet when it rains at least this much (mm).
    pub threshold: f64,
    /// Daily parameters per calendar month and regime: index `month * REGIMES + regime`. With regimes
    /// off, all three regimes of a month hold the same parameters.
    pub table: Vec<Month>,
    /// Upper rain total of the dry and of the normal regime, per calendar month (mm).
    pub regime_cut: Vec<[f64; 2]>,
    /// Month-to-month regime transitions `P(next | current)`: the slow chain that lets dry (or wet) months
    /// follow each other, which a daily chain alone cannot do.
    pub regime_p: [[f64; REGIMES]; REGIMES],
    pub regimes: bool,
    /// Lag-1 autocorrelation of the standardised ET0 anomaly.
    pub et0_phi: f64,
    /// First and last year of the history it was fitted to.
    pub years: (i32, i32),
}

/// Maximum-likelihood gamma fit (Thom's approximation): `A = ln x̄ − mean(ln x)`,
/// `shape = (1 + √(1 + 4A/3)) / 4A`, `scale = x̄ / shape`.
pub fn fit_gamma(x: &[f64]) -> (f64, f64) {
    if x.len() < 2 {
        let m = x.first().copied().unwrap_or(1.0).max(0.1);
        return (1.0, m);
    }
    let n = x.len() as f64;
    let mean = x.iter().sum::<f64>() / n;
    let mean_ln = x.iter().map(|v| v.max(1e-6).ln()).sum::<f64>() / n;
    let a = (mean.ln() - mean_ln).max(1e-6);
    let shape = (1.0 + (1.0 + 4.0 * a / 3.0).sqrt()) / (4.0 * a);
    (shape, mean / shape)
}

fn mean_sd(v: &[f64]) -> (f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    let n = v.len() as f64;
    let m = v.iter().sum::<f64>() / n;
    let var = v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0).max(1.0);
    (m, var.sqrt())
}

#[derive(Default)]
struct Acc {
    n: [usize; 4], // dry→dry, dry→wet, wet→dry, wet→wet
    amounts: Vec<f64>,
    et0: [Vec<f64>; 2],
}

fn month_from(acc: &Acc) -> Month {
    let [dd, dw, wd, ww] = acc.n;
    let (shape, scale) = fit_gamma(&acc.amounts);
    let et0_dry = mean_sd(&acc.et0[0]);
    Month {
        // a pinch of prior keeps sparse cells away from 0 or 1
        p01: (dw as f64 + 0.5) / ((dd + dw) as f64 + 1.0),
        p11: (ww as f64 + 0.5) / ((wd + ww) as f64 + 1.0),
        shape,
        scale,
        et0_dry,
        et0_wet: if acc.et0[1].len() >= 5 { mean_sd(&acc.et0[1]) } else { et0_dry },
    }
}

impl WeatherGen {
    /// Fits the generator to every day in `wx`. Transitions are counted only between consecutive days that
    /// both have data. With `regimes`, each month of the record is classed dry / normal / wet by its total
    /// (terciles per calendar month), the month-to-month regime chain is fitted, and the daily parameters
    /// are fitted per (calendar month, regime).
    pub fn fit(wx: &DailyWeather, threshold: f64, regimes: bool) -> Option<Self> {
        let mut days: Vec<(Date, (f64, f64))> = wx.iter().map(|(d, v)| (*d, *v)).collect();
        days.sort_by_key(|(d, _)| *d);
        if days.len() < 365 {
            return None;
        }
        // monthly totals → terciles → the regime of every month of the record
        let mut totals: std::collections::BTreeMap<(i32, u32), (f64, usize)> = std::collections::BTreeMap::new();
        for (d, (r, _)) in &days {
            let e = totals.entry((d.year(), d.month())).or_default();
            e.0 += r;
            e.1 += 1;
        }
        let full: Vec<((i32, u32), f64)> = totals.iter().filter(|((y, m), (_, n))| *n == crate::date::days_in_month(*y, *m) as usize).map(|(k, v)| (*k, v.0)).collect();
        let regime_cut: Vec<[f64; 2]> = (1..=12u32)
            .map(|m| {
                let v: Vec<f64> = full.iter().filter(|((_, mm), _)| *mm == m).map(|(_, t)| *t).collect();
                if v.len() < 3 { [f64::INFINITY; 2] } else { [crate::stats::percentile(&v, 1.0 / 3.0), crate::stats::percentile(&v, 2.0 / 3.0)] }
            })
            .collect();
        let regime_of = |y: i32, m: u32| -> usize {
            if !regimes {
                return 1;
            }
            let Some((t, n)) = totals.get(&(y, m)) else { return 1 };
            // a partial month is judged on its pace
            let t = t * f64::from(crate::date::days_in_month(y, m)) / (*n).max(1) as f64;
            let c = regime_cut[(m - 1) as usize];
            if t <= c[0] { 0 } else if t <= c[1] { 1 } else { 2 }
        };
        let mut rp = [[0.5f64; REGIMES]; REGIMES];
        let keys: Vec<(i32, u32)> = totals.keys().copied().collect();
        for w in keys.windows(2) {
            let ((y0, m0), (y1, m1)) = (w[0], w[1]);
            if (y1 * 12 + m1 as i32) - (y0 * 12 + m0 as i32) == 1 {
                rp[regime_of(y0, m0)][regime_of(y1, m1)] += 1.0;
            }
        }
        for row in &mut rp {
            let s: f64 = row.iter().sum();
            for v in row.iter_mut() {
                *v /= s;
            }
        }

        let mut acc: Vec<Acc> = (0..12 * REGIMES).map(|_| Acc::default()).collect();
        let mut pooled: Vec<Acc> = (0..12).map(|_| Acc::default()).collect();
        for w in days.windows(2) {
            let (d0, (r0, _)) = w[0];
            let (d1, (r1, e1)) = w[1];
            let m = (d1.month() - 1) as usize;
            let k = m * REGIMES + regime_of(d1.year(), d1.month());
            let (wet0, wet1) = (r0 >= threshold, r1 >= threshold);
            for a in [&mut acc[k], &mut pooled[m]] {
                if d1.since(d0) == 1 {
                    a.n[usize::from(wet0) * 2 + usize::from(wet1)] += 1;
                }
                if wet1 {
                    a.amounts.push(r1);
                }
                a.et0[usize::from(wet1)].push(e1);
            }
        }
        let table: Vec<Month> = (0..12 * REGIMES)
            .map(|k| if regimes && acc[k].n.iter().sum::<usize>() >= 60 { month_from(&acc[k]) } else { month_from(&pooled[k / REGIMES]) })
            .collect();

        // lag-1 autocorrelation of the standardised ET0 anomaly
        let z: Vec<Option<f64>> = days
            .iter()
            .map(|(d, (r, e))| {
                let mo = &table[(d.month() - 1) as usize * REGIMES + regime_of(d.year(), d.month())];
                let (mu, sd) = if *r >= threshold { mo.et0_wet } else { mo.et0_dry };
                (sd > 0.0).then(|| (e - mu) / sd)
            })
            .collect();
        let (mut sxy, mut sxx) = (0.0, 0.0);
        for (i, w) in days.windows(2).enumerate() {
            if let (Some(a), Some(b)) = (z[i], z[i + 1])
                && w[1].0.since(w[0].0) == 1
            {
                sxy += a * b;
                sxx += a * a;
            }
        }
        let et0_phi = if sxx > 0.0 { (sxy / sxx).clamp(0.0, 0.99) } else { 0.0 };
        Some(Self { threshold, table, regime_cut, regime_p: rp, regimes, et0_phi, years: (days[0].0.year(), days[days.len() - 1].0.year()) })
    }

    /// The regime a month is in, from the rain seen so far in it (`days_seen` of the month).
    pub fn regime_of(&self, month: u32, rain: f64, days_seen: u32, days_in_month: u32) -> usize {
        if !self.regimes || days_seen == 0 {
            return 1;
        }
        let t = rain * f64::from(days_in_month) / f64::from(days_seen);
        let c = self.regime_cut[(month - 1) as usize];
        if t <= c[0] { 0 } else if t <= c[1] { 1 } else { 2 }
    }

    pub fn month(&self, month: u32, regime: usize) -> &Month {
        &self.table[(month - 1) as usize * REGIMES + regime]
    }

    /// `n` days of (rain, ET0) starting on `start`. `wet`: was the day before wet; `regime`: the regime of the
    /// month before `start` (or of `start`'s month when it is already under way).
    pub fn simulate(&self, start: Date, n: usize, mut wet: bool, mut regime: usize, rng: &mut Rng) -> Vec<(f64, f64)> {
        let mut z = 0.0;
        let k = (1.0 - self.et0_phi * self.et0_phi).sqrt();
        (0..n)
            .map(|i| {
                let d = start.add_days(i as i32);
                if self.regimes && d.day() == 1 {
                    let u = rng.uniform();
                    let row = self.regime_p[regime];
                    regime = if u < row[0] { 0 } else if u < row[0] + row[1] { 1 } else { 2 };
                }
                let mo = self.month(d.month(), regime);
                wet = rng.uniform() < if wet { mo.p11 } else { mo.p01 };
                let rain = if wet { (mo.scale * rng.gamma(mo.shape)).max(self.threshold) } else { 0.0 };
                z = self.et0_phi * z + k * rng.normal();
                let (mu, sd) = if wet { mo.et0_wet } else { mo.et0_dry };
                (rain, (mu + sd * z).max(0.0))
            })
            .collect()
    }
}

// ───────────────────────────────────────────────────────── how good is it?

/// Summary statistics of a daily record, to compare history with simulations.
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    /// Wet-day frequency per month.
    pub wet_freq: [f64; 12],
    /// Mean monthly rain total (mm).
    pub monthly_rain: [f64; 12],
    /// Mean monthly ET0 total (mm).
    pub monthly_et0: [f64; 12],
    /// Mean length of dry spells (days).
    pub dry_spell: f64,
    /// Standard deviation of the November–March rain total (mm): what refills the lake.
    pub winter_sd: f64,
    pub winter_mean: f64,
}

pub fn stats(days: &[(Date, f64, f64)], threshold: f64) -> Stats {
    let mut wet = [0.0f64; 12];
    let mut cnt = [0.0f64; 12];
    let mut rain = [0.0; 12];
    let mut et0 = [0.0; 12];
    let mut month_keys = std::collections::BTreeSet::new();
    let mut spells = Vec::new();
    let mut run = 0usize;
    let mut winters: std::collections::BTreeMap<i32, f64> = std::collections::BTreeMap::new();
    for &(d, r, e) in days {
        let m = (d.month() - 1) as usize;
        cnt[m] += 1.0;
        rain[m] += r;
        et0[m] += e;
        month_keys.insert((d.year(), d.month()));
        if r >= threshold {
            wet[m] += 1.0;
            if run > 0 {
                spells.push(run as f64);
            }
            run = 0;
        } else {
            run += 1;
        }
        let mo = d.month();
        if mo >= 11 || mo <= 3 {
            *winters.entry(if mo >= 11 { d.year() } else { d.year() - 1 }).or_default() += r;
        }
    }
    let mut n_months = [0.0f64; 12];
    for (_, m) in &month_keys {
        n_months[(*m - 1) as usize] += 1.0;
    }
    // only complete winters
    let first = days.first().map(|x| x.0);
    let last = days.last().map(|x| x.0);
    let w: Vec<f64> = winters
        .iter()
        .filter(|(y, _)| first.is_some_and(|f| f <= Date::ymd(**y, 11, 1)) && last.is_some_and(|l| l >= Date::ymd(**y + 1, 3, 31)))
        .map(|(_, v)| *v)
        .collect();
    let (winter_mean, winter_sd) = mean_sd(&w);
    Stats {
        wet_freq: std::array::from_fn(|m| wet[m] / cnt[m].max(1.0)),
        monthly_rain: std::array::from_fn(|m| rain[m] / n_months[m].max(1.0)),
        monthly_et0: std::array::from_fn(|m| et0[m] / n_months[m].max(1.0)),
        dry_spell: spells.iter().sum::<f64>() / spells.len().max(1) as f64,
        winter_sd,
        winter_mean,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamma_sampler_and_fit_agree() {
        let mut rng = Rng::new(7);
        for &(k, th) in &[(0.6, 8.0), (1.0, 3.0), (2.5, 4.0)] {
            let x: Vec<f64> = (0..200_000).map(|_| th * rng.gamma(k)).collect();
            let (k2, th2) = fit_gamma(&x);
            assert!((k2 - k).abs() / k < 0.03, "shape {k} → {k2}");
            assert!((k2 * th2 - k * th).abs() / (k * th) < 0.01, "mean");
        }
    }

    #[test]
    fn markov_chain_recovers_its_own_parameters() {
        // simulate from known parameters, refit, compare
        let m = Month { p01: 0.2, p11: 0.6, shape: 0.8, scale: 9.0, et0_dry: (3.0, 1.0), et0_wet: (1.5, 0.6) };
        let mut g = WeatherGen { threshold: 0.5, table: vec![m; 12 * REGIMES], regime_cut: vec![[0.0; 2]; 12], regime_p: [[1.0 / 3.0; 3]; 3], regimes: false, et0_phi: 0.6, years: (0, 0) };
        for r in 0..REGIMES {
            g.table[6 * REGIMES + r].p01 = 0.05;
        }
        let mut rng = Rng::new(1);
        let start = Date::ymd(1950, 1, 1);
        let days = g.simulate(start, 365 * 200, false, 1, &mut rng);
        let wx: DailyWeather = days.iter().enumerate().map(|(i, v)| (start.add_days(i as i32), *v)).collect();
        let f = WeatherGen::fit(&wx, 0.5, false).unwrap();
        let (jan, jul, apr) = (f.month(1, 1), f.month(7, 1), f.month(4, 1));
        assert!((jan.p01 - 0.2).abs() < 0.01 && (jan.p11 - 0.6).abs() < 0.02, "{jan:?}");
        assert!((jul.p01 - 0.05).abs() < 0.01);
        assert!((f.et0_phi - 0.6).abs() < 0.05, "phi {}", f.et0_phi);
        assert!((apr.et0_wet.0 - 1.5).abs() < 0.1);
        // with regimes on, the persistent regime chain must come out persistent
        let fr = WeatherGen::fit(&wx, 0.5, true).unwrap();
        assert!(fr.regime_p.iter().all(|row| (row.iter().sum::<f64>() - 1.0).abs() < 1e-12));
    }
}
