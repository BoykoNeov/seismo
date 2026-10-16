//! `sim`: the deterministic simulation core — events, geometry, wavefront state at a time.
//! Allowed dependencies: `tables` (re-exported), chrono, serde. No GPU, no network, no async,
//! no I/O, no threads.

pub use tables;

pub mod coast;
pub mod crust;
pub mod ellipticity;
mod event;
pub mod facts;
mod geo;
pub mod mantle;
pub mod radiation;
pub mod raytrace;
pub mod seismogram;
mod simulation;
pub mod slice;
mod station;
pub mod surface;

pub use event::{Event, EventKind, SourceProfile};
pub use geo::{EARTH_RADIUS_KM, azimuth_deg, destination, great_circle_deg, normalise_lon};
pub use radiation::{FaultStyle, FocalMechanism, Radiation};
pub use raytrace::{RayPhase, RayTracer, Wave};
pub use simulation::{EventState, MapEvent, SURFACE_MAP_MIN_MAG, Simulation, Snapshot};
pub use station::{Station, StationError, parse_stations};
