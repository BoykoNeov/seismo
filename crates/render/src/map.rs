//! Map pass: equirectangular graticule, epicentres and table-driven wavefronts.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::gpu;
use sim::tables::TravelTimeTable;

/// Events the map can draw at once; matches `MAX_EVENTS` in `map.wgsl`.
pub const MAX_MAP_EVENTS: usize = 32;

/// One event as the map shader sees it. `t` is seconds since origin (the only time the GPU
/// receives).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapEventInstance {
    pub lat: f32,
    pub lon: f32,
    pub depth_km: f32,
    pub t: f32,
    pub magnitude: f32,
}

/// The great circle the slice view cuts along, drawn on the map from the epicentre to its
/// antipode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapSliceLine {
    pub lat: f32,
    pub lon: f32,
    /// Degrees clockwise from north.
    pub azimuth_deg: f32,
}

/// Look of the map pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapStyle {
    /// Width of a wavefront line in pixels.
    pub front_width_px: f32,
    /// e-folding time (s) of the fading trail behind a front.
    pub trail_s: f32,
    /// Brightness of the trail just behind the front, [0, 1].
    pub trail_strength: f32,
    /// Brightness of the graticule, [0, 1].
    pub grid_brightness: f32,
}

impl Default for MapStyle {
    fn default() -> Self {
        Self {
            front_width_px: 2.0,
            trail_s: 40.0,
            trail_strength: 0.35,
            grid_brightness: 0.35,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuEvent {
    a: [f32; 4],
    b: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view: [f32; 4],
    depth_axis: [f32; 4],
    dist_axis: [f32; 4],
    layers: [f32; 4],
    style: [f32; 4],
    phosphor: [f32; 4],
    background: [f32; 4],
    slice: [f32; 4],
    events: [GpuEvent; MAX_MAP_EVENTS],
}

pub struct MapPass {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    depth_axis: [f32; 4],
    dist_axis: [f32; 4],
    layers: [f32; 4],
}

impl MapPass {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, table: &TravelTimeTable) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("map"),
            entries: &[
                gpu::uniform_entry(0),
                gpu::texture_entry(1, wgpu::TextureViewDimension::D2Array),
            ],
        });
        let pipeline = gpu::fullscreen_pipeline(
            device,
            "map",
            include_str!("../../../assets/shaders/map.wgsl"),
            &layout,
            &[Some(wgpu::ColorTargetState {
                format: crate::SCENE_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        );

        let table_view = upload_table(device, queue, table);
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("map uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("map"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&table_view),
                },
            ],
        });

        let (d, x) = (table.depth_axis(), table.distance_axis());
        let layer = |name: &str| table.phase_index(name).map_or(-1.0, |i| i as f32);
        Self {
            pipeline,
            uniform_buffer,
            bind_group,
            depth_axis: [d.start as f32, d.step as f32, d.count as f32, 0.0],
            dist_axis: [x.start as f32, x.step as f32, x.count as f32, 0.0],
            layers: [layer("P"), layer("S"), -1.0, -1.0],
        }
    }

    /// Draw into `viewport` of `target`, clearing the whole target first. At most
    /// [`MAX_MAP_EVENTS`] events are drawn; the caller culls and orders them.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: gpu::Viewport,
        events: &[MapEventInstance],
        style: &MapStyle,
        slice: Option<MapSliceLine>,
    ) {
        let n = events.len().min(MAX_MAP_EVENTS);
        let mut gpu_events = [GpuEvent::zeroed(); MAX_MAP_EVENTS];
        for (g, e) in gpu_events.iter_mut().zip(&events[..n]) {
            *g = GpuEvent {
                a: [e.lat, e.lon, e.depth_km, e.t],
                b: [e.magnitude, 0.0, 0.0, 0.0],
            };
        }
        let [pr, pg, pb] = crate::PHOSPHOR;
        let [br, bg, bb] = crate::BACKGROUND;
        let uniforms = Uniforms {
            view: [viewport.w, viewport.h, n as f32, 0.0],
            depth_axis: self.depth_axis,
            dist_axis: self.dist_axis,
            layers: self.layers,
            style: [
                style.front_width_px,
                style.trail_s,
                style.trail_strength,
                style.grid_brightness,
            ],
            phosphor: [pr, pg, pb, 0.0],
            background: [br, bg, bb, 0.0],
            slice: slice.map_or([0.0; 4], |l| [l.lat, l.lon, l.azimuth_deg, 1.0]),
            events: gpu_events,
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("map"),
            color_attachments: &[gpu::color_attachment(target)],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        viewport.apply(&mut pass);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Upload every phase as one layer of an RG32Float 2D array texture
/// (width = distance, height = depth), missing values as the sentinel.
fn upload_table(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    table: &TravelTimeTable,
) -> wgpu::TextureView {
    let (d, x) = (table.depth_axis(), table.distance_axis());
    let layers = table.phases().len() as u32;
    let mut data: Vec<[f32; 2]> = Vec::with_capacity(d.count * x.count * layers as usize);
    for phase in table.phases() {
        data.extend(table.gpu_grid(phase).expect("phase listed by the table"));
    }
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("travel-time table"),
            size: wgpu::Extent3d {
                width: x.count as u32,
                height: d.count as u32,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(&data),
    );
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("travel-time table"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}
