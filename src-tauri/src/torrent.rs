//! BitTorrent downloads (magnet links and `.torrent` files) via librqbit.
//!
//! librqbit keeps its own resume data, so torrents survive restarts and
//! network changes on their own. Finished torrents are paused, so a "done"
//! download stops using bandwidth like any other download.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ListenerOptions, ManagedTorrent, Session,
    SessionOptions, SessionPersistenceConfig, TorrentStatsState,
};
use tokio::sync::OnceCell;

use crate::engine::{DownloadInfo, Status};

const ID_PREFIX: &str = "t:";

pub struct Torrents {
    session: OnceCell<Arc<Session>>,
}

pub fn is_torrent(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    url.starts_with("magnet:") || path.ends_with(".torrent")
}

pub fn owns(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

impl Torrents {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            session: OnceCell::new(),
        })
    }

    pub async fn start(&self, data_dir: PathBuf, default_dir: PathBuf) -> Result<(), String> {
        let opts = SessionOptions {
            fastresume: true,
            persistence: Some(SessionPersistenceConfig::Json {
                folder: Some(data_dir.join("torrents")),
            }),
            listen: Some(ListenerOptions::default()),
            ..Default::default()
        };
        let session = Session::new_with_opts(default_dir, opts)
            .await
            .map_err(|e| format!("torrent engine failed to start: {e:#}"))?;
        let _ = self.session.set(session);
        Ok(())
    }

    fn session(&self) -> Result<&Arc<Session>, String> {
        self.session
            .get()
            .ok_or_else(|| "the torrent engine is still starting, try again in a moment".into())
    }

    fn handle(&self, id: &str) -> Result<Arc<ManagedTorrent>, String> {
        let num: usize = id
            .strip_prefix(ID_PREFIX)
            .and_then(|n| n.parse().ok())
            .ok_or("bad torrent id")?;
        self.session()?
            .get(num.into())
            .ok_or_else(|| "torrent not found".into())
    }

    /// Accepts a magnet link, a `.torrent` URL, or a local `.torrent` file path.
    pub async fn add(&self, url: &str, dir: PathBuf) -> Result<String, String> {
        let session = self.session()?;
        let add = if url.starts_with("magnet:") || url.starts_with("http") {
            AddTorrent::from_url(url.to_string())
        } else {
            let bytes = std::fs::read(url).map_err(|e| format!("can't read torrent file: {e}"))?;
            AddTorrent::from_bytes(bytes)
        };
        let opts = AddTorrentOptions {
            output_folder: Some(dir.to_string_lossy().into_owned()),
            overwrite: true,
            ..Default::default()
        };
        match session.add_torrent(add, Some(opts)).await {
            Ok(AddTorrentResponse::Added(id, _)) | Ok(AddTorrentResponse::AlreadyManaged(id, _)) => {
                Ok(format!("{ID_PREFIX}{id}"))
            }
            Ok(AddTorrentResponse::ListOnly(_)) => Err("torrent was only listed".into()),
            Err(e) => Err(format!("{e:#}")),
        }
    }

    pub fn list(&self) -> Vec<DownloadInfo> {
        let Ok(session) = self.session() else {
            return Vec::new();
        };
        session.with_torrents(|torrents| torrents.map(|(id, h)| info(id, h)).collect())
    }

    /// Stops seeding once a torrent is complete.
    pub async fn pause_finished(&self) {
        let Ok(session) = self.session() else {
            return;
        };
        let finished: Vec<_> = session.with_torrents(|torrents| {
            torrents
                .filter(|(_, h)| {
                    let s = h.stats();
                    s.finished && matches!(s.state, TorrentStatsState::Live)
                })
                .map(|(_, h)| h.clone())
                .collect()
        });
        for h in finished {
            let _ = session.pause(&h).await;
        }
    }

    pub async fn pause(&self, id: &str) -> Result<(), String> {
        let h = self.handle(id)?;
        self.session()?.pause(&h).await.map_err(|e| format!("{e:#}"))
    }

    pub async fn resume(&self, id: &str) -> Result<(), String> {
        let h = self.handle(id)?;
        self.session()?.unpause(&h).await.map_err(|e| format!("{e:#}"))
    }

    pub async fn remove(&self, id: &str, delete_files: bool) -> Result<(), String> {
        let h = self.handle(id)?;
        self.session()?
            .delete(h.id().into(), delete_files)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

fn info(id: usize, h: &Arc<ManagedTorrent>) -> DownloadInfo {
    let stats = h.stats();
    let name = h.name();
    let folder: &Path = h.output_folder();
    let status = if stats.finished {
        Status::Completed
    } else {
        match stats.state {
            TorrentStatsState::Paused => Status::Paused,
            TorrentStatsState::Error => Status::Error,
            _ => Status::Downloading,
        }
    };
    let live = stats.live.as_ref();
    let path = name
        .as_ref()
        .map(|n| folder.join(n))
        .filter(|p| p.exists())
        .unwrap_or_else(|| folder.to_path_buf());
    DownloadInfo {
        id: format!("{ID_PREFIX}{id}"),
        url: format!("magnet:?xt=urn:btih:{}", h.info_hash().as_string()),
        file_name: name.unwrap_or_else(|| "Fetching torrent info…".into()),
        path,
        total: (stats.total_bytes > 0).then_some(stats.total_bytes),
        downloaded: stats.progress_bytes,
        // librqbit reports MiB/s.
        speed: live.map_or(0, |l| (l.download_speed.mbps * 1024.0 * 1024.0) as u64),
        status,
        error: stats.error.clone(),
        resumable: true,
        segments: Vec::new(),
        last_try: 0,
        reconnecting: false,
        hls: false,
        parts_total: 0,
        parts_done: 0,
        torrent: true,
        peers: live.map_or(0, |l| l.snapshot.peer_stats.live as u32),
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Fetches metadata and some data for the open-licensed film "Sintel".
    /// Run with `cargo test torrent -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn magnet_starts_downloading() {
        tauri::async_runtime::block_on(async {
            let root = std::env::temp_dir().join("turbofetch-test-torrent");
            let _ = std::fs::remove_dir_all(&root);
            let torrents = Torrents::new();
            torrents.start(root.join("data"), root.join("out")).await.unwrap();
            let magnet = "magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel\
                &tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=udp%3A%2F%2Fexplodie.org%3A6969\
                &tr=wss%3A%2F%2Ftracker.webtorrent.dev&ws=https%3A%2F%2Fwebtorrent.io%2Ftorrents%2F";
            let id = torrents.add(magnet, root.join("out")).await.unwrap();

            let start = std::time::Instant::now();
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let d = torrents.list().into_iter().find(|d| d.id == id).unwrap();
                println!("{} {:?} {}/{:?} bytes, {} peers", d.file_name, d.status, d.downloaded, d.total, d.peers);
                if d.downloaded > 1_000_000 {
                    break;
                }
                assert!(start.elapsed().as_secs() < 150, "no data within timeout");
            }
            torrents.pause(&id).await.unwrap();
            torrents.remove(&id, true).await.unwrap();
        });
    }
}
