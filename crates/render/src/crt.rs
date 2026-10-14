//! CRT post-process: scanlines and phosphor persistence; bloom, barrel and flicker are reserved
//! uniform slots for phase 2.

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
    // Reserved for phase 2; passed to the shader but not used yet.
    pub bloom: bool,
    pub bloom_intensity: f32,
    pub barrel: bool,
    pub barrel_amount: f32,
    pub flicker: bool,
    pub flicker_amount: f32,
}

impl Default for CrtSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            scanlines: true,
            scanline_intensity: 0.35,
            scanline_period_px: 3.0,
            persistence: true,
            persistence_tau_s: 0.25,
            bloom: false,
            bloom_intensity: 0.0,
            barrel: false,
            barrel_amount: 0.0,
            flicker: false,
            flicker_amount: 0.0,
        }
    }
}

impl CrtSettings {
    /// Factor applied to last frame's persistence buffer after `real_dt_s` real seconds:
    /// decay is per real second, so it is independent of frame rate and of sim speed.
    pub fn persistence_decay(&self, real_dt_s: f32) -> f32 {
        if self.persistence_tau_s <= 0.0 {
            return 0.0;
        }
        (-real_dt_s.max(0.0) / self.persistence_tau_s).exp()
    }
}

/// Real (wall-clock) frame timing, as opposed to sim time.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameTime {
    /// Real seconds since the previous frame.
    pub dt_s: f32,
    /// Real seconds since start (for flicker, later).
    pub time_s: f32,
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
}

fn flag(b: bool) -> f32 {
    if b { 1.0 } else { 0.0 }
}

pub struct CrtPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    /// Ping-pong persistence buffers; `current` is the one written this frame.
    persist: [gpu::Target; 2],
    /// `bind_groups[i]` reads `persist[1 - i]` and is used when writing `persist[i]`.
    bind_groups: [wgpu::BindGroup; 2],
    output: gpu::Target,
    current: usize,
}

impl CrtPass {
    pub fn new(device: &wgpu::Device, size: (u32, u32), scene: &wgpu::TextureView) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("crt"),
            entries: &[
                gpu::uniform_entry(0),
                gpu::texture_entry(1, wgpu::TextureViewDimension::D2),
                gpu::texture_entry(2, wgpu::TextureViewDimension::D2),
            ],
        });
        let target = |format| {
            Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        let pipeline = gpu::fullscreen_pipeline(
            device,
            "crt",
            include_str!("../../../assets/shaders/crt.wgsl"),
            &layout,
            &[target(crate::SCENE_FORMAT), target(crate::OUTPUT_FORMAT)],
        );
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("crt uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (persist, bind_groups, output) =
            Self::targets(device, &layout, &uniform_buffer, size, scene);
        Self {
            pipeline,
            layout,
            uniform_buffer,
            persist,
            bind_groups,
            output,
            current: 0,
        }
    }

    fn targets(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniforms: &wgpu::Buffer,
        size: (u32, u32),
        scene: &wgpu::TextureView,
    ) -> ([gpu::Target; 2], [wgpu::BindGroup; 2], gpu::Target) {
        let persist = [
            gpu::Target::new(device, "persistence a", size, crate::SCENE_FORMAT),
            gpu::Target::new(device, "persistence b", size, crate::SCENE_FORMAT),
        ];
        let bind = |reads: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("crt"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(scene),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(reads),
                    },
                ],
            })
        };
        let bind_groups = [bind(&persist[1].view), bind(&persist[0].view)];
        let output = gpu::Target::new(device, "crt output", size, crate::OUTPUT_FORMAT);
        (persist, bind_groups, output)
    }

    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32), scene: &wgpu::TextureView) {
        (self.persist, self.bind_groups, self.output) =
            Self::targets(device, &self.layout, &self.uniform_buffer, size, scene);
        self.current = 0;
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.output.view
    }

    /// The displayed image as tightly packed RGBA8 rows (blocking; for screenshots).
    pub fn read_output(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
        self.output.read_rgba8(device, queue)
    }

    /// Encode the pass. New persistence buffers start cleared, and `clear_persistence`
    /// (on scrub/seek) drops the afterglow for this frame.
    pub fn draw(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        size: (u32, u32),
        s: &CrtSettings,
        time: FrameTime,
    ) {
        let decay = if time.clear_persistence {
            0.0
        } else {
            s.persistence_decay(time.dt_s)
        };
        let uniforms = Uniforms {
            view: [size.0 as f32, size.1 as f32, time.time_s, flag(s.enabled)],
            scanlines: [
                flag(s.scanlines),
                s.scanline_intensity,
                s.scanline_period_px,
                0.0,
            ],
            persistence: [flag(s.persistence), decay, 0.0, 0.0],
            bloom: [flag(s.bloom), s.bloom_intensity, 0.0, 0.0],
            barrel: [flag(s.barrel), s.barrel_amount, 0.0, 0.0],
            flicker: [flag(s.flicker), s.flicker_amount, 0.0, 0.0],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        self.current = 1 - self.current;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("crt"),
            color_attachments: &[
                gpu::color_attachment(&self.persist[self.current].view),
                gpu::color_attachment(&self.output.view),
            ],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_groups[self.current], &[]);
        pass.draw(0..3, 0..1);
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
}
