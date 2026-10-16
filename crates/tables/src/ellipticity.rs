//! Ellipticity coefficients (`ak135_ellip.bin`, written by `tools/gen_ellipticity.py`):
//! format, parsing and interpolation.
//!
//! The Earth is flattened by its spin, so a wave's travel time differs a little from the
//! spherical model's. The correction is a degree-2 expansion in the source's colatitude and the
//! azimuth to the station (Dziewonski & Gilbert 1976) whose three coefficients depend only on
//! the phase, the source depth and the distance. The file holds them for each phase's first
//! arrival on a coarse grid, computed by EllipticiPy (Russell, Rudge, Irving and Cottaar 2022);
//! `sim::ellipticity` turns them into seconds.

use std::path::Path;

use serde::Deserialize;

use crate::{Axis, TableError};

/// File magic, first 8 bytes.
pub const ELLIP_MAGIC: &[u8; 8] = b"SEISMOEL";
/// Format version this reader understands.
pub const ELLIP_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct Header {
    model: String,
    obspy_version: String,
    ellipticipy_version: String,
    citation: String,
    depth: Axis,
    distance: Axis,
    phases: Vec<String>,
}

/// The coefficients sigma_0, sigma_1, sigma_2 (s) of each phase on a depth × distance grid.
#[derive(Debug, Clone)]
pub struct EllipticityTable {
    model: String,
    obspy_version: String,
    ellipticipy_version: String,
    citation: String,
    depth: Axis,
    distance: Axis,
    phases: Vec<String>,
    /// Per phase: `[depth][distance]` row-major; NaN where the phase has no arrival.
    coefficients: Vec<Vec<[f32; 3]>>,
}

impl EllipticityTable {
    /// Parse the coefficients from the bytes of an `ak135_ellip.bin` file.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TableError> {
        let bad = |msg: String| TableError::Inconsistent(format!("ellipticity: {msg}"));
        if bytes.len() < 16 {
            return Err(TableError::Truncated("ellipticity preamble"));
        }
        if &bytes[0..8] != ELLIP_MAGIC {
            return Err(TableError::BadMagic);
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if version != ELLIP_FORMAT_VERSION {
            return Err(bad(format!(
                "unsupported format version {version} (expected {ELLIP_FORMAT_VERSION})"
            )));
        }
        let header_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let data_start = 16usize
            .checked_add(header_len)
            .filter(|&e| e <= bytes.len())
            .ok_or(TableError::Truncated("ellipticity header"))?;
        let header: Header = serde_json::from_slice(&bytes[16..data_start])?;
        if header.depth.count < 2 || header.distance.count < 2 {
            return Err(bad("each axis needs at least 2 samples".into()));
        }
        if !(header.depth.step > 0.0 && header.distance.step > 0.0) {
            return Err(bad("axis steps must be positive".into()));
        }
        let cells = header.depth.count * header.distance.count;
        let per_phase = cells * 3 * 4;
        let data = &bytes[data_start..];
        let expected = header.phases.len() * per_phase;
        if data.len() < expected {
            return Err(TableError::Truncated("ellipticity data"));
        }
        if data.len() > expected {
            return Err(bad(format!(
                "{} trailing bytes after the coefficients",
                data.len() - expected
            )));
        }
        let coefficients = data
            .chunks_exact(per_phase)
            .map(|grid| {
                grid.as_chunks::<12>()
                    .0
                    .iter()
                    .map(|c| {
                        std::array::from_fn(|m| {
                            f32::from_le_bytes(c[4 * m..4 * m + 4].try_into().unwrap())
                        })
                    })
                    .collect()
            })
            .collect();
        Ok(Self {
            model: header.model,
            obspy_version: header.obspy_version,
            ellipticipy_version: header.ellipticipy_version,
            citation: header.citation,
            depth: header.depth,
            distance: header.distance,
            phases: header.phases,
            coefficients,
        })
    }

    /// Read and parse a file. Thin wrapper over [`Self::from_bytes`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self, TableError> {
        Self::from_bytes(&std::fs::read(path)?)
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn obspy_version(&self) -> &str {
        &self.obspy_version
    }

    pub fn ellipticipy_version(&self) -> &str {
        &self.ellipticipy_version
    }

    pub fn citation(&self) -> &str {
        &self.citation
    }

    pub fn depth_axis(&self) -> Axis {
        self.depth
    }

    pub fn distance_axis(&self) -> Axis {
        self.distance
    }

    /// Phase names in file order (the travel-time table's).
    pub fn phases(&self) -> &[String] {
        &self.phases
    }

    /// The coefficients stored at a grid node, `None` if the phase has no arrival there or the
    /// node is off the grid.
    pub fn node(&self, phase: &str, i_depth: usize, i_dist: usize) -> Option<[f64; 3]> {
        let k = self.phases.iter().position(|p| p == phase)?;
        if i_depth >= self.depth.count || i_dist >= self.distance.count {
            return None;
        }
        let c = self.coefficients[k][i_depth * self.distance.count + i_dist];
        c[0].is_finite().then(|| c.map(f64::from))
    }

    /// The coefficients of `phase` for a source at `depth_km` and a station `dist_deg` away,
    /// interpolated bilinearly between the four surrounding nodes. Depths and distances past
    /// the grid's ends take the end's values. Near the end of a phase's range some of the four
    /// nodes have no arrival: the others are weighted on their own (their weights scaled to
    /// add up to one), so the correction runs on to the end of the travel-time table's
    /// finer-grained range. `None` if none of the four has one, or the phase is unknown.
    pub fn coefficients(&self, phase: &str, depth_km: f64, dist_deg: f64) -> Option<[f64; 3]> {
        let k = self.phases.iter().position(|p| p == phase)?;
        let (i, u) = self
            .depth
            .locate(depth_km.clamp(self.depth.start, self.depth.end()))?;
        let (j, v) = self
            .distance
            .locate(dist_deg.clamp(self.distance.start, self.distance.end()))?;
        let grid = &self.coefficients[k];
        let n = self.distance.count;
        let mut sum = [0.0f64; 3];
        let mut weight = 0.0;
        for (di, dj, w) in [
            (0, 0, (1.0 - u) * (1.0 - v)),
            (0, 1, (1.0 - u) * v),
            (1, 0, u * (1.0 - v)),
            (1, 1, u * v),
        ] {
            let c = grid[(i + di) * n + j + dj];
            if w > 0.0 && c[0].is_finite() {
                for m in 0..3 {
                    sum[m] += w * f64::from(c[m]);
                }
                weight += w;
            }
        }
        // A point exactly on a node gives that node its whole weight: a node without an
        // arrival there means none, even if its neighbours have one.
        (weight > 0.0).then(|| sum.map(|s| s / weight))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file with one phase on a 2 × 3 grid (depths 0, 100 km; distances 0, 10, 20°).
    fn file(cells: &[[f32; 3]; 6]) -> Vec<u8> {
        let header =
            br#"{"model":"test","obspy_version":"0","ellipticipy_version":"0","citation":"c",
            "depth":{"start":0.0,"step":100.0,"count":2},
            "distance":{"start":0.0,"step":10.0,"count":3},"phases":["P"]}"#;
        let mut out = ELLIP_MAGIC.to_vec();
        out.extend(ELLIP_FORMAT_VERSION.to_le_bytes());
        out.extend((header.len() as u32).to_le_bytes());
        out.extend(header);
        for c in cells {
            for v in c {
                out.extend(v.to_le_bytes());
            }
        }
        out
    }

    const NAN: f32 = f32::NAN;

    #[test]
    fn bilinear_between_nodes() {
        let t = EllipticityTable::from_bytes(&file(&[
            [0.0, 1.0, 2.0],
            [1.0, 1.0, 2.0],
            [2.0, 1.0, 2.0],
            [10.0, 1.0, 2.0],
            [11.0, 1.0, 2.0],
            [12.0, 1.0, 2.0],
        ]))
        .unwrap();
        assert_eq!(t.node("P", 1, 2), Some([12.0, 1.0, 2.0]));
        let c = t.coefficients("P", 50.0, 15.0).unwrap();
        assert!((c[0] - 6.5).abs() < 1e-9, "{c:?}");
        assert!((c[1] - 1.0).abs() < 1e-9 && (c[2] - 2.0).abs() < 1e-9);
        // Past the ends: the end's values.
        assert_eq!(t.coefficients("P", 900.0, 20.0), Some([12.0, 1.0, 2.0]));
        assert_eq!(t.coefficients("S", 50.0, 15.0), None);
    }

    #[test]
    fn missing_nodes_leave_the_others_weighted_alone() {
        // The phase ends between 10° and 20° at the surface and has no arrival at 0°.
        let t = EllipticityTable::from_bytes(&file(&[
            [NAN, NAN, NAN],
            [4.0, 0.0, 0.0],
            [NAN, NAN, NAN],
            [NAN, NAN, NAN],
            [8.0, 0.0, 0.0],
            [9.0, 0.0, 0.0],
        ]))
        .unwrap();
        // At 15°, 0 km only the 10° surface node has an arrival: its value, not a quarter of it.
        assert_eq!(t.coefficients("P", 0.0, 15.0), Some([4.0, 0.0, 0.0]));
        // Halfway down, the three present nodes, weighted 1/4, 1/4 and 1/4 out of 3/4.
        let c = t.coefficients("P", 50.0, 15.0).unwrap();
        assert!((c[0] - 7.0).abs() < 1e-9, "{c:?}");
        // On a node without an arrival, none, however close the neighbours are.
        assert_eq!(t.coefficients("P", 0.0, 0.0), None);
        assert_eq!(t.coefficients("P", 0.0, 20.0), None);
        assert_eq!(t.node("P", 0, 0), None);
    }

    #[test]
    fn rejects_damaged_files() {
        let good = file(&[[0.0; 3]; 6]);
        assert!(EllipticityTable::from_bytes(&good).is_ok());
        assert!(matches!(
            EllipticityTable::from_bytes(&good[..good.len() - 4]),
            Err(TableError::Truncated(_))
        ));
        let mut long = good.clone();
        long.push(0);
        assert!(EllipticityTable::from_bytes(&long).is_err());
        let mut magic = good.clone();
        magic[0] = b'X';
        assert!(matches!(
            EllipticityTable::from_bytes(&magic),
            Err(TableError::BadMagic)
        ));
    }
}
