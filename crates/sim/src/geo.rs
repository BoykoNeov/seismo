//! Spherical-Earth geometry.

/// Mean Earth radius (km) of the spherical model.
pub const EARTH_RADIUS_KM: f64 = 6371.0;

/// Great-circle distance in degrees of arc between two points given in degrees.
///
/// Spherical Earth, geographic latitude used directly on the sphere (the ObsPy
/// `locations2degrees` convention). Uses the atan2 (Vincenty, sphere) form, which stays
/// accurate from 0° through 180° where the haversine and arccos forms lose precision.
pub fn great_circle_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dl = (lon2 - lon1).to_radians();
    let (s1, c1) = p1.sin_cos();
    let (s2, c2) = p2.sin_cos();
    let (sdl, cdl) = dl.sin_cos();
    let y = ((c2 * sdl).powi(2) + (c1 * s2 - s1 * c2 * cdl).powi(2)).sqrt();
    let x = s1 * s2 + c1 * c2 * cdl;
    y.atan2(x).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn same_point_is_zero() {
        assert_eq!(great_circle_deg(12.3, 45.6, 12.3, 45.6), 0.0);
        // At the pole longitude is meaningless; cos(90°) is not exactly 0 in floating point.
        assert!(close(great_circle_deg(90.0, 0.0, 90.0, 120.0), 0.0, 1e-12));
    }

    #[test]
    fn antipodes_are_180() {
        assert!(close(great_circle_deg(0.0, 0.0, 0.0, 180.0), 180.0, 1e-12));
        assert!(close(
            great_circle_deg(38.297, 142.373, -38.297, -37.627),
            180.0,
            1e-9
        ));
        assert!(close(great_circle_deg(90.0, 0.0, -90.0, 0.0), 180.0, 1e-12));
    }

    #[test]
    fn equator_quarter_turn() {
        assert!(close(great_circle_deg(0.0, 0.0, 0.0, 90.0), 90.0, 1e-12));
        assert!(close(great_circle_deg(0.0, -45.0, 0.0, 45.0), 90.0, 1e-12));
    }

    #[test]
    fn nearly_antipodal_stays_accurate() {
        // 1e-6° short of the antipode along the equator.
        let d = great_circle_deg(0.0, 0.0, 0.0, 180.0 - 1e-6);
        assert!(close(d, 180.0 - 1e-6, 1e-9), "{d}");
        // Off the equator: moving the antipode 1e-5° in latitude.
        let d = great_circle_deg(10.0, 20.0, -10.0 + 1e-5, -160.0);
        assert!(close(d, 180.0 - 1e-5, 1e-9), "{d}");
    }

    #[test]
    fn tiny_distances_stay_accurate() {
        let d = great_circle_deg(0.0, 0.0, 0.0, 1e-7);
        assert!(close(d, 1e-7, 1e-15), "{d}");
    }

    #[test]
    fn london_new_york() {
        let d = great_circle_deg(51.5074, -0.1278, 40.7128, -74.0060);
        assert!(close(d, 50.094, 0.001), "{d}");
    }

    #[test]
    fn symmetric_and_wraps_longitude() {
        let a = great_circle_deg(10.0, 170.0, -5.0, -170.0);
        let b = great_circle_deg(-5.0, -170.0, 10.0, 170.0);
        let c = great_circle_deg(10.0, 170.0, -5.0, 190.0);
        assert!(close(a, b, 1e-12) && close(a, c, 1e-12));
    }
}
