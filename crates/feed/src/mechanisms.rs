//! Fault orientations looked up event by event: a summary feed only says USGS holds one
//! (`FeedEvent::mechanism_listed`), so the feed thread asks for each such event once and
//! remembers the answer, found or not, in `mechanisms.json` beside the feed cache, so a start
//! without the internet still has them. Pure: which events to ask about, and the remembered
//! answers; the asking is in [`crate::live`].

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sim::{EventKind, FocalMechanism};

use crate::store::{EventStore, StoredEvent};

/// Most lookups between two polls, so a first start with a long list catches up over a few
/// minutes instead of holding up the polls.
pub const MAX_LOOKUPS_PER_POLL: usize = 10;

/// An event USGS keeps updating (felt reports, new magnitudes) is looked up again at most this
/// often, in case its fault orientation was revised.
pub const RECHECK_MINUTES: i64 = 60;

/// One lookup's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checked {
    /// The event's `updated` when it was looked up.
    pub updated: DateTime<Utc>,
    pub mechanism: Option<FocalMechanism>,
}

/// Every lookup's answer, by the event's preferred id at the time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mechanisms {
    by_id: HashMap<String, Checked>,
}

impl Mechanisms {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("plain data serialises")
    }

    /// The latest answer for any of `ids`.
    pub fn get(&self, ids: &[String]) -> Option<&Checked> {
        ids.iter()
            .filter_map(|id| self.by_id.get(id))
            .max_by_key(|c| c.updated)
    }

    pub fn record(&mut self, id: &str, checked: Checked) {
        self.by_id.insert(id.to_owned(), checked);
    }

    /// Give every event in `store` the fault orientation remembered for it. Returns whether
    /// anything changed.
    pub fn attach(&self, store: &mut EventStore) -> bool {
        let found: Vec<(u64, FocalMechanism)> = store
            .events()
            .iter()
            .filter_map(|r| Some((r.handle, self.get(&r.feed.event.ids)?.mechanism?)))
            .collect();
        let mut changed = false;
        for (handle, m) in found {
            changed |= store.set_mechanism(handle, m);
        }
        changed
    }

    /// Whether `r`'s answer is still good: looked up at its current version (or later).
    fn up_to_date(&self, r: &StoredEvent) -> bool {
        self.get(&r.feed.event.ids)
            .is_some_and(|c| c.updated >= r.feed.updated)
    }
}

/// The listed earthquakes to look up now, largest first, at most [`MAX_LOOKUPS_PER_POLL`]:
/// USGS lists a fault orientation for them (or they are kept: kept events saved before the
/// listing was recorded do not say, so each is asked about once), the remembered answer (if
/// any) is for an older version, and `last_tried` (handle → when this session last asked) is
/// not within [`RECHECK_MINUTES`].
pub fn wanted(
    store: &EventStore,
    known: &Mechanisms,
    last_tried: &HashMap<u64, DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Vec<StoredEvent> {
    let mut out: Vec<StoredEvent> = store
        .events()
        .iter()
        .filter(|r| r.listed() && (r.feed.mechanism_listed || r.saved))
        .filter(|r| r.feed.event.kind == EventKind::Earthquake)
        .filter(|r| !known.up_to_date(r))
        .filter(|r| {
            last_tried
                .get(&r.handle)
                .is_none_or(|&t| now - t >= Duration::minutes(RECHECK_MINUTES))
        })
        .cloned()
        .collect();
    out.sort_by(|a, b| b.feed.event.magnitude.total_cmp(&a.feed.event.magnitude));
    out.truncate(MAX_LOOKUPS_PER_POLL);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usgs::{FeedEvent, FeedKind};
    use chrono::TimeZone;
    use sim::Event;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap()
    }

    fn ev(id: &str, mag: f64, listed: bool, kind: EventKind) -> FeedEvent {
        let origin = now() - Duration::hours(1);
        FeedEvent {
            event: Event::new(id, kind, origin, 0.0, 0.0, 10.0, mag),
            usgs_type: "earthquake".into(),
            place: String::new(),
            updated: origin,
            status: "reviewed".into(),
            mechanism_listed: listed,
        }
    }

    fn store(events: Vec<FeedEvent>) -> EventStore {
        let mut s = EventStore::new();
        s.apply(FeedKind::AllDay, events, now());
        s
    }

    fn ids(v: &[StoredEvent]) -> Vec<&str> {
        v.iter().map(|r| r.feed.event.id.as_str()).collect()
    }

    #[test]
    fn asks_about_listed_earthquakes_largest_first() {
        let s = store(vec![
            ev("small", 4.5, true, EventKind::Earthquake),
            ev("none", 7.0, false, EventKind::Earthquake),
            ev("big", 6.8, true, EventKind::Earthquake),
            ev("blast", 5.0, true, EventKind::NuclearTest),
        ]);
        let got = wanted(&s, &Mechanisms::default(), &HashMap::new(), now());
        assert_eq!(ids(&got), ["big", "small"]);

        // Many at once: the largest ten.
        let many = store(
            (0..15)
                .map(|k| {
                    ev(
                        &format!("e{k}"),
                        4.0 + k as f64 * 0.1,
                        true,
                        EventKind::Earthquake,
                    )
                })
                .collect(),
        );
        let got = wanted(&many, &Mechanisms::default(), &HashMap::new(), now());
        assert_eq!(got.len(), MAX_LOOKUPS_PER_POLL);
        assert_eq!(got[0].feed.event.id, "e14");
    }

    #[test]
    fn remembered_answers_and_recent_tries_are_not_asked_again() {
        let fault = FocalMechanism::new(10.0, 20.0, 30.0);
        let mut s = store(vec![
            ev("a", 6.0, true, EventKind::Earthquake),
            ev("b", 6.0, true, EventKind::Earthquake),
        ]);
        let mut known = Mechanisms::default();
        let updated = s.events()[0].feed.updated;
        known.record(
            "a",
            Checked {
                updated,
                mechanism: Some(fault),
            },
        );
        // "b" was found to have none; that answer counts too.
        known.record(
            "b",
            Checked {
                updated,
                mechanism: None,
            },
        );
        assert!(wanted(&s, &known, &HashMap::new(), now()).is_empty());
        assert!(known.attach(&mut s));
        assert!(!known.attach(&mut s));
        assert_eq!(s.events()[0].feed.event.mechanism, Some(fault));
        assert_eq!(s.events()[1].feed.event.mechanism, None);

        // A newer version of "b": asked again, unless this session asked within the hour.
        let mut newer = ev("b", 6.0, true, EventKind::Earthquake);
        newer.updated = updated + Duration::minutes(5);
        // (Through the hour feed: a day feed without "a" would delete it.)
        s.apply(FeedKind::AllHour, vec![newer], now());
        let handle_b = s.events()[1].handle;
        assert_eq!(ids(&wanted(&s, &known, &HashMap::new(), now())), ["b"]);
        let recently = HashMap::from([(handle_b, now() - Duration::minutes(10))]);
        assert!(wanted(&s, &known, &recently, now()).is_empty());
        let long_ago = HashMap::from([(handle_b, now() - Duration::minutes(RECHECK_MINUTES))]);
        assert_eq!(ids(&wanted(&s, &known, &long_ago, now())), ["b"]);
    }

    /// Kept events saved before faults were looked up carry no product list: asked once.
    #[test]
    fn kept_events_without_a_product_list_are_asked_once() {
        let mut s = EventStore::new();
        s.apply_saved(vec![ev("kept", 8.3, false, EventKind::Earthquake)]);
        let mut known = Mechanisms::default();
        assert_eq!(ids(&wanted(&s, &known, &HashMap::new(), now())), ["kept"]);
        known.record(
            "kept",
            Checked {
                updated: s.events()[0].feed.updated,
                mechanism: None,
            },
        );
        assert!(wanted(&s, &known, &HashMap::new(), now()).is_empty());
    }

    #[test]
    fn answers_found_under_any_id_and_kept_on_disk() {
        let fault = FocalMechanism::new(10.0, 20.0, 30.0);
        let mut known = Mechanisms::default();
        let t = now();
        known.record(
            "old",
            Checked {
                updated: t - Duration::hours(1),
                mechanism: None,
            },
        );
        known.record(
            "new",
            Checked {
                updated: t,
                mechanism: Some(fault),
            },
        );
        let ids = ["x".to_owned(), "old".to_owned(), "new".to_owned()];
        assert_eq!(known.get(&ids).unwrap().mechanism, Some(fault));
        assert_eq!(known.get(&["y".to_owned()]), None);
        let back = Mechanisms::from_bytes(&known.to_bytes()).unwrap();
        assert_eq!(back, known);
        assert!(Mechanisms::from_bytes(b"not json").is_err());
    }
}
