//! `tables`: the ak135 travel-time table and velocity model — file format, parsing, interpolation.
//! Allowed dependencies: serde, serde_json, thiserror, bytemuck. No GPU, no network, no threads.

mod model;
mod table;

pub use model::{EarthModel, ModelPoint};
pub use table::{Axis, FORMAT_VERSION, GPU_MISSING, MAGIC, TravelTimeTable};

use thiserror::Error;

/// Errors from parsing a table or model file.
#[derive(Debug, Error)]
pub enum TableError {
    #[error("not a SEISMO travel-time table (bad magic)")]
    BadMagic,
    #[error("unsupported table format version {0} (expected {FORMAT_VERSION})")]
    UnsupportedVersion(u32),
    #[error("file truncated: {0}")]
    Truncated(&'static str),
    #[error("bad table header: {0}")]
    Header(#[from] serde_json::Error),
    #[error("inconsistent table: {0}")]
    Inconsistent(String),
    #[error("bad velocity model: line {line}: {msg}")]
    Model { line: usize, msg: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
