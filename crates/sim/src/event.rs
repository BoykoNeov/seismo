//! Seismic source events.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What produced the waves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    Earthquake,
    NuclearTest,
    VolcanicTremor,
    Landslide,
    Impact,
}

/// A source: where, when, how big.
///
/// Built through [`Event::new`], which normalises coordinates to the project conventions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Preferred id.
    pub id: String,
    /// Every id the source knows this event by (USGS network ids); used for dedupe.
    pub ids: Vec<String>,
    pub kind: EventKind,
    pub origin: DateTime<Utc>,
    /// Degrees, [-90, 90].
    pub lat: f64,
    /// Degrees, [-180, 180).
    pub lon: f64,
    /// km below the surface, positive down, >= 0.
    pub depth_km: f64,
    pub magnitude: f64,
    /// Magnitude type as given by the source, e.g. "Mw", "mb".
    pub mag_type: String,
}

impl Event {
    /// A new event with `ids = [id]` and magnitude type "Mw".
    ///
    /// Negative depth (above sea level) is clamped to 0, latitude is clamped to [-90, 90] and
    /// longitude is wrapped into [-180, 180) (so 180 becomes -180).
    pub fn new(
        id: impl Into<String>,
        kind: EventKind,
        origin: DateTime<Utc>,
        lat: f64,
        lon: f64,
        depth_km: f64,
        magnitude: f64,
    ) -> Self {
        let id = id.into();
        Self {
            ids: vec![id.clone()],
            id,
            kind,
            origin,
            lat: lat.clamp(-90.0, 90.0),
            lon: normalize_lon(lon),
            depth_km: depth_km.max(0.0),
            magnitude,
            mag_type: "Mw".to_owned(),
        }
    }

    pub fn with_mag_type(mut self, mag_type: impl Into<String>) -> Self {
        self.mag_type = mag_type.into();
        self
    }

    /// Replace the id list; the preferred id is added if missing.
    pub fn with_ids(mut self, ids: Vec<String>) -> Self {
        self.ids = ids;
        if !self.ids.contains(&self.id) {
            self.ids.insert(0, self.id.clone());
        }
        self
    }

    /// Seconds since origin at `now` (negative before the event).
    pub fn t_at(&self, now: DateTime<Utc>) -> f64 {
        let d = now - self.origin;
        // Microsecond precision is plenty and cannot overflow for any plausible span.
        d.num_microseconds()
            .map(|us| us as f64 * 1e-6)
            .unwrap_or_else(|| d.num_milliseconds() as f64 * 1e-3)
    }
}

fn normalize_lon(lon: f64) -> f64 {
    let l = (lon + 180.0).rem_euclid(360.0) - 180.0;
    // rem_euclid can return exactly 360.0 for tiny negative inputs due to rounding.
    if l >= 180.0 { l - 360.0 } else { l }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn origin() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2011, 3, 11, 5, 46, 24).unwrap()
    }

    fn ev(lat: f64, lon: f64, depth: f64) -> Event {
        Event::new("x", EventKind::Earthquake, origin(), lat, lon, depth, 5.0)
    }

    #[test]
    fn clamps_negative_depth() {
        assert_eq!(ev(0.0, 0.0, -1.2).depth_km, 0.0);
        assert_eq!(ev(0.0, 0.0, 29.0).depth_km, 29.0);
    }

    #[test]
    fn normalises_longitude() {
        assert_eq!(ev(0.0, 180.0, 0.0).lon, -180.0);
        assert_eq!(ev(0.0, -180.0, 0.0).lon, -180.0);
        assert_eq!(ev(0.0, 179.5, 0.0).lon, 179.5);
        assert!((ev(0.0, 190.0, 0.0).lon - -170.0).abs() < 1e-12);
        assert!((ev(0.0, -190.0, 0.0).lon - 170.0).abs() < 1e-12);
        let tiny = ev(0.0, -1e-17, 0.0).lon;
        assert!((-180.0..180.0).contains(&tiny), "{tiny}");
    }

    #[test]
    fn clamps_latitude() {
        assert_eq!(ev(91.0, 0.0, 0.0).lat, 90.0);
        assert_eq!(ev(-95.0, 0.0, 0.0).lat, -90.0);
    }

    #[test]
    fn ids_and_mag_type() {
        let e = ev(0.0, 0.0, 0.0);
        assert_eq!(e.ids, vec!["x".to_owned()]);
        assert_eq!(e.mag_type, "Mw");
        let e = e.with_ids(vec!["a".into(), "b".into()]).with_mag_type("mb");
        assert_eq!(e.ids, vec!["x".to_owned(), "a".into(), "b".into()]);
        assert_eq!(e.mag_type, "mb");
    }

    #[test]
    fn time_since_origin() {
        let e = ev(0.0, 0.0, 0.0);
        assert_eq!(e.t_at(origin()), 0.0);
        assert_eq!(e.t_at(origin() + chrono::Duration::milliseconds(1500)), 1.5);
        assert_eq!(e.t_at(origin() - chrono::Duration::seconds(60)), -60.0);
    }
}
