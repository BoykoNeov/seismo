//! The Earth's flattening, as a correction to the seismograms' arrival times.
//!
//! The Earth spins, so it bulges at the equator: its surface and every layer inside it are
//! slightly flattened ellipsoids rather than spheres. A wave's travel time then differs from
//! the spherical model's by a second or two, depending on where it starts, which way it goes
//! and how far. To first order the difference is a degree-2 expansion (Dziewonski & Gilbert
//! 1976; Kennett & Gudmundsson 1996):
//!
//! δt = Σₘ σₘ(depth, Δ) · P₂ₘ(cos θ) · cos(m ζ),  m = 0, 1, 2,
//!
//! with θ the source's colatitude, ζ the azimuth from the source to the station and P₂ₘ the
//! Schmidt semi-normalised associated Legendre functions. The coefficients σₘ come from
//! EllipticiPy (Russell, Rudge, Irving and Cottaar 2022) through
//! [`EllipticityTable`]; like EllipticiPy itself, SEISMO puts its geographic latitude in for
//! θ (see `docs/approximations.md`). The travel-time table, the map and the slice stay
//! spherical; only the seismograms' arrival times move.

use crate::tables::EllipticityTable;

/// The flattening correction of each phase's travel times.
#[derive(Debug, Clone)]
pub struct Ellipticity {
    table: EllipticityTable,
}

impl Ellipticity {
    pub fn new(table: EllipticityTable) -> Self {
        Self { table }
    }

    /// The coefficients and where they come from.
    pub fn table(&self) -> &EllipticityTable {
        &self.table
    }

    /// Seconds to add to the spherical travel time of `phase` from a source at
    /// `source_lat_deg`, `depth_km` to a station `dist_deg` away at azimuth `azimuth_deg` (from
    /// the source, clockwise from north). 0 where the table has no coefficients for it.
    pub fn correction(
        &self,
        phase: &str,
        depth_km: f64,
        dist_deg: f64,
        source_lat_deg: f64,
        azimuth_deg: f64,
    ) -> f64 {
        self.table
            .coefficients(phase, depth_km, dist_deg)
            .map_or(0.0, |sigma| {
                from_coefficients(sigma, source_lat_deg, azimuth_deg)
            })
    }
}

/// The correction (s) from the three coefficients, the source's latitude and the azimuth to
/// the station (degrees).
pub fn from_coefficients(sigma: [f64; 3], source_lat_deg: f64, azimuth_deg: f64) -> f64 {
    let theta = (90.0 - source_lat_deg).to_radians();
    let (sin, cos) = theta.sin_cos();
    let zeta = azimuth_deg.to_radians();
    // P₂₀, P₂₁ and P₂₂ with Schmidt semi-normalisation: √((2 − δₘ₀)(2 − m)!/(2 + m)!).
    let p20 = 0.5 * (3.0 * cos * cos - 1.0);
    let p21 = (1.0f64 / 3.0).sqrt() * 3.0 * cos * sin;
    let p22 = (1.0f64 / 12.0).sqrt() * 3.0 * sin * sin;
    sigma[0] * p20 + sigma[1] * p21 * zeta.cos() + sigma[2] * p22 * (2.0 * zeta).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ellipticity() -> Ellipticity {
        Ellipticity::new(
            EllipticityTable::from_bytes(include_bytes!("../../../assets/ak135_ellip.bin"))
                .unwrap(),
        )
    }

    /// (phase, depth_km, distance_deg, source_latitude, azimuth_deg, correction_s), printed by
    /// `tools/gen_ellipticity.py` from EllipticiPy 1.0.1 with ObsPy's own ray path for each
    /// (`TauPyModel.get_ray_paths`, not the generator's grid). The first six are grid nodes,
    /// the rest lie between nodes.
    const ANCHORS: &[(&str, f64, f64, f64, f64, f64)] = &[
        ("P", 0.0, 60.0, 45.0, 30.0, -0.5118),
        ("S", 100.0, 80.0, -30.0, 120.0, 0.5077),
        ("PKPdf", 300.0, 150.0, -20.0, 200.0, 1.0375),
        ("PKPab", 0.0, 170.0, 70.0, 0.0, -1.6489),
        ("ScS", 50.0, 40.0, 10.0, 300.0, 0.9020),
        ("pP", 200.0, 50.0, 60.0, 90.0, -0.3540),
        ("P", 110.0, 62.5, 35.5, 45.0, -0.1996),
        ("SKS", 37.0, 101.3, -52.0, 250.0, 0.2168),
        ("PcP", 612.0, 33.7, 5.0, 170.0, 0.2832),
    ];

    #[test]
    fn matches_ellipticipy() {
        let e = ellipticity();
        for &(phase, depth, dist, lat, az, want) in ANCHORS {
            let got = e.correction(phase, depth, dist, lat, az);
            // On nodes only the f32 storage and the rounding of the printed value differ;
            // between nodes the interpolation adds a little. The whole correction is about a
            // second, so the usual half-second tolerance would test nothing.
            let on_node = depth % 25.0 == 0.0 && dist.fract() == 0.0;
            let tol = if on_node { 0.001 } else { 0.05 };
            assert!(
                (got - want).abs() < tol,
                "{phase} {depth} km {dist}° lat {lat} az {az}: {got:.4} s, EllipticiPy {want} s"
            );
        }
    }

    #[test]
    fn legendre_weights() {
        // At the north pole only m = 0 is left, P₂₀ = 1; on the equator P₂₀ = −1/2 and
        // P₂₂ = √3/2; m = 1 vanishes at both and peaks at 45°.
        assert!((from_coefficients([1.0, 5.0, 5.0], 90.0, 30.0) - 1.0).abs() < 1e-12);
        let s3 = 3.0f64.sqrt();
        assert!((from_coefficients([1.0, 5.0, 0.0], 0.0, 0.0) + 0.5).abs() < 1e-12);
        assert!((from_coefficients([0.0, 0.0, 1.0], 0.0, 0.0) - s3 / 2.0).abs() < 1e-12);
        assert!((from_coefficients([0.0, 0.0, 1.0], 0.0, 90.0) + s3 / 2.0).abs() < 1e-12);
        assert!((from_coefficients([0.0, 1.0, 0.0], 45.0, 0.0) - s3 / 2.0).abs() < 1e-12);
        assert!((from_coefficients([0.0, 1.0, 0.0], 45.0, 180.0) + s3 / 2.0).abs() < 1e-12);
    }

    #[test]
    fn nothing_where_the_phase_does_not_arrive() {
        let e = ellipticity();
        // Direct P is gone in the shadow of the core, PKIKP does not reach 60°.
        assert_eq!(e.correction("P", 0.0, 130.0, 45.0, 30.0), 0.0);
        assert_eq!(e.correction("PKPdf", 0.0, 60.0, 45.0, 30.0), 0.0);
        assert_eq!(e.correction("no such phase", 0.0, 60.0, 45.0, 30.0), 0.0);
    }
}
