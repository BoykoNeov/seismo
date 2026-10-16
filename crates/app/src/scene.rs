//! The scene: map, slice and traces drawn by `render`, and the text over them.

use std::sync::Arc;
use std::time::Instant;

use eframe::{egui, wgpu};
use sim::crust::CUT_STEP_DEG;
use sim::seismogram::{Component, StationArrival};
use sim::slice::{PhaseLabel, SliceLine};
use sim::{EARTH_RADIUS_KM, RayPhase};

use session::events::EventKey;

use crate::capture::write_ppm;
use crate::mantle::{CutKey, Ready};
use crate::{MIN_TRACES_WIDTH, SeismoApp, bench};

impl SeismoApp {
    pub(crate) fn scene(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame, real_dt: f64) {
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
        let shown = |phase: RayPhase| enabled[phase_index(phase)];
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
        let fronts = fans.fronts_where(t, shown);
        let labels: Vec<PhaseLabel> = fronts.labels;
        let front_lines = fronts.lines;
        let spacing = trail_s / 2.0;
        let ghosts: Vec<(f32, Vec<SliceLine>)> = (1..=trail_fronts)
            .map(|k| {
                let dt = spacing * k as f64;
                let gain = trail_strength * (-dt / trail_s).exp() as f32;
                (gain, fans.fronts_where(t - dt, shown).lines)
            })
            .collect();
        let paths = if ray_paths {
            fans.ray_paths_where(t, per_phase, shown)
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
        // The crust strip: the real crust along the cut, or with the seismograms' real-crust
        // switch off the 1D model's, which they then use.
        if let Some(crust) = self.sim.crust() {
            let (lat, lon) = (self.sim.events[sel].lat, self.sim.events[sel].lon);
            let (az, real) = (self.slice.azimuth_deg, self.traces.real_crust);
            let key = [lat.to_bits(), lon.to_bits(), az.to_bits(), u64::from(real)];
            if self.crust_cut.as_ref().is_none_or(|(k, _)| *k != key) {
                let samples = if real {
                    crust.along(lat, lon, az)
                } else {
                    vec![crust.flat(); (180.0 / CUT_STEP_DEG).round() as usize + 1]
                };
                self.crust_cut = Some((key, samples));
            }
        }
        let crust_strip = match (self.sim.crust(), &self.crust_cut) {
            (Some(crust), Some((_, samples))) => Some(render::CrustStrip {
                samples,
                step_deg: CUT_STEP_DEG,
                range_km: strip_range_km(crust),
                reference_moho_km: crust.reference_moho_km(),
            }),
            _ => None,
        };
        let slice_scene = render::SliceScene {
            source_depth_km: depth_km,
            boundaries: self.sim.boundaries(),
            crust: crust_strip,
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
        // A phase's later branches get a tick but no label of their own.
        let in_window: Vec<Vec<(f32, &StationArrival)>> = shown
            .iter()
            .map(|tr| {
                tr.arrivals
                    .iter()
                    .filter(|a| a.motion.weight(component) > 0.0)
                    .map(|a| (frac(a.time_s), a))
                    .filter(|(f, _)| (0.0..=1.0).contains(f))
                    .collect()
            })
            .collect();
        let ticks: Vec<Vec<f32>> = in_window
            .iter()
            .map(|v| v.iter().map(|(f, _)| *f).collect())
            .collect();
        let picks: Vec<Vec<(f32, &'static str)>> = in_window
            .iter()
            .map(|v| {
                v.iter()
                    .filter(|(_, a)| a.branch == 0)
                    .map(|(f, a)| (*f, a.phase))
                    .collect()
            })
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
        let playhead = self.listen.playhead();
        let trace_labels: Vec<TraceLabel> = shown
            .iter()
            .zip(picks)
            .map(|(tr, picks)| {
                let s = &self.traces.stations[tr.station];
                TraceLabel {
                    text: format!("{}  {:.1}°  {}", s.label(), tr.dist_deg, s.name),
                    picks,
                    playhead: playhead
                        .filter(|&(station, _)| station == tr.station)
                        .map(|(_, at)| frac(at))
                        .filter(|f| (0.0..=1.0).contains(f)),
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
    pub(crate) fn ui_to_scene(&self, rect: egui::Rect, pos: egui::Pos2) -> Option<[f32; 2]> {
        let size = self.renderer.size();
        let scale = rect.width() / size.0.max(1) as f32;
        let d = (pos - rect.min) / scale;
        self.post().display_to_scene(size, [d.x, d.y])
    }

    /// The map's projection and the UI point of a station's marker.
    pub(crate) fn map_frame(&self) -> (render::MapProjection, render::Viewport) {
        let vp = self.renderer.map_viewport();
        (render::MapProjection { size: (vp.w, vp.h) }, vp)
    }

    /// Where the map point `lat`, `lon` appears in UI points.
    pub(crate) fn map_to_ui(&self, rect: egui::Rect, lat: f64, lon: f64) -> egui::Pos2 {
        let (proj, vp) = self.map_frame();
        let [x, y] = proj.pixel(lat, lon);
        self.scene_to_ui(rect, [x + vp.x, y + vp.y])
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
            // Where the sound is, while this trace is played.
            if let Some(f) = row.playhead {
                let x = layout.x_at(f);
                painter.line_segment(
                    [to_ui(x, base - half), to_ui(x, base + half)],
                    egui::Stroke::new(1.5, pal.text),
                );
            }
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
        let view = render::SliceView::new((vp.w, vp.h), self.sim.crust().is_some());
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

        // The crust strip: what it is, how much it is stretched, and its depths.
        if let Some(crust) = self.sim.crust() {
            let range = strip_range_km(crust);
            let y = |depth: f64| view.strip_y(depth, range).unwrap_or(0.0);
            if let (Some([x0, y0, x1, _]), Some(stretch)) = (view.strip, view.strip_stretch(range))
            {
                let which = if self.traces.real_crust {
                    "CRUST ALONG THE CUT"
                } else {
                    "CRUST ALONG THE CUT: FLAT (REAL CRUST OFF)"
                };
                text(
                    to_ui([0.5 * (x0 + x1), y0 - 3.0]),
                    egui::Align2::CENTER_BOTTOM,
                    &format!("{which}  ·  DEPTH ×{stretch:.0}"),
                    pal.dim,
                );
                let right = |depth: f64, s: &str| {
                    text(
                        to_ui([x1 + 4.0, y(depth)]),
                        egui::Align2::LEFT_CENTER,
                        s,
                        pal.dim,
                    );
                };
                right(0.0, "0 km");
                right(
                    crust.reference_moho_km(),
                    &format!("{:.0} km", crust.reference_moho_km()),
                );
                right(range.1, &format!("{:.0} km", range.1));
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
    /// Where the sound is along the row (0–1 of the window), while it is played.
    playhead: Option<f32>,
}

/// Depths (km below sea level) the crust strip spans: the crust model's highest ground and
/// deepest Moho, out to the next 5 km.
fn strip_range_km(crust: &sim::crust::Crust) -> (f64, f64) {
    let (top, bottom) = crust.extremes_km();
    ((top / 5.0).floor() * 5.0, (bottom / 5.0).ceil() * 5.0)
}

pub(crate) fn phase_index(phase: RayPhase) -> usize {
    RayPhase::ALL
        .iter()
        .position(|&p| p == phase)
        .expect("every phase is in ALL")
}
