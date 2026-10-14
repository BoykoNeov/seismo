//! The feed thread: a tokio runtime of its own that loads the cache, then polls the USGS
//! summary feeds and sends the merged event list to the app over a channel. The app only ever
//! polls the channel, so the UI thread never waits on the network.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use crossbeam_channel::{Receiver, Sender};

use crate::cache::{Cache, CacheMeta};
use crate::store::{EventStore, StoredEvent};
use crate::usgs::{FeedKind, parse};

/// How the feed thread runs.
#[derive(Debug, Clone)]
pub struct FeedConfig {
    /// Where raw downloads are kept; `None` runs without a cache.
    pub cache_dir: Option<PathBuf>,
    /// Time between polls.
    pub poll: Duration,
    /// `false` loads the cache and stops: no network at all.
    pub online: bool,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            cache_dir: Cache::default_dir(),
            poll: Duration::from_secs(60),
            online: true,
        }
    }
}

/// What one feed last did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeedState {
    /// When the data in hand was fetched (from the cache or the network).
    pub data_from: Option<DateTime<Utc>>,
    /// The last time USGS answered (new data or "not modified").
    pub last_ok: Option<DateTime<Utc>>,
    /// The last poll's error, cleared by the next success.
    pub error: Option<String>,
}

/// Every feed's state, in [`FeedKind::ALL`] order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeedStatus {
    pub feeds: [FeedState; 3],
    /// No network: the cache was loaded and the thread stopped.
    pub offline: bool,
    pub cache_dir: Option<PathBuf>,
}

impl FeedStatus {
    pub fn get(&self, feed: FeedKind) -> &FeedState {
        &self.feeds[index(feed)]
    }

    fn get_mut(&mut self, feed: FeedKind) -> &mut FeedState {
        &mut self.feeds[index(feed)]
    }
}

fn index(feed: FeedKind) -> usize {
    FeedKind::ALL
        .iter()
        .position(|&f| f == feed)
        .expect("every feed is in ALL")
}

/// A message from the feed thread.
#[derive(Debug, Clone)]
pub enum FeedMsg {
    /// The whole merged list, whenever it changes.
    Events(Vec<StoredEvent>),
    Status(FeedStatus),
}

/// The app's end of the feed.
pub struct FeedHandle {
    rx: Receiver<FeedMsg>,
}

impl FeedHandle {
    /// The next message, if one is waiting. `None` both when nothing is waiting and when the
    /// thread has stopped.
    pub fn try_recv(&self) -> Option<FeedMsg> {
        self.rx.try_recv().ok()
    }
}

/// Start the feed thread.
pub fn spawn(config: FeedConfig) -> FeedHandle {
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("feed".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("feed runtime: {e}; no live events");
                    return;
                }
            };
            runtime.block_on(run(config, tx));
        })
        .expect("spawn feed thread");
    FeedHandle { rx }
}

async fn run(config: FeedConfig, tx: Sender<FeedMsg>) {
    let cache = config.cache_dir.clone().map(Cache::new);
    let mut store = EventStore::new();
    let mut status = FeedStatus {
        offline: !config.online,
        cache_dir: config.cache_dir.clone(),
        ..Default::default()
    };
    // `If-Modified-Since` per feed: only once that feed's cached body is in the store, or a
    // "not modified" answer would leave it out.
    let mut last_modified: [Option<String>; 3] = Default::default();

    // Widest window first, so the fresher feeds fold in last.
    for feed in FeedKind::ALL.into_iter().rev() {
        let Some(cached) = cache.as_ref().and_then(|c| c.load(feed)) else {
            continue;
        };
        match parse(&cached.body) {
            Ok(events) => {
                store.apply(feed, events, cached.meta.fetched_at);
                status.get_mut(feed).data_from = Some(cached.meta.fetched_at);
                last_modified[index(feed)] = cached.meta.last_modified;
            }
            Err(e) => tracing::warn!(feed = feed.name(), "cached feed unreadable: {e}"),
        }
    }
    tracing::info!(events = store.events().len(), "feed cache loaded");
    if tx.send(FeedMsg::Events(store.events().to_vec())).is_err()
        || tx.send(FeedMsg::Status(status.clone())).is_err()
        || !config.online
    {
        return;
    }

    let client = match reqwest::Client::builder()
        .user_agent(concat!("SEISMO/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(45))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("http client: {e}; no live events");
            return;
        }
    };

    loop {
        let mut changed = false;
        for feed in FeedKind::ALL.into_iter().rev() {
            let i = index(feed);
            let state = status.get_mut(feed);
            match fetch(&client, feed, last_modified[i].as_deref()).await {
                Ok(Fetched::NotModified) => {
                    state.last_ok = Some(Utc::now());
                    state.error = None;
                }
                Ok(Fetched::Body { body, last_mod }) => {
                    let now = Utc::now();
                    match parse(&body) {
                        Ok(events) => {
                            changed |= store.apply(feed, events, now);
                            *state = FeedState {
                                data_from: Some(now),
                                last_ok: Some(now),
                                error: None,
                            };
                            let meta = CacheMeta {
                                fetched_at: now,
                                last_modified: last_mod.clone(),
                            };
                            if let Some(c) = &cache
                                && let Err(e) = c.store(feed, &body, &meta)
                            {
                                tracing::warn!(feed = feed.name(), "caching feed: {e}");
                            }
                            last_modified[i] = last_mod;
                        }
                        Err(e) => state.error = Some(format!("unreadable reply: {e}")),
                    }
                }
                Err(e) => {
                    tracing::debug!(feed = feed.name(), "fetch failed: {e}");
                    state.error = Some(e.to_string());
                }
            }
        }
        if changed {
            tracing::info!(events = store.events().len(), "feed updated");
            if tx.send(FeedMsg::Events(store.events().to_vec())).is_err() {
                return;
            }
        }
        if tx.send(FeedMsg::Status(status.clone())).is_err() {
            return;
        }
        tokio::time::sleep(config.poll).await;
    }
}

enum Fetched {
    NotModified,
    Body {
        body: Vec<u8>,
        last_mod: Option<String>,
    },
}

async fn fetch(
    client: &reqwest::Client,
    feed: FeedKind,
    if_modified_since: Option<&str>,
) -> Result<Fetched, reqwest::Error> {
    let mut req = client.get(feed.url());
    if let Some(lm) = if_modified_since {
        req = req.header(reqwest::header::IF_MODIFIED_SINCE, lm);
    }
    let resp = req.send().await?;
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    let resp = resp.error_for_status()?;
    let last_mod = resp
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    Ok(Fetched::Body {
        body: resp.bytes().await?.to_vec(),
        last_mod,
    })
}
