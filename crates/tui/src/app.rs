//! The terminal version's state: the simulation, the clock, the event list and the traces
//! (all shared with the desktop app through `session`), plus what this front end shows.

use std::collections::HashMap;

use anyhow::Context as _;
use chrono::Utc;
use feed::FeedHandle;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use sim::Simulation;
use sim::seismogram::Component;

use session::assets;
use session::clock::{MAX_SPEED, MIN_SPEED, SimClock, secs};
use session::events::{EventKey, EventList, Listed};
use session::guide::{Front, Guide, Values};
use session::made;
use session::traces::Traces;

use crate::guide::{GuideView, Outcome};
use crate::map::{MapView, Projection};
use crate::palette::Look;
use crate::slice::SliceWorker;

/// Small and large scrub steps (s).
const STEP_S: f64 = 30.0;
const BIG_STEP_S: f64 = 300.0;
/// Slice azimuth step (deg).
const AZIMUTH_STEP: f64 = 10.0;
/// Minimum-magnitude step, and its range.
const MAG_STEP: f64 = 0.5;
const MAG_RANGE: (f64, f64) = (0.0, 9.0);
/// A click within this many cells of a station picks it.
const PICK_CELLS: f64 = 1.5;

/// The big picture shown above the traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Map,
    Slice,
}

/// Start-up choices, from the command line.
#[derive(Debug, Clone)]
pub struct Options {
    pub offline: bool,
    /// Select this event (a built-in one, one of yours as `made-<n>`, or a USGS id).
    pub event: Option<String>,
    /// Replay the selected event from this many seconds after its origin.
    pub start_offset: Option<f64>,
    pub speed: Option<f64>,
    pub look: Look,
    pub view: View,
    pub azimuth_deg: f64,
    pub traces: bool,
    pub component: Component,
    pub shared_scale: bool,
    /// Open the guide at this topic.
    pub guide: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            offline: false,
            event: None,
            start_offset: None,
            speed: None,
            look: Look::Crt,
            view: View::Map,
            azimuth_deg: 90.0,
            traces: true,
            component: Component::Vertical,
            shared_scale: false,
            guide: None,
        }
    }
}

pub struct App {
    pub sim: Simulation,
    /// Keys of `sim.events`, in the same order.
    pub keys: Vec<EventKey>,
    pub events: EventList,
    events_rev: u64,
    pub clock: SimClock,
    pub traces: Traces,
    /// The selected event's handle when the traces last followed it.
    traces_follow: u64,
    /// `--event` with `--start-offset`: seek this many seconds after its origin once listed.
    pending_seek: Option<f64>,
    pub map: MapView,
    pub slice: SliceWorker,
    pub azimuth_deg: f64,
    pub view: View,
    pub look: Look,
    /// The event list's cursor, by handle.
    pub cursor: u64,
    /// The help overlay is up: the keys, or the guide.
    pub help: bool,
    pub guide: GuideView,
    /// A note for the status line (e.g. why your events could not be read).
    pub message: Option<String>,
    /// Where the map was last drawn and its size in cells, for mouse picking.
    pub map_area: Option<Rect>,
    pub quit: bool,
}

impl App {
    pub fn new(opts: &Options, feed: Option<FeedHandle>) -> anyhow::Result<Self> {
        let (table, model, dispersion) = assets::earth().context("embedded assets")?;
        let mut events = EventList::new(
            vec![Listed::builtin(assets::tohoku(), assets::TOHOKU_PLACE)],
            feed,
        );
        let mut message = None;
        // Your own events are listed but never changed here: the desktop app makes and edits them.
        if let Some(path) = made::default_path() {
            match made::load(&path) {
                Ok(m) => {
                    events.load_made(m);
                }
                Err(e) => message = Some(format!("Could not read your events ({e})")),
            }
        }
        if let Some(id) = &opts.event {
            events.want(id.clone());
        }
        let pending_seek = opts.start_offset.filter(|_| events.wanted_id.is_some());
        let mut clock = match opts.start_offset.filter(|_| pending_seek.is_none()) {
            Some(offset) => {
                events.user_chose = true;
                let selected = events.selected_handle();
                let origin = events
                    .shown()
                    .iter()
                    .find(|l| l.key.handle == selected)
                    .map_or(assets::tohoku().origin, |l| l.event.origin);
                SimClock::new(origin + secs(offset))
            }
            None => SimClock::new(Utc::now()),
        };
        if let Some(speed) = opts.speed {
            clock.set_speed(speed);
        }
        let stations = sim::parse_stations(assets::STATIONS).context("embedded stations.csv")?;
        let coast =
            sim::coast::Coastlines::from_bytes(assets::COAST).context("embedded coastlines.txt")?;
        let shown: Vec<sim::Event> = events.shown().iter().map(|l| l.event.clone()).collect();
        let keys: Vec<EventKey> = events.shown().iter().map(|l| l.key).collect();
        let sim = Simulation::new(table, model, dispersion, shown).context("ray tracer")?;
        let mut traces = Traces::new(stations, sim.tracer().clone());
        traces.show = opts.traces;
        traces.component = opts.component;
        traces.shared_scale = opts.shared_scale;
        let map = MapView::new(coast, sim.surface_waves().map_grid());
        let slice = SliceWorker::spawn(sim.tracer().clone());
        let mut values = Values::new(sim.facts());
        values.set("stations", traces.stations.len().to_string());
        let mut guide =
            GuideView::new(Guide::new(Front::Terminal, &values).map_err(anyhow::Error::msg)?);
        if let Some(id) = &opts.guide {
            guide.open_at(id);
        }
        let mut app = Self {
            events_rev: events.rev(),
            traces_follow: u64::MAX - 1,
            cursor: events.selected_handle(),
            events,
            keys,
            sim,
            clock,
            traces,
            pending_seek,
            map,
            slice,
            azimuth_deg: opts.azimuth_deg.rem_euclid(360.0),
            view: opts.view,
            look: opts.look,
            help: opts.guide.is_some(),
            guide,
            message,
            map_area: None,
            quit: false,
        };
        app.sync();
        Ok(app)
    }

    /// Index of the selected event in `sim.events`. The list always holds the selected event.
    pub fn sel(&self) -> usize {
        let handle = self.events.selected_handle();
        self.keys
            .iter()
            .position(|k| k.handle == handle)
            .unwrap_or(0)
    }

    /// The selected event as listed.
    pub fn selected(&self) -> Option<&Listed> {
        let handle = self.events.selected_handle();
        self.events.shown().iter().find(|l| l.key.handle == handle)
    }

    /// Seconds since the selected event's origin.
    pub fn t(&self) -> f64 {
        self.sim.events[self.sel()].t_at(self.clock.now())
    }

    /// Advance the clock by `real_dt` real seconds and bring everything up to date.
    pub fn tick(&mut self, real_dt: f64) {
        self.clock.tick(real_dt);
        self.sync();
    }

    /// Take the feed's news, bring the simulation's events up to date with the list, and
    /// rebuild the traces that need it.
    fn sync(&mut self) {
        self.events.poll(self.clock.now());
        if self.events.rev() != self.events_rev {
            self.events_rev = self.events.rev();
            let shown = self.events.shown();
            self.sim.events = shown.iter().map(|l| l.event.clone()).collect();
            self.keys = shown.iter().map(|l| l.key).collect();
            if !self.keys.iter().any(|k| k.handle == self.cursor) {
                self.cursor = self.events.selected_handle();
            }
        }
        let replay = self.events.take_replay();
        let origin = self.sim.events[self.sel()].origin;
        if let Some(offset) = self.pending_seek
            && self.events.wanted_id.is_none()
        {
            self.pending_seek = None;
            self.clock.seek(origin + secs(offset));
        } else if replay {
            self.clock.seek(origin);
            self.clock.set_playing(true);
        }
        let handle = self.events.selected_handle();
        if handle != self.traces_follow {
            self.traces_follow = handle;
            self.cursor = handle;
            if !self.traces.user_picked {
                let e = self.sim.events[self.sel()].clone();
                self.traces.pick_defaults(&e);
            }
        }
        if self.view == View::Slice {
            // Ask early: the fans take a while, and the frame only shows them once in.
            let depth = self.sim.events[self.sel()].depth_km;
            self.slice.fans(depth);
        }
        if self.traces.show {
            // 1D traces: no 3D-mantle delays here.
            let sel = self.sel();
            self.traces
                .update(&self.sim, &self.keys, sel, &HashMap::new(), 0.0);
        }
    }

    fn move_cursor(&mut self, by: isize) {
        let shown = self.events.shown();
        if shown.is_empty() {
            return;
        }
        let at = shown
            .iter()
            .position(|l| l.key.handle == self.cursor)
            .unwrap_or(0) as isize;
        let to = (at + by).clamp(0, shown.len() as isize - 1) as usize;
        self.cursor = shown[to].key.handle;
    }

    fn seek_by(&mut self, s: f64) {
        let to = self.clock.now() + secs(s);
        self.clock.seek(to);
    }

    pub fn key(&mut self, k: KeyEvent) {
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        if self.help {
            // q still quits; the overlay takes every other key (any key closes the key list).
            if matches!(k.code, KeyCode::Char('q')) {
                self.help = false;
            } else {
                if self.guide.key(k) == Outcome::Close {
                    self.help = false;
                }
                return;
            }
        }
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char('?') | KeyCode::Char('h') | KeyCode::F(1) => {
                self.help = true;
                self.guide.page = crate::guide::Page::Keys;
            }
            KeyCode::Char('g') => {
                self.help = true;
                self.guide.page = crate::guide::Page::Topics;
            }
            KeyCode::Char(' ') => self.clock.set_playing(!self.clock.playing()),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.clock
                    .set_speed((self.clock.speed() * 2.0).min(MAX_SPEED));
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                self.clock
                    .set_speed((self.clock.speed() / 2.0).max(MIN_SPEED));
            }
            KeyCode::Char('0') => self.clock.set_speed(1.0),
            KeyCode::Left => self.seek_by(if shift { -BIG_STEP_S } else { -STEP_S }),
            KeyCode::Right => self.seek_by(if shift { BIG_STEP_S } else { STEP_S }),
            KeyCode::Char('<') | KeyCode::Char(',') => self.seek_by(-BIG_STEP_S),
            KeyCode::Char('>') | KeyCode::Char('.') => self.seek_by(BIG_STEP_S),
            KeyCode::Char('r') => {
                let origin = self.sim.events[self.sel()].origin;
                self.clock.seek(origin);
                self.clock.set_playing(true);
            }
            KeyCode::Char('n') => {
                self.clock.seek(Utc::now());
                self.clock.set_speed(1.0);
                self.clock.set_playing(true);
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::PageUp => self.move_cursor(-10),
            KeyCode::PageDown => self.move_cursor(10),
            KeyCode::Home => self.move_cursor(isize::MIN / 2),
            KeyCode::End => self.move_cursor(isize::MAX / 2),
            KeyCode::Enter => {
                // Selecting never moves the clock.
                let cursor = self.cursor;
                self.events.select(cursor);
                self.sync();
            }
            KeyCode::Tab | KeyCode::Char('v') => {
                self.view = match self.view {
                    View::Map => View::Slice,
                    View::Slice => View::Map,
                };
                self.sync();
            }
            KeyCode::Char('[') => {
                self.azimuth_deg = (self.azimuth_deg - AZIMUTH_STEP).rem_euclid(360.0);
            }
            KeyCode::Char(']') => {
                self.azimuth_deg = (self.azimuth_deg + AZIMUTH_STEP).rem_euclid(360.0);
            }
            KeyCode::Char('c') => {
                self.traces.component = match self.traces.component {
                    Component::Vertical => Component::Transverse,
                    Component::Transverse => Component::Vertical,
                };
                self.sync();
            }
            KeyCode::Char('s') => self.traces.shared_scale = !self.traces.shared_scale,
            KeyCode::Char('t') => {
                self.traces.show = !self.traces.show;
                self.sync();
            }
            KeyCode::Char('d') => {
                let e = self.sim.events[self.sel()].clone();
                self.traces.pick_defaults(&e);
                self.traces.note = None;
                self.sync();
            }
            KeyCode::Char('m') => self.step_min_mag(-MAG_STEP),
            KeyCode::Char('M') => self.step_min_mag(MAG_STEP),
            KeyCode::Char('l') => self.look = self.look.other(),
            _ => {}
        }
    }

    fn step_min_mag(&mut self, by: f64) {
        let m = (self.events.min_mag() + by).clamp(MAG_RANGE.0, MAG_RANGE.1);
        self.events.set_min_mag(m);
        self.sync();
    }

    /// A left click on the map shows or hides the nearest station's trace, or places a new
    /// station there.
    pub fn mouse(&mut self, m: MouseEvent) {
        if m.kind != MouseEventKind::Down(MouseButton::Left) || self.view != View::Map {
            return;
        }
        let Some(area) = self.map_area else {
            return;
        };
        let (col, row) = (m.column, m.row);
        if col < area.x || row < area.y || col >= area.x + area.width || row >= area.y + area.height
        {
            return;
        }
        let proj = Projection {
            width: 2 * area.width as usize,
            height: 4 * area.height as usize,
        };
        let (cx, cy) = (f64::from(col - area.x), f64::from(row - area.y));
        let (lat, lon) = proj.lat_lon(2.0 * cx + 1.0, 4.0 * cy + 2.0);
        // Nearest station within a cell or so, measured in cells.
        let nearest = self
            .traces
            .stations
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (x, y) = proj.dot(s.lat, s.lon);
                let d = ((x / 2.0 - cx - 0.5).powi(2) + (y / 4.0 - cy - 0.5).powi(2)).sqrt();
                (i, d)
            })
            .filter(|(_, d)| *d <= PICK_CELLS)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        match nearest {
            Some((i, _)) => self.traces.toggle(i),
            None => self.traces.place(lat, lon),
        }
        self.sync();
    }
}
