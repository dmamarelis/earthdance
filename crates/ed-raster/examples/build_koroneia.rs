//! Times the full Lake Koronia build on the repository's input files.
//!
//!     cargo run --release -p ed-raster --example build_koroneia
use std::time::Instant;

use ed_raster::koroneia::{Legends, SiteConfig, build, read_inputs_from_dirs};
use ed_raster::render::parse_hex;

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let t = Instant::now();
    let inputs = read_inputs_from_dirs(root.join("Koroneia_NISAR"), root.join("Koroneia_Sentinel")).expect("inputs");
    let read = t.elapsed();
    let hex = |s| parse_hex(s).expect("colour");
    let legends = Legends {
        hh: vec![(-26.0, hex("#05070d")), (-18.0, hex("#2a3550")), (-11.0, hex("#8a97ad")), (-4.0, hex("#eef2f7"))],
        change: vec![(-8.0, hex("#2f6df6")), (-3.0, hex("#7fb2ff")), (0.0, hex("#e8e6df")), (3.0, hex("#ffb454")), (8.0, hex("#f2622e"))],
    };
    let cfg = SiteConfig::koronia();
    let runs = 5;
    let mut best = f64::INFINITY;
    let mut out = None;
    for _ in 0..runs {
        let t = Instant::now();
        out = Some(build(&cfg, &inputs, &legends).expect("build"));
        best = best.min(t.elapsed().as_secs_f64());
    }
    let b = out.expect("ran");
    println!(
        "read+decode {:.1} ms; build best of {runs}: {:.1} ms ({} SAR frames, {} optical images, lake {:.2} km2, {} threads)",
        read.as_secs_f64() * 1e3,
        best * 1e3,
        b.sar_frames.len(),
        b.optical_frames.len(),
        b.lake_km2,
        rayon::current_num_threads()
    );
}
