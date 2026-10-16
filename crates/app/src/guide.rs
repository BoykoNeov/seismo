//! The guide on the desktop: its window (topics by part, a search box, links between topics),
//! the `?` glyphs that unfold a section's explanation under its name, the `?` buttons in the
//! corners of the pictures, and a welcome card on the very first start. The text itself is
//! `session::guide`, shared with the terminal version.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui;
use session::guide::{Block, Guide, Part, Span, Topic};

/// Width of the topic list in the guide window (points).
const LIST_WIDTH: f32 = 210.0;

pub struct GuideUi {
    guide: Guide,
    /// The guide window is open.
    pub open: bool,
    /// The topic shown, as an index into the guide's topics.
    current: usize,
    /// Topics shown before, for Back.
    back: Vec<usize>,
    query: String,
    /// Glyphs whose explanation is unfolded, by topic id.
    unfolded: HashSet<&'static str>,
    /// The page changed: scroll it back to the top.
    to_top: bool,
    /// The welcome card is up; dismissing it leaves this marker file so it never shows again.
    welcome: Option<PathBuf>,
}

impl GuideUi {
    /// `topic`: open the window there at once. `welcome`: show the welcome card, and leave this
    /// marker file when it is dismissed.
    pub fn new(guide: Guide, topic: Option<&str>, welcome: Option<PathBuf>) -> Self {
        let mut ui = Self {
            guide,
            open: false,
            current: 0,
            back: Vec::new(),
            query: String::new(),
            unfolded: HashSet::new(),
            to_top: true,
            welcome,
        };
        if let Some(id) = topic {
            match ui.guide.index_of(id) {
                Some(_) => ui.open_at(id),
                None => tracing::warn!(topic = id, "no such guide topic"),
            }
        }
        ui
    }

    /// Open the window at topic `id` (remembering the page left, for Back).
    pub fn open_at(&mut self, id: &str) {
        let Some(i) = self.guide.index_of(id) else {
            tracing::warn!(topic = id, "no such guide topic");
            return;
        };
        self.go(i);
        self.open = true;
    }

    fn go(&mut self, i: usize) {
        if i != self.current {
            if self.open {
                self.back.push(self.current);
            }
            self.current = i;
            self.to_top = true;
        }
    }

    /// F1 (or `?` while no text box is being typed in) opens and closes the guide; Escape closes
    /// it.
    pub fn keys(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let (f1, question, escape) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::F1),
                i.key_pressed(egui::Key::Questionmark),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if f1 || (question && !typing) {
            self.open = !self.open;
            self.dismiss_welcome();
        } else if escape && !typing && self.open {
            self.open = false;
        } else if escape && !typing {
            self.dismiss_welcome();
        }
    }

    /// A small `?` beside a section's name: unfolds the topic's opening paragraph under it.
    pub fn glyph(&mut self, ui: &mut egui::Ui, topic: &'static str) {
        let unfolded = self.unfolded.contains(topic);
        let hint = match self.guide.topic(topic) {
            Some(t) if unfolded => format!("Hide the explanation of {}", t.title.to_lowercase()),
            Some(t) => format!("What is this? ({})", t.title),
            None => return,
        };
        let glyph = egui::Button::new(egui::RichText::new("?").strong())
            .small()
            .selected(unfolded);
        if ui.add(glyph).on_hover_text(hint).clicked() {
            if unfolded {
                self.unfolded.remove(topic);
            } else {
                self.unfolded.insert(topic);
            }
        }
    }

    /// The unfolded explanation of `topic`, if its glyph is on: the topic's opening paragraph and
    /// a link to the whole topic in the guide.
    pub fn note(&mut self, ui: &mut egui::Ui, topic: &'static str) {
        if !self.unfolded.contains(topic) {
            return;
        }
        let Some(t) = self.guide.topic(topic) else {
            return;
        };
        let lead = t.lead().to_vec();
        let title = t.title.clone();
        let mut follow = None;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            follow = spans(ui, &lead);
            if ui.link(format!("More: {title} →")).clicked() {
                follow = Some(topic.to_owned());
            }
        });
        if let Some(id) = follow {
            self.open_at(&id);
        }
    }

    /// A `?` button centred at `centre` (over a picture): opens the guide at `topic`.
    pub fn corner(&mut self, ui: &mut egui::Ui, centre: egui::Pos2, topic: &str) {
        let Some(t) = self.guide.topic(topic) else {
            return;
        };
        let hint = format!("What am I looking at? ({})", t.title);
        let rect = egui::Rect::from_center_size(centre, egui::vec2(18.0, 18.0));
        let button = egui::Button::new(egui::RichText::new("?").strong()).small();
        if ui.put(rect, button).on_hover_text(hint).clicked() {
            self.open_at(topic);
        }
    }

    /// The guide window, when open.
    pub fn window(&mut self, ctx: &egui::Context, dim: egui::Color32) {
        if !self.open {
            return;
        }
        let mut open = true;
        egui::Window::new("SEISMO guide")
            .open(&mut open)
            .default_size([780.0, 580.0])
            .min_size([480.0, 300.0])
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| self.contents(ui, dim));
        self.open &= open;
    }

    fn contents(&mut self, ui: &mut egui::Ui, dim: egui::Color32) {
        let n = self.guide.topics().len();
        let mut go = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.back.is_empty(), egui::Button::new("‹ Back"))
                .clicked()
                && let Some(i) = self.back.pop()
            {
                self.current = i;
                self.to_top = true;
            }
            if ui
                .add_enabled(self.current > 0, egui::Button::new("‹ Previous"))
                .clicked()
            {
                go = Some(self.current - 1);
            }
            if ui
                .add_enabled(self.current + 1 < n, egui::Button::new("Next ›"))
                .clicked()
            {
                go = Some(self.current + 1);
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("search the guide")
                    .desired_width(f32::INFINITY),
            );
        });
        ui.separator();
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(LIST_WIDTH);
                egui::ScrollArea::vertical()
                    .id_salt("guide topics")
                    .max_height(height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(i) = self.topic_list(ui, dim) {
                            go = Some(i);
                        }
                    });
            });
            ui.separator();
            ui.vertical(|ui| {
                let mut area = egui::ScrollArea::vertical()
                    .id_salt("guide page")
                    .max_height(height)
                    .auto_shrink([false, false]);
                if std::mem::take(&mut self.to_top) {
                    area = area.vertical_scroll_offset(0.0);
                }
                area.show(ui, |ui| {
                    let topic = &self.guide.topics()[self.current];
                    if let Some(id) = page(ui, topic, dim) {
                        go = self.guide.index_of(&id);
                    }
                    ui.add_space(12.0);
                    if let Some(next) = self.guide.topics().get(self.current + 1)
                        && ui.link(format!("Next: {} ›", next.title)).clicked()
                    {
                        go = Some(self.current + 1);
                    }
                    ui.add_space(8.0);
                });
            });
        });
        if let Some(i) = go {
            self.go(i);
        }
    }

    /// The list of topics: by part, or the search's matches. Returns a clicked topic.
    fn topic_list(&self, ui: &mut egui::Ui, dim: egui::Color32) -> Option<usize> {
        let topics = self.guide.topics();
        let mut picked = None;
        let mut entry = |ui: &mut egui::Ui, i: usize| {
            if ui
                .selectable_label(i == self.current, &topics[i].title)
                .clicked()
            {
                picked = Some(i);
            }
        };
        if self.query.trim().is_empty() {
            for part in Part::ALL {
                ui.add_space(4.0);
                ui.colored_label(
                    dim,
                    egui::RichText::new(part.title().to_uppercase()).small(),
                );
                for (i, _) in topics.iter().enumerate().filter(|(_, t)| t.part == part) {
                    entry(ui, i);
                }
            }
        } else {
            let found = self.guide.search(&self.query);
            if found.is_empty() {
                ui.colored_label(dim, "Nothing found.");
            }
            for i in found {
                entry(ui, i);
            }
        }
        picked
    }

    /// The welcome card on the very first start: what SEISMO is, and where the help is.
    pub fn welcome(&mut self, ctx: &egui::Context, dim: egui::Color32) {
        if self.welcome.is_none() {
            return;
        }
        let (mut tour, mut close) = (false, false);
        egui::Window::new("Welcome to SEISMO")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(
                    "Watch an earthquake's waves cross the whole planet: spreading over the \
                     world map, diving through a slice of the Earth, and arriving at seismograph \
                     stations as wiggly lines.",
                );
                ui.add_space(6.0);
                ui.label(
                    "• Replay from origin starts the selected quake's waves from the beginning.",
                );
                ui.label("• The speed slider makes them cross the planet in seconds.");
                ui.label("• Events lists live quakes from USGS; click one to follow it.");
                ui.add_space(6.0);
                ui.colored_label(
                    dim,
                    "Every section has a ? that explains it. F1 opens the guide, which also \
                     explains seismic waves, the inside of the Earth and how quakes are measured.",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    tour = ui.button("Show me around").clicked();
                    close = ui.button("Close").clicked();
                });
            });
        if tour {
            self.open_at("first-steps");
        }
        if tour || close {
            self.dismiss_welcome();
        }
    }

    /// Put the welcome card away for good.
    fn dismiss_welcome(&mut self) {
        let Some(marker) = self.welcome.take() else {
            return;
        };
        let written = marker
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&marker, "The welcome card was shown.\n"));
        if let Err(e) = written {
            tracing::warn!(path = %marker.display(), "could not note the welcome as seen: {e}");
        }
    }
}

/// A topic's page. Returns the id of a clicked link.
fn page(ui: &mut egui::Ui, topic: &Topic, dim: egui::Color32) -> Option<String> {
    let mut follow = None;
    ui.label(egui::RichText::new(&topic.title).heading());
    ui.colored_label(dim, egui::RichText::new(topic.part.title()).small());
    ui.add_space(6.0);
    for block in &topic.blocks {
        let clicked = match block {
            Block::Heading(h) => {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(h).strong().size(15.0));
                None
            }
            Block::Para(s) => {
                let clicked = spans(ui, s);
                ui.add_space(4.0);
                clicked
            }
            Block::Bullet(s) => {
                ui.horizontal_top(|ui| {
                    ui.label("•");
                    ui.vertical(|ui| spans(ui, s)).inner
                })
                .inner
            }
        };
        follow = follow.or(clicked);
    }
    follow
}

/// Text with strong runs and links, wrapped. Returns the id of a clicked link.
fn spans(ui: &mut egui::Ui, spans: &[Span]) -> Option<String> {
    let mut follow = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for s in spans {
            match s {
                Span::Text(t) => {
                    ui.label(t);
                }
                Span::Strong(t) => {
                    ui.label(egui::RichText::new(t).strong());
                }
                Span::Link { id, text } => {
                    // Underlined: in the CRT's one green, colour alone does not mark a link.
                    if ui.link(egui::RichText::new(text).underline()).clicked() {
                        follow = Some(id.clone());
                    }
                }
            }
        }
    });
    follow
}

#[cfg(test)]
mod tests {
    use super::*;
    use session::guide::{Front, Values};

    fn guide() -> GuideUi {
        let src = "=== map | The world map | screen\nThe map.\n\n\
                   === events | Choosing an event | start\nThe list.\n";
        let text = Guide::parse(src, Front::Desktop, &Values::default()).unwrap();
        GuideUi::new(text, None, None)
    }

    /// Where the clickable picture is, and the corner `?` drawn over it.
    const PICTURE: egui::Rect =
        egui::Rect::from_min_max(egui::pos2(0.0, 60.0), egui::pos2(400.0, 300.0));
    const CORNER: egui::Pos2 = egui::pos2(380.0, 80.0);

    /// One frame: a section `?` at the top, a picture that senses clicks (as the scene does),
    /// and a corner `?` over it. Returns the section glyph's rect and whether the picture was
    /// clicked.
    fn frame(ctx: &egui::Context, g: &mut GuideUi, events: Vec<egui::Event>) -> (egui::Rect, bool) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            events,
            ..Default::default()
        };
        let (mut glyph, mut picture) = (egui::Rect::NOTHING, false);
        let mut out = ctx.run_ui(input, |ui| {
            glyph = ui.horizontal(|ui| g.glyph(ui, "events")).response.rect;
            picture = ui.allocate_rect(PICTURE, egui::Sense::click()).clicked();
            g.corner(ui, CORNER, "map");
        });
        // No renderer here: drop the font texture updates on purpose.
        out.textures_delta.clear();
        (glyph, picture)
    }

    /// Move to `at`, press and release; whether the picture saw the click.
    fn click(ctx: &egui::Context, g: &mut GuideUi, at: egui::Pos2) -> bool {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let mut picture = false;
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![button(true)],
            vec![button(false)],
            vec![],
        ] {
            picture |= frame(ctx, g, events).1;
        }
        picture
    }

    #[test]
    fn a_corner_glyph_takes_the_click_from_the_picture_under_it() {
        let ctx = egui::Context::default();
        let mut g = guide();
        frame(&ctx, &mut g, vec![]);
        // The picture itself is clickable (so a station would be placed)...
        assert!(click(&ctx, &mut g, egui::pos2(200.0, 200.0)));
        assert!(!g.open);
        // ...but not through the ?, which opens the guide at its topic.
        assert!(!click(&ctx, &mut g, CORNER));
        assert!(g.open);
        assert_eq!(g.guide.topics()[g.current].id, "map");
    }

    #[test]
    fn a_section_glyph_unfolds_and_folds_its_note() {
        let ctx = egui::Context::default();
        let mut g = guide();
        let (glyph, _) = frame(&ctx, &mut g, vec![]);
        click(&ctx, &mut g, glyph.center());
        assert!(g.unfolded.contains("events") && !g.open);
        click(&ctx, &mut g, glyph.center());
        assert!(!g.unfolded.contains("events"));
    }
}
