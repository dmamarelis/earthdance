# EarthDance in Rust

A native desktop rewrite of EarthDance: the legacy browser app and its Python scripts, now one Rust
workspace. Nothing is pre-baked any more: the real data set is processed from the acquisitions when the
app starts, and the lake model is fitted on fresh ERA5 weather.

## Run

```bash
cargo run --release -p ed-app            # real data: NISAR + Sentinel-2 → water, change, lake forecast
cargo run --release -p ed-app -- --demo  # the synthetic demo set (HV, subsidence, points, Story)
cargo test --workspace --release         # unit, golden and end-to-end tests
```

Keys: ← → step through acquisitions (or Story chapters), `R` toggles optical / radar, `Esc` closes sheets.

## Crates

| Crate | What it does | Replaces |
|---|---|---|
| `ed-core` | Domain model, statistics (incl. a port of scipy's TRF least squares and Brent), JS-exact number/date formatting (el/en), the rules engine, the lake forecast (fit + ensemble run share one `step`), Ask, the Status computations. Pure: no IO, "today" is passed in. | `js/insights.js`, `js/forecast.js`, `js/ask.js`, half of `js/status.js`, `tools/fit_forecast.py` |
| `ed-raster` | GeoTIFF decoding, WGS84 UTM, a GDAL-exact bilinear warp, scipy-compatible filters, water masks, colouring, the whole Koronia pipeline. | `tools/build_real_data.py`, `tools/colormap.py` |
| `ed-sources` | Open-Meteo (history + forecast, disk-cached: the cache is the offline fallback), the `AcquisitionSource` trait, the real and demo data-set builders. | `js/weather.js`, `tools/fetch_weather.py` |
| `ed-app` | The egui app: map canvas, Explore / Story / Status, charts, sonification, report. | `index.html`, `js/app.js`, the rest of `js/status.js`, `js/sound.js`, `css/` |

## How the parts connect

```
AcquisitionSource ──► ed-raster::koroneia::build ──► Dataset (layers, optical, regions, reservoir)
                                                        │
Open-Meteo ERA5 ─────────► ed-core::forecast::fit ──────┤ (ForecastModel)
                                                        ▼
Open-Meteo daily ──► WeatherSet ──► ed-core::status::compute ──► Status (findings, brief) ──► ed-app
```

## How we know it is right

`oracle/gen.mjs` ran the legacy JS on fixed inputs (today = 2026-11-14, the saved weather) and wrote
`tests/fixtures/`. The Rust code is checked against it:

* every finding, alert line and Ask answer, both languages: identical strings;
* forecast ensembles (6 what-ifs × with / without weather): equal to 1e-9;
* the refit of the lake model: parameters to ~9 significant digits, back-test identical;
* the raster pipeline against the committed `data/real/`: frame lists, water km², HH means, reservoir,
  outline and all 20 hover grids identical;
* end to end, TIFFs → findings: identical to the legacy app's (`crates/ed-sources/tests/real.rs`).

`EARTHDANCE_SHOT=out.png` makes the app open a view (see `crates/ed-app/src/shot.rs`), screenshot it and
quit, which is how the UI was checked.

## Still open

| | Decision | Now |
|---|---|---|
| D1 | Where acquisitions come from (ASF / Earthdata for NISAR GCOV, LP DAAC for HLS, auth) | `LocalDirs` reads `Koroneia_NISAR/` and `Koroneia_Sentinel/` |
| D2 | Map renderer and basemap tiles | Web-Mercator canvas with graticule, no basemap |
| D3 | Where live processing runs | in-process, background thread at start-up (0.6 s) |
| D4 | Report format | self-contained HTML opened in the browser to print |

The demo set is still read from `data/` (it is synthetic and labelled as such everywhere).
