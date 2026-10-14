#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! `app`: the eframe/egui shell. Owns the sim clock and wires sim, render and feed together.
//! Allowed dependencies: everything in the workspace.

mod clock;

use std::time::Instant;

use anyhow::{Context as _, anyhow};
use chrono::{DateTime, TimeZone, Utc};
use eframe::{egui, wgpu};
use sim::tables::{EarthModel, TravelTimeTable};
use sim::{Event, EventKind, Simulation};

use clock::{MAX_SPEED, MIN_SPEED, SimClock, secs};

static TABLE_BYTES: &[u8] = include_bytes!("../../../assets/ak135.tt");
static MODEL_BYTES: &[u8] = include_bytes!("../../../assets/ak135_model.csv");

/// Scrub range around the selected event's origin (s).
const SCRUB_BEFORE_S: f64 = 60.0;
const SCRUB_AFTER_S: f64 = 90.0 * 60.0;

const GREEN: egui::Color32 = egui::Color32::from_rgb(0x33, 0xff, 0x33);
const GREEN_DIM: egui::Color32 = egui::Color32::from_rgb(0x1f, 0x99, 0x1f);
const GREEN_DARK: egui::Color32 = egui::Color32::from_rgb(0x0e, 0x40, 0x0e);
const BACKGROUND: egui::Color32 = egui::Color32::from_rgb(0x05, 0x08, 0x05);

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn,naga=warn".into()),
        )
        .init();

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("SEISMO")
            .with_inner_size([1600.0, 900.0])
            .with_min_inner_size([640.0, 360.0]),
        ..Default::default()
    };
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

/// The hardcoded phase-1 test event: 2011 Tōhoku.
fn tohoku() -> Event {
    let origin = Utc.with_ymd_and_hms(2011, 3, 11, 5, 46, 24).unwrap();
    Event::new(
        "usp000hvnu",
        EventKind::Earthquake,
        origin,
        38.297,
        142.373,
        29.0,
        9.1,
    )
}

struct SeismoApp {
    sim: Simulation,
    clock: SimClock,
    selected: usize,
    renderer: render::SceneRenderer,
    texture_id: egui::TextureId,
    crt: render::CrtSettings,
    style: render::MapStyle,
    started: Instant,
    last_frame: Instant,
    last_seek_count: u64,
    /// Smoothed real frame time (s), for the fps readout.
    frame_time_s: f64,
    /// `--capture <file.ppm>` / `--capture-after <s>`: save the scene once, then quit.
    capture: Option<(std::path::PathBuf, f64)>,
}

impl SeismoApp {
    fn new(cc: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .ok_or_else(|| anyhow!("wgpu backend not available"))?;
        let table = TravelTimeTable::from_bytes(TABLE_BYTES).context("embedded ak135.tt")?;
        let model = EarthModel::from_bytes(MODEL_BYTES).context("embedded ak135_model.csv")?;
        tracing::info!(
            model = table.model(),
            obspy = table.obspy_version(),
            phases = ?table.phases(),
            velocity_points = model.points().len(),
            "loaded ak135 assets"
        );
        tracing::info!(adapter = ?rs.adapter.get_info().name, backend = ?rs.adapter.get_info().backend, "gpu");

        let renderer = render::SceneRenderer::new(&rs.device, &rs.queue, &table, (256, 128));
        let texture_id = rs.renderer.write().register_native_texture(
            &rs.device,
            renderer.output_view(),
            wgpu::FilterMode::Nearest,
        );

        cc.egui_ctx.set_visuals(phosphor_visuals());

        let events = vec![tohoku()];
        // Debug/screenshot flags:
        //   --start-offset <s>  start the clock that many seconds after the event's origin
        //   --speed <x>         initial clock speed
        //   --crt <list>        comma-separated: off, noscan, nopersist
        let arg = |name: &str| std::env::args().skip_while(|a| a != name).nth(1);
        let offset = arg("--start-offset")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let mut clock = SimClock::new(events[0].origin + secs(offset));
        if let Some(speed) = arg("--speed").and_then(|s| s.parse::<f64>().ok()) {
            clock.set_speed(speed);
        }
        //   --capture <file.ppm> [--capture-after <s>]  save the rendered scene after <s> real
        //                       seconds (default 3) as a binary PPM, then quit
        let capture = arg("--capture").map(|p| {
            let after = arg("--capture-after")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(3.0);
            (std::path::PathBuf::from(p), after)
        });
        let mut crt = render::CrtSettings::default();
        for flag in arg("--crt").unwrap_or_default().split(',') {
            match flag {
                "off" => crt.enabled = false,
                "noscan" => crt.scanlines = false,
                "nopersist" => crt.persistence = false,
                _ => {}
            }
        }
        let now = Instant::now();
        Ok(Self {
            sim: Simulation::new(table, events),
            clock,
            selected: 0,
            renderer,
            texture_id,
            crt,
            style: render::MapStyle::default(),
            started: now,
            last_frame: now,
            last_seek_count: 0,
            frame_time_s: 1.0 / 60.0,
            capture,
        })
    }

    fn origin(&self) -> DateTime<Utc> {
        self.sim.events[self.selected].origin
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("SEISMO");
            ui.colored_label(
                GREEN_DIM,
                format!("{:.0} fps", 1.0 / self.frame_time_s.max(1e-6)),
            );
        });
        ui.separator();

        let now = self.clock.now();
        // chrono has no one-decimal fraction specifier; append tenths by hand.
        let tenths = now.timestamp_subsec_millis() / 100;
        ui.monospace(format!("{}.{tenths} UTC", now.format("%Y-%m-%d %H:%M:%S")));

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
        });

        let mut speed = self.clock.speed();
        ui.add(
            egui::Slider::new(&mut speed, MIN_SPEED..=MAX_SPEED)
                .logarithmic(true)
                .text("speed ×")
                .max_decimals(1),
        );
        self.clock.set_speed(speed);

        let event = &self.sim.events[self.selected];
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
        ui.label(format!(
            "2011 Tōhoku  M{:.1} {}",
            event.magnitude, event.mag_type
        ));
        ui.monospace(format!(
            "{:.3}°N {:.3}°E  {:.0} km",
            event.lat, event.lon, event.depth_km
        ));
        let state = &self.sim.state_at(now).events[self.selected];
        let fmt = |d: Option<f64>| d.map_or("—".to_owned(), |d| format!("{d:6.2}°"));
        ui.monospace(format!("t   {:9.1} s", state.t));
        ui.monospace(format!("P   {}", fmt(state.p_front_deg)));
        ui.monospace(format!("S   {}", fmt(state.s_front_deg)));
        ui.colored_label(GREEN_DIM, "P solid, S dashed. Direct waves only.");

        ui.separator();
        egui::CollapsingHeader::new("CRT")
            .default_open(true)
            .show(ui, |ui| {
                let c = &mut self.crt;
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
                    ui.colored_label(GREEN_DIM, "bloom, barrel, flicker: phase 2");
                });
            });

        egui::CollapsingHeader::new("Map")
            .default_open(false)
            .show(ui, |ui| {
                let s = &mut self.style;
                ui.add(egui::Slider::new(&mut s.front_width_px, 1.0..=6.0).text("front px"));
                ui.add(
                    egui::Slider::new(&mut s.trail_s, 1.0..=300.0)
                        .logarithmic(true)
                        .text("trail s"),
                );
                ui.add(egui::Slider::new(&mut s.trail_strength, 0.0..=1.0).text("trail"));
                ui.add(egui::Slider::new(&mut s.grid_brightness, 0.0..=1.0).text("grid"));
            });
    }

    fn scene(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame, real_dt: f64) {
        let Some(rs) = frame.wgpu_render_state() else {
            return;
        };

        // Largest 2:1 rectangle in the available space, centred.
        let avail = ui.available_rect_before_wrap();
        let w = avail.width().min(avail.height() * 2.0).max(1.0);
        let rect = egui::Rect::from_center_size(avail.center(), egui::vec2(w, w / 2.0));
        let ppp = ui.ctx().pixels_per_point();
        let px = (
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        );

        if self.renderer.resize(&rs.device, px) {
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
        let events: Vec<render::MapEventInstance> = self
            .sim
            .map_events(now, render::MAX_MAP_EVENTS, linger)
            .into_iter()
            .map(|m| {
                let e = &self.sim.events[m.index];
                render::MapEventInstance {
                    lat: e.lat as f32,
                    lon: e.lon as f32,
                    depth_km: e.depth_km as f32,
                    t: m.t as f32,
                    magnitude: e.magnitude as f32,
                }
            })
            .collect();

        let seek = self.clock.seek_count();
        let time = render::FrameTime {
            dt_s: real_dt as f32,
            time_s: self.started.elapsed().as_secs_f32(),
            clear_persistence: seek != self.last_seek_count,
        };
        self.last_seek_count = seek;

        let mut encoder = rs
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene"),
            });
        self.renderer.render(
            &rs.queue,
            &mut encoder,
            &events,
            &self.style,
            &self.crt,
            time,
        );
        rs.queue.submit([encoder.finish()]);

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

        ui.put(
            rect,
            egui::Image::new(egui::load::SizedTexture::new(self.texture_id, rect.size())),
        );
    }
}

impl eframe::App for SeismoApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let now = Instant::now();
        let real_dt = (now - self.last_frame).as_secs_f64();
        self.last_frame = now;
        self.clock.tick(real_dt);
        self.frame_time_s += (real_dt.min(1.0) - self.frame_time_s) * 0.05;

        egui::Panel::left("controls")
            .resizable(false)
            .exact_size(260.0)
            .show(ui, |ui| {
                self.controls(ui);
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND))
            .show(ui, |ui| self.scene(ui, frame, real_dt));

        ui.ctx().request_repaint();
    }
}

/// Write tightly packed RGBA8 rows as a binary PPM (P6), dropping alpha.
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
