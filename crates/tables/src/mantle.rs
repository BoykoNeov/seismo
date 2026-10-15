//! 3D mantle velocity perturbations in the `.sph` spherical-harmonic format of the S20RTS /
//! S40RTS / SP12RTS model family (Ritsema et al.), parsed for the ray integrals in `sim`.
//!
//! A model file holds, for each of 21 radial splines (top of the mantle first) and each degree
//! l = 0..lmax, the 2l+1 coefficients of the real and imaginary parts of fully normalised complex
//! spherical harmonics (Condon–Shortley phase): m = 0, then cos and sin for m = 1..l. The value
//! is the fractional velocity perturbation dlnV relative to the model's reference (PREM). See
//! `assets/sp12rts/README.md` for the source and citation.
//!
//! The exact sum ([`SphModel::dlnv`]) is slow: every radial spline has support over the whole
//! mantle and every harmonic is non-zero almost everywhere. [`MantleModel`] samples it once onto a
//! regular grid for fast trilinear lookups along rays.

use std::f64::consts::PI;

use crate::TableError;

/// Radius (km) of the bottom of the radial parameterisation: the model's core-mantle boundary.
pub const SPH_R_BOTTOM_KM: f64 = 3480.0;
/// Radius (km) of the top of the radial parameterisation: the model's Moho (PREM, 24.4 km deep).
pub const SPH_R_TOP_KM: f64 = 6346.619;

const NUM_SPLINES: usize = 21;
/// Spline knots in x = -1 (bottom) .. 1 (top), from the authors' `splhsetup.f`.
const KNOTS: [f64; NUM_SPLINES] = [
    -1.00000, -0.78631, -0.59207, -0.41550, -0.25499, -0.10909, 0.02353, 0.14409, 0.25367, 0.35329,
    0.44384, 0.52615, 0.60097, 0.66899, 0.73081, 0.78701, 0.83810, 0.88454, 0.92675, 0.96512,
    1.00000,
];

/// Grid spacing of [`MantleModel`]: latitude and longitude (deg), radius (km, at most).
const GRID_DEG: f64 = 2.0;
const GRID_MAX_DR_KM: f64 = 25.0;

/// A cubic spline over [`KNOTS`] as per-interval polynomials a + b·h + c·h² + d·h³, h = x − x_i.
#[derive(Debug, Clone)]
struct Cubic {
    coef: [[f64; 4]; NUM_SPLINES - 1],
}

impl Cubic {
    /// The interpolating cubic through `y` at the knots, with end slopes taken from the parabola
    /// through the three end points (the authors' `rspln.f`).
    fn through(y: &[f64; NUM_SPLINES]) -> Self {
        let x = &KNOTS;
        let n = NUM_SPLINES;
        // Slope at `at` of the parabola through three points.
        let parabola_slope = |i: [usize; 3], at: f64| {
            let [a, b, c] = i;
            y[a] * ((at - x[b]) + (at - x[c])) / ((x[a] - x[b]) * (x[a] - x[c]))
                + y[b] * ((at - x[a]) + (at - x[c])) / ((x[b] - x[a]) * (x[b] - x[c]))
                + y[c] * ((at - x[a]) + (at - x[b])) / ((x[c] - x[a]) * (x[c] - x[b]))
        };
        let s0 = parabola_slope([0, 1, 2], x[0]);
        let sn = parabola_slope([n - 3, n - 2, n - 1], x[n - 1]);
        // Second derivatives m_i of the clamped spline: a tridiagonal system.
        let h: Vec<f64> = (0..n - 1).map(|i| x[i + 1] - x[i]).collect();
        let (mut sub, mut diag, mut sup, mut rhs) =
            (vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n]);
        diag[0] = 2.0 * h[0];
        sup[0] = h[0];
        rhs[0] = 6.0 * ((y[1] - y[0]) / h[0] - s0);
        for i in 1..n - 1 {
            sub[i] = h[i - 1];
            diag[i] = 2.0 * (h[i - 1] + h[i]);
            sup[i] = h[i];
            rhs[i] = 6.0 * ((y[i + 1] - y[i]) / h[i] - (y[i] - y[i - 1]) / h[i - 1]);
        }
        sub[n - 1] = h[n - 2];
        diag[n - 1] = 2.0 * h[n - 2];
        rhs[n - 1] = 6.0 * (sn - (y[n - 1] - y[n - 2]) / h[n - 2]);
        for i in 1..n {
            let w = sub[i] / diag[i - 1];
            diag[i] -= w * sup[i - 1];
            rhs[i] -= w * rhs[i - 1];
        }
        let mut m = vec![0.0; n];
        m[n - 1] = rhs[n - 1] / diag[n - 1];
        for i in (0..n - 1).rev() {
            m[i] = (rhs[i] - sup[i] * m[i + 1]) / diag[i];
        }
        let mut coef = [[0.0; 4]; NUM_SPLINES - 1];
        for (i, c) in coef.iter_mut().enumerate() {
            *c = [
                y[i],
                (y[i + 1] - y[i]) / h[i] - h[i] * (2.0 * m[i] + m[i + 1]) / 6.0,
                m[i] / 2.0,
                (m[i + 1] - m[i]) / (6.0 * h[i]),
            ];
        }
        Self { coef }
    }

    fn eval(&self, x: f64) -> f64 {
        let i = KNOTS.partition_point(|&k| k <= x).clamp(1, NUM_SPLINES - 1) - 1;
        let h = x - KNOTS[i];
        let [a, b, c, d] = self.coef[i];
        a + h * (b + h * (c + h * d))
    }
}

/// The 21 radial basis functions, in file order (top of the mantle first): basis `k` is 1 at
/// knot `NUM_SPLINES - 1 - k` and 0 at the others.
#[derive(Debug, Clone)]
struct RadialBasis {
    splines: Vec<Cubic>,
}

impl RadialBasis {
    fn new() -> Self {
        let splines = (0..NUM_SPLINES)
            .map(|k| {
                let mut y = [0.0; NUM_SPLINES];
                y[NUM_SPLINES - 1 - k] = 1.0;
                Cubic::through(&y)
            })
            .collect();
        Self { splines }
    }

    /// Weights of the basis functions at radius `r_km`; `None` outside the parameterisation.
    fn weights(&self, r_km: f64) -> Option<[f64; NUM_SPLINES]> {
        if !(SPH_R_BOTTOM_KM..=SPH_R_TOP_KM).contains(&r_km) {
            return None;
        }
        let x = -1.0 + 2.0 * (r_km - SPH_R_BOTTOM_KM) / (SPH_R_TOP_KM - SPH_R_BOTTOM_KM);
        let mut w = [0.0; NUM_SPLINES];
        for (wk, s) in w.iter_mut().zip(&self.splines) {
            *wk = s.eval(x);
        }
        Some(w)
    }
}

/// Fully normalised associated Legendre functions with the Condon–Shortley phase,
/// (−1)^m √((2l+1)/4π · (l−m)!/(l+m)!) P_lm(cos θ), at geographic latitude `lat_deg`, indexed
/// `l(l+1)/2 + m`.
fn legendre(lmax: usize, lat_deg: f64) -> Vec<f64> {
    let theta = (90.0 - lat_deg).to_radians();
    let (x, s) = (theta.cos(), theta.sin());
    let idx = |l: usize, m: usize| l * (l + 1) / 2 + m;
    let mut p = vec![0.0; idx(lmax, lmax) + 1];
    p[0] = (1.0 / (4.0 * PI)).sqrt();
    for m in 1..=lmax {
        p[idx(m, m)] = -p[idx(m - 1, m - 1)] * s * ((2 * m + 1) as f64 / (2 * m) as f64).sqrt();
    }
    for m in 0..lmax {
        p[idx(m + 1, m)] = x * ((2 * m + 3) as f64).sqrt() * p[idx(m, m)];
    }
    for m in 0..=lmax {
        for l in m + 2..=lmax {
            let (lf, mf) = (l as f64, m as f64);
            let a = ((4.0 * lf * lf - 1.0) / (lf * lf - mf * mf)).sqrt();
            let b = (((lf - 1.0).powi(2) - mf * mf) / (4.0 * (lf - 1.0).powi(2) - 1.0)).sqrt();
            p[idx(l, m)] = a * (x * p[idx(l - 1, m)] - b * p[idx(l - 2, m)]);
        }
    }
    p
}

/// The real harmonics at a point, in file order: per degree l, m = 0, then cos and sin parts
/// for m = 1..l.
fn harmonics(lmax: usize, lat_deg: f64, lon_deg: f64) -> Vec<f64> {
    let p = legendre(lmax, lat_deg);
    let phi = lon_deg.to_radians();
    let mut out = Vec::with_capacity((lmax + 1) * (lmax + 1));
    for l in 0..=lmax {
        let base = l * (l + 1) / 2;
        out.push(p[base]);
        for m in 1..=l {
            let (sin, cos) = (m as f64 * phi).sin_cos();
            out.push(p[base + m] * cos);
            out.push(p[base + m] * sin);
        }
    }
    out
}

/// One parameter (e.g. shear velocity) of a `.sph` model.
#[derive(Debug, Clone)]
pub struct SphModel {
    lmax: usize,
    /// `(lmax+1)²` coefficients per radial spline, top spline first.
    coeffs: Vec<f64>,
    radial: RadialBasis,
}

impl SphModel {
    /// Parse a `.sph` file: header line, then whitespace-separated coefficients. Only files with
    /// every degree present and exactly 21 radial splines are accepted.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TableError> {
        let bad = |msg: String| TableError::Sph(msg);
        let text = std::str::from_utf8(bytes).map_err(|e| bad(e.to_string()))?;
        let (header, body) = text
            .split_once('\n')
            .ok_or_else(|| bad("no header line".into()))?;
        let fields: Vec<&str> = header.split_whitespace().collect();
        let [lmax, degree_mask, nrad, rad_mask] = fields[..] else {
            return Err(bad(format!(
                "expected 4 header fields, got {}",
                fields.len()
            )));
        };
        let lmax: usize = lmax
            .parse()
            .map_err(|_| bad(format!("bad lmax {lmax:?}")))?;
        if degree_mask.len() != lmax + 1 || degree_mask.bytes().any(|b| b != b'1') {
            return Err(bad(format!(
                "degree mask {degree_mask:?}: every degree must be present"
            )));
        }
        let nrad: usize = nrad
            .parse()
            .map_err(|_| bad(format!("bad parameter count {nrad:?}")))?;
        let present = rad_mask.bytes().filter(|&b| b == b'1').count();
        if rad_mask.len() != nrad
            || present != NUM_SPLINES
            || !rad_mask.ends_with(&"1".repeat(NUM_SPLINES))
        {
            return Err(bad(format!(
                "radial mask {rad_mask:?}: expected {NUM_SPLINES} trailing splines"
            )));
        }
        let coeffs: Vec<f64> = body
            .split_whitespace()
            .map(|t| {
                t.parse::<f64>()
                    .map_err(|_| bad(format!("bad coefficient {t:?}")))
            })
            .collect::<Result<_, _>>()?;
        let want = NUM_SPLINES * (lmax + 1) * (lmax + 1);
        if coeffs.len() != want {
            return Err(bad(format!(
                "expected {want} coefficients, got {}",
                coeffs.len()
            )));
        }
        Ok(Self {
            lmax,
            coeffs,
            radial: RadialBasis::new(),
        })
    }

    pub fn lmax(&self) -> usize {
        self.lmax
    }

    fn harmonic_count(&self) -> usize {
        (self.lmax + 1) * (self.lmax + 1)
    }

    /// Harmonic coefficients at radius `r_km` (the radial splines summed), degree 0 zeroed;
    /// `None` outside the parameterisation.
    fn coeffs_at(&self, r_km: f64) -> Option<Vec<f64>> {
        let w = self.radial.weights(r_km)?;
        let n = self.harmonic_count();
        let mut c = vec![0.0; n];
        for (k, wk) in w.iter().enumerate() {
            for (ci, v) in c.iter_mut().zip(&self.coeffs[k * n..(k + 1) * n]) {
                *ci += wk * v;
            }
        }
        // The radial average is not a lateral variation; the perturbations are applied to ak135,
        // itself an average model.
        c[0] = 0.0;
        Some(c)
    }

    /// The exact perturbation dlnV (a fraction, not %) at radius `r_km`, geographic latitude and
    /// longitude in degrees, without the degree-0 term. 0 outside the parameterisation.
    pub fn dlnv(&self, r_km: f64, lat_deg: f64, lon_deg: f64) -> f64 {
        let Some(c) = self.coeffs_at(r_km) else {
            return 0.0;
        };
        let y = harmonics(self.lmax, lat_deg, lon_deg);
        c.iter().zip(&y).map(|(a, b)| a * b).sum()
    }
}

/// Shear and compressional perturbations of a 3D mantle model, sampled onto a regular grid
/// (2° in latitude and longitude, at most 25 km in radius) for fast lookups.
#[derive(Debug, Clone)]
pub struct MantleModel {
    vs: SphModel,
    vp: SphModel,
    nr: usize,
    dr_km: f64,
    nlat: usize,
    nlon: usize,
    /// (dlnVs, dlnVp) per node, indexed `[radius][lat][lon]`.
    grid: Vec<[f32; 2]>,
}

impl MantleModel {
    /// Parse the shear (`..ES.sph`) and compressional (`..EP.sph`) files and build the grid.
    pub fn from_bytes(vs: &[u8], vp: &[u8]) -> Result<Self, TableError> {
        Ok(Self::new(
            SphModel::from_bytes(vs)?,
            SphModel::from_bytes(vp)?,
        ))
    }

    pub fn new(vs: SphModel, vp: SphModel) -> Self {
        let span = SPH_R_TOP_KM - SPH_R_BOTTOM_KM;
        let nr = (span / GRID_MAX_DR_KM).ceil() as usize + 1;
        let dr_km = span / (nr - 1) as f64;
        let nlat = (180.0 / GRID_DEG) as usize + 1;
        let nlon = (360.0 / GRID_DEG) as usize;
        let mut grid = vec![[0.0f32; 2]; nr * nlat * nlon];
        for (which, model) in [&vs, &vp].into_iter().enumerate() {
            let lmax = model.lmax;
            // cos(mφ), sin(mφ) per longitude node.
            let trig: Vec<Vec<(f64, f64)>> = (0..nlon)
                .map(|j| {
                    let phi = (-180.0 + GRID_DEG * j as f64).to_radians();
                    (0..=lmax)
                        .map(|m| (m as f64 * phi).sin_cos())
                        .map(|(s, c)| (c, s))
                        .collect()
                })
                .collect();
            let legendre_rows: Vec<Vec<f64>> = (0..nlat)
                .map(|i| legendre(lmax, -90.0 + GRID_DEG * i as f64))
                .collect();
            for ir in 0..nr {
                let r = SPH_R_BOTTOM_KM + dr_km * ir as f64;
                let c = model
                    .coeffs_at(r.min(SPH_R_TOP_KM))
                    .expect("grid radii lie inside the parameterisation");
                for (ilat, p) in legendre_rows.iter().enumerate() {
                    // a_m, b_m: the cos and sin amplitudes of order m at this latitude.
                    let mut a = vec![0.0; lmax + 1];
                    let mut b = vec![0.0; lmax + 1];
                    for l in 0..=lmax {
                        let base = l * l;
                        let pl = l * (l + 1) / 2;
                        a[0] += p[pl] * c[base];
                        for m in 1..=l {
                            a[m] += p[pl + m] * c[base + 2 * m - 1];
                            b[m] += p[pl + m] * c[base + 2 * m];
                        }
                    }
                    for (ilon, t) in trig.iter().enumerate() {
                        let v: f64 = (0..=lmax).map(|m| a[m] * t[m].0 + b[m] * t[m].1).sum();
                        grid[(ir * nlat + ilat) * nlon + ilon][which] = v as f32;
                    }
                }
            }
        }
        Self {
            vs,
            vp,
            nr,
            dr_km,
            nlat,
            nlon,
            grid,
        }
    }

    pub fn vs_model(&self) -> &SphModel {
        &self.vs
    }

    pub fn vp_model(&self) -> &SphModel {
        &self.vp
    }

    /// Shear perturbation dlnVs (fraction) from the grid; 0 outside the model's mantle.
    pub fn dlnvs(&self, r_km: f64, lat_deg: f64, lon_deg: f64) -> f64 {
        self.sample(0, r_km, lat_deg, lon_deg)
    }

    /// Compressional perturbation dlnVp (fraction) from the grid; 0 outside the model's mantle.
    pub fn dlnvp(&self, r_km: f64, lat_deg: f64, lon_deg: f64) -> f64 {
        self.sample(1, r_km, lat_deg, lon_deg)
    }

    /// Trilinear lookup, wrapping in longitude.
    fn sample(&self, which: usize, r_km: f64, lat_deg: f64, lon_deg: f64) -> f64 {
        if !(SPH_R_BOTTOM_KM..=SPH_R_TOP_KM).contains(&r_km) {
            return 0.0;
        }
        let fr = (r_km - SPH_R_BOTTOM_KM) / self.dr_km;
        let ir = (fr.floor() as usize).min(self.nr - 2);
        let wr = fr - ir as f64;
        let fa = ((lat_deg + 90.0) / GRID_DEG).clamp(0.0, (self.nlat - 1) as f64);
        let ia = (fa.floor() as usize).min(self.nlat - 2);
        let wa = fa - ia as f64;
        let fo = ((lon_deg + 180.0) / GRID_DEG).rem_euclid(self.nlon as f64);
        let io = (fo.floor() as usize).min(self.nlon - 1);
        let wo = fo - io as f64;
        let io1 = (io + 1) % self.nlon;
        let at = |r: usize, a: usize, o: usize| {
            self.grid[(r * self.nlat + a) * self.nlon + o][which] as f64
        };
        let mut v = 0.0;
        for (r, fr) in [(ir, 1.0 - wr), (ir + 1, wr)] {
            for (a, fa) in [(ia, 1.0 - wa), (ia + 1, wa)] {
                for (o, fo) in [(io, 1.0 - wo), (io1, wo)] {
                    v += fr * fa * fo * at(r, a, o);
                }
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ES: &[u8] = include_bytes!("../../../assets/sp12rts/SP12RTS..ES.sph");
    static EP: &[u8] = include_bytes!("../../../assets/sp12rts/SP12RTS..EP.sph");

    /// A degree-`lmax` file with a single non-zero coefficient: `value` at harmonic `h` of
    /// radial spline `k`, or at every spline when `k` is `None`.
    fn synthetic(lmax: usize, k: Option<usize>, h: usize, value: f64) -> SphModel {
        let n = (lmax + 1) * (lmax + 1);
        let mut text = format!(
            "{lmax} {} 24 000{}\n",
            "1".repeat(lmax + 1),
            "1".repeat(NUM_SPLINES)
        );
        for spline in 0..NUM_SPLINES {
            for i in 0..n {
                let v = if i == h && k.is_none_or(|k| k == spline) {
                    value
                } else {
                    0.0
                };
                text += &format!("{v:12.4E}");
            }
            text.push('\n');
        }
        SphModel::from_bytes(text.as_bytes()).unwrap()
    }

    /// Radius (km) of knot `i` (0 = bottom).
    fn knot_radius(i: usize) -> f64 {
        SPH_R_BOTTOM_KM + 0.5 * (KNOTS[i] + 1.0) * (SPH_R_TOP_KM - SPH_R_BOTTOM_KM)
    }

    #[test]
    fn parses_the_bundled_files() {
        for bytes in [ES, EP] {
            let m = SphModel::from_bytes(bytes).unwrap();
            assert_eq!(m.lmax(), 12);
            assert_eq!(m.coeffs.len(), 21 * 169);
        }
        // First and last coefficients of the shear file, as printed in it.
        let s = SphModel::from_bytes(ES).unwrap();
        assert_eq!(s.coeffs[0], 0.1009E-01);
        assert!(s.coeffs.last().unwrap().is_finite());
    }

    #[test]
    fn harmonic_conventions() {
        let top = SPH_R_TOP_KM;
        // l = 1, m = 0 (index 1) at the north pole: √(3/4π).
        let m = synthetic(1, None, 1, 1.0);
        assert!((m.dlnv(top, 90.0, 0.0) - (3.0 / (4.0 * PI)).sqrt()).abs() < 1e-12);
        // l = 1, m = 1 cos part (index 2) at (0°, 0°): −√(3/8π), Condon–Shortley sign.
        let m = synthetic(1, None, 2, 1.0);
        assert!((m.dlnv(top, 0.0, 0.0) + (3.0 / (8.0 * PI)).sqrt()).abs() < 1e-12);
        // l = 1, m = 1 sin part (index 3) at (0°, 90°E): −√(3/8π).
        let m = synthetic(1, None, 3, 1.0);
        assert!((m.dlnv(top, 0.0, 90.0) + (3.0 / (8.0 * PI)).sqrt()).abs() < 1e-12);
        assert!(m.dlnv(top, 0.0, 0.0).abs() < 1e-12);
    }

    #[test]
    fn degree_zero_is_ignored() {
        let m = synthetic(2, None, 0, 0.05);
        assert_eq!(m.dlnv(5000.0, 10.0, 20.0), 0.0);
    }

    #[test]
    fn zero_outside_the_parameterisation() {
        let m = synthetic(1, None, 1, 1.0);
        assert_eq!(m.dlnv(SPH_R_TOP_KM + 1.0, 90.0, 0.0), 0.0);
        assert_eq!(m.dlnv(SPH_R_BOTTOM_KM - 1.0, 90.0, 0.0), 0.0);
        assert_eq!(m.dlnv(1000.0, 90.0, 0.0), 0.0);
    }

    #[test]
    fn radial_splines_are_cardinal_and_sum_to_one() {
        let y10 = (3.0 / (4.0 * PI)).sqrt();
        for k in [0, 5, 20] {
            let m = synthetic(1, Some(k), 1, 1.0);
            for i in 0..NUM_SPLINES {
                let want = if i == NUM_SPLINES - 1 - k { y10 } else { 0.0 };
                let got = m.dlnv(knot_radius(i), 90.0, 0.0);
                assert!((got - want).abs() < 1e-12, "spline {k} at knot {i}: {got}");
            }
        }
        // Every spline weighted 1: the profile is constant through the whole mantle.
        let m = synthetic(1, None, 1, 1.0);
        for i in 0..=100 {
            let r = SPH_R_BOTTOM_KM + (SPH_R_TOP_KM - SPH_R_BOTTOM_KM) * i as f64 / 100.0;
            assert!((m.dlnv(r, 90.0, 0.0) - y10).abs() < 1e-12, "r = {r}");
        }
    }

    #[test]
    fn harmonics_are_orthonormal() {
        // ∫ Y_a Y_b dΩ = δ_ab, halved for the cos and sin parts of m > 0 (they are Re and Im of
        // a unit-norm complex harmonic). Midpoint rule on a 1° grid.
        let lmax = 4;
        let n = (lmax + 1) * (lmax + 1);
        let mut gram = vec![0.0; n * n];
        let step = 1.0f64;
        for i in 0..180 {
            let lat = -90.0 + step * (i as f64 + 0.5);
            let da = (step.to_radians()).powi(2) * lat.to_radians().cos();
            for j in 0..360 {
                let y = harmonics(lmax, lat, -180.0 + step * (j as f64 + 0.5));
                for a in 0..n {
                    for b in 0..n {
                        gram[a * n + b] += y[a] * y[b] * da;
                    }
                }
            }
        }
        let is_m0: Vec<bool> = (0..=lmax)
            .flat_map(|l| std::iter::once(true).chain(std::iter::repeat_n(false, 2 * l)))
            .collect();
        for a in 0..n {
            for b in 0..n {
                let want = match (a == b, is_m0[a]) {
                    (false, _) => 0.0,
                    (true, true) => 1.0,
                    (true, false) => 0.5,
                };
                assert!(
                    (gram[a * n + b] - want).abs() < 1e-3,
                    "({a},{b}): {}",
                    gram[a * n + b]
                );
            }
        }
    }

    #[test]
    fn grid_matches_exact_sum() {
        let m = MantleModel::from_bytes(ES, EP).unwrap();
        let mut worst: f64 = 0.0;
        for i in 0..400 {
            // Deterministic scatter over the mantle, including off-node longitudes near ±180°.
            let f = i as f64;
            let r = SPH_R_BOTTOM_KM + (SPH_R_TOP_KM - SPH_R_BOTTOM_KM) * ((f * 0.618_034) % 1.0);
            let lat = -89.0 + 178.0 * ((f * 0.414_214) % 1.0);
            let lon = -180.0 + 360.0 * ((f * 0.732_051) % 1.0);
            worst = worst
                .max((m.dlnvs(r, lat, lon) - m.vs_model().dlnv(r, lat, lon)).abs())
                .max((m.dlnvp(r, lat, lon) - m.vp_model().dlnv(r, lat, lon)).abs());
        }
        // Peaks are ~0.035; the grid is far finer than degree 12 (30° wavelength).
        assert!(worst < 1e-3, "worst grid error {worst}");
    }

    #[test]
    fn amplitudes_match_the_paper() {
        // Koelemeijer et al. (2016): dlnVs "up to 3.5 per cent at the top boundary, the
        // lithosphere and up to 1.5 per cent at the bottom boundary". Checked on the exact sum,
        // 2° grid, at 100 km depth and just above the core.
        let s = SphModel::from_bytes(ES).unwrap();
        let peak = |depth: f64| {
            let mut p: f64 = 0.0;
            for i in 0..91 {
                for j in 0..180 {
                    let v = s.dlnv(
                        6371.0 - depth,
                        -90.0 + 2.0 * i as f64,
                        -180.0 + 2.0 * j as f64,
                    );
                    p = p.max(v.abs());
                }
            }
            100.0 * p
        };
        let top = peak(100.0);
        let bottom = peak(2880.0);
        assert!((3.0..=4.0).contains(&top), "peak at 100 km: {top}%");
        assert!((1.2..=1.8).contains(&bottom), "peak at 2880 km: {bottom}%");
    }

    #[test]
    fn matches_the_authors_own_code() {
        // (model, depth km, lat, lon, dlnV in %) printed by the authors' own programs,
        // `depmaphj_jr` then `raw2xyz_jr` (2° grid, degrees 1–12), from SP12RTS_plotting.tar.gz
        // (retrieved 2026-10-15), compiled unchanged with gfortran and run on the bundled files.
        // Knot and mid-knot depths, the strongest value within ±60° and one beside the ±180°
        // seam. Within ±60° the two agree to the 5 printed digits at every depth; nearer the
        // poles the authors' single-precision Legendre recursion drifts (up to 0.002 %).
        const REF: [(&str, f64, f64, f64, f64); 32] = [
            ("ES", 24.500, 22.0, -112.0, -2.9176),
            ("ES", 24.500, 12.0, 178.0, 2.3612),
            ("ES", 49.378, 22.0, -114.0, -3.2527),
            ("ES", 49.378, 12.0, 178.0, 2.5243),
            ("ES", 129.371, 30.0, -116.0, -3.5805),
            ("ES", 129.371, -60.0, 178.0, -1.9114),
            ("ES", 159.621, 32.0, -118.0, -3.4003),
            ("ES", 159.621, -60.0, 178.0, -1.8502),
            ("ES", 649.935, 12.0, -10.0, 1.3404),
            ("ES", 649.935, -60.0, 178.0, -0.97125),
            ("ES", 1423.965, 54.0, -88.0, 0.82897),
            ("ES", 1423.965, -16.0, 178.0, -0.56942),
            ("ES", 2737.858, 0.0, -158.0, -1.2282),
            ("ES", 2737.858, 0.0, 178.0, -1.0984),
            ("ES", 2890.500, 0.0, -150.0, -1.5397),
            ("ES", 2890.500, -12.0, 178.0, -1.2823),
            ("EP", 24.500, 26.0, -114.0, -3.4748),
            ("EP", 24.500, -60.0, 178.0, -1.7727),
            ("EP", 49.378, 28.0, -114.0, -2.6496),
            ("EP", 49.378, -60.0, 178.0, -1.4081),
            ("EP", 129.371, 36.0, -120.0, -1.2612),
            ("EP", 129.371, 44.0, 178.0, -0.62031),
            ("EP", 159.621, 36.0, -120.0, -1.1268),
            ("EP", 159.621, 40.0, 178.0, -0.65263),
            ("EP", 649.935, 42.0, -124.0, -0.79302),
            ("EP", 649.935, -18.0, 178.0, -0.42747),
            ("EP", 1423.965, 2.0, 104.0, 0.40803),
            ("EP", 1423.965, 10.0, 178.0, -0.3615),
            ("EP", 2737.858, -14.0, 14.0, -0.60972),
            ("EP", 2737.858, 2.0, 178.0, -0.44267),
            ("EP", 2890.500, -12.0, 10.0, -0.96076),
            ("EP", 2890.500, 4.0, 178.0, -0.78381),
        ];
        let es = SphModel::from_bytes(ES).unwrap();
        let ep = SphModel::from_bytes(EP).unwrap();
        for (which, depth, lat, lon, want) in REF {
            let m = if which == "ES" { &es } else { &ep };
            let got = 100.0 * m.dlnv(6371.0 - depth, lat, lon);
            // Half a unit in the 5th printed digit is at most 5e-5 %.
            assert!(
                (got - want).abs() < 1e-4,
                "{which} {depth} km ({lat}, {lon}): {got} % vs the authors' {want} %"
            );
        }
    }

    #[test]
    fn known_structures_have_the_right_sign() {
        let m = MantleModel::from_bytes(ES, EP).unwrap();
        let r = |depth: f64| 6371.0 - depth;
        // Fast cratonic root under the Canadian shield; slow under the East Pacific Rise;
        // slow lowermost mantle under Africa and the Pacific (the two large low-velocity
        // provinces).
        assert!(m.dlnvs(r(150.0), 60.0, -100.0) > 0.02);
        assert!(m.dlnvs(r(100.0), -15.0, -112.0) < -0.01);
        assert!(m.dlnvs(r(2800.0), -10.0, 20.0) < -0.005);
        assert!(m.dlnvs(r(2800.0), -10.0, -160.0) < -0.005);
    }

    #[test]
    fn grid_build_and_lookup_speed() {
        // Generous bounds for an unoptimised test build; prints the actual times.
        let t0 = std::time::Instant::now();
        let m = MantleModel::from_bytes(ES, EP).unwrap();
        let build = t0.elapsed();
        let t1 = std::time::Instant::now();
        let mut acc = 0.0;
        for i in 0..1_000_000 {
            let f = i as f64;
            acc += m.dlnvs(
                3500.0 + (f * 0.37) % 2800.0,
                (f * 0.11) % 180.0 - 90.0,
                (f * 0.73) % 360.0 - 180.0,
            );
        }
        let lookups = t1.elapsed();
        println!("grid build {build:?}, 1e6 lookups {lookups:?} (sum {acc:.3})");
        assert!(build.as_secs_f64() < 20.0, "grid build {build:?}");
        assert!(lookups.as_secs_f64() < 10.0, "1e6 lookups {lookups:?}");
    }

    #[test]
    fn longitude_wraps() {
        let m = MantleModel::from_bytes(ES, EP).unwrap();
        let r = 5000.0;
        assert!((m.dlnvs(r, 10.0, 179.9) - m.dlnvs(r, 10.0, -180.1)).abs() < 1e-12);
        assert!((m.dlnvs(r, 10.0, 180.0) - m.dlnvs(r, 10.0, -180.0)).abs() < 1e-12);
    }

    #[test]
    fn rejects_bad_files() {
        assert!(SphModel::from_bytes(b"").is_err());
        assert!(SphModel::from_bytes(b"12 1111111111111 24 000111\n1.0\n").is_err());
        // Right header, too few coefficients.
        let short = format!("1 11 24 000{}\n0.1 0.2\n", "1".repeat(21));
        assert!(SphModel::from_bytes(short.as_bytes()).is_err());
        // A missing degree.
        let gap = format!("1 10 24 000{}\n", "1".repeat(21));
        assert!(SphModel::from_bytes(gap.as_bytes()).is_err());
    }
}
