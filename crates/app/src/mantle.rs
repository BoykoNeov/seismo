//! 3D-mantle delays for the map, computed on a worker thread: the worker loads the SP12RTS
//! model, then turns event requests into delay grids. The UI thread asks, collects finished
//! grids, and keeps track of which GPU slot holds which event's grid.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Instant;

use sim::RayTracer;
use sim::mantle::{DelayGrid, Mantle3d, delay_grid};
use sim::slice::Boundary;
use sim::tables::MantleModel;

static SP12RTS_VS: &[u8] = include_bytes!("../../../assets/sp12rts/SP12RTS..ES.sph");
static SP12RTS_VP: &[u8] = include_bytes!("../../../assets/sp12rts/SP12RTS..EP.sph");

/// GPU delay slots, one per event drawn at once.
const SLOTS: usize = render::MAX_MAP_EVENTS;

struct Request {
    event: usize,
    lat: f64,
    lon: f64,
    depth_km: f64,
}

struct Done {
    event: usize,
    grid: DelayGrid,
}

pub struct MantleDelays {
    requests: Sender<Request>,
    results: Receiver<Done>,
    requested: HashSet<usize>,
    /// Per GPU slot: the event whose grid it holds and that grid's largest |delay|.
    slots: [Option<(usize, f32)>; SLOTS],
    /// The worker has stopped (the model failed to load, or it panicked).
    failed: bool,
}

impl MantleDelays {
    /// Start the worker. It owns copies of the ray tracer and the 1D boundaries.
    pub fn spawn(tracer: RayTracer, boundaries: Vec<Boundary>) -> Self {
        let (requests, rx) = mpsc::channel::<Request>();
        let (tx, results) = mpsc::channel::<Done>();
        std::thread::Builder::new()
            .name("mantle-delays".into())
            .spawn(move || {
                let started = Instant::now();
                let model = match MantleModel::from_bytes(SP12RTS_VS, SP12RTS_VP) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::error!("SP12RTS model: {e}; the map stays 1D");
                        return;
                    }
                };
                tracing::info!(
                    ms = started.elapsed().as_millis() as u64,
                    "loaded SP12RTS mantle model"
                );
                let mantle = Mantle3d::new(model, &boundaries);
                for r in rx {
                    let started = Instant::now();
                    let grid = delay_grid(&tracer, &mantle, r.lat, r.lon, r.depth_km);
                    tracing::info!(
                        event = r.event,
                        max_abs_s = grid.max_abs_s,
                        ms = started.elapsed().as_millis() as u64,
                        "mantle delays"
                    );
                    if tx
                        .send(Done {
                            event: r.event,
                            grid,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            })
            .expect("spawn mantle worker");
        Self {
            requests,
            results,
            requested: HashSet::new(),
            slots: [None; SLOTS],
            failed: false,
        }
    }

    /// Queue `event` once; later calls for the same event do nothing.
    pub fn request(&mut self, event: usize, lat: f64, lon: f64, depth_km: f64) {
        if self.failed || !self.requested.insert(event) {
            return;
        }
        let r = Request {
            event,
            lat,
            lon,
            depth_km,
        };
        if self.requests.send(r).is_err() {
            self.failed = true;
        }
    }

    /// Take finished grids and hand each to `upload` with its slot. A free slot is used
    /// first, then one whose event is not in `on_map`; with none, the grid is dropped and the
    /// event asked for again later.
    pub fn collect(&mut self, on_map: &[usize], mut upload: impl FnMut(u32, &DelayGrid)) {
        loop {
            let done = match self.results.try_recv() {
                Ok(d) => d,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.failed = true;
                    return;
                }
            };
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
                    upload(i as u32, &done.grid);
                    self.slots[i] = Some((done.event, done.grid.max_abs_s));
                }
                None => {
                    self.requested.remove(&done.event);
                }
            }
        }
    }

    /// The GPU slot holding `event`'s delays and their largest |delay| (s).
    pub fn slot(&self, event: usize) -> Option<(u32, f32)> {
        self.slots
            .iter()
            .position(|s| s.is_some_and(|(e, _)| e == event))
            .map(|i| (i as u32, self.slots[i].expect("found").1))
    }

    /// A short status line for `event`, empty once its delays are drawn.
    pub fn status(&self, event: usize) -> &'static str {
        if self.failed {
            "3D model unavailable: map is 1D"
        } else if self.slot(event).is_some() {
            ""
        } else if self.requested.contains(&event) {
            "Computing 3D delays…"
        } else {
            "3D delays start when the event's waves are on the map."
        }
    }
}
