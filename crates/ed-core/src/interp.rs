//! Filling the gaps between acquisitions.
//!
//! Satellite passes come every 5–35 days, at dates nobody chose. Three interpolants, for three jobs:
//!
//! * [`Lagrange`]: barycentric form of the polynomial interpolant (Berrut & Trefethen 2004). Stable to
//!   evaluate (O(n)), but it is still *the* degree-(n−1) polynomial through the nodes, so on equispaced or
//!   irregular nodes it oscillates (Runge): its Lebesgue constant grows like 2ⁿ/(e·n·ln n). Kept for
//!   reference and for nodes we can choose (Chebyshev); the tests show why it is not used on passes.
//! * [`FloaterHormann`]: the barycentric *rational* interpolant with blending degree `d` (Floater & Hormann
//!   2007). No poles on the real line, exact on polynomials of degree ≤ d, and a Lebesgue constant that grows
//!   only logarithmically on equispaced nodes. On irregular nodes it also grows with the local mesh ratio,
//!   so keep `d` small there: on the lake's passes (gaps of 5 to 45 days) leave-one-out RMSE is 2.3 km² for
//!   d ≤ 2 but 10.7 km² for d = 3 (`tests/gaps.rs`). [`SMOOTH_D`] is what the app uses.
//! * [`Pchip`]: monotone piecewise cubic Hermite (Fritsch & Carlson 1980, the derivative rule of scipy's
//!   `PchipInterpolator`). Never overshoots the data: right for bounded quantities such as a lake's area.
//!
//! For the lake there is something better than any interpolant: the water balance itself, run between two
//! passes on the real weather (see `forecast::bridge`).
//!
//! Interpolated values are estimates, and the app always draws them as such.

/// Blending degree for satellite time series (irregular passes).
pub const SMOOTH_D: usize = 2;

/// Common interface: evaluate at `x`.
pub trait Interpolant {
    fn eval(&self, x: f64) -> f64;
}

fn barycentric(xs: &[f64], ys: &[f64], w: &[f64], x: f64) -> f64 {
    let (mut num, mut den) = (0.0, 0.0);
    for ((xk, yk), wk) in xs.iter().zip(ys).zip(w) {
        let d = x - xk;
        if d == 0.0 {
            return *yk;
        }
        let t = wk / d;
        num += t * yk;
        den += t;
    }
    num / den
}

/// Polynomial interpolation in barycentric form ("second form").
#[derive(Clone, Debug)]
pub struct Lagrange {
    xs: Vec<f64>,
    ys: Vec<f64>,
    w: Vec<f64>,
}

impl Lagrange {
    pub fn new(xs: &[f64], ys: &[f64]) -> Self {
        assert_eq!(xs.len(), ys.len());
        // w_k = 1 / Π_{j≠k} (x_k − x_j), rescaled (the common factor cancels in the quotient)
        let mut w: Vec<f64> = (0..xs.len()).map(|k| 1.0 / xs.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, xj)| xs[k] - xj).product::<f64>()).collect();
        let m = w.iter().fold(0.0f64, |a, v| a.max(v.abs()));
        if m > 0.0 && m.is_finite() {
            for v in &mut w {
                *v /= m;
            }
        }
        Self { xs: xs.to_vec(), ys: ys.to_vec(), w }
    }
}

impl Interpolant for Lagrange {
    fn eval(&self, x: f64) -> f64 {
        barycentric(&self.xs, &self.ys, &self.w, x)
    }
}

/// Floater–Hormann barycentric rational interpolation with blending degree `d`.
#[derive(Clone, Debug)]
pub struct FloaterHormann {
    xs: Vec<f64>,
    ys: Vec<f64>,
    w: Vec<f64>,
}

impl FloaterHormann {
    /// `xs` strictly increasing; `d ≤ n − 1` (it is clamped). `d = 3` is a good default for smooth data.
    pub fn new(xs: &[f64], ys: &[f64], d: usize) -> Self {
        assert_eq!(xs.len(), ys.len());
        assert!(!xs.is_empty());
        let n = xs.len() - 1;
        let d = d.min(n);
        // w_k = (−1)^(k−d) Σ_{i ∈ J_k} Π_{j=i, j≠k}^{i+d} 1/|x_k − x_j|,  J_k = { i : max(0,k−d) ≤ i ≤ min(k, n−d) }
        let w = (0..=n)
            .map(|k| {
                let lo = k.saturating_sub(d);
                let hi = k.min(n - d);
                let s: f64 = (lo..=hi).map(|i| (i..=i + d).filter(|&j| j != k).map(|j| 1.0 / (xs[k] - xs[j]).abs()).product::<f64>()).sum();
                if (k + d).is_multiple_of(2) { s } else { -s }
            })
            .collect();
        Self { xs: xs.to_vec(), ys: ys.to_vec(), w }
    }
}

impl Interpolant for FloaterHormann {
    fn eval(&self, x: f64) -> f64 {
        barycentric(&self.xs, &self.ys, &self.w, x)
    }
}

/// Monotone piecewise cubic Hermite interpolation.
#[derive(Clone, Debug)]
pub struct Pchip {
    xs: Vec<f64>,
    ys: Vec<f64>,
    d: Vec<f64>,
}

impl Pchip {
    pub fn new(xs: &[f64], ys: &[f64]) -> Self {
        assert_eq!(xs.len(), ys.len());
        let n = xs.len();
        if n < 2 {
            return Self { xs: xs.to_vec(), ys: ys.to_vec(), d: vec![0.0; n] };
        }
        let h: Vec<f64> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        let m: Vec<f64> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / h[i]).collect();
        let mut d = vec![0.0; n];
        if n == 2 {
            d = vec![m[0], m[0]];
        } else {
            // interior: weighted harmonic mean of the neighbouring slopes, 0 at a local extremum
            for k in 1..n - 1 {
                if m[k - 1] * m[k] > 0.0 {
                    let w1 = 2.0 * h[k] + h[k - 1];
                    let w2 = h[k] + 2.0 * h[k - 1];
                    d[k] = (w1 + w2) / (w1 / m[k - 1] + w2 / m[k]);
                }
            }
            // ends: one-sided three-point formula, kept shape-preserving
            let end = |h0: f64, h1: f64, m0: f64, m1: f64| {
                let e = ((2.0 * h0 + h1) * m0 - h0 * m1) / (h0 + h1);
                if e.signum() != m0.signum() {
                    0.0
                } else if m0.signum() != m1.signum() && e.abs() > 3.0 * m0.abs() {
                    3.0 * m0
                } else {
                    e
                }
            };
            d[0] = end(h[0], h[1], m[0], m[1]);
            d[n - 1] = end(h[n - 2], h[n - 3], m[n - 2], m[n - 3]);
        }
        Self { xs: xs.to_vec(), ys: ys.to_vec(), d }
    }
}

impl Interpolant for Pchip {
    fn eval(&self, x: f64) -> f64 {
        let n = self.xs.len();
        if n == 1 {
            return self.ys[0];
        }
        let k = match self.xs.binary_search_by(|v| v.total_cmp(&x)) {
            Ok(i) => return self.ys[i],
            Err(i) => i.clamp(1, n - 1) - 1,
        };
        let h = self.xs[k + 1] - self.xs[k];
        let t = (x - self.xs[k]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.ys[k] + (t3 - 2.0 * t2 + t) * h * self.d[k] + (-2.0 * t3 + 3.0 * t2) * self.ys[k + 1] + (t3 - t2) * h * self.d[k + 1]
    }
}

/// Maximum error of an interpolant against a function on a fine grid over `[a, b]`.
pub fn max_error(f: impl Fn(f64) -> f64, p: &dyn Interpolant, a: f64, b: f64) -> f64 {
    (0..=2000).map(|i| a + (b - a) * f64::from(i) / 2000.0).map(|x| (f(x) - p.eval(x)).abs()).fold(0.0, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runge(x: f64) -> f64 {
        1.0 / (1.0 + 25.0 * x * x)
    }

    #[test]
    fn runge_phenomenon_lagrange_vs_floater_hormann() {
        // 21 equispaced nodes on [−1, 1]: the classic failure of polynomial interpolation
        let xs: Vec<f64> = (0..21).map(|i| -1.0 + 0.1 * f64::from(i)).collect();
        let ys: Vec<f64> = xs.iter().map(|&x| runge(x)).collect();
        let lag = max_error(runge, &Lagrange::new(&xs, &ys), -1.0, 1.0);
        let fh = max_error(runge, &FloaterHormann::new(&xs, &ys, 3), -1.0, 1.0);
        let pc = max_error(runge, &Pchip::new(&xs, &ys), -1.0, 1.0);
        println!("max error, 21 equispaced nodes: Lagrange {lag:.3e}  Floater–Hormann(d=3) {fh:.3e}  PCHIP {pc:.3e}");
        assert!(lag > 10.0, "Lagrange should blow up near the ends: {lag}");
        assert!(fh < 0.01 && pc < 0.05);
        // on Chebyshev nodes Lagrange is excellent: the nodes, not the barycentric form, were the problem
        let cx: Vec<f64> = (0..21).map(|k| (std::f64::consts::PI * f64::from(k) / 20.0).cos()).rev().collect();
        let cy: Vec<f64> = cx.iter().map(|&x| runge(x)).collect();
        assert!(max_error(runge, &Lagrange::new(&cx, &cy), -1.0, 1.0) < 0.02);
    }

    #[test]
    fn floater_hormann_reproduces_low_degree_polynomials_on_irregular_nodes() {
        let xs = [0.0, 5.0, 17.0, 24.0, 30.0, 42.0, 47.0, 59.0, 66.0, 90.0];
        let p = |x: f64| 2.0 - 0.3 * x + 0.01 * x * x - 1e-4 * x * x * x;
        let ys: Vec<f64> = xs.iter().map(|&x| p(x)).collect();
        let fh = FloaterHormann::new(&xs, &ys, 3);
        assert!(max_error(p, &fh, 0.0, 90.0) < 1e-9);
    }

    #[test]
    fn pchip_never_overshoots_a_falling_lake() {
        // a lake shrinking in steps, with a plateau: no value outside [min, max] of each interval
        let xs = [0.0, 25.0, 50.0, 87.0, 124.0, 136.0, 143.0, 160.0];
        let ys = [27.26, 27.25, 26.62, 25.31, 22.81, 22.48, 22.48, 9.2];
        let p = Pchip::new(&xs, &ys);
        for w in 0..xs.len() - 1 {
            let (lo, hi) = (ys[w].min(ys[w + 1]), ys[w].max(ys[w + 1]));
            for i in 0..=100 {
                let x = xs[w] + (xs[w + 1] - xs[w]) * f64::from(i) / 100.0;
                let v = p.eval(x);
                assert!(v >= lo - 1e-12 && v <= hi + 1e-12, "overshoot at {x}: {v}");
            }
        }
        // Lagrange through the same 8 points leaves the data range
        let lag = Lagrange::new(&xs, &ys);
        let worst = (0..=1600).map(|i| lag.eval(f64::from(i) / 10.0)).fold(f64::NEG_INFINITY, f64::max);
        assert!(worst > 27.26, "the polynomial bulges above the fullest lake: {worst}");
    }
}
