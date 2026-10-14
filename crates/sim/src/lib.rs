//! `sim`: the deterministic simulation core — events, geometry, wavefront state at a time.
//! Allowed dependencies: `tables` (re-exported), chrono, serde. No GPU, no network, no async,
//! no I/O, no threads.

pub use tables;

mod event;
mod geo;
pub mod mantle;
pub mod raytrace;
pub mod seismogram;
mod simulation;
pub mod slice;
mod station;

pub use event::{Event, EventKind, SourceProfile};
pub use geo::{EARTH_RADIUS_KM, azimuth_deg, destination, great_circle_deg, normalise_lon};
pub use raytrace::{RayPhase, RayTracer, Wave};
pub use simulation::{EventState, MapEvent, Simulation, Snapshot};
pub use station::{Station, StationError, parse_stations};
