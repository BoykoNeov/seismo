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
/// that leave a buried source upward (`p`, `s`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
}

/// Where a phase's ray bottoms out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bottom {
    TurnMantle,
    ReflectCmb,
    TurnOuterCore,
    TurnInnerCore,
}

impl RayPhase {
    pub const ALL: [RayPhase; 9] = [
        RayPhase::P,
        RayPhase::S,
        RayPhase::PcP,
        RayPhase::ScS,
        RayPhase::PKP,
        RayPhase::PKIKP,
        RayPhase::SKS,
        RayPhase::PP,
        RayPhase::SS,
    ];

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
        }
    }

    /// Wave type of the legs above the core (in the core every phase here travels as P).
    pub fn mantle_wave(self) -> Wave {
        match self {
            RayPhase::P | RayPhase::PcP | RayPhase::PKP | RayPhase::PKIKP | RayPhase::PP => Wave::P,
            RayPhase::S | RayPhase::ScS | RayPhase::SKS | RayPhase::SS => Wave::S,
        }
    }

    fn bottom(self) -> Bottom {
        match self {
            RayPhase::P | RayPhase::S | RayPhase::PP | RayPhase::SS => Bottom::TurnMantle,
            RayPhase::PcP | RayPhase::ScS => Bottom::ReflectCmb,
            RayPhase::PKP | RayPhase::SKS => Bottom::TurnOuterCore,
            RayPhase::PKIKP => Bottom::TurnInnerCore,
        }
    }

    /// Surface-to-surface hops: 2 for the surface reflections PP and SS.
    fn hops(self) -> usize {
        match self {
            RayPhase::PP | RayPhase::SS => 2,
            _ => 1,
        }
    }

    /// Whether rays leaving the source upward belong to this phase.
    pub fn has_upgoing(self) -> bool {
        matches!(self, RayPhase::P | RayPhase::S)
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
    /// mantle leg with PcP, PP its first hop with P).
    pub shared_until_s: f64,
    /// From this time the ray is on the leg that gives the phase its name (after the core
    /// reflection for PcP, after the core entry for PKP, ...).
    pub distinct_after_s: f64,
}

impl Ray {
    pub fn arrival_s(&self) -> f64 {
        self.points.last().map_or(0.0, |q| q.t_s)
    }

    pub fn end(&self) -> PathPoint {
        *self.points.last().expect("a ray has at least two points")
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
        })
    }

    /// Radius (km) of the core-mantle boundary.
    pub fn cmb_radius_km(&self) -> f64 {
        self.r_cmb
    }

    /// Radius (km) of the inner-core boundary.
    pub fn icb_radius_km(&self) -> f64 {
        self.r_icb
    }

    /// Wave type at radius `r` on a ray of `phase`.
    pub fn wave_at(&self, phase: RayPhase, r_km: f64) -> Wave {
        if r_km > self.r_cmb {
            phase.mantle_wave()
        } else {
            Wave::P
        }
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

    /// Whether a downward leg ending at `end` is the right one for `bottom`.
    fn leg_ok(&self, bottom: Bottom, end: LegEnd) -> bool {
        match (bottom, end) {
            (Bottom::TurnMantle, LegEnd::Turned(r)) => r > self.r_cmb,
            (Bottom::ReflectCmb, LegEnd::Reached(_)) => true,
            (Bottom::TurnOuterCore, LegEnd::Turned(r)) => r < self.r_cmb,
            // Turning exactly at the ICB is total reflection off it (PKiKP), not PKIKP.
            (Bottom::TurnInnerCore, LegEnd::Turned(r)) => r < self.r_icb,
            _ => false,
        }
    }

    fn stop_radius(&self, bottom: Bottom) -> f64 {
        match bottom {
            Bottom::TurnMantle | Bottom::ReflectCmb => self.r_cmb,
            Bottom::TurnOuterCore => self.r_icb,
            Bottom::TurnInnerCore => 0.0,
        }
    }

    /// Ray parameter (s/rad) of a ray of `phase` leaving a source at `depth_km` at `takeoff_rad`
    /// from straight down.
    pub fn ray_param(&self, phase: RayPhase, depth_km: f64, takeoff_rad: f64) -> f64 {
        let r_s = EARTH_RADIUS_KM - depth_km;
        let eta_s = r_s / self.profile(phase.mantle_wave()).velocity_at(r_s);
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
        let wave = phase.mantle_wave();
        let p = self.ray_param(phase, depth_km, takeoff_rad);
        let source = PathPoint {
            r_km: r_s,
            theta_rad: 0.0,
            t_s: 0.0,
        };

        if takeoff_rad > FRAC_PI_2 {
            if !phase.has_upgoing() || r_s >= EARTH_RADIUS_KM {
                return None;
            }
            // Upgoing: the surface-to-source leg, reversed.
            let leg = self.descend(wave, EARTH_RADIUS_KM, r_s, p, record);
            if !matches!(leg.end, LegEnd::Reached(_)) {
                return None;
            }
            let mut points: Vec<PathPoint> = leg
                .points
                .iter()
                .rev()
                .map(|q| PathPoint {
                    r_km: q.r_km,
                    theta_rad: leg.theta - q.theta_rad,
                    t_s: leg.t - q.t_s,
                })
                .collect();
            if !record {
                points = vec![
                    source,
                    PathPoint {
                        r_km: EARTH_RADIUS_KM,
                        theta_rad: leg.theta,
                        t_s: leg.t,
                    },
                ];
            }
            return Some(Ray {
                phase,
                takeoff_rad,
                p_s_rad: p,
                points: decimate(points),
                bottom: source,
                shared_until_s: 0.0,
                distinct_after_s: 0.0,
            });
        }

        let bottom = phase.bottom();
        let r_stop = self.stop_radius(bottom);
        let src = self.descend(wave, r_s, r_stop, p, record);
        if !self.leg_ok(bottom, src.end) {
            return None;
        }
        let surf = self.descend(wave, EARTH_RADIUS_KM, r_stop, p, record);
        if !self.leg_ok(bottom, surf.end) || surf.end.radius() != src.end.radius() {
            return None;
        }

        let mut points = src.points;
        let (mut theta, mut t) = (src.theta, src.t);
        let bottom_pt = PathPoint {
            r_km: src.end.radius(),
            theta_rad: theta,
            t_s: t,
        };
        // Up from the bottom: the surface leg mirrored. Further hops: down then up again.
        let mut first_surface_t = 0.0;
        for hop in 0..phase.hops() {
            if hop > 0 {
                points.extend(surf.points.iter().skip(1).map(|q| PathPoint {
                    r_km: q.r_km,
                    theta_rad: theta + q.theta_rad,
                    t_s: t + q.t_s,
                }));
                theta += surf.theta;
                t += surf.t;
            }
            points.extend(surf.points.iter().rev().skip(1).map(|q| PathPoint {
                r_km: q.r_km,
                theta_rad: theta + surf.theta - q.theta_rad,
                t_s: t + surf.t - q.t_s,
            }));
            theta += surf.theta;
            t += surf.t;
            if hop == 0 {
                first_surface_t = t;
            }
        }
        if !record {
            points = vec![
                source,
                PathPoint {
                    r_km: EARTH_RADIUS_KM,
                    theta_rad: theta,
                    t_s: t,
                },
            ];
        }

        let core_entry_t = || self.descend(wave, r_s, self.r_cmb, p, false).t;
        let (shared_until_s, distinct_after_s) = match phase {
            RayPhase::P | RayPhase::S => (0.0, 0.0),
            RayPhase::PcP | RayPhase::ScS => (0.0, bottom_pt.t_s),
            RayPhase::PKP | RayPhase::PKIKP | RayPhase::SKS => {
                let tc = core_entry_t();
                (tc, tc)
            }
            RayPhase::PP | RayPhase::SS => (first_surface_t, first_surface_t),
        };
        Some(Ray {
            phase,
            takeoff_rad,
            p_s_rad: p,
            points: decimate(points),
            bottom: bottom_pt,
            shared_until_s,
            distinct_after_s,
        })
    }

    /// Every arrival of `phase` at epicentral distance `dist_deg` from a source at `depth_km`,
    /// earliest first. Found by scanning take-off angles and bisecting each bracket, so every
    /// branch of a triplication is found.
    pub fn arrivals(&self, phase: RayPhase, depth_km: f64, dist_deg: f64) -> Vec<Arrival> {
        const STEPS: usize = 3600;
        let target = dist_deg.to_radians();
        let max_angle = if phase.has_upgoing() { PI } else { FRAC_PI_2 };
        let miss = |i: f64| -> Option<f64> {
            self.trace_inner(phase, depth_km, i, false)
                .map(|r| fold_rad(r.end().theta_rad) - target)
        };
        let mut out = Vec::new();
        let mut prev: Option<(f64, f64)> = None;
        for k in 0..=STEPS {
            let i = max_angle * k as f64 / STEPS as f64;
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
        assert_eq!(rt.wave_at(RayPhase::SKS, sks.bottom.r_km), Wave::P);
        assert_eq!(rt.wave_at(RayPhase::SKS, 6000.0), Wave::S);
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
            for deg in [1.0, 4.0, 10.0, 20.0, 35.0, 60.0, 89.0, 120.0] {
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
    fn fast_trace_agrees_with_recorded_trace() {
        let rt = tracer();
        let a = rt.trace_inner(RayPhase::PP, 29.0, 0.4, true).unwrap();
        let b = rt.trace_inner(RayPhase::PP, 29.0, 0.4, false).unwrap();
        assert!((a.arrival_s() - b.arrival_s()).abs() < 1e-9);
        assert!((a.end().theta_rad - b.end().theta_rad).abs() < 1e-12);
    }
}
