//! WGS 84 Universal Transverse Mercator, forward and inverse.
//!
//! Krüger's series to sixth order in the third flattening n (C. F. F. Karney, "Transverse Mercator with
//! an accuracy of a few nanometers", J. Geodesy 85 (2011), eqs. 7–36). Within the UTM zones (±3.5° of
//! the central meridian) the truncation error is far below a millimetre; the same algorithm family as
//! PROJ's default `tmerc`, so the reprojection agrees with GDAL to well under a millimetre.

/// WGS 84 semi-major axis (m).
pub const A: f64 = 6_378_137.0;
/// WGS 84 flattening.
pub const F: f64 = 1.0 / 298.257_223_563;
/// UTM scale on the central meridian.
pub const K0: f64 = 0.9996;
const FALSE_EASTING: f64 = 500_000.0;
const FALSE_NORTHING_SOUTH: f64 = 10_000_000.0;

/// A UTM zone (1..=60) and hemisphere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zone {
    pub number: u8,
    pub north: bool,
}

impl Zone {
    /// EPSG 326zz (north) / 327zz (south) → zone. `None` for any other code.
    pub fn from_epsg(epsg: u32) -> Option<Self> {
        let (base, north) = match epsg {
            32601..=32660 => (32600, true),
            32701..=32760 => (32700, false),
            _ => return None,
        };
        Some(Self { number: (epsg - base) as u8, north })
    }

    pub const fn epsg(self) -> u32 {
        (if self.north { 32600 } else { 32700 }) + self.number as u32
    }

    /// Central meridian in degrees.
    pub fn central_meridian(self) -> f64 {
        f64::from(self.number) * 6.0 - 183.0
    }
}

/// Precomputed series coefficients (they depend only on the ellipsoid).
struct Series {
    /// k0 · rectifying radius A.
    k0a: f64,
    /// First eccentricity.
    e: f64,
    alpha: [f64; 6],
    beta: [f64; 6],
}

fn series() -> Series {
    let n = F / (2.0 - F);
    let n2 = n * n;
    let n3 = n2 * n;
    let n4 = n3 * n;
    let n5 = n4 * n;
    let n6 = n5 * n;
    let a_rect = A / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0 + n6 / 256.0);
    let alpha = [
        n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0 - 127.0 * n5 / 288.0 + 7891.0 * n6 / 37800.0,
        13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0 + 281.0 * n5 / 630.0 - 1_983_433.0 * n6 / 1_935_360.0,
        61.0 * n3 / 240.0 - 103.0 * n4 / 140.0 + 15061.0 * n5 / 26880.0 + 167_603.0 * n6 / 181_440.0,
        49561.0 * n4 / 161_280.0 - 179.0 * n5 / 168.0 + 6_601_661.0 * n6 / 7_257_600.0,
        34729.0 * n5 / 80640.0 - 3_418_889.0 * n6 / 1_995_840.0,
        212_378_941.0 * n6 / 319_334_400.0,
    ];
    let beta = [
        n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0 - n4 / 360.0 - 81.0 * n5 / 512.0 + 96199.0 * n6 / 604_800.0,
        n2 / 48.0 + n3 / 15.0 - 437.0 * n4 / 1440.0 + 46.0 * n5 / 105.0 - 1_118_711.0 * n6 / 3_870_720.0,
        17.0 * n3 / 480.0 - 37.0 * n4 / 840.0 - 209.0 * n5 / 4480.0 + 5569.0 * n6 / 90720.0,
        4397.0 * n4 / 161_280.0 - 11.0 * n5 / 504.0 - 830_251.0 * n6 / 7_257_600.0,
        4583.0 * n5 / 161_280.0 - 108_847.0 * n6 / 3_991_680.0,
        20_648_693.0 * n6 / 638_668_800.0,
    ];
    Series { k0a: K0 * a_rect, e: (F * (2.0 - F)).sqrt(), alpha, beta }
}

static SERIES: std::sync::LazyLock<Series> = std::sync::LazyLock::new(series);

/// Geodetic (lon, lat) in degrees → (easting, northing) in metres in `zone`.
pub fn forward(zone: Zone, lon: f64, lat: f64) -> (f64, f64) {
    let s = &*SERIES;
    let phi = lat.to_radians();
    let lam = (lon - zone.central_meridian()).to_radians();
    let sphi = phi.sin();
    // Conformal latitude via τ' = sinh(atanh(sin φ) − e·atanh(e·sin φ)).
    let t = (sphi.atanh() - s.e * (s.e * sphi).atanh()).sinh();
    let xi_p = t.atan2(lam.cos());
    let eta_p = (lam.sin() / (1.0 + t * t).sqrt()).atanh();
    let mut xi = xi_p;
    let mut eta = eta_p;
    for (j, a) in s.alpha.iter().enumerate() {
        let k = 2.0 * (j + 1) as f64;
        xi += a * (k * xi_p).sin() * (k * eta_p).cosh();
        eta += a * (k * xi_p).cos() * (k * eta_p).sinh();
    }
    let e = FALSE_EASTING + s.k0a * eta;
    let n = s.k0a * xi + if zone.north { 0.0 } else { FALSE_NORTHING_SOUTH };
    (e, n)
}

/// (easting, northing) in metres in `zone` → geodetic (lon, lat) in degrees.
pub fn inverse(zone: Zone, easting: f64, northing: f64) -> (f64, f64) {
    let s = &*SERIES;
    let xi = (northing - if zone.north { 0.0 } else { FALSE_NORTHING_SOUTH }) / s.k0a;
    let eta = (easting - FALSE_EASTING) / s.k0a;
    let mut xi_p = xi;
    let mut eta_p = eta;
    for (j, b) in s.beta.iter().enumerate() {
        let k = 2.0 * (j + 1) as f64;
        xi_p -= b * (k * xi).sin() * (k * eta).cosh();
        eta_p -= b * (k * xi).cos() * (k * eta).sinh();
    }
    // Conformal latitude χ and longitude.
    let tau_p = xi_p.sin() / (eta_p.sinh().powi(2) + xi_p.cos().powi(2)).sqrt();
    let lam = eta_p.sinh().atan2(xi_p.cos());
    let tau = tau_from_conformal(tau_p, s.e);
    (zone.central_meridian() + lam.to_degrees(), tau.atan().to_degrees())
}

/// Solves τ' = τ·√(1+σ²) − σ·√(1+τ²), σ = sinh(e·atanh(e·τ/√(1+τ²))), for τ = tan φ (Newton, Karney eq. 19–21).
fn tau_from_conformal(tau_p: f64, e: f64) -> f64 {
    let e2 = e * e;
    let mut tau = tau_p / (1.0 - e2);
    for _ in 0..5 {
        let h = (1.0 + tau * tau).sqrt();
        let sigma = (e * (e * tau / h).atanh()).sinh();
        let tp = tau * (1.0 + sigma * sigma).sqrt() - sigma * h;
        let dtau = (tau_p - tp) / (1.0 + tp * tp).sqrt() * (1.0 + (1.0 - e2) * tau * tau) / ((1.0 - e2) * h);
        tau += dtau;
        if dtau.abs() < 1e-15 * tau.abs().max(1.0) {
            break;
        }
    }
    tau
}

#[cfg(test)]
mod tests {
    use super::*;

    const Z34: Zone = Zone { number: 34, north: true };
    const Z35: Zone = Zone { number: 35, north: true };

    #[test]
    fn zones() {
        assert_eq!(Zone::from_epsg(32634), Some(Z34));
        assert_eq!(Zone::from_epsg(32735), Some(Zone { number: 35, north: false }));
        assert_eq!(Zone::from_epsg(4326), None);
        assert_eq!(Z34.central_meridian(), 21.0);
        assert_eq!(Z35.epsg(), 32635);
    }

    #[test]
    fn central_meridian_is_scaled_meridian_arc() {
        // Equator on the central meridian.
        let (e, n) = forward(Z34, 21.0, 0.0);
        assert!((e - 500_000.0).abs() < 1e-9 && n.abs() < 1e-9);
        // WGS 84 meridian arc from the equator to 45° is 4 984 944.378 m; UTM northing = 0.9996 × that.
        let (e, n) = forward(Z34, 21.0, 45.0);
        assert!((e - 500_000.0).abs() < 1e-9);
        assert!((n - 0.9996 * 4_984_944.378).abs() < 2e-3, "{n}");
        // Southern hemisphere: false northing.
        let (_, n) = forward(Zone { number: 34, north: false }, 21.0, -45.0);
        assert!((n - (10_000_000.0 - 0.9996 * 4_984_944.378)).abs() < 2e-3, "{n}");
    }

    #[test]
    fn thessaloniki_reference() {
        // White Tower, Thessaloniki (40.626446 N, 22.948426 E). Reference: Snyder's "Map Projections — A
        // Working Manual" eqs. 8-9/8-10 (the USGS power series in A = Δλ·cos φ), evaluated independently in
        // double precision. That series is itself accurate to ~1 cm at 2° from the central meridian and to
        // ~1 m at 4°, so the tolerances below are Snyder's error, not ours.
        let (e, n) = forward(Z34, 22.948426, 40.626446);
        assert!((e - 664_792.162).abs() < 0.03, "{e}");
        assert!((n - 4_499_115.203).abs() < 0.03, "{n}");
        // The same point from the neighbouring zone (4° off its central meridian; some NISAR frames).
        let (e, n) = forward(Z35, 22.948426, 40.626446);
        assert!((e - 157_296.043).abs() < 1.0, "{e}");
        assert!((n - 4_505_187.094).abs() < 1.0, "{n}");
    }

    #[test]
    fn round_trip_is_sub_millimetre() {
        for &(lon, lat) in &[(23.15, 40.68), (18.1, 40.0), (23.9, 41.5), (21.0, 0.0), (19.5, -33.9), (25.0, 70.0)] {
            for zone in [Z34, Z35, Zone { number: 34, north: false }] {
                let (e, n) = forward(zone, lon, lat);
                let (lo, la) = inverse(zone, e, n);
                let (e2, n2) = forward(zone, lo, la);
                assert!((lo - lon).abs() < 1e-10 && (la - lat).abs() < 1e-10, "{lon},{lat} -> {lo},{la}");
                assert!((e2 - e).abs() < 1e-4 && (n2 - n).abs() < 1e-4);
            }
        }
    }
}
