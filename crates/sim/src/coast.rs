//! Coastlines: the polylines of `assets/coastlines.txt` (written by `tools/gen_coastlines.py`
//! from Natural Earth), parsed from bytes. Geography for every front end's map, not physics.

use std::fmt;

/// Coastline polylines, each a list of `[lat, lon]` in degrees.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Coastlines {
    lines: Vec<Vec<[f32; 2]>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CoastError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for CoastError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "coastlines.txt line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for CoastError {}

impl Coastlines {
    /// Parse `coastlines.txt`: `#` comment lines, then one polyline per line as space-separated
    /// `lat,lon` pairs in degrees, at least two per line.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CoastError> {
        let text = std::str::from_utf8(bytes).map_err(|e| CoastError {
            line: 0,
            msg: e.to_string(),
        })?;
        let mut lines = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let err = |msg: String| CoastError { line: n + 1, msg };
            let mut points = Vec::new();
            for pair in line.split_whitespace() {
                let (lat, lon) = pair
                    .split_once(',')
                    .ok_or_else(|| err(format!("{pair:?}: expected lat,lon")))?;
                let num = |s: &str| s.parse::<f32>().map_err(|e| err(format!("{pair:?}: {e}")));
                let (lat, lon) = (num(lat)?, num(lon)?);
                if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
                    return Err(err(format!("coordinates out of range: {lat}, {lon}")));
                }
                points.push([lat, lon]);
            }
            if points.len() < 2 {
                return Err(err(format!("{} point(s); a line needs two", points.len())));
            }
            lines.push(points);
        }
        Ok(Self { lines })
    }

    pub fn lines(&self) -> &[Vec<[f32; 2]>] {
        &self.lines
    }

    pub fn point_count(&self) -> usize {
        self.lines.iter().map(Vec::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_and_polylines() {
        let c = Coastlines::from_bytes(b"# a comment\n\n10,20 11,21.5 -12,-30\n0,-180 1,180\n")
            .unwrap();
        assert_eq!(c.lines().len(), 2);
        assert_eq!(
            c.lines()[0],
            vec![[10.0, 20.0], [11.0, 21.5], [-12.0, -30.0]]
        );
        assert_eq!(c.point_count(), 5);
    }

    #[test]
    fn rejects_bad_lines() {
        for (bad, line) in [
            (&b"10,20\n"[..], 1),
            (b"#\n10,20 11\n", 2),
            (b"10,20 91,0\n", 1),
            (b"10,20 0,181\n", 1),
            (b"10,20 x,1\n", 1),
        ] {
            let e = Coastlines::from_bytes(bad).unwrap_err();
            assert_eq!(e.line, line, "{e}");
        }
    }

    #[test]
    fn bundled_coastlines_parse_under_the_point_budget() {
        let c = Coastlines::from_bytes(include_bytes!("../../../assets/coastlines.txt")).unwrap();
        assert!(c.lines().len() > 1000);
        assert!(c.point_count() > 20_000 && c.point_count() < 50_000);
    }
}
