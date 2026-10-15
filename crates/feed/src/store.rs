//! The merged event list: every feed snapshot folded in, duplicates merged on their USGS ids,
//! events USGS deleted taken out. Past events found by a search and the ones the user keeps
//! go through the same merge, so an event has one handle however it arrived. Pure: no I/O, no
//! clock.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};

use crate::usgs::{FeedEvent, FeedKind};

/// `all_day` lists every event whose origin is in the past 24 hours. An event missing from a
/// fresh copy is taken as deleted only if its origin is at least this far inside that window,
/// so events simply ageing out at the old edge are kept.
const DAY_EDGE_MARGIN_H: i64 = 1;

/// An event in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEvent {
    /// Stable for the life of the store: kept through new versions and merges (the merged
    /// event keeps the smallest handle). Starts at 1.
    pub handle: u64,
    /// Changes whenever the event's physics does (origin, position, depth, magnitude, kind),
    /// so anything computed from it can tell it is stale.
    pub revision: u64,
    pub feed: FeedEvent,
    /// A live summary feed has listed it (this session).
    pub live: bool,
    /// The user keeps it (a picked past event, remembered across restarts).
    pub saved: bool,
}

impl StoredEvent {
    /// Listed in the app: live or kept. Anything else is only a search result.
    pub fn listed(&self) -> bool {
        self.live || self.saved
    }
}

#[derive(Debug, Default)]
pub struct EventStore {
    records: Vec<StoredEvent>,
    next_handle: u64,
    /// Handles in the last `all_day` snapshot.
    last_day: Option<HashSet<u64>>,
}

impl EventStore {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            next_handle: 1,
            last_day: None,
        }
    }

    /// Every record, search-only results included.
    pub fn events(&self) -> &[StoredEvent] {
        &self.records
    }

    /// The events the app lists: live or kept.
    pub fn listed(&self) -> Vec<StoredEvent> {
        self.records
            .iter()
            .filter(|r| r.listed())
            .cloned()
            .collect()
    }

    /// The kept events, for saving.
    pub fn saved(&self) -> impl Iterator<Item = &FeedEvent> {
        self.records.iter().filter(|r| r.saved).map(|r| &r.feed)
    }

    /// Fold in a snapshot of `feed` fetched at `fetched_at`. Returns whether anything changed.
    ///
    /// Only `all_day` can delete: an event in the previous `all_day` snapshot that is missing
    /// from this one, with its origin well inside the past day, was deleted by USGS. Events
    /// leave `all_hour` by ageing and `significant_month` when their significance is
    /// recomputed, so neither means a deletion.
    pub fn apply(
        &mut self,
        feed: FeedKind,
        snapshot: Vec<FeedEvent>,
        fetched_at: DateTime<Utc>,
    ) -> bool {
        let mut changed = false;
        let mut seen = HashSet::new();
        for e in snapshot {
            let (handle, c) = self.merge(e);
            seen.insert(handle);
            let r = self.record_mut(handle);
            changed |= c || !r.live;
            r.live = true;
        }
        if feed == FeedKind::AllDay {
            if let Some(prev) = &self.last_day {
                let edge = fetched_at - Duration::hours(24 - DAY_EDGE_MARGIN_H);
                let gone: HashSet<u64> = prev.difference(&seen).copied().collect();
                let deleted =
                    |r: &StoredEvent| gone.contains(&r.handle) && r.feed.event.origin > edge;
                let before = self.records.len();
                // A deleted event the user keeps stays, as a kept event only.
                for r in &mut self.records {
                    if deleted(r) && r.saved && r.live {
                        r.live = false;
                        changed = true;
                    }
                }
                self.records.retain(|r| !(deleted(r) && !r.saved));
                changed |= self.records.len() != before;
            }
            self.last_day = Some(seen);
        }
        changed
    }

    /// Fold in the events kept from earlier sessions. Returns whether anything changed.
    pub fn apply_saved(&mut self, events: Vec<FeedEvent>) -> bool {
        let mut changed = false;
        for e in events {
            let (handle, c) = self.merge(e);
            let r = self.record_mut(handle);
            changed |= c || !r.saved;
            r.saved = true;
        }
        changed
    }

    /// Fold in a search's results and return them as records, in result order, one per event
    /// (results that turn out to be the same event appear once). The previous search's
    /// results that are neither live, kept, nor found again are dropped. Returns the records
    /// and whether the listed events changed (a result can be a newer version of a listed
    /// event).
    pub fn apply_search(&mut self, results: Vec<FeedEvent>) -> (Vec<StoredEvent>, bool) {
        let mut changed = false;
        let mut handles: Vec<u64> = Vec::new();
        for e in results {
            let (handle, c) = self.merge(e);
            changed |= c && self.record_mut(handle).listed();
            if !handles.contains(&handle) {
                handles.push(handle);
            }
        }
        self.records
            .retain(|r| r.listed() || handles.contains(&r.handle));
        // A later result can merge an earlier one away: only survivors are returned.
        let found = handles
            .iter()
            .filter_map(|&h| self.records.iter().find(|r| r.handle == h).cloned())
            .collect();
        (found, changed)
    }

    /// Keep (`true`) or stop keeping the event with `handle`. Returns whether that changed
    /// anything. An event neither kept nor live stays as a search result until the next search.
    pub fn set_saved(&mut self, handle: u64, saved: bool) -> bool {
        match self.records.iter_mut().find(|r| r.handle == handle) {
            Some(r) if r.saved != saved => {
                r.saved = saved;
                true
            }
            _ => false,
        }
    }

    fn record_mut(&mut self, handle: u64) -> &mut StoredEvent {
        self.records
            .iter_mut()
            .find(|r| r.handle == handle)
            .expect("merge returns a stored handle")
    }

    /// Merge one incoming version with every record sharing any of its ids. The newest
    /// `updated` wins (on a tie the stored version stays, so re-applying changes nothing); the
    /// survivor keeps the smallest handle and the union of everyone's ids.
    fn merge(&mut self, incoming: FeedEvent) -> (u64, bool) {
        let shares = |r: &StoredEvent| {
            r.feed
                .event
                .ids
                .iter()
                .any(|id| incoming.event.ids.contains(id))
        };
        let matched: Vec<usize> = (0..self.records.len())
            .filter(|&i| shares(&self.records[i]))
            .collect();
        if matched.is_empty() {
            let handle = self.next_handle;
            self.next_handle += 1;
            self.records.push(StoredEvent {
                handle,
                revision: revision(&incoming),
                feed: incoming,
                live: false,
                saved: false,
            });
            return (handle, true);
        }

        let survivor = *matched
            .iter()
            .min_by_key(|&&i| self.records[i].handle)
            .expect("not empty");
        // Candidates in order of preference on equal `updated`: the survivor, the other
        // matches, then the incoming version.
        let mut candidates: Vec<&FeedEvent> = vec![&self.records[survivor].feed];
        candidates.extend(
            matched
                .iter()
                .filter(|&&i| i != survivor)
                .map(|&i| &self.records[i].feed),
        );
        candidates.push(&incoming);
        let winner = candidates
            .iter()
            .copied()
            .reduce(|best, c| if c.updated > best.updated { c } else { best })
            .expect("not empty");
        let mut merged = winner.clone();
        for c in &candidates {
            for id in &c.event.ids {
                if !merged.event.ids.contains(id) {
                    merged.event.ids.push(id.clone());
                }
            }
        }

        let handle = self.records[survivor].handle;
        let live = matched.iter().any(|&i| self.records[i].live);
        let saved = matched.iter().any(|&i| self.records[i].saved);
        let mut changed = merged != self.records[survivor].feed;
        let r = &mut self.records[survivor];
        r.revision = revision(&merged);
        r.feed = merged;
        r.live = live;
        r.saved = saved;
        if matched.len() > 1 {
            let others: HashSet<u64> = matched
                .iter()
                .filter(|&&i| i != survivor)
                .map(|&i| self.records[i].handle)
                .collect();
            self.records.retain(|r| !others.contains(&r.handle));
            changed = true;
        }
        (handle, changed)
    }
}

/// A hash of what the simulation uses from an event (FNV-1a over the values' bits).
fn revision(e: &FeedEvent) -> u64 {
    let ev = &e.event;
    let words = [
        ev.origin.timestamp_micros() as u64,
        ev.lat.to_bits(),
        ev.lon.to_bits(),
        ev.depth_km.to_bits(),
        ev.magnitude.to_bits(),
        ev.kind as u64,
    ];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for w in words {
        for b in w.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use sim::{Event, EventKind};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap()
    }

    /// An event `mins_ago` before `now()`, last updated `updated_min` after its origin.
    fn ev(ids: &[&str], mins_ago: i64, updated_min: i64, mag: f64) -> FeedEvent {
        let origin = now() - Duration::minutes(mins_ago);
        FeedEvent {
            event: Event::new(ids[0], EventKind::Earthquake, origin, 10.0, 20.0, 10.0, mag)
                .with_ids(ids.iter().map(|s| s.to_string()).collect()),
            usgs_type: "earthquake".into(),
            place: "somewhere".into(),
            updated: origin + Duration::minutes(updated_min),
            status: "automatic".into(),
        }
    }

    fn ids(store: &EventStore) -> Vec<(u64, String, f64)> {
        store
            .events()
            .iter()
            .map(|r| (r.handle, r.feed.event.id.clone(), r.feed.event.magnitude))
            .collect()
    }

    #[test]
    fn new_events_get_handles_and_reapplying_changes_nothing() {
        let mut s = EventStore::new();
        let snap = vec![ev(&["a"], 10, 1, 3.0), ev(&["b"], 20, 1, 4.0)];
        assert!(s.apply(FeedKind::AllHour, snap.clone(), now()));
        assert_eq!(ids(&s), [(1, "a".into(), 3.0), (2, "b".into(), 4.0)]);
        assert!(!s.apply(FeedKind::AllHour, snap, now()));
    }

    #[test]
    fn newer_version_wins_and_keeps_handle() {
        let mut s = EventStore::new();
        s.apply(FeedKind::AllHour, vec![ev(&["a"], 10, 1, 3.0)], now());
        let rev = s.events()[0].revision;
        // Older version: ignored.
        assert!(!s.apply(FeedKind::AllDay, vec![ev(&["a"], 10, 0, 2.0)], now()));
        assert_eq!(ids(&s), [(1, "a".into(), 3.0)]);
        // Newer version with a revised magnitude: replaces it, same handle, new revision.
        assert!(s.apply(FeedKind::AllDay, vec![ev(&["a"], 10, 5, 3.4)], now()));
        assert_eq!(ids(&s), [(1, "a".into(), 3.4)]);
        assert_ne!(s.events()[0].revision, rev);
    }

    #[test]
    fn preferred_id_change_is_the_same_event() {
        let mut s = EventStore::new();
        s.apply(FeedKind::AllHour, vec![ev(&["ak1"], 10, 1, 3.0)], now());
        // USGS now prefers the "us" id and lists both.
        s.apply(
            FeedKind::AllHour,
            vec![ev(&["us9", "ak1"], 10, 3, 3.1)],
            now(),
        );
        assert_eq!(ids(&s), [(1, "us9".into(), 3.1)]);
        assert_eq!(s.events()[0].feed.event.ids, ["us9", "ak1"]);
    }

    #[test]
    fn one_version_merging_two_records() {
        let mut s = EventStore::new();
        s.apply(
            FeedKind::AllHour,
            vec![ev(&["ak1"], 10, 1, 3.0), ev(&["nc2"], 10, 2, 3.2)],
            now(),
        );
        assert_eq!(s.events().len(), 2);
        assert!(s.apply(
            FeedKind::AllHour,
            vec![ev(&["us9", "ak1", "nc2"], 10, 4, 3.3)],
            now()
        ));
        assert_eq!(ids(&s), [(1, "us9".into(), 3.3)]);
        let e = &s.events()[0].feed.event;
        assert_eq!(e.ids, ["us9", "ak1", "nc2"]);
    }

    #[test]
    fn merged_ids_survive_an_older_partial_version() {
        let mut s = EventStore::new();
        s.apply(
            FeedKind::AllHour,
            vec![ev(&["us9", "ak1"], 10, 4, 3.3)],
            now(),
        );
        // A stale copy (another feed, older `updated`) naming only one of the ids.
        assert!(!s.apply(
            FeedKind::SignificantMonth,
            vec![ev(&["ak1"], 10, 1, 3.0)],
            now()
        ));
        assert_eq!(ids(&s), [(1, "us9".into(), 3.3)]);
        assert_eq!(s.events()[0].feed.event.ids, ["us9", "ak1"]);
    }

    #[test]
    fn all_day_detects_deletions_but_not_ageing() {
        let mut s = EventStore::new();
        let fresh = ev(&["fresh"], 60, 1, 3.0);
        let old = ev(&["old"], 23 * 60 + 30, 1, 3.0);
        let gone = ev(&["gone"], 120, 1, 3.0);
        s.apply(FeedKind::AllDay, vec![fresh.clone(), old, gone], now());
        // A minute later "gone" (deleted by USGS) and "old" (ageing out) are both missing.
        let later = now() + Duration::minutes(1);
        assert!(s.apply(FeedKind::AllDay, vec![fresh], later));
        let left: Vec<String> = ids(&s).into_iter().map(|(_, id, _)| id).collect();
        assert_eq!(left, ["fresh", "old"]);
    }

    #[test]
    fn other_feeds_never_delete() {
        let mut s = EventStore::new();
        s.apply(FeedKind::AllHour, vec![ev(&["a"], 10, 1, 3.0)], now());
        s.apply(
            FeedKind::SignificantMonth,
            vec![ev(&["big"], 3000, 1, 7.0)],
            now(),
        );
        // Both drop out of their feeds.
        assert!(!s.apply(FeedKind::AllHour, vec![], now()));
        assert!(!s.apply(FeedKind::SignificantMonth, vec![], now()));
        assert_eq!(s.events().len(), 2);
        // The first all_day snapshot has nothing to compare with, so it deletes nothing.
        assert!(!s.apply(FeedKind::AllDay, vec![], now()));
        assert_eq!(s.events().len(), 2);
    }

    #[test]
    fn search_hits_share_handles_with_live_events() {
        let mut s = EventStore::new();
        s.apply(FeedKind::AllDay, vec![ev(&["ak1"], 60, 1, 4.0)], now());
        // A search finds the same event under a newer preferred id, and an older one.
        let search = || vec![ev(&["us9", "ak1"], 60, 2, 4.1), ev(&["old"], 5000, 1, 7.0)];
        let (found, changed) = s.apply_search(search());
        assert!(changed);
        let got: Vec<(u64, bool)> = found.iter().map(|r| (r.handle, r.live)).collect();
        assert_eq!(got, [(1, true), (2, false)]);
        // Only the live one is listed.
        let listed: Vec<u64> = s.listed().iter().map(|r| r.handle).collect();
        assert_eq!(listed, [1]);
        // The same search again: same handles, nothing changed.
        let (again, changed) = s.apply_search(search());
        assert!(!changed);
        assert_eq!(again, found);
        // A different search drops the old result that nobody kept.
        let (other, _) = s.apply_search(vec![ev(&["new"], 9000, 1, 8.0)]);
        assert_eq!(other[0].handle, 3);
        assert_eq!(s.events().len(), 2);
    }

    #[test]
    fn duplicate_search_results_appear_once() {
        let mut s = EventStore::new();
        let (found, _) = s.apply_search(vec![
            ev(&["a"], 5000, 1, 7.0),
            ev(&["b", "a"], 5000, 2, 7.1),
        ]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].feed.event.magnitude, 7.1);
    }

    #[test]
    fn kept_events_are_listed_and_survive_searches_and_deletion() {
        let mut s = EventStore::new();
        let (found, _) = s.apply_search(vec![ev(&["old"], 5000, 1, 7.0)]);
        assert!(s.set_saved(found[0].handle, true));
        assert!(!s.set_saved(found[0].handle, true));
        s.apply_search(vec![]);
        assert_eq!(s.listed().len(), 1);
        assert_eq!(s.saved().count(), 1);

        // A recent event, kept, then deleted by USGS: stays, no longer live.
        s.apply(FeedKind::AllDay, vec![ev(&["k"], 60, 1, 5.0)], now());
        let k = s.listed().iter().find(|r| r.live).unwrap().handle;
        s.set_saved(k, true);
        assert!(s.apply(FeedKind::AllDay, vec![], now()));
        let r = s.events().iter().find(|r| r.handle == k).unwrap();
        assert!(r.saved && !r.live);

        // No longer kept: not listed, gone at the next search.
        assert!(s.set_saved(k, false));
        assert_eq!(s.listed().len(), 1);
        s.apply_search(vec![]);
        assert_eq!(s.events().len(), 1);
    }

    #[test]
    fn saved_from_disk_merge_with_live() {
        let mut s = EventStore::new();
        assert!(s.apply_saved(vec![ev(&["ak1"], 60, 1, 4.0)]));
        assert!(!s.apply_saved(vec![ev(&["ak1"], 60, 1, 4.0)]));
        s.apply(
            FeedKind::AllHour,
            vec![ev(&["us9", "ak1"], 60, 3, 4.2)],
            now(),
        );
        let r = &s.events()[0];
        assert_eq!(s.events().len(), 1);
        assert!(r.live && r.saved);
        assert_eq!(r.feed.event.magnitude, 4.2);
    }

    #[test]
    fn deletion_follows_a_merge() {
        let mut s = EventStore::new();
        s.apply(FeedKind::AllDay, vec![ev(&["ak1"], 60, 1, 3.0)], now());
        // The event comes back under a new preferred id that still lists the old one: kept.
        assert!(s.apply(
            FeedKind::AllDay,
            vec![ev(&["us9", "ak1"], 60, 2, 3.1)],
            now()
        ));
        assert_eq!(ids(&s), [(1, "us9".into(), 3.1)]);
        // Then it is deleted.
        assert!(s.apply(FeedKind::AllDay, vec![], now()));
        assert!(s.events().is_empty());
    }
}
