//! The world map in braille: coastlines, each event's P and S fronts, surface-wave bands,
//! epicentres, stations and the slice's great circle. Equirectangular, like the desktop map.
//!
//! The fronts come from the travel-time table (sampled along distance once per event and
//! frame, then looked up per dot), the bands from `sim::surface`'s map grid: the same sources
//! the desktop map's shader reads. 1D only: the 3D-mantle nudge is not applied here.

use chrono::{DateTime, Utc};
use sim::coast::Coastlines;
use sim::surface::SurfaceMapGrid;
use sim::{Simulation, Station, destination, great_circle_deg};

use crate::canvas::Canvas;
use crate::palette::Ink;

/// Most events drawn at once (the desktop map draws 32; a terminal has far fewer dots).
pub const MAP_EVENT_CAP: usize = 12;
/// Seconds a trail fades over (the desktop map's default), and how bright it starts: dimmer
/// than the desktop map's 0.35, as a terminal cell is several degrees wide and a trail fills it.
const TRAIL_S: f64 = 40.0;
const TRAIL_STRENGTH: f64 = 0.25;
/// Ray parameters below this (s/deg) are taken as this when turning time into distance, as
/// the desktop map does: p reaches 0 at the epicentre of a buried source.
const MIN_RAY_PARAM: f64 = 0.5;
/// Half the width of a front, in dots.
const FRONT_HALF_DOTS: f64 = 0.6;
/// Surface-wave glow brightness (the desktop map's default).
const SURFACE_BRIGHTNESS: f64 = 0.45;
/// Love-wave dots show where the band is at least this bright.
const LOVE_DOT_LEVEL: f64 = 0.2;

/// Map dots to and from latitude and longitude.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    /// Size in dots.
    pub width: usize,
    pub height: usize,
}

impl Projection {
    /// Dot coordinates (x right, y down) of a point.
    pub fn dot(&self, lat: f64, lon: f64) -> (f64, f64) {
        (
            (lon + 180.0) / 360.0 * self.width as f64,
            (90.0 - lat) / 180.0 * self.height as f64,
        )
    }

    /// Latitude and longitude of the centre of dot coordinates (`x`, `y`).
    pub fn lat_lon(&self, x: f64, y: f64) -> (f64, f64) {
        (
            90.0 - y / self.height as f64 * 180.0,
            sim::normalise_lon(x / self.width as f64 * 360.0 - 180.0),
        )
    }

    /// Size of a dot in degrees of latitude.
    fn dot_deg(&self) -> f64 {
        180.0 / self.height as f64
    }
}

/// One phase's arrival time and ray parameter along distance, for one source depth.
struct Front {
    step: f64,
    nodes: Vec<Option<(f64, f64)>>,
}

impl Front {
    fn new(sim: &Simulation, phase: &str, depth_km: f64) -> Self {
        let table = sim.table();
        let axis = table.distance_axis();
        let nodes = (0..axis.count)
            .map(|j| {
                let d = axis.value(j);
                table
                    .arrival(phase, depth_km, d)
                    .zip(table.ray_param(phase, depth_km, d))
            })
            .collect();
        Self {
            step: axis.step,
            nodes,
        }
    }

    /// Arrival time and ray parameter at `dist_deg`, linear between the table's distances
    /// (which is exactly the table's bilinear lookup at this depth); `None` where either
    /// neighbour has no arrival.
    fn at(&self, dist_deg: f64) -> Option<(f64, f64)> {
        let f = dist_deg / self.step;
        let i = (f.floor() as usize).min(self.nodes.len().saturating_sub(2));
        let (a, b) = (
            self.nodes.get(i)?.as_ref()?,
            self.nodes.get(i + 1)?.as_ref()?,
        );
        let w = (f - i as f64).clamp(0.0, 1.0);
        Some((a.0 + (b.0 - a.0) * w, a.1 + (b.1 - a.1) * w))
    }
}

/// What the map shows besides the events.
pub struct MapScene<'a> {
    pub sim: &'a Simulation,
    pub now: DateTime<Utc>,
    /// Index into `sim.events` of the selected event.
    pub selected: usize,
    pub stations: &'a [Station],
    /// Indices into `stations` of those shown as traces.
    pub chosen: &'a [usize],
    /// Draw the slice's great circle from the selected event at this azimuth.
    pub slice_azimuth: Option<f64>,
}

/// Coastline dots for one map size (in dots).
type CoastDots = ((usize, usize), Vec<(i64, i64)>);

/// The map's fixed inputs: coastlines (rasterised once per size) and the surface-wave grid.
pub struct MapView {
    coast: Coastlines,
    coast_dots: Option<CoastDots>,
    surface: SurfaceMapGrid,
}

impl MapView {
    pub fn new(coast: Coastlines, surface: SurfaceMapGrid) -> Self {
        Self {
            coast,
            coast_dots: None,
            surface,
        }
    }

    fn coast_dots(&mut self, proj: Projection) -> &[(i64, i64)] {
        let size = (proj.width, proj.height);
        if self.coast_dots.as_ref().is_none_or(|(s, _)| *s != size) {
            let mut c = Canvas::new(
                size.0.div_ceil(2),
                size.1.div_ceil(4),
                crate::palette::Palette::new(crate::palette::Look::Crt),
            );
            for line in self.coast.lines() {
                let pts: Vec<(f64, f64)> = line
                    .iter()
                    .map(|&[lat, lon]| proj.dot(f64::from(lat), f64::from(lon)))
                    .collect();
                for (k, w) in pts.windows(2).enumerate() {
                    // A step across the antimeridian would run across the whole map.
                    if (line[k + 1][1] - line[k][1]).abs() > 180.0 {
                        continue;
                    }
                    c.line(w[0], w[1], Ink::Coast);
                }
            }
            let mut dots = Vec::new();
            for y in 0..size.1 {
                for x in 0..size.0 {
                    if c.is_lit(x, y) {
                        dots.push((x as i64, y as i64));
                    }
                }
            }
            self.coast_dots = Some((size, dots));
        }
        &self.coast_dots.as_ref().expect("just built").1
    }

    /// Draw the map filling `canvas`.
    pub fn draw(&mut self, canvas: &mut Canvas, scene: &MapScene) {
        let (w, h) = canvas.dots();
        if w == 0 || h == 0 {
            return;
        }
        let proj = Projection {
            width: w,
            height: h,
        };
        for &(x, y) in self.coast_dots(proj) {
            canvas.dot(x, y, Ink::Coast);
        }
        let sim = scene.sim;
        let map_events = sim.map_events(scene.now, MAP_EVENT_CAP, 5.0 * TRAIL_S);
        let dot_deg = proj.dot_deg();
        for m in &map_events {
            let e = &sim.events[m.index];
            let fronts = m.body.then(|| {
                (
                    Front::new(sim, "P", e.depth_km),
                    Front::new(sim, "S", e.depth_km),
                )
            });
            let t = m.t;
            // Per cell: surface-wave bands and trails, sampled at the cell's centre.
            for cy in 0..canvas.height {
                for cx in 0..canvas.width {
                    let (lat, lon) = proj.lat_lon(2.0 * cx as f64 + 1.0, 4.0 * cy as f64 + 2.0);
                    let dist = great_circle_deg(e.lat, e.lon, lat, lon);
                    if m.surface {
                        let band = |layer: usize| {
                            let a = self.surface.sample(layer, dist, t);
                            let b = self.surface.sample(layer, 360.0 - dist, t);
                            (a * a).max(0.6 * b * b)
                        };
                        let r = band(0);
                        canvas.tint(cx, cy, Ink::Rayleigh, (SURFACE_BRIGHTNESS * r) as f32);
                        if band(1) >= LOVE_DOT_LEVEL {
                            canvas.dot(2 * cx as i64, 4 * cy as i64 + 1, Ink::Love);
                        }
                    }
                    if let Some((p, s)) = &fronts {
                        for (front, ink) in [(p, Ink::P), (s, Ink::S)] {
                            if let Some((arrival, _)) = front.at(dist) {
                                let dt = t - arrival;
                                if dt > 0.0 {
                                    let level = TRAIL_STRENGTH * (-dt / TRAIL_S).exp();
                                    canvas.tint(cx, cy, ink, level as f32);
                                }
                            }
                        }
                    }
                }
            }
            // Per dot: the fronts themselves.
            if let Some((p, s)) = &fronts {
                let half = FRONT_HALF_DOTS * dot_deg;
                let dash = 6.0 * dot_deg;
                for y in 0..h {
                    for x in 0..w {
                        let (lat, lon) = proj.lat_lon(x as f64 + 0.5, y as f64 + 0.5);
                        let dist = great_circle_deg(e.lat, e.lon, lat, lon);
                        let on = |front: &Front| {
                            front.at(dist).is_some_and(|(arrival, rp)| {
                                ((t - arrival) / rp.max(MIN_RAY_PARAM)).abs() <= half
                            })
                        };
                        if on(p) {
                            canvas.dot(x as i64, y as i64, Ink::P);
                        } else if on(s) {
                            // S dashed along its ring, as on the desktop map.
                            let az = sim::azimuth_deg(e.lat, e.lon, lat, lon);
                            let along = az * dist.to_radians().sin();
                            if (along / dash).rem_euclid(1.0) < 0.6 {
                                canvas.dot(x as i64, y as i64, Ink::S);
                            }
                        }
                    }
                }
            }
        }

        if let Some(az) = scene.slice_azimuth {
            let e = &sim.events[scene.selected];
            let step = 0.5 * dot_deg;
            let n = (180.0 / step) as usize;
            for k in 0..=n {
                let d = k as f64 * step;
                if ((d / (2.0 * dot_deg)) as usize).is_multiple_of(2) {
                    let (lat, lon) = destination(e.lat, e.lon, az, d);
                    let (x, y) = proj.dot(lat, lon);
                    canvas.dot(x as i64, y as i64, Ink::Circle);
                }
            }
        }

        for (i, s) in scene.stations.iter().enumerate() {
            let (x, y) = proj.dot(s.lat, s.lon);
            if scene.chosen.contains(&i) {
                let (cx, cy) = ((x / 2.0) as usize, (y / 4.0) as usize);
                canvas.text(cx, cy, "▲", Ink::StationShown);
                let code = &s.code;
                if canvas.no_text(cx + 1, cy, code.chars().count()) {
                    canvas.text(cx + 1, cy, code, Ink::Dim);
                }
            } else {
                canvas.dot(x as i64, y as i64, Ink::Station);
            }
        }

        for m in &map_events {
            if m.index != scene.selected {
                let e = &sim.events[m.index];
                let (x, y) = proj.dot(e.lat, e.lon);
                canvas.text((x / 2.0) as usize, (y / 4.0) as usize, "+", Ink::OtherEvent);
            }
        }
        if let Some(e) = sim.events.get(scene.selected) {
            let (x, y) = proj.dot(e.lat, e.lon);
            canvas.text((x / 2.0) as usize, (y / 4.0) as usize, "*", Ink::Epicentre);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{Look, Palette};
    use chrono::Duration;
    use session::assets;

    fn sim() -> Simulation {
        let (table, model, dispersion) = assets::earth().unwrap();
        Simulation::new(table, model, dispersion, vec![assets::tohoku()]).unwrap()
    }

    #[test]
    fn projection_round_trips() {
        let p = Projection {
            width: 240,
            height: 120,
        };
        for (lat, lon) in [(0.0, 0.0), (38.3, 142.4), (-60.0, -170.0), (80.0, 179.0)] {
            let (x, y) = p.dot(lat, lon);
            let (lat2, lon2) = p.lat_lon(x, y);
            assert!((lat - lat2).abs() < 1e-9 && (lon - lon2).abs() < 1e-9);
        }
        assert_eq!(p.dot(90.0, -180.0), (0.0, 0.0));
    }

    #[test]
    fn front_lookup_matches_the_table() {
        let sim = sim();
        let f = Front::new(&sim, "P", 29.0);
        for d in [10.0, 30.1, 47.6, 90.0] {
            let (t, p) = f.at(d).unwrap();
            assert!(
                (t - sim.table().arrival("P", 29.0, d).unwrap()).abs() < 1e-3,
                "{d}"
            );
            assert!(
                (p - sim.table().ray_param("P", 29.0, d).unwrap()).abs() < 1e-3,
                "{d}"
            );
        }
        // Direct P is gone in the core shadow.
        assert!(f.at(120.0).is_none());
    }

    /// The P front drawn on the map lies where the simulation says the P front is.
    #[test]
    fn p_ring_sits_at_the_simulated_front() {
        let sim = sim();
        let e = sim.events[0].clone();
        let now = e.origin + Duration::seconds(400);
        let front = sim.front_distance("P", e.depth_km, 400.0).unwrap();
        let mut view = MapView::new(Coastlines::default(), sim.surface_waves().map_grid());
        let mut canvas = Canvas::new(180, 45, Palette::new(Look::Crt));
        view.draw(
            &mut canvas,
            &MapScene {
                sim: &sim,
                now,
                selected: 0,
                stations: &[],
                chosen: &[],
                slice_azimuth: None,
            },
        );
        let proj = Projection {
            width: 360,
            height: 180,
        };
        let mut n = 0;
        for y in 0..180 {
            for x in 0..360 {
                if canvas.is_lit(x, y) && canvas.ink_at_dot(x, y) == Some(Ink::P) {
                    let (lat, lon) = proj.lat_lon(x as f64 + 0.5, y as f64 + 0.5);
                    let d = great_circle_deg(e.lat, e.lon, lat, lon);
                    // Within a dot (1 deg here) of the front, plus ring width.
                    assert!(
                        (d - front).abs() < 1.5,
                        "P dot at {d} deg, front at {front}"
                    );
                    n += 1;
                }
            }
        }
        // A whole ring: its circumference in dots, roughly.
        assert!(n > 50, "only {n} P dots");
    }

    #[test]
    fn surface_bands_only_for_big_events_and_epicentre_marked() {
        let sim = sim();
        let e = &sim.events[0];
        // Twenty minutes in: R1 is about 40 deg out, the body waves long past there.
        let now = e.origin + Duration::seconds(1200);
        let mut view = MapView::new(Coastlines::default(), sim.surface_waves().map_grid());
        let mut canvas = Canvas::new(120, 30, Palette::new(Look::Crt));
        view.draw(
            &mut canvas,
            &MapScene {
                sim: &sim,
                now,
                selected: 0,
                stations: &[],
                chosen: &[],
                slice_azimuth: Some(90.0),
            },
        );
        let proj = Projection {
            width: 240,
            height: 120,
        };
        let (x, y) = proj.dot(e.lat, e.lon);
        assert_eq!(canvas.symbol((x / 2.0) as usize, (y / 4.0) as usize), '*');
        let love = (0..120)
            .flat_map(|y| (0..240).map(move |x| (x, y)))
            .filter(|&(x, y)| canvas.ink_at_dot(x, y) == Some(Ink::Love))
            .count();
        assert!(love > 0, "no Love-wave dots for an M9.1");
    }
}
