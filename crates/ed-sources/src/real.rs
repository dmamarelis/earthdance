//! Real data set (filled in once ed-raster lands).

use std::path::Path;

use ed_core::model::{Dataset, WeatherSet};

/// Builds the real data set. Placeholder until the raster pipeline is wired in.
pub fn load(_root: &Path, _progress: &dyn Fn(&str)) -> crate::Result<(Dataset, Option<WeatherSet>)> {
    Err(crate::Error::Data("the real data pipeline is not wired in yet".into()))
}
