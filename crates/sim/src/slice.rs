//! The vertical slice: ray fans per phase from one source depth, and the wavefronts and ray
//! paths they give at a time.
//!
//! The slice shows half of the great circle through the epicentre at one azimuth: from the
//! epicentre (δ = 0°) along that azimuth to the antipode (δ = 180°). Positions are radius and
//! epicentral distance δ. That half holds the rays leaving along the azimuth until they pass the
//! antipode (the forward fans), and the rays leaving the opposite way once they have passed it
//! (the back fans, folded onto δ = 360° − θ). In a 1D Earth the two are the same rays.

use crate::geo::EARTH_RADIUS_KM;
use crate::raytrace::{PathPoint, Ray, RayPhase, RayTracer, Wave, fold_rad};
use crate::tables::EarthModel;

use std::f64::consts::{FRAC_PI_2, PI};

/// Initial take-off angle spacing (rad) of a fan before refinement: 0.5°.
const BASE_STEP_RAD: f64 = PI / 360.0;
/// Neighbouring rays are split until their surface end points and their bottom points are
/// within this distance (km) ...
const REFINE_KM: f64 = 60.0;
/// ... or until their take-off angles are `BASE_STEP_RAD / 2^MAX_LEVEL` apart.
const MAX_LEVEL: u32 = 10;
/// Where a fan ends (one ray valid, its neighbour not), bisect this many times to find the
/// edge.
const EDGE_LEVEL: u32 = 14;
/// A wavefront line is broken between neighbouring rays farther apart than this (km).
const BREAK_KM: f64 = 400.0;

/// A point in the slice: radius (km) and epicentral distance (deg, 0–180).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlicePoint {
    pub r_km: f64,
    pub delta_deg: f64,
}

impl SlicePoint {
    fn from_path(q: &PathPoint) -> Self {
        Self {
            r_km: q.r_km,
            delta_deg: fold_rad(q.theta_rad).to_degrees(),
        }
    }

    fn chord_km(&self, o: &SlicePoint) -> f64 {
        let (a, b) = (self.delta_deg.to_radians(), o.delta_deg.to_radians());
        (self.r_km * a.cos() - o.r_km * b.cos()).hypot(self.r_km * a.sin() - o.r_km * b.sin())
    }
}

/// A polyline in the slice with one wave type throughout.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceLine {
    pub phase: RayPhase,
    pub wave: Wave,
    pub points: Vec<SlicePoint>,
}

/// Where to write a phase's name: a point on its front, on the leg that gives it its name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhaseLabel {
    pub phase: RayPhase,
    pub at: SlicePoint,
}

/// Wavefronts at one time.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SliceFronts {
    pub lines: Vec<SliceLine>,
    pub labels: Vec<PhaseLabel>,
}

/// A named velocity discontinuity to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Boundary {
    pub name: &'static str,
    pub depth_km: f64,
}

/// Named boundaries of the model: the Moho, 410 and 660 (the model discontinuities nearest
/// their nominal depths) and the core-mantle and inner-core boundaries (the ends of the fluid
/// core). A nominal boundary with no model discontinuity within 10 km is left out.
pub fn named_boundaries(model: &EarthModel) -> Vec<Boundary> {
    let disc = model.discontinuities();
    let mut out: Vec<Boundary> = [("Moho", 35.0), ("410", 410.0), ("660", 660.0)]
        .into_iter()
        .filter_map(|(name, nominal)| {
            disc.iter()
                .copied()
                .min_by(|a, b| (a - nominal).abs().total_cmp(&(b - nominal).abs()))
                .filter(|d| (d - nominal).abs() <= 10.0)
                .map(|depth_km| Boundary { name, depth_km })
        })
        .collect();
    if let Some((cmb, icb)) = model.fluid_core() {
        out.push(Boundary {
            name: "CMB",
            depth_km: cmb,
        });
        out.push(Boundary {
            name: "ICB",
            depth_km: icb,
        });
    }
    out
}

/// Which way a fan's rays leave the source, relative to the slice's azimuth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Along the azimuth: drawn while the ray is between the epicentre and the antipode.
    Forward,
    /// Opposite the azimuth: drawn once the ray has passed the antipode.
    Back,
}

impl Side {
    /// Whether a ray point `theta_rad` from the source along its own great circle lies in the
    /// slice's half.
    pub fn shows(self, theta_rad: f64) -> bool {
        let x = theta_rad.rem_euclid(2.0 * PI);
        match self {
            Side::Forward => x <= PI,
            Side::Back => x > PI,
        }
    }
}

/// The rays of one phase leaving one way, ordered by take-off angle.
#[derive(Debug, Clone, PartialEq)]
pub struct Fan {
    pub phase: RayPhase,
    pub side: Side,
    pub rays: Vec<Ray>,
}

/// Ray fans of every slice phase for one source depth, forward fans first. Build once per depth
/// and reuse: in the 1D model the fans depend only on the model and the depth.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceFans {
    pub depth_km: f64,
    r_cmb: f64,
    pub fans: Vec<Fan>,
}

impl SliceFans {
    /// Fans through the 1D model: the back fans are the forward fans' rays.
    pub fn build(tracer: &RayTracer, depth_km: f64) -> Self {
        let forward: Vec<Fan> = RayPhase::ALL
            .into_iter()
            .map(|phase| Fan {
                phase,
                side: Side::Forward,
                rays: build_fan(tracer, phase, depth_km),
            })
            .collect();
        let back: Vec<Fan> = forward
            .iter()
            .map(|f| Fan {
                side: Side::Back,
                ..f.clone()
            })
            .collect();
        Self::from_fans(tracer, depth_km, forward.into_iter().chain(back).collect())
    }

    /// Fans traced elsewhere (forward fans first).
    pub fn from_fans(tracer: &RayTracer, depth_km: f64, fans: Vec<Fan>) -> Self {
        Self {
            depth_km,
            r_cmb: tracer.cmb_radius_km(),
            fans,
        }
    }

    fn wave_at(&self, phase: RayPhase, r_km: f64) -> Wave {
        if r_km > self.r_cmb {
            phase.mantle_wave()
        } else {
            Wave::P
        }
    }

    /// Wavefronts `t` seconds after origin. Each fan's front joins the positions of
    /// neighbouring rays at `t`; it breaks where a ray has finished (reached the surface), is
    /// still on a path another fan draws, or is outside the slice's half, and where neighbours
    /// are far apart. Each phase is labelled once, on its forward fan if that shows it.
    pub fn fronts_at(&self, t: f64) -> SliceFronts {
        let mut out = SliceFronts::default();
        if t < 0.0 {
            return out;
        }
        for fan in &self.fans {
            let mut current: Vec<SlicePoint> = Vec::new();
            let mut current_wave = fan.phase.mantle_wave();
            let mut labelled: Vec<SlicePoint> = Vec::new();
            let flush = |pts: &mut Vec<SlicePoint>, wave: Wave, out: &mut SliceFronts| {
                if pts.len() >= 2 {
                    out.lines.push(SliceLine {
                        phase: fan.phase,
                        wave,
                        points: std::mem::take(pts),
                    });
                } else {
                    pts.clear();
                }
            };
            for ray in &fan.rays {
                let here = (t >= ray.shared_until_s)
                    .then(|| ray.point_at(t))
                    .flatten()
                    .filter(|q| fan.side.shows(q.theta_rad))
                    .map(|q| SlicePoint::from_path(&q));
                let Some(pt) = here else {
                    flush(&mut current, current_wave, &mut out);
                    continue;
                };
                if t >= ray.distinct_after_s {
                    labelled.push(pt);
                }
                let wave = self.wave_at(fan.phase, pt.r_km);
                if let Some(last) = current.last().copied() {
                    if last.chord_km(&pt) > BREAK_KM {
                        flush(&mut current, current_wave, &mut out);
                    } else if wave != current_wave {
                        // Change of line style: start the new line from the previous point so
                        // the front stays connected.
                        flush(&mut current, current_wave, &mut out);
                        current.push(last);
                    }
                }
                current_wave = wave;
                current.push(pt);
            }
            flush(&mut current, current_wave, &mut out);
            if !labelled.is_empty() && !out.labels.iter().any(|l| l.phase == fan.phase) {
                out.labels.push(PhaseLabel {
                    phase: fan.phase,
                    at: labelled[labelled.len() / 2],
                });
            }
        }
        out
    }

    /// The part of each fan's rays travelled by time `t`, for about `per_fan` rays per fan
    /// spread evenly through it, inside the slice's half. Lines break where the wave type
    /// changes.
    pub fn ray_paths_until(&self, t: f64, per_fan: usize) -> Vec<SliceLine> {
        let mut out = Vec::new();
        if t <= 0.0 || per_fan == 0 {
            return out;
        }
        for fan in &self.fans {
            let n = fan.rays.len();
            if n == 0 {
                continue;
            }
            let stride = n.div_ceil(per_fan).max(1);
            for ray in fan.rays.iter().skip(stride / 2).step_by(stride) {
                let mut pts: Vec<PathPoint> = ray
                    .points
                    .iter()
                    .take_while(|q| q.t_s < t)
                    .copied()
                    .collect();
                if let Some(q) = ray.point_at(t) {
                    pts.push(q);
                }
                let mut line: Vec<SlicePoint> = Vec::new();
                let mut wave = fan.phase.mantle_wave();
                for w in pts.windows(2) {
                    if !(fan.side.shows(w[0].theta_rad) && fan.side.shows(w[1].theta_rad)) {
                        if line.len() >= 2 {
                            out.push(SliceLine {
                                phase: fan.phase,
                                wave,
                                points: std::mem::take(&mut line),
                            });
                        }
                        line.clear();
                        continue;
                    }
                    let mid = 0.5 * (w[0].r_km + w[1].r_km);
                    let seg_wave = self.wave_at(fan.phase, mid);
                    if seg_wave != wave && !line.is_empty() {
                        out.push(SliceLine {
                            phase: fan.phase,
                            wave,
                            points: std::mem::take(&mut line),
                        });
                    }
                    wave = seg_wave;
                    if line.is_empty() {
                        line.push(SlicePoint::from_path(&w[0]));
                    }
                    line.push(SlicePoint::from_path(&w[1]));
                }
                if line.len() >= 2 {
                    out.push(SliceLine {
                        phase: fan.phase,
                        wave,
                        points: line,
                    });
                }
            }
        }
        out
    }
}

/// Trace a fan: rays at regular take-off angles, then midpoints inserted between neighbours
/// whose end or bottom points are far apart, and bisection towards the fan's edges.
pub(crate) fn build_fan(tracer: &RayTracer, phase: RayPhase, depth_km: f64) -> Vec<Ray> {
    let max_angle = if phase.has_upgoing() && depth_km > 0.0 {
        PI
    } else {
        FRAC_PI_2
    };
    let n = (max_angle / BASE_STEP_RAD).round() as usize;
    let mut samples: Vec<(f64, Option<Ray>)> = (0..=n)
        .map(|k| {
            let a = max_angle * k as f64 / n as f64;
            (a, tracer.trace(phase, depth_km, a))
        })
        .collect();

    let min_gap = |level: u32| BASE_STEP_RAD / f64::from(1u32 << level) * 1.0001;
    loop {
        let mut next: Vec<(f64, Option<Ray>)> = Vec::with_capacity(samples.len() * 2);
        let mut inserted = false;
        for i in 0..samples.len() {
            if i > 0 {
                let (a0, r0) = &samples[i - 1];
                let (a1, r1) = &samples[i];
                let gap = a1 - a0;
                let split = match (r0, r1) {
                    (Some(x), Some(y)) => {
                        gap > min_gap(MAX_LEVEL)
                            && (SlicePoint::from_path(&x.end())
                                .chord_km(&SlicePoint::from_path(&y.end()))
                                > REFINE_KM
                                || SlicePoint::from_path(&x.bottom)
                                    .chord_km(&SlicePoint::from_path(&y.bottom))
                                    > REFINE_KM)
                    }
                    (None, None) => false,
                    _ => gap > min_gap(EDGE_LEVEL),
                };
                if split {
                    let mid = 0.5 * (a0 + a1);
                    next.push((mid, tracer.trace(phase, depth_km, mid)));
                    inserted = true;
                }
            }
            next.push(samples[i].clone());
        }
        // `next` interleaves midpoints before their right neighbour, so it stays sorted.
        samples = next;
        if !inserted {
            break;
        }
    }
    samples.into_iter().filter_map(|(_, r)| r).collect()
}

/// Depth (km) below the surface of a slice point.
pub fn depth_of(p: &SlicePoint) -> f64 {
    EARTH_RADIUS_KM - p.r_km
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> EarthModel {
        EarthModel::from_bytes(include_bytes!("../../../assets/ak135_model.csv")).unwrap()
    }

    fn fans(depth: f64) -> SliceFans {
        SliceFans::build(&RayTracer::new(&model()).unwrap(), depth)
    }

    #[test]
    fn named_boundaries_come_from_the_model() {
        let m = model();
        let b = named_boundaries(&m);
        let names: Vec<&str> = b.iter().map(|x| x.name).collect();
        assert_eq!(names, ["Moho", "410", "660", "CMB", "ICB"]);
        let disc = m.discontinuities();
        for x in &b {
            assert!(
                disc.contains(&x.depth_km),
                "{} at {} km",
                x.name,
                x.depth_km
            );
        }
    }

    #[test]
    fn every_fan_has_rays_and_they_are_ordered() {
        let f = fans(29.0);
        for fan in &f.fans {
            assert!(
                fan.rays.len() > 10,
                "{:?}: {} rays",
                fan.phase,
                fan.rays.len()
            );
            assert!(
                fan.rays.len() < 5000,
                "{:?}: {} rays",
                fan.phase,
                fan.rays.len()
            );
            for w in fan.rays.windows(2) {
                assert!(w[0].takeoff_rad < w[1].takeoff_rad);
            }
        }
    }

    #[test]
    fn no_hole_under_the_source() {
        // 200 s after origin the downgoing P energy is ~1500 km down, straight below the
        // source: the core-bound fan (PcP) must carry it.
        let f = fans(29.0).fronts_at(200.0);
        let below = f
            .lines
            .iter()
            .flat_map(|l| &l.points)
            .any(|p| p.delta_deg < 0.5 && depth_of(p) > 1000.0 && depth_of(p) < 2500.0);
        assert!(below, "no front point below the source");
    }

    #[test]
    fn fronts_lie_at_time_t_and_stay_connected() {
        let f = fans(29.0);
        let t = 400.0;
        let fronts = f.fronts_at(t);
        assert!(!fronts.lines.is_empty());
        for line in &fronts.lines {
            for w in line.points.windows(2) {
                assert!(w[0].chord_km(&w[1]) <= BREAK_KM, "{:?}", line.phase);
            }
            for p in &line.points {
                assert!(p.r_km <= EARTH_RADIUS_KM + 1e-6 && p.r_km >= 0.0);
                assert!((0.0..=180.0).contains(&p.delta_deg));
            }
        }
        // Core-entering phases only appear once their rays are in the core.
        for line in fronts.lines.iter().filter(|l| l.phase == RayPhase::PKP) {
            for p in &line.points {
                assert!(p.r_km <= f.r_cmb + 1.0, "PKP front above the CMB at {p:?}");
            }
        }
    }

    #[test]
    fn fan_reaches_the_surface_where_the_table_says() {
        // The farthest surface point the fan's P and S rays have reached by time t matches the
        // table's first-arrival front: the fan has no gaps and no stray rays.
        for depth in [0.0, 29.0] {
            let f = fans(depth);
            let table = crate::tables::TravelTimeTable::from_bytes(include_bytes!(
                "../../../assets/ak135.tt"
            ))
            .unwrap();
            let sim = crate::Simulation::new(table, model(), vec![]).unwrap();
            for (phase, name) in [(RayPhase::P, "P"), (RayPhase::S, "S")] {
                let fan = f.fans.iter().find(|x| x.phase == phase).unwrap();
                for t in [120.0, 300.0, 600.0] {
                    let want = sim.front_distance(name, depth, t).unwrap();
                    let got = fan
                        .rays
                        .iter()
                        .filter(|r| r.arrival_s() <= t)
                        .map(|r| SlicePoint::from_path(&r.end()).delta_deg)
                        .fold(0.0, f64::max);
                    assert!(
                        (got - want).abs() < 0.5,
                        "{name} {depth} km t={t}: fan {got:.2}°, table {want:.2}°"
                    );
                }
            }
        }
    }

    #[test]
    fn labels_only_on_the_naming_leg() {
        let f = fans(29.0);
        // At 200 s no PcP ray has reached the core yet (~260 s straight down), so no label.
        let early = f.fronts_at(200.0);
        assert!(!early.labels.iter().any(|l| l.phase == RayPhase::PcP));
        assert!(early.labels.iter().any(|l| l.phase == RayPhase::P));
        let late = f.fronts_at(700.0);
        assert!(late.labels.iter().any(|l| l.phase == RayPhase::PcP));
    }

    #[test]
    fn ray_paths_grow_with_time() {
        let f = fans(29.0);
        let a = f.ray_paths_until(100.0, 6);
        let b = f.ray_paths_until(600.0, 6);
        let len = |v: &Vec<SliceLine>| v.iter().map(|l| l.points.len()).sum::<usize>();
        assert!(!a.is_empty());
        assert!(len(&b) > len(&a));
    }

    #[test]
    fn halves_show_each_one_dimensional_front_point_once() {
        // In 1D the forward and back fans are the same rays; between them every point of every
        // ray at time t is drawn exactly once, on one side or the other.
        let f = fans(29.0);
        for t in [300.0, 900.0, 1200.0] {
            let fronts = f.fronts_at(t);
            let drawn: usize = fronts.lines.iter().map(|l| l.points.len()).sum();
            let expected: usize = f
                .fans
                .iter()
                .filter(|fan| fan.side == Side::Forward)
                .flat_map(|fan| &fan.rays)
                .filter(|r| t >= r.shared_until_s && r.point_at(t).is_some())
                .count();
            // Fronts drop isolated points and repeat a point where the wave type changes.
            assert!(
                drawn.abs_diff(expected) <= 60,
                "t={t}: drew {drawn} of {expected}"
            );
            let mut phases: Vec<RayPhase> = fronts.labels.iter().map(|l| l.phase).collect();
            phases.sort_by_key(|p| p.name());
            phases.dedup();
            assert_eq!(phases.len(), fronts.labels.len(), "a phase labelled twice");
        }
    }

    #[test]
    fn sides_split_at_the_antipode() {
        assert!(Side::Forward.shows(0.0) && Side::Forward.shows(PI));
        assert!(!Side::Back.shows(PI) && Side::Back.shows(PI + 1e-9));
        assert!(Side::Back.shows(1.5 * PI) && Side::Forward.shows(2.0 * PI + 0.1));
    }

    #[test]
    fn deterministic() {
        assert_eq!(fans(29.0).fronts_at(321.0), fans(29.0).fronts_at(321.0));
    }
}
