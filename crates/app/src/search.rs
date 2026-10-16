//! The USGS past-event search.

use chrono::{NaiveDate, Utc};
use eframe::egui;

use crate::SeismoApp;

impl SeismoApp {
    /// The USGS past-event search: by dates and size, or by event code.
    pub(crate) fn search_controls(&mut self, ui: &mut egui::Ui) {
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
}

/// The search form's fields, as typed.
pub(crate) struct SearchForm {
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
