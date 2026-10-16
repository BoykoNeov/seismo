//! USGS GeoJSON: the summary feeds, parsing their bytes into events, writing events back in the
//! same shape (the remembered past events), and how USGS event types map to source kinds.
//!
//! A summary feed only says which products USGS holds for an event (`types`); the products
//! themselves, among them the fault's orientation, come with a lookup of the one event
//! (`eventid=`), which [`parse`] reads the same way.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::json;
use sim::{Event, EventKind, FocalMechanism};

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
    /// USGS holds a fault orientation for it (a `moment-tensor` or `focal-mechanism` product),
    /// which a lookup of the event brings.
    pub mechanism_listed: bool,
}

/// The USGS products that carry a fault orientation, in order of preference.
const MECHANISM_PRODUCTS: [&str; 2] = ["moment-tensor", "focal-mechanism"];

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
    Feature(Box<Feature>),
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
    /// Comma-wrapped list of the products USGS holds: ",dyfi,moment-tensor,origin,".
    types: Option<String>,
    /// Only in a lookup of one event: the products themselves, by type.
    products: Option<HashMap<String, Vec<Product>>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Product {
    /// The highest weight is the product USGS prefers.
    preferred_weight: Option<f64>,
    /// Everything is a string here, numbers included.
    #[serde(default)]
    properties: HashMap<String, serde_json::Value>,
}

impl Product {
    fn number(&self, key: &str) -> Option<f64> {
        match self.properties.get(key)? {
            serde_json::Value::String(s) => s.trim().parse().ok(),
            serde_json::Value::Number(n) => n.as_f64(),
            _ => None,
        }
        .filter(|v: &f64| v.is_finite())
    }

    /// The first nodal plane (the rake is called slip in some older products).
    fn mechanism(&self) -> Option<FocalMechanism> {
        let rake = self
            .number("nodal-plane-1-rake")
            .or_else(|| self.number("nodal-plane-1-slip"))?;
        Some(FocalMechanism::new(
            self.number("nodal-plane-1-strike")?,
            self.number("nodal-plane-1-dip")?,
            rake,
        ))
    }
}

/// USGS's preferred fault orientation among `products`: the moment tensor with the highest
/// preferred weight that has a nodal plane, else such a focal mechanism.
fn preferred_mechanism(products: &HashMap<String, Vec<Product>>) -> Option<FocalMechanism> {
    MECHANISM_PRODUCTS.iter().find_map(|kind| {
        let mut candidates: Vec<&Product> = products.get(*kind)?.iter().collect();
        // Stable: on equal weights the first listed stays first.
        candidates.sort_by(|a, b| {
            let w = |p: &Product| p.preferred_weight.unwrap_or(0.0);
            w(b).total_cmp(&w(a))
        });
        candidates.into_iter().find_map(Product::mechanism)
    })
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
        Document::Feature(f) => vec![*f],
    };
    Ok(features.into_iter().filter_map(feature_event).collect())
}

/// `events` as a USGS-shaped FeatureCollection that [`parse`] reads back to the same events.
/// Only the fields [`parse`] uses are written; a fault orientation as a one-product
/// `moment-tensor` list.
pub fn to_collection<'a>(events: impl IntoIterator<Item = &'a FeedEvent>) -> Vec<u8> {
    let features: Vec<serde_json::Value> = events
        .into_iter()
        .map(|f| {
            let e = &f.event;
            let types = if f.mechanism_listed {
                ",moment-tensor,"
            } else {
                ""
            };
            let products = e.mechanism.map(|m| {
                json!({"moment-tensor": [{
                    "preferredWeight": 1,
                    "properties": {
                        "nodal-plane-1-strike": m.strike_deg.to_string(),
                        "nodal-plane-1-dip": m.dip_deg.to_string(),
                        "nodal-plane-1-rake": m.rake_deg.to_string(),
                    },
                }]})
            });
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
                    "types": types,
                    "products": products,
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
    let mechanism_listed = p.types.as_deref().is_some_and(|t| {
        t.split(',')
            .any(|product| MECHANISM_PRODUCTS.contains(&product.trim()))
    });
    let mechanism = p.products.as_ref().and_then(preferred_mechanism);
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
    .with_ids(ids)
    .with_mechanism(mechanism);
    Some(FeedEvent {
        updated: p.updated.and_then(millis).unwrap_or(origin),
        event,
        usgs_type,
        place: p.place.unwrap_or_default(),
        status: p.status.unwrap_or_default(),
        mechanism_listed: mechanism_listed || mechanism.is_some(),
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
           "types": ",focal-mechanism,origin,phase-data,",
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

    /// Trimmed from a real FDSN `eventid=usp000hvnu` answer (2026-10-16): the fault products,
    /// listed lower weight first here (USGS lists the preferred first) so the order is not what
    /// decides, plus a heavier moment tensor without a nodal plane, which is passed over.
    const WITH_PRODUCTS: &str = r#"{"type":"Feature","properties":{"mag":9.1,
      "place":"2011 Great Tohoku Earthquake, Japan","time":1299822384120,"updated":1790377722510,
      "status":"reviewed","ids":",usc0001xgp,official20110311054624120_30,usp000hvnu,",
      "magType":"mww","type":"earthquake",
      "types":",dyfi,finite-fault,focal-mechanism,moment-tensor,origin,",
      "products":{
        "moment-tensor":[
          {"source":"us","preferredWeight":68,"properties":{"beachball-source":"US",
            "nodal-plane-1-dip":"14","nodal-plane-1-rake":"81","nodal-plane-1-strike":"193"}},
          {"source":"us","preferredWeight":163,"properties":{"beachball-source":"duputel",
            "derived-magnitude-type":"Mww","nodal-plane-1-dip":"8","nodal-plane-1-rake":"78",
            "nodal-plane-1-strike":"192"}},
          {"source":"xx","preferredWeight":500,"properties":{"scalar-moment":"1e22"}}],
        "focal-mechanism":[
          {"source":"us","preferredWeight":3,"properties":{"beachball-source":"us",
            "nodal-plane-1-dip":"80","nodal-plane-1-rake":"90","nodal-plane-1-strike":"25"}}]}},
      "geometry":{"type":"Point","coordinates":[142.373,38.297,29]},
      "id":"official20110311054624120_30"}"#;

    #[test]
    fn reads_the_preferred_fault_orientation() {
        let e = &parse(WITH_PRODUCTS.as_bytes()).unwrap()[0];
        assert!(e.mechanism_listed);
        assert_eq!(
            e.event.mechanism,
            Some(FocalMechanism::new(192.0, 8.0, 78.0))
        );
        // Only a focal mechanism: that one. Strings with spaces, and the old "slip" name.
        let only_fm = WITH_PRODUCTS.replace("\"moment-tensor\"", "\"other\"");
        let e = &parse(only_fm.as_bytes()).unwrap()[0];
        assert_eq!(
            e.event.mechanism,
            Some(FocalMechanism::new(25.0, 80.0, 90.0))
        );
        let slip = only_fm.replace(
            "\"nodal-plane-1-rake\":\"90\"",
            "\"nodal-plane-1-slip\":\" 90 \"",
        );
        let e = &parse(slip.as_bytes()).unwrap()[0];
        assert_eq!(
            e.event.mechanism,
            Some(FocalMechanism::new(25.0, 80.0, 90.0))
        );
        // A summary feature only lists them; a bare lookup without them has none.
        let events = parse(SAMPLE.as_bytes()).unwrap();
        assert!(events[0].mechanism_listed && events[0].event.mechanism.is_none());
        assert!(!events[1].mechanism_listed);
        let lookup = &parse(LOOKUP.as_bytes()).unwrap()[0];
        assert!(!lookup.mechanism_listed && lookup.event.mechanism.is_none());
    }

    #[test]
    fn written_collection_reads_back_the_same() {
        let mut events = parse(SAMPLE.as_bytes()).unwrap();
        events.extend(parse(LOOKUP.as_bytes()).unwrap());
        events.extend(parse(WITH_PRODUCTS.as_bytes()).unwrap());
        // An odd angle survives the trip through a string.
        events.last_mut().unwrap().event.mechanism = Some(FocalMechanism::new(0.1, 33.3, -0.7));
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
