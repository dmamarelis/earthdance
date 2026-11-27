# AGENTS.md

Notes for AI coding agents (and new contributors) working on this repository. Read README.md first for
what the app does; docs/rust-rewrite.md has the design and the evaluations.

## Layout

The product is the Rust workspace (`Cargo.toml`, `crates/`). The original web app (`index.html`, `js/`,
`css/`, Python in `tools/`) is kept as the **reference implementation**. Do not change it: the Rust code is
tested against what it produces.

| Path | What it is |
|---|---|
| `crates/ed-core` | Domain model, rules → findings, lake water-balance forecast and fitting, Markov weather generator, gap filling, "Ask". **Pure: no IO, no threads, "today" is passed in.** |
| `crates/ed-raster` | GeoTIFF / COG reading, UTM reprojection (GDAL-exact), filters, water detection, colouring. Pure compute. |
| `crates/ed-sources` | All network and disk IO: HTTP fetcher with pool, retry and byte cache (`http.rs`), Open-Meteo weather, STAC search, live Sentinel-2 builds (`live.rs`), basemap tiles, demo and Koronia data sets. |
| `crates/ed-app` | The eframe/egui desktop app. |
| `assets/` | Content (layers, legends, story, texts) loaded at runtime: data, not code. |
| `oracle/` | Node scripts that run the legacy JS to write golden fixtures (`gen.mjs`) and the speed benchmark (`bench.mjs`). |
| `tests/fixtures/` | Golden outputs of the legacy app. Regenerate with `node oracle/gen.mjs`, never edit by hand. |
| `Koroneia_NISAR/`, `Koroneia_Sentinel/` | The real Lake Koronia rasters (inputs of the parity tests). |

## Build, test, lint

```bash
cargo build --release -p ed-app
cargo test --workspace --release          # all tests must pass; some tests are slow in debug builds
cargo clippy --workspace --all-targets -- -D warnings   # must be clean
```

Linux needs the ALSA headers (`alsa-lib-devel` / `libasound2-dev`) for sound.

Useful examples:

```bash
cargo run --release -p ed-sources --example live_place -- "Lake Bled" 14.0938 46.3639 1.5  # live build + timings
cargo run --release -p ed-sources --example find_place -- "Λίμνη Βόλβη"                    # geocoder
cargo run --release -p ed-core --example bench_status                                      # vs node oracle/bench.mjs
```

## Rules that keep the app correct

* **Golden parity.** `crates/ed-core/tests/golden.rs`, `crates/ed-raster/tests/parity.rs` and
  `crates/ed-sources/tests/real.rs` compare against the legacy outputs: exact strings in Greek and English,
  arrays to 1e-9, pixels exactly. If one fails, fix the Rust code; do not loosen the test or edit fixtures.
  New behaviour that departs from the legacy app goes behind a config switch (see `FitConfig`, and
  `legacy_cfg` in `crates/ed-sources/tests/real.rs`) with the legacy path kept for the tests.
* **JS-exact formatting.** Use `text::to_fixed` and `text::js_round` instead of `format!("{:.n}")` and
  `f64::round` for anything shown to users (JS rounds ties differently).
* **Measured vs estimated.** Values estimated between satellite passes are always drawn and labelled as
  estimates and never mixed into the measurements. A forecast is shown only when the fit passes the quality
  gate (`forecast::fit_quality`).
* **Demo data is labelled.** Anything synthetic must say "Demo data" on screen.
* **The UI thread never waits on the network.** Fetching happens on worker threads (`http::parallel`,
  `tiles::Tiles`); the UI polls.

## Checking the UI

The app can render itself to a PNG without user input: set `EARTHDANCE_SHOT=out.png` plus the
`EARTHDANCE_SHOT_*` variables listed at the top of `crates/ed-app/src/shot.rs` (mode, sensor, frame,
language, zoom, …). Use it to look at visual changes before committing them. Screenshot runs start from the
default settings and never read or write the user's saved preferences.

Two more tools for checking the whole UI:

* `EARTHDANCE_TOUR=dir target/release/earthdance --demo` (or `--real`) walks the app through every view
  (modes, layers, points, areas, tabs, story chapters, sheets, closed panels, both languages), saves a
  screenshot of each, and prints `OVERFLOW <state> <panel> +N px` for any panel whose contents are wider
  than the panel. It must end with `0 problem(s)` (`crates/ed-app/src/tour.rs`).
* `EARTHDANCE_SCRIPT=steps.txt EARTHDANCE_SCRIPT_OUT=dir` drives the app with real clicks, keys, drags
  and scrolls and takes screenshots, for menus and popups (`crates/ed-app/src/script.rs` lists the
  steps; negative coordinates count from the right/bottom edge, since the window manager picks the size).

## Conventions

* Work on the `rust-rewrite` branch and push often. Never commit `target/`.
* **Update the docs in the same commit as each change:** README.md, docs/rust-rewrite.md, and the numbers
  in docs/rust-rewrite-team-brief.pdf when they change.
* Commit messages: a short summary line, then what changed and why. Commits written by an AI agent end
  with a `Co-Authored-By:` line naming it.
* No licence has been chosen for the project. Do not add one.
* Network requests go through `ed_sources::http::fetcher()` (shared pool, retries, disk cache in
  `~/.cache/earthdance`). Do not add user data or e-mail addresses to requests.
