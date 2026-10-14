//! Travel-time delays from the 3D mantle model, for the map.
//!
//! Each ray of the 1D model is laid out along an azimuth from the source, and its delay is the
//! first-order change in travel time, −∫ dlnV dt along the 1D path (a fast region, dlnV > 0,
//! makes the wave early). The rays are not bent: by Fermat's principle the extra change from
//! bending is second order in the perturbation, a small fraction of a second at the model's real
//! strength. The map scales the delays linearly by its strength setting.
//!
//! Per event the delays are gridded by epicentral distance and azimuth. Each grid node takes the
//! delay of the earliest-arriving branch through it, matching the table's first-arrival rule;
//! nodes no ray reaches get 0.

use crate::geo::{EARTH_RADIUS_KM, destination};
use crate::raytrace::{Ray, RayPhase, RayTracer, Wave, fold_rad};
use crate::slice::{Boundary, build_fan};
use crate::tables::MantleModel;

use std::f64::consts::PI;

/// Distance spacing (deg) of a [`DelayGrid`], from 0° to 180°.
pub const DELAY_DIST_STEP_DEG: f64 = 1.0;
pub const DELAY_DIST_COUNT: usize = 181;
/// Azimuth spacing (deg) of a [`DelayGrid`], from 0° (north) clockwise; the last column wraps
/// to the first.
pub const DELAY_AZ_STEP_DEG: f64 = 5.0;
pub const DELAY_AZ_COUNT: usize = 72;
/// The phases a [`DelayGrid`] holds, in layer order: the map's direct waves.
pub const DELAY_PHASES: [RayPhase; 2] = [RayPhase::P, RayPhase::S];

/// Neighbouring rays whose surface points are farther apart than this (deg) are not
/// interpolated between: there is a gap in the branch.
const MAX_BRACKET_DEG: f64 = 2.0;

/// The 3D mantle model applied to ak135: zero in ak135's crust (above its Moho), and outside
/// the model's own mantle.
#[derive(Debug, Clone)]
pub struct Mantle3d {
    model: MantleModel,
    moho_depth_km: f64,
}

impl Mantle3d {
    /// `boundaries` are the 1D model's named boundaries; its Moho is where the perturbations
    /// start. Without one they start at the top of the 3D model.
    pub fn new(model: MantleModel, boundaries: &[Boundary]) -> Self {
        let moho_depth_km = boundaries
            .iter()
            .find(|b| b.name == "Moho")
            .map_or(0.0, |b| b.depth_km);
        Self {
            model,
            moho_depth_km,
        }
    }

    pub fn model(&self) -> &MantleModel {
        &self.model
    }

    /// Fractional velocity perturbation of `wave` at radius `r_km` below (`lat`, `lon`).
    pub fn dlnv(&self, wave: Wave, r_km: f64, lat: f64, lon: f64) -> f64 {
        if EARTH_RADIUS_KM - r_km < self.moho_depth_km {
            return 0.0;
        }
        match wave {
            Wave::P => self.model.dlnvp(r_km, lat, lon),
            Wave::S => self.model.dlnvs(r_km, lat, lon),
        }
    }
}

/// First-order delay (s) of `ray` with its path laid out from (`lat`, `lon`) along
/// `azimuth_deg`: −Σ dlnV·Δt over the path's segments, dlnV taken at each segment's midpoint.
/// `dlnv(r_km, lat, lon)` gives the perturbation of the ray's wave type.
pub fn path_delay(
    ray: &Ray,
    lat: f64,
    lon: f64,
    azimuth_deg: f64,
    dlnv: impl Fn(f64, f64, f64) -> f64,
) -> f64 {
    ray.points
        .windows(2)
        .map(|w| {
            let r = 0.5 * (w[0].r_km + w[1].r_km);
            let theta = 0.5 * (w[0].theta_rad + w[1].theta_rad);
            let (la, lo) = destination(lat, lon, azimuth_deg, theta.to_degrees());
            -dlnv(r, la, lo) * (w[1].t_s - w[0].t_s)
        })
        .sum()
}

/// One event's delays: per phase in [`DELAY_PHASES`] order, `DELAY_AZ_COUNT` rows (azimuth) of
/// `DELAY_DIST_COUNT` values (distance), seconds at strength 1, 0 where the phase has no ray.
#[derive(Debug, Clone, PartialEq)]
pub struct DelayGrid {
    pub delays: Vec<Vec<f32>>,
    /// Largest |delay| over all phases (s).
    pub max_abs_s: f32,
}

impl DelayGrid {
    /// Delay (s) of phase layer `layer` at grid node (`az_index`, `dist_index`).
    pub fn at(&self, layer: usize, az_index: usize, dist_index: usize) -> f32 {
        self.delays[layer][az_index * DELAY_DIST_COUNT + dist_index]
    }
}

/// The delay grid of an event at (`lat`, `lon`), `depth_km` deep. Tens of thousands of ray
/// integrals: run it off the UI thread.
pub fn delay_grid(
    tracer: &RayTracer,
    mantle: &Mantle3d,
    lat: f64,
    lon: f64,
    depth_km: f64,
) -> DelayGrid {
    delay_grid_with(tracer, lat, lon, depth_km, |w, r, la, lo| {
        mantle.dlnv(w, r, la, lo)
    })
}

fn delay_grid_with(
    tracer: &RayTracer,
    lat: f64,
    lon: f64,
    depth_km: f64,
    dlnv: impl Fn(Wave, f64, f64, f64) -> f64,
) -> DelayGrid {
    let delays: Vec<Vec<f32>> = DELAY_PHASES
        .iter()
        .map(|&phase| {
            let fan = build_fan(tracer, phase, depth_km);
            let wave = phase.mantle_wave();
            let (_, d) = phase_grid(&fan, lat, lon, |r, la, lo| dlnv(wave, r, la, lo));
            d.into_iter().map(|x| x as f32).collect()
        })
        .collect();
    let max_abs_s = delays.iter().flatten().fold(0.0f32, |m, &x| m.max(x.abs()));
    DelayGrid { delays, max_abs_s }
}

/// Arrival time and delay per node of one phase's fan (row-major [azimuth][distance]). The
/// time is infinite and the delay 0 where no ray arrives.
fn phase_grid(
    fan: &[Ray],
    lat: f64,
    lon: f64,
    dlnv: impl Fn(f64, f64, f64) -> f64,
) -> (Vec<f64>, Vec<f64>) {
    let n = DELAY_AZ_COUNT * DELAY_DIST_COUNT;
    let mut best_t = vec![f64::INFINITY; n];
    let mut delay = vec![0.0; n];
    struct Landing {
        dist_deg: f64,
        /// Lands past the antipode: on the opposite azimuth.
        wrapped: bool,
        t_s: f64,
        delay_s: f64,
    }
    for k in 0..DELAY_AZ_COUNT {
        let az = DELAY_AZ_STEP_DEG * k as f64;
        let landings: Vec<Landing> = fan
            .iter()
            .map(|ray| {
                let end = ray.end();
                Landing {
                    dist_deg: fold_rad(end.theta_rad).to_degrees(),
                    wrapped: end.theta_rad.rem_euclid(2.0 * PI) > PI,
                    t_s: end.t_s,
                    delay_s: path_delay(ray, lat, lon, az, &dlnv),
                }
            })
            .collect();
        for w in landings.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let (lo, hi) = (a.dist_deg.min(b.dist_deg), a.dist_deg.max(b.dist_deg));
            if a.wrapped != b.wrapped || hi - lo > MAX_BRACKET_DEG {
                continue;
            }
            let col = if a.wrapped {
                (k + DELAY_AZ_COUNT / 2) % DELAY_AZ_COUNT
            } else {
                k
            };
            let first = (lo / DELAY_DIST_STEP_DEG).ceil() as usize;
            let last = ((hi / DELAY_DIST_STEP_DEG).floor() as usize).min(DELAY_DIST_COUNT - 1);
            for j in first..=last {
                let d = DELAY_DIST_STEP_DEG * j as f64;
                let f = if hi > lo {
                    (d - a.dist_deg) / (b.dist_deg - a.dist_deg)
                } else {
                    0.0
                };
                let t = a.t_s + (b.t_s - a.t_s) * f;
                let idx = col * DELAY_DIST_COUNT + j;
                if t < best_t[idx] {
                    best_t[idx] = t;
                    delay[idx] = a.delay_s + (b.delay_s - a.delay_s) * f;
                }
            }
        }
    }
    (best_t, delay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::great_circle_deg;
    use crate::slice::named_boundaries;
    use crate::tables::EarthModel;

    const TOHOKU: (f64, f64, f64) = (38.297, 142.373, 29.0);

    fn earth() -> EarthModel {
        EarthModel::from_bytes(include_bytes!("../../../assets/ak135_model.csv")).unwrap()
    }

    fn mantle(model: &EarthModel) -> Mantle3d {
        let m = MantleModel::from_bytes(
            include_bytes!("../../../assets/sp12rts/SP12RTS..ES.sph"),
            include_bytes!("../../../assets/sp12rts/SP12RTS..EP.sph"),
        )
        .unwrap();
        Mantle3d::new(m, &named_boundaries(model))
    }

    #[test]
    fn no_perturbation_no_delay() {
        let tracer = RayTracer::new(&earth()).unwrap();
        let (lat, lon, depth) = TOHOKU;
        let g = delay_grid_with(&tracer, lat, lon, depth, |_, _, _, _| 0.0);
        assert_eq!(g.max_abs_s, 0.0);
        assert!(g.delays.iter().flatten().all(|&d| d == 0.0));
    }

    #[test]
    fn uniform_perturbation_scales_travel_time() {
        // A uniformly 1% faster Earth: to first order every ray is 1% early.
        let tracer = RayTracer::new(&earth()).unwrap();
        for phase in [RayPhase::P, RayPhase::S] {
            let a = tracer.arrivals(phase, 29.0, 60.0)[0];
            let ray = tracer.trace(phase, 29.0, a.takeoff_rad).unwrap();
            let d = path_delay(&ray, 10.0, 20.0, 33.0, |_, _, _| 0.01);
            assert!((d + 0.01 * ray.arrival_s()).abs() < 1e-9, "{phase:?}: {d}");
        }
    }

    #[test]
    fn symmetric_structure_gives_the_same_delay_in_every_direction() {
        let tracer = RayTracer::new(&earth()).unwrap();
        let (lat, lon, depth) = TOHOKU;
        // Depends only on radius and distance from the source.
        let g = delay_grid_with(&tracer, lat, lon, depth, |_, r, la, lo| {
            let d = great_circle_deg(lat, lon, la, lo).to_radians();
            0.02 * d.cos() * (r / EARTH_RADIUS_KM)
        });
        assert!(g.max_abs_s > 1.0);
        for layer in 0..DELAY_PHASES.len() {
            for j in 0..DELAY_DIST_COUNT {
                let first = g.at(layer, 0, j);
                for k in 1..DELAY_AZ_COUNT {
                    let v = g.at(layer, k, j);
                    assert!(
                        (v - first).abs() < 1e-3,
                        "layer {layer} dist {j} az {k}: {v} vs {first}"
                    );
                }
            }
        }
    }

    #[test]
    fn grid_takes_the_first_arrival() {
        let tracer = RayTracer::new(&earth()).unwrap();
        let (lat, lon, depth) = TOHOKU;
        for phase in DELAY_PHASES {
            let fan = build_fan(&tracer, phase, depth);
            let (t, _) = phase_grid(&fan, lat, lon, |_, _, _| 0.0);
            // Through the 410/660 triplications and at teleseismic distances.
            for j in [5, 15, 18, 20, 22, 25, 30, 60, 90] {
                let want = tracer.arrivals(phase, depth, j as f64)[0].time_s;
                for k in [0, 17, 50] {
                    let got = t[k * DELAY_DIST_COUNT + j];
                    assert!((got - want).abs() < 0.1, "{phase:?} {j}°: {got} vs {want}");
                }
            }
        }
    }

    #[test]
    fn no_delay_where_the_phase_does_not_arrive() {
        let model = earth();
        let tracer = RayTracer::new(&model).unwrap();
        let m = mantle(&model);
        let (lat, lon, depth) = TOHOKU;
        let g = delay_grid(&tracer, &m, lat, lon, depth);
        // Direct P and S end near 100°, in the core's shadow.
        for layer in 0..DELAY_PHASES.len() {
            for j in 110..DELAY_DIST_COUNT {
                for k in 0..DELAY_AZ_COUNT {
                    assert_eq!(g.at(layer, k, j), 0.0, "layer {layer} dist {j}");
                }
            }
        }
    }

    #[test]
    fn crust_is_left_one_dimensional() {
        let model = earth();
        let m = mantle(&model);
        let r = |depth: f64| EARTH_RADIUS_KM - depth;
        // Under the Canadian shield: fast just below ak135's Moho, nothing above it (the 3D
        // model itself starts at 24.4 km).
        assert_eq!(m.dlnv(Wave::S, r(30.0), 60.0, -100.0), 0.0);
        assert!(m.dlnv(Wave::S, r(40.0), 60.0, -100.0) > 0.01);
    }

    #[test]
    fn tohoku_delays_are_seconds() {
        let model = earth();
        let tracer = RayTracer::new(&model).unwrap();
        let m = mantle(&model);
        let (lat, lon, depth) = TOHOKU;
        let started = std::time::Instant::now();
        let g = delay_grid(&tracer, &m, lat, lon, depth);
        println!("Tohoku delay grid in {:?}", started.elapsed());
        // Spread over azimuth at 90°: a few seconds for P, several times that for S (shear
        // perturbations are larger and S is slower).
        let spread = |layer: usize, j: usize| {
            let v: Vec<f32> = (0..DELAY_AZ_COUNT).map(|k| g.at(layer, k, j)).collect();
            v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min)
        };
        let (p, s) = (spread(0, 90), spread(1, 90));
        assert!((1.0..6.0).contains(&p), "P spread {p}");
        assert!((5.0..25.0).contains(&s), "S spread {s}");
        assert!(g.max_abs_s < 20.0, "{}", g.max_abs_s);
    }

    #[test]
    fn gridded_model_agrees_with_exact_sum_along_a_ray() {
        let model = earth();
        let tracer = RayTracer::new(&model).unwrap();
        let m = mantle(&model);
        let (lat, lon, depth) = TOHOKU;
        let a = tracer.arrivals(RayPhase::S, depth, 80.0)[0];
        let ray = tracer.trace(RayPhase::S, depth, a.takeoff_rad).unwrap();
        let moho = EARTH_RADIUS_KM - 35.0;
        for az in [0.0, 120.0, 250.0] {
            let fast = path_delay(&ray, lat, lon, az, |r, la, lo| m.dlnv(Wave::S, r, la, lo));
            let exact = path_delay(&ray, lat, lon, az, |r, la, lo| {
                if r > moho {
                    0.0
                } else {
                    m.model().vs_model().dlnv(r, la, lo)
                }
            });
            assert!((fast - exact).abs() < 0.05, "az {az}: {fast} vs {exact}");
        }
    }
}
