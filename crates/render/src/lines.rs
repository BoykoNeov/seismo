//! Anti-aliased lines, shared by the passes that draw strokes over a viewport (slice fronts and
//! ray paths, seismogram traces, map station markers). One quad per polyline segment, built on
//! the CPU per line, not per pixel; blended with max so overlaps do not brighten.

use bytemuck::{Pod, Zeroable};

use crate::theme::Rgb;

pub const DASH_SOLID: f32 = 0.0;
pub const DASH_DASHED: f32 = 1.0;
pub const DASH_DOTTED: f32 = 2.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 2],
    pub along_across: [f32; 2],
    pub params: [f32; 4],
    /// Display-space RGB of the line (the fourth value is unused).
    pub colour: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LineUniforms {
    view: [f32; 4],
    background: [f32; 4],
}

/// Append one anti-aliased quad per segment of `points` (viewport pixels, y down), in `colour`.
/// Each quad reaches one pixel beyond the half width for the soft edge and overlaps its
/// neighbours at the joints; the max blend hides the overlap.
pub fn push_polyline(
    out: &mut Vec<LineVertex>,
    points: &[[f32; 2]],
    half_w: f32,
    intensity: f32,
    dash: f32,
    colour: Rgb,
) {
    let colour = crate::theme::vec4(colour);
    let ext = half_w + 1.0;
    let mut along = 0.0f32;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy);
        if len < 1e-3 {
            continue;
        }
        let (ux, uy) = (dx / len, dy / len);
        let (nx, ny) = (-uy, ux);
        let v = |p: [f32; 2], s_along: f32, s_across: f32, along_px: f32| LineVertex {
            pos: [
                p[0] + ux * s_along + nx * s_across,
                p[1] + uy * s_along + ny * s_across,
            ],
            along_across: [along_px, s_across],
            params: [half_w, intensity, dash, 0.0],
            colour,
        };
        let a0 = v(a, -ext, ext, along - ext);
        let a1 = v(a, -ext, -ext, along - ext);
        let b0 = v(b, ext, ext, along + len + ext);
        let b1 = v(b, ext, -ext, along + len + ext);
        out.extend_from_slice(&[a0, a1, b0, b0, a1, b1]);
        along += len;
    }
}

/// A pipeline plus its own uniform and vertex buffers: one per pass, since buffer writes all
/// land before the frame's passes run.
pub struct LineBatch {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    capacity: usize,
    count: u32,
    label: &'static str,
}

impl LineBatch {
    pub fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &[wgpu::BindGroupLayoutEntry {
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ..crate::gpu::uniform_entry(0)
            }],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size_of::<LineUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let capacity = 4096;
        Self {
            pipeline: pipeline(device, &layout, label),
            uniforms,
            bind_group,
            vertices: vertex_buffer(device, label, capacity),
            capacity,
            count: 0,
            label,
        }
    }

    /// Upload `vertices` (pixels of a viewport `size` wide and high, over `background`) for the
    /// next [`Self::draw`].
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (f32, f32),
        background: Rgb,
        vertices: &[LineVertex],
    ) {
        let u = LineUniforms {
            view: [size.0, size.1, 0.0, 0.0],
            background: crate::theme::vec4(background),
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));
        if vertices.len() > self.capacity {
            self.capacity = vertices.len().next_power_of_two();
            self.vertices = vertex_buffer(device, self.label, self.capacity);
        }
        if !vertices.is_empty() {
            queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(vertices));
        }
        self.count = vertices.len() as u32;
    }

    /// Draw what was last uploaded into the pass's current viewport.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..self.count, 0..1);
    }
}

fn vertex_buffer(device: &wgpu::Device, label: &str, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (capacity * size_of::<LineVertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    label: &str,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(include_str!("../../../assets/shaders/lines.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    let max = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Max,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<LineVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4],
            })],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: crate::SCENE_FORMAT,
                blend: Some(wgpu::BlendState {
                    color: max,
                    alpha: max,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polyline_quads_carry_distance_along() {
        let mut out = Vec::new();
        let colour = [0.1, 0.2, 0.3];
        push_polyline(
            &mut out,
            &[[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]],
            1.0,
            1.0,
            0.0,
            colour,
        );
        assert_eq!(out.len(), 12);
        // Second segment starts 10 px along the line (minus the cap overlap).
        assert!((out[6].along_across[0] - (10.0 - 2.0)).abs() < 1e-6);
        assert!(out.iter().all(|v| v.colour == [0.1, 0.2, 0.3, 0.0]));
        // Degenerate segments are skipped.
        let mut none = Vec::new();
        push_polyline(&mut none, &[[1.0, 1.0], [1.0, 1.0]], 1.0, 1.0, 0.0, colour);
        assert!(none.is_empty());
    }
}
