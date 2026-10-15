//! 3D-mantle work off the UI thread: the worker loads the SP12RTS model, then turns requests
//! into per-event delay grids for the map, and rock grids and bent ray fans for the slice. The
//! UI thread asks, collects finished results, and keeps track of which GPU slot holds which
//! event's delays.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Instant;

use sim::mantle::{CutGrid, DelayGrid, Mantle3d, cut_grid, delay_grid};
use sim::raytrace::bend::{PlaneGrid, bent_fan, bent_fan_jobs};
use sim::slice::{Boundary, Fan, SliceFans};
use sim::tables::MantleModel;
use sim::{RayTracer, Wave};

use session::events::EventKey;

static SP12RTS_VS: &[u8] = include_bytes!("../../../assets/sp12rts/SP12RTS..ES.sph");
static SP12RTS_VP: &[u8] = include_bytes!("../../../assets/sp12rts/SP12RTS..EP.sph");

/// GPU delay slots, one per event drawn at once.
const SLOTS: usize = render::MAX_MAP_EVENTS;

/// What a slice rock grid was made for. The azimuth is in tenths of a degree so that equal
/// keys mean equal grids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutKey {
    pub event: EventKey,
    pub azimuth_decideg: i32,
    pub wave: Wave,
}

impl CutKey {
    pub fn new(event: EventKey, azimuth_deg: f64, wave: Wave) -> Self {
        Self {
            event,
            azimuth_decideg: (azimuth_deg.rem_euclid(360.0) * 10.0).round() as i32,
            wave,
        }
    }

    fn azimuth_deg(&self) -> f64 {
        f64::from(self.azimuth_decideg) / 10.0
    }
}

/// What a set of bent slice fans was made for: event, azimuth (tenths of a degree) and 3D
/// strength (hundredths).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BendKey {
    pub event: EventKey,
    pub azimuth_decideg: i32,
    pub strength_centi: i32,
}

impl BendKey {
    pub fn new(event: EventKey, azimuth_deg: f64, strength: f32) -> Self {
        Self {
            event,
            azimuth_decideg: (azimuth_deg.rem_euclid(360.0) * 10.0).round() as i32,
            strength_centi: (strength * 100.0).round() as i32,
        }
    }
}

enum Job {
    Delays {
        event: EventKey,
        lat: f64,
        lon: f64,
        depth_km: f64,
    },
    Cut {
        key: CutKey,
        lat: f64,
        lon: f64,
    },
    Bend {
        key: BendKey,
        lat: f64,
        lon: f64,
        depth_km: f64,
    },
}

enum Output {
    Delays { event: EventKey, grid: DelayGrid },
    Cut { key: CutKey, grid: CutGrid },
    Bend { key: BendKey, fans: SliceFans },
}

/// A result ready to upload.
pub enum Ready {
    /// `event`'s map delays, for GPU slot `slot`.
    Delays {
        event: EventKey,
        slot: u32,
        grid: DelayGrid,
    },
    /// The rock grid for the slice as it is now.
    Cut(CutGrid),
    /// Bent ray fans for the slice as it is now.
    Bend(BendKey, SliceFans),
}

pub struct MantleWorker {
    jobs: Sender<Job>,
    results: Receiver<Output>,
    requested: HashSet<EventKey>,
    /// Per GPU slot: the event whose grid it holds and that grid's largest |delay|.
    slots: [Option<(EventKey, f32)>; SLOTS],
    /// The rock grid last asked for, and the one last uploaded.
    cut_wanted: Option<CutKey>,
    cut_shown: Option<CutKey>,
    /// The bent fans last asked for.
    bend_wanted: Option<BendKey>,
    /// The worker has stopped (the model failed to load, or it panicked).
    failed: bool,
}

impl MantleWorker {
    /// Start the worker. It owns copies of the ray tracer and the 1D boundaries.
    pub fn spawn(tracer: RayTracer, boundaries: Vec<Boundary>) -> Self {
        let (jobs, rx) = mpsc::channel::<Job>();
        let (tx, results) = mpsc::channel::<Output>();
        std::thread::Builder::new()
            .name("mantle".into())
            .spawn(move || {
                let started = Instant::now();
                let model = match MantleModel::from_bytes(SP12RTS_VS, SP12RTS_VP) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::error!("SP12RTS model: {e}; the map and slice stay 1D");
                        return;
                    }
                };
                tracing::info!(
                    ms = started.elapsed().as_millis() as u64,
                    "loaded SP12RTS mantle model"
                );
                let mantle = Mantle3d::new(model, &boundaries);
                let mut pending: VecDeque<Job> = VecDeque::new();
                // 1D fans seed the bent ones; they depend only on the source depth.
                let mut seeds: Option<SliceFans> = None;
                loop {
                    if pending.is_empty() {
                        match rx.recv() {
                            Ok(j) => pending.push_back(j),
                            Err(_) => return,
                        }
                    }
                    pending.extend(rx.try_iter());
                    // Only the latest slice request of each kind matters, and they go first: the
                    // user is looking at them. Rock grids are cheap, so they go before fans.
                    let latest = |pending: &mut VecDeque<Job>, is: fn(&Job) -> bool| {
                        let i = pending.iter().rposition(is)?;
                        let j = pending.remove(i).expect("index from rposition");
                        pending.retain(|j| !is(j));
                        Some(j)
                    };
                    let job = latest(&mut pending, |j| matches!(j, Job::Cut { .. }))
                        .or_else(|| latest(&mut pending, |j| matches!(j, Job::Bend { .. })))
                        .unwrap_or_else(|| pending.pop_front().expect("not empty"));
                    let out = match job {
                        Job::Delays {
                            event,
                            lat,
                            lon,
                            depth_km,
                        } => {
                            let started = Instant::now();
                            let grid = delay_grid(&tracer, &mantle, lat, lon, depth_km);
                            tracing::info!(
                                handle = event.handle,
                                max_abs_s = grid.max_abs_s,
                                ms = started.elapsed().as_millis() as u64,
                                "mantle delays"
                            );
                            Output::Delays { event, grid }
                        }
                        Job::Cut { key, lat, lon } => Output::Cut {
                            key,
                            grid: cut_grid(&mantle, key.wave, lat, lon, key.azimuth_deg()),
                        },
                        Job::Bend {
                            key,
                            lat,
                            lon,
                            depth_km,
                        } => {
                            let started = Instant::now();
                            if seeds.as_ref().is_none_or(|f| f.depth_km != depth_km) {
                                seeds = Some(SliceFans::build(&tracer, depth_km));
                            }
                            let seeds = seeds.as_ref().expect("just built");
                            let azimuth = f64::from(key.azimuth_decideg) / 10.0;
                            let grid = PlaneGrid::new(&mantle, lat, lon, azimuth);
                            let strength = f64::from(key.strength_centi) / 100.0;
                            let fans = bend_in_parallel(&tracer, &grid, strength, depth_km, seeds);
                            tracing::info!(
                                azimuth,
                                strength,
                                rays = fans.iter().map(|f| f.rays.len()).sum::<usize>(),
                                ms = started.elapsed().as_millis() as u64,
                                "bent slice fans"
                            );
                            Output::Bend {
                                key,
                                fans: SliceFans::from_fans(&tracer, depth_km, fans),
                            }
                        }
                    };
                    if tx.send(out).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn mantle worker");
        Self {
            jobs,
            results,
            requested: HashSet::new(),
            slots: [None; SLOTS],
            cut_wanted: None,
            cut_shown: None,
            bend_wanted: None,
            failed: false,
        }
    }

    fn send(&mut self, job: Job) {
        if self.jobs.send(job).is_err() {
            self.failed = true;
        }
    }

    /// Queue `event`'s map delays once; later calls for the same event do nothing.
    pub fn request_delays(&mut self, event: EventKey, lat: f64, lon: f64, depth_km: f64) {
        if self.failed || !self.requested.insert(event) {
            return;
        }
        self.send(Job::Delays {
            event,
            lat,
            lon,
            depth_km,
        });
    }

    /// Ask for the slice's rock grid; does nothing if `key` is already asked for.
    pub fn request_cut(&mut self, key: CutKey, lat: f64, lon: f64) {
        if self.failed || self.cut_wanted == Some(key) {
            return;
        }
        self.cut_wanted = Some(key);
        self.send(Job::Cut { key, lat, lon });
    }

    /// Ask for bent slice fans; does nothing if `key` is already asked for.
    pub fn request_bend(&mut self, key: BendKey, lat: f64, lon: f64, depth_km: f64) {
        if self.failed || self.bend_wanted == Some(key) {
            return;
        }
        self.bend_wanted = Some(key);
        self.send(Job::Bend {
            key,
            lat,
            lon,
            depth_km,
        });
    }

    /// Whether the uploaded rock grid is the one last asked for.
    pub fn cut_current(&self) -> bool {
        self.cut_shown.is_some() && self.cut_shown == self.cut_wanted
    }

    /// Take finished results. Delay grids get a GPU slot: a free one first, then one whose
    /// event is not in `on_map`; with none, the grid is dropped and the event asked for again
    /// later. Rock grids other than the latest asked for are dropped.
    pub fn collect(&mut self, on_map: &[EventKey]) -> Vec<Ready> {
        let mut out = Vec::new();
        loop {
            let done = match self.results.try_recv() {
                Ok(d) => d,
                Err(TryRecvError::Empty) => return out,
                Err(TryRecvError::Disconnected) => {
                    self.failed = true;
                    return out;
                }
            };
            match done {
                Output::Cut { key, grid } => {
                    if Some(key) == self.cut_wanted {
                        self.cut_shown = Some(key);
                        out.push(Ready::Cut(grid));
                    }
                }
                Output::Bend { key, fans } => {
                    if Some(key) == self.bend_wanted {
                        out.push(Ready::Bend(key, fans));
                    }
                }
                Output::Delays { event, grid } => {
                    let slot = self.slots.iter().position(Option::is_none).or_else(|| {
                        self.slots
                            .iter()
                            .position(|s| s.is_some_and(|(e, _)| !on_map.contains(&e)))
                    });
                    match slot {
                        Some(i) => {
                            if let Some((old, _)) = self.slots[i] {
                                self.requested.remove(&old);
                            }
                            self.slots[i] = Some((event, grid.max_abs_s));
                            out.push(Ready::Delays {
                                event,
                                slot: i as u32,
                                grid,
                            });
                        }
                        None => {
                            self.requested.remove(&event);
                        }
                    }
                }
            }
        }
    }

    /// The GPU slot holding `event`'s delays and their largest |delay| (s).
    pub fn slot(&self, event: EventKey) -> Option<(u32, f32)> {
        self.slots
            .iter()
            .position(|s| s.is_some_and(|(e, _)| e == event))
            .map(|i| (i as u32, self.slots[i].expect("found").1))
    }

    /// A short status line for `event`'s map delays, empty once they are drawn.
    pub fn status(&self, event: EventKey) -> &'static str {
        if self.failed {
            "3D model unavailable: map and slice are 1D"
        } else if self.slot(event).is_some() {
            ""
        } else if self.requested.contains(&event) {
            "Computing 3D delays…"
        } else {
            "3D delays start when the event's waves are on the map."
        }
    }
}

/// All bent fans for one slice, one thread per fan (the fans are independent). Forward fans
/// first, as `SliceFans` expects.
fn bend_in_parallel(
    tracer: &RayTracer,
    grid: &PlaneGrid,
    strength: f64,
    depth_km: f64,
    seeds: &SliceFans,
) -> Vec<Fan> {
    let jobs = bent_fan_jobs(&seeds.fans);
    std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .into_iter()
            .map(|(side, seed)| {
                scope.spawn(move || bent_fan(tracer, grid, strength, depth_km, side, seed))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("bent fan thread"))
            .collect()
    })
}
