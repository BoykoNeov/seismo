//! The bundled assets, embedded so a front end starts with no dependence on the working
//! directory, and the built-in example event.

use chrono::{TimeZone, Utc};
use sim::tables::{Dispersion, EarthModel, TableError, TravelTimeTable};
use sim::{Event, EventKind};

pub static TABLE: &[u8] = include_bytes!("../../../assets/ak135.tt");
pub static MODEL: &[u8] = include_bytes!("../../../assets/ak135_model.csv");
pub static STATIONS: &[u8] = include_bytes!("../../../assets/stations.csv");
pub static COAST: &[u8] = include_bytes!("../../../assets/coastlines.txt");
pub static DISPERSION: &[u8] = include_bytes!("../../../assets/surface_dispersion.csv");

/// The travel-time table, velocity model and surface-wave dispersion, parsed.
pub fn earth() -> Result<(TravelTimeTable, EarthModel, Dispersion), TableError> {
    Ok((
        TravelTimeTable::from_bytes(TABLE)?,
        EarthModel::from_bytes(MODEL)?,
        Dispersion::from_bytes(DISPERSION)?,
    ))
}

/// The built-in example event, there with or without the feed: 2011 Tōhoku.
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
}

/// Where the built-in example happened, for the event list.
pub const TOHOKU_PLACE: &str =
    "2011 Tōhoku, near the east coast of Honshu, Japan (built-in example)";
