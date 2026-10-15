//! `render`: wgpu passes (map, slice, traces, post-process).
//! Allowed dependencies: `sim` (tables via `sim::tables`), wgpu, bytemuck. No egui, no I/O.
//!
//! Colours are display-space (gamma-encoded) values written to non-sRGB targets, so
//! `#33ff33` lands on screen as `#33ff33`. Which colours: a [`Theme`], one per [`Look`].

mod coast;
mod crt;
mod gpu;
mod lines;
mod map;
mod slice;
mod theme;
mod traces;

pub use crt::{CrtPass, CrtSettings, FrameTime};
pub use gpu::Viewport;
pub use map::{
    MAX_MAP_EVENTS, MapEventInstance, MapMarker, MapPass, MapProjection, MapSliceLine, MapStyle,
};
pub use sim::coast::{CoastError, Coastlines};
pub use slice::{
    MAX_SLICE_BOUNDARIES, SliceLayer, SlicePass, SliceScene, SliceStyle, SliceView, StrokeKind,
    phase_gain,
};
pub use theme::{Look, Rgb, Theme};
pub use traces::{TraceLayout, TracePass, TraceRow, TraceScene, TraceStyle};

/// Phosphor green `#33ff33`, display space.
pub const PHOSPHOR: [f32; 3] = [0x33 as f32 / 255.0, 1.0, 0x33 as f32 / 255.0];
/// Background `#050805`, display space.
pub const BACKGROUND: [f32; 3] = [5.0 / 255.0, 8.0 / 255.0, 5.0 / 255.0];

/// Format of the scene and persistence textures (HDR, so bright lines can exceed 1 before bloom).
pub const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Format of the displayed texture; egui-wgpu requires Rgba8Unorm for native textures.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The scene: the map (2:1) in the top half and the slice (2:1 half-disc) in the bottom half
/// of a square-ish block on the left, the seismogram traces in a column on the right, then one
/// CRT pass over the whole of it.
pub struct SceneRenderer {
    pub map: MapPass,
    pub slice: SlicePass,
    pub traces: TracePass,
    pub crt: CrtPass,
    size: (u32, u32),
    /// Width (px) of the traces column; 0 hides it.
    traces_width: u32,
    scene: gpu::Target,
}

impl SceneRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        table: &sim::tables::TravelTimeTable,
        size: (u32, u32),
    ) -> Self {
        let size = (size.0.max(1), size.1.max(1));
        let map = MapPass::new(device, queue, table);
        let slice = SlicePass::new(device);
        let traces = TracePass::new(device);
        let scene = gpu::Target::new(device, "scene", size, SCENE_FORMAT);
        let crt = CrtPass::new(device, size, &scene.view);
        Self {
            map,
            slice,
            traces,
            crt,
            size,
            traces_width: 0,
            scene,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Width (px) of the map and slice block, left of the traces column.
    fn main_width(&self) -> f32 {
        self.size.0.saturating_sub(self.traces_width).max(1) as f32
    }

    /// Where the map is drawn in the scene (pixels): the top half of the left block.
    pub fn map_viewport(&self) -> Viewport {
        let (w, h) = (self.main_width(), self.size.1 as f32);
        Viewport {
            x: 0.0,
            y: 0.0,
            w,
            h: (h / 2.0).floor().max(1.0),
        }
    }

    /// Where the slice is drawn in the scene (pixels): the bottom half of the left block.
    pub fn slice_viewport(&self) -> Viewport {
        let (w, h) = (self.main_width(), self.size.1 as f32);
        let top = (h / 2.0).floor();
        Viewport {
            x: 0.0,
            y: top,
            w,
            h: (h - top).max(1.0),
        }
    }

    /// Where the traces are drawn in the scene (pixels): the right-hand column, `None` when it
    /// has no width.
    pub fn traces_viewport(&self) -> Option<Viewport> {
        (self.traces_width > 0).then(|| Viewport {
            x: self.main_width(),
            y: 0.0,
            w: self.traces_width as f32,
            h: self.size.1 as f32,
        })
    }

    /// Recreate the targets for a new size, with a traces column `traces_width` pixels wide on
    /// the right (0 for none); returns true if the targets changed (the output view is then new
    /// and must be re-registered with the UI).
    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32), traces_width: u32) -> bool {
        let size = (size.0.max(1), size.1.max(1));
        self.traces_width = traces_width.min(size.0.saturating_sub(1));
        if size == self.size {
            return false;
        }
        self.size = size;
        self.scene = gpu::Target::new(device, "scene", size, SCENE_FORMAT);
        self.crt.resize(device, size, &self.scene.view);
        true
    }

    /// The displayed image (Rgba8Unorm).
    pub fn output_view(&self) -> &wgpu::TextureView {
        self.crt.output_view()
    }

    /// The displayed image as tightly packed RGBA8 rows (blocking; for screenshots).
    pub fn read_output(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
        self.crt.read_output(device, queue)
    }

    /// Encode one frame: map, slice and traces into the scene texture in the theme's colours,
    /// then the post-process into the output.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        theme: &Theme,
        events: &[MapEventInstance],
        style: &MapStyle,
        slice_line: Option<MapSliceLine>,
        markers: &[MapMarker],
        slice: &SliceScene,
        slice_style: &SliceStyle,
        traces: &TraceScene,
        trace_style: &TraceStyle,
        crt: &CrtSettings,
        time: FrameTime,
    ) {
        let (map_vp, slice_vp) = (self.map_viewport(), self.slice_viewport());
        self.map.draw(
            device,
            queue,
            encoder,
            &self.scene.view,
            map_vp,
            events,
            style,
            theme,
            slice_line,
            markers,
        );
        self.slice.draw(
            device,
            queue,
            encoder,
            &self.scene.view,
            slice_vp,
            slice,
            slice_style,
            theme,
        );
        if let Some(vp) = self.traces_viewport() {
            self.traces.draw(
                device,
                queue,
                encoder,
                &self.scene.view,
                vp,
                traces,
                trace_style,
                theme,
            );
        }
        self.crt
            .draw(queue, encoder, self.size, crt, theme.p_front, time);
    }

    /// The same frame as [`Self::render`], but each pass submitted on its own and waited for,
    /// returning for map, slice, traces and post-process in that order the real time (ms) spent
    /// encoding on the CPU (building lines, writing buffers) and then waiting for the GPU. For
    /// `--bench` only: the waits stall the GPU, so this is slower than a normal frame, and each
    /// wait includes one submit.
    #[allow(clippy::too_many_arguments)]
    pub fn render_timed(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        theme: &Theme,
        events: &[MapEventInstance],
        style: &MapStyle,
        slice_line: Option<MapSliceLine>,
        markers: &[MapMarker],
        slice: &SliceScene,
        slice_style: &SliceStyle,
        traces: &TraceScene,
        trace_style: &TraceStyle,
        crt: &CrtSettings,
        time: FrameTime,
    ) -> [[f64; 2]; 4] {
        let timed = |encode: &mut dyn FnMut(&mut wgpu::CommandEncoder)| {
            let t0 = std::time::Instant::now();
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("timed pass"),
            });
            encode(&mut encoder);
            let cb = encoder.finish();
            let t1 = std::time::Instant::now();
            queue.submit([cb]);
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll device");
            [
                (t1 - t0).as_secs_f64() * 1e3,
                t1.elapsed().as_secs_f64() * 1e3,
            ]
        };
        let (map_vp, slice_vp, traces_vp) = (
            self.map_viewport(),
            self.slice_viewport(),
            self.traces_viewport(),
        );
        let size = self.size;
        let view = &self.scene.view;
        let map = timed(&mut |enc| {
            self.map.draw(
                device, queue, enc, view, map_vp, events, style, theme, slice_line, markers,
            )
        });
        let slice_ms = timed(&mut |enc| {
            self.slice.draw(
                device,
                queue,
                enc,
                view,
                slice_vp,
                slice,
                slice_style,
                theme,
            )
        });
        let traces_ms = timed(&mut |enc| {
            if let Some(vp) = traces_vp {
                self.traces
                    .draw(device, queue, enc, view, vp, traces, trace_style, theme);
            }
        });
        let post = timed(&mut |enc| self.crt.draw(queue, enc, size, crt, theme.p_front, time));
        [map, slice_ms, traces_ms, post]
    }
}
