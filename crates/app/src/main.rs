#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! `app`: the eframe/egui shell. Owns the sim clock and wires sim, render and feed together.
//! Allowed dependencies: everything in the workspace.

mod beachball;
mod bench;
mod capture;
mod controls;
mod guide;
mod make_form;
mod mantle;
mod scene;
mod search;
mod stations;
mod style;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context as _, anyhow};
use chrono::{DateTime, Utc};
use eframe::{egui, wgpu};
use sim::crust::CrustSample;
use sim::mantle::DelayGrid;
use sim::seismogram::Component;
use sim::slice::SliceFans;
use sim::tables::{Dispersion, EarthModel, TravelTimeTable};
use sim::{Event, RayPhase, Simulation, Wave};

use beachball::BeachBall;
use guide::GuideUi;
use mantle::{BendKey, MantleWorker};
use scene::phase_index;
use search::SearchForm;
use session::clock::{SimClock, secs};
use session::events::{EventKey, EventList, Listed};
use session::guide::{Front, Guide, Values};
use session::made::{self, MakeForm};
use session::traces::Traces;
use style::{UiPalette, visuals_for};

use session::assets::crust as real_crust;
use session::assets::{
    COAST as COAST_BYTES, DISPERSION as DISPERSION_BYTES, MODEL as MODEL_BYTES,
    STATIONS as STATIONS_BYTES, TABLE as TABLE_BYTES, TOHOKU_PLACE, tohoku,
};

/// Narrowest traces column (points) beside the map and slice.
const MIN_TRACES_WIDTH: f32 = 260.0;
/// A click within this many points of a station marker picks the station.
const PICK_RADIUS: f32 = 9.0;
/// Shortest and longest trace window offered (minutes).
const TRACE_WINDOW_MIN: (f64, f64) = (5.0, 120.0);

/// Largest 3D-mantle strength offered (× the published model).
const MAX_MANTLE_STRENGTH: f32 = 20.0;

/// Scrub range around the selected event's origin (s).
const SCRUB_BEFORE_S: f64 = 60.0;
const SCRUB_AFTER_S: f64 = 90.0 * 60.0;

/// Events older than this show their year in lists; newer ones show the time of day instead.
const SHOW_YEAR_AFTER_DAYS: i64 = 300;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn,naga=warn".into()),
        )
        .init();

    let mut options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("SEISMO")
            .with_inner_size([1600.0, 900.0])
            .with_min_inner_size([640.0, 360.0]),
        ..Default::default()
    };
    // `--bench` measures how fast frames can be drawn, so it presents without waiting for vsync.
    if std::env::args().any(|a| a == "--bench") {
        options.wgpu_options.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
    }
    eframe::run_native(
        "SEISMO",
        options,
        Box::new(|cc| {
            SeismoApp::new(cc)
                .map(|a| Box::new(a) as Box<dyn eframe::App>)
                .map_err(Into::into)
        }),
    )
}

/// Slice view settings.
struct SliceSettings {
    /// Direction of the cut from the epicentre, degrees clockwise from north.
    azimuth_deg: f64,
    /// Phases drawn, in `RayPhase::ALL` order.
    phases: [bool; RayPhase::ALL.len()],
    ray_paths: bool,
    /// Ray paths per phase when shown.
    paths_per_phase: usize,
    /// Fainter copies of the fronts at earlier times, as a trail.
    trail_fronts: usize,
    labels: bool,
    /// Which wave's speed the mantle shading shows; `None` hides it.
    rock: Option<Wave>,
    /// The look of the slice, per [`render::Look`] (its fills differ between the two).
    style: [render::SliceStyle; 2],
}

impl Default for SliceSettings {
    fn default() -> Self {
        Self {
            azimuth_deg: 90.0,
            phases: RayPhase::ALL.map(RayPhase::shown_by_default),
            ray_paths: true,
            paths_per_phase: 8,
            trail_fronts: 3,
            labels: true,
            rock: Some(Wave::S),
            style: render::Look::ALL.map(render::SliceStyle::for_look),
        }
    }
}

struct SeismoApp {
    sim: Simulation,
    /// 3D-mantle map delays and slice rock, computed on a worker thread.
    mantle: MantleWorker,
    /// Ray fans for the selected event's source depth, built on first use.
    fans: Option<SliceFans>,
    /// Ray fans bent through the 3D mantle, for the slice as it was when asked for.
    bent: Option<(BendKey, SliceFans)>,
    /// The crust along the slice's cut for its strip, by epicentre, azimuth (f64 bits) and
    /// whether it is the real crust.
    crust_cut: Option<([u64; 4], Vec<CrustSample>)>,
    slice: SliceSettings,
    /// Virtual seismograph stations and their traces.
    traces: Traces,
    trace_style: render::TraceStyle,
    /// CPU copies of the events' 3D-mantle delays (the map has them on the GPU), so traces put
    /// P and S where the map does.
    delay_grids: HashMap<EventKey, Arc<DelayGrid>>,
    clock: SimClock,
    /// Built-in and USGS events, and which one is selected.
    events: EventList,
    /// Keys of `sim.events`, in the same order, and the list revision they were taken from.
    keys: Vec<EventKey>,
    events_rev: u64,
    /// The selected event's handle when the traces last followed it.
    traces_follow: u64,
    /// `--event` with `--start-offset`: seek this many seconds after its origin once listed.
    pending_seek: Option<f64>,
    search_form: SearchForm,
    make_form: MakeForm,
    /// Where the user's own events are kept; `None` if the platform has no data folder.
    made_path: Option<std::path::PathBuf>,
    /// The user's events changed since they were last kept on disk.
    made_dirty: bool,
    /// The selected event's handle when the make-your-own form last followed it.
    form_follow: u64,
    /// The make-your-own section is open: its place is marked on the map.
    make_open: bool,
    renderer: render::SceneRenderer,
    texture_id: egui::TextureId,
    /// The look drawn: the CRT or the modern one.
    look: render::Look,
    /// The look was switched since the last frame: the CRT afterglow is dropped.
    look_changed: bool,
    /// When the current look was switched on (the CRT warms up from then): the first frame
    /// drawn, or the switch of looks.
    look_since: Option<Instant>,
    /// Post-process settings per [`render::Look`]: the CRT's effects, the modern look's glow.
    post: [render::CrtSettings; 2],
    style: render::MapStyle,
    started: Instant,
    last_frame: Instant,
    last_seek_count: u64,
    /// Smoothed real frame time (s), for the fps readout.
    frame_time_s: f64,
    /// `--capture <file.ppm>` / `--capture-after <s>`: save the scene once, then quit.
    capture: Option<(std::path::PathBuf, f64)>,
    /// `--capture-ui <file.ppm>`: the whole window, egui text included; true once asked for.
    capture_ui: Option<(std::path::PathBuf, f64, bool)>,
    /// `--size WxH`: the window's inner size in physical pixels, set on the first frame (the
    /// screen's scale is not known before the window exists).
    pending_size: Option<[u32; 2]>,
    /// `--map-cap <n>`: most events drawn on the map (default and ceiling `MAX_MAP_EVENTS`).
    map_cap: usize,
    /// `--bench <s>`: frame-time measurement, reported before quitting.
    bench: Option<bench::Bench>,
    /// The guide window, the `?` glyphs and the welcome card.
    guide: GuideUi,
    /// The selected earthquake's fault pattern, drawn in the event panel.
    beach_ball: BeachBall,
}

impl SeismoApp {
    fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .ok_or_else(|| anyhow!("wgpu backend not available"))?;
        let table = TravelTimeTable::from_bytes(TABLE_BYTES).context("embedded ak135.tt")?;
        let model = EarthModel::from_bytes(MODEL_BYTES).context("embedded ak135_model.csv")?;
        let dispersion =
            Dispersion::from_bytes(DISPERSION_BYTES).context("embedded surface_dispersion.csv")?;
        tracing::info!(
            model = table.model(),
            obspy = table.obspy_version(),
            phases = ?table.phases(),
            velocity_points = model.points().len(),
            "loaded ak135 assets"
        );
        tracing::info!(adapter = ?rs.adapter.get_info().name, backend = ?rs.adapter.get_info().backend, "gpu");

        let mut renderer = render::SceneRenderer::new(&rs.device, &rs.queue, &table, (256, 128));
        let coast =
            render::Coastlines::from_bytes(COAST_BYTES).context("embedded coastlines.txt")?;
        tracing::info!(
            lines = coast.lines().len(),
            points = coast.point_count(),
            "loaded coastlines"
        );
        renderer.map.set_coastlines(coast);
        let texture_id = rs.renderer.write().register_native_texture(
            &rs.device,
            renderer.output_view(),
            wgpu::FilterMode::Nearest,
        );

        // Debug/screenshot flags:
        //   --look crt|modern   the look to start with (default crt)
        //   --start-offset <s>  replay the selected event (the built-in one, or one of yours
        //                       picked with --event): start the clock that many seconds after
        //                       its origin (without it the clock starts at now, live)
        //   --offline           no network: events come from the saved copy only
        //   --speed <x>         initial clock speed
        //   --crt <list>        comma-separated: off, noscan, nopersist, nobloom, nobarrel,
        //                       noflicker, nohot, novignette, nonoise, nohum, nowarmup
        let arg = |name: &str| std::env::args().skip_while(|a| a != name).nth(1);
        let flag = |name: &str| std::env::args().any(|a| a == name);
        let look = match arg("--look").as_deref() {
            Some("modern") => render::Look::Modern,
            _ => render::Look::Crt,
        };
        cc.egui_ctx.set_visuals(visuals_for(look));
        let feed = feed::spawn(feed::FeedConfig {
            online: !flag("--offline"),
            ..Default::default()
        });
        let mut events = EventList::new(vec![Listed::builtin(tohoku(), TOHOKU_PLACE)], Some(feed));
        let mut make_form = MakeForm::default();
        let made_path = made::default_path();
        if let Some(path) = &made_path {
            match made::load(path) {
                Ok(made) => {
                    tracing::info!(count = made.len(), path = %path.display(), "loaded your events");
                    events.load_made(made);
                }
                Err(e) => {
                    // Kept aside, not overwritten by the next save.
                    let aside = path.with_extension("json.bad");
                    let moved = std::fs::rename(path, &aside).is_ok();
                    tracing::warn!(path = %path.display(), moved, "could not read your events: {e}");
                    make_form.message = Some(format!(
                        "Could not read your saved events ({e}); {}",
                        if moved {
                            format!("the file was moved to {}", aside.display())
                        } else {
                            "starting without them".to_owned()
                        }
                    ));
                }
            }
        }
        //   --event <id>        select this event: a built-in one or one of yours (made-<n>)
        //                       at once; a USGS one once the feed
        //                       lists it, or once a USGS lookup finds it (then it is kept, and
        //                       replayed from its origin); with --start-offset, replay it from
        //                       that many seconds after its origin
        if let Some(id) = arg("--event") {
            events.want(id);
        }
        let replay = arg("--start-offset").and_then(|s| s.parse::<f64>().ok());
        let pending_seek = replay.filter(|_| events.wanted_id.is_some());
        let mut clock = match replay.filter(|_| pending_seek.is_none()) {
            Some(offset) => {
                // Replaying the selected event: the feed must not pick another.
                events.user_chose = true;
                let selected = events.selected_handle();
                let origin = events
                    .shown()
                    .iter()
                    .find(|l| l.key.handle == selected)
                    .map_or(tohoku().origin, |l| l.event.origin);
                SimClock::new(origin + secs(offset))
            }
            None => SimClock::new(Utc::now()),
        };

        let stations = sim::parse_stations(STATIONS_BYTES).context("embedded stations.csv")?;
        let shown: Vec<Event> = events.shown().iter().map(|l| l.event.clone()).collect();
        let keys: Vec<EventKey> = events.shown().iter().map(|l| l.key).collect();
        let crust = real_crust(&model).context("embedded crust1.bin")?;
        let sim = Simulation::new(table, model, dispersion, shown)
            .context("ray tracer")?
            .with_crust(crust)
            .with_ellipticity(session::assets::ellipticity().context("embedded ak135_ellip.bin")?);
        let started = Instant::now();
        renderer
            .map
            .upload_surface(&rs.queue, &sim.surface_waves().map_grid());
        tracing::info!(
            ms = started.elapsed().as_millis() as u64,
            "built surface-wave map grid"
        );
        let mut traces = Traces::new(stations, sim.tracer().clone());
        traces.pick_defaults(&sim.events[0]);
        let mantle = MantleWorker::spawn(sim.tracer().clone(), sim.boundaries().to_vec());
        if let Some(speed) = arg("--speed").and_then(|s| s.parse::<f64>().ok()) {
            clock.set_speed(speed);
        }
        //   --paused            start with the clock stopped (for repeatable screenshots)
        if flag("--paused") {
            clock.set_playing(false);
        }
        //   --capture <file.ppm> [--capture-after <s>]  save the rendered scene after <s> real
        //                       seconds (default 3) as a binary PPM, then quit
        let capture = arg("--capture").map(|p| {
            let after = arg("--capture-after")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(3.0);
            (std::path::PathBuf::from(p), after)
        });
        //   --capture-ui <file.ppm>  the same for the whole window, controls and labels included
        let capture_ui = arg("--capture-ui").map(|p| {
            let after = arg("--capture-after")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(3.0);
            (std::path::PathBuf::from(p), after, false)
        });
        //   --mantle <x>        3D-mantle strength (0 = 1D, 1 = published model)
        let mut style = render::MapStyle::default();
        if let Some(x) = arg("--mantle").and_then(|s| s.parse::<f32>().ok()) {
            style.mantle_strength = x.clamp(0.0, MAX_MANTLE_STRENGTH);
        }
        //   --surface <x>       surface-wave band brightness (0 hides them)
        if let Some(x) = arg("--surface").and_then(|s| s.parse::<f32>().ok()) {
            style.surface_brightness = x.clamp(0.0, 1.0);
        }
        //   --coast <list>      comma-separated: off, wide
        for flag in arg("--coast").unwrap_or_default().split(',') {
            match flag {
                "off" => style.coast_brightness = 0.0,
                "wide" => style.coast_wide = true,
                _ => {}
            }
        }
        //   --azimuth <deg>     direction of the slice from the epicentre
        let mut slice = SliceSettings::default();
        if let Some(az) = arg("--azimuth").and_then(|s| s.parse::<f64>().ok()) {
            slice.azimuth_deg = az.rem_euclid(360.0);
        }
        //   --phases <list>     slice phases to draw: "all", or comma-separated names (P,PKiKP,...)
        if let Some(list) = arg("--phases") {
            slice.phases = RayPhase::ALL
                .map(|p| list == "all" || list.split(',').any(|name| name == p.name()));
        }
        let mut post = render::Look::ALL.map(render::CrtSettings::for_look);
        let crt = &mut post[render::Look::Crt.index()];
        for flag in arg("--crt").unwrap_or_default().split(',') {
            match flag {
                "off" => crt.enabled = false,
                "noscan" => crt.scanlines = false,
                "nopersist" => crt.persistence = false,
                "nobloom" => crt.bloom = false,
                "nobarrel" => crt.barrel = false,
                "noflicker" => crt.flicker = false,
                "nohot" => crt.hot_spots = false,
                "novignette" => crt.vignette = false,
                "nonoise" => crt.noise = false,
                "nohum" => crt.hum_bar = false,
                "nowarmup" => crt.warm_up = false,
                _ => {}
            }
        }
        //   --traces <list>     comma-separated: off, side (transverse), shared (one scale),
        //                       flatcrust (the 1D model's crust everywhere), <n>min (the
        //                       window's length in minutes, as the slider allows)
        for flag in arg("--traces").unwrap_or_default().split(',') {
            match flag {
                "off" => traces.show = false,
                "side" => traces.component = Component::Transverse,
                "shared" => traces.shared_scale = true,
                "flatcrust" => traces.real_crust = false,
                _ => {
                    if let Some(min) = flag.strip_suffix("min").and_then(|n| n.parse::<f64>().ok())
                    {
                        traces.window_min = min.clamp(TRACE_WINDOW_MIN.0, TRACE_WINDOW_MIN.1);
                    }
                }
            }
        }
        //   --size <W>x<H>      the window's inner size in physical pixels (the controls panel
        //                       included), e.g. 1920x1080
        let pending_size = arg("--size").and_then(|s| bench::parse_size(&s));
        //   --map-cap <n>       draw at most n events on the map (1 to the default 32)
        let map_cap = arg("--map-cap")
            .and_then(|s| s.parse::<usize>().ok())
            .map_or(render::MAX_MAP_EVENTS, |n| {
                n.clamp(1, render::MAX_MAP_EVENTS)
            });
        //   --bench <s> [--bench-warmup <s>] [--bench-out <file>]  without vsync, measure
        //                       frame times for <s> seconds after a warm-up (default 5 s), then
        //                       time the scene alone offscreen, log the report (and append it
        //                       to <file>), and quit
        let bench = arg("--bench").and_then(|s| s.parse::<f64>().ok()).map(|s| {
            let warmup = arg("--bench-warmup")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(5.0);
            bench::Bench::new(s, warmup, arg("--bench-out").map(Into::into))
        });
        //   --guide <topic>     open the guide at that topic (e.g. map, slice, shadow)
        //   --no-welcome        never show the first-start welcome card (nor do --capture,
        //                       --capture-ui, --bench, --event and --guide)
        //   --welcome           show it anyway (for screenshots)
        let mut values = Values::new(sim.facts());
        values.set("map_cap", map_cap.to_string());
        values.set("stations", traces.stations.len().to_string());
        let text = Guide::new(Front::Desktop, &values).map_err(|e| anyhow!("guide.txt: {e}"))?;
        let automated = [
            "--capture",
            "--capture-ui",
            "--bench",
            "--event",
            "--guide",
            "--no-welcome",
        ]
        .iter()
        .any(|f| flag(f));
        let welcome = made_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|dir| dir.join("welcomed"))
            .filter(|marker| flag("--welcome") || (!automated && !marker.exists()));
        let guide = GuideUi::new(text, arg("--guide").as_deref(), welcome);
        let now = Instant::now();
        Ok(Self {
            sim,
            mantle,
            fans: None,
            bent: None,
            crust_cut: None,
            slice,
            traces,
            trace_style: render::TraceStyle::default(),
            delay_grids: HashMap::new(),
            clock,
            events_rev: events.rev(),
            traces_follow: events.selected_handle(),
            pending_seek,
            search_form: SearchForm::default(),
            make_form,
            made_path,
            made_dirty: false,
            // No event yet: the first sync fills the form if one of the user's is selected.
            form_follow: 0,
            //   --make              open the make-your-own-event section (and fold the event
            //                       list, so the form fits on screen)
            make_open: flag("--make"),
            events,
            keys,
            renderer,
            texture_id,
            look,
            look_changed: false,
            look_since: None,
            post,
            style,
            started: now,
            last_frame: now,
            last_seek_count: 0,
            frame_time_s: 1.0 / 60.0,
            capture,
            capture_ui,
            pending_size,
            map_cap,
            bench,
            guide,
            beach_ball: BeachBall::default(),
        })
    }

    /// The current look's UI colours.
    fn pal(&self) -> UiPalette {
        UiPalette::for_look(self.look)
    }

    /// The font of the text written over the scene: the CRT's typewriter, the modern look's
    /// proportional face.
    fn font(&self, size: f32) -> egui::FontId {
        match self.look {
            render::Look::Crt => egui::FontId::monospace(size),
            render::Look::Modern => egui::FontId::proportional(size),
        }
    }

    /// The current look's post-process settings.
    fn post(&self) -> &render::CrtSettings {
        &self.post[self.look.index()]
    }

    /// Switch looks: the UI theme follows, and the next frame starts without afterglow.
    fn set_look(&mut self, ctx: &egui::Context, look: render::Look) {
        if look == self.look {
            return;
        }
        self.look = look;
        self.look_changed = true;
        self.look_since = Some(Instant::now());
        ctx.set_visuals(visuals_for(look));
    }

    /// Index of the selected event in `sim.events`. The list always shows the selected event.
    fn sel(&self) -> usize {
        let handle = self.events.selected_handle();
        self.keys
            .iter()
            .position(|k| k.handle == handle)
            .unwrap_or(0)
    }

    fn selected_key(&self) -> EventKey {
        self.keys[self.sel()]
    }

    fn origin(&self) -> DateTime<Utc> {
        self.sim.events[self.sel()].origin
    }

    /// Take the feed's news and bring the simulation's events up to date with the list.
    fn sync_events(&mut self) {
        if std::mem::take(&mut self.made_dirty) {
            self.save_made();
        }
        self.events.poll(self.clock.now());
        if self.events.rev() != self.events_rev {
            self.events_rev = self.events.rev();
            let shown = self.events.shown();
            self.sim.events = shown.iter().map(|l| l.event.clone()).collect();
            self.keys = shown.iter().map(|l| l.key).collect();
            let keys = &self.keys;
            self.delay_grids.retain(|k, _| keys.contains(k));
        }
        // A picked search result replays from its origin, unless `--start-offset` says where.
        let replay = self.events.take_replay();
        if let Some(offset) = self.pending_seek
            && self.events.wanted_id.is_none()
        {
            self.pending_seek = None;
            self.clock.seek(self.origin() + secs(offset));
        } else if replay {
            self.clock.seek(self.origin());
            self.clock.set_playing(true);
        }
        // Default stations follow a newly selected event.
        let handle = self.events.selected_handle();
        if handle != self.traces_follow {
            self.traces_follow = handle;
            if !self.traces.user_picked {
                self.traces.pick_defaults(&self.sim.events[self.sel()]);
            }
        }
        // Selecting one of the user's events fills the form to edit it.
        if handle != self.form_follow {
            self.form_follow = handle;
            let f = &mut self.make_form;
            f.editing = None;
            if let Some(l) = self.events.shown().iter().find(|l| l.key.handle == handle)
                && l.made
                && !l.gone
            {
                f.fill(&l.event, &l.place);
                f.editing = Some(handle);
            }
        }
    }

    /// Keep the user's events on disk.
    fn save_made(&mut self) {
        let Some(path) = &self.made_path else {
            self.make_form.message =
                Some("No data folder on this computer: your events last only this session.".into());
            return;
        };
        if let Err(e) = made::save(path, &self.events.made()) {
            tracing::error!(path = %path.display(), "could not save your events: {e}");
            self.make_form.message = Some(format!("Could not save to {}: {e}", path.display()));
        }
    }

    /// The bent fans for the slice as it is now, if the 3D mantle is on.
    fn bend_key(&self) -> Option<BendKey> {
        let s = self.style.mantle_strength;
        (s > 0.0).then(|| BendKey::new(self.selected_key(), self.slice.azimuth_deg, s))
    }

    /// Whether the slice is drawn with rays bent for its current settings.
    fn bent_current(&self) -> bool {
        self.bend_key()
            .is_some_and(|k| self.bent.as_ref().is_some_and(|(b, _)| *b == k))
    }

    /// The ray fans to draw: bent ones when they match the slice, otherwise the 1D fans.
    fn fans(&mut self) -> &SliceFans {
        if self.bent_current() {
            return &self.bent.as_ref().expect("checked").1;
        }
        let depth = self.sim.events[self.sel()].depth_km;
        // Only the phases switched on: ticking another one rebuilds them with it.
        let enabled = self.slice.phases;
        let shown = |phase: RayPhase| enabled[phase_index(phase)];
        if self.fans.as_ref().is_none_or(|f| {
            f.depth_km != depth || RayPhase::ALL.into_iter().any(|p| shown(p) && !f.has(p))
        }) {
            let started = Instant::now();
            let fans = self.sim.slice_fans(depth, shown);
            tracing::info!(
                depth_km = depth,
                rays = fans.fans.iter().map(|f| f.rays.len()).sum::<usize>(),
                ms = started.elapsed().as_millis() as u64,
                "built slice ray fans"
            );
            self.fans = Some(fans);
        }
        self.fans.as_ref().expect("just built")
    }
}

impl eframe::App for SeismoApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let now = Instant::now();
        let real_dt = (now - self.last_frame).as_secs_f64();
        self.last_frame = now;
        self.clock.tick(real_dt);
        if real_dt > 0.25 {
            tracing::debug!(ms = (real_dt * 1e3) as u64, "slow frame");
        }
        self.sync_events();
        self.frame_time_s += (real_dt.min(1.0) - self.frame_time_s) * 0.05;
        if let Some([w, h]) = self.pending_size.take() {
            let ppp = ui
                .ctx()
                .native_pixels_per_point()
                .unwrap_or(ui.ctx().pixels_per_point());
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    w as f32 / ppp,
                    h as f32 / ppp,
                )));
        }
        if let Some(b) = &mut self.bench {
            b.frame(
                self.started.elapsed().as_secs_f64(),
                real_dt,
                frame.info().cpu_usage,
            );
        }

        egui::Panel::left("controls")
            .resizable(false)
            .exact_size(260.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui));
            });
        // The controls may have picked an event or changed the filter.
        self.sync_events();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(self.pal().background))
            .show(ui, |ui| self.scene(ui, frame, real_dt));

        let ctx = ui.ctx().clone();
        let dim = self.pal().dim;
        self.guide.keys(&ctx);
        self.guide.window(&ctx, dim);
        self.guide.welcome(&ctx, dim);

        self.capture_window(&ctx);
        ctx.request_repaint();
    }
}
