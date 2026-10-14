//! The events the app shows: built-in examples plus everything the USGS feed reports, filtered
//! by a minimum magnitude, with the selection tracked by a stable handle so it survives the
//! list being replaced, re-sorted, merged or pruned.

use chrono::{DateTime, Duration, Utc};
use feed::{FeedHandle, FeedMsg, FeedStatus, StoredEvent};
use sim::Event;

/// Identity of an event version: `handle` stays put through new versions and merges, and
/// `revision` changes whenever the event's physics does. Anything computed from an event is
/// keyed by both. Built-in examples use handles from [`BUILTIN_HANDLE`] down; the feed's
/// start at 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventKey {
    pub handle: u64,
    pub revision: u64,
}

pub const BUILTIN_HANDLE: u64 = u64::MAX;

/// Default minimum magnitude shown: smaller events are far too weak to be felt across the
/// globe (decided by the user, 2026-10-14).
pub const DEFAULT_MIN_MAG: f64 = 2.5;

/// When the app picks an event by itself, the largest of this many recent hours wins: their
/// surface waves can still be circling the globe.
const AUTO_PICK_HOURS: i64 = 3;

/// One event in the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub key: EventKey,
    pub event: Event,
    /// The raw USGS type ("earthquake", "quarry blast", ...): shown instead of the source kind.
    pub usgs_type: String,
    pub place: String,
    /// "automatic", "reviewed", or "built-in".
    pub status: String,
    /// The feed no longer lists it (deleted or merged away); kept only while selected.
    pub gone: bool,
}

impl Listed {
    pub fn builtin(event: Event, place: &str) -> Self {
        Self {
            key: EventKey {
                handle: BUILTIN_HANDLE,
                revision: 0,
            },
            event,
            usgs_type: "earthquake".into(),
            place: place.into(),
            status: "built-in".into(),
            gone: false,
        }
    }

    fn from_feed(s: &StoredEvent) -> Self {
        Self {
            key: EventKey {
                handle: s.handle,
                revision: s.revision,
            },
            event: s.feed.event.clone(),
            usgs_type: s.feed.usgs_type.clone(),
            place: s.feed.place.clone(),
            status: s.feed.status.clone(),
            gone: false,
        }
    }
}

pub struct EventList {
    feed: Option<FeedHandle>,
    pub status: Option<FeedStatus>,
    builtins: Vec<Listed>,
    feed_events: Vec<StoredEvent>,
    min_mag: f64,
    selected: u64,
    /// The selected event as last seen, so it stays when the feed drops it.
    selected_copy: Listed,
    /// What the app draws: shown events, newest origin first.
    shown: Vec<Listed>,
    /// Bumped whenever `shown` changes.
    rev: u64,
    /// The user (or a replay flag) has picked an event, so the app never picks one itself.
    pub user_chose: bool,
    /// Feed events have arrived at least once (from the cache or the network).
    pub got_feed: bool,
    /// A USGS id to select as soon as the feed lists it (`--event`).
    pub wanted_id: Option<String>,
}

impl EventList {
    /// `builtins` must not be empty: the first is selected until something else is.
    pub fn new(builtins: Vec<Listed>, feed: Option<FeedHandle>) -> Self {
        let first = builtins
            .first()
            .expect("at least one built-in event")
            .clone();
        let mut list = Self {
            feed,
            status: None,
            builtins,
            feed_events: Vec::new(),
            min_mag: DEFAULT_MIN_MAG,
            selected: first.key.handle,
            selected_copy: first,
            shown: Vec::new(),
            rev: 0,
            user_chose: false,
            got_feed: false,
            wanted_id: None,
        };
        list.rebuild();
        list
    }

    pub fn shown(&self) -> &[Listed] {
        &self.shown
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    pub fn selected_handle(&self) -> u64 {
        self.selected
    }

    pub fn min_mag(&self) -> f64 {
        self.min_mag
    }

    /// Feed events at or above the minimum magnitude, and how many there are in all.
    pub fn feed_counts(&self) -> (usize, usize) {
        let above = self
            .feed_events
            .iter()
            .filter(|s| s.feed.event.magnitude >= self.min_mag)
            .count();
        (above, self.feed_events.len())
    }

    /// Take the feed's news. Returns whether the shown list changed.
    pub fn poll(&mut self, now: DateTime<Utc>) -> bool {
        let mut changed = false;
        while let Some(msg) = self.feed.as_ref().and_then(FeedHandle::try_recv) {
            match msg {
                FeedMsg::Events(events) => changed |= self.take_events(events, now),
                FeedMsg::Status(s) => self.status = Some(s),
            }
        }
        changed
    }

    /// Replace the feed's events. The first non-empty list picks an event, unless the user has.
    fn take_events(&mut self, events: Vec<StoredEvent>, now: DateTime<Utc>) -> bool {
        self.feed_events = events;
        if let Some(id) = &self.wanted_id
            && let Some(s) = self
                .feed_events
                .iter()
                .find(|s| s.feed.event.ids.contains(id))
        {
            self.selected = s.handle;
            self.user_chose = true;
            self.wanted_id = None;
        }
        if !self.got_feed && !self.feed_events.is_empty() {
            self.got_feed = true;
            if !self.user_chose
                && let Some(handle) = self.auto_pick(now)
            {
                self.selected = handle;
            }
        }
        self.rebuild()
    }

    /// The largest event of the last few hours at or above the minimum magnitude, or else the
    /// most recent such event.
    fn auto_pick(&self, now: DateTime<Utc>) -> Option<u64> {
        let candidates = self
            .feed_events
            .iter()
            .filter(|s| s.feed.event.magnitude >= self.min_mag && s.feed.event.origin <= now);
        let recent = now - Duration::hours(AUTO_PICK_HOURS);
        candidates
            .clone()
            .filter(|s| s.feed.event.origin >= recent)
            .max_by(|a, b| a.feed.event.magnitude.total_cmp(&b.feed.event.magnitude))
            .or_else(|| candidates.max_by_key(|s| s.feed.event.origin))
            .map(|s| s.handle)
    }

    pub fn set_min_mag(&mut self, m: f64) -> bool {
        if m == self.min_mag {
            return false;
        }
        self.min_mag = m;
        self.rebuild()
    }

    /// Select the event with `handle`. Returns whether the shown list changed (an event the
    /// feed has dropped leaves it once deselected).
    pub fn select(&mut self, handle: u64) -> bool {
        self.user_chose = true;
        self.selected = handle;
        self.rebuild()
    }

    fn rebuild(&mut self) -> bool {
        let min_mag = self.min_mag;
        let selected = self.selected;
        let mut shown: Vec<Listed> = self
            .builtins
            .iter()
            .cloned()
            .chain(self.feed_events.iter().map(Listed::from_feed))
            .filter(|l| l.event.magnitude >= min_mag || l.key.handle == selected)
            .collect();
        match shown.iter().find(|l| l.key.handle == selected) {
            Some(l) => self.selected_copy = l.clone(),
            None => {
                let mut kept = self.selected_copy.clone();
                kept.gone = true;
                shown.push(kept);
            }
        }
        shown.sort_by(|a, b| {
            b.event
                .origin
                .cmp(&a.event.origin)
                .then(a.key.handle.cmp(&b.key.handle))
        });
        if shown == self.shown {
            return false;
        }
        self.shown = shown;
        self.rev += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use feed::FeedEvent;
    use sim::EventKind;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap()
    }

    fn stored(handle: u64, mins_ago: i64, mag: f64) -> StoredEvent {
        let origin = now() - Duration::minutes(mins_ago);
        StoredEvent {
            handle,
            revision: handle * 10,
            feed: FeedEvent {
                event: Event::new(
                    format!("e{handle}"),
                    EventKind::Earthquake,
                    origin,
                    0.0,
                    0.0,
                    10.0,
                    mag,
                ),
                usgs_type: "earthquake".into(),
                place: String::new(),
                updated: origin,
                status: "automatic".into(),
            },
        }
    }

    fn list() -> EventList {
        let tohoku = Event::new(
            "usp000hvnu",
            EventKind::Earthquake,
            Utc.with_ymd_and_hms(2011, 3, 11, 5, 46, 24).unwrap(),
            38.3,
            142.4,
            29.0,
            9.1,
        );
        EventList::new(vec![Listed::builtin(tohoku, "Tohoku")], None)
    }

    fn handles(l: &EventList) -> Vec<u64> {
        l.shown().iter().map(|e| e.key.handle).collect()
    }

    fn feed_in(l: &mut EventList, events: Vec<StoredEvent>) -> bool {
        l.take_events(events, now())
    }

    #[test]
    fn filters_by_magnitude_newest_first() {
        let mut l = list();
        let rev = l.rev();
        assert!(feed_in(
            &mut l,
            vec![stored(1, 300, 2.0), stored(2, 30, 4.0), stored(3, 10, 3.0)]
        ));
        assert!(l.rev() > rev);
        // Auto-picked the largest recent event; the M2.0 is below the default minimum.
        assert_eq!(l.selected_handle(), 2);
        assert_eq!(handles(&l), [3, 2, BUILTIN_HANDLE]);
        assert_eq!(l.feed_counts(), (2, 3));
        assert!(l.set_min_mag(1.0));
        assert_eq!(handles(&l), [3, 2, 1, BUILTIN_HANDLE]);
        assert!(!l.set_min_mag(1.0));
    }

    #[test]
    fn auto_pick_falls_back_to_most_recent_and_respects_user() {
        let mut l = list();
        feed_in(&mut l, vec![stored(1, 600, 5.0), stored(2, 400, 3.0)]);
        // Nothing in the last 3 hours: the most recent shown event.
        assert_eq!(l.selected_handle(), 2);

        let mut l = list();
        l.select(BUILTIN_HANDLE);
        feed_in(&mut l, vec![stored(1, 10, 6.0)]);
        assert_eq!(l.selected_handle(), BUILTIN_HANDLE);
    }

    #[test]
    fn selected_stays_below_minimum_and_when_dropped() {
        let mut l = list();
        l.select(BUILTIN_HANDLE);
        feed_in(&mut l, vec![stored(1, 10, 2.0), stored(2, 20, 5.0)]);
        // Selected but below the minimum magnitude: still shown.
        l.select(1);
        assert_eq!(handles(&l), [1, 2, BUILTIN_HANDLE]);
        // The feed drops it (deleted): kept, marked gone.
        assert!(feed_in(&mut l, vec![stored(2, 20, 5.0)]));
        assert_eq!(handles(&l), [1, 2, BUILTIN_HANDLE]);
        assert!(l.shown()[0].gone);
        // Selecting another lets it go.
        assert!(l.select(2));
        assert_eq!(handles(&l), [2, BUILTIN_HANDLE]);
    }

    #[test]
    fn new_revision_changes_the_key() {
        let mut l = list();
        l.select(BUILTIN_HANDLE);
        feed_in(&mut l, vec![stored(1, 10, 4.0)]);
        let before = l.shown()[0].key;
        let mut moved = stored(1, 10, 4.0);
        moved.revision += 1;
        moved.feed.event.depth_km = 12.0;
        assert!(feed_in(&mut l, vec![moved]));
        let after = l.shown()[0].key;
        assert_eq!(before.handle, after.handle);
        assert_ne!(before, after);
    }
}
