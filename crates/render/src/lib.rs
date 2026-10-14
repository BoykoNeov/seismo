//! `render`: wgpu passes (map, slice, CRT post-process; later traces).
//! Allowed dependencies: `sim` (tables via `sim::tables`), wgpu, bytemuck. No egui, no I/O.
//!
//! Colours are display-space (gamma-encoded) values written to non-sRGB targets, so
//! `#33ff33` lands on screen as `#33ff33`.

mod crt;
mod gpu;
mod map;
mod slice;

pub use crt::{CrtPass, CrtSettings, FrameTime};
pub use gpu::Viewport;
pub use map::{MAX_MAP_EVENTS, MapEventInstance, MapPass, MapSliceLine, MapStyle};
pub use slice::{
    MAX_SLICE_BOUNDARIES, SliceLayer, SlicePass, SliceScene, SliceStyle, SliceView, StrokeKind,
    phase_gain,
};

/// Phosphor green `#33ff33`, display space.
pub const PHOSPHOR: [f32; 3] = [0x33 as f32 / 255.0, 1.0, 0x33 as f32 / 255.0];
/// Background `#050805`, display space.
pub const BACKGROUND: [f32; 3] = [5.0 / 255.0, 8.0 / 255.0, 5.0 / 255.0];

/// Format of the scene and persistence textures (HDR so bloom can use values > 1 later).
pub const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Format of the displayed texture; egui-wgpu requires Rgba8Unorm for native textures.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The scene: the map (2:1) in the top half and the slice (2:1 half-disc) in the bottom half
/// of one square-ish target, then one CRT pass over the whole of it.
pub struct SceneRenderer {
    pub map: MapPass,
    pub slice: SlicePass,
    pub crt: CrtPass,
    size: (u32, u32),
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
        let scene = gpu::Target::new(device, "scene", size, SCENE_FORMAT);
        let crt = CrtPass::new(device, size, &scene.view);
        Self {
            map,
            slice,
            crt,
            size,
            scene,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Where the map is drawn in the scene (pixels): the top half.
    pub fn map_viewport(&self) -> Viewport {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        Viewport {
            x: 0.0,
            y: 0.0,
            w,
            h: (h / 2.0).floor().max(1.0),
        }
    }

    /// Where the slice is drawn in the scene (pixels): the bottom half.
    pub fn slice_viewport(&self) -> Viewport {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let top = (h / 2.0).floor();
        Viewport {
            x: 0.0,
            y: top,
            w,
            h: (h - top).max(1.0),
        }
    }

    /// Recreate the targets for a new size; returns true if anything changed (the output view
    /// is then new and must be re-registered with the UI).
    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) -> bool {
        let size = (size.0.max(1), size.1.max(1));
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

    /// Encode one frame: map and slice into the scene texture, then CRT into the output.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        events: &[MapEventInstance],
        style: &MapStyle,
        slice_line: Option<MapSliceLine>,
        slice: &SliceScene,
        slice_style: &SliceStyle,
        crt: &CrtSettings,
        time: FrameTime,
    ) {
        let (map_vp, slice_vp) = (self.map_viewport(), self.slice_viewport());
        self.map.draw(
            queue,
            encoder,
            &self.scene.view,
            map_vp,
            events,
            style,
            slice_line,
        );
        self.slice.draw(
            device,
            queue,
            encoder,
            &self.scene.view,
            slice_vp,
            slice,
            slice_style,
        );
        self.crt.draw(queue, encoder, self.size, crt, time);
    }
}
