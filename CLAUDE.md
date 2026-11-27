# CLAUDE.md

Instructions for Claude Code sessions on EarthDance. The repo facts (layout, build/test/lint commands,
parity rules, conventions) live in AGENTS.md and are imported here:

@AGENTS.md

Read README.md and docs/rust-rewrite.md before changing anything; the second one records why things are
the way they are (evaluations, rejected alternatives, measured timings).

## How this project is run

* **Claude writes the code; the user directs and reviews.** On this repository that overrides any
  general "teach, don't write code" preference: when asked for a change, make it, end to end.
* Work on the `rust-rewrite` branch. **Commit and push after every finished change** (small commits,
  `git push` each time). End every commit message with
  `Co-Authored-By: <the model you are> <noreply@anthropic.com>`. Never commit `target/`.
* **Docs go in the same commit as the change**: README.md (user-facing), docs/rust-rewrite.md (design and
  numbers), AGENTS.md (if workflow or layout changed). When counts change (lines of code, tests, speed),
  regenerate docs/rust-rewrite-team-brief.pdf (one page, ~300 words, little maths, before vs after).
* Before committing: `cargo clippy --workspace --all-targets -- -D warnings` must be clean, and
  `cargo test --workspace --release` must pass when core, raster or sources code changed.
* Keep the user posted with one short line while working on long tasks. When they send a message mid-task,
  answer it first, then continue.
* Decisions that are the user's (data sources, licence, anything outward-facing, dropping a feature) are
  asked, not assumed. Everything else: pick the sensible option, do it, say what you chose.

## Check visual changes by looking at them

The user judges the app by how it looks, and has caught every visual bug that was only "built", not seen.
For any UI or map change:

1. `cargo build --release -p ed-app`
2. Take screenshots with the hook (variables at the top of `crates/ed-app/src/shot.rs`), for example
   `EARTHDANCE_SHOT=/path/out.png EARTHDANCE_SHOT_SENSOR=radar EARTHDANCE_SHOT_ZOOM=-2 EARTHDANCE_SHOT_LANG=en target/release/earthdance --demo`
   (`--demo` or `--real`; always pass one, or the run uses the default source). Use a scratch directory
   for the PNGs, not the repo.
3. **Read the PNGs** and check several zoom levels (`EARTHDANCE_SHOT_ZOOM`, e.g. -4, -2, 0, 1.5), both
   data sets, and both languages when text changed. Look for repetition, seams, rectangles, overlapping
   labels, unreadable text, legends that do not match what is shown.
4. Only then commit, and say what you looked at.

To show the user the app: `setsid nohup target/release/earthdance --demo >/dev/null 2>&1 &`
(stop an older instance first with `pkill -x earthdance`).

## What the user cares about (learned the hard way)

* **Real, live data**: fetched and processed by the app itself, not pre-baked image or JSON files.
* **Honesty over polish**: estimates are labelled as estimates, demo data says "Demo data", docs never
  claim more than was tested (e.g. live builds are verified at Koronia only until other lakes are run),
  and a test is never loosened to make it pass. If something is not done or failed, say so plainly.
* **A Google-Earth look**: imagery basemap, layers that blend in, no square pictures, no visible tiling
  or repeating patterns (procedural noise, not tiled textures), muted colours, small markers and labels.
* **Speed and no jank**: the UI thread never waits on the network; measure before and after optimising
  and put the numbers in docs/rust-rewrite.md.
* Choices already made, do not reopen without a new reason: no Tokio (blocking pool + rayon, reasons in
  the docs); the "30 % real / 70 % synthetic" data mix was declined (synthetic data stays a separate,
  labelled demo); barycentric Lagrange is not used on pass dates; no licence is claimed.

## Open items (check docs/rust-rewrite.md for the current state)

* Five-lake live check (Bled, Mývatn, Naivasha, Taupō, Mead) via the `live_place` example: postponed by
  the user; run it only when asked.
* Whether to tone down the pink/green false-colour Koronia Sentinel-2 picture for display (the parity
  tests pin the pixels, so it would be display-only): the user's call.
* Radar for arbitrary places (Sentinel-1 RTC, NISAR with an Earthdata token) is not wired in.
* Sentinel-2 (26–31 km²) and NISAR (~9 km²) disagree about Koronia's area: a question for the team.

## Shell pitfalls seen here

* `pkill -f <pattern>` can match the shell running it and kill your own command (exit 144). Use
  `pkill -x <name>` or kill by PID.
* Release builds and the full test run take minutes: give them long timeouts, run slow network jobs in
  the background.
* The disk cache in `~/.cache/earthdance` holds downloads and the user's saved preferences; do not delete
  it.
