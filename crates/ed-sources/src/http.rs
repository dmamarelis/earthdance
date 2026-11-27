//! The network layer for acquisitions: one shared connection pool, parallel byte-range requests, retries
//! with jittered backoff, and an on-disk cache for immutable bytes.
//!
//! Measured from here to the AWS archive: a cold connection costs 5–10 s (DNS + TLS), a request on a warm
//! one ~1.2 s. So the rules are: reuse connections, issue requests in parallel, never fetch the same bytes
//! twice (archive files never change, so byte ranges are cached forever), and fetch only the tiles needed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use crate::{cache, Error, Result};

/// Parallel connections to one archive.
pub const WORKERS: usize = 16;

pub struct Fetcher {
    agent: ureq::Agent,
    dir: PathBuf,
    /// Bytes downloaded / served from the cache, for the progress line.
    pub fetched: AtomicUsize,
    pub cached: AtomicUsize,
}

pub fn fetcher() -> &'static Fetcher {
    static F: OnceLock<Fetcher> = OnceLock::new();
    F.get_or_init(|| {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_global(Some(Duration::from_secs(90)))
            .max_idle_connections(64)
            .max_idle_connections_per_host(WORKERS * 2)
            .max_idle_age(Duration::from_secs(300))
            .user_agent("earthdance/0.2")
            .build()
            .into();
        Fetcher { agent, dir: cache::dir().join("bytes"), fetched: AtomicUsize::new(0), cached: AtomicUsize::new(0) }
    })
}

/// FNV-1a: a stable file name for a cache key (std's hasher is not guaranteed stable across releases).
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Retries `f` on network errors and HTTP 429 / 5xx, waiting 1, 2, 4 … s plus random jitter (so parallel
/// workers do not retry in lock-step).
pub fn retry<T>(mut f: impl FnMut() -> std::result::Result<T, ureq::Error>) -> Result<T> {
    let mut wait = 1.0f64;
    for attempt in 0..5 {
        match f() {
            Ok(v) => return Ok(v),
            Err(ureq::Error::StatusCode(c)) if c != 429 && c < 500 => return Err(Error::Http(format!("http status: {c}"))),
            Err(e) if attempt == 4 => return Err(Error::Http(e.to_string())),
            Err(_) => {
                let jitter = (fnv(&format!("{:?}{attempt}", std::thread::current().id())) % 1000) as f64 / 1000.0;
                std::thread::sleep(Duration::from_secs_f64(wait + jitter));
                wait *= 2.0;
            }
        }
    }
    unreachable!()
}

impl Fetcher {
    /// `len` bytes from `start` of `url`, from the disk cache when present.
    pub fn range(&self, url: &str, start: u64, len: u64) -> Result<Vec<u8>> {
        let path = self.dir.join(format!("{:016x}.bin", fnv(&format!("{url}#{start}+{len}"))));
        if let Ok(b) = std::fs::read(&path)
            && b.len() as u64 == len
        {
            self.cached.fetch_add(b.len(), Ordering::Relaxed);
            return Ok(b);
        }
        let end = start + len - 1;
        let bytes = retry(|| {
            let mut r = self.agent.get(url).header("Range", format!("bytes={start}-{end}")).call()?;
            r.body_mut().with_config().limit(64 << 20).read_to_vec()
        })?;
        self.fetched.fetch_add(bytes.len(), Ordering::Relaxed);
        if bytes.len() as u64 == len {
            let _ = std::fs::create_dir_all(&self.dir);
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, &bytes).is_ok() {
                let _ = std::fs::rename(tmp, &path);
            }
        }
        Ok(bytes)
    }

    /// A whole (small) file, cached on disk forever (for immutable resources such as map tiles).
    pub fn get_cached(&self, url: &str) -> Result<Vec<u8>> {
        let path = self.dir.join(format!("{:016x}.get", fnv(url)));
        if let Ok(b) = std::fs::read(&path) {
            self.cached.fetch_add(b.len(), Ordering::Relaxed);
            return Ok(b);
        }
        let bytes = retry(|| self.agent.get(url).call()?.body_mut().with_config().limit(16 << 20).read_to_vec())?;
        self.fetched.fetch_add(bytes.len(), Ordering::Relaxed);
        let _ = std::fs::create_dir_all(&self.dir);
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(tmp, &path);
        }
        Ok(bytes)
    }

    pub fn get_json(&self, url: &str) -> Result<serde_json::Value> {
        retry(|| self.agent.get(url).call()?.body_mut().read_json())
    }

    pub fn post_json(&self, url: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let t0 = std::time::Instant::now();
        let text = retry(|| self.agent.post(url).send_json(body)?.body_mut().with_config().limit(64 << 20).read_to_string())?;
        self.note_throughput(text.len(), t0.elapsed());
        self.fetched.fetch_add(text.len(), Ordering::Relaxed);
        Ok(serde_json::from_str(&text)?)
    }

    /// Remembers the link speed seen on a large response (bytes/s), for choosing how much to download.
    fn note_throughput(&self, bytes: usize, t: Duration) {
        if bytes > 50_000 && t.as_secs_f64() > 0.05 {
            cache::put("net-throughput", &(bytes as f64 / t.as_secs_f64()));
        }
    }

    pub fn remember_throughput(&self, bps: f64) {
        cache::put("net-throughput", &bps);
    }

    /// Last measured link speed (bytes/s), if any.
    pub fn throughput(&self) -> Option<f64> {
        cache::get("net-throughput")
    }
}

/// Runs `f` over `items` on up to `n` threads (work-stealing by an atomic index); results keep the order.
pub fn parallel<T: Sync, R: Send>(items: &[T], n: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<R>>> = items.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..n.min(items.len()).max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(it) = items.get(i) else { break };
                    let r = f(it);
                    *slots[i].lock().expect("slot") = Some(r);
                }
            });
        }
    });
    slots.into_iter().map(|m| m.into_inner().expect("slot").expect("every item ran")).collect()
}
