//! The sim clock: absolute UTC that the app advances from real elapsed time.

use chrono::{DateTime, Duration, Utc};

pub const MIN_SPEED: f64 = 0.1;
pub const MAX_SPEED: f64 = 1000.0;

/// Absolute UTC sim time with play/pause, speed and seeking.
///
/// `seek_count` increments on every discontinuous jump (seek/scrub), so the renderer can clear
/// phosphor persistence when it changes.
#[derive(Debug, Clone)]
pub struct SimClock {
    now: DateTime<Utc>,
    playing: bool,
    speed: f64,
    seek_count: u64,
}

impl SimClock {
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            now: start,
            playing: true,
            speed: 1.0,
            seek_count: 0,
        }
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.now
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    /// Set the speed multiplier, clamped to [`MIN_SPEED`, `MAX_SPEED`].
    pub fn set_speed(&mut self, speed: f64) {
        self.speed = if speed.is_finite() {
            speed.clamp(MIN_SPEED, MAX_SPEED)
        } else {
            1.0
        };
    }

    pub fn seek_count(&self) -> u64 {
        self.seek_count
    }

    /// Advance by `real_dt_s` real seconds (times the speed) if playing.
    pub fn tick(&mut self, real_dt_s: f64) {
        if self.playing && real_dt_s > 0.0 {
            self.now += secs(real_dt_s * self.speed);
        }
    }

    /// Jump to an absolute time.
    pub fn seek(&mut self, to: DateTime<Utc>) {
        if to != self.now {
            self.now = to;
            self.seek_count += 1;
        }
    }
}

/// Seconds (f64) as a chrono duration, microsecond resolution.
pub fn secs(s: f64) -> Duration {
    Duration::microseconds((s * 1e6).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2011, 3, 11, 5, 46, 24).unwrap()
    }

    #[test]
    fn ticks_at_speed_when_playing() {
        let mut c = SimClock::new(t0());
        c.tick(0.5);
        assert_eq!(c.now(), t0() + Duration::milliseconds(500));
        c.set_speed(100.0);
        c.tick(0.25);
        assert_eq!(c.now(), t0() + Duration::milliseconds(500 + 25_000));
    }

    #[test]
    fn paused_clock_does_not_move() {
        let mut c = SimClock::new(t0());
        c.set_playing(false);
        c.tick(10.0);
        assert_eq!(c.now(), t0());
        assert!(!c.playing());
    }

    #[test]
    fn speed_is_clamped() {
        let mut c = SimClock::new(t0());
        c.set_speed(0.01);
        assert_eq!(c.speed(), MIN_SPEED);
        c.set_speed(1e9);
        assert_eq!(c.speed(), MAX_SPEED);
        c.set_speed(f64::NAN);
        assert_eq!(c.speed(), 1.0);
    }

    #[test]
    fn seek_moves_and_counts() {
        let mut c = SimClock::new(t0());
        assert_eq!(c.seek_count(), 0);
        c.seek(t0() - Duration::seconds(60));
        assert_eq!(c.now(), t0() - Duration::seconds(60));
        assert_eq!(c.seek_count(), 1);
        // Seeking to the current time is not a jump.
        c.seek(c.now());
        assert_eq!(c.seek_count(), 1);
        // Ticking is continuous, not a seek.
        c.tick(1.0);
        assert_eq!(c.seek_count(), 1);
    }

    #[test]
    fn negative_or_zero_dt_is_ignored() {
        let mut c = SimClock::new(t0());
        c.tick(-1.0);
        c.tick(0.0);
        assert_eq!(c.now(), t0());
    }
}
