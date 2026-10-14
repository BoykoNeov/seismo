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

/// Azimuth (degrees clockwise from north, in [0, 360)) of the great circle from point 1 towards
/// point 2. Same formula as `distance_azimuth` in `map.wgsl`. 0 when the points coincide.
pub fn azimuth_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dl = (lon2 - lon1).to_radians();
    let az = (dl.sin() * p2.cos()).atan2(p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos());
    az.to_degrees().rem_euclid(360.0)
}

/// The point reached by travelling `dist_deg` of arc from (`lat`, `lon`) along the great circle
/// leaving at `azimuth_deg` (clockwise from north). Distances past 180° carry on round the
/// circle. Longitude in [-180, 180).
pub fn destination(lat: f64, lon: f64, azimuth_deg: f64, dist_deg: f64) -> (f64, f64) {
    let (p1, l1) = (lat.to_radians(), lon.to_radians());
    let (az, d) = (azimuth_deg.to_radians(), dist_deg.to_radians());
    let (sp1, cp1) = p1.sin_cos();
    let (sd, cd) = d.sin_cos();
    let sp2 = (sp1 * cd + cp1 * sd * az.cos()).clamp(-1.0, 1.0);
    let p2 = sp2.asin();
    let l2 = l1 + (az.sin() * sd * cp1).atan2(cd - sp1 * sp2);
    (p2.to_degrees(), normalise_lon(l2.to_degrees()))
}

/// Longitude wrapped into [-180, 180).
pub fn normalise_lon(lon: f64) -> f64 {
    (lon + 180.0).rem_euclid(360.0) - 180.0
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
    fn east_is_ninety_degrees() {
        assert!(close(azimuth_deg(0.0, 0.0, 0.0, 10.0), 90.0, 1e-12));
        assert!(close(azimuth_deg(0.0, 0.0, 0.0, -10.0), 270.0, 1e-12));
        assert!(close(azimuth_deg(0.0, 0.0, 10.0, 0.0), 0.0, 1e-12));
        let (lat, lon) = destination(0.0, 0.0, 90.0, 10.0);
        assert!(
            close(lat, 0.0, 1e-12) && close(lon, 10.0, 1e-12),
            "{lat} {lon}"
        );
    }

    #[test]
    fn destination_round_trips() {
        let (lat0, lon0) = (38.297, 142.373);
        for k in 0..24 {
            let az = 15.0 * k as f64;
            for d in [1.0, 30.0, 90.0, 150.0, 179.0] {
                let (lat, lon) = destination(lat0, lon0, az, d);
                assert!((-180.0..180.0).contains(&lon));
                assert!(
                    close(great_circle_deg(lat0, lon0, lat, lon), d, 1e-9),
                    "az {az} d {d}"
                );
                let back = azimuth_deg(lat0, lon0, lat, lon);
                let diff = (back - az + 540.0).rem_euclid(360.0) - 180.0;
                assert!(diff.abs() < 1e-7, "az {az} d {d}: {back}");
            }
        }
        // Past the antipode the path comes back along the opposite azimuth.
        let (lat, lon) = destination(lat0, lon0, 40.0, 200.0);
        assert!(close(great_circle_deg(lat0, lon0, lat, lon), 160.0, 1e-9));
        assert!(close(azimuth_deg(lat0, lon0, lat, lon), 220.0, 1e-7));
    }

    #[test]
    fn symmetric_and_wraps_longitude() {
        let a = great_circle_deg(10.0, 170.0, -5.0, -170.0);
        let b = great_circle_deg(-5.0, -170.0, 10.0, 170.0);
        let c = great_circle_deg(10.0, 170.0, -5.0, 190.0);
        assert!(close(a, b, 1e-12) && close(a, c, 1e-12));
    }
}
