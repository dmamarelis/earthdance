//! The demo data set: clearly synthetic data (made by the legacy `tools/make_demo_data.py`) for the features
//! that have no real data yet: HV, subsidence, Sentinel-1, soil moisture, EGMS, points and the Story.
//! It is read from `data/` into the same in-memory [`Dataset`] the real source builds; the app shows a
//! permanent "demo data" badge for it.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use ed_core::model::{points_from_geojson, Content, Dataset, Frame, Layer, LayerInfo, OpticalFrame, Reference, Regions, Site, Validation, WeatherSet};
use ed_core::{Bounds, Date, Grid, Rgba};
use serde::Deserialize;

use crate::{Error, Result};

#[derive(Deserialize)]
struct ManFrame {
    date: Date,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    values: Option<String>,
    #[serde(default)]
    empty: bool,
    #[serde(default)]
    source: String,
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
struct ManLayer {
    #[serde(flatten)]
    info: LayerInfo,
    #[serde(default)]
    grid: Option<[usize; 2]>,
    #[serde(default)]
    bounds: Option<Bounds>,
    frames: Vec<ManFrame>,
}

#[derive(Deserialize)]
struct Cloud {
    date: Date,
    image: String,
    #[serde(default)]
    cover: Option<f64>,
}

#[derive(Deserialize)]
struct Optical {
    #[serde(default)]
    bounds: Option<Bounds>,
    clouds: Vec<Cloud>,
}

#[derive(Deserialize)]
struct Manifest {
    demo: bool,
    site: Site,
    reference: Option<Reference>,
    layers: Vec<ManLayer>,
    optical: Option<Optical>,
    points: String,
    regions: String,
    #[serde(default)]
    validation: Option<String>,
    content: String,
}

fn json<T: serde::de::DeserializeOwned>(root: &Path, rel: &str) -> Result<T> {
    let p = root.join(rel);
    let s = fs::read_to_string(&p).map_err(|e| Error::Data(format!("{}: {e}", p.display())))?;
    serde_json::from_str(&s).map_err(|e| Error::Data(format!("{}: {e}", p.display())))
}

/// A hover grid file: little-endian int16, value × 10, −32768 = no data.
pub fn read_values(bytes: &[u8], w: usize, h: usize, bounds: Bounds) -> Result<Grid<f32>> {
    if bytes.len() != w * h * 2 {
        return Err(Error::Data(format!("values grid: {} bytes for {w}×{h}", bytes.len())));
    }
    let data = bytes
        .chunks_exact(2)
        .map(|c| match i16::from_le_bytes([c[0], c[1]]) {
            -32768 => f32::NAN,
            v => f32::from(v) / 10.0,
        })
        .collect();
    Ok(Grid::from_vec(w, h, bounds, data))
}

pub fn decode_image(bytes: &[u8], bounds: Bounds) -> Result<Rgba> {
    let img = image::load_from_memory(bytes).map_err(|e| Error::Data(format!("image: {e}")))?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px = img.into_raw().chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
    Ok(Rgba { w, h, bounds, px })
}

/// Loads the demo set from `<root>/data/manifest.json`. Also returns the weather snapshot shipped with it.
pub fn load(root: &Path) -> Result<(Dataset, Option<WeatherSet>)> {
    let m: Manifest = json(root, "data/manifest.json")?;
    let site_bounds = m.site.bounds;

    // decode all frames in parallel: ~70 small images
    let mut jobs: Vec<(usize, usize, &ManFrame, Bounds, Option<[usize; 2]>)> = Vec::new();
    for (li, l) in m.layers.iter().enumerate() {
        for (fi, f) in l.frames.iter().enumerate() {
            jobs.push((li, fi, f, l.bounds.unwrap_or(site_bounds), l.grid));
        }
    }
    type Decoded = (usize, usize, Option<Arc<Grid<f32>>>, Option<Arc<Rgba>>);
    let decoded: Vec<Decoded> = std::thread::scope(|s| {
        let chunks: Vec<_> = jobs
            .chunks(jobs.len().div_ceil(8).max(1))
            .map(|chunk| {
                s.spawn(move || {
                    chunk
                        .iter()
                        .map(|&(li, fi, f, b, grid)| {
                            let values = match (&f.values, grid) {
                                (Some(v), Some([w, h])) => fs::read(root.join(v)).ok().and_then(|b2| read_values(&b2, w, h, b).ok()).map(Arc::new),
                                _ => None,
                            };
                            let image = f.image.as_ref().and_then(|p| fs::read(root.join(p)).ok()).and_then(|b2| decode_image(&b2, b).ok()).map(Arc::new);
                            (li, fi, values, image)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        chunks.into_iter().flat_map(|h| h.join().expect("decoder thread")).collect()
    });

    let mut layers: Vec<Layer> = m
        .layers
        .iter()
        .map(|l| Layer {
            info: l.info.clone(),
            frames: l.frames.iter().map(|f| Frame { date: f.date, empty: f.empty, source: f.source.clone(), url: f.url.clone(), values: None, image: None }).collect(),
        })
        .collect();
    for (li, fi, values, image) in decoded {
        layers[li].frames[fi].values = values;
        layers[li].frames[fi].image = image;
    }

    let optical = match &m.optical {
        Some(o) => {
            let b = o.bounds.unwrap_or(site_bounds);
            o.clouds
                .iter()
                .filter_map(|c| {
                    let img = decode_image(&fs::read(root.join(&c.image)).ok()?, b).ok()?;
                    Some(OpticalFrame { date: c.date, cover: c.cover, source: "demo clouds".into(), image: Arc::new(img) })
                })
                .collect()
        }
        None => Vec::new(),
    };

    let points = points_from_geojson(&json::<serde_json::Value>(root, &m.points)?)?;
    let regions: Regions = json(root, &m.regions)?;
    let validation: Option<Validation> = m.validation.as_deref().map(|v| json(root, v)).transpose()?;
    let content: Content = json(root, &m.content)?;
    let wx: Option<WeatherSet> = json(root, "data/weather.json").ok();
    Ok((
        Dataset {
            demo: m.demo,
            site: m.site,
            reference: m.reference,
            layers,
            optical,
            points,
            regions,
            forecast: None,
            validation,
            content,
            missing: Vec::new(),
        },
        wx,
    ))
}
