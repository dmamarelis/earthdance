//! Cloud-Optimized GeoTIFF access by byte ranges.
//!
//! A COG keeps every IFD (full resolution and each overview) and every tile's offset at the start of the
//! file, so one small request (typically 16 KB) says exactly which compressed tiles cover an area at a
//! given resolution. This module is IO-free: [`Cog::parse`] reads a header prefix (and asks for more bytes
//! when the prefix is too short), [`Cog::plan`] lists the byte ranges to fetch for a window, and
//! [`Cog::assemble`] decodes the fetched tiles into a [`SourceRaster`] that `warp` puts on any grid.
//!
//! Supported: classic TIFF (not BigTIFF), both byte orders, tiled images, 8/16/32-bit integer and 32-bit
//! float samples, no compression or DEFLATE (8 / 32946), horizontal predictor (2). That covers the Sentinel-2
//! and Sentinel-1 RTC COGs of the public archives.

use crate::geotiff::SourceRaster;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CogError {
    /// The header prefix is too short: fetch at least this many bytes from the start and parse again.
    NeedMore(usize),
    Malformed(String),
    Unsupported(String),
}

impl std::fmt::Display for CogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CogError::NeedMore(n) => write!(f, "COG header needs {n} bytes"),
            CogError::Malformed(s) => write!(f, "malformed COG: {s}"),
            CogError::Unsupported(s) => write!(f, "unsupported COG: {s}"),
        }
    }
}
impl std::error::Error for CogError {}

/// One image of the pyramid (level 0 = full resolution).
#[derive(Clone, Debug)]
pub struct Level {
    pub width: usize,
    pub height: usize,
    pub tile_w: usize,
    pub tile_h: usize,
    pub offsets: Vec<u64>,
    pub counts: Vec<u64>,
    pub compression: u16,
    pub predictor: u16,
    pub bits: u16,
    /// 1 = unsigned, 2 = signed, 3 = float.
    pub sample_format: u16,
    /// Pixel size in CRS units.
    pub res: (f64, f64),
}

impl Level {
    pub fn tiles_across(&self) -> usize {
        self.width.div_ceil(self.tile_w)
    }
}

#[derive(Clone, Debug)]
pub struct Cog {
    pub little_endian: bool,
    pub levels: Vec<Level>,
    /// Top-left corner of the image in CRS units.
    pub origin: (f64, f64),
    pub epsg: u32,
    pub nodata: Option<f64>,
}

/// The tiles to fetch for a window, and where the window sits in its level.
#[derive(Clone, Debug)]
pub struct Plan {
    pub level: usize,
    /// Window in pixels of that level: column, row, width, height.
    pub window: (usize, usize, usize, usize),
    /// (tile index, byte range start, length)
    pub tiles: Vec<(usize, u64, u64)>,
}

struct Reader<'a> {
    b: &'a [u8],
    le: bool,
}

impl Reader<'_> {
    fn need(&self, end: usize) -> Result<(), CogError> {
        if end > self.b.len() { Err(CogError::NeedMore(end.next_power_of_two().max(16384))) } else { Ok(()) }
    }
    fn u16(&self, at: usize) -> Result<u16, CogError> {
        self.need(at + 2)?;
        let a = [self.b[at], self.b[at + 1]];
        Ok(if self.le { u16::from_le_bytes(a) } else { u16::from_be_bytes(a) })
    }
    fn u32(&self, at: usize) -> Result<u32, CogError> {
        self.need(at + 4)?;
        let a = [self.b[at], self.b[at + 1], self.b[at + 2], self.b[at + 3]];
        Ok(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }
    fn u64(&self, at: usize) -> Result<u64, CogError> {
        self.need(at + 8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(&self.b[at..at + 8]);
        Ok(if self.le { u64::from_le_bytes(a) } else { u64::from_be_bytes(a) })
    }
    fn f64(&self, at: usize) -> Result<f64, CogError> {
        Ok(f64::from_bits(self.u64(at)?))
    }
}

/// A tag's values, read as u64 / f64 / text.
struct Entry {
    typ: u16,
    count: usize,
    /// Byte position of the values (inline or pointed to).
    at: usize,
}

fn type_size(t: u16) -> usize {
    match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 | 16 | 17 => 8,
        _ => 1,
    }
}

impl Entry {
    fn ints(&self, r: &Reader) -> Result<Vec<u64>, CogError> {
        let s = type_size(self.typ);
        r.need(self.at + s * self.count)?;
        (0..self.count)
            .map(|i| {
                let p = self.at + i * s;
                Ok(match self.typ {
                    1 | 7 => u64::from(r.b[p]),
                    3 => u64::from(r.u16(p)?),
                    4 => u64::from(r.u32(p)?),
                    16 => r.u64(p)?,
                    t => return Err(CogError::Unsupported(format!("integer tag of type {t}"))),
                })
            })
            .collect()
    }
    fn first(&self, r: &Reader) -> Result<u64, CogError> {
        self.ints(r)?.first().copied().ok_or_else(|| CogError::Malformed("empty tag".into()))
    }
    fn doubles(&self, r: &Reader) -> Result<Vec<f64>, CogError> {
        r.need(self.at + 8 * self.count)?;
        (0..self.count).map(|i| r.f64(self.at + 8 * i)).collect()
    }
    fn text(&self, r: &Reader) -> Result<String, CogError> {
        r.need(self.at + self.count)?;
        Ok(String::from_utf8_lossy(&r.b[self.at..self.at + self.count]).trim_end_matches('\0').to_string())
    }
}

type Tags = std::collections::HashMap<u16, Entry>;

fn tag(t: &Tags, id: u16) -> Result<&Entry, CogError> {
    t.get(&id).ok_or_else(|| CogError::Unsupported(format!("missing tag {id}")))
}

impl Cog {
    /// Parses a header prefix. Returns `NeedMore(n)` when the IFDs or tile tables reach beyond it.
    pub fn parse(b: &[u8]) -> Result<Cog, CogError> {
        if b.len() < 8 {
            return Err(CogError::NeedMore(16384));
        }
        let le = match &b[..2] {
            b"II" => true,
            b"MM" => false,
            _ => return Err(CogError::Malformed("not a TIFF".into())),
        };
        let r = Reader { b, le };
        match r.u16(2)? {
            42 => {}
            43 => return Err(CogError::Unsupported("BigTIFF".into())),
            m => return Err(CogError::Malformed(format!("magic {m}"))),
        }
        let mut off = r.u32(4)? as usize;
        let mut ifds: Vec<std::collections::HashMap<u16, Entry>> = Vec::new();
        while off != 0 {
            let n = r.u16(off)? as usize;
            r.need(off + 2 + 12 * n + 4)?;
            let mut tags = std::collections::HashMap::new();
            for i in 0..n {
                let p = off + 2 + 12 * i;
                let (tag, typ, count) = (r.u16(p)?, r.u16(p + 2)?, r.u32(p + 4)? as usize);
                let at = if type_size(typ) * count <= 4 { p + 8 } else { r.u32(p + 8)? as usize };
                tags.insert(tag, Entry { typ, count, at });
            }
            ifds.push(tags);
            off = r.u32(off + 2 + 12 * n)? as usize;
            if ifds.len() > 32 {
                return Err(CogError::Malformed("too many IFDs".into()));
            }
        }
        let first = ifds.first().ok_or_else(|| CogError::Malformed("no IFD".into()))?;

        let scale = tag(first, 33550)?.doubles(&r)?;
        let tie = tag(first, 33922)?.doubles(&r)?;
        if scale.len() < 2 || tie.len() < 6 {
            return Err(CogError::Malformed("georeference".into()));
        }
        let origin = (tie[3] - tie[0] * scale[0], tie[4] + tie[1] * scale[1]);
        let epsg = match first.get(&34735) {
            Some(e) => {
                let keys = e.ints(&r)?;
                keys.chunks(4).skip(1).find(|k| k[0] == 3072 || k[0] == 2048).map_or(0, |k| k[3] as u32)
            }
            None => 0,
        };
        let nodata = first.get(&42113).map(|e| e.text(&r)).transpose()?.and_then(|s| s.trim().parse::<f64>().ok());
        let w0 = tag(first, 256)?.first(&r)? as f64;

        let mut levels = Vec::new();
        for t in &ifds {
            // skip masks (NewSubfileType bit 2)
            if t.get(&254).map(|e| e.first(&r)).transpose()?.is_some_and(|v| v & 4 != 0) {
                continue;
            }
            let Some(tw) = t.get(&322) else { return Err(CogError::Unsupported("striped (not tiled) image".into())) };
            if t.get(&277).map(|e| e.first(&r)).transpose()?.unwrap_or(1) != 1 {
                return Err(CogError::Unsupported("more than one band".into()));
            }
            let width = tag(t, 256)?.first(&r)? as usize;
            let k = w0 / width as f64;
            levels.push(Level {
                width,
                height: tag(t, 257)?.first(&r)? as usize,
                tile_w: tw.first(&r)? as usize,
                tile_h: tag(t, 323)?.first(&r)? as usize,
                offsets: tag(t, 324)?.ints(&r)?,
                counts: tag(t, 325)?.ints(&r)?,
                compression: t.get(&259).map(|e| e.first(&r)).transpose()?.unwrap_or(1) as u16,
                predictor: t.get(&317).map(|e| e.first(&r)).transpose()?.unwrap_or(1) as u16,
                bits: tag(t, 258)?.first(&r)? as u16,
                sample_format: t.get(&339).map(|e| e.first(&r)).transpose()?.unwrap_or(1) as u16,
                res: (scale[0] * k, scale[1] * k),
            });
        }
        Ok(Cog { little_endian: le, levels, origin, epsg, nodata })
    }

    /// The coarsest level whose pixels are no larger than `res` (CRS units); level 0 if none is fine enough.
    pub fn level_for(&self, res: f64) -> usize {
        (0..self.levels.len()).rev().find(|&i| self.levels[i].res.0 <= res * 1.0001).unwrap_or(0)
    }

    /// Like [`Self::level_for`] but accepts pixels up to `slack` × coarser (fewer bytes; the warp resamples).
    pub fn level_near(&self, res: f64, slack: f64) -> usize {
        self.level_for(res * slack)
    }

    /// The tiles covering `[x0, x1] × [y0, y1]` (CRS units) at `level`, plus one pixel of margin for the
    /// bilinear warp. `None` if the window misses the image.
    pub fn plan(&self, level: usize, x0: f64, y0: f64, x1: f64, y1: f64) -> Option<Plan> {
        let l = &self.levels[level];
        let c0 = (((x0.min(x1) - self.origin.0) / l.res.0).floor() as i64 - 1).max(0);
        let c1 = (((x0.max(x1) - self.origin.0) / l.res.0).ceil() as i64 + 1).min(l.width as i64);
        let r0 = (((self.origin.1 - y0.max(y1)) / l.res.1).floor() as i64 - 1).max(0);
        let r1 = (((self.origin.1 - y0.min(y1)) / l.res.1).ceil() as i64 + 1).min(l.height as i64);
        if c1 <= c0 || r1 <= r0 {
            return None;
        }
        let (c0, c1, r0, r1) = (c0 as usize, c1 as usize, r0 as usize, r1 as usize);
        let across = l.tiles_across();
        let mut tiles = Vec::new();
        for ty in r0 / l.tile_h..=(r1 - 1) / l.tile_h {
            for tx in c0 / l.tile_w..=(c1 - 1) / l.tile_w {
                let i = ty * across + tx;
                if l.counts.get(i).copied().unwrap_or(0) > 0 {
                    tiles.push((i, l.offsets[i], l.counts[i]));
                }
            }
        }
        Some(Plan { level, window: (c0, r0, c1 - c0, r1 - r0), tiles })
    }

    /// Decodes the fetched tiles (same order as `plan.tiles`) into the window. Missing tiles are nodata.
    pub fn assemble(&self, plan: &Plan, tiles: &[Vec<u8>]) -> Result<SourceRaster, CogError> {
        let l = &self.levels[plan.level];
        let (c0, r0, w, h) = plan.window;
        let mut data = vec![f32::NAN; w * h];
        let across = l.tiles_across();
        let nodata = self.nodata;
        for ((i, _, _), raw) in plan.tiles.iter().zip(tiles) {
            let px = decode_tile(l, raw, self.little_endian)?;
            let (tx, ty) = (i % across * l.tile_w, i / across * l.tile_h);
            for yy in 0..l.tile_h {
                let row = ty + yy;
                if row < r0 || row >= r0 + h {
                    continue;
                }
                for xx in 0..l.tile_w {
                    let col = tx + xx;
                    if col < c0 || col >= c0 + w {
                        continue;
                    }
                    let v = px[yy * l.tile_w + xx];
                    data[(row - r0) * w + (col - c0)] = if nodata.is_some_and(|n| f64::from(v) == n) { f32::NAN } else { v };
                }
            }
        }
        let gt = [self.origin.0 + c0 as f64 * l.res.0, l.res.0, 0.0, self.origin.1 - r0 as f64 * l.res.1, 0.0, -l.res.1];
        Ok(SourceRaster { width: w, height: h, data, geotransform: gt, epsg: self.epsg, nodata })
    }
}

/// One tile → `tile_w × tile_h` samples as f32.
pub fn decode_tile(l: &Level, raw: &[u8], le: bool) -> Result<Vec<f32>, CogError> {
    let bytes = match l.compression {
        1 => raw.to_vec(),
        8 | 32946 => miniz_oxide::inflate::decompress_to_vec_zlib(raw).map_err(|e| CogError::Malformed(format!("deflate: {e:?}")))?,
        c => return Err(CogError::Unsupported(format!("compression {c}"))),
    };
    let n = l.tile_w * l.tile_h;
    let bps = usize::from(l.bits / 8);
    if bytes.len() < n * bps {
        return Err(CogError::Malformed(format!("tile has {} bytes, expected {}", bytes.len(), n * bps)));
    }
    let word = |i: usize| -> u64 {
        let s = &bytes[i * bps..(i + 1) * bps];
        let mut a = [0u8; 8];
        if le {
            a[..bps].copy_from_slice(s);
            u64::from_le_bytes(a)
        } else {
            a[8 - bps..].copy_from_slice(s);
            u64::from_be_bytes(a)
        }
    };
    let mut w: Vec<u64> = (0..n).map(word).collect();
    match l.predictor {
        1 => {}
        2 => {
            // horizontal differencing: each sample is stored as the difference from its left neighbour
            let mask = if l.bits == 64 { u64::MAX } else { (1u64 << l.bits) - 1 };
            for row in w.chunks_mut(l.tile_w) {
                for x in 1..row.len() {
                    row[x] = (row[x].wrapping_add(row[x - 1])) & mask;
                }
            }
        }
        p => return Err(CogError::Unsupported(format!("predictor {p}"))),
    }
    Ok(w.into_iter()
        .map(|v| match (l.sample_format, l.bits) {
            (3, 32) => f32::from_bits(v as u32),
            (2, 8) => f32::from(v as u8 as i8),
            (2, 16) => f32::from(v as u16 as i16),
            (2, 32) => v as u32 as i32 as f32,
            _ => v as f32,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_sentinel2_header() {
        let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/cog/s2_b04_header.bin")).unwrap();
        let c = Cog::parse(&b).unwrap();
        assert_eq!(c.levels.len(), 5);
        assert_eq!((c.levels[0].width, c.levels[0].tile_w, c.levels[0].compression, c.levels[0].predictor), (10980, 1024, 8, 2));
        assert_eq!(c.levels[1].res, (20.0, 20.0));
        assert_eq!(c.epsg, 32634);
        assert_eq!(c.level_for(25.0), 1);
        assert_eq!(c.level_for(5.0), 0);
        // a 20 × 13 km window needs at most 3 × 3 of the 10-km tiles at 20 m
        let (x, y) = (c.origin.0 + 50_000.0, c.origin.1 - 50_000.0);
        let p = c.plan(1, x, y - 13_000.0, x + 20_000.0, y).unwrap();
        assert!(p.tiles.len() <= 9 && !p.tiles.is_empty(), "{} tiles", p.tiles.len());
        // a truncated header asks for more
        assert!(matches!(Cog::parse(&b[..200]), Err(CogError::NeedMore(_))));
    }

    #[test]
    fn predictor_and_deflate_round_trip() {
        let l = Level { width: 4, height: 2, tile_w: 4, tile_h: 2, offsets: vec![0], counts: vec![1], compression: 8, predictor: 2, bits: 16, sample_format: 1, res: (1.0, 1.0) };
        let vals: [u16; 8] = [10, 12, 9, 65535, 0, 1, 2, 3];
        let mut diff = Vec::new();
        for row in vals.chunks(4) {
            let mut prev = 0u16;
            for (i, v) in row.iter().enumerate() {
                diff.extend_from_slice(&(if i == 0 { *v } else { v.wrapping_sub(prev) }).to_le_bytes());
                prev = *v;
            }
        }
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&diff, 6);
        let out = decode_tile(&l, &z, true).unwrap();
        assert_eq!(out, vals.iter().map(|v| f32::from(*v)).collect::<Vec<_>>());
    }
}
