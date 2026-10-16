//! The beach ball: a fault's P-wave pattern drawn as seismologists draw it, the lower half of
//! a sphere round the quake seen from above (equal-area, north up), filled where the P wave
//! leaves as a push and empty where it leaves as a pull.

use eframe::egui;
use sim::FocalMechanism;

/// Cells across the ball.
const CELLS: usize = 64;

/// A filled stretch of one row: row, first cell, last cell.
type Run = (usize, usize, usize);

/// The filled stretches of each row of a ball, kept while the fault stays the same.
#[derive(Default)]
pub struct BeachBall {
    drawn: Option<(FocalMechanism, Vec<Run>)>,
}

impl BeachBall {
    /// Filled runs (row, first cell, last cell) of `fault`'s ball, top row (north) first.
    fn runs(&mut self, fault: FocalMechanism) -> &[Run] {
        if self.drawn.as_ref().is_none_or(|(f, _)| *f != fault) {
            let at = |k: usize| (k as f64 + 0.5) / CELLS as f64 * 2.0 - 1.0;
            let mut runs = Vec::new();
            for row in 0..CELLS {
                let north = -at(row);
                let mut start = None;
                for col in 0..=CELLS {
                    let filled =
                        col < CELLS && fault.compressional_at(at(col), north) == Some(true);
                    match (filled, start) {
                        (true, None) => start = Some(col),
                        (false, Some(s)) => {
                            runs.push((row, s, col - 1));
                            start = None;
                        }
                        _ => {}
                    }
                }
            }
            self.drawn = Some((fault, runs));
        }
        &self.drawn.as_ref().expect("just drawn").1
    }

    /// Draw `fault`'s ball, `size` points across, in `colour`.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        fault: FocalMechanism,
        size: f32,
        colour: egui::Color32,
    ) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        let cell = size / CELLS as f32;
        let painter = ui.painter_at(rect);
        // One mesh, no edge feathering: neighbouring rows join without hairline gaps.
        let mut mesh = egui::Mesh::default();
        for &(row, c0, c1) in self.runs(fault) {
            let min = rect.min + egui::vec2(c0 as f32 * cell, row as f32 * cell);
            let max = rect.min + egui::vec2((c1 + 1) as f32 * cell, (row + 1) as f32 * cell);
            mesh.add_colored_rect(egui::Rect::from_min_max(min, max), colour);
        }
        painter.add(egui::Shape::mesh(mesh));
        painter.circle_stroke(
            rect.center(),
            size / 2.0 - 0.5,
            egui::Stroke::new(1.0, colour),
        );
        response
    }
}
