//! Daily weather from Open-Meteo (free, no key, CC BY 4.0).
//!
//! * history: `archive-api.open-meteo.com` (ERA5 reanalysis: a model constrained by observations, not a station)
//! * today + 7-day forecast: `api.open-meteo.com`
//!
//! Every successful load is cached on disk; without a network the newest cached copy is returned and marked
//! as a snapshot, so the Status view keeps working offline.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use ed_core::forecast::DailyWeather;
use ed_core::model::{Region, Weather, WeatherOrigin, WeatherSet};
use ed_core::Date;
use serde::{Deserialize, Serialize};

use crate::{cache, Error, Result};

const DAILY: &str = "precipitation_sum,temperature_2m_max,temperature_2m_min,et0_fao_evapotranspiration,wind_speed_10m_max";
const ARCHIVE: &str = "https://archive-api.open-meteo.com/v1/archive";
const FORECAST: &str = "https://api.open-meteo.com/v1/forecast";

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(120))).user_agent("earthdance/0.1").build().into()
}

#[derive(Deserialize)]
struct Daily {
    time: Vec<Date>,
    #[serde(default)]
    precipitation_sum: Vec<Option<f64>>,
    #[serde(default)]
    temperature_2m_max: Vec<Option<f64>>,
    #[serde(default)]
    temperature_2m_min: Vec<Option<f64>>,
    #[serde(default)]
    et0_fao_evapotranspiration: Vec<Option<f64>>,
    #[serde(default)]
    wind_speed_10m_max: Vec<Option<f64>>,
}

#[derive(Deserialize)]
struct Place {
    daily: Option<Daily>,
}

/// One or many places: Open-Meteo returns an object for one location and an array for several.
#[derive(Deserialize)]
#[serde(untagged)]
enum Places {
    Many(Vec<Place>),
    One(Place),
}

fn call(base: &str, points: &[[f64; 2]], extra: &[(&str, String)]) -> Result<Vec<Place>> {
    let lat = points.iter().map(|p| p[1].to_string()).collect::<Vec<_>>().join(",");
    let lon = points.iter().map(|p| p[0].to_string()).collect::<Vec<_>>().join(",");
    let mut req = agent().get(base).query("latitude", &lat).query("longitude", &lon).query("daily", DAILY).query("timezone", "auto");
    for (k, v) in extra {
        req = req.query(*k, v);
    }
    let mut resp = req.call()?;
    let body = resp.body_mut().with_config().limit(64 << 20).read_to_string()?;
    Ok(match serde_json::from_str::<Places>(&body)? {
        Places::Many(v) => v,
        Places::One(p) => vec![p],
    })
}

/// Merges daily series into one per place; later parts win where both have a value
/// (pass the forecast first and the reanalysis second, so the reanalysis wins).
fn merge(parts: &[&Place]) -> Weather {
    type Row = [Option<f64>; 5];
    let mut by_day: BTreeMap<Date, Row> = BTreeMap::new();
    for p in parts {
        let Some(d) = &p.daily else { continue };
        let cols = [&d.precipitation_sum, &d.temperature_2m_max, &d.temperature_2m_min, &d.et0_fao_evapotranspiration, &d.wind_speed_10m_max];
        for (i, day) in d.time.iter().enumerate() {
            let row = by_day.entry(*day).or_default();
            for (k, c) in cols.iter().enumerate() {
                if let Some(Some(v)) = c.get(i) {
                    row[k] = Some(*v);
                }
            }
        }
    }
    let mut w = Weather::default();
    for (d, r) in by_day {
        w.time.push(d);
        w.rain.push(r[0]);
        w.tmax.push(r[1]);
        w.tmin.push(r[2]);
        w.et0.push(r[3]);
        w.wind.push(r[4]);
    }
    w
}

#[derive(Serialize, Deserialize)]
struct Cached {
    saved: Date,
    #[serde(rename = "byRegion")]
    by_region: BTreeMap<String, Weather>,
}

/// Daily weather for each area from `start` to a week after `today`: live when possible, otherwise the
/// newest cached copy (`source = Snapshot`), otherwise `fallback` (e.g. a snapshot shipped with the data).
pub fn load(regions: &[Region], start: Date, today: Date, fallback: Option<WeatherSet>) -> Option<WeatherSet> {
    let ids = regions.iter().map(|r| r.id.as_str()).collect::<Vec<_>>().join("+");
    let prefix = format!("wx-{ids}-{start}");
    let key = format!("{prefix}-{today}");
    if let Some(c) = cache::get::<Cached>(&key) {
        return Some(WeatherSet { source: WeatherOrigin::Live, saved: Some(c.saved), by_region: c.by_region });
    }
    match load_live(regions, start, today) {
        Ok(by_region) => {
            cache::put(&key, &Cached { saved: today, by_region: by_region.clone() });
            Some(WeatherSet { source: WeatherOrigin::Live, saved: Some(today), by_region })
        }
        Err(e) => {
            eprintln!("live weather unavailable ({e}); using a saved copy");
            cache::newest::<Cached>(&prefix)
                .map(|c| WeatherSet { source: WeatherOrigin::Snapshot, saved: Some(c.saved), by_region: c.by_region })
                .or(fallback)
        }
    }
}

fn load_live(regions: &[Region], start: Date, today: Date) -> Result<BTreeMap<String, Weather>> {
    let pts: Vec<[f64; 2]> = regions.iter().map(|r| r.center).collect();
    let hist = call(ARCHIVE, &pts, &[("start_date", start.iso()), ("end_date", today.add_days(-1).iso())])?;
    let fc = call(FORECAST, &pts, &[("past_days", "14".into()), ("forecast_days", "7".into())])?;
    if hist.len() != regions.len() || fc.len() != regions.len() {
        return Err(Error::Data("Open-Meteo returned a different number of places".into()));
    }
    Ok(regions.iter().enumerate().map(|(i, r)| (r.id.clone(), merge(&[&fc[i], &hist[i]]))).collect())
}

/// The archive in decade-long requests (long ones time out). Finished decades are cached on their own, so
/// a rate limit (HTTP 429) part-way loses nothing; 429s are retried with a growing pause.
fn fetch_chunked(lon: f64, lat: f64, from: Date, to: Date) -> Result<Vec<Place>> {
    let mut out = Vec::new();
    let mut a = from;
    while a <= to {
        let b = Date::ymd(a.year() + 9, 12, 31).min(to);
        let key = format!("era5chunk-{lat:.3}-{lon:.3}-{a}-{b}");
        let rows: Vec<(Date, f64, f64)> = match cache::get(&key) {
            Some(r) => r,
            None => {
                let q = [("start_date", a.iso()), ("end_date", b.iso())];
                let mut tries = 0;
                let part = loop {
                    match call(ARCHIVE, &[[lon, lat]], &q) {
                        Ok(p) => break p,
                        Err(Error::Http(e)) if tries < 5 => {
                            tries += 1;
                            let wait = if e.contains("429") { 20 * tries } else { 3 };
                            eprintln!("Open-Meteo: {e}; retrying in {wait} s");
                            std::thread::sleep(Duration::from_secs(wait));
                        }
                        Err(e) => return Err(e),
                    }
                };
                let w = merge(&part.iter().collect::<Vec<_>>());
                let rows: Vec<(Date, f64, f64)> = w.time.iter().enumerate().filter_map(|(i, d)| Some((*d, w.rain[i]?, w.et0[i]?))).collect();
                // a decade that is over never changes: cache it
                if b < to {
                    cache::put(&key, &rows);
                }
                rows
            }
        };
        out.push(Place {
            daily: Some(Daily {
                time: rows.iter().map(|r| r.0).collect(),
                precipitation_sum: rows.iter().map(|r| Some(r.1)).collect(),
                et0_fao_evapotranspiration: rows.iter().map(|r| Some(r.2)).collect(),
                temperature_2m_max: Vec::new(),
                temperature_2m_min: Vec::new(),
                wind_speed_10m_max: Vec::new(),
            }),
        });
        a = b.add_days(1);
    }
    Ok(out)
}

/// Daily rain + ET0 at one place over `[from, to]` (reanalysis), for fitting the lake model.
/// Cached per (place, to); offline the newest cached copy is used.
pub fn daily_rain_et0(lon: f64, lat: f64, from: Date, to: Date) -> Result<DailyWeather> {
    let prefix = format!("era5-{lat:.3}-{lon:.3}-{from}");
    let key = format!("{prefix}-{to}");
    let rows: Vec<(Date, f64, f64)> = match cache::get(&key) {
        Some(r) => r,
        None => match fetch_chunked(lon, lat, from, to) {
            Ok(places) => {
                let w = merge(&places.iter().collect::<Vec<_>>());
                let rows: Vec<(Date, f64, f64)> =
                    w.time.iter().enumerate().filter_map(|(i, d)| Some((*d, w.rain[i]?, w.et0[i]?))).collect();
                cache::put(&key, &rows);
                rows
            }
            Err(e) => cache::newest(&prefix).ok_or(e)?,
        },
    };
    Ok(rows.into_iter().map(|(d, r, e)| (d, (r, e))).collect::<HashMap<_, _>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_prefers_later_parts() {
        let a: Place = serde_json::from_str(r#"{"daily":{"time":["2026-01-01","2026-01-02"],"precipitation_sum":[1.0,2.0],"et0_fao_evapotranspiration":[null,0.5]}}"#).unwrap();
        let b: Place = serde_json::from_str(r#"{"daily":{"time":["2026-01-02","2026-01-03"],"precipitation_sum":[9.0,null]}}"#).unwrap();
        let w = merge(&[&a, &b]);
        assert_eq!(w.time.len(), 3);
        assert_eq!(w.rain, vec![Some(1.0), Some(9.0), None]);
        assert_eq!(w.et0, vec![None, Some(0.5), None]);
    }
}
