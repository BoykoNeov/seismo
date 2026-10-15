//! Coastlines on the map: the polylines of `assets/coastlines.txt` (parsed by `sim::coast`),
//! projected to pixels once per map size and drawn as dim lines under the wavefronts.

use sim::coast::Coastlines;

use crate::lines::{DASH_SOLID, LineVertex, push_polyline};
use crate::map::MapProjection;
use crate::theme::Rgb;

/// Line quads for every coastline in map pixels, `half_w` pixels either side of the line, in
/// `colour`. A step of more than 180 deg in longitude would run across the whole map, so a
/// line is broken there instead (Natural Earth cuts its lines at the antimeridian, so this is
/// only a guard).
pub(crate) fn coast_vertices(
    projection: &MapProjection,
    coast: &Coastlines,
    half_w: f32,
    intensity: f32,
    colour: Rgb,
) -> Vec<LineVertex> {
    let mut out = Vec::new();
    let mut run: Vec<[f32; 2]> = Vec::new();
    for line in coast.lines() {
        run.clear();
        for (k, &[lat, lon]) in line.iter().enumerate() {
            if k > 0 && (lon - line[k - 1][1]).abs() > 180.0 {
                push_polyline(&mut out, &run, half_w, intensity, DASH_SOLID, colour);
                run.clear();
            }
            run.push(projection.pixel(f64::from(lat), f64::from(lon)));
        }
        push_polyline(&mut out, &run, half_w, intensity, DASH_SOLID, colour);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaks_lines_at_the_antimeridian() {
        let p = MapProjection {
            size: (360.0, 180.0),
        };
        // Three points, the middle step crossing the antimeridian: two separate one-segment
        // pieces, no quad spanning the map.
        let c = Coastlines::from_bytes(b"0,170 0,179 0,-179 0,-170\n").unwrap();
        let v = coast_vertices(&p, &c, 0.5, 0.5, crate::PHOSPHOR);
        assert_eq!(v.len(), 2 * 6);
        let span = v.iter().fold((f32::MAX, f32::MIN), |(lo, hi), q| {
            (lo.min(q.pos[0]), hi.max(q.pos[0]))
        });
        assert!(span.0 < 20.0 && span.1 > 340.0);
        assert!(v.chunks(6).all(|q| {
            let xs = q.iter().map(|v| v.pos[0]);
            let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(l, h), x| (l.min(x), h.max(x)));
            hi - lo < 15.0
        }));
    }
}
