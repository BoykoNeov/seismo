//! Checks the committed `assets/ak135_ellip.bin` against the travel-time table it was made
//! for: the same phases, a grid on the table's nodes, and coefficients exactly where the table
//! has an arrival. The coefficients' values are checked against EllipticiPy in
//! `sim::ellipticity`.

use tables::{EllipticityTable, TravelTimeTable};

static TABLE: &[u8] = include_bytes!("../../../assets/ak135.tt");
static ELLIP: &[u8] = include_bytes!("../../../assets/ak135_ellip.bin");

#[test]
fn coefficients_exist_where_the_table_has_an_arrival() {
    let table = TravelTimeTable::from_bytes(TABLE).unwrap();
    let ellip = EllipticityTable::from_bytes(ELLIP).unwrap();
    assert_eq!(ellip.model(), table.model());
    assert_eq!(ellip.obspy_version(), table.obspy_version());
    assert!(ellip.citation().contains("Russell"), "{}", ellip.citation());
    assert_eq!(ellip.phases(), table.phases());

    // Every ellipticity node is a table node.
    let (td, tx) = (table.depth_axis(), table.distance_axis());
    let (ed, ex) = (ellip.depth_axis(), ellip.distance_axis());
    let step = |coarse: f64, fine: f64| {
        let n = (coarse / fine).round();
        assert!((n * fine - coarse).abs() < 1e-9 && n >= 1.0);
        n as usize
    };
    let (sd, sx) = (step(ed.step, td.step), step(ex.step, tx.step));
    assert_eq!((ed.start, ex.start), (td.start, tx.start));
    assert_eq!((ed.end(), ex.end()), (td.end(), tx.end()));

    let mut checked = 0;
    for phase in table.phases() {
        for i in 0..ed.count {
            for j in 0..ex.count {
                let time = table.node_time(phase, i * sd, j * sx);
                let c = ellip.node(phase, i, j);
                assert_eq!(
                    time.is_some(),
                    c.is_some(),
                    "{phase} at {} km, {}°: time {time:?}, coefficients {c:?}",
                    ed.value(i),
                    ex.value(j)
                );
                if let Some(c) = c {
                    // A few seconds at most: the Earth is only slightly flattened.
                    assert!(c.iter().all(|s| s.abs() < 5.0), "{phase}: {c:?}");
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 40_000, "{checked}");
}
