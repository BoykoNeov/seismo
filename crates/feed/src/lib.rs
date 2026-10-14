//! `feed`: USGS event fetch, cache and parse, on its own tokio runtime thread, reporting to the
//! app over `crossbeam_channel`. Allowed dependencies: `sim`, tokio, reqwest, serde, chrono,
//! crossbeam-channel, directories.
//!
//! Parsing ([`usgs`]), merging ([`store`]) and the cache ([`cache`]) are plain functions and
//! structs, tested without a network; [`live`] is the thread that polls USGS.

pub mod cache;
pub mod live;
pub mod store;
pub mod usgs;

pub use live::{FeedConfig, FeedHandle, FeedMsg, FeedState, FeedStatus, spawn};
pub use store::{EventStore, StoredEvent};
pub use usgs::{FeedEvent, FeedKind, kind_for, parse};
