//! Parity with the committed outputs of the legacy `tools/build_real_data.py` (data/real/), which were
//! produced from the very TIFFs in Koroneia_NISAR/ and Koroneia_Sentinel/.
//!
//! Run with `--nocapture` to see the per-date comparison tables.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use ed_core::Date;
use ed_raster::koroneia::{Build, Legends, SiteConfig, build, read_inputs_from_dirs};
use ed_raster::render::{Stop, parse_hex};
use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn json(rel: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(root().join(rel)).unwrap()).unwrap()
}

fn layer<'a>(manifest: &'a Value, id: &str) -> &'a Value {
    manifest["layers"].as_array().unwrap().iter().find(|l| l["id"] == id).unwrap()
}

fn stops(l: &Value) -> Vec<Stop> {
    l["legend"]["stops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s[0].as_f64().unwrap(), parse_hex(s[1].as_str().unwrap()).unwrap()))
        .collect()
}

static BUILD: LazyLock<Build> = LazyLock::new(|| {
    let r = root();
    let inputs = read_inputs_from_dirs(r.join("Koroneia_NISAR"), r.join("Koroneia_Sentinel")).unwrap();
    let m = json("data/real/manifest.json");
    let legends = Legends { hh: stops(layer(&m, "hh")), change: stops(layer(&m, "change")) };
    let t = std::time::Instant::now();
    let b = build(&SiteConfig::koronia(), &inputs, &legends).unwrap();
    eprintln!("build: {:.2?}", t.elapsed());
    b
});

fn d(s: &Value) -> Date {
    Date::parse(s.as_str().unwrap()).unwrap()
}

#[test]
fn manifest_frames_match() {
    let b = &*BUILD;
    let m = json("data/real/manifest.json");
    for id in ["hh", "change"] {
        let l = layer(&m, id);
        let want: Vec<(Date, bool)> =
            l["frames"].as_array().unwrap().iter().map(|f| (d(&f["date"]), f["empty"].as_bool().unwrap_or(false))).collect();
        let got: Vec<(Date, bool)> = b.sar_frames.iter().map(|f| (f.date, f.is_empty())).collect();
        assert_eq!(got, want, "{id} frames");
        let grid = b.hover(&b.sar_frames[0].layers.as_ref().unwrap().hh_db);
        assert_eq!([grid.w, grid.h], [l["grid"][0].as_u64().unwrap() as usize, l["grid"][1].as_u64().unwrap() as usize]);
        // Source labels as the legacy manifest wrote them.
        for (f, g) in l["frames"].as_array().unwrap().iter().zip(&b.sar_frames) {
            assert_eq!(f["source"].as_str().unwrap(), g.source);
        }
    }
    let clouds = m["optical"]["clouds"].as_array().unwrap();
    let got: Vec<(Date, &str)> = b.optical_frames.iter().map(|o| (o.date, o.source.as_str())).collect();
    let want: Vec<(Date, &str)> = clouds.iter().map(|c| (d(&c["date"]), c["source"].as_str().unwrap())).collect();
    assert_eq!(got, want, "optical frames");
    for p in &b.pass_stats {
        eprintln!(
            "NISAR {}  orbit {:2}  lake water {:6.2} km2  {}",
            p.date,
            p.orbit,
            p.water_km2,
            p.dropped.map_or("used".to_owned(), |r| format!("not used ({})", r.as_str()))
        );
    }
}

#[test]
fn regions_match() {
    let b = &*BUILD;
    let r = json("data/real/regions.json");
    for (region, s) in r["regions"].as_array().unwrap().iter().zip(&b.series) {
        assert_eq!(region["id"].as_str().unwrap(), s.id);
        let ser = &region["series"];
        let dates: Vec<Date> = ser["dates"].as_array().unwrap().iter().map(d).collect();
        assert_eq!(s.dates, dates, "{} dates", s.id);
        let src: Vec<&str> = ser["water_source"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(s.water_source.iter().map(|x| x.as_str()).collect::<Vec<_>>(), src);
        eprintln!("{}:  date        water km2 (py / rs)      hh dB (py / rs)", s.id);
        let (mut exact, mut worst_w, mut worst_h) = (0, 0.0f64, 0.0f64);
        for (i, date) in dates.iter().enumerate() {
            let pw = ser["water_km2"][i].as_f64().unwrap();
            let rw = s.water_km2[i];
            let ph = ser["hh_db"][i].as_f64();
            let rh = s.hh_db[i];
            eprintln!(
                "  {date}  {pw:7.2} / {rw:7.2} {:+6.2}   {:>7} / {:>7}",
                rw - pw,
                ph.map_or("-".into(), |v| format!("{v:.2}")),
                rh.map_or("-".into(), |v| format!("{v:.2}"))
            );
            exact += usize::from(pw == rw);
            worst_w = worst_w.max((rw - pw).abs());
            assert!((rw - pw).abs() <= (0.005 * pw).max(0.05) + 1e-9, "{} {date}: water {rw} vs {pw}", s.id);
            assert_eq!(ph.is_some(), rh.is_some(), "{} {date}: hh_db presence", s.id);
            if let (Some(p), Some(q)) = (ph, rh) {
                worst_h = worst_h.max((p - q).abs());
                assert!((p - q).abs() <= 0.05 + 1e-9, "{} {date}: hh_db {q} vs {p}", s.id);
            }
        }
        eprintln!("  water exact on {exact}/{} dates, worst |Δ| {worst_w:.2} km2; worst |Δ hh| {worst_h:.2} dB", dates.len());
        if let Some(res) = region.get("reservoir") {
            let got = b.reservoir.as_ref().expect("reservoir");
            assert_eq!(got.zone_id, s.id);
            let cap = res["capacity"].as_f64().unwrap();
            let lim = res["limit"].as_f64().unwrap();
            eprintln!("  reservoir capacity {cap} / {}  limit {lim} / {}", got.capacity, got.limit);
            assert!((got.capacity - cap).abs() <= (0.005 * cap).max(0.05));
            assert!((got.limit - lim).abs() <= (0.005 * lim).max(0.05));
            assert_eq!(got.area_km2, s.water_km2);
            let want: Vec<[f64; 2]> =
                res["outline"].as_array().unwrap().iter().map(|p| [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()]).collect();
            let cfg = SiteConfig::koronia();
            let mut worst = 0.0f64;
            let mut same = 0;
            for (p, q) in want.iter().zip(&got.outline) {
                same += usize::from(p == q);
                worst = worst.max(((p[0] - q[0]) / cfg.res).abs().max(((p[1] - q[1]) / cfg.res).abs()));
            }
            eprintln!("  outline: {} / {} points, {same} identical, worst {worst:.1} px", want.len(), got.outline.len());
            assert_eq!(got.outline.len(), want.len());
            assert!(worst <= 2.0 + 1e-6, "outline off by {worst} px");
        }
    }
}

#[test]
fn hover_values_match() {
    let b = &*BUILD;
    let dir = root().join("data/real/values");
    let (mut all_cells, mut all_nd_agree, mut all_valid, mut all_close, mut all_exact) = (0, 0, 0, 0, 0);
    for f in b.sar_frames.iter().filter(|f| !f.is_empty()) {
        let l = f.layers.as_ref().unwrap();
        for (id, grid) in [("hh", &l.hh_db), ("change", &l.change_db)] {
            let bytes = std::fs::read(dir.join(format!("{id}_{}.bin", f.date))).unwrap();
            let want: Vec<i16> = bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
            let got = b.hover(grid);
            assert_eq!(got.data.len(), want.len());
            let (mut nd_agree, mut valid, mut close, mut exact) = (0, 0, 0, 0);
            for (&g, &w) in got.data.iter().zip(&want) {
                let (gn, wn) = (g == i16::MIN, w == i16::MIN);
                nd_agree += usize::from(gn == wn);
                if !gn && !wn {
                    valid += 1;
                    close += usize::from((i32::from(g) - i32::from(w)).abs() <= 1);
                    exact += usize::from(g == w);
                }
            }
            let n = want.len();
            eprintln!(
                "{id:6} {}  nodata agree {:7.3} %   valid {valid:5}: exact {:6.2} %, ±1 LSB {:7.3} %",
                f.date,
                100.0 * nd_agree as f64 / n as f64,
                100.0 * exact as f64 / valid as f64,
                100.0 * close as f64 / valid as f64
            );
            assert!(nd_agree as f64 >= 0.995 * n as f64, "{id} {}: nodata agreement", f.date);
            assert!(close as f64 >= 0.99 * valid as f64, "{id} {}: ±1 LSB", f.date);
            (all_cells, all_nd_agree, all_valid, all_close, all_exact) =
                (all_cells + n, all_nd_agree + nd_agree, all_valid + valid, all_close + close, all_exact + exact);
        }
    }
    eprintln!(
        "ALL    nodata agree {:.3} %, exact {:.2} %, ±1 LSB {:.3} % of {all_valid} valid cells",
        100.0 * all_nd_agree as f64 / all_cells as f64,
        100.0 * all_exact as f64 / all_valid as f64,
        100.0 * all_close as f64 / all_valid as f64
    );
}

#[test]
fn optical_water_log() {
    let b = &*BUILD;
    for s in &b.optical_stats {
        eprintln!("S2    {}  water {:6.2} km2  {}", s.date, s.water_km2, if s.used { "used" } else { "not used" });
    }
    eprintln!("lake at its fullest {:.2} km2", b.lake_km2);
    assert!(b.lake_km2 > 20.0);
}
