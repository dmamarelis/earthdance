//! Golden tests: the Rust port against what the legacy JS produced on the same inputs
//! (`tests/fixtures/*.json`, written by `oracle/gen.mjs` with today = 2026-11-14 and the saved weather).

use std::fs;
use std::path::PathBuf;

use ed_core::ask::{self, Question};
use ed_core::forecast::{self, Options};
use ed_core::insights::{scenario, WhatIf};
use ed_core::model::{points_from_geojson, ForecastModel, Regions, Validation, WeatherSet};
use ed_core::stats::{agreement, corr, slope_per_year};
use ed_core::status::compute;
use ed_core::text::{fmt_date, js_round, to_fixed, DateStyle, Lang};
use ed_core::Date;
use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn json(p: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(root().join(p)).unwrap_or_else(|e| panic!("{p}: {e}"))).expect(p)
}
fn load<T: serde::de::DeserializeOwned>(p: &str) -> T {
    serde_json::from_value(json(p)).unwrap_or_else(|e| panic!("{p}: {e}"))
}

const TODAY: Date = Date::ymd(2026, 11, 14);

/// Walks the fixture and checks every value it holds against ours: strings exactly, numbers to a relative
/// 1e-9, `null` / missing / "Infinity" (ours: null) as equivalent. Keys only we have are ignored.
fn like(path: &str, want: &Value, got: &Value, errs: &mut Vec<String>) {
    match (want, got) {
        (Value::Null, Value::Null) => {}
        (Value::String(s), Value::Null) if s == "Infinity" || s == "-Infinity" => {}
        (Value::Null, _) if got.is_null() => {}
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            if (a - b).abs() > 1e-9 * a.abs().max(1.0) {
                errs.push(format!("{path}: want {a}, got {b}"));
            }
        }
        (Value::Bool(a), Value::Bool(b)) if a == b => {}
        (Value::String(a), Value::String(b)) if a == b => {}
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                errs.push(format!("{path}: want {} items, got {}", a.len(), b.len()));
                return;
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                like(&format!("{path}[{i}]"), x, y, errs);
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            for (k, x) in a {
                like(&format!("{path}.{k}"), x, b.get(k).unwrap_or(&Value::Null), errs);
            }
        }
        (Value::Object(a), Value::Null) if a.is_empty() => {}
        _ => errs.push(format!("{path}: want {want}, got {got}")),
    }
}

fn check(name: &str, want: &Value, got: &Value) {
    let mut errs = Vec::new();
    like(name, want, got, &mut errs);
    if !errs.is_empty() {
        let n = errs.len();
        errs.truncate(25);
        panic!("{n} mismatches:\n{}", errs.join("\n"));
    }
}

#[test]
fn numbers_follow_js() {
    for row in json("tests/fixtures/numbers.json").as_array().unwrap() {
        let v = row["v"].as_f64().unwrap();
        assert_eq!(to_fixed(v, 0), row["f0"].as_str().unwrap(), "{v}.toFixed(0)");
        assert_eq!(to_fixed(v, 1), row["f1"].as_str().unwrap(), "{v}.toFixed(1)");
        assert_eq!(to_fixed(v, 2), row["f2"].as_str().unwrap(), "{v}.toFixed(2)");
        assert_eq!(js_round(v), row["round"].as_f64().unwrap(), "Math.round({v})");
    }
}

#[test]
fn dates_follow_intl() {
    let styles = [
        ("dmy", DateStyle::DayMonYear),
        ("dm", DateStyle::DayMon),
        ("m", DateStyle::MonthLong),
        ("wdm", DateStyle::WeekdayDayMonth),
        ("dMy", DateStyle::DayMonthYear),
        ("w", DateStyle::WeekdayShort),
        ("dmyy", DateStyle::DayMonYY),
        ("wdMy", DateStyle::WeekdayDayMonthYear),
    ];
    for (iso, row) in json("tests/fixtures/dates.json").as_object().unwrap() {
        let d = Date::parse(iso).unwrap();
        for l in Lang::ALL {
            for (k, s) in styles {
                assert_eq!(fmt_date(d, l, s), row[format!("{}.{k}", l.code())].as_str().unwrap(), "{iso} {k} {l:?}");
            }
        }
    }
}

#[test]
fn router_matches() {
    for (q, want) in json("tests/fixtures/route.json").as_object().unwrap() {
        let got = ask::route(q).map(|q| q.id());
        assert_eq!(got, want.as_str(), "{q}");
    }
}

#[test]
fn stats_match() {
    let pts = points_from_geojson(&json("data/points.geojson")).unwrap();
    let st = json("tests/fixtures/stats.json");
    for (p, want) in pts.iter().zip(st["slope"].as_array().unwrap()) {
        assert!((slope_per_year(&p.series.dates, &p.series.disp_mm) - want["disp"].as_f64().unwrap()).abs() < 1e-9);
    }
    for (p, want) in pts.iter().zip(st["corr"].as_array().unwrap()) {
        assert!((corr(&p.series.hh_db, &p.series.hv_db) - want.as_f64().unwrap()).abs() < 1e-12);
    }
    let v: Validation = load("data/validation.json");
    let x: Vec<f64> = v.pairs.iter().map(|p| p.reference).collect();
    let y: Vec<f64> = v.pairs.iter().map(|p| p.nisar).collect();
    let a = agreement(&x, &y);
    let want = json("tests/fixtures/validation.json");
    assert_eq!(a.n as u64, want["n"].as_u64().unwrap());
    for (got, k) in [(a.r2, "r2"), (a.rmse, "rmse"), (a.bias, "bias")] {
        assert!((got - want[k].as_f64().unwrap()).abs() < 1e-12, "{k}");
    }
}

struct Set {
    regions: Regions,
    wx: WeatherSet,
    fc: Option<ForecastModel>,
    ref_name: &'static str,
}

fn set(name: &str) -> Set {
    let dir = if name == "real" { "data/real/" } else { "data/" };
    let manifest = json(&format!("{dir}manifest.json"));
    Set {
        regions: load(&format!("{dir}regions.json")),
        wx: load(&format!("{dir}weather.json")),
        fc: manifest.get("forecast").map(|p| load(p.as_str().unwrap())),
        ref_name: "EGMS",
    }
}

#[test]
fn assessments_match() {
    for name in ["real", "demo"] {
        let s = set(name);
        for tag in ["wx", "nowx"] {
            let wx = (tag == "wx").then_some(&s.wx);
            let st = compute(&s.regions, wx, true, s.ref_name, s.fc.as_ref(), TODAY);
            let got = serde_json::to_value(&st.assessments).unwrap();
            let want = json(&format!("tests/fixtures/assess_{name}_{tag}.json"));
            let mut a = want["A"].clone();
            for v in a.as_object_mut().unwrap().values_mut() {
                v.as_object_mut().unwrap().remove("headline"); // = findings[0]
            }
            check(&format!("{name}/{tag}"), &a, &got);
        }
    }
}

#[test]
fn forecast_and_answers_match() {
    let s = set("real");
    let fc = s.fc.as_ref().unwrap();
    let lake = s.regions.regions.iter().find(|r| r.id == fc.region).unwrap();
    let res = lake.reservoir.as_ref().unwrap();
    for tag in ["wx", "nowx"] {
        let wx = (tag == "wx").then(|| &s.wx.by_region[&lake.id]);
        let want = json(&format!("tests/fixtures/forecast_real_{tag}.json"));
        for (k, w) in want["runs"].as_object().unwrap() {
            let o: Value = serde_json::from_str(k).unwrap();
            let opt = Options::what_if(o["rain"].as_f64().unwrap_or(0.0), o["irrigation"].as_f64().unwrap_or(0.0));
            let run = forecast::run(fc, res, wx, TODAY, opt);
            check(&format!("forecast/{tag}/{k}"), w, &serde_json::to_value(&run).unwrap());
        }

        let b = ask::brief(fc, res, wx, TODAY, &lake.name);
        let want = json(&format!("tests/fixtures/ask_real_{tag}.json"));
        for l in Lang::ALL {
            let w = &want["ans"][l.code()];
            assert_eq!(serde_json::to_value(ask::lines(&b, l)).unwrap(), w["lines"], "lines {tag} {l:?}");
            assert_eq!(ask::backtest_line(&b, l), w["backtest"].as_str().unwrap(), "backtest {tag} {l:?}");
            for (id, a) in w["answers"].as_object().unwrap() {
                check(&format!("answer/{tag}/{l:?}/{id}"), a, &serde_json::to_value(ask::answer(Question::from_id(id), &b, l)).unwrap());
            }
        }
        check(&format!("finding/{tag}"), &want["finding"], &serde_json::to_value(ask::finding(&b, &s.regions.thresholds)).unwrap());
        for (q, wq) in Question::LISTED.iter().zip(want["questions"].as_array().unwrap()) {
            assert_eq!(q.id(), wq["id"].as_str().unwrap());
            assert_eq!(q.label(Lang::El), wq["el"].as_str().unwrap());
            assert_eq!(q.label(Lang::En), wq["en"].as_str().unwrap());
        }
    }
}

#[test]
fn simple_scenarios_match() {
    for name in ["real", "demo"] {
        let s = set(name);
        let st = compute(&s.regions, Some(&s.wx), true, s.ref_name, None, TODAY);
        let want = json(&format!("tests/fixtures/scenario_{name}.json"));
        for (k, w) in want.as_object().unwrap() {
            let mut it = k.split('|');
            let (id, rain, irr) = (it.next().unwrap(), it.next().unwrap().parse().unwrap(), it.next().unwrap().parse().unwrap());
            let r = s.regions.regions.iter().find(|r| r.id == id).unwrap();
            let got = scenario(r, st.get(id), WhatIf { rain, irrigation: irr }).unwrap();
            check(&format!("scenario/{name}/{k}"), w, &serde_json::to_value(got).unwrap());
        }
    }
}
