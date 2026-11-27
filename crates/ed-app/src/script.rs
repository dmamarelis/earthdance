//! Scripted UI testing: `EARTHDANCE_SCRIPT=steps.txt EARTHDANCE_SCRIPT_OUT=dir` drives the app with real
//! input events (they go through egui exactly like the mouse and keyboard) and screenshots it, so every
//! button, menu and panel can be pressed and looked at without a person at the controls.
//!
//! One step per line (`#` starts a comment); coordinates are logical points, and a negative X or Y counts
//! from the right or bottom edge (the window manager decides the window size; the UI is anchored to the
//! edges, so `-26 19` is the language button at any size):
//!
//!   click X Y          press and release the left button at (X, Y)
//!   rclick X Y         the same with the right button
//!   move X Y           hover at (X, Y)
//!   drag X0 Y0 X1 Y1   press at (X0, Y0), move to (X1, Y1) in steps, release
//!   scroll X Y DY      mouse wheel at (X, Y), DY points (positive = content moves down, i.e. scroll up)
//!   key NAME           press and release a key (egui names: Escape, ArrowRight, R, ...)
//!   wait N             let N frames pass
//!   shot NAME          wait for the map tiles (up to ~5 s), then save DIR/NAME.png
//!
//! The script starts once the data set is shown; after each input it lets a few frames pass so menus and
//! animations can open. The app closes after the last step. Like screenshot runs, scripted runs leave the
//! saved preferences alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Event, Modifiers, Pos2, PointerButton, Vec2};

use crate::app::App;

enum Step {
    Click(Pos2, PointerButton),
    Move(Pos2),
    Drag(Pos2, Pos2),
    Scroll(Pos2, f32),
    Key(egui::Key),
    Wait(u32),
    Shot(String),
}

#[derive(Default)]
pub struct Script {
    steps: Vec<Step>,
    out: String,
    next: usize,
    frames: u32,
    wait: u32,
    /// Events for the next frames (frame offset, event).
    queue: Vec<(u32, Event)>,
    shot: Option<(Arc<AtomicBool>, u32)>,
    tiles_wait: u32,
    pub active: bool,
}

fn parse(line: &str) -> Result<Option<Step>, String> {
    let line = line.split('#').next().unwrap_or("").trim();
    if line.is_empty() {
        return Ok(None);
    }
    let w: Vec<&str> = line.split_whitespace().collect();
    let f = |i: usize| w.get(i).and_then(|v| v.parse::<f32>().ok()).ok_or_else(|| format!("bad number in `{line}`"));
    Ok(Some(match w[0] {
        "click" => Step::Click(Pos2::new(f(1)?, f(2)?), PointerButton::Primary),
        "rclick" => Step::Click(Pos2::new(f(1)?, f(2)?), PointerButton::Secondary),
        "move" => Step::Move(Pos2::new(f(1)?, f(2)?)),
        "drag" => Step::Drag(Pos2::new(f(1)?, f(2)?), Pos2::new(f(3)?, f(4)?)),
        "scroll" => Step::Scroll(Pos2::new(f(1)?, f(2)?), f(3)?),
        "key" => Step::Key(w.get(1).and_then(|k| egui::Key::from_name(k)).ok_or_else(|| format!("unknown key in `{line}`"))?),
        "wait" => Step::Wait(f(1)? as u32),
        "shot" => Step::Shot(w.get(1).ok_or("shot needs a name")?.to_string()),
        other => return Err(format!("unknown step `{other}`")),
    }))
}

impl Script {
    pub fn from_env() -> Self {
        let Ok(path) = std::env::var("EARTHDANCE_SCRIPT") else { return Self::default() };
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("EARTHDANCE_SCRIPT {path}: {e}"));
        let steps = text.lines().filter_map(|l| parse(l).unwrap_or_else(|e| panic!("{path}: {e}"))).collect();
        let out = std::env::var("EARTHDANCE_SCRIPT_OUT").unwrap_or_else(|_| ".".into());
        let _ = std::fs::create_dir_all(&out);
        Self { steps, out, active: true, ..Self::default() }
    }

    /// Hands the events due this frame to egui (called from `raw_input_hook`).
    pub fn inject(&mut self, raw: &mut egui::RawInput) {
        let mut later = Vec::new();
        for (d, e) in self.queue.drain(..) {
            if d == 0 {
                raw.events.push(e);
            } else {
                later.push((d - 1, e));
            }
        }
        self.queue = later;
    }
}

fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }
}

impl App {
    pub fn script_tick(&mut self, ctx: &egui::Context) {
        if !self.script.active {
            return;
        }
        ctx.request_repaint();
        if self.data.is_none() || self.weather_pending() {
            return;
        }
        let s = &mut self.script;
        s.frames += 1;
        if s.frames < 120 || !s.queue.is_empty() {
            return;
        }
        if s.wait > 0 {
            s.wait -= 1;
            return;
        }
        if let Some((done, _)) = &s.shot {
            if done.load(Ordering::Acquire) {
                s.shot = None;
            } else {
                return;
            }
        }
        let Some(step) = s.steps.get(s.next) else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        // a shot waits for the map tiles first (at most ~300 frames)
        if let Step::Shot(_) = step
            && self.basemaps.loading() > 0
            && s.tiles_wait < 300
        {
            s.tiles_wait += 1;
            return;
        }
        s.tiles_wait = 0;
        s.next += 1;
        let screen = ctx.content_rect();
        let at = |p: &Pos2| Pos2::new(if p.x < 0.0 { screen.right() + p.x } else { p.x }, if p.y < 0.0 { screen.bottom() + p.y } else { p.y });
        let settle = 25;
        match step {
            Step::Click(p, b) => {
                let p = at(p);
                s.queue.push((0, Event::PointerMoved(p)));
                s.queue.push((1, button(p, *b, true)));
                s.queue.push((3, button(p, *b, false)));
                s.wait = settle;
            }
            Step::Move(p) => {
                s.queue.push((0, Event::PointerMoved(at(p))));
                s.wait = settle;
            }
            Step::Drag(a, b) => {
                let (a, b) = (&at(a), &at(b));
                s.queue.push((0, Event::PointerMoved(*a)));
                s.queue.push((1, button(*a, PointerButton::Primary, true)));
                for i in 1..=10u32 {
                    s.queue.push((1 + i, Event::PointerMoved(*a + (*b - *a) * (i as f32 / 10.0))));
                }
                s.queue.push((13, button(*b, PointerButton::Primary, false)));
                s.wait = settle;
            }
            Step::Scroll(p, dy) => {
                s.queue.push((0, Event::PointerMoved(at(p))));
                s.queue.push((1, Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, *dy), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Modifiers::NONE }));
                s.wait = settle;
            }
            Step::Key(k) => {
                for (d, pressed) in [(0, true), (2, false)] {
                    s.queue.push((d, Event::Key { key: *k, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }));
                }
                s.wait = settle;
            }
            Step::Wait(n) => s.wait = *n,
            Step::Shot(name) => {
                let path = format!("{}/{name}.png", s.out);
                let done = Arc::new(AtomicBool::new(false));
                let d2 = done.clone();
                let ctx2 = ctx.clone();
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                    let [w, h] = img.size;
                    let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    match image::RgbaImage::from_raw(w as u32, h as u32, bytes).map(|b| b.save(&path)) {
                        Some(Ok(())) => eprintln!("shot {path}"),
                        other => eprintln!("shot failed {path}: {other:?}"),
                    }
                    d2.store(true, Ordering::Release);
                    ctx2.request_repaint();
                })));
                s.shot = Some((done, 0));
            }
        }
    }
}
