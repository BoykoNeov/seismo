//! The make-your-own-event form.

use eframe::egui;
use sim::EventKind;

use session::made::{self, TIME_FORMAT};

use crate::SeismoApp;

impl SeismoApp {
    /// The make-your-own-event form: a new event, or changes to the selected one of yours.
    pub(crate) fn make_controls(&mut self, ui: &mut egui::Ui) {
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
        if f.kind == EventKind::Earthquake {
            ui.checkbox(&mut f.fault, "fault orientation")
                .on_hover_text(
                    "Give the quake a fault: its waves then leave stronger in some directions \
                 than others, and start with a push or a pull. Unticked, they leave evenly.",
                );
            if f.fault {
                ui.horizontal(|ui| {
                    for (name, dip, rake) in [
                        ("thrust", 30.0, 90.0),
                        ("normal", 60.0, -90.0),
                        ("strike-slip", 90.0, 0.0),
                    ] {
                        if ui
                            .small_button(name)
                            .on_hover_text(format!("dip {dip:.0}°, rake {rake:.0}°"))
                            .clicked()
                        {
                            (f.dip_deg, f.rake_deg) = (dip, rake);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("strike");
                    ui.add(
                        egui::DragValue::new(&mut f.strike_deg)
                            .range(0.0..=360.0)
                            .max_decimals(0)
                            .suffix("°"),
                    )
                    .on_hover_text("The fault's direction, clockwise from north");
                    ui.label("dip");
                    ui.add(
                        egui::DragValue::new(&mut f.dip_deg)
                            .range(0.0..=90.0)
                            .max_decimals(0)
                            .suffix("°"),
                    )
                    .on_hover_text("How steep the fault is: 0 flat, 90 vertical");
                    ui.label("rake");
                    ui.add(
                        egui::DragValue::new(&mut f.rake_deg)
                            .range(-180.0..=180.0)
                            .max_decimals(0)
                            .suffix("°"),
                    )
                    .on_hover_text(
                        "Which way the rock above the fault slid: 90 up (thrust), -90 down \
                         (normal), 0 or 180 sideways (strike-slip)",
                    );
                });
            }
        }
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
}
