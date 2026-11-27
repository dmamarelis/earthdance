//! A data set for any place on Earth, built from the archives while the user watches.
//!
//! Sentinel-2 L2A from the public AWS archive (Earth Search STAC, Cloud-Optimized GeoTIFFs, no account):
//! 1. search the scenes over the area for the last months (STAC results cached for a day);
//! 2. per date, read the scene classification (SCL) first: if the area is mostly cloud, stop there;
//! 3. otherwise read green, red and NIR, only the tiles covering the area at the needed overview level
//!    (byte ranges, cached forever on disk), and mosaic the MGRS tiles of the same date;
//! 4. find the lake and its area series (`ed_raster::site`), fit the lake model on ERA5, assemble a
//!    `Dataset`. Newest dates first, with a partial data set handed over every few dates.

use std::sync::Arc;

use ed_core::forecast::{self, FitConfig};
use ed_core::model::{bundled_catalog, bundled_content, Content, Dataset, Frame, Layer, LayerInfo, Legend, Missing, OpticalFrame, Region, Regions, Reservoir, Series, Site, Thresholds};
use ed_core::{Bounds, Date, Grid, Text};
use ed_raster::cog::{Cog, CogError};
use ed_raster::geotiff::SourceRaster;
use ed_raster::site::{analyse, OpticalScene, SiteAnalysis, SiteGrid};
use ed_raster::utm::{self, Zone};
use serde::{Deserialize, Serialize};

use crate::stac::{self, Item};
use crate::{http, Error, Result};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub name: String,
    pub lon: f64,
    pub lat: f64,
    /// Half the side of the square looked at (km).
    pub half_km: f64,
}

#[derive(Clone, Debug)]
pub struct LiveConfig {
    /// How far back to look (months). 18 gives at least one winter.
    pub months: i32,
    /// Skip scenes the catalogue says are cloudier than this (% of the whole 110 km tile).
    pub max_tile_cloud: f64,
    /// Skip a date when less than this share of the area is seen clearly.
    pub min_clear: f64,
    /// Grid size limit (pixels across).
    pub max_px: usize,
    /// Dates processed between two partial updates.
    pub batch: usize,
    /// At most one date per this many days (the least cloudy one).
    pub every_days: i32,
    /// Accept a COG overview up to this many times coarser than the grid (bytes fall with the square).
    /// `None` = decide from the measured link speed.
    pub slack: Option<f64>,
}

/// Largest area looked at: 120 km across. Bigger lakes are measured inside it (partially) and say so.
pub const MAX_HALF_KM: f64 = 60.0;

/// Below this link speed (bytes/s) the coarser overview is used (~4× fewer bytes).
pub const SLOW_LINK: f64 = 300_000.0;

impl Default for LiveConfig {
    fn default() -> Self {
        Self { months: 18, max_tile_cloud: 60.0, min_clear: 0.4, max_px: 500, batch: 6, every_days: 8, slack: None }
    }
}

// ───────────────────────────────────────────────────────── reading one band of one scene

/// Stage timers (nanoseconds, summed over threads), printed by the `live_place` example.
pub mod timing {
    use std::sync::atomic::{AtomicU64, Ordering};
    pub static READ: AtomicU64 = AtomicU64::new(0);
    pub static WARP: AtomicU64 = AtomicU64::new(0);
    pub static NEAREST: AtomicU64 = AtomicU64::new(0);
    pub static ANALYSE: AtomicU64 = AtomicU64::new(0);
    pub static ASSEMBLE: AtomicU64 = AtomicU64::new(0);
    pub static FIT: AtomicU64 = AtomicU64::new(0);
    pub fn add(c: &AtomicU64, t: std::time::Instant) {
        c.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
    pub fn report() -> String {
        let s = |c: &AtomicU64| c.load(Ordering::Relaxed) as f64 / 1e9;
        format!(
            "read+decode {:.2}s · warp bilinear {:.2}s · warp nearest {:.2}s · analyse {:.2}s · assemble {:.2}s · fit {:.2}s",
            s(&READ), s(&WARP), s(&NEAREST), s(&ANALYSE), s(&ASSEMBLE), s(&FIT)
        )
    }
}

thread_local! {
    static SLACK: std::cell::Cell<f64> = const { std::cell::Cell::new(1.5) };
}

/// The COG header (cached), growing the request when the IFDs reach beyond the first 16 KB.
fn header(url: &str) -> Result<Cog> {
    let mut len = 16_384u64;
    loop {
        let b = http::fetcher().range(url, 0, len)?;
        match Cog::parse(&b) {
            Ok(c) => return Ok(c),
            Err(CogError::NeedMore(n)) if (n as u64) > len && len < (4 << 20) => len = (n as u64).max(len * 2),
            Err(e) => return Err(Error::Data(e.to_string())),
        }
    }
}

/// The area's window of a COG at about `res_m`, as a source raster in the file's CRS.
fn read_window(url: &str, bounds: Bounds, res_m: f64) -> Result<Option<SourceRaster>> {
    let cog = header(url)?;
    let zone = Zone::from_epsg(cog.epsg).ok_or_else(|| Error::Data(format!("CRS EPSG:{} is not UTM", cog.epsg)))?;
    // the area's corners and edge midpoints in UTM (the box is not axis-aligned there)
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for fx in [0.0, 0.5, 1.0] {
        for fy in [0.0, 0.5, 1.0] {
            let lon = bounds.west() + fx * (bounds.east() - bounds.west());
            let lat = bounds.south() + fy * (bounds.north() - bounds.south());
            let (e, n) = utm::forward(zone, lon, lat);
            (x0, y0, x1, y1) = (x0.min(e), y0.min(n), x1.max(e), y1.max(n));
        }
    }
    let level = cog.level_near(res_m, SLACK.with(|s| s.get()));
    let Some(plan) = cog.plan(level, x0, y0, x1, y1) else { return Ok(None) };
    let tiles: Vec<Result<Vec<u8>>> = http::parallel(&plan.tiles, 6, |(_, off, len)| http::fetcher().range(url, *off, *len));
    let tiles: Vec<Vec<u8>> = tiles.into_iter().collect::<Result<_>>()?;
    Ok(Some(cog.assemble(&plan, &tiles).map_err(|e| Error::Data(e.to_string()))?))
}

/// For every pixel of the site grid, its position in a UTM zone. Identical for every scene and band of a
/// build, so it is computed once per (zone, grid) and shared: the per-pixel trigonometry of the projection
/// was the most expensive step of reopening a place.
fn utm_coords(epsg: u32, g: &SiteGrid) -> Option<std::sync::Arc<Vec<(f64, f64)>>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    type Key = (u32, [u64; 4], usize, usize);
    type Coords = Arc<Vec<(f64, f64)>>;
    static CACHE: OnceLock<Mutex<HashMap<Key, Coords>>> = OnceLock::new();
    let zone = Zone::from_epsg(epsg)?;
    let b = g.bounds.0;
    let key = (epsg, [b[0].to_bits(), b[1].to_bits(), b[2].to_bits(), b[3].to_bits()], g.w, g.h);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().expect("utm cache").get(&key) {
        return Some(v.clone());
    }
    let probe = Grid::filled(g.w, g.h, g.bounds, 0u8);
    use rayon::prelude::*;
    let v: Vec<(f64, f64)> = (0..g.w * g.h).into_par_iter().map(|i| utm::forward(zone, probe.lon(i % g.w), probe.lat(i / g.w))).collect();
    let v = Arc::new(v);
    let mut c = cache.lock().expect("utm cache");
    if c.len() > 16 {
        c.clear();
    }
    c.insert(key, v.clone());
    Some(v)
}

/// Nearest-neighbour resampling (for class bands such as SCL, where averaging classes is meaningless).
fn warp_nearest(src: &SourceRaster, g: &SiteGrid) -> Grid<f32> {
    let Some(en) = utm_coords(src.epsg, g) else { return Grid::filled(g.w, g.h, g.bounds, f32::NAN) };
    let gt = src.geotransform;
    let data = en
        .iter()
        .map(|&(e, n)| {
            let c = ((e - gt[0]) / gt[1]).floor();
            let r = ((n - gt[3]) / gt[5]).floor();
            if c >= 0.0 && r >= 0.0 && (c as usize) < src.width && (r as usize) < src.height { src.data[r as usize * src.width + c as usize] } else { f32::NAN }
        })
        .collect();
    Grid::from_vec(g.w, g.h, g.bounds, data)
}

/// Fills NaN cells of `a` from `b` (mosaicking the MGRS tiles of one date).
fn mosaic(a: Option<Grid<f32>>, b: Grid<f32>) -> Grid<f32> {
    match a {
        None => b,
        Some(mut a) => {
            for (x, y) in a.data.iter_mut().zip(&b.data) {
                if !x.is_finite() {
                    *x = *y;
                }
            }
            a
        }
    }
}

fn band(items: &[&Item], key: &str, g: &SiteGrid, nearest: bool) -> Result<Option<Grid<f32>>> {
    let mut out = None;
    for it in items {
        let Some(url) = it.assets.get(key) else { continue };
        let t = std::time::Instant::now();
        let src = read_window(url, g.bounds, g.res_m())?;
        timing::add(&timing::READ, t);
        if let Some(src) = src {
            let t = std::time::Instant::now();
            let gr = if nearest { warp_nearest(&src, g) } else { ed_raster::warp_bilinear(&src, g.bounds, g.w, g.h).map_err(|e| Error::Data(format!("{e:?}")))? };
            timing::add(if nearest { &timing::NEAREST } else { &timing::WARP }, t);
            out = Some(mosaic(out, gr));
        }
    }
    Ok(out)
}

/// One date: SCL first, the other bands only if the area is clear enough.
fn scene(date: Date, items: &[&Item], g: &SiteGrid, min_clear: f64) -> Result<Option<OpticalScene>> {
    let Some(scl) = band(items, "scl", g, true)? else { return Ok(None) };
    let clear = scl.data.iter().filter(|c| ed_raster::site::scl_clear(**c)).count() as f64 / scl.data.len() as f64;
    if clear < min_clear {
        return Ok(None);
    }
    // green + NIR give the water index; the picture is made from them too (no red band: fewer bytes)
    let (Some(green), Some(nir)) = (band(items, "green", g, false)?, band(items, "nir", g, false)?) else { return Ok(None) };
    let red = green.clone();
    let source = format!("Sentinel-2 L2A · {}", items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>().join(" + "));
    Ok(Some(OpticalScene { date, source, green, red, nir, scl }))
}

// ───────────────────────────────────────────────────────── the data set

fn ndwi_layer(threshold: f32) -> LayerInfo {
    let t = f64::from(threshold);
    LayerInfo {
        id: "ndwi".into(),
        band: "S2".into(),
        sensor: "Sentinel-2".into(),
        product: "L2A".into(),
        unit: "NDWI".into(),
        name: Text::new("Δείκτης νερού (NDWI)", "Water index (NDWI)"),
        short: Text::new("Πού υπάρχει νερό", "Where there is water"),
        description: Text::new(
            format!("(Πράσινο − Υπέρυθρο) / (Πράσινο + Υπέρυθρο) από το Sentinel-2. Πάνω από {t:.2} (όριο που βρέθηκε από τα ίδια τα δεδομένα με τη μέθοδο Otsu) = νερό. Τα σύννεφα αφαιρούνται."),
            format!("(Green − NIR) / (Green + NIR) from Sentinel-2. Above {t:.2} (a threshold found from the data itself, Otsu's method) = water. Clouds are removed."),
        ),
        legend: Legend {
            stops: vec![(-0.6, "#5a4a32".into()), (-0.2, "#8b8a6a".into()), (0.0, "#c9c9bd".into()), (0.25, "#6f9fb8".into()), (0.6, "#24506e".into())],
            low: Text::new("ξηρά / βλάστηση", "land / vegetation"),
            high: Text::new("νερό", "water"),
        },
        classes: vec![
            (t, Text::new("Ξηρά ή βλάστηση", "Land or vegetation")),
            (1.0, Text::new("Νερό (πάνω από το όριο Otsu)", "Water (above the Otsu threshold)")),
        ],
    }
}

/// Zoom that shows a square of `2·half_km` in about 800 px.
pub fn zoom_for(lat: f64, half_km: f64) -> f64 {
    (40_075.0 * lat.to_radians().cos() * 800.0 / (2.0 * half_km * 512.0)).log2().clamp(4.0, 16.0)
}

fn assemble(place: &Place, grid: &SiteGrid, a: &SiteAnalysis, model: Option<ed_core::model::ForecastModel>, content: &Content) -> Dataset {
    let info = ndwi_layer(a.threshold);
    let stops = info.legend.rgb_stops();
    use rayon::prelude::*;
    let frames = a
        .scenes
        .par_iter()
        .map(|s| {
            let alpha = Grid::from_vec(grid.w, grid.h, grid.bounds, s.ndwi.data.iter().map(|v| if v.is_finite() { 0.85 } else { 0.0 }).collect());
            // clouds are not data: blank them in the index layer
            let mut vals = s.ndwi.clone();
            for (v, px) in vals.data.iter_mut().zip(&s.image.px) {
                if px[0] > 230 && px[1] > 230 && px[3] > 200 {
                    *v = f32::NAN;
                }
            }
            let mut image = ed_raster::render::colorize(&vals, &stops, Some(&alpha));
            // clouds are drawn as clouds (light, half transparent), not as a gap that reads like dark water
            for (px, src) in image.px.iter_mut().zip(&s.image.px) {
                if src[0] > 230 && src[1] > 230 && src[3] > 200 {
                    *px = [205, 210, 218, 150];
                }
            }
            Frame { date: s.date, empty: false, source: s.source.clone(), url: String::new(), values: Some(Arc::new(vals)), image: Some(Arc::new(image)) }
        })
        .collect();
    // the dated picture only around the lake (~1.5 km), fading softly into the basemap
    let soft = {
        let px_km = grid.px_km2().sqrt();
        let wide = ed_raster::filters::dilate_cross(&a.zone, ((1.5 / px_km).round() as usize).clamp(2, 200));
        let sigma = (0.5 / px_km).clamp(1.0, 40.0);
        ed_raster::filters::gaussian(&wide.map(|v| if *v { 1.0 } else { 0.0 }), sigma)
    };
    let optical = a
        .scenes
        .par_iter()
        .map(|s| {
            let mut img = s.image.clone();
            for (px, f) in img.px.iter_mut().zip(&soft.data) {
                px[3] = (f64::from(px[3]) * f.clamp(0.0, 1.0)) as u8;
            }
            OpticalFrame { date: s.date, cover: Some(((1.0 - s.clear) * 100.0).round()), source: s.source.clone(), image: Arc::new(img) }
        })
        .collect();
    let used: Vec<&ed_raster::site::SceneResult> = a.scenes.iter().filter(|s| s.water_km2.is_some()).collect();
    let dates: Vec<Date> = used.iter().map(|s| s.date).collect();
    let area: Vec<f64> = used.iter().map(|s| (s.water_km2.unwrap_or(0.0) * 100.0).round() / 100.0).collect();
    let cap = area.iter().copied().fold(0.0, f64::max);
    let lake_name = Text::new(format!("{} · λίμνη", place.name), format!("{} · lake", place.name));
    let reservoir = (area.len() >= 2).then(|| Reservoir {
        name: lake_name.clone(),
        unit: "km²".into(),
        capacity: cap,
        limit: (cap * 0.25 * 100.0).round() / 100.0,
        dates: dates.clone(),
        volume: area.clone(),
        area_km2: Some(area.clone()),
        outline: (!a.outline.is_empty()).then(|| a.outline.clone()),
        method: Text::new(
            format!(
                "Επιφάνεια νερού από το Sentinel-2: NDWI πάνω από {:.2} (Otsu), μόνο σε λήψεις όπου η ζώνη της λίμνης φαίνεται ≥ 90 % χωρίς σύννεφα ή πάγο.{}",
                a.threshold,
                if a.touches_edge { " Η λίμνη ξεπερνά την περιοχή των 120 km: μετριέται μόνο το μέρος της μέσα σε αυτήν." } else { "" }
            ),
            format!(
                "Water surface from Sentinel-2: NDWI above {:.2} (Otsu), only on dates where the lake zone is ≥ 90 % free of cloud and ice.{}",
                a.threshold,
                if a.touches_edge { " The lake extends beyond the 120 km area: only the part inside it is measured." } else { "" }
            ),
        ),
        rule: Text::new("Ενδεικτικό όριο: 25 % της μεγαλύτερης μετρημένης επιφάνειας. Να αντικατασταθεί με το επίσημο.", "Indicative limit: 25 % of the largest measured surface. Replace with the official one."),
    });
    let region = Region {
        id: "lake".into(),
        name: lake_name,
        center: a.center,
        radius_km: (a.radius_km * 1.4).max(0.5),
        points: Vec::new(),
        series: Series { dates, water_km2: Some(area), water_source: Some(vec!["Sentinel-2".into(); used.len()]), ..Default::default() },
        reservoir,
    };
    let catalog = bundled_catalog();
    let missing = ["hh", "hv", "disp", "sm"].iter().filter_map(|id| catalog.layers.iter().find(|l| l.id == *id).map(|l| Missing { id: l.id.clone(), name: l.name.clone() })).collect();
    Dataset {
        demo: false,
        site: Site { name: Text::same(&place.name), country: Text::same(""), bounds: grid.bounds, center: [place.lon, place.lat], zoom: zoom_for(place.lat, place.half_km) },
        reference: None,
        layers: vec![Layer { info, frames }],
        optical,
        points: Vec::new(),
        regions: Regions { thresholds: Thresholds::default(), regions: vec![region] },
        forecast: model,
        validation: None,
        content: content.clone(),
        missing,
    }
}

/// Builds the data set for `place`. `partial` receives intermediate data sets (newest dates first, growing).
/// The dates to read: one per date (the single MGRS tile covering the area, or all to mosaic), at most one
/// per `every_days` window (the least cloudy), newest first.
fn pick_dates(items: &[Item], b: Bounds, every_days: i32) -> Vec<(Date, Vec<&Item>)> {
    let mut by_date: std::collections::BTreeMap<Date, Vec<&Item>> = std::collections::BTreeMap::new();
    for it in items {
        by_date.entry(it.date).or_default().push(it);
    }
    let covers = |it: &Item| it.bbox.is_some_and(|x| x[0] <= b.west() && x[1] <= b.south() && x[2] >= b.east() && x[3] >= b.north());
    let cloud = |its: &[&Item]| its.iter().map(|i| i.cloud.unwrap_or(50.0)).fold(0.0, f64::max);
    let mut all: Vec<(Date, Vec<&Item>)> = by_date
        .into_iter()
        .map(|(d, its)| {
            let one = its.iter().filter(|i| covers(i)).min_by(|a, b| a.cloud.unwrap_or(50.0).total_cmp(&b.cloud.unwrap_or(50.0))).copied();
            (d, one.map_or(its, |i| vec![i]))
        })
        .collect();
    all.sort_by_key(|(d, _)| std::cmp::Reverse(*d));
    let mut dates: Vec<(Date, Vec<&Item>)> = Vec::new();
    for (d, its) in all {
        match dates.last_mut() {
            Some((d0, its0)) if d0.since(d) < every_days => {
                if cloud(&its) < cloud(its0) {
                    *d0 = d;
                    *its0 = its;
                }
            }
            _ => dates.push((d, its)),
        }
    }
    dates
}

/// Builds the data set for `place`. `partial` receives intermediate data sets (newest dates first, growing).
///
/// Stale-while-revalidate: if this area was searched before, the old scene list is processed first (its
/// tiles are all on disk, so that takes a second), then the archive is searched again and only the new
/// dates are fetched.
pub fn build(place: &Place, cfg: &LiveConfig, today: Date, progress: &dyn Fn(&str), partial: &dyn Fn(Dataset)) -> Result<Dataset> {
    let grid = SiteGrid::around(place.lon, place.lat, place.half_km, cfg.max_px);
    let b = grid.bounds;
    let bbox = [b.west(), b.south(), b.east(), b.north()];
    let from = today.add_days(-30 * cfg.months);
    let content = bundled_content();
    // one resolution for the whole build (mixing them would bias the area series)
    let speed = http::fetcher().throughput();
    let slack = cfg.slack.unwrap_or(if speed.is_none_or(|s| s < SLOW_LINK) { 3.5 } else { 1.5 });
    SLACK.with(|s| s.set(slack));
    let mut scenes: Vec<OpticalScene> = Vec::new();
    let mut done: std::collections::BTreeSet<Date> = std::collections::BTreeSet::new();
    // set when the first look shows the lake running off the edge: the area is then widened
    let grow = std::cell::Cell::new(false);

    let run = |items: &[Item], label: &str, scenes: &mut Vec<OpticalScene>, done: &mut std::collections::BTreeSet<Date>| {
        let todo: Vec<(Date, Vec<&Item>)> = pick_dates(items, b, cfg.every_days).into_iter().filter(|(d, _)| *d >= from && !done.contains(d)).collect();
        if todo.is_empty() {
            return;
        }
        progress(&format!(
            "Sentinel-2 {label}: {} dates to read · link {} → {}",
            todo.len(),
            speed.map_or("not measured yet".into(), |s| format!("~{:.0} KB/s", s / 1000.0)),
            if slack > 2.0 { "coarse overview (fast)" } else { "fine overview (detailed)" }
        ));
        let total = todo.len();
        for (k, chunk) in todo.chunks(cfg.batch.max(1)).enumerate() {
            if grow.get() {
                return;
            }
            let (t0, b0) = (std::time::Instant::now(), http::fetcher().fetched.load(std::sync::atomic::Ordering::Relaxed));
            let got: Vec<Result<Option<OpticalScene>>> = http::parallel(chunk, 3, |(d, its)| {
                SLACK.with(|s| s.set(slack));
                scene(*d, its, &grid, cfg.min_clear)
            });
            for ((d, _), r) in chunk.iter().zip(got) {
                match r {
                    Ok(Some(s)) => {
                        done.insert(*d);
                        scenes.push(s);
                    }
                    Ok(None) => {
                        done.insert(*d);
                    }
                    Err(e) => eprintln!("{d}: scene skipped: {e}"),
                }
            }
            // the link speed actually achieved (all connections together), remembered for the next build
            let got_bytes = http::fetcher().fetched.load(std::sync::atomic::Ordering::Relaxed) - b0;
            if got_bytes > 500_000 {
                http::fetcher().remember_throughput(got_bytes as f64 / t0.elapsed().as_secs_f64());
            }
            let n = ((k + 1) * cfg.batch).min(total);
            let mb = http::fetcher().fetched.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6;
            progress(&format!("Sentinel-2 {label}: {n}/{total} dates · {} clear in all · {mb:.0} MB downloaded", scenes.len()));
            // partial results: after the first batch (the first map), then every third batch; the analysis
            // runs over all scenes so far, so doing it after every batch would cost O(n²)
            if !(k == 0 || (k % 3 == 2 && n < total)) {
                continue;
            }
            let t = std::time::Instant::now();
            let an = if scenes.len() >= 2 { analyse(&grid, scenes, Some([place.lon, place.lat])) } else { None };
            timing::add(&timing::ANALYSE, t);
            if let Some(a) = an {
                if a.touches_edge && place.half_km < MAX_HALF_KM {
                    grow.set(true);
                    progress(&format!("the lake runs past the {:.0} km area: widening", 2.0 * place.half_km));
                }
                let t = std::time::Instant::now();
                partial(assemble(place, &grid, &a, None, &content));
                timing::add(&timing::ASSEMBLE, t);
            }
        }
    };

    if let Some(old) = stac::last("sentinel-2-l2a", bbox, Some(cfg.max_tile_cloud)) {
        run(&old, "(saved list)", &mut scenes, &mut done);
    }
    if grow.get() {
        return build(&Place { half_km: (place.half_km * 2.2).min(MAX_HALF_KM), ..place.clone() }, cfg, today, progress, partial);
    }
    progress("Sentinel-2: searching the archive …");
    match stac::search(stac::EARTH_SEARCH, "sentinel-2-l2a", bbox, from, today, Some(cfg.max_tile_cloud)) {
        Ok(items) => run(&items, "(new)", &mut scenes, &mut done),
        Err(e) if !scenes.is_empty() => progress(&format!("archive search failed ({e}); showing the saved scenes")),
        Err(e) => return Err(e),
    }
    if grow.get() {
        return build(&Place { half_km: (place.half_km * 2.2).min(MAX_HALF_KM), ..place.clone() }, cfg, today, progress, partial);
    }
    if scenes.is_empty() {
        return Err(Error::Data("no clear Sentinel-2 scene over this place".into()));
    }
    let t = std::time::Instant::now();
    let a = analyse(&grid, &scenes, Some([place.lon, place.lat])).ok_or_else(|| Error::Data("no water body found in the clear scenes".into()))?;
    timing::add(&timing::ANALYSE, t);

    // the lake model, fitted on the measured areas with ERA5 at the lake
    let used: Vec<_> = a.scenes.iter().filter(|s| s.water_km2.is_some()).collect();
    let model = if used.len() >= 6 {
        progress("ERA5 → lake model fit …");
        let fit_cfg = FitConfig::default();
        let end = today.add_days(-3);
        let from = Date::ymd(fit_cfg.generator_years.map_or(fit_cfg.climate_from, |y| (end.year() - y - 1).min(fit_cfg.climate_from)), 1, 1);
        match crate::weather::daily_rain_et0(a.center[0], a.center[1], from, end) {
            Ok(wx) => {
                let ds = assemble(place, &grid, &a, None, &content);
                let t = std::time::Instant::now();
                let m = ds.regions.regions[0].reservoir.as_ref().map(|res| forecast::fit("lake", res, &wx, end, &fit_cfg));
                timing::add(&timing::FIT, t);
                m.and_then(|m| match forecast::fit_quality(&m) {
                    Ok(r2) => {
                        progress(&format!("lake model fitted (R² {r2:.2})"));
                        Some(m)
                    }
                    Err(why) => {
                        progress(&format!("no lake forecast: {why}"));
                        None
                    }
                })
            }
            Err(e) => {
                eprintln!("ERA5 unavailable: {e}");
                None
            }
        }
    } else {
        None
    };
    Ok(assemble(place, &grid, &a, model, &content))
}

// ───────────────────────────────────────────────────────── finding a place by name

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub name: String,
    pub detail: String,
    pub lon: f64,
    pub lat: f64,
    /// A lake, reservoir, lagoon or pond.
    #[serde(default)]
    pub water: bool,
    /// Its extent [w, s, e, n], when known (OpenStreetMap).
    #[serde(default)]
    pub bbox: Option<[f64; 4]>,
}

/// GeoNames feature codes of standing water.
fn is_water(code: &str) -> bool {
    matches!(code, "LK" | "LKS" | "LKN" | "LKNI" | "LKI" | "LKO" | "LKOI" | "LKC" | "LKSC" | "LKSI" | "LKSN" | "LKSB" | "LKX" | "RSV" | "RSVI" | "RSVT" | "LGN" | "LGNS" | "PND" | "PNDS" | "PNDI" | "PNDSF" | "RES")
}

impl Found {
    /// Half the side of an area that holds the whole water body (with a margin), when its extent is known.
    pub fn half_km(&self) -> Option<f64> {
        let b = self.bbox?;
        let wk = (b[2] - b[0]) * 111.32 * self.lat.to_radians().cos();
        let hk = (b[3] - b[1]) * 110.57;
        Some((wk.max(hk) / 2.0 * 1.3).clamp(1.5, MAX_HALF_KM))
    }
}

/// Place search. Lakes, reservoirs and lagoons come from OpenStreetMap (Nominatim), which also gives their
/// extent; towns and anything Nominatim misses from GeoNames (Open-Meteo). Water bodies are listed first.
pub fn geocode(query: &str, lang: &str) -> Result<Vec<Found>> {
    let mut out: Vec<Found> = Vec::new();
    if let Ok(v) = http::fetcher().get_json_query(
        "https://nominatim.openstreetmap.org/search",
        &[("q", query), ("format", "jsonv2"), ("limit", "8"), ("accept-language", lang)],
    ) {
        for r in v.as_array().into_iter().flatten() {
            let num = |k: &str| r[k].as_str().and_then(|s| s.parse::<f64>().ok());
            let (Some(lat), Some(lon)) = (num("lat"), num("lon")) else { continue };
            let kind = (r["category"].as_str().unwrap_or(""), r["type"].as_str().unwrap_or(""));
            let water = matches!(kind, ("water" | "natural", "lake" | "reservoir" | "water" | "lagoon" | "pond" | "oxbow" | "basin") | ("landuse", "reservoir" | "basin"));
            let bbox = r["boundingbox"].as_array().filter(|b| b.len() == 4).and_then(|b| {
                let f = |i: usize| b[i].as_str()?.parse::<f64>().ok();
                Some([f(2)?, f(0)?, f(3)?, f(1)?]) // [south, north, west, east] → [w, s, e, n]
            });
            let display = r["display_name"].as_str().unwrap_or("");
            let name = r["name"].as_str().filter(|s| !s.is_empty()).unwrap_or(display.split(',').next().unwrap_or(query)).to_string();
            let detail = display.split(", ").skip(1).collect::<Vec<_>>().join(", ");
            out.push(Found { name, detail, lon, lat, water, bbox: if water { bbox } else { None } });
        }
    }
    if out.iter().all(|f| !f.water) {
        let v = http::fetcher().get_json_query("https://geocoding-api.open-meteo.com/v1/search", &[("name", query), ("count", "8"), ("language", lang), ("format", "json")])?;
        for r in v["results"].as_array().into_iter().flatten() {
            let (Some(name), Some(lon), Some(lat)) = (r["name"].as_str(), r["longitude"].as_f64(), r["latitude"].as_f64()) else { continue };
            out.push(Found {
                name: name.to_string(),
                detail: [r["admin1"].as_str(), r["country"].as_str()].into_iter().flatten().collect::<Vec<_>>().join(", "),
                lon,
                lat,
                water: r["feature_code"].as_str().is_some_and(is_water),
                bbox: None,
            });
        }
    }
    out.sort_by_key(|f| !f.water);
    Ok(out)
}
