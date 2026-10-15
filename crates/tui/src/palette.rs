//! The two looks in a terminal: green phosphor on near-black, and the modern dark slate with
//! one colour per kind of feature. Same colours as the desktop app's themes. Every ink is a
//! colour and a level, painted as `mix(background, colour, level)`, so the CRT look tells
//! features apart by brightness alone.

use ratatui::style::Color;

/// What a mark on the canvas is. Later variants win when two share a terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ink {
    /// Region fills in the slice: mantle, outer core, inner core.
    Mantle,
    OuterCore,
    InnerCore,
    Boundary,
    Coast,
    /// The slice's great circle on the map.
    Circle,
    RayPath,
    /// Axes, the Earth's surface in the slice, the traces' time axis.
    Chrome,
    /// A bundled station not shown as a trace.
    Station,
    Trace,
    Love,
    Rayleigh,
    S,
    P,
    /// Arrival marks on the traces.
    Tick,
    /// An event on the map other than the selected one.
    OtherEvent,
    /// A station whose trace is shown.
    StationShown,
    Epicentre,
    /// Plain text: labels, the event list.
    Dim,
    Text,
    /// The selected row, the playing clock.
    Accent,
}

/// Which look is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Look {
    #[default]
    Crt,
    Modern,
}

impl Look {
    pub fn other(self) -> Self {
        match self {
            Look::Crt => Look::Modern,
            Look::Modern => Look::Crt,
        }
    }
}

type Rgb = [f32; 3];

const fn hex(rgb: u32) -> Rgb {
    [
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
    ]
}

const PHOSPHOR: Rgb = hex(0x33ff33);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub look: Look,
    background: Rgb,
}

impl Palette {
    pub fn new(look: Look) -> Self {
        let background = match look {
            Look::Crt => hex(0x050805),
            Look::Modern => hex(0x0b0f17),
        };
        Self { look, background }
    }

    /// Colour and level of an ink.
    fn ink(&self, ink: Ink) -> (Rgb, f32) {
        match self.look {
            Look::Crt => {
                let level = match ink {
                    // Fills are faint tints; the liquid outer core brightest.
                    Ink::Mantle => 0.05,
                    Ink::OuterCore => 0.14,
                    Ink::InnerCore => 0.09,
                    Ink::Boundary => 0.4,
                    Ink::Coast => 0.42,
                    Ink::Circle => 0.6,
                    Ink::RayPath => 0.3,
                    Ink::Chrome => 0.55,
                    Ink::Station => 0.5,
                    Ink::Trace => 0.9,
                    Ink::Love => 0.75,
                    Ink::Rayleigh => 1.0,
                    Ink::S => 0.8,
                    Ink::P => 1.0,
                    Ink::Tick => 0.7,
                    Ink::OtherEvent => 0.75,
                    Ink::StationShown => 1.0,
                    Ink::Epicentre => 1.0,
                    Ink::Dim => 0.55,
                    Ink::Text => 0.9,
                    Ink::Accent => 1.0,
                };
                (PHOSPHOR, level)
            }
            Look::Modern => match ink {
                Ink::Mantle => (hex(0x1d2633), 1.0),
                Ink::OuterCore => (hex(0x4a3418), 1.0),
                Ink::InnerCore => (hex(0x6e5834), 1.0),
                Ink::Boundary => (hex(0x8fa3bf), 0.7),
                Ink::Coast => (hex(0xd5dee9), 0.45),
                Ink::Circle => (hex(0xffffff), 0.6),
                Ink::RayPath => (hex(0xa8b8cc), 0.45),
                Ink::Chrome => (hex(0x93a1b5), 1.0),
                Ink::Station => (hex(0xaeb9c9), 0.7),
                Ink::Trace => (hex(0xe3eaf3), 1.0),
                Ink::Love => (hex(0xff7fb0), 1.0),
                Ink::Rayleigh => (hex(0xb48cff), 1.0),
                Ink::S => (hex(0x45c8ff), 1.0),
                Ink::P => (hex(0xffb547), 1.0),
                Ink::Tick => (hex(0xffb547), 1.0),
                Ink::OtherEvent => (hex(0xff5a5a), 0.75),
                Ink::StationShown => (hex(0xffffff), 1.0),
                Ink::Epicentre => (hex(0xff5a5a), 1.0),
                Ink::Dim => (hex(0x93a1b5), 1.0),
                Ink::Text => (hex(0xe3eaf3), 1.0),
                Ink::Accent => (hex(0x5ab0ff), 1.0),
            },
        }
    }

    pub fn background_rgb(&self) -> Rgb {
        self.background
    }

    /// `ink` painted at `level` (times the ink's own) over the background.
    pub fn mix(&self, ink: Ink, level: f32) -> Rgb {
        let (c, own) = self.ink(ink);
        let k = (own * level).clamp(0.0, 1.0);
        [0, 1, 2].map(|i| self.background[i] + (c[i] - self.background[i]) * k)
    }

    pub fn colour(&self, ink: Ink) -> Color {
        to_colour(self.mix(ink, 1.0))
    }

    pub fn background(&self) -> Color {
        to_colour(self.background)
    }
}

pub fn to_colour(c: Rgb) -> Color {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::Rgb(b(c[0]), b(c[1]), b(c[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crt_is_one_hue() {
        let p = Palette::new(Look::Crt);
        for ink in [Ink::P, Ink::S, Ink::Coast, Ink::Epicentre, Ink::Text] {
            let [r, g, b] = p.mix(ink, 1.0);
            assert!(g > r && (r - b).abs() < 1e-6, "{ink:?}");
        }
        // Brightness tells P from S.
        assert!(p.mix(Ink::P, 1.0)[1] > p.mix(Ink::S, 1.0)[1]);
    }

    #[test]
    fn level_zero_is_the_background() {
        for look in [Look::Crt, Look::Modern] {
            let p = Palette::new(look);
            assert_eq!(p.mix(Ink::P, 0.0), p.background_rgb());
        }
    }
}
