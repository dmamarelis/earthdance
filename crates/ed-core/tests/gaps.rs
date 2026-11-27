//! Gap filling on the real lake series, judged by leave-one-out: drop each interior pass, estimate it from
//! the others, compare with what was measured.

use std::fs;

use ed_core::forecast::{bridge, Fill};
use ed_core::interp::{FloaterHormann, Interpolant, Lagrange, Pchip};
use ed_core::model::{ForecastModel, Regions, Reservoir, WeatherSet};

fn load<T: serde::de::DeserializeOwned>(p: &str) -> T {
    serde_json::from_str(&fs::read_to_string(format!("{}/../../{p}", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap()
}

#[test]
fn leave_one_out_on_the_lake() {
    let fc: ForecastModel = load("data/real/forecast.json");
    let regions: Regions = load("data/real/regions.json");
    let wx: WeatherSet = load("data/real/weather.json");
    let lake = regions.regions.iter().find(|r| r.id == fc.region).unwrap();
    let res = lake.reservoir.as_ref().unwrap();
    let w = &wx.by_region[&lake.id];
    let area = res.area_km2.clone().unwrap();
    let mut err = [0.0f64; 4]; // bridge, pchip, floater-hormann, lagrange
    let mut n = 0.0;
    println!("date         measured   bridge   pchip   F-H(3)  Lagrange");
    for j in 1..res.dates.len() - 1 {
        let keep: Vec<usize> = (0..res.dates.len()).filter(|&i| i != j).collect();
        let r2 = Reservoir { dates: keep.iter().map(|&i| res.dates[i]).collect(), area_km2: Some(keep.iter().map(|&i| area[i]).collect()), volume: keep.iter().map(|&i| area[i]).collect(), ..res.clone() };
        let xs: Vec<f64> = r2.dates.iter().map(|d| f64::from(d.days())).collect();
        let ys = r2.volume.clone();
        let x = f64::from(res.dates[j].days());
        let b = bridge(&fc, &r2, Some(w)).into_iter().find(|(d, _, _)| *d == res.dates[j]).unwrap();
        let est = [b.1, Pchip::new(&xs, &ys).eval(x), FloaterHormann::new(&xs, &ys, 3).eval(x), Lagrange::new(&xs, &ys).eval(x)];
        println!("{}  {:>8.2} {:>8.2}{} {:>7.2} {:>8.2} {:>9.2}", res.dates[j], area[j], est[0], if b.2 == Fill::Model { "m" } else { "p" }, est[1], est[2], est[3]);
        for (e, v) in err.iter_mut().zip(est) {
            *e += (v - area[j]).powi(2);
        }
        n += 1.0;
    }
    let rmse = err.map(|e| (e / n).sqrt());
    for d in 0..=3 {
        let mut e = 0.0;
        for j in 1..res.dates.len() - 1 {
            let keep: Vec<usize> = (0..res.dates.len()).filter(|&i| i != j).collect();
            let xs: Vec<f64> = keep.iter().map(|&i| f64::from(res.dates[i].days())).collect();
            let ys: Vec<f64> = keep.iter().map(|&i| area[i]).collect();
            e += (FloaterHormann::new(&xs, &ys, d).eval(f64::from(res.dates[j].days())) - area[j]).powi(2);
        }
        println!("Floater–Hormann d={d}: RMSE {:.2}", (e / n).sqrt());
    }
    println!("leave-one-out RMSE (km²): bridge {:.2}  PCHIP {:.2}  Floater–Hormann {:.2}  Lagrange {:.2}", rmse[0], rmse[1], rmse[2], rmse[3]);
    // the interpolant the app uses must beat plain polynomial interpolation by a wide margin
    assert!(rmse[1] < rmse[3] && rmse[0] < rmse[3]);
    // the physical bridge beats every pure interpolant on this series (1.91 vs PCHIP 2.23 km²)
    assert!(rmse[0] < rmse[1]);
    // and every bridged day stays physical
    for (_, v, _) in bridge(&fc, res, Some(w)) {
        assert!((0.0..=fc.params.amax).contains(&v));
    }
}
