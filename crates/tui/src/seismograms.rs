//! The seismograph traces in braille: one row band per shown station, nearest first, drawn
//! up to the clock (the pen has not drawn what is still to come), with arrival marks and a
//! time axis. The traces themselves come from `session::traces`, the same synthesis as the
//! desktop app's.

use session::traces::Trace;
use sim::Station;
use sim::seismogram::Component;

use crate::canvas::Canvas;
use crate::palette::Ink;

/// Width (cells) of the label column left of the traces.
pub const LABEL_W: usize = 14;
/// Phases marked above the traces: the first arrivals a reader looks for.
const MARKED: [&str; 9] = ["P", "Pdiff", "PKPdf", "PP", "S", "SKS", "SS", "R1", "L1"];

/// Everything the traces panel draws.
pub struct TraceScene<'a> {
    pub traces: &'a [&'a Trace],
    pub stations: &'a [Station],
    pub component: Component,
    pub shared_scale: bool,
    /// Window, seconds since the selected event's origin.
    pub from_s: f64,
    pub to_s: f64,
    /// The clock, seconds since the selected event's origin.
    pub t: f64,
    /// The selected event's ray tubes are still being built.
    pub pending: bool,
}

pub fn draw(canvas: &mut Canvas, scene: &TraceScene) {
    if canvas.width <= LABEL_W + 4 || canvas.height < 3 {
        return;
    }
    let component = match scene.component {
        Component::Vertical => "up-down (Z)",
        Component::Transverse => "side-to-side (T)",
    };
    let scaling = if scene.shared_scale {
        "shared scale"
    } else {
        "each row scaled"
    };
    canvas.text(
        0,
        0,
        &format!("SEISMOGRAMS  {component}  {scaling}  ·  synthetic: display-plausible, not waveform-accurate"),
        Ink::Dim,
    );
    let plot_w = canvas.width - LABEL_W;
    let rows_avail = canvas.height - 2;
    if scene.traces.is_empty() {
        let msg = if scene.pending {
            "Tracing rays for this event…"
        } else {
            "No stations chosen: click one on the map, or press d for the default ones."
        };
        canvas.text(LABEL_W, 1 + rows_avail / 2, msg, Ink::Dim);
    }
    let span = scene.to_s - scene.from_s;
    let col_of = |t: f64| ((t - scene.from_s) / span * (2 * plot_w) as f64).floor();

    if !scene.traces.is_empty() {
        let n = scene.traces.len();
        let band = (rows_avail / n).clamp(1, 4);
        let shared = scene
            .traces
            .iter()
            .fold(0.0f32, |m, t| m.max(t.seismogram.peak));
        for (i, trace) in scene.traces.iter().enumerate() {
            let row0 = 1 + i * band;
            if row0 + band > canvas.height - 1 {
                break;
            }
            let station = &scene.stations[trace.station];
            let label = format!("{:<8}{:>4.0}°", station.code, trace.dist_deg);
            canvas.text(0, row0 + band / 2, &label, Ink::Text);
            let scale = if scene.shared_scale {
                shared
            } else {
                trace.seismogram.peak
            };
            let cols =
                trace
                    .seismogram
                    .columns(scene.from_s, scene.to_s, 2 * plot_w, scene.t, scale);
            // With room, the band's top row holds the arrival marks and the trace the rest;
            // otherwise the marks go wherever the trace leaves the top row empty.
            let (trace_row, trace_rows) = if band >= 3 {
                (row0 + 1, band - 1)
            } else {
                (row0, band)
            };
            // Dot rows of the trace: the baseline on a whole dot row in the middle.
            let top = (4 * trace_row) as i64;
            let height = (4 * trace_rows) as i64;
            let mid = (top + height / 2) as f64;
            let half = (height / 2 - 1).max(1) as f64;
            let x0 = (2 * LABEL_W) as i64;
            let mut prev: Option<(f64, f64)> = None;
            for (k, [lo, hi]) in cols.iter().enumerate() {
                let (ylo, yhi) = (
                    mid - f64::from(*hi).clamp(-1.0, 1.0) * half,
                    mid - f64::from(*lo).clamp(-1.0, 1.0) * half,
                );
                // Join to the previous column so steep wiggles stay one line.
                let (a, b) = match prev {
                    Some((p0, p1)) => (ylo.min(p1), yhi.max(p0)),
                    None => (ylo, yhi),
                };
                let x = x0 + k as i64;
                let (ya, yb) = (a.round() as i64, b.round() as i64);
                for y in ya.min(yb)..=ya.max(yb) {
                    canvas.dot(x, y, Ink::Trace);
                }
                prev = Some((ylo, yhi));
            }
            // Arrival marks on the band's top row, where the trace leaves room.
            let mut last_end = 0usize;
            for a in &trace.arrivals {
                // Only each phase's first arrival is named, not its later branches.
                if !MARKED.contains(&a.phase)
                    || a.branch != 0
                    || a.time_s < scene.from_s
                    || a.time_s > scene.to_s
                {
                    continue;
                }
                // A wave this component does not record is not marked on it.
                if a.motion.weight(scene.component) == 0.0 {
                    continue;
                }
                let cx = LABEL_W + (col_of(a.time_s) / 2.0) as usize;
                if cx < last_end || cx + a.phase.len() > canvas.width {
                    continue;
                }
                if canvas.is_clear(cx, row0, a.phase.len()) {
                    canvas.text(cx, row0, a.phase, Ink::Tick);
                    last_end = cx + a.phase.len() + 1;
                }
            }
        }
    }

    // Time axis: a tick and label every ten minutes, and the clock's position.
    let axis = canvas.height - 1;
    canvas.text(LABEL_W.saturating_sub(5), axis, "min", Ink::Dim);
    if (scene.from_s..=scene.to_s).contains(&scene.t) {
        let cx = LABEL_W + (col_of(scene.t) / 2.0) as usize;
        canvas.text(cx.min(canvas.width - 1), axis, "▲", Ink::Accent);
    }
    let step_s = if span > 3.0 * 3600.0 { 1800.0 } else { 600.0 };
    let mut m = (scene.from_s / step_s).ceil() * step_s;
    while m <= scene.to_s {
        let cx = LABEL_W + (col_of(m) / 2.0) as usize;
        // Adding 0 turns -0 into 0.
        let label = format!("{:.0}", m / 60.0 + 0.0);
        // Each label one cell clear of the clock's mark and of the last label.
        if canvas.no_text(cx.saturating_sub(1), axis, label.len() + 2) {
            canvas.text(cx, axis, &label, Ink::Chrome);
        }
        m += step_s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{Look, Palette};

    #[test]
    fn empty_panel_says_why() {
        let mut c = Canvas::new(90, 8, Palette::new(Look::Crt));
        draw(
            &mut c,
            &TraceScene {
                traces: &[],
                stations: &[],
                component: Component::Vertical,
                shared_scale: false,
                from_s: -60.0,
                to_s: 5400.0,
                t: 300.0,
                pending: true,
            },
        );
        let text: String = (0..c.height)
            .flat_map(|y| (0..c.width).map(move |x| (x, y)))
            .map(|(x, y)| c.symbol(x, y))
            .collect();
        assert!(text.contains("Tracing rays"));
        assert!(text.contains("display-plausible"));
        assert!(text.contains("▲"), "clock marker on the axis");
    }
}
