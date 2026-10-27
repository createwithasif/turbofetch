//! Multi-connection download engine.
//!
//! A file is split into segments, each fetched over its own HTTP range request.
//! When a connection finishes its segment it "steals" half of the largest
//! remaining segment, so all connections stay busy until the very end.
//! Progress is persisted so downloads survive pause and app restarts.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_RANGE, RANGE};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

mod dash;
mod hls;
pub use hls::Variant;

/// Never split a segment into pieces smaller than this.
const MIN_SPLIT: u64 = 512 * 1024;
/// Longest wait between reconnect attempts; network errors are retried forever.
const MAX_BACKOFF: Duration = Duration::from_secs(10);
/// A server that keeps answering with a hard error (404, 403, ...) gets this many tries.
const MAX_FATAL_RETRIES: u32 = 3;
const UNKNOWN_END: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Downloading,
    Paused,
    Completed,
    Error,
}

/// A byte range `[pos, end)` still to be fetched; `start` is kept for display.
struct Segment {
    start: u64,
    pos: AtomicU64,
    end: AtomicU64,
    active: AtomicBool,
}

impl Segment {
    fn new(start: u64, pos: u64, end: u64) -> Self {
        Self {
            start,
            pos: AtomicU64::new(pos),
            end: AtomicU64::new(end),
            active: AtomicBool::new(false),
        }
    }

    fn remaining(&self) -> u64 {
        self.end
            .load(Ordering::SeqCst)
            .saturating_sub(self.pos.load(Ordering::SeqCst))
    }

    fn done(&self) -> bool {
        self.remaining() == 0
    }
}

pub struct Download {
    id: String,
    url: String,
    file_name: String,
    dir: PathBuf,
    total: Option<u64>,
    resumable: bool,
    /// Extra request headers from the browser (cookies, referrer, user agent).
    headers: Vec<(String, String)>,
    status: Mutex<Status>,
    error: Mutex<Option<String>>,
    segments: Mutex<Vec<Arc<Segment>>>,
    cancel: Mutex<Option<CancellationToken>>,
    last_bytes: AtomicU64,
    speed: AtomicU64,
    /// Unix seconds of the last start or resume.
    last_try: AtomicU64,
    /// Connections currently waiting to reconnect after a network error.
    reconnecting: AtomicU32,
    /// HLS stream: progress is counted in pieces instead of byte ranges.
    hls: bool,
    parts_total: AtomicU64,
    parts_done: AtomicU64,
    hls_bytes: AtomicU64,
}

impl Download {
    fn downloaded(&self) -> u64 {
        if self.hls {
            return self.hls_bytes.load(Ordering::SeqCst);
        }
        if *self.status.lock().unwrap() == Status::Completed {
            return self.total.unwrap_or(0);
        }
        self.segments
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.pos.load(Ordering::SeqCst).min(s.end.load(Ordering::SeqCst)) - s.start)
            .sum()
    }

    fn final_path(&self) -> PathBuf {
        self.dir.join(&self.file_name)
    }

    fn part_path(&self) -> PathBuf {
        if self.hls {
            // A hidden folder holding one file per stream piece.
            return self.dir.join(format!(".{}.parts", self.file_name));
        }
        self.dir.join(format!("{}.part", self.file_name))
    }

    fn remove_partial(&self) {
        if self.hls {
            let _ = std::fs::remove_dir_all(self.part_path());
        } else {
            let _ = std::fs::remove_file(self.part_path());
        }
    }

    fn set_status(&self, status: Status, error: Option<String>) {
        *self.status.lock().unwrap() = status;
        *self.error.lock().unwrap() = error;
    }

    fn record(&self) -> Record {
        Record {
            id: self.id.clone(),
            url: self.url.clone(),
            file_name: self.file_name.clone(),
            dir: self.dir.clone(),
            total: self.total,
            resumable: self.resumable,
            headers: self.headers.clone(),
            status: *self.status.lock().unwrap(),
            error: self.error.lock().unwrap().clone(),
            segments: self.segment_infos(),
            last_try: self.last_try.load(Ordering::SeqCst),
            hls: self.hls,
            parts_total: self.parts_total.load(Ordering::SeqCst),
            parts_done: self.parts_done.load(Ordering::SeqCst),
            hls_bytes: self.hls_bytes.load(Ordering::SeqCst),
        }
    }

    fn segment_infos(&self) -> Vec<SegmentInfo> {
        self.segments
            .lock()
            .unwrap()
            .iter()
            .map(|s| SegmentInfo {
                start: s.start,
                pos: s.pos.load(Ordering::SeqCst),
                end: s.end.load(Ordering::SeqCst),
            })
            .collect()
    }

    fn info(&self) -> DownloadInfo {
        let status = *self.status.lock().unwrap();
        DownloadInfo {
            id: self.id.clone(),
            url: self.url.clone(),
            file_name: self.file_name.clone(),
            path: if status == Status::Completed {
                self.final_path()
            } else {
                self.part_path()
            },
            total: if self.hls && status == Status::Completed {
                Some(self.hls_bytes.load(Ordering::SeqCst))
            } else {
                self.total
            },
            downloaded: self.downloaded(),
            speed: self.speed.load(Ordering::SeqCst),
            status,
            error: self.error.lock().unwrap().clone(),
            resumable: self.resumable,
            segments: self.segment_infos(),
            last_try: self.last_try.load(Ordering::SeqCst),
            reconnecting: self.reconnecting.load(Ordering::SeqCst) > 0,
            hls: self.hls,
            parts_total: self.parts_total.load(Ordering::SeqCst),
            parts_done: self.parts_done.load(Ordering::SeqCst),
            torrent: false,
            peers: 0,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SegmentInfo {
    pub start: u64,
    pub pos: u64,
    pub end: u64,
}

/// What gets written to disk between sessions.
#[derive(Serialize, Deserialize)]
struct Record {
    id: String,
    url: String,
    file_name: String,
    dir: PathBuf,
    total: Option<u64>,
    resumable: bool,
    #[serde(default)]
    headers: Vec<(String, String)>,
    status: Status,
    error: Option<String>,
    segments: Vec<SegmentInfo>,
    #[serde(default)]
    last_try: u64,
    #[serde(default)]
    hls: bool,
    #[serde(default)]
    parts_total: u64,
    #[serde(default)]
    parts_done: u64,
    #[serde(default)]
    hls_bytes: u64,
}

/// What the UI receives.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadInfo {
    pub id: String,
    pub url: String,
    pub file_name: String,
    pub path: PathBuf,
    pub total: Option<u64>,
    pub downloaded: u64,
    pub speed: u64,
    pub status: Status,
    pub error: Option<String>,
    pub resumable: bool,
    pub segments: Vec<SegmentInfo>,
    pub last_try: u64,
    pub reconnecting: bool,
    pub hls: bool,
    pub parts_total: u64,
    pub parts_done: u64,
    pub torrent: bool,
    pub peers: u32,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Connections per download.
    pub connections: usize,
    /// Downloads running at once; the rest wait in the queue.
    pub max_active: usize,
    /// Global speed limit in KB/s, 0 = unlimited.
    pub speed_limit_kb: u64,
    pub download_dir: Option<PathBuf>,
    pub clipboard_monitor: bool,
    pub browser_integration: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            connections: 16,
            max_active: 3,
            speed_limit_kb: 0,
            download_dir: None,
            clipboard_monitor: true,
            browser_integration: true,
        }
    }
}

/// Token bucket shared by every connection of every download.
struct Limiter {
    rate: AtomicU64,
    state: Mutex<(f64, Instant)>,
}

impl Limiter {
    async fn consume(&self, bytes: u64) {
        let rate = self.rate.load(Ordering::SeqCst) as f64;
        if rate == 0.0 {
            return;
        }
        let wait = {
            let mut state = self.state.lock().unwrap();
            let now = Instant::now();
            let refill = now.duration_since(state.1).as_secs_f64() * rate;
            state.0 = (state.0 + refill).min(rate) - bytes as f64;
            state.1 = now;
            // A negative balance is debt the caller pays off by sleeping.
            (state.0 < 0.0).then(|| Duration::from_secs_f64(-state.0 / rate))
        };
        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }
    }
}

pub struct Manager {
    downloads: Mutex<Vec<Arc<Download>>>,
    client: reqwest::Client,
    store_path: PathBuf,
    settings_path: PathBuf,
    settings: Mutex<Settings>,
    limiter: Arc<Limiter>,
}

impl Manager {
    pub fn new(data_dir: PathBuf) -> Arc<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("ADM/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        let settings_path = data_dir.join("settings.json");
        let settings: Settings = std::fs::read(&settings_path)
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        let limiter = Arc::new(Limiter {
            rate: AtomicU64::new(settings.speed_limit_kb * 1024),
            state: Mutex::new((0.0, Instant::now())),
        });
        let manager = Arc::new(Self {
            downloads: Mutex::new(Vec::new()),
            client,
            store_path: data_dir.join("downloads.json"),
            settings_path,
            settings: Mutex::new(settings),
            limiter,
        });
        manager.load();
        manager.pump();
        manager
    }

    fn load(&self) {
        let Ok(data) = std::fs::read(&self.store_path) else {
            return;
        };
        let Ok(records) = serde_json::from_slice::<Vec<Record>>(&data) else {
            return;
        };
        let mut downloads = self.downloads.lock().unwrap();
        for r in records {
            // Anything that was running when the app closed picks up where it left off.
            let status = if matches!(r.status, Status::Downloading | Status::Queued) {
                Status::Queued
            } else {
                r.status
            };
            downloads.push(Arc::new(Download {
                id: r.id,
                url: r.url,
                file_name: r.file_name,
                dir: r.dir,
                total: r.total,
                resumable: r.resumable,
                headers: r.headers,
                status: Mutex::new(status),
                error: Mutex::new(r.error),
                segments: Mutex::new(
                    r.segments
                        .into_iter()
                        .map(|s| Arc::new(Segment::new(s.start, s.pos, s.end)))
                        .collect(),
                ),
                cancel: Mutex::new(None),
                last_bytes: AtomicU64::new(0),
                speed: AtomicU64::new(0),
                last_try: AtomicU64::new(r.last_try),
                reconnecting: AtomicU32::new(0),
                hls: r.hls,
                parts_total: AtomicU64::new(r.parts_total),
                parts_done: AtomicU64::new(r.parts_done),
                hls_bytes: AtomicU64::new(r.hls_bytes),
            }));
        }
    }

    pub fn save(&self) {
        let records: Vec<Record> = self
            .downloads
            .lock()
            .unwrap()
            .iter()
            .map(|d| d.record())
            .collect();
        if let Some(parent) = self.store_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.store_path.with_extension("json.tmp");
        if let Ok(data) = serde_json::to_vec_pretty(&records) {
            if std::fs::write(&tmp, data).is_ok() {
                let _ = std::fs::rename(&tmp, &self.store_path);
            }
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn set_settings(self: &Arc<Self>, mut settings: Settings) -> Result<(), String> {
        settings.connections = settings.connections.clamp(1, 32);
        settings.max_active = settings.max_active.clamp(1, 20);
        self.limiter
            .rate
            .store(settings.speed_limit_kb * 1024, Ordering::SeqCst);
        let data = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
        if let Some(parent) = self.settings_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.settings_path, data).map_err(|e| e.to_string())?;
        *self.settings.lock().unwrap() = settings;
        // A higher limit may let queued downloads start now.
        self.pump();
        Ok(())
    }

    pub fn list(&self) -> Vec<DownloadInfo> {
        self.downloads.lock().unwrap().iter().map(|d| d.info()).collect()
    }

    /// Called twice a second: updates speeds and returns a snapshot for the UI.
    pub fn tick(&self, interval: Duration) -> Vec<DownloadInfo> {
        for d in self.downloads.lock().unwrap().iter() {
            let now = d.downloaded();
            let before = d.last_bytes.swap(now, Ordering::SeqCst);
            let speed = if *d.status.lock().unwrap() == Status::Downloading {
                (now.saturating_sub(before) as f64 / interval.as_secs_f64()) as u64
            } else {
                0
            };
            d.speed.store(speed, Ordering::SeqCst);
        }
        self.list()
    }

    fn get(&self, id: &str) -> Option<Arc<Download>> {
        self.downloads
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.id == id)
            .cloned()
    }

    pub async fn add(
        self: &Arc<Self>,
        url: String,
        dir: PathBuf,
        headers: Vec<(String, String)>,
        name_hint: Option<String>,
    ) -> Result<String, String> {
        if is_youtube_watch(&url) {
            return Err("YouTube watch pages do not provide direct media files (YouTube streams via SABR/UMP). Channel owners can download their own videos via YouTube Studio.".into());
        }
        let hint = name_hint.map(|n| sanitize(&n)).filter(|n| !n.is_empty());
        let probe = if hls::looks_like(&url) || dash::looks_like(&url) {
            None
        } else {
            Some(probe(&self.client, &url, &headers).await?)
        };

        let is_dash = dash::looks_like(&url) || probe.as_ref().is_some_and(|p| p.dash);
        let (url, name, total, resumable, is_hls) = match probe {
            Some(p) if !p.playlist && !p.dash => {
                let name = hint.unwrap_or(p.file_name);
                (url, name, p.total, p.resumable, false)
            }
            _ => {
                let (media_url, ext) = if is_dash {
                    (dash::pick(&self.client, &url, &headers).await?, "mp4")
                } else {
                    let (media_url, fmp4) = hls::pick_media(&self.client, &url, &headers).await?;
                    (media_url, if fmp4 || hls::ffmpeg().is_some() { "mp4" } else { "ts" })
                };
                // Playlist names like "index.m3u8" are useless, so prefer the page's title.
                let stem = hint
                    .as_deref()
                    .map(|h| h.rsplit_once('.').map_or(h, |(s, _)| s).to_string())
                    .unwrap_or_else(|| "video".into());
                (media_url, format!("{stem}.{ext}"), None, false, true)
            }
        };
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // Names of downloads still waiting or running here count as taken too.
        let listed: Vec<String> = self
            .downloads
            .lock()
            .unwrap()
            .iter()
            .filter(|d| d.dir == dir)
            .map(|d| d.file_name.clone())
            .collect();
        let file_name = unique_name(&dir, &name, &listed);

        let id = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let download = Arc::new(Download {
            id: id.clone(),
            url,
            file_name,
            dir,
            total,
            resumable,
            headers,
            status: Mutex::new(Status::Paused),
            error: Mutex::new(None),
            segments: Mutex::new(Vec::new()),
            cancel: Mutex::new(None),
            last_bytes: AtomicU64::new(0),
            speed: AtomicU64::new(0),
            last_try: AtomicU64::new(0),
            reconnecting: AtomicU32::new(0),
            hls: is_hls,
            parts_total: AtomicU64::new(0),
            parts_done: AtomicU64::new(0),
            hls_bytes: AtomicU64::new(0),
        });
        self.downloads.lock().unwrap().insert(0, download.clone());
        self.enqueue(&download);
        Ok(id)
    }

    /// Qualities offered by an HLS playlist, for the browser's quality menu.
    pub async fn hls_variants(
        &self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<Vec<Variant>, String> {
        if dash::looks_like(url) {
            return dash::variants(&self.client, url, headers).await;
        }
        match hls::variants(&self.client, url, headers).await {
            // The extension spotted it by its `dash+xml` type, not its name.
            Err(e) if e == "not an HLS playlist" => dash::variants(&self.client, url, headers).await,
            r => r,
        }
    }

    pub fn resume(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let d = self.get(id).ok_or("download not found")?;
        match *d.status.lock().unwrap() {
            Status::Paused | Status::Error => {}
            _ => return Ok(()),
        }
        self.enqueue(&d);
        Ok(())
    }

    pub fn pause(&self, id: &str) -> Result<(), String> {
        let d = self.get(id).ok_or("download not found")?;
        if *d.status.lock().unwrap() == Status::Queued {
            d.set_status(Status::Paused, None);
            self.save();
        }
        if let Some(token) = d.cancel.lock().unwrap().take() {
            token.cancel();
        }
        Ok(())
    }

    pub fn remove(&self, id: &str, delete_file: bool) -> Result<(), String> {
        let d = self.get(id).ok_or("download not found")?;
        if let Some(token) = d.cancel.lock().unwrap().take() {
            token.cancel();
        }
        self.downloads.lock().unwrap().retain(|x| x.id != id);
        if delete_file {
            d.remove_partial();
            if *d.status.lock().unwrap() == Status::Completed {
                let _ = std::fs::remove_file(d.final_path());
            }
        } else if *d.status.lock().unwrap() != Status::Completed {
            // An unfinished .part file is useless once the record is gone.
            d.remove_partial();
        }
        self.save();
        Ok(())
    }

    fn enqueue(self: &Arc<Self>, d: &Download) {
        d.set_status(Status::Queued, None);
        self.pump();
        self.save();
    }

    /// Starts queued downloads, oldest first, until `max_active` are running.
    fn pump(self: &Arc<Self>) {
        let max_active = self.settings.lock().unwrap().max_active;
        let to_start: Vec<_> = {
            let downloads = self.downloads.lock().unwrap();
            let running = downloads
                .iter()
                .filter(|d| *d.status.lock().unwrap() == Status::Downloading)
                .count();
            downloads
                .iter()
                .rev()
                .filter(|d| *d.status.lock().unwrap() == Status::Queued)
                .take(max_active.saturating_sub(running))
                .cloned()
                .collect()
        };
        for d in to_start {
            self.launch(d);
        }
    }

    fn launch(self: &Arc<Self>, d: Arc<Download>) {
        let token = CancellationToken::new();
        *d.cancel.lock().unwrap() = Some(token.clone());
        d.set_status(Status::Downloading, None);
        d.last_try.store(unix_now(), Ordering::SeqCst);
        self.save();

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let result = manager.run(&d, &token).await;
            let still_listed = manager.get(&d.id).is_some();
            match result {
                Err(e) => d.set_status(Status::Error, Some(e)),
                Ok(()) if token.is_cancelled() => d.set_status(Status::Paused, None),
                Ok(()) => {
                    d.segments.lock().unwrap().clear();
                    d.set_status(Status::Completed, None);
                }
            }
            d.cancel.lock().unwrap().take();
            if still_listed {
                manager.save();
            }
            manager.pump();
        });
    }

    async fn run(&self, d: &Arc<Download>, token: &CancellationToken) -> Result<(), String> {
        if d.hls && dash::looks_like(&d.url) {
            return dash::run(self, d, token).await;
        }
        if d.hls {
            return hls::run(self, d, token).await;
        }
        let part = d.part_path();
        let file = prepare_file(d, &part)?;

        let workers = if d.resumable && d.total.is_some() {
            self.settings.lock().unwrap().connections
        } else {
            1
        };
        let mut handles = Vec::new();
        for _ in 0..workers {
            let (d, file, token) = (d.clone(), file.clone(), token.clone());
            let (client, limiter) = (self.client.clone(), self.limiter.clone());
            handles.push(tauri::async_runtime::spawn(async move {
                worker(&client, &limiter, &d, &file, &token).await
            }));
        }

        let mut first_error = None;
        for h in handles {
            if let Err(e) = h.await.map_err(|e| e.to_string()).and_then(|r| r) {
                first_error.get_or_insert(e);
                // One connection gave up; stop the rest so we can report it.
                token.cancel();
            }
        }
        if let Some(e) = first_error {
            // Not a user pause: let `start` report this as an error.
            return Err(e);
        }
        if token.is_cancelled() {
            return Ok(());
        }
        if !d.segments.lock().unwrap().iter().all(|s| s.done()) {
            return Err("download ended early".into());
        }
        if !d.resumable {
            // A restarted non-resumable download may leave stale bytes past the real end.
            let len = d.segments.lock().unwrap().first().map_or(0, |s| s.pos.load(Ordering::SeqCst));
            file.set_len(len).map_err(|e| e.to_string())?;
        }
        drop(file);
        std::fs::rename(&part, d.final_path()).map_err(|e| e.to_string())
    }
}

/// Opens the `.part` file and lays out the initial segments if needed.
fn prepare_file(d: &Download, part: &Path) -> Result<Arc<File>, String> {
    let mut segments = d.segments.lock().unwrap();
    let fresh = segments.is_empty() || !d.resumable || !part.exists();

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(fresh)
        .open(part)
        .map_err(|e| e.to_string())?;

    if fresh {
        segments.clear();
        match (d.resumable, d.total) {
            (true, Some(total)) => {
                file.set_len(total).map_err(|e| e.to_string())?;
                let count = (total / MIN_SPLIT).clamp(1, 8);
                let size = total / count;
                for i in 0..count {
                    let start = i * size;
                    let end = if i == count - 1 { total } else { start + size };
                    segments.push(Arc::new(Segment::new(start, start, end)));
                }
            }
            _ => segments.push(Arc::new(Segment::new(0, 0, d.total.unwrap_or(UNKNOWN_END)))),
        }
    }
    for s in segments.iter() {
        s.active.store(false, Ordering::SeqCst);
    }
    Ok(Arc::new(file))
}

/// One connection: keeps claiming or stealing segments until nothing is left.
async fn worker(
    client: &reqwest::Client,
    limiter: &Limiter,
    d: &Download,
    file: &File,
    token: &CancellationToken,
) -> Result<(), String> {
    while let Some(seg) = claim_segment(d) {
        let result = fetch_segment(client, limiter, d, &seg, file, token).await;
        seg.active.store(false, Ordering::SeqCst);
        result?;
        if token.is_cancelled() {
            break;
        }
    }
    Ok(())
}

fn claim_segment(d: &Download) -> Option<Arc<Segment>> {
    let mut segments = d.segments.lock().unwrap();

    // An idle unfinished segment (e.g. left over from a previous session).
    for s in segments.iter() {
        if !s.done()
            && s.active
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            return Some(s.clone());
        }
    }

    // Otherwise split the busiest segment in half and take the back half.
    if !d.resumable {
        return None;
    }
    let victim = segments.iter().max_by_key(|s| s.remaining())?.clone();
    let remaining = victim.remaining();
    if remaining < 2 * MIN_SPLIT {
        return None;
    }
    let mid = victim.pos.load(Ordering::SeqCst) + remaining / 2;
    let old_end = victim.end.swap(mid, Ordering::SeqCst);
    let new = Arc::new(Segment::new(mid, mid, old_end));
    new.active.store(true, Ordering::SeqCst);
    segments.push(new.clone());
    Some(new)
}

/// Why a transfer stopped: network trouble is always retried, a hard server error is not.
enum Failure {
    Network(String),
    Fatal(String),
}

async fn fetch_segment(
    client: &reqwest::Client,
    limiter: &Limiter,
    d: &Download,
    seg: &Segment,
    file: &File,
    token: &CancellationToken,
) -> Result<(), String> {
    let mut backoff_step = 0u32;
    let mut fatal_tries = 0u32;
    loop {
        if seg.done() || token.is_cancelled() {
            return Ok(());
        }
        let before = seg.pos.load(Ordering::SeqCst);
        let result = stream_range(client, limiter, d, seg, file, token).await;
        if seg.pos.load(Ordering::SeqCst) > before {
            // We made progress, so the connection was healthy: retry quickly next time.
            backoff_step = 0;
            fatal_tries = 0;
            if d.reconnecting.load(Ordering::SeqCst) == 0 {
                d.error.lock().unwrap().take();
            }
        }
        match result {
            Ok(()) if seg.done() || token.is_cancelled() => return Ok(()),
            Ok(()) if seg.end.load(Ordering::SeqCst) == UNKNOWN_END => {
                // Size was unknown: the server closing the stream means we're done.
                seg.end.store(seg.pos.load(Ordering::SeqCst), Ordering::SeqCst);
                return Ok(());
            }
            // Closed early (Wi-Fi switch, VPN, sleep...): reconnect and continue.
            Ok(()) => {}
            Err(Failure::Network(e)) => *d.error.lock().unwrap() = Some(e),
            Err(Failure::Fatal(e)) => {
                fatal_tries += 1;
                if fatal_tries >= MAX_FATAL_RETRIES {
                    return Err(e);
                }
            }
        }
        if !d.resumable {
            // Without range support the only option is starting over.
            seg.pos.store(0, Ordering::SeqCst);
        }

        backoff_step += 1;
        let backoff = Duration::from_millis(500 * 2u64.pow(backoff_step.min(5))).min(MAX_BACKOFF);
        d.reconnecting.fetch_add(1, Ordering::SeqCst);
        let cancelled = tokio::select! {
            _ = tokio::time::sleep(backoff) => false,
            _ = token.cancelled() => true,
        };
        d.reconnecting.fetch_sub(1, Ordering::SeqCst);
        if cancelled {
            return Ok(());
        }
    }
}

async fn stream_range(
    client: &reqwest::Client,
    limiter: &Limiter,
    d: &Download,
    seg: &Segment,
    file: &File,
    token: &CancellationToken,
) -> Result<(), Failure> {
    let mut req = with_headers(client.get(&d.url), &d.headers);
    if d.resumable {
        let pos = seg.pos.load(Ordering::SeqCst);
        let end = seg.end.load(Ordering::SeqCst);
        req = req.header(RANGE, format!("bytes={}-{}", pos, end - 1));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| Failure::Network(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        let msg = format!("server returned {status}");
        // Timeouts, rate limits and 5xx are temporary; 403/404/410 mean the link is dead.
        return Err(match status.as_u16() {
            408 | 425 | 429 | 500..=599 => Failure::Network(msg),
            _ => Failure::Fatal(msg),
        });
    }
    if d.resumable && status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(Failure::Fatal("server stopped supporting resume".into()));
    }

    let mut stream = resp.bytes_stream();
    loop {
        let chunk = tokio::select! {
            c = stream.next() => c,
            _ = token.cancelled() => return Ok(()),
        };
        let Some(chunk) = chunk else {
            return Ok(());
        };
        let chunk = chunk.map_err(|e| Failure::Network(e.to_string()))?;

        // `end` can shrink at any moment when another connection steals from us.
        let pos = seg.pos.load(Ordering::SeqCst);
        let end = seg.end.load(Ordering::SeqCst);
        if pos >= end {
            return Ok(());
        }
        let n = (chunk.len() as u64).min(end - pos) as usize;
        // Disk errors (full disk, removed drive) won't fix themselves by retrying.
        write_at(file, &chunk[..n], pos).map_err(|e| Failure::Fatal(e.to_string()))?;
        seg.pos.fetch_add(n as u64, Ordering::SeqCst);
        tokio::select! {
            _ = limiter.consume(n as u64) => {}
            _ = token.cancelled() => return Ok(()),
        }
    }
}

fn with_headers(
    mut req: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        req = req.header(name.as_str(), value.as_str());
    }
    req
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(unix)]
fn write_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

#[cfg(windows)]
fn write_at(file: &File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

struct Probe {
    total: Option<u64>,
    resumable: bool,
    file_name: String,
    /// The link is an HLS playlist served without a `.m3u8` name.
    playlist: bool,
    /// The link is a DASH manifest served without a `.mpd` name.
    dash: bool,
}

/// Asks for the first byte to learn the size, range support and file name.
async fn probe(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
) -> Result<Probe, String> {
    let resp = with_headers(client.get(url), headers)
        .header(RANGE, "bytes=0-0")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("server returned {}", resp.status()));
    }

    let header = |name| resp.headers().get(name).and_then(|v| v.to_str().ok());
    let (total, resumable) = if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        // "bytes 0-0/12345"
        let total = header(CONTENT_RANGE)
            .and_then(|v| v.rsplit('/').next())
            .and_then(|v| v.parse().ok());
        (total, total.is_some())
    } else {
        (resp.content_length(), false)
    };

    let file_name = header(CONTENT_DISPOSITION)
        .and_then(filename_from_disposition)
        .or_else(|| {
            resp.url()
                .path_segments()
                .and_then(|mut s| s.next_back())
                .filter(|s| !s.is_empty())
                .map(percent_decode)
        })
        .map(|n| sanitize(&n))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "download".into());

    let playlist = header(reqwest::header::CONTENT_TYPE)
        .is_some_and(|t| t.to_ascii_lowercase().contains("mpegurl"));
    let dash = header(reqwest::header::CONTENT_TYPE).is_some_and(dash::is_dash_type);
    Ok(Probe {
        total,
        resumable,
        file_name,
        playlist,
        dash,
    })
}

fn is_youtube_watch(url: &str) -> bool {
    if let Ok(parsed) = reqwest::Url::parse(url) {
        let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
        if host == "youtube.com" || host.ends_with(".youtube.com") {
            let path = parsed.path();
            if path == "/watch" || path.starts_with("/shorts/") || path.starts_with("/live/") {
                return true;
            }
        }
        if host == "youtu.be" {
            return true;
        }
    }
    false
}

fn filename_from_disposition(value: &str) -> Option<String> {
    // Prefer RFC 5987 `filename*=UTF-8''name`, fall back to `filename="name"`.
    let mut plain = None;
    for part in value.split(';').map(str::trim) {
        if let Some(v) = part.strip_prefix("filename*=") {
            let v = v.split("''").nth(1).unwrap_or(v);
            return Some(percent_decode(v.trim_matches('"')));
        }
        if let Some(v) = part.strip_prefix("filename=") {
            plain = Some(v.trim_matches('"').to_string());
        }
    }
    plain
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .to_string()
}

/// "file.zip" -> "file (1).zip" if the name (or its .part) is already taken.
fn unique_name(dir: &Path, name: &str, listed: &[String]) -> String {
    let taken = |n: &str| {
        dir.join(n).exists()
            || dir.join(format!("{n}.part")).exists()
            || dir.join(format!(".{n}.parts")).exists()
            || listed.iter().any(|l| l == n)
    };
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..)
        .map(|i| format!("{stem} ({i}){ext}"))
        .find(|n| !taken(n))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serves one fixed HTTP response on 127.0.0.1 and returns its link.
    fn serve_once(head: &'static str, body: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.read(&mut [0u8; 4096]);
            let reply = format!("{head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = s.write_all(reply.as_bytes());
        });
        format!("http://{addr}/play?id=7")
    }

    fn probe_local(head: &'static str, body: &'static str) -> Probe {
        let url = serve_once(head, body);
        tauri::async_runtime::block_on(probe(&reqwest::Client::new(), &url, &[])).unwrap()
    }

    /// Local DASH server: `/ok.mpd` has two slow working pieces, `/dead.mpd`
    /// points at pieces that answer 404.
    fn serve_dash() -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut s = stream;
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let mpd = |dir: &str| {
                        format!(
                            r#"<MPD type="static" mediaPresentationDuration="PT8S"><Period>
                            <AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1">
                            <SegmentTemplate duration="4" initialization="/{dir}/init" media="/{dir}/$Number$"/>
                            </Representation></AdaptationSet></Period></MPD>"#
                        )
                    };
                    let (status, body) = match path.as_str() {
                        "/ok.mpd" => ("200 OK", mpd("ok")),
                        "/dead.mpd" => ("200 OK", mpd("dead")),
                        p if p.starts_with("/ok/") => {
                            std::thread::sleep(Duration::from_millis(700));
                            ("200 OK", format!("[{}]", &p[4..]))
                        }
                        _ => ("404 Not Found", String::new()),
                    };
                    let reply = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/dash+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = s.write_all(reply.as_bytes());
                });
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn dash_uses_queue_progress_and_reports_failures() {
        let dir = std::env::temp_dir().join("turbofetch-test-dash-local");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = Manager::new(dir.clone());
        let mut settings = manager.settings();
        settings.max_active = 1;
        manager.set_settings(settings).unwrap();

        let base = serve_dash();
        let add = |path: &str| {
            let id = tauri::async_runtime::block_on(manager.add(format!("{base}{path}"), dir.clone(), Vec::new(), None))
                .unwrap();
            manager.get(&id).unwrap()
        };
        let ok = add("/ok.mpd");
        let dead = add("/dead.mpd");
        assert_eq!(*dead.status.lock().unwrap(), Status::Queued, "only one may run at a time");

        let wait = |d: &Download, want: Status| {
            let start = std::time::Instant::now();
            while *d.status.lock().unwrap() != want {
                assert!(start.elapsed() < Duration::from_secs(60), "{:?}: {:?}", d.status.lock().unwrap(), d.error.lock().unwrap());
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        wait(&ok, Status::Completed);
        assert_eq!(std::fs::read_to_string(ok.final_path()).unwrap(), "[init][1][2]");
        assert_eq!(ok.parts_done.load(Ordering::SeqCst), 3);
        assert_eq!(ok.parts_total.load(Ordering::SeqCst), 3);
        assert!(!ok.part_path().exists(), "pieces folder should be cleaned up");

        wait(&dead, Status::Error);
        let error = dead.error.lock().unwrap().clone().unwrap_or_default();
        assert!(error.contains("404"), "{error}");
        assert!(!dead.final_path().exists(), "a failed download must not leave a finished file");
    }

    #[test]
    fn probe_spots_extensionless_dash() {
        let p = probe_local("HTTP/1.1 200 OK\r\nContent-Type: application/dash+xml", "<MPD/>");
        assert!(p.dash);
        assert!(!p.playlist);
    }

    #[test]
    fn probe_keeps_hls_and_file_behaviour() {
        let p = probe_local("HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl", "#EXTM3U\n");
        assert!(p.playlist);
        assert!(!p.dash);

        let p = probe_local(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: video/mp4\r\nContent-Range: bytes 0-0/5000",
            "x",
        );
        assert!(!p.playlist && !p.dash);
        assert!(p.resumable);
        assert_eq!(p.total, Some(5000));
        assert_eq!(p.file_name, "play");
    }

    #[test]
    fn parses_disposition() {
        assert_eq!(
            filename_from_disposition("attachment; filename=\"a b.zip\"").as_deref(),
            Some("a b.zip")
        );
        assert_eq!(
            filename_from_disposition("attachment; filename*=UTF-8''caf%C3%A9.txt").as_deref(),
            Some("café.txt")
        );
    }

    #[test]
    fn decodes_and_sanitizes() {
        assert_eq!(percent_decode("my%20file.iso"), "my file.iso");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(sanitize("../evil/name.txt"), "_evil_name.txt");
    }

    #[test]
    fn detects_youtube_watch_urls() {
        assert!(is_youtube_watch("https://www.youtube.com/watch?v=aqz-KE-bpKQ"));
        assert!(is_youtube_watch("https://m.youtube.com/watch?v=aqz-KE-bpKQ&feature=share"));
        assert!(is_youtube_watch("https://youtu.be/aqz-KE-bpKQ"));
        assert!(is_youtube_watch("https://www.youtube.com/shorts/12345678901"));
        assert!(!is_youtube_watch("https://video-downloads.googleusercontent.com/xyz.mp4"));
        assert!(!is_youtube_watch("https://rr5---sn-1vob-aix6.googlevideo.com/videoplayback?expire=..."));
        assert!(!is_youtube_watch("https://example.com/video.mp4"));
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Real network download; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_file_in_segments() {
        let dir = std::env::temp_dir().join("turbofetch-test");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = Manager::new(dir.clone());
        let url = "https://proof.ovh.net/files/10Mb.dat".to_string();
        let id = tauri::async_runtime::block_on(manager.add(url, dir.clone(), Vec::new(), None)).unwrap();

        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let d = manager.get(&id).unwrap();
            let status = *d.status.lock().unwrap();
            println!("{:?} {} bytes, {} segments", status, d.downloaded(), d.segments.lock().unwrap().len());
            match status {
                Status::Completed => break,
                Status::Error => panic!("{:?}", d.error.lock().unwrap()),
                _ => assert!(start.elapsed() < Duration::from_secs(120), "timed out"),
            }
        }
        let d = manager.get(&id).unwrap();
        let len = std::fs::metadata(d.final_path()).unwrap().len();
        assert_eq!(Some(len), d.total);
        println!("done in {:?}", start.elapsed());
    }

    /// Pause halfway, resume, and check the file is still byte-perfect in size.
    #[test]
    #[ignore]
    fn pause_and_resume() {
        let dir = std::env::temp_dir().join("turbofetch-test-resume");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = Manager::new(dir.clone());
        let url = "https://proof.ovh.net/files/10Mb.dat".to_string();
        let id = tauri::async_runtime::block_on(manager.add(url, dir.clone(), Vec::new(), None)).unwrap();

        std::thread::sleep(Duration::from_millis(1500));
        manager.pause(&id).unwrap();
        std::thread::sleep(Duration::from_millis(800));
        let d = manager.get(&id).unwrap();
        assert_eq!(*d.status.lock().unwrap(), Status::Paused);
        let paused_at = d.downloaded();
        println!("paused at {paused_at} bytes");

        manager.resume(&id).unwrap();
        let start = std::time::Instant::now();
        while *d.status.lock().unwrap() != Status::Completed {
            assert!(start.elapsed() < Duration::from_secs(120), "timed out");
            assert_ne!(*d.status.lock().unwrap(), Status::Error, "{:?}", d.error.lock().unwrap());
            std::thread::sleep(Duration::from_millis(300));
        }
        let len = std::fs::metadata(d.final_path()).unwrap().len();
        assert_eq!(Some(len), d.total);
    }

    /// Real HLS stream: lowest quality of a public test stream, joined into one file.
    #[test]
    #[ignore]
    fn downloads_hls_stream() {
        let dir = std::env::temp_dir().join("turbofetch-test-hls");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = Manager::new(dir.clone());
        let master = "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8";
        let variants = tauri::async_runtime::block_on(manager.hls_variants(master, &[])).unwrap();
        println!("qualities: {:?}", variants.iter().map(|v| &v.label).collect::<Vec<_>>());
        let lowest = variants.last().unwrap().url.clone();
        let id = tauri::async_runtime::block_on(manager.add(lowest, dir.clone(), Vec::new(), Some("bunny.m3u8".into())))
            .unwrap();

        let d = manager.get(&id).unwrap();
        let start = std::time::Instant::now();
        while *d.status.lock().unwrap() != Status::Completed {
            assert!(start.elapsed() < Duration::from_secs(300), "timed out");
            assert_ne!(*d.status.lock().unwrap(), Status::Error, "{:?}", d.error.lock().unwrap());
            std::thread::sleep(Duration::from_millis(500));
        }
        let len = std::fs::metadata(d.final_path()).unwrap().len();
        println!("{} -> {len} bytes in {:?}", d.file_name, start.elapsed());
        assert!(len > 1_000_000);
        assert!(!d.part_path().exists(), "pieces folder should be cleaned up");
    }

    /// Real DASH streams, downloaded in full through the manager and merged by
    /// ffmpeg: one file per quality, and numbered `$Number$` segments.
    /// Checks picture + sound, length against the manifest and a clean decode.
    #[test]
    #[ignore]
    fn downloads_dash_stream() {
        let ffmpeg = hls::ffmpeg().expect("ffmpeg must be installed for this test");
        let ffprobe = ffmpeg.with_file_name("ffprobe");
        let dir = std::env::temp_dir().join("turbofetch-test-dash");
        let _ = std::fs::remove_dir_all(&dir);
        let manager = Manager::new(dir.clone());
        // (manifest, length in seconds from its mediaPresentationDuration)
        for (mpd, secs) in [
            ("https://dash.akamaized.net/dash264/TestCases/1a/netflix/exMPD_BIP_TC1.mpd", 654.0),
            ("https://dash.akamaized.net/akamai/bbb_30fps/bbb_30fps.mpd", 634.6),
        ] {
            let variants = tauri::async_runtime::block_on(manager.hls_variants(mpd, &[])).unwrap();
            println!("qualities: {:?}", variants.iter().map(|v| &v.label).collect::<Vec<_>>());
            let lowest = variants.last().unwrap().url.clone();
            let id = tauri::async_runtime::block_on(manager.add(lowest, dir.clone(), Vec::new(), Some("dash.mpd".into())))
                .unwrap();

            let d = manager.get(&id).unwrap();
            let start = std::time::Instant::now();
            while *d.status.lock().unwrap() != Status::Completed {
                assert!(start.elapsed() < Duration::from_secs(600), "timed out");
                assert_ne!(*d.status.lock().unwrap(), Status::Error, "{:?}", d.error.lock().unwrap());
                std::thread::sleep(Duration::from_millis(500));
            }
            let path = d.final_path();
            let len = std::fs::metadata(&path).unwrap().len();
            let (done, total) = (d.parts_done.load(Ordering::SeqCst), d.parts_total.load(Ordering::SeqCst));
            println!("{} -> {len} bytes, {done}/{total} pieces in {:?}", d.file_name, start.elapsed());
            assert_eq!(done, total, "progress should reach every piece");
            assert!(!d.part_path().exists(), "pieces folder should be cleaned up");

            let probe = std::process::Command::new(&ffprobe)
                .args(["-v", "error", "-show_entries", "stream=codec_type:format=duration", "-of", "csv=p=0"])
                .arg(&path)
                .output()
                .unwrap();
            let out = String::from_utf8_lossy(&probe.stdout);
            println!("ffprobe: {}", out.split_whitespace().collect::<Vec<_>>().join(" "));
            assert!(out.contains("video") && out.contains("audio"), "needs both picture and sound");
            let length: f64 = out.lines().filter_map(|l| l.trim().parse().ok()).next().unwrap();
            assert!((length - secs).abs() < 2.0, "length {length}s, manifest says {secs}s");

            let decode = std::process::Command::new(&ffmpeg)
                .args(["-v", "error", "-i"])
                .arg(&path)
                .args(["-f", "null", "-"])
                .output()
                .unwrap();
            let errors = String::from_utf8_lossy(&decode.stderr);
            assert!(decode.status.success() && errors.trim().is_empty(), "decode errors: {errors}");
        }
    }
}
