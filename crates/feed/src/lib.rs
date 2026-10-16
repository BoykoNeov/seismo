//! `feed`: USGS event fetch, cache and parse, on its own tokio runtime thread, reporting to the
//! app over `crossbeam_channel`. Allowed dependencies: `sim`, tokio, reqwest, serde, chrono,
//! crossbeam-channel, directories.
//!
//! Parsing ([`usgs`]), merging ([`store`]) and the cache ([`cache`]) are plain functions and
//! structs, tested without a network, as are the past-event searches ([`search`]) and the
//! fault-orientation lookups' bookkeeping ([`mechanisms`]); [`live`] is the thread that polls
//! USGS and runs the searches and lookups.

pub mod cache;
pub mod live;
pub mod mechanisms;
pub mod search;
pub mod store;
pub mod usgs;

pub use live::{
    FeedCmd, FeedConfig, FeedHandle, FeedMsg, FeedState, FeedStatus, SearchReply, spawn,
};
pub use search::{MAX_RESULTS, Search, SearchError};
pub use store::{EventStore, StoredEvent};
pub use usgs::{FeedEvent, FeedKind, kind_for, parse, to_collection};
