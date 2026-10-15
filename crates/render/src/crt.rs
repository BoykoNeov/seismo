//! CRT post-process: phosphor persistence, bloom (half-resolution blur), barrel distortion,
//! scanlines (thinner where the beam is bright), hot spots going pale, signal noise, a rolling
//! hum bar, vignette, rounded glass corners, flicker, and a warm-up when the tube is switched
//! on, over the scene only. egui draws on top, undistorted; whatever it
//! places over the scene goes through [`CrtSettings::scene_to_display`], and mouse picking
//! through [`CrtSettings::display_to_scene`].

use bytemuck::{Pod, Zeroable};

use crate::gpu;

/// Every CRT effect has an enable flag so it can be switched off for debugging.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrtSettings {
    pub enabled: bool,
    pub scanlines: bool,
    /// How dark the gaps between scanlines get, [0, 1].
    pub scanline_intensity: f32,
    /// Scanline period in output pixels.
    pub scanline_period_px: f32,
    pub persistence: bool,
    /// Time (real seconds) for the afterglow to fall to 1/e.
    pub persistence_tau_s: f32,
    pub bloom: bool,
    /// How much of the blurred glow is added back.
    pub bloom_intensity: f32,
    /// Glow radius: Gaussian sigma in scene pixels.
    pub bloom_radius_px: f32,
    /// Channel values at or below this do not glow (the background and faint shading stay
    /// clean).
    pub bloom_threshold: f32,
    pub barrel: bool,
    /// Bulge of the glass: 0 is flat; the corners move out by about this fraction.
    pub barrel_amount: f32,
    pub flicker: bool,
    /// Largest brightness dip, [0, 1].
    pub flicker_amount: f32,
    /// A bright beam is a fat beam: how much a fully bright pixel fills the gap between its
    /// scanlines, [0, 1] (0 = the lines are as deep everywhere).
    pub scanline_fill: f32,
    /// Overbright cores go pale, as a saturated phosphor does.
    pub hot_spots: bool,
    /// How fast: 1 means a level of 2 is fully white.
    pub hot_spot_amount: f32,
    /// The tube dims towards its edges, and the glass has rounded corners.
    pub vignette: bool,
    /// Darkening at the far corners, [0, 1].
    pub vignette_amount: f32,
    /// Corner radius as a fraction of the picture's shorter side (0 = square).
    pub corner_radius: f32,
    /// Faint grain that changes every frame.
    pub noise: bool,
    /// Peak-to-peak brightness of the grain on a bright pixel.
    pub noise_amount: f32,
    /// A faint band of brightness rolling down the picture.
    pub hum_bar: bool,
    /// Brightness gain at the band's centre.
    pub hum_bar_amount: f32,
    /// Seconds for the band to roll once down the picture.
    pub hum_bar_period_s: f32,
    /// The picture opens from a bright horizontal line when the tube is switched on.
    pub warm_up: bool,
    /// How long the opening takes, real seconds.
    pub warm_up_s: f32,
}

impl Default for CrtSettings {
    /// The CRT look: every effect on.
    fn default() -> Self {
        Self {
            enabled: true,
            scanlines: true,
            scanline_intensity: 0.35,
            scanline_period_px: 3.0,
            persistence: true,
            persistence_tau_s: 0.25,
            bloom: true,
            bloom_intensity: 0.8,
            bloom_radius_px: 8.0,
            bloom_threshold: 0.1,
            barrel: true,
            barrel_amount: 0.04,
            flicker: true,
            flicker_amount: 0.03,
            scanline_fill: 0.6,
            hot_spots: true,
            hot_spot_amount: 0.6,
            vignette: true,
            vignette_amount: 0.35,
            corner_radius: 0.03,
            noise: true,
            noise_amount: 0.06,
            hum_bar: true,
            hum_bar_amount: 0.035,
            hum_bar_period_s: 9.0,
            warm_up: true,
            warm_up_s: 0.9,
        }
    }
}

/// Flicker picks a new brightness this many times per real second, whatever the frame rate.
const FLICKER_HZ: f32 = 24.0;

impl CrtSettings {
    /// The modern look's post-process: flat glass, steady, no afterglow; only a soft glow
    /// around the bright fronts (the threshold keeps the region fills and chrome out of it).
    pub fn modern() -> Self {
        Self {
            enabled: true,
            scanlines: false,
            persistence: false,
            bloom: true,
            bloom_intensity: 0.45,
            bloom_radius_px: 7.0,
            bloom_threshold: 0.45,
            barrel: false,
            flicker: false,
            hot_spots: false,
            vignette: false,
            noise: false,
            hum_bar: false,
            warm_up: false,
            ..Self::default()
        }
    }

    /// The settings a look starts with.
    pub fn for_look(look: crate::Look) -> Self {
        match look {
            crate::Look::Crt => Self::default(),
            crate::Look::Modern => Self::modern(),
        }
    }

    /// Factor applied to last frame's persistence buffer after `real_dt_s` real seconds:
    /// decay is per real second, so it is independent of frame rate and of sim speed.
    pub fn persistence_decay(&self, real_dt_s: f32) -> f32 {
        if self.persistence_tau_s <= 0.0 {
            return 0.0;
        }
        (-real_dt_s.max(0.0) / self.persistence_tau_s).exp()
    }

    /// Barrel strength actually applied: 0 when barrel or the master switch is off.
    fn barrel_k(&self) -> f32 {
        if self.enabled && self.barrel {
            self.barrel_amount.max(0.0)
        } else {
            0.0
        }
    }

    /// Brightness gain at `time_s` real seconds: 1 when flicker is off, otherwise in
    /// [1 - amount, 1], stepping `FLICKER_HZ` times a second. Deterministic in `time_s`.
    pub fn flicker_gain(&self, time_s: f32) -> f32 {
        if !(self.enabled && self.flicker) {
            return 1.0;
        }
        let step = (time_s.max(0.0) * FLICKER_HZ) as u32;
        // A small integer hash (lowbias32) to a value in [0, 1).
        let mut x = step;
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb_352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846c_a68b);
        x ^= x >> 16;
        let n = (x >> 8) as f32 / (1u32 << 24) as f32;
        1.0 - self.flicker_amount.clamp(0.0, 1.0) * n
    }

    /// Centre of the hum bar at `time_s` real seconds, as a fraction of the picture's height
    /// (top 0, bottom 1): it rolls down, from just above the top to just below the bottom, once
    /// every `hum_bar_period_s`. Parked off-screen when off.
    pub fn hum_bar_centre(&self, time_s: f32) -> f32 {
        if !(self.enabled && self.hum_bar) || self.hum_bar_period_s <= 0.0 {
            return -10.0;
        }
        let phase = (time_s.max(0.0) / self.hum_bar_period_s).fract();
        -0.25 + 1.5 * phase
    }

    /// Warm-up progress `since_on_s` real seconds after the tube was switched on: 0 is a
    /// collapsed line, 1 the steady picture (at once when warm-up is off).
    pub fn warm_up_progress(&self, since_on_s: f32) -> f32 {
        if !(self.enabled && self.warm_up) || self.warm_up_s <= 0.0 {
            return 1.0;
        }
        (since_on_s.max(0.0) / self.warm_up_s).min(1.0)
    }

    /// Where the displayed pixel `p` (pixels, top-left origin, of an image `size` pixels) reads
    /// the scene from, or `None` outside the curved glass. The same mapping as `barrel` in
    /// `crt.wgsl`.
    pub fn display_to_scene(&self, size: (u32, u32), p: [f32; 2]) -> Option<[f32; 2]> {
        let k = self.barrel_k();
        let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let c = [p[0] / w * 2.0 - 1.0, p[1] / h * 2.0 - 1.0];
        let f = (1.0 + k * (c[0] * c[0] + c[1] * c[1])) / (1.0 + k);
        let d = [c[0] * f, c[1] * f];
        // A hair of slack so the scene's own corners map back inside despite rounding.
        let inside = |v: f32| v.abs() <= 1.0 + 1e-5;
        (inside(d[0]) && inside(d[1])).then(|| [(d[0] + 1.0) * 0.5 * w, (d[1] + 1.0) * 0.5 * h])
    }

    /// Where the scene pixel `p` appears on screen: the inverse of [`Self::display_to_scene`].
    pub fn scene_to_display(&self, size: (u32, u32), p: [f32; 2]) -> [f32; 2] {
        let k = self.barrel_k();
        if k == 0.0 {
            return p;
        }
        let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let d = [p[0] / w * 2.0 - 1.0, p[1] / h * 2.0 - 1.0];
        let target = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if target == 0.0 {
            return p;
        }
        // The mapping is radial: solve rho (1 + k rho^2) = target (1 + k) for rho >= 0
        // (monotonic for k >= 0) by Newton's method from rho = target.
        let rhs = target * (1.0 + k);
        let mut rho = target;
        for _ in 0..8 {
            let f = rho * (1.0 + k * rho * rho) - rhs;
            rho -= f / (1.0 + 3.0 * k * rho * rho);
        }
        let s = rho / target;
        [(d[0] * s + 1.0) * 0.5 * w, (d[1] * s + 1.0) * 0.5 * h]
    }
}

/// Real (wall-clock) frame timing, as opposed to sim time.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameTime {
    /// Real seconds since the previous frame.
    pub dt_s: f32,
    /// Real seconds since start (for flicker and the hum bar).
    pub time_s: f32,
    /// Real seconds since the tube was switched on: the app's start, or the switch to this
    /// look (for the warm-up).
    pub since_on_s: f32,
    /// Drop the afterglow this frame (set on scrub/seek).
    pub clear_persistence: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view: [f32; 4],
    scanlines: [f32; 4],
    persistence: [f32; 4],
    bloom: [f32; 4],
    barrel: [f32; 4],
    flicker: [f32; 4],
    glass: [f32; 4],
    signal: [f32; 4],
    tint: [f32; 4],
}

fn flag(b: bool) -> f32 {
    if b { 1.0 } else { 0.0 }
}

fn half(size: (u32, u32)) -> (u32, u32) {
    (size.0.div_ceil(2).max(1), size.1.div_ceil(2).max(1))
}

struct Targets {
    /// Ping-pong persistence buffers; `current` is the one written this frame.
    persist: [gpu::Target; 2],
    /// Half-res glow: blurred across, then down.
    bloom_h: gpu::Target,
    bloom_v: gpu::Target,
    output: gpu::Target,
    /// `persist_groups[i]` reads `persist[1 - i]` and is used when writing `persist[i]`;
    /// `bloom_h_groups[i]` and `display_groups[i]` read `persist[i]`.
    persist_groups: [wgpu::BindGroup; 2],
    bloom_h_groups: [wgpu::BindGroup; 2],
    bloom_v_group: wgpu::BindGroup,
    display_groups: [wgpu::BindGroup; 2],
}

struct Layouts {
    persist: wgpu::BindGroupLayout,
    bloom_h: wgpu::BindGroupLayout,
    bloom_v: wgpu::BindGroupLayout,
    display: wgpu::BindGroupLayout,
}

pub struct CrtPass {
    persist_pipeline: wgpu::RenderPipeline,
    bloom_h_pipeline: wgpu::RenderPipeline,
    bloom_v_pipeline: wgpu::RenderPipeline,
    display_pipeline: wgpu::RenderPipeline,
    layouts: Layouts,
    uniform_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    targets: Targets,
    current: usize,
}

impl CrtPass {
    pub fn new(device: &wgpu::Device, size: (u32, u32), scene: &wgpu::TextureView) -> Self {
        let layout = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let d2 = wgpu::TextureViewDimension::D2;
        let layouts = Layouts {
            persist: layout(
                "crt persist",
                &[
                    gpu::uniform_entry(0),
                    gpu::texture_entry(1, d2),
                    gpu::texture_entry(2, d2),
                ],
            ),
            bloom_h: layout(
                "crt bloom h",
                &[
                    gpu::uniform_entry(0),
                    gpu::filterable_texture_entry(3),
                    gpu::sampler_entry(4),
                ],
            ),
            bloom_v: layout(
                "crt bloom v",
                &[gpu::uniform_entry(0), gpu::texture_entry(5, d2)],
            ),
            display: layout(
                "crt display",
                &[
                    gpu::uniform_entry(0),
                    gpu::filterable_texture_entry(3),
                    gpu::sampler_entry(4),
                    gpu::filterable_texture_entry(6),
                ],
            ),
        };
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("crt"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../assets/shaders/crt.wgsl").into(),
            ),
        });
        let pipeline = |entry: &str, layout: &wgpu::BindGroupLayout, format| {
            let target = Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            });
            gpu::fullscreen_pipeline_entry(device, entry, &module, entry, layout, &[target])
        };
        let persist_pipeline = pipeline("fs_persist", &layouts.persist, crate::SCENE_FORMAT);
        let bloom_h_pipeline = pipeline("fs_bloom_h", &layouts.bloom_h, crate::SCENE_FORMAT);
        let bloom_v_pipeline = pipeline("fs_bloom_v", &layouts.bloom_v, crate::SCENE_FORMAT);
        let display_pipeline = pipeline("fs_display", &layouts.display, crate::OUTPUT_FORMAT);
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("crt uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("crt linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let targets = Self::targets(device, &layouts, &uniform_buffer, &sampler, size, scene);
        Self {
            persist_pipeline,
            bloom_h_pipeline,
            bloom_v_pipeline,
            display_pipeline,
            layouts,
            uniform_buffer,
            sampler,
            targets,
            current: 0,
        }
    }

    fn targets(
        device: &wgpu::Device,
        layouts: &Layouts,
        uniforms: &wgpu::Buffer,
        sampler: &wgpu::Sampler,
        size: (u32, u32),
        scene: &wgpu::TextureView,
    ) -> Targets {
        use wgpu::BindingResource::{Sampler, TextureView};
        let persist = [
            gpu::Target::new(device, "persistence a", size, crate::SCENE_FORMAT),
            gpu::Target::new(device, "persistence b", size, crate::SCENE_FORMAT),
        ];
        let bloom_h = gpu::Target::new(device, "bloom h", half(size), crate::SCENE_FORMAT);
        let bloom_v = gpu::Target::new(device, "bloom v", half(size), crate::SCENE_FORMAT);
        let output = gpu::Target::new(device, "crt output", size, crate::OUTPUT_FORMAT);
        // Binding 0 is always the uniforms; `rest` are the pass's own slots.
        let bind = |label: &str,
                    layout: &wgpu::BindGroupLayout,
                    rest: Vec<(u32, wgpu::BindingResource)>| {
            let mut entries = vec![wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }];
            entries.extend(
                rest.into_iter()
                    .map(|(binding, resource)| wgpu::BindGroupEntry { binding, resource }),
            );
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &entries,
            })
        };
        let persist_group = |reads: &wgpu::TextureView| {
            bind(
                "crt persist",
                &layouts.persist,
                vec![(1, TextureView(scene)), (2, TextureView(reads))],
            )
        };
        let bloom_h_group = |reads: &wgpu::TextureView| {
            bind(
                "crt bloom h",
                &layouts.bloom_h,
                vec![(3, TextureView(reads)), (4, Sampler(sampler))],
            )
        };
        let display_group = |reads: &wgpu::TextureView| {
            bind(
                "crt display",
                &layouts.display,
                vec![
                    (3, TextureView(reads)),
                    (4, Sampler(sampler)),
                    (6, TextureView(&bloom_v.view)),
                ],
            )
        };
        Targets {
            persist_groups: [
                persist_group(&persist[1].view),
                persist_group(&persist[0].view),
            ],
            bloom_h_groups: [
                bloom_h_group(&persist[0].view),
                bloom_h_group(&persist[1].view),
            ],
            bloom_v_group: bind(
                "crt bloom v",
                &layouts.bloom_v,
                vec![(5, TextureView(&bloom_h.view))],
            ),
            display_groups: [
                display_group(&persist[0].view),
                display_group(&persist[1].view),
            ],
            persist,
            bloom_h,
            bloom_v,
            output,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32), scene: &wgpu::TextureView) {
        self.targets = Self::targets(
            device,
            &self.layouts,
            &self.uniform_buffer,
            &self.sampler,
            size,
            scene,
        );
        self.current = 0;
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.targets.output.view
    }

    /// The displayed image as tightly packed RGBA8 rows (blocking; for screenshots).
    pub fn read_output(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
        self.targets.output.read_rgba8(device, queue)
    }

    /// Encode the passes. New persistence buffers start cleared, and `clear_persistence`
    /// (on scrub/seek) drops the afterglow for this frame. `phosphor` is the colour the noise
    /// is drawn in.
    pub fn draw(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        size: (u32, u32),
        s: &CrtSettings,
        phosphor: crate::Rgb,
        time: FrameTime,
    ) {
        // The master switch and the enable flags are applied here; the shader sees only
        // neutral values for what is off.
        let on = |b: bool| s.enabled && b;
        let decay = if time.clear_persistence {
            0.0
        } else {
            s.persistence_decay(time.dt_s)
        };
        let bloom = on(s.bloom) && s.bloom_intensity > 0.0;
        let h = half(size);
        let uniforms = Uniforms {
            view: [size.0 as f32, size.1 as f32, h.0 as f32, h.1 as f32],
            scanlines: [
                flag(on(s.scanlines)),
                s.scanline_intensity,
                s.scanline_period_px,
                0.0,
            ],
            persistence: [flag(on(s.persistence)), decay, 0.0, 0.0],
            bloom: [
                if bloom { s.bloom_intensity } else { 0.0 },
                s.bloom_threshold.max(0.0),
                // Scene pixels to half-res pixels.
                s.bloom_radius_px * 0.5,
                0.0,
            ],
            barrel: [s.barrel_k(), 0.0, 0.0, 0.0],
            flicker: [s.flicker_gain(time.time_s), 0.0, 0.0, 0.0],
            glass: [
                if on(s.vignette) {
                    s.vignette_amount.clamp(0.0, 1.0)
                } else {
                    0.0
                },
                if on(s.vignette) {
                    s.corner_radius.max(0.0) * size.0.min(size.1) as f32
                } else {
                    0.0
                },
                if on(s.noise) {
                    s.noise_amount.max(0.0)
                } else {
                    0.0
                },
                if on(s.hot_spots) {
                    s.hot_spot_amount.max(0.0)
                } else {
                    0.0
                },
            ],
            signal: [
                s.hum_bar_centre(time.time_s),
                if on(s.hum_bar) {
                    s.hum_bar_amount.max(0.0)
                } else {
                    0.0
                },
                s.warm_up_progress(time.since_on_s),
                if on(s.scanlines) {
                    s.scanline_fill.clamp(0.0, 1.0)
                } else {
                    0.0
                },
            ],
            // The noise seed changes every frame; it only needs to differ frame to frame.
            tint: [
                phosphor[0],
                phosphor[1],
                phosphor[2],
                (time.time_s * 1000.0) % 65536.0,
            ],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        self.current = 1 - self.current;
        let (t, cur) = (&self.targets, self.current);
        let mut run = |label: &str,
                       view: &wgpu::TextureView,
                       pipeline: &wgpu::RenderPipeline,
                       group: &wgpu::BindGroup| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[gpu::color_attachment(view)],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        };
        run(
            "crt persist",
            &t.persist[cur].view,
            &self.persist_pipeline,
            &t.persist_groups[cur],
        );
        if bloom {
            run(
                "crt bloom h",
                &t.bloom_h.view,
                &self.bloom_h_pipeline,
                &t.bloom_h_groups[cur],
            );
            run(
                "crt bloom v",
                &t.bloom_v.view,
                &self.bloom_v_pipeline,
                &t.bloom_v_group,
            );
        }
        run(
            "crt display",
            &t.output.view,
            &self.display_pipeline,
            &t.display_groups[cur],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decay_is_per_real_second() {
        let s = CrtSettings {
            persistence_tau_s: 0.5,
            ..Default::default()
        };
        // Two half-frames decay exactly as much as one full frame.
        let one = s.persistence_decay(1.0 / 30.0);
        let two = s.persistence_decay(1.0 / 60.0).powi(2);
        assert!((one - two).abs() < 1e-6);
        assert!((s.persistence_decay(0.5) - (-1.0f32).exp()).abs() < 1e-6);
        assert_eq!(s.persistence_decay(0.0), 1.0);
        let off = CrtSettings {
            persistence_tau_s: 0.0,
            ..Default::default()
        };
        assert_eq!(off.persistence_decay(0.1), 0.0);
    }

    const SIZE: (u32, u32) = (1600, 900);

    fn close(a: [f32; 2], b: [f32; 2], tol: f32) -> bool {
        (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol
    }

    #[test]
    fn barrel_off_is_identity() {
        for s in [
            CrtSettings {
                barrel: false,
                ..Default::default()
            },
            CrtSettings {
                enabled: false,
                ..Default::default()
            },
        ] {
            let p = [123.25, 700.5];
            assert_eq!(s.scene_to_display(SIZE, p), p);
            assert!(close(s.display_to_scene(SIZE, p).unwrap(), p, 1e-3));
        }
    }

    #[test]
    fn barrel_keeps_centre_and_edge_middles_and_hides_corners() {
        let s = CrtSettings {
            barrel_amount: 0.1,
            ..Default::default()
        };
        let (w, h) = (SIZE.0 as f32, SIZE.1 as f32);
        for p in [
            [w / 2.0, h / 2.0],
            [0.0, h / 2.0],
            [w, h / 2.0],
            [w / 2.0, 0.0],
        ] {
            let q = s.display_to_scene(SIZE, p).expect("inside");
            assert!(close(q, p, 1e-2), "{p:?} -> {q:?}");
        }
        // The display's corners look past the scene's edge: nothing is drawn there.
        assert_eq!(s.display_to_scene(SIZE, [1.0, 1.0]), None);
        // Points between centre and edge move outwards on screen: the middle is magnified,
        // as behind bulging glass.
        let q = s.scene_to_display(SIZE, [w * 0.25, h * 0.25]);
        assert!(q[0] < w * 0.25 && q[1] < h * 0.25, "{q:?}");
    }

    #[test]
    fn scene_to_display_inverts_display_to_scene() {
        let s = CrtSettings {
            barrel_amount: 0.2,
            ..Default::default()
        };
        for x in (0..=16).map(|i| i as f32 * 100.0) {
            for y in (0..=9).map(|i| i as f32 * 100.0) {
                let p = [x, y];
                let d = s.scene_to_display(SIZE, p);
                let back = s
                    .display_to_scene(SIZE, d)
                    .expect("every scene point is on screen");
                assert!(close(back, p, 1e-2), "{p:?} -> {d:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn modern_post_is_flat_and_steady() {
        let m = CrtSettings::modern();
        assert!(!m.scanlines && !m.persistence && !m.barrel && !m.flicker);
        assert!(!m.hot_spots && !m.vignette && !m.noise && !m.hum_bar && !m.warm_up);
        assert_eq!(m.flicker_gain(2.5), 1.0);
        assert_eq!(m.warm_up_progress(0.0), 1.0);
        assert!(m.hum_bar_centre(2.5) < 0.0);
        let p = [100.0, 200.0];
        assert_eq!(m.scene_to_display(SIZE, p), p);
        // Its glow starts higher up, so only bright features get one.
        assert!(m.bloom_threshold > CrtSettings::default().bloom_threshold);
        assert_eq!(CrtSettings::for_look(crate::Look::Modern), m);
        assert_eq!(
            CrtSettings::for_look(crate::Look::Crt),
            CrtSettings::default()
        );
    }

    #[test]
    fn flicker_is_bounded_frame_rate_independent_and_off_when_disabled() {
        let s = CrtSettings {
            flicker_amount: 0.2,
            ..Default::default()
        };
        let gains: Vec<f32> = (0..1000).map(|i| s.flicker_gain(i as f32 / 97.0)).collect();
        assert!(gains.iter().all(|g| (0.8..=1.0).contains(g)));
        // It moves, and only changes at its own rate: the same time gives the same gain.
        assert!(gains.iter().any(|g| (g - gains[0]).abs() > 1e-3));
        assert_eq!(s.flicker_gain(1.0), s.flicker_gain(1.0 + 0.5 / FLICKER_HZ));
        let off = CrtSettings {
            flicker: false,
            ..s
        };
        assert_eq!(off.flicker_gain(3.7), 1.0);
    }

    #[test]
    fn hum_bar_rolls_down_once_a_period_and_parks_when_off() {
        let s = CrtSettings {
            hum_bar_period_s: 4.0,
            ..Default::default()
        };
        let top = s.hum_bar_centre(0.0);
        assert!(top < 0.0, "starts above the picture: {top}");
        let mid = s.hum_bar_centre(2.0);
        assert!((0.0..=1.0).contains(&mid) && mid > top);
        assert!(s.hum_bar_centre(3.99) > 1.0, "leaves at the bottom");
        assert!((s.hum_bar_centre(4.0) - top).abs() < 1e-5, "wraps");
        let off = CrtSettings {
            hum_bar: false,
            ..s
        };
        assert!(off.hum_bar_centre(2.0) < -1.0);
        let master_off = CrtSettings {
            enabled: false,
            ..s
        };
        assert!(master_off.hum_bar_centre(2.0) < -1.0);
    }

    #[test]
    fn warm_up_opens_then_holds() {
        let s = CrtSettings {
            warm_up_s: 0.5,
            ..Default::default()
        };
        assert_eq!(s.warm_up_progress(0.0), 0.0);
        assert!((s.warm_up_progress(0.25) - 0.5).abs() < 1e-6);
        assert_eq!(s.warm_up_progress(0.5), 1.0);
        assert_eq!(s.warm_up_progress(60.0), 1.0);
        assert_eq!(s.warm_up_progress(-1.0), 0.0);
        let off = CrtSettings {
            warm_up: false,
            ..s
        };
        assert_eq!(off.warm_up_progress(0.0), 1.0);
    }
}
