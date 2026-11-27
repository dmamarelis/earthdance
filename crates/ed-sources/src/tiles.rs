//! Basemap tiles (XYZ / Web Mercator, 256 px), loaded in the background and cached on disk forever.
//!
//! The map asks for the tiles it needs every frame with [`Tiles::get`]; it never waits. Missing tiles are
//! queued (newest request first: what is on screen now beats what was on screen a second ago), fetched by
//! a few worker threads over the shared connection pool, decoded off the UI thread, and the UI is told to
//! repaint. Until a tile arrives the map draws a scaled-up ancestor, so the view sharpens instead of
//! flashing.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

use crate::http;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Basemap {
    /// Esri World Imagery (keyless; attribution required).
    EsriImagery,
    /// Sentinel-2 cloudless mosaic by EOX (CC BY-NC-SA 4.0).
    S2Cloudless,
    /// Place names and borders, transparent, drawn on top.
    EsriLabels,
}

impl Basemap {
    pub fn url(self, z: u8, x: u32, y: u32) -> String {
        match self {
            Basemap::EsriImagery => format!("https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}"),
            Basemap::S2Cloudless => format!("https://tiles.maps.eox.at/wmts/1.0.0/s2cloudless-2023_3857/default/g/{z}/{y}/{x}.jpg"),
            Basemap::EsriLabels => format!("https://server.arcgisonline.com/ArcGIS/rest/services/Reference/World_Boundaries_and_Places/MapServer/tile/{z}/{y}/{x}"),
        }
    }
    pub fn max_zoom(self) -> u8 {
        match self {
            Basemap::EsriImagery => 19,
            Basemap::S2Cloudless => 15,
            Basemap::EsriLabels => 16,
        }
    }
    pub fn attribution(self) -> &'static str {
        match self {
            Basemap::EsriImagery => "Imagery © Esri, Maxar, Earthstar Geographics",
            Basemap::S2Cloudless => "Sentinel-2 cloudless – s2maps.eu by EOX IT Services GmbH (contains modified Copernicus Sentinel data 2023)",
            Basemap::EsriLabels => "Labels © Esri",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub map: Basemap,
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileKey {
    pub fn parent(self) -> Option<TileKey> {
        (self.z > 0).then(|| TileKey { z: self.z - 1, x: self.x / 2, y: self.y / 2, ..self })
    }
}

/// A decoded tile: 8-bit RGBA, row-major.
pub struct TileImage {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

enum State {
    Queued,
    Ready(Arc<TileImage>),
    Failed,
}

struct Entry {
    state: State,
    /// Frame in which the map last asked for this tile, and how urgently (0 = on screen).
    seen: u64,
    prio: u8,
}

struct Inner {
    state: HashMap<TileKey, Entry>,
    queue: Vec<TileKey>,
    frame: u64,
}

pub struct Tiles {
    inner: Mutex<Inner>,
    wake: Condvar,
    repaint: Box<dyn Fn() + Send + Sync>,
}

/// A queued tile not asked for in this many frames has scrolled or zoomed away: it is dropped, not fetched.
const STALE_FRAMES: u64 = 30;
const MAX_QUEUE: usize = 400;

impl Tiles {
    /// Starts `workers` background fetchers. `repaint` is called when a tile becomes ready.
    pub fn new(workers: usize, repaint: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        let t = Arc::new(Self { inner: Mutex::new(Inner { state: HashMap::new(), queue: Vec::new(), frame: 0 }), wake: Condvar::new(), repaint: Box::new(repaint) });
        for _ in 0..workers {
            let t = t.clone();
            std::thread::spawn(move || t.work());
        }
        t
    }

    /// Call once per frame: requests older than [`STALE_FRAMES`] are then dropped.
    pub fn tick(&self) {
        self.inner.lock().expect("tiles").frame += 1;
    }

    /// The tile if it is ready; otherwise it is queued with priority `prio` (0 = on screen now, higher =
    /// prefetch) and `None` is returned at once.
    pub fn get(&self, key: TileKey, prio: u8) -> Option<Arc<TileImage>> {
        let mut g = self.inner.lock().expect("tiles");
        let frame = g.frame;
        if let Some(e) = g.state.get_mut(&key) {
            e.seen = frame;
            e.prio = e.prio.min(prio);
            return match &e.state {
                State::Ready(img) => Some(img.clone()),
                _ => None,
            };
        }
        g.state.insert(key, Entry { state: State::Queued, seen: frame, prio });
        g.queue.push(key);
        if g.queue.len() > MAX_QUEUE {
            Self::prune(&mut g);
        }
        drop(g);
        self.wake.notify_one();
        None
    }

    fn prune(g: &mut Inner) {
        let frame = g.frame;
        let state = &mut g.state;
        g.queue.retain(|k| {
            let keep = state.get(k).is_some_and(|e| frame.saturating_sub(e.seen) <= STALE_FRAMES);
            if !keep {
                state.remove(k);
            }
            keep
        });
    }

    /// Ready without queuing (for looking up stand-ins while waiting).
    pub fn peek(&self, key: TileKey) -> Option<Arc<TileImage>> {
        match self.inner.lock().expect("tiles").state.get(&key).map(|e| &e.state) {
            Some(State::Ready(img)) => Some(img.clone()),
            _ => None,
        }
    }

    pub fn pending(&self) -> usize {
        self.inner.lock().expect("tiles").queue.len()
    }

    fn work(&self) {
        loop {
            let key = {
                let mut g = self.inner.lock().expect("tiles");
                loop {
                    Self::prune(&mut g);
                    // most urgent first, then the most recently asked for
                    let best = g.queue.iter().enumerate().min_by_key(|(_, k)| g.state.get(k).map_or((u8::MAX, 0), |e| (e.prio, u64::MAX - e.seen))).map(|(i, _)| i);
                    if let Some(i) = best {
                        break g.queue.swap_remove(i);
                    }
                    g = self.wake.wait(g).expect("tiles");
                }
            };
            let img = http::fetcher().get_cached(&key.map.url(key.z, key.x, key.y)).ok().and_then(|b| image::load_from_memory(&b).ok()).map(|i| {
                let i = i.to_rgba8();
                Arc::new(TileImage { w: i.width() as usize, h: i.height() as usize, rgba: i.into_raw() })
            });
            let mut g = self.inner.lock().expect("tiles");
            let frame = g.frame;
            let e = g.state.entry(key).or_insert(Entry { state: State::Queued, seen: frame, prio: 9 });
            e.state = img.map_or(State::Failed, State::Ready);
            drop(g);
            (self.repaint)();
        }
    }
}
