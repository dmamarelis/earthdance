//! Builds a live data set for a place and reports timings.
//!
//!     cargo run --release -p ed-sources --example live_place -- "Lake Koronia" 23.145 40.682 12

use std::time::Instant;

use ed_sources::live::{build, LiveConfig, Place};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let place = Place {
        name: a.get(1).cloned().unwrap_or_else(|| "Lake Koronia".into()),
        lon: a.get(2).and_then(|v| v.parse().ok()).unwrap_or(23.145),
        lat: a.get(3).and_then(|v| v.parse().ok()).unwrap_or(40.682),
        half_km: a.get(4).and_then(|v| v.parse().ok()).unwrap_or(12.0),
    };
    let t0 = Instant::now();
    let first = std::cell::Cell::new(None);
    let ds = build(&place, &LiveConfig::default(), ed_sources::today_local(), &|s| println!("[{:>6.1}s] {s}", t0.elapsed().as_secs_f64()), &|d| {
        if first.get().is_none() {
            first.set(Some(t0.elapsed().as_secs_f64()));
            println!("[{:>6.1}s] first partial data set: {} frames", t0.elapsed().as_secs_f64(), d.layers[0].frames.len());
        }
    })
    .expect("build");
    let f = ed_sources::http::fetcher();
    println!(
        "\ndone in {:.1}s (first map after {:.1}s) · downloaded {:.1} MB · from cache {:.1} MB",
        t0.elapsed().as_secs_f64(),
        first.get().unwrap_or(0.0),
        f.fetched.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6,
        f.cached.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6
    );
    println!("stages (CPU, summed over threads): {}", ed_sources::live::timing::report());
    let r = &ds.regions.regions[0];
    println!("frames {} · optical {} · lake centre {:.3},{:.3} r={:.1} km", ds.layers[0].frames.len(), ds.optical.len(), r.center[0], r.center[1], r.radius_km);
    if let Some(res) = &r.reservoir {
        for (d, v) in res.dates.iter().zip(&res.volume) {
            print!("{} {v:.1}  ", d);
        }
        println!("\ncapacity {:.1} km², limit {:.1}", res.capacity, res.limit);
    }
    if let Some(res) = &r.reservoir {
        println!("method: {}", res.method.en);
    }
    if let Some(fc) = &ds.forecast {
        let l = ed_core::Lang::En;
        println!("seasons: wet {} · irrigation {}", ed_core::text::month_span(fc.params.wet_months, l), fc.params.irrigation_months.map_or("15 Apr–30 Sep".into(), |m| ed_core::text::month_span(m, l)));
        println!("model: {:?} rmse {:.2} · winter fitted from data: {}", fc.params, fc.rmse, fc.params.a_wet != fc.params.a_dry);
    }
}
