//! The real crust (CRUST1.0) under the source and the station, as a correction to the arrival
//! times of the 1D model, whose crust is the same everywhere (ak135's: two flat layers down to
//! its Moho).
//!
//! Where a ray crosses the crust it travels nearly straight up or down, so the crust changes
//! its time by the change in the vertical delay time τ = ∫ √(1/v² − u²) dz, u the ray's
//! horizontal slowness: the sideways shift of the ray is compensated by its horizontal
//! slowness to first order. Each crossing compares the real column with ak135's down to a
//! depth below both Mohos; below that the two Earths are the same. Between a Moho shallower
//! than ak135's and ak135's own the real column is mantle, given ak135's speed just below its
//! Moho (the crust is replaced, the 1D mantle is kept). The travel-time table, the ray tracer,
//! the map and the slice's rays stay 1D; only the seismograms' arrival times move.

use std::f64::consts::PI;

use crate::event::Event;
use crate::geo::{EARTH_RADIUS_KM, azimuth_deg, destination};
use crate::raytrace::{RayPhase, Wave};
use crate::slice::named_boundaries;
use crate::tables::{CrustColumn, CrustModel, EarthModel, TravelTimeTable};

/// Thickest slice (km) of a layer over which the slowness is taken as constant.
const STEP_KM: f64 = 0.5;
/// Spacing (deg) of the samples of [`Crust::along`]: half the crust model's cells.
pub const CUT_STEP_DEG: f64 = 0.5;

/// The crust at one point of a cut: depths below sea level (km, negative above it) of the solid
/// surface and of the Moho.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrustSample {
    pub surface_km: f64,
    pub moho_km: f64,
}

/// Which crust a wave crosses on its way, and as which wave type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrustPath {
    /// The wave type it leaves the source as.
    pub source_wave: Wave,
    /// The wave type it reaches the station as.
    pub arrival_wave: Wave,
    /// It leaves the source upward and reaches the station without turning (p, s): it
    /// crosses only the crust at the station, down to the source's depth.
    pub upgoing: bool,
    /// It reflects off the surface above the source (pP, sP), crossing the crust there twice.
    pub depth_phase: bool,
    /// It reflects off the surface on its way (PP, SS, PS, SP), crossing the crust there
    /// twice.
    pub surface_bounce: bool,
}

impl CrustPath {
    /// The path of a traced phase's ray that left the source at `takeoff_rad` (from straight
    /// down).
    pub fn of(phase: RayPhase, takeoff_rad: f64) -> Self {
        Self {
            source_wave: phase.source_wave(),
            arrival_wave: phase.arrival_wave(),
            upgoing: !phase.is_depth_phase() && takeoff_rad > PI / 2.0,
            depth_phase: phase.is_depth_phase(),
            surface_bounce: matches!(
                phase,
                RayPhase::PP | RayPhase::SS | RayPhase::PS | RayPhase::SP
            ),
        }
    }

    /// A wave that leaves downward and arrives as the same type, with no surface reflection:
    /// the diffracted waves (Pdiff, Sdiff).
    pub fn direct(wave: Wave) -> Self {
        Self {
            source_wave: wave,
            arrival_wave: wave,
            upgoing: false,
            depth_phase: false,
            surface_bounce: false,
        }
    }
}

/// The real crust, and ak135's crust and top of mantle to compare it with.
#[derive(Debug, Clone)]
pub struct Crust {
    model: CrustModel,
    /// ak135 down past the deepest Moho of the crust model: (depth km, vp, vs), a
    /// discontinuity as two samples at one depth.
    reference: Vec<(f64, f64, f64)>,
    /// ak135's Moho (km) and its speeds just below it.
    moho_km: f64,
    mantle_top: (f64, f64),
    /// The crust model's highest solid surface and deepest Moho (km below sea level).
    extremes_km: (f64, f64),
}

impl Crust {
    /// `earth` is the 1D model the travel-time table was made from; its Moho is the base of
    /// the crust being replaced. `None` if it has no Moho.
    pub fn new(model: CrustModel, earth: &EarthModel) -> Option<Self> {
        let moho_km = named_boundaries(earth)
            .iter()
            .find(|b| b.name == "Moho")?
            .depth_km;
        let below = earth.points().iter().rfind(|p| p.depth_km == moho_km)?;
        let (highest, deepest) = model.columns().fold((0.0f64, moho_km), |(h, d), c| {
            (h.min(c.surface_km), d.max(c.moho_km))
        });
        let pts = earth.points();
        let keep = pts
            .iter()
            .position(|p| p.depth_km > deepest)
            .map_or(pts.len(), |i| i + 1);
        Some(Self {
            model,
            reference: pts[..keep]
                .iter()
                .map(|p| (p.depth_km, p.vp_km_s, p.vs_km_s))
                .collect(),
            moho_km,
            mantle_top: (below.vp_km_s, below.vs_km_s),
            extremes_km: (highest, deepest),
        })
    }

    pub fn model(&self) -> &CrustModel {
        &self.model
    }

    /// Depth (km) of the 1D model's Moho, the crust every column is compared with.
    pub fn reference_moho_km(&self) -> f64 {
        self.moho_km
    }

    /// The crust under (`lat`, `lon`).
    pub fn column(&self, lat: f64, lon: f64) -> CrustColumn {
        self.model.column(lat, lon)
    }

    /// The highest solid surface and the deepest Moho (km below sea level) anywhere in the
    /// crust model: the depths a picture of any cut needs room for.
    pub fn extremes_km(&self) -> (f64, f64) {
        self.extremes_km
    }

    /// The crust along the slice's cut, from the epicentre (`lat`, `lon`) along `azimuth_deg`
    /// to the antipode: a sample every [`CUT_STEP_DEG`].
    pub fn along(&self, lat: f64, lon: f64, azimuth_deg: f64) -> Vec<CrustSample> {
        let n = (180.0 / CUT_STEP_DEG).round() as usize;
        (0..=n)
            .map(|k| {
                let (plat, plon) = destination(lat, lon, azimuth_deg, k as f64 * CUT_STEP_DEG);
                let c = self.column(plat, plon);
                CrustSample {
                    surface_km: c.surface_km,
                    moho_km: c.moho_km,
                }
            })
            .collect()
    }

    /// The 1D model's crust as a sample: from sea level to its Moho, the same everywhere.
    pub fn flat(&self) -> CrustSample {
        CrustSample {
            surface_km: 0.0,
            moho_km: self.moho_km,
        }
    }

    /// Time (s) the real crust under (`lat`, `lon`) adds to a `wave` crossing it straight up
    /// or down, against the 1D model's crust.
    pub fn vertical_delay(&self, lat: f64, lon: f64, wave: Wave) -> f64 {
        self.crossing(&self.column(lat, lon), wave, 0.0)
    }

    /// Time (s) the real crust adds to an arrival from `event` at (`lat`, `lon`) that travelled
    /// `path` with ray parameter `ray_param` (s/deg); negative when the real crust is faster
    /// than the 1D model's. `table` places the surface reflections of PP, SS, PS and SP.
    pub fn delay(
        &self,
        table: &TravelTimeTable,
        event: &Event,
        lat: f64,
        lon: f64,
        path: CrustPath,
        ray_param: f64,
    ) -> f64 {
        let p = ray_param * 180.0 / PI;
        let depth = event.depth_km;
        let source = self.column(event.lat, event.lon);
        let station = self.column(lat, lon);
        let (ws, wa) = (path.source_wave, path.arrival_wave);
        if path.upgoing {
            return self.above(&station, wa, p, depth);
        }
        let receiver = self.crossing(&station, wa, p);
        if path.depth_phase {
            // Up to the surface and back down, taken as right above the source.
            return self.above(&source, ws, p, depth) + self.crossing(&source, wa, p) + receiver;
        }
        let mut total = self.below(&source, ws, p, depth) + receiver;
        if path.surface_bounce {
            let dist = crate::geo::great_circle_deg(event.lat, event.lon, lat, lon);
            let hop = first_hop_deg(table, ws, depth, ray_param).unwrap_or(dist / 2.0);
            let az = azimuth_deg(event.lat, event.lon, lat, lon);
            let (blat, blon) = destination(event.lat, event.lon, az, hop.min(dist));
            let bounce = self.column(blat, blon);
            total += self.crossing(&bounce, ws, p) + self.crossing(&bounce, wa, p);
        }
        total
    }

    /// Depth (km) below which `column` and the 1D model are the same Earth.
    fn base_km(&self, column: &CrustColumn) -> f64 {
        column.moho_km.max(self.moho_km)
    }

    /// One crossing of the whole crust at `column`: from the solid surface down to below both
    /// Mohos (the same time up or down).
    fn crossing(&self, column: &CrustColumn, wave: Wave, p: f64) -> f64 {
        let base = self.base_km(column);
        self.column_tau(column, wave, p, column.surface_km, base)
            - self.reference_tau(wave, p, 0.0, base, false)
    }

    /// The part of the crust at `column` below a source `depth` km deep: what a ray leaving
    /// it downward crosses. Zero for a source below both Mohos. A source the 1D model puts in
    /// the sea or the air of the real column starts at the solid surface.
    fn below(&self, column: &CrustColumn, wave: Wave, p: f64, depth: f64) -> f64 {
        let base = self.base_km(column);
        self.column_tau(column, wave, p, depth.max(column.surface_km), base)
            - self.reference_tau(wave, p, depth.max(0.0), base, false)
    }

    /// The part of the crust at `column` above a source `depth` km deep (or all of it, for a
    /// source below both Mohos): what a ray leaving it upward crosses.
    fn above(&self, column: &CrustColumn, wave: Wave, p: f64, depth: f64) -> f64 {
        let bottom = depth.min(self.base_km(column));
        self.column_tau(column, wave, p, column.surface_km, bottom)
            - self.reference_tau(wave, p, 0.0, bottom.max(0.0), false)
    }

    /// Delay time τ (s) of the real column from `z0` to `z1` km (below sea level): its layers,
    /// then the 1D mantle below its Moho.
    fn column_tau(&self, column: &CrustColumn, wave: Wave, p: f64, z0: f64, z1: f64) -> f64 {
        let layers: f64 = column
            .layers
            .iter()
            .map(|l| {
                let v = match wave {
                    Wave::P => l.vp_km_s,
                    Wave::S => l.vs_km_s,
                };
                tau(|_| v, p, l.top_km.max(z0), l.bottom_km.min(z1))
            })
            .sum();
        layers + self.reference_tau(wave, p, column.moho_km.max(z0), z1, true)
    }

    /// Delay time τ (s) of the 1D model from `z0` to `z1` km. With `mantle_only` its crust is
    /// replaced by the speed just below its Moho.
    fn reference_tau(&self, wave: Wave, p: f64, z0: f64, z1: f64, mantle_only: bool) -> f64 {
        if z1 <= z0 {
            return 0.0;
        }
        // Integrate piece by piece between the model's samples so every piece is one
        // straight-line segment of the model (or the replaced crust).
        let mut cuts: Vec<f64> = self
            .reference
            .iter()
            .map(|r| r.0)
            .chain(mantle_only.then_some(self.moho_km))
            .filter(|&d| d > z0 && d < z1)
            .collect();
        cuts.push(z1);
        cuts.dedup();
        let mut total = 0.0;
        let mut top = z0;
        for bottom in cuts {
            total += tau(
                |z| self.reference_speed(wave, z, mantle_only),
                p,
                top,
                bottom,
            );
            top = bottom;
        }
        total
    }

    /// Speed of `wave` (km/s) in the 1D model at `depth` km, inside a segment (not at a
    /// discontinuity).
    fn reference_speed(&self, wave: Wave, depth: f64, mantle_only: bool) -> f64 {
        let pick = |r: &(f64, f64, f64)| match wave {
            Wave::P => r.1,
            Wave::S => r.2,
        };
        if mantle_only && depth < self.moho_km {
            return pick(&(0.0, self.mantle_top.0, self.mantle_top.1));
        }
        let r = &self.reference;
        let i = r
            .windows(2)
            .position(|w| w[0].0 < w[1].0 && depth < w[1].0)
            .unwrap_or(r.len() - 2);
        let (a, b) = (&r[i], &r[i + 1]);
        let f = ((depth - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
        pick(a) + f * (pick(b) - pick(a))
    }
}

/// τ (s) from `z0` to `z1` km below sea level at speed `v(z)`, for a ray of ray parameter `p`
/// (s/rad): its horizontal slowness at depth z is p / (R − z). Rays that would turn inside
/// (none that reach the mantle) count as horizontal there.
fn tau(v: impl Fn(f64) -> f64, p: f64, z0: f64, z1: f64) -> f64 {
    if z1 <= z0 {
        return 0.0;
    }
    let n = ((z1 - z0) / STEP_KM).ceil().max(1.0);
    let h = (z1 - z0) / n;
    (0..n as usize)
        .map(|k| {
            let z = z0 + (k as f64 + 0.5) * h;
            let u = p / (EARTH_RADIUS_KM - z);
            let s = 1.0 / v(z);
            (s * s - u * u).max(0.0).sqrt() * h
        })
        .sum()
}

/// Distance (deg) from the source to where a wave of ray parameter `ray_param` (s/deg),
/// leaving `depth_km` deep downward as `wave`, first comes back up to the surface: the first
/// hop of PP, SS, PS or SP. Found on the table's direct-wave branch, searched from far to near
/// so a deep source's upgoing rays are passed over. `None` if no direct wave has that ray
/// parameter.
pub fn first_hop_deg(
    table: &TravelTimeTable,
    wave: Wave,
    depth_km: f64,
    ray_param: f64,
) -> Option<f64> {
    let phase = match wave {
        Wave::P => "P",
        Wave::S => "S",
    };
    let axis = table.distance_axis();
    let mut far: Option<(f64, f64)> = None;
    for i in (0..axis.count).rev() {
        let d = axis.value(i);
        let Some(p) = table.ray_param(phase, depth_km, d) else {
            far = None;
            continue;
        };
        if let Some((d1, p1)) = far
            && p >= ray_param
            && p1 < ray_param
        {
            return Some(d + (d1 - d) * (p - ray_param) / (p - p1));
        }
        far = Some((d, p));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventKind;
    use crate::tables::CrustLayer;
    use chrono::{TimeZone, Utc};

    fn earth() -> EarthModel {
        EarthModel::from_bytes(include_bytes!("../../../assets/ak135_model.csv")).unwrap()
    }

    fn table() -> TravelTimeTable {
        TravelTimeTable::from_bytes(include_bytes!("../../../assets/ak135.tt")).unwrap()
    }

    fn crust() -> Crust {
        let model = CrustModel::from_bytes(include_bytes!("../../../assets/crust1.bin")).unwrap();
        Crust::new(model, &earth()).unwrap()
    }

    fn event(lat: f64, lon: f64, depth_km: f64) -> Event {
        let t = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        Event::new("t", EventKind::Earthquake, t, lat, lon, depth_km, 6.0)
    }

    /// ak135's own crust as a column: two layers from sea level to its Moho.
    fn ak135_column(c: &Crust) -> CrustColumn {
        let r = &c.reference;
        let layer = |top: usize| CrustLayer {
            top_km: r[top].0,
            bottom_km: r[top + 1].0,
            vp_km_s: r[top].1,
            vs_km_s: r[top].2,
        };
        CrustColumn {
            surface_km: 0.0,
            water_km: 0.0,
            moho_km: c.moho_km,
            layers: vec![layer(0), layer(2)],
        }
    }

    /// Ray parameter (s/rad) of a teleseismic P wave, about 6 s/deg.
    const P_TELE: f64 = 6.0 * 180.0 / PI;

    #[test]
    fn ak135_crust_gives_no_correction() {
        let c = crust();
        let col = ak135_column(&c);
        assert_eq!(col.moho_km, 35.0);
        for wave in [Wave::P, Wave::S] {
            for p in [0.0, P_TELE] {
                assert!(c.crossing(&col, wave, p).abs() < 1e-9);
                for depth in [0.0, 10.0, 20.0, 34.0, 100.0] {
                    assert!(c.below(&col, wave, p, depth).abs() < 1e-9);
                    assert!(c.above(&col, wave, p, depth).abs() < 1e-9);
                }
            }
        }
    }

    #[test]
    fn vertical_crossing_matches_hand_sums() {
        // Straight down through the Tibet cell (gen_crust.py prints it from the source files)
        // against ak135 down to the cell's Moho, 67.28 km.
        let c = crust();
        let col = c.column(32.5, 88.5);
        let real = 0.1 / 2.5 + 36.05 / 6.0 + 17.31 / 6.3 + 18.74 / 6.6;
        let reference = 20.0 / 5.8 + 15.0 / 6.5 + (67.28 - 35.0) / 8.0418;
        let got = c.crossing(&col, Wave::P, 0.0);
        assert!((got - (real - reference)).abs() < 0.01, "{got}");
    }

    #[test]
    fn thick_crust_is_late_and_thin_crust_early() {
        let c = crust();
        let tibet = c.column(32.5, 88.5);
        let pacific = c.column(0.5, -150.5);
        let (tp, ts) = (
            c.crossing(&tibet, Wave::P, P_TELE),
            c.crossing(&tibet, Wave::S, P_TELE),
        );
        let (pp, ps) = (
            c.crossing(&pacific, Wave::P, P_TELE),
            c.crossing(&pacific, Wave::S, P_TELE),
        );
        assert!(tp > 1.0 && tp < 3.0, "Tibet P {tp}");
        assert!(ts > tp, "Tibet S {ts} vs P {tp}");
        assert!(pp < -0.5 && pp > -2.5, "Pacific P {pp}");
        // Under the ocean the slow sediments win back some of what the thin crust gains.
        assert!(ps < 0.0, "Pacific S {ps}");
    }

    #[test]
    fn deep_sources_cross_no_crust_of_their_own() {
        let c = crust();
        let col = c.column(32.5, 88.5);
        assert_eq!(c.below(&col, Wave::P, P_TELE, 100.0), 0.0);
        assert!(c.below(&col, Wave::P, P_TELE, 10.0) > 0.5);
        // From inside the crust, only the part below the source counts.
        assert!(c.below(&col, Wave::P, P_TELE, 10.0) < c.crossing(&col, Wave::P, P_TELE));
    }

    #[test]
    fn delays_add_the_crossings_each_phase_makes() {
        let (c, t) = (crust(), table());
        let e = event(32.5, 88.5, 10.0); // in Tibet
        let (slat, slon) = (0.5, -150.5); // mid-Pacific
        let p_deg = 5.0;
        let p = p_deg * 180.0 / PI;
        let (src, rcv) = (c.column(e.lat, e.lon), c.column(slat, slon));
        let direct = c.delay(&t, &e, slat, slon, CrustPath::of(RayPhase::P, 0.3), p_deg);
        let want = c.below(&src, Wave::P, p, 10.0) + c.crossing(&rcv, Wave::P, p);
        assert!((direct - want).abs() < 1e-12);
        // A converted wave crosses the station's crust as what it arrives as.
        let pcs = c.delay(&t, &e, slat, slon, CrustPath::of(RayPhase::PcS, 0.3), p_deg);
        let want = c.below(&src, Wave::P, p, 10.0) + c.crossing(&rcv, Wave::S, p);
        assert!((pcs - want).abs() < 1e-12);
        // pP goes up through the crust above the source and back down through all of it.
        let pp = c.delay(&t, &e, slat, slon, CrustPath::of(RayPhase::pP, 2.5), p_deg);
        let want = c.above(&src, Wave::P, p, 10.0)
            + c.crossing(&src, Wave::P, p)
            + c.crossing(&rcv, Wave::P, p);
        assert!((pp - want).abs() < 1e-12);
        // A ray leaving upward reaches the station without crossing the source's crust.
        let up = c.delay(&t, &e, slat, slon, CrustPath::of(RayPhase::P, 2.5), p_deg);
        assert!((up - c.above(&rcv, Wave::P, p, 10.0)).abs() < 1e-12);
    }

    #[test]
    fn a_cut_samples_the_cells_it_crosses() {
        let c = crust();
        // East along the equator from 30° W: the Atlantic's thin crust, then Africa's.
        let cut = c.along(0.0, -30.0, 90.0);
        assert_eq!(cut.len(), 361);
        let col = |lon: f64| c.column(0.0, lon);
        assert_eq!(cut[0].moho_km, col(-30.0).moho_km);
        assert_eq!(cut[0].surface_km, col(-30.0).surface_km);
        assert_eq!(cut[100].moho_km, col(20.0).moho_km);
        assert!(cut[0].moho_km < c.reference_moho_km());
        assert!(cut[100].moho_km > c.reference_moho_km());
        // Every cut fits in the model's extremes.
        let (top, bottom) = c.extremes_km();
        assert!(top < 0.0 && bottom > c.reference_moho_km());
        assert!(
            cut.iter()
                .all(|s| s.surface_km >= top && s.moho_km <= bottom)
        );
        assert_eq!(c.flat().moho_km, c.reference_moho_km());
    }

    #[test]
    fn surface_reflections_bounce_where_the_first_hop_lands() {
        let t = table();
        // From a surface source PP bounces halfway: its first hop is a P wave with the same
        // ray parameter.
        for dist in [60.0, 100.0, 140.0] {
            let p = t.ray_param("PP", 0.0, dist).unwrap();
            let hop = first_hop_deg(&t, Wave::P, 0.0, p).unwrap();
            assert!((hop - dist / 2.0).abs() < 0.5, "PP at {dist}: {hop}");
        }
        // From a deep source the first hop is shorter than half the way.
        let p = t.ray_param("PP", 500.0, 100.0).unwrap();
        let hop = first_hop_deg(&t, Wave::P, 500.0, p).unwrap();
        assert!(hop < 49.0 && hop > 35.0, "{hop}");
        // PS: at one ray parameter P turns shallower than S, so its hop is the shorter; the
        // two hops make up the whole way.
        let p = t.ray_param("PS", 0.0, 100.0).unwrap();
        let hop_p = first_hop_deg(&t, Wave::P, 0.0, p).unwrap();
        let hop_s = first_hop_deg(&t, Wave::S, 0.0, p).unwrap();
        assert!(hop_p < 40.0, "{hop_p}");
        assert!((hop_p + hop_s - 100.0).abs() < 0.5, "{hop_p} + {hop_s}");
        // The bounce's crust counts twice, once as each leg's wave.
        let c = crust();
        let e = event(0.0, 0.0, 0.0);
        let p = t.ray_param("PP", 0.0, 100.0).unwrap();
        let pp = c.delay(&t, &e, 0.0, 100.0, CrustPath::of(RayPhase::PP, 0.5), p);
        let pr = p * 180.0 / PI;
        let hop = first_hop_deg(&t, Wave::P, 0.0, p).unwrap();
        let bounce = c.column(0.0, hop);
        let want = c.below(&c.column(0.0, 0.0), Wave::P, pr, 0.0)
            + c.crossing(&c.column(0.0, 100.0), Wave::P, pr)
            + 2.0 * c.crossing(&bounce, Wave::P, pr);
        assert!((pp - want).abs() < 1e-12, "{pp} vs {want}");
    }
}
