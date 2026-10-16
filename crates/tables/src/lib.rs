//! `tables`: the ak135 travel-time table, velocity model and surface-wave dispersion, and the
//! SP12RTS 3D mantle perturbations — file formats, parsing, interpolation.
//! Allowed dependencies: serde, serde_json, thiserror, bytemuck. No GPU, no network, no threads.

mod dispersion;
mod mantle;
mod model;
mod table;

pub use dispersion::{Dispersion, SurfaceWave};
pub use mantle::{MantleModel, SPH_R_BOTTOM_KM, SPH_R_TOP_KM, SphModel};
pub use model::{EarthModel, ModelPoint};
pub use table::{Arrival, Axis, FORMAT_VERSION, GPU_MISSING, MAGIC, TravelTimeTable};

use thiserror::Error;

/// Errors from parsing a table or model file.
#[derive(Debug, Error)]
pub enum TableError {
    #[error("not a SEISMO travel-time table (bad magic)")]
    BadMagic,
    #[error("unsupported table format version {0} (expected 1 to {FORMAT_VERSION})")]
    UnsupportedVersion(u32),
    #[error("file truncated: {0}")]
    Truncated(&'static str),
    #[error("bad table header: {0}")]
    Header(#[from] serde_json::Error),
    #[error("inconsistent table: {0}")]
    Inconsistent(String),
    #[error("bad velocity model: line {line}: {msg}")]
    Model { line: usize, msg: String },
    #[error("bad surface-wave dispersion: line {line}: {msg}")]
    Dispersion { line: usize, msg: String },
    #[error("bad 3D mantle model: {0}")]
    Sph(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
