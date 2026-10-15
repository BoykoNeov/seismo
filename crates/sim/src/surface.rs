//! Dispersed surface waves: the Rayleigh or Love wave train that reaches a point after
//! travelling a given distance along the surface.
//!
//! Each period travels at its own speed, so a train that leaves the source as one short pulse
//! arrives spread out: the long periods (fastest energy) first, the short ones trailing. The
//! train is the sum of its periods,
//!
//! ```text
//! u(t) = Σ A(ω) · e^(−ω x / (2 Q U)) · cos(ω t − ω x / c(ω) + m π/2)
//! ```
//!
//! over the band of `surface_dispersion.csv`, with c the phase velocity, U the group velocity,
//! Q the wave's quality factor, x the path length and m the number of times the path has
//! passed through the antipode or back through the source (each adds a quarter cycle). A(ω) is
//! a fixed source spectrum (a display choice, see `docs/approximations.md`), scaled so that a
//! train that had not dispersed would peak at 1. The sum is evaluated with one inverse FFT
//! per train.

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI};

use crate::geo::EARTH_RADIUS_KM;
use crate::tables::{Dispersion, SurfaceWave};

/// Sample spacing (s) of a computed train: eight samples per period at the band's shortest
/// period (8 s). [`Train::at`] interpolates between them.
pub const TRAIN_DT_S: f64 = 1.0;

/// Centre of the source spectrum (s): the 20 s waves the surface-wave magnitude Ms is measured
/// on. A display choice, as is its width.
pub const CENTRAL_PERIOD_S: f64 = 20.0;
/// Width of the source spectrum: one standard deviation in log period is this factor.
const SPECTRUM_WIDTH: f64 = 2.5;
/// The spectrum fades to zero over this factor in period at each end of the band, so the
/// cut-off does not ring.
const TAPER_FACTOR: f64 = 1.3;
/// Room (s) before the fastest energy and after the slowest, for the spread of the pulse
/// itself (about five cycles of the longest period).
const PAD_S: f64 = 1000.0;
/// The train fades in over this window (s before the fastest energy's arrival: from, to). The
/// source spectrum has no phase, so its pulse is symmetric and its longest periods would
/// otherwise reach out ahead of the train, as a slow wobble before the first body waves.
const FADE_IN_S: (f64, f64) = (150.0, 0.0);

/// Quality factor Q of the fundamental-mode Rayleigh and Love waves against period (s): PREM's,
/// computed with Mineos on 2026-10-15 (ak135 has no Q model bundled). Interpolated in log
/// period, held constant past the ends. Amplitude falls by e^(−π x / (Q U T)).
const PREM_Q: [(f64, f64, f64); 11] = [
    (8.0, 654.0, 600.0),
    (10.0, 664.0, 600.0),
    (15.0, 683.0, 601.0),
    (20.0, 612.0, 597.0),
    (30.0, 330.0, 434.0),
    (40.0, 208.0, 256.0),
    (50.0, 161.0, 192.0),
    (75.0, 124.0, 148.0),
    (100.0, 119.0, 138.0),
    (150.0, 133.0, 132.0),
    (200.0, 153.0, 132.0),
];

/// Quality factor of `wave` at `period_s`.
pub fn quality(wave: SurfaceWave, period_s: f64) -> f64 {
    let pick = |row: &(f64, f64, f64)| match wave {
        SurfaceWave::Rayleigh => row.1,
        SurfaceWave::Love => row.2,
    };
    let (first, last) = (&PREM_Q[0], &PREM_Q[PREM_Q.len() - 1]);
    if period_s <= first.0 {
        return pick(first);
    }
    if period_s >= last.0 {
        return pick(last);
    }
    let k = PREM_Q.partition_point(|r| r.0 <= period_s);
    let (a, b) = (&PREM_Q[k - 1], &PREM_Q[k]);
    let f = (period_s / a.0).ln() / (b.0 / a.0).ln();
    pick(a) + f * (pick(b) - pick(a))
}

/// A train sampled every `dt_s` from `start_s` (seconds since the source's origin).
#[derive(Debug, Clone, PartialEq)]
pub struct Train {
    pub start_s: f64,
    pub dt_s: f64,
    pub samples: Vec<f64>,
}

impl Train {
    /// Time (s) of the last sample.
    pub fn end_s(&self) -> f64 {
        self.start_s + (self.samples.len().saturating_sub(1)) as f64 * self.dt_s
    }

    /// The train at time `t_s`, interpolated (Catmull-Rom); 0 outside the samples.
    pub fn at(&self, t_s: f64) -> f64 {
        let n = self.samples.len();
        let x = (t_s - self.start_s) / self.dt_s;
        if n < 2 || x < 0.0 || x > (n - 1) as f64 {
            return 0.0;
        }
        let k = (x.floor() as usize).min(n - 2);
        let f = x - k as f64;
        let s = |i: isize| {
            let i = i.clamp(0, n as isize - 1) as usize;
            self.samples[i]
        };
        let k = k as isize;
        let (p0, p1, p2, p3) = (s(k - 1), s(k), s(k + 1), s(k + 2));
        p1 + 0.5
            * f
            * (p2 - p0
                + f * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + f * (3.0 * (p1 - p2) + p3 - p0)))
    }

    /// The samples at time `t_s`, interpolated linearly; 0 outside them.
    pub fn linear(&self, t_s: f64) -> f64 {
        let n = self.samples.len();
        let x = (t_s - self.start_s) / self.dt_s;
        if n < 2 || x < 0.0 || x > (n - 1) as f64 {
            return 0.0;
        }
        let k = (x.floor() as usize).min(n - 2);
        let f = x - k as f64;
        self.samples[k] * (1.0 - f) + self.samples[k + 1] * f
    }

    /// Largest |sample|.
    pub fn peak(&self) -> f64 {
        self.samples.iter().fold(0.0f64, |m, v| m.max(v.abs()))
    }
}

/// Trains already computed by one [`SurfaceWaves`], by wave and path, so rebuilding a
/// seismogram reuses them. Holds at most [`TrainCache::CAPACITY`] trains and starts afresh
/// when full.
#[derive(Debug, Clone, Default)]
pub struct TrainCache {
    trains: HashMap<(SurfaceWave, u64), Train>,
}

impl TrainCache {
    /// About 20 MB at most: a train is a few thousand samples.
    pub const CAPACITY: usize = 512;

    /// The `wave` train after `path_km`, computed by `surface` unless cached.
    pub fn get(&mut self, surface: &SurfaceWaves, wave: SurfaceWave, path_km: f64) -> &Train {
        let key = (wave, path_km.to_bits());
        if self.trains.len() >= Self::CAPACITY && !self.trains.contains_key(&key) {
            self.trains.clear();
        }
        self.trains
            .entry(key)
            .or_insert_with(|| surface.train(wave, path_km))
    }

    pub fn len(&self) -> usize {
        self.trains.len()
    }

    pub fn is_empty(&self) -> bool {
        self.trains.is_empty()
    }
}

/// Dispersed Rayleigh and Love waves over the band of a [`Dispersion`].
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceWaves {
    dispersion: Dispersion,
}

impl SurfaceWaves {
    pub fn new(dispersion: Dispersion) -> Self {
        Self { dispersion }
    }

    pub fn dispersion(&self) -> &Dispersion {
        &self.dispersion
    }

    /// When the central period's energy arrives after `path_km` (s since origin): the time
    /// a surface wave is labelled with. The train itself starts earlier and ends later.
    pub fn arrival_s(&self, wave: SurfaceWave, path_km: f64) -> f64 {
        let u = self
            .dispersion
            .group_km_s(wave, CENTRAL_PERIOD_S)
            .expect("the central period lies inside the band");
        path_km / u
    }

    /// First and last time (s since origin) of the train after `path_km`: from the fastest
    /// energy to the slowest, padded for the spread of the pulse.
    pub fn span_s(&self, wave: SurfaceWave, path_km: f64) -> (f64, f64) {
        let (slow, fast) = self.dispersion.group_range(wave);
        (path_km / fast - PAD_S, path_km / slow + PAD_S)
    }

    /// When the train after `path_km` starts fading in (s since origin): it is silent before.
    pub fn onset_s(&self, wave: SurfaceWave, path_km: f64) -> f64 {
        path_km.max(0.0) / self.dispersion.group_range(wave).1 - FADE_IN_S.0
    }

    /// The end (s since origin) of the slowest train once round the globe: past this an event's
    /// surface waves (R2, L2 at most) are over everywhere.
    pub fn once_round_s(&self) -> f64 {
        let circle = 2.0 * PI * EARTH_RADIUS_KM;
        let r = self.span_s(SurfaceWave::Rayleigh, circle).1;
        let l = self.span_s(SurfaceWave::Love, circle).1;
        r.max(l)
    }

    /// Weight of `period_s` in the source spectrum: a Gaussian in log period around
    /// [`CENTRAL_PERIOD_S`], faded to zero at the band's ends.
    fn spectrum(&self, period_s: f64) -> f64 {
        let (lo, hi) = self.dispersion.band();
        if period_s <= lo || period_s >= hi {
            return 0.0;
        }
        let taper = |from_edge: f64| {
            let f = (from_edge / TAPER_FACTOR.ln()).min(1.0);
            0.5 * (1.0 - (PI * f).cos())
        };
        let g = (period_s / CENTRAL_PERIOD_S).ln() / SPECTRUM_WIDTH.ln();
        (-0.5 * g * g).exp() * taper((period_s / lo).ln()) * taper((hi / period_s).ln())
    }

    /// The `wave` train after `path_km` along the surface, for a source spectrum that would
    /// peak at 1 had it not dispersed, attenuated by Q along the way. Paths longer than half
    /// the globe have passed the antipode (R2, L2) and are a quarter cycle shifted per pass.
    pub fn train(&self, wave: SurfaceWave, path_km: f64) -> Train {
        let (start_s, z) = self.analytic(wave, path_km);
        Train {
            start_s,
            dt_s: TRAIN_DT_S,
            samples: z.into_iter().map(|v| v.0).collect(),
        }
    }

    /// The train's envelope (its amplitude, smoothed over each cycle), sampled like
    /// [`Self::train`].
    pub fn envelope(&self, wave: SurfaceWave, path_km: f64) -> Train {
        let (start_s, z) = self.analytic(wave, path_km);
        Train {
            start_s,
            dt_s: TRAIN_DT_S,
            samples: z.into_iter().map(|v| v.0.hypot(v.1)).collect(),
        }
    }

    /// The train as a complex (analytic) signal, faded in: its real part is the train, its
    /// magnitude the envelope. The spectrum holds positive frequencies only, so one inverse FFT
    /// gives both. Returns the start time (s) and the samples, [`TRAIN_DT_S`] apart.
    fn analytic(&self, wave: SurfaceWave, path_km: f64) -> (f64, Vec<(f64, f64)>) {
        let (t0, t1) = self.span_s(wave, path_km.max(0.0));
        let needed = ((t1 - t0) / TRAIN_DT_S).ceil() as usize + 1;
        let n = needed.next_power_of_two().max(64);
        let df = 1.0 / (n as f64 * TRAIN_DT_S);
        let passes = (path_km / (PI * EARTH_RADIUS_KM)).floor();
        let shift = passes * FRAC_PI_2;

        // Spectrum per unit frequency: Gaussian per unit log period, so divide by f.
        let mut spec = vec![(0.0f64, 0.0f64); n];
        let mut norm = 0.0;
        for (j, slot) in spec.iter_mut().enumerate().take(n / 2).skip(1) {
            let f = j as f64 * df;
            let period = 1.0 / f;
            let a = self.spectrum(period) / f;
            if a == 0.0 {
                continue;
            }
            norm += a;
            let (Some(c), Some(u)) = (
                self.dispersion.phase_km_s(wave, period),
                self.dispersion.group_km_s(wave, period),
            ) else {
                continue;
            };
            let w = 2.0 * PI * f;
            let decay = (-PI * f * path_km / (quality(wave, period) * u)).exp();
            let phase = w * t0 - w * path_km / c + shift;
            *slot = (a * decay * phase.cos(), a * decay * phase.sin());
        }
        if norm > 0.0 {
            for v in &mut spec {
                v.0 /= norm;
                v.1 /= norm;
            }
        }
        inverse_fft(&mut spec);
        let from = self.onset_s(wave, path_km);
        let to = from + FADE_IN_S.0 - FADE_IN_S.1;
        spec.truncate(needed);
        for (k, v) in spec.iter_mut().enumerate() {
            let t = t0 + k as f64 * TRAIN_DT_S;
            let f = ((t - from) / (to - from)).clamp(0.0, 1.0);
            let fade = (0.5 * PI * f).sin().powi(2);
            *v = (v.0 * fade, v.1 * fade);
        }
        (t0, spec)
    }

    /// The map's surface-wave brightness: per wave, the envelope of the train after each path
    /// (rows [`MAP_PATH_STEP_DEG`] apart, 0° to 360°), sampled at apparent speeds path ÷ time
    /// (columns). Each row is scaled to peak at 1: the map shows where a train is, not how
    /// strong.
    pub fn map_grid(&self) -> SurfaceMapGrid {
        let (r_slow, r_fast) = self.dispersion.group_range(SurfaceWave::Rayleigh);
        let (l_slow, l_fast) = self.dispersion.group_range(SurfaceWave::Love);
        let km_per_deg = EARTH_RADIUS_KM.to_radians();
        let lo = (r_slow.min(l_slow) - MAP_SPEED_MARGIN_KM_S) / km_per_deg;
        let hi = (r_fast.max(l_fast) + MAP_SPEED_MARGIN_KM_S) / km_per_deg;
        let step = (hi - lo) / (MAP_SPEED_COUNT - 1) as f64;
        let mut layers = Vec::new();
        for wave in [SurfaceWave::Rayleigh, SurfaceWave::Love] {
            let mut grid = vec![0.0f32; MAP_SPEED_COUNT * MAP_PATH_COUNT];
            for row in 0..MAP_PATH_COUNT {
                // At the epicentre apparent speed means nothing; use the next row's shape.
                let path_deg = row.max(1) as f64 * MAP_PATH_STEP_DEG;
                let env = self.envelope(wave, path_deg * km_per_deg);
                let cells = &mut grid[row * MAP_SPEED_COUNT..(row + 1) * MAP_SPEED_COUNT];
                for (col, cell) in cells.iter_mut().enumerate() {
                    let speed = lo + col as f64 * step;
                    *cell = env.linear(path_deg / speed) as f32;
                }
                let peak = cells.iter().fold(0.0f32, |m, &v| m.max(v));
                if peak > 0.0 {
                    cells.iter_mut().for_each(|v| *v /= peak);
                }
            }
            layers.push(grid);
        }
        SurfaceMapGrid {
            speed_start_deg_s: lo,
            speed_step_deg_s: step,
            layers,
        }
    }
}

/// Columns of [`SurfaceMapGrid`]: apparent speeds from a little below the slowest group velocity
/// of either wave to a little above the fastest.
pub const MAP_SPEED_COUNT: usize = 128;
const MAP_SPEED_MARGIN_KM_S: f64 = 0.3;
/// Rows of [`SurfaceMapGrid`]: path lengths 0° to 360° (R2 and L2 come back to the epicentre).
pub const MAP_PATH_STEP_DEG: f64 = 2.5;
pub const MAP_PATH_COUNT: usize = 145;

/// Where the surface-wave trains are, for the map: per wave (Rayleigh, then Love), a grid of
/// brightness 0..1, row-major `[path][speed]`, rows [`MAP_PATH_STEP_DEG`] apart from 0°, columns
/// apparent speeds (degrees of arc per second since origin) from `speed_start_deg_s`.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceMapGrid {
    pub speed_start_deg_s: f64,
    pub speed_step_deg_s: f64,
    pub layers: Vec<Vec<f32>>,
}

impl SurfaceMapGrid {
    /// Brightness of `layer` at `path_deg` and `t_s` since origin, bilinear as in the map
    /// shader; 0 outside the grid.
    pub fn sample(&self, layer: usize, path_deg: f64, t_s: f64) -> f64 {
        if t_s <= 0.0 {
            return 0.0;
        }
        let fu = (path_deg / t_s - self.speed_start_deg_s) / self.speed_step_deg_s;
        let fv = path_deg / MAP_PATH_STEP_DEG;
        let (nu, nv) = (MAP_SPEED_COUNT as f64, MAP_PATH_COUNT as f64);
        if !(0.0..=nu - 1.0).contains(&fu) || !(0.0..=nv - 1.0).contains(&fv) {
            return 0.0;
        }
        let (i, j) = (fu.floor().min(nu - 2.0), fv.floor().min(nv - 2.0));
        let (fx, fy) = (fu - i, fv - j);
        let g = &self.layers[layer];
        let at = |x: f64, y: f64| f64::from(g[y as usize * MAP_SPEED_COUNT + x as usize]);
        let top = at(i, j) * (1.0 - fx) + at(i + 1.0, j) * fx;
        let bottom = at(i, j + 1.0) * (1.0 - fx) + at(i + 1.0, j + 1.0) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

/// In-place inverse DFT without the 1/n factor, x_k = Σ_j X_j e^(+2πi jk/n); `n` a power of
/// two. Iterative radix-2.
fn inverse_fft(x: &mut [(f64, f64)]) {
    let n = x.len();
    debug_assert!(n.is_power_of_two());
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            x.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = 2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (ar, ai) = x[start + k];
                let (br, bi) = x[start + k + len / 2];
                let (tr, ti) = (br * cr - bi * ci, br * ci + bi * cr);
                x[start + k] = (ar + tr, ai + ti);
                x[start + k + len / 2] = (ar - tr, ai - ti);
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waves() -> SurfaceWaves {
        SurfaceWaves::new(
            Dispersion::from_bytes(include_bytes!("../../../assets/surface_dispersion.csv"))
                .unwrap(),
        )
    }

    fn km(deg: f64) -> f64 {
        deg.to_radians() * EARTH_RADIUS_KM
    }

    #[test]
    fn fft_matches_a_direct_sum() {
        let n = 16;
        let mut x: Vec<(f64, f64)> = (0..n)
            .map(|j| ((j as f64 * 0.7).sin(), (j as f64 * 1.3).cos()))
            .collect();
        let input = x.clone();
        inverse_fft(&mut x);
        for (k, got) in x.iter().enumerate() {
            let (mut re, mut im) = (0.0, 0.0);
            for (j, v) in input.iter().enumerate() {
                let a = 2.0 * PI * (j * k) as f64 / n as f64;
                re += v.0 * a.cos() - v.1 * a.sin();
                im += v.0 * a.sin() + v.1 * a.cos();
            }
            assert!(
                (got.0 - re).abs() < 1e-9 && (got.1 - im).abs() < 1e-9,
                "{k}"
            );
        }
    }

    /// Time of the train's largest swing among periods near `period_s`, found by band-passing
    /// crudely: the zero-crossing spacing near the time must match.
    fn zero_crossing_period(train: &Train, around_s: f64, half_window_s: f64) -> f64 {
        let mut crossings = Vec::new();
        for (k, w) in train.samples.windows(2).enumerate() {
            let t = train.start_s + k as f64 * train.dt_s;
            if (t - around_s).abs() < half_window_s && w[0].signum() != w[1].signum() {
                crossings.push(t);
            }
        }
        assert!(crossings.len() >= 3, "too few crossings near {around_s}");
        2.0 * (crossings[crossings.len() - 1] - crossings[0]) / (crossings.len() - 1) as f64
    }

    #[test]
    fn long_periods_arrive_first() {
        let sw = waves();
        let x = km(90.0);
        let train = sw.train(SurfaceWave::Rayleigh, x);
        let d = sw.dispersion();
        // Early in the train (the fastest energy, ~3.8 km/s) the waves are long; late (the
        // crust's slow ~3 km/s energy) they are short.
        let early = x / 3.8;
        let late = x / 3.0;
        let t_early = zero_crossing_period(&train, early, 120.0);
        let t_late = zero_crossing_period(&train, late, 40.0);
        assert!(
            t_early > 2.0 * t_late,
            "early {t_early:.1} s, late {t_late:.1} s"
        );
        // ... and those periods are the ones whose group velocity matches the arrival time.
        let u_late = d.group_km_s(SurfaceWave::Rayleigh, t_late).unwrap();
        assert!(
            (u_late - x / late).abs() < 0.25,
            "{t_late:.1} s travels at {u_late:.2}"
        );
    }

    #[test]
    fn the_train_lengthens_and_weakens_with_distance() {
        let sw = waves();
        // Duration that holds most of the energy, measured from the samples.
        let duration = |t: &Train| {
            let total: f64 = t.samples.iter().map(|v| v * v).sum();
            let mut acc = 0.0;
            let (mut a, mut b) = (None, None);
            for (k, v) in t.samples.iter().enumerate() {
                acc += v * v;
                if a.is_none() && acc > 0.05 * total {
                    a = Some(k);
                }
                if b.is_none() && acc > 0.95 * total {
                    b = Some(k);
                }
            }
            (b.unwrap() - a.unwrap()) as f64 * t.dt_s
        };
        let near = sw.train(SurfaceWave::Rayleigh, km(20.0));
        let far = sw.train(SurfaceWave::Rayleigh, km(80.0));
        let ratio = duration(&far) / duration(&near);
        // Four times the distance: roughly four times as long (less, as the pulse's own
        // length counts for more near the source).
        assert!((2.5..4.5).contains(&ratio), "{ratio:.2}");
        assert!(far.peak() < near.peak());
        // Love waves are faster: their train starts earlier.
        let love = sw.span_s(SurfaceWave::Love, km(80.0));
        assert!(love.0 < sw.span_s(SurfaceWave::Rayleigh, km(80.0)).0);
    }

    #[test]
    fn an_undispersed_train_peaks_at_one() {
        // At the source the periods are all in phase: the pulse peaks at 1 at time 0.
        let sw = waves();
        let t = sw.train(SurfaceWave::Rayleigh, 0.0);
        assert!((t.at(0.0) - 1.0).abs() < 1e-6, "{}", t.at(0.0));
        assert!((t.peak() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn energy_stays_inside_the_span() {
        let sw = waves();
        for wave in [SurfaceWave::Rayleigh, SurfaceWave::Love] {
            for deg in [5.0, 60.0, 170.0, 250.0, 355.0] {
                let t = sw.train(wave, km(deg));
                let peak = t.peak();
                let n = t.samples.len();
                // The first and last ~100 s of the padded span are quiet: nothing wrapped round.
                let edge = (100.0 / t.dt_s) as usize;
                let quiet = t.samples[..edge]
                    .iter()
                    .chain(&t.samples[n - edge..])
                    .fold(0.0f64, |m, v| m.max(v.abs()));
                assert!(quiet < 0.01 * peak, "{wave:?} {deg}°: {quiet} vs {peak}");
            }
        }
    }

    #[test]
    fn crossing_the_antipode_shifts_a_quarter_cycle() {
        // Compare a path just short of half the globe with one just past it, with the shift
        // undone by hand: they should be nearly the same train.
        let sw = waves();
        let half = PI * EARTH_RADIUS_KM;
        let a = sw.train(SurfaceWave::Love, half - 1.0);
        let b = sw.train(SurfaceWave::Love, half + 1.0);
        // Correlate b with a's samples shifted by a quarter cycle: a quarter cycle at 20 s is
        // 5 s, but a pure phase shift is not a time shift, so instead check the trains differ
        // while their energy is the same.
        let energy = |t: &Train| t.samples.iter().map(|v| v * v).sum::<f64>();
        assert!((energy(&a) / energy(&b) - 1.0).abs() < 0.05);
        let diff: f64 = a
            .samples
            .iter()
            .zip(&b.samples)
            .map(|(x, y)| (x - y).powi(2))
            .sum();
        assert!(diff > 0.5 * energy(&a), "the shift changes the waveform");
    }

    #[test]
    fn deterministic_and_interpolates_through_samples() {
        let sw = waves();
        let a = sw.train(SurfaceWave::Rayleigh, km(45.0));
        assert_eq!(a, sw.train(SurfaceWave::Rayleigh, km(45.0)));
        for k in [10, 500, 1200] {
            let t = a.start_s + k as f64 * a.dt_s;
            assert!((a.at(t) - a.samples[k]).abs() < 1e-12);
        }
        assert_eq!(a.at(a.start_s - 1.0), 0.0);
        assert_eq!(a.at(a.end_s() + 1.0), 0.0);
    }

    #[test]
    fn cache_returns_the_same_train_and_stays_bounded() {
        let sw = waves();
        let mut cache = TrainCache::default();
        let a = cache.get(&sw, SurfaceWave::Love, km(30.0)).clone();
        assert_eq!(a, sw.train(SurfaceWave::Love, km(30.0)));
        assert_eq!(cache.get(&sw, SurfaceWave::Love, km(30.0)), &a);
        assert_eq!(cache.len(), 1);
        for k in 0..TrainCache::CAPACITY + 3 {
            cache.get(&sw, SurfaceWave::Rayleigh, k as f64);
        }
        assert!(cache.len() <= TrainCache::CAPACITY);
    }

    /// The map draws each train where the traces have it: in each row the brightest apparent
    /// speed is the path over the time of the train's envelope peak, within a column.
    #[test]
    fn map_grid_puts_the_band_where_the_train_is() {
        let sw = waves();
        let grid = sw.map_grid();
        assert_eq!(grid.layers.len(), 2);
        for (layer, wave) in [SurfaceWave::Rayleigh, SurfaceWave::Love]
            .into_iter()
            .enumerate()
        {
            for row in [4, 24, 60, 100, 140] {
                let path_deg = row as f64 * MAP_PATH_STEP_DEG;
                let env = sw.envelope(wave, km(path_deg));
                let k = (0..env.samples.len())
                    .max_by(|&a, &b| env.samples[a].total_cmp(&env.samples[b]))
                    .unwrap();
                let want = path_deg / (env.start_s + k as f64 * env.dt_s);
                let cells = &grid.layers[layer][row * MAP_SPEED_COUNT..(row + 1) * MAP_SPEED_COUNT];
                let col = (0..MAP_SPEED_COUNT)
                    .max_by(|&a, &b| cells[a].total_cmp(&cells[b]))
                    .unwrap();
                let got = grid.speed_start_deg_s + col as f64 * grid.speed_step_deg_s;
                assert!(
                    (got - want).abs() <= 1.01 * grid.speed_step_deg_s,
                    "{wave:?} {path_deg}°: grid {got:.5} deg/s, train {want:.5}"
                );
                assert_eq!(cells[col], 1.0);
                // The shader's lookup agrees with the cells at a node.
                let t = path_deg / got;
                assert!((grid.sample(layer, path_deg, t) - 1.0).abs() < 1e-5);
            }
        }
        // Nothing before origin, nothing far outside the speeds.
        assert_eq!(grid.sample(0, 30.0, -5.0), 0.0);
        assert_eq!(grid.sample(0, 30.0, 1.0), 0.0);
    }

    #[test]
    fn envelope_bounds_the_train() {
        let sw = waves();
        let t = sw.train(SurfaceWave::Rayleigh, km(70.0));
        let e = sw.envelope(SurfaceWave::Rayleigh, km(70.0));
        assert_eq!(t.samples.len(), e.samples.len());
        assert!(
            t.samples
                .iter()
                .zip(&e.samples)
                .all(|(a, b)| a.abs() <= b + 1e-12)
        );
    }

    #[test]
    fn q_follows_prem_and_holds_past_the_ends() {
        assert_eq!(quality(SurfaceWave::Rayleigh, 100.0), 119.0);
        assert_eq!(quality(SurfaceWave::Love, 1.0), 600.0);
        assert_eq!(quality(SurfaceWave::Love, 500.0), 132.0);
        let mid = quality(SurfaceWave::Rayleigh, (20f64 * 30.0).sqrt());
        assert!((mid - 0.5 * (612.0 + 330.0)).abs() < 1e-9);
    }

    #[test]
    fn once_round_covers_the_slowest_wave() {
        let sw = waves();
        let (slow, _) = sw.dispersion().group_range(SurfaceWave::Rayleigh);
        assert!(sw.once_round_s() > 2.0 * PI * EARTH_RADIUS_KM / slow);
    }
}
