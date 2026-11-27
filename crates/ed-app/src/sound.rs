//! Sonification (port of js/sound.js): a time series becomes a short melody. Higher value = higher note on a
//! pentatonic scale, so it always sounds musical; `bright` (0..1 per step) opens a low-pass filter, e.g. more
//! vegetation = brighter tone. Notes are synthesised here (three oscillators through a one-pole filter with
//! an exponential envelope) and handed to the audio device as sample buffers.

use std::num::NonZero;
use std::time::{Duration, Instant};

const SCALE: [i32; 5] = [0, 2, 4, 7, 9];
const BASE_HZ: f32 = 146.83; // D3
const RATE: u32 = 44_100;
pub const STEP: Duration = Duration::from_millis(340);

#[derive(Default)]
pub struct Player {
    sink: Option<rodio::MixerDeviceSink>,
    notes: Vec<(f32, f32)>,
    next: usize,
    started: Option<Instant>,
}


impl Player {
    pub fn is_playing(&self) -> bool {
        self.started.is_some()
    }

    pub fn stop(&mut self) {
        self.started = None;
        self.notes.clear();
    }

    pub fn play(&mut self, values: &[f64], bright: &[f64]) {
        self.stop();
        if values.is_empty() {
            return;
        }
        if self.sink.is_none() {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(s) => self.sink = Some(s),
                Err(e) => eprintln!("no audio device: {e}"),
            }
        }
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let span = values.iter().copied().fold(f64::NEG_INFINITY, f64::max) - min;
        let span = if span == 0.0 { 1.0 } else { span };
        self.notes = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let degree = (((v - min) / span) * 12.0).round() as i32;
                let semis = 12 * degree.div_euclid(5) + SCALE[degree.rem_euclid(5) as usize];
                (BASE_HZ * 2f32.powf(semis as f32 / 12.0), bright.get(i).copied().unwrap_or(0.5) as f32)
            })
            .collect();
        self.next = 0;
        self.started = Some(Instant::now());
    }

    /// Call every frame: plays the notes that are due and returns the indices of the steps that started.
    pub fn tick(&mut self) -> Vec<usize> {
        let Some(t0) = self.started else { return Vec::new() };
        let mut out = Vec::new();
        while self.next < self.notes.len() && t0.elapsed() >= STEP * self.next as u32 {
            let (f, b) = self.notes[self.next];
            if let Some(s) = &self.sink {
                s.mixer().add(rodio::buffer::SamplesBuffer::new(NonZero::new(1).expect("1"), NonZero::new(RATE).expect("rate"), note(f, STEP.as_secs_f32() * 2.2, b)));
            }
            out.push(self.next);
            self.next += 1;
        }
        if self.next >= self.notes.len() && t0.elapsed() >= STEP * self.notes.len() as u32 {
            self.stop();
        }
        out
    }
}

fn note(freq: f32, dur: f32, brightness: f32) -> Vec<f32> {
    let n = ((dur + 0.05) * RATE as f32) as usize;
    let cutoff = 500.0 + 3800.0 * brightness.clamp(0.0, 1.0);
    let k = 1.0 - (-2.0 * std::f32::consts::PI * cutoff / RATE as f32).exp();
    let mut y = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let ph = |m: f32| (t * freq * m).fract();
            let tri = 4.0 * (ph(1.0) - 0.5).abs() - 1.0;
            let s = tri + 0.6 * (std::f32::consts::TAU * ph(0.5)).sin() + 0.15 * (std::f32::consts::TAU * ph(2.0)).sin();
            y += k * (s - y);
            // 20 ms exponential attack to 0.22, then exponential decay to silence at `dur`
            let env = if t < 0.02 { 0.0001 * (0.22f32 / 0.0001).powf(t / 0.02) } else { 0.22 * (0.0001f32 / 0.22).powf(((t - 0.02) / (dur - 0.02)).min(1.0)) };
            y * env * 0.6
        })
        .collect()
}
