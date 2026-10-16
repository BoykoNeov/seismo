//! Format and interpolation tests against a tiny hand-built table written in the real format
//! by the test itself (not by the crate), so reader bugs cannot cancel writer bugs.

use tables::{GPU_MISSING, TableError, TravelTimeTable};

const NAN: f32 = f32::NAN;

/// 2 depths (0, 10 km) × 3 distances (0, 1, 2 deg) × 1 phase "P".
/// Times, `[depth][distance]`:
///   depth 0:  0, 10, 20
///   depth 10: 2, 12, NaN
/// Ray parameters:
///   depth 0:  14, 13, 12
///   depth 10: 15, NaN, 11
const TIMES: [f32; 6] = [0.0, 10.0, 20.0, 2.0, 12.0, NAN];
const RAYS: [f32; 6] = [14.0, 13.0, 12.0, 15.0, NAN, 11.0];

fn build(magic: &[u8; 8], version: u32, header_pad: usize, extra_tail: usize) -> Vec<u8> {
    let header = r#"{"model":"fixture","obspy_version":"none",
        "depth":{"start":0.0,"step":10.0,"count":2},
        "distance":{"start":0.0,"step":1.0,"count":3},
        "phases":["P"]}"#;
    let mut header = header.as_bytes().to_vec();
    header.extend(std::iter::repeat_n(b' ', header_pad));
    let mut out = Vec::new();
    out.extend_from_slice(magic);
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    for v in TIMES.iter().chain(&RAYS) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend(std::iter::repeat_n(0u8, extra_tail));
    out
}

fn fixture() -> TravelTimeTable {
    TravelTimeTable::from_bytes(&build(b"SEISMOTT", 1, 0, 0)).unwrap()
}

fn close(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-9)
}

#[test]
fn parses_header() {
    let t = fixture();
    assert_eq!(t.model(), "fixture");
    assert_eq!(t.phases(), ["P"]);
    assert_eq!(t.depth_axis().count, 2);
    assert_eq!(t.distance_axis().end(), 2.0);
}

#[test]
fn reads_unaligned_data() {
    // Every padding length puts the float data at a different alignment.
    for pad in 0..4 {
        let t = TravelTimeTable::from_bytes(&build(b"SEISMOTT", 1, pad, 0)).unwrap();
        assert!(close(t.node_time("P", 1, 1), 12.0), "pad {pad}");
    }
}

#[test]
fn values_at_nodes() {
    let t = fixture();
    assert!(close(t.arrival("P", 0.0, 0.0), 0.0));
    assert!(close(t.arrival("P", 10.0, 0.0), 2.0));
    assert!(close(t.node_time("P", 0, 2), 20.0));
    assert!(close(t.node_ray_param("P", 0, 1), 13.0));
    assert_eq!(t.node_time("P", 1, 2), None);
    assert_eq!(t.node_time("P", 2, 0), None);
    assert_eq!(t.node_ray_param("P", 1, 1), None);
}

#[test]
fn bilinear_interpolation() {
    let t = fixture();
    // Inside the all-finite cell [depth 0..10] × [dist 0..1]:
    // top edge 0..10, bottom edge 2..12 → at (5 km, 0.5°): (5 + 7) / 2 = 6.
    assert!(close(t.arrival("P", 5.0, 0.5), 6.0));
    assert!(close(t.arrival("P", 0.0, 0.25), 2.5));
    assert!(close(t.arrival("P", 2.5, 0.0), 0.5));
}

#[test]
fn nan_corner_gives_none() {
    let t = fixture();
    // Cell [0..10 km] × [1..2°] has a NaN time corner at (10 km, 2°).
    assert_eq!(t.arrival("P", 5.0, 1.5), None);
    // Exactly on the depth-0 edge the NaN corner has zero weight, but the rule is "any of the
    // four corners missing → None" (the GPU 4-tap lookup does the same), so this is None too.
    assert_eq!(t.arrival("P", 0.0, 1.5), None);
    // Exactly on the last node the cell is still [1..2°], so the NaN corner applies there too.
    assert_eq!(t.arrival("P", 10.0, 2.0), None);
    // Ray parameter NaN at (10 km, 1°) poisons both cells that touch it.
    assert_eq!(t.ray_param("P", 5.0, 0.5), None);
    assert_eq!(t.ray_param("P", 5.0, 1.5), None);
}

#[test]
fn out_of_range_and_unknown_phase() {
    let t = fixture();
    assert_eq!(t.arrival("P", -1.0, 0.5), None);
    assert_eq!(t.arrival("P", 11.0, 0.5), None);
    assert_eq!(t.arrival("P", 0.0, 2.5), None);
    assert_eq!(t.arrival("S", 0.0, 0.5), None);
}

#[test]
fn gpu_grid_replaces_nan_in_both_channels() {
    let g = fixture().gpu_grid("P").unwrap();
    assert_eq!(g.len(), 6);
    assert_eq!(g[0], [0.0, 14.0]);
    assert_eq!(g[4], [12.0, GPU_MISSING]);
    assert_eq!(g[5], [GPU_MISSING, 11.0]);
    assert!(fixture().gpu_grid("S").is_none());
}

#[test]
fn max_arrival_time_skips_nan() {
    assert_eq!(fixture().max_arrival_time(), 20.0);
}

#[test]
fn wrong_magic() {
    let e = TravelTimeTable::from_bytes(&build(b"SEISMOXX", 1, 0, 0)).unwrap_err();
    assert!(matches!(e, TableError::BadMagic), "{e}");
}

#[test]
fn wrong_version() {
    let e = TravelTimeTable::from_bytes(&build(b"SEISMOTT", 3, 0, 0)).unwrap_err();
    assert!(matches!(e, TableError::UnsupportedVersion(3)), "{e}");
    // A version 2 table must say how many later arrivals it holds.
    let e = TravelTimeTable::from_bytes(&build(b"SEISMOTT", 2, 0, 0)).unwrap_err();
    assert!(matches!(e, TableError::Inconsistent(_)), "{e}");
}

#[test]
fn truncated_and_trailing_data() {
    let full = build(b"SEISMOTT", 1, 0, 0);
    let e = TravelTimeTable::from_bytes(&full[..full.len() - 1]).unwrap_err();
    assert!(matches!(e, TableError::Truncated(_)), "{e}");
    let e = TravelTimeTable::from_bytes(&full[..10]).unwrap_err();
    assert!(matches!(e, TableError::Truncated(_)), "{e}");
    let e = TravelTimeTable::from_bytes(&build(b"SEISMOTT", 1, 0, 4)).unwrap_err();
    assert!(matches!(e, TableError::Inconsistent(_)), "{e}");
}

// ---- Version 2: later arrivals ----

/// 2 depths (0, 10 km) × 4 distances (0..3 deg) × phase "P", with a triplication: branch A
/// (p = 10 s/deg, t = 100 + 10x), branch C (p = 8, t = 105 + 8x) and the retrograde branch B
/// between them (p = 9, t = 108 + 9x, from 1° on). A and C cross at 2.5°, so the first arrival
/// is A up to 2° and C at 3°. The 10 km row is the same, 1 s later.
const V2_TIMES: [f32; 8] = [100.0, 110.0, 120.0, 129.0, 101.0, 111.0, 121.0, 130.0];
const V2_RAYS: [f32; 8] = [10.0, 10.0, 10.0, 8.0, 10.0, 10.0, 10.0, 8.0];
/// (depth index, distance index, time, ray parameter), deliberately not sorted.
const V2_LATER: [(u16, u16, f32, f32); 14] = [
    (0, 0, 105.0, 8.0),
    (0, 1, 117.0, 9.0),
    (0, 1, 113.0, 8.0),
    (0, 2, 121.0, 8.0),
    (0, 2, 126.0, 9.0),
    (0, 3, 130.0, 10.0),
    (0, 3, 135.0, 9.0),
    (1, 0, 106.0, 8.0),
    (1, 1, 114.0, 8.0),
    (1, 1, 118.0, 9.0),
    (1, 2, 122.0, 8.0),
    (1, 2, 127.0, 9.0),
    (1, 3, 131.0, 10.0),
    (1, 3, 136.0, 9.0),
];

fn build_v2(version: u32, counts: &str, later: &[(u16, u16, f32, f32)]) -> Vec<u8> {
    let header = format!(
        r#"{{"model":"fixture","obspy_version":"none",
        "depth":{{"start":0.0,"step":10.0,"count":2}},
        "distance":{{"start":0.0,"step":1.0,"count":4}},
        "phases":["P"],"later":{{"merge_s":0.5,"counts":{counts}}}}}"#
    );
    let mut out = Vec::new();
    out.extend_from_slice(b"SEISMOTT");
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(header.as_bytes());
    for v in V2_TIMES.iter().chain(&V2_RAYS) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for &(i, j, t, p) in later {
        out.extend_from_slice(&i.to_le_bytes());
        out.extend_from_slice(&j.to_le_bytes());
        out.extend_from_slice(&t.to_le_bytes());
        out.extend_from_slice(&p.to_le_bytes());
    }
    out
}

fn v2() -> TravelTimeTable {
    TravelTimeTable::from_bytes(&build_v2(2, "[14]", &V2_LATER)).unwrap()
}

fn times(arrivals: &[tables::Arrival]) -> Vec<f64> {
    arrivals.iter().map(|a| a.time_s).collect()
}

fn all_close(got: &[f64], want: &[f64]) -> bool {
    got.len() == want.len() && got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-9)
}

#[test]
fn version_1_has_only_first_arrivals() {
    let t = fixture();
    assert_eq!(t.merge_s(), 0.0);
    assert!(all_close(&times(&t.node_arrivals("P", 0, 1)), &[10.0]));
    // An arrival needs its ray parameter too: (10 km, 1°) has none, so the cell has no arrivals
    // though its times are all there.
    assert!(close(t.arrival("P", 5.0, 0.5), 6.0));
    assert!(t.arrivals("P", 5.0, 0.5).is_empty());
    assert!(t.node_arrivals("P", 1, 1).is_empty());
    assert!(t.arrivals("P", 5.0, 1.5).is_empty());
    assert!(t.arrivals("S", 0.0, 0.5).is_empty());
}

#[test]
fn later_arrivals_at_nodes_in_time_order() {
    let t = v2();
    assert_eq!(t.merge_s(), 0.5);
    assert!(all_close(
        &times(&t.node_arrivals("P", 0, 1)),
        &[110.0, 113.0, 117.0]
    ));
    assert!(all_close(
        &times(&t.node_arrivals("P", 1, 3)),
        &[130.0, 131.0, 136.0]
    ));
    // The first arrivals and their lookup are unchanged by the later section.
    assert!(close(t.arrival("P", 0.0, 2.25), 122.25));
}

#[test]
fn branches_are_followed_through_the_crossover() {
    let t = v2();
    // At 2.25°, between nodes where the first arrival is A (2°) and C (3°): the first is the
    // plain bilinear 122.25 s, A's own branch (122.5 s) merges into it, and C (123 s) and B
    // (128.25 s) follow, each interpolated along its own branch.
    let got = t.arrivals("P", 0.0, 2.25);
    assert!(all_close(&times(&got), &[122.25, 123.0, 128.25]), "{got:?}");
    assert!((got[1].ray_param - 8.0).abs() < 1e-9 && (got[2].ray_param - 9.0).abs() < 1e-9);
    // Halfway down the depth cell everything is half a second later.
    assert!(all_close(
        &times(&t.arrivals("P", 5.0, 2.25)),
        &[122.75, 123.5, 128.75]
    ));
    // Where the first arrival is A at all four corners, C is still found.
    assert!(all_close(
        &times(&t.arrivals("P", 0.0, 1.5)),
        &[115.0, 117.0, 121.5]
    ));
}

#[test]
fn a_branch_stops_one_cell_short_of_its_end() {
    // B starts at 1°: in the cell from 0° to 1° only A and C arrive.
    let t = v2();
    assert!(all_close(
        &times(&t.arrivals("P", 0.0, 0.5)),
        &[105.0, 109.0]
    ));
}

#[test]
fn bad_later_sections() {
    let e = TravelTimeTable::from_bytes(&build_v2(2, "[3, 4]", &V2_LATER)).unwrap_err();
    assert!(matches!(e, TableError::Inconsistent(_)), "{e}");
    let e = TravelTimeTable::from_bytes(&build_v2(1, "[14]", &V2_LATER)).unwrap_err();
    assert!(matches!(e, TableError::Inconsistent(_)), "{e}");
    let e = TravelTimeTable::from_bytes(&build_v2(2, "[15]", &V2_LATER)).unwrap_err();
    assert!(matches!(e, TableError::Truncated(_)), "{e}");
    let e = TravelTimeTable::from_bytes(&build_v2(2, "[13]", &V2_LATER)).unwrap_err();
    assert!(matches!(e, TableError::Inconsistent(_)), "{e}");
    let off_grid = [(0, 4, 140.0, 8.0)];
    let e = TravelTimeTable::from_bytes(&build_v2(2, "[1]", &off_grid)).unwrap_err();
    assert!(matches!(e, TableError::Inconsistent(_)), "{e}");
}
