//! The real data set, processed at load time: acquisitions → `ed-raster` → layers, area series, lake
//! outline; ERA5 weather → the lake model is fitted on the spot. Nothing is read back from pre-made images.
//!
//! Where the acquisitions come from is [`AcquisitionSource`]. The only implementation so far,
//! [`LocalDirs`], reads the GeoTIFFs kept in the repository; fetching NISAR GCOV and HLS from the archives
//! is decision D1 and slots in behind the same trait.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed_core::forecast::{self, DailyWeather, FitConfig};
use ed_core::model::{
    bundled_catalog, bundled_content, Dataset, ForecastModel, Frame, Layer, LayerInfo, Missing, OpticalFrame, Region, Regions, Reservoir, Series, Site,
    Thresholds, WeatherSet,
};
use ed_core::{Date, Grid, Text};
use ed_raster::koroneia::{self, Inputs, Legends, SiteConfig};

use crate::{Error, Result};

/// Something that can hand over the acquisitions for a site.
pub trait AcquisitionSource: Send + Sync {
    fn inputs(&self, progress: &dyn Fn(&str)) -> Result<Inputs>;
}

/// GeoTIFFs in two folders (NISAR GCOV HHHH and HLS Sentinel-2 B04/B08).
pub struct LocalDirs {
    pub nisar: PathBuf,
    pub optical: PathBuf,
}

impl LocalDirs {
    pub fn in_repo(root: &Path) -> Self {
        Self { nisar: root.join("Koroneia_NISAR"), optical: root.join("Koroneia_Sentinel") }
    }
}

impl AcquisitionSource for LocalDirs {
    fn inputs(&self, progress: &dyn Fn(&str)) -> Result<Inputs> {
        progress("NISAR + Sentinel-2 …");
        Ok(koroneia::read_inputs_from_dirs(&self.nisar, &self.optical)?)
    }
}

/// Where the fit's daily weather comes from.
pub enum FitWeather {
    /// Open-Meteo ERA5 at the lake (cached; offline the newest cached copy).
    OpenMeteo,
    /// A given series (tests, or offline with no cache).
    Given(DailyWeather),
}

fn zone_name(id: &str) -> Text {
    match id {
        "koroneia_core" => Text::new("Λίμνη Κορώνεια", "Lake Koronia"),
        "koroneia_shore" => Text::new("Δυτική όχθη & καλαμιώνας", "West shore & reeds"),
        other => Text::same(other),
    }
}

fn change_info(mut info: LayerInfo) -> LayerInfo {
    info.description = Text::new(
        "Διαφορά HH από την πρώτη λήψη της ίδιας τροχιάς. Μπλε = πιο σκοτεινό (συνήθως νερό). Πορτοκαλί = πιο φωτεινό (τραχύτητα, υγρασία, βλάστηση).",
        "HH difference from the first pass of the same orbit. Blue = darker (usually water). Orange = brighter (roughness, moisture, vegetation).",
    );
    info
}

/// Builds the real set from the repository's acquisitions with live ERA5 weather for the fit.
pub fn load(root: &Path, progress: &dyn Fn(&str)) -> Result<(Dataset, Option<WeatherSet>)> {
    let fallback = || -> FitWeather {
        // offline and nothing cached: refit on the (rounded) weather the legacy forecast file kept
        std::fs::read_to_string(root.join("data/real/forecast.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<ForecastModel>(&s).ok())
            .map_or(FitWeather::Given(DailyWeather::new()), |f| FitWeather::Given(forecast::weather_from_climate(&f.climate)))
    };
    let today = crate::today_local();
    let cfg = FitConfig::default();
    let ds = build(&LocalDirs::in_repo(root), FitWeather::OpenMeteo, &cfg, today, progress).or_else(|e| match e {
        Error::Http(_) => build(&LocalDirs::in_repo(root), fallback(), &cfg, today, progress),
        e => Err(e),
    })?;
    let snapshot: Option<WeatherSet> = std::fs::read_to_string(root.join("data/real/weather.json")).ok().and_then(|s| serde_json::from_str(&s).ok());
    Ok((ds, snapshot))
}

/// The whole real data set from any acquisition source.
pub fn build(src: &dyn AcquisitionSource, fit_weather: FitWeather, fit_cfg: &FitConfig, today: Date, progress: &dyn Fn(&str)) -> Result<Dataset> {
    let cfg = SiteConfig::koronia();
    let catalog = bundled_catalog();
    let info = |id: &str| catalog.layers.iter().find(|l| l.id == id).cloned().ok_or_else(|| Error::Data(format!("layer {id} missing from the catalogue")));
    let (hh_info, ch_info) = (info("hh")?, change_info(info("change")?));
    let legends = Legends { hh: hh_info.legend.rgb_stops(), change: ch_info.legend.rgb_stops() };

    let inputs = src.inputs(progress)?;
    progress("SAR → dB, water, change …");
    let b = koroneia::build(&cfg, &inputs, &legends).map_err(|e| Error::Data(format!("{e:?}")))?;

    // hover reads full-resolution values, only where the picture is drawn
    let masked = |g: &Grid<f32>| -> Arc<Grid<f32>> {
        let mut g = g.clone();
        for (v, z) in g.data.iter_mut().zip(&b.zone.data) {
            if !*z {
                *v = f32::NAN;
            }
        }
        Arc::new(g)
    };
    let mut hh = Layer { info: hh_info, frames: Vec::new() };
    let mut ch = Layer { info: ch_info, frames: Vec::new() };
    for f in &b.sar_frames {
        let frame = |values: Option<Arc<Grid<f32>>>, image| Frame { date: f.date, empty: f.layers.is_none(), source: f.source.clone(), url: String::new(), values, image };
        match &f.layers {
            Some(l) => {
                hh.frames.push(frame(Some(masked(&l.hh_db)), Some(Arc::new(l.hh_rgba.clone()))));
                ch.frames.push(frame(Some(masked(&l.change_db)), Some(Arc::new(l.change_rgba.clone()))));
            }
            None => {
                hh.frames.push(frame(None, None));
                ch.frames.push(frame(None, None));
            }
        }
    }
    let optical = b.optical_frames.iter().map(|o| OpticalFrame { date: o.date, cover: None, source: o.source.clone(), image: Arc::new(o.rgba.clone()) }).collect();

    let limit_pct = format!("{}%", ed_core::text::js_number(cfg.limit_share * 100.0));
    let sar_db = ed_core::text::js_number(f64::from(cfg.sar_water_db));
    let regions: Vec<Region> = cfg
        .zones
        .iter()
        .filter_map(|z| {
            let s = b.series.iter().find(|s| s.id == z.id)?;
            let reservoir = b.reservoir.as_ref().filter(|r| r.zone_id == z.id).map(|r| Reservoir {
                name: Text::new("Επιφάνεια Λίμνης Κορώνειας", "Lake Koronia surface"),
                unit: "km²".into(),
                capacity: r.capacity,
                limit: r.limit,
                dates: r.dates.clone(),
                volume: r.area_km2.clone(),
                area_km2: Some(r.area_km2.clone()),
                outline: Some(r.outline.clone()),
                method: Text::new(
                    format!("Επιφάνεια νερού μετρημένη από δορυφόρο: Sentinel-2 (HLS) έως τον Μάιο, NISAR HH ≤ {sar_db} dB από τον Ιούνιο. Το όριο ({limit_pct} της μέγιστης επιφάνειας) είναι ενδεικτικό."),
                    format!("Water surface measured from satellite: Sentinel-2 (HLS) until May, NISAR HH ≤ {sar_db} dB from June. The limit ({limit_pct} of the largest surface) is indicative."),
                ),
                rule: Text::new(
                    "Ενδεικτικό όριο. Να αντικατασταθεί με την ελάχιστη στάθμη/έκταση του Σχεδίου Διαχείρισης Λεκανών Απορροής Κεντρικής Μακεδονίας (EL10) και του σχεδίου αποκατάστασης της Κορώνειας.",
                    "Indicative limit. Replace with the minimum level/extent in the Central Macedonia River Basin Management Plan (EL10) and the Koronia restoration plan.",
                ),
            });
            Some(Region {
                id: z.id.clone(),
                name: zone_name(&z.id),
                center: z.center,
                radius_km: z.radius_km,
                points: Vec::new(),
                series: Series {
                    dates: s.dates.clone(),
                    hh_db: Some(s.hh_db.clone()),
                    water_km2: Some(s.water_km2.clone()),
                    water_source: Some(s.water_source.iter().map(|x| x.as_str().to_string()).collect()),
                    ..Default::default()
                },
                reservoir,
            })
        })
        .collect();

    // the lake model, fitted now on the measured areas
    let lake = regions.iter().find(|r| r.reservoir.is_some());
    let model = match lake {
        Some(lake) => {
            progress("ERA5 → lake model fit …");
            let end = today.add_days(-2);
            let wx = match fit_weather {
                FitWeather::Given(w) => w,
                FitWeather::OpenMeteo => {
                    // the climate scenarios start in climate_from; the weather generator needs its window too
                    let from = fit_cfg.generator_years.map_or(fit_cfg.climate_from, |y| (end.year() - y - 1).min(fit_cfg.climate_from));
                    crate::weather::daily_rain_et0(lake.center[0], lake.center[1], Date::ymd(from, 1, 1), end)?
                }
            };
            (!wx.is_empty()).then(|| {
                let end = wx.keys().max().copied().unwrap_or(end).min(end);
                forecast::fit(&lake.id, lake.reservoir.as_ref().expect("lake"), &wx, end, fit_cfg)
            })
        }
        None => None,
    };

    let missing = ["hv", "disp", "s1vv", "sm"]
        .iter()
        .filter_map(|id| catalog.layers.iter().find(|l| l.id == *id).map(|l| Missing { id: l.id.clone(), name: l.name.clone() }))
        .collect();
    Ok(Dataset {
        demo: false,
        site: Site { bounds: cfg.bounds, center: [23.15, 40.68], zoom: 11.1, ..catalog.site },
        reference: catalog.reference,
        layers: vec![hh, ch],
        optical,
        points: Vec::new(),
        regions: Regions { thresholds: Thresholds::default(), regions },
        forecast: model,
        validation: None,
        content: bundled_content(),
        missing,
    })
}
