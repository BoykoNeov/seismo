//! The two looks: the retro green-phosphor CRT, and a modern dark look where each kind of
//! feature has its own colour. A [`Theme`] is only colours (plus a few level weights the CRT
//! look needs to stay exactly as it was); which post-process effects run is the app's choice.
//!
//! Every pass paints a feature as `mix(background, colour, level) * max(1, level)` and keeps
//! the per-channel maximum over features ("lighten"), so a dim feature never darkens a bright
//! one. With every colour the same green, the maximum over features equals the green at the
//! largest level: the CRT look is pixel-identical to the single-hue rendering it replaces.

/// A display-space (gamma-encoded) RGB colour.
pub type Rgb = [f32; 3];

/// `#rrggbb` to display-space RGB.
const fn hex(rgb: u32) -> Rgb {
    [
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
    ]
}

/// Which look is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Look {
    /// Green phosphor, scanlines, curved glass, afterglow.
    #[default]
    Crt,
    /// Dark slate, one colour per kind of feature, clean lines, a soft glow.
    Modern,
}

impl Look {
    pub const ALL: [Look; 2] = [Look::Crt, Look::Modern];

    pub fn name(self) -> &'static str {
        match self {
            Look::Crt => "CRT",
            Look::Modern => "Modern",
        }
    }

    /// Index into per-look arrays.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Colours of every feature the scene draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub background: Rgb,
    // Map.
    /// Graticule.
    pub grid: Rgb,
    pub coast: Rgb,
    /// P wavefronts, on the map and in the slice.
    pub p_front: Rgb,
    /// S wavefronts, on the map and in the slice.
    pub s_front: Rgb,
    /// Rayleigh-wave bands.
    pub rayleigh: Rgb,
    /// Love-wave bands.
    pub love: Rgb,
    /// Epicentre on the map; the source in the slice.
    pub epicentre: Rgb,
    /// The slice's great circle on the map.
    pub great_circle: Rgb,
    /// Station markers, and those whose trace is shown.
    pub marker: Rgb,
    pub marker_selected: Rgb,
    // Slice.
    /// Region fills: mantle, outer core, inner core.
    pub fills: [Rgb; 3],
    /// Level weight of each fill, times the style's fill brightness.
    pub fill_weights: [f32; 3],
    /// The crust strip's fills, crust and sea, and their weights as in `fill_weights` (its
    /// mantle is the mantle fill).
    pub fill_crust: Rgb,
    pub fill_water: Rgb,
    pub strip_weights: [f32; 2],
    /// Layer boundaries.
    pub boundary: Rgb,
    /// The Earth's surface, distance ticks, the flat edge; the traces' axes, grids, baselines,
    /// rule and pen.
    pub chrome: Rgb,
    pub ray_path: Rgb,
    /// Fast and slow rock in the mantle shading.
    pub rock_fast: Rgb,
    pub rock_slow: Rgb,
    /// Rock shading levels, with `x` the clamped relative speed in [-1, 1] (±1%): fast rock is
    /// painted at `base + fast * x` (where positive), slow rock at `slow * -x` (where
    /// positive), both times the style's rock gain.
    pub rock_levels: [f32; 3],
    // Traces.
    pub trace: Rgb,
    /// Arrival ticks on the traces.
    pub tick: Rgb,
}

impl Theme {
    pub fn for_look(look: Look) -> Self {
        match look {
            Look::Crt => Self::crt(),
            Look::Modern => Self::modern(),
        }
    }

    /// Phosphor green `#33ff33` on `#050805`: everything one hue, told apart by brightness and
    /// line style.
    pub fn crt() -> Self {
        let g = crate::PHOSPHOR;
        Self {
            background: crate::BACKGROUND,
            grid: g,
            coast: g,
            p_front: g,
            s_front: g,
            rayleigh: g,
            love: g,
            epicentre: g,
            great_circle: g,
            marker: g,
            marker_selected: g,
            fills: [g; 3],
            // Faint fills: the liquid outer core brightest.
            fill_weights: [0.35, 1.0, 0.6],
            // The strip's crust stands out from the faint mantle, the sea a little.
            fill_crust: g,
            fill_water: g,
            strip_weights: [3.0, 1.2],
            boundary: g,
            chrome: g,
            ray_path: g,
            rock_fast: g,
            // Slow rock only darkens towards the background here.
            rock_slow: crate::BACKGROUND,
            rock_levels: [0.09, 0.09, 0.0],
            trace: g,
            tick: g,
        }
    }

    /// Dark slate; amber P, cyan S, violet Rayleigh, pink Love, coral epicentre, blue fast and
    /// red slow rock (the tomography convention), light grey chrome.
    pub fn modern() -> Self {
        Self {
            background: hex(0x0b_0f_17),
            grid: hex(0x7d_8d_a3),
            coast: hex(0xd5_de_e9),
            p_front: hex(0xff_b5_47),
            s_front: hex(0x45_c8_ff),
            rayleigh: hex(0xb4_8c_ff),
            love: hex(0xff_7f_b0),
            epicentre: hex(0xff_5a_5a),
            great_circle: hex(0xff_ff_ff),
            marker: hex(0xae_b9_c9),
            marker_selected: hex(0xff_ff_ff),
            fills: [hex(0x1d_26_33), hex(0x4a_34_18), hex(0x6e_58_34)],
            fill_weights: [1.0, 1.0, 1.0],
            fill_crust: hex(0x5a_4f_42),
            fill_water: hex(0x14_2f_52),
            strip_weights: [1.0, 1.0],
            boundary: hex(0x8f_a3_bf),
            chrome: hex(0x93_a1_b5),
            ray_path: hex(0xa8_b8_cc),
            rock_fast: hex(0x4f_8a_e6),
            rock_slow: hex(0xe0_5a_4e),
            rock_levels: [0.0, 0.3, 0.3],
            trace: hex(0xe3_ea_f3),
            tick: hex(0xff_b5_47),
        }
    }
}

/// An RGB colour padded to a `vec4` uniform.
pub(crate) fn vec4(c: Rgb) -> [f32; 4] {
    [c[0], c[1], c[2], 0.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_decodes_display_space() {
        assert_eq!(hex(0x33ff33), crate::PHOSPHOR);
        assert_eq!(hex(0x050805), crate::BACKGROUND);
    }

    #[test]
    fn crt_theme_is_one_hue() {
        let t = Theme::crt();
        let g = crate::PHOSPHOR;
        for c in [
            t.grid,
            t.coast,
            t.p_front,
            t.s_front,
            t.rayleigh,
            t.love,
            t.epicentre,
            t.great_circle,
            t.marker,
            t.marker_selected,
            t.boundary,
            t.chrome,
            t.ray_path,
            t.rock_fast,
            t.trace,
            t.tick,
        ] {
            assert_eq!(c, g);
        }
        assert!(t.fills.iter().all(|&f| f == g));
        assert_eq!((t.fill_crust, t.fill_water), (g, g));
        // Slow rock paints nothing: its level weight is zero.
        assert_eq!(t.rock_levels[2], 0.0);
    }

    #[test]
    fn modern_theme_tells_waves_apart() {
        let t = Theme::modern();
        let distinct = [t.p_front, t.s_front, t.rayleigh, t.love, t.epicentre];
        for (i, a) in distinct.iter().enumerate() {
            for b in &distinct[i + 1..] {
                let d: f32 = a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum();
                assert!(d > 0.3, "{a:?} vs {b:?}");
            }
        }
        // Every colour is brighter than the background, so painting it shows.
        let lum = |c: Rgb| c[0] + c[1] + c[2];
        assert!(lum(t.grid) > lum(t.background) && lum(t.fills[0]) > lum(t.background));
    }

    #[test]
    fn looks_index_their_arrays() {
        assert_eq!(Look::Crt.index(), 0);
        assert_eq!(Look::Modern.index(), 1);
        assert_eq!(Look::ALL.len(), 2);
    }
}
