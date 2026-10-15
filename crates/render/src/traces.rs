//! Traces pass: stacked seismograms of the chosen stations, in a column beside the map and the
//! slice. `sim` synthesises the traces; the app squeezes each into one min/max pair per screen
//! column ([`sim::seismogram::Seismogram::columns`]), so this pass only draws lines.

use crate::gpu;
use crate::lines::{DASH_DASHED, DASH_DOTTED, DASH_SOLID, LineBatch, LineVertex, push_polyline};
use crate::theme::Theme;

/// Where the rows sit in a viewport of a given size. Shared with the app, which writes the
/// labels over it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceLayout {
    pub size: (f32, f32),
    pub rows: usize,
    /// Scene pixels per UI point: the text over the rows is sized in points.
    pub px_per_pt: f32,
}

/// Room (UI points) above the rows for the title, below them for the time axis labels, and
/// above each trace for its station label.
const HEADER_PT: f32 = 32.0;
const AXIS_PT: f32 = 22.0;
const LABEL_PT: f32 = 15.0;
const SIDE_PX: f32 = 12.0;

impl TraceLayout {
    pub fn new(size: (f32, f32), rows: usize, px_per_pt: f32) -> Self {
        Self {
            size,
            rows,
            px_per_pt,
        }
    }

    fn header(&self) -> f32 {
        HEADER_PT * self.px_per_pt
    }

    fn label(&self) -> f32 {
        LABEL_PT * self.px_per_pt
    }

    fn axis(&self) -> f32 {
        AXIS_PT * self.px_per_pt
    }

    /// Left and right ends (px) of the time axis.
    pub fn plot_x(&self) -> (f32, f32) {
        (SIDE_PX, (self.size.0 - SIDE_PX).max(SIDE_PX + 1.0))
    }

    /// Screen columns along the time axis: one per pixel.
    pub fn columns(&self) -> usize {
        let (x0, x1) = self.plot_x();
        (x1 - x0).round().max(1.0) as usize
    }

    /// x (px) of a fraction `f` along the time axis.
    pub fn x_at(&self, f: f32) -> f32 {
        let (x0, x1) = self.plot_x();
        x0 + (x1 - x0) * f
    }

    /// Top (px) of row `i`'s station label.
    pub fn label_y(&self, i: usize) -> f32 {
        self.header() + i as f32 * self.row_height()
    }

    /// Baseline (px) and half height (px) of row `i`'s trace band.
    pub fn band(&self, i: usize) -> (f32, f32) {
        let h = self.row_height();
        let half = ((h - self.label()) / 2.0 - 2.0).max(1.0);
        (self.label_y(i) + self.label() + 2.0 + half, half)
    }

    /// y (px) of the time axis under the last row; its labels go below.
    pub fn axis_y(&self) -> f32 {
        self.size.1 - self.axis()
    }

    fn row_height(&self) -> f32 {
        (self.size.1 - self.header() - self.axis()) / self.rows.max(1) as f32
    }
}

/// One station's trace as min/max pairs per column (fractions of the band's half height
/// before `gain`), and its arrival ticks as fractions along the time axis.
#[derive(Debug, Clone, Copy)]
pub struct TraceRow<'a> {
    pub columns: &'a [[f32; 2]],
    pub ticks: &'a [f32],
}

/// Everything the traces pass draws in one frame.
#[derive(Debug, Clone, Copy)]
pub struct TraceScene<'a> {
    pub rows: &'a [TraceRow<'a>],
    /// Amplification of every trace; wiggles beyond the band are clipped.
    pub gain: f32,
    /// Where the pen is (the current time) as a fraction along the time axis.
    pub pen: Option<f32>,
    /// Time-axis grid lines as fractions along the axis.
    pub grid: &'a [f32],
    /// Scene pixels per UI point (see [`TraceLayout`]).
    pub px_per_pt: f32,
}

/// Look of the traces pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceStyle {
    pub trace_width_px: f32,
    pub trace_brightness: f32,
    pub baseline_brightness: f32,
    pub grid_brightness: f32,
}

impl Default for TraceStyle {
    fn default() -> Self {
        Self {
            trace_width_px: 1.0,
            trace_brightness: 0.9,
            baseline_brightness: 0.15,
            grid_brightness: 0.2,
        }
    }
}

fn build_vertices(
    layout: &TraceLayout,
    scene: &TraceScene,
    style: &TraceStyle,
    theme: &Theme,
) -> Vec<LineVertex> {
    let mut out = Vec::new();
    let (x0, x1) = layout.plot_x();
    let top = layout.label_y(0);
    let bottom = layout.axis_y();
    let chrome = theme.chrome;
    // A dim rule down the left edge, separating the column from the map and slice.
    push_polyline(
        &mut out,
        &[[0.5, 0.0], [0.5, layout.size.1]],
        0.5,
        0.3,
        DASH_SOLID,
        chrome,
    );
    for &f in scene.grid {
        let x = layout.x_at(f);
        push_polyline(
            &mut out,
            &[[x, top], [x, bottom]],
            0.5,
            style.grid_brightness,
            DASH_DOTTED,
            chrome,
        );
        push_polyline(
            &mut out,
            &[[x, bottom], [x, bottom + 5.0]],
            0.5,
            0.5,
            DASH_SOLID,
            chrome,
        );
    }
    push_polyline(
        &mut out,
        &[[x0, bottom], [x1, bottom]],
        0.5,
        0.4,
        DASH_SOLID,
        chrome,
    );

    let mut pts: Vec<[f32; 2]> = Vec::new();
    for (i, row) in scene.rows.iter().enumerate() {
        let (base, half) = layout.band(i);
        push_polyline(
            &mut out,
            &[[x0, base], [x1, base]],
            0.5,
            style.baseline_brightness,
            DASH_SOLID,
            chrome,
        );
        for &f in row.ticks {
            let x = layout.x_at(f);
            push_polyline(
                &mut out,
                &[[x, base - half], [x, base - 0.55 * half]],
                0.5,
                0.6,
                DASH_SOLID,
                theme.tick,
            );
        }
        let n = layout.columns();
        let width = (x1 - x0) / n as f32;
        let y = |v: f32| base - (v * scene.gain).clamp(-1.0, 1.0) * half;
        pts.clear();
        for (c, [lo, hi]) in row.columns.iter().enumerate().take(n) {
            let x = x0 + (c as f32 + 0.5) * width;
            // Alternate the order so each column's stroke starts where the last one ended.
            if c % 2 == 0 {
                pts.extend([[x, y(*lo)], [x, y(*hi)]]);
            } else {
                pts.extend([[x, y(*hi)], [x, y(*lo)]]);
            }
        }
        push_polyline(
            &mut out,
            &pts,
            0.5 * style.trace_width_px,
            style.trace_brightness,
            DASH_SOLID,
            theme.trace,
        );
    }
    if let Some(f) = scene.pen.filter(|f| (0.0..=1.0).contains(f)) {
        let x = layout.x_at(f);
        push_polyline(
            &mut out,
            &[[x, top], [x, bottom]],
            0.5,
            0.45,
            DASH_DASHED,
            chrome,
        );
    }
    out
}

pub struct TracePass {
    lines: LineBatch,
}

impl TracePass {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            lines: LineBatch::new(device, "traces"),
        }
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
        scene: &TraceScene,
        style: &TraceStyle,
        theme: &Theme,
    ) {
        let layout = TraceLayout::new((viewport.w, viewport.h), scene.rows.len(), scene.px_per_pt);
        let vertices = build_vertices(&layout, scene, style, theme);
        self.lines.upload(
            device,
            queue,
            (viewport.w, viewport.h),
            theme.background,
            &vertices,
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("traces"),
            color_attachments: &[gpu::color_attachment_load(target)],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        viewport.apply(&mut pass);
        self.lines.draw(&mut pass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_stack_inside_the_viewport_without_overlap() {
        let l = TraceLayout::new((400.0, 900.0), 6, 1.25);
        assert_eq!(l.columns(), 376);
        let mut last_bottom = l.label_y(0);
        for i in 0..6 {
            let (base, half) = l.band(i);
            assert!(
                l.label_y(i) >= last_bottom - 1e-3,
                "row {i} overlaps the one above"
            );
            assert!(base - half > l.label_y(i), "band {i} under its label");
            last_bottom = base + half;
        }
        assert!(last_bottom <= l.axis_y() + 1e-3);
        assert_eq!(l.x_at(0.0), 12.0);
        assert_eq!(l.x_at(1.0), 388.0);
    }

    #[test]
    fn traces_are_clipped_to_their_band() {
        let l = TraceLayout::new((300.0, 300.0), 1, 1.0);
        let cols = [[-5.0f32, 5.0]; 10];
        let rows = [TraceRow {
            columns: &cols,
            ticks: &[],
        }];
        let scene = TraceScene {
            rows: &rows,
            gain: 3.0,
            pen: None,
            grid: &[],
            px_per_pt: 1.0,
        };
        let v = build_vertices(&l, &scene, &TraceStyle::default(), &Theme::crt());
        let (base, half) = l.band(0);
        // Every vertex stays within the band plus the line's soft edge.
        let ext = 0.5 * TraceStyle::default().trace_width_px + 1.0;
        assert!(
            v.iter()
                .all(|q| q.pos[1] >= base - half - ext - 1e-3 || q.pos[0] < 2.5)
        );
    }
}
