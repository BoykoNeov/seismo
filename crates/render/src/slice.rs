//! Slice pass: the half-disc cross-section along a great circle through the epicentre, with
//! the 3D mantle's fast and slow rock as shading, and wavefronts and ray paths from `sim`'s ray
//! fans.
//!
//! The half shown runs from the epicentre, at the left end of the flat edge, along the slice's
//! azimuth to the antipode at the right end; epicentral distance runs over the arc.

use bytemuck::{Pod, Zeroable};
use sim::mantle::{
    CUT_DELTA_COUNT, CUT_DELTA_STEP_DEG, CUT_R_BOTTOM_KM, CUT_R_COUNT, CUT_R_TOP_KM, CutGrid,
};
use sim::slice::{Boundary, SliceLine};
use sim::{EARTH_RADIUS_KM, RayPhase, Wave};

use crate::gpu;
use crate::lines::{DASH_DASHED, DASH_DOTTED, DASH_SOLID, LineBatch, LineVertex, push_polyline};
use crate::theme::{Look, Theme, vec4};

/// Most boundaries the disc shader draws; matches `rings_a` + `rings_b` in `slice_disc.wgsl`.
pub const MAX_SLICE_BOUNDARIES: usize = 8;

/// Where the half-disc sits in a viewport of a given size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceView {
    /// Viewport size in pixels.
    pub size: (f32, f32),
    /// Centre of the Earth, viewport pixels (y down).
    pub centre: [f32; 2],
    /// Earth radius in pixels.
    pub radius_px: f32,
}

impl SliceView {
    pub fn new(size: (f32, f32)) -> Self {
        let (w, h) = size;
        // Room around the arc for distance labels, below the flat edge for end labels.
        let side = 0.07 * h;
        let top = 0.09 * h;
        let bottom = 0.09 * h;
        let radius_px = ((w - 2.0 * side) / 2.0).min(h - top - bottom).max(1.0);
        Self {
            size,
            centre: [0.5 * w, top + radius_px],
            radius_px,
        }
    }

    /// Viewport pixel position (y down) of the point at radius `r_km` and epicentral distance
    /// `delta_deg`.
    pub fn project(&self, r_km: f64, delta_deg: f64) -> [f32; 2] {
        let phi = (180.0 - delta_deg).to_radians();
        let s = (r_km / EARTH_RADIUS_KM) as f32 * self.radius_px;
        [
            self.centre[0] + s * phi.cos() as f32,
            self.centre[1] - s * phi.sin() as f32,
        ]
    }
}

/// How a layer of lines is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeKind {
    /// Wavefronts: P legs solid, S legs dashed.
    Front,
    /// Ray paths: dotted.
    Path,
}

/// A set of lines drawn with one brightness and stroke.
#[derive(Debug, Clone, Copy)]
pub struct SliceLayer<'a> {
    pub lines: &'a [SliceLine],
    pub kind: StrokeKind,
    /// Brightness multiplier, [0, 1+].
    pub intensity: f32,
}

/// Everything the slice pass draws in one frame.
#[derive(Debug, Clone, Copy)]
pub struct SliceScene<'a> {
    pub source_depth_km: f64,
    pub boundaries: &'a [Boundary],
    pub layers: &'a [SliceLayer<'a>],
    /// Shade the mantle with the grid last given to [`SlicePass::upload_cut`].
    pub rock_shading: bool,
}

/// Look of the slice pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceStyle {
    /// Width of a wavefront line in pixels.
    pub front_width_px: f32,
    /// Width of a ray path in pixels.
    pub path_width_px: f32,
    /// Brightness of the layer boundaries, [0, 1].
    pub boundary_brightness: f32,
    /// Brightness of the faint region fills, [0, 1].
    pub fill_brightness: f32,
    /// Display gain of the rock shading: fast rock brighter, slow rock darker. A look
    /// setting, independent of the 3D strength.
    pub rock_gain: f32,
}

impl Default for SliceStyle {
    /// The CRT look's values.
    fn default() -> Self {
        Self::for_look(Look::Crt)
    }
}

impl SliceStyle {
    /// Each look's tuned defaults: the CRT's fills are faint tints of its one green, the modern
    /// look's are full region colours (its theme carries them already dimmed).
    pub fn for_look(look: Look) -> Self {
        match look {
            Look::Crt => Self {
                front_width_px: 2.0,
                path_width_px: 1.0,
                boundary_brightness: 0.45,
                fill_brightness: 0.06,
                rock_gain: 1.0,
            },
            Look::Modern => Self {
                front_width_px: 2.0,
                path_width_px: 1.0,
                boundary_brightness: 0.6,
                fill_brightness: 1.0,
                rock_gain: 1.0,
            },
        }
    }
}

/// Relative brightness of a phase: direct waves brightest.
pub fn phase_gain(phase: RayPhase) -> f32 {
    match phase {
        RayPhase::P => 1.0,
        RayPhase::S => 0.85,
        RayPhase::PKP | RayPhase::PKIKP | RayPhase::SKS => 0.75,
        RayPhase::PcP | RayPhase::ScS => 0.7,
        RayPhase::PP | RayPhase::SS | RayPhase::pP | RayPhase::sP => 0.6,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DiscUniforms {
    view: [f32; 4],
    disc: [f32; 4],
    rings_a: [f32; 4],
    rings_b: [f32; 4],
    style: [f32; 4],
    background: [f32; 4],
    fill_mantle: [f32; 4],
    fill_outer: [f32; 4],
    fill_inner: [f32; 4],
    fill_weights: [f32; 4],
    boundary_colour: [f32; 4],
    chrome_colour: [f32; 4],
    source_colour: [f32; 4],
    rock_fast: [f32; 4],
    rock_slow: [f32; 4],
    rock_levels: [f32; 4],
    rock: [f32; 4],
    rock_axis: [f32; 4],
}

/// Build the vertex list for every layer.
fn build_vertices(
    view: &SliceView,
    scene: &SliceScene,
    style: &SliceStyle,
    theme: &Theme,
) -> Vec<LineVertex> {
    let mut out = Vec::new();
    let mut pts: Vec<[f32; 2]> = Vec::new();
    for layer in scene.layers {
        for line in layer.lines {
            pts.clear();
            pts.extend(
                line.points
                    .iter()
                    .map(|p| view.project(p.r_km, p.delta_deg)),
            );
            let (width, dash, colour) = match (layer.kind, line.wave) {
                (StrokeKind::Front, Wave::P) => (style.front_width_px, DASH_SOLID, theme.p_front),
                (StrokeKind::Front, Wave::S) => (style.front_width_px, DASH_DASHED, theme.s_front),
                (StrokeKind::Path, _) => (style.path_width_px, DASH_DOTTED, theme.ray_path),
            };
            let intensity = layer.intensity * phase_gain(line.phase);
            push_polyline(&mut out, &pts, 0.5 * width, intensity, dash, colour);
        }
    }
    out
}

pub struct SlicePass {
    disc_pipeline: wgpu::RenderPipeline,
    disc_uniforms: wgpu::Buffer,
    disc_bind_group: wgpu::BindGroup,
    cut_texture: wgpu::Texture,
    lines: LineBatch,
}

impl SlicePass {
    pub fn new(device: &wgpu::Device) -> Self {
        let disc_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("slice disc"),
            entries: &[
                gpu::uniform_entry(0),
                gpu::texture_entry(1, wgpu::TextureViewDimension::D2),
            ],
        });

        let disc_pipeline = gpu::fullscreen_pipeline(
            device,
            "slice disc",
            include_str!("../../../assets/shaders/slice_disc.wgsl"),
            &disc_layout,
            &[Some(wgpu::ColorTargetState {
                format: crate::SCENE_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        );

        let disc_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("slice disc uniforms"),
            size: size_of::<DiscUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cut_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("slice rock"),
            size: wgpu::Extent3d {
                width: CUT_DELTA_COUNT as u32,
                height: CUT_R_COUNT as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let cut_view = cut_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let disc_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("slice disc"),
            layout: &disc_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: disc_uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&cut_view),
                },
            ],
        });
        Self {
            disc_pipeline,
            disc_uniforms,
            disc_bind_group,
            cut_texture,
            lines: LineBatch::new(device, "slice lines"),
        }
    }

    /// The rock to shade: dlnV over the slice's half.
    pub fn upload_cut(&self, queue: &wgpu::Queue, grid: &CutGrid) {
        queue.write_texture(
            self.cut_texture.as_image_copy(),
            bytemuck::cast_slice(&grid.values),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * CUT_DELTA_COUNT as u32),
                rows_per_image: Some(CUT_R_COUNT as u32),
            },
            wgpu::Extent3d {
                width: CUT_DELTA_COUNT as u32,
                height: CUT_R_COUNT as u32,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Draw into `viewport` of `target`, keeping the rest of the target.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: gpu::Viewport,
        scene: &SliceScene,
        style: &SliceStyle,
        theme: &Theme,
    ) {
        let view = SliceView::new((viewport.w, viewport.h));
        let frac = |depth_km: f64| ((EARTH_RADIUS_KM - depth_km) / EARTH_RADIUS_KM) as f32;
        let find = |name: &str| {
            scene
                .boundaries
                .iter()
                .find(|b| b.name == name)
                .map_or(-1.0, |b| frac(b.depth_km))
        };
        let mut rings = [-1.0f32; MAX_SLICE_BOUNDARIES];
        for (r, b) in rings.iter_mut().zip(scene.boundaries) {
            *r = frac(b.depth_km);
        }
        let [fw0, fw1, fw2] = theme.fill_weights;
        let [rl0, rl1, rl2] = theme.rock_levels;
        let disc = DiscUniforms {
            view: [viewport.w, viewport.h, view.centre[0], view.centre[1]],
            disc: [
                view.radius_px,
                frac(scene.source_depth_km),
                find("CMB"),
                find("ICB"),
            ],
            rings_a: [rings[0], rings[1], rings[2], rings[3]],
            rings_b: [rings[4], rings[5], rings[6], rings[7]],
            style: [style.boundary_brightness, style.fill_brightness, 0.0, 0.0],
            background: vec4(theme.background),
            fill_mantle: vec4(theme.fills[0]),
            fill_outer: vec4(theme.fills[1]),
            fill_inner: vec4(theme.fills[2]),
            fill_weights: [fw0, fw1, fw2, 0.0],
            boundary_colour: vec4(theme.boundary),
            chrome_colour: vec4(theme.chrome),
            source_colour: vec4(theme.epicentre),
            rock_fast: vec4(theme.rock_fast),
            rock_slow: vec4(theme.rock_slow),
            rock_levels: [rl0, rl1, rl2, 0.0],
            rock: [
                if scene.rock_shading { 1.0 } else { 0.0 },
                style.rock_gain,
                find("Moho"),
                (CUT_R_BOTTOM_KM / CUT_R_TOP_KM) as f32,
            ],
            rock_axis: [
                CUT_DELTA_STEP_DEG as f32,
                CUT_DELTA_COUNT as f32,
                CUT_R_COUNT as f32,
                0.0,
            ],
        };
        queue.write_buffer(&self.disc_uniforms, 0, bytemuck::bytes_of(&disc));
        let vertices = build_vertices(&view, scene, style, theme);
        self.lines.upload(
            device,
            queue,
            (viewport.w, viewport.h),
            theme.background,
            &vertices,
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("slice"),
            color_attachments: &[gpu::color_attachment_load(target)],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        viewport.apply(&mut pass);
        pass.set_pipeline(&self.disc_pipeline);
        pass.set_bind_group(0, &self.disc_bind_group, &[]);
        pass.draw(0..3, 0..1);
        self.lines.draw(&mut pass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_puts_source_left_and_antipode_right() {
        let v = SliceView::new((800.0, 400.0));
        let src = v.project(EARTH_RADIUS_KM, 0.0);
        let anti = v.project(EARTH_RADIUS_KM, 180.0);
        let top = v.project(EARTH_RADIUS_KM, 90.0);
        let centre = v.project(0.0, 37.0);
        assert!((src[0] - (v.centre[0] - v.radius_px)).abs() < 1e-3);
        assert!((anti[0] - (v.centre[0] + v.radius_px)).abs() < 1e-3);
        assert!((src[1] - v.centre[1]).abs() < 1e-3 && (anti[1] - v.centre[1]).abs() < 1e-3);
        assert!((top[1] - (v.centre[1] - v.radius_px)).abs() < 1e-3);
        assert_eq!(centre, v.centre);
        // The half-disc fits in the viewport.
        assert!(src[0] >= 0.0 && anti[0] <= 800.0 && top[1] >= 0.0 && v.centre[1] <= 400.0);
    }

    #[test]
    fn fronts_take_their_wave_colour_and_paths_their_own() {
        use sim::slice::SlicePoint;
        let v = SliceView::new((800.0, 400.0));
        let line = |wave: Wave| SliceLine {
            phase: RayPhase::P,
            wave,
            points: vec![
                SlicePoint {
                    r_km: EARTH_RADIUS_KM,
                    delta_deg: 0.0,
                },
                SlicePoint {
                    r_km: EARTH_RADIUS_KM - 500.0,
                    delta_deg: 10.0,
                },
            ],
        };
        let (p, s) = (vec![line(Wave::P)], vec![line(Wave::S)]);
        let layers = [
            SliceLayer {
                lines: &p,
                kind: StrokeKind::Front,
                intensity: 1.0,
            },
            SliceLayer {
                lines: &s,
                kind: StrokeKind::Front,
                intensity: 1.0,
            },
            SliceLayer {
                lines: &p,
                kind: StrokeKind::Path,
                intensity: 0.3,
            },
        ];
        let scene = SliceScene {
            source_depth_km: 0.0,
            boundaries: &[],
            layers: &layers,
            rock_shading: false,
        };
        let t = Theme::modern();
        let out = build_vertices(&v, &scene, &SliceStyle::for_look(Look::Modern), &t);
        assert_eq!(out.len(), 3 * 6);
        assert_eq!(out[0].colour, vec4(t.p_front));
        assert_eq!(out[6].colour, vec4(t.s_front));
        assert_eq!(out[12].colour, vec4(t.ray_path));
        assert_eq!(out[12].params[2], DASH_DOTTED);
    }

    #[test]
    fn default_style_is_the_crt_look() {
        assert_eq!(SliceStyle::default(), SliceStyle::for_look(Look::Crt));
        assert!(
            SliceStyle::for_look(Look::Modern).fill_brightness
                > SliceStyle::for_look(Look::Crt).fill_brightness
        );
    }
}
