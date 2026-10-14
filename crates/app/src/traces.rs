//! Virtual seismographs: the stations (bundled real ones and user-placed ones), which of them
//! are shown as traces, and the synthetic traces themselves, rebuilt only when what they
//! depend on changes.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::Instant;

use sim::mantle::DelayGrid;
use sim::seismogram::{
    Component, Seismogram, Spreading, StationArrival, SurfaceWaves, seed_from, station_arrivals,
    synthesize,
};
use sim::{Event, Simulation, Station, great_circle_deg};

/// Most traces shown at once.
pub const MAX_TRACES: usize = 8;
/// The trace window starts this long before the selected event's origin (s).
pub const WINDOW_BEFORE_S: f64 = 60.0;

/// Distances (deg) the default stations are picked near: one each, spread from near to far.
const DEFAULT_DISTANCES: [f64; 6] = [15.0, 35.0, 60.0, 90.0, 120.0, 150.0];

/// One shown trace: the station, its arrivals (times on the window's clock: seconds since the
/// selected event's origin) and the synthetic seismogram.
pub struct Trace {
    pub station: usize,
    pub dist_deg: f64,
    pub arrivals: Vec<StationArrival>,
    pub seismogram: Seismogram,
    key: u64,
}

pub struct Traces {
    pub stations: Vec<Station>,
    /// Indices into `stations` of the shown traces, at most [`MAX_TRACES`].
    pub chosen: Vec<usize>,
    pub show: bool,
    pub component: Component,
    /// One amplitude scale for every trace (the largest of them) instead of each its own.
    pub shared_scale: bool,
    pub gain: f32,
    /// Length of the window after the origin (minutes).
    pub window_min: f64,
    /// A click on empty map adds a user station there.
    pub click_to_place: bool,
    /// Default stations follow the selected event until the user picks their own.
    pub user_picked: bool,
    /// A short note for the controls (e.g. "8 traces at most").
    pub note: Option<String>,
    next_user: usize,
    built: HashMap<usize, Trace>,
    /// Ray-tube spreading per source depth (f64 bits).
    spreading: HashMap<u64, Spreading>,
}

impl Traces {
    pub fn new(stations: Vec<Station>) -> Self {
        Self {
            stations,
            chosen: Vec::new(),
            show: true,
            component: Component::Vertical,
            shared_scale: false,
            gain: 1.0,
            window_min: 60.0,
            click_to_place: true,
            user_picked: false,
            note: None,
            next_user: 1,
            built: HashMap::new(),
            spreading: HashMap::new(),
        }
    }

    /// Pick one bundled station near each default distance from `event`, unless the user has
    /// chosen their own.
    pub fn pick_defaults(&mut self, event: &Event) {
        let mut chosen: Vec<usize> = Vec::new();
        for want in DEFAULT_DISTANCES {
            let best = self
                .stations
                .iter()
                .enumerate()
                .filter(|(i, s)| !s.is_user() && !chosen.contains(i))
                .map(|(i, s)| {
                    let d = great_circle_deg(event.lat, event.lon, s.lat, s.lon);
                    (i, (d - want).abs())
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, miss)) = best
                && miss < 10.0
            {
                chosen.push(i);
            }
        }
        self.chosen = chosen;
        self.user_picked = false;
    }

    /// Show or hide `station`'s trace.
    pub fn toggle(&mut self, station: usize) {
        self.user_picked = true;
        if let Some(i) = self.chosen.iter().position(|&c| c == station) {
            self.chosen.remove(i);
            self.note = None;
        } else if self.chosen.len() < MAX_TRACES {
            self.chosen.push(station);
            self.note = None;
        } else {
            self.note = Some(format!("{MAX_TRACES} traces at most: remove one first."));
        }
    }

    /// Add a user station at (`lat`, `lon`) and show it, if there is room.
    pub fn place(&mut self, lat: f64, lon: f64) {
        if self.chosen.len() >= MAX_TRACES {
            self.note = Some(format!("{MAX_TRACES} traces at most: remove one first."));
            return;
        }
        self.stations
            .push(Station::user(format!("U{}", self.next_user), lat, lon));
        self.next_user += 1;
        self.toggle(self.stations.len() - 1);
    }

    /// Delete a user-placed station altogether.
    pub fn remove_user(&mut self, station: usize) {
        if !self.stations[station].is_user() {
            return;
        }
        self.stations.remove(station);
        self.chosen.retain(|&c| c != station);
        for c in &mut self.chosen {
            if *c > station {
                *c -= 1;
            }
        }
        self.built.clear();
    }

    /// Window (s since the selected event's origin) the traces cover.
    pub fn window(&self) -> (f64, f64) {
        (-WINDOW_BEFORE_S, self.window_min * 60.0)
    }

    /// Bring the shown traces up to date: synthesise any whose inputs changed. `delays` are the
    /// events' 3D-mantle delay grids by event index.
    pub fn update(
        &mut self,
        sim: &Simulation,
        selected: usize,
        delays: &HashMap<usize, Arc<DelayGrid>>,
        strength: f64,
    ) {
        let anchor = &sim.events[selected];
        let (from, to) = self.window();
        for e in &sim.events {
            if let Entry::Vacant(slot) = self.spreading.entry(e.depth_km.to_bits()) {
                let started = Instant::now();
                slot.insert(Spreading::build(sim.tracer(), e.depth_km));
                tracing::info!(
                    depth_km = e.depth_km,
                    ms = started.elapsed().as_millis() as u64,
                    "built seismogram ray tubes"
                );
            }
        }
        self.built.retain(|k, _| self.chosen.contains(k));
        for &i in &self.chosen {
            let station = &self.stations[i];
            let key = trace_key(
                station,
                selected,
                sim.events.len(),
                self.component,
                strength,
                delays,
                (from, to),
            );
            if self.built.get(&i).is_some_and(|t| t.key == key) {
                continue;
            }
            let mut arrivals = Vec::new();
            for (k, e) in sim.events.iter().enumerate() {
                // Seconds from the selected event's origin to this event's.
                let shift = -e.t_at(anchor.origin);
                let spreading = &self.spreading[&e.depth_km.to_bits()];
                let grid = delays.get(&k).map(Arc::as_ref);
                arrivals.extend(
                    station_arrivals(
                        sim.table(),
                        spreading,
                        e,
                        station,
                        grid,
                        strength,
                        SurfaceWaves::default(),
                    )
                    .into_iter()
                    .map(|a| StationArrival {
                        time_s: a.time_s + shift,
                        ..a
                    }),
                );
            }
            arrivals.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
            let seed = seed_from(&[&anchor.id, &station.label()]);
            let seismogram = synthesize(&arrivals, self.component, from, to, seed);
            self.built.insert(
                i,
                Trace {
                    station: i,
                    dist_deg: great_circle_deg(anchor.lat, anchor.lon, station.lat, station.lon),
                    arrivals,
                    seismogram,
                    key,
                },
            );
        }
    }

    /// The shown traces as last updated, nearest first.
    pub fn shown(&self) -> Vec<&Trace> {
        let mut out: Vec<&Trace> = self
            .chosen
            .iter()
            .filter_map(|i| self.built.get(i))
            .collect();
        out.sort_by(|a, b| a.dist_deg.total_cmp(&b.dist_deg));
        out
    }
}

/// Everything a trace depends on, hashed: when it changes the trace is rebuilt.
fn trace_key(
    station: &Station,
    selected: usize,
    events: usize,
    component: Component,
    strength: f64,
    delays: &HashMap<usize, Arc<DelayGrid>>,
    window: (f64, f64),
) -> u64 {
    let mut ready: Vec<usize> = delays.keys().copied().collect();
    ready.sort_unstable();
    let ready: Vec<String> = ready.iter().map(usize::to_string).collect();
    seed_from(&[
        &station.label(),
        &station.lat.to_bits().to_string(),
        &station.lon.to_bits().to_string(),
        &selected.to_string(),
        &events.to_string(),
        &format!("{component:?}"),
        &strength.to_bits().to_string(),
        &ready.join(","),
        &window.0.to_bits().to_string(),
        &window.1.to_bits().to_string(),
    ])
}
