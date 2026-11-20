use crate::models::{
    magnet_display_name, DownloadItem, DownloadStatus, TorrentFileEntry, TorrentSource,
    APP_VERSION,
};
use chrono::Utc;
use librqbit::api::TorrentIdOrHash;
use librqbit::limits::LimitsConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ConnectionOptions, ListenerMode,
    ListenerOptions, PeerConnectionOptions, Session, SessionOptions, SessionPersistenceConfig,
    TorrentStatsState,
};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, RwLock};

/// Force HTTP trackers to re-announce periodically without triggering tracker rate-limits.
const DEFAULT_FORCE_TRACKER_INTERVAL: Duration = Duration::from_secs(90);
/// Raise default per-torrent peer concurrency limit from librqbit's 128 to 200 for active swarms.
const DEFAULT_PEER_LIMIT: usize = 200;
/// Fail unreachable peers quickly so connection slots recycle toward active peers/seeders.
const DEFAULT_PEER_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// Keep temporarily choked/quiet seeders connected across normal BitTorrent choke rotations (10-30s)
/// and standard 60-120s peer keep-alive intervals instead of prematurely killing & backing them off.
const DEFAULT_PEER_RW_TIMEOUT: Duration = Duration::from_secs(90);
/// Proactively keep NAT/TCP/uTP peer connections alive well before remote clients time out.
const DEFAULT_PEER_KEEP_ALIVE: Duration = Duration::from_secs(45);
/// Deeper per-peer in-flight chunk pipeline (256 * 16 KB = 4 MB, clamped by peer `reqq`) to avoid
/// pipeline bubbles on high-latency/high-bandwidth seeders.
const DEFAULT_PEER_REQUEST_WINDOW: usize = 256;
/// EWMA smoothing factor applied at 250ms ticks on top of librqbit's 5s piece-completion estimator
/// so multi-MB piece completions produce smooth, stable speed & ETA readings.
const TORRENT_SPEED_EWMA_ALPHA: f64 = 0.22;

/// Pure-Rust BitTorrent session manager built on `librqbit 9.1` (DHT + HTTP/UDP Trackers +
/// BEP-9 `ut_metadata` Magnet Links + uTP/TCP Peer Wire + Fast-Resume persistence).
pub struct TorrentManager {
    session: RwLock<Option<Arc<Session>>>,
    init_lock: AsyncMutex<()>,
    default_download_dir: PathBuf,
    persistence_dir: PathBuf,
    enable_dht: bool,
    enable_persistence: bool,
}

impl TorrentManager {
    pub fn new(
        default_download_dir: PathBuf,
        data_dir: &Path,
        enable_dht: bool,
        enable_persistence: bool,
    ) -> Self {
        let persistence_dir = data_dir.join("torrents");
        let _ = std::fs::create_dir_all(&persistence_dir);
        Self {
            session: RwLock::new(None),
            init_lock: AsyncMutex::new(()),
            default_download_dir,
            persistence_dir,
            enable_dht,
            enable_persistence,
        }
    }

    /// Initialize the underlying `librqbit::Session` lazily or eagerly at startup (strictly single-init).
    pub async fn init(&self, initial_dl_limit_bps: u64) -> Result<Arc<Session>, String> {
        {
            if let Some(existing) = self.session.read().await.as_ref() {
                return Ok(existing.clone());
            }
        }

        let _init_guard = self.init_lock.lock().await;
        {
            if let Some(existing) = self.session.read().await.as_ref() {
                return Ok(existing.clone());
            }
        }

        let _ = std::fs::create_dir_all(&self.default_download_dir);
        let download_bps = NonZeroU32::new(initial_dl_limit_bps.min(u32::MAX as u64) as u32);
        let peer_opts = PeerConnectionOptions {
            connect_timeout: Some(DEFAULT_PEER_CONNECT_TIMEOUT),
            read_write_timeout: Some(DEFAULT_PEER_RW_TIMEOUT),
            keep_alive_interval: Some(DEFAULT_PEER_KEEP_ALIVE),
            max_request_window: Some(DEFAULT_PEER_REQUEST_WINDOW),
        };

        let build_opts = |listen: Option<ListenerOptions>, ipv4_only: bool| SessionOptions {
            dht: if self.enable_dht {
                Some(Default::default())
            } else {
                None
            },
            fastresume: self.enable_persistence,
            persistence: if self.enable_persistence {
                Some(SessionPersistenceConfig::Json {
                    folder: Some(self.persistence_dir.clone()),
                })
            } else {
                None
            },
            listen,
            connect: Some(ConnectionOptions {
                enable_tcp: true,
                proxy_url: None,
                peer_opts: Some(peer_opts),
            }),
            runtime_worker_threads: Some(8),
            ratelimits: LimitsConfig {
                download_bps,
                upload_bps: None,
            },
            peer_limit: Some(DEFAULT_PEER_LIMIT),
            disable_local_service_discovery: !self.enable_dht,
            ipv4_only,
            client_name_and_version: Some(format!("VortexDM {}", APP_VERSION)),
            ..Default::default()
        };

        // In production mode (enable_dht=true), bind an ephemeral TCP+uTP listener with UPnP
        // forwarding so incoming peers can connect and outgoing uTP sockets are enabled.
        // Fall back cleanly to IPv4-only listener or outgoing-only TCP if binding fails.
        let sess = if self.enable_dht {
            let dualstack_listen = ListenerOptions {
                mode: ListenerMode::TcpAndUtp,
                listen_addr: (Ipv6Addr::UNSPECIFIED, 0).into(),
                enable_upnp_port_forwarding: true,
                ..Default::default()
            };
            match Session::new_with_opts(
                self.default_download_dir.clone(),
                build_opts(Some(dualstack_listen), false),
            )
            .await
            {
                Ok(s) => s,
                Err(_) => {
                    let v4_listen = ListenerOptions {
                        mode: ListenerMode::TcpAndUtp,
                        listen_addr: (Ipv4Addr::UNSPECIFIED, 0).into(),
                        enable_upnp_port_forwarding: true,
                        ipv4_only: true,
                        ..Default::default()
                    };
                    match Session::new_with_opts(
                        self.default_download_dir.clone(),
                        build_opts(Some(v4_listen), true),
                    )
                    .await
                    {
                        Ok(s) => s,
                        Err(_) => Session::new_with_opts(
                            self.default_download_dir.clone(),
                            build_opts(None, false),
                        )
                        .await
                        .map_err(|e| format!("Failed starting BitTorrent session: {}", e))?,
                    }
                }
            }
        } else {
            Session::new_with_opts(self.default_download_dir.clone(), build_opts(None, false))
                .await
                .map_err(|e| format!("Failed starting BitTorrent session: {}", e))?
        };

        let mut guard = self.session.write().await;
        *guard = Some(sess.clone());
        Ok(sess)
    }

    pub async fn get_session(&self) -> Option<Arc<Session>> {
        self.session.read().await.clone()
    }

    pub async fn set_download_limit(&self, bps: u64) {
        if let Some(sess) = self.get_session().await {
            let lim = NonZeroU32::new(bps.min(u32::MAX as u64) as u32);
            sess.ratelimits.set_download_bps(lim);
        }
    }

    /// Add a torrent from either a `magnet:?` URI, an HTTP `.torrent` URL, or a local `.torrent` file.
    pub async fn add_torrent(
        &self,
        source: &TorrentSource,
        save_dir: Option<PathBuf>,
        start_paused: bool,
        initial_dl_limit_bps: u64,
    ) -> Result<(usize, String, String, PathBuf), String> {
        let sess = self.init(initial_dl_limit_bps).await?;

        let out_dir = save_dir.unwrap_or_else(|| self.default_download_dir.clone());
        let _ = std::fs::create_dir_all(&out_dir);

        let opts = AddTorrentOptions {
            paused: start_paused,
            overwrite: true,
            output_folder: Some(out_dir.to_string_lossy().to_string()),
            force_tracker_interval: Some(DEFAULT_FORCE_TRACKER_INTERVAL),
            peer_limit: Some(DEFAULT_PEER_LIMIT),
            ..Default::default()
        };

        let (add_req, fallback_name) = match source {
            TorrentSource::MagnetOrUrl(url) => {
                let name = if url.to_ascii_lowercase().starts_with("magnet:") {
                    magnet_display_name(url)
                } else {
                    crate::models::filename_from_url(url)
                };
                (AddTorrent::from_url(url.trim()), name)
            }
            TorrentSource::FilePath(path) => {
                let bytes = std::fs::read(path)
                    .map_err(|e| format!("Failed reading .torrent file: {}", e))?;
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Torrent")
                    .to_string();
                (AddTorrent::from_bytes(bytes), name)
            }
        };

        let resp = sess
            .add_torrent(add_req, Some(opts))
            .await
            .map_err(|e| format!("Failed adding torrent: {}", e))?;

        let (id, handle) = match resp {
            AddTorrentResponse::Added(id, h) => (id, h),
            AddTorrentResponse::AlreadyManaged(id, h) => {
                if !start_paused {
                    let _ = sess.unpause(&h).await;
                }
                (id, h)
            }
            AddTorrentResponse::ListOnly(_) => {
                return Err("Torrent returned list-only unexpectedly".to_string());
            }
        };

        let info_hash: String = handle.info_hash().as_string().to_string();
        let display_name = handle.name().unwrap_or(fallback_name);

        Ok((id, info_hash, display_name, out_dir))
    }

    fn resolve_tid(
        sess: &Session,
        rqbit_id: Option<usize>,
        info_hash: Option<&str>,
    ) -> Option<usize> {
        if let Some(tid) = rqbit_id {
            if sess.get(TorrentIdOrHash::Id(tid)).is_some() {
                return Some(tid);
            }
        }
        if let Some(target_hash) = info_hash {
            return sess.with_torrents(|iter| {
                for (tid, h) in iter {
                    if h.info_hash().as_string().eq_ignore_ascii_case(target_hash) {
                        return Some(tid);
                    }
                }
                None
            });
        }
        None
    }

    pub async fn pause(
        &self,
        rqbit_id: Option<usize>,
        info_hash: Option<&str>,
    ) -> Result<(), String> {
        let Some(sess) = self.get_session().await else {
            return Ok(());
        };
        if let Some(tid) = Self::resolve_tid(&sess, rqbit_id, info_hash) {
            if let Some(handle) = sess.get(TorrentIdOrHash::Id(tid)) {
                sess.pause(&handle)
                    .await
                    .map_err(|e| format!("Failed pausing torrent: {}", e))?;
            }
        }
        Ok(())
    }

    pub async fn unpause(
        &self,
        rqbit_id: Option<usize>,
        info_hash: Option<&str>,
    ) -> Result<(), String> {
        let Some(sess) = self.get_session().await else {
            return Ok(());
        };
        if let Some(tid) = Self::resolve_tid(&sess, rqbit_id, info_hash) {
            if let Some(handle) = sess.get(TorrentIdOrHash::Id(tid)) {
                sess.unpause(&handle)
                    .await
                    .map_err(|e| format!("Failed resuming torrent: {}", e))?;
            }
        }
        Ok(())
    }

    /// Perform a seamless in-memory swarm refresh on a live, incomplete torrent by cycling
    /// `pause` + immediate `unpause`. Because `librqbit` preserves `ChunkTracker` across
    /// `Live -> Paused -> Live`, disk files are NOT re-checked; instead `PeerStates` backoffs
    /// and `NotNeeded` flags are cleared, HTTP & UDP trackers immediately receive a fresh
    /// `EVENT_STARTED` announce, and fresh DHT `get_peers` + LSD lookups begin.
    ///
    /// Returns `Ok(true)` if the swarm was refreshed, or `Ok(false)` if the torrent was not
    /// currently live & downloading (e.g. paused, initializing, finished, or missing).
    pub async fn refresh_peers(
        &self,
        rqbit_id: Option<usize>,
        info_hash: Option<&str>,
    ) -> Result<bool, String> {
        self.refresh_peers_if(rqbit_id, info_hash, || true).await
    }

    /// Same as [`Self::refresh_peers`], but invokes `should_resume` both before pausing and
    /// immediately before unpausing so concurrent user Pause/Cancel/Remove commands always win.
    /// Never refreshes completed/finished torrents.
    pub async fn refresh_peers_if<F>(
        &self,
        rqbit_id: Option<usize>,
        info_hash: Option<&str>,
        should_resume: F,
    ) -> Result<bool, String>
    where
        F: Fn() -> bool,
    {
        let Some(sess) = self.get_session().await else {
            return Ok(false);
        };
        let Some(tid) = Self::resolve_tid(&sess, rqbit_id, info_hash) else {
            return Ok(false);
        };
        let Some(handle) = sess.get(TorrentIdOrHash::Id(tid)) else {
            return Ok(false);
        };

        let stats = handle.stats();
        if !matches!(stats.state, TorrentStatsState::Live) || stats.finished {
            return Ok(false);
        }
        if !should_resume() {
            return Ok(false);
        }

        // Transition Live -> Paused in memory (drops stale PeerStates without re-hashing disk files)
        if sess.pause(&handle).await.is_err() {
            // Likely paused/deleted concurrently; do not resurrect it
            return Ok(false);
        }

        // Abort before unpausing if user paused/cancelled/deleted during the pause transition
        if !should_resume() {
            return Ok(false);
        }

        // Transition Paused -> Live immediately, triggering fresh HTTP/UDP tracker announces & DHT lookup
        sess.unpause(&handle)
            .await
            .map_err(|e| format!("Failed re-announcing torrent swarm: {}", e))?;

        Ok(true)
    }

    pub async fn delete(
        &self,
        rqbit_id: Option<usize>,
        info_hash: Option<&str>,
        delete_files: bool,
    ) -> Result<(), String> {
        let Some(sess) = self.get_session().await else {
            return Ok(());
        };
        let Some(tid) = Self::resolve_tid(&sess, rqbit_id, info_hash) else {
            return Ok(());
        };
        let Some(handle) = sess.get(TorrentIdOrHash::Id(tid)) else {
            return Ok(());
        };
        // Guard against librqbit panic when deleting a magnet whose metadata hasn't resolved yet
        let has_metadata = handle.with_metadata(|_| ()).is_ok();
        if has_metadata {
            sess.delete(TorrentIdOrHash::Id(tid), delete_files)
                .await
                .map_err(|e| format!("Failed deleting torrent: {}", e))
        } else {
            let _ = sess.pause(&handle).await;
            Ok(())
        }
    }

    pub async fn shutdown(&self) {
        if let Some(sess) = self.get_session().await {
            sess.stop().await;
        }
    }

    /// Synchronize live `librqbit` torrent telemetry into VortexDM's unified `DownloadItem` list.
    /// Returns `(total_dl_bps, total_ul_bps, active_peers_total, delta_bytes_downloaded)`.
    #[allow(clippy::too_many_arguments)]
    pub fn sync_stats(
        sess: &Session,
        default_download_dir: &Path,
        downloads: &mut Vec<DownloadItem>,
        prev_torrent_bytes: &mut HashMap<usize, u64>,
        deleting_tids: &HashSet<usize>,
        deleting_hashes: &HashSet<String>,
        pending_pause_tids: &mut HashSet<usize>,
        refreshing_tids: &HashSet<usize>,
        speed_history_len: usize,
    ) -> (f64, f64, usize, u64) {
        let mut total_dl_bps = 0.0f64;
        let mut total_ul_bps = 0.0f64;
        let mut total_peers = 0usize;
        let mut delta_bytes_sum = 0u64;

        // Collect snapshot of all managed torrents from librqbit without holding its lock long
        let managed: Vec<(usize, String, Option<String>, librqbit::TorrentStats, PathBuf, Vec<TorrentFileEntry>)> = sess.with_torrents(|iter| {
            iter.map(|(tid, handle)| {
                let info_hash: String = handle.info_hash().as_string().to_string();
                let name = handle.name();
                let stats = handle.stats();
                let out_folder = default_download_dir.to_path_buf();
                let files: Vec<TorrentFileEntry> = handle
                    .with_metadata(|meta| {
                        meta.file_infos
                            .iter()
                            .enumerate()
                            .map(|(i, fi)| TorrentFileEntry {
                                relative_path: fi.relative_filename.to_string_lossy().to_string(),
                                length: fi.len,
                                downloaded: stats.file_progress.get(i).copied().unwrap_or(0),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (tid, info_hash, name, stats, out_folder, files)
            })
            .collect()
        });

        for (tid, info_hash, name_opt, stats, out_folder, files) in managed {
            // Never re-adopt or update a torrent currently being deleted!
            if deleting_tids.contains(&tid)
                || deleting_hashes.contains(&info_hash.to_ascii_lowercase())
            {
                continue;
            }

            // Find matching DownloadItem by rqbit_id OR info_hash
            let existing_idx = downloads.iter().position(|d| {
                d.torrent.as_ref().is_some_and(|t| {
                    t.rqbit_id == Some(tid)
                        || t.info_hash
                            .as_deref()
                            .is_some_and(|h| h.eq_ignore_ascii_case(&info_hash))
                })
            });

            let idx = match existing_idx {
                Some(i) => i,
                None => {
                    // Adopt torrent restored automatically by librqbit fast-resume persistence
                    let display = name_opt
                        .clone()
                        .unwrap_or_else(|| format!("Torrent ({}...)", &info_hash[..info_hash.len().min(10)]));
                    let item = DownloadItem::new_torrent(
                        format!("magnet:?xt=urn:btih:{}", info_hash),
                        display,
                        out_folder,
                        Some(tid),
                        Some(info_hash.clone()),
                        matches!(stats.state, TorrentStatsState::Paused),
                    );
                    downloads.insert(0, item);
                    0
                }
            };

            let item = &mut downloads[idx];
            if let Some(name) = name_opt.filter(|s| !s.trim().is_empty()) {
                item.filename = name;
            }
            if stats.total_bytes > 0 {
                item.total_bytes = Some(stats.total_bytes);
            }

            let prev = prev_torrent_bytes.insert(tid, stats.progress_bytes);
            if let Some(p) = prev {
                delta_bytes_sum = delta_bytes_sum.saturating_add(stats.progress_bytes.saturating_sub(p));
            }
            item.downloaded_bytes = stats.progress_bytes;

            let is_pending_pause = pending_pause_tids.contains(&tid);
            let tid_in_refresh = refreshing_tids.contains(&tid);
            if matches!(stats.state, TorrentStatsState::Paused) && !tid_in_refresh {
                pending_pause_tids.remove(&tid);
            }
            let is_refreshing = !is_pending_pause && tid_in_refresh;

            // During the ~5ms in-memory pause+unpause swarm refresh, `stats.live` is briefly None.
            // Preserve `Downloading` status and prior telemetry so the UI never flickers to Paused or 0.
            // Never override a torrent that already completed.
            if is_refreshing
                && !stats.finished
                && stats.live.is_none()
                && item.completed_at.is_none()
                && item.status != DownloadStatus::Completed
            {
                item.status = DownloadStatus::Downloading;
                item.error_message = None;
                if let Some(t) = item.torrent.as_mut() {
                    t.rqbit_id = Some(tid);
                    t.info_hash = Some(info_hash);
                    t.uploaded_bytes = stats.uploaded_bytes;
                    if !files.is_empty() {
                        t.files = files;
                    }
                    total_ul_bps += t.upload_speed_bps;
                }
                total_dl_bps += item.speed_bps;
                total_peers += item.active_connections;
                continue;
            }

            let (raw_dl_bps, raw_ul_bps, live_peers, seeders, seen, utp_peers) =
                if is_pending_pause {
                    (0.0, 0.0, 0, 0, 0, 0)
                } else if let Some(live) = &stats.live {
                    let dl = live.download_speed.as_bytes() as f64;
                    let ul = live.upload_speed.as_bytes() as f64;
                    let ps = &live.snapshot.peer_stats;
                    (dl, ul, ps.live, ps.live_seeders, ps.seen, ps.live_utp)
                } else {
                    (0.0, 0.0, 0, 0, 0, 0)
                };

            // Smooth bursty multi-MB piece-completion speed spikes with EWMA so speed & ETA stay steady.
            let dl_bps = if is_pending_pause || stats.finished || stats.live.is_none() {
                0.0
            } else if item.speed_bps <= 1.0 {
                raw_dl_bps
            } else {
                let smoothed = TORRENT_SPEED_EWMA_ALPHA * raw_dl_bps
                    + (1.0 - TORRENT_SPEED_EWMA_ALPHA) * item.speed_bps;
                if raw_dl_bps < 1.0 && smoothed < 64.0 {
                    0.0
                } else {
                    smoothed
                }
            };

            let prev_ul_bps = item.torrent.as_ref().map(|t| t.upload_speed_bps).unwrap_or(0.0);
            let ul_bps = if is_pending_pause || stats.live.is_none() {
                0.0
            } else if prev_ul_bps <= 1.0 {
                raw_ul_bps
            } else {
                let smoothed = TORRENT_SPEED_EWMA_ALPHA * raw_ul_bps
                    + (1.0 - TORRENT_SPEED_EWMA_ALPHA) * prev_ul_bps;
                if raw_ul_bps < 1.0 && smoothed < 64.0 {
                    0.0
                } else {
                    smoothed
                }
            };

            let eta_secs = if !is_pending_pause
                && !stats.finished
                && dl_bps > 10.0
                && stats.total_bytes > stats.progress_bytes
            {
                Some((stats.total_bytes - stats.progress_bytes) as f64 / dl_bps)
            } else {
                None
            };

            item.speed_bps = dl_bps;
            if dl_bps > item.peak_speed_bps {
                item.peak_speed_bps = dl_bps;
            }
            item.eta_seconds = if stats.finished { None } else { eta_secs };
            item.active_connections = live_peers as usize;

            let seeding = !is_pending_pause && stats.finished && matches!(stats.state, TorrentStatsState::Live);

            item.status = if is_pending_pause {
                if stats.finished {
                    DownloadStatus::Completed
                } else {
                    DownloadStatus::Paused
                }
            } else {
                match stats.state {
                    TorrentStatsState::Initializing { paused } => {
                        if paused {
                            DownloadStatus::Paused
                        } else {
                            DownloadStatus::Probing
                        }
                    }
                    TorrentStatsState::Live => {
                        if stats.finished {
                            if item.completed_at.is_none() {
                                item.completed_at = Some(Utc::now());
                            }
                            DownloadStatus::Completed
                        } else {
                            DownloadStatus::Downloading
                        }
                    }
                    TorrentStatsState::Paused => {
                        if stats.finished {
                            DownloadStatus::Completed
                        } else {
                            DownloadStatus::Paused
                        }
                    }
                    TorrentStatsState::Error => DownloadStatus::Failed,
                }
            };

            item.error_message = stats.error;

            if item.status.is_active() || seeding {
                if item.speed_history.len() >= speed_history_len {
                    item.speed_history.pop_front();
                }
                item.speed_history.push_back(dl_bps);
                total_dl_bps += dl_bps;
                total_ul_bps += ul_bps;
                total_peers += live_peers as usize;
            }

            if let Some(t) = item.torrent.as_mut() {
                t.rqbit_id = Some(tid);
                t.info_hash = Some(info_hash);
                t.uploaded_bytes = stats.uploaded_bytes;
                t.upload_speed_bps = ul_bps;
                t.peers_live = live_peers;
                t.peers_seeders = seeders;
                t.peers_leechers = live_peers.saturating_sub(seeders);
                t.peers_seen = seen;
                t.peers_utp = utp_peers;
                t.seeding = seeding;
                if !files.is_empty() {
                    t.files = files;
                }
            }
        }

        (total_dl_bps, total_ul_bps, total_peers, delta_bytes_sum)
    }
}
