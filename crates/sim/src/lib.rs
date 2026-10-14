//! `sim`: the deterministic simulation core — events, geometry, wavefront state at a time.
//! Allowed dependencies: `tables` (re-exported), chrono, serde. No GPU, no network, no async,
//! no I/O, no threads.

pub use tables;

mod event;
mod geo;
pub mod raytrace;
mod simulation;
pub mod slice;

pub use event::{Event, EventKind};
pub use geo::{EARTH_RADIUS_KM, great_circle_deg};
pub use raytrace::{RayPhase, RayTracer, Wave};
pub use simulation::{EventState, MapEvent, Simulation, Snapshot};
