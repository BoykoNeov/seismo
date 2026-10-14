//! The simulation: events plus the travel-time table, queried at an absolute time.

use chrono::{DateTime, Utc};

use crate::event::Event;
use crate::raytrace::{RayTraceError, RayTracer};
use crate::slice::{Boundary, SliceFans, named_boundaries};
use crate::tables::{EarthModel, TravelTimeTable};

/// Events, the table their surface arrivals are timed by, and the velocity model their rays
/// are traced through. Pure: `state_at` depends only on the events, the table and `now`.
#[derive(Debug, Clone)]
pub struct Simulation {
    table: TravelTimeTable,
    tracer: RayTracer,
    boundaries: Vec<Boundary>,
    pub events: Vec<Event>,
}

/// Wave state of every event at one instant.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub now: DateTime<Utc>,
    /// One entry per event, in `Simulation::events` order.
    pub events: Vec<EventState>,
}

/// Wave state of one event.
#[derive(Debug, Clone, PartialEq)]
pub struct EventState {
    /// Index into `Simulation::events`.
    pub index: usize,
    /// Seconds since origin; negative before the event.
    pub t: f64,
    /// Epicentral distance (deg) the direct P front has reached at the surface, or `None`
    /// before origin and after P has run off the tabulated distances (the shadow zone).
    pub p_front_deg: Option<f64>,
    /// Same for direct S.
    pub s_front_deg: Option<f64>,
}

/// An event selected for drawing on the map, with its time since origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapEvent {
    pub index: usize,
    pub t: f64,
}

impl Simulation {
    pub fn new(
        table: TravelTimeTable,
        model: EarthModel,
        events: Vec<Event>,
    ) -> Result<Self, RayTraceError> {
        Ok(Self {
            table,
            tracer: RayTracer::new(&model)?,
            boundaries: named_boundaries(&model),
            events,
        })
    }

    pub fn table(&self) -> &TravelTimeTable {
        &self.table
    }

    pub fn tracer(&self) -> &RayTracer {
        &self.tracer
    }

    /// Named discontinuities of the velocity model (Moho, 410, 660, CMB, ICB).
    pub fn boundaries(&self) -> &[Boundary] {
        &self.boundaries
    }

    /// Ray fans for the slice view from a source at `depth_km`. Takes a fraction of a second;
    /// callers cache the result per depth.
    pub fn slice_fans(&self, depth_km: f64) -> SliceFans {
        SliceFans::build(&self.tracer, depth_km)
    }

    /// The state of every event at `now`.
    pub fn state_at(&self, now: DateTime<Utc>) -> Snapshot {
        let events = self
            .events
            .iter()
            .enumerate()
            .map(|(index, e)| {
                let t = e.t_at(now);
                EventState {
                    index,
                    t,
                    p_front_deg: self.front_distance("P", e.depth_km, t),
                    s_front_deg: self.front_distance("S", e.depth_km, t),
                }
            })
            .collect();
        Snapshot { now, events }
    }

    /// Distance (deg) of `phase`'s surface front `t` seconds after origin for a source at
    /// `depth_km`: the first tabulated distance whose arrival time is >= `t`, refined by linear
    /// interpolation against the previous distance when that one has already been reached.
    /// `None` before origin, or once `t` is later than every tabulated arrival of the phase.
    pub fn front_distance(&self, phase: &str, depth_km: f64, t: f64) -> Option<f64> {
        if t < 0.0 {
            return None;
        }
        let axis = self.table.distance_axis();
        let mut prev: Option<(f64, f64)> = None;
        for j in 0..axis.count {
            let d = axis.value(j);
            let Some(arr) = self.table.arrival(phase, depth_km, d) else {
                prev = None;
                continue;
            };
            if arr >= t {
                return Some(match prev {
                    Some((d0, t0)) if arr > t0 => d0 + (d - d0) * (t - t0) / (arr - t0),
                    _ => d,
                });
            }
            prev = Some((d, arr));
        }
        None
    }

    /// Events whose waves are on the globe at `now` (origin passed, and within `linger_s`
    /// seconds of the table's last arrival), largest magnitude first, at most `cap`.
    /// Ties keep event order, so the result is deterministic.
    pub fn map_events(&self, now: DateTime<Utc>, cap: usize, linger_s: f64) -> Vec<MapEvent> {
        let horizon = self.table.max_arrival_time() + linger_s;
        let mut out: Vec<MapEvent> = self
            .events
            .iter()
            .enumerate()
            .map(|(index, e)| MapEvent {
                index,
                t: e.t_at(now),
            })
            .filter(|m| m.t >= 0.0 && m.t <= horizon)
            .collect();
        out.sort_by(|a, b| {
            self.events[b.index]
                .magnitude
                .total_cmp(&self.events[a.index].magnitude)
        });
        out.truncate(cap);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventKind;
    use chrono::{Duration, TimeZone};

    static TABLE: &[u8] = include_bytes!("../../../assets/ak135.tt");

    fn origin() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2011, 3, 11, 5, 46, 24).unwrap()
    }

    static MODEL: &[u8] = include_bytes!("../../../assets/ak135_model.csv");

    fn sim(events: Vec<Event>) -> Simulation {
        Simulation::new(
            TravelTimeTable::from_bytes(TABLE).unwrap(),
            EarthModel::from_bytes(MODEL).unwrap(),
            events,
        )
        .unwrap()
    }

    fn tohoku() -> Event {
        Event::new(
            "tohoku",
            EventKind::Earthquake,
            origin(),
            38.297,
            142.373,
            29.0,
            9.1,
        )
    }

    #[test]
    fn nothing_before_origin() {
        let s = sim(vec![tohoku()]).state_at(origin() - Duration::seconds(5));
        assert_eq!(s.events[0].t, -5.0);
        assert_eq!(s.events[0].p_front_deg, None);
        assert_eq!(s.events[0].s_front_deg, None);
    }

    #[test]
    fn fronts_match_table_and_p_leads_s() {
        let sim = sim(vec![tohoku()]);
        let s = sim.state_at(origin() + Duration::seconds(300));
        let p = s.events[0].p_front_deg.unwrap();
        let sfront = s.events[0].s_front_deg.unwrap();
        assert!(p > sfront, "P {p} should lead S {sfront}");
        // The front sits where the table says the arrival time equals t.
        let at_p = sim.table().arrival("P", 29.0, p).unwrap();
        assert!((at_p - 300.0).abs() < 1.0, "P arrival at front {at_p}");
        let at_s = sim.table().arrival("S", 29.0, sfront).unwrap();
        assert!((at_s - 300.0).abs() < 1.0, "S arrival at front {at_s}");
    }

    #[test]
    fn fronts_grow_with_time() {
        let sim = sim(vec![tohoku()]);
        let mut last = 0.0;
        for secs in [10, 60, 120, 300, 600] {
            let p = sim.state_at(origin() + Duration::seconds(secs)).events[0]
                .p_front_deg
                .unwrap();
            assert!(p > last, "P front {p} at {secs}s not beyond {last}");
            last = p;
        }
    }

    #[test]
    fn direct_p_vanishes_into_the_shadow() {
        let sim = sim(vec![tohoku()]);
        // An hour after origin direct P has passed every distance it reaches.
        let s = sim.state_at(origin() + Duration::seconds(3600));
        assert_eq!(s.events[0].p_front_deg, None);
        assert_eq!(s.events[0].s_front_deg, None);
    }

    #[test]
    fn deterministic() {
        let a = sim(vec![tohoku()]).state_at(origin() + Duration::milliseconds(123_456));
        let b = sim(vec![tohoku()]).state_at(origin() + Duration::milliseconds(123_456));
        assert_eq!(a, b);
    }

    #[test]
    fn map_events_culls_and_caps_by_magnitude() {
        let now = origin() + Duration::seconds(100);
        let mk = |id: &str, mins_ago: i64, mag: f64| {
            Event::new(
                id,
                EventKind::Earthquake,
                now - Duration::minutes(mins_ago),
                0.0,
                0.0,
                10.0,
                mag,
            )
        };
        let events = vec![
            mk("future", -5, 8.0),   // not yet happened
            mk("old", 24 * 60, 9.0), // waves long gone
            mk("small", 1, 4.0),
            mk("big", 2, 7.0),
            mk("mid", 3, 5.5),
        ];
        let sim = sim(events);
        let ids = |v: Vec<MapEvent>| -> Vec<String> {
            v.iter().map(|m| sim.events[m.index].id.clone()).collect()
        };
        assert_eq!(ids(sim.map_events(now, 32, 0.0)), ["big", "mid", "small"]);
        assert_eq!(ids(sim.map_events(now, 2, 0.0)), ["big", "mid"]);
        let m = sim.map_events(now, 1, 0.0)[0];
        assert_eq!(m.t, 120.0);
    }
}
