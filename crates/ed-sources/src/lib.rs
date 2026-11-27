//! EarthDance data sources: everything that touches the network or the disk.
//!
//! * [`weather`]: Open-Meteo (reanalysis history + 7-day forecast), with a disk cache that doubles as the
//!   offline fallback.
//! * [`demo`]: the synthetic demo data set.
//! * [`real`]: the real data set, processed live from the acquisitions (NISAR + Sentinel-2) by `ed-raster`.
//!   Where the acquisitions come from is the [`AcquisitionSource`] trait; the only implementation so far
//!   reads GeoTIFFs from a folder (decision D1 picks the live archive).

pub mod cache;
pub mod http;
pub mod live;
pub mod stac;
pub mod tiles;
pub mod demo;
pub mod real;
pub mod weather;

use std::path::PathBuf;

use ed_core::Date;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Http(String),
    Json(serde_json::Error),
    Data(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Http(e) => write!(f, "network: {e}"),
            Error::Json(e) => write!(f, "json: {e}"),
            Error::Data(e) => write!(f, "data: {e}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}
impl From<ureq::Error> for Error {
    fn from(e: ureq::Error) -> Self {
        Error::Http(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Today in the local time zone (the app's notion of "today").
pub fn today_local() -> Date {
    let d = jiff::Zoned::now().date();
    Date::ymd(i32::from(d.year()), d.month() as u32, d.day() as u32)
}

/// Where the repository's data lives: `$EARTHDANCE_DATA`, else the folder holding `data/` next to the
/// executable or above the working directory, else the source tree (development).
pub fn data_root() -> PathBuf {
    if let Some(p) = std::env::var_os("EARTHDANCE_DATA") {
        return PathBuf::from(p);
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        candidates.extend(exe.ancestors().skip(1).map(PathBuf::from));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.extend(cwd.ancestors().map(PathBuf::from));
    }
    candidates.push(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")));
    candidates.into_iter().find(|p| p.join("data/manifest.json").exists()).unwrap_or_else(|| PathBuf::from("."))
}
