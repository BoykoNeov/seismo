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
    let e = TravelTimeTable::from_bytes(&build(b"SEISMOTT", 2, 0, 0)).unwrap_err();
    assert!(matches!(e, TableError::UnsupportedVersion(2)), "{e}");
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
