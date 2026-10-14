//! The `ak135.tt` travel-time table: format, parsing, bilinear lookup.

use std::path::Path;

use serde::Deserialize;

use crate::TableError;

/// File magic, first 8 bytes.
pub const MAGIC: &[u8; 8] = b"SEISMOTT";
/// Format version this reader understands.
pub const FORMAT_VERSION: u32 = 1;
/// Value that replaces a missing arrival (NaN) in grids uploaded to the GPU.
pub const GPU_MISSING: f32 = -1.0;

/// A regularly sampled axis: `start + i * step` for `i` in `0..count`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Axis {
    pub start: f64,
    pub step: f64,
    pub count: usize,
}

impl Axis {
    pub fn value(&self, i: usize) -> f64 {
        self.start + i as f64 * self.step
    }

    pub fn end(&self) -> f64 {
        self.value(self.count - 1)
    }

    /// Lower cell index and fraction in [0, 1] for `x`, or `None` outside the axis.
    fn locate(&self, x: f64) -> Option<(usize, f64)> {
        let u = (x - self.start) / self.step;
        let last = (self.count - 1) as f64;
        // Small tolerance so an exact end value is not rejected by rounding.
        if !(u >= -1e-9 && u <= last + 1e-9) {
            return None;
        }
        let u = u.clamp(0.0, last);
        let i = (u.floor() as usize).min(self.count - 2);
        Some((i, u - i as f64))
    }
}

#[derive(Debug, Deserialize)]
struct Header {
    model: String,
    obspy_version: String,
    depth: Axis,
    distance: Axis,
    phases: Vec<String>,
}

/// Travel times (s) and ray parameters (s/deg) per phase on a depth × distance grid.
///
/// Each cell holds the first arrival of its named phase; a missing arrival is NaN in
/// storage and `None` from the lookup API.
#[derive(Debug, Clone)]
pub struct TravelTimeTable {
    model: String,
    obspy_version: String,
    depth: Axis,
    distance: Axis,
    phases: Vec<String>,
    /// Per phase: `[depth][distance]` row-major.
    times: Vec<Vec<f32>>,
    ray_params: Vec<Vec<f32>>,
}

impl TravelTimeTable {
    /// Parse a table from the bytes of an `.tt` file.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TableError> {
        if bytes.len() < 16 {
            return Err(TableError::Truncated("preamble"));
        }
        if &bytes[0..8] != MAGIC {
            return Err(TableError::BadMagic);
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if version != FORMAT_VERSION {
            return Err(TableError::UnsupportedVersion(version));
        }
        let header_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let data_start = 16usize
            .checked_add(header_len)
            .filter(|&e| e <= bytes.len())
            .ok_or(TableError::Truncated("header"))?;
        let header: Header = serde_json::from_slice(&bytes[16..data_start])?;

        if header.depth.count < 2 || header.distance.count < 2 {
            return Err(TableError::Inconsistent(
                "each axis needs at least 2 samples".into(),
            ));
        }
        if !(header.depth.step > 0.0 && header.distance.step > 0.0) {
            return Err(TableError::Inconsistent(
                "axis steps must be positive".into(),
            ));
        }
        let cells = header.depth.count * header.distance.count;
        let expected = header.phases.len() * 2 * cells * 4;
        let data = &bytes[data_start..];
        if data.len() < expected {
            return Err(TableError::Truncated("grid data"));
        }
        if data.len() > expected {
            return Err(TableError::Inconsistent(format!(
                "{} trailing bytes after grid data",
                data.len() - expected
            )));
        }

        // The data is not guaranteed to be 4-byte aligned (e.g. from `include_bytes!`), so copy
        // it out (`pod_collect_to_vec`) rather than casting in place, then fix endianness.
        let read_grid = |k: usize| -> Vec<f32> {
            bytemuck::pod_collect_to_vec::<u8, u32>(&data[k * cells * 4..(k + 1) * cells * 4])
                .into_iter()
                .map(|bits| f32::from_bits(u32::from_le(bits)))
                .collect()
        };
        let mut times = Vec::with_capacity(header.phases.len());
        let mut ray_params = Vec::with_capacity(header.phases.len());
        for p in 0..header.phases.len() {
            times.push(read_grid(2 * p));
            ray_params.push(read_grid(2 * p + 1));
        }

        Ok(Self {
            model: header.model,
            obspy_version: header.obspy_version,
            depth: header.depth,
            distance: header.distance,
            phases: header.phases,
            times,
            ray_params,
        })
    }

    /// Read and parse a table file. Thin wrapper over [`Self::from_bytes`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self, TableError> {
        Self::from_bytes(&std::fs::read(path)?)
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn obspy_version(&self) -> &str {
        &self.obspy_version
    }

    pub fn depth_axis(&self) -> Axis {
        self.depth
    }

    pub fn distance_axis(&self) -> Axis {
        self.distance
    }

    /// Phase names in file (and GPU layer) order.
    pub fn phases(&self) -> &[String] {
        &self.phases
    }

    pub fn phase_index(&self, phase: &str) -> Option<usize> {
        self.phases.iter().position(|p| p == phase)
    }

    /// Arrival time (s) of `phase` for a source at `depth_km` and a receiver at `dist_deg`,
    /// bilinearly interpolated. `None` if the phase is unknown, the point is off the grid, or
    /// any of the four surrounding cells has no arrival.
    pub fn arrival(&self, phase: &str, depth_km: f64, dist_deg: f64) -> Option<f64> {
        let k = self.phase_index(phase)?;
        self.bilinear(&self.times[k], depth_km, dist_deg)
    }

    /// Ray parameter (s/deg) of `phase`, same lookup rules as [`Self::arrival`].
    pub fn ray_param(&self, phase: &str, depth_km: f64, dist_deg: f64) -> Option<f64> {
        let k = self.phase_index(phase)?;
        self.bilinear(&self.ray_params[k], depth_km, dist_deg)
    }

    /// Arrival time stored at a grid node, `None` if missing or out of range.
    pub fn node_time(&self, phase: &str, i_depth: usize, i_dist: usize) -> Option<f64> {
        let k = self.phase_index(phase)?;
        self.node(&self.times[k], i_depth, i_dist)
    }

    /// Ray parameter stored at a grid node, `None` if missing or out of range.
    pub fn node_ray_param(&self, phase: &str, i_depth: usize, i_dist: usize) -> Option<f64> {
        let k = self.phase_index(phase)?;
        self.node(&self.ray_params[k], i_depth, i_dist)
    }

    /// Largest arrival time anywhere in the table (s): after this, every tabulated wave has
    /// reached every distance.
    pub fn max_arrival_time(&self) -> f64 {
        self.times
            .iter()
            .flatten()
            .filter(|t| t.is_finite())
            .fold(0.0f32, |a, &b| a.max(b)) as f64
    }

    /// Time and ray parameter of `phase` interleaved per cell, `[depth][distance]` row-major,
    /// with missing values replaced by [`GPU_MISSING`] in both, for upload as an RG32Float
    /// texture layer.
    pub fn gpu_grid(&self, phase: &str) -> Option<Vec<[f32; 2]>> {
        let k = self.phase_index(phase)?;
        let fix = |v: f32| if v.is_nan() { GPU_MISSING } else { v };
        Some(
            self.times[k]
                .iter()
                .zip(&self.ray_params[k])
                .map(|(&t, &p)| [fix(t), fix(p)])
                .collect(),
        )
    }

    fn node(&self, grid: &[f32], i_depth: usize, i_dist: usize) -> Option<f64> {
        if i_depth >= self.depth.count || i_dist >= self.distance.count {
            return None;
        }
        let v = grid[i_depth * self.distance.count + i_dist];
        (!v.is_nan()).then_some(v as f64)
    }

    fn bilinear(&self, grid: &[f32], depth_km: f64, dist_deg: f64) -> Option<f64> {
        let (i, fy) = self.depth.locate(depth_km)?;
        let (j, fx) = self.distance.locate(dist_deg)?;
        let v00 = self.node(grid, i, j)?;
        let v01 = self.node(grid, i, j + 1)?;
        let v10 = self.node(grid, i + 1, j)?;
        let v11 = self.node(grid, i + 1, j + 1)?;
        let top = v00 + (v01 - v00) * fx;
        let bottom = v10 + (v11 - v10) * fx;
        Some(top + (bottom - top) * fy)
    }
}
