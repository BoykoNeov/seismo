//! The two looks' UI colours and egui themes.

use eframe::egui;

// The CRT look's UI colours.
const GREEN: egui::Color32 = egui::Color32::from_rgb(0x33, 0xff, 0x33);
const GREEN_DIM: egui::Color32 = egui::Color32::from_rgb(0x1f, 0x99, 0x1f);
const GREEN_DARK: egui::Color32 = egui::Color32::from_rgb(0x0e, 0x40, 0x0e);
const BACKGROUND: egui::Color32 = egui::Color32::from_rgb(0x05, 0x08, 0x05);

// The modern look's UI colours: slate panels, light text, a blue accent.
const SLATE_BG: egui::Color32 = egui::Color32::from_rgb(0x0b, 0x0f, 0x17);
const SLATE_PANEL: egui::Color32 = egui::Color32::from_rgb(0x12, 0x17, 0x21);
const SLATE_WIDGET: egui::Color32 = egui::Color32::from_rgb(0x1c, 0x24, 0x31);
const SLATE_HOVER: egui::Color32 = egui::Color32::from_rgb(0x26, 0x30, 0x41);
const SLATE_ACTIVE: egui::Color32 = egui::Color32::from_rgb(0x2f, 0x3c, 0x52);
const SLATE_LINE: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x34, 0x44);
const TEXT: egui::Color32 = egui::Color32::from_rgb(0xe3, 0xea, 0xf3);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0x93, 0xa1, 0xb5);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x5a, 0xb0, 0xff);
const ACCENT_DIM: egui::Color32 = egui::Color32::from_rgb(0x24, 0x4a, 0x78);

/// The colours of the text the app writes over the scene and in its notes, per look.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UiPalette {
    pub(crate) text: egui::Color32,
    pub(crate) dim: egui::Color32,
    pub(crate) background: egui::Color32,
}

impl UiPalette {
    pub(crate) fn for_look(look: render::Look) -> Self {
        match look {
            render::Look::Crt => Self {
                text: GREEN,
                dim: GREEN_DIM,
                background: BACKGROUND,
            },
            render::Look::Modern => Self {
                text: TEXT,
                dim: TEXT_DIM,
                background: SLATE_BG,
            },
        }
    }
}

/// The egui theme of a look.
pub(crate) fn visuals_for(look: render::Look) -> egui::Visuals {
    match look {
        render::Look::Crt => phosphor_visuals(),
        render::Look::Modern => modern_visuals(),
    }
}

/// Slate panels, light text, blue accents, rounded widgets.
fn modern_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = SLATE_PANEL;
    v.window_fill = SLATE_PANEL;
    v.extreme_bg_color = SLATE_BG;
    v.faint_bg_color = SLATE_WIDGET;
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    v.slider_trailing_fill = true;
    let radius = egui::CornerRadius::same(5);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius;
        w.bg_fill = SLATE_WIDGET;
        w.weak_bg_fill = SLATE_WIDGET;
        w.bg_stroke = egui::Stroke::NONE;
        w.fg_stroke = egui::Stroke::new(1.0, TEXT);
    }
    v.widgets.noninteractive.bg_fill = SLATE_PANEL;
    v.widgets.noninteractive.weak_bg_fill = SLATE_PANEL;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, SLATE_LINE);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT_DIM);
    v.widgets.hovered.bg_fill = SLATE_HOVER;
    v.widgets.hovered.weak_bg_fill = SLATE_HOVER;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT_DIM);
    v.widgets.active.bg_fill = SLATE_ACTIVE;
    v.widgets.active.weak_bg_fill = SLATE_ACTIVE;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);
    v.widgets.open.bg_fill = SLATE_ACTIVE;
    v
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
