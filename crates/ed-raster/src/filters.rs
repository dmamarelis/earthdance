//! The `scipy.ndimage` filters the legacy pipeline used, reproduced to the last bit where it matters.
//!
//! * [`median3`] — `median_filter(x, size=3)` (mode `reflect`: `d c b a | a b c d`).
//! * [`gaussian`] — `gaussian_filter(x, sigma)` (truncate 4.0, mode `reflect`), separable, axis 0 then axis 1,
//!   with scipy's kernel construction (`exp(-0.5/σ² · x²)` normalised by numpy's pairwise sum) and the
//!   symmetric-kernel loop of `NI_Correlate1D`, all in f64.
//! * [`dilate_cross`] — `binary_dilation(mask, iterations=k)` with the default cross structuring element and
//!   `border_value=0`.
//! * [`label4`] / [`largest_component`] — `label(mask)` (default 4-connectivity), labels numbered in raster
//!   order of each component's first pixel, as scipy numbers them.

use std::collections::VecDeque;


use ed_core::Grid;

/// `scipy.ndimage` mode `reflect`: index `i` folded into `0..n` by half-sample symmetric reflection.
#[inline]
pub fn reflect(i: isize, n: usize) -> usize {
    let n = n as isize;
    let p = 2 * n;
    let m = i.rem_euclid(p);
    (if m >= n { p - 1 - m } else { m }) as usize
}

/// 3×3 median, `reflect` borders. The input must not contain NaN (the legacy pipeline fills invalid pixels
/// first, see [`crate::koroneia`]); NaNs would sort last rather than propagate.
pub fn median3(g: &Grid<f32>) -> Grid<f32> {
    let (w, h) = (g.w, g.h);
    let mut out = Vec::with_capacity(w * h);
    let xs: Vec<[usize; 3]> = (0..w as isize).map(|x| [reflect(x - 1, w), x as usize, reflect(x + 1, w)]).collect();
    for y in 0..h as isize {
        let rows = [reflect(y - 1, h), y as usize, reflect(y + 1, h)].map(|r| &g.data[r * w..(r + 1) * w]);
        for c in &xs {
            let mut v = [0f32; 9];
            for (k, row) in rows.iter().enumerate() {
                v[3 * k] = row[c[0]];
                v[3 * k + 1] = row[c[1]];
                v[3 * k + 2] = row[c[2]];
            }
            out.push(median9(v));
        }
    }
    Grid::from_vec(w, h, g.bounds, out)
}

/// Median of 9 by Paeth's 19-exchange network (exact selection: the result is one of the inputs).
#[inline]
fn median9(mut p: [f32; 9]) -> f32 {
    let mut s = |a: usize, b: usize| {
        let (x, y) = (p[a], p[b]);
        p[a] = x.min(y);
        p[b] = x.max(y);
    };
    s(1, 2);
    s(4, 5);
    s(7, 8);
    s(0, 1);
    s(3, 4);
    s(6, 7);
    s(1, 2);
    s(4, 5);
    s(7, 8);
    s(0, 3);
    s(5, 8);
    s(4, 7);
    s(3, 6);
    s(1, 4);
    s(2, 5);
    s(4, 7);
    s(4, 2);
    s(6, 4);
    s(4, 2);
    p[4]
}

/// numpy's `pairwise_sum` for a short contiguous f64 array (n ≤ 128: eight interleaved accumulators).
fn numpy_sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        return a.iter().fold(0.0, |s, &v| s + v);
    }
    if n <= 128 {
        let mut r = [0.0; 8];
        r.copy_from_slice(&a[..8]);
        let mut i = 8;
        while i + 8 <= n {
            for (k, rk) in r.iter_mut().enumerate() {
                *rk += a[i + k];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        for &v in &a[i..] {
            res += v;
        }
        return res;
    }
    let m = (n / 2) - (n / 2) % 8;
    numpy_sum(&a[..m]) + numpy_sum(&a[m..])
}

/// scipy's `_gaussian_kernel1d(sigma, 0, radius)` with `radius = int(truncate·σ + 0.5)`, truncate = 4.
pub fn gaussian_kernel(sigma: f64) -> Vec<f64> {
    let radius = (4.0 * sigma + 0.5) as i64;
    let c = -0.5 / (sigma * sigma);
    let phi: Vec<f64> = (-radius..=radius).map(|x| (c * (x * x) as f64).exp()).collect();
    let s = numpy_sum(&phi);
    phi.into_iter().map(|p| p / s).collect()
}

/// `correlate1d` of one line with a symmetric odd kernel (`NI_Correlate1D`'s symmetric branch).
fn correlate_line(line: &[f64], k: &[f64], out: &mut [f64], ext: &mut Vec<f64>) {
    let n = line.len();
    let r = k.len() / 2;
    ext.clear();
    ext.extend((-(r as isize)..(n + r) as isize).map(|i| line[reflect(i, n)]));
    // Per output i (centre ext[i + r]): acc = c·k[r], then the symmetric pairs, outermost first —
    // scipy's operation order, evaluated for the whole line one tap at a time (contiguous, vectorisable).
    for (o, &c) in out.iter_mut().zip(&ext[r..r + n]) {
        *o = c * k[r];
    }
    for jj in (1..=r).rev() {
        let kw = k[r - jj];
        for ((o, &a), &b) in out.iter_mut().zip(&ext[r - jj..r - jj + n]).zip(&ext[r + jj..r + jj + n]) {
            *o += (a + b) * kw;
        }
    }
}

/// Separable Gaussian blur over f64 values (scipy `gaussian_filter`, `reflect` borders, truncate 4.0).
pub fn gaussian(g: &Grid<f64>, sigma: f64) -> Grid<f64> {
    let (w, h) = (g.w, g.h);
    let k = gaussian_kernel(sigma);
    let mut ext = Vec::new();
    // Axis 0 (along y), a whole row at a time: per pixel the same operations in the same order as
    // `correlate_line`, so the result is identical, but the inner loop runs over contiguous memory.
    let r = k.len() / 2;
    let mut tmp = vec![0.0; w * h];
    for (y, dst) in tmp.chunks_exact_mut(w).enumerate() {
        let row = |j: isize| &g.data[reflect(y as isize + j, h) * w..][..w];
        for (d, &c) in dst.iter_mut().zip(row(0)) {
            *d = c * k[r];
        }
        for jj in (1..=r).rev() {
            let (up, down, kw) = (row(-(jj as isize)), row(jj as isize), k[r - jj]);
            for ((d, &a), &b) in dst.iter_mut().zip(up).zip(down) {
                *d += (a + b) * kw;
            }
        }
    }
    // Axis 1 (rows, along x).
    let mut out = vec![0.0; w * h];
    for (src, dst) in tmp.chunks_exact(w).zip(out.chunks_exact_mut(w)) {
        correlate_line(src, &k, dst, &mut ext);
    }
    Grid::from_vec(w, h, g.bounds, out)
}

/// Binary dilation by the 4-neighbour cross, `iterations` times, outside the grid = false.
///
/// Inside a rectangle this is exactly "within Manhattan distance `iterations` of a set pixel", computed here
/// by one breadth-first pass instead of `iterations` full sweeps.
pub fn dilate_cross(m: &Grid<bool>, iterations: usize) -> Grid<bool> {
    let (w, h) = (m.w, m.h);
    let mut dist = vec![u32::MAX; w * h];
    let mut q = VecDeque::new();
    for (i, &v) in m.data.iter().enumerate() {
        if v {
            dist[i] = 0;
            q.push_back(i);
        }
    }
    while let Some(i) = q.pop_front() {
        let d = dist[i];
        if d as usize >= iterations {
            continue;
        }
        let (x, y) = (i % w, i / w);
        let mut visit = |j: usize| {
            if dist[j] == u32::MAX {
                dist[j] = d + 1;
                q.push_back(j);
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < w {
            visit(i + 1);
        }
        if y > 0 {
            visit(i - w);
        }
        if y + 1 < h {
            visit(i + w);
        }
    }
    Grid::from_vec(w, h, m.bounds, dist.into_iter().map(|d| d != u32::MAX).collect())
}

/// 4-connected component labels (0 = background, 1..=n in raster order of first pixel) and n.
pub fn label4(m: &Grid<bool>) -> (Vec<u32>, u32) {
    let (w, h) = (m.w, m.h);
    let mut lab = vec![0u32; w * h];
    let mut n = 0;
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !m.data[start] || lab[start] != 0 {
            continue;
        }
        n += 1;
        lab[start] = n;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            let nb = [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then(|| i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then(|| i + w),
            ];
            for j in nb.into_iter().flatten() {
                if m.data[j] && lab[j] == 0 {
                    lab[j] = n;
                    stack.push(j);
                }
            }
        }
    }
    (lab, n)
}

/// The largest 4-connected component (ties → the lowest label, like `bincount(...).argmax()`), or `None`
/// if the mask is empty.
pub fn largest_component(m: &Grid<bool>) -> Option<Grid<bool>> {
    let (lab, n) = label4(m);
    if n == 0 {
        return None;
    }
    let mut count = vec![0usize; n as usize + 1];
    for &l in &lab {
        count[l as usize] += 1;
    }
    let mut best = 1;
    for l in 2..=n as usize {
        if count[l] > count[best] {
            best = l;
        }
    }
    Some(Grid::from_vec(m.w, m.h, m.bounds, lab.into_iter().map(|l| l as usize == best).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed_core::Bounds;

    const B: Bounds = Bounds([0.0, 0.0, 1.0, 1.0]);

    fn g<T: Clone>(w: usize, h: usize, d: Vec<T>) -> Grid<T> {
        Grid::from_vec(w, h, B, d)
    }

    #[test]
    fn reflect_indices() {
        let n = 4;
        let got: Vec<usize> = (-5..9).map(|i| reflect(i, n)).collect();
        // … b a | a b c d | d c b a | a …
        assert_eq!(got, vec![3, 3, 2, 1, 0, 0, 1, 2, 3, 3, 2, 1, 0, 0]);
        assert_eq!(reflect(-1, 1), 0);
        assert_eq!(reflect(5, 1), 0);
    }

    #[test]
    fn gaussian_matches_scipy_docs() {
        // scipy.ndimage.gaussian_filter1d([1, 2, 3, 4, 5], 1) and (…, 4), from the scipy documentation.
        let row = g(5, 1, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        let one = gaussian(&row, 1.0);
        for (a, b) in one.data.iter().zip([1.427_040_95, 2.067_822_03, 3.0, 3.932_177_97, 4.572_959_05]) {
            assert!((a - b).abs() < 1e-8, "{a} vs {b}");
        }
        // σ = 4 → radius 16 on a 5-sample line: exercises repeated reflections.
        let four = gaussian(&row, 4.0);
        for (a, b) in four.data.iter().zip([2.919_483_43, 2.950_235_02, 3.0, 3.049_764_98, 3.080_516_57]) {
            assert!((a - b).abs() < 1e-8, "{a} vs {b}");
        }
        // Separable: a column behaves like a row.
        let col = gaussian(&g(1, 5, vec![1.0, 2.0, 3.0, 4.0, 5.0]), 1.0);
        assert_eq!(col.data, one.data);
    }

    #[test]
    fn gaussian_kernel_shape() {
        let k = gaussian_kernel(3.0);
        assert_eq!(k.len(), 2 * 12 + 1);
        assert!((k.iter().sum::<f64>() - 1.0).abs() < 1e-15);
        assert_eq!(k[0], k[24]);
        assert_eq!(gaussian_kernel(4.0).len(), 33);
    }

    #[test]
    fn median_network_selects_the_median() {
        let mut state = 0x2545_f491_u32;
        for _ in 0..10_000 {
            let v: [f32; 9] = std::array::from_fn(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as f32 - 128.0 // small range: plenty of ties
            });
            let mut s = v;
            s.sort_by(f32::total_cmp);
            assert_eq!(median9(v), s[4], "{v:?}");
        }
    }

    #[test]
    fn gaussian_2d_is_separable_and_transpose_symmetric() {
        let (w, h) = (7, 5);
        let d: Vec<f64> = (0..w * h).map(|i| ((i * 37) % 11) as f64).collect();
        let t: Vec<f64> = (0..w * h).map(|i| d[(i % h) * w + i / h]).collect();
        let a = gaussian(&g(w, h, d), 1.5);
        let b = gaussian(&g(h, w, t), 1.5);
        for y in 0..h {
            for x in 0..w {
                assert!((a.at(x, y) - b.at(y, x)).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn median_reflect_borders() {
        // Hand-computed scipy.ndimage.median_filter(a, size=3) on 1..9 (reflect = edge rows/cols repeated).
        let m = median3(&g(3, 3, (1..=9).map(|v| v as f32).collect()));
        assert_eq!(m.data, vec![2.0, 3.0, 3.0, 4.0, 5.0, 6.0, 7.0, 7.0, 8.0]);
        // A single spike vanishes, a constant stays.
        let mut d = vec![1.0f32; 25];
        d[12] = 100.0;
        assert!(median3(&g(5, 5, d)).data.iter().all(|&v| v == 1.0));
    }

    #[test]
    fn dilation_is_a_clipped_diamond() {
        let mut d = vec![false; 7 * 5];
        d[2 * 7 + 3] = true;
        let r = dilate_cross(&g(7, 5, d.clone()), 2);
        let txt: String = r.data.chunks(7).map(|row| row.iter().map(|&b| if b { '#' } else { '.' }).collect::<String>() + "\n").collect();
        assert_eq!(txt, "...#...\n..###..\n.#####.\n..###..\n...#...\n");
        // At the border: nothing wraps; zero iterations = identity.
        let mut c = vec![false; 9];
        c[0] = true;
        assert_eq!(dilate_cross(&g(3, 3, c.clone()), 1).data, vec![true, true, false, true, false, false, false, false, false]);
        assert_eq!(dilate_cross(&g(3, 3, c.clone()), 0).data, c);
    }

    #[test]
    fn labels_four_connected_in_raster_order() {
        #[rustfmt::skip]
        let d = vec![
            0, 1, 0, 0, 1,
            1, 0, 0, 1, 1,
            1, 1, 0, 0, 0,
            0, 0, 1, 0, 1,
        ].into_iter().map(|v| v == 1).collect();
        let m = g(5, 4, d);
        let (lab, n) = label4(&m);
        // Diagonal contacts do not connect.
        assert_eq!(n, 5);
        #[rustfmt::skip]
        assert_eq!(lab, vec![
            0, 1, 0, 0, 2,
            3, 0, 0, 2, 2,
            3, 3, 0, 0, 0,
            0, 0, 4, 0, 5,
        ]);
        let big = largest_component(&m).unwrap();
        // Labels 2 and 3 both have 3 pixels: the lower label wins.
        assert_eq!(big.data.iter().filter(|&&b| b).count(), 3);
        assert!(big.data[4] && big.data[8] && big.data[9]);
        assert!(largest_component(&g(2, 1, vec![false, false])).is_none());
    }
}
