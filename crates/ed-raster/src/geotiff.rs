//! A small, strict GeoTIFF reader for single-band, uncompressed rasters.
//!
//! Supports what the EarthDance inputs use (and a little more): classic TIFF (not BigTIFF) in either byte
//! order, strip or tile layout, `Compression = 1`, `SamplesPerPixel = 1`, 8/16/32/64-bit integer and
//! 32/64-bit float samples. Georeferencing comes from `ModelPixelScale` + `ModelTiepoint` (or
//! `ModelTransformation`), the CRS from the GeoKey directory (`ProjectedCSTypeGeoKey`, falling back to a
//! "UTM zone NNN" citation), nodata from GDAL's `GDAL_NODATA` ASCII tag. Anything else is a clear error.
//!
//! All samples are converted to `f32`; samples equal to the nodata value become NaN (as the legacy
//! script did: `src[src == nodata] = nan` after `astype(float32)`).

use std::fmt;

// Baseline / extension tags.
const IMAGE_WIDTH: u16 = 256;
const IMAGE_LENGTH: u16 = 257;
const BITS_PER_SAMPLE: u16 = 258;
const COMPRESSION: u16 = 259;
const STRIP_OFFSETS: u16 = 273;
const SAMPLES_PER_PIXEL: u16 = 277;
const ROWS_PER_STRIP: u16 = 278;
const STRIP_BYTE_COUNTS: u16 = 279;
const PLANAR_CONFIGURATION: u16 = 284;
const PREDICTOR: u16 = 317;
const TILE_WIDTH: u16 = 322;
const TILE_LENGTH: u16 = 323;
const TILE_OFFSETS: u16 = 324;
const TILE_BYTE_COUNTS: u16 = 325;
const SAMPLE_FORMAT: u16 = 339;
// GeoTIFF / GDAL tags.
const MODEL_PIXEL_SCALE: u16 = 33550;
const MODEL_TIEPOINT: u16 = 33922;
const MODEL_TRANSFORMATION: u16 = 34264;
const GEO_KEY_DIRECTORY: u16 = 34735;
const GEO_ASCII_PARAMS: u16 = 34737;
const GDAL_NODATA: u16 = 42113;
// GeoKeys.
const GT_MODEL_TYPE_KEY: u16 = 1024;
const GT_RASTER_TYPE_KEY: u16 = 1025;
const GT_CITATION_KEY: u16 = 1026;
const GEOGRAPHIC_TYPE_KEY: u16 = 2048;
const PROJECTED_CS_TYPE_KEY: u16 = 3072;
const PCS_CITATION_KEY: u16 = 3073;

/// Decoding failure: what was wrong with the file, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a TIFF, truncated, or internally inconsistent.
    Malformed(String),
    /// A valid TIFF using a feature this reader does not implement (compression, BigTIFF, …).
    Unsupported(String),
    /// No usable georeferencing (missing tags or an unknown CRS).
    NoGeoreference(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(s) => write!(f, "malformed TIFF: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported TIFF: {s}"),
            Self::NoGeoreference(s) => write!(f, "no georeference: {s}"),
        }
    }
}

impl std::error::Error for Error {}

fn malformed(s: impl Into<String>) -> Error {
    Error::Malformed(s.into())
}

/// A decoded single-band raster in its native CRS.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRaster {
    pub width: usize,
    pub height: usize,
    /// Row-major, row 0 = top. NaN = nodata.
    pub data: Vec<f32>,
    /// GDAL-style affine geotransform: `X = gt[0] + col·gt[1] + row·gt[2]`, `Y = gt[3] + col·gt[4] + row·gt[5]`,
    /// with (col, row) = (0, 0) at the top-left corner of the top-left pixel.
    pub geotransform: [f64; 6],
    /// EPSG code of the CRS (e.g. 32634 = WGS 84 / UTM zone 34N, 4326 = WGS 84 lat/lon).
    pub epsg: u32,
    /// The nodata value declared in the file (already mapped to NaN in `data`).
    pub nodata: Option<f64>,
}

impl SourceRaster {
    /// True if no sample holds data (the legacy script treats such a file like an empty one).
    pub fn is_all_nodata(&self) -> bool {
        !self.data.iter().any(|v| v.is_finite())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    Little,
    Big,
}

struct Reader<'a> {
    b: &'a [u8],
    order: Order,
}

impl Reader<'_> {
    fn bytes<const N: usize>(&self, off: usize) -> Result<[u8; N], Error> {
        let end = off.checked_add(N).ok_or_else(|| malformed("offset overflow"))?;
        let s = self.b.get(off..end).ok_or_else(|| malformed(format!("read past end of file at {off}")))?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        if self.order == Order::Big {
            a.reverse();
        }
        Ok(a)
    }
    fn u16(&self, off: usize) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.bytes(off)?))
    }
    fn u32(&self, off: usize) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.bytes(off)?))
    }
}

/// One IFD entry with its values already decoded.
#[derive(Debug)]
enum Value {
    Ints(Vec<u64>),
    Signed(Vec<i64>),
    Floats(Vec<f64>),
    Ascii(String),
    Other,
}

impl Value {
    fn as_u64s(&self) -> Option<Vec<u64>> {
        match self {
            Self::Ints(v) => Some(v.clone()),
            Self::Signed(v) => v.iter().map(|&x| u64::try_from(x).ok()).collect(),
            _ => None,
        }
    }
    fn as_f64s(&self) -> Option<Vec<f64>> {
        match self {
            Self::Floats(v) => Some(v.clone()),
            Self::Ints(v) => Some(v.iter().map(|&x| x as f64).collect()),
            Self::Signed(v) => Some(v.iter().map(|&x| x as f64).collect()),
            _ => None,
        }
    }
}

fn read_entry(r: &Reader<'_>, at: usize) -> Result<(u16, Value), Error> {
    let tag = r.u16(at)?;
    let typ = r.u16(at + 2)?;
    let count = r.u32(at + 4)? as usize;
    let size = match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16 | 17 | 18 => 8,
        _ => return Ok((tag, Value::Other)),
    };
    let len = count.checked_mul(size).ok_or_else(|| malformed("tag size overflow"))?;
    let off = if len <= 4 { at + 8 } else { r.u32(at + 8)? as usize };
    if off.checked_add(len).is_none_or(|e| e > r.b.len()) {
        return Err(malformed(format!("tag {tag} data out of bounds")));
    }
    let v = match typ {
        1 | 7 => Value::Ints(r.b[off..off + len].iter().map(|&x| u64::from(x)).collect()),
        6 => Value::Signed(r.b[off..off + len].iter().map(|&x| i64::from(x as i8)).collect()),
        2 => {
            let s = &r.b[off..off + len];
            let s = s.split(|&c| c == 0).next().unwrap_or(&[]);
            Value::Ascii(String::from_utf8_lossy(s).into_owned())
        }
        3 => Value::Ints((0..count).map(|i| r.u16(off + 2 * i).map(u64::from)).collect::<Result<_, _>>()?),
        8 => Value::Signed((0..count).map(|i| r.u16(off + 2 * i).map(|x| i64::from(x as i16))).collect::<Result<_, _>>()?),
        4 | 13 => Value::Ints((0..count).map(|i| r.u32(off + 4 * i).map(u64::from)).collect::<Result<_, _>>()?),
        9 => Value::Signed((0..count).map(|i| r.u32(off + 4 * i).map(|x| i64::from(x as i32))).collect::<Result<_, _>>()?),
        11 => Value::Floats(
            (0..count).map(|i| r.bytes::<4>(off + 4 * i).map(|b| f64::from(f32::from_le_bytes(b)))).collect::<Result<_, _>>()?,
        ),
        12 => Value::Floats((0..count).map(|i| r.bytes::<8>(off + 8 * i).map(f64::from_le_bytes)).collect::<Result<_, _>>()?),
        5 => Value::Floats(
            (0..count)
                .map(|i| Ok(f64::from(r.u32(off + 8 * i)?) / f64::from(r.u32(off + 8 * i + 4)?)))
                .collect::<Result<_, Error>>()?,
        ),
        10 => Value::Floats(
            (0..count)
                .map(|i| Ok(f64::from(r.u32(off + 8 * i)? as i32) / f64::from(r.u32(off + 8 * i + 4)? as i32)))
                .collect::<Result<_, Error>>()?,
        ),
        _ => Value::Other,
    };
    Ok((tag, v))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SampleKind {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    F64,
}

impl SampleKind {
    fn from_tags(bits: u64, format: u64) -> Result<Self, Error> {
        Ok(match (format, bits) {
            (1, 8) => Self::U8,
            (2, 8) => Self::I8,
            (1, 16) => Self::U16,
            (2, 16) => Self::I16,
            (1, 32) => Self::U32,
            (2, 32) => Self::I32,
            (3, 32) => Self::F32,
            (3, 64) => Self::F64,
            _ => return Err(Error::Unsupported(format!("{bits}-bit samples with SampleFormat {format}"))),
        })
    }
    const fn size(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
    /// The sample at `b[0..size]` as f64 (exact for every supported type).
    fn read(self, b: &[u8], order: Order) -> f64 {
        let mut a = [0u8; 8];
        let n = self.size();
        a[..n].copy_from_slice(&b[..n]);
        if order == Order::Big {
            a[..n].reverse();
        }
        match self {
            Self::U8 => f64::from(a[0]),
            Self::I8 => f64::from(a[0] as i8),
            Self::U16 => f64::from(u16::from_le_bytes([a[0], a[1]])),
            Self::I16 => f64::from(i16::from_le_bytes([a[0], a[1]])),
            Self::U32 => f64::from(u32::from_le_bytes([a[0], a[1], a[2], a[3]])),
            Self::I32 => f64::from(i32::from_le_bytes([a[0], a[1], a[2], a[3]])),
            Self::F32 => f64::from(f32::from_le_bytes([a[0], a[1], a[2], a[3]])),
            Self::F64 => f64::from_le_bytes(a),
        }
    }
}

/// Decodes the first image of a GeoTIFF file.
pub fn decode(bytes: &[u8]) -> Result<SourceRaster, Error> {
    let order = match bytes.get(0..2) {
        Some(b"II") => Order::Little,
        Some(b"MM") => Order::Big,
        _ => return Err(malformed("not a TIFF (bad byte-order mark)")),
    };
    let r = Reader { b: bytes, order };
    match r.u16(2)? {
        42 => {}
        43 => return Err(Error::Unsupported("BigTIFF".into())),
        m => return Err(malformed(format!("bad magic number {m}"))),
    }
    let ifd = r.u32(4)? as usize;
    let n = r.u16(ifd)? as usize;
    let mut tags = std::collections::HashMap::with_capacity(n);
    for i in 0..n {
        let (tag, v) = read_entry(&r, ifd + 2 + 12 * i)?;
        tags.insert(tag, v);
    }

    let int = |tag: u16| -> Option<u64> { tags.get(&tag)?.as_u64s()?.first().copied() };
    let ints = |tag: u16| -> Option<Vec<u64>> { tags.get(&tag)?.as_u64s() };
    let floats = |tag: u16| -> Option<Vec<f64>> { tags.get(&tag)?.as_f64s() };
    let ascii = |tag: u16| -> Option<&str> {
        match tags.get(&tag)? {
            Value::Ascii(s) => Some(s.as_str()),
            _ => None,
        }
    };

    let width = int(IMAGE_WIDTH).ok_or_else(|| malformed("no ImageWidth"))? as usize;
    let height = int(IMAGE_LENGTH).ok_or_else(|| malformed("no ImageLength"))? as usize;
    if width == 0 || height == 0 {
        return Err(malformed("zero-sized image"));
    }
    let compression = int(COMPRESSION).unwrap_or(1);
    if compression != 1 {
        return Err(Error::Unsupported(format!("Compression = {compression} (only 1 = none is supported)")));
    }
    let spp = int(SAMPLES_PER_PIXEL).unwrap_or(1);
    if spp != 1 {
        return Err(Error::Unsupported(format!("SamplesPerPixel = {spp} (only single-band files)")));
    }
    if let Some(p) = int(PREDICTOR)
        && p != 1
    {
        return Err(Error::Unsupported(format!("Predictor = {p}")));
    }
    let _planar = int(PLANAR_CONFIGURATION).unwrap_or(1); // irrelevant with one sample per pixel
    let bits = int(BITS_PER_SAMPLE).unwrap_or(1);
    let kind = SampleKind::from_tags(bits, int(SAMPLE_FORMAT).unwrap_or(1))?;
    let ss = kind.size();

    let nodata = match ascii(GDAL_NODATA) {
        Some(s) => {
            let s = s.trim();
            Some(if s.eq_ignore_ascii_case("nan") {
                f64::NAN
            } else {
                s.parse::<f64>().map_err(|_| malformed(format!("bad GDAL_NODATA {s:?}")))?
            })
        }
        None => None,
    };
    let to_f32 = |v: f64| -> f32 {
        match nodata {
            Some(nd) if !nd.is_nan() && v == nd => f32::NAN,
            _ => v as f32,
        }
    };

    let mut data = vec![f32::NAN; width * height];
    // A block = strip or tile: (offset, byte count, x0, y0, block width, block height).
    let (block_w, block_h, offsets, counts) = if let Some(offs) = ints(TILE_OFFSETS) {
        let tw = int(TILE_WIDTH).ok_or_else(|| malformed("tiled without TileWidth"))? as usize;
        let th = int(TILE_LENGTH).ok_or_else(|| malformed("tiled without TileLength"))? as usize;
        let counts = ints(TILE_BYTE_COUNTS).ok_or_else(|| malformed("no TileByteCounts"))?;
        (tw, th, offs, counts)
    } else {
        let offs = ints(STRIP_OFFSETS).ok_or_else(|| malformed("no StripOffsets"))?;
        let rps = int(ROWS_PER_STRIP).map_or(height, |v| (v as usize).min(height));
        let counts = ints(STRIP_BYTE_COUNTS).ok_or_else(|| malformed("no StripByteCounts"))?;
        (width, rps, offs, counts)
    };
    if block_w == 0 || block_h == 0 {
        return Err(malformed("zero-sized strip/tile"));
    }
    let across = width.div_ceil(block_w);
    let down = height.div_ceil(block_h);
    if offsets.len() < across * down || counts.len() < across * down {
        return Err(malformed(format!("{} blocks listed, {} needed", offsets.len().min(counts.len()), across * down)));
    }
    for by in 0..down {
        for bx in 0..across {
            let i = by * across + bx;
            let (x0, y0) = (bx * block_w, by * block_h);
            let (cols, rows) = (block_w.min(width - x0), block_h.min(height - y0));
            // Bytes actually read from this block: full rows of `block_w` samples (tiles are padded past the
            // image edge, strips are exactly `width` wide), the last row only up to `cols`.
            let need = ((rows - 1) * block_w + cols) * ss;
            if (counts[i] as usize) < need {
                return Err(malformed(format!("block {i}: {} bytes, {need} expected", counts[i])));
            }
            let off = offsets[i] as usize;
            let blk = bytes
                .get(off..off.saturating_add(need))
                .ok_or_else(|| malformed(format!("block {i} out of bounds")))?;
            for ry in 0..rows {
                let row = &blk[ry * block_w * ss..];
                let dst = &mut data[(y0 + ry) * width + x0..][..cols];
                for (rx, d) in dst.iter_mut().enumerate() {
                    *d = to_f32(kind.read(&row[rx * ss..], order));
                }
            }
        }
    }

    let geotransform = geotransform(&floats)?;
    let geokeys = ints(GEO_KEY_DIRECTORY).ok_or_else(|| Error::NoGeoreference("no GeoKeyDirectory".into()))?;
    let epsg = epsg_from_geokeys(&geokeys, ascii(GEO_ASCII_PARAMS))?;
    let mut gt = geotransform;
    if geokey(&geokeys, GT_RASTER_TYPE_KEY) == Some(2) {
        // PixelIsPoint: the tiepoint names a pixel centre. GDAL shifts to the corner convention.
        gt[0] -= 0.5 * gt[1] + 0.5 * gt[2];
        gt[3] -= 0.5 * gt[4] + 0.5 * gt[5];
    }
    Ok(SourceRaster { width, height, data, geotransform: gt, epsg, nodata })
}

fn geotransform(floats: &dyn Fn(u16) -> Option<Vec<f64>>) -> Result<[f64; 6], Error> {
    if let Some(m) = floats(MODEL_TRANSFORMATION)
        && m.len() >= 16
    {
        return Ok([m[3], m[0], m[1], m[7], m[4], m[5]]);
    }
    let scale = floats(MODEL_PIXEL_SCALE).ok_or_else(|| Error::NoGeoreference("no ModelPixelScale".into()))?;
    let tie = floats(MODEL_TIEPOINT).ok_or_else(|| Error::NoGeoreference("no ModelTiepoint".into()))?;
    if scale.len() < 2 || tie.len() < 6 {
        return Err(malformed("short ModelPixelScale / ModelTiepoint"));
    }
    let (sx, sy) = (scale[0], scale[1]);
    let (i, j, x, y) = (tie[0], tie[1], tie[3], tie[4]);
    Ok([x - i * sx, sx, 0.0, y + j * sy, 0.0, -sy])
}

/// Value of a SHORT GeoKey stored inline in the directory.
fn geokey(dir: &[u64], key: u16) -> Option<u64> {
    dir.get(4..)?.chunks_exact(4).find(|k| k[0] == u64::from(key) && k[1] == 0).map(|k| k[3])
}

/// An ASCII GeoKey (stored in GeoAsciiParams).
fn geokey_ascii<'a>(dir: &[u64], key: u16, ascii: Option<&'a str>) -> Option<&'a str> {
    let k = dir.get(4..)?.chunks_exact(4).find(|k| k[0] == u64::from(key) && k[1] == u64::from(GEO_ASCII_PARAMS))?;
    let (n, off) = (k[2] as usize, k[3] as usize);
    let s = ascii?.get(off..off + n)?;
    Some(s.trim_end_matches(['|', '\0']))
}

fn epsg_from_geokeys(dir: &[u64], ascii: Option<&str>) -> Result<u32, Error> {
    if let Some(p) = geokey(dir, PROJECTED_CS_TYPE_KEY)
        && p != 0
        && p != 32767
    {
        return Ok(p as u32);
    }
    // No EPSG code: try the citations ("WGS 84 / UTM zone 35N").
    for key in [PCS_CITATION_KEY, GT_CITATION_KEY] {
        if let Some(c) = geokey_ascii(dir, key, ascii)
            && let Some(e) = epsg_from_citation(c)
        {
            return Ok(e);
        }
    }
    if let Some(e) = ascii.and_then(epsg_from_citation) {
        return Ok(e);
    }
    // Geographic model.
    if geokey(dir, GT_MODEL_TYPE_KEY) == Some(2) && geokey(dir, GEOGRAPHIC_TYPE_KEY) == Some(4326) {
        return Ok(4326);
    }
    Err(Error::NoGeoreference("unknown CRS (no ProjectedCSTypeGeoKey and no UTM citation)".into()))
}

/// "WGS 84 / UTM zone 35N" → 32635; "… zone 34S" → 32734.
pub fn epsg_from_citation(s: &str) -> Option<u32> {
    let lower = s.to_ascii_lowercase();
    if !lower.contains("wgs 84") && !lower.contains("wgs84") {
        return None;
    }
    let rest = &lower[lower.find("utm zone")? + "utm zone".len()..];
    let rest = rest.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let zone: u32 = digits.parse().ok()?;
    if !(1..=60).contains(&zone) {
        return None;
    }
    match rest[digits.len()..].chars().next()? {
        'n' => Some(32600 + zone),
        's' => Some(32700 + zone),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal stripped TIFF: 3×2 int16, two strips (rows per strip = 1), UTM 34N.
    fn tiny(order: Order, with_epsg: bool) -> Vec<u8> {
        let w16 = |v: &mut Vec<u8>, x: u16| match order {
            Order::Little => v.extend_from_slice(&x.to_le_bytes()),
            Order::Big => v.extend_from_slice(&x.to_be_bytes()),
        };
        let w32 = |v: &mut Vec<u8>, x: u32| match order {
            Order::Little => v.extend_from_slice(&x.to_le_bytes()),
            Order::Big => v.extend_from_slice(&x.to_be_bytes()),
        };
        let w64 = |v: &mut Vec<u8>, x: f64| match order {
            Order::Little => v.extend_from_slice(&x.to_le_bytes()),
            Order::Big => v.extend_from_slice(&x.to_be_bytes()),
        };
        let mut b = Vec::new();
        b.extend_from_slice(if order == Order::Little { b"II" } else { b"MM" });
        w16(&mut b, 42);
        w32(&mut b, 8);
        // Data area placed after the IFD; compute layout first.
        let pixels: [i16; 6] = [1, -9999, 3, 4, 5, -6];
        let geokeys: Vec<u16> = if with_epsg {
            vec![1, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, 1, 3072, 0, 1, 32634]
        } else {
            vec![1, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, 1, 3073, 34737, 22, 0]
        };
        let citation = b"WGS 84 / UTM zone 35N|\0";
        let n_entries = 16u16;
        let ifd_size = 2 + 12 * n_entries as usize + 4;
        let data_start = 8 + ifd_size;
        let strip0 = data_start;
        let strip1 = strip0 + 6;
        let scale_off = strip1 + 6;
        let tie_off = scale_off + 24;
        let gk_off = tie_off + 48;
        let cit_off = gk_off + 2 * geokeys.len();
        let nd_off = cit_off + citation.len();
        let so_off = nd_off + 6;
        let sc_off = so_off + 8;
        w16(&mut b, n_entries);
        let entry = |b: &mut Vec<u8>, tag: u16, typ: u16, count: u32, val: u32, short: bool| {
            w16(b, tag);
            w16(b, typ);
            w32(b, count);
            if short {
                w16(b, val as u16);
                w16(b, 0);
            } else {
                w32(b, val);
            }
        };
        entry(&mut b, 256, 3, 1, 3, true);
        entry(&mut b, 257, 3, 1, 2, true);
        entry(&mut b, 258, 3, 1, 16, true);
        entry(&mut b, 259, 3, 1, 1, true);
        entry(&mut b, 262, 3, 1, 1, true);
        entry(&mut b, 273, 4, 2, so_off as u32, false);
        entry(&mut b, 277, 3, 1, 1, true);
        entry(&mut b, 278, 3, 1, 1, true);
        entry(&mut b, 279, 4, 2, sc_off as u32, false);
        entry(&mut b, 284, 3, 1, 1, true);
        entry(&mut b, 339, 3, 1, 2, true);
        entry(&mut b, 33550, 12, 3, scale_off as u32, false);
        entry(&mut b, 33922, 12, 6, tie_off as u32, false);
        entry(&mut b, 34735, 3, geokeys.len() as u32, gk_off as u32, false);
        entry(&mut b, 34737, 2, citation.len() as u32, cit_off as u32, false);
        entry(&mut b, 42113, 2, 6, nd_off as u32, false);
        w32(&mut b, 0);
        assert_eq!(b.len(), data_start);
        for p in pixels {
            w16(&mut b, p as u16);
        }
        for v in [30.0, 30.0, 0.0] {
            w64(&mut b, v);
        }
        for v in [0.0, 0.0, 0.0, 600_000.0, 4_600_020.0, 0.0] {
            w64(&mut b, v);
        }
        for g in &geokeys {
            w16(&mut b, *g);
        }
        b.extend_from_slice(citation);
        b.extend_from_slice(b"-9999\0");
        w32(&mut b, strip0 as u32);
        w32(&mut b, strip1 as u32);
        w32(&mut b, 6);
        w32(&mut b, 6);
        b
    }

    #[test]
    fn decodes_both_byte_orders() {
        for order in [Order::Little, Order::Big] {
            let r = decode(&tiny(order, true)).unwrap();
            assert_eq!((r.width, r.height), (3, 2));
            assert_eq!(r.epsg, 32634);
            assert_eq!(r.geotransform, [600_000.0, 30.0, 0.0, 4_600_020.0, 0.0, -30.0]);
            assert_eq!(r.nodata, Some(-9999.0));
            assert_eq!(r.data[0], 1.0);
            assert!(r.data[1].is_nan());
            assert_eq!(&r.data[2..], &[3.0, 4.0, 5.0, -6.0]);
        }
    }

    #[test]
    fn falls_back_to_citation() {
        let r = decode(&tiny(Order::Little, false)).unwrap();
        assert_eq!(r.epsg, 32635);
    }

    #[test]
    fn rejects_garbage_and_compression() {
        assert!(matches!(decode(b""), Err(Error::Malformed(_))));
        assert!(matches!(decode(b"II\x2b\x00\x08\x00\x00\x00"), Err(Error::Unsupported(_))));
        let mut b = tiny(Order::Little, true);
        // Entry 3 (index 3) is Compression; patch its value to 5 (LZW).
        let at = 8 + 2 + 12 * 3 + 8;
        b[at] = 5;
        assert!(matches!(decode(&b), Err(Error::Unsupported(_))));
        // Truncated file.
        let b = tiny(Order::Little, true);
        assert!(decode(&b[..100]).is_err());
    }

    #[test]
    fn citations() {
        assert_eq!(epsg_from_citation("WGS 84 / UTM zone 35N"), Some(32635));
        assert_eq!(epsg_from_citation("WGS 84 / UTM zone 4S|WGS 84|"), Some(32704));
        assert_eq!(epsg_from_citation("ETRS89 / UTM zone 34N"), None);
        assert_eq!(epsg_from_citation("WGS 84 / UTM zone 61N"), None);
    }
}
