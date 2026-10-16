//! The `ak135.tt` travel-time table: format, parsing, bilinear lookup, and the later arrivals
//! of each phase (triplications and other folds) followed branch by branch between grid nodes.

use std::path::Path;

use serde::Deserialize;

use crate::TableError;

/// File magic, first 8 bytes.
pub const MAGIC: &[u8; 8] = b"SEISMOTT";
/// Newest format version this reader understands. Version 1 has the first arrivals only;
/// version 2 adds a sparse section of later arrivals after the grids.
pub const FORMAT_VERSION: u32 = 2;
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
    pub(crate) fn locate(&self, x: f64) -> Option<(usize, f64)> {
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
    /// Version 2 only.
    #[serde(default)]
    later: Option<LaterHeader>,
}

#[derive(Debug, Deserialize)]
struct LaterHeader {
    /// A later arrival closer than this (s) to an earlier kept one was left out.
    merge_s: f64,
    /// Number of later-arrival entries per phase, in phase order.
    counts: Vec<usize>,
}

/// Bytes per later-arrival entry: u16 depth index, u16 distance index, f32 time, f32 ray
/// parameter.
const LATER_ENTRY: usize = 12;

/// Following one branch between neighbouring grid nodes: the ray parameter (s/deg) may differ
/// by at most `MATCH_RAY_PARAM`, and the time by at most `MATCH_TIME_S` from what the ray
/// parameter predicts (t + p·Δ along distance; unchanged across depth, where one depth step
/// moves an arrival by well under that).
const MATCH_RAY_PARAM: f64 = 0.3;
const MATCH_TIME_S: f64 = 2.0;

/// One arrival of a phase: its time (s) and ray parameter (s/deg).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrival {
    pub time_s: f64,
    pub ray_param: f64,
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
    /// Per phase: the later arrivals as (cell = depth index × distance count + distance
    /// index, time, ray parameter), sorted by cell then time. Empty for a version 1 file.
    later: Vec<Vec<(u32, f32, f32)>>,
    merge_s: f64,
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
        if !(1..=FORMAT_VERSION).contains(&version) {
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
        let counts = match (version, &header.later) {
            (1, None) => vec![0; header.phases.len()],
            (1, Some(_)) => {
                return Err(TableError::Inconsistent(
                    "a version 1 table has no later arrivals".into(),
                ));
            }
            (_, None) => {
                return Err(TableError::Inconsistent(
                    "header lacks the later arrivals' counts".into(),
                ));
            }
            (_, Some(l)) if l.counts.len() != header.phases.len() => {
                return Err(TableError::Inconsistent(format!(
                    "{} later-arrival counts for {} phases",
                    l.counts.len(),
                    header.phases.len()
                )));
            }
            (_, Some(l)) => l.counts.clone(),
        };
        let grids = header.phases.len() * 2 * cells * 4;
        let expected = grids + counts.iter().sum::<usize>() * LATER_ENTRY;
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

        let mut later = Vec::with_capacity(counts.len());
        let mut at = grids;
        for &n in &counts {
            let mut entries = Vec::with_capacity(n);
            for e in data[at..at + n * LATER_ENTRY].as_chunks::<LATER_ENTRY>().0 {
                let i = usize::from(u16::from_le_bytes([e[0], e[1]]));
                let j = usize::from(u16::from_le_bytes([e[2], e[3]]));
                let t = f32::from_le_bytes(e[4..8].try_into().unwrap());
                let p = f32::from_le_bytes(e[8..12].try_into().unwrap());
                if i >= header.depth.count || j >= header.distance.count {
                    return Err(TableError::Inconsistent(format!(
                        "later arrival at node ({i}, {j}) is off the grid"
                    )));
                }
                if !(t.is_finite() && p.is_finite()) {
                    return Err(TableError::Inconsistent(format!(
                        "later arrival at node ({i}, {j}) is not a number"
                    )));
                }
                entries.push(((i * header.distance.count + j) as u32, t, p));
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            later.push(entries);
            at += n * LATER_ENTRY;
        }

        Ok(Self {
            model: header.model,
            obspy_version: header.obspy_version,
            depth: header.depth,
            distance: header.distance,
            phases: header.phases,
            times,
            ray_params,
            later,
            merge_s: header.later.map_or(0.0, |l| l.merge_s),
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

    /// Every arrival of `phase` stored at a grid node, earliest first: the first arrival (the
    /// node's [`Self::node_time`]) and then its later ones. Empty if the node has none.
    pub fn node_arrivals(&self, phase: &str, i_depth: usize, i_dist: usize) -> Vec<Arrival> {
        let Some(k) = self.phase_index(phase) else {
            return Vec::new();
        };
        self.node_list(k, i_depth, i_dist)
    }

    /// A later arrival closer than this (s) to an earlier one of its phase is left out, both by
    /// the generator and by [`Self::arrivals`]. 0 for a table with first arrivals only.
    pub fn merge_s(&self) -> f64 {
        self.merge_s
    }

    /// Every arrival of `phase` at `depth_km` and `dist_deg`, earliest first. The first is
    /// exactly [`Self::arrival`] and [`Self::ray_param`]; the others are the phase's later
    /// branches (triplications, folds), each followed from node to node by its ray parameter
    /// and interpolated on its own, so the first arrival switching branch inside a cell leaves
    /// no hole. A branch is left out within one cell of where it ends, like
    /// [`Self::arrival`], and when it comes within [`Self::merge_s`] of an earlier arrival.
    pub fn arrivals(&self, phase: &str, depth_km: f64, dist_deg: f64) -> Vec<Arrival> {
        let Some(k) = self.phase_index(phase) else {
            return Vec::new();
        };
        let (Some(time_s), Some(ray_param)) = (
            self.bilinear(&self.times[k], depth_km, dist_deg),
            self.bilinear(&self.ray_params[k], depth_km, dist_deg),
        ) else {
            return Vec::new();
        };
        let mut out = vec![Arrival { time_s, ray_param }];
        if self.later[k].is_empty() {
            return out;
        }
        let (Some((i, fy)), Some((j, fx))) =
            (self.depth.locate(depth_km), self.distance.locate(dist_deg))
        else {
            return out;
        };
        // The other three corners, with how many distance steps each lies past the first.
        let corners = [
            (self.node_list(k, i, j + 1), 1.0),
            (self.node_list(k, i + 1, j), 0.0),
            (self.node_list(k, i + 1, j + 1), 1.0),
        ];
        let mut branches: Vec<Arrival> = self
            .node_list(k, i, j)
            .into_iter()
            .filter_map(|a| {
                let mut v = [a; 4];
                for (c, (list, steps)) in corners.iter().enumerate() {
                    v[c + 1] = follow(a, list, steps * self.distance.step)?;
                }
                let lerp = |f: fn(&Arrival) -> f64| {
                    let top = f(&v[0]) + (f(&v[1]) - f(&v[0])) * fx;
                    let bottom = f(&v[2]) + (f(&v[3]) - f(&v[2])) * fx;
                    top + (bottom - top) * fy
                };
                Some(Arrival {
                    time_s: lerp(|a| a.time_s),
                    ray_param: lerp(|a| a.ray_param),
                })
            })
            .collect();
        branches.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
        // No branch comes before the first arrival (each corner's first is its earliest), and
        // the first's own branch lies within a small fraction of a second of it.
        for b in branches {
            if b.time_s - out.last().expect("the first arrival").time_s > self.merge_s {
                out.push(b);
            }
        }
        out
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

    /// The first and later arrivals of phase `k` at a node, earliest first.
    fn node_list(&self, k: usize, i_depth: usize, i_dist: usize) -> Vec<Arrival> {
        let (Some(time_s), Some(ray_param)) = (
            self.node(&self.times[k], i_depth, i_dist),
            self.node(&self.ray_params[k], i_depth, i_dist),
        ) else {
            return Vec::new();
        };
        let cell = (i_depth * self.distance.count + i_dist) as u32;
        let later = &self.later[k];
        let from = later.partition_point(|e| e.0 < cell);
        let to = later.partition_point(|e| e.0 <= cell);
        std::iter::once(Arrival { time_s, ray_param })
            .chain(later[from..to].iter().map(|&(_, t, p)| Arrival {
                time_s: f64::from(t),
                ray_param: f64::from(p),
            }))
            .collect()
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

/// The arrival in `list` on the same branch as `a`, at a node `dx` degrees farther: the nearest
/// in ray parameter and in time (as `a`'s ray parameter predicts it), if within the matching
/// tolerances.
fn follow(a: Arrival, list: &[Arrival], dx: f64) -> Option<Arrival> {
    let predicted = a.time_s + a.ray_param * dx;
    list.iter()
        .map(|b| {
            let dp = (b.ray_param - a.ray_param).abs();
            let dt = (b.time_s - predicted).abs();
            (dp / MATCH_RAY_PARAM + dt / MATCH_TIME_S, dp, dt, *b)
        })
        .filter(|&(_, dp, dt, _)| dp <= MATCH_RAY_PARAM && dt <= MATCH_TIME_S)
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .map(|(.., b)| b)
}
