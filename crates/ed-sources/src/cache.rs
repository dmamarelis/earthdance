//! A small on-disk cache (`$XDG_CACHE_HOME/earthdance`, else `~/.cache/earthdance`).
//!
//! Network results are written here; when the network is down the newest copy is used instead, which is
//! what the legacy app's `data/weather.json` snapshot did by hand.

use std::fs;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::Serialize;

pub fn dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("earthdance")
}

fn path(key: &str) -> PathBuf {
    let safe: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    dir().join(format!("{safe}.json"))
}

pub fn put<T: Serialize>(key: &str, v: &T) {
    let p = path(key);
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(s) = serde_json::to_string(v) {
        let tmp = p.with_extension("tmp");
        if fs::write(&tmp, s).is_ok() {
            let _ = fs::rename(tmp, p);
        }
    }
}

pub fn get<T: DeserializeOwned>(key: &str) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(path(key)).ok()?).ok()
}

/// The newest cached entry whose key starts with `prefix` (by file modification time).
pub fn newest<T: DeserializeOwned>(prefix: &str) -> Option<T> {
    let safe_prefix = path(prefix);
    let stem = safe_prefix.file_stem()?.to_string_lossy().into_owned();
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in fs::read_dir(dir()).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(&stem) && name.ends_with(".json") {
            let t = e.metadata().and_then(|m| m.modified()).ok()?;
            if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
                best = Some((t, e.path()));
            }
        }
    }
    serde_json::from_str(&fs::read_to_string(best?.1).ok()?).ok()
}
