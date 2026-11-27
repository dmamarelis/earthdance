//! Reprojection of a [`SourceRaster`] onto a lat/lon (EPSG:4326) [`Grid`], bilinear, as GDAL's warper does it.
//!
//! The legacy script called `rasterio.warp.reproject(..., resampling=bilinear, src_nodata=nan)`, i.e.
//! `GDALWarpOperation::ChunkAndWarpImage` with an approximate transformer (`tolerance = 0.125` px) over the
//! `GDALGenImgProjTransformer`. Bit-level parity with the committed outputs needs every step of that chain:
//!
//! 1. **Chunking** (`collect_chunks`, from `CollectChunkListInternal`): the destination is cut in half along its
//!    longer side (height on ties), recursively, while the *source fill ratio* of a chunk is below 0.5 — the
//!    clamped, padded source window over the chunk's padded bounding box in source pixels. The inputs here
//!    cover about half of the site grid, so the 1200×600 grid is split into 2–7 chunks depending on the file.
//! 2. **Coordinates** (`approx_row`, from `GDALApproxTransform`): per chunk row, the pixel centres go destination
//!    pixel → lon/lat → UTM ([`crate::utm`]) → source pixel. Only the first, middle and last point are
//!    transformed exactly; if linear interpolation between the ends misses the middle by more than 0.125
//!    source pixels (|Δx| + |Δy|) the row is halved recursively, otherwise interpolated linearly. Rows run
//!    over the chunk width, which is why chunking changes the result.
//! 3. **Validity** (`GWKCheckAndComputeSrcOffsets` + `panUnifiedSrcValid`): a destination pixel is written
//!    only if the source pixel *containing* the point (`floor(coordinate + 1e-10)`) exists and is valid;
//!    otherwise it stays NaN.
//! 4. **Value**: GDAL picks per chunk from the scale = chunk size / clamped source extent (`dfSrcXSizeRaw`),
//!    snapped to an integer reciprocal when within 0.05. If both scales are ≥ 0.95, the 4-sample bilinear
//!    formula (`GWKBilinearResample4Sample`): the 2×2 source centres around the point, invalid or
//!    out-of-range ones skipped and the rest renormalised. Otherwise the general kernel (`GWKResample`) with
//!    the bilinear filter stretched by 1/scale along the downsampled axis (here: chunks at the top of the
//!    UTM 34N footprint, y scale ≈ 0.88, 3 source rows per sample).
//!
//! GDAL's code changed over versions (newer releases estimate the scale by sampling the transformer and use
//! 4 samples down to scale 0.5). The rules above are those that reproduce `data/real/` exactly: 100 % of the
//! hover cells, every water area and every zone mean (see `tests/parity.rs`).

use std::fmt;

use ed_core::{Bounds, Grid};

use crate::geotiff::SourceRaster;
use crate::utm::{self, Zone};

/// The source CRS is not one this module can reproject from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnsupportedCrs(pub u32);

impl fmt::Display for UnsupportedCrs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot reproject from EPSG:{} (only WGS 84 / UTM and EPSG:4326 are supported)", self.0)
    }
}

impl std::error::Error for UnsupportedCrs {}

/// rasterio's default `tolerance` for the approximate transformer, in source pixels.
pub const APPROX_TOLERANCE: f64 = 0.125;

/// Maps destination pixel coordinates to source pixel coordinates exactly (`GDALGenImgProjTransform`, dst → src).
#[derive(Clone, Copy)]
struct Transformer {
    dst_gt: [f64; 6],
    src_inv: [f64; 6],
    proj: Proj,
}

#[derive(Clone, Copy)]
enum Proj {
    Utm(Zone),
    Geographic,
}

impl Transformer {
    fn new(src: &SourceRaster, dst_gt: [f64; 6]) -> Result<Self, UnsupportedCrs> {
        let proj = match src.epsg {
            4326 => Proj::Geographic,
            e => Proj::Utm(Zone::from_epsg(e).ok_or(UnsupportedCrs(e))?),
        };
        Ok(Self { dst_gt, src_inv: inv_geotransform(&src.geotransform), proj })
    }

    fn apply(&self, px: f64, py: f64) -> (f64, f64) {
        let g = &self.dst_gt;
        let lon = g[0] + px * g[1] + py * g[2];
        let lat = g[3] + px * g[4] + py * g[5];
        let (x, y) = match self.proj {
            Proj::Utm(z) => utm::forward(z, lon, lat),
            Proj::Geographic => (lon, lat),
        };
        let i = &self.src_inv;
        (i[0] + x * i[1] + y * i[2], i[3] + x * i[4] + y * i[5])
    }
}

/// `GDALInvGeoTransform`, including its exact formulas for the common north-up case.
fn inv_geotransform(gt: &[f64; 6]) -> [f64; 6] {
    if gt[2] == 0.0 && gt[4] == 0.0 && gt[1] != 0.0 && gt[5] != 0.0 {
        return [-gt[0] / gt[1], 1.0 / gt[1], 0.0, -gt[3] / gt[5], 0.0, 1.0 / gt[5]];
    }
    let det = gt[1] * gt[5] - gt[2] * gt[4];
    let inv = 1.0 / det;
    [
        (gt[2] * gt[3] - gt[0] * gt[5]) * inv,
        gt[5] * inv,
        -gt[2] * inv,
        (-gt[1] * gt[3] + gt[0] * gt[4]) * inv,
        -gt[4] * inv,
        gt[1] * inv,
    ]
}

/// Source pixel coordinates of the destination pixel centres `x + 0.5` (x = 0..n) on row centre `py`,
/// following `GDALApproxTransform` / `GDALApproxTransformInternal`. Writes into `sx`, `sy`.
fn approx_row(t: &Transformer, py: f64, max_error: f64, sx: &mut [f64], sy: &mut [f64]) {
    let n = sx.len();
    for (i, x) in sx.iter_mut().enumerate() {
        *x = i as f64 + 0.5;
    }
    let mid = (n.max(1) - 1) / 2;
    if n <= 5 || max_error == 0.0 {
        exact(t, py, sx, sy);
        return;
    }
    let sme = [t.apply(sx[0], py), t.apply(sx[mid], py), t.apply(sx[n - 1], py)];
    approx_internal(t, py, max_error, sx, sy, sme);
}

fn exact(t: &Transformer, py: f64, sx: &mut [f64], sy: &mut [f64]) {
    for (x, y) in sx.iter_mut().zip(sy.iter_mut()) {
        (*x, *y) = t.apply(*x, py);
    }
}

/// `sx` holds destination x on entry, source x on exit; `sme` = transformed start, middle, end.
fn approx_internal(t: &Transformer, py: f64, max_error: f64, sx: &mut [f64], sy: &mut [f64], sme: [(f64, f64); 3]) {
    let n = sx.len();
    let mid = (n - 1) / 2;
    let span = sx[n - 1] - sx[0];
    let dx = (sme[2].0 - sme[0].0) / span;
    let dy = (sme[2].1 - sme[0].1) / span;
    let along = sx[mid] - sx[0];
    let err = ((sme[0].0 + dx * along) - sme[1].0).abs() + ((sme[0].1 + dy * along) - sme[1].1).abs();
    if err > max_error {
        // First half: points [0, mid) with its own middle (mid − 1) / 2 and end mid − 1;
        // second half: points [mid, n), starting at the already transformed middle.
        let (lx, rx) = sx.split_at_mut(mid);
        let (ly, ry) = sy.split_at_mut(mid);
        if mid <= 5 {
            exact(t, py, lx, ly);
        } else {
            let m1 = t.apply(lx[(mid - 1) / 2], py);
            let e1 = t.apply(lx[mid - 1], py);
            approx_internal(t, py, max_error, lx, ly, [sme[0], m1, e1]);
        }
        let n2 = n - mid;
        if n2 <= 5 {
            exact(t, py, rx, ry);
        } else {
            let m2 = t.apply(rx[(n2 - 1) / 2], py);
            approx_internal(t, py, max_error, rx, ry, [sme[1], m2, sme[2]]);
        }
        return;
    }
    let x0 = sx[0];
    for (x, y) in sx.iter_mut().zip(sy.iter_mut()) {
        let d = *x - x0;
        *y = sme[0].1 + dy * d;
        *x = sme[0].0 + dx * d;
    }
}

/// `GWKBilinearResample4Sample` with a validity mask (NaN = invalid). `None` = no valid neighbour.
fn bilinear4(src: &SourceRaster, sx: f64, sy: f64) -> Option<f64> {
    let (w, h) = (src.width as i64, src.height as i64);
    let mut ix = (sx - 0.5).floor() as i64;
    let mut iy = (sy - 0.5).floor() as i64;
    let mut rx = 1.5 - (sx - ix as f64);
    let mut ry = 1.5 - (sy - iy as f64);
    if ix == -1 {
        ix = 0;
        rx = 1.0;
    }
    if iy == -1 {
        iy = 0;
        ry = 1.0;
    }
    let get = |x: i64, y: i64| -> Option<f64> {
        if x < 0 || y < 0 || x >= w || y >= h {
            return None;
        }
        let v = src.data[(y * w + x) as usize];
        v.is_finite().then_some(f64::from(v))
    };
    let mut acc = 0.0;
    let mut div = 0.0;
    for (dy, wy) in [(0, ry), (1, 1.0 - ry)] {
        for (dx, wx) in [(0, rx), (1, 1.0 - rx)] {
            if let Some(v) = get(ix + dx, iy + dy) {
                let m = wx * wy;
                div += m;
                acc += v * m;
            }
        }
    }
    if div == 1.0 {
        Some(acc)
    } else if div < 0.00001 {
        None
    } else {
        Some(acc / div)
    }
}

/// GDAL's bilinear filter weight.
fn bilinear_weight(x: f64) -> f64 {
    let a = x.abs();
    if a <= 1.0 { 1.0 - a } else { 0.0 }
}

/// `GWKResample` (the general kernel path) for the bilinear filter: when the destination is coarser than the
/// source along an axis (scale < 1), the filter is stretched by 1/scale, so more than two source rows/columns
/// contribute. Invalid pixels are skipped and the weights renormalised.
fn kernel_resample(src: &SourceRaster, sx: f64, sy: f64, scale: (f64, f64)) -> Option<f64> {
    let (xscale, yscale) = scale;
    let (w, h) = (src.width as i64, src.height as i64);
    let ix = (sx - 0.5).floor() as i64;
    let iy = (sy - 0.5).floor() as i64;
    let dx = sx - 0.5 - ix as f64;
    let dy = sy - 0.5 - iy as f64;
    // Radius 1 for bilinear, ceil(1/scale) when downsampling; the window starts at −radius (even filter size).
    let xr = if xscale < 1.0 { (1.0 / xscale).ceil() as i64 } else { 1 };
    let yr = if yscale < 1.0 { (1.0 / yscale).ceil() as i64 } else { 1 };
    let (jmin, jmax) = ((-yr).max(-iy), yr.min(h - iy - 1));
    let (imin, imax) = ((-xr).max(-ix), xr.min(w - ix - 1));
    let (mut acc, mut accw) = (0.0, 0.0);
    for j in jmin..=jmax {
        let wy = bilinear_weight(if yscale < 1.0 { (j as f64 - dy) * yscale } else { j as f64 - dy });
        let row = &src.data[((iy + j) * w) as usize..];
        let (mut a, mut ww) = (0.0, 0.0);
        for i in imin..=imax {
            let v = row[(ix + i) as usize];
            if !v.is_finite() {
                continue;
            }
            let wx = bilinear_weight(if xscale < 1.0 { (i as f64 - dx) * xscale } else { i as f64 - dx });
            a += f64::from(v) * wx;
            ww += wx;
        }
        acc += a * wy;
        accw += ww * wy;
    }
    if accw < 0.000001 {
        None
    } else if !(0.99999..=1.00001).contains(&accw) {
        Some(acc / accw)
    } else {
        Some(acc)
    }
}

/// The destination value for source coordinates (sx, sy): NaN unless the containing source pixel is valid.
/// `kernel = None` → the 4-sample formula, otherwise the stretched kernel with these (x, y) scales.
fn resample(src: &SourceRaster, sx: f64, sy: f64, kernel: Option<(f64, f64)>) -> f32 {
    // `!(a >= 0)` also rejects NaN coordinates.
    if !(sx >= 0.0 && sy >= 0.0) {
        return f32::NAN;
    }
    // The containing pixel. (Some GDAL versions also accept a point up to one pixel past the last
    // row/column; the version that produced the oracle does not: matching `hh_db` to 0.01 dB requires this.)
    let ix = (sx + 1e-10) as usize;
    let iy = (sy + 1e-10) as usize;
    if ix >= src.width || iy >= src.height || !src.data[iy * src.width + ix].is_finite() {
        return f32::NAN;
    }
    match kernel {
        None => bilinear4(src, sx, sy),
        Some(scale) => kernel_resample(src, sx, sy, scale),
    }
    .map_or(f32::NAN, |v| v as f32)
}

/// A destination chunk: `(x0, y0, width, height)`.
type Chunk = (usize, usize, usize, usize);

/// What `GDALWarpOperation::ComputeSourceWindow` derives for a destination chunk.
struct SourceWindow {
    /// Clamped source window area over the area of the chunk's bounding box in source pixels.
    fill: f64,
    /// Destination/source scale per axis, from the clamped source extent (`dfSrcXSizeRaw`).
    scale: (f64, f64),
}

/// The bounding box of the chunk's edges (21 points per edge, pixel corners) in source pixels, clamped to the
/// raster. `None` if it does not intersect the source.
fn source_window(t: &Transformer, src: &SourceRaster, c: Chunk) -> Option<SourceWindow> {
    let (x0, y0, cw, ch) = c;
    const STEPS: usize = 21;
    let (mut mnx, mut mxx, mut mny, mut mxy) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    let mut add = |px: f64, py: f64| {
        let (x, y) = t.apply(px, py);
        mnx = mnx.min(x);
        mxx = mxx.max(x);
        mny = mny.min(y);
        mxy = mxy.max(y);
    };
    for i in 0..STEPS {
        let f = i as f64 / (STEPS - 1) as f64;
        let (x, y) = (x0 as f64 + f * cw as f64, y0 as f64 + f * ch as f64);
        add(x, y0 as f64);
        add(x, (y0 + ch) as f64);
        add(x0 as f64, y);
        add((x0 + cw) as f64, y);
    }
    let (rw, rh) = (src.width as f64, src.height as f64);
    if mnx > rw || mxx < 0.0 || mny > rh || mxy < 0.0 {
        return None;
    }
    let (mut cx0, mut cx1) = (mnx.max(0.0).trunc(), mxx.ceil().min(rw));
    let (mut cy0, mut cy1) = (mny.max(0.0).trunc(), mxy.ceil().min(rh));
    let mut xraw = (rw - cx0).min(mxx - mnx).max(0.0);
    let mut yraw = (rh - cy0).min(mxy - mny).max(0.0);
    // Covering more than 90 % of an axis → the whole axis.
    if cx1 - cx0 > 0.9 * rw {
        (cx0, cx1, xraw) = (0.0, rw, rw);
    }
    if cy1 - cy0 > 0.9 * rh {
        (cy0, cy1, yraw) = (0.0, rh, rh);
    }
    // Resampling padding: the bilinear radius, stretched when the chunk is coarser than the source.
    let radius = |n: usize, extent: f64| {
        let s = n as f64 / extent;
        if s < FOUR_SAMPLE_MIN_SCALE { (BILINEAR_RADIUS / s).ceil() } else { BILINEAR_RADIUS }
    };
    let (xr, yr) = (radius(cw, mxx - mnx), radius(ch, mxy - mny));
    let xoff = (cx0 - xr).max(0.0);
    let yoff = (cy0 - yr).max(0.0);
    let xsize = (rw - xoff).min(cx1 - xoff + xr);
    let ysize = (rh - yoff).min(cy1 - yoff + yr);
    let fill = xsize * ysize / ((mxx - mnx + 2.0 * xr) * (mxy - mny + 2.0 * yr)).max(1.0);
    let scale = |n: usize, raw: f64| if raw > 0.0 { reciprocal_round(n as f64 / raw) } else { 1.0 };
    Some(SourceWindow { fill, scale: (scale(cw, xraw), scale(ch, yraw)) })
}

const BILINEAR_RADIUS: f64 = 1.0;

/// `PerformWarp`: a downsampling scale within 0.05 of an integer reciprocal snaps to it.
fn reciprocal_round(s: f64) -> f64 {
    if s < 1.0 {
        let r = 1.0 / s;
        let n = (r + 0.5) as i64;
        if (r - n as f64).abs() < 0.05 {
            return 1.0 / n as f64;
        }
    }
    s
}

/// `GDALWarpOperation::CollectChunkListInternal` (memory is never the limit here): a chunk whose source fill
/// ratio is below 0.5 is halved along its longer side (height on ties), recursively; chunks that miss the
/// source are dropped.
fn collect_chunks(t: &Transformer, src: &SourceRaster, c: Chunk, out: &mut Vec<(Chunk, SourceWindow)>) {
    let (x0, y0, cw, ch) = c;
    let Some(win) = source_window(t, src, c) else { return };
    if win.fill > 0.0 && win.fill < 0.5 && (cw > 100 || ch > 100) {
        if cw > ch {
            let c1 = cw / 2;
            collect_chunks(t, src, (x0, y0, c1, ch), out);
            collect_chunks(t, src, (x0 + c1, y0, cw - c1, ch), out);
        } else {
            let c1 = ch / 2;
            collect_chunks(t, src, (x0, y0, cw, c1), out);
            collect_chunks(t, src, (x0, y0 + c1, cw, ch - c1), out);
        }
        return;
    }
    out.push((c, win));
}

/// Below this destination/source scale (either axis) GDAL uses the stretched kernel instead of 4 samples
/// (and pads the source window by the stretched radius).
const FOUR_SAMPLE_MIN_SCALE: f64 = 0.95;

/// Reprojects `src` onto the `w × h` EPSG:4326 grid covering `bounds` (as `rasterio.transform.from_bounds`),
/// bilinear, NaN = nodata.
pub fn warp_bilinear(src: &SourceRaster, bounds: Bounds, w: usize, h: usize) -> Result<Grid<f32>, UnsupportedCrs> {
    let dst_gt = [
        bounds.west(),
        (bounds.east() - bounds.west()) / w as f64,
        0.0,
        bounds.north(),
        0.0,
        (bounds.south() - bounds.north()) / h as f64,
    ];
    let t = Transformer::new(src, dst_gt)?;
    let mut out = vec![f32::NAN; w * h];
    let mut chunks = Vec::new();
    if w > 0 && h > 0 {
        collect_chunks(&t, src, (0, 0, w, h), &mut chunks);
    }
    for ((x0, y0, cw, ch), win) in chunks {
        let kernel = (win.scale.0 < FOUR_SAMPLE_MIN_SCALE || win.scale.1 < FOUR_SAMPLE_MIN_SCALE).then_some(win.scale);
        // The warp kernel works in chunk-local destination pixels: rows of `cw` points from x0.
        let local = Transformer { dst_gt: [dst_gt[0] + x0 as f64 * dst_gt[1], dst_gt[1], 0.0, dst_gt[3], 0.0, dst_gt[5]], ..t };
        let (mut sx, mut sy) = (vec![0.0; cw], vec![0.0; cw]);
        for y in y0..y0 + ch {
            approx_row(&local, y as f64 + 0.5, APPROX_TOLERANCE, &mut sx, &mut sy);
            for ((o, &x), &yy) in out[y * w + x0..y * w + x0 + cw].iter_mut().zip(&sx).zip(&sy) {
                *o = resample(src, x, yy, kernel);
            }
        }
    }
    Ok(Grid::from_vec(w, h, bounds, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A geographic source aligned with the destination: warping must be the identity.
    fn geo_src(w: usize, h: usize, data: Vec<f32>, bounds: Bounds) -> SourceRaster {
        let gt = [
            bounds.west(),
            (bounds.east() - bounds.west()) / w as f64,
            0.0,
            bounds.north(),
            0.0,
            (bounds.south() - bounds.north()) / h as f64,
        ];
        SourceRaster { width: w, height: h, data, geotransform: gt, epsg: 4326, nodata: None }
    }

    #[test]
    fn identity_on_aligned_grid() {
        let b = Bounds([0.0, 0.0, 4.0, 3.0]);
        let data: Vec<f32> = (0..12).map(|v| v as f32).collect();
        let g = warp_bilinear(&geo_src(4, 3, data.clone(), b), b, 4, 3).unwrap();
        assert_eq!(g.data, data);
    }

    #[test]
    fn bilinear_between_centres_and_nan_rules() {
        // Destination at twice the resolution: centres fall a quarter pixel from source centres.
        let b = Bounds([0.0, 0.0, 2.0, 2.0]);
        let src = geo_src(2, 2, vec![0.0, 4.0, 8.0, f32::NAN], b);
        let g = warp_bilinear(&src, b, 4, 4).unwrap();
        // Row 0 (top), x = 0.25 → 0 (edge clamp: only pixel 0 counts along x); x = 0.75: 0.75·0 + 0.25·4 = 1.
        assert_eq!(g.data[0], 0.0);
        assert_eq!(g.data[1], 1.0);
        // Row 1 (sy = 0.75), x = 0.75: weights 0.75·0.75 (0), 0.25·0.75 (4), 0.75·0.25 (8); the NaN corner dropped
        // and the rest renormalised: (0.1875·4 + 0.1875·8) / 0.9375 = 2.4.
        assert!((g.data[4 + 1] - 2.4).abs() < 1e-6, "{}", g.data[5]);
        // A destination pixel whose containing source pixel is NaN stays NaN even though neighbours are valid.
        assert!(g.data[2 * 4 + 2].is_nan());
        assert!(g.data[3 * 4 + 3].is_nan());
        // Outside the source → NaN.
        let g = warp_bilinear(&src, Bounds([-2.0, 0.0, 0.0, 2.0]), 2, 2).unwrap();
        assert!(g.data.iter().all(|v| v.is_nan()));
    }

    #[test]
    fn approx_transformer_close_to_exact() {
        // A UTM 34N source around Lake Koronia: the approximate rows stay within the tolerance of exact.
        let src = SourceRaster {
            width: 1,
            height: 1,
            data: vec![1.0],
            geotransform: [660_000.0, 20.0, 0.0, 4_515_200.0, 0.0, -20.0],
            epsg: 32634,
            nodata: None,
        };
        let b = Bounds([23.03, 40.62, 23.27, 40.74]);
        let gt = [b.west(), 0.0002, 0.0, b.north(), 0.0, -0.0002];
        let t = Transformer::new(&src, gt).unwrap();
        let (mut sx, mut sy) = (vec![0.0; 1200], vec![0.0; 1200]);
        for py in [0.5, 300.5, 599.5] {
            approx_row(&t, py, APPROX_TOLERANCE, &mut sx, &mut sy);
            let mut worst = 0.0f64;
            for (i, (&x, &y)) in sx.iter().zip(&sy).enumerate() {
                let (ex, ey) = t.apply(i as f64 + 0.5, py);
                worst = worst.max((x - ex).abs() + (y - ey).abs());
            }
            assert!(worst <= APPROX_TOLERANCE && worst > 0.0, "{worst}");
        }
    }

    #[test]
    fn stretched_kernel() {
        // Constant field: any normalised kernel returns the constant, NaNs are skipped.
        let b = Bounds([0.0, 0.0, 6.0, 6.0]);
        let mut d = vec![3.0f32; 36];
        d[14] = f32::NAN;
        let src = geo_src(6, 6, d, b);
        for (x, y) in [(2.3, 2.7), (0.2, 5.9), (3.5, 3.5)] {
            let v = kernel_resample(&src, x, y, (1.4, 0.8)).unwrap();
            assert!((v - 3.0).abs() < 1e-12, "{v}");
        }
        // A vertical ramp v = row: at scale 0.5 rows ±2 contribute with weights 1 − |j − δ|/2.
        let src = geo_src(1, 9, (0..9).map(|v| v as f32).collect(), Bounds([0.0, 0.0, 1.0, 9.0]));
        // sy = 4.75 → iy = 4, δ = 0.25: rows 3..6 weights 0.375, 0.875, 0.625, 0.125 → mean 4.25 (linear field).
        let v = kernel_resample(&src, 0.5, 4.75, (1.0, 0.5)).unwrap();
        assert!((v - 4.25).abs() < 1e-12, "{v}");
        // Scale ≥ 1: same as the 4-sample formula away from nodata.
        let v1 = kernel_resample(&src, 0.5, 4.75, (1.0, 1.0)).unwrap();
        assert!((v1 - bilinear4(&src, 0.5, 4.75).unwrap()).abs() < 1e-12);
    }

    #[test]
    fn scale_snapping() {
        assert_eq!(reciprocal_round(0.49), 0.5);
        assert_eq!(reciprocal_round(0.883), 0.883);
        assert_eq!(reciprocal_round(1.7), 1.7);
    }

    #[test]
    fn small_source_splits_the_destination() {
        // A source covering a third of the destination width: the fill ratio forces chunking, and the
        // chunks that miss the source are dropped, but the result equals the unchunked identity.
        let b = Bounds([0.0, 0.0, 300.0, 100.0]);
        let src = geo_src(100, 100, (0..10_000).map(|v| v as f32).collect(), Bounds([100.0, 0.0, 200.0, 100.0]));
        let t = Transformer::new(&src, [0.0, 1.0, 0.0, 100.0, 0.0, -1.0]).unwrap();
        let mut chunks = Vec::new();
        collect_chunks(&t, &src, (0, 0, 300, 100), &mut chunks);
        assert!(chunks.len() > 1);
        let g = warp_bilinear(&src, b, 300, 100).unwrap();
        assert_eq!(g.at(150, 40), &(40.0 * 100.0 + 50.0));
        assert!(g.at(50, 40).is_nan() && g.at(250, 40).is_nan());
    }

    #[test]
    fn rejects_unknown_crs() {
        let mut s = geo_src(1, 1, vec![1.0], Bounds([0.0, 0.0, 1.0, 1.0]));
        s.epsg = 3857;
        assert_eq!(warp_bilinear(&s, Bounds([0.0, 0.0, 1.0, 1.0]), 1, 1), Err(UnsupportedCrs(3857)));
    }
}
