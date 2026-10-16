#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! `app`: the eframe/egui shell. Owns the sim clock and wires sim, render and feed together.
//! Allowed dependencies: everything in the workspace.

mod bench;
mod guide;
mod mantle;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context as _, anyhow};
use chrono::{DateTime, NaiveDate, Utc};
use eframe::{egui, wgpu};
use sim::mantle::DelayGrid;
use sim::seismogram::Component;
use sim::slice::{PhaseLabel, SliceFans, SliceLine};
use sim::tables::{Dispersion, EarthModel, TravelTimeTable};
use sim::{EARTH_RADIUS_KM, Event, EventKind, RayPhase, Simulation, Wave, azimuth_deg};

use guide::GuideUi;
use mantle::{BendKey, CutKey, MantleWorker, Ready};
use session::clock::{MAX_SPEED, MIN_SPEED, SimClock, secs};
use session::events::{EventKey, EventList, FOLLOW_MIN_MAG, Listed};
use session::guide::{Front, Guide, Values};
use session::made::{self, MakeForm, TIME_FORMAT};
use session::traces::{MAX_TRACES, Traces};

use session::assets::{
    COAST as COAST_BYTES, DISPERSION as DISPERSION_BYTES, MODEL as MODEL_BYTES,
    STATIONS as STATIONS_BYTES, TABLE as TABLE_BYTES, TOHOKU_PLACE, tohoku,
};

/// Narrowest traces column (points) beside the map and slice.
const MIN_TRACES_WIDTH: f32 = 260.0;
/// A click within this many points of a station marker picks the station.
const PICK_RADIUS: f32 = 9.0;

/// Largest 3D-mantle strength offered (× the published model).
const MAX_MANTLE_STRENGTH: f32 = 20.0;

/// Scrub range around the selected event's origin (s).
const SCRUB_BEFORE_S: f64 = 60.0;
const SCRUB_AFTER_S: f64 = 90.0 * 60.0;

/// Events older than this show their year in lists; newer ones show the time of day instead.
const SHOW_YEAR_AFTER_DAYS: i64 = 300;

// The CRT look's UI colours.
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x33, 0xff, 0x33);
const GREEN_DIM: egui::Color32 = egui::Color32::from_rgb(0x1f, 0x99, 0x1f);
const GREEN_DARK: egui::Color32 = egui::Color32::from_rgb(0x0e, 0x40, 0x0e);
const BACKGROUND: egui::Color32 = egui::Color32::from_rgb(0x05, 0x08, 0x05);

// The modern look's UI colours: slate panels, light text, a blue accent.
const SLATE_BG: egui::Color32 = egui::Color32::from_rgb(0x0b, 0x0f, 0x17);
const SLATE_PANEL: egui::Color32 = egui::Color32::from_rgb(0x12, 0x17, 0x21);
const SLATE_WIDGET: egui::Color32 = egui::Color32::from_rgb(0x1c, 0x24, 0x31);
const SLATE_HOVER: egui::Color32 = egui::Color32::from_rgb(0x26, 0x30, 0x41);
const SLATE_ACTIVE: egui::Color32 = egui::Color32::from_rgb(0x2f, 0x3c, 0x52);
const SLATE_LINE: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x34, 0x44);
const TEXT: egui::Color32 = egui::Color32::from_rgb(0xe3, 0xea, 0xf3);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0x93, 0xa1, 0xb5);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x5a, 0xb0, 0xff);
const ACCENT_DIM: egui::Color32 = egui::Color32::from_rgb(0x24, 0x4a, 0x78);

/// The colours of the text the app writes over the scene and in its notes, per look.
#[derive(Debug, Clone, Copy)]
struct UiPalette {
    text: egui::Color32,
    dim: egui::Color32,
    background: egui::Color32,
}

impl UiPalette {
    fn for_look(look: render::Look) -> Self {
        match look {
            render::Look::Crt => Self {
                text: GREEN,
                dim: GREEN_DIM,
                background: BACKGROUND,
            },
            render::Look::Modern => Self {
                text: TEXT,
                dim: TEXT_DIM,
                background: SLATE_BG,
            },
        }
    }
}

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
            phases: [true; RayPhase::ALL.len()],
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
        let sim = Simulation::new(table, model, dispersion, shown).context("ray tracer")?;
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
        //   --traces <list>     comma-separated: off, side (transverse), shared (one scale)
        for flag in arg("--traces").unwrap_or_default().split(',') {
            match flag {
                "off" => traces.show = false,
                "side" => traces.component = Component::Transverse,
                "shared" => traces.shared_scale = true,
                _ => {}
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

    /// The ray fans for the selected event, rebuilt when its depth changes.
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
        if self.fans.as_ref().is_none_or(|f| f.depth_km != depth) {
            let started = Instant::now();
            let fans = self.sim.slice_fans(depth);
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

    /// A collapsible section of the controls with a `?` beside its name, whose explanation
    /// unfolds under the name. Returns whether the section is open.
    fn section(
        &mut self,
        ui: &mut egui::Ui,
        title: &str,
        topic: &'static str,
        default_open: bool,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) -> bool {
        let id = ui.make_persistent_id(title);
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            id,
            default_open,
        );
        let header = ui.horizontal(|ui| {
            state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
            let name = egui::Label::new(egui::RichText::new(title).strong())
                .selectable(false)
                .sense(egui::Sense::click());
            if ui.add(name).clicked() {
                state.toggle(ui);
            }
            self.guide.glyph(ui, topic);
        });
        self.guide.note(ui, topic);
        state.show_body_indented(&header.response, ui, |ui| body(self, ui));
        state.is_open()
    }

    fn slice_controls(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        let three_d = self.style.mantle_strength > 0.0;
        let look = self.look;
        let s = &mut self.slice;
        let style = &mut s.style[look.index()];
        ui.add(
            egui::Slider::new(&mut s.azimuth_deg, 0.0..=360.0)
                .text("azimuth °")
                .max_decimals(0),
        );
        ui.colored_label(
            pal.dim,
            if three_d {
                "Each azimuth cuts through different rock; the other half of the circle is at \
                 azimuth + 180°."
            } else {
                "1D: every azimuth looks the same; azimuth only moves the cut on the map."
            },
        );
        ui.horizontal(|ui| {
            ui.label("rock:");
            ui.radio_value(&mut s.rock, None, "off");
            ui.radio_value(&mut s.rock, Some(Wave::S), "S speed");
            ui.radio_value(&mut s.rock, Some(Wave::P), "P speed");
        });
        ui.add_enabled(
            s.rock.is_some() && three_d,
            egui::Slider::new(&mut style.rock_gain, 0.0..=3.0).text("rock shading"),
        );
        ui.horizontal_wrapped(|ui| {
            for (on, phase) in s.phases.iter_mut().zip(RayPhase::ALL) {
                ui.checkbox(on, phase.name());
            }
        });
        ui.checkbox(&mut s.labels, "labels");
        ui.checkbox(&mut s.ray_paths, "ray paths");
        ui.add_enabled(
            s.ray_paths,
            egui::Slider::new(&mut s.paths_per_phase, 1..=40).text("rays/phase"),
        );
        ui.add(egui::Slider::new(&mut s.trail_fronts, 0..=6).text("trail fronts"));
        ui.add(egui::Slider::new(&mut style.front_width_px, 1.0..=6.0).text("front px"));
        ui.add(egui::Slider::new(&mut style.boundary_brightness, 0.0..=1.0).text("layers"));
        let fill_max = match look {
            render::Look::Crt => 0.3,
            render::Look::Modern => 1.0,
        };
        ui.add(egui::Slider::new(&mut style.fill_brightness, 0.0..=fill_max).text("fill"));
        ui.colored_label(
            pal.dim,
            match look {
                render::Look::Crt => {
                    "Solid: P (compression). Dashed: S (shear). Dotted: ray paths. \
                     Shown: from the epicentre along the azimuth to the antipode. Brighter rock \
                     is faster (±1% spans the shading). With the 3D mantle on, rays bend \
                     through the rock in the plane of the cut."
                }
                render::Look::Modern => {
                    "Amber solid: P (compression). Cyan dashed: S (shear). Dotted: ray paths. \
                     Shown: from the epicentre along the azimuth to the antipode. Blue rock is \
                     faster, red slower (±1% spans the shading). With the 3D mantle on, rays \
                     bend through the rock in the plane of the cut."
                }
            },
        );
    }

    fn trace_controls(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        let event = self.sim.events[self.sel()].clone();
        let tr = &mut self.traces;
        ui.checkbox(&mut tr.show, "show traces");
        ui.horizontal(|ui| {
            ui.label("shaking:");
            ui.radio_value(&mut tr.component, Component::Vertical, "up-down");
            ui.radio_value(&mut tr.component, Component::Transverse, "side-to-side");
        });
        ui.horizontal(|ui| {
            ui.label("scale:");
            ui.radio_value(&mut tr.shared_scale, false, "each row");
            ui.radio_value(&mut tr.shared_scale, true, "shared");
        });
        ui.add(
            egui::Slider::new(&mut tr.gain, 0.25..=20.0)
                .logarithmic(true)
                .text("gain")
                .max_decimals(2),
        );
        ui.add(
            egui::Slider::new(&mut tr.window_min, 5.0..=120.0)
                .text("window min")
                .max_decimals(0),
        );
        ui.checkbox(&mut tr.click_to_place, "click empty map: add a station");

        // Shown stations, nearest first, each with aim-the-slice and remove buttons.
        let mut rows: Vec<(usize, f64)> = tr
            .chosen
            .iter()
            .map(|&i| {
                let s = &tr.stations[i];
                (i, sim::great_circle_deg(event.lat, event.lon, s.lat, s.lon))
            })
            .collect();
        rows.sort_by(|a, b| a.1.total_cmp(&b.1));
        let (mut toggle, mut delete, mut aim) = (None, None, None);
        for (i, dist) in rows {
            let s = &tr.stations[i];
            ui.horizontal(|ui| {
                ui.monospace(format!("{:<9}{dist:6.1}°", s.label()));
                if ui
                    .small_button("aim")
                    .on_hover_text("Turn the slice towards this station")
                    .clicked()
                {
                    aim = Some(azimuth_deg(event.lat, event.lon, s.lat, s.lon));
                }
                if ui.small_button("hide").clicked() {
                    toggle = Some(i);
                }
                if s.is_user() && ui.small_button("delete").clicked() {
                    delete = Some(i);
                }
            });
        }
        egui::ComboBox::from_id_salt("add station")
            .selected_text("add a station…")
            .width(220.0)
            .show_ui(ui, |ui| {
                for (i, s) in tr.stations.iter().enumerate() {
                    if !tr.chosen.contains(&i)
                        && ui
                            .selectable_label(false, format!("{} {}", s.label(), s.name))
                            .clicked()
                    {
                        toggle = Some(i);
                    }
                }
            });
        if ui.button("Default stations").clicked() {
            tr.pick_defaults(&event);
        }
        if let Some(i) = toggle {
            tr.toggle(i);
        }
        if let Some(i) = delete {
            tr.remove_user(i);
        }
        if let Some(az) = aim {
            self.slice.azimuth_deg = az;
        }
        let tr = &self.traces;
        if let Some(note) = &tr.note {
            ui.colored_label(pal.text, note);
        }
        ui.colored_label(
            pal.dim,
            format!(
                "Synthetic traces: display-plausible, not waveform-accurate. Up to {MAX_TRACES}. \
                 Click a station on the map to show or hide it. Triangles: real stations \
                 (Global Seismographic Network); diamonds: yours. Ticks mark arrivals."
            ),
        );
    }

    fn event_controls(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        ui.colored_label(pal.dim, self.feed_line());
        let mut min_mag = self.events.min_mag();
        ui.add(
            egui::Slider::new(&mut min_mag, 0.0..=8.0)
                .step_by(0.1)
                .text("min M"),
        );
        self.events.set_min_mag(min_mag);
        let (above, all) = self.events.feed_counts();
        ui.colored_label(
            pal.dim,
            format!("USGS: {above} of {all} events at M{min_mag:.1}+ (past day, and the month's significant ones)"),
        );
        let selected = self.events.selected_handle();
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("event list")
            .max_height(220.0)
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let now = Utc::now();
                for l in self.events.shown() {
                    let e = &l.event;
                    let mark = if l.made {
                        '+'
                    } else if l.saved {
                        '*'
                    } else {
                        ' '
                    };
                    let mut text =
                        format!("{mark}M{:<4.1}{}  ", e.magnitude, list_time(e.origin, now));
                    if !l.usgs_type.eq_ignore_ascii_case("earthquake") {
                        text.push_str(&format!("[{}] ", l.usgs_type));
                    }
                    text.push_str(&l.place);
                    let label = egui::RichText::new(text).monospace().size(11.0);
                    if ui
                        .selectable_label(l.key.handle == selected, label)
                        .on_hover_text(format!(
                            "{} UTC  {}",
                            e.origin.format("%Y-%m-%d %H:%M:%S"),
                            l.place
                        ))
                        .clicked()
                    {
                        pick = Some(l.key.handle);
                    }
                }
            });
        if let Some(handle) = pick {
            self.events.select(handle);
        }
        ui.colored_label(
            pal.dim,
            format!(
                "Until you pick an event, the app picks one and switches to each new \
                 M{FOLLOW_MIN_MAG:.0}+ quake. Picking from this list never moves the clock: use \
                 Replay from origin, or Live. * = a past quake you picked from a search, \
                 + = an event you made."
            ),
        );
    }

    /// The make-your-own-event form: a new event, or changes to the selected one of yours.
    fn make_controls(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        let clock_now = self.clock.now();
        let f = &mut self.make_form;
        ui.horizontal(|ui| {
            ui.label("name");
            ui.add(
                egui::TextEdit::singleline(&mut f.name)
                    .desired_width(180.0)
                    .hint_text("e.g. Test blast"),
            );
        });
        egui::ComboBox::from_label("type")
            .selected_text(made::kind_label(f.kind))
            .show_ui(ui, |ui| {
                for kind in EventKind::ALL {
                    ui.selectable_value(&mut f.kind, kind, made::kind_label(kind));
                }
            });
        ui.horizontal(|ui| {
            ui.label("lat");
            ui.add(
                egui::DragValue::new(&mut f.lat)
                    .range(-90.0..=90.0)
                    .speed(0.1)
                    .max_decimals(3)
                    .suffix("°"),
            );
            ui.label("lon");
            ui.add(
                egui::DragValue::new(&mut f.lon)
                    .range(-180.0..=180.0)
                    .speed(0.1)
                    .max_decimals(3)
                    .suffix("°"),
            );
        });
        let pick_label = if f.picking {
            "Click the map… (cancel)"
        } else {
            "Pick on map"
        };
        if ui
            .button(pick_label)
            .on_hover_text("The next click on the map sets the place")
            .clicked()
        {
            f.picking = !f.picking;
        }
        // Each kind has its depth range; changing the kind brings the depth into it.
        let profile = f.kind.profile();
        f.depth_km = profile.clamp_depth(f.depth_km);
        if profile.max_depth_km > profile.min_depth_km {
            ui.add(
                egui::Slider::new(&mut f.depth_km, profile.min_depth_km..=profile.max_depth_km)
                    .text("depth km")
                    .max_decimals(1),
            );
        } else {
            ui.colored_label(
                pal.dim,
                format!(
                    "depth {:.0} km: a {} is at the surface",
                    f.depth_km,
                    made::kind_label(f.kind)
                ),
            );
        }
        ui.add(
            egui::Slider::new(&mut f.magnitude, 1.0..=10.0)
                .step_by(0.1)
                .text("magnitude"),
        );
        ui.horizontal(|ui| {
            ui.label("time");
            ui.add(
                egui::TextEdit::singleline(&mut f.time)
                    .desired_width(140.0)
                    .hint_text("the clock's time"),
            );
            if ui
                .small_button("Clock")
                .on_hover_text("Fill in the clock's current time")
                .clicked()
            {
                f.time = clock_now.format(TIME_FORMAT).to_string();
            }
        });

        // Update only while one of the user's events is selected and still listed.
        let selected = self.events.selected_handle();
        let selected_listed = self
            .events
            .shown()
            .iter()
            .find(|l| l.key.handle == selected);
        let editing = f
            .editing
            .filter(|&h| h == selected && selected_listed.is_some_and(|l| l.made && !l.gone));
        let (mut make, mut update, mut copy) = (false, false, false);
        ui.horizontal(|ui| {
            make = ui
                .button("Make new")
                .on_hover_text("Add this event to the list and select it")
                .clicked();
            update = ui
                .add_enabled(editing.is_some(), egui::Button::new("Update"))
                .on_hover_text("Change the selected event of yours to these values")
                .on_disabled_hover_text("Select one of your events (+) to change it")
                .clicked();
            copy = ui
                .button("Copy selected")
                .on_hover_text("Fill the form from the selected event")
                .clicked();
        });
        if copy && let Some(l) = selected_listed {
            f.fill(&l.event, &l.place);
            f.message = None;
        }
        if make || update {
            match f.build(clock_now) {
                Ok(m) => {
                    let place = m.place.clone();
                    // The form now shows the event as kept: an empty time becomes its origin, so
                    // a later Update does not move it to the clock's time again.
                    f.fill(&m.event, &m.place);
                    if let Some(handle) = editing.filter(|_| update) {
                        self.events.update_made(handle, m);
                        f.message = Some(format!("Updated: {place}"));
                    } else {
                        let handle = self.events.make(m);
                        f.message = Some(format!("Made: {place}"));
                        // Filled from this event above: no need to fill again on follow.
                        f.editing = Some(handle);
                        self.form_follow = handle;
                    }
                    self.made_dirty = true;
                }
                Err(e) => f.message = Some(e),
            }
        }
        if let Some(m) = &f.message {
            ui.colored_label(pal.dim, m);
        }
        ui.colored_label(
            pal.dim,
            "Times are UTC, YYYY-MM-DD HH:MM:SS; empty = the clock's time, so the waves start \
             where the clock is. Making an event never moves the clock: use Replay from origin. \
             Each type has its own depth range and wave mix. Your events are kept across \
             restarts; selecting one fills this form.",
        );
    }

    /// The USGS past-event search: by dates and size, or by event code.
    fn search_controls(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        let busy = self.events.search.busy;
        let f = &mut self.search_form;
        let mut search = None;
        ui.horizontal(|ui| {
            ui.label("from");
            ui.add(egui::TextEdit::singleline(&mut f.from).desired_width(76.0));
            ui.label("to");
            ui.add(egui::TextEdit::singleline(&mut f.to).desired_width(76.0));
        });
        ui.add(
            egui::Slider::new(&mut f.min_mag, 4.0..=9.5)
                .step_by(0.1)
                .text("min M"),
        );
        ui.add(egui::Slider::new(&mut f.limit, 1..=feed::MAX_RESULTS).text("max results"));
        if ui
            .add_enabled(!busy, egui::Button::new("Search USGS"))
            .on_hover_text("Dates are UTC days, YYYY-MM-DD, both included. Largest quakes first.")
            .clicked()
        {
            search = Some(f.range());
        }
        ui.horizontal(|ui| {
            ui.label("code");
            ui.add(
                egui::TextEdit::singleline(&mut f.code)
                    .desired_width(110.0)
                    .hint_text("e.g. usp000dbed"),
            );
            if ui
                .add_enabled(!busy, egui::Button::new("Look up"))
                .on_hover_text("One quake by its USGS event code")
                .clicked()
            {
                search = Some(Ok(feed::Search::Id(f.code.trim().to_owned())));
            }
        });
        if let Some(search) = search {
            match search.and_then(|s| s.url().map(|_| s).map_err(|e| e.to_string())) {
                Ok(s) => {
                    f.error = None;
                    self.events.search(s);
                }
                Err(e) => f.error = Some(e),
            }
        }
        let status = if self.events.search.busy {
            Some("Searching USGS…")
        } else {
            f.error.as_deref().or(self.events.search.message.as_deref())
        };
        if let Some(status) = status {
            ui.colored_label(pal.dim, status);
        }

        let selected = self.events.selected_handle();
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("search results")
            .max_height(180.0)
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                for r in &self.events.search.results {
                    let e = &r.feed.event;
                    let text = format!(
                        "M{:<4.1}{}  {}",
                        e.magnitude,
                        e.origin.format("%Y-%m-%d"),
                        r.feed.place
                    );
                    let label = egui::RichText::new(text).monospace().size(11.0);
                    if ui
                        .selectable_label(r.handle == selected, label)
                        .on_hover_text(format!(
                            "{} UTC  {}\n{} {}  {:.0} km deep\nClick to keep it in the list and \
                             replay it from its origin.",
                            e.origin.format("%Y-%m-%d %H:%M:%S"),
                            r.feed.place,
                            e.id,
                            e.mag_type,
                            e.depth_km
                        ))
                        .clicked()
                    {
                        pick = Some(r.handle);
                    }
                }
            });
        if let Some(handle) = pick {
            self.events.pick_result(handle);
        }
    }

    /// One line on where the events come from and how fresh they are.
    fn feed_line(&self) -> String {
        self.events.feed_line()
    }

    fn mantle_controls(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        let strength = &mut self.style.mantle_strength;
        ui.add(
            egui::Slider::new(strength, 0.0..=MAX_MANTLE_STRENGTH)
                .text("strength ×")
                .max_decimals(1),
        );
        ui.horizontal(|ui| {
            if ui.button("1D").clicked() {
                *strength = 0.0;
            }
            if ui.button("Model").clicked() {
                *strength = 1.0;
            }
        });
        let s = *strength;
        if s == 0.0 {
            ui.label("1D: ak135 only");
        } else if s == 1.0 {
            ui.label("Published model strength");
        } else if s > 1.0 {
            ui.colored_label(
                pal.text,
                egui::RichText::new(format!("EXAGGERATED ×{s:.1}")).strong(),
            );
        } else {
            ui.label(format!("Weakened ×{s:.1}"));
        }
        let status = self.mantle.status(self.selected_key());
        if !status.is_empty() {
            ui.colored_label(pal.dim, status);
        }
        if s > 0.0 && !self.bent_current() {
            ui.colored_label(pal.dim, "Bending slice rays… (1D rays shown meanwhile)");
        }
        ui.colored_label(
            pal.dim,
            "Fast and slow rock in the mantle (SP12RTS, Koelemeijer et al. 2016) moves the map \
             fronts earlier or later (times nudged along the 1D rays). In the slice the rays \
             bend through the rock.",
        );
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        ui.horizontal(|ui| {
            ui.heading("SEISMO");
            ui.colored_label(
                pal.dim,
                format!("{:.0} fps", 1.0 / self.frame_time_s.max(1e-6)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .selectable_label(self.guide.open, "? Guide")
                    .on_hover_text(
                        "How to use SEISMO, what each picture shows, and how earthquakes and \
                         seismic waves work (F1)",
                    )
                    .clicked()
                {
                    self.guide.open = !self.guide.open;
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label("look:");
            let mut look = self.look;
            for l in render::Look::ALL {
                ui.selectable_value(&mut look, l, l.name());
            }
            self.set_look(ui.ctx(), look);
        });
        ui.separator();

        let now = self.clock.now();
        // chrono has no one-decimal fraction specifier; append tenths by hand.
        let tenths = now.timestamp_subsec_millis() / 100;
        ui.horizontal(|ui| {
            ui.monospace(format!("{}.{tenths} UTC", now.format("%Y-%m-%d %H:%M:%S")));
            self.guide.glyph(ui, "clock");
        });
        self.guide.note(ui, "clock");

        ui.horizontal(|ui| {
            let label = if self.clock.playing() {
                "Pause"
            } else {
                "Play"
            };
            if ui.button(label).clicked() {
                self.clock.set_playing(!self.clock.playing());
            }
            if ui.button("Replay from origin").clicked() {
                self.clock.seek(self.origin());
                self.clock.set_playing(true);
            }
            if ui
                .button("Live")
                .on_hover_text("Jump to now, at real speed")
                .clicked()
            {
                self.clock.seek(Utc::now());
                self.clock.set_speed(1.0);
                self.clock.set_playing(true);
            }
        });

        let mut speed = self.clock.speed();
        ui.add(
            egui::Slider::new(&mut speed, MIN_SPEED..=MAX_SPEED)
                .logarithmic(true)
                .text("speed ×")
                .max_decimals(1),
        );
        self.clock.set_speed(speed);

        let sel = self.sel();
        let event = &self.sim.events[sel];
        let mut t = event.t_at(now);
        let scrub = ui.add(
            egui::Slider::new(&mut t, -SCRUB_BEFORE_S..=SCRUB_AFTER_S)
                .text("s since origin")
                .clamping(egui::SliderClamping::Never)
                .max_decimals(1),
        );
        if scrub.changed() {
            self.clock.seek(event.origin + secs(t));
        }

        ui.separator();
        let handle = self.events.selected_handle();
        let Some(listed) = self.events.shown().iter().find(|l| l.key.handle == handle) else {
            return;
        };
        let kind = if listed.usgs_type.eq_ignore_ascii_case("earthquake") {
            String::new()
        } else {
            format!("  {}", listed.usgs_type.to_uppercase())
        };
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("M{:.1} {}{kind}", event.magnitude, event.mag_type))
                    .strong(),
            );
            self.guide.glyph(ui, "event-panel");
        });
        self.guide.note(ui, "event-panel");
        ui.label(&listed.place);
        ui.monospace(format!("{} UTC", event.origin.format("%Y-%m-%d %H:%M:%S")));
        ui.monospace(format!(
            "{:.3}°N {:.3}°E  {:.0} km",
            event.lat, event.lon, event.depth_km
        ));
        ui.colored_label(pal.dim, format!("{}  {}", event.id, listed.status));
        if listed.gone {
            ui.colored_label(
                pal.text,
                "No longer in the event list; shown while selected.",
            );
        }
        let handle = listed.key.handle;
        let (mine, saved) = (listed.made && !listed.gone, listed.saved);
        if mine {
            ui.horizontal(|ui| {
                ui.colored_label(pal.dim, "Edit it under Make your own event.");
                if ui
                    .small_button("Delete")
                    .on_hover_text("Take it off the list for good (it stays while selected)")
                    .clicked()
                {
                    self.events.delete_made(handle);
                    self.made_dirty = true;
                }
            });
        }
        if saved {
            ui.horizontal(|ui| {
                ui.colored_label(pal.dim, "Kept from a search.");
                if ui
                    .small_button("Forget")
                    .on_hover_text("Take it off the list (it stays while selected)")
                    .clicked()
                {
                    self.events.forget(handle);
                }
            });
        }
        let t = event.t_at(now);
        let fmt = |d: Option<f64>| d.map_or("—".to_owned(), |d| format!("{d:6.2}°"));
        let p = self.sim.front_distance("P", event.depth_km, t);
        let s = self.sim.front_distance("S", event.depth_km, t);
        ui.monospace(format!("t   {t:9.1} s"));
        ui.monospace(format!("P   {}", fmt(p)));
        ui.monospace(format!("S   {}", fmt(s)));
        ui.colored_label(
            pal.dim,
            match self.look {
                render::Look::Crt => {
                    "Map: P solid, S dashed (direct waves only); surface waves: Rayleigh glow, \
                     Love dotted."
                }
                render::Look::Modern => {
                    "Map: P amber (solid), S cyan (dashed), direct waves only; surface waves: \
                     Rayleigh violet glow, Love pink dots."
                }
            },
        );

        ui.separator();
        let make_open = self.make_open;
        self.section(ui, "Events", "events", !make_open, Self::event_controls);

        ui.separator();
        self.make_open = self.section(
            ui,
            "Make your own event",
            "make",
            make_open,
            Self::make_controls,
        );

        ui.separator();
        self.section(
            ui,
            "Search past quakes",
            "search",
            true,
            Self::search_controls,
        );

        ui.separator();
        self.section(
            ui,
            "3D mantle",
            "mantle-control",
            true,
            Self::mantle_controls,
        );

        ui.separator();
        match self.look {
            render::Look::Crt => self.section(ui, "CRT", "looks", true, Self::crt_controls),
            render::Look::Modern => self.section(ui, "Glow", "looks", true, Self::glow_controls),
        };

        self.section(
            ui,
            "Seismographs",
            "seismograms",
            true,
            Self::trace_controls,
        );
        self.section(ui, "Slice", "slice", true, Self::slice_controls);
        self.section(ui, "Map", "map", false, |app, ui| {
            let s = &mut app.style;
            ui.add(egui::Slider::new(&mut s.front_width_px, 1.0..=6.0).text("front px"));
            ui.add(
                egui::Slider::new(&mut s.trail_s, 1.0..=300.0)
                    .logarithmic(true)
                    .text("trail s"),
            );
            ui.add(egui::Slider::new(&mut s.trail_strength, 0.0..=1.0).text("trail"));
            ui.add(egui::Slider::new(&mut s.grid_brightness, 0.0..=1.0).text("grid"));
            ui.add(egui::Slider::new(&mut s.coast_brightness, 0.0..=1.0).text("coast"));
            ui.add(egui::Slider::new(&mut s.surface_brightness, 0.0..=1.0).text("surface"))
                .on_hover_text("Surface-wave bands; 0 hides them");
            ui.checkbox(&mut s.coast_wide, "Wide coastlines")
                .on_hover_text("Wider lines stay solid; thin ones are hatched by the scanlines");
        });
    }

    /// The modern look's only post-process: a soft glow around the bright fronts.
    fn glow_controls(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.post[render::Look::Modern.index()];
        ui.checkbox(&mut c.bloom, "glow around bright lines");
        ui.add_enabled(
            c.bloom,
            egui::Slider::new(&mut c.bloom_intensity, 0.0..=2.0).text("strength"),
        );
        ui.add_enabled(
            c.bloom,
            egui::Slider::new(&mut c.bloom_radius_px, 2.0..=24.0).text("radius px"),
        );
        ui.add_enabled(
            c.bloom,
            egui::Slider::new(&mut c.bloom_threshold, 0.0..=1.0).text("from brightness"),
        )
        .on_hover_text("Only parts brighter than this glow");
    }

    fn crt_controls(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.post[render::Look::Crt.index()];
        ui.checkbox(&mut c.enabled, "enabled (master)");
        ui.add_enabled_ui(c.enabled, |ui| {
            ui.checkbox(&mut c.scanlines, "scanlines");
            ui.add_enabled(
                c.scanlines,
                egui::Slider::new(&mut c.scanline_intensity, 0.0..=1.0).text("depth"),
            );
            ui.add_enabled(
                c.scanlines,
                egui::Slider::new(&mut c.scanline_period_px, 2.0..=8.0).text("period px"),
            );
            ui.checkbox(&mut c.persistence, "persistence");
            ui.add_enabled(
                c.persistence,
                egui::Slider::new(&mut c.persistence_tau_s, 0.02..=3.0)
                    .logarithmic(true)
                    .text("decay s"),
            );
            ui.checkbox(&mut c.bloom, "bloom (glow)");
            ui.add_enabled(
                c.bloom,
                egui::Slider::new(&mut c.bloom_intensity, 0.0..=2.0).text("strength"),
            );
            ui.add_enabled(
                c.bloom,
                egui::Slider::new(&mut c.bloom_radius_px, 2.0..=24.0).text("radius px"),
            );
            ui.checkbox(&mut c.barrel, "barrel (curved glass)");
            ui.add_enabled(
                c.barrel,
                egui::Slider::new(&mut c.barrel_amount, 0.0..=0.2).text("bulge"),
            );
            ui.checkbox(&mut c.flicker, "flicker");
            ui.add_enabled(
                c.flicker,
                egui::Slider::new(&mut c.flicker_amount, 0.0..=0.2).text("amount"),
            );
            ui.checkbox(&mut c.hot_spots, "hot spots go pale")
                .on_hover_text(
                    "The overbright core of a line whitens, as a saturated phosphor does",
                );
            ui.add_enabled(
                c.hot_spots,
                egui::Slider::new(&mut c.hot_spot_amount, 0.0..=2.0).text("amount"),
            );
            ui.checkbox(&mut c.vignette, "vignette (dim edges, rounded corners)");
            ui.add_enabled(
                c.vignette,
                egui::Slider::new(&mut c.vignette_amount, 0.0..=1.0).text("darkening"),
            );
            ui.add_enabled(
                c.vignette,
                egui::Slider::new(&mut c.corner_radius, 0.0..=0.15).text("corner radius"),
            );
            ui.checkbox(&mut c.noise, "signal noise");
            ui.add_enabled(
                c.noise,
                egui::Slider::new(&mut c.noise_amount, 0.0..=0.3).text("amount"),
            );
            ui.checkbox(&mut c.hum_bar, "hum bar (rolling band)");
            ui.add_enabled(
                c.hum_bar,
                egui::Slider::new(&mut c.hum_bar_amount, 0.0..=0.2).text("amount"),
            );
            ui.add_enabled(
                c.hum_bar,
                egui::Slider::new(&mut c.hum_bar_period_s, 2.0..=30.0).text("period s"),
            );
            ui.checkbox(&mut c.warm_up, "warm-up when switched on");
            ui.add_enabled(
                c.warm_up,
                egui::Slider::new(&mut c.warm_up_s, 0.2..=3.0).text("seconds"),
            );
        });
    }

    fn scene(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame, real_dt: f64) {
        let Some(rs) = frame.wgpu_render_state() else {
            return;
        };
        let pal = self.pal();

        // A square block (the 2:1 map above the 2:1 slice), with the traces column filling the
        // rest of the width; without traces, the largest square, centred.
        let avail = ui.available_rect_before_wrap();
        let (rect, traces_pts) = if self.traces.show {
            let side = avail
                .height()
                .min(avail.width() - MIN_TRACES_WIDTH)
                .max(1.0);
            let traces_w = (avail.width() - side).max(0.0);
            let rect = egui::Rect::from_min_size(
                egui::pos2(avail.left(), avail.center().y - side / 2.0),
                egui::vec2(side + traces_w, side),
            );
            (rect, traces_w)
        } else {
            let w = avail.width().min(avail.height()).max(1.0);
            (
                egui::Rect::from_center_size(avail.center(), egui::vec2(w, w)),
                0.0,
            )
        };
        let ppp = ui.ctx().pixels_per_point();
        let px = (
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        );
        let traces_px = (traces_pts * ppp).round() as u32;

        if self.renderer.resize(&rs.device, px, traces_px) {
            tracing::debug!(w = px.0, h = px.1, "scene resized");
            rs.renderer.write().update_egui_texture_from_wgpu_texture(
                &rs.device,
                self.renderer.output_view(),
                wgpu::FilterMode::Nearest,
                self.texture_id,
            );
        }

        let now = self.clock.now();
        let linger = 5.0 * self.style.trail_s as f64;
        let map_events = self.sim.map_events(now, self.map_cap, linger);
        // Ask for the delays of every event whose body waves are on the map (the selected one,
        // then largest first), and upload any that are ready into a GPU slot.
        let sel = self.sel();
        // Surface waves are 1D: events with only those left need no delays.
        let mut order: Vec<usize> = map_events
            .iter()
            .filter(|m| m.body)
            .map(|m| m.index)
            .collect();
        if let Some(i) = order.iter().position(|&i| i == sel) {
            order[..=i].rotate_right(1);
        }
        for &i in &order {
            let e = &self.sim.events[i];
            self.mantle
                .request_delays(self.keys[i], e.lat, e.lon, e.depth_km);
        }
        let on_map: Vec<EventKey> = order.iter().map(|&i| self.keys[i]).collect();
        if let Some(key) = self.bend_key() {
            let e = &self.sim.events[sel];
            self.mantle.request_bend(key, e.lat, e.lon, e.depth_km);
        }
        if let Some(wave) = self.slice.rock {
            let e = &self.sim.events[sel];
            let key = CutKey::new(self.keys[sel], self.slice.azimuth_deg, wave);
            self.mantle.request_cut(key, e.lat, e.lon);
        }
        for ready in self.mantle.collect(&on_map) {
            match ready {
                Ready::Delays { event, slot, grid } => {
                    self.renderer.map.upload_delays(&rs.queue, slot, &grid);
                    self.delay_grids.insert(event, Arc::new(grid));
                }
                Ready::Cut(grid) => self.renderer.slice.upload_cut(&rs.queue, &grid),
                Ready::Bend(key, fans) => self.bent = Some((key, fans)),
            }
        }
        let events: Vec<render::MapEventInstance> = map_events
            .into_iter()
            .map(|m| {
                let e = &self.sim.events[m.index];
                let slot = self.mantle.slot(self.keys[m.index]);
                render::MapEventInstance {
                    lat: e.lat as f32,
                    lon: e.lon as f32,
                    depth_km: e.depth_km as f32,
                    t: m.t as f32,
                    magnitude: e.magnitude as f32,
                    delay_slot: slot.map(|(s, _)| s),
                    max_delay_s: slot.map_or(0.0, |(_, m)| m),
                    surface: m.surface,
                }
            })
            .collect();

        // Slice: fronts now, fainter fronts at earlier times as a trail, and ray paths so far.
        let event = &self.sim.events[sel];
        let (t, depth_km) = (event.t_at(now), event.depth_km);
        let slice_line = Some(render::MapSliceLine {
            lat: event.lat as f32,
            lon: event.lon as f32,
            azimuth_deg: self.slice.azimuth_deg as f32,
        });
        let enabled = self.slice.phases;
        let keep = |lines: Vec<SliceLine>| -> Vec<SliceLine> {
            lines
                .into_iter()
                .filter(|l| enabled[phase_index(l.phase)])
                .collect()
        };
        let (trail_s, trail_strength) = (self.style.trail_s as f64, self.style.trail_strength);
        let (trail_fronts, ray_paths, per_phase) = (
            self.slice.trail_fronts,
            self.slice.ray_paths,
            self.slice.paths_per_phase,
        );
        let rock_shading = self.slice.rock.is_some()
            && self.style.mantle_strength > 0.0
            && self.mantle.cut_current();
        let fans = self.fans();
        let fronts = fans.fronts_at(t);
        let labels: Vec<PhaseLabel> = fronts
            .labels
            .iter()
            .filter(|l| enabled[phase_index(l.phase)])
            .copied()
            .collect();
        let front_lines = keep(fronts.lines);
        let spacing = trail_s / 2.0;
        let ghosts: Vec<(f32, Vec<SliceLine>)> = (1..=trail_fronts)
            .map(|k| {
                let dt = spacing * k as f64;
                let gain = trail_strength * (-dt / trail_s).exp() as f32;
                (gain, keep(fans.fronts_at(t - dt).lines))
            })
            .collect();
        let paths = if ray_paths {
            keep(fans.ray_paths_until(t, per_phase))
        } else {
            Vec::new()
        };
        let mut layers = vec![render::SliceLayer {
            lines: &paths,
            kind: render::StrokeKind::Path,
            intensity: 0.3,
        }];
        layers.extend(ghosts.iter().map(|(gain, lines)| render::SliceLayer {
            lines,
            kind: render::StrokeKind::Front,
            intensity: *gain,
        }));
        layers.push(render::SliceLayer {
            lines: &front_lines,
            kind: render::StrokeKind::Front,
            intensity: 1.0,
        });
        let slice_scene = render::SliceScene {
            source_depth_km: depth_km,
            boundaries: self.sim.boundaries(),
            layers: &layers,
            rock_shading,
        };

        // Traces: rebuilt when their inputs change, squeezed to the column's pixels each frame
        // and drawn up to the pen (now).
        if !self.traces.user_picked && self.traces.chosen.is_empty() {
            self.traces.pick_defaults(&self.sim.events[sel]);
        }
        self.traces.update(
            &self.sim,
            &self.keys,
            sel,
            &self.delay_grids,
            f64::from(self.style.mantle_strength),
        );
        let trace_vp = self.renderer.traces_viewport();
        let shown = self.traces.shown();
        let layout = render::TraceLayout::new(
            trace_vp.map_or((1.0, 1.0), |v| (v.w, v.h)),
            shown.len(),
            ppp,
        );
        let (from, to) = self.traces.window();
        let frac = |s: f64| ((s - from) / (to - from)) as f32;
        let shared = shown.iter().fold(0.0f32, |m, t| m.max(t.seismogram.peak));
        let component = self.traces.component;
        let columns: Vec<Vec<[f32; 2]>> = shown
            .iter()
            .map(|tr| {
                let scale = if self.traces.shared_scale {
                    shared
                } else {
                    tr.seismogram.peak
                };
                tr.seismogram.columns(from, to, layout.columns(), t, scale)
            })
            .collect();
        // Arrivals this component shows, inside the window: ticks, and labels for the overlay.
        let picks: Vec<Vec<(f32, &'static str)>> = shown
            .iter()
            .map(|tr| {
                tr.arrivals
                    .iter()
                    .filter(|a| a.motion.weight(component) > 0.0)
                    .map(|a| (frac(a.time_s), a.phase))
                    .filter(|(f, _)| (0.0..=1.0).contains(f))
                    .collect()
            })
            .collect();
        let ticks: Vec<Vec<f32>> = picks
            .iter()
            .map(|p| p.iter().map(|(f, _)| *f).collect())
            .collect();
        let rows: Vec<render::TraceRow> = columns
            .iter()
            .zip(&ticks)
            .map(|(c, k)| render::TraceRow {
                columns: c,
                ticks: k,
            })
            .collect();
        // Grid every 15, 10 or 2 minutes, so the axis labels never crowd.
        let step_s = match to - from {
            w if w > 65.0 * 60.0 => 900.0,
            w if w > 25.0 * 60.0 => 600.0,
            _ => 120.0,
        };
        let grid_times: Vec<f64> = (0..)
            .map(|k| k as f64 * step_s)
            .take_while(|&s| s <= to)
            .collect();
        let grid: Vec<f32> = grid_times.iter().map(|&s| frac(s)).collect();
        let trace_scene = render::TraceScene {
            rows: &rows,
            gain: self.traces.gain,
            pen: Some(frac(t)),
            grid: &grid,
            px_per_pt: ppp,
        };
        let trace_labels: Vec<TraceLabel> = shown
            .iter()
            .zip(picks)
            .map(|(tr, picks)| {
                let s = &self.traces.stations[tr.station];
                TraceLabel {
                    text: format!("{}  {:.1}°  {}", s.label(), tr.dist_deg, s.name),
                    picks,
                }
            })
            .collect();
        let markers: Vec<render::MapMarker> = self
            .traces
            .stations
            .iter()
            .enumerate()
            .map(|(i, s)| render::MapMarker {
                lat: s.lat as f32,
                lon: s.lon as f32,
                selected: self.traces.chosen.contains(&i),
                user: s.is_user(),
            })
            .collect();

        let seek = self.clock.seek_count();
        let time = render::FrameTime {
            dt_s: real_dt as f32,
            time_s: self.started.elapsed().as_secs_f32(),
            since_on_s: self
                .look_since
                .get_or_insert_with(Instant::now)
                .elapsed()
                .as_secs_f32(),
            clear_persistence: seek != self.last_seek_count
                || std::mem::take(&mut self.look_changed),
        };
        self.last_seek_count = seek;
        let theme = render::Theme::for_look(self.look);
        let post = *self.post();
        // While the CRT's picture is still opening, the labels would float over a band.
        let warming_up = post.warm_up_progress(time.since_on_s) < 1.0;

        let mut encoder = rs
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene"),
            });
        self.renderer.render(
            &rs.device,
            &rs.queue,
            &mut encoder,
            &theme,
            &events,
            &self.style,
            slice_line,
            &markers,
            &slice_scene,
            &self.slice.style[self.look.index()],
            &trace_scene,
            &self.trace_style,
            &post,
            time,
        );
        rs.queue.submit([encoder.finish()]);

        let elapsed = self.started.elapsed().as_secs_f64();
        if let Some(b) = &mut self.bench {
            let delayed = events.iter().filter(|e| e.delay_slot.is_some()).count();
            b.map(elapsed, events.len(), delayed);
        }
        if self.bench.as_ref().is_some_and(|b| b.due(elapsed)) {
            // The scene alone: the same frame drawn again and waited for each time, with no
            // egui and no presenting, so the GPU's share is told apart from the rest.
            let mut ms = Vec::new();
            for _ in 0..60 {
                let t0 = Instant::now();
                let mut encoder =
                    rs.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("bench scene"),
                        });
                self.renderer.render(
                    &rs.device,
                    &rs.queue,
                    &mut encoder,
                    &theme,
                    &events,
                    &self.style,
                    slice_line,
                    &markers,
                    &slice_scene,
                    &self.slice.style[self.look.index()],
                    &trace_scene,
                    &self.trace_style,
                    &post,
                    time,
                );
                rs.queue.submit([encoder.finish()]);
                if let Err(e) = rs.device.poll(wgpu::PollType::wait_indefinitely()) {
                    tracing::warn!("bench poll: {e}");
                }
                ms.push(t0.elapsed().as_secs_f64() * 1e3);
            }
            // Then each pass on its own.
            let mut passes: [[Vec<f64>; 2]; 4] = Default::default();
            for _ in 0..30 {
                let t = self.renderer.render_timed(
                    &rs.device,
                    &rs.queue,
                    &theme,
                    &events,
                    &self.style,
                    slice_line,
                    &markers,
                    &slice_scene,
                    &self.slice.style[self.look.index()],
                    &trace_scene,
                    &self.trace_style,
                    &post,
                    time,
                );
                for (v, t) in passes.iter_mut().zip(t) {
                    v[0].push(t[0]);
                    v[1].push(t[1]);
                }
            }
            let info = rs.adapter.get_info();
            let win = ui.ctx().content_rect().size() * ui.ctx().pixels_per_point();
            let (sw, sh) = self.renderer.size();
            let map = self.renderer.map_viewport();
            let args: Vec<String> = std::env::args().skip(1).collect();
            let context = format!(
                "{} ({:?}) | window {:.0}x{:.0} px | scene {sw}x{sh} px | map {:.0}x{:.0} px | look {:?} | args {}",
                info.name,
                info.backend,
                win.x,
                win.y,
                map.w,
                map.h,
                self.look,
                args.join(" "),
            );
            if let Some(b) = self.bench.take() {
                b.report(
                    &context,
                    bench::Stats::of(&ms),
                    passes.map(|p| p.map(|v| bench::Stats::of(&v))),
                );
            }
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if let Some((path, after)) = &self.capture
            && self.started.elapsed().as_secs_f64() >= *after
        {
            let (w, h) = self.renderer.size();
            let rgba = self.renderer.read_output(&rs.device, &rs.queue);
            match write_ppm(path, w, h, &rgba) {
                Ok(()) => tracing::info!(path = %path.display(), w, h, "captured scene"),
                Err(e) => tracing::error!(path = %path.display(), "capture failed: {e}"),
            }
            self.capture = None;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }

        let response = ui.put(
            rect,
            egui::Image::new(egui::load::SizedTexture::new(self.texture_id, rect.size()))
                .sense(egui::Sense::click()),
        );
        if warming_up {
            return;
        }
        self.slice_labels(ui, rect, &labels);
        let trace_header = trace_vp.map(|vp| {
            self.trace_labels(ui, rect, vp, &layout, &trace_labels, &grid_times, from, to)
        });
        self.station_picking(ui, rect, &response);
        self.picture_glyphs(ui, rect, trace_header);
        if self.style.mantle_strength > 1.0 {
            let map_h = self.renderer.map_viewport().h;
            ui.painter_at(rect).text(
                self.scene_to_ui(rect, [12.0, map_h - 12.0]),
                egui::Align2::LEFT_BOTTOM,
                format!("3D MANTLE EXAGGERATED ×{:.1}", self.style.mantle_strength),
                self.font(13.0),
                pal.text,
            );
        }
    }

    /// A `?` on each picture that opens its topic in the guide: in the top-right corner of the
    /// map and the slice, and after the seismograms' header (`trace_header`, where it was
    /// written), whose top-right corner is the CRT's rounded glass and crowded with labels.
    fn picture_glyphs(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        trace_header: Option<egui::Rect>,
    ) {
        let inset = 16.0 * ui.ctx().pixels_per_point();
        for (vp, topic) in [
            (self.renderer.map_viewport(), "map"),
            (self.renderer.slice_viewport(), "slice"),
        ] {
            let at = self.scene_to_ui(rect, [vp.x + vp.w - inset, vp.y + inset]);
            self.guide.corner(ui, at, topic);
        }
        // After the header, but never past the column's edge (a narrow column cuts the header).
        if let (Some(header), Some(vp)) = (trace_header, self.renderer.traces_viewport()) {
            let edge = self
                .scene_to_ui(rect, [vp.x + vp.w - inset, vp.y + inset])
                .x;
            let x = (header.right() + 16.0).min(edge);
            self.guide
                .corner(ui, egui::pos2(x, header.center().y), "seismograms");
        }
    }

    /// Where scene pixel `p` appears in UI points, through the CRT's barrel distortion.
    fn scene_to_ui(&self, rect: egui::Rect, p: [f32; 2]) -> egui::Pos2 {
        let size = self.renderer.size();
        let scale = rect.width() / size.0.max(1) as f32;
        let [x, y] = self.post().scene_to_display(size, p);
        rect.min + egui::vec2(x, y) * scale
    }

    /// The scene pixel under `pos` (UI points), undoing the barrel distortion; `None` off the
    /// curved glass.
    fn ui_to_scene(&self, rect: egui::Rect, pos: egui::Pos2) -> Option<[f32; 2]> {
        let size = self.renderer.size();
        let scale = rect.width() / size.0.max(1) as f32;
        let d = (pos - rect.min) / scale;
        self.post().display_to_scene(size, [d.x, d.y])
    }

    /// The map's projection and the UI point of a station's marker.
    fn map_frame(&self) -> (render::MapProjection, render::Viewport) {
        let vp = self.renderer.map_viewport();
        (render::MapProjection { size: (vp.w, vp.h) }, vp)
    }

    /// Where the map point `lat`, `lon` appears in UI points.
    fn map_to_ui(&self, rect: egui::Rect, lat: f64, lon: f64) -> egui::Pos2 {
        let (proj, vp) = self.map_frame();
        let [x, y] = proj.pixel(lat, lon);
        self.scene_to_ui(rect, [x + vp.x, y + vp.y])
    }

    /// The station whose map marker is under `pos` (UI points), if any.
    fn station_at(&self, rect: egui::Rect, pos: egui::Pos2) -> Option<usize> {
        self.traces
            .stations
            .iter()
            .enumerate()
            .map(|(i, s)| (i, self.map_to_ui(rect, s.lat, s.lon).distance(pos)))
            .filter(|(_, d)| *d <= PICK_RADIUS)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// Map clicks show or hide a station's trace, or place a new station, or (when asked) set
    /// the place of the event being made; hovering a marker names it. Shown stations are
    /// labelled on the map, and the event form's place is marked while the form is open.
    fn station_picking(&mut self, ui: &egui::Ui, rect: egui::Rect, response: &egui::Response) {
        let pal = self.pal();
        let painter = ui.painter_at(rect);
        let font = self.font(11.0);
        for &i in &self.traces.chosen {
            let s = &self.traces.stations[i];
            painter.text(
                self.map_to_ui(rect, s.lat, s.lon) + egui::vec2(7.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &s.code,
                font.clone(),
                pal.text,
            );
        }
        if let Some(pos) = response.hover_pos()
            && let Some(i) = self.station_at(rect, pos)
        {
            let s = &self.traces.stations[i];
            painter.text(
                pos + egui::vec2(10.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{}  {}", s.label(), s.name),
                font,
                pal.text,
            );
        }
        // The make-your-own form's place, and picking it: the next map click sets it.
        if self.make_open || self.make_form.picking {
            let f = &self.make_form;
            let c = self.map_to_ui(rect, f.lat, f.lon);
            let stroke = egui::Stroke::new(1.5, pal.text);
            for d in [egui::vec2(6.0, 6.0), egui::vec2(6.0, -6.0)] {
                painter.line_segment([c - d, c + d], stroke);
            }
        }
        if self.make_form.picking {
            if let Some(pos) = response.hover_pos() {
                painter.text(
                    pos + egui::vec2(10.0, 10.0),
                    egui::Align2::LEFT_TOP,
                    "click: put your event here",
                    self.font(11.0),
                    pal.text,
                );
            }
            if response.clicked()
                && let Some(pos) = response.interact_pointer_pos()
                && let Some([x, y]) = self.ui_to_scene(rect, pos)
            {
                let (proj, vp) = self.map_frame();
                if let Some((lat, lon)) = proj.lat_lon([x - vp.x, y - vp.y]) {
                    let f = &mut self.make_form;
                    // Rounded to what the form shows.
                    f.lat = (lat * 1000.0).round() / 1000.0;
                    f.lon = (lon * 1000.0).round() / 1000.0;
                    f.picking = false;
                }
            }
            return;
        }
        if !response.clicked() || !self.traces.show {
            return;
        }
        let Some(pos) = response.interact_pointer_pos() else {
            return;
        };
        if let Some(i) = self.station_at(rect, pos) {
            self.traces.toggle(i);
        } else if self.traces.click_to_place
            && let Some([x, y]) = self.ui_to_scene(rect, pos)
        {
            let (proj, vp) = self.map_frame();
            if let Some((lat, lon)) = proj.lat_lon([x - vp.x, y - vp.y]) {
                self.traces.place(lat, lon);
            }
        }
    }

    /// Text over the traces column: title, station names, phase names at their ticks, and the
    /// time axis. Returns where the title was written.
    #[allow(clippy::too_many_arguments)]
    fn trace_labels(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        vp: render::Viewport,
        layout: &render::TraceLayout,
        rows: &[TraceLabel],
        grid_times: &[f64],
        from: f64,
        to: f64,
    ) -> egui::Rect {
        let pal = self.pal();
        let scale = rect.width() / self.renderer.size().0.max(1) as f32;
        let to_ui = |x: f32, y: f32| self.scene_to_ui(rect, [x + vp.x, y + vp.y]);
        let painter = ui.painter_at(rect);
        let font = self.font(11.0);
        let small = self.font(9.5);
        let component = match self.traces.component {
            Component::Vertical => "UP-DOWN (Z)",
            Component::Transverse => "SIDE-TO-SIDE (T)",
        };
        let scaling = if self.traces.shared_scale {
            "shared scale"
        } else {
            "each row scaled"
        };
        let header = to_ui(layout.plot_x().0, 0.0);
        let title = painter.text(
            header + egui::vec2(0.0, 3.0),
            egui::Align2::LEFT_TOP,
            format!("SEISMOGRAMS  {component}  {scaling}"),
            font.clone(),
            pal.text,
        );
        painter.text(
            header + egui::vec2(0.0, 17.0),
            egui::Align2::LEFT_TOP,
            "synthetic: display-plausible, not waveform-accurate",
            small.clone(),
            pal.dim,
        );
        if rows.is_empty() {
            painter.text(
                to_ui(layout.plot_x().0, layout.label_y(0) + 8.0),
                egui::Align2::LEFT_TOP,
                if self.traces.pending {
                    "Tracing rays for this event…"
                } else {
                    "No stations chosen: click one on the map."
                },
                font.clone(),
                pal.dim,
            );
        }
        for (i, row) in rows.iter().enumerate() {
            painter.text(
                to_ui(layout.plot_x().0, layout.label_y(i)),
                egui::Align2::LEFT_TOP,
                &row.text,
                font.clone(),
                pal.dim,
            );
            // Phase names at their ticks, skipping any that would overlap the previous one.
            let (base, half) = layout.band(i);
            let mut last_x = f32::NEG_INFINITY;
            for &(f, name) in &row.picks {
                let x = layout.x_at(f);
                if (x - last_x) * scale < 26.0 {
                    continue;
                }
                last_x = x;
                painter.text(
                    to_ui(x + 2.0, base - half),
                    egui::Align2::LEFT_TOP,
                    name,
                    small.clone(),
                    pal.dim,
                );
            }
        }
        let frac = |s: f64| ((s - from) / (to - from)) as f32;
        for &s in grid_times {
            let label = if s == 0.0 {
                "origin".to_owned()
            } else {
                format!("+{:.0} min", s / 60.0)
            };
            // Labels at the ends of the axis lean inwards so they stay on screen.
            let f = frac(s);
            let align = if f < 0.05 {
                egui::Align2::LEFT_TOP
            } else if f > 0.95 {
                egui::Align2::RIGHT_TOP
            } else {
                egui::Align2::CENTER_TOP
            };
            painter.text(
                to_ui(layout.x_at(f), layout.axis_y()) + egui::vec2(0.0, 6.0),
                align,
                label,
                small.clone(),
                pal.dim,
            );
        }
        title
    }

    /// Text over the slice, drawn by egui on top of the scene (undistorted): distances along
    /// the surface, layer names, and phase names at their fronts.
    fn slice_labels(&self, ui: &egui::Ui, rect: egui::Rect, labels: &[PhaseLabel]) {
        let pal = self.pal();
        let vp = self.renderer.slice_viewport();
        let view = render::SliceView::new((vp.w, vp.h));
        // Scene pixels to UI points.
        let to_ui = |p: [f32; 2]| self.scene_to_ui(rect, [p[0] + vp.x, p[1] + vp.y]);
        let at = |r_km: f64, delta_deg: f64| to_ui(view.project(r_km, delta_deg));
        let painter = ui.painter_at(rect);
        let font = self.font(11.0);
        let text = |pos: egui::Pos2, align: egui::Align2, s: &str, colour: egui::Color32| {
            painter.text(pos, align, s, font.clone(), colour);
        };

        // Distance from the epicentre, outside the arc; the two ends of the flat edge.
        for d in (0..=180).step_by(30) {
            let p = at(EARTH_RADIUS_KM * 1.08, f64::from(d));
            text(p, egui::Align2::CENTER_CENTER, &format!("{d}°"), pal.dim);
        }
        let under = egui::vec2(0.0, 14.0);
        text(
            at(EARTH_RADIUS_KM, 0.0) + under,
            egui::Align2::LEFT_CENTER,
            "EPICENTRE",
            pal.dim,
        );
        text(
            at(EARTH_RADIUS_KM, 180.0) + under,
            egui::Align2::RIGHT_CENTER,
            "ANTIPODE",
            pal.dim,
        );

        // Region and boundary names.
        let bounds = self.sim.boundaries();
        let depth = |name: &str| bounds.iter().find(|b| b.name == name).map(|b| b.depth_km);
        if let (Some(cmb), Some(icb)) = (depth("CMB"), depth("ICB")) {
            let (r_cmb, r_icb) = (EARTH_RADIUS_KM - cmb, EARTH_RADIUS_KM - icb);
            let centre = egui::Align2::CENTER_CENTER;
            text(
                at(0.5 * (EARTH_RADIUS_KM + r_cmb), 90.0),
                centre,
                "MANTLE",
                pal.dim,
            );
            text(
                at(0.5 * (r_cmb + r_icb), 90.0),
                centre,
                "OUTER CORE (LIQUID)",
                pal.dim,
            );
            text(at(0.45 * r_icb, 90.0), centre, "INNER CORE", pal.dim);
        }
        for name in ["660", "CMB", "ICB"] {
            if let Some(d) = depth(name) {
                let p = at(EARTH_RADIUS_KM - d, 125.0);
                text(p, egui::Align2::LEFT_BOTTOM, name, pal.dim);
            }
        }

        if self.slice.labels {
            for l in labels {
                let p = at(l.at.r_km, l.at.delta_deg);
                text(
                    p + egui::vec2(4.0, -4.0),
                    egui::Align2::LEFT_BOTTOM,
                    l.phase.name(),
                    pal.text,
                );
            }
        }
    }
}

/// Overlay text for one trace row: the station line and its phases at their ticks.
struct TraceLabel {
    text: String,
    picks: Vec<(f32, &'static str)>,
}

fn phase_index(phase: RayPhase) -> usize {
    RayPhase::ALL
        .iter()
        .position(|&p| p == phase)
        .expect("every phase is in ALL")
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

impl SeismoApp {
    /// `--capture-ui`: ask egui for a screenshot of the window once it is due, save it when it
    /// arrives, then quit.
    fn capture_window(&mut self, ctx: &egui::Context) {
        let Some((path, after, asked)) = &mut self.capture_ui else {
            return;
        };
        if !*asked && self.started.elapsed().as_secs_f64() >= *after {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            *asked = true;
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            match write_ppm(path, w as u32, h as u32, &rgba) {
                Ok(()) => tracing::info!(path = %path.display(), w, h, "captured window"),
                Err(e) => tracing::error!(path = %path.display(), "capture failed: {e}"),
            }
            self.capture_ui = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// Write tightly packed RGBA8 rows as a binary PPM (P6), dropping alpha.
/// The search form's fields, as typed.
struct SearchForm {
    from: String,
    to: String,
    min_mag: f64,
    limit: u32,
    code: String,
    /// Why the typed search cannot be sent.
    error: Option<String>,
}

impl Default for SearchForm {
    /// The largest quakes on record.
    fn default() -> Self {
        Self {
            from: "1900-01-01".into(),
            to: Utc::now().format("%Y-%m-%d").to_string(),
            min_mag: 8.0,
            limit: 50,
            code: String::new(),
            error: None,
        }
    }
}

impl SearchForm {
    fn range(&self) -> Result<feed::Search, String> {
        let day = |s: &str, which: &str| {
            NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
                .map_err(|_| format!("the {which} date is not YYYY-MM-DD"))
        };
        Ok(feed::Search::Range {
            from: day(&self.from, "start")?,
            to: day(&self.to, "end")?,
            min_mag: self.min_mag,
            limit: self.limit,
        })
    }
}

/// An event's origin for a list row: the date with the year for old events, else month, day and
/// time of day. Both are 11 characters.
fn list_time(origin: DateTime<Utc>, now: DateTime<Utc>) -> String {
    if (now - origin).num_days() > SHOW_YEAR_AFTER_DAYS {
        origin.format("%Y-%m-%d ").to_string()
    } else {
        origin.format("%m-%d %H:%M").to_string()
    }
}

fn write_ppm(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let mut out = format!(
        "P6
{w} {h}
255
"
    )
    .into_bytes();
    out.extend(
        rgba.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]]),
    );
    std::fs::write(path, out)
}

/// The egui theme of a look.
fn visuals_for(look: render::Look) -> egui::Visuals {
    match look {
        render::Look::Crt => phosphor_visuals(),
        render::Look::Modern => modern_visuals(),
    }
}

/// Slate panels, light text, blue accents, rounded widgets.
fn modern_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = SLATE_PANEL;
    v.window_fill = SLATE_PANEL;
    v.extreme_bg_color = SLATE_BG;
    v.faint_bg_color = SLATE_WIDGET;
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    v.slider_trailing_fill = true;
    let radius = egui::CornerRadius::same(5);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius;
        w.bg_fill = SLATE_WIDGET;
        w.weak_bg_fill = SLATE_WIDGET;
        w.bg_stroke = egui::Stroke::NONE;
        w.fg_stroke = egui::Stroke::new(1.0, TEXT);
    }
    v.widgets.noninteractive.bg_fill = SLATE_PANEL;
    v.widgets.noninteractive.weak_bg_fill = SLATE_PANEL;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, SLATE_LINE);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT_DIM);
    v.widgets.hovered.bg_fill = SLATE_HOVER;
    v.widgets.hovered.weak_bg_fill = SLATE_HOVER;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT_DIM);
    v.widgets.active.bg_fill = SLATE_ACTIVE;
    v.widgets.active.weak_bg_fill = SLATE_ACTIVE;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);
    v.widgets.open.bg_fill = SLATE_ACTIVE;
    v
}

fn phosphor_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(GREEN);
    v.panel_fill = BACKGROUND;
    v.window_fill = BACKGROUND;
    v.extreme_bg_color = BACKGROUND;
    v.faint_bg_color = GREEN_DARK;
    v.hyperlink_color = GREEN;
    v.selection.bg_fill = GREEN_DARK;
    v.selection.stroke = egui::Stroke::new(1.0, GREEN);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.bg_fill = BACKGROUND;
        w.weak_bg_fill = BACKGROUND;
        w.bg_stroke = egui::Stroke::new(1.0, GREEN_DARK);
        w.fg_stroke = egui::Stroke::new(1.0, GREEN_DIM);
    }
    v.widgets.inactive.weak_bg_fill = GREEN_DARK;
    v.widgets.inactive.bg_fill = GREEN_DARK;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, GREEN_DIM);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.5, GREEN);
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, GREEN);
    v.widgets.active.fg_stroke = egui::Stroke::new(2.0, GREEN);
    v
}
