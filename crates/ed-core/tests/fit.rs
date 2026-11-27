//! M3: the fit reproduces the parameters tools/fit_forecast.py found (scipy TRF). The Python fit used Open-Meteo's
//! unrounded ERA5 series; offline we only have the rounded copy kept in forecast.json, hence tolerances.

use std::fs;

use ed_core::forecast::{fit, weather_from_climate, FitConfig};
use ed_core::model::{ForecastModel, Regions};
use ed_core::Date;

#[test]
fn refit_matches_python() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
    let fc: ForecastModel = serde_json::from_str(&fs::read_to_string(format!("{root}data/real/forecast.json")).unwrap()).unwrap();
    let regions: Regions = serde_json::from_str(&fs::read_to_string(format!("{root}data/real/regions.json")).unwrap()).unwrap();
    let lake = regions.regions.iter().find(|r| r.id == fc.region).unwrap();
    let wx = weather_from_climate(&fc.climate);
    let cfg = FitConfig { climate_from: 2015, backtest_cutoff: Some(Date::ymd(2026, 8, 17)), refill_from: Some(Date::ymd(2025, 10, 1)), generator: false, generator_years: None };
    let t = std::time::Instant::now();
    let got = fit(&lake.id, lake.reservoir.as_ref().unwrap(), &wx, fc.climate.until, &cfg);
    println!("fit in {:?}", t.elapsed());
    let (p, w) = (got.params, fc.params);
    println!("rust   {p:?} rmse {}", got.rmse);
    println!("python {w:?} rmse {}", fc.rmse);
    println!("model  {:?}\npython {:?}", got.fit.model, fc.fit.model);
    println!("backtest rust {:?}\n         py   {:?}", got.backtest, fc.backtest);
    let rel = |a: f64, b: f64| (a - b).abs() / b.abs();
    assert!(rel(p.a_dry, w.a_dry) < 1e-6, "a_dry");
    assert!(rel(p.a_wet, w.a_wet) < 1e-6, "a_wet");
    assert!(rel(p.k, w.k) < 1e-6, "k");
    assert!(rel(p.q, w.q) < 1e-6, "q");
    assert_eq!(p.amax, w.amax);
    assert!((got.rmse - fc.rmse).abs() < 0.05);
    for (a, b) in got.fit.model.iter().zip(&fc.fit.model) {
        assert!((a - b).abs() < 0.011, "fitted areas {a} vs {b}");
    }
    let (bt, wbt) = (got.backtest.unwrap(), fc.backtest.clone().unwrap());
    assert_eq!((bt.cutoff, bt.passes, bt.target, bt.members), (wbt.cutoff, wbt.passes, wbt.target, wbt.members));
    assert_eq!(bt.predicted, wbt.predicted);
    let (c, wc) = (bt.cross.unwrap(), wbt.cross.unwrap());
    for (a, b) in [(c.early, wc.early), (c.mid, wc.mid), (c.late, wc.late)] {
        assert_eq!(a, b, "cross");
    }
    // the climate block is rebuilt identically from the same (rounded) weather
    assert_eq!(got.climate.years, fc.climate.years);
    for y in &fc.climate.years {
        assert_eq!(got.climate.year(*y).unwrap().rain, fc.climate.year(*y).unwrap().rain, "rain {y}");
    }
}

#[test]
fn koronia_model_passes_the_quality_gate() {
    let fc: ForecastModel = serde_json::from_str(&fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/real/forecast.json")).unwrap()).unwrap();
    let r2 = ed_core::forecast::fit_quality(&fc).expect("the radar-based Koronia model is well determined");
    assert!(r2 > 0.8, "{r2}");
}
