//! Picking stations (and the made event's place) on the map.

use eframe::egui;

use crate::{PICK_RADIUS, SeismoApp};

impl SeismoApp {
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
    pub(crate) fn station_picking(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        response: &egui::Response,
    ) {
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
}
