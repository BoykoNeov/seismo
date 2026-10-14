//! Raw feed downloads kept on disk, so the app starts and works offline: one `<feed>.geojson`
//! body and one `<feed>.meta.json` (when it was fetched, and USGS's `Last-Modified`) per feed.

use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::usgs::FeedKind;

/// A cached download.
#[derive(Debug, Clone, PartialEq)]
pub struct Cached {
    pub body: Vec<u8>,
    pub meta: CacheMeta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheMeta {
    pub fetched_at: DateTime<Utc>,
    /// USGS's `Last-Modified` header, sent back as `If-Modified-Since`.
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `<platform data dir>/usgs`, e.g. `%APPDATA%\SEISMO\data\usgs` on Windows; `None` when
    /// the platform has no home directory.
    pub fn default_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "SEISMO").map(|d| d.data_dir().join("usgs"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn paths(&self, feed: FeedKind) -> (PathBuf, PathBuf) {
        (
            self.dir.join(format!("{}.geojson", feed.name())),
            self.dir.join(format!("{}.meta.json", feed.name())),
        )
    }

    /// The cached download of `feed`, if there is a complete one.
    pub fn load(&self, feed: FeedKind) -> Option<Cached> {
        let (body, meta) = self.paths(feed);
        let meta: CacheMeta = serde_json::from_slice(&std::fs::read(meta).ok()?).ok()?;
        Some(Cached {
            body: std::fs::read(body).ok()?,
            meta,
        })
    }

    /// Save a download. The body is written before the meta file, each through a temporary
    /// file and a rename, so a crash never leaves a meta file describing a half-written body.
    pub fn store(&self, feed: FeedKind, body: &[u8], meta: &CacheMeta) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let (body_path, meta_path) = self.paths(feed);
        write_atomic(&body_path, body)?;
        let meta = serde_json::to_vec_pretty(meta).map_err(io::Error::other)?;
        write_atomic(&meta_path, &meta)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("seismo-cache-test-{}", std::process::id()));
        let cache = Cache::new(&dir);
        assert_eq!(cache.load(FeedKind::AllDay), None);
        let meta = CacheMeta {
            fetched_at: Utc.with_ymd_and_hms(2026, 10, 14, 17, 11, 16).unwrap(),
            last_modified: Some("Wed, 14 Oct 2026 17:11:16 GMT".into()),
        };
        cache
            .store(FeedKind::AllDay, b"{\"features\": []}", &meta)
            .unwrap();
        let got = cache.load(FeedKind::AllDay).unwrap();
        assert_eq!(got.body, b"{\"features\": []}");
        assert_eq!(got.meta, meta);
        // Other feeds are separate.
        assert_eq!(cache.load(FeedKind::AllHour), None);
        // A body without its meta file is not a complete download.
        std::fs::remove_file(dir.join("all_day.meta.json")).unwrap();
        assert_eq!(cache.load(FeedKind::AllDay), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
