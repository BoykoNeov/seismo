//! The help overlay's pages beyond the key list: the guide's topics, and one topic to read. The
//! text is `session::guide`, shared with the desktop app.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span as TextSpan};
use session::guide::{Block, Guide, Part, Span};

use crate::palette::{Ink, Palette};

/// Which page of the help overlay is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Keys,
    Topics,
    /// Reading a topic (an index into the guide's topics).
    Topic(usize),
}

/// What a key did to the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    Close,
}

pub struct GuideView {
    pub guide: Guide,
    pub page: Page,
    /// The highlighted topic in the list.
    pub cursor: usize,
    /// First line shown of the page being read.
    pub scroll: usize,
    /// Rows the last frame had for text, for paging.
    pub rows: usize,
}

impl GuideView {
    pub fn new(guide: Guide) -> Self {
        Self {
            guide,
            page: Page::Keys,
            cursor: 0,
            scroll: 0,
            rows: 10,
        }
    }

    /// Open on topic `id`, or on the list if there is no such topic.
    pub fn open_at(&mut self, id: &str) {
        match self.guide.index_of(id) {
            Some(i) => self.read(i),
            None => self.page = Page::Topics,
        }
    }

    fn read(&mut self, i: usize) {
        self.cursor = i;
        self.scroll = 0;
        self.page = Page::Topic(i);
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        let n = self.guide.topics().len();
        let page_rows = self.rows.saturating_sub(1).max(1);
        match self.page {
            // Any key closes the key list, except the ones that go to the guide.
            Page::Keys => match k.code {
                KeyCode::Tab | KeyCode::Right | KeyCode::Char('g') => self.page = Page::Topics,
                _ => return Outcome::Close,
            },
            Page::Topics => match k.code {
                KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => self.cursor = (self.cursor + 1).min(n - 1),
                KeyCode::PageUp => self.cursor = self.cursor.saturating_sub(page_rows),
                KeyCode::PageDown => self.cursor = (self.cursor + page_rows).min(n - 1),
                KeyCode::Home => self.cursor = 0,
                KeyCode::End => self.cursor = n - 1,
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.read(self.cursor),
                KeyCode::Tab | KeyCode::Left => self.page = Page::Keys,
                KeyCode::Esc | KeyCode::Char('?') => return Outcome::Close,
                _ => {}
            },
            Page::Topic(i) => match k.code {
                KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => self.scroll += 1,
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(page_rows),
                KeyCode::PageDown | KeyCode::Char(' ') => self.scroll += page_rows,
                KeyCode::Home => self.scroll = 0,
                KeyCode::Char('n') if i + 1 < n => self.read(i + 1),
                KeyCode::Char('p') if i > 0 => self.read(i - 1),
                KeyCode::Left | KeyCode::Backspace | KeyCode::Esc | KeyCode::Char('h') => {
                    self.page = Page::Topics;
                }
                KeyCode::Tab => self.page = Page::Keys,
                KeyCode::Char('?') => return Outcome::Close,
                _ => {}
            },
        }
        Outcome::Stay
    }

    /// The topic list, one line per topic under its part's heading, and the line of the cursor.
    pub fn list_lines(&self, pal: &Palette) -> (Vec<Line<'static>>, usize) {
        let mut lines = Vec::new();
        let mut at = 0;
        let topics = self.guide.topics();
        for part in Part::ALL {
            if !lines.is_empty() {
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(
                format!(" {}", part.title().to_uppercase()),
                style(pal, Ink::Dim),
            ));
            for (i, t) in topics.iter().enumerate().filter(|(_, t)| t.part == part) {
                let here = i == self.cursor;
                if here {
                    at = lines.len();
                }
                let (mark, ink) = if here {
                    ("▶ ", Ink::Accent)
                } else {
                    ("  ", Ink::Text)
                };
                let mut s = style(pal, ink);
                if here {
                    s = s.add_modifier(Modifier::BOLD);
                }
                lines.push(Line::styled(format!(" {mark}{}", t.title), s));
            }
        }
        (lines, at)
    }

    /// Topic `i` wrapped to `width` columns.
    pub fn topic_lines(&self, i: usize, width: usize, pal: &Palette) -> Vec<Line<'static>> {
        let t = &self.guide.topics()[i];
        let mut lines = vec![
            Line::styled(
                format!(" {}", t.title),
                style(pal, Ink::Accent).add_modifier(Modifier::BOLD),
            ),
            Line::styled(format!(" {}", t.part.title()), style(pal, Ink::Dim)),
        ];
        for b in &t.blocks {
            match b {
                Block::Heading(h) => {
                    lines.push(Line::raw(""));
                    lines.push(Line::styled(
                        format!(" {h}"),
                        style(pal, Ink::Accent).add_modifier(Modifier::BOLD),
                    ));
                }
                Block::Para(spans) => {
                    lines.push(Line::raw(""));
                    lines.extend(wrap(spans, width, " ", " ", pal));
                }
                Block::Bullet(spans) => lines.extend(wrap(spans, width, " • ", "   ", pal)),
            }
        }
        lines
    }
}

fn style(pal: &Palette, ink: Ink) -> Style {
    Style::default().fg(pal.colour(ink))
}

/// Word-wrap styled spans to `width` columns, the first line after `first`, the rest after
/// `rest`.
fn wrap(
    spans: &[Span],
    width: usize,
    first: &str,
    rest: &str,
    pal: &Palette,
) -> Vec<Line<'static>> {
    let plain = style(pal, Ink::Text);
    // Words, each with its style and whether a space came before it.
    // Words between spaces, each made of styled pieces (a link and the full stop after it are
    // one word, so they wrap together).
    let mut words: Vec<Vec<(String, Style)>> = Vec::new();
    // A space came before the next piece (spaces carry across spans).
    let mut space = true;
    for s in spans {
        let st = match s {
            Span::Text(_) => plain,
            Span::Strong(_) => plain.add_modifier(Modifier::BOLD),
            Span::Link { .. } => style(pal, Ink::Accent),
        };
        for (k, w) in s.text().split(' ').enumerate() {
            space |= k > 0;
            if w.is_empty() {
                continue;
            }
            match words.last_mut() {
                Some(word) if !space => word.push((w.to_owned(), st)),
                _ => words.push(vec![(w.to_owned(), st)]),
            }
            space = false;
        }
    }
    let mut lines = Vec::new();
    let mut line: Vec<TextSpan<'static>> = vec![TextSpan::raw(first.to_owned())];
    let mut used = first.chars().count();
    let mut empty = true;
    for word in words {
        let len: usize = word.iter().map(|(w, _)| w.chars().count()).sum();
        if !empty && used + 1 + len > width {
            lines.push(Line::from(std::mem::take(&mut line)));
            line.push(TextSpan::raw(rest.to_owned()));
            used = rest.chars().count();
            empty = true;
        }
        if !empty {
            line.push(TextSpan::styled(" ", plain));
            used += 1;
        }
        used += len;
        line.extend(word.into_iter().map(|(w, st)| TextSpan::styled(w, st)));
        empty = false;
    }
    lines.push(Line::from(line));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;
    use session::guide::{Front, Values};

    fn view() -> GuideView {
        let src = "=== a | Alpha | start\nOne two three four five six seven eight.\n\n\
                   - a **bold** point\n\n=== b | Beta | learn\nB.\n";
        GuideView::new(Guide::parse(src, Front::Terminal, &Values::default()).unwrap())
    }

    fn press(v: &mut GuideView, code: KeyCode) -> Outcome {
        v.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn keys_move_between_pages() {
        let mut v = view();
        assert_eq!(press(&mut v, KeyCode::Char('x')), Outcome::Close);
        assert_eq!(press(&mut v, KeyCode::Tab), Outcome::Stay);
        assert_eq!(v.page, Page::Topics);
        press(&mut v, KeyCode::Down);
        press(&mut v, KeyCode::Down);
        assert_eq!(v.cursor, 1);
        press(&mut v, KeyCode::Enter);
        assert_eq!(v.page, Page::Topic(1));
        press(&mut v, KeyCode::Char('p'));
        assert_eq!(v.page, Page::Topic(0));
        press(&mut v, KeyCode::Esc);
        assert_eq!(v.page, Page::Topics);
        assert_eq!(press(&mut v, KeyCode::Esc), Outcome::Close);
    }

    #[test]
    fn text_wraps_at_words_with_a_hanging_indent() {
        let v = view();
        let pal = Palette::new(crate::palette::Look::Crt);
        let lines = v.topic_lines(0, 20, &pal);
        let body: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(
            body[2..],
            [
                "",
                " One two three four",
                " five six seven",
                " eight.",
                " • a bold point",
            ]
        );
        assert!(lines.iter().all(|l| text(l).chars().count() <= 20));
    }

    #[test]
    fn the_shared_guide_fits_the_terminal() {
        let (table, model, dispersion) = session::assets::earth().unwrap();
        let mut facts = sim::facts::ModelFacts::new(&table, &model, &dispersion);
        facts.add_crust(&session::assets::crust(&model).unwrap());
        let mut values = Values::new(&facts);
        values.set("stations", "73");
        let v = GuideView::new(Guide::new(Front::Terminal, &values).unwrap());
        let pal = Palette::new(crate::palette::Look::Modern);
        for i in 0..v.guide.topics().len() {
            for l in v.topic_lines(i, 56, &pal) {
                // A single word longer than the line may overflow; none is.
                assert!(text(&l).chars().count() <= 56, "{}", text(&l));
            }
        }
    }
}
