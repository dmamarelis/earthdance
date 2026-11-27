//! Speed of the Rust port on the Status computation, same inputs and algorithm as `oracle/bench.mjs`
//! (replayed climate years, so both do exactly the same work).
//!   cargo run --release -p ed-core --example bench_status

use std::time::Instant;

use ed_core::ask::{self, Question};
use ed_core::model::{ForecastModel, Regions, WeatherSet};
use ed_core::status::compute;
use ed_core::{Date, Lang};

fn load<T: serde::de::DeserializeOwned>(p: &str) -> T {
    serde_json::from_str(&std::fs::read_to_string(format!("{}/../../{p}", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap()
}

fn main() {
    let regions: Regions = load("data/real/regions.json");
    let wx: WeatherSet = load("data/real/weather.json");
    let fc: ForecastModel = load("data/real/forecast.json");
    let today = Date::ymd(2026, 11, 14);
    let once = || {
        let st = compute(&regions, Some(&wx), true, "EGMS", Some(&fc), today);
        let b = st.brief.as_ref().unwrap();
        let mut n = 0;
        for l in Lang::ALL {
            for q in Question::LISTED.into_iter().chain([Question::None]) {
                n += ask::answer(q, b, l).text.len();
            }
        }
        n + st.assessments.len()
    };
    for _ in 0..20 {
        std::hint::black_box(once());
    }
    let mut t: Vec<f64> = (0..200)
        .map(|_| {
            let t0 = Instant::now();
            std::hint::black_box(once());
            t0.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    t.sort_by(f64::total_cmp);
    println!("{{\"impl\":\"Rust (release)\",\"median_ms\":{:.3},\"p10_ms\":{:.3},\"p90_ms\":{:.3}}}", t[100], t[20], t[180]);
}
