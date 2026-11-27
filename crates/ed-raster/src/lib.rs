//! Raster processing for EarthDance: the legacy Python/GDAL/scipy pipeline (`tools/build_real_data.py`),
//! in pure Rust.
//!
//! * [`geotiff`] — decode the single-band, uncompressed GeoTIFF inputs.
//! * [`utm`] — WGS 84 UTM forward/inverse (Krüger series).
//! * [`warp`] — bilinear reprojection onto the lat/lon site grid, as GDAL's warper.
//! * [`filters`] — the `scipy.ndimage` filters used (median, Gaussian, dilation, labelling).
//! * [`render`] — legend colour ramps, soft edges, optical pseudo-colour.
//! * [`koroneia`] — the whole "Real data" build as a pure function ([`koroneia::build`]).
//!
//! Bytes in, structs out: the only file-system code is [`koroneia::read_inputs_from_dirs`].

pub mod cog;
pub mod filters;
pub mod geotiff;
pub mod koroneia;
pub mod render;
pub mod site;
pub mod utm;
pub mod warp;

pub use geotiff::{SourceRaster, decode};
pub use koroneia::{Build, Inputs, Legends, SiteConfig, block_values, build, read_inputs_from_dirs};
pub use render::{Stop, colorize};
pub use warp::{UnsupportedCrs, warp_bilinear};
