//! The domain model: what a monitored site is made of. Field names follow the legacy JSON files so that
//! existing data (and the test fixtures) deserialize directly.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{Bounds, Date, Grid, Rgba, Text};

// ───────────────────────────────────────────────────────── site + layer catalogue

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Site {
    pub name: Text,
    pub country: Text,
    pub bounds: Bounds,
    /// [lon, lat]
    pub center: [f64; 2],
    pub zoom: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reference {
    pub name: String,
    pub label: Text,
}

/// A colour ramp: `[value, "#rrggbb"]` stops, interpolated linearly. Drives the map and the legend alike.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Legend {
    pub stops: Vec<(f64, String)>,
    pub low: Text,
    pub high: Text,
}

impl Legend {
    /// Stops sorted by value with parsed colours.
    pub fn rgb_stops(&self) -> Vec<(f64, [u8; 3])> {
        let mut s: Vec<(f64, [u8; 3])> = self.stops.iter().map(|(v, c)| (*v, parse_hex(c))).collect();
        s.sort_by(|a, b| a.0.total_cmp(&b.0));
        s
    }
    pub fn range(&self) -> (f64, f64) {
        let it = self.stops.iter().map(|s| s.0);
        (it.clone().fold(f64::INFINITY, f64::min), it.fold(f64::NEG_INFINITY, f64::max))
    }
}

pub fn parse_hex(c: &str) -> [u8; 3] {
    let h = c.trim_start_matches('#');
    let p = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).unwrap_or(0);
    [p(0), p(2), p(4)]
}

/// A layer as described in the catalogue (what it means), without its frames.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerInfo {
    pub id: String,
    pub band: String,
    pub sensor: String,
    pub product: String,
    pub unit: String,
    pub name: Text,
    pub short: Text,
    pub description: Text,
    pub legend: Legend,
    /// `(upper bound, explanation)`: the first class with `value <= bound` explains the hovered value.
    #[serde(default)]
    pub classes: Vec<(f64, Text)>,
}

impl LayerInfo {
    pub fn class_of(&self, v: f64) -> Option<&Text> {
        self.classes.iter().find(|(max, _)| v <= *max).map(|(_, t)| t)
    }
}

/// One acquisition of a layer.
#[derive(Clone, Debug)]
pub struct Frame {
    pub date: Date,
    /// The pass exists but its file holds no valid pixels.
    pub empty: bool,
    pub source: String,
    pub url: String,
    /// Physical values (NaN = no data), what the hover reads.
    pub values: Option<Arc<Grid<f32>>>,
    /// The coloured picture drawn on the map.
    pub image: Option<Arc<Rgba>>,
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub info: LayerInfo,
    pub frames: Vec<Frame>,
}

/// What a normal (optical) satellite saw on a date: the "Optical" side of the sensor switch.
#[derive(Clone, Debug)]
pub struct OpticalFrame {
    pub date: Date,
    /// Cloud cover in %, when known (demo set).
    pub cover: Option<f64>,
    pub source: String,
    pub image: Arc<Rgba>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Missing {
    pub id: String,
    pub name: Text,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub site: Site,
    pub reference: Option<Reference>,
    pub layers: Vec<LayerInfo>,
}

// ───────────────────────────────────────────────────────── points

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PointSeries {
    pub dates: Vec<Date>,
    pub disp_mm: Vec<f64>,
    #[serde(default)]
    pub ref_mm: Option<Vec<f64>>,
    pub hh_db: Vec<f64>,
    pub hv_db: Vec<f64>,
    #[serde(default)]
    pub vv_db: Option<Vec<f64>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Point {
    pub id: String,
    pub name: Text,
    /// subsidence | water | flood | crop | urban | stable
    pub kind: String,
    pub lon: f64,
    pub lat: f64,
    pub series: PointSeries,
}

/// Reads a GeoJSON FeatureCollection of points (`data/points.geojson`).
pub fn points_from_geojson(v: &serde_json::Value) -> serde_json::Result<Vec<Point>> {
    #[derive(Deserialize)]
    struct Props {
        id: String,
        name: Text,
        kind: String,
        series: PointSeries,
    }
    #[derive(Deserialize)]
    struct Geom {
        coordinates: [f64; 2],
    }
    #[derive(Deserialize)]
    struct Feature {
        properties: Props,
        geometry: Geom,
    }
    #[derive(Deserialize)]
    struct Fc {
        features: Vec<Feature>,
    }
    let fc = Fc::deserialize(v)?;
    Ok(fc
        .features
        .into_iter()
        .map(|f| Point {
            id: f.properties.id,
            name: f.properties.name,
            kind: f.properties.kind,
            lon: f.geometry.coordinates[0],
            lat: f.geometry.coordinates[1],
            series: f.properties.series,
        })
        .collect())
}

// ───────────────────────────────────────────────────────── monitoring areas

/// Per-pass statistics of one area. Every series is aligned with `dates`; any of them may be absent.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Series {
    pub dates: Vec<Date>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disp_mm: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_mm: Option<Vec<f64>>,
    /// `None` entries = no radar value for that date (e.g. an optical-only date).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hh_db: Option<Vec<Option<f64>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hv_db: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vv_db: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water_km2: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water_source: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vol_loss_hm3: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soil_moist_pct: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reed_km2: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared_ha: Option<Vec<f64>>,
}

/// A lake or reservoir with a minimum limit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reservoir {
    pub name: Text,
    pub unit: String,
    pub capacity: f64,
    pub limit: f64,
    pub dates: Vec<Date>,
    pub volume: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_km2: Option<Vec<f64>>,
    /// Today's shoreline, closed ring of [lon, lat].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<Vec<[f64; 2]>>,
    pub method: Text,
    pub rule: Text,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Region {
    pub id: String,
    pub name: Text,
    /// [lon, lat]
    pub center: [f64; 2],
    pub radius_km: f64,
    #[serde(default)]
    pub points: Vec<String>,
    pub series: Series,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservoir: Option<Reservoir>,
}

/// Every threshold the rules use. Change one and the conclusions change with it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thresholds {
    pub sink_watch_mm_yr: f64,
    pub sink_alert_mm_yr: f64,
    pub egms_tolerance_mm_yr: f64,
    pub reservoir_warn_days: f64,
    pub heavy_rain_mm_day: f64,
    pub heat_c: f64,
    pub dry_spell_days: f64,
    pub soil_dry_drop_pct: f64,
    pub reed_loss_pct: f64,
    pub cleared_watch_ha: f64,
    pub cleared_alert_ha: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            sink_watch_mm_yr: -15.0,
            sink_alert_mm_yr: -40.0,
            egms_tolerance_mm_yr: 8.0,
            reservoir_warn_days: 45.0,
            heavy_rain_mm_day: 40.0,
            heat_c: 38.0,
            dry_spell_days: 30.0,
            soil_dry_drop_pct: 15.0,
            reed_loss_pct: 10.0,
            cleared_watch_ha: 5.0,
            cleared_alert_ha: 25.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Regions {
    pub thresholds: Thresholds,
    pub regions: Vec<Region>,
}

// ───────────────────────────────────────────────────────── weather

/// Daily weather for one place. All vectors are aligned with `time`; `None` = no value that day.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Weather {
    pub time: Vec<Date>,
    pub rain: Vec<Option<f64>>,
    pub tmax: Vec<Option<f64>>,
    pub tmin: Vec<Option<f64>>,
    pub et0: Vec<Option<f64>>,
    pub wind: Vec<Option<f64>>,
}

impl Weather {
    pub fn index_of(&self, d: Date) -> Option<usize> {
        self.time.binary_search(&d).ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WeatherOrigin {
    Live,
    Snapshot,
}

fn snapshot() -> WeatherOrigin {
    WeatherOrigin::Snapshot
}

/// Weather for every area, with where it came from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WeatherSet {
    /// Not read from files: a saved file is a snapshot by definition.
    #[serde(skip_deserializing, default = "snapshot")]
    pub source: WeatherOrigin,
    /// When the snapshot was saved (snapshot only).
    #[serde(default)]
    pub saved: Option<Date>,
    #[serde(rename = "byRegion")]
    pub by_region: BTreeMap<String, Weather>,
}

// ───────────────────────────────────────────────────────── lake forecast model (fitted)

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Params {
    pub a_wet: f64,
    pub a_dry: f64,
    pub k: f64,
    pub q: f64,
    pub amax: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateSpan {
    pub early: Option<Date>,
    pub mid: Option<Date>,
    pub late: Option<Date>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Backtest {
    pub cutoff: Date,
    pub passes: usize,
    pub target: Date,
    pub predicted: f64,
    pub observed: f64,
    pub cross: Option<DateSpan>,
    pub members: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FitSeries {
    pub dates: Vec<Date>,
    pub measured: Vec<f64>,
    pub model: Vec<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClimateYear {
    pub rain: Vec<Option<f64>>,
    pub et0: Vec<Option<f64>>,
}

/// Daily rain and ET0 for past years, 366 slots each (index = [`Date::climate_slot`]).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Climate {
    pub years: Vec<i32>,
    pub until: Date,
    pub days: BTreeMap<String, ClimateYear>,
}

impl Climate {
    pub fn year(&self, y: i32) -> Option<&ClimateYear> {
        self.days.get(&y.to_string())
    }
}

/// `data/real/forecast.json`: the fitted water balance of the lake.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ForecastModel {
    pub region: String,
    pub params: Params,
    #[serde(default)]
    pub sd: BTreeMap<String, f64>,
    pub rmse: f64,
    pub fit: FitSeries,
    pub backtest: Option<Backtest>,
    pub climate: Climate,
    /// The fitted weather generator; without it the forecast replays the climate years.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<crate::weathergen::WeatherGen>,
}

// ───────────────────────────────────────────────────────── validation + content

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ValidationPair {
    pub nisar: f64,
    #[serde(rename = "ref")]
    pub reference: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Validation {
    pub unit: String,
    pub note: Text,
    pub pairs: Vec<ValidationPair>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoryChapter {
    pub kicker: Text,
    pub title: Text,
    pub body: Text,
    #[serde(default)]
    pub sensor: Option<String>,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub frame: Option<usize>,
    #[serde(default)]
    pub point: Option<String>,
    #[serde(default)]
    pub center: Option<[f64; 2]>,
    #[serde(default)]
    pub zoom: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Explanation {
    pub title: Text,
    pub text: Text,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Voice {
    pub quote: Text,
    pub who: Text,
    #[serde(default)]
    pub media: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceLink {
    pub name: String,
    pub url: String,
    pub by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct About {
    pub tagline: Text,
    pub sources: Vec<SourceLink>,
    pub team: Text,
    pub ai: Text,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Content {
    #[serde(default)]
    pub story: Vec<StoryChapter>,
    #[serde(default)]
    pub explanations: BTreeMap<String, Explanation>,
    #[serde(default)]
    pub voices: BTreeMap<String, Voice>,
    #[serde(default)]
    pub unknowns: Vec<Text>,
    pub about: About,
}

/// The bundled content (explanations, story, about) and layer catalogue.
pub fn bundled_content() -> Content {
    serde_json::from_str(include_str!("../../../assets/content.json")).expect("assets/content.json is valid")
}
pub fn bundled_catalog() -> Catalog {
    serde_json::from_str(include_str!("../../../assets/catalog.json")).expect("assets/catalog.json is valid")
}

// ───────────────────────────────────────────────────────── the whole data set

/// Everything one view of a site needs, built in memory by a source (no files in between).
#[derive(Clone, Debug)]
pub struct Dataset {
    /// Synthetic data: the app says so everywhere.
    pub demo: bool,
    pub site: Site,
    pub reference: Option<Reference>,
    pub layers: Vec<Layer>,
    pub optical: Vec<OpticalFrame>,
    pub points: Vec<Point>,
    pub regions: Regions,
    pub forecast: Option<ForecastModel>,
    pub validation: Option<Validation>,
    pub content: Content,
    /// Layers this data set does not have yet.
    pub missing: Vec<Missing>,
}

impl Dataset {
    pub fn layer(&self, id: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.info.id == id)
    }
    pub fn region(&self, id: &str) -> Option<&Region> {
        self.regions.regions.iter().find(|r| r.id == id)
    }
    pub fn point(&self, id: &str) -> Option<&Point> {
        self.points.iter().find(|p| p.id == id)
    }
    pub fn reference_name(&self) -> &str {
        self.reference.as_ref().map_or("Reference", |r| r.name.as_str())
    }
}
