//! EarthDance core: the domain model and every computation that turns numbers into conclusions.
//! Pure: no IO, no clock. "Today" is always passed in.

pub mod ask;
pub mod date;
pub mod forecast;
pub mod grid;
pub mod insights;
pub mod interp;
pub mod model;
pub mod stats;
pub mod status;
pub mod text;
pub mod weathergen;

pub use date::Date;
pub use grid::{Bounds, Grid, Rgba};
pub use insights::Level;
pub use text::{Lang, Text};
