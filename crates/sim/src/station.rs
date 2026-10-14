//! Virtual seismograph stations: the bundled real ones (`stations.csv`) and user-placed ones.

use std::fmt;

use crate::geo::normalise_lon;

/// A place a seismogram is recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct Station {
    /// FDSN network code ("IU", "II"); empty for a user-placed station.
    pub network: String,
    /// Station code ("ANMO"), or a short name for a user-placed one.
    pub code: String,
    /// Site description.
    pub name: String,
    /// Degrees, [-90, 90].
    pub lat: f64,
    /// Degrees, [-180, 180).
    pub lon: f64,
}

impl Station {
    /// A user-placed station; coordinates normalised like an event's.
    pub fn user(code: impl Into<String>, lat: f64, lon: f64) -> Self {
        Self {
            network: String::new(),
            code: code.into(),
            name: "user-placed".to_owned(),
            lat: lat.clamp(-90.0, 90.0),
            lon: normalise_lon(lon),
        }
    }

    pub fn is_user(&self) -> bool {
        self.network.is_empty()
    }

    /// "IU.ANMO" for a network station, the code alone for a user-placed one.
    pub fn label(&self) -> String {
        if self.is_user() {
            self.code.clone()
        } else {
            format!("{}.{}", self.network, self.code)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StationError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for StationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stations.csv line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for StationError {}

const HEADER: &str = "network,station,lat,lon,elevation_m,name";

/// Parse `stations.csv` (written by `tools/gen_stations.py`): `#` comment lines, a header, then
/// `network,station,lat,lon,elevation_m,name` rows. The name is the rest of the line and may be
/// double-quoted (it contains commas). Elevation is read but not used: the Earth is a sphere.
pub fn parse_stations(bytes: &[u8]) -> Result<Vec<Station>, StationError> {
    let text = std::str::from_utf8(bytes).map_err(|e| StationError {
        line: 0,
        msg: e.to_string(),
    })?;
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'));
    match lines.next() {
        Some((_, h)) if h.trim() == HEADER => {}
        other => {
            return Err(StationError {
                line: other.map_or(1, |(n, _)| n + 1),
                msg: format!("expected header {HEADER:?}"),
            });
        }
    }
    let mut out = Vec::new();
    for (n, line) in lines {
        let err = |msg: String| StationError { line: n + 1, msg };
        let fields: Vec<&str> = line.trim_end().splitn(6, ',').collect();
        let [network, code, lat, lon, elev, name] = fields[..] else {
            return Err(err(format!("expected 6 fields, got {}", fields.len())));
        };
        let num = |s: &str| {
            s.trim()
                .parse::<f64>()
                .map_err(|e| err(format!("{s:?}: {e}")))
        };
        let (lat, lon) = (num(lat)?, num(lon)?);
        num(elev)?;
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return Err(err(format!("coordinates out of range: {lat}, {lon}")));
        }
        let name = name.trim();
        let name = name
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(name)
            .replace("\"\"", "\"");
        out.push(Station {
            network: network.trim().to_owned(),
            code: code.trim().to_owned(),
            name,
            lat,
            lon: normalise_lon(lon),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_bundled_list() {
        let s = parse_stations(include_bytes!("../../../assets/stations.csv")).unwrap();
        assert!((40..=120).contains(&s.len()), "{} stations", s.len());
        let anmo = s
            .iter()
            .find(|s| s.code == "ANMO")
            .expect("ANMO is bundled");
        assert_eq!(anmo.label(), "IU.ANMO");
        assert!((anmo.lat - 34.9).abs() < 0.1 && (anmo.lon + 106.5).abs() < 0.1);
        assert!(
            anmo.name.contains("Albuquerque, New Mexico"),
            "{}",
            anmo.name
        );
        assert!(s.iter().all(|s| !s.is_user()));
    }

    #[test]
    fn quoted_names_comments_and_errors() {
        let text = "# provenance\nnetwork,station,lat,lon,elevation_m,name\n\
                    IU,X,1.5,180,10,\"A, B\"\nII,Y,-2,-3,0,Plain\n";
        let s = parse_stations(text.as_bytes()).unwrap();
        assert_eq!(s[0].name, "A, B");
        assert_eq!(s[0].lon, -180.0);
        assert_eq!(s[1].name, "Plain");
        let bad = "network,station,lat,lon,elevation_m,name\nIU,X,95,0,0,n\n";
        assert_eq!(parse_stations(bad.as_bytes()).unwrap_err().line, 2);
        assert!(parse_stations(b"nope\n").is_err());
    }

    #[test]
    fn user_station() {
        let u = Station::user("U1", 10.0, 190.0);
        assert!(u.is_user());
        assert_eq!(u.label(), "U1");
        assert!((u.lon + 170.0).abs() < 1e-9);
    }
}
