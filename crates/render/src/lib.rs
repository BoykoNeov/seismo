//! `render`: wgpu passes (map, CRT post-process; later slice and traces).
//! Allowed dependencies: `sim` (tables via `sim::tables`), wgpu, bytemuck. No egui, no I/O.
//!
//! Colours are display-space (gamma-encoded) values written to non-sRGB targets, so
//! `#33ff33` lands on screen as `#33ff33`.

mod crt;
mod gpu;
mod map;

pub use crt::{CrtPass, CrtSettings, FrameTime};
pub use map::{MAX_MAP_EVENTS, MapEventInstance, MapPass, MapStyle};

/// Phosphor green `#33ff33`, display space.
pub const PHOSPHOR: [f32; 3] = [0x33 as f32 / 255.0, 1.0, 0x33 as f32 / 255.0];
/// Background `#050805`, display space.
pub const BACKGROUND: [f32; 3] = [5.0 / 255.0, 8.0 / 255.0, 5.0 / 255.0];

/// Format of the scene and persistence textures (HDR so bloom can use values > 1 later).
pub const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Format of the displayed texture; egui-wgpu requires Rgba8Unorm for native textures.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Map pass + CRT pass with their offscreen targets, sized together.
pub struct SceneRenderer {
    pub map: MapPass,
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
        let scene = gpu::Target::new(device, "scene", size, SCENE_FORMAT);
        let crt = CrtPass::new(device, size, &scene.view);
        Self {
            map,
            crt,
            size,
            scene,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
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

    /// Encode one frame: map into the scene texture, then CRT into the output.
    pub fn render(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        events: &[MapEventInstance],
        style: &MapStyle,
        crt: &CrtSettings,
        time: FrameTime,
    ) {
        self.map
            .draw(queue, encoder, &self.scene.view, self.size, events, style);
        self.crt.draw(queue, encoder, self.size, crt, time);
    }
}
