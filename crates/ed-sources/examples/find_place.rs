//! Place search from the command line: cargo run -p ed-sources --example find_place -- "Lake Taupo"
fn main() {
    let q = std::env::args().nth(1).unwrap_or_else(|| "Lake Taupo".into());
    for f in ed_sources::live::geocode(&q, "en").expect("geocode") {
        println!("{}{} · {} ({:.4}, {:.4}) half_km {:?}", if f.water { "[water] " } else { "" }, f.name, f.detail, f.lat, f.lon, f.half_km());
    }
}
