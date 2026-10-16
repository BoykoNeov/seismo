//! `session`: everything a front end needs that is not drawing. The sim clock, the event list
//! (built-in, USGS and user-made events, and which one is selected), the user's own events on
//! disk, the virtual seismograph traces and their sound, the guide's text, and the embedded
//! assets. Shared by the desktop app and the terminal version, so both behave the same.
//! Allowed dependencies: `sim`, `feed`, chrono, serde. No GPU, no UI framework.

pub mod assets;
pub mod clock;
pub mod events;
pub mod guide;
pub mod made;
pub mod sound;
pub mod traces;
