//! `feed`: USGS event fetch, cache and parse, on its own tokio runtime thread, reporting to the
//! app over `crossbeam_channel`. Allowed dependencies: `sim`, tokio, reqwest, serde, chrono,
//! crossbeam-channel, directories. Not used yet (phase 1 has a hardcoded event).
