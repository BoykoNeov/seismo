//! The CRUST1.0 crust model (`crust1.bin`, written by `tools/gen_crust.py`): format, parsing
//! and the layer column under a point.
//!
//! CRUST1.0 (Laske, Masters, Ma and Pasyanos 2013) gives, per 1° × 1° cell, eight layers
//! (water, ice, three sediment layers, three crystalline crust layers) with their P and S
//! speeds, and the Moho beneath them. Heights in the file are above sea level; the column
//! returned here gives depths below sea level, positive down, like everything else in SEISMO.

use std::path::Path;

use serde::Deserialize;

use crate::TableError;

/// File magic, first 8 bytes.
pub const CRUST_MAGIC: &[u8; 8] = b"SEISMOCR";
/// Format version this reader understands.
pub const CRUST_FORMAT_VERSION: u32 = 1;

/// Layers per cell: water, ice, three sediment layers, three crystalline crust layers.
const N_LAYER: usize = 8;
/// Boundaries per cell: the top of each layer, then the Moho.
const N_BND: usize = N_LAYER + 1;
/// Bytes per cell: u16 profile index, then the boundaries as i16.
const CELL_BYTES: usize = 2 + 2 * N_BND;
/// Bytes per profile: u16 vp and u16 vs per layer.
const PROFILE_BYTES: usize = 4 * N_LAYER;

#[derive(Debug, Deserialize)]
struct LatLonAxis {
    start: f64,
    step: f64,
    count: usize,
}

#[derive(Debug, Deserialize)]
struct Header {
    model: String,
    citation: String,
    lat: LatLonAxis,
    lon: LatLonAxis,
    layers: Vec<String>,
    profiles: usize,
}

/// One layer of rock or ice under a point: depths below sea level (km, positive down) and
/// speeds (km/s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrustLayer {
    pub top_km: f64,
    pub bottom_km: f64,
    pub vp_km_s: f64,
    pub vs_km_s: f64,
}

/// The crust under a point: its solid layers (ice, sediments, crystalline crust) from the top
/// down, skipping layers of zero thickness, and the sea above them.
#[derive(Debug, Clone, PartialEq)]
pub struct CrustColumn {
    /// Depth below sea level (km) of the top of the solid ground: negative on land above sea
    /// level, the sea floor's depth under the ocean.
    pub surface_km: f64,
    /// Thickness (km) of the water above the solid ground; 0 on land.
    pub water_km: f64,
    /// Depth below sea level (km) of the Moho, the base of the crust.
    pub moho_km: f64,
    pub layers: Vec<CrustLayer>,
}

/// CRUST1.0 on its 1° grid.
#[derive(Debug, Clone)]
pub struct CrustModel {
    model: String,
    citation: String,
    /// Per profile, per layer: (vp, vs) in km/s; zero for layers no cell of it has.
    profiles: Vec<[(f64, f64); N_LAYER]>,
    /// Per cell, north to south then west to east: profile index and boundary heights above
    /// sea level (km).
    cells: Vec<(u16, [f64; N_BND])>,
    n_lat: usize,
    n_lon: usize,
}

impl CrustModel {
    /// Parse the model from the bytes of a `crust1.bin` file.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TableError> {
        let bad = |msg: &str| TableError::Crust(msg.to_owned());
        if bytes.len() < 16 {
            return Err(TableError::Truncated("crust preamble"));
        }
        if &bytes[0..8] != CRUST_MAGIC {
            return Err(TableError::BadMagic);
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if version != CRUST_FORMAT_VERSION {
            return Err(bad(&format!(
                "unsupported format version {version} (expected {CRUST_FORMAT_VERSION})"
            )));
        }
        let header_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let data_start = 16usize
            .checked_add(header_len)
            .filter(|&e| e <= bytes.len())
            .ok_or(TableError::Truncated("crust header"))?;
        let header: Header = serde_json::from_slice(&bytes[16..data_start])?;
        // Only CRUST1.0's own grid is understood: cell centres from 89.5° N and 179.5° W, 1°.
        let grid_ok = header.lat.start == 89.5
            && header.lat.step == -1.0
            && header.lat.count == 180
            && header.lon.start == -179.5
            && header.lon.step == 1.0
            && header.lon.count == 360;
        if !grid_ok {
            return Err(bad("expected CRUST1.0's 1° grid"));
        }
        if header.layers.len() != N_LAYER {
            return Err(bad(&format!(
                "expected {N_LAYER} layers, got {}",
                header.layers.len()
            )));
        }
        let n_cells = header.lat.count * header.lon.count;
        let profile_end = data_start + header.profiles * PROFILE_BYTES;
        let end = profile_end + n_cells * CELL_BYTES;
        if bytes.len() < end {
            return Err(TableError::Truncated("crust data"));
        }
        if bytes.len() > end {
            return Err(bad("trailing bytes after the cells"));
        }
        let hundredths =
            |c: &[u8], at: usize| f64::from(u16::from_le_bytes([c[at], c[at + 1]])) / 100.0;
        let profiles = bytes[data_start..profile_end]
            .as_chunks::<PROFILE_BYTES>()
            .0
            .iter()
            .map(|c| std::array::from_fn(|i| (hundredths(c, 4 * i), hundredths(c, 4 * i + 2))))
            .collect::<Vec<_>>();
        let mut cells = Vec::with_capacity(n_cells);
        for c in bytes[profile_end..end].as_chunks::<CELL_BYTES>().0 {
            let profile = u16::from_le_bytes([c[0], c[1]]);
            if usize::from(profile) >= profiles.len() {
                return Err(bad(&format!("profile index {profile} out of range")));
            }
            let b: [f64; N_BND] = std::array::from_fn(|i| {
                f64::from(i16::from_le_bytes([c[2 + 2 * i], c[3 + 2 * i]])) / 100.0
            });
            if b.windows(2).any(|w| w[0] < w[1]) {
                return Err(bad("layer boundaries out of order"));
            }
            cells.push((profile, b));
        }
        Ok(Self {
            model: header.model,
            citation: header.citation,
            profiles,
            cells,
            n_lat: header.lat.count,
            n_lon: header.lon.count,
        })
    }

    /// Read and parse a crust file. Thin wrapper over [`Self::from_bytes`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self, TableError> {
        Self::from_bytes(&std::fs::read(path)?)
    }

    /// Model name from the header ("CRUST1.0").
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The reference the model's authors ask for.
    pub fn citation(&self) -> &str {
        &self.citation
    }

    /// Row (north to south) and column (west to east) of the cell holding (`lat`, `lon`).
    /// Points on a cell edge belong to the cell south or east of it; the poles and the date
    /// line fall in the edge cells.
    fn cell_index(&self, lat: f64, lon: f64) -> usize {
        let row = (90.0 - lat).floor().clamp(0.0, (self.n_lat - 1) as f64) as usize;
        let lon = (lon + 180.0).rem_euclid(360.0);
        let col = lon.floor().clamp(0.0, (self.n_lon - 1) as f64) as usize;
        row * self.n_lon + col
    }

    /// The crust in the cell holding (`lat`, `lon`), degrees.
    pub fn column(&self, lat: f64, lon: f64) -> CrustColumn {
        let (profile, b) = &self.cells[self.cell_index(lat, lon)];
        let speeds = &self.profiles[usize::from(*profile)];
        // Layer 0 is the water; the solid ground starts at the top of layer 1.
        let layers = (1..N_LAYER)
            .filter(|&i| b[i] > b[i + 1])
            .map(|i| CrustLayer {
                top_km: -b[i],
                bottom_km: -b[i + 1],
                vp_km_s: speeds[i].0,
                vs_km_s: speeds[i].1,
            })
            .collect();
        CrustColumn {
            surface_km: -b[1],
            water_km: b[0] - b[1],
            moho_km: -b[N_LAYER],
            layers,
        }
    }

    /// The crust of every cell, for scanning the whole model (thinnest and thickest crust).
    pub fn columns(&self) -> impl Iterator<Item = CrustColumn> + '_ {
        (0..self.n_lat).flat_map(move |row| {
            (0..self.n_lon).map(move |col| self.column(89.5 - row as f64, -179.5 + col as f64))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real() -> CrustModel {
        CrustModel::from_bytes(include_bytes!("../../../assets/crust1.bin")).unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn tibet_matches_the_source_files() {
        // tools/gen_crust.py prints this cell straight from crust1.bnds, .vp and .vs:
        // bnds 4.92 4.92 4.92 4.82 4.82 4.82 -31.23 -48.54 -67.28,
        // vp 1.5 3.81 2.5 0 0 6.0 6.3 6.6, vs 0 1.94 1.07 0 0 3.52 3.68 3.82.
        let c = real().column(32.5, 88.5);
        assert!(close(c.surface_km, -4.92) && close(c.water_km, 0.0));
        assert!(close(c.moho_km, 67.28));
        let want = [
            (-4.92, -4.82, 2.5, 1.07),
            (-4.82, 31.23, 6.0, 3.52),
            (31.23, 48.54, 6.3, 3.68),
            (48.54, 67.28, 6.6, 3.82),
        ];
        assert_eq!(c.layers.len(), want.len(), "{c:?}");
        for (l, (top, bottom, vp, vs)) in c.layers.iter().zip(want) {
            assert!(close(l.top_km, top) && close(l.bottom_km, bottom), "{l:?}");
            assert!(close(l.vp_km_s, vp) && close(l.vs_km_s, vs), "{l:?}");
        }
    }

    #[test]
    fn mid_pacific_is_thin_crust_under_deep_water() {
        // bnds 0 -4.29 -4.29 -4.71 -4.71 -4.71 -5.35 -6.76 -11.13.
        let c = real().column(0.5, -150.5);
        assert!(close(c.water_km, 4.29) && close(c.surface_km, 4.29));
        assert!(close(c.moho_km, 11.13));
        assert!(close(c.layers[0].top_km, 4.29));
        assert!(close(c.layers.last().unwrap().bottom_km, 11.13));
        // The layers stack without gaps from the sea floor to the Moho.
        for w in c.layers.windows(2) {
            assert!(close(w[0].bottom_km, w[1].top_km));
        }
    }

    #[test]
    fn cells_are_found_by_their_edges_and_wrap() {
        let m = real();
        // The whole of a cell maps to it; its edges belong to the cells south and east.
        assert_eq!(m.cell_index(32.99, 88.01), m.cell_index(32.5, 88.5));
        assert_eq!(m.cell_index(33.0, 88.5), m.cell_index(32.5, 88.5));
        assert_ne!(m.cell_index(32.0, 88.5), m.cell_index(32.5, 88.5));
        assert_eq!(m.cell_index(90.0, 0.0), m.cell_index(89.5, 0.5));
        assert_eq!(m.cell_index(-90.0, 0.0), m.cell_index(-89.5, 0.5));
        assert_eq!(m.cell_index(0.5, 180.0), m.cell_index(0.5, -180.0));
        assert_eq!(m.cell_index(0.5, -180.0), 89 * 360);
        assert_eq!(m.cell_index(0.5, 179.99), 89 * 360 + 359);
    }

    #[test]
    fn every_column_is_ordered_and_has_speeds() {
        let mut n = 0;
        for c in real().columns() {
            n += 1;
            assert!(c.water_km >= 0.0 && c.moho_km > c.surface_km, "{c:?}");
            let mut top = c.surface_km;
            for l in &c.layers {
                assert!(close(l.top_km, top) && l.bottom_km > l.top_km, "{c:?}");
                assert!(l.vp_km_s > 0.0 && l.vs_km_s > 0.0, "{c:?}");
                top = l.bottom_km;
            }
            assert!(close(top, c.moho_km), "{c:?}");
        }
        assert_eq!(n, 180 * 360);
    }

    #[test]
    fn rejects_bad_files() {
        let bytes = include_bytes!("../../../assets/crust1.bin");
        let mut bad = bytes.to_vec();
        bad[0] = b'X';
        assert!(matches!(
            CrustModel::from_bytes(&bad),
            Err(TableError::BadMagic)
        ));
        assert!(matches!(
            CrustModel::from_bytes(&bytes[..bytes.len() - 1]),
            Err(TableError::Truncated(_))
        ));
        assert!(matches!(
            CrustModel::from_bytes(&bytes[..10]),
            Err(TableError::Truncated(_))
        ));
        let mut version = bytes.to_vec();
        version[8] = 9;
        assert!(matches!(
            CrustModel::from_bytes(&version),
            Err(TableError::Crust(_))
        ));
    }

    #[test]
    fn header_names_the_model_and_its_citation() {
        let m = real();
        assert_eq!(m.model(), "CRUST1.0");
        assert!(m.citation().contains("Laske"), "{}", m.citation());
    }
}
