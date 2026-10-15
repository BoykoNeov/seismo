//! Surface-wave dispersion (`surface_dispersion.csv`): phase and group velocity of the
//! fundamental-mode Rayleigh and Love waves as a function of period, computed from ak135 by
//! `tools/gen_dispersion.py`.

use std::path::Path;

use crate::TableError;

/// The two kinds of surface wave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceWave {
    /// Up-and-down and back-and-forth along the path (P and SV combined).
    Rayleigh,
    /// Side to side, across the path (SH).
    Love,
}

/// Phase and group velocity (km/s) of one wave, sampled at [`Dispersion::periods`].
#[derive(Debug, Clone, PartialEq)]
struct Curve {
    phase: Vec<f64>,
    group: Vec<f64>,
}

/// Dispersion curves over a band of periods (increasing). Lookups interpolate linearly in log
/// period and return `None` outside the band.
#[derive(Debug, Clone, PartialEq)]
pub struct Dispersion {
    periods: Vec<f64>,
    rayleigh: Curve,
    love: Curve,
}

const HEADER: &str =
    "period_s,rayleigh_phase_km_s,rayleigh_group_km_s,love_phase_km_s,love_group_km_s";

impl Dispersion {
    /// Parse the CSV text: `#` comment lines, a header line, then rows with strictly
    /// increasing period and positive velocities.
    pub fn from_csv_str(text: &str) -> Result<Self, TableError> {
        let err = |line: usize, msg: String| TableError::Dispersion { line, msg };
        let mut lines = text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'));
        match lines.next() {
            Some((_, h)) if h.trim() == HEADER => {}
            Some((n, _)) => return Err(err(n + 1, format!("expected header {HEADER:?}"))),
            None => return Err(err(1, "empty file".into())),
        }
        let mut periods = Vec::new();
        let mut rayleigh = Curve {
            phase: Vec::new(),
            group: Vec::new(),
        };
        let mut love = rayleigh.clone();
        for (n, line) in lines {
            let vals: Vec<f64> = line
                .split(',')
                .map(|f| f.trim().parse::<f64>())
                .collect::<Result<_, _>>()
                .map_err(|e| err(n + 1, e.to_string()))?;
            let [t, rc, ru, lc, lu] = vals[..] else {
                return Err(err(n + 1, format!("expected 5 fields, got {}", vals.len())));
            };
            if periods.last().is_some_and(|&p| t <= p) || t <= 0.0 {
                return Err(err(n + 1, format!("period {t} does not increase")));
            }
            if [rc, ru, lc, lu].iter().any(|&v| v.is_nan() || v <= 0.0) {
                return Err(err(n + 1, "velocities must be positive".into()));
            }
            periods.push(t);
            rayleigh.phase.push(rc);
            rayleigh.group.push(ru);
            love.phase.push(lc);
            love.group.push(lu);
        }
        if periods.len() < 2 {
            return Err(err(1, "need at least 2 rows".into()));
        }
        Ok(Self {
            periods,
            rayleigh,
            love,
        })
    }

    /// Parse from the bytes of the CSV file.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TableError> {
        let text = std::str::from_utf8(bytes).map_err(|e| TableError::Dispersion {
            line: 1,
            msg: e.to_string(),
        })?;
        Self::from_csv_str(text)
    }

    /// Read and parse a dispersion file. Thin wrapper over [`Self::from_bytes`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self, TableError> {
        Self::from_bytes(&std::fs::read(path)?)
    }

    /// The sampled periods (s), increasing.
    pub fn periods(&self) -> &[f64] {
        &self.periods
    }

    /// Shortest and longest period (s) of the band.
    pub fn band(&self) -> (f64, f64) {
        (self.periods[0], self.periods[self.periods.len() - 1])
    }

    fn curve(&self, wave: SurfaceWave) -> &Curve {
        match wave {
            SurfaceWave::Rayleigh => &self.rayleigh,
            SurfaceWave::Love => &self.love,
        }
    }

    /// Phase velocity (km/s): the speed of the crests.
    pub fn phase_km_s(&self, wave: SurfaceWave, period_s: f64) -> Option<f64> {
        self.lookup(&self.curve(wave).phase, period_s)
    }

    /// Group velocity (km/s): the speed at which the energy of this period travels.
    pub fn group_km_s(&self, wave: SurfaceWave, period_s: f64) -> Option<f64> {
        self.lookup(&self.curve(wave).group, period_s)
    }

    /// Slowest and fastest group velocity (km/s) over the band.
    pub fn group_range(&self, wave: SurfaceWave) -> (f64, f64) {
        let g = &self.curve(wave).group;
        g.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        })
    }

    fn lookup(&self, values: &[f64], period_s: f64) -> Option<f64> {
        let (lo, hi) = self.band();
        if !(lo..=hi).contains(&period_s) {
            return None;
        }
        let k = self
            .periods
            .partition_point(|&p| p <= period_s)
            .clamp(1, self.periods.len() - 1);
        let (t0, t1) = (self.periods[k - 1].ln(), self.periods[k].ln());
        let f = (period_s.ln() - t0) / (t1 - t0);
        Some(values[k - 1] + f * (values[k] - values[k - 1]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ASSET: &[u8] = include_bytes!("../../../assets/surface_dispersion.csv");

    fn asset() -> Dispersion {
        Dispersion::from_bytes(ASSET).unwrap()
    }

    #[test]
    fn parses_comments_and_interpolates_in_log_period() {
        let text = format!("# made by hand\n{HEADER}\n10,3.0,2.8,3.5,3.3\n40,4.0,3.6,4.5,4.0\n");
        let d = Dispersion::from_csv_str(&text).unwrap();
        assert_eq!(d.band(), (10.0, 40.0));
        assert_eq!(d.phase_km_s(SurfaceWave::Rayleigh, 10.0), Some(3.0));
        assert_eq!(d.group_km_s(SurfaceWave::Love, 40.0), Some(4.0));
        // 20 s is half way between 10 s and 40 s in log period.
        let mid = d.phase_km_s(SurfaceWave::Love, 20.0).unwrap();
        assert!((mid - 4.0).abs() < 1e-12, "{mid}");
        assert_eq!(d.phase_km_s(SurfaceWave::Rayleigh, 9.99), None);
        assert_eq!(d.group_km_s(SurfaceWave::Rayleigh, 40.01), None);
        assert_eq!(d.group_range(SurfaceWave::Rayleigh), (2.8, 3.6));
    }

    #[test]
    fn rejects_bad_files() {
        let bad = |text: &str, line: usize| {
            let e = Dispersion::from_csv_str(text).unwrap_err();
            assert!(
                matches!(e, TableError::Dispersion { line: l, .. } if l == line),
                "{e}"
            );
        };
        bad("period,c\n1,2\n", 1);
        bad(&format!("{HEADER}\n10,3,2.8,3.5\n"), 2);
        bad(
            &format!("{HEADER}\n10,3,2.8,3.5,3.3\n10,3,2.8,3.5,3.3\n"),
            3,
        );
        bad(&format!("{HEADER}\n10,3,0,3.5,3.3\n"), 2);
        bad(&format!("{HEADER}\n10,3,2.8,3.5,3.3\n"), 1);
    }

    /// The generator's own check, repeated on the committed file: group velocity agrees with
    /// the one implied by the phase velocity, U = c / (1 + (T / c) dc/dT). The traces' crests
    /// come from the phase velocity and the map's bands from the group velocity, so the two
    /// must not drift apart.
    #[test]
    fn group_velocity_matches_phase_velocity() {
        let d = asset();
        let t = d.periods();
        for wave in [SurfaceWave::Rayleigh, SurfaceWave::Love] {
            let c = &d.curve(wave).phase;
            let u = &d.curve(wave).group;
            for k in 1..t.len() - 1 {
                let dc_dlnt = (c[k + 1] - c[k - 1]) / (t[k + 1].ln() - t[k - 1].ln());
                let from_c = c[k] / (1.0 + dc_dlnt / c[k]);
                let rel = (from_c - u[k]).abs() / u[k];
                assert!(rel < 3e-3, "{wave:?} at {} s: {from_c} vs {}", t[k], u[k]);
            }
        }
    }

    /// Mineos (spherical normal modes, with gravity; github.com/geodynamics/mineos at
    /// 26f842d) on the same ak135 without attenuation, 2026-10-15: phase and group velocity
    /// (km/s), interpolated to these periods in log period.
    #[test]
    fn agrees_with_mineos() {
        let d = asset();
        let mineos = [
            (SurfaceWave::Rayleigh, 20.0, 3.5721, 2.9704),
            (SurfaceWave::Rayleigh, 50.0, 3.9970, 3.7864),
            (SurfaceWave::Rayleigh, 100.0, 4.1629, 3.8539),
            (SurfaceWave::Rayleigh, 150.0, 4.3621, 3.7498),
            (SurfaceWave::Love, 20.0, 3.8721, 3.4201),
            (SurfaceWave::Love, 50.0, 4.3561, 3.9827),
            (SurfaceWave::Love, 100.0, 4.6024, 4.2697),
            (SurfaceWave::Love, 150.0, 4.7710, 4.3255),
        ];
        for (wave, t, c, u) in mineos {
            let (dc, du) = (
                d.phase_km_s(wave, t).unwrap(),
                d.group_km_s(wave, t).unwrap(),
            );
            assert!(
                (dc - c).abs() / c < 1e-3,
                "{wave:?} {t} s phase {dc} vs {c}"
            );
            assert!(
                (du - u).abs() / u < 3e-3,
                "{wave:?} {t} s group {du} vs {u}"
            );
        }
    }

    #[test]
    fn real_curves_have_the_expected_shape() {
        let d = asset();
        assert_eq!(d.band(), (8.0, 200.0));
        for &t in d.periods() {
            let r = d.group_km_s(SurfaceWave::Rayleigh, t).unwrap();
            let l = d.group_km_s(SurfaceWave::Love, t).unwrap();
            // Love waves outrun Rayleigh waves, and each is slower than its crests.
            assert!(l > r, "{t} s");
            assert!(r < d.phase_km_s(SurfaceWave::Rayleigh, t).unwrap(), "{t} s");
            assert!(l < d.phase_km_s(SurfaceWave::Love, t).unwrap(), "{t} s");
        }
        // Phase velocity rises with period (longer waves reach deeper, faster rock).
        let c = &d.curve(SurfaceWave::Rayleigh).phase;
        assert!(c.windows(2).all(|w| w[1] > w[0]));
        // The crust's slow Rayleigh waves: slowest energy near 15-20 s.
        let (lo, hi) = d.group_range(SurfaceWave::Rayleigh);
        let slowest = d
            .periods()
            .iter()
            .copied()
            .find(|&t| d.group_km_s(SurfaceWave::Rayleigh, t) == Some(lo))
            .unwrap();
        assert!((12.0..20.0).contains(&slowest), "{slowest}");
        assert!(lo > 2.8 && hi < 4.0, "{lo} {hi}");
    }
}
