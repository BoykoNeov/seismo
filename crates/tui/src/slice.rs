//! The vertical slice in braille: the half-disc cut along a great circle from the epicentre
//! (left end of the flat edge) to the antipode (right end), with the layer boundaries, ray
//! paths and wavefronts from `sim`'s ray fans. 1D only: the fans are not bent through the 3D
//! mantle here, and the mantle is not shaded by rock speed.

use std::sync::mpsc::{self, Receiver, Sender};

use sim::slice::{Boundary, SliceFans, SlicePoint};
use sim::{EARTH_RADIUS_KM, RayPhase, RayTracer, Wave};

use crate::canvas::Canvas;
use crate::palette::Ink;

/// Ray paths drawn per phase.
const PATHS_PER_PHASE: usize = 4;

/// Builds ray fans (a fraction of a second each in a release build) off the UI thread, keeping
/// only the latest depth asked for.
pub struct SliceWorker {
    jobs: Sender<f64>,
    results: Receiver<SliceFans>,
    asked: Option<u64>,
    ready: Option<(u64, SliceFans)>,
}

impl SliceWorker {
    pub fn spawn(tracer: RayTracer) -> Self {
        let (jobs, rx) = mpsc::channel::<f64>();
        let (tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("slice fans".into())
            .spawn(move || {
                while let Ok(mut depth) = rx.recv() {
                    // Only the newest request matters.
                    while let Ok(d) = rx.try_recv() {
                        depth = d;
                    }
                    // The desktop's default phases: the terminal has no switches for others.
                    let fans = SliceFans::build_for(&tracer, depth, RayPhase::shown_by_default);
                    if tx.send(fans).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn slice worker");
        Self {
            jobs,
            results,
            asked: None,
            ready: None,
        }
    }

    /// The fans for a source at `depth_km`, once built; asks for them if not yet asked.
    pub fn fans(&mut self, depth_km: f64) -> Option<&SliceFans> {
        let key = depth_km.to_bits();
        for fans in self.results.try_iter() {
            self.ready = Some((fans.depth_km.to_bits(), fans));
        }
        if self.ready.as_ref().is_some_and(|(k, _)| *k == key) {
            return self.ready.as_ref().map(|(_, f)| f);
        }
        if self.asked != Some(key) {
            self.asked = Some(key);
            // A send only fails if the worker panicked; the slice then says it is waiting.
            let _ = self.jobs.send(depth_km);
        }
        None
    }
}

/// Where the half-disc sits in a canvas, in dots.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Disc {
    /// Centre of the Earth (dots, y down): the middle of the flat edge.
    pub centre: (f64, f64),
    /// Earth radius in dots.
    pub radius: f64,
}

impl Disc {
    /// The largest half-disc that fits a canvas of `width` × `height` dots with a text row
    /// below the flat edge.
    pub fn fit(width: usize, height: usize) -> Self {
        let edge = height as f64 - 5.5;
        let radius = (width as f64 / 2.0 - 1.0).min(edge - 0.5).max(1.0);
        Self {
            centre: (width as f64 / 2.0, edge),
            radius,
        }
    }

    pub fn project(&self, r_km: f64, delta_deg: f64) -> (f64, f64) {
        let phi = (180.0 - delta_deg).to_radians();
        let s = r_km / EARTH_RADIUS_KM * self.radius;
        (self.centre.0 + s * phi.cos(), self.centre.1 - s * phi.sin())
    }

    fn point(&self, p: &SlicePoint) -> (f64, f64) {
        self.project(p.r_km, p.delta_deg)
    }
}

/// Everything the slice draws.
pub struct SliceScene<'a> {
    pub fans: Option<&'a SliceFans>,
    pub boundaries: &'a [Boundary],
    pub depth_km: f64,
    /// Seconds since the selected event's origin.
    pub t: f64,
    pub azimuth_deg: f64,
}

pub fn draw(canvas: &mut Canvas, scene: &SliceScene) {
    let (w, h) = canvas.dots();
    if w < 8 || h < 12 {
        return;
    }
    let disc = Disc::fit(w, h);
    let radius_of = |name: &str| {
        scene
            .boundaries
            .iter()
            .find(|b| b.name == name)
            .map(|b| EARTH_RADIUS_KM - b.depth_km)
    };
    let (r_cmb, r_icb) = (radius_of("CMB"), radius_of("ICB"));

    // Region fills, per cell centre.
    for cy in 0..canvas.height {
        for cx in 0..canvas.width {
            let (x, y) = (2.0 * cx as f64 + 1.0, 4.0 * cy as f64 + 2.0);
            let (dx, dy) = (x - disc.centre.0, disc.centre.1 - y);
            if dy < -1.0 {
                continue;
            }
            let r = dx.hypot(dy.max(0.0)) / disc.radius * EARTH_RADIUS_KM;
            let ink = match (r_cmb, r_icb) {
                _ if r > EARTH_RADIUS_KM => continue,
                (Some(cmb), _) if r > cmb => Ink::Mantle,
                (_, Some(icb)) if r > icb => Ink::OuterCore,
                (Some(_), Some(_)) => Ink::InnerCore,
                _ => Ink::Mantle,
            };
            canvas.tint(cx, cy, ink, 1.0);
        }
    }

    // Boundaries dotted, the surface and the flat edge solid.
    let arc = |r_km: f64| -> Vec<(f64, f64)> {
        let n = (disc.radius * 4.0).ceil().max(8.0) as usize;
        (0..=n)
            .map(|k| disc.project(r_km, 180.0 * k as f64 / n as f64))
            .collect()
    };
    for b in scene.boundaries {
        let pts = arc(EARTH_RADIUS_KM - b.depth_km);
        for (k, seg) in pts.windows(2).enumerate() {
            if k % 2 == 0 {
                canvas.line(seg[0], seg[1], Ink::Boundary);
            }
        }
    }
    canvas.polyline(&arc(EARTH_RADIUS_KM), Ink::Chrome);
    let (cx0, cy0) = disc.centre;
    canvas.line(
        (cx0 - disc.radius, cy0),
        (cx0 + disc.radius, cy0),
        Ink::Chrome,
    );

    // Boundary names under the arc's top, where they do not crowd each other;
    // before the fronts, so the phase labels keep clear of them.
    for b in scene.boundaries {
        let (x, y) = disc.project(EARTH_RADIUS_KM - b.depth_km, 90.0);
        let (cx, cy) = ((x / 2.0) as usize + 1, (y / 4.0) as usize + 1);
        if canvas.no_text(cx, cy, b.name.len()) && canvas.no_text(cx, cy.saturating_sub(1), 1) {
            canvas.text(cx, cy, b.name, Ink::Dim);
        }
    }

    match scene.fans {
        Some(fans) => {
            for line in fans.ray_paths_until(scene.t, PATHS_PER_PHASE) {
                let pts: Vec<(f64, f64)> = line.points.iter().map(|p| disc.point(p)).collect();
                canvas.polyline(&pts, Ink::RayPath);
            }
            let fronts = fans.fronts_at(scene.t);
            for line in &fronts.lines {
                let ink = wave_ink(line.wave);
                let pts: Vec<(f64, f64)> = line.points.iter().map(|p| disc.point(p)).collect();
                canvas.polyline(&pts, ink);
            }
            for label in &fronts.labels {
                let (x, y) = disc.point(&label.at);
                let name = label.phase.name();
                let (cx, cy) = ((x / 2.0) as usize + 1, (y / 4.0) as usize);
                if canvas.no_text(cx, cy, name.len()) {
                    canvas.text(cx, cy, name, wave_ink(label.phase.arrival_wave()));
                }
            }
        }
        None => {
            let msg = "tracing rays…";
            let (x, y) = disc.project(0.5 * EARTH_RADIUS_KM, 90.0);
            let cx = ((x / 2.0) as usize).saturating_sub(msg.chars().count() / 2);
            canvas.text(cx, (y / 4.0) as usize, msg, Ink::Dim);
        }
    }

    let (sx, sy) = disc.project(EARTH_RADIUS_KM - scene.depth_km, 0.0);
    canvas.text(
        (sx / 2.0) as usize,
        (sy / 4.0) as usize,
        "*",
        Ink::Epicentre,
    );

    let row = canvas.height - 1;
    let left = ((cx0 - disc.radius) / 2.0) as usize;
    let right = ((cx0 + disc.radius) / 2.0) as usize;
    canvas.text(left, row, "epicentre", Ink::Dim);
    let anti = "antipode";
    canvas.text(right.saturating_sub(anti.len() - 1), row, anti, Ink::Dim);
    let mid = format!("cut toward {:.0}°", scene.azimuth_deg);
    canvas.text(
        ((cx0 / 2.0) as usize).saturating_sub(mid.chars().count() / 2),
        row,
        &mid,
        Ink::Dim,
    );
}

fn wave_ink(wave: Wave) -> Ink {
    match wave {
        Wave::P => Ink::P,
        Wave::S => Ink::S,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{Look, Palette};
    use session::assets;
    use sim::Simulation;

    #[test]
    fn disc_fits_and_projects() {
        let d = Disc::fit(200, 60);
        assert!(d.radius > 50.0 && d.radius <= 99.0);
        let (x, y) = d.project(EARTH_RADIUS_KM, 0.0);
        assert!((x - (d.centre.0 - d.radius)).abs() < 1e-9 && (y - d.centre.1).abs() < 1e-9);
        let (x, y) = d.project(EARTH_RADIUS_KM, 90.0);
        assert!((x - d.centre.0).abs() < 1e-9 && (y - (d.centre.1 - d.radius)).abs() < 1e-9);
        // The top of the disc stays on the canvas.
        assert!(d.centre.1 - d.radius >= 0.0);
    }

    #[test]
    fn draws_fronts_and_labels() {
        let (table, model, dispersion) = assets::earth().unwrap();
        let sim = Simulation::new(table, model, dispersion, vec![assets::tohoku()]).unwrap();
        let fans = sim.slice_fans(29.0, RayPhase::shown_by_default);
        let mut canvas = Canvas::new(100, 26, Palette::new(Look::Crt));
        draw(
            &mut canvas,
            &SliceScene {
                fans: Some(&fans),
                boundaries: sim.boundaries(),
                depth_km: 29.0,
                t: 600.0,
                azimuth_deg: 90.0,
            },
        );
        let (w, h) = canvas.dots();
        let count = |ink: Ink| {
            (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|&(x, y)| canvas.is_lit(x, y) && canvas.ink_at_dot(x, y) == Some(ink))
                .count()
        };
        assert!(count(Ink::P) > 20 && count(Ink::S) > 20);
        let text: String = (0..canvas.height)
            .flat_map(|y| (0..canvas.width).map(move |x| (x, y)))
            .map(|(x, y)| canvas.symbol(x, y))
            .collect();
        for word in ["epicentre", "antipode", "CMB", "ICB"] {
            assert!(text.contains(word), "{word} missing");
        }
    }
}
