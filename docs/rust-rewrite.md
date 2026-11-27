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
quit, which is how the UI was checked. Screenshot runs use the default settings and leave the user's saved
preferences alone.

## The weather behind the lake forecast

Beyond the 7-day forecast (Open-Meteo), the lake model needs plausible future weather. The legacy app
replayed 2015–2025. Now a **stochastic weather generator built from two Markov chains** supplies 150
simulated years (× 3 rain coefficients = 450 runs) (`crates/ed-core/src/weathergen.rs`):

* a **monthly regime chain**: each month is dry / normal / wet (terciles of its rain total), with
  month-to-month transition probabilities, so dry months can follow dry months (a dry winter);
* a **daily occurrence chain** per (month, regime): `P(wet | dry yesterday)`, `P(wet | wet yesterday)`;
* rain amounts from a **gamma** distribution (maximum likelihood, Thom), ET0 normal per wet/dry state with
  an AR(1) memory;
* fitted to the **last 30 years of ERA5** at the lake (the climate is drifting: since 2015 July is drier and
  ET0 higher than in 1950–2014).

It was chosen on held-out data, not on looks
(`cargo run --release -p ed-sources --example weathergen_eval`). For each hydrological year 2005–2026,
each method is fitted only on what came before and forecasts winter rain (the refill) and the
April–September water deficit (the drain):

| method | CRPS winter | CRPS deficit | in 10–90 % band, winter | deficit |
|---|---|---|---|---|
| replay of the last 11 years (legacy) | 50.2 | 58.6 | 73 % | 55 % |
| daily chain, all years | 48.8 | 59.1 | 64 % | 64 % |
| daily + regime chains, all years | 48.1 | 57.0 | 95 % | 86 % |
| daily chain, last 30 years | 49.9 | 55.4 | 55 % | 64 % |
| **daily + regime chains, last 30 years** | 48.9 | **54.4** | 91 % | **82 %** |

A daily chain alone is no better than replay: it underestimates year-to-year variance (simulated
winter-rain s.d. 67 mm vs 81 mm observed), so its bands are too narrow. The regime chain fixes that and
slightly overshoots (99 mm), which makes the winter band conservative. The chosen variant forecasts the
summer deficit 7 % better than replay and is well calibrated for it (82 % for an ideal 80 %). With 22
held-out years, read these as modest, consistent gains rather than a large improvement.

What the generator does not fix: the lake model's winter refill rate is still unidentified (no winter
radar pass yet). Lake areas from the Sentinel-2 archive back to 2017 would constrain it; that needs D1.

## Filling the gaps between passes

Passes come every 5–45 days. What is drawn between them is an estimate and is always shown as such
(dotted, labelled "estimated between passes", switchable in Show ▾). Leave-one-out on the real lake series
(drop each interior pass, estimate it from the rest; `crates/ed-core/tests/gaps.rs`):

| method | RMSE (km²) |
|---|---|
| **model bridge**: the water balance run on the real weather between two passes, nudged to land on the next one | **1.91** |
| PCHIP (monotone cubic, never overshoots) | 2.23 |
| Floater–Hormann barycentric rational, d ≤ 2 | 2.3–2.4 |
| Floater–Hormann, d = 3 | 10.7 |
| barycentric Lagrange (one polynomial through all passes) | 68 900 |

The barycentric form makes Lagrange *stable to evaluate*; it does not stop the degree-13 polynomial through
irregular dates from oscillating (it predicts 236 150 km² for a 27 km² lake). The same test on Runge's
function with 21 equispaced nodes: max error Lagrange 59.8, Floater–Hormann 0.0028, PCHIP 0.013.

So: the lake uses the model bridge (PCHIP where there is no weather), point series use Floater–Hormann
with d = 2. Synthetic data is never mixed into the real set: the demo set stays separate and labelled.

## Anywhere on Earth (live Sentinel-2)

`cargo run --release -p ed-app -- --place "Lake Koronia,23.145,40.682,8"`, or **Source ▾ → Anywhere…**
(search a place by name, or take the map centre).

* **Data**: Sentinel-2 L2A from the public AWS archive (Earth Search STAC, Cloud-Optimized GeoTIFFs, no
  account). Radar anywhere (Sentinel-1 RTC on Planetary Computer, NISAR with an Earthdata token) is next.
* **Fetching**: one 16 KB header per band, then only the tiles covering the area at a suitable overview;
  the cloud band first (cloudy dates cost one band); one MGRS tile when it covers the area; one date per
  8 days; 16 pooled connections with jittered retries; every byte range cached on disk forever; the scene
  list shown from the last search while a fresh search runs.
* **Any lake**: search by name in any script (OpenStreetMap Nominatim for water bodies, with their extent;
  GeoNames via Open-Meteo for towns). A lake's extent sets the area (up to 120 km across); without one,
  the area doubles whenever the lake reaches its edge.
* **Water**: SCL mask for cloud, shadow, snow and ice (a frozen lake is not measured), NDWI threshold from
  the data (Otsu), the lake = the water body at (or nearest to) the place asked for, not merely the
  largest one in view (a coast or a bigger neighbour no longer gets picked). No per-site constants.
* **Seasons from the local climate**: the wet season (five months with the highest rain − ET0) and the
  irrigation season (six months with the highest ET0) come from each lake's ERA5 record; for Koronia this
  reproduces Nov–Mar and Apr–Sep exactly, south of the equator it flips. The weather rule's dry-season
  deficit follows the hemisphere. The lake model is fitted only if it passes a quality gate (R² ≥ 0.5, no
  parameter on a bound).
* **Measured on this machine's link (~70 KB/s)**: Lake Koronia, 18 months, 43 dates: first map after
  57 s, complete after 12 min, 39 MB. On a 50 Mbit/s line the first build is a matter of seconds.
* **Reopening a place** (everything from the disk cache) builds in **1.0 s** (first map after 0.1 s), down
  from 5.0 s: the UTM position of every grid pixel is computed once per build instead of once per scene
  and band (nearest-neighbour warp 3.2 → 0.2 s CPU), partial results are analysed after the first batch
  and then every third instead of after every batch (analysis 1.9 → 0.3 s, assembly 1.0 → 0.1 s), and the
  per-scene work runs on all cores (rayon).
* **Status of other lakes**: the build has been run end to end only at Koronia. A five-lake check (Bled,
  Mývatn, Naivasha, Taupō, Mead) was started and then stopped before finishing, to be run when there is
  time; until then, results for other lakes are untested.

**For the team to look at**: for Koronia, Sentinel-2 sees ~26–31 km² of water all through 2025–26, while
the NISAR radar (legacy method, HH ≤ −18 dB) saw the lake fall to ~9 km² in September 2026. Radar counts
only calm open water as dark; wind, floating or emergent vegetation and very shallow water read as land.
Optical NDWI counts any water-looking surface. The headline alert rests on the radar series, so which one
the Region should trust needs a field check or a comparison of both on the same day.

## Speed: Rust vs the legacy JS

The Status computation (every rule on every area, the lake forecast brief with its 3 ensemble runs, all
Ask answers in both languages), same inputs, identical outputs (golden tests), 200 runs after warm-up,
AMD Ryzen 7 5825U:

| | median | p10–p90 |
|---|---|---|
| legacy JS (Node 22) | 32.4 ms | 29.7–33.7 ms |
| **Rust (release)** | **1.96 ms** | 1.93–2.00 ms |

**≈ 16.5× faster** (`node oracle/bench.mjs` vs `cargo run --release -p ed-core --example bench_status`).

## The map (Google-Earth style)

Satellite basemap tiles (`crates/ed-sources/src/tiles.rs`, `crates/ed-app/src/basemap.rs`):

* **Warm-up**: as soon as a data source is chosen the map moves to that site and the loading screen
  already fetches and shows (dimmed) the basemap, so the map is ready when the data is.
* **Priorities and cancellation**: tiles on screen first, the zoom level above prefetched (it is the
  stand-in when zooming in), the level below prefetched once the screen is complete; requests not repeated
  for 30 frames (views already left) are dropped instead of fetched. Separate worker pools for imagery and
  place names.
* **Stand-ins while loading**: a scaled-up ancestor when zooming in, the sharper children when zooming out,
  so there are no holes. At most 8 tile textures are uploaded per frame, so motion never stutters.
* **Measured** (this machine, warm connection): zoom in by two levels to tiles never fetched before,
  complete in 1.6 s; zoom out by two levels, 0.4 s. Tiles are cached on disk forever.
* **Overlays** sit on the imagery at a chosen opacity (Show ▾ → Layer opacity, default 70 %). Data layers
  that cover a whole rectangle fade out in a soft, irregular oval, so no straight edge or corner reads as a
  pasted square; live places show the dated Sentinel-2 picture only around the lake, tone-mapped to the
  basemap. In the demo set (all made up) the clouds are generated over the whole view from fractal Perlin
  noise at map coordinates (`crates/ed-app/src/clouds.rs`): anchored to the ground, never repeating, finer
  detail fading in as you zoom, a new sky for each date, with the threshold set so the lake's area is as
  cloudy as the date's cloud cover says; thin cloud is translucent and clouds cast a soft shadow. And the radar layer fades into a wash of its own average colour that tints the whole map.

## Threads

The UI runs on its own thread and never waits. The raster pipeline is data-parallel with rayon (0.59 s on
16 threads vs 1.73 s on one); tiles have 13 background workers; live downloads use up to 16 parallel
connections; data loading, weather and place search run on background threads. An async runtime (tokio)
was considered and not adopted: concurrency is at most ~30 requests, the main archives (Esri, AWS S3)
speak HTTP/1.1 only so there is no HTTP/2 multiplexing to gain, and the CPU-heavy work already uses all
cores.

## Still open

| | Decision | Now |
|---|---|---|
| D1 | Where acquisitions come from | Sentinel-2 live from AWS for any place; NISAR / HLS (Earthdata token) and Sentinel-1 RTC still to plug in |
| D2 | Map renderer and basemap tiles | Web-Mercator canvas on a satellite basemap (Esri World Imagery or Sentinel-2 cloudless by EOX, place names on top), tiles streamed and cached; no 3D terrain |
| D3 | Where live processing runs | in-process, background thread at start-up (0.6 s) |
| D4 | Report format | self-contained HTML opened in the browser to print |

The demo set is still read from `data/` (it is synthetic and labelled as such everywhere).
