//! Rays bent through the 3D mantle, in the plane of the slice.
//!
//! Each ray is integrated through v(x) = v₁(r)·(1 + s·dlnV(x)), with v₁ the 1D model (velocity
//! linear in depth between its samples, which the closed-form tracer's shells approximate),
//! dlnV the 3D model along the slice's great circle and s the strength. The ray equations are the kinematic ones in the plane, in
//! Cartesian coordinates (no singularity at the centre) and with time as the parameter:
//!
//! ```text
//! dx/dt = v² p        dp/dt = −∇v / v        |p| = 1/v
//! ```
//!
//! integrated with fixed-step RK4. A step that would cross a velocity discontinuity is cut
//! short: the crossing is found by re-integrating shorter partial steps (a bracketing root
//! finder on the step length), and the slowness is refracted (Snell's law: the tangential component is kept),
//! reflected, or the ray rejected, by the same per-phase rules as the closed-form tracer.
//! The rays stay in the plane: the out-of-plane part of the gradient is ignored.

use super::{Bottom, MIN_RAY_PARAM, PathPoint, Profile, Ray, RayPhase, RayTracer, Wave, decimate};
use crate::geo::{EARTH_RADIUS_KM, destination};
use crate::mantle::Mantle3d;
use crate::slice::{Fan, Side};

use std::f64::consts::{PI, TAU};

/// Integration step (s).
const DT_S: f64 = 4.0;
/// Shorter step (s) near a turning point, where the ray runs within ~15° of horizontal and the
/// error builds up fastest.
const DT_TURN_S: f64 = 0.5;
const TURN_SIN: f64 = 0.25;
/// Rays taking more steps than this are dropped (a guard against a ray stalling on a kink).
const MAX_STEPS: usize = 20_000;
/// Rays still travelling after this long (s) are dropped.
const MAX_TIME_S: f64 = 3600.0;
/// The velocity factor 1 + s·dlnV is kept at or above this, so exaggerated models stay
/// physical.
const MIN_FACTOR: f64 = 0.2;
/// Spacing of the plane grid: along the great circle (deg) and in radius (km, at most).
const PLANE_STEP_DEG: f64 = 0.5;
const PLANE_MAX_DR_KM: f64 = 12.5;
/// Most root-finder steps when locating where a step crosses a discontinuity or a kink, or
/// where the ray turns.
const SOLVE_ITERATIONS: usize = 40;
/// Shortest step the root finder returns, as a fraction of the step it was given.
const MIN_STEP_FRACTION: f64 = 1e-3;
/// A ray this close (km) to a kink counts as on it: its next step uses the interval it is
/// heading into.
const KINK_EPS_KM: f64 = 1e-3;
/// Fan refinement, as for the 1D fans: neighbours whose surface or bottom points are farther
/// apart than this (km) get a ray between them, down to take-off gaps of `MIN_REFINE_RAD`;
/// where one neighbour is part of the phase and the other is not, the edge is bisected down to
/// `MIN_EDGE_RAD`. A fan never grows past `MAX_FAN_RAYS`.
const REFINE_KM: f64 = 60.0;
const MIN_REFINE_RAD: f64 = PI / 360.0 / 1024.0;
const MIN_EDGE_RAD: f64 = PI / 360.0 / 16384.0;
const MAX_FAN_RAYS: usize = 2000;
/// Seeds taken from a 1D fan, at most, besides its edges.
const MAX_SEEDS: usize = 300;
/// Probes this far (rad) beyond each edge of the 1D fan, so the bent fan's edge can be found
/// by bisection even where it has moved.
const EDGE_PROBE_RAD: f64 = PI / 360.0;

/// dlnVp and dlnVs on the whole great circle through the epicentre at one azimuth: angle from
/// the epicentre along the azimuth in [0°, 360°), radius from the bottom of the 3D model to the
/// 1D model's Moho.
#[derive(Debug, Clone)]
pub struct PlaneGrid {
    n_theta: usize,
    n_r: usize,
    r_bottom_km: f64,
    dr_km: f64,
    /// (dlnVp, dlnVs) per node, `[radius][theta]`.
    values: Vec<[f32; 2]>,
}

impl PlaneGrid {
    pub fn new(mantle: &Mantle3d, lat: f64, lon: f64, azimuth_deg: f64) -> Self {
        let (r_bottom_km, r_top_km) = mantle.radius_range_km();
        let n_theta = (360.0 / PLANE_STEP_DEG).round() as usize;
        let n_r = ((r_top_km - r_bottom_km) / PLANE_MAX_DR_KM).ceil() as usize + 1;
        let dr_km = (r_top_km - r_bottom_km) / (n_r - 1) as f64;
        let points: Vec<(f64, f64)> = (0..n_theta)
            .map(|j| destination(lat, lon, azimuth_deg, PLANE_STEP_DEG * j as f64))
            .collect();
        let mut values = Vec::with_capacity(n_theta * n_r);
        for i in 0..n_r {
            let r = (r_bottom_km + dr_km * i as f64).min(r_top_km);
            for &(la, lo) in &points {
                values.push([
                    mantle.dlnv(Wave::P, r, la, lo) as f32,
                    mantle.dlnv(Wave::S, r, la, lo) as f32,
                ]);
            }
        }
        Self {
            n_theta,
            n_r,
            r_bottom_km,
            dr_km,
            values,
        }
    }

    /// A grid with no perturbation anywhere.
    pub fn flat() -> Self {
        Self {
            n_theta: 0,
            n_r: 0,
            r_bottom_km: 0.0,
            dr_km: 1.0,
            values: Vec::new(),
        }
    }

    /// dlnV of `wave` and its derivatives along the radius (per km) and along the circle (per
    /// rad) at radius `r` and angle `theta` (rad, any value; wraps). Bilinear in the cell, so
    /// the derivatives are those of the cell's bilinear patch. 0 outside the radius range.
    fn sample(&self, wave: Wave, r: f64, theta: f64) -> (f64, f64, f64) {
        if self.n_r < 2 {
            return (0.0, 0.0, 0.0);
        }
        let fr = (r - self.r_bottom_km) / self.dr_km;
        if fr < 0.0 || fr > (self.n_r - 1) as f64 {
            return (0.0, 0.0, 0.0);
        }
        let step = PLANE_STEP_DEG.to_radians();
        let ft = theta.rem_euclid(TAU) / step;
        let i = (fr.floor() as usize).min(self.n_r - 2);
        let j = (ft.floor() as usize).min(self.n_theta - 1);
        let (wr, wt) = (fr - i as f64, ft - j as f64);
        let j1 = (j + 1) % self.n_theta;
        let k = match wave {
            Wave::P => 0,
            Wave::S => 1,
        };
        let at = |i: usize, j: usize| f64::from(self.values[i * self.n_theta + j][k]);
        let (c00, c01, c10, c11) = (at(i, j), at(i, j1), at(i + 1, j), at(i + 1, j1));
        let lo = c00 + (c01 - c00) * wt;
        let hi = c10 + (c11 - c10) * wt;
        let v = lo + (hi - lo) * wr;
        let d_r = (hi - lo) / self.dr_km;
        let d_t = ((c01 - c00) * (1.0 - wr) + (c11 - c10) * wr) / step;
        (v, d_r, d_t)
    }
}

/// Ray state in the plane: position (km) with the source at angle 0 and angles increasing
/// along the slice's direction, slowness (s/km), time (s), and the angle travelled (rad,
/// unwrapped, so it can exceed π).
#[derive(Debug, Clone, Copy)]
struct State {
    x: f64,
    y: f64,
    px: f64,
    py: f64,
    t: f64,
    theta: f64,
}

impl State {
    fn r(&self) -> f64 {
        self.x.hypot(self.y)
    }

    /// Radial slowness component (positive outward) and tangential component (positive towards
    /// increasing angle).
    fn radial_tangential(&self) -> (f64, f64) {
        let r = self.r().max(1e-12);
        let (ux, uy) = (self.x / r, self.y / r);
        (self.px * ux + self.py * uy, -self.px * uy + self.py * ux)
    }

    fn set_radial_tangential(&mut self, pr: f64, pt: f64) {
        let r = self.r().max(1e-12);
        let (ux, uy) = (self.x / r, self.y / r);
        self.px = pr * ux - pt * uy;
        self.py = pr * uy + pt * ux;
    }

    fn point(&self) -> PathPoint {
        PathPoint {
            r_km: self.r(),
            theta_rad: self.theta,
            t_s: self.t,
        }
    }
}

/// Traces rays of the slice's phases through the 1D model plus the plane's perturbations.
pub struct Bender<'a> {
    tracer: &'a RayTracer,
    grid: &'a PlaneGrid,
    strength: f64,
    /// +1 for rays leaving along the slice's azimuth (grid angle = ray angle), −1 for the back
    /// fans (grid angle = −ray angle).
    dir: f64,
    /// Velocity discontinuities (km), surface first, centre (0) last.
    interfaces: Vec<f64>,
    /// Radii (km) where the 1D velocity gradient changes (the model samples), surface first.
    /// Steps end on them so RK4 never integrates across a kink.
    kinks: Vec<f64>,
}

impl<'a> Bender<'a> {
    pub fn new(tracer: &'a RayTracer, grid: &'a PlaneGrid, strength: f64, side: Side) -> Self {
        let mut interfaces: Vec<f64> = [&tracer.p_profile, &tracer.s_profile]
            .into_iter()
            .flat_map(|p| p.discontinuities())
            .chain([EARTH_RADIUS_KM, tracer.r_cmb, tracer.r_icb, 0.0])
            .collect();
        interfaces.sort_by(|a, b| b.total_cmp(a));
        interfaces.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        let mut kinks: Vec<f64> = [&tracer.p_profile, &tracer.s_profile]
            .into_iter()
            .flat_map(|p| p.sample_radii())
            .filter(|r| *r > 0.0 && *r < EARTH_RADIUS_KM)
            .collect();
        kinks.sort_by(|a, b| b.total_cmp(a));
        kinks.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        Self {
            tracer,
            grid,
            strength,
            dir: match side {
                Side::Forward => 1.0,
                Side::Back => -1.0,
            },
            interfaces,
            kinks,
        }
    }

    /// The step length in (0, `h`] at which `g` of the stepped state is zero, given that `g`
    /// changes sign over the step (Illinois false position: a bracketing root finder that
    /// converges in a handful of steps). A ray starting on the root (on the surface it is about
    /// to cross again, grazing it) gets the next root, at least `MIN_STEP_FRACTION` of `h` on.
    #[allow(clippy::too_many_arguments)]
    fn solve_step(
        &self,
        profile: &Profile,
        wave: Wave,
        layer: usize,
        s: &State,
        h: f64,
        g: impl Fn(&State) -> f64,
        tol: f64,
    ) -> f64 {
        let (mut a, mut b) = (0.0, h);
        let (mut ga, mut gb) = (g(s), g(&self.step(profile, wave, layer, s, h)));
        if ga.abs() < tol {
            // Starting on the root (a ray just placed on the surface it is about to cross
            // again): look for the next root, at least a little way along.
            a = MIN_STEP_FRACTION * h;
            ga = g(&self.step(profile, wave, layer, s, a));
            if ga * gb > 0.0 {
                return a;
            }
        }
        if ga * gb > 0.0 {
            return h;
        }
        let mut side = 0;
        for _ in 0..SOLVE_ITERATIONS {
            let c = (a * gb - b * ga) / (gb - ga);
            let gc = g(&self.step(profile, wave, layer, s, c));
            if gc.abs() < tol || (b - a).abs() < 1e-12 {
                return c;
            }
            if gc * gb < 0.0 {
                (a, ga) = (b, gb);
                if side == 1 {
                    ga *= 0.5;
                }
                side = 1;
            } else {
                if side == -1 {
                    ga *= 0.5;
                }
                side = -1;
            }
            (b, gb) = (c, gc);
        }
        b
    }

    /// The first kink strictly between radii `r0` and `r1` (more than a hair from `r0`), seen
    /// from `r0`.
    fn kink_crossed(&self, r0: f64, r1: f64) -> Option<f64> {
        if r1 < r0 {
            let i = self.kinks.partition_point(|&k| k >= r0 - KINK_EPS_KM);
            self.kinks.get(i).copied().filter(|&k| k > r1 + KINK_EPS_KM)
        } else {
            let i = self.kinks.partition_point(|&k| k > r0 + KINK_EPS_KM);
            i.checked_sub(1)
                .map(|i| self.kinks[i])
                .filter(|&k| k < r1 - KINK_EPS_KM)
        }
    }

    /// Step length (at most `h`) that ends on the next kink ahead of the ray, if the ray reaches
    /// one within `h` at its current radial speed.
    fn to_next_kink(&self, profile: &Profile, wave: Wave, layer: usize, s: &State, h: f64) -> f64 {
        let r = s.r();
        let (pr, _) = s.radial_tangential();
        let (v, _, _) = self.velocity(profile, wave, layer, s.r(), s.x, s.y);
        let rdot = v * v * pr;
        // |v·pr| is the sine of the ray's angle to the horizontal.
        let h = if (v * pr).abs() < TURN_SIN {
            h.min(DT_TURN_S)
        } else {
            h
        };
        let kink = if rdot < 0.0 {
            // Kinks are sorted downwards: the first one below r.
            let i = self.kinks.partition_point(|&k| k >= r - KINK_EPS_KM);
            self.kinks.get(i).copied()
        } else if rdot > 0.0 {
            let i = self.kinks.partition_point(|&k| k > r + KINK_EPS_KM);
            i.checked_sub(1).map(|i| self.kinks[i])
        } else {
            None
        };
        match kink {
            Some(k) => {
                let hk = (k - r) / rdot;
                // Aim at the kink; the next step picks its interval by the direction of travel,
                // so landing a little short or past it is fine.
                if hk > 0.0 && hk < h { hk } else { h }
            }
            None => h,
        }
    }

    /// The layer (index into the gaps between `interfaces`) holding radius `r`, taking the one
    /// below when `r` is on an interface and the ray goes down, the one above when it goes up.
    fn layer_of(&self, r: f64, going_down: bool) -> usize {
        let n = self.interfaces.len();
        for k in 0..n - 1 {
            let (hi, lo) = (self.interfaces[k], self.interfaces[k + 1]);
            if (r < hi && r > lo)
                || (going_down && r == hi)
                || (!going_down && r == lo && k + 1 < n - 1)
            {
                return k;
            }
        }
        if going_down { n - 2 } else { 0 }
    }

    /// Velocity (km/s) of `profile` and its gradient (per s) at (x, y). The 1D part is the
    /// linear law of the model interval holding radius `lock_r`, inside layer `layer`, used
    /// even slightly outside that interval: within one step the medium is smooth.
    fn velocity(
        &self,
        profile: &Profile,
        wave: Wave,
        layer: usize,
        lock_r: f64,
        x: f64,
        y: f64,
    ) -> (f64, f64, f64) {
        let r = x.hypot(y).max(1e-9);
        let (hi, lo) = (self.interfaces[layer], self.interfaces[layer + 1]);
        let clamp = |q: f64| q.clamp(lo + 1e-7, hi - 1e-7).max(1e-7);
        let r_in = clamp(r);
        let (v1, dv1) = profile.interval_below(clamp(lock_r)).velocity(r);
        let (mut f, mut df_r, mut df_t) = (1.0, 0.0, 0.0);
        if self.strength != 0.0 {
            let pert = if r_in > self.tracer.r_cmb {
                wave
            } else {
                Wave::P
            };
            let theta = y.atan2(x) * self.dir;
            let (d, d_r, d_t) = self.grid.sample(pert, r_in, theta);
            f = 1.0 + self.strength * d;
            df_r = self.strength * d_r;
            df_t = self.strength * d_t * self.dir;
            if f < MIN_FACTOR {
                (f, df_r, df_t) = (MIN_FACTOR, 0.0, 0.0);
            }
        }
        let v = v1 * f;
        let dv_r = dv1 * f + v1 * df_r;
        let dv_t = v1 * df_t; // per rad
        // ∇v = dv/dr r̂ + (dv/dθ / r) θ̂
        let (ux, uy) = (x / r, y / r);
        let gx = dv_r * ux - dv_t / r * uy;
        let gy = dv_r * uy + dv_t / r * ux;
        (v, gx, gy)
    }

    fn deriv(
        &self,
        profile: &Profile,
        wave: Wave,
        layer: usize,
        lock_r: f64,
        s: &[f64; 4],
    ) -> [f64; 4] {
        let (v, gx, gy) = self.velocity(profile, wave, layer, lock_r, s[0], s[1]);
        let v2 = v * v;
        [v2 * s[2], v2 * s[3], -gx / v, -gy / v]
    }

    /// One RK4 step of length `h`, in the model interval the ray is entering from where it
    /// starts (the one below if it is on a kink and going down).
    fn step(&self, profile: &Profile, wave: Wave, layer: usize, s: &State, h: f64) -> State {
        let y0 = [s.x, s.y, s.px, s.py];
        let (pr, _) = s.radial_tangential();
        let lock = s.r() + KINK_EPS_KM * pr.signum();
        let add = |a: &[f64; 4], k: &[f64; 4], f: f64| {
            [
                a[0] + k[0] * f,
                a[1] + k[1] * f,
                a[2] + k[2] * f,
                a[3] + k[3] * f,
            ]
        };
        let k1 = self.deriv(profile, wave, layer, lock, &y0);
        let k2 = self.deriv(profile, wave, layer, lock, &add(&y0, &k1, 0.5 * h));
        let k3 = self.deriv(profile, wave, layer, lock, &add(&y0, &k2, 0.5 * h));
        let k4 = self.deriv(profile, wave, layer, lock, &add(&y0, &k3, h));
        let mut y = y0;
        for i in 0..4 {
            y[i] += h / 6.0 * (k1[i] + 2.0 * k2[i] + 2.0 * k3[i] + k4[i]);
        }
        // Unwrapped angle: add the wrapped change.
        let dtheta = (y[1].atan2(y[0]) - s.y.atan2(s.x) + PI).rem_euclid(TAU) - PI;
        State {
            x: y[0],
            y: y[1],
            px: y[2],
            py: y[3],
            t: s.t + h,
            theta: s.theta + dtheta,
        }
    }

    /// Velocity of `profile` just inside layer `layer` at the interface radius `r_if`, at the
    /// state's angle.
    fn velocity_in_layer(
        &self,
        profile: &Profile,
        wave: Wave,
        layer: usize,
        s: &State,
        r_if: f64,
    ) -> f64 {
        let r = s.r().max(1e-9);
        let (x, y) = (s.x / r * r_if, s.y / r * r_if);
        self.velocity(profile, wave, layer, r_if, x, y).0
    }

    /// Trace a ray of `phase` from a source `depth_km` deep with take-off angle `takeoff_rad`
    /// (0 = straight down, π = straight up). `None` if the bent ray is not part of the phase.
    pub fn trace(&self, phase: RayPhase, depth_km: f64, takeoff_rad: f64) -> Option<Ray> {
        let wave = phase.mantle_wave();
        let profile = self.tracer.profile(wave);
        let upgoing = takeoff_rad > std::f64::consts::FRAC_PI_2;
        if upgoing && !phase.has_upgoing() {
            return None;
        }
        let depth_km = depth_km.clamp(0.0, EARTH_RADIUS_KM - 1.0);
        let r_s = EARTH_RADIUS_KM - depth_km;
        if upgoing && r_s >= EARTH_RADIUS_KM {
            return None;
        }
        let mut layer = self.layer_of(r_s, !upgoing);
        let (v_s, _, _) = self.velocity(profile, wave, layer, r_s, r_s, 0.0);
        let mut s = State {
            x: r_s,
            y: 0.0,
            px: -takeoff_rad.cos() / v_s,
            // As in the closed-form tracer, at least the smallest ray parameter: a ray straight
            // down passes just beside the centre, on the side of increasing angle.
            py: (takeoff_rad.sin() / v_s).max(MIN_RAY_PARAM / r_s),
            t: 0.0,
            theta: 0.0,
        };
        let source = s.point();
        let p_s_rad = (r_s * takeoff_rad.sin() / v_s).max(MIN_RAY_PARAM);
        let bottom_kind = phase.bottom();
        let mut hops_left = phase.hops();
        let mut points = vec![source];
        let mut bottom = source;
        let mut first_hop = true;
        let (mut core_entry_t, mut first_surface_t, mut reflected_t) = (None, None, None);
        let (mut entered_core, mut entered_inner) = (false, false);
        let n_layers = self.interfaces.len() - 1;

        let mut steps = 0usize;
        while s.t < MAX_TIME_S {
            steps += 1;
            if steps > MAX_STEPS {
                return None;
            }
            let mut h = self.to_next_kink(profile, wave, layer, &s, DT_S);
            let mut next = self.step(profile, wave, layer, &s, h);
            let (hi, lo) = (self.interfaces[layer], self.interfaces[layer + 1]);
            // A step that still crosses a kink (the prediction fails where the ray curves, near
            // a turning point) is shortened to end just past the first one.
            if let Some(k) = self.kink_crossed(s.r(), next.r())
                && k < hi
                && k > lo
            {
                h = self.solve_step(profile, wave, layer, &s, h, |q| q.r() - k, 1e-4);
                next = self.step(profile, wave, layer, &s, h);
            }
            let r1 = next.r();
            let mut crossing = if r1 > hi {
                Some((hi, false))
            } else if r1 < lo && lo > 0.0 {
                Some((lo, true))
            } else {
                None
            };
            let mut turned_at = None;
            if crossing.is_none()
                && s.radial_tangential().0 < 0.0
                && next.radial_tangential().0 >= 0.0
            {
                // The ray turns within this step. If it dips below the layer's floor on the
                // way (it grazes the interface), that is a crossing; otherwise note the turning
                // point.
                let ht = self.solve_step(
                    profile,
                    wave,
                    layer,
                    &s,
                    h,
                    |q| q.radial_tangential().0,
                    1e-9,
                );
                let turn = self.step(profile, wave, layer, &s, ht);
                if let Some(k) = self.kink_crossed(s.r().min(next.r()), turn.r())
                    && k > lo
                {
                    // It dips past a kink: the turn was found in the wrong interval's medium.
                    // End the step on the kink; the next steps continue in the right one.
                    h = self.solve_step(profile, wave, layer, &s, ht, |q| q.r() - k, 1e-4);
                    next = self.step(profile, wave, layer, &s, h);
                } else if lo > 0.0 && turn.r() < lo {
                    crossing = Some((lo, true));
                    h = ht;
                } else {
                    turned_at = Some(turn);
                }
            }
            let Some((r_if, down)) = crossing else {
                if let Some(turn) = turned_at
                    && first_hop
                    && !upgoing
                    && turn.r() < bottom.r_km
                {
                    bottom = turn.point();
                }
                s = next;
                points.push(s.point());
                if first_hop && s.r() < bottom.r_km && !upgoing {
                    bottom = s.point();
                }
                continue;
            };
            // Shorten the step until it ends on the interface.
            let hc = self.solve_step(profile, wave, layer, &s, h, |q| q.r() - r_if, 1e-7);
            s = self.step(profile, wave, layer, &s, hc);
            // Put the point exactly on the interface.
            let scale = r_if / s.r();
            s.x *= scale;
            s.y *= scale;
            points.push(s.point());
            if first_hop && s.r() < bottom.r_km && !upgoing {
                bottom = s.point();
            }

            let (pr, pt) = s.radial_tangential();
            let reflect = |s: &mut State| s.set_radial_tangential(-pr, pt);

            if !down && layer == 0 {
                // At the surface.
                if hops_left > 1 {
                    hops_left -= 1;
                    first_hop = false;
                    first_surface_t.get_or_insert(s.t);
                    reflect(&mut s);
                    continue;
                }
                break;
            }
            let target = if down { layer + 1 } else { layer - 1 };
            if target >= n_layers {
                return None;
            }
            let is_cmb = (r_if - self.tracer.r_cmb).abs() < 1e-6;
            let is_icb = (r_if - self.tracer.r_icb).abs() < 1e-6;
            enum Act {
                Transmit,
                Reflect,
            }
            let act = if is_cmb && down {
                match bottom_kind {
                    Bottom::TurnMantle => return None,
                    Bottom::ReflectCmb => {
                        reflected_t = Some(s.t);
                        bottom = s.point();
                        Act::Reflect
                    }
                    Bottom::TurnOuterCore | Bottom::TurnInnerCore => {
                        entered_core = true;
                        core_entry_t.get_or_insert(s.t);
                        Act::Transmit
                    }
                }
            } else if is_icb && down {
                match bottom_kind {
                    Bottom::TurnInnerCore => {
                        entered_inner = true;
                        Act::Transmit
                    }
                    _ => return None,
                }
            } else {
                Act::Transmit
            };
            match act {
                Act::Reflect => reflect(&mut s),
                Act::Transmit => {
                    let v_out = self.velocity_in_layer(profile, wave, target, &s, r_if);
                    let q = 1.0 / (v_out * v_out) - pt * pt;
                    if q >= 0.0 {
                        let pr_out = q.sqrt() * pr.signum();
                        s.set_radial_tangential(pr_out, pt);
                        layer = target;
                    } else if is_cmb || is_icb {
                        // Total reflection off the core is a different phase (PcP-like, PKiKP).
                        return None;
                    } else {
                        // Total reflection off the top of a velocity increase, as in 1D.
                        reflect(&mut s);
                    }
                }
            }
        }
        if s.t >= MAX_TIME_S {
            return None;
        }
        let ok = match bottom_kind {
            Bottom::TurnMantle => !entered_core,
            Bottom::ReflectCmb => reflected_t.is_some(),
            Bottom::TurnOuterCore => entered_core && !entered_inner,
            Bottom::TurnInnerCore => entered_inner,
        };
        if !ok {
            return None;
        }
        let (shared_until_s, distinct_after_s) = match phase {
            RayPhase::P | RayPhase::S => (0.0, 0.0),
            RayPhase::PcP | RayPhase::ScS => (0.0, reflected_t.unwrap_or(0.0)),
            RayPhase::PKP | RayPhase::PKIKP | RayPhase::SKS => {
                let tc = core_entry_t.unwrap_or(0.0);
                (tc, tc)
            }
            RayPhase::PP | RayPhase::SS => {
                let ts = first_surface_t.unwrap_or(0.0);
                (ts, ts)
            }
        };
        Some(Ray {
            phase,
            takeoff_rad,
            p_s_rad,
            points: decimate(points),
            bottom: if upgoing { source } else { bottom },
            shared_until_s,
            distinct_after_s,
        })
    }

    /// A fan of bent rays, seeded with the take-off angles of a 1D fan (at most `MAX_SEEDS`
    /// of them, evenly spread, plus every ray at an edge of the 1D fan and a probe just beyond
    /// it), then refined like the 1D fans, up to `MAX_FAN_RAYS` rays.
    pub fn fan(&self, phase: RayPhase, depth_km: f64, seeds: &[Ray]) -> Vec<Ray> {
        let max_angle = if phase.has_upgoing() && depth_km > 0.0 {
            PI
        } else {
            std::f64::consts::FRAC_PI_2
        };
        let stride = seeds.len().div_ceil(MAX_SEEDS).max(1);
        let mut angles: Vec<f64> = seeds
            .iter()
            .step_by(stride)
            .map(|r| r.takeoff_rad)
            .collect();
        // The 1D fan's edges: its ends and both sides of any gap in it.
        for (i, r) in seeds.iter().enumerate() {
            let a = r.takeoff_rad;
            let gap_before = i == 0 || a - seeds[i - 1].takeoff_rad > 1.5 * EDGE_PROBE_RAD;
            let gap_after =
                i + 1 == seeds.len() || seeds[i + 1].takeoff_rad - a > 1.5 * EDGE_PROBE_RAD;
            if gap_before {
                angles.extend([a, (a - EDGE_PROBE_RAD).max(0.0)]);
            }
            if gap_after {
                angles.extend([a, (a + EDGE_PROBE_RAD).min(max_angle)]);
            }
        }
        angles.sort_by(f64::total_cmp);
        angles.dedup();
        let mut rays: Vec<(f64, Option<Ray>)> = angles
            .iter()
            .map(|&a| (a, self.trace(phase, depth_km, a)))
            .collect();
        loop {
            let mut next = Vec::with_capacity(rays.len() * 2);
            let mut inserted = false;
            for i in 0..rays.len() {
                if i > 0 && rays.len() - i + next.len() < MAX_FAN_RAYS {
                    let (a0, r0) = &rays[i - 1];
                    let (a1, r1) = &rays[i];
                    let gap = a1 - a0;
                    let split = match (r0, r1) {
                        (Some(x), Some(y)) => {
                            gap > MIN_REFINE_RAD
                                && (end_chord_km(x, y) > REFINE_KM
                                    || bottom_chord_km(x, y) > REFINE_KM)
                        }
                        (None, None) => false,
                        _ => gap > MIN_EDGE_RAD,
                    };
                    if split {
                        let mid = 0.5 * (a0 + a1);
                        next.push((mid, self.trace(phase, depth_km, mid)));
                        inserted = true;
                    }
                }
                next.push(rays[i].clone());
            }
            rays = next;
            if !inserted {
                break;
            }
        }
        rays.into_iter().filter_map(|(_, r)| r).collect()
    }
}

/// Distance (km) between the surface ends of two rays.
fn end_chord_km(a: &Ray, b: &Ray) -> f64 {
    chord_km(&a.end(), &b.end())
}

/// Distance (km) between the deepest points of two rays.
fn bottom_chord_km(a: &Ray, b: &Ray) -> f64 {
    chord_km(&a.bottom, &b.bottom)
}

fn chord_km(pa: &PathPoint, pb: &PathPoint) -> f64 {
    let (xa, ya) = (pa.r_km * pa.theta_rad.cos(), pa.r_km * pa.theta_rad.sin());
    let (xb, yb) = (pb.r_km * pb.theta_rad.cos(), pb.r_km * pb.theta_rad.sin());
    (xa - xb).hypot(ya - yb)
}

/// Bent fans of every slice phase, forward and back, seeded from 1D fans (`seeds`, as built by
/// `SliceFans::build`, forward fans first).
/// One at a time; the fans are independent, so callers that may use threads can build them
/// in parallel with [`bent_fan`] (the jobs are [`bent_fan_jobs`]).
pub fn bent_fans(
    tracer: &RayTracer,
    grid: &PlaneGrid,
    strength: f64,
    depth_km: f64,
    seeds: &[Fan],
) -> Vec<Fan> {
    bent_fan_jobs(seeds)
        .into_iter()
        .map(|(side, seed)| bent_fan(tracer, grid, strength, depth_km, side, seed))
        .collect()
}

/// The (side, 1D seed fan) pairs that make up a full set of bent fans, forward fans first.
pub fn bent_fan_jobs(seeds: &[Fan]) -> Vec<(Side, &Fan)> {
    [Side::Forward, Side::Back]
        .into_iter()
        .flat_map(|side| {
            seeds
                .iter()
                .filter(|f| f.side == Side::Forward)
                .map(move |f| (side, f))
        })
        .collect()
}

/// The bent fan of `seed`'s phase leaving towards `side`.
pub fn bent_fan(
    tracer: &RayTracer,
    grid: &PlaneGrid,
    strength: f64,
    depth_km: f64,
    side: Side,
    seed: &Fan,
) -> Fan {
    let bender = Bender::new(tracer, grid, strength, side);
    Fan {
        phase: seed.phase,
        side,
        rays: bender.fan(seed.phase, depth_km, &seed.rays),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mantle::path_delay;
    use crate::slice::named_boundaries;
    use crate::tables::{EarthModel, MantleModel};

    fn model() -> EarthModel {
        EarthModel::from_bytes(include_bytes!("../../../../assets/ak135_model.csv")).unwrap()
    }

    fn mantle(m: &EarthModel) -> Mantle3d {
        let mm = MantleModel::from_bytes(
            include_bytes!("../../../../assets/sp12rts/SP12RTS..ES.sph"),
            include_bytes!("../../../../assets/sp12rts/SP12RTS..EP.sph"),
        )
        .unwrap();
        Mantle3d::new(mm, &named_boundaries(m))
    }

    /// Take-off angles spread over a phase's 1D fan.
    fn sample_takeoffs(tracer: &RayTracer, phase: RayPhase, depth: f64, n: usize) -> Vec<f64> {
        let fan = crate::slice::build_fan(tracer, phase, depth);
        let stride = (fan.len() / n).max(1);
        fan.iter().step_by(stride).map(|r| r.takeoff_rad).collect()
    }

    #[test]
    fn without_perturbation_bent_rays_match_the_closed_form_tracer() {
        let m = model();
        let tracer = RayTracer::new(&m).unwrap();
        let grid = PlaneGrid::flat();
        let bender = Bender::new(&tracer, &grid, 0.0, Side::Forward);
        for depth in [29.0, 600.0] {
            for phase in RayPhase::ALL {
                let mut compared = 0;
                for a in sample_takeoffs(&tracer, phase, depth, 25) {
                    let exact = tracer.trace(phase, depth, a).unwrap();
                    // Skip knife-edge rays, where a 1e-5 rad change of take-off angle moves the
                    // landing point a lot (a ray turning just above a discontinuity or reflecting
                    // off it): there the two tracers may fairly disagree on which it does.
                    let near = |d: f64| {
                        tracer
                            .trace(phase, depth, a + d)
                            .map(|r| r.end().theta_rad.to_degrees())
                    };
                    let e = exact.end().theta_rad.to_degrees();
                    if [near(1e-5), near(-1e-5)]
                        .iter()
                        .any(|n| n.is_none_or(|n| (n - e).abs() > 0.05))
                    {
                        continue;
                    }
                    let Some(bent) = bender.trace(phase, depth, a) else {
                        continue;
                    };
                    compared += 1;
                    let (e, b) = (exact.end(), bent.end());
                    let dth = (b.theta_rad - e.theta_rad).to_degrees();
                    // Time at the same landing point: a ray landing a little further along
                    // the same front is late by p·dθ, which is not an error of the front.
                    let dt = (b.t_s - e.t_s - exact.p_s_rad * dth.to_radians()).abs();
                    assert!(
                        dt < 0.1 && dth.abs() < 0.05,
                        "{phase:?} {depth} km take-off {a:.4}: dt {dt:.4} s, dθ {dth:.4}°"
                    );
                    let (eb, bb) = (exact.bottom, bent.bottom);
                    assert!(
                        (eb.r_km - bb.r_km).abs() < 2.0,
                        "{phase:?} bottom {eb:?} vs {bb:?}"
                    );
                    for (x, y) in [
                        (exact.shared_until_s, bent.shared_until_s),
                        (exact.distinct_after_s, bent.distinct_after_s),
                    ] {
                        assert!((x - y).abs() < 0.1, "{phase:?}: {x} vs {y}");
                    }
                }
                assert!(
                    compared >= 15,
                    "{phase:?} {depth} km: only {compared} rays traced"
                );
            }
        }
    }

    #[test]
    fn at_real_strength_bent_times_match_the_map_delays() {
        // Bending changes the time only at second order: the bent ray's time, moved to the 1D
        // ray's landing point with the ray parameter, equals the 1D time plus the first-order
        // delay along the 1D path, to a fraction of a second.
        let m = model();
        let tracer = RayTracer::new(&m).unwrap();
        let mantle = mantle(&m);
        let (lat, lon, depth) = (38.297, 142.373, 29.0);
        for az in [0.0, 90.0, 225.0] {
            let grid = PlaneGrid::new(&mantle, lat, lon, az);
            let bender = Bender::new(&tracer, &grid, 1.0, Side::Forward);
            for (phase, wave) in [(RayPhase::P, Wave::P), (RayPhase::S, Wave::S)] {
                for dist in [30.0, 60.0, 90.0] {
                    let a = tracer.arrivals(phase, depth, dist)[0];
                    let ray = tracer.trace(phase, depth, a.takeoff_rad).unwrap();
                    let delay =
                        path_delay(&ray, lat, lon, az, |r, la, lo| mantle.dlnv(wave, r, la, lo));
                    let bent = bender.trace(phase, depth, a.takeoff_rad).unwrap();
                    let p = bent.p_s_rad;
                    let moved = bent.end().t_s - p * (bent.end().theta_rad - ray.end().theta_rad);
                    let want = ray.arrival_s() + delay;
                    assert!(
                        (moved - want).abs() < 0.3,
                        "{phase:?} {dist}° az {az}: bent {moved:.2} vs 1D+delay {want:.2} (delay {delay:.2})"
                    );
                }
            }
        }
    }

    #[test]
    fn exaggerated_fans_stay_bounded() {
        let m = model();
        let tracer = RayTracer::new(&m).unwrap();
        let mantle = mantle(&m);
        let grid = PlaneGrid::new(&mantle, 38.297, 142.373, 90.0);
        let seeds = crate::slice::SliceFans::build(&tracer, 29.0);
        let started = std::time::Instant::now();
        let real = bent_fans(&tracer, &grid, 1.0, 29.0, &seeds.fans);
        println!(
            "bent fans at 1x: {:?}, {} rays",
            started.elapsed(),
            real.iter().map(|f| f.rays.len()).sum::<usize>()
        );
        let fans = bent_fans(&tracer, &grid, 20.0, 29.0, &seeds.fans);
        assert_eq!(fans.len(), 2 * RayPhase::ALL.len());
        for f in &fans {
            assert!(
                f.rays.len() <= MAX_FAN_RAYS,
                "{:?}: {}",
                f.phase,
                f.rays.len()
            );
        }
        assert!(fans.iter().any(|f| !f.rays.is_empty()));
    }

    fn span(rays: &[Ray]) -> (f64, f64) {
        rays.iter()
            .map(|r| crate::raytrace::fold_rad(r.end().theta_rad).to_degrees())
            .fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)))
    }

    #[test]
    fn bent_fans_without_perturbation_cover_the_1d_fans() {
        // The bent fans are built differently from the 1D ones (fewer seeds, probes beyond the
        // 1D edges): with no perturbation each must still reach the surface over at least the
        // same range of distances, so no front ends early because of how the fan was sampled.
        let m = model();
        let tracer = RayTracer::new(&m).unwrap();
        let depth = 29.0;
        let seeds = crate::slice::SliceFans::build(&tracer, depth);
        let bent = bent_fans(&tracer, &PlaneGrid::flat(), 0.0, depth, &seeds.fans);
        for (b, s) in bent
            .iter()
            .zip(&seeds.fans)
            .filter(|(b, _)| b.side == Side::Forward)
        {
            assert_eq!(b.phase, s.phase);
            let ((bl, bh), (sl, sh)) = (span(&b.rays), span(&s.rays));
            assert!(
                bl < sl + 0.1 && bh > sh - 0.1,
                "{:?}: bent {bl:.2}–{bh:.2}°, 1D {sl:.2}–{sh:.2}°",
                b.phase
            );
        }
    }

    #[test]
    fn bent_fans_at_real_strength_cover_the_1d_fans() {
        // At real strength each phase's bent fan reaches the surface over nearly the same range
        // as its 1D fan, and the bent P and S fronts run as far along the surface by each time.
        // Exception: the far end of PKP (towards 180°) comes from rays that only graze the core;
        // where the rock above their exit point is faster than above their entry, ray theory
        // reflects them back into the core, and that end of the branch is lost (see
        // docs/approximations.md).
        let m = model();
        let tracer = RayTracer::new(&m).unwrap();
        let mantle = mantle(&m);
        let (lat, lon, depth) = (38.297, 142.373, 29.0);
        let seeds = crate::slice::SliceFans::build(&tracer, depth);
        for az in [90.0, 200.0] {
            let grid = PlaneGrid::new(&mantle, lat, lon, az);
            let bent = bent_fans(&tracer, &grid, 1.0, depth, &seeds.fans);
            for (b, s) in bent
                .iter()
                .zip(&seeds.fans)
                .filter(|(b, _)| b.side == Side::Forward)
            {
                assert_eq!(b.phase, s.phase);
                let ((bl, bh), (sl, sh)) = (span(&b.rays), span(&s.rays));
                let far_ok = b.phase == RayPhase::PKP || (bh - sh).abs() < 1.0;
                assert!(
                    (bl - sl).abs() < 1.0 && far_ok,
                    "{:?} az {az}: bent {bl:.2}–{bh:.2}°, 1D {sl:.2}–{sh:.2}°",
                    b.phase
                );
                if matches!(b.phase, RayPhase::P | RayPhase::S) {
                    for t in [120.0, 300.0, 600.0] {
                        let reach = |rays: &[Ray]| {
                            rays.iter()
                                .filter(|r| r.arrival_s() <= t)
                                .map(|r| crate::raytrace::fold_rad(r.end().theta_rad).to_degrees())
                                .fold(0.0, f64::max)
                        };
                        let (rb, rs) = (reach(&b.rays), reach(&s.rays));
                        assert!(
                            (rb - rs).abs() < 0.6,
                            "{:?} az {az} t {t}: {rb:.2} vs {rs:.2}",
                            b.phase
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn back_fan_sees_the_other_half() {
        // A perturbation only on one side of the circle slows the rays going that way.
        let m = model();
        let tracer = RayTracer::new(&m).unwrap();
        let mut grid = PlaneGrid {
            n_theta: 720,
            n_r: 3,
            r_bottom_km: 3480.0,
            dr_km: (6336.0 - 3480.0) / 2.0,
            values: vec![[0.0, 0.0]; 3 * 720],
        };
        // Slow (−2%) between 10° and 80° along the azimuth.
        for i in 0..3 {
            for j in 20..160 {
                grid.values[i * 720 + j] = [-0.02, -0.02];
            }
        }
        let a = tracer.arrivals(RayPhase::P, 29.0, 60.0)[0].takeoff_rad;
        let fwd = Bender::new(&tracer, &grid, 1.0, Side::Forward)
            .trace(RayPhase::P, 29.0, a)
            .unwrap();
        let back = Bender::new(&tracer, &grid, 1.0, Side::Back)
            .trace(RayPhase::P, 29.0, a)
            .unwrap();
        let flat = tracer.trace(RayPhase::P, 29.0, a).unwrap();
        assert!(
            fwd.arrival_s() > flat.arrival_s() + 2.0,
            "{} vs {}",
            fwd.arrival_s(),
            flat.arrival_s()
        );
        assert!((back.arrival_s() - flat.arrival_s()).abs() < 0.05);
    }

    #[test]
    fn deterministic() {
        let m = model();
        let tracer = RayTracer::new(&m).unwrap();
        let mantle = mantle(&m);
        let grid = PlaneGrid::new(&mantle, 10.0, 20.0, 45.0);
        let b = Bender::new(&tracer, &grid, 3.0, Side::Forward);
        assert_eq!(
            b.trace(RayPhase::S, 100.0, 0.5),
            b.trace(RayPhase::S, 100.0, 0.5)
        );
    }
}
