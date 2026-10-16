//! Ray tracing through the 1D velocity model: ray paths and travel times for the phases the
//! slice view draws.
//!
//! The model is cut into thin spherical shells. Inside a shell the velocity follows a power
//! law in radius, v = a·r^b, for which the ray integrals have closed forms. With η = r/v and
//! ray parameter p (s/rad):
//!
//! ```text
//! Δθ = [arccos(p/η)] / (1 − b)        Δt = [√(η² − p²)] / (1 − b)
//! ```
//!
//! evaluated between the shell's end radii, or between the shell top and η = p where the ray
//! turns. Rays are symmetric about their turning (or reflection) point, so each one-way leg is
//! integrated downward and mirrored for the way up.

use std::f64::consts::{FRAC_PI_2, PI};
use std::fmt;

use crate::geo::EARTH_RADIUS_KM;
use crate::tables::EarthModel;

pub mod bend;

/// Thickest integration shell (km). Velocities are linear in depth between model samples;
/// each shell replaces that with a power law between its end values.
const MAX_SHELL_KM: f64 = 10.0;
/// Longest straight chord (km) between stored path points.
const MAX_CHORD_KM: f64 = 50.0;
/// Smallest ray parameter traced (s/rad). p = 0 is singular at the centre; this ray passes
/// within 1e-5 km of it.
const MIN_RAY_PARAM: f64 = 1e-6;

/// Wave type of a ray leg: compressional or shear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wave {
    P,
    S,
}

/// The phases the ray tracer follows. `PKP` covers both outer-core branches (the table's
/// `PKPab` and `PKPbc`); `PKIKP` is the table's `PKPdf`. Direct `P` and `S` include the rays
/// that leave a buried source upward (`p`, `s`). The depth phases `pP` and `sP` leave upward
/// only, reflect off the surface above the source (`sP` converting from S to P there) and then
/// travel as P. `PcS` and `ScP` convert where they reflect off the core, `PS` and `SP` where
/// they reflect off the surface halfway; `PKiKP` reflects off the inner core; `PKKP` and `SKKS`
/// reflect once off the underside of the core-mantle boundary, inside the outer core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(non_camel_case_types)] // pP and sP are the seismological names
pub enum RayPhase {
    P,
    S,
    PcP,
    ScS,
    PKP,
    PKIKP,
    SKS,
    PP,
    SS,
    pP,
    sP,
    PcS,
    ScP,
    PKiKP,
    PKKP,
    SKKS,
    PS,
    SP,
}

/// Where a phase's ray bottoms out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bottom {
    TurnMantle,
    ReflectCmb,
    TurnOuterCore,
    ReflectIcb,
    TurnInnerCore,
}

/// Times (s) along a ray at the points that decide which stretch of it another phase already
/// draws, where its own leg begins and where it converts.
#[derive(Debug, Clone, Copy, Default)]
struct Marks {
    /// First time it is back at the surface (end of the first hop, or of a depth phase's way
    /// up).
    first_surface: f64,
    /// First time it enters the core.
    core_entry: f64,
    /// Its reflection off the core-mantle boundary (PcP family) or the inner core (PKiKP).
    reflected: f64,
    /// Its first reflection off the underside of the core-mantle boundary (PKKP, SKKS).
    underside: f64,
}

impl RayPhase {
    pub const ALL: [RayPhase; 18] = [
        RayPhase::P,
        RayPhase::S,
        RayPhase::PcP,
        RayPhase::ScS,
        RayPhase::PKP,
        RayPhase::PKIKP,
        RayPhase::SKS,
        RayPhase::PP,
        RayPhase::SS,
        RayPhase::pP,
        RayPhase::sP,
        RayPhase::PcS,
        RayPhase::ScP,
        RayPhase::PKiKP,
        RayPhase::PKKP,
        RayPhase::SKKS,
        RayPhase::PS,
        RayPhase::SP,
    ];

    /// The phases drawn in the slice unless the user turns more on: all but the converted and
    /// doubly reflected ones, which are weaker and would crowd the picture.
    pub fn shown_by_default(self) -> bool {
        !matches!(
            self,
            RayPhase::PcS
                | RayPhase::ScP
                | RayPhase::PKiKP
                | RayPhase::PKKP
                | RayPhase::SKKS
                | RayPhase::PS
                | RayPhase::SP
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            RayPhase::P => "P",
            RayPhase::S => "S",
            RayPhase::PcP => "PcP",
            RayPhase::ScS => "ScS",
            RayPhase::PKP => "PKP",
            RayPhase::PKIKP => "PKIKP",
            RayPhase::SKS => "SKS",
            RayPhase::PP => "PP",
            RayPhase::SS => "SS",
            RayPhase::pP => "pP",
            RayPhase::sP => "sP",
            RayPhase::PcS => "PcS",
            RayPhase::ScP => "ScP",
            RayPhase::PKiKP => "PKiKP",
            RayPhase::PKKP => "PKKP",
            RayPhase::SKKS => "SKKS",
            RayPhase::PS => "PS",
            RayPhase::SP => "SP",
        }
    }

    /// Wave type the ray leaves the source as (in the core every phase here travels as P).
    pub fn source_wave(self) -> Wave {
        match self {
            RayPhase::P
            | RayPhase::PcP
            | RayPhase::PKP
            | RayPhase::PKIKP
            | RayPhase::PP
            | RayPhase::pP
            | RayPhase::PcS
            | RayPhase::PKiKP
            | RayPhase::PKKP
            | RayPhase::PS => Wave::P,
            RayPhase::S
            | RayPhase::ScS
            | RayPhase::SKS
            | RayPhase::SS
            | RayPhase::sP
            | RayPhase::ScP
            | RayPhase::SKKS
            | RayPhase::SP => Wave::S,
        }
    }

    /// Wave type the ray reaches the surface as: the source wave, except for the converted
    /// phases (sP, PcS, ScP, PS, SP).
    pub fn arrival_wave(self) -> Wave {
        match self {
            RayPhase::sP | RayPhase::ScP | RayPhase::SP => Wave::P,
            RayPhase::PcS | RayPhase::PS => Wave::S,
            _ => self.source_wave(),
        }
    }

    /// Whether the ray converts where it reflects off the core-mantle boundary (PcS, ScP).
    /// The other converted phases convert at the surface.
    fn converts_at_bottom(self) -> bool {
        matches!(self, RayPhase::PcS | RayPhase::ScP)
    }

    fn bottom(self) -> Bottom {
        match self {
            RayPhase::P
            | RayPhase::S
            | RayPhase::PP
            | RayPhase::SS
            | RayPhase::pP
            | RayPhase::sP
            | RayPhase::PS
            | RayPhase::SP => Bottom::TurnMantle,
            RayPhase::PcP | RayPhase::ScS | RayPhase::PcS | RayPhase::ScP => Bottom::ReflectCmb,
            RayPhase::PKP | RayPhase::SKS | RayPhase::PKKP | RayPhase::SKKS => {
                Bottom::TurnOuterCore
            }
            RayPhase::PKiKP => Bottom::ReflectIcb,
            RayPhase::PKIKP => Bottom::TurnInnerCore,
        }
    }

    /// Surface-to-surface hops: 2 for the surface reflections PP, SS, PS and SP.
    fn hops(self) -> usize {
        match self {
            RayPhase::PP | RayPhase::SS | RayPhase::PS | RayPhase::SP => 2,
            _ => 1,
        }
    }

    /// Arcs through the outer core per hop: 2 for PKKP and SKKS, which reflect off the
    /// underside of the core-mantle boundary between them.
    fn core_arcs(self) -> usize {
        match self {
            RayPhase::PKKP | RayPhase::SKKS => 2,
            _ => 1,
        }
    }

    /// Wave types of hop `hop` (0 = the first hop after the source, or after a depth phase's
    /// way up): down to its bottom and back up.
    fn hop_waves(self, hop: usize, after_up_leg: bool) -> (Wave, Wave) {
        let down = if hop == 0 && !after_up_leg {
            self.source_wave()
        } else {
            self.arrival_wave()
        };
        let up = if self.converts_at_bottom() {
            self.arrival_wave()
        } else {
            down
        };
        (down, up)
    }

    /// Until when the ray follows the path of a ray of another phase, and from when it is on
    /// the leg that gives it its name: see [`Ray::shared_until_s`].
    fn shared_and_distinct(self, m: &Marks) -> (f64, f64) {
        match self {
            RayPhase::P | RayPhase::S => (0.0, 0.0),
            RayPhase::PcP | RayPhase::ScS => (0.0, m.reflected),
            // Down the way PcP or ScS goes, until the conversion.
            RayPhase::PcS | RayPhase::ScP => (m.reflected, m.reflected),
            RayPhase::PKP | RayPhase::PKIKP | RayPhase::SKS => (m.core_entry, m.core_entry),
            // Down the way PKIKP goes, until PKIKP enters the inner core.
            RayPhase::PKiKP => (m.reflected, m.reflected),
            // The way PKP or SKS goes, until the underside of the core-mantle boundary.
            RayPhase::PKKP | RayPhase::SKKS => (m.underside, m.underside),
            RayPhase::PP
            | RayPhase::SS
            | RayPhase::PS
            | RayPhase::SP
            | RayPhase::pP
            | RayPhase::sP => (m.first_surface, m.first_surface),
        }
    }

    /// When the ray converts between P and S (0 for phases that do not).
    fn conversion(self, m: &Marks) -> f64 {
        match self {
            RayPhase::PcS | RayPhase::ScP => m.reflected,
            RayPhase::PS | RayPhase::SP | RayPhase::sP => m.first_surface,
            _ => 0.0,
        }
    }

    /// Whether rays leaving the source upward belong to this phase.
    pub fn has_upgoing(self) -> bool {
        matches!(
            self,
            RayPhase::P | RayPhase::S | RayPhase::pP | RayPhase::sP
        )
    }

    /// Whether only rays leaving the source upward belong to this phase: the depth phases.
    pub fn is_depth_phase(self) -> bool {
        matches!(self, RayPhase::pP | RayPhase::sP)
    }

    /// Take-off angles (rad from straight down) of this phase's rays from a source `depth_km`
    /// deep: downward (0 to π/2), also upward for direct P and S from a buried source (to π),
    /// upward only for the depth phases. `None` for a depth phase from a surface source.
    pub fn takeoff_range(self, depth_km: f64) -> Option<(f64, f64)> {
        let buried = depth_km > 0.0;
        match (self.is_depth_phase(), self.has_upgoing() && buried) {
            (true, true) => Some((FRAC_PI_2, PI)),
            (true, false) => None,
            (false, true) => Some((0.0, PI)),
            (false, false) => Some((0.0, FRAC_PI_2)),
        }
    }
}

/// One point along a ray: radius (km), angle travelled from the source (rad, may exceed π
/// for long phases), and time since origin (s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathPoint {
    pub r_km: f64,
    pub theta_rad: f64,
    pub t_s: f64,
}

/// A traced ray from the source to the surface.
#[derive(Debug, Clone, PartialEq)]
pub struct Ray {
    pub phase: RayPhase,
    /// Take-off angle at the source, rad from straight down (0) to straight up (π).
    pub takeoff_rad: f64,
    /// Ray parameter, s/rad.
    pub p_s_rad: f64,
    /// Path from source to surface, time increasing, at most `MAX_CHORD_KM` apart.
    pub points: Vec<PathPoint>,
    /// The deepest point (the turning or reflection point); the source for upgoing rays.
    pub bottom: PathPoint,
    /// Until this time the ray follows the same path as a ray of another phase (PKP shares its
    /// mantle leg with PcP, PP its first hop with P, pP and sP their way up with p and s).
    pub shared_until_s: f64,
    /// From this time the ray is on the leg that gives the phase its name (after the core
    /// reflection for PcP, after the core entry for PKP, ...).
    pub distinct_after_s: f64,
    /// Where the ray converts between P and S (sP and PS at the surface, PcS at the core): it
    /// is the source wave before this time and the arrival wave after. 0 for phases that keep
    /// their wave type.
    pub converted_at_s: f64,
}

impl Ray {
    pub fn arrival_s(&self) -> f64 {
        self.points.last().map_or(0.0, |q| q.t_s)
    }

    pub fn end(&self) -> PathPoint {
        *self.points.last().expect("a ray has at least two points")
    }

    /// Wave type where the ray is at radius `r_km` at time `t_s`, given the core-mantle
    /// boundary radius `r_cmb_km`: P in the core; above it the source wave until the ray
    /// converts, the arrival wave after.
    pub fn wave_at(&self, t_s: f64, r_km: f64, r_cmb_km: f64) -> Wave {
        if r_km <= r_cmb_km {
            Wave::P
        } else if t_s < self.converted_at_s {
            self.phase.source_wave()
        } else {
            self.phase.arrival_wave()
        }
    }

    /// Position at time `t`, interpolated linearly between stored points; `None` outside the
    /// ray's travel time.
    pub fn point_at(&self, t: f64) -> Option<PathPoint> {
        let pts = &self.points;
        if t < 0.0 || t > pts.last()?.t_s {
            return None;
        }
        let i = pts.partition_point(|q| q.t_s < t).max(1);
        let (a, b) = (pts[i - 1], pts[i]);
        let f = if b.t_s > a.t_s {
            (t - a.t_s) / (b.t_s - a.t_s)
        } else {
            0.0
        };
        Some(PathPoint {
            r_km: a.r_km + (b.r_km - a.r_km) * f,
            theta_rad: a.theta_rad + (b.theta_rad - a.theta_rad) * f,
            t_s: t,
        })
    }
}

/// One arrival of a phase at a surface distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrival {
    pub time_s: f64,
    /// Ray parameter, s/deg (the table's unit).
    pub ray_param_s_deg: f64,
    pub takeoff_rad: f64,
}

/// Where one ray reaches the surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Landing {
    pub takeoff_rad: f64,
    /// Epicentral distance, deg, folded into [0, 180].
    pub dist_deg: f64,
    pub time_s: f64,
    /// Ray parameter, s/rad.
    pub p_s_rad: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RayTraceError {
    NoFluidCore,
    NotToCentre,
}

impl fmt::Display for RayTraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RayTraceError::NoFluidCore => write!(f, "velocity model has no fluid outer core"),
            RayTraceError::NotToCentre => write!(
                f,
                "velocity model must run from the surface to {EARTH_RADIUS_KM} km depth"
            ),
        }
    }
}

impl std::error::Error for RayTraceError {}

/// A shell r_bot < r ≤ r_top with v = a·r^b.
#[derive(Debug, Clone, Copy)]
struct Shell {
    r_top: f64,
    r_bot: f64,
    a: f64,
    b: f64,
}

impl Shell {
    fn new(r_top: f64, r_bot: f64, v_top: f64, v_bot: f64) -> Self {
        let constant = |v: f64| Shell {
            r_top,
            r_bot,
            a: v,
            b: 0.0,
        };
        if r_bot <= 0.0 {
            // The power law is singular at the centre; the innermost shell is uniform.
            return constant(v_top);
        }
        let b = (v_top / v_bot).ln() / (r_top / r_bot).ln();
        if !b.is_finite() || (1.0 - b).abs() < 1e-6 {
            return constant(0.5 * (v_top + v_bot));
        }
        Shell {
            r_top,
            r_bot,
            a: v_top / r_top.powf(b),
            b,
        }
    }

    fn velocity(&self, r: f64) -> f64 {
        self.a * r.powf(self.b)
    }

    fn eta(&self, r: f64) -> f64 {
        r / self.velocity(r)
    }

    fn radius_at_eta(&self, eta: f64) -> f64 {
        (self.a * eta).powf(1.0 / (1.0 - self.b))
    }

    /// θ and t potentials at slowness-radius `eta`: differences between two points of the
    /// shell give Δθ and Δt.
    fn potentials(&self, p: f64, eta: f64) -> (f64, f64) {
        let k = 1.0 - self.b;
        let x = if eta > 0.0 { (p / eta).min(1.0) } else { 1.0 };
        (x.acos() / k, (eta * eta - p * p).max(0.0).sqrt() / k)
    }
}

/// Shells for one wave type, surface to centre.
#[derive(Debug, Clone)]
struct Profile {
    shells: Vec<Shell>,
    /// The model's own description: velocity linear in depth between samples, surface first
    /// (the shells approximate this). Used by the bent-ray tracer.
    intervals: Vec<Interval>,
}

/// Velocity linear in depth between two model samples, r_bot < r ≤ r_top.
#[derive(Debug, Clone, Copy)]
struct Interval {
    r_top: f64,
    r_bot: f64,
    v_top: f64,
    v_bot: f64,
}

impl Interval {
    /// Velocity (km/s) and its radial derivative (per s) at `r`, extrapolating past the ends.
    fn velocity(&self, r: f64) -> (f64, f64) {
        let slope = (self.v_top - self.v_bot) / (self.r_top - self.r_bot);
        (self.v_bot + slope * (r - self.r_bot), slope)
    }
}

impl Profile {
    /// `velocity(point)` picks the velocity this profile uses from a model sample.
    fn new(model: &EarthModel, velocity: impl Fn(&tables::ModelPoint) -> f64) -> Self {
        let mut shells = Vec::new();
        for w in model.points().windows(2) {
            let (top, bot) = (&w[0], &w[1]);
            let thick = bot.depth_km - top.depth_km;
            if thick <= 0.0 {
                continue; // a discontinuity: the jump happens between shells
            }
            let (v0, v1) = (velocity(top), velocity(bot));
            let n = (thick / MAX_SHELL_KM).ceil().max(1.0) as usize;
            for k in 0..n {
                let f0 = k as f64 / n as f64;
                let f1 = (k + 1) as f64 / n as f64;
                let d0 = top.depth_km + thick * f0;
                let d1 = top.depth_km + thick * f1;
                shells.push(Shell::new(
                    EARTH_RADIUS_KM - d0,
                    (EARTH_RADIUS_KM - d1).max(0.0),
                    v0 + (v1 - v0) * f0,
                    v0 + (v1 - v0) * f1,
                ));
            }
        }
        let intervals = model
            .points()
            .windows(2)
            .filter(|w| w[1].depth_km > w[0].depth_km)
            .map(|w| Interval {
                r_top: EARTH_RADIUS_KM - w[0].depth_km,
                r_bot: EARTH_RADIUS_KM - w[1].depth_km,
                v_top: velocity(&w[0]),
                v_bot: velocity(&w[1]),
            })
            .collect();
        Self { shells, intervals }
    }

    /// Velocity just below radius `r` (the lower side at a discontinuity).
    fn velocity_at(&self, r: f64) -> f64 {
        let s = self.shell_below(r);
        s.velocity(r.min(s.r_top))
    }

    /// The shell just below radius `r` (the lower one at a shell boundary); the innermost
    /// shell for `r` at or below the centre.
    fn shell_below(&self, r: f64) -> &Shell {
        // Shells run from the surface down, so `r_bot` decreases.
        let i = self.shells.partition_point(|s| s.r_bot >= r);
        &self.shells[i.min(self.shells.len() - 1)]
    }

    /// The linear interval just below radius `r`.
    fn interval_below(&self, r: f64) -> &Interval {
        let i = self.intervals.partition_point(|s| s.r_bot >= r);
        &self.intervals[i.min(self.intervals.len() - 1)]
    }

    /// Radii (km) of the model samples, where the velocity gradient changes.
    fn sample_radii(&self) -> impl Iterator<Item = f64> + '_ {
        self.intervals.iter().flat_map(|i| [i.r_top, i.r_bot])
    }

    /// Radii (km) where the velocity jumps between neighbouring shells.
    fn discontinuities(&self) -> Vec<f64> {
        self.shells
            .windows(2)
            .filter(|w| (w[0].velocity(w[0].r_bot) - w[1].velocity(w[1].r_top)).abs() > 1e-6)
            .map(|w| w[0].r_bot)
            .collect()
    }
}

/// How a downward leg ended.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LegEnd {
    /// Turned at this radius (including total reflection off the top of a velocity jump).
    Turned(f64),
    /// Reached the stop radius without turning.
    Reached(f64),
}

impl LegEnd {
    fn radius(self) -> f64 {
        match self {
            LegEnd::Turned(r) | LegEnd::Reached(r) => r,
        }
    }
}

/// A one-way downward leg: angle and time from its start to its end, and optionally the path
/// (θ and t measured from the start).
struct Leg {
    theta: f64,
    t: f64,
    end: LegEnd,
    points: Vec<PathPoint>,
}

fn chord_km(a: &PathPoint, b: &PathPoint) -> f64 {
    let (ax, ay) = (a.r_km * a.theta_rad.cos(), a.r_km * a.theta_rad.sin());
    let (bx, by) = (b.r_km * b.theta_rad.cos(), b.r_km * b.theta_rad.sin());
    (ax - bx).hypot(ay - by)
}

/// Keep the first and last points and as few others as keep neighbours within `MAX_CHORD_KM`.
fn decimate(points: Vec<PathPoint>) -> Vec<PathPoint> {
    if points.len() <= 2 {
        return points;
    }
    let mut out = vec![points[0]];
    for i in 1..points.len() - 1 {
        if chord_km(out.last().unwrap(), &points[i + 1]) > MAX_CHORD_KM {
            out.push(points[i]);
        }
    }
    out.push(*points.last().unwrap());
    out
}

/// Fold an angle travelled along the great circle to an epicentral distance in [0, π].
pub fn fold_rad(theta: f64) -> f64 {
    let x = theta.rem_euclid(2.0 * PI);
    if x > PI { 2.0 * PI - x } else { x }
}

/// A ray's path assembled from one-way legs, each followed down (the way it was integrated)
/// or up (mirrored). Each leg is thinned on its own, so the corners between legs (turning and
/// reflection points, surface bounces) stay path points. Without `record` only the angle and
/// time travelled are kept.
struct PathBuilder {
    record: bool,
    theta: f64,
    t: f64,
    points: Vec<PathPoint>,
}

impl PathBuilder {
    fn new(source: PathPoint, record: bool) -> Self {
        Self {
            record,
            theta: 0.0,
            t: 0.0,
            points: vec![source],
        }
    }

    /// The point where the path has got to, at radius `r_km`.
    fn here(&self, r_km: f64) -> PathPoint {
        PathPoint {
            r_km,
            theta_rad: self.theta,
            t_s: self.t,
        }
    }

    fn extend(&mut self, piece: Vec<PathPoint>) {
        self.points.extend(decimate(piece).into_iter().skip(1));
    }

    /// Follow `leg` downward, from its start radius to its end.
    fn down(&mut self, leg: &Leg) {
        if self.record {
            let (theta, t) = (self.theta, self.t);
            self.extend(
                leg.points
                    .iter()
                    .map(|q| PathPoint {
                        r_km: q.r_km,
                        theta_rad: theta + q.theta_rad,
                        t_s: t + q.t_s,
                    })
                    .collect(),
            );
        }
        self.theta += leg.theta;
        self.t += leg.t;
    }

    /// Follow `leg` upward, from its end radius back to its start.
    fn up(&mut self, leg: &Leg) {
        let (theta, t) = (self.theta + leg.theta, self.t + leg.t);
        if self.record {
            self.extend(
                leg.points
                    .iter()
                    .rev()
                    .map(|q| PathPoint {
                        r_km: q.r_km,
                        theta_rad: theta - q.theta_rad,
                        t_s: t - q.t_s,
                    })
                    .collect(),
            );
        }
        self.theta = theta;
        self.t = t;
    }

    /// The path, ending at the surface; just its two ends without `record`.
    fn finish(self) -> Vec<PathPoint> {
        if self.record {
            return self.points;
        }
        let end = self.here(EARTH_RADIUS_KM);
        vec![self.points[0], end]
    }
}

/// Ray tracer over one velocity model.
#[derive(Debug, Clone)]
pub struct RayTracer {
    /// P velocity everywhere.
    p_profile: Profile,
    /// S velocity above the core-mantle boundary, P velocity below (SKS travels as P in the
    /// liquid outer core).
    s_profile: Profile,
    r_cmb: f64,
    r_icb: f64,
    /// Radius (km) of the Moho, the base of the crust, if the model has one.
    r_moho: Option<f64>,
}

impl RayTracer {
    pub fn new(model: &EarthModel) -> Result<Self, RayTraceError> {
        let (cmb_depth, icb_depth) = model.fluid_core().ok_or(RayTraceError::NoFluidCore)?;
        let pts = model.points();
        if pts.first().map(|q| q.depth_km) != Some(0.0)
            || pts.last().map(|q| q.depth_km) != Some(EARTH_RADIUS_KM)
        {
            return Err(RayTraceError::NotToCentre);
        }
        Ok(Self {
            p_profile: Profile::new(model, |q| q.vp_km_s),
            s_profile: Profile::new(model, |q| {
                if q.depth_km < cmb_depth || (q.depth_km == cmb_depth && q.vs_km_s > 0.0) {
                    q.vs_km_s
                } else {
                    q.vp_km_s
                }
            }),
            r_cmb: EARTH_RADIUS_KM - cmb_depth,
            r_icb: EARTH_RADIUS_KM - icb_depth,
            r_moho: crate::slice::named_boundaries(model)
                .iter()
                .find(|b| b.name == "Moho")
                .map(|b| EARTH_RADIUS_KM - b.depth_km),
        })
    }

    /// Smallest ray parameter (s/rad) of a ray of `phase` that stays in the crust on one of its
    /// hops: above the slowness radius η just below the Moho of a wave it makes a hop as, it
    /// turns or reflects inside the crust without reaching the mantle. `None` without a Moho.
    pub fn crust_ray_param(&self, phase: RayPhase) -> Option<f64> {
        let r = self.r_moho?;
        let after_up_leg = phase.is_depth_phase();
        (0..phase.hops())
            .flat_map(|hop| {
                let (down, up) = phase.hop_waves(hop, after_up_leg);
                [down, up]
            })
            .map(|w| r / self.profile(w).velocity_at(r))
            .reduce(f64::min)
    }

    /// Radius (km) of the core-mantle boundary.
    pub fn cmb_radius_km(&self) -> f64 {
        self.r_cmb
    }

    /// Radius (km) of the inner-core boundary.
    pub fn icb_radius_km(&self) -> f64 {
        self.r_icb
    }

    fn profile(&self, wave: Wave) -> &Profile {
        match wave {
            Wave::P => &self.p_profile,
            Wave::S => &self.s_profile,
        }
    }

    /// Integrate downward from `r_start` until the ray turns or reaches `r_stop`.
    fn descend(&self, wave: Wave, r_start: f64, r_stop: f64, p: f64, record: bool) -> Leg {
        let (mut theta, mut t) = (0.0, 0.0);
        let mut points = Vec::new();
        if record {
            points.push(PathPoint {
                r_km: r_start,
                theta_rad: 0.0,
                t_s: 0.0,
            });
        }
        for shell in &self.profile(wave).shells {
            if shell.r_bot >= r_start {
                continue;
            }
            if shell.r_top <= r_stop {
                break;
            }
            let top = shell.r_top.min(r_start);
            let bot = shell.r_bot.max(r_stop);
            let eta_top = shell.eta(top);
            if eta_top < p {
                // Cannot enter: totally reflected off the top of a velocity increase.
                return Leg {
                    theta,
                    t,
                    end: LegEnd::Turned(top),
                    points,
                };
            }
            let eta_bot = shell.eta(bot);
            let (end_r, end_eta, turned) = if eta_bot >= p {
                (bot, eta_bot, false)
            } else {
                (shell.radius_at_eta(p), p, true)
            };
            let (th0, t0) = shell.potentials(p, eta_top);
            let (th1, t1) = shell.potentials(p, end_eta);
            if record {
                // Sub-steps, uniform in arccos(p/η) (so in θ), where the shell's chord is long:
                // near the turning point the ray runs almost horizontally.
                let dth = th0 - th1;
                let chord = (top - end_r).hypot(top * dth);
                let k = (chord / MAX_CHORD_KM).ceil().clamp(1.0, 256.0) as usize;
                let (phi0, phi1) = ((p / eta_top).min(1.0).acos(), (p / end_eta).min(1.0).acos());
                for j in 1..k {
                    let phi = phi0 + (phi1 - phi0) * j as f64 / k as f64;
                    let eta = p / phi.cos();
                    let (th, tt) = shell.potentials(p, eta);
                    points.push(PathPoint {
                        r_km: shell.radius_at_eta(eta),
                        theta_rad: theta + th0 - th,
                        t_s: t + t0 - tt,
                    });
                }
            }
            theta += th0 - th1;
            t += t0 - t1;
            if record {
                points.push(PathPoint {
                    r_km: end_r,
                    theta_rad: theta,
                    t_s: t,
                });
            }
            if turned {
                return Leg {
                    theta,
                    t,
                    end: LegEnd::Turned(end_r),
                    points,
                };
            }
        }
        Leg {
            theta,
            t,
            end: LegEnd::Reached(r_stop),
            points,
        }
    }

    /// One hop of a ray with ray parameter `p`, appended to `path`: from radius `r0` (the
    /// source, or the surface) down as `down` to `bottom` and back up to the surface as `up`,
    /// crossing the outer core `arcs` times (reflecting off the underside of the core-mantle
    /// boundary between crossings) for a bottom in the core. Returns the hop's deepest point,
    /// or `None` (and leaves `path` alone) if the ray does not bottom out that way. With
    /// `must_leave`, a ray turning where it starts is no hop: after a conversion at the surface
    /// a ray parameter past the new wave's surface slowness has no leg.
    #[allow(clippy::too_many_arguments)]
    fn hop(
        &self,
        path: &mut PathBuilder,
        marks: &mut Marks,
        (down, up): (Wave, Wave),
        bottom: Bottom,
        arcs: usize,
        r0: f64,
        p: f64,
        must_leave: bool,
    ) -> Option<PathPoint> {
        let record = path.record;
        let src = self.descend(down, r0, self.r_cmb, p, record);
        let surf = self.descend(up, EARTH_RADIUS_KM, self.r_cmb, p, record);
        if bottom == Bottom::TurnMantle {
            let (LegEnd::Turned(r), LegEnd::Turned(r_up)) = (src.end, surf.end) else {
                return None;
            };
            if r <= self.r_cmb || r != r_up || (must_leave && r >= r0) {
                return None;
            }
            path.down(&src);
            let deepest = path.here(r);
            path.up(&surf);
            return Some(deepest);
        }
        // Every other bottom is reached through the core-mantle boundary.
        if !matches!(
            (src.end, surf.end),
            (LegEnd::Reached(_), LegEnd::Reached(_))
        ) {
            return None;
        }
        if bottom == Bottom::ReflectCmb {
            path.down(&src);
            let deepest = path.here(self.r_cmb);
            marks.reflected = path.t;
            path.up(&surf);
            return Some(deepest);
        }
        // In the core every wave is P.
        let stop = if bottom == Bottom::TurnInnerCore {
            0.0
        } else {
            self.r_icb
        };
        let arc = self.descend(Wave::P, self.r_cmb, stop, p, record);
        let ok = match (bottom, arc.end) {
            (Bottom::TurnOuterCore, LegEnd::Turned(r)) => r < self.r_cmb,
            (Bottom::ReflectIcb, LegEnd::Reached(_)) => true,
            // Turning exactly at the ICB is total reflection off it (PKiKP), not PKIKP.
            (Bottom::TurnInnerCore, LegEnd::Turned(r)) => r < self.r_icb,
            _ => false,
        };
        if !ok {
            return None;
        }
        path.down(&src);
        if marks.core_entry == 0.0 {
            marks.core_entry = path.t;
        }
        let mut deepest = None;
        for k in 0..arcs {
            if k == 1 {
                marks.underside = path.t;
            }
            path.down(&arc);
            if k == 0 {
                deepest = Some(path.here(arc.end.radius()));
                if bottom == Bottom::ReflectIcb {
                    marks.reflected = path.t;
                }
            }
            path.up(&arc);
        }
        path.up(&surf);
        deepest
    }

    /// Ray parameter (s/rad) of a ray of `phase` leaving a source at `depth_km` at `takeoff_rad`
    /// from straight down.
    pub fn ray_param(&self, phase: RayPhase, depth_km: f64, takeoff_rad: f64) -> f64 {
        let r_s = EARTH_RADIUS_KM - depth_km;
        let eta_s = r_s / self.profile(phase.source_wave()).velocity_at(r_s);
        (eta_s * takeoff_rad.sin()).max(MIN_RAY_PARAM)
    }

    /// Slowness radius η = r/v (s/rad) of `wave` at the surface: a ray of parameter `p` meets
    /// the surface at arcsin(p/η) from the vertical.
    pub fn surface_eta(&self, wave: Wave) -> f64 {
        EARTH_RADIUS_KM / self.profile(wave).velocity_at(EARTH_RADIUS_KM)
    }

    /// Where a ray of `phase` leaving at `takeoff_rad` lands, without recording its path:
    /// cheaper than [`Self::trace`]. `None` if this ray is not part of the phase.
    pub fn landing(&self, phase: RayPhase, depth_km: f64, takeoff_rad: f64) -> Option<Landing> {
        let ray = self.trace_inner(phase, depth_km, takeoff_rad, false)?;
        Some(Landing {
            takeoff_rad,
            dist_deg: fold_rad(ray.end().theta_rad).to_degrees(),
            time_s: ray.arrival_s(),
            p_s_rad: ray.p_s_rad,
        })
    }

    /// Trace a ray of `phase` from a source at `depth_km` with take-off angle `takeoff_rad`
    /// (0 = straight down, π = straight up). `None` if this ray is not part of the phase.
    pub fn trace(&self, phase: RayPhase, depth_km: f64, takeoff_rad: f64) -> Option<Ray> {
        self.trace_inner(phase, depth_km, takeoff_rad, true)
    }

    fn trace_inner(
        &self,
        phase: RayPhase,
        depth_km: f64,
        takeoff_rad: f64,
        record: bool,
    ) -> Option<Ray> {
        let depth_km = depth_km.clamp(0.0, EARTH_RADIUS_KM - 1.0);
        let r_s = EARTH_RADIUS_KM - depth_km;
        let p = self.ray_param(phase, depth_km, takeoff_rad);
        let source = PathPoint {
            r_km: r_s,
            theta_rad: 0.0,
            t_s: 0.0,
        };
        let upgoing = takeoff_rad > FRAC_PI_2;
        let mut path = PathBuilder::new(source, record);
        let mut marks = Marks::default();
        let mut bottom = source;

        let hops = if upgoing {
            if !phase.has_upgoing() || r_s >= EARTH_RADIUS_KM {
                return None;
            }
            // The surface-to-source leg, followed up.
            let leg = self.descend(phase.source_wave(), EARTH_RADIUS_KM, r_s, p, record);
            if !matches!(leg.end, LegEnd::Reached(_)) {
                return None;
            }
            path.up(&leg);
            marks.first_surface = path.t;
            // A depth phase then reflects off the surface above the source (sP converted to P
            // there, keeping its ray parameter) and makes one hop through the mantle.
            usize::from(phase.is_depth_phase())
        } else {
            if phase.is_depth_phase() {
                return None;
            }
            phase.hops()
        };
        for hop in 0..hops {
            let waves = phase.hop_waves(hop, upgoing);
            let from_source = hop == 0 && !upgoing;
            let r0 = if from_source { r_s } else { EARTH_RADIUS_KM };
            let must_leave = !from_source && (upgoing || waves.0 != phase.source_wave());
            let deepest = self.hop(
                &mut path,
                &mut marks,
                waves,
                phase.bottom(),
                phase.core_arcs(),
                r0,
                p,
                must_leave,
            )?;
            if hop == 0 {
                bottom = deepest;
                if !upgoing {
                    marks.first_surface = path.t;
                }
            }
        }

        let (shared_until_s, distinct_after_s) = phase.shared_and_distinct(&marks);
        Some(Ray {
            phase,
            takeoff_rad,
            p_s_rad: p,
            points: path.finish(),
            bottom,
            shared_until_s,
            distinct_after_s,
            converted_at_s: phase.conversion(&marks),
        })
    }

    /// Every arrival of `phase` at epicentral distance `dist_deg` from a source at `depth_km`,
    /// earliest first. Found by scanning take-off angles and bisecting each bracket, so every
    /// branch of a triplication is found.
    pub fn arrivals(&self, phase: RayPhase, depth_km: f64, dist_deg: f64) -> Vec<Arrival> {
        const STEPS: usize = 3600;
        let target = dist_deg.to_radians();
        let Some((lo, hi)) = phase.takeoff_range(depth_km) else {
            return Vec::new();
        };
        let miss = |i: f64| -> Option<f64> {
            self.trace_inner(phase, depth_km, i, false)
                .map(|r| fold_rad(r.end().theta_rad) - target)
        };
        let mut out = Vec::new();
        let mut prev: Option<(f64, f64)> = None;
        for k in 0..=STEPS {
            let i = lo + (hi - lo) * k as f64 / STEPS as f64;
            let cur = miss(i).map(|m| (i, m));
            // A distance at the end of the range (180° for PKIKP, 0° for upgoing rays) is
            // touched, not crossed, so there is no bracket: accept samples that land on it.
            if let Some((_, m)) = cur
                && m.abs() < 1e-5
                && let Some(ray) = self.trace_inner(phase, depth_km, i, false)
            {
                out.push(Arrival {
                    time_s: ray.arrival_s(),
                    ray_param_s_deg: ray.p_s_rad.to_radians(),
                    takeoff_rad: i,
                });
                prev = None; // the next bracket would find the same root
                continue;
            }
            if let (Some((i0, m0)), Some((i1, m1))) = (prev, cur)
                && (m0 <= 0.0) != (m1 <= 0.0)
            {
                let (mut lo, mut hi, mut mlo) = (i0, i1, m0);
                for _ in 0..60 {
                    let mid = 0.5 * (lo + hi);
                    match miss(mid) {
                        Some(m) if (m <= 0.0) == (mlo <= 0.0) => {
                            lo = mid;
                            mlo = m;
                        }
                        Some(_) => hi = mid,
                        None => break,
                    }
                }
                let i = 0.5 * (lo + hi);
                if let Some(ray) = self.trace_inner(phase, depth_km, i, false)
                    && (fold_rad(ray.end().theta_rad) - target).abs() < 1e-6
                {
                    out.push(Arrival {
                        time_s: ray.arrival_s(),
                        ray_param_s_deg: ray.p_s_rad.to_radians(),
                        takeoff_rad: i,
                    });
                }
            }
            prev = cur;
        }
        out.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracer() -> RayTracer {
        let model =
            EarthModel::from_bytes(include_bytes!("../../../assets/ak135_model.csv")).unwrap();
        RayTracer::new(&model).unwrap()
    }

    #[test]
    fn shell_power_law_matches_end_velocities() {
        let s = Shell::new(6000.0, 5990.0, 10.0, 10.2);
        assert!((s.velocity(6000.0) - 10.0).abs() < 1e-9);
        assert!((s.velocity(5990.0) - 10.2).abs() < 1e-9);
        assert!((s.radius_at_eta(s.eta(5995.0)) - 5995.0).abs() < 1e-6);
    }

    #[test]
    fn uniform_sphere_matches_straight_line_geometry() {
        // In a uniform sphere rays are straight chords: a ray turning at radius r_t covers
        // 2·arccos(r_t/R) and takes 2·√(R² − r_t²)/v.
        let (r, v) = (EARTH_RADIUS_KM, 8.0);
        let shell = Shell::new(r, 0.0, v, v);
        let p = 3000.0 / v; // turning radius 3000 km
        let (th0, t0) = shell.potentials(p, shell.eta(r));
        let (th1, t1) = shell.potentials(p, p);
        assert!((2.0 * (th0 - th1) - 2.0 * (3000.0 / r).acos()).abs() < 1e-12);
        let chord_t = 2.0 * (r * r - 3000.0f64.powi(2)).sqrt() / v;
        assert!((2.0 * (t0 - t1) - chord_t).abs() < 1e-9);
    }

    #[test]
    fn straight_down_pkikp_crosses_the_centre() {
        let rt = tracer();
        let ray = rt.trace(RayPhase::PKIKP, 0.0, 0.0).unwrap();
        assert!(
            (ray.end().theta_rad - PI).abs() < 1e-3,
            "{}",
            ray.end().theta_rad
        );
        assert!(ray.bottom.r_km < 1.0);
        // Travel time is twice the vertical one-way time through the whole model.
        let one_way = rt
            .descend(Wave::P, EARTH_RADIUS_KM, 0.0, MIN_RAY_PARAM, false)
            .t;
        assert!((ray.arrival_s() - 2.0 * one_way).abs() < 0.01);
    }

    #[test]
    fn phases_bottom_where_they_should() {
        let rt = tracer();
        let p = rt.trace(RayPhase::P, 0.0, 30f64.to_radians()).unwrap();
        assert!(p.bottom.r_km > rt.cmb_radius_km());
        let pcp = rt.trace(RayPhase::PcP, 0.0, 5f64.to_radians()).unwrap();
        assert!((pcp.bottom.r_km - rt.cmb_radius_km()).abs() < 1e-9);
        assert!(rt.trace(RayPhase::P, 0.0, 5f64.to_radians()).is_none());
        let pkp = rt.trace(RayPhase::PKP, 0.0, 10f64.to_radians()).unwrap();
        assert!(pkp.bottom.r_km < rt.cmb_radius_km() && pkp.bottom.r_km > rt.icb_radius_km());
        assert!(pkp.shared_until_s > 0.0 && pkp.shared_until_s < pkp.bottom.t_s);
        let sks = rt.trace(RayPhase::SKS, 0.0, 10f64.to_radians()).unwrap();
        let r_cmb = rt.cmb_radius_km();
        assert_eq!(sks.wave_at(sks.bottom.t_s, sks.bottom.r_km, r_cmb), Wave::P);
        assert_eq!(sks.wave_at(1.0, 6000.0, r_cmb), Wave::S);
    }

    #[test]
    fn depth_phases_bounce_off_the_surface_above_the_source() {
        let rt = tracer();
        let r_cmb = rt.cmb_radius_km();
        for phase in [RayPhase::pP, RayPhase::sP] {
            // Upward only, and only from a buried source.
            assert!(rt.trace(phase, 100.0, 30f64.to_radians()).is_none());
            assert!(rt.trace(phase, 0.0, 160f64.to_radians()).is_none());
            assert_eq!(phase.takeoff_range(0.0), None);

            let ray = rt.trace(phase, 100.0, 160f64.to_radians()).unwrap();
            let up = rt.trace(
                if phase == RayPhase::pP {
                    RayPhase::P
                } else {
                    RayPhase::S
                },
                100.0,
                160f64.to_radians(),
            );
            let up = up.unwrap();
            // The way up is the direct upgoing ray's; the bounce is where that one ends.
            assert!((ray.shared_until_s - up.arrival_s()).abs() < 1e-9);
            let bounce = ray.point_at(ray.shared_until_s).unwrap();
            assert!((bounce.r_km - EARTH_RADIUS_KM).abs() < 1e-6);
            assert!((bounce.theta_rad - up.end().theta_rad).abs() < 1e-9);
            // Then down through the mantle and up again, as P.
            assert!(ray.bottom.r_km < EARTH_RADIUS_KM - 100.0 && ray.bottom.r_km > r_cmb);
            assert!(ray.bottom.t_s > ray.shared_until_s);
            let hop = ray.bottom.theta_rad - bounce.theta_rad;
            assert!((ray.end().theta_rad - bounce.theta_rad - 2.0 * hop).abs() < 1e-9);
            let t_up = 0.5 * ray.shared_until_s;
            let r_up = ray.point_at(t_up).unwrap().r_km;
            assert_eq!(ray.wave_at(t_up, r_up, r_cmb), phase.source_wave());
            assert_eq!(ray.wave_at(ray.bottom.t_s, ray.bottom.r_km, r_cmb), Wave::P);
        }
        assert_eq!(RayPhase::sP.source_wave(), Wave::S);
        assert_eq!(RayPhase::sP.arrival_wave(), Wave::P);
        // sP leaving too close to horizontal has a ray parameter no P wave at the surface can
        // carry: there is no sP for it.
        assert!(rt.trace(RayPhase::sP, 100.0, 100f64.to_radians()).is_none());
        assert!(rt.trace(RayPhase::pP, 100.0, 100f64.to_radians()).is_some());
    }

    #[test]
    fn upgoing_rays_reach_the_surface_near_a_buried_source() {
        let rt = tracer();
        let up = rt.trace(RayPhase::P, 100.0, 170f64.to_radians()).unwrap();
        assert!(up.end().theta_rad < 2f64.to_radians());
        assert!((up.end().r_km - EARTH_RADIUS_KM).abs() < 1e-9);
        assert_eq!(up.bottom.r_km, EARTH_RADIUS_KM - 100.0);
        assert!(
            rt.trace(RayPhase::PcP, 100.0, 170f64.to_radians())
                .is_none()
        );
    }

    #[test]
    fn paths_are_continuous_and_time_ordered() {
        let rt = tracer();
        for phase in RayPhase::ALL {
            for deg in [1.0, 4.0, 10.0, 20.0, 35.0, 60.0, 89.0, 120.0, 150.0, 170.0] {
                let Some(ray) = rt.trace(phase, 29.0, f64::to_radians(deg)) else {
                    continue;
                };
                for w in ray.points.windows(2) {
                    assert!(w[1].t_s >= w[0].t_s, "{phase:?} {deg}: time goes back");
                    assert!(
                        chord_km(&w[0], &w[1]) <= MAX_CHORD_KM + 1e-6,
                        "{phase:?} {deg}: gap {}",
                        chord_km(&w[0], &w[1])
                    );
                }
                assert!((ray.end().r_km - EARTH_RADIUS_KM).abs() < 1e-6);
            }
        }
    }

    /// Earliest ray-traced arrival of `phase` vs the table node, within `tol` seconds.
    fn check_against_table(
        rt: &RayTracer,
        table: &crate::tables::TravelTimeTable,
        phase: RayPhase,
        table_phase: &str,
        depth_km: f64,
        dist_deg: f64,
        tol: f64,
    ) {
        let i_depth = ((depth_km - table.depth_axis().start) / table.depth_axis().step).round();
        let i_dist =
            ((dist_deg - table.distance_axis().start) / table.distance_axis().step).round();
        let want = table
            .node_time(table_phase, i_depth as usize, i_dist as usize)
            .unwrap_or_else(|| panic!("table has no {table_phase} at {depth_km} km {dist_deg}°"));
        let got = rt.arrivals(phase, depth_km, dist_deg);
        let first = got
            .first()
            .unwrap_or_else(|| panic!("no traced {phase:?} at {depth_km} km {dist_deg}°"));
        assert!(
            (first.time_s - want).abs() <= tol,
            "{phase:?} at {depth_km} km {dist_deg}°: traced {:.2} s, table {want:.2} s",
            first.time_s
        );
    }

    #[test]
    fn traced_p_and_s_match_the_table() {
        let rt = tracer();
        let table =
            crate::tables::TravelTimeTable::from_bytes(include_bytes!("../../../assets/ak135.tt"))
                .unwrap();
        for depth in [0.0, 100.0] {
            for dist in [30.0, 60.0, 90.0] {
                check_against_table(&rt, &table, RayPhase::P, "P", depth, dist, 1.0);
                check_against_table(&rt, &table, RayPhase::S, "S", depth, dist, 1.0);
            }
        }
    }

    #[test]
    fn traced_core_and_reflected_phases_match_the_table() {
        let rt = tracer();
        let table =
            crate::tables::TravelTimeTable::from_bytes(include_bytes!("../../../assets/ak135.tt"))
                .unwrap();
        let cases = [
            (RayPhase::PcP, "PcP", 40.0),
            (RayPhase::ScS, "ScS", 40.0),
            (RayPhase::PKIKP, "PKPdf", 150.0),
            (RayPhase::PKIKP, "PKPdf", 180.0),
            (RayPhase::SKS, "SKS", 100.0),
            (RayPhase::PP, "PP", 120.0),
            (RayPhase::SS, "SS", 120.0),
        ];
        for (phase, name, dist) in cases {
            check_against_table(&rt, &table, phase, name, 100.0, dist, 1.0);
        }
        // PKP: both outer-core branches are traced; each table branch matches one of them.
        let got = rt.arrivals(RayPhase::PKP, 100.0, 150.0);
        for branch in ["PKPab", "PKPbc"] {
            let want = table.arrival(branch, 100.0, 150.0).unwrap();
            assert!(
                got.iter().any(|a| (a.time_s - want).abs() <= 1.0),
                "{branch} {want:.2} s not among traced PKP {got:?}"
            );
        }
    }

    #[test]
    fn traced_depth_phases_match_the_table() {
        let rt = tracer();
        let table =
            crate::tables::TravelTimeTable::from_bytes(include_bytes!("../../../assets/ak135.tt"))
                .unwrap();
        // Away from the triplications at 15-30°, where each phase has one branch.
        for depth in [100.0, 600.0] {
            for dist in [40.0, 60.0, 80.0] {
                for (phase, name) in [(RayPhase::pP, "pP"), (RayPhase::sP, "sP")] {
                    assert_eq!(
                        rt.arrivals(phase, depth, dist).len(),
                        1,
                        "{name} {depth} {dist}"
                    );
                    check_against_table(&rt, &table, phase, name, depth, dist, 1.0);
                }
            }
        }
    }

    #[test]
    fn fast_trace_agrees_with_recorded_trace() {
        let rt = tracer();
        for (phase, takeoff) in [
            (RayPhase::PP, 0.4),
            (RayPhase::sP, 2.8),
            (RayPhase::PcS, 0.2),
            (RayPhase::PKKP, 0.15),
            (RayPhase::SP, 0.5),
        ] {
            let a = rt.trace_inner(phase, 29.0, takeoff, true).unwrap();
            let b = rt.trace_inner(phase, 29.0, takeoff, false).unwrap();
            assert!((a.arrival_s() - b.arrival_s()).abs() < 1e-9);
            assert!((a.end().theta_rad - b.end().theta_rad).abs() < 1e-12);
        }
    }
    #[test]
    fn converted_phases_change_wave_where_they_convert() {
        let rt = tracer();
        let r_cmb = rt.cmb_radius_km();
        // (phase, take-off, converts at the core rather than at the surface)
        for (phase, takeoff, at_core) in [
            (RayPhase::PcS, 0.2, true),
            // An S ray this steep or steeper: a slanter one has no P leg reaching the core.
            (RayPhase::ScP, 0.1, true),
            // Slanted enough for the S leg to turn above the core.
            (RayPhase::PS, 0.9, false),
            (RayPhase::SP, 0.5, false),
        ] {
            let ray = rt
                .trace(phase, 100.0, takeoff)
                .unwrap_or_else(|| panic!("no {phase:?}"));
            let tc = ray.converted_at_s;
            let at = ray.point_at(tc).unwrap();
            if at_core {
                assert!((at.r_km - r_cmb).abs() < 1e-6, "{phase:?} at {at:?}");
                assert!((tc - ray.bottom.t_s).abs() < 1e-9);
            } else {
                assert!(
                    (at.r_km - EARTH_RADIUS_KM).abs() < 1e-6,
                    "{phase:?} at {at:?}"
                );
            }
            // Down the way the unconverted phase goes, then its own leg.
            assert_eq!(ray.shared_until_s, tc);
            let before = ray.point_at(0.5 * tc).unwrap();
            let after = ray.point_at(0.5 * (tc + ray.arrival_s())).unwrap();
            assert_eq!(
                ray.wave_at(before.t_s, before.r_km, r_cmb),
                phase.source_wave()
            );
            assert_eq!(
                ray.wave_at(after.t_s, after.r_km, r_cmb),
                phase.arrival_wave()
            );
            assert_ne!(phase.source_wave(), phase.arrival_wave());
        }
        // Phases that keep their wave never convert.
        let pp = rt.trace(RayPhase::PP, 100.0, 0.5).unwrap();
        assert_eq!(pp.converted_at_s, 0.0);
    }

    #[test]
    fn converted_phases_are_reciprocal_from_a_surface_source() {
        // From the surface, PcS and ScP run the same path in opposite directions, and so do PS
        // and SP: their times are equal. From depth they are not.
        let rt = tracer();
        // (pair, distance from the surface source, from the deep one: PS from 600 km first
        // arrives near 100°)
        for (a, b, dist, deep_dist) in [
            (RayPhase::PcS, RayPhase::ScP, 40.0, 30.0),
            (RayPhase::PS, RayPhase::SP, 60.0, 100.0),
        ] {
            let (ta, tb) = (
                rt.arrivals(a, 0.0, dist)[0].time_s,
                rt.arrivals(b, 0.0, dist)[0].time_s,
            );
            assert!((ta - tb).abs() < 0.01, "{a:?} {ta} vs {b:?} {tb}");
            let (da, db) = (
                rt.arrivals(a, 600.0, deep_dist)[0].time_s,
                rt.arrivals(b, 600.0, deep_dist)[0].time_s,
            );
            assert!(
                (da - db).abs() > 10.0,
                "{a:?} {da} vs {b:?} {db} from 600 km"
            );
        }
    }

    #[test]
    fn core_reflections_bounce_where_they_should() {
        let rt = tracer();
        let (r_cmb, r_icb) = (rt.cmb_radius_km(), rt.icb_radius_km());
        // PKiKP reflects off the inner core, sharing its way down with PKIKP until there.
        let ray = rt.trace(RayPhase::PKiKP, 29.0, 0.05).unwrap();
        assert!((ray.bottom.r_km - r_icb).abs() < 1e-6);
        assert!((ray.shared_until_s - ray.bottom.t_s).abs() < 1e-9);
        let pkikp = rt.trace(RayPhase::PKIKP, 29.0, 0.05).unwrap();
        let (q, k) = (
            ray.point_at(0.9 * ray.bottom.t_s).unwrap(),
            pkikp.point_at(0.9 * ray.bottom.t_s).unwrap(),
        );
        assert!((q.r_km - k.r_km).abs() < 1e-6 && (q.theta_rad - k.theta_rad).abs() < 1e-9);
        assert!(ray.points.iter().all(|q| q.r_km >= r_icb - 1e-6));
        // PKKP and SKKS cross the outer core twice, touching the core-mantle boundary from
        // below in between; until then they go the way PKP and SKS do.
        for (phase, single) in [
            (RayPhase::PKKP, RayPhase::PKP),
            (RayPhase::SKKS, RayPhase::SKS),
        ] {
            let ray = rt.trace(phase, 29.0, 0.15).unwrap();
            let one = rt.trace(single, 29.0, 0.15).unwrap();
            assert!(ray.bottom.r_km < r_cmb && ray.bottom.r_km > r_icb);
            let under = ray.point_at(ray.shared_until_s).unwrap();
            assert!((under.r_km - r_cmb).abs() < 1e-6, "{phase:?} {under:?}");
            assert!(ray.shared_until_s > ray.bottom.t_s);
            // The second crossing adds one more arc through the core.
            let arc_t = ray.shared_until_s - one.shared_until_s;
            assert!((ray.arrival_s() - one.arrival_s() - arc_t).abs() < 1e-6);
            let mid = ray
                .point_at(0.5 * (ray.shared_until_s + ray.arrival_s()))
                .unwrap();
            assert_eq!(ray.wave_at(mid.t_s, mid.r_km, r_cmb), Wave::P);
        }
    }

    #[test]
    fn traced_new_phases_match_the_table() {
        let rt = tracer();
        let table =
            crate::tables::TravelTimeTable::from_bytes(include_bytes!("../../../assets/ak135.tt"))
                .unwrap();
        // Away from triplications, where each phase has one branch.
        let cases = [
            (RayPhase::PcS, "PcS", 0.0, 40.0),
            (RayPhase::ScP, "ScP", 600.0, 30.0),
            (RayPhase::PcS, "PcS", 600.0, 30.0),
            (RayPhase::PKiKP, "PKiKP", 100.0, 60.0),
            (RayPhase::PKiKP, "PKiKP", 100.0, 120.0),
            (RayPhase::PKKP, "PKKP", 0.0, 100.0),
            (RayPhase::SKKS, "SKKS", 100.0, 90.0),
            (RayPhase::PS, "PS", 0.0, 60.0),
            (RayPhase::SP, "SP", 600.0, 60.0),
            (RayPhase::PS, "PS", 600.0, 100.0),
        ];
        for (phase, name, depth, dist) in cases {
            check_against_table(&rt, &table, phase, name, depth, dist, 1.0);
        }
    }
}
