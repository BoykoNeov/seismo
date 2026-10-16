//! The feed thread: a tokio runtime of its own that loads the cache, then polls the USGS
//! summary feeds and sends the merged event list to the app over a channel. After each poll it
//! looks up the fault orientation of the listed earthquakes USGS has one for (see
//! [`crate::mechanisms`]). Between polls it answers the app's commands: searches for past
//! events, and keeping or forgetting one. The app only ever polls its channel, so the UI
//! thread never waits on the network.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use crossbeam_channel::{Receiver, Sender};
use tokio::sync::mpsc;

use crate::cache::{Cache, CacheMeta};
use crate::mechanisms::{self, Checked, Mechanisms};
use crate::search::Search;
use crate::store::{EventStore, StoredEvent};
use crate::usgs::{FeedKind, parse, to_collection};

/// How the feed thread runs.
#[derive(Debug, Clone)]
pub struct FeedConfig {
    /// Where raw downloads are kept; `None` runs without a cache.
    pub cache_dir: Option<PathBuf>,
    /// Time between polls.
    pub poll: Duration,
    /// `false` loads the cache and never polls: no network at all.
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
    /// No network: the cache was loaded and nothing is polled.
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
    /// The whole list of live and kept events, whenever it changes.
    Events(Vec<StoredEvent>),
    Status(FeedStatus),
    /// The answer to a [`FeedCmd::Search`].
    Found(SearchReply),
}

/// A search's results (handles shared with the event list), or why there are none.
#[derive(Debug, Clone)]
pub struct SearchReply {
    pub search: Search,
    pub result: Result<Vec<StoredEvent>, String>,
}

/// A command from the app.
#[derive(Debug, Clone)]
pub enum FeedCmd {
    Search(Search),
    /// Keep the event with this handle (a search result or a listed event) across restarts.
    Keep(u64),
    /// Stop keeping it.
    Forget(u64),
}

/// The app's end of the feed.
pub struct FeedHandle {
    rx: Receiver<FeedMsg>,
    cmd: mpsc::UnboundedSender<FeedCmd>,
}

impl FeedHandle {
    /// The next message, if one is waiting. `None` both when nothing is waiting and when the
    /// thread has stopped.
    pub fn try_recv(&self) -> Option<FeedMsg> {
        self.rx.try_recv().ok()
    }

    /// Send a command. `false` if the feed thread is not running.
    pub fn send(&self, cmd: FeedCmd) -> bool {
        self.cmd.send(cmd).is_ok()
    }
}

/// Start the feed thread.
pub fn spawn(config: FeedConfig) -> FeedHandle {
    let (tx, rx) = crossbeam_channel::unbounded();
    let (cmd, cmd_rx) = mpsc::unbounded_channel();
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
            runtime.block_on(run(config, tx, cmd_rx));
        })
        .expect("spawn feed thread");
    FeedHandle { rx, cmd }
}

/// The feed thread's state.
struct Feed {
    tx: Sender<FeedMsg>,
    cache: Option<Cache>,
    store: EventStore,
    status: FeedStatus,
    /// `None` when offline.
    client: Option<reqwest::Client>,
    /// Fault-orientation lookups, remembered on disk.
    mechanisms: Mechanisms,
    /// When this session last looked up each event's fault orientation, by handle.
    last_tried: HashMap<u64, DateTime<Utc>>,
}

impl Feed {
    /// Send the listed events. `false` once the app has gone.
    fn send_events(&self) -> bool {
        self.tx.send(FeedMsg::Events(self.store.listed())).is_ok()
    }

    /// Carry out one command. `false` once the app has gone.
    async fn command(&mut self, cmd: FeedCmd) -> bool {
        match cmd {
            FeedCmd::Search(search) => {
                let result = self.search(&search).await;
                let changed = matches!(result, Ok((_, true)));
                let reply = SearchReply {
                    search,
                    result: result.map(|(found, _)| found),
                };
                if changed && !self.send_events() {
                    return false;
                }
                self.tx.send(FeedMsg::Found(reply)).is_ok()
            }
            FeedCmd::Keep(handle) => {
                // A kept search result is listed from now on: look up its fault at once.
                self.set_saved(handle, true)
                    && (!self.look_up_mechanisms().await || self.send_events())
            }
            FeedCmd::Forget(handle) => self.set_saved(handle, false),
        }
    }

    /// Look up the fault orientations [`mechanisms::wanted`] picks, one request each, and
    /// remember the answers. Returns whether the listed events changed.
    async fn look_up_mechanisms(&mut self) -> bool {
        let Some(client) = self.client.clone() else {
            return false;
        };
        let now = Utc::now();
        let wanted = mechanisms::wanted(&self.store, &self.mechanisms, &self.last_tried, now);
        let (mut changed, mut learned) = (false, false);
        for r in wanted {
            self.last_tried.insert(r.handle, now);
            let id = &r.feed.event.id;
            let Ok(url) = Search::Id(id.clone()).url() else {
                continue;
            };
            let body = match fetch(&client, &url, None).await {
                Ok(Fetched::Body { body, .. }) => body,
                Ok(Fetched::NotModified) => continue,
                Err(e) => {
                    tracing::debug!(id, "fault lookup failed: {e}");
                    continue;
                }
            };
            let Some(found) = parse(&body).ok().and_then(|v| v.into_iter().next()) else {
                continue;
            };
            let mechanism = found.event.mechanism;
            self.mechanisms.record(
                id,
                Checked {
                    updated: r.feed.updated.max(found.updated),
                    mechanism,
                },
            );
            learned = true;
            if let Some(m) = mechanism {
                changed |= self.store.set_mechanism(r.handle, m);
            }
        }
        if learned {
            self.save_mechanisms();
        }
        changed
    }

    fn save_mechanisms(&self) {
        if let Some(c) = &self.cache
            && let Err(e) = c.store_mechanisms(&self.mechanisms.to_bytes())
        {
            tracing::warn!("saving fault orientations: {e}");
        }
    }

    /// Run a search; the results, and whether the listed events changed.
    async fn search(&mut self, search: &Search) -> Result<(Vec<StoredEvent>, bool), String> {
        let url = search.url().map_err(|e| e.to_string())?;
        let Some(client) = &self.client else {
            return Err("offline: searching needs the internet".into());
        };
        let body = match fetch(client, &url, None).await {
            Ok(Fetched::Body { body, .. }) => body,
            Ok(Fetched::NotModified) => return Err("USGS sent no data".into()),
            // An unknown event id is a 404.
            Err(e) if e.status() == Some(reqwest::StatusCode::NOT_FOUND) => {
                return Err("USGS has no event with that code".into());
            }
            Err(e) => return Err(format!("USGS search failed: {e}")),
        };
        let found = parse(&body).map_err(|e| format!("unreadable reply: {e}"))?;
        // A lookup by code brings the fault orientation along: remember it.
        let mut learned = false;
        for f in found.iter().filter(|f| f.mechanism_listed) {
            if let Some(m) = f.event.mechanism {
                let checked = Checked {
                    updated: f.updated,
                    mechanism: Some(m),
                };
                self.mechanisms.record(&f.event.id, checked);
                learned = true;
            }
        }
        if learned {
            self.save_mechanisms();
        }
        let (found, mut changed) = self.store.apply_search(found);
        changed |= self.mechanisms.attach(&mut self.store);
        // The results as they are now, remembered faults included.
        let found = found
            .iter()
            .filter_map(|f| {
                self.store
                    .events()
                    .iter()
                    .find(|r| r.handle == f.handle)
                    .cloned()
            })
            .collect();
        Ok((found, changed))
    }

    /// Keep or forget an event, and save the kept ones. `false` once the app has gone.
    fn set_saved(&mut self, handle: u64, saved: bool) -> bool {
        if !self.store.set_saved(handle, saved) {
            return true;
        }
        if let Some(c) = &self.cache
            && let Err(e) = c.store_saved(&to_collection(self.store.saved()))
        {
            tracing::warn!("saving kept events: {e}");
        }
        self.send_events()
    }
}

async fn run(config: FeedConfig, tx: Sender<FeedMsg>, mut cmds: mpsc::UnboundedReceiver<FeedCmd>) {
    let mut feed = Feed {
        tx,
        cache: config.cache_dir.clone().map(Cache::new),
        store: EventStore::new(),
        status: FeedStatus {
            offline: !config.online,
            cache_dir: config.cache_dir.clone(),
            ..Default::default()
        },
        client: None,
        mechanisms: Mechanisms::default(),
        last_tried: HashMap::new(),
    };
    // `If-Modified-Since` per feed: only once that feed's cached body is in the store, or a
    // "not modified" answer would leave it out.
    let mut last_modified: [Option<String>; 3] = Default::default();

    if let Some(body) = feed.cache.as_ref().and_then(Cache::load_saved) {
        match parse(&body) {
            Ok(events) => {
                feed.store.apply_saved(events);
            }
            Err(e) => tracing::warn!("kept events unreadable: {e}"),
        }
    }
    // Widest window first, so the fresher feeds fold in last.
    for kind in FeedKind::ALL.into_iter().rev() {
        let Some(cached) = feed.cache.as_ref().and_then(|c| c.load(kind)) else {
            continue;
        };
        match parse(&cached.body) {
            Ok(events) => {
                feed.store.apply(kind, events, cached.meta.fetched_at);
                feed.status.get_mut(kind).data_from = Some(cached.meta.fetched_at);
                last_modified[index(kind)] = cached.meta.last_modified;
            }
            Err(e) => tracing::warn!(feed = kind.name(), "cached feed unreadable: {e}"),
        }
    }
    if let Some(body) = feed.cache.as_ref().and_then(Cache::load_mechanisms) {
        match Mechanisms::from_bytes(&body) {
            Ok(m) => feed.mechanisms = m,
            Err(e) => tracing::warn!("remembered fault orientations unreadable: {e}"),
        }
    }
    feed.mechanisms.attach(&mut feed.store);
    tracing::info!(events = feed.store.events().len(), "feed cache loaded");
    if !feed.send_events() || feed.tx.send(FeedMsg::Status(feed.status.clone())).is_err() {
        return;
    }

    if config.online {
        match reqwest::Client::builder()
            .user_agent(concat!("SEISMO/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(45))
            .build()
        {
            Ok(c) => feed.client = Some(c),
            Err(e) => tracing::error!("http client: {e}; no live events"),
        }
    }
    let Some(client) = feed.client.clone() else {
        // Offline: nothing to poll, but commands are still answered.
        while let Some(cmd) = cmds.recv().await {
            if !feed.command(cmd).await {
                return;
            }
        }
        return;
    };

    loop {
        let mut changed = false;
        for kind in FeedKind::ALL.into_iter().rev() {
            let i = index(kind);
            let fetched = fetch(&client, &kind.url(), last_modified[i].as_deref()).await;
            let state = feed.status.get_mut(kind);
            match fetched {
                Ok(Fetched::NotModified) => {
                    state.last_ok = Some(Utc::now());
                    state.error = None;
                }
                Ok(Fetched::Body { body, last_mod }) => {
                    let now = Utc::now();
                    match parse(&body) {
                        Ok(events) => {
                            *state = FeedState {
                                data_from: Some(now),
                                last_ok: Some(now),
                                error: None,
                            };
                            changed |= feed.store.apply(kind, events, now);
                            let meta = CacheMeta {
                                fetched_at: now,
                                last_modified: last_mod.clone(),
                            };
                            if let Some(c) = &feed.cache
                                && let Err(e) = c.store(kind, &body, &meta)
                            {
                                tracing::warn!(feed = kind.name(), "caching feed: {e}");
                            }
                            last_modified[i] = last_mod;
                        }
                        Err(e) => state.error = Some(format!("unreadable reply: {e}")),
                    }
                }
                Err(e) => {
                    tracing::debug!(feed = kind.name(), "fetch failed: {e}");
                    state.error = Some(e.to_string());
                }
            }
        }
        changed |= feed.mechanisms.attach(&mut feed.store);
        if changed {
            tracing::info!(events = feed.store.events().len(), "feed updated");
            if !feed.send_events() {
                return;
            }
        }
        if feed.look_up_mechanisms().await && !feed.send_events() {
            return;
        }
        if feed.tx.send(FeedMsg::Status(feed.status.clone())).is_err() {
            return;
        }
        // Answer commands until the next poll is due.
        let next = tokio::time::Instant::now() + config.poll;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(next) => break,
                cmd = cmds.recv() => match cmd {
                    Some(cmd) => {
                        if !feed.command(cmd).await {
                            return;
                        }
                    }
                    None => return,
                },
            }
        }
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
    url: &str,
    if_modified_since: Option<&str>,
) -> Result<Fetched, reqwest::Error> {
    let mut req = client.get(url);
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
