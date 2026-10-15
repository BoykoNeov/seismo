//! Virtual seismographs: the stations (bundled real ones and user-placed ones), which of them
//! are shown as traces, and the synthetic traces themselves, rebuilt only when what they
//! depend on changes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use sim::mantle::DelayGrid;
use sim::seismogram::{
    Component, Seismogram, Spreading, StationArrival, seed_from, station_arrivals, synthesize,
};
use sim::{Event, RayTracer, Simulation, Station, great_circle_deg};

use crate::events::EventKey;

/// Most traces shown at once.
pub const MAX_TRACES: usize = 8;
/// The trace window starts this long before the selected event's origin (s).
pub const WINDOW_BEFORE_S: f64 = 60.0;

/// Distances (deg) the default stations are picked near: one each, spread from near to far.
const DEFAULT_DISTANCES: [f64; 6] = [15.0, 35.0, 60.0, 90.0, 120.0, 150.0];

/// Other events drawn on the selected event's traces, at most.
const MAX_OTHER_EVENTS: usize = 12;
/// Other events more than this many magnitude units smaller than the selected one are left off
/// its traces: their waves would be under 1% of its amplitude.
const MAG_RANGE: f64 = 2.0;

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
    /// Ray-tube spreading per source depth (f64 bits), built on the `tubes` thread.
    spreading: HashMap<u64, Spreading>,
    tubes: TubeWorker,
    /// The selected event's ray tubes are still being built, so no trace is drawn yet.
    pub pending: bool,
}

/// Builds ray tubes (about 0.2 s each in a release build, measured 2026-10-14) off the UI
/// thread, in the order asked for.
struct TubeWorker {
    jobs: Sender<f64>,
    results: Receiver<Spreading>,
    requested: HashSet<u64>,
}

impl TubeWorker {
    fn spawn(tracer: RayTracer) -> Self {
        let (jobs, rx) = mpsc::channel::<f64>();
        let (tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("tubes".into())
            .spawn(move || {
                for depth_km in rx {
                    let started = Instant::now();
                    let spreading = Spreading::build(&tracer, depth_km);
                    tracing::info!(
                        depth_km,
                        ms = started.elapsed().as_millis() as u64,
                        "built seismogram ray tubes"
                    );
                    if tx.send(spreading).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn ray-tube worker");
        Self {
            jobs,
            results,
            requested: HashSet::new(),
        }
    }

    fn request(&mut self, depth_km: f64) {
        if self.requested.insert(depth_km.to_bits()) {
            // A send only fails if the worker panicked; the traces then stay pending.
            let _ = self.jobs.send(depth_km);
        }
    }
}

impl Traces {
    /// `tracer` is the simulation's, copied for the ray-tube thread.
    pub fn new(stations: Vec<Station>, tracer: RayTracer) -> Self {
        Self {
            stations,
            chosen: Vec::new(),
            show: true,
            component: Component::Vertical,
            shared_scale: false,
            gain: 1.0,
            window_min: 90.0,
            click_to_place: true,
            user_picked: false,
            note: None,
            next_user: 1,
            built: HashMap::new(),
            spreading: HashMap::new(),
            tubes: TubeWorker::spawn(tracer),
            pending: false,
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

    /// Bring the shown traces up to date: synthesise any whose inputs changed. `keys` match
    /// `sim.events`; `delays` are 3D-mantle delay grids by event key.
    pub fn update(
        &mut self,
        sim: &Simulation,
        keys: &[EventKey],
        selected: usize,
        delays: &HashMap<EventKey, Arc<DelayGrid>>,
        strength: f64,
    ) {
        let anchor = &sim.events[selected];
        let (from, to) = self.window();
        let mut events = contributors(sim, selected, (from, to));
        // Ray tubes for the contributors' depths only: USGS depths are arbitrary decimals. The
        // selected event's are asked for first; events whose tubes are not in yet are left off
        // until they are.
        let needed: HashSet<u64> = events
            .iter()
            .map(|&k| sim.events[k].depth_km.to_bits())
            .collect();
        for &k in &events {
            self.tubes.request(sim.events[k].depth_km);
        }
        for s in self.tubes.results.try_iter() {
            self.spreading.insert(s.depth_km.to_bits(), s);
        }
        self.spreading.retain(|d, _| needed.contains(d));
        self.tubes.requested.retain(|d| needed.contains(d));
        let ready = |k: &usize| {
            self.spreading
                .contains_key(&sim.events[*k].depth_km.to_bits())
        };
        self.pending = !ready(&selected);
        if self.pending {
            self.built.clear();
            return;
        }
        events.retain(ready);
        let sources: Vec<(EventKey, bool)> = events
            .iter()
            .map(|&k| (keys[k], delays.contains_key(&keys[k])))
            .collect();
        self.built.retain(|k, _| self.chosen.contains(k));
        for &i in &self.chosen {
            let station = &self.stations[i];
            let key = trace_key(station, &sources, self.component, strength, (from, to));
            if self.built.get(&i).is_some_and(|t| t.key == key) {
                continue;
            }
            let mut arrivals = Vec::new();
            for &k in &events {
                let e = &sim.events[k];
                // Seconds from the selected event's origin to this event's.
                let shift = -e.t_at(anchor.origin);
                let spreading = &self.spreading[&e.depth_km.to_bits()];
                let grid = delays.get(&keys[k]).map(Arc::as_ref);
                arrivals.extend(
                    station_arrivals(
                        sim.table(),
                        spreading,
                        e,
                        station,
                        grid,
                        strength,
                        sim.surface_waves(),
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
            let seismogram = synthesize(
                &arrivals,
                self.component,
                from,
                to,
                seed,
                sim.surface_waves(),
            );
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

/// The events whose waves are drawn on the selected event's traces: the selected event first,
/// then up to [`MAX_OTHER_EVENTS`] others, largest first, that are no more than [`MAG_RANGE`]
/// smaller and whose waves can reach a station inside the window `(from, to)` (seconds since
/// the selected event's origin).
fn contributors(sim: &Simulation, selected: usize, (from, to): (f64, f64)) -> Vec<usize> {
    let anchor = &sim.events[selected];
    // The last wave drawn is the end of the slowest second surface-wave pass, at most once
    // round the globe.
    let longest_s = sim.surface_waves().once_round_s();
    let mut others: Vec<usize> = (0..sim.events.len())
        .filter(|&k| k != selected)
        .filter(|&k| {
            let e = &sim.events[k];
            let shift = -e.t_at(anchor.origin);
            shift < to && shift + longest_s > from && e.magnitude >= anchor.magnitude - MAG_RANGE
        })
        .collect();
    others.sort_by(|&a, &b| {
        sim.events[b]
            .magnitude
            .total_cmp(&sim.events[a].magnitude)
            .then(a.cmp(&b))
    });
    others.truncate(MAX_OTHER_EVENTS);
    others.insert(0, selected);
    others
}

/// Everything a trace depends on, hashed: when it changes the trace is rebuilt. `sources` are
/// the contributing events' keys, selected first, each with whether its 3D delays are in.
fn trace_key(
    station: &Station,
    sources: &[(EventKey, bool)],
    component: Component,
    strength: f64,
    window: (f64, f64),
) -> u64 {
    let sources: Vec<String> = sources
        .iter()
        .map(|(k, ready)| format!("{}:{}:{ready}", k.handle, k.revision))
        .collect();
    seed_from(&[
        &station.label(),
        &station.lat.to_bits().to_string(),
        &station.lon.to_bits().to_string(),
        &sources.join(","),
        &format!("{component:?}"),
        &strength.to_bits().to_string(),
        &window.0.to_bits().to_string(),
        &window.1.to_bits().to_string(),
    ])
}
