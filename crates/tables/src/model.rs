//! The ak135 velocity model (`ak135_model.csv`), parsed for the ray tracer.

use std::path::Path;

use crate::TableError;

/// One sample of the velocity model. A discontinuity appears as two consecutive points at the
/// same depth (values above, then below).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPoint {
    pub depth_km: f64,
    pub vp_km_s: f64,
    pub vs_km_s: f64,
    pub density_g_cm3: f64,
}

/// A 1D Earth model as depth-ordered samples, surface to centre.
#[derive(Debug, Clone, PartialEq)]
pub struct EarthModel {
    points: Vec<ModelPoint>,
}

const HEADER: &str = "depth_km,vp_km_s,vs_km_s,density_g_cm3";

impl EarthModel {
    /// Parse the CSV text: a header line, then `depth_km,vp_km_s,vs_km_s,density_g_cm3` rows
    /// with non-decreasing depth.
    pub fn from_csv_str(text: &str) -> Result<Self, TableError> {
        let mut lines = text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty());
        match lines.next() {
            Some((_, h)) if h.trim() == HEADER => {}
            Some((n, _)) => {
                return Err(TableError::Model {
                    line: n + 1,
                    msg: format!("expected header {HEADER:?}"),
                });
            }
            None => {
                return Err(TableError::Model {
                    line: 1,
                    msg: "empty file".into(),
                });
            }
        }
        let mut points: Vec<ModelPoint> = Vec::new();
        for (n, line) in lines {
            let err = |msg: String| TableError::Model { line: n + 1, msg };
            let vals: Vec<f64> = line
                .split(',')
                .map(|f| f.trim().parse::<f64>())
                .collect::<Result<_, _>>()
                .map_err(|e| err(e.to_string()))?;
            let [depth_km, vp_km_s, vs_km_s, density_g_cm3] = vals[..] else {
                return Err(err(format!("expected 4 fields, got {}", vals.len())));
            };
            if let Some(prev) = points.last()
                && depth_km < prev.depth_km
            {
                return Err(err(format!(
                    "depth {depth_km} decreases (previous {})",
                    prev.depth_km
                )));
            }
            points.push(ModelPoint {
                depth_km,
                vp_km_s,
                vs_km_s,
                density_g_cm3,
            });
        }
        if points.len() < 2 {
            return Err(TableError::Model {
                line: 1,
                msg: "need at least 2 rows".into(),
            });
        }
        Ok(Self { points })
    }

    /// Parse from the bytes of the CSV file.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TableError> {
        let text = std::str::from_utf8(bytes).map_err(|e| TableError::Model {
            line: 1,
            msg: e.to_string(),
        })?;
        Self::from_csv_str(text)
    }

    /// Read and parse a model file. Thin wrapper over [`Self::from_bytes`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self, TableError> {
        Self::from_bytes(&std::fs::read(path)?)
    }

    pub fn points(&self) -> &[ModelPoint] {
        &self.points
    }

    /// Depths (km) where any property jumps: consecutive samples at the same depth.
    pub fn discontinuities(&self) -> Vec<f64> {
        self.points
            .windows(2)
            .filter(|w| w[0].depth_km == w[1].depth_km)
            .map(|w| w[0].depth_km)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "depth_km,vp_km_s,vs_km_s,density_g_cm3\n\
        0.0,5.8,3.46,2.72\n\
        20.0,5.8,3.46,2.72\n\
        20.0,6.5,3.85,2.92\n\
        35.0,6.5,3.85,2.92\n";

    #[test]
    fn parses_rows_and_discontinuities() {
        let m = EarthModel::from_csv_str(SAMPLE).unwrap();
        assert_eq!(m.points().len(), 4);
        assert_eq!(m.points()[2].vp_km_s, 6.5);
        assert_eq!(m.discontinuities(), vec![20.0]);
    }

    #[test]
    fn rejects_bad_header() {
        let e = EarthModel::from_csv_str("depth,vp\n0,1\n").unwrap_err();
        assert!(matches!(e, TableError::Model { line: 1, .. }), "{e}");
    }

    #[test]
    fn rejects_short_row_and_decreasing_depth() {
        let short = format!("{HEADER}\n0,5.8,3.4,2.7\n1,5.8,3.4\n");
        assert!(matches!(
            EarthModel::from_csv_str(&short),
            Err(TableError::Model { line: 3, .. })
        ));
        let back = format!("{HEADER}\n10,5.8,3.4,2.7\n5,5.8,3.4,2.7\n");
        assert!(matches!(
            EarthModel::from_csv_str(&back),
            Err(TableError::Model { line: 3, .. })
        ));
    }
}
