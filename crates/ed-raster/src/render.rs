//! Values → pixels: the legend colour ramp (`tools/colormap.py`), the soft footprint edge and the optical
//! pseudo-colour image of the legacy pipeline, with numpy's exact arithmetic (interpolation in f64, then
//! truncation by `astype(uint8)`).

use ed_core::{Grid, Rgba};

use crate::filters::gaussian;

/// A legend stop: physical value → sRGB colour.
pub type Stop = (f64, [u8; 3]);

/// "#rrggbb" (leading '#' optional) → [r, g, b].
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#').unwrap_or(s);
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([c(0)?, c(2)?, c(4)?])
}

/// `np.interp(x, xs, ys)` for increasing `xs`: clamps outside, exact `ys[j]` on a stop.
fn interp(x: f64, xs: &[f64], ys: &[f64], slopes: &[f64]) -> f64 {
    let n = xs.len();
    if x.is_nan() {
        return x;
    }
    if x < xs[0] {
        return ys[0];
    }
    if x >= xs[n - 1] {
        return ys[n - 1];
    }
    // j with xs[j] <= x < xs[j + 1].
    let j = xs.partition_point(|&v| v <= x) - 1;
    if xs[j] == x {
        return ys[j];
    }
    slopes[j] * (x - xs[j]) + ys[j]
}

/// Colours `values` with the legend ramp (`colormap.colorize`): per channel `np.interp` over the stops
/// (sorted by value), truncated to u8; alpha = `clip(alpha, 0, 1)·255` truncated (1 when `alpha` is `None`),
/// 0 where the value is NaN/infinite.
///
/// # Panics
/// If `stops` is empty or `alpha` has a different size than `values`.
pub fn colorize(values: &Grid<f32>, stops: &[Stop], alpha: Option<&Grid<f64>>) -> Rgba {
    assert!(!stops.is_empty(), "colorize: no legend stops");
    let mut s = stops.to_vec();
    s.sort_by(|a, b| a.0.total_cmp(&b.0));
    let xs: Vec<f64> = s.iter().map(|t| t.0).collect();
    let ch: [Vec<f64>; 3] = std::array::from_fn(|c| s.iter().map(|t| f64::from(t.1[c])).collect());
    let slopes: [Vec<f64>; 3] =
        std::array::from_fn(|c| xs.windows(2).zip(ch[c].windows(2)).map(|(x, y)| (y[1] - y[0]) / (x[1] - x[0])).collect());
    if let Some(a) = alpha {
        assert_eq!((a.w, a.h), (values.w, values.h), "colorize: alpha size");
    }
    let px = values
        .data
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let nodata = !v.is_finite();
            let x = if nodata { xs[0] } else { f64::from(v) };
            let rgb: [u8; 3] = std::array::from_fn(|c| interp(x, &xs, &ch[c], &slopes[c]) as u8);
            let a = if nodata { 0.0 } else { alpha.map_or(1.0, |a| a.data[i].clamp(0.0, 1.0)) };
            [rgb[0], rgb[1], rgb[2], (a * 255.0) as u8]
        })
        .collect();
    Rgba { w: values.w, h: values.h, bounds: values.bounds, px }
}

/// Soft edge along the border of a footprint: `clip(gaussian(valid, σ=3)·1.6 − 0.3, 0, 1)`.
pub fn edge_alpha(valid: &Grid<bool>) -> Grid<f64> {
    let g = gaussian(&valid.map(|&v| f64::from(u8::from(v))), 3.0);
    g.map(|&v| (v * 1.6 - 0.3).clamp(0.0, 1.0))
}

/// Pseudo-natural colour from red + NIR only (`optical_image` in the legacy script): vegetation green,
/// water dark. Computed in f32 as numpy did; alpha = `edge_alpha(valid)·255` truncated.
///
/// # Panics
/// If the two bands differ in size.
pub fn optical_image(red: &Grid<f32>, nir: &Grid<f32>) -> Rgba {
    assert_eq!((red.w, red.h), (nir.w, nir.h), "optical_image: band sizes");
    let valid = Grid::from_vec(red.w, red.h, red.bounds, red.data.iter().zip(&nir.data).map(|(r, n)| r.is_finite() && n.is_finite()).collect());
    let alpha = edge_alpha(&valid);
    let to_u8 = |v: f32| if v.is_nan() { 0 } else { (v * 255.0) as u8 };
    let px = red
        .data
        .iter()
        .zip(&nir.data)
        .zip(&alpha.data)
        .map(|((&rd, &ni), &a)| {
            let r = (rd / 2200.0).clamp(0.0, 1.0).powf(0.75);
            let n = (ni / 4200.0).clamp(0.0, 1.0).powf(0.75);
            [to_u8(r), to_u8(0.55 * r + 0.45 * n), to_u8(0.85 * r + 0.08 * n), (a * 255.0) as u8]
        })
        .collect();
    Rgba { w: red.w, h: red.h, bounds: red.bounds, px }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed_core::Bounds;

    const B: Bounds = Bounds([0.0, 0.0, 1.0, 1.0]);

    #[test]
    fn hex() {
        assert_eq!(parse_hex("#05070d"), Some([5, 7, 13]));
        assert_eq!(parse_hex("EEF2F7"), Some([0xee, 0xf2, 0xf7]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#zz0000"), None);
    }

    #[test]
    fn colorize_interpolates_truncates_and_clamps() {
        // The hh legend of the real data set.
        let stops = [(-26.0, [5, 7, 13]), (-18.0, [42, 53, 80]), (-11.0, [138, 151, 173]), (-4.0, [238, 242, 247])];
        let v = Grid::from_vec(6, 1, B, vec![-30.0, -26.0, -22.0, -14.5, -4.0, f32::NAN]);
        let a = Grid::from_vec(6, 1, B, vec![1.0, 0.5, 2.0, -1.0, 0.999, 1.0]);
        let img = colorize(&v, &stops, Some(&a));
        assert_eq!(img.px[0], [5, 7, 13, 255]); // below the first stop: clamped
        assert_eq!(img.px[1], [5, 7, 13, 127]); // exactly on a stop; 0.5·255 = 127.5 → 127
        // Halfway between −26 and −18: (5+42)/2 = 23.5 → 23, (7+53)/2 = 30, (13+80)/2 = 46.5 → 46.
        assert_eq!(img.px[2], [23, 30, 46, 255]);
        // Halfway between −18 and −11: 90, 102, 126.5 → 126; alpha clipped to 0.
        assert_eq!(img.px[3], [90, 102, 126, 0]);
        assert_eq!(img.px[4], [238, 242, 247, 254]); // 0.999·255 = 254.7 → 254
        assert_eq!(img.px[5], [5, 7, 13, 0]); // nodata: first stop colour, transparent
        // Unsorted stops are sorted first; no alpha = opaque.
        let rev: Vec<Stop> = stops.iter().rev().copied().collect();
        assert_eq!(colorize(&v, &rev, None).px[2], [23, 30, 46, 255]);
    }

    #[test]
    fn edge_alpha_inside_and_outside() {
        let w = 40;
        let valid = Grid::from_vec(w, 1, B, (0..w).map(|x| x < 20).collect());
        let a = edge_alpha(&valid);
        assert_eq!(a.data[0], 1.0); // deep inside
        assert_eq!(a.data[w - 1], 0.0); // far outside
        // In between it is the affine map of the blur.
        assert!((a.data[19] - (gaussian(&valid.map(|&v| f64::from(u8::from(v))), 3.0).data[19] * 1.6 - 0.3)).abs() < 1e-15);
        assert!(a.data.windows(2).all(|p| p[0] >= p[1]));
    }

    #[test]
    fn optical_colours() {
        let red = Grid::from_vec(3, 1, B, vec![2200.0, 0.0, f32::NAN]);
        let nir = Grid::from_vec(3, 1, B, vec![0.0, 4200.0, 100.0]);
        let img = optical_image(&red, &nir);
        assert_eq!(&img.px[0][..3], &[255, 140, 216]); // r = 1: 0.55·255 = 140.25, 0.85·255 = 216.75
        assert_eq!(&img.px[1][..3], &[0, 114, 20]); // n = 1: 0.45·255 = 114.75, 0.08·255 = 20.4
        assert_eq!(&img.px[2][..3], &[0, 0, 0]); // NaN → 0
    }
}
