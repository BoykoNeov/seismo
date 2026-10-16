//! Checks the committed `assets/ak135.tt` against values printed by `tools/gen_tables.py`
//! straight from ObsPy 1.5.1 (`TauPyModel("ak135").get_travel_times`), a code path independent
//! of the one that filled the table. Regenerate both together.

use tables::{EarthModel, TravelTimeTable};

static TABLE: &[u8] = include_bytes!("../../../assets/ak135.tt");
static MODEL: &[u8] = include_bytes!("../../../assets/ak135_model.csv");

/// Max allowed disagreement with ObsPy (s).
const TIME_TOL_S: f64 = 0.5;
/// Max allowed ray-parameter disagreement (s/deg).
const RAY_TOL: f64 = 0.05;

/// (phase, depth_km, distance_deg, time_s, ray_param_s_per_deg), pasted from gen_tables.py.
/// On grid nodes unless noted; off-node points sit away from branch splits and triplications.
const ANCHORS: &[(&str, f64, f64, f64, f64)] = &[
    ("P", 0.00, 30.00, 370.265, 8.8489),
    ("P", 0.00, 90.00, 781.388, 4.6429),
    ("S", 0.00, 30.00, 669.127, 15.6939),
    ("S", 0.00, 90.00, 1435.422, 9.2712),
    ("PKPdf", 0.00, 180.00, 1212.485, 0.0000),
    ("PKPab", 0.00, 150.00, 1198.052, 4.1059),
    ("PKPbc", 0.00, 150.00, 1192.350, 2.4184),
    ("PcP", 0.00, 40.00, 581.595, 3.1967),
    ("ScS", 0.00, 50.00, 1128.765, 6.7990),
    ("SKS", 0.00, 100.00, 1467.019, 4.9178),
    ("Pdiff", 0.00, 110.00, 871.478, 4.4457),
    ("PP", 0.00, 60.00, 740.530, 8.8489),
    ("SS", 0.00, 80.00, 1645.833, 14.9744),
    ("P", 100.00, 30.00, 359.069, 8.8328),
    ("P", 100.00, 60.00, 595.993, 6.8357),
    ("S", 100.00, 60.00, 1080.743, 12.8095),
    ("P", 600.00, 5.00, 92.791, 7.8804), // upgoing "p" at depth, merged into P
    ("P", 100.00, 47.13, 502.040, 7.7663), // off-node distance
    ("S", 0.00, 72.60, 1254.968, 11.4284), // off-node distance
    ("P", 32.00, 40.00, 451.693, 8.2978), // off-node depth
    ("S", 212.50, 55.55, 1001.830, 13.2255), // off-node depth and distance
    ("pP", 100.00, 40.00, 467.939, 8.3510),
    ("pP", 100.00, 80.00, 744.074, 5.4382),
    ("sP", 100.00, 60.00, 631.650, 6.8870),
    ("pP", 600.00, 40.00, 506.432, 8.6995),
    ("pP", 600.00, 60.00, 665.607, 7.1785),
    ("sP", 600.00, 40.00, 574.196, 8.4647),
    ("sP", 600.00, 80.00, 854.439, 5.5166),
    ("PcS", 0.00, 40.00, 811.828, 3.8957),
    ("ScP", 0.00, 40.00, 811.828, 3.8957), // = PcS from a surface source
    ("PcS", 600.00, 30.00, 708.460, 3.3669),
    ("ScP", 600.00, 30.00, 650.135, 3.3079),
    ("PKiKP", 0.00, 60.00, 1033.450, 1.2367),
    ("PKiKP", 100.00, 120.00, 1118.844, 1.9569),
    ("PKKP", 0.00, 100.00, 1805.645, 2.8439),
    ("PKKP", 600.00, 80.00, 1786.878, 2.2113),
    ("SKKS", 0.00, 90.00, 1426.695, 7.4794),
    ("SKKS", 100.00, 140.00, 1738.469, 5.8482),
    ("PS", 0.00, 60.00, 1112.892, 13.7225),
    ("SP", 0.00, 60.00, 1112.892, 13.7225), // = PS from a surface source
    ("PS", 600.00, 100.00, 1576.296, 10.0023),
    ("SP", 600.00, 60.00, 1012.899, 13.6751),
];

/// Direct-wave cutoffs for a 29 km source from ObsPy (0.05° scan), printed by gen_tables.py.
const P_CUTOFF_29KM: f64 = 99.55;
const S_CUTOFF_29KM: f64 = 99.95;

fn table() -> TravelTimeTable {
    TravelTimeTable::from_bytes(TABLE).expect("assets/ak135.tt parses")
}

#[test]
fn header_is_as_generated() {
    let t = table();
    assert_eq!(t.model(), "ak135");
    assert_eq!(t.obspy_version(), "1.5.1");
    assert_eq!(
        t.phases(),
        [
            "P", "S", "PcP", "ScS", "PKPab", "PKPbc", "PKPdf", "PP", "SS", "SKS", "Pdiff", "Sdiff",
            "pP", "sP", "PcS", "ScP", "PKiKP", "PKKP", "SKKS", "PS", "SP"
        ]
    );
    let (d, x) = (t.depth_axis(), t.distance_axis());
    assert_eq!((d.start, d.step, d.count), (0.0, 5.0, 141));
    assert_eq!((x.start, x.step, x.count), (0.0, 0.25, 721));
}

#[test]
fn matches_obspy_anchors() {
    let t = table();
    let mut failures = Vec::new();
    for &(phase, depth, dist, time, ray) in ANCHORS {
        let got_t = t.arrival(phase, depth, dist);
        let got_p = t.ray_param(phase, depth, dist);
        match (got_t, got_p) {
            (Some(gt), Some(gp)) => {
                if (gt - time).abs() > TIME_TOL_S || (gp - ray).abs() > RAY_TOL {
                    failures.push(format!(
                        "{phase} {depth} km {dist}°: table {gt:.3} s / {gp:.4} s/deg, ObsPy {time:.3} / {ray:.4}"
                    ));
                }
            }
            _ => failures.push(format!("{phase} {depth} km {dist}°: missing in table")),
        }
    }
    assert!(
        failures.is_empty(),
        "disagreements with ObsPy:\n{}",
        failures.join("\n")
    );
}

#[test]
fn pkp_branches_are_split() {
    let t = table();
    // At 150° from a surface source all three PKP branches exist, df first, then bc, then ab.
    let df = t.arrival("PKPdf", 0.0, 150.0).unwrap();
    let bc = t.arrival("PKPbc", 0.0, 150.0).unwrap();
    let ab = t.arrival("PKPab", 0.0, 150.0).unwrap();
    assert!(df < bc && bc < ab, "df {df} bc {bc} ab {ab}");
    // bc turns deeper than ab, so it leaves the source steeper (smaller ray parameter).
    assert!(t.ray_param("PKPbc", 0.0, 150.0).unwrap() < t.ray_param("PKPab", 0.0, 150.0).unwrap());
    // bc ends at the C cusp (~155°); ab carries on to the antipode region.
    assert_eq!(t.arrival("PKPbc", 0.0, 170.0), None);
    assert!(t.arrival("PKPab", 0.0, 170.0).is_some());
}

#[test]
fn depth_phases_need_a_buried_source() {
    let t = table();
    let x = t.distance_axis();
    for phase in ["pP", "sP"] {
        for j in 0..x.count {
            assert_eq!(
                t.node_time(phase, 0, j),
                None,
                "{phase} from the surface at {}°",
                x.value(j)
            );
        }
        // From depth they follow P, later the deeper the source.
        let gap = |depth: f64| {
            t.arrival(phase, depth, 60.0).unwrap() - t.arrival("P", depth, 60.0).unwrap()
        };
        assert!(
            0.0 < gap(50.0) && gap(50.0) < gap(300.0) && gap(300.0) < gap(600.0),
            "{phase}"
        );
    }
}

/// Last tabulated distance (deg) with a direct-wave arrival, scanning on the distance grid.
fn table_cutoff(t: &TravelTimeTable, phase: &str, depth: f64) -> f64 {
    let x = t.distance_axis();
    (0..x.count)
        .map(|j| x.value(j))
        .filter(|&d| t.arrival(phase, depth, d).is_some())
        .fold(f64::NAN, f64::max)
}

#[test]
fn direct_waves_stop_where_obspy_says() {
    // Not 103° by assumption: the cutoff is whatever ObsPy gives for this depth. Bilinear
    // lookup needs all four corners, so the table's cutoff can sit up to one distance step plus
    // the difference between the bracketing depth rows short of ObsPy's.
    let t = table();
    let p = table_cutoff(&t, "P", 29.0);
    let s = table_cutoff(&t, "S", 29.0);
    assert!((p - P_CUTOFF_29KM).abs() <= 0.5, "P cutoff {p}");
    assert!((s - S_CUTOFF_29KM).abs() <= 0.5, "S cutoff {s}");
    // Beyond the cutoff only the diffracted waves remain.
    assert_eq!(t.arrival("P", 29.0, 120.0), None);
    assert!(t.arrival("Pdiff", 29.0, 120.0).is_some());
}

#[test]
fn first_arrival_times_increase_with_distance() {
    // Direct P/S first arrivals are monotonic in distance at every tabulated depth.
    let t = table();
    let (d, x) = (t.depth_axis(), t.distance_axis());
    for phase in ["P", "S"] {
        for i in 0..d.count {
            let mut last = f64::NEG_INFINITY;
            for j in 0..x.count {
                if let Some(v) = t.node_time(phase, i, j) {
                    assert!(
                        v >= last,
                        "{phase} depth {} dist {}: {v} < {last}",
                        d.value(i),
                        x.value(j)
                    );
                    last = v;
                }
            }
        }
    }
}

#[test]
fn gpu_grid_has_no_nan() {
    let t = table();
    for phase in t.phases() {
        let g = t.gpu_grid(phase).unwrap();
        assert!(g.iter().flatten().all(|v| !v.is_nan()), "{phase}");
        assert!(
            g.iter()
                .flatten()
                .all(|&v| v >= 0.0 || v == tables::GPU_MISSING),
            "{phase}"
        );
    }
}

#[test]
fn velocity_model_has_ak135_discontinuities() {
    let m = EarthModel::from_bytes(MODEL).unwrap();
    let disc = m.discontinuities();
    for depth in [20.0, 35.0, 210.0, 410.0, 660.0, 2891.5, 5153.5] {
        assert!(
            disc.iter().any(|&d| (d - depth).abs() < 0.01),
            "missing {depth} in {disc:?}"
        );
    }
    let last = m.points().last().unwrap();
    assert!((last.depth_km - 6371.0).abs() < 0.01);
    // Liquid outer core: no S velocity just below the CMB.
    let below_cmb = m
        .points()
        .iter()
        .find(|p| p.depth_km > 2891.5 && p.depth_km < 3000.0)
        .unwrap();
    assert_eq!(below_cmb.vs_km_s, 0.0);
}

#[test]
fn converted_phases_are_reciprocal_from_the_surface() {
    // From a surface source PcS and ScP (and PS and SP) run one path in opposite directions:
    // wherever both exist their times agree. From depth they do not.
    let t = table();
    let x = t.distance_axis();
    for (a, b) in [("PcS", "ScP"), ("PS", "SP")] {
        let mut both = 0;
        for j in 0..x.count {
            if let (Some(ta), Some(tb)) = (t.node_time(a, 0, j), t.node_time(b, 0, j)) {
                both += 1;
                assert!((ta - tb).abs() < 0.01, "{a} {ta} vs {b} {tb} at node {j}");
            }
        }
        assert!(both > 100, "{a}/{b}: only {both} nodes with both");
    }
    let (pcs, scp) = (
        t.arrival("PcS", 600.0, 30.0).unwrap(),
        t.arrival("ScP", 600.0, 30.0).unwrap(),
    );
    assert!(pcs - scp > 30.0, "PcS {pcs} ScP {scp}");
    // PS from a deep source needs a long path: none at 40° from 600 km.
    assert_eq!(t.arrival("PS", 600.0, 40.0), None);
}
