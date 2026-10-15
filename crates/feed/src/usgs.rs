//! USGS GeoJSON: the summary feeds, parsing their bytes into events, writing events back in the
//! same shape (the remembered past events), and how USGS event types map to source kinds.

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::json;
use sim::{Event, EventKind};

/// The USGS summary feeds the app polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeedKind {
    AllHour,
    AllDay,
    SignificantMonth,
}

impl FeedKind {
    pub const ALL: [FeedKind; 3] = [
        FeedKind::AllHour,
        FeedKind::AllDay,
        FeedKind::SignificantMonth,
    ];

    /// The feed's name in USGS URLs and in cache file names.
    pub fn name(self) -> &'static str {
        match self {
            FeedKind::AllHour => "all_hour",
            FeedKind::AllDay => "all_day",
            FeedKind::SignificantMonth => "significant_month",
        }
    }

    pub fn url(self) -> String {
        format!(
            "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/{}.geojson",
            self.name()
        )
    }
}

/// One event as a USGS feed reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedEvent {
    /// The event, normalised to the project conventions; `kind` comes from [`kind_for`].
    pub event: Event,
    /// The raw USGS `type` ("earthquake", "quarry blast", ...), shown in the UI in place of the
    /// kind.
    pub usgs_type: String,
    /// e.g. "44 km SSE of Glacier View, Alaska".
    pub place: String,
    /// When USGS last changed the event; the newer version wins a dedupe.
    pub updated: DateTime<Utc>,
    /// "automatic" or "reviewed".
    pub status: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("not USGS GeoJSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// The source kind for a USGS event type. Blasts of every sort are explosion-like (the
/// `NuclearTest` profile); slides, avalanches and collapses are landslides; anything not
/// listed (ice quakes, rock bursts, induced quakes, sonic booms, "other event", unknown types)
/// is an earthquake. Decided by the user on 2026-10-14.
pub fn kind_for(usgs_type: &str) -> EventKind {
    match usgs_type.trim().to_ascii_lowercase().as_str() {
        "explosion"
        | "quarry blast"
        | "mining explosion"
        | "chemical explosion"
        | "nuclear explosion"
        | "accidental explosion"
        | "experimental explosion"
        | "industrial explosion" => EventKind::NuclearTest,
        "landslide" | "rockslide" | "rock slide" | "snow avalanche" | "avalanche" | "collapse"
        | "mine collapse" | "building collapse" => EventKind::Landslide,
        "volcanic eruption" | "volcanic explosion" => EventKind::VolcanicTremor,
        "meteorite" | "meteor" => EventKind::Impact,
        _ => EventKind::Earthquake,
    }
}

/// A FeatureCollection (summary feeds, FDSN searches), or a bare Feature (an FDSN lookup by
/// event id).
#[derive(Deserialize)]
#[serde(tag = "type")]
enum Document {
    FeatureCollection { features: Vec<Feature> },
    Feature(Feature),
}

#[derive(Deserialize)]
struct Feature {
    id: String,
    properties: Properties,
    geometry: Option<Geometry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Properties {
    mag: Option<f64>,
    mag_type: Option<String>,
    place: Option<String>,
    /// Milliseconds since the Unix epoch.
    time: Option<i64>,
    updated: Option<i64>,
    /// Comma-wrapped list: ",us6000u2al,aka2026ujmjdz,".
    ids: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct Geometry {
    /// [lon, lat, depth_km].
    coordinates: Vec<Option<f64>>,
}

/// Parse USGS GeoJSON: a FeatureCollection (a summary feed, or an FDSN `format=geojson` search)
/// or a single Feature (an FDSN lookup by `eventid`). Features without a magnitude, an origin
/// time or a full position are skipped: the map cannot draw them.
pub fn parse(bytes: &[u8]) -> Result<Vec<FeedEvent>, ParseError> {
    let features = match serde_json::from_slice(bytes)? {
        Document::FeatureCollection { features } => features,
        Document::Feature(f) => vec![f],
    };
    Ok(features.into_iter().filter_map(feature_event).collect())
}

/// `events` as a USGS-shaped FeatureCollection that [`parse`] reads back to the same events.
/// Only the fields [`parse`] uses are written.
pub fn to_collection<'a>(events: impl IntoIterator<Item = &'a FeedEvent>) -> Vec<u8> {
    let features: Vec<serde_json::Value> = events
        .into_iter()
        .map(|f| {
            let e = &f.event;
            json!({
                "type": "Feature",
                "id": e.id,
                "properties": {
                    "mag": e.magnitude,
                    "magType": e.mag_type,
                    "place": f.place,
                    "time": e.origin.timestamp_millis(),
                    "updated": f.updated.timestamp_millis(),
                    "ids": format!(",{},", e.ids.join(",")),
                    "type": f.usgs_type,
                    "status": f.status,
                },
                "geometry": {"type": "Point", "coordinates": [e.lon, e.lat, e.depth_km]},
            })
        })
        .collect();
    let doc = json!({"type": "FeatureCollection", "features": features});
    serde_json::to_vec_pretty(&doc).expect("JSON values always serialise")
}

fn feature_event(f: Feature) -> Option<FeedEvent> {
    let p = f.properties;
    let coords = f.geometry?.coordinates;
    let (lon, lat, depth) = match coords[..] {
        [Some(lon), Some(lat), Some(depth), ..] => (lon, lat, depth),
        _ => return None,
    };
    let magnitude = p.mag.filter(|m| m.is_finite())?;
    let origin = millis(p.time?)?;
    let usgs_type = p.kind.unwrap_or_else(|| "earthquake".to_owned());
    let mut ids: Vec<String> = p
        .ids
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    ids.dedup();
    let event = Event::new(
        f.id,
        kind_for(&usgs_type),
        origin,
        lat,
        lon,
        depth,
        magnitude,
    )
    .with_mag_type(p.mag_type.unwrap_or_else(|| "?".to_owned()))
    .with_ids(ids);
    Some(FeedEvent {
        updated: p.updated.and_then(millis).unwrap_or(origin),
        event,
        usgs_type,
        place: p.place.unwrap_or_default(),
        status: p.status.unwrap_or_default(),
    })
}

fn millis(ms: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `all_day` download (2026-10-14), plus edge cases.
    const SAMPLE: &str = r#"{
      "type": "FeatureCollection",
      "metadata": {"generated": 1791997876000, "count": 5},
      "features": [
        {"type": "Feature",
         "properties": {"mag": 1.8, "place": "44 km SSE of Glacier View, Alaska",
           "time": 1791997629209, "updated": 1791997730156, "status": "automatic",
           "ids": ",aka2026ujondh,", "magType": "ml", "type": "earthquake",
           "title": "M 1.8 - 44 km SSE of Glacier View, Alaska"},
         "geometry": {"type": "Point", "coordinates": [-147.462, 61.416, 19.1]},
         "id": "aka2026ujondh"},
        {"type": "Feature",
         "properties": {"mag": 4.9, "place": "Fiji region", "time": 1791990000000,
           "updated": 1791995000000, "status": "reviewed",
           "ids": ",us6000u2al,aka2026ujmjdz,", "magType": "mb", "type": "quarry blast"},
         "geometry": {"type": "Point", "coordinates": [180.0, -17.5, -1.2]},
         "id": "us6000u2al"},
        {"type": "Feature",
         "properties": {"mag": null, "time": 1791990000000, "ids": ",x1,", "type": "earthquake"},
         "geometry": {"type": "Point", "coordinates": [0.0, 0.0, 10.0]},
         "id": "x1"},
        {"type": "Feature",
         "properties": {"mag": 2.0, "magType": null, "time": 1791990000000, "ids": null,
           "type": "ice quake"},
         "geometry": {"type": "Point", "coordinates": [10.0, 70.0, 0.5]},
         "id": "x2"},
        {"type": "Feature",
         "properties": {"mag": 2.0, "time": 1791990000000, "ids": ",x3,"},
         "geometry": null,
         "id": "x3"}
      ]
    }"#;

    #[test]
    fn parses_real_shape() {
        let events = parse(SAMPLE.as_bytes()).unwrap();
        // The null-magnitude and geometry-less features are skipped.
        assert_eq!(events.len(), 3);

        let a = &events[0];
        assert_eq!(a.event.id, "aka2026ujondh");
        assert_eq!(a.event.ids, ["aka2026ujondh"]);
        assert_eq!(a.event.lat, 61.416);
        assert_eq!(a.event.lon, -147.462);
        assert_eq!(a.event.depth_km, 19.1);
        assert_eq!(a.event.magnitude, 1.8);
        assert_eq!(a.event.mag_type, "ml");
        assert_eq!(a.event.kind, EventKind::Earthquake);
        assert_eq!(a.event.origin.timestamp_millis(), 1791997629209);
        assert_eq!(a.updated.timestamp_millis(), 1791997730156);
        assert_eq!(a.place, "44 km SSE of Glacier View, Alaska");
        assert_eq!(a.status, "automatic");
    }

    #[test]
    fn normalises_and_keeps_raw_type() {
        let events = parse(SAMPLE.as_bytes()).unwrap();
        let b = &events[1];
        assert_eq!(b.event.ids, ["us6000u2al", "aka2026ujmjdz"]);
        // lon 180 becomes -180; depth above sea level clamps to 0.
        assert_eq!(b.event.lon, -180.0);
        assert_eq!(b.event.depth_km, 0.0);
        assert_eq!(b.usgs_type, "quarry blast");
        assert_eq!(b.event.kind, EventKind::NuclearTest);

        let c = &events[2];
        assert_eq!(c.event.ids, ["x2"]);
        assert_eq!(c.event.mag_type, "?");
        assert_eq!(c.event.kind, EventKind::Earthquake);
        assert_eq!(c.usgs_type, "ice quake");
        // No `updated`: falls back to the origin.
        assert_eq!(c.updated, c.event.origin);
    }

    /// Trimmed from a real FDSN `eventid=usp000hvnu` answer (2026-10-15): a bare Feature whose
    /// preferred id is no longer the one asked for.
    const LOOKUP: &str = r#"{"type":"Feature","properties":{"mag":9.1,
      "place":"2011 Great Tohoku Earthquake, Japan","time":1299822384120,"updated":1790377722510,
      "status":"reviewed","net":"official","code":"20110311054624120_30",
      "ids":",usc0001xgp,official20110311054624120_30,usp000hvnu,choy20110311054623,",
      "magType":"mww","type":"earthquake","title":"M 9.1 - 2011 Great Tohoku Earthquake, Japan",
      "products":{"origin":[{"id":"x"}]}},
      "geometry":{"type":"Point","coordinates":[142.373,38.297,29]},
      "id":"official20110311054624120_30"}"#;

    /// A real FDSN search with no matches (2026-10-15): 200 and an empty collection.
    const EMPTY: &str = r#"{"type":"FeatureCollection","metadata":{"generated":1792047080000,
      "url":"https://earthquake.usgs.gov/fdsnws/event/1/query?format=geojson&starttime=2011-03-01&endtime=2011-03-02&minmagnitude=9.5",
      "title":"USGS Earthquakes","status":200,"api":"2.8.1","count":0},"features":[]}"#;

    #[test]
    fn parses_fdsn_lookup_and_empty_search() {
        let events = parse(LOOKUP.as_bytes()).unwrap();
        assert_eq!(events.len(), 1);
        let e = &events[0].event;
        assert_eq!(e.id, "official20110311054624120_30");
        assert!(e.ids.iter().any(|id| id == "usp000hvnu"));
        assert_eq!(e.magnitude, 9.1);
        assert_eq!(e.mag_type, "mww");
        assert_eq!(e.depth_km, 29.0);
        assert_eq!(e.origin.timestamp_millis(), 1299822384120);
        assert!(parse(EMPTY.as_bytes()).unwrap().is_empty());
    }

    #[test]
    fn written_collection_reads_back_the_same() {
        let mut events = parse(SAMPLE.as_bytes()).unwrap();
        events.extend(parse(LOOKUP.as_bytes()).unwrap());
        let back = parse(&to_collection(&events)).unwrap();
        assert_eq!(back, events);
        assert!(parse(&to_collection([])).unwrap().is_empty());
    }

    #[test]
    fn rejects_non_geojson() {
        assert!(parse(b"<html>503</html>").is_err());
        assert!(parse(br#"{"features": [{"id": 3}]}"#).is_err());
        assert!(
            parse(br#"{"type": "FeatureCollection", "features": []}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn type_mapping() {
        for t in [
            "explosion",
            "quarry blast",
            "Mining Explosion",
            "nuclear explosion",
            "chemical explosion",
            "accidental explosion",
            "experimental explosion",
        ] {
            assert_eq!(kind_for(t), EventKind::NuclearTest, "{t}");
        }
        for t in [
            "landslide",
            "rockslide",
            "snow avalanche",
            "collapse",
            "mine collapse",
        ] {
            assert_eq!(kind_for(t), EventKind::Landslide, "{t}");
        }
        assert_eq!(kind_for("volcanic eruption"), EventKind::VolcanicTremor);
        assert_eq!(kind_for("meteorite"), EventKind::Impact);
        for t in [
            "earthquake",
            "ice quake",
            "rock burst",
            "induced or triggered event",
            "sonic boom",
            "other event",
            "not reported",
            "",
        ] {
            assert_eq!(kind_for(t), EventKind::Earthquake, "{t}");
        }
    }
}
