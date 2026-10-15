//! Events the user makes: what the form holds while typed, and the file they are kept in
//! (`made.json` in the app's data folder) so they are listed at every start, offline too
//! (decided by the user, 2026-10-15).

use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sim::{Event, EventKind};

/// One made event: the source, and the name the user gave it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MadeEvent {
    pub event: Event,
    pub place: String,
}

impl MadeEvent {
    /// Coordinates normalised and depth brought into its kind's range, as [`Event::new`] and the
    /// form would have it (a hand-edited file may say anything).
    fn normalised(self) -> Self {
        let e = self.event;
        let depth = e.kind.profile().clamp_depth(e.depth_km);
        let event = Event::new(e.id, e.kind, e.origin, e.lat, e.lon, depth, e.magnitude);
        Self {
            event,
            place: self.place,
        }
    }
}

const FILE: &str = "made.json";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct MadeFile {
    version: u32,
    events: Vec<MadeEvent>,
}

/// `<platform data dir>/made.json`, beside the USGS cache folder; `None` when the platform has
/// no home directory.
pub fn default_path() -> Option<PathBuf> {
    let usgs = feed::cache::Cache::default_dir()?;
    Some(usgs.parent()?.join(FILE))
}

pub fn parse(bytes: &[u8]) -> Result<Vec<MadeEvent>, String> {
    let file: MadeFile = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if file.version != VERSION {
        return Err(format!("unknown version {}", file.version));
    }
    Ok(file.events.into_iter().map(MadeEvent::normalised).collect())
}

pub fn to_bytes(events: &[MadeEvent]) -> Vec<u8> {
    let file = MadeFile {
        version: VERSION,
        events: events.to_vec(),
    };
    serde_json::to_vec_pretty(&file).expect("plain data serialises")
}

/// The made events kept at `path`; none if there is no file yet.
pub fn load(path: &Path) -> Result<Vec<MadeEvent>, String> {
    match std::fs::read(path) {
        Ok(bytes) => parse(&bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

/// Keep `events` at `path`, replacing what was there in one step (a crash mid-write leaves the
/// old file).
pub fn save(path: &Path, events: &[MadeEvent]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, to_bytes(events))?;
    std::fs::rename(&tmp, path)
}

/// What the user calls each kind of source: made events have no USGS type to show.
pub fn kind_label(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Earthquake => "earthquake",
        EventKind::NuclearTest => "nuclear test",
        EventKind::VolcanicTremor => "volcanic tremor",
        EventKind::Landslide => "landslide",
        EventKind::Impact => "impact",
    }
}

/// The make-your-own-event form's fields, as typed.
pub struct MakeForm {
    pub name: String,
    pub kind: EventKind,
    pub lat: f64,
    pub lon: f64,
    pub depth_km: f64,
    pub magnitude: f64,
    /// UTC origin as typed; empty means the clock's time when the event is made.
    pub time: String,
    /// The next click on the map sets the place.
    pub picking: bool,
    /// The made event the form was filled from, which Update changes.
    pub editing: Option<u64>,
    /// What the last Make/Update/Delete came to, or why it failed.
    pub message: Option<String>,
}

impl Default for MakeForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: EventKind::Earthquake,
            lat: 0.0,
            lon: 0.0,
            depth_km: 10.0,
            magnitude: 6.0,
            time: String::new(),
            picking: false,
            editing: None,
            message: None,
        }
    }
}

/// How origin times are shown in the form and read back.
pub const TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

impl MakeForm {
    /// Take an event's values (to edit it, or to start from a real one).
    pub fn fill(&mut self, event: &Event, place: &str) {
        self.name = place.to_owned();
        self.kind = event.kind;
        self.lat = event.lat;
        self.lon = event.lon;
        self.depth_km = event.kind.profile().clamp_depth(event.depth_km);
        self.magnitude = event.magnitude;
        self.time = event.origin.format(TIME_FORMAT).to_string();
    }

    /// The event the form describes, with origin `clock_now` if no time is typed. The id is
    /// left for the event list to give.
    pub fn build(&self, clock_now: DateTime<Utc>) -> Result<MadeEvent, String> {
        let origin = match self.time.trim() {
            "" => clock_now,
            t => parse_time(t)?,
        };
        let depth = self.kind.profile().clamp_depth(self.depth_km);
        let event = Event::new(
            "",
            self.kind,
            origin,
            self.lat,
            self.lon,
            depth,
            self.magnitude,
        );
        let place = match self.name.trim() {
            "" => format!(
                "Your {} at {:.2}°, {:.2}°",
                kind_label(self.kind),
                event.lat,
                event.lon
            ),
            name => name.to_owned(),
        };
        Ok(MadeEvent { event, place })
    }
}

/// A typed UTC time: `YYYY-MM-DD`, optionally followed by `HH:MM` or `HH:MM:SS` (a `T` may
/// stand for the space, and seconds may have a fraction).
pub fn parse_time(s: &str) -> Result<DateTime<Utc>, String> {
    let s = s.trim().replacen('T', " ", 1);
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M"] {
        if let Ok(t) = NaiveDateTime::parse_from_str(&s, format) {
            return Ok(t.and_utc());
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).expect("midnight exists").and_utc());
    }
    Err(format!(
        "time \"{s}\" is not UTC YYYY-MM-DD HH:MM:SS (or leave it empty for the clock's time)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn parses_typed_times_as_utc() {
        assert_eq!(
            parse_time("2024-01-01 07:10:09"),
            Ok(t(2024, 1, 1, 7, 10, 9))
        );
        assert_eq!(
            parse_time(" 2024-01-01T07:10:09 "),
            Ok(t(2024, 1, 1, 7, 10, 9))
        );
        assert_eq!(parse_time("2024-01-01 07:10"), Ok(t(2024, 1, 1, 7, 10, 0)));
        assert_eq!(parse_time("2024-01-01"), Ok(t(2024, 1, 1, 0, 0, 0)));
        let frac = parse_time("2024-01-01 07:10:09.5").unwrap();
        assert_eq!(frac.timestamp_subsec_millis(), 500);
        assert!(parse_time("01/01/2024").is_err());
        assert!(parse_time("2024-02-30").is_err());
    }

    #[test]
    fn builds_from_the_form() {
        let now = t(2026, 10, 15, 12, 0, 0);
        let mut f = MakeForm {
            kind: EventKind::NuclearTest,
            lat: 41.3,
            lon: 180.0,
            depth_km: 30.0,
            magnitude: 6.3,
            ..Default::default()
        };
        // No time typed: the clock's time. Depth kept in the kind's range, longitude wrapped.
        let m = f.build(now).unwrap();
        assert_eq!(m.event.origin, now);
        assert_eq!(m.event.depth_km, 2.0);
        assert_eq!(m.event.lon, -180.0);
        assert_eq!(m.event.kind, EventKind::NuclearTest);
        assert_eq!(m.place, "Your nuclear test at 41.30°, -180.00°");
        f.name = "  Test blast ".into();
        f.time = "2017-09-03 03:30:01".into();
        let m = f.build(now).unwrap();
        assert_eq!(m.place, "Test blast");
        assert_eq!(m.event.origin, t(2017, 9, 3, 3, 30, 1));
        f.time = "yesterday".into();
        assert!(f.build(now).is_err());
    }

    #[test]
    fn fill_then_build_gives_the_same_event() {
        let e = Event::new(
            "made-3",
            EventKind::VolcanicTremor,
            t(2022, 1, 15, 4, 14, 45),
            -20.5,
            -175.4,
            12.0,
            5.8,
        );
        let mut f = MakeForm::default();
        f.fill(&e, "Hunga Tonga");
        let m = f.build(t(2026, 1, 1, 0, 0, 0)).unwrap();
        assert_eq!(m.place, "Hunga Tonga");
        assert_eq!(
            Event {
                id: e.id.clone(),
                ids: e.ids.clone(),
                ..m.event
            },
            e
        );
    }

    #[test]
    fn file_round_trip_and_bad_files() {
        let e = Event::new(
            "made-1",
            EventKind::Impact,
            t(2013, 2, 15, 3, 20, 33),
            54.8,
            61.1,
            0.0,
            4.2,
        );
        let events = vec![MadeEvent {
            event: e,
            place: "Chelyabinsk-ish".into(),
        }];
        assert_eq!(parse(&to_bytes(&events)), Ok(events.clone()));
        assert!(parse(b"not json").is_err());
        assert!(parse(br#"{"version":99,"events":[]}"#).is_err());
        // A hand-edited depth outside the kind's range is brought back into it.
        let text = String::from_utf8(to_bytes(&events))
            .unwrap()
            .replace("\"depth_km\": 0.0", "\"depth_km\": 40.0");
        assert_eq!(parse(text.as_bytes()).unwrap()[0].event.depth_km, 0.0);
    }

    #[test]
    fn save_and_load_from_disk() {
        let dir = std::env::temp_dir().join(format!("seismo-made-test-{}", std::process::id()));
        let path = dir.join("sub").join(FILE);
        assert_eq!(load(&path), Ok(Vec::new()));
        let events = vec![MadeEvent {
            event: Event::new(
                "made-1",
                EventKind::Earthquake,
                t(2020, 1, 1, 0, 0, 0),
                1.0,
                2.0,
                3.0,
                4.0,
            ),
            place: "x".into(),
        }];
        save(&path, &events).unwrap();
        assert_eq!(load(&path), Ok(events));
        save(&path, &[]).unwrap();
        assert_eq!(load(&path), Ok(Vec::new()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
