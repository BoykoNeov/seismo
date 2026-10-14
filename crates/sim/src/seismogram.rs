//! Synthetic seismograms at virtual stations: which waves arrive when, how strongly, and a
//! display-plausible trace built from them. Not waveform-accurate (see
//! `docs/approximations.md`).
//!
//! Arrival times of body waves come from the travel-time table (plus the 3D-mantle delay for P
//! and S, the same one the map uses). Their amplitudes come from the ray tracer's ray tubes:
//! for a source radiating evenly, the energy leaving between take-off angles i and i + di
//! spreads over the surface band between Δ and Δ + dΔ, so on a sphere
//!
//! ```text
//! A ∝ √( sin i / ( |dΔ/di| · sin Δ · cos i₀ ) )
//! ```
//!
//! with i₀ the angle of incidence at the station. dΔ/di is taken between neighbouring traced
//! rays (one difference along the fan), never by differencing the table twice. Surface waves
//! are packets at a constant group velocity.

use std::f64::consts::{FRAC_PI_2, PI};

use crate::event::Event;
use crate::geo::{EARTH_RADIUS_KM, azimuth_deg, great_circle_deg};
use crate::mantle::{DELAY_PHASES, DelayGrid};
use crate::raytrace::{Landing, RayPhase, RayTracer, Wave};
use crate::station::Station;
use crate::tables::TravelTimeTable;

/// Group velocities (km/s) of the surface-wave packets: constant, so not dispersive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceWaves {
    pub rayleigh_km_s: f64,
    pub love_km_s: f64,
}

impl Default for SurfaceWaves {
    fn default() -> Self {
        Self {
            rayleigh_km_s: 3.5,
            love_km_s: 4.4,
        }
    }
}

/// Which way of shaking a seismogram records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    /// Up and down: P waves, the vertical part of SV, Rayleigh waves.
    Vertical,
    /// Side to side, across the path from the source: SH waves and Love waves.
    Transverse,
}

/// The kind of motion an arrival carries, which decides what each component sees of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Motion {
    /// Arrives as a P wave.
    Compressional,
    /// Arrives as an S wave with both SV and SH parts (S, ScS, SS, Sdiff).
    Shear,
    /// SKS: converted to P in the liquid core, so pure SV; no SH at all.
    CoreShear,
    Rayleigh,
    Love,
}

impl Motion {
    /// Relative amplitude of this motion on `component`. Display weights, not a polarisation
    /// calculation.
    pub fn weight(self, component: Component) -> f64 {
        match (component, self) {
            (Component::Vertical, Motion::Compressional | Motion::Rayleigh) => 1.0,
            (Component::Vertical, Motion::CoreShear) => 0.6,
            (Component::Vertical, Motion::Shear) => 0.35,
            (Component::Vertical, Motion::Love) => 0.0,
            (Component::Transverse, Motion::Shear | Motion::Love) => 1.0,
            (Component::Transverse, _) => 0.0,
        }
    }
}

/// One wave arriving at a station.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StationArrival {
    /// Table phase name ("P", "PKPab", "Sdiff"), or "R1", "R2", "L1", "L2" for the surface
    /// waves going the short and the long way round.
    pub phase: &'static str,
    pub motion: Motion,
    /// Seconds since the event's origin (callers may shift it to another clock).
    pub time_s: f64,
    /// Relative amplitude, comparable between arrivals and between events.
    pub amplitude: f64,
    /// Dominant period (s) of the wavelet.
    pub period_s: f64,
}

/// Body-wave phases on the traces: the table's name, the traced phase giving its ray-tube
/// amplitude (`None` for the diffracted waves the tracer does not follow), the motion, and a
/// fixed factor standing in for the energy lost at the boundaries the ray reflects off or
/// passes through (the tracer has no reflection or transmission coefficients).
const BODY_PHASES: [(&str, Option<RayPhase>, Motion, f64); 12] = [
    ("P", Some(RayPhase::P), Motion::Compressional, 1.0),
    ("S", Some(RayPhase::S), Motion::Shear, 1.0),
    ("PcP", Some(RayPhase::PcP), Motion::Compressional, 0.2),
    ("ScS", Some(RayPhase::ScS), Motion::Shear, 0.5),
    ("PKPab", Some(RayPhase::PKP), Motion::Compressional, 0.4),
    ("PKPbc", Some(RayPhase::PKP), Motion::Compressional, 0.4),
    ("PKPdf", Some(RayPhase::PKIKP), Motion::Compressional, 0.3),
    ("PP", Some(RayPhase::PP), Motion::Compressional, 0.6),
    ("SS", Some(RayPhase::SS), Motion::Shear, 0.6),
    ("SKS", Some(RayPhase::SKS), Motion::CoreShear, 0.4),
    ("Pdiff", None, Motion::Compressional, 1.0),
    ("Sdiff", None, Motion::Shear, 1.0),
];

/// Wavelet periods (s). Real body waves are shorter; these keep them visible at trace scale.
const P_PERIOD_S: f64 = 1.0;
const S_PERIOD_S: f64 = 2.0;
const SURFACE_PERIOD_S: f64 = 20.0;
/// Width (s, one standard deviation) of a surface-wave packet's envelope.
const SURFACE_SIGMA_S: f64 = 30.0;
/// e-folding time (s) of the scattered coda after a body-wave arrival.
const P_CODA_S: f64 = 20.0;
const S_CODA_S: f64 = 40.0;
/// Coda amplitude relative to its arrival.
const CODA_GAIN: f64 = 0.4;
/// Background noise, relative to the trace's largest arrival.
const NOISE_FRACTION: f64 = 0.003;

/// Surface-wave amplitude relative to body waves at the same weight; chosen so that for a
/// shallow earthquake at 60° the Rayleigh packet is about ten times direct P on the vertical.
const SURFACE_GAIN: f64 = 4.0;
/// Surface-wave excitation falls off with source depth over this scale (km).
const SURFACE_DEPTH_KM: f64 = 60.0;
/// Stand-in for surface-wave attenuation: amplitude falls by e over this path length (km).
const SURFACE_DECAY_KM: f64 = 15_000.0;
/// Diffracted waves fade by e over this many degrees past the shadow edge.
const PDIFF_DECAY_DEG: f64 = 8.0;
const SDIFF_DECAY_DEG: f64 = 5.0;

/// Ray-tube amplitude caps: at a caustic |dΔ/di| → 0, at the epicentre and antipode sin Δ → 0,
/// at grazing incidence cos i₀ → 0, and the formula diverges. Ray theory breaks down there.
const MIN_SLOPE: f64 = 0.1;
const MIN_SIN_DELTA: f64 = 0.017_452_4; // sin 1°
const MIN_COS_INCIDENCE: f64 = 0.1;

/// Sampling of a fan of landings: take-off steps of 0.25°, halved where neighbouring rays land
/// more than `REFINE_DEG` apart, at most `MAX_LEVEL` times; fan edges bisected `EDGE_LEVEL`
/// times.
const BASE_STEP_RAD: f64 = PI / 720.0;
const REFINE_DEG: f64 = 0.2;
const MAX_LEVEL: u32 = 8;
const EDGE_LEVEL: u32 = 12;
/// Neighbouring rays landing farther apart than this (deg) bound a gap, not a branch.
const MAX_BRACKET_DEG: f64 = 2.0;

/// One branch of a phase crossing a distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TubeArrival {
    pub time_s: f64,
    pub amplitude: f64,
}

/// Where every traced phase lands, for one source depth: the ray tubes the amplitudes come
/// from. Build once per depth (tens of milliseconds) and reuse.
#[derive(Debug, Clone, PartialEq)]
pub struct Spreading {
    pub depth_km: f64,
    /// Per phase in `RayPhase::ALL` order: runs of neighbouring rays (a missing ray between
    /// two valid ones splits the fan), each ordered by take-off angle.
    fans: Vec<Vec<Vec<Landing>>>,
    eta_p: f64,
    eta_s: f64,
}

impl Spreading {
    pub fn build(tracer: &RayTracer, depth_km: f64) -> Self {
        Self {
            depth_km,
            fans: RayPhase::ALL
                .iter()
                .map(|&phase| landing_fan(tracer, phase, depth_km))
                .collect(),
            eta_p: tracer.surface_eta(Wave::P),
            eta_s: tracer.surface_eta(Wave::S),
        }
    }

    fn fan(&self, phase: RayPhase) -> &[Vec<Landing>] {
        let i = RayPhase::ALL
            .iter()
            .position(|&p| p == phase)
            .expect("every phase is in ALL");
        &self.fans[i]
    }

    /// Neighbouring landings of `phase` that bound one stretch of a branch: no missing ray
    /// between them and no gap wider than `MAX_BRACKET_DEG`.
    fn pairs(&self, phase: RayPhase) -> impl Iterator<Item = (Landing, Landing)> + '_ {
        self.fan(phase)
            .iter()
            .flat_map(|run| run.windows(2))
            .map(|w| (w[0], w[1]))
            .filter(|(a, b)| (b.dist_deg - a.dist_deg).abs() <= MAX_BRACKET_DEG)
    }

    /// Ray-tube amplitude at `dist_deg` of a ray of `phase` with take-off `takeoff_rad`, ray
    /// parameter `p` (s/rad) and distance slope `slope` = dΔ/di (rad/rad).
    fn tube(&self, phase: RayPhase, takeoff_rad: f64, p: f64, slope: f64, dist_deg: f64) -> f64 {
        let eta0 = match phase.mantle_wave() {
            Wave::P => self.eta_p,
            Wave::S => self.eta_s,
        };
        let sin_i0 = (p / eta0).min(1.0);
        let cos_i0 = (1.0 - sin_i0 * sin_i0).sqrt().max(MIN_COS_INCIDENCE);
        let sin_d = dist_deg.to_radians().sin().max(MIN_SIN_DELTA);
        (takeoff_rad.sin().abs() / (slope.abs().max(MIN_SLOPE) * sin_d * cos_i0)).sqrt()
    }

    /// Every branch of `phase` reaching `dist_deg`, earliest first.
    pub fn arrivals(&self, phase: RayPhase, dist_deg: f64) -> Vec<TubeArrival> {
        let mut out: Vec<TubeArrival> = self
            .pairs(phase)
            .filter_map(|(a, b)| {
                let crosses = (a.dist_deg <= dist_deg && dist_deg < b.dist_deg)
                    || (b.dist_deg <= dist_deg && dist_deg < a.dist_deg);
                if !crosses {
                    return None;
                }
                let f = (dist_deg - a.dist_deg) / (b.dist_deg - a.dist_deg);
                Some(self.between(phase, a, b, f, dist_deg))
            })
            .collect();
        out.sort_by(|x, y| x.time_s.total_cmp(&y.time_s));
        out
    }

    /// The arrival a fraction `f` of the way from landing `a` to its neighbour `b`.
    fn between(
        &self,
        phase: RayPhase,
        a: Landing,
        b: Landing,
        f: f64,
        dist_deg: f64,
    ) -> TubeArrival {
        let lerp = |x: f64, y: f64| x + (y - x) * f;
        let slope = (b.dist_deg - a.dist_deg).to_radians() / (b.takeoff_rad - a.takeoff_rad);
        TubeArrival {
            time_s: lerp(a.time_s, b.time_s),
            amplitude: self.tube(
                phase,
                lerp(a.takeoff_rad, b.takeoff_rad),
                lerp(a.p_s_rad, b.p_s_rad),
                slope,
                dist_deg,
            ),
        }
    }

    /// Amplitude of the branch of `phase` at `dist_deg` arriving closest to `time_s`. When no
    /// branch crosses that distance (the table and the fan disagree slightly at a fan's edge),
    /// the nearest landing pair within `MAX_BRACKET_DEG` stands in.
    fn amplitude_near(&self, phase: RayPhase, dist_deg: f64, time_s: f64) -> Option<f64> {
        if let Some(best) = self.arrivals(phase, dist_deg).into_iter().min_by(|x, y| {
            (x.time_s - time_s)
                .abs()
                .total_cmp(&(y.time_s - time_s).abs())
        }) {
            return Some(best.amplitude);
        }
        self.pairs(phase)
            .map(|(a, b)| {
                let (ga, gb) = ((a.dist_deg - dist_deg).abs(), (b.dist_deg - dist_deg).abs());
                (ga.min(gb), a, b, if ga < gb { 0.0 } else { 1.0 })
            })
            .filter(|(gap, ..)| *gap <= MAX_BRACKET_DEG)
            .min_by(|x, y| x.0.total_cmp(&y.0))
            .map(|(_, a, b, f)| self.between(phase, a, b, f, dist_deg).amplitude)
    }

    /// Farthest distance (deg) the downgoing rays of a direct phase reach: the shadow edge, past
    /// which only the diffracted wave arrives.
    fn shadow_edge_deg(&self, phase: RayPhase) -> Option<f64> {
        self.fan(phase)
            .iter()
            .flatten()
            .filter(|l| l.takeoff_rad <= FRAC_PI_2)
            .map(|l| l.dist_deg)
            .reduce(f64::max)
    }

    /// Amplitude of a diffracted wave (Pdiff from `RayPhase::P`, Sdiff from `RayPhase::S`) at
    /// `dist_deg`: the direct wave's amplitude just inside the shadow edge, fading
    /// exponentially past it. A stand-in; the tracer does not follow diffraction.
    fn diffracted(&self, phase: RayPhase, dist_deg: f64) -> Option<f64> {
        let edge = self.shadow_edge_deg(phase)?;
        if dist_deg <= edge {
            return None;
        }
        let decay = match phase.mantle_wave() {
            Wave::P => PDIFF_DECAY_DEG,
            Wave::S => SDIFF_DECAY_DEG,
        };
        let inside = edge - 0.5;
        let a = self.amplitude_near(phase, inside, f64::INFINITY)?;
        Some(a * (-(dist_deg - edge) / decay).exp())
    }
}

/// The landings of a fan of `phase` rays, ordered by take-off angle, refined where neighbours
/// land far apart and towards the fan's edges, as runs of consecutive valid rays.
fn landing_fan(tracer: &RayTracer, phase: RayPhase, depth_km: f64) -> Vec<Vec<Landing>> {
    let max_angle = if phase.has_upgoing() && depth_km > 0.0 {
        PI
    } else {
        FRAC_PI_2
    };
    let n = (max_angle / BASE_STEP_RAD).round() as usize;
    let mut samples: Vec<(f64, Option<Landing>)> = (0..=n)
        .map(|k| {
            let a = max_angle * k as f64 / n as f64;
            (a, tracer.landing(phase, depth_km, a))
        })
        .collect();
    let min_gap = |level: u32| BASE_STEP_RAD / f64::from(1u32 << level) * 1.0001;
    loop {
        let mut next = Vec::with_capacity(samples.len() * 2);
        let mut inserted = false;
        for i in 0..samples.len() {
            if i > 0 {
                let (a0, l0) = samples[i - 1];
                let (a1, l1) = samples[i];
                let split = match (l0, l1) {
                    (Some(x), Some(y)) => {
                        a1 - a0 > min_gap(MAX_LEVEL) && (x.dist_deg - y.dist_deg).abs() > REFINE_DEG
                    }
                    (None, None) => false,
                    _ => a1 - a0 > min_gap(EDGE_LEVEL),
                };
                if split {
                    let mid = 0.5 * (a0 + a1);
                    next.push((mid, tracer.landing(phase, depth_km, mid)));
                    inserted = true;
                }
            }
            next.push(samples[i]);
        }
        samples = next;
        if !inserted {
            break;
        }
    }
    let mut runs: Vec<Vec<Landing>> = Vec::new();
    let mut run = Vec::new();
    for (_, l) in samples {
        match l {
            Some(l) => run.push(l),
            None if !run.is_empty() => runs.push(std::mem::take(&mut run)),
            None => {}
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}

/// Every wave from `event` reaching `station`, in time order. Times are seconds since the
/// event's origin. `spreading` must be built for the event's depth. `delays` are the event's
/// 3D-mantle delays (P and S only), scaled by `strength`; `None` gives the 1D times.
#[allow(clippy::too_many_arguments)]
pub fn station_arrivals(
    table: &TravelTimeTable,
    spreading: &Spreading,
    event: &Event,
    station: &Station,
    delays: Option<&DelayGrid>,
    strength: f64,
    surface: SurfaceWaves,
) -> Vec<StationArrival> {
    let dist = great_circle_deg(event.lat, event.lon, station.lat, station.lon);
    let az = azimuth_deg(event.lat, event.lon, station.lat, station.lon);
    let depth = event.depth_km;
    let profile = event.kind.profile();
    let size = 10f64.powf(event.magnitude - 5.0);
    let mut out = Vec::new();

    for (name, traced, motion, boundaries) in BODY_PHASES {
        let Some(mut time_s) = table.arrival(name, depth, dist) else {
            continue;
        };
        let geometric = match (traced, name) {
            (Some(phase), _) => spreading.amplitude_near(phase, dist, time_s),
            (None, "Pdiff") => spreading.diffracted(RayPhase::P, dist),
            (None, _) => spreading.diffracted(RayPhase::S, dist),
        };
        let Some(geometric) = geometric else {
            continue;
        };
        if let Some(grid) = delays
            && let Some(layer) = traced.and_then(|t| {
                DELAY_PHASES
                    .iter()
                    .position(|&d| d == t && matches!(name, "P" | "S"))
            })
        {
            time_s += strength * grid.sample(layer, dist, az);
        }
        let (weight, period_s) = match motion {
            Motion::Compressional => (profile.p, P_PERIOD_S),
            _ => (profile.s, S_PERIOD_S),
        };
        out.push(StationArrival {
            phase: name,
            motion,
            time_s,
            amplitude: size * weight * boundaries * geometric,
            period_s,
        });
    }

    let surface_amp = |path_deg: f64| {
        let km = path_deg.to_radians() * EARTH_RADIUS_KM;
        size * profile.surface
            * SURFACE_GAIN
            * (-depth / SURFACE_DEPTH_KM).exp()
            * (-km / SURFACE_DECAY_KM).exp()
            / dist.to_radians().sin().max(MIN_SIN_DELTA).sqrt()
    };
    for (name, motion, speed, path_deg) in [
        ("R1", Motion::Rayleigh, surface.rayleigh_km_s, dist),
        ("R2", Motion::Rayleigh, surface.rayleigh_km_s, 360.0 - dist),
        ("L1", Motion::Love, surface.love_km_s, dist),
        ("L2", Motion::Love, surface.love_km_s, 360.0 - dist),
    ] {
        out.push(StationArrival {
            phase: name,
            motion,
            time_s: path_deg.to_radians() * EARTH_RADIUS_KM / speed,
            amplitude: surface_amp(path_deg),
            period_s: SURFACE_PERIOD_S,
        });
    }
    out.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
    out
}

/// A seed for [`synthesize`] from names (event id, station label, ...): FNV-1a, so the same
/// names always give the same trace.
pub fn seed_from(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.bytes().chain([0xff]) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// splitmix64: a tiny deterministic generator for the coda and noise phases.
fn splitmix(state: &mut u64) -> f64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// A few sinusoids with seeded frequencies (around `f0`) and phases: smooth, band-limited
/// pseudo-noise of unit RMS that can be evaluated at any time.
#[derive(Debug, Clone, Copy)]
struct Wobble {
    f: [f64; 4],
    phi: [f64; 4],
}

impl Wobble {
    fn new(f0: f64, state: &mut u64) -> Self {
        let mut w = Wobble {
            f: [0.0; 4],
            phi: [0.0; 4],
        };
        for k in 0..4 {
            w.f[k] = f0 * (0.6 + 0.8 * splitmix(state));
            w.phi[k] = 2.0 * PI * splitmix(state);
        }
        w
    }

    fn at(&self, t: f64) -> f64 {
        let s: f64 = (0..4)
            .map(|k| (2.0 * PI * self.f[k] * t + self.phi[k]).sin())
            .sum();
        s * (2.0f64 / 4.0).sqrt()
    }
}

/// Sample spacing (s) of synthetic traces: eight samples per shortest wavelet period.
pub const SAMPLE_DT_S: f64 = P_PERIOD_S / 8.0;

/// A sampled synthetic trace.
#[derive(Debug, Clone, PartialEq)]
pub struct Seismogram {
    /// Time (s) of the first sample, on the arrivals' clock.
    pub start_s: f64,
    pub dt_s: f64,
    pub samples: Vec<f32>,
    /// Largest |sample|; 0 for a silent trace.
    pub peak: f32,
}

/// The `component` seismogram of `arrivals` from `start_s` to `end_s`: a Ricker wavelet and a
/// decaying scattered coda per body wave, a Gaussian packet per surface wave, and a faint
/// background noise. `seed` fixes the coda and noise; the same inputs give the same trace.
pub fn synthesize(
    arrivals: &[StationArrival],
    component: Component,
    start_s: f64,
    end_s: f64,
    seed: u64,
) -> Seismogram {
    let dt = SAMPLE_DT_S;
    let n = (((end_s - start_s) / dt).ceil().max(0.0) as usize) + 1;
    let mut x = vec![0.0f64; n];
    let mut state = seed;
    let index = |t: f64| ((t - start_s) / dt).clamp(0.0, n as f64 - 1.0);

    for a in arrivals {
        let w = a.amplitude * a.motion.weight(component);
        // Draw the generator's numbers for every arrival, used or not, so switching component
        // keeps each arrival's coda the same.
        let wobble = Wobble::new(1.0 / a.period_s, &mut state);
        if w == 0.0 {
            continue;
        }
        let f = 1.0 / a.period_s;
        let surface = matches!(a.motion, Motion::Rayleigh | Motion::Love);
        let (before, after) = if surface {
            (4.0 * SURFACE_SIGMA_S, 4.0 * SURFACE_SIGMA_S)
        } else {
            let coda = if a.motion == Motion::Compressional {
                P_CODA_S
            } else {
                S_CODA_S
            };
            (1.5 * a.period_s, 6.0 * coda)
        };
        let (k0, k1) = (
            index(a.time_s - before) as usize,
            index(a.time_s + after) as usize,
        );
        if a.time_s + after < start_s || a.time_s - before > end_s {
            continue;
        }
        for (k, out) in x.iter_mut().enumerate().take(k1 + 1).skip(k0) {
            let tau = start_s + k as f64 * dt - a.time_s;
            *out += w * if surface {
                (-0.5 * (tau / SURFACE_SIGMA_S).powi(2)).exp()
                    * (2.0 * PI * f * tau + wobble.phi[0]).cos()
            } else {
                let coda_s = if a.motion == Motion::Compressional {
                    P_CODA_S
                } else {
                    S_CODA_S
                };
                let arg = (PI * f * tau).powi(2);
                let ricker = (1.0 - 2.0 * arg) * (-arg).exp();
                let coda = if tau > 0.0 {
                    CODA_GAIN
                        * (1.0 - (-tau / a.period_s).exp())
                        * (-tau / coda_s).exp()
                        * wobble.at(tau)
                } else {
                    0.0
                };
                ricker + coda
            };
        }
    }

    let signal_peak = x.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if signal_peak > 0.0 {
        let noise = Wobble::new(1.0 / 6.0, &mut state);
        for (k, v) in x.iter_mut().enumerate() {
            *v += NOISE_FRACTION * signal_peak * noise.at(start_s + k as f64 * dt);
        }
    }
    let samples: Vec<f32> = x.iter().map(|&v| v as f32).collect();
    let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    Seismogram {
        start_s,
        dt_s: dt,
        samples,
        peak,
    }
}

impl Seismogram {
    /// Time (s) of sample `k`.
    pub fn time(&self, k: usize) -> f64 {
        self.start_s + k as f64 * self.dt_s
    }

    /// The trace squeezed into `columns` equal columns from `from_s` to `to_s`: per column the
    /// smallest and largest sample, divided by `scale` (the trace's own `peak` to fill its row,
    /// or a peak shared between traces to compare them), so no wiggle narrower than a column
    /// is lost. Only columns starting before `until_s` are returned, and samples after `until_s`
    /// are left out (the pen has not drawn them yet). Each column includes the samples at its
    /// edges, so neighbouring columns join up.
    pub fn columns(
        &self,
        from_s: f64,
        to_s: f64,
        columns: usize,
        until_s: f64,
        scale: f32,
    ) -> Vec<[f32; 2]> {
        if columns == 0 || to_s <= from_s || self.samples.is_empty() {
            return Vec::new();
        }
        let scale = if scale > 0.0 { 1.0 / scale } else { 0.0 };
        let last = self.samples.len() - 1;
        let width = (to_s - from_s) / columns as f64;
        let mut out = Vec::with_capacity(columns);
        for c in 0..columns {
            let c0 = from_s + c as f64 * width;
            if c0 >= until_s {
                break;
            }
            let c1 = (c0 + width).min(until_s);
            let k0 = ((c0 - self.start_s) / self.dt_s).floor();
            let k1 = ((c1 - self.start_s) / self.dt_s).ceil();
            if k1 < 0.0 || k0 > last as f64 {
                out.push([0.0, 0.0]);
                continue;
            }
            let (k0, k1) = (k0.max(0.0) as usize, (k1 as usize).min(last));
            let (lo, hi) = self.samples[k0..=k1]
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
                    (lo.min(v), hi.max(v))
                });
            out.push([lo * scale, hi * scale]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventKind;
    use crate::tables::EarthModel;
    use chrono::{TimeZone, Utc};

    fn tracer() -> RayTracer {
        let model =
            EarthModel::from_bytes(include_bytes!("../../../assets/ak135_model.csv")).unwrap();
        RayTracer::new(&model).unwrap()
    }

    fn table() -> TravelTimeTable {
        TravelTimeTable::from_bytes(include_bytes!("../../../assets/ak135.tt")).unwrap()
    }

    fn event(depth_km: f64) -> Event {
        let origin = Utc.with_ymd_and_hms(2011, 3, 11, 5, 46, 24).unwrap();
        Event::new("e", EventKind::Earthquake, origin, 0.0, 0.0, depth_km, 7.0)
    }

    /// A station `dist_deg` east of (0, 0) along the equator.
    fn station_at(dist_deg: f64) -> Station {
        Station::user("X", 0.0, dist_deg)
    }

    #[test]
    fn uniform_mantle_spreading_is_one_over_path_length() {
        // In a uniform mantle rays are straight chords: a surface source's ray to distance Δ
        // has length L = 2R·sin(Δ/2), and the ray-tube amplitude falls as 1/L.
        let csv = "depth_km,vp_km_s,vs_km_s,density_g_cm3\n\
                   0,8,4.5,3.3\n2891.5,8,4.5,3.3\n2891.5,8,0,10\n5153.5,8,0,10\n\
                   5153.5,11,3.5,12\n6371,11,3.5,12\n";
        let rt = RayTracer::new(&EarthModel::from_csv_str(csv).unwrap()).unwrap();
        let sp = Spreading::build(&rt, 0.0);
        let al = |d: f64| {
            let a = sp.arrivals(RayPhase::P, d);
            assert_eq!(a.len(), 1, "{d}°: {a:?}");
            a[0].amplitude * 2.0 * (d.to_radians() / 2.0).sin()
        };
        let reference = al(30.0);
        for d in [15.0, 45.0, 60.0, 80.0, 100.0] {
            let ratio = al(d) / reference;
            assert!(
                (ratio - 1.0).abs() < 0.01,
                "A·L at {d}° is {ratio:.4} of 30°"
            );
        }
    }

    #[test]
    fn traced_branch_times_match_the_table() {
        let (rt, table) = (tracer(), table());
        let sp = Spreading::build(&rt, 29.0);
        for (phase, name, d) in [
            (RayPhase::P, "P", 40.0),
            (RayPhase::P, "P", 80.0),
            (RayPhase::S, "S", 60.0),
            (RayPhase::PKIKP, "PKPdf", 160.0),
            (RayPhase::PP, "PP", 110.0),
        ] {
            let want = table.arrival(name, 29.0, d).unwrap();
            let got = sp.arrivals(phase, d);
            assert!(
                got.iter().any(|a| (a.time_s - want).abs() < 1.0),
                "{name} at {d}°: table {want:.1} s, traced {got:?}"
            );
        }
    }

    #[test]
    fn direct_p_weakens_with_distance_away_from_caustics() {
        let sp = Spreading::build(&tracer(), 29.0);
        let a = |d: f64| sp.arrivals(RayPhase::P, d)[0].amplitude;
        // Past the upper-mantle triplications, out to the core shadow.
        assert!(
            a(40.0) > a(60.0) && a(60.0) > a(85.0),
            "{} {} {}",
            a(40.0),
            a(60.0),
            a(85.0)
        );
    }

    #[test]
    fn arrivals_follow_the_table_and_skip_missing_phases() {
        let (rt, table) = (tracer(), table());
        let sp = Spreading::build(&rt, 29.0);
        let e = event(29.0);
        for d in [30.0, 60.0, 90.0, 120.0, 150.0] {
            let got = station_arrivals(
                &table,
                &sp,
                &e,
                &station_at(d),
                None,
                1.0,
                SurfaceWaves::default(),
            );
            for (name, ..) in BODY_PHASES {
                let want = table.arrival(name, 29.0, d);
                let have = got.iter().find(|a| a.phase == name);
                match (want, have) {
                    (Some(t), Some(a)) => assert!((a.time_s - t).abs() < 1e-9, "{name} {d}°"),
                    // Diffracted waves only past the fan's edge; a traced phase may lack a ray.
                    (Some(_), None) => {}
                    (None, Some(_)) => panic!("{name} at {d}° has no table time but arrived"),
                    (None, None) => {}
                }
            }
            assert!(
                got.iter()
                    .all(|a| a.amplitude.is_finite() && a.amplitude > 0.0)
            );
            assert!(got.windows(2).all(|w| w[0].time_s <= w[1].time_s));
            // Direct S exactly where the table has it; no hardcoded shadow distance.
            assert_eq!(
                got.iter().any(|a| a.phase == "S"),
                table.arrival("S", 29.0, d).is_some(),
                "S at {d}°"
            );
        }
    }

    #[test]
    fn shadow_zone_has_diffracted_p_but_no_direct_p() {
        let (rt, table) = (tracer(), table());
        let sp = Spreading::build(&rt, 29.0);
        let edge = sp.shadow_edge_deg(RayPhase::P).unwrap();
        let d = edge + 10.0;
        let got = station_arrivals(
            &table,
            &sp,
            &event(29.0),
            &station_at(d),
            None,
            1.0,
            SurfaceWaves::default(),
        );
        if table.arrival("P", 29.0, d).is_none() {
            assert!(!got.iter().any(|a| a.phase == "P"));
        }
        let pdiff = got
            .iter()
            .find(|a| a.phase == "Pdiff")
            .expect("Pdiff past the edge");
        let p_inside = station_arrivals(
            &table,
            &sp,
            &event(29.0),
            &station_at(edge - 5.0),
            None,
            1.0,
            SurfaceWaves::default(),
        )
        .into_iter()
        .find(|a| a.phase == "P")
        .unwrap();
        assert!(pdiff.amplitude < p_inside.amplitude);
    }

    #[test]
    fn surface_waves_go_both_ways_and_weaken_with_depth() {
        let (rt, table) = (tracer(), table());
        let sp_shallow = Spreading::build(&rt, 10.0);
        let sp_deep = Spreading::build(&rt, 300.0);
        let st = station_at(60.0);
        let get = |sp: &Spreading, depth: f64, name: &str| {
            station_arrivals(
                &table,
                sp,
                &event(depth),
                &st,
                None,
                1.0,
                SurfaceWaves::default(),
            )
            .into_iter()
            .find(|a| a.phase == name)
            .unwrap()
        };
        let r1 = get(&sp_shallow, 10.0, "R1");
        let r2 = get(&sp_shallow, 10.0, "R2");
        let km = 60f64.to_radians() * EARTH_RADIUS_KM;
        assert!((r1.time_s - km / 3.5).abs() < 1e-9);
        assert!(r2.time_s > r1.time_s && r2.amplitude < r1.amplitude);
        assert!(
            get(&sp_shallow, 10.0, "L1").time_s < r1.time_s,
            "Love is faster"
        );
        assert!(get(&sp_deep, 300.0, "R1").amplitude < 0.1 * r1.amplitude);
        // Calibration: shallow earthquake at 60°, Rayleigh about ten times direct P.
        let p = get(&sp_shallow, 10.0, "P");
        let ratio = r1.amplitude / p.amplitude;
        assert!((4.0..25.0).contains(&ratio), "R1/P = {ratio:.1}");
    }

    #[test]
    fn three_d_delay_moves_only_p_and_s() {
        let (rt, table) = (tracer(), table());
        let sp = Spreading::build(&rt, 29.0);
        let n = crate::mantle::DELAY_AZ_COUNT * crate::mantle::DELAY_DIST_COUNT;
        let grid = DelayGrid {
            delays: vec![vec![2.0; n], vec![-3.0; n]],
            max_abs_s: 3.0,
        };
        let st = station_at(50.0);
        let one_d = station_arrivals(
            &table,
            &sp,
            &event(29.0),
            &st,
            None,
            1.0,
            SurfaceWaves::default(),
        );
        let three_d = station_arrivals(
            &table,
            &sp,
            &event(29.0),
            &st,
            Some(&grid),
            2.0,
            SurfaceWaves::default(),
        );
        let t =
            |v: &[StationArrival], name: &str| v.iter().find(|a| a.phase == name).unwrap().time_s;
        assert!((t(&three_d, "P") - t(&one_d, "P") - 4.0).abs() < 1e-6);
        assert!((t(&three_d, "S") - t(&one_d, "S") + 6.0).abs() < 1e-6);
        assert_eq!(t(&three_d, "PcP"), t(&one_d, "PcP"));
        assert_eq!(t(&three_d, "R1"), t(&one_d, "R1"));
    }

    #[test]
    fn delay_sample_matches_nodes_and_wraps_in_azimuth() {
        let (nd, na) = (
            crate::mantle::DELAY_DIST_COUNT,
            crate::mantle::DELAY_AZ_COUNT,
        );
        let mut v = vec![0.0f32; nd * na];
        for j in 0..na {
            for i in 0..nd {
                v[j * nd + i] = (j * 1000 + i) as f32;
            }
        }
        let g = DelayGrid {
            delays: vec![v],
            max_abs_s: 0.0,
        };
        assert_eq!(g.sample(0, 10.0, 15.0), 3010.0);
        assert!((g.sample(0, 10.5, 15.0) - 3010.5).abs() < 1e-9);
        // Between the last row (355°) and the first (0°/360°).
        let wrap = g.sample(0, 10.0, 357.5);
        assert!((wrap - 0.5 * (71_010.0 + 10.0)).abs() < 1e-6, "{wrap}");
        assert_eq!(g.sample(0, 10.0, -5.0), g.sample(0, 10.0, 355.0));
    }

    fn picks() -> Vec<StationArrival> {
        let mk = |phase, motion, time_s, amplitude, period_s| StationArrival {
            phase,
            motion,
            time_s,
            amplitude,
            period_s,
        };
        vec![
            mk("P", Motion::Compressional, 100.0, 1.0, P_PERIOD_S),
            mk("S", Motion::Shear, 200.0, 3.0, S_PERIOD_S),
            mk("R1", Motion::Rayleigh, 400.0, 10.0, SURFACE_PERIOD_S),
            mk("L1", Motion::Love, 250.0, 10.0, SURFACE_PERIOD_S),
        ]
    }

    #[test]
    fn quiet_before_the_first_arrival_and_deterministic() {
        let s = synthesize(&picks(), Component::Vertical, 0.0, 600.0, 7);
        assert!(s.peak > 0.0);
        let before = s
            .samples
            .iter()
            .enumerate()
            .filter(|(k, _)| s.time(*k) < 100.0 - 1.5 * P_PERIOD_S)
            .fold(0.0f32, |m, (_, v)| m.max(v.abs()));
        // The noise is four unit sinusoids scaled to unit RMS: at most 2√2 times its fraction.
        assert!(
            before <= 3.0 * NOISE_FRACTION as f32 * s.peak,
            "{before} vs {}",
            s.peak
        );
        // The P wavelet peaks at its arrival time.
        let k = ((100.0 - s.start_s) / s.dt_s).round() as usize;
        assert!(s.samples[k] > 0.5);
        assert_eq!(s, synthesize(&picks(), Component::Vertical, 0.0, 600.0, 7));
        assert_ne!(s, synthesize(&picks(), Component::Vertical, 0.0, 600.0, 8));
    }

    #[test]
    fn components_see_different_waves() {
        let z = synthesize(&picks(), Component::Vertical, 0.0, 600.0, 7);
        let t = synthesize(&picks(), Component::Transverse, 0.0, 600.0, 7);
        let at =
            |s: &Seismogram, time: f64| s.samples[((time - s.start_s) / s.dt_s).round() as usize];
        // No P on the transverse; no Love on the vertical (only noise there).
        assert!(at(&t, 100.0).abs() < 0.05);
        let love_window = |s: &Seismogram| {
            (0..s.samples.len())
                .filter(|&k| (s.time(k) - 250.0).abs() < 20.0)
                .fold(0.0f32, |m, k| m.max(s.samples[k].abs()))
        };
        assert!(love_window(&t) > 5.0);
        assert!(love_window(&z) < 1.0, "{}", love_window(&z));
    }

    #[test]
    fn columns_keep_every_peak_and_stop_at_the_pen() {
        let s = synthesize(&picks(), Component::Vertical, 0.0, 600.0, 7);
        // 600 s over 50 columns: 12 s per column, far wider than the 1 s P wavelet.
        let cols = s.columns(0.0, 600.0, 50, f64::INFINITY, s.peak);
        assert_eq!(cols.len(), 50);
        let top = cols.iter().fold(0.0f32, |m, c| m.max(c[1]));
        let bottom = cols.iter().fold(0.0f32, |m, c| m.min(c[0]));
        assert!((top.max(-bottom) - 1.0).abs() < 1e-6, "the peak survives");
        let p_col = &cols[(100.0 / 12.0) as usize];
        assert!(p_col[1] * s.peak > 0.5, "P is visible in its column");
        let partial = s.columns(0.0, 600.0, 50, 150.0, s.peak);
        assert_eq!(partial.len(), 13);
        assert!(s.columns(0.0, 600.0, 0, 1e9, s.peak).is_empty());
        // A shared scale twice this trace's peak draws it at half height.
        let half = s.columns(0.0, 600.0, 50, f64::INFINITY, 2.0 * s.peak);
        let top = half
            .iter()
            .fold(0.0f32, |m, c| m.max(c[1].abs()).max(c[0].abs()));
        assert!((top - 0.5).abs() < 1e-6);
    }

    #[test]
    fn same_inputs_same_arrivals() {
        let (rt, table) = (tracer(), table());
        let sp = Spreading::build(&rt, 29.0);
        let a = station_arrivals(
            &table,
            &sp,
            &event(29.0),
            &station_at(70.0),
            None,
            1.0,
            SurfaceWaves::default(),
        );
        let b = station_arrivals(
            &table,
            &Spreading::build(&rt, 29.0),
            &event(29.0),
            &station_at(70.0),
            None,
            1.0,
            SurfaceWaves::default(),
        );
        assert_eq!(a, b);
        assert_eq!(seed_from(&["a", "bc"]), seed_from(&["a", "bc"]));
        assert_ne!(seed_from(&["ab", "c"]), seed_from(&["a", "bc"]));
    }
}
