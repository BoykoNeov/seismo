//! Numbers about the loaded Earth model and its tables, for the text that teaches (the guide):
//! layer depths, wave speeds, travel times, shadow-zone edges and surface-wave speeds. They are
//! read from the model, never written out by hand, so the guide always quotes the Earth the app
//! simulates.

use crate::geo::EARTH_RADIUS_KM;
use crate::slice::named_boundaries;
use crate::tables::{Dispersion, EarthModel, SurfaceWave, TravelTimeTable};

/// The two example distances (deg) the guide quotes travel times at: a nearby station and one a
/// quarter of the way round the world.
pub const NEAR_DEG: f64 = 30.0;
pub const FAR_DEG: f64 = 90.0;

/// P and S speed (km/s) at one place in the model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Speeds {
    pub vp_km_s: f64,
    pub vs_km_s: f64,
}

/// What the guide quotes. `None` where the model or table has no such thing.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelFacts {
    pub model: String,
    pub earth_radius_km: f64,
    /// Named boundaries (depth, km).
    pub moho_km: Option<f64>,
    pub d410_km: Option<f64>,
    pub d660_km: Option<f64>,
    pub cmb_km: Option<f64>,
    pub icb_km: Option<f64>,
    /// At the surface.
    pub surface: Speeds,
    /// Just below the Moho: the top of the mantle.
    pub upper_mantle: Option<Speeds>,
    /// Just above the core-mantle boundary: the bottom of the mantle.
    pub lowest_mantle: Option<Speeds>,
    /// Just below the core-mantle boundary: the top of the liquid outer core.
    pub outer_core: Option<Speeds>,
    /// Just below the inner-core boundary.
    pub inner_core: Option<Speeds>,
    /// Travel times (s) from a surface source to [`NEAR_DEG`] and [`FAR_DEG`].
    pub p30_s: Option<f64>,
    pub p90_s: Option<f64>,
    pub s30_s: Option<f64>,
    pub s90_s: Option<f64>,
    /// PKIKP to the antipode.
    pub pkikp180_s: Option<f64>,
    /// Farthest distance (deg) direct P reaches from a surface source: where its shadow begins.
    pub p_last_deg: Option<f64>,
    /// Nearest distance (deg) the PKP waves (the ab and bc branches) reach: where P's shadow
    /// ends.
    pub pkp_first_deg: Option<f64>,
    /// Farthest distance (deg) direct S reaches: beyond it S has no direct path.
    pub s_last_deg: Option<f64>,
    /// Slowest and fastest group speed (km/s) over the dispersion band.
    pub rayleigh_group_km_s: (f64, f64),
    pub love_group_km_s: (f64, f64),
    /// Shortest and longest surface-wave period (s) simulated.
    pub surface_band_s: (f64, f64),
}

impl ModelFacts {
    pub fn new(table: &TravelTimeTable, model: &EarthModel, dispersion: &Dispersion) -> Self {
        let bounds = named_boundaries(model);
        let depth = |name: &str| bounds.iter().find(|b| b.name == name).map(|b| b.depth_km);
        let (moho, cmb, icb) = (depth("Moho"), depth("CMB"), depth("ICB"));
        let pts = model.points();
        // At a discontinuity the model lists the values above, then below.
        let below = |d: f64| {
            pts.iter().rev().find(|p| p.depth_km == d).map(|p| Speeds {
                vp_km_s: p.vp_km_s,
                vs_km_s: p.vs_km_s,
            })
        };
        let above = |d: f64| {
            pts.iter().find(|p| p.depth_km == d).map(|p| Speeds {
                vp_km_s: p.vp_km_s,
                vs_km_s: p.vs_km_s,
            })
        };
        let at_surface = |phase: &str, dist: f64| table.arrival(phase, 0.0, dist);
        // Distances (deg) a phase reaches from a surface source, at the table's nodes.
        let reach = |phase: &str| {
            let axis = table.distance_axis();
            (0..axis.count)
                .filter(|&i| table.node_time(phase, 0, i).is_some())
                .map(|i| axis.value(i))
                .collect::<Vec<f64>>()
        };
        let last = |phase: &str| reach(phase).last().copied();
        let pkp_first = ["PKPab", "PKPbc"]
            .into_iter()
            .filter_map(|p| reach(p).first().copied())
            .min_by(f64::total_cmp);
        Self {
            model: table.model().to_owned(),
            earth_radius_km: EARTH_RADIUS_KM,
            moho_km: moho,
            d410_km: depth("410"),
            d660_km: depth("660"),
            cmb_km: cmb,
            icb_km: icb,
            surface: Speeds {
                vp_km_s: pts[0].vp_km_s,
                vs_km_s: pts[0].vs_km_s,
            },
            upper_mantle: moho.and_then(below),
            lowest_mantle: cmb.and_then(above),
            outer_core: cmb.and_then(below),
            inner_core: icb.and_then(below),
            p30_s: at_surface("P", NEAR_DEG),
            p90_s: at_surface("P", FAR_DEG),
            s30_s: at_surface("S", NEAR_DEG),
            s90_s: at_surface("S", FAR_DEG),
            pkikp180_s: at_surface("PKPdf", 180.0),
            p_last_deg: last("P"),
            pkp_first_deg: pkp_first,
            s_last_deg: last("S"),
            rayleigh_group_km_s: dispersion.group_range(SurfaceWave::Rayleigh),
            love_group_km_s: dispersion.group_range(SurfaceWave::Love),
            surface_band_s: dispersion.band(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> ModelFacts {
        let table =
            TravelTimeTable::from_bytes(include_bytes!("../../../assets/ak135.tt")).unwrap();
        let model =
            EarthModel::from_bytes(include_bytes!("../../../assets/ak135_model.csv")).unwrap();
        let dispersion =
            Dispersion::from_bytes(include_bytes!("../../../assets/surface_dispersion.csv"))
                .unwrap();
        ModelFacts::new(&table, &model, &dispersion)
    }

    #[test]
    fn every_fact_is_found_in_the_shipped_model() {
        let f = facts();
        assert_eq!(f.model, "ak135");
        for d in [f.moho_km, f.d410_km, f.d660_km, f.cmb_km, f.icb_km] {
            assert!(d.is_some());
        }
        for s in [f.upper_mantle, f.lowest_mantle, f.outer_core, f.inner_core] {
            assert!(s.is_some());
        }
        for t in [f.p30_s, f.p90_s, f.s30_s, f.s90_s, f.pkikp180_s] {
            assert!(t.is_some());
        }
        for d in [f.p_last_deg, f.pkp_first_deg, f.s_last_deg] {
            assert!(d.is_some());
        }
    }

    #[test]
    fn facts_tell_the_story_the_guide_tells() {
        let f = facts();
        // Depths in order, and the boundaries the model names.
        let d = [
            f.moho_km.unwrap(),
            f.d410_km.unwrap(),
            f.d660_km.unwrap(),
            f.cmb_km.unwrap(),
            f.icb_km.unwrap(),
        ];
        assert!(d.windows(2).all(|w| w[0] < w[1]));
        assert!(d[4] < f.earth_radius_km);
        // S is slower than P everywhere it exists; the outer core is liquid; P slows into it.
        let (lm, oc, ic) = (
            f.lowest_mantle.unwrap(),
            f.outer_core.unwrap(),
            f.inner_core.unwrap(),
        );
        assert!(f.surface.vs_km_s < f.surface.vp_km_s);
        assert!(f.upper_mantle.unwrap().vp_km_s > f.surface.vp_km_s);
        assert_eq!(oc.vs_km_s, 0.0);
        assert!(oc.vp_km_s < lm.vp_km_s);
        assert!(ic.vs_km_s > 0.0);
        // S arrives after P, more so farther away.
        let (p30, p90, s30, s90) = (
            f.p30_s.unwrap(),
            f.p90_s.unwrap(),
            f.s30_s.unwrap(),
            f.s90_s.unwrap(),
        );
        assert!(p30 < s30 && p90 < s90 && s90 - p90 > s30 - p30);
        // A shadow: direct P stops before the PKP branches begin; direct S stops too.
        let (p_last, pkp_first) = (f.p_last_deg.unwrap(), f.pkp_first_deg.unwrap());
        assert!(p_last < pkp_first && pkp_first < 180.0);
        assert!(f.s_last_deg.unwrap() < 180.0);
        // Love waves outrun Rayleigh waves.
        assert!(f.love_group_km_s.1 > f.rayleigh_group_km_s.1);
    }
}
