//! Small statistics and the two numerical solvers the forecast fit needs.

use crate::Date;

pub fn sum(a: &[f64]) -> f64 {
    a.iter().sum()
}

/// Mean; 0 for an empty slice (the legacy `sum / (n || 1)`).
pub fn mean(a: &[f64]) -> f64 {
    sum(a) / a.len().max(1) as f64
}

/// The legacy "median": the element at `floor(n/2)` of the sorted copy (upper median for even n).
pub fn median(a: &[f64]) -> f64 {
    let mut s = a.to_vec();
    s.sort_by(f64::total_cmp);
    s[s.len() / 2]
}

pub fn max(a: &[f64]) -> f64 {
    a.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}
pub fn min(a: &[f64]) -> f64 {
    a.iter().copied().fold(f64::INFINITY, f64::min)
}

/// First index of the maximum (`a.indexOf(Math.max(...a))`).
pub fn argmax(a: &[f64]) -> usize {
    let m = max(a);
    a.iter().position(|&v| v == m).unwrap_or(0)
}

/// Least-squares slope of `vals` against time, per year (365.25 days).
pub fn slope_per_year(dates: &[Date], vals: &[f64]) -> f64 {
    let Some(&d0) = dates.first() else { return 0.0 };
    let x: Vec<f64> = dates.iter().map(|d| f64::from(d.since(d0)) / 365.25).collect();
    let (mx, my) = (mean(&x), mean(vals));
    let den: f64 = x.iter().map(|xi| (xi - mx).powi(2)).sum();
    if den == 0.0 {
        return 0.0;
    }
    x.iter().zip(vals).map(|(xi, v)| (xi - mx) * (v - my)).sum::<f64>() / den
}

/// Pearson correlation; 0 when either side has no variance.
pub fn corr(a: &[f64], b: &[f64]) -> f64 {
    let (ma, mb) = (mean(a), mean(b));
    let den = (a.iter().map(|v| (v - ma).powi(2)).sum::<f64>() * b.iter().map(|v| (v - mb).powi(2)).sum::<f64>()).sqrt();
    if den == 0.0 {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum::<f64>() / den
}

/// Linear-interpolated percentile of an unsorted slice, `p` in 0..1 (`pct()` in forecast.js, numpy's default).
pub fn percentile(a: &[f64], p: f64) -> f64 {
    let mut s = a.to_vec();
    s.sort_by(f64::total_cmp);
    percentile_sorted(&s, p)
}

pub fn percentile_sorted(s: &[f64], p: f64) -> f64 {
    let i = (s.len() - 1) as f64 * p;
    let lo = i.floor() as usize;
    let hi = i.ceil() as usize;
    s[lo] + (s[hi] - s[lo]) * (i - lo as f64)
}

/// R², RMSE and mean bias of `y` (measured) against `x` (reference), as the validation sheet shows them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Agreement {
    pub n: usize,
    pub r2: f64,
    pub rmse: f64,
    pub bias: f64,
}

pub fn agreement(x: &[f64], y: &[f64]) -> Agreement {
    let (mx, my) = (mean(x), mean(y));
    let sxy: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    let sxx: f64 = x.iter().map(|a| (a - mx).powi(2)).sum();
    let syy: f64 = y.iter().map(|b| (b - my).powi(2)).sum();
    let r2 = sxy.powi(2) / (sxx * syy);
    let se: Vec<f64> = x.iter().zip(y).map(|(a, b)| (b - a).powi(2)).collect();
    Agreement { n: x.len(), r2, rmse: mean(&se).sqrt(), bias: my - mx }
}

// ───────────────────────────────────────────────────────── Brent root finding

/// A root of `f` in `[a, b]` where `f(a)` and `f(b)` have opposite signs (Brent 1973, as scipy's `brentq`:
/// bisection, secant and inverse quadratic interpolation, never worse than bisection).
pub fn brent(mut f: impl FnMut(f64) -> f64, a: f64, b: f64, xtol: f64, max_iter: usize) -> Option<f64> {
    let (mut a, mut b) = (a, b);
    let (mut fa, mut fb) = (f(a), f(b));
    if fa == 0.0 {
        return Some(a);
    }
    if fb == 0.0 {
        return Some(b);
    }
    if fa.signum() == fb.signum() {
        return None;
    }
    let (mut c, mut fc) = (a, fa);
    let mut d = b - a;
    let mut e = d;
    for _ in 0..max_iter {
        if fb.signum() == fc.signum() {
            c = a;
            fc = fa;
            d = b - a;
            e = d;
        }
        if fc.abs() < fb.abs() {
            (a, b, c) = (b, c, b);
            (fa, fb, fc) = (fb, fc, fb);
        }
        let tol = 2.0 * f64::EPSILON * b.abs() + 0.5 * xtol;
        let m = 0.5 * (c - b);
        if m.abs() <= tol || fb == 0.0 {
            return Some(b);
        }
        if e.abs() >= tol && fa.abs() > fb.abs() {
            let s = fb / fa;
            let (mut p, mut q);
            if a == c {
                p = 2.0 * m * s;
                q = 1.0 - s;
            } else {
                let qa = fa / fc;
                let r = fb / fc;
                p = s * (2.0 * m * qa * (qa - r) - (b - a) * (r - 1.0));
                q = (qa - 1.0) * (r - 1.0) * (s - 1.0);
            }
            if p > 0.0 {
                q = -q;
            } else {
                p = -p;
            }
            if 2.0 * p < (3.0 * m * q - (tol * q).abs()).min((e * q).abs()) {
                e = d;
                d = p / q;
            } else {
                d = m;
                e = m;
            }
        } else {
            d = m;
            e = m;
        }
        a = b;
        fa = fb;
        b += if d.abs() > tol { d } else { tol.copysign(m) };
        fb = f(b);
    }
    Some(b)
}

// ───────────────────────────────────────────────────────── bounded nonlinear least squares

/// Result of [`least_squares`].
#[derive(Clone, Debug)]
pub struct LsqResult {
    pub x: Vec<f64>,
    /// Residuals at `x`.
    pub fun: Vec<f64>,
    /// Jacobian at `x`, row-major `m × n`.
    pub jac: Vec<f64>,
    pub iterations: usize,
}

/// Minimises `½·Σ r(x)²` subject to `lo ≤ x ≤ hi`: a port of scipy's `least_squares(method='trf')` with its
/// defaults (`'2-point'` Jacobian, `x_scale=1`, `ftol = xtol = gtol = 1e-8`, exact trust-region solver).
///
/// Trust Region Reflective (Branch, Coleman & Li 1999): the bounds enter through the Coleman–Li scaling
/// `v` (distance to the bound the gradient points at), so the iterates stay strictly inside the box and slow
/// down near a bound instead of jumping onto it. Matching this matters on flat problems such as the
/// back-test fit, where where the solver stops *is* the answer.
pub fn least_squares(mut r: impl FnMut(&[f64]) -> Vec<f64>, x0: &[f64], lo: &[f64], hi: &[f64], max_nfev: usize) -> LsqResult {
    let n = x0.len();
    let (ftol, xtol, gtol) = (1e-8, 1e-8, 1e-8);
    let max_nfev = max_nfev.max(1);
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let norm = |a: &[f64]| dot(a, a).sqrt();

    let mut x = make_strictly_feasible(x0, lo, hi, 1e-10);
    let mut f = r(&x);
    let m = f.len();
    let mut nfev = 1;
    let jac = |r: &mut dyn FnMut(&[f64]) -> Vec<f64>, x: &[f64], f: &[f64]| -> Vec<f64> {
        let rstep = f64::EPSILON.sqrt();
        let mut j = vec![0.0; m * n];
        for c in 0..n {
            let sign = if x[c] >= 0.0 { 1.0 } else { -1.0 };
            let mut h = rstep * sign * x[c].abs().max(1.0);
            // one-sided scheme adjusted to the bounds: flip the step when it would leave the box
            let xh = x[c] + h;
            if (xh < lo[c] || xh > hi[c]) && h.abs() <= (lo[c] - x[c]).max(hi[c] - x[c]) {
                h = -h;
            }
            let mut xs = x.to_vec();
            xs[c] = x[c] + h;
            let dx = xs[c] - x[c];
            let fh = r(&xs);
            for row in 0..m {
                j[row * n + c] = (fh[row] - f[row]) / dx;
            }
        }
        j
    };
    let mut j = jac(&mut r, &x, &f);
    let grad = |j: &[f64], f: &[f64]| -> Vec<f64> { (0..n).map(|c| (0..m).map(|row| j[row * n + c] * f[row]).sum()).collect() };
    let mut cost = 0.5 * dot(&f, &f);
    let mut g = grad(&j, &f);

    let (v0, _) = cl_scaling(&x, &g, lo, hi);
    let mut delta = norm(&x.iter().zip(&v0).map(|(xi, vi)| xi / vi.sqrt()).collect::<Vec<_>>());
    if delta == 0.0 {
        delta = 1.0;
    }
    let mut alpha = 0.0;
    let mut status = None;
    let mut it = 0;
    loop {
        let (v, dv) = cl_scaling(&x, &g, lo, hi);
        let g_norm = g.iter().zip(&v).map(|(a, b)| (a * b).abs()).fold(0.0, f64::max);
        if g_norm < gtol {
            status = Some(1);
        }
        if status.is_some() || nfev >= max_nfev {
            break;
        }
        let d: Vec<f64> = v.iter().map(|x| x.sqrt()).collect();
        let diag_h: Vec<f64> = g.iter().zip(&dv).map(|(a, b)| a * b).collect();
        let g_h: Vec<f64> = d.iter().zip(&g).map(|(a, b)| a * b).collect();
        let j_h: Vec<f64> = (0..m * n).map(|i| j[i] * d[i % n]).collect();

        // SVD of [J_h; diag(√diag_h)] through the eigen-decomposition of its normal matrix (n is tiny)
        let mut ata = vec![0.0; n * n];
        for p in 0..n {
            for q in 0..n {
                ata[p * n + q] = (0..m).map(|row| j_h[row * n + p] * j_h[row * n + q]).sum::<f64>() + if p == q { diag_h[p] } else { 0.0 };
            }
        }
        let (mut ev, mut vecs) = eig_sym(&ata, n);
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| ev[b].total_cmp(&ev[a]));
        ev = order.iter().map(|&k| ev[k]).collect();
        vecs = (0..n * n).map(|i| vecs[(i / n) * n + order[i % n]]).collect(); // column k = k-th largest
        let s: Vec<f64> = ev.iter().map(|e| e.max(0.0).sqrt()).collect();
        let jtf: Vec<f64> = (0..n).map(|c| (0..m).map(|row| j_h[row * n + c] * f[row]).sum()).collect();
        let uf: Vec<f64> = (0..n).map(|k| if s[k] > 0.0 { (0..n).map(|p| vecs[p * n + k] * jtf[p]).sum::<f64>() / s[k] } else { 0.0 }).collect();

        let theta = 0.995f64.max(1.0 - g_norm);
        let mut actual_reduction = -1.0;
        let (mut x_new, mut f_new, mut cost_new) = (x.clone(), f.clone(), cost);
        while actual_reduction <= 0.0 && nfev < max_nfev {
            let (p_h, a) = solve_lsq_trust_region(n, m, &uf, &s, &vecs, delta, alpha);
            alpha = a;
            let p: Vec<f64> = d.iter().zip(&p_h).map(|(a, b)| a * b).collect();
            let (step, step_h, predicted) = select_step(&x, &j_h, m, &diag_h, &g_h, p, p_h, &d, delta, lo, hi, theta);
            x_new = make_strictly_feasible(&x.iter().zip(&step).map(|(a, b)| a + b).collect::<Vec<_>>(), lo, hi, 0.0);
            f_new = r(&x_new);
            nfev += 1;
            let step_h_norm = norm(&step_h);
            if !f_new.iter().all(|v| v.is_finite()) {
                delta = 0.25 * step_h_norm;
                continue;
            }
            cost_new = 0.5 * dot(&f_new, &f_new);
            actual_reduction = cost - cost_new;
            let ratio = if predicted > 0.0 {
                actual_reduction / predicted
            } else if predicted == 0.0 && actual_reduction == 0.0 {
                1.0
            } else {
                0.0
            };
            let delta_new = if ratio < 0.25 {
                0.25 * step_h_norm
            } else if ratio > 0.75 && step_h_norm > 0.95 * delta {
                delta * 2.0
            } else {
                delta
            };
            let step_norm = norm(&step);
            let ftol_ok = actual_reduction < ftol * cost && ratio > 0.25;
            let xtol_ok = step_norm < xtol * (xtol + norm(&x));
            status = match (ftol_ok, xtol_ok) {
                (true, true) => Some(4),
                (true, false) => Some(2),
                (false, true) => Some(3),
                _ => None,
            };
            if status.is_some() {
                break;
            }
            alpha *= delta / delta_new;
            delta = delta_new;
        }
        if actual_reduction > 0.0 {
            x = x_new;
            f = f_new;
            cost = cost_new;
            j = jac(&mut r, &x, &f);
            g = grad(&j, &f);
        }
        it += 1;
    }
    LsqResult { x, fun: f, jac: j, iterations: it }
}

fn cl_scaling(x: &[f64], g: &[f64], lo: &[f64], hi: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let mut v = vec![1.0; x.len()];
    let mut dv = vec![0.0; x.len()];
    for i in 0..x.len() {
        if g[i] < 0.0 && hi[i].is_finite() {
            v[i] = hi[i] - x[i];
            dv[i] = -1.0;
        }
        if g[i] > 0.0 && lo[i].is_finite() {
            v[i] = x[i] - lo[i];
            dv[i] = 1.0;
        }
    }
    (v, dv)
}

fn make_strictly_feasible(x: &[f64], lo: &[f64], hi: &[f64], rstep: f64) -> Vec<f64> {
    let mut out = x.to_vec();
    for i in 0..x.len() {
        if rstep == 0.0 {
            if x[i] <= lo[i] {
                out[i] = next_toward(lo[i], hi[i]);
            } else if x[i] >= hi[i] {
                out[i] = next_toward(hi[i], lo[i]);
            }
        } else {
            let (l, u) = ((lo[i] + rstep * lo[i].abs().max(1.0)), (hi[i] - rstep * hi[i].abs().max(1.0)));
            if x[i] - lo[i] <= rstep * lo[i].abs().max(1.0) {
                out[i] = l;
            } else if hi[i] - x[i] <= rstep * hi[i].abs().max(1.0) {
                out[i] = u;
            }
        }
        if out[i] < lo[i] || out[i] > hi[i] {
            out[i] = 0.5 * (lo[i] + hi[i]);
        }
    }
    out
}

fn next_toward(a: f64, b: f64) -> f64 {
    if a == b {
        return a;
    }
    let bits = a.to_bits();
    let up = (b > a) == (a >= 0.0);
    let nb = if a == 0.0 { if b > a { 1 } else { (1u64 << 63) | 1 } } else if up { bits + 1 } else { bits - 1 };
    f64::from_bits(nb)
}

/// Exact solution of the trust-region subproblem from the SVD (Moré's algorithm, as scipy's
/// `solve_lsq_trust_region`). `vecs` holds V column-wise in row-major order.
fn solve_lsq_trust_region(n: usize, m: usize, uf: &[f64], s: &[f64], vecs: &[f64], delta: f64, initial_alpha: f64) -> (Vec<f64>, f64) {
    let suf: Vec<f64> = s.iter().zip(uf).map(|(a, b)| a * b).collect();
    let vdot = |w: &[f64]| -> Vec<f64> { (0..n).map(|p| (0..n).map(|k| vecs[p * n + k] * w[k]).sum()).collect() };
    let norm = |a: &[f64]| a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let phi = |alpha: f64| {
        let q: Vec<f64> = suf.iter().zip(s).map(|(a, si)| a / (si * si + alpha)).collect();
        let p_norm = norm(&q);
        let phi_prime = -suf.iter().zip(s).map(|(a, si)| a * a / (si * si + alpha).powi(3)).sum::<f64>() / p_norm;
        (p_norm - delta, phi_prime)
    };
    let full_rank = m >= n && s[n - 1] > f64::EPSILON * m as f64 * s[0];
    if full_rank {
        let p: Vec<f64> = vdot(&uf.iter().zip(s).map(|(u, si)| -u / si).collect::<Vec<_>>());
        if norm(&p) <= delta {
            return (p, 0.0);
        }
    }
    let mut alpha_upper = norm(&suf) / delta;
    let mut alpha_lower = if full_rank {
        let (ph, pp) = phi(0.0);
        -ph / pp
    } else {
        0.0
    };
    let mut alpha = if !full_rank && initial_alpha == 0.0 {
        (0.001 * alpha_upper).max((alpha_lower * alpha_upper).sqrt())
    } else {
        initial_alpha
    };
    for _ in 0..10 {
        if alpha < alpha_lower || alpha > alpha_upper {
            alpha = (0.001 * alpha_upper).max((alpha_lower * alpha_upper).sqrt());
        }
        let (ph, pp) = phi(alpha);
        if ph < 0.0 {
            alpha_upper = alpha;
        }
        let ratio = ph / pp;
        alpha_lower = alpha_lower.max(alpha - ratio);
        alpha -= (ph + delta) * ratio / delta;
        if ph.abs() < 0.01 * delta {
            break;
        }
    }
    let mut p = vdot(&suf.iter().zip(s).map(|(a, si)| -a / (si * si + alpha)).collect::<Vec<_>>());
    let pn = norm(&p);
    for x in &mut p {
        *x *= delta / pn;
    }
    (p, alpha)
}

fn step_size_to_bound(x: &[f64], s: &[f64], lo: &[f64], hi: &[f64]) -> (f64, Vec<f64>) {
    let steps: Vec<f64> = (0..x.len()).map(|i| if s[i] != 0.0 { ((lo[i] - x[i]) / s[i]).max((hi[i] - x[i]) / s[i]) } else { f64::INFINITY }).collect();
    let min = steps.iter().copied().fold(f64::INFINITY, f64::min);
    (min, steps.iter().zip(s).map(|(st, si)| if *st == min { si.signum() * (*si != 0.0) as i32 as f64 } else { 0.0 }).collect())
}

fn quad_1d(j: &[f64], m: usize, g: &[f64], s: &[f64], diag: &[f64], s0: Option<&[f64]>) -> (f64, f64, f64) {
    let n = g.len();
    let jv = |w: &[f64]| -> Vec<f64> { (0..m).map(|row| (0..n).map(|c| j[row * n + c] * w[c]).sum()).collect() };
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let v = jv(s);
    let mut a = dot(&v, &v) + s.iter().zip(diag).zip(s).map(|((x, d), y)| x * d * y).sum::<f64>();
    a *= 0.5;
    let mut b = dot(g, s);
    let mut c = 0.0;
    if let Some(s0) = s0 {
        let u = jv(s0);
        b += dot(&u, &v);
        c = 0.5 * dot(&u, &u) + dot(g, s0);
        b += s0.iter().zip(diag).zip(s).map(|((x, d), y)| x * d * y).sum::<f64>();
        c += 0.5 * s0.iter().zip(diag).zip(s0).map(|((x, d), y)| x * d * y).sum::<f64>();
    }
    (a, b, c)
}

fn min_quad_1d(a: f64, b: f64, lb: f64, ub: f64, c: f64) -> (f64, f64) {
    let mut t = vec![lb, ub];
    if a != 0.0 {
        let e = -0.5 * b / a;
        if lb < e && e < ub {
            t.push(e);
        }
    }
    t.into_iter().map(|t| (t, t * (a * t + b) + c)).min_by(|x, y| x.1.total_cmp(&y.1)).expect("non-empty")
}

fn eval_quad(j: &[f64], m: usize, g: &[f64], s: &[f64], diag: &[f64]) -> f64 {
    let (a, b, _) = quad_1d(j, m, g, s, diag, None);
    a + b
}

#[allow(clippy::too_many_arguments)]
fn select_step(
    x: &[f64],
    j_h: &[f64],
    m: usize,
    diag_h: &[f64],
    g_h: &[f64],
    mut p: Vec<f64>,
    mut p_h: Vec<f64>,
    d: &[f64],
    delta: f64,
    lo: &[f64],
    hi: &[f64],
    theta: f64,
) -> (Vec<f64>, Vec<f64>, f64) {
    let n = x.len();
    let xp: Vec<f64> = x.iter().zip(&p).map(|(a, b)| a + b).collect();
    if (0..n).all(|i| xp[i] >= lo[i] && xp[i] <= hi[i]) {
        let v = eval_quad(j_h, m, g_h, &p_h, diag_h);
        return (p, p_h, -v);
    }
    let (p_stride, hits) = step_size_to_bound(x, &p, lo, hi);
    let mut r_h = p_h.clone();
    for i in 0..n {
        if hits[i] != 0.0 {
            r_h[i] = -r_h[i];
        }
    }
    let mut r: Vec<f64> = d.iter().zip(&r_h).map(|(a, b)| a * b).collect();
    for i in 0..n {
        p[i] *= p_stride;
        p_h[i] *= p_stride;
    }
    let x_on_bound: Vec<f64> = x.iter().zip(&p).map(|(a, b)| a + b).collect();
    let to_tr = {
        let a: f64 = r_h.iter().map(|v| v * v).sum();
        let b: f64 = p_h.iter().zip(&r_h).map(|(x, y)| x * y).sum();
        let c: f64 = p_h.iter().map(|v| v * v).sum::<f64>() - delta * delta;
        let dd = (b * b - a * c).max(0.0).sqrt();
        let q = -(b + dd.copysign(b));
        let (t1, t2) = (q / a, c / q);
        t1.max(t2)
    };
    let (to_bound, _) = step_size_to_bound(&x_on_bound, &r, lo, hi);
    let r_stride = to_bound.min(to_tr);
    let (r_lo, r_hi) = if r_stride > 0.0 {
        ((1.0 - theta) * p_stride / r_stride, if r_stride == to_bound { theta * to_bound } else { to_tr })
    } else {
        (0.0, -1.0)
    };
    let r_value = if r_lo <= r_hi {
        let (a, b, c) = quad_1d(j_h, m, g_h, &r_h, diag_h, Some(&p_h));
        let (rs, rv) = min_quad_1d(a, b, r_lo, r_hi, c);
        for i in 0..n {
            r_h[i] = r_h[i] * rs + p_h[i];
            r[i] = r_h[i] * d[i];
        }
        rv
    } else {
        f64::INFINITY
    };
    for i in 0..n {
        p[i] *= theta;
        p_h[i] *= theta;
    }
    let p_value = eval_quad(j_h, m, g_h, &p_h, diag_h);
    let mut ag_h: Vec<f64> = g_h.iter().map(|v| -v).collect();
    let mut ag: Vec<f64> = d.iter().zip(&ag_h).map(|(a, b)| a * b).collect();
    let to_tr = delta / ag_h.iter().map(|v| v * v).sum::<f64>().sqrt();
    let (to_bound, _) = step_size_to_bound(x, &ag, lo, hi);
    let ag_stride = if to_bound < to_tr { theta * to_bound } else { to_tr };
    let (a, b, _) = quad_1d(j_h, m, g_h, &ag_h, diag_h, None);
    let (ag_stride, ag_value) = min_quad_1d(a, b, 0.0, ag_stride, 0.0);
    for i in 0..n {
        ag_h[i] *= ag_stride;
        ag[i] *= ag_stride;
    }
    if p_value < r_value && p_value < ag_value {
        (p, p_h, -p_value)
    } else if r_value < p_value && r_value < ag_value {
        (r, r_h, -r_value)
    } else {
        (ag, ag_h, -ag_value)
    }
}

/// Eigen-decomposition of a small symmetric matrix by cyclic Jacobi rotations: (eigenvalues, eigenvectors
/// stored column-wise in row-major order).
pub fn eig_sym(a: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut m = a.to_vec();
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _ in 0..100 {
        let off: f64 = (0..n).flat_map(|i| (0..n).filter(move |&j| j != i).map(move |j| (i, j))).map(|(i, j)| m[i * n + j].powi(2)).sum();
        let diag: f64 = (0..n).map(|i| m[i * n + i].powi(2)).sum();
        if off <= 1e-30 * diag.max(1e-300) {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = m[p * n + q];
                if apq == 0.0 {
                    continue;
                }
                let theta = (m[q * n + q] - m[p * n + p]) / (2.0 * apq);
                let t = if theta == 0.0 { 1.0 } else { theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt()) };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (mkp, mkq) = (m[k * n + p], m[k * n + q]);
                    m[k * n + p] = c * mkp - s * mkq;
                    m[k * n + q] = s * mkp + c * mkq;
                }
                for k in 0..n {
                    let (mpk, mqk) = (m[p * n + k], m[q * n + k]);
                    m[p * n + k] = c * mpk - s * mqk;
                    m[q * n + k] = s * mpk + c * mqk;
                }
                for k in 0..n {
                    let (vkp, vkq) = (v[k * n + p], v[k * n + q]);
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    ((0..n).map(|i| m[i * n + i]).collect(), v)
}

/// Pseudo-inverse of a small symmetric positive semi-definite matrix (what `np.linalg.pinv(JᵀJ)` gives for
/// the fit's covariance): eigenvalues below `1e-15·max` are dropped.
pub fn pinv_sym(a: &[f64], n: usize) -> Vec<f64> {
    let (ev, v) = eig_sym(a, n);
    let cutoff = 1e-15 * ev.iter().copied().fold(0.0, f64::max).abs();
    let mut out = vec![0.0; n * n];
    for (k, &e) in ev.iter().enumerate() {
        if e.abs() <= cutoff {
            continue;
        }
        for i in 0..n {
            for j in 0..n {
                out[i * n + j] += v[i * n + k] * v[j * n + k] / e;
            }
        }
    }
    out
}

/// Gaussian elimination with partial pivoting for a small dense system.
pub fn solve(a: &[f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut m = a.to_vec();
    let mut v = b.to_vec();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &k| m[i * n + col].abs().total_cmp(&m[k * n + col].abs()))?;
        if m[piv * n + col].abs() < 1e-300 {
            return None;
        }
        if piv != col {
            for k in 0..n {
                m.swap(piv * n + k, col * n + k);
            }
            v.swap(piv, col);
        }
        for row in col + 1..n {
            let fct = m[row * n + col] / m[col * n + col];
            for k in col..n {
                m[row * n + k] -= fct * m[col * n + k];
            }
            v[row] -= fct * v[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| m[row * n + k] * x[k]).sum();
        x[row] = (v[row] - s) / m[row * n + row];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brent_finds_roots() {
        let r = brent(|x| x * x - 2.0, 0.0, 2.0, 1e-14, 100).unwrap();
        assert!((r - 2f64.sqrt()).abs() < 1e-12);
        assert!(brent(|x| x * x + 1.0, 0.0, 2.0, 1e-12, 100).is_none());
    }

    #[test]
    fn lsq_fits_an_exponential_inside_bounds() {
        let ts: Vec<f64> = (0..20).map(f64::from).collect();
        let ys: Vec<f64> = ts.iter().map(|t| 3.0 * (-0.2 * t).exp() + 0.5).collect();
        let res = least_squares(
            |p| ts.iter().zip(&ys).map(|(t, y)| p[0] * (-p[1] * t).exp() + p[2] - y).collect(),
            &[1.0, 0.05, 0.0],
            &[0.0, 0.0, 0.0],
            &[10.0, 1.0, 2.0],
            200,
        );
        for (got, want) in res.x.iter().zip([3.0, 0.2, 0.5]) {
            assert!((got - want).abs() < 1e-6, "{:?}", res.x);
        }
    }

    #[test]
    fn upper_median_and_percentile() {
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), 3.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 0.5), 2.5);
        let inv = pinv_sym(&[4.0, 2.0, 2.0, 3.0], 2);
        let id = [4.0 * inv[0] + 2.0 * inv[2], 4.0 * inv[1] + 2.0 * inv[3]];
        assert!((id[0] - 1.0).abs() < 1e-12 && id[1].abs() < 1e-12);
    }
}
