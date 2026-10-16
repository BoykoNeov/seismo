//! The bundled assets, embedded so a front end starts with no dependence on the working
//! directory, and the built-in example event.

use chrono::{TimeZone, Utc};
use sim::crust::Crust;
use sim::tables::{CrustModel, Dispersion, EarthModel, TableError, TravelTimeTable};
use sim::{Event, EventKind, FocalMechanism};

pub static TABLE: &[u8] = include_bytes!("../../../assets/ak135.tt");
pub static MODEL: &[u8] = include_bytes!("../../../assets/ak135_model.csv");
pub static STATIONS: &[u8] = include_bytes!("../../../assets/stations.csv");
pub static COAST: &[u8] = include_bytes!("../../../assets/coastlines.txt");
pub static DISPERSION: &[u8] = include_bytes!("../../../assets/surface_dispersion.csv");
pub static CRUST: &[u8] = include_bytes!("../../../assets/crust1.bin");

/// The travel-time table, velocity model and surface-wave dispersion, parsed.
pub fn earth() -> Result<(TravelTimeTable, EarthModel, Dispersion), TableError> {
    Ok((
        TravelTimeTable::from_bytes(TABLE)?,
        EarthModel::from_bytes(MODEL)?,
        Dispersion::from_bytes(DISPERSION)?,
    ))
}

/// The real crust (CRUST1.0), set against `model`'s own crust. An error if the file does not
/// parse or the model has no Moho.
pub fn crust(model: &EarthModel) -> Result<Crust, TableError> {
    Crust::new(CrustModel::from_bytes(CRUST)?, model)
        .ok_or_else(|| TableError::Crust("the velocity model has no Moho".into()))
}

/// The built-in example event, there with or without the feed: 2011 Tōhoku, with the fault
/// USGS prefers for it (W-phase moment tensor, Duputel; first nodal plane of the
/// `moment-tensor` product of event usp000hvnu, read 2026-10-16): a thrust on the shallow plate
/// boundary dipping west under Japan.
pub fn tohoku() -> Event {
    let origin = Utc.with_ymd_and_hms(2011, 3, 11, 5, 46, 24).unwrap();
    Event::new(
        "usp000hvnu",
        EventKind::Earthquake,
        origin,
        38.297,
        142.373,
        29.0,
        9.1,
    )
    .with_mechanism(Some(FocalMechanism::new(192.0, 8.0, 78.0)))
}

/// Where the built-in example happened, for the event list.
pub const TOHOKU_PLACE: &str =
    "2011 Tōhoku, near the east coast of Honshu, Japan (built-in example)";
