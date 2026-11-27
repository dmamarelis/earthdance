//! End to end: the repository's GeoTIFFs → ed-raster → fitted lake model → every rule, against what the
//! legacy Python + JS produced (data/real/*.json and tests/fixtures/assess_real_wx.json).

use std::fs;

use ed_core::forecast::weather_from_climate;
use ed_core::model::{ForecastModel, Regions, WeatherSet};
use ed_core::status::compute;
use ed_core::Date;
use ed_sources::real::{build, FitWeather, LocalDirs};
use ed_sources::data_root;

fn json<T: serde::de::DeserializeOwned>(p: &str) -> T {
    serde_json::from_str(&fs::read_to_string(data_root().join(p)).unwrap()).unwrap()
}

#[test]
fn real_set_reproduces_the_legacy_pipeline() {
    let root = data_root();
    let legacy_fc: ForecastModel = json("data/real/forecast.json");
    let legacy: Regions = json("data/real/regions.json");
    // the legacy fit ran with end = its run day − 2 = climate.until
    let today = legacy_fc.climate.until.add_days(2);
    let t = std::time::Instant::now();
    let ds = build(&LocalDirs::in_repo(&root), FitWeather::Given(weather_from_climate(&legacy_fc.climate)), today, &|s| println!("{s}")).unwrap();
    println!("built in {:?}", t.elapsed());

    assert_eq!(ds.layers.len(), 2);
    assert_eq!(ds.layers[0].frames.len(), 12);
    assert_eq!(ds.layers[0].frames.iter().filter(|f| f.empty).count(), 2);
    assert_eq!(ds.optical.len(), 16);
    for (got, want) in ds.regions.regions.iter().zip(&legacy.regions) {
        assert_eq!(got.id, want.id);
        assert_eq!(got.series.dates, want.series.dates);
        assert_eq!(got.series.water_km2, want.series.water_km2);
        assert_eq!(got.series.hh_db, want.series.hh_db);
        assert_eq!(got.reservoir.as_ref().map(|r| (r.capacity, r.limit)), want.reservoir.as_ref().map(|r| (r.capacity, r.limit)));
    }
    let fc = ds.forecast.as_ref().expect("fitted");
    let rel = |a: f64, b: f64| (a - b).abs() / b.abs();
    assert!(rel(fc.params.a_dry, legacy_fc.params.a_dry) < 1e-6 && rel(fc.params.k, legacy_fc.params.k) < 1e-6 && rel(fc.params.q, legacy_fc.params.q) < 1e-6);
    assert_eq!(fc.backtest, legacy_fc.backtest);

    // every rule, on the fresh data, with the saved weather and the fixture's day
    let wx: WeatherSet = json("data/real/weather.json");
    let st = compute(&ds.regions, Some(&wx), true, ds.reference_name(), Some(fc), Date::ymd(2026, 11, 14));
    let fixture: serde_json::Value = json("tests/fixtures/assess_real_wx.json");
    for (id, a) in fixture["A"].as_object().unwrap() {
        let got = serde_json::to_value(&st.get(id).findings).unwrap();
        assert_eq!(got, drop_nulls(a["findings"].clone()), "findings of {id}");
    }
}

fn drop_nulls(v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Object(m) => Value::Object(m.into_iter().filter(|(_, x)| !x.is_null()).map(|(k, x)| (k, drop_nulls(x))).collect()),
        Value::Array(a) => Value::Array(a.into_iter().map(drop_nulls).collect()),
        x => x,
    }
}
