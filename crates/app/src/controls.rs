//! The controls panel: its sections and their widgets.

use chrono::{DateTime, Utc};
use eframe::egui;
use sim::seismogram::Component;
use sim::{EventKind, RayPhase, Wave, azimuth_deg};

use session::clock::{MAX_SPEED, MIN_SPEED, secs};
use session::events::FOLLOW_MIN_MAG;
use session::traces::MAX_TRACES;

use crate::{
    MAX_MANTLE_STRENGTH, SCRUB_AFTER_S, SCRUB_BEFORE_S, SHOW_YEAR_AFTER_DAYS, SeismoApp,
    TRACE_WINDOW_MIN,
};

impl SeismoApp {
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
        ui.checkbox(&mut tr.real_crust, "real crust").on_hover_text(
            "Arrival times include the real crust (CRUST1.0) under the quake, the station and              where a wave bounces off the surface: thin under oceans (early), thick under              mountains (late). Off: the same flat crust everywhere.",
        );
        ui.add(
            egui::Slider::new(&mut tr.gain, 0.25..=20.0)
                .logarithmic(true)
                .text("gain")
                .max_decimals(2),
        );
        ui.add(
            egui::Slider::new(&mut tr.window_min, TRACE_WINDOW_MIN.0..=TRACE_WINDOW_MIN.1)
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

    pub(crate) fn controls(&mut self, ui: &mut egui::Ui) {
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
        if event.kind == EventKind::Earthquake {
            match event.fault() {
                Some(&fault) => {
                    ui.horizontal(|ui| {
                        self.beach_ball
                            .show(ui, fault, 56.0, pal.text)
                            .on_hover_text(
                                "The fault's beach ball: the lower half of a sphere round the \
                                 quake, seen from above with north up. Filled: the P wave \
                                 leaves that way as a push (the ground moves up); empty: as \
                                 a pull (it moves down). The P and S waves on the seismograms \
                                 follow this pattern: the big swing of each P pulse is up or \
                                 down to match.",
                            );
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(format!("{} fault", fault.style().name()));
                                self.guide.glyph(ui, "fault");
                            });
                            ui.label(format!(
                                "strike {:.0}° dip {:.0}° rake {:.0}°",
                                fault.strike_deg, fault.dip_deg, fault.rake_deg
                            ));
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(fault.style().describe()).color(pal.dim),
                                )
                                .wrap(),
                            );
                        });
                    });
                }
                None => {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(pal.dim, "No fault orientation known: waves leave");
                        ui.colored_label(pal.dim, "evenly in every direction.");
                        self.guide.glyph(ui, "fault");
                    });
                }
            }
            self.guide.note(ui, "fault");
        }
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
