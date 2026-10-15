//! A braille canvas: each terminal cell holds 2 × 4 dots, so lines and rings are drawn at
//! twice the column and four times the row resolution. Each cell also has a background tint
//! (for glows and fills) and may hold a text character instead of dots.
//!
//! A cell's dots take the colour of the strongest ink among them ([`Ink`] order). Tints keep
//! the per-channel maximum, as the desktop app's passes do, so a dim glow never darkens a
//! bright one.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::palette::{Ink, Palette, to_colour};

/// Bit of dot (`x`, `y`) in a braille cell, `x` in 0..2, `y` in 0..4 (Unicode braille order).
const BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

#[derive(Debug, Clone, Copy, PartialEq)]
struct Cell {
    bits: u8,
    ink: Option<Ink>,
    bg: [f32; 3],
    text: Option<(char, Ink)>,
}

#[derive(Debug, Clone)]
pub struct Canvas {
    /// Size in cells.
    pub width: usize,
    pub height: usize,
    palette: Palette,
    cells: Vec<Cell>,
}

impl Canvas {
    pub fn new(width: usize, height: usize, palette: Palette) -> Self {
        let blank = Cell {
            bits: 0,
            ink: None,
            bg: palette.background_rgb(),
            text: None,
        };
        Self {
            width,
            height,
            palette,
            cells: vec![blank; width * height],
        }
    }

    /// Size in dots.
    pub fn dots(&self) -> (usize, usize) {
        (2 * self.width, 4 * self.height)
    }

    fn cell_mut(&mut self, cx: usize, cy: usize) -> Option<&mut Cell> {
        if cx < self.width && cy < self.height {
            Some(&mut self.cells[cy * self.width + cx])
        } else {
            None
        }
    }

    /// Light dot (`x`, `y`) in `ink`; outside the canvas does nothing.
    pub fn dot(&mut self, x: i64, y: i64, ink: Ink) {
        if x < 0 || y < 0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        if let Some(c) = self.cell_mut(x / 2, y / 4) {
            c.bits |= BITS[x % 2][y % 4];
            c.ink = Some(c.ink.map_or(ink, |i| i.max(ink)));
        }
    }

    /// Whether dot (`x`, `y`) is lit.
    pub fn is_lit(&self, x: usize, y: usize) -> bool {
        x / 2 < self.width
            && y / 4 < self.height
            && self.cells[(y / 4) * self.width + x / 2].bits & BITS[x % 2][y % 4] != 0
    }

    /// The ink of the cell holding dot (`x`, `y`), if any dot or text is there.
    #[cfg(test)]
    pub fn ink_at_dot(&self, x: usize, y: usize) -> Option<Ink> {
        let c = self.cells.get((y / 4) * self.width + x / 2)?;
        c.text.map(|t| t.1).or(c.ink)
    }

    /// A straight line of dots between two points given in dot coordinates.
    pub fn line(&mut self, (x0, y0): (f64, f64), (x1, y1): (f64, f64), ink: Ink) {
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0);
        if !steps.is_finite() || steps > 100_000.0 {
            return;
        }
        let n = steps as i64;
        for k in 0..=n {
            let f = k as f64 / steps;
            let x = x0 + (x1 - x0) * f;
            let y = y0 + (y1 - y0) * f;
            self.dot(x.floor() as i64, y.floor() as i64, ink);
        }
    }

    /// A polyline through dot-coordinate points.
    pub fn polyline(&mut self, points: &[(f64, f64)], ink: Ink) {
        for w in points.windows(2) {
            self.line(w[0], w[1], ink);
        }
        if let [p] = points {
            self.dot(p.0.floor() as i64, p.1.floor() as i64, ink);
        }
    }

    /// Tint cell (`cx`, `cy`)'s background with `ink` at `level` (0..1).
    pub fn tint(&mut self, cx: usize, cy: usize, ink: Ink, level: f32) {
        if level <= 0.0 {
            return;
        }
        let c = self.palette.mix(ink, level);
        if let Some(cell) = self.cell_mut(cx, cy) {
            for (b, v) in cell.bg.iter_mut().zip(c) {
                *b = b.max(v);
            }
        }
    }

    /// Write `text` from cell (`cx`, `cy`) rightwards, over any dots, clipped at the edge.
    pub fn text(&mut self, cx: usize, cy: usize, text: &str, ink: Ink) {
        for (k, ch) in text.chars().enumerate() {
            if let Some(cell) = self.cell_mut(cx + k, cy) {
                cell.text = Some((ch, ink));
            }
        }
    }

    /// Whether cells `cx..cx + len` of row `cy` are free of text and dots.
    pub fn is_clear(&self, cx: usize, cy: usize, len: usize) -> bool {
        (cx..cx + len).all(|x| {
            x < self.width
                && cy < self.height
                && self.cells[cy * self.width + x].text.is_none()
                && self.cells[cy * self.width + x].bits == 0
        })
    }

    /// Whether cells `cx..cx + len` of row `cy` hold no text.
    pub fn no_text(&self, cx: usize, cy: usize, len: usize) -> bool {
        (cx..cx + len).all(|x| {
            x < self.width && cy < self.height && self.cells[cy * self.width + x].text.is_none()
        })
    }

    /// The character cell (`cx`, `cy`) shows.
    pub fn symbol(&self, cx: usize, cy: usize) -> char {
        let c = &self.cells[cy * self.width + cx];
        match (c.text, c.bits) {
            (Some((ch, _)), _) => ch,
            (None, 0) => ' ',
            (None, bits) => char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' '),
        }
    }

    /// Paint into `buf` with the top-left cell at (`area.x`, `area.y`), clipped to `area`.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        for cy in 0..self.height.min(area.height as usize) {
            for cx in 0..self.width.min(area.width as usize) {
                let c = &self.cells[cy * self.width + cx];
                let ink = c.text.map(|t| t.1).or(c.ink).unwrap_or(Ink::Dim);
                let style = Style::default()
                    .fg(self.palette.colour(ink))
                    .bg(to_colour(c.bg));
                let pos = (area.x + cx as u16, area.y + cy as u16);
                if let Some(cell) = buf.cell_mut(pos) {
                    let mut tmp = [0u8; 4];
                    cell.set_symbol(self.symbol(cx, cy).encode_utf8(&mut tmp));
                    cell.set_style(style);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::Look;

    fn canvas() -> Canvas {
        Canvas::new(3, 2, Palette::new(Look::Crt))
    }

    #[test]
    fn braille_dot_order() {
        let mut c = canvas();
        c.dot(0, 0, Ink::P);
        assert_eq!(c.symbol(0, 0), '⠁');
        let mut c = canvas();
        c.dot(1, 3, Ink::P);
        assert_eq!(c.symbol(0, 0), '⢀');
        let mut c = canvas();
        for x in 0..2 {
            for y in 0..4 {
                c.dot(x, y, Ink::P);
            }
        }
        assert_eq!(c.symbol(0, 0), '⣿');
        assert_eq!(c.symbol(1, 0), ' ');
        // Second cell row, second column.
        c.dot(2, 4, Ink::S);
        assert_eq!(c.symbol(1, 1), '⠁');
        assert!(c.is_lit(2, 4) && !c.is_lit(3, 4));
    }

    #[test]
    fn strongest_ink_wins_and_outside_is_ignored() {
        let mut c = canvas();
        c.dot(0, 0, Ink::P);
        c.dot(1, 1, Ink::Coast);
        assert_eq!(c.ink_at_dot(0, 0), Some(Ink::P));
        c.dot(-1, 0, Ink::P);
        c.dot(6, 0, Ink::P);
        c.dot(0, 8, Ink::P);
        assert_eq!(c.symbol(2, 1), ' ');
    }

    #[test]
    fn lines_are_unbroken() {
        let mut c = Canvas::new(10, 3, Palette::new(Look::Crt));
        c.line((0.5, 0.5), (19.5, 11.5), Ink::P);
        for x in 0..20 {
            assert!((0..12).any(|y| c.is_lit(x, y)), "column {x} empty");
        }
    }

    #[test]
    fn text_overrides_dots_and_tints_keep_the_brighter() {
        let mut c = canvas();
        c.dot(0, 0, Ink::P);
        c.text(0, 0, "AB", Ink::Text);
        assert_eq!(c.symbol(0, 0), 'A');
        assert_eq!(c.symbol(1, 0), 'B');
        assert!(!c.no_text(0, 0, 1) && c.no_text(2, 0, 1));
        c.tint(0, 1, Ink::P, 0.8);
        let bright = c.cells[3].bg;
        c.tint(0, 1, Ink::P, 0.2);
        assert_eq!(c.cells[3].bg, bright);
    }
}
