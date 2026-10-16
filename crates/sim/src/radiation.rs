//! The radiation pattern of an earthquake: how strongly, and with which sign, a fault that
//! slipped sends P, SV and SH waves in each direction.
//!
//! A fault is a double couple described by one of its two nodal planes (Aki & Richards 2002,
//! box 4.4): strike (degrees clockwise from north, the fault dipping to the right when looking
//! along it), dip (degrees down from horizontal) and rake (degrees, the hanging wall's slip
//! direction measured in the fault plane from the strike direction; 90 = thrust, −90 = normal,
//! 0 = left-lateral, 180 = right-lateral). Its moment tensor of unit scalar moment, in
//! north-east-down axes, gives the far-field coefficients of a ray leaving the source with
//! take-off angle i (from straight down) towards azimuth φ (Aki & Richards eq. 4.29):
//!
//! ```text
//! P  = γ·M·γ      SV = p̂·M·γ      SH = φ̂·M·γ
//! γ = (sin i cos φ, sin i sin φ, cos i)      ray direction
//! p̂ = (cos i cos φ, cos i sin φ, −sin i)     SV: towards larger take-off angle
//! φ̂ = (−sin φ, cos φ, 0)                    SH: clockwise seen from above
//! ```
//!
//! Averaged over every direction, P² is 4/15 and SV² and SH² are 1/5 each for any double
//! couple; the coefficients returned here are divided by those root-mean-squares, so a fault
//! only redistributes energy between directions and on average leaves a trace as strong as
//! the even source it replaces. Only the double couple is used: the non-double-couple part of
//! a published moment tensor is ignored (see `docs/approximations.md`).

use serde::{Deserialize, Serialize};

/// A fault's orientation and slip: one nodal plane of a double couple, in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FocalMechanism {
    /// [0, 360), clockwise from north.
    pub strike_deg: f64,
    /// [0, 90].
    pub dip_deg: f64,
    /// (−180, 180].
    pub rake_deg: f64,
}

/// The kind of faulting, from the rake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultStyle {
    /// One side pushed up and over the other (rake near 90°).
    Thrust,
    /// One side slid down away from the other (rake near −90°).
    Normal,
    /// The sides slid past each other sideways (rake near 0° or 180°).
    StrikeSlip,
    /// Between thrust and strike-slip.
    ObliqueThrust,
    /// Between normal and strike-slip.
    ObliqueNormal,
}

impl FaultStyle {
    /// Its name: "thrust", "strike-slip", ...
    pub fn name(self) -> &'static str {
        match self {
            FaultStyle::Thrust => "thrust",
            FaultStyle::Normal => "normal",
            FaultStyle::StrikeSlip => "strike-slip",
            FaultStyle::ObliqueThrust => "oblique thrust",
            FaultStyle::ObliqueNormal => "oblique normal",
        }
    }

    /// What happened, in plain words, for the event panel.
    pub fn describe(self) -> &'static str {
        match self {
            FaultStyle::Thrust => "one side pushed up over the other",
            FaultStyle::Normal => "one side slid down, pulling apart",
            FaultStyle::StrikeSlip => "the sides slid past each other",
            FaultStyle::ObliqueThrust => "pushed up and sideways",
            FaultStyle::ObliqueNormal => "slid down and sideways",
        }
    }
}

/// Signed far-field coefficients of one ray, each divided by its root-mean-square over all
/// directions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radiation {
    /// Positive: the ground first moves away from the source (compression).
    pub p: f64,
    /// Positive: along the direction of larger take-off angle.
    pub sv: f64,
    /// Positive: clockwise seen from above (towards the right of the ray's way).
    pub sh: f64,
}

impl Radiation {
    /// What a source with no known fault radiates: the same in every direction.
    pub const EVEN: Radiation = Radiation {
        p: 1.0,
        sv: 1.0,
        sh: 1.0,
    };
}

/// Root-mean-square of the P coefficient of a unit double couple over all directions: √(4/15).
const P_RMS: f64 = 0.516_397_779_494_322_3;
/// Root-mean-square of the SV and of the SH coefficient over all directions and orientations:
/// √(1/5). (For one fault the two can share the total 2/5 unevenly; their sum is exact.)
const S_RMS: f64 = 0.447_213_595_499_958;

impl FocalMechanism {
    /// A mechanism with its angles brought into range: strike into [0, 360), rake into
    /// (−180, 180], dip clamped to [0, 90].
    pub fn new(strike_deg: f64, dip_deg: f64, rake_deg: f64) -> Self {
        // Angles already in range are kept exactly as given.
        let rake = if rake_deg > -180.0 && rake_deg <= 180.0 {
            rake_deg
        } else {
            (rake_deg + 180.0).rem_euclid(360.0) - 180.0
        };
        Self {
            strike_deg: strike_deg.rem_euclid(360.0) % 360.0,
            dip_deg: dip_deg.clamp(0.0, 90.0),
            rake_deg: if rake == -180.0 { 180.0 } else { rake },
        }
    }

    pub fn style(&self) -> FaultStyle {
        let r = self.rake_deg;
        match r.abs() {
            a if a <= 30.0 || a >= 150.0 => FaultStyle::StrikeSlip,
            a if (60.0..=120.0).contains(&a) => {
                if r > 0.0 {
                    FaultStyle::Thrust
                } else {
                    FaultStyle::Normal
                }
            }
            _ if r > 0.0 => FaultStyle::ObliqueThrust,
            _ => FaultStyle::ObliqueNormal,
        }
    }

    /// The moment tensor of unit scalar moment, north-east-down axes (Aki & Richards box 4.4).
    pub fn tensor(&self) -> [[f64; 3]; 3] {
        let (s, d, r) = (
            self.strike_deg.to_radians(),
            self.dip_deg.to_radians(),
            self.rake_deg.to_radians(),
        );
        let (sin_d, cos_d, sin_2d, cos_2d) = (d.sin(), d.cos(), (2.0 * d).sin(), (2.0 * d).cos());
        let (sin_r, cos_r) = (r.sin(), r.cos());
        let xx = -(sin_d * cos_r * (2.0 * s).sin() + sin_2d * sin_r * s.sin().powi(2));
        let xy = sin_d * cos_r * (2.0 * s).cos() + 0.5 * sin_2d * sin_r * (2.0 * s).sin();
        let xz = -(cos_d * cos_r * s.cos() + cos_2d * sin_r * s.sin());
        let yy = sin_d * cos_r * (2.0 * s).sin() - sin_2d * sin_r * s.cos().powi(2);
        let yz = -(cos_d * cos_r * s.sin() - cos_2d * sin_r * s.cos());
        let zz = sin_2d * sin_r;
        [[xx, xy, xz], [xy, yy, yz], [xz, yz, zz]]
    }

    /// The coefficients of a ray leaving with take-off angle `takeoff_rad` (from straight down,
    /// 0 to π) towards `azimuth_deg` (clockwise from north).
    pub fn radiation(&self, takeoff_rad: f64, azimuth_deg: f64) -> Radiation {
        let m = self.tensor();
        let (si, ci) = takeoff_rad.sin_cos();
        let (sf, cf) = azimuth_deg.to_radians().sin_cos();
        let gamma = [si * cf, si * sf, ci];
        let p_hat = [ci * cf, ci * sf, -si];
        let phi_hat = [-sf, cf, 0.0];
        let mg: [f64; 3] = std::array::from_fn(|i| (0..3).map(|j| m[i][j] * gamma[j]).sum());
        let dot = |a: [f64; 3]| (0..3).map(|i| a[i] * mg[i]).sum::<f64>();
        Radiation {
            p: dot(gamma) / P_RMS,
            sv: dot(p_hat) / S_RMS,
            sh: dot(phi_hat) / S_RMS,
        }
    }

    /// The beach ball: at a point (`east`, `north`) of the unit disk, an equal-area projection
    /// of the lower half of the focal sphere, whether the P wave leaving that way starts with a
    /// push (`Some(true)`, drawn filled) or a pull (`Some(false)`); `None` outside the disk.
    pub fn compressional_at(&self, east: f64, north: f64) -> Option<bool> {
        let r = east.hypot(north);
        if r > 1.0 {
            return None;
        }
        // Equal-area (Schmidt): r = √2 · sin(i / 2), so the rim is horizontal (i = 90°).
        let takeoff = 2.0 * (r / std::f64::consts::SQRT_2).asin();
        let azimuth = east.atan2(north).to_degrees();
        Some(self.radiation(takeoff, azimuth).p > 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Midpoint sums over the sphere of directions, `n` take-off by `2n` azimuth cells.
    fn sphere_mean(n: usize, f: impl Fn(f64, f64) -> f64) -> f64 {
        let (mut sum, mut weight) = (0.0, 0.0);
        for a in 0..n {
            let i = (a as f64 + 0.5) / n as f64 * PI;
            for b in 0..2 * n {
                let az = (b as f64 + 0.5) / (2 * n) as f64 * 360.0;
                sum += f(i, az) * i.sin();
                weight += i.sin();
            }
        }
        sum / weight
    }

    #[test]
    fn tensor_is_a_unit_double_couple() {
        for (s, d, r) in [(0.0, 90.0, 0.0), (192.0, 8.0, 78.0), (40.0, 55.0, -120.0)] {
            let m = FocalMechanism::new(s, d, r).tensor();
            let trace = m[0][0] + m[1][1] + m[2][2];
            let norm2: f64 = m.iter().flatten().map(|v| v * v).sum();
            assert!(trace.abs() < 1e-12, "{s} {d} {r}: trace {trace}");
            // Scalar moment √(ΣM²/2) = 1.
            assert!((norm2 - 2.0).abs() < 1e-12, "{s} {d} {r}: {norm2}");
        }
    }

    /// The closed forms of Aki & Richards eq. 4.91 (φ measured from the strike), derived
    /// independently of the tensor projection.
    fn closed_form(m: &FocalMechanism, i: f64, az: f64) -> (f64, f64, f64) {
        let (d, r) = (m.dip_deg.to_radians(), m.rake_deg.to_radians());
        let phi = (az - m.strike_deg).to_radians();
        let p = r.cos() * d.sin() * i.sin().powi(2) * (2.0 * phi).sin()
            - r.cos() * d.cos() * (2.0 * i).sin() * phi.cos()
            + r.sin() * (2.0 * d).sin() * (i.cos().powi(2) - i.sin().powi(2) * phi.sin().powi(2))
            + r.sin() * (2.0 * d).cos() * (2.0 * i).sin() * phi.sin();
        let sv = r.sin() * (2.0 * d).cos() * (2.0 * i).cos() * phi.sin()
            - r.cos() * d.cos() * (2.0 * i).cos() * phi.cos()
            + 0.5 * r.cos() * d.sin() * (2.0 * i).sin() * (2.0 * phi).sin()
            - 0.5 * r.sin() * (2.0 * d).sin() * (2.0 * i).sin() * (1.0 + phi.sin().powi(2));
        let sh = r.cos() * d.cos() * i.cos() * phi.sin()
            + r.cos() * d.sin() * i.sin() * (2.0 * phi).cos()
            + r.sin() * (2.0 * d).cos() * i.cos() * phi.cos()
            - 0.5 * r.sin() * (2.0 * d).sin() * i.sin() * (2.0 * phi).sin();
        (p, sv, sh)
    }

    #[test]
    fn projection_matches_the_closed_forms() {
        for (s, d, r) in [
            (0.0, 90.0, 0.0),
            (192.0, 8.0, 78.0),
            (40.0, 55.0, -120.0),
            (300.0, 30.0, 170.0),
        ] {
            let m = FocalMechanism::new(s, d, r);
            for i in [0.1, 0.6, 1.2, 1.7, 2.5, 3.0] {
                for az in [0.0, 37.0, 135.0, 250.0, 333.0] {
                    let got = m.radiation(i, az);
                    let (p, sv, sh) = closed_form(&m, i, az);
                    let tag = format!("{s}/{d}/{r} i {i} az {az}");
                    assert!((got.p * P_RMS - p).abs() < 1e-12, "P {tag}");
                    assert!((got.sv * S_RMS - sv).abs() < 1e-12, "SV {tag}");
                    assert!((got.sh * S_RMS - sh).abs() < 1e-12, "SH {tag}");
                }
            }
        }
    }

    #[test]
    fn both_nodal_planes_radiate_the_same() {
        // A published pair (USGS focal mechanism of the 2011 Tohoku earthquake): exact halves
        // of one double couple.
        let (a, b) = (
            FocalMechanism::new(25.0, 80.0, 90.0),
            FocalMechanism::new(205.0, 10.0, 90.0),
        );
        for i in [0.2, 0.9, 1.5, 2.4] {
            for az in [10.0, 100.0, 200.0, 290.0] {
                let (x, y) = (a.radiation(i, az), b.radiation(i, az));
                assert!((x.p - y.p).abs() < 1e-9, "P {i} {az}");
                assert!((x.sv - y.sv).abs() < 1e-9, "SV {i} {az}");
                assert!((x.sh - y.sh).abs() < 1e-9, "SH {i} {az}");
            }
        }
    }

    /// USGS's own moment tensor for the 2011 Tohoku earthquake (Mww, Duputel; product
    /// `moment-tensor` of event usp000hvnu, units of 10²² N·m) in up-south-east axes, against
    /// the tensor built from its first nodal plane 192/8/78. A wrong sign of the strike or the
    /// azimuth drops the match to about 0.91.
    #[test]
    fn matches_the_published_tohoku_tensor() {
        let (rr, tt, pp, rt, rp, tp) = (1.674, -0.1509, -1.523, 2.243, 4.837, -0.5286);
        // r = up, t = south, p = east  →  x = north = −t, y = east = p, z = down = −r.
        let usgs = [[tt, -tp, rt], [-tp, pp, -rp], [rt, -rp, rr]];
        let ours = FocalMechanism::new(192.0, 8.0, 78.0).tensor();
        let dot = |a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]| -> f64 {
            (0..3)
                .flat_map(|i| (0..3).map(move |j| (i, j)))
                .map(|(i, j)| a[i][j] * b[i][j])
                .sum()
        };
        let cos = dot(&usgs, &ours) / (dot(&usgs, &usgs) * dot(&ours, &ours)).sqrt();
        assert!(cos > 0.99, "{cos}");
    }

    #[test]
    fn averaged_over_directions_the_source_is_as_strong_as_an_even_one() {
        for (s, d, r) in [(0.0, 90.0, 0.0), (192.0, 8.0, 78.0), (40.0, 55.0, -120.0)] {
            let m = FocalMechanism::new(s, d, r);
            let p2 = sphere_mean(90, |i, az| m.radiation(i, az).p.powi(2));
            let s2 = sphere_mean(90, |i, az| {
                let x = m.radiation(i, az);
                x.sv * x.sv + x.sh * x.sh
            });
            assert!((p2 - 1.0).abs() < 1e-3, "{s}/{d}/{r}: ⟨P²⟩ {p2}");
            assert!((s2 - 2.0).abs() < 2e-3, "{s}/{d}/{r}: ⟨SV²+SH²⟩ {s2}");
        }
    }

    #[test]
    fn vertical_strike_slip_has_the_textbook_quadrants() {
        // Strike north, vertical, rake 0: left-lateral. P = sin²i · sin 2φ.
        let m = FocalMechanism::new(0.0, 90.0, 0.0);
        let h = std::f64::consts::FRAC_PI_2;
        // Nodal along the fault and across it.
        for az in [0.0, 90.0, 180.0, 270.0] {
            assert!(m.radiation(h, az).p.abs() < 1e-12, "{az}");
        }
        // The east side moves north, towards a point to the north-east: a push there.
        assert!(m.radiation(h, 45.0).p > 0.0);
        assert!(m.radiation(h, 135.0).p < 0.0);
        assert!(m.radiation(h, 225.0).p > 0.0);
        assert!((m.radiation(h, 45.0).p * P_RMS - 1.0).abs() < 1e-12);
        // SH is largest along the fault, where P is nodal.
        assert!((m.radiation(h, 0.0).sh * S_RMS).abs() > 0.999);
        // Turning the slip round flips every sign.
        let flipped = FocalMechanism::new(0.0, 90.0, 180.0);
        let (a, b) = (m.radiation(1.0, 60.0), flipped.radiation(1.0, 60.0));
        assert!((a.p + b.p).abs() < 1e-12 && (a.sv + b.sv).abs() < 1e-12);
        assert!((a.sh + b.sh).abs() < 1e-12);
    }

    #[test]
    fn beach_balls_of_thrust_and_normal_faults() {
        // A thrust pushes straight down (filled centre), a normal fault pulls (empty centre).
        let thrust = FocalMechanism::new(192.0, 8.0, 78.0);
        let normal = FocalMechanism::new(192.0, 45.0, -90.0);
        assert_eq!(thrust.compressional_at(0.0, 0.0), Some(true));
        assert_eq!(normal.compressional_at(0.0, 0.0), Some(false));
        assert_eq!(thrust.compressional_at(0.8, 0.7), None);
        // The rim is horizontal: take-off 90°.
        let m = FocalMechanism::new(0.0, 90.0, 0.0);
        let rim = std::f64::consts::FRAC_1_SQRT_2;
        assert_eq!(m.compressional_at(rim, rim), Some(true));
        assert_eq!(m.compressional_at(rim, -rim), Some(false));
    }

    #[test]
    fn angles_are_normalised_and_styled() {
        let m = FocalMechanism::new(-30.0, 95.0, 190.0);
        assert_eq!((m.strike_deg, m.dip_deg), (330.0, 90.0));
        assert!((m.rake_deg - -170.0).abs() < 1e-12);
        assert_eq!(FocalMechanism::new(0.0, 45.0, -180.0).rake_deg, 180.0);
        assert_eq!(FocalMechanism::new(360.0, 45.0, 0.0).strike_deg, 0.0);
        let style = |r| FocalMechanism::new(0.0, 45.0, r).style();
        assert_eq!(style(90.0), FaultStyle::Thrust);
        assert_eq!(style(-80.0), FaultStyle::Normal);
        assert_eq!(style(10.0), FaultStyle::StrikeSlip);
        assert_eq!(style(-170.0), FaultStyle::StrikeSlip);
        assert_eq!(style(45.0), FaultStyle::ObliqueThrust);
        assert_eq!(style(-135.0), FaultStyle::ObliqueNormal);
    }
}
