//! The events the app shows: built-in examples plus everything the USGS feed reports, filtered
//! by a minimum magnitude, plus past events the user picked from a search and events the user
//! made (both kept across restarts), with the selection tracked by a stable handle so it
//! survives the list being replaced, re-sorted, merged or pruned.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use feed::{FeedCmd, FeedHandle, FeedMsg, FeedStatus, Search, StoredEvent};
use sim::Event;

use crate::made::{MadeEvent, kind_label};

/// Identity of an event version: `handle` stays put through new versions and merges, and
/// `revision` changes whenever the event's physics does. Anything computed from an event is
/// keyed by both. Built-in examples use handles from [`BUILTIN_HANDLE`] down, the user's own
/// events from [`MADE_HANDLE`] up; the feed's start at 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventKey {
    pub handle: u64,
    pub revision: u64,
}

pub const BUILTIN_HANDLE: u64 = u64::MAX;
pub const MADE_HANDLE: u64 = 1 << 63;

/// Default minimum magnitude shown: smaller events are far too weak to be felt across the
/// globe (decided by the user, 2026-10-14).
pub const DEFAULT_MIN_MAG: f64 = 2.5;

/// When the app picks an event by itself, the largest of this many recent hours wins: their
/// surface waves can still be circling the globe.
const AUTO_PICK_HOURS: i64 = 3;

/// Until the user picks an event, a newly listed event at least this big (and from the last
/// [`AUTO_PICK_HOURS`]) becomes the selected one (decided by the user, 2026-10-14).
pub const FOLLOW_MIN_MAG: f64 = 6.0;

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
    /// Kept by the user (a picked past event): listed whatever its magnitude.
    pub saved: bool,
    /// Made by the user: listed whatever its magnitude, kept across restarts.
    pub made: bool,
    /// No longer in the list (deleted by USGS, merged away, or forgotten); kept only while
    /// selected.
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
            saved: false,
            made: false,
            gone: false,
        }
    }

    fn made(key: EventKey, m: MadeEvent) -> Self {
        Self {
            key,
            usgs_type: kind_label(m.event.kind).into(),
            event: m.event,
            place: m.place,
            status: "made by you".into(),
            saved: false,
            made: true,
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
            saved: s.saved,
            made: false,
            gone: false,
        }
    }
}

/// The past-event search as the app sees it.
#[derive(Debug, Default)]
pub struct SearchView {
    /// A search is on its way.
    pub busy: bool,
    /// The last search's results, in USGS's order (largest first).
    pub results: Vec<StoredEvent>,
    /// What the last search came to ("12 found", or why it failed).
    pub message: Option<String>,
}

pub struct EventList {
    feed: Option<FeedHandle>,
    pub status: Option<FeedStatus>,
    builtins: Vec<Listed>,
    feed_events: Vec<StoredEvent>,
    /// The user's own events, oldest made first.
    made: Vec<Listed>,
    /// Handle and revision for the next made event or edit.
    next_made: u64,
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
    /// The first automatic pick has been made.
    auto_picked: bool,
    /// Feed handles already listed when the selection last settled: anything else is new.
    seen: HashSet<u64>,
    /// A USGS id to select as soon as the feed lists it or a lookup finds it (`--event`).
    pub wanted_id: Option<String>,
    pub search: SearchView,
    /// A search result was picked: the app should replay it from its origin.
    replay: bool,
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
            made: Vec::new(),
            next_made: MADE_HANDLE,
            min_mag: DEFAULT_MIN_MAG,
            selected: first.key.handle,
            selected_copy: first,
            shown: Vec::new(),
            rev: 0,
            user_chose: false,
            auto_picked: false,
            seen: HashSet::new(),
            wanted_id: None,
            search: SearchView::default(),
            replay: false,
        };
        list.rebuild();
        list
    }

    /// One line on how the USGS feed is doing, for the status display.
    pub fn feed_line(&self) -> String {
        let Some(status) = &self.status else {
            return "USGS feed: starting…".to_owned();
        };
        let ago = |t: DateTime<Utc>| {
            let s = (Utc::now() - t).num_seconds().max(0);
            if s < 120 {
                format!("{s} s ago")
            } else if s < 2 * 3600 {
                format!("{} min ago", s / 60)
            } else {
                format!("{} UTC", t.format("%Y-%m-%d %H:%M"))
            }
        };
        let latest = |f: fn(&feed::FeedState) -> Option<DateTime<Utc>>| {
            status.feeds.iter().filter_map(f).max()
        };
        let data = latest(|s| s.data_from);
        let saved = data.map_or("no saved copy yet".to_owned(), |t| {
            format!("saved copy from {}", ago(t))
        });
        if status.offline {
            return format!("Offline: {saved}");
        }
        let error = status.feeds.iter().find_map(|s| s.error.as_deref());
        match (latest(|s| s.last_ok), error) {
            (Some(ok), None) => format!("USGS live: checked {}", ago(ok)),
            (Some(ok), Some(e)) => format!("USGS reached {}; now failing: {e}", ago(ok)),
            (None, Some(e)) => format!("USGS unreachable ({e}); {saved}"),
            (None, None) => format!("USGS: connecting… ({saved})"),
        }
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

    /// Live feed events at or above the minimum magnitude, and how many there are in all.
    pub fn feed_counts(&self) -> (usize, usize) {
        let live = self.feed_events.iter().filter(|s| s.live);
        let above = live
            .clone()
            .filter(|s| s.feed.event.magnitude >= self.min_mag)
            .count();
        (above, live.count())
    }

    /// Whether a search result was picked since the last call: the app replays it from its
    /// origin (decided by the user, 2026-10-15).
    pub fn take_replay(&mut self) -> bool {
        std::mem::take(&mut self.replay)
    }

    /// Start a past-event search; the results arrive through [`Self::poll`]. The previous
    /// results go at once: the feed drops the ones the new search does not find again, so
    /// picking one meanwhile could not keep it.
    pub fn search(&mut self, search: Search) {
        self.search.results.clear();
        let sent = self
            .feed
            .as_ref()
            .is_some_and(|f| f.send(FeedCmd::Search(search)));
        self.search.busy = sent;
        self.search.message = (!sent).then(|| "the USGS feed is not running".to_owned());
    }

    /// Select the event with id `id` (`--event`): a built-in one or one the user made at once,
    /// otherwise (a USGS id) as soon as the feed lists it or a lookup finds it.
    pub fn want(&mut self, id: String) {
        if let Some(b) = self
            .builtins
            .iter()
            .chain(&self.made)
            .find(|b| b.event.ids.contains(&id))
        {
            let handle = b.key.handle;
            self.select(handle);
            return;
        }
        self.search(Search::Id(id.clone()));
        self.wanted_id = Some(id);
    }

    /// Pick a search result: it is kept (listed from now on, across restarts), selected, and
    /// replayed. A result that is a built-in example selects that instead. Returns whether the
    /// shown list changed.
    pub fn pick_result(&mut self, handle: u64) -> bool {
        let Some(r) = self.search.results.iter().find(|r| r.handle == handle) else {
            return false;
        };
        self.replay = true;
        let ids = &r.feed.event.ids;
        if let Some(b) = self
            .builtins
            .iter()
            .find(|b| b.event.ids.iter().any(|id| ids.contains(id)))
        {
            let handle = b.key.handle;
            return self.select(handle);
        }
        let mut kept = r.clone();
        kept.saved = true;
        if let Some(f) = &self.feed {
            f.send(FeedCmd::Keep(handle));
        }
        // Listed at once; the feed's next list says the same.
        match self.feed_events.iter_mut().find(|s| s.handle == handle) {
            Some(s) => s.saved = true,
            None => self.feed_events.push(kept),
        }
        for r in &mut self.search.results {
            if r.handle == handle {
                r.saved = true;
            }
        }
        self.select(handle)
    }

    /// Stop keeping the event with `handle`. It leaves the list unless the live feed lists it
    /// (or it is selected). Returns whether the shown list changed.
    pub fn forget(&mut self, handle: u64) -> bool {
        if let Some(f) = &self.feed {
            f.send(FeedCmd::Forget(handle));
        }
        for s in self
            .feed_events
            .iter_mut()
            .chain(self.search.results.iter_mut())
        {
            if s.handle == handle {
                s.saved = false;
            }
        }
        self.feed_events.retain(StoredEvent::listed);
        self.rebuild()
    }

    /// Take the feed's news. Returns whether the shown list changed.
    pub fn poll(&mut self, now: DateTime<Utc>) -> bool {
        let mut fresh = false;
        let mut found = false;
        while let Some(msg) = self.feed.as_ref().and_then(FeedHandle::try_recv) {
            match msg {
                FeedMsg::Events(events) => {
                    self.take_events(events);
                    fresh = true;
                }
                FeedMsg::Status(s) => self.status = Some(s),
                FeedMsg::Found(reply) => {
                    self.take_results(reply.result);
                    found = true;
                }
            }
        }
        let picked = self.settle(now, fresh);
        if fresh || picked || found {
            self.rebuild()
        } else {
            false
        }
    }

    /// Take a search's results, picking the `--event` one if it is among them.
    fn take_results(&mut self, result: Result<Vec<StoredEvent>, String>) {
        self.search.busy = false;
        match result {
            Ok(found) => {
                self.search.message = Some(match found.len() {
                    0 => "nothing found".to_owned(),
                    1 => "1 found".to_owned(),
                    n => format!("{n} found"),
                });
                self.search.results = found;
                let wanted = self.wanted_id.as_ref().and_then(|id| {
                    self.search
                        .results
                        .iter()
                        .find(|r| r.feed.event.ids.contains(id))
                        .map(|r| r.handle)
                });
                if let Some(handle) = wanted {
                    self.wanted_id = None;
                    self.pick_result(handle);
                }
            }
            Err(e) => self.search.message = Some(e),
        }
    }

    /// Replace the feed's events, selecting `wanted_id` if it is among them.
    fn take_events(&mut self, events: Vec<StoredEvent>) {
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
    }

    /// Let the app pick the event while the user has not. The first pick waits until the list is
    /// as fresh as it will get: USGS has answered, or the app is offline, or USGS is unreachable
    /// and the saved copy is all there is (picking from a saved copy that a download is about to
    /// replace would choose from stale events). After that, each `fresh` list switches to the
    /// largest newly listed event of [`FOLLOW_MIN_MAG`] or more from the last few hours.
    /// Returns whether the selection changed.
    fn settle(&mut self, now: DateTime<Utc>, fresh: bool) -> bool {
        let settled = self.status.as_ref().is_some_and(|s| {
            let answered = s.feeds.iter().any(|f| f.last_ok.is_some());
            let failing = s.feeds.iter().any(|f| f.error.is_some());
            s.offline || answered || failing
        });
        if !settled || self.feed_events.is_empty() || (self.auto_picked && !fresh) {
            return false;
        }
        let pick = if self.user_chose {
            None
        } else if !self.auto_picked {
            self.auto_pick(now)
        } else {
            let recent = now - Duration::hours(AUTO_PICK_HOURS);
            self.feed_events
                .iter()
                .filter(|s| s.live && !self.seen.contains(&s.handle))
                .filter(|s| {
                    s.feed.event.magnitude >= FOLLOW_MIN_MAG && s.feed.event.origin >= recent
                })
                .max_by(|a, b| a.feed.event.magnitude.total_cmp(&b.feed.event.magnitude))
                .map(|s| s.handle)
        };
        self.auto_picked = true;
        self.seen = self.feed_events.iter().map(|s| s.handle).collect();
        match pick {
            Some(handle) if handle != self.selected => {
                self.selected = handle;
                true
            }
            _ => false,
        }
    }

    /// The largest live event of the last few hours at or above the minimum magnitude, or else
    /// the most recent such event. Kept past events are never picked by the app.
    fn auto_pick(&self, now: DateTime<Utc>) -> Option<u64> {
        let candidates = self.feed_events.iter().filter(|s| {
            s.live && s.feed.event.magnitude >= self.min_mag && s.feed.event.origin <= now
        });
        let recent = now - Duration::hours(AUTO_PICK_HOURS);
        candidates
            .clone()
            .filter(|s| s.feed.event.origin >= recent)
            .max_by(|a, b| a.feed.event.magnitude.total_cmp(&b.feed.event.magnitude))
            .or_else(|| candidates.max_by_key(|s| s.feed.event.origin))
            .map(|s| s.handle)
    }

    /// List `events` the user made earlier (read back at start). Selects nothing.
    pub fn load_made(&mut self, events: Vec<MadeEvent>) -> bool {
        for m in events {
            let key = self.next_made_key();
            self.made.push(Listed::made(key, m));
        }
        self.rebuild()
    }

    /// The user's events, to keep on disk.
    pub fn made(&self) -> Vec<MadeEvent> {
        self.made
            .iter()
            .map(|l| MadeEvent {
                event: l.event.clone(),
                place: l.place.clone(),
            })
            .collect()
    }

    /// List and select an event the user made; it gets a new id. Never moves the clock.
    /// Returns its handle.
    pub fn make(&mut self, mut m: MadeEvent) -> u64 {
        let n = self
            .made
            .iter()
            .filter_map(|l| l.event.id.strip_prefix("made-")?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let id = format!("made-{n}");
        m.event.ids = vec![id.clone()];
        m.event.id = id;
        let key = self.next_made_key();
        self.made.push(Listed::made(key, m));
        self.select(key.handle);
        key.handle
    }

    /// Change the made event with `handle` (keeping its id), under a new revision so everything
    /// computed from the old one is redone. Returns whether there was such an event.
    pub fn update_made(&mut self, handle: u64, mut m: MadeEvent) -> bool {
        let revision = self.next_made_key().revision;
        let Some(l) = self.made.iter_mut().find(|l| l.key.handle == handle) else {
            return false;
        };
        m.event.id = l.event.id.clone();
        m.event.ids = l.event.ids.clone();
        *l = Listed::made(EventKey { handle, revision }, m);
        self.rebuild();
        true
    }

    /// Delete the made event with `handle`. Like a forgotten past event, it stays (gone) while
    /// selected. Returns whether there was such an event.
    pub fn delete_made(&mut self, handle: u64) -> bool {
        let before = self.made.len();
        self.made.retain(|l| l.key.handle != handle);
        self.rebuild();
        self.made.len() != before
    }

    /// A fresh handle for a made event, and a revision no made event has had.
    fn next_made_key(&mut self) -> EventKey {
        let n = self.next_made;
        self.next_made += 1;
        EventKey {
            handle: n,
            revision: n,
        }
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
            .chain(self.made.iter().cloned())
            .filter(|l| {
                l.event.magnitude >= min_mag || l.saved || l.made || l.key.handle == selected
            })
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
    use feed::{FeedEvent, FeedStatus};
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
                mechanism_listed: false,
                status: "automatic".into(),
            },
            live: true,
            saved: false,
        }
    }

    /// A past event as a search finds it: neither live nor kept.
    fn found(handle: u64, years_ago: i64, mag: f64) -> StoredEvent {
        let mut s = stored(handle, years_ago * 365 * 24 * 60, mag);
        s.live = false;
        s
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

    /// Feed `events` in as a fresh download: USGS answered.
    fn feed_in(l: &mut EventList, events: Vec<StoredEvent>) -> bool {
        l.take_events(events);
        l.status = Some(answered());
        l.settle(now(), true);
        l.rebuild()
    }

    fn answered() -> FeedStatus {
        let mut s = FeedStatus::default();
        s.feeds[1].last_ok = Some(now());
        s
    }

    #[test]
    fn auto_pick_waits_for_the_fresh_download() {
        let mut l = list();
        // The saved copy arrives first: only an event from yesterday, and no answer from USGS
        // yet. No pick.
        l.take_events(vec![stored(1, 20 * 60, 4.0)]);
        l.status = Some(FeedStatus::default());
        assert!(!l.settle(now(), true));
        assert_eq!(l.selected_handle(), BUILTIN_HANDLE);
        // The fresh download brings a recent M5: that is the pick.
        l.take_events(vec![stored(1, 20 * 60, 4.0), stored(2, 10, 5.0)]);
        l.status = Some(answered());
        assert!(l.settle(now(), true));
        assert_eq!(l.selected_handle(), 2);
    }

    #[test]
    fn follows_new_big_events_until_the_user_picks() {
        let mut l = list();
        let mut events = vec![stored(1, 30, 6.5), stored(2, 10, 4.0)];
        feed_in(&mut l, events.clone());
        assert_eq!(l.selected_handle(), 1);
        // A new M5.9 is too small; an old M7 newly listed (e.g. a merge) is too old.
        events.push(stored(3, 2, 5.9));
        events.push(stored(4, 6 * 60, 7.0));
        feed_in(&mut l, events.clone());
        assert_eq!(l.selected_handle(), 1);
        // A new M6.1: followed.
        events.push(stored(5, 1, 6.1));
        feed_in(&mut l, events.clone());
        assert_eq!(l.selected_handle(), 5);
        // A list with nothing new leaves it.
        feed_in(&mut l, events.clone());
        assert_eq!(l.selected_handle(), 5);
        // Once the user picks, new big events no longer take over.
        l.select(2);
        events.push(stored(6, 0, 7.5));
        feed_in(&mut l, events);
        assert_eq!(l.selected_handle(), 2);
    }

    #[test]
    fn auto_pick_when_offline_or_unreachable() {
        for status in [
            FeedStatus {
                offline: true,
                ..Default::default()
            },
            {
                let mut s = FeedStatus::default();
                s.feeds[0].error = Some("timed out".into());
                s
            },
        ] {
            let mut l = list();
            l.take_events(vec![stored(1, 20 * 60, 4.0)]);
            l.status = Some(status);
            assert!(l.settle(now(), true));
            assert_eq!(l.selected_handle(), 1);
        }
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
    fn picking_a_result_keeps_selects_and_replays() {
        let mut l = list();
        feed_in(&mut l, vec![stored(1, 10, 4.0)]);
        assert_eq!(l.selected_handle(), 1);
        assert!(!l.take_replay());
        l.take_results(Ok(vec![found(7, 20, 8.0), found(8, 30, 7.0)]));
        assert_eq!(l.search.message.as_deref(), Some("2 found"));
        // Results are not listed until picked.
        assert_eq!(handles(&l), [1, BUILTIN_HANDLE]);
        assert!(l.pick_result(8));
        assert_eq!(l.selected_handle(), 8);
        assert!(l.user_chose);
        assert!(l.take_replay());
        assert!(!l.take_replay());
        // Listed, oldest last.
        assert_eq!(handles(&l), [1, BUILTIN_HANDLE, 8]);
        assert!(l.shown()[2].saved);
        // Still listed when the minimum magnitude rises above it and it is not selected.
        l.set_min_mag(7.5);
        l.select(BUILTIN_HANDLE);
        assert_eq!(handles(&l), [BUILTIN_HANDLE, 8]);
        // Forgotten while selected: stays until something else is selected.
        l.select(8);
        l.forget(8);
        assert_eq!(handles(&l), [BUILTIN_HANDLE, 8]);
        assert!(l.shown()[1].gone);
        l.select(BUILTIN_HANDLE);
        assert_eq!(handles(&l), [BUILTIN_HANDLE]);
    }

    #[test]
    fn a_new_search_clears_the_old_results() {
        let mut l = list();
        l.take_results(Ok(vec![found(7, 20, 8.0)]));
        l.search(Search::Id("us1".into()));
        assert!(l.search.results.is_empty());
        // No feed: nothing to pick, and it says so.
        assert!(!l.pick_result(7));
        assert!(!l.search.busy);
        assert!(l.search.message.is_some());
    }

    #[test]
    fn a_result_that_is_the_builtin_selects_it() {
        let mut l = list();
        feed_in(&mut l, vec![stored(1, 10, 4.0)]);
        let mut tohoku = found(9, 15, 9.1);
        tohoku.feed.event = tohoku.feed.event.with_ids(vec![
            "official20110311054624120_30".into(),
            "usp000hvnu".into(),
        ]);
        l.take_results(Ok(vec![tohoku]));
        l.pick_result(9);
        assert_eq!(l.selected_handle(), BUILTIN_HANDLE);
        assert!(l.take_replay());
        assert_eq!(handles(&l), [1, BUILTIN_HANDLE]);
    }

    #[test]
    fn kept_events_never_picked_or_followed_by_the_app() {
        let mut l = list();
        // A kept M7.5 from an hour ago (picked in an earlier session) and a live M3.
        let mut kept = stored(1, 60, 7.5);
        kept.live = false;
        kept.saved = true;
        feed_in(&mut l, vec![kept.clone(), stored(2, 30, 3.0)]);
        assert_eq!(l.selected_handle(), 2);
        // Nor followed when it first appears later.
        let mut l = list();
        feed_in(&mut l, vec![stored(2, 30, 3.0)]);
        feed_in(&mut l, vec![stored(2, 30, 3.0), kept]);
        assert_eq!(l.selected_handle(), 2);
        assert_eq!(l.feed_counts(), (1, 1));
    }

    #[test]
    fn wanted_event_found_by_lookup_is_picked() {
        let mut l = list();
        l.wanted_id = Some("e7".into());
        l.take_results(Err("USGS has no event with that code".into()));
        assert_eq!(l.wanted_id.as_deref(), Some("e7"));
        l.take_results(Ok(vec![found(7, 20, 8.0)]));
        assert_eq!(l.wanted_id, None);
        assert_eq!(l.selected_handle(), 7);
        assert!(l.take_replay());
        // A built-in id is selected at once, with no lookup.
        let mut l = list();
        l.want("usp000hvnu".into());
        assert_eq!(l.selected_handle(), BUILTIN_HANDLE);
        assert!(l.user_chose);
        assert_eq!(l.wanted_id, None);
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

    fn made_event(mins_ago: i64, mag: f64) -> MadeEvent {
        MadeEvent {
            event: Event::new(
                "",
                EventKind::NuclearTest,
                now() - Duration::minutes(mins_ago),
                41.3,
                129.1,
                1.0,
                mag,
            ),
            place: "Test site".into(),
        }
    }

    #[test]
    fn making_an_event_lists_and_selects_it() {
        let mut l = list();
        feed_in(&mut l, vec![stored(1, 30, 4.0)]);
        let rev = l.rev();
        let h = l.make(made_event(5, 1.5));
        assert!(h >= MADE_HANDLE);
        assert_eq!(l.selected_handle(), h);
        assert!(l.user_chose);
        // Never a replay: making an event does not move the clock.
        assert!(!l.take_replay());
        assert!(l.rev() > rev);
        // Listed below the minimum magnitude, newest first.
        assert_eq!(handles(&l), [h, 1, BUILTIN_HANDLE]);
        let shown = &l.shown()[0];
        assert!(shown.made);
        assert_eq!(shown.event.id, "made-1");
        assert_eq!(shown.event.ids, ["made-1"]);
        assert_eq!(shown.usgs_type, "nuclear test");
        assert_eq!(shown.status, "made by you");
        // Still listed when another event is selected.
        l.select(1);
        assert_eq!(handles(&l), [h, 1, BUILTIN_HANDLE]);
        // The next one gets the next id; ids survive a save and reload.
        l.make(made_event(2, 3.0));
        let kept = l.made();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1].event.id, "made-2");
        let mut again = list();
        assert!(again.load_made(kept));
        assert_eq!(again.selected_handle(), BUILTIN_HANDLE);
        assert_eq!(again.shown().len(), 3);
        again.make(made_event(1, 3.0));
        assert_eq!(again.made()[2].event.id, "made-3");
    }

    #[test]
    fn made_events_are_never_picked_or_followed_by_the_app() {
        let mut l = list();
        l.load_made(vec![made_event(10, 8.0)]);
        feed_in(&mut l, vec![stored(2, 30, 3.0)]);
        assert_eq!(l.selected_handle(), 2);
        assert_eq!(l.feed_counts(), (1, 1));
    }

    #[test]
    fn updating_a_made_event_keeps_handle_and_id_but_changes_the_key() {
        let mut l = list();
        let h = l.make(made_event(5, 5.0));
        let before = l.shown()[0].clone();
        let mut changed = made_event(5, 6.5);
        changed.place = "Bigger".into();
        assert!(l.update_made(h, changed));
        let after = &l.shown()[0];
        assert_eq!(after.key.handle, h);
        assert_ne!(after.key, before.key);
        assert_eq!(after.event.id, before.event.id);
        assert_eq!(after.event.magnitude, 6.5);
        assert_eq!(after.place, "Bigger");
        assert_eq!(l.selected_handle(), h);
        assert!(!l.update_made(12345, made_event(1, 1.0)));
    }

    #[test]
    fn deleting_a_made_event() {
        let mut l = list();
        let h = l.make(made_event(5, 5.0));
        // Deleted while selected: stays, gone, until something else is selected.
        assert!(l.delete_made(h));
        assert!(l.made().is_empty());
        assert_eq!(handles(&l), [h, BUILTIN_HANDLE]);
        assert!(l.shown()[0].gone);
        l.select(BUILTIN_HANDLE);
        assert_eq!(handles(&l), [BUILTIN_HANDLE]);
        assert!(!l.delete_made(h));
    }
}
