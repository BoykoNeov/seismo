//! Map pass: equirectangular graticule, coastlines, epicentres and table-driven wavefronts,
//! with the seismograph stations marked on top.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::coast::{Coastlines, coast_vertices};
use crate::gpu;
use crate::lines::{DASH_SOLID, LineBatch, LineVertex, push_polyline};
use sim::mantle::{
    DELAY_AZ_COUNT, DELAY_AZ_STEP_DEG, DELAY_DIST_COUNT, DELAY_DIST_STEP_DEG, DELAY_PHASES,
    DelayGrid,
};
use sim::tables::TravelTimeTable;

/// Half width (px) of a coastline, normal and wide (see [`MapStyle::coast_wide`]).
const COAST_HALF_WIDTH_PX: f32 = 0.5;
const COAST_HALF_WIDTH_WIDE_PX: f32 = 0.85;

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
    /// Slot of this event's 3D-mantle delays (see [`MapPass::upload_delays`]); `None` draws
    /// the 1D arrival times.
    pub delay_slot: Option<u32>,
    /// Largest |delay| (s) in that slot, so the shader can skip the lookup far from a front.
    pub max_delay_s: f32,
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

/// The map's equirectangular projection over a viewport of `size` pixels: longitude -180 deg at
/// the left edge to 180 deg at the right, latitude 90 deg at the top to -90 deg at the bottom.
/// The same mapping as `fs_main` in `map.wgsl`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapProjection {
    pub size: (f32, f32),
}

impl MapProjection {
    /// Viewport pixel (y down) of a point.
    pub fn pixel(&self, lat: f64, lon: f64) -> [f32; 2] {
        [
            ((lon + 180.0) / 360.0) as f32 * self.size.0,
            ((90.0 - lat) / 180.0) as f32 * self.size.1,
        ]
    }

    /// Latitude and longitude (deg) under a viewport pixel, or `None` off the map. The scene
    /// has no barrel distortion yet; when the CRT pass gets one, a pick must undo it before
    /// calling this.
    pub fn lat_lon(&self, px: [f32; 2]) -> Option<(f64, f64)> {
        let (u, v) = (px[0] / self.size.0, px[1] / self.size.1);
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        let lon = f64::from(u) * 360.0 - 180.0;
        let lat = 90.0 - f64::from(v) * 180.0;
        Some((lat, sim::normalise_lon(lon)))
    }
}

/// A seismograph station marked on the map: a triangle for a bundled station, a diamond for a
/// user-placed one; larger and brighter when its trace is shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapMarker {
    pub lat: f32,
    pub lon: f32,
    pub selected: bool,
    pub user: bool,
}

fn marker_vertices(projection: &MapProjection, markers: &[MapMarker]) -> Vec<LineVertex> {
    let mut out = Vec::new();
    for m in markers {
        let [x, y] = projection.pixel(f64::from(m.lat), f64::from(m.lon));
        let (r, intensity) = if m.selected { (6.0, 1.0) } else { (3.5, 0.45) };
        let shape: Vec<[f32; 2]> = if m.user {
            vec![[x, y - r], [x + r, y], [x, y + r], [x - r, y], [x, y - r]]
        } else {
            let h = 0.5 * r;
            vec![
                [x, y - r],
                [x + 0.87 * r, y + h],
                [x - 0.87 * r, y + h],
                [x, y - r],
            ]
        };
        push_polyline(&mut out, &shape, 0.5, intensity, DASH_SOLID);
    }
    out
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
    /// Brightness of the coastlines, [0, 1]; 0 hides them.
    pub coast_brightness: f32,
    /// Wider coastlines, so a line never sits wholly in a scanline gap; off draws them one
    /// pixel wide, hatched by the scanlines like an old screen.
    pub coast_wide: bool,
    /// Strength of the 3D-mantle delays: 0 draws the 1D arrival times, 1 the model's real
    /// strength, larger values exaggerate them linearly.
    pub mantle_strength: f32,
}

impl Default for MapStyle {
    fn default() -> Self {
        Self {
            front_width_px: 2.0,
            trail_s: 40.0,
            trail_strength: 0.35,
            grid_brightness: 0.35,
            coast_brightness: 0.45,
            coast_wide: false,
            mantle_strength: 1.0,
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
    delay_axis: [f32; 4],
    mantle: [f32; 4],
    events: [GpuEvent; MAX_MAP_EVENTS],
}

pub struct MapPass {
    pipeline: wgpu::RenderPipeline,
    markers: LineBatch,
    coast: LineBatch,
    coastlines: Coastlines,
    /// Map size, brightness and half width the coastline vertices were last built for.
    coast_built: Option<((f32, f32), f32, f32)>,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    delay_texture: wgpu::Texture,
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
                gpu::texture_entry(2, wgpu::TextureViewDimension::D2Array),
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
        let delay_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mantle delays"),
            size: wgpu::Extent3d {
                width: DELAY_DIST_COUNT as u32,
                height: DELAY_AZ_COUNT as u32,
                depth_or_array_layers: (MAX_MAP_EVENTS * DELAY_PHASES.len()) as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let delay_view = delay_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("mantle delays"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&delay_view),
                },
            ],
        });

        let (d, x) = (table.depth_axis(), table.distance_axis());
        let layer = |name: &str| table.phase_index(name).map_or(-1.0, |i| i as f32);
        Self {
            pipeline,
            markers: LineBatch::new(device, "map markers"),
            coast: LineBatch::new(device, "map coastlines"),
            coastlines: Coastlines::default(),
            coast_built: None,
            uniform_buffer,
            bind_group,
            delay_texture,
            depth_axis: [d.start as f32, d.step as f32, d.count as f32, 0.0],
            dist_axis: [x.start as f32, x.step as f32, x.count as f32, 0.0],
            layers: [layer("P"), layer("S"), -1.0, -1.0],
        }
    }

    /// Coastlines to draw from the next frame on.
    pub fn set_coastlines(&mut self, coastlines: Coastlines) {
        self.coastlines = coastlines;
        self.coast_built = None;
    }

    /// Store an event's delays in `slot` (< [`MAX_MAP_EVENTS`]); events drawn with that
    /// `delay_slot` use them. The caller assigns slots per event.
    pub fn upload_delays(&self, queue: &wgpu::Queue, slot: u32, grid: &DelayGrid) {
        assert!(
            (slot as usize) < MAX_MAP_EVENTS,
            "delay slot {slot} out of range"
        );
        for (layer, values) in grid.delays.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.delay_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: slot * DELAY_PHASES.len() as u32 + layer as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(values),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * DELAY_DIST_COUNT as u32),
                    rows_per_image: Some(DELAY_AZ_COUNT as u32),
                },
                wgpu::Extent3d {
                    width: DELAY_DIST_COUNT as u32,
                    height: DELAY_AZ_COUNT as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Draw into `viewport` of `target`, clearing the whole target first. At most
    /// [`MAX_MAP_EVENTS`] events are drawn; the caller culls and orders them.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: gpu::Viewport,
        events: &[MapEventInstance],
        style: &MapStyle,
        slice: Option<MapSliceLine>,
        markers: &[MapMarker],
    ) {
        let projection = MapProjection {
            size: (viewport.w, viewport.h),
        };
        self.markers.upload(
            device,
            queue,
            projection.size,
            &marker_vertices(&projection, markers),
        );
        // The coastlines only change with the map's size or their style, so their vertices are
        // rebuilt then.
        let half_w = if style.coast_wide {
            COAST_HALF_WIDTH_WIDE_PX
        } else {
            COAST_HALF_WIDTH_PX
        };
        let coast_key = (projection.size, style.coast_brightness, half_w);
        if self.coast_built != Some(coast_key) {
            let vertices = if style.coast_brightness > 0.0 {
                coast_vertices(
                    &projection,
                    &self.coastlines,
                    half_w,
                    style.coast_brightness,
                )
            } else {
                Vec::new()
            };
            self.coast.upload(device, queue, projection.size, &vertices);
            self.coast_built = Some(coast_key);
        }
        let n = events.len().min(MAX_MAP_EVENTS);
        let mut gpu_events = [GpuEvent::zeroed(); MAX_MAP_EVENTS];
        for (g, e) in gpu_events.iter_mut().zip(&events[..n]) {
            *g = GpuEvent {
                a: [e.lat, e.lon, e.depth_km, e.t],
                b: [
                    e.magnitude,
                    e.delay_slot
                        .map_or(-1.0, |s| (s as usize * DELAY_PHASES.len()) as f32),
                    e.max_delay_s,
                    0.0,
                ],
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
            delay_axis: [
                DELAY_DIST_STEP_DEG as f32,
                DELAY_DIST_COUNT as f32,
                DELAY_AZ_STEP_DEG as f32,
                DELAY_AZ_COUNT as f32,
            ],
            mantle: [style.mantle_strength, 0.0, 0.0, 0.0],
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
        // Max blending: the dim coast never covers a brighter front, trail or marker.
        self.coast.draw(&mut pass);
        self.markers.draw(&mut pass);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_round_trips_and_matches_the_shader() {
        let p = MapProjection {
            size: (1000.0, 500.0),
        };
        assert_eq!(p.pixel(90.0, -180.0), [0.0, 0.0]);
        assert_eq!(p.pixel(0.0, 0.0), [500.0, 250.0]);
        for (lat, lon) in [(34.9, -106.5), (-41.3, 174.7), (0.0, -180.0), (89.0, 0.1)] {
            let (la, lo) = p.lat_lon(p.pixel(lat, lon)).unwrap();
            assert!(
                (la - lat).abs() < 1e-3 && (lo - lon).abs() < 1e-3,
                "{lat},{lon}"
            );
        }
        assert!(p.lat_lon([-1.0, 10.0]).is_none());
        assert!(p.lat_lon([10.0, 501.0]).is_none());
        // The right edge is 180 deg, normalised to -180.
        assert_eq!(p.lat_lon([1000.0, 250.0]).unwrap().1, -180.0);
    }

    #[test]
    fn markers_grow_when_selected() {
        let p = MapProjection {
            size: (360.0, 180.0),
        };
        let m = |selected, user| MapMarker {
            lat: 0.0,
            lon: 0.0,
            selected,
            user,
        };
        let extent =
            |v: Vec<LineVertex>| v.iter().fold(0.0f32, |e, q| e.max((q.pos[1] - 90.0).abs()));
        assert!(
            extent(marker_vertices(&p, &[m(true, false)]))
                > extent(marker_vertices(&p, &[m(false, false)]))
        );
        assert_eq!(marker_vertices(&p, &[m(false, true)]).len(), 4 * 6);
    }
}
