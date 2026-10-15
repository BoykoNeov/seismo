//! `--bench`: measure frame times over a fixed stretch of a run, report them, then quit.
//!
//! The on-screen fps readout is smoothed and momentary; this gives reproducible numbers for the
//! integrated-GPU target (60 fps at 1920x1080). With `--bench` the window presents without
//! vsync, so the figures show the headroom above the display's refresh rate rather than being
//! capped by it.

use std::io::Write as _;
use std::path::PathBuf;

/// Frame-time collection for one `--bench` run.
pub struct Bench {
    /// Seconds after start before measuring (shader compiles, the CRT warm-up, the first
    /// 3D-mantle delays, the window settling at its `--size`).
    pub warmup_s: f64,
    /// Seconds measured.
    pub measure_s: f64,
    /// Where to append the report line, besides the log.
    pub out: Option<PathBuf>,
    /// Real time between frames (s), one per measured frame.
    frames: Vec<f64>,
    /// eframe's CPU time of the previous frame (s): `ui` plus encoding, without the vsync wait.
    cpu: Vec<f64>,
    /// Fewest and most events on the map during the measured frames.
    map_events: Option<(usize, usize)>,
    /// Fewest and most of those with their 3D-mantle delays on the GPU.
    with_delays: Option<(usize, usize)>,
}

/// Summary of a sample: mean, median and 95th percentile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
}

impl Stats {
    /// `None` for an empty sample.
    pub fn of(sample: &[f64]) -> Option<Self> {
        if sample.is_empty() {
            return None;
        }
        let mut s = sample.to_vec();
        s.sort_by(f64::total_cmp);
        let at = |q: f64| s[((s.len() - 1) as f64 * q).round() as usize];
        Some(Self {
            mean: s.iter().sum::<f64>() / s.len() as f64,
            median: at(0.5),
            p95: at(0.95),
        })
    }
}

impl Bench {
    pub fn new(measure_s: f64, warmup_s: f64, out: Option<PathBuf>) -> Self {
        Self {
            warmup_s,
            measure_s,
            out,
            frames: Vec::new(),
            cpu: Vec::new(),
            map_events: None,
            with_delays: None,
        }
    }

    fn measuring(&self, elapsed_s: f64) -> bool {
        elapsed_s >= self.warmup_s && elapsed_s < self.warmup_s + self.measure_s
    }

    /// The measured stretch is over.
    pub fn due(&self, elapsed_s: f64) -> bool {
        elapsed_s >= self.warmup_s + self.measure_s
    }

    /// One frame, `elapsed_s` after start: `dt` since the previous one, `cpu` as eframe
    /// reports it for the previous frame.
    pub fn frame(&mut self, elapsed_s: f64, dt: f64, cpu: Option<f32>) {
        if self.measuring(elapsed_s) {
            self.frames.push(dt);
            if let Some(c) = cpu {
                self.cpu.push(c as f64);
            }
        }
    }

    /// The map's event count this frame, and how many of them had 3D delays.
    pub fn map(&mut self, elapsed_s: f64, events: usize, with_delays: usize) {
        if self.measuring(elapsed_s) {
            let widen = |r: Option<(usize, usize)>, n: usize| {
                Some(r.map_or((n, n), |(lo, hi)| (lo.min(n), hi.max(n))))
            };
            self.map_events = widen(self.map_events, events);
            self.with_delays = widen(self.with_delays, with_delays);
        }
    }

    /// Log the report (and append it to `out`). `context` names the setup (adapter, sizes,
    /// flags); `offscreen_ms` is the scene alone, rendered and waited for without presenting;
    /// `passes_ms` the map, slice, traces and post-process passes each on its own, as (CPU
    /// encoding, GPU wait).
    pub fn report(
        &self,
        context: &str,
        offscreen_ms: Option<Stats>,
        passes_ms: [[Option<Stats>; 2]; 4],
    ) {
        let ms = |s: Option<Stats>| {
            s.map_or("n/a".to_owned(), |s| {
                format!("mean {:.2} median {:.2} p95 {:.2}", s.mean, s.median, s.p95)
            })
        };
        let med = |[cpu, gpu]: [Option<Stats>; 2]| {
            let m = |s: Option<Stats>| s.map_or("n/a".to_owned(), |s| format!("{:.2}", s.median));
            format!("cpu {} gpu {}", m(cpu), m(gpu))
        };
        let to_ms: Vec<f64> = self.frames.iter().map(|s| s * 1e3).collect();
        let cpu_ms: Vec<f64> = self.cpu.iter().map(|s| s * 1e3).collect();
        let frame = Stats::of(&to_ms);
        let fps = frame.map_or(0.0, |s| 1e3 / s.mean.max(1e-9));
        let range =
            |r: Option<(usize, usize)>| r.map_or("n/a".to_owned(), |(a, b)| format!("{a}-{b}"));
        let line = format!(
            "{context} | frames {} | fps {fps:.1} | frame ms {} | cpu ms {} | scene offscreen ms {} \
             | passes median ms: map {} / slice {} / traces {} / post {} | map events {} | with 3D delays {}",
            self.frames.len(),
            ms(frame),
            ms(Stats::of(&cpu_ms)),
            ms(offscreen_ms),
            med(passes_ms[0]),
            med(passes_ms[1]),
            med(passes_ms[2]),
            med(passes_ms[3]),
            range(self.map_events),
            range(self.with_delays),
        );
        tracing::info!("bench: {line}");
        if let Some(path) = &self.out {
            let written = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| writeln!(f, "{line}"));
            if let Err(e) = written {
                tracing::error!(path = %path.display(), "bench report not written: {e}");
            }
        }
    }
}

/// Parse `WxH` (e.g. `1920x1080`) into a size.
pub fn parse_size(s: &str) -> Option<[u32; 2]> {
    let (w, h) = s.split_once(['x', 'X'])?;
    let (w, h) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (w > 0 && h > 0).then_some([w, h])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_of_a_sample() {
        assert_eq!(Stats::of(&[]), None);
        let s = Stats::of(&[4.0, 1.0, 3.0, 2.0, 100.0]).unwrap();
        assert_eq!(s.mean, 22.0);
        assert_eq!(s.median, 3.0);
        assert_eq!(s.p95, 100.0);
    }

    #[test]
    fn measures_only_after_the_warm_up() {
        let mut b = Bench::new(2.0, 1.0, None);
        b.frame(0.5, 0.1, Some(0.01));
        b.frame(1.5, 0.02, Some(0.01));
        b.frame(3.5, 0.03, None);
        assert_eq!(b.frames, [0.02]);
        assert!(!b.due(2.9) && b.due(3.0));
        b.map(1.2, 8, 3);
        b.map(1.4, 5, 5);
        b.map(0.2, 1, 0);
        assert_eq!(b.map_events, Some((5, 8)));
        assert_eq!(b.with_delays, Some((3, 5)));
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1920x1080"), Some([1920, 1080]));
        assert_eq!(parse_size("800X600"), Some([800, 600]));
        assert_eq!(parse_size("0x600"), None);
        assert_eq!(parse_size("1920"), None);
        assert_eq!(parse_size("ax1"), None);
    }
}
