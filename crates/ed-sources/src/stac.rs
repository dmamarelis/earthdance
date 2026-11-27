//! STAC search (SpatioTemporal Asset Catalog): finding the scenes over an area.

use ed_core::Date;
use serde_json::{json, Value};

use crate::{cache, http, Result};

pub const EARTH_SEARCH: &str = "https://earth-search.aws.element84.com/v1/search";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Item {
    pub id: String,
    pub date: Date,
    /// Cloud cover of the whole scene (%), when the catalogue gives it.
    pub cloud: Option<f64>,
    /// Asset name → URL.
    pub assets: std::collections::BTreeMap<String, String>,
    /// Footprint bounding box [w, s, e, n].
    pub bbox: Option<[f64; 4]>,
}

/// Every item of `collection` intersecting `bbox` in `[from, to]`, following the catalogue's pages.
/// Results are cached for a day (new scenes appear, old ones do not change).
pub fn search(endpoint: &str, collection: &str, bbox: [f64; 4], from: Date, to: Date, max_cloud: Option<f64>) -> Result<Vec<Item>> {
    let key = format!("stac-{collection}-{:.4}-{:.4}-{:.4}-{:.4}-{from}-{to}-{}-{}", bbox[0], bbox[1], bbox[2], bbox[3], max_cloud.unwrap_or(100.0), crate::today_local());
    if let Some(v) = cache::get::<Vec<Item>>(&key) {
        return Ok(v);
    }
    let mut body = json!({
        "collections": [collection],
        "bbox": bbox,
        "datetime": format!("{from}T00:00:00Z/{to}T23:59:59Z"),
        "limit": 200,
        // only what we use (~1 KB instead of ~17 KB per item); some catalogues, Earth Search among them,
        // ignore it and send everything, which is why search results are cached
        "fields": {
            "include": ["id", "bbox", "properties.datetime", "properties.eo:cloud_cover",
                        "assets.green.href", "assets.red.href", "assets.nir.href", "assets.scl.href"],
            "exclude": ["links", "geometry"]
        },
    });
    if let Some(c) = max_cloud {
        body["query"] = json!({ "eo:cloud_cover": { "lt": c } });
    }
    let mut out = Vec::new();
    let mut page = http::fetcher().post_json(endpoint, &body)?;
    for _ in 0..50 {
        for f in page["features"].as_array().into_iter().flatten() {
            if let Some(it) = item(f) {
                out.push(it);
            }
        }
        // the next page: a link with its own body (POST) or href (GET)
        let Some(next) = page["links"].as_array().and_then(|l| l.iter().find(|x| x["rel"] == "next")).cloned() else { break };
        page = match (next["method"].as_str(), next.get("body")) {
            (Some("POST"), Some(b)) => {
                let mut nb = body.clone();
                if let (Some(o), Some(extra)) = (nb.as_object_mut(), b.as_object()) {
                    for (k, v) in extra {
                        o.insert(k.clone(), v.clone());
                    }
                }
                http::fetcher().post_json(next["href"].as_str().unwrap_or(endpoint), &nb)?
            }
            _ => http::fetcher().get_json(next["href"].as_str().unwrap_or(endpoint))?,
        };
    }
    out.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
    out.dedup_by(|a, b| a.id == b.id);
    cache::put(&key, &out);
    Ok(out)
}

fn item(f: &Value) -> Option<Item> {
    let dt = f["properties"]["datetime"].as_str()?;
    let date = Date::parse(dt.get(..10)?)?;
    let assets = f["assets"].as_object()?.iter().filter_map(|(k, v)| Some((k.clone(), v["href"].as_str()?.to_string()))).collect();
    let bbox = f["bbox"].as_array().filter(|b| b.len() == 4).and_then(|b| Some([b[0].as_f64()?, b[1].as_f64()?, b[2].as_f64()?, b[3].as_f64()?]));
    Some(Item { id: f["id"].as_str()?.to_string(), date, cloud: f["properties"]["eo:cloud_cover"].as_f64(), assets, bbox })
}
