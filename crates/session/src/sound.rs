//! A seismogram made audible. The ground shakes too slowly to hear: a P wave's pulse lasts about
//! a second, a surface wave's swings tens of seconds. Played a thousand times faster, they land
//! in the range of hearing (P near a kilohertz, surface waves as a low rumble), and a whole
//! hour of record takes a few seconds. This builds the sound; a front end plays it.

use sim::seismogram::Seismogram;

/// Speed-ups offered (× real time), and the one chosen at start.
pub const SPEEDS: [f64; 4] = [500.0, 1000.0, 2000.0, 4000.0];
pub const DEFAULT_SPEED: f64 = 1000.0;
/// Audio samples per second.
pub const SAMPLE_RATE_HZ: u32 = 44_100;
/// Loudest sample, as a fraction of full scale: the trace's peak, with room to spare.
pub const PEAK: f32 = 0.8;
/// Length (s of sound) of the fade at each end, so the sound starts and stops without a click.
pub const FADE_S: f64 = 0.02;

/// The part of `trace` from `from_s` to `to_s` (seconds on the trace's own clock) played `speed`
/// times faster as mono samples at `rate_hz`, scaled so its loudest sample is [`PEAK`] and
/// faded in and out over [`FADE_S`]. Between the trace's samples it is interpolated with a
/// Catmull-Rom curve, smooth enough not to buzz. Outside the trace it is silent; a silent
/// trace gives silence.
pub fn audio(trace: &Seismogram, from_s: f64, to_s: f64, speed: f64, rate_hz: u32) -> Vec<f32> {
    if !(to_s > from_s && speed > 0.0 && rate_hz > 0) {
        return Vec::new();
    }
    let n = ((to_s - from_s) / speed * f64::from(rate_hz)).ceil() as usize;
    let x = &trace.samples;
    let last = x.len() as f64 - 1.0;
    // The curve's neighbours past either end repeat the end sample, so it does not swing.
    let at = |k: isize| f64::from(x[k.clamp(0, x.len() as isize - 1) as usize]);
    let mut out: Vec<f64> = (0..n)
        .map(|i| {
            let t = from_s + i as f64 * speed / f64::from(rate_hz);
            let u = (t - trace.start_s) / trace.dt_s;
            if !(0.0..=last).contains(&u) {
                return 0.0;
            }
            let k = u.floor();
            let f = u - k;
            let k = k as isize;
            let (p0, p1, p2, p3) = (at(k - 1), at(k), at(k + 1), at(k + 2));
            // Catmull-Rom between p1 and p2.
            p1 + 0.5
                * f
                * (p2 - p0
                    + f * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + f * (3.0 * (p1 - p2) + p3 - p0)))
        })
        .collect();
    let peak = out.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let gain = if peak > 0.0 {
        f64::from(PEAK) / peak
    } else {
        0.0
    };
    let fade = (FADE_S * f64::from(rate_hz)).max(1.0);
    for (i, v) in out.iter_mut().enumerate() {
        let edge = (i.min(n - 1 - i) as f64 / fade).min(1.0);
        *v *= gain * edge;
    }
    out.into_iter().map(|v| v as f32).collect()
}

/// Where on the trace's clock (s) the sound is after `played_s` seconds of it, for a sound made
/// from `from_s` at `speed`.
pub fn trace_time(from_s: f64, speed: f64, played_s: f64) -> f64 {
    from_s + played_s * speed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trace sampled like SEISMO's (8 a second) holding `f(t)`.
    fn trace(start_s: f64, end_s: f64, f: impl Fn(f64) -> f64) -> Seismogram {
        let dt = sim::seismogram::SAMPLE_DT_S;
        let samples: Vec<f32> = (0..=((end_s - start_s) / dt) as usize)
            .map(|k| f(start_s + k as f64 * dt) as f32)
            .collect();
        let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        Seismogram {
            start_s,
            dt_s: dt,
            samples,
            peak,
        }
    }

    #[test]
    fn an_hour_in_a_few_seconds_at_the_right_pitch() {
        // A 1 s wave (P's period) for an hour, at 1000×: 3.6 s of a 1 kHz tone.
        let tr = trace(0.0, 3600.0, |t| (2.0 * std::f64::consts::PI * t).sin());
        let rate = SAMPLE_RATE_HZ;
        let a = audio(&tr, 0.0, 3600.0, 1000.0, rate);
        assert_eq!(a.len(), (3.6 * f64::from(rate)).ceil() as usize);
        // Count upward zero crossings in the middle second: about a thousand.
        let mid = &a[rate as usize..2 * rate as usize];
        let ups = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((995..=1005).contains(&ups), "{ups} Hz");
        // Smooth, not stepped: neighbouring samples of a 1 kHz tone at 44.1 kHz differ by at
        // most 2π · 1000 / 44100 of the peak.
        let step = mid
            .windows(2)
            .fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(
            step < 1.1 * PEAK * 2.0 * std::f32::consts::PI * 1000.0 / rate as f32,
            "{step}"
        );
    }

    #[test]
    fn scaled_to_the_peak_and_faded_at_both_ends() {
        let tr = trace(-60.0, 600.0, |t| {
            1e-6 * (t / 7.0).sin() + 3e-5 * (-(t - 300.0).powi(2) / 50.0).exp()
        });
        let a = audio(&tr, -60.0, 600.0, 500.0, 8000);
        let peak = a.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - PEAK).abs() < 1e-3, "{peak}");
        assert_eq!(a[0], 0.0);
        assert_eq!(*a.last().unwrap(), 0.0);
        // The same inputs give the same sound.
        assert_eq!(a, audio(&tr, -60.0, 600.0, 500.0, 8000));

        // A steady signal rises through the fade, holds at the peak and falls at the end.
        let steady = trace(0.0, 1000.0, |_| -2.0);
        let a = audio(&steady, 0.0, 1000.0, 500.0, 8000);
        let fade = (FADE_S * 8000.0) as usize;
        assert!((a[fade / 2] + PEAK / 2.0).abs() < 1e-3, "{}", a[fade / 2]);
        assert!(
            a[fade..a.len() - fade]
                .iter()
                .all(|&v| (v + PEAK).abs() < 1e-5)
        );
        assert!((a[a.len() - 1 - fade / 2] + PEAK / 2.0).abs() < 1e-3);
    }

    #[test]
    fn silence_where_there_is_nothing() {
        let quiet = trace(0.0, 100.0, |_| 0.0);
        assert!(
            audio(&quiet, 0.0, 100.0, 1000.0, 8000)
                .iter()
                .all(|&v| v == 0.0)
        );
        // Past the trace's end the sound is silent, and nothing is made for an empty span.
        let tr = trace(0.0, 100.0, |t| (t / 3.0).sin());
        let a = audio(&tr, 0.0, 200.0, 100.0, 1000);
        assert!(a[a.len() * 3 / 4..].iter().all(|&v| v == 0.0));
        assert!(audio(&tr, 50.0, 50.0, 1000.0, 8000).is_empty());
        assert!(audio(&tr, 0.0, 100.0, 0.0, 8000).is_empty());
    }

    #[test]
    fn playhead_follows_the_speed() {
        assert_eq!(trace_time(-60.0, 1000.0, 0.0), -60.0);
        assert_eq!(trace_time(-60.0, 1000.0, 2.5), 2440.0);
    }
}
