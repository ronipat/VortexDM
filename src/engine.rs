use crate::models::*;
use crate::platform;
use crate::storage::{
    compute_file_sha256, finalize_part_file, open_or_preallocate_part_file, write_all_at_offset,
    AppStorage,
};
use crate::throttle::TokenBucket;
use crate::torrent::TorrentManager;
use chrono::Utc;
use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG,
    IF_RANGE, LAST_MODIFIED, RANGE,
};
use reqwest::{Client, StatusCode};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

/// Minimum remaining bytes inside an active segment required before an idle worker may split it in half.
const MIN_DYNAMIC_SPLIT_BYTES: u64 = 256 * 1024; // 256 KB
/// EWMA smoothing factor alpha (0 < alpha <= 1). 0.18 at 250ms ticks gives ~1.3s effective smoothing
/// for steady speed & ETA without sluggish ramp-up.
const EWMA_ALPHA: f64 = 0.18;
const SPEED_HISTORY_LEN: usize = 90;
/// Per-worker HTTP disk write batch size (128 KB) to eliminate per-chunk Win32 WriteFile jitter.
const HTTP_WRITE_BUFFER_BYTES: usize = 128 * 1024;

/// Automatic BitTorrent swarm recovery policy:
/// `librqbit` already discovers fresh peers non-destructively while live via DHT (`get_peers`),
/// HTTP trackers (`force_tracker_interval`), PEX, and LSD. Cycling `pause` + `unpause` disconnects
/// active TCP/uTP peers and resets congestion windows, so we ONLY trigger it on genuine stalls
/// (0 peers or sustained near-zero throughput) with progressive backoff — never interrupting a
/// healthy, steadily downloading swarm.
const TORRENT_ZERO_PEER_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const TORRENT_STALLED_REFRESH_INTERVAL: Duration = Duration::from_secs(45);
const TORRENT_MAX_REFRESH_BACKOFF: Duration = Duration::from_secs(300);
const TORRENT_STALLED_SPEED_BPS: f64 = 15.0 * 1024.0; // 15 KB/s
const TORRENT_COLLAPSE_PEAK_BPS: f64 = 512.0 * 1024.0; // 512 KB/s
const TORRENT_COLLAPSE_FLOOR_BPS: f64 = 32.0 * 1024.0; // 32 KB/s
const TORRENT_HEALTHY_RECOVERY_BPS: f64 = 64.0 * 1024.0; // 64 KB/s

#[derive(Debug, Clone)]
struct TorrentSwarmHealth {
    last_refresh_at: Instant,
    stalled_since: Option<Instant>,
    zero_peer_since: Option<Instant>,
    window_peak_speed_bps: f64,
    consecutive_stalled_refreshes: u32,
}

impl TorrentSwarmHealth {
    fn new(now: Instant) -> Self {
        Self {
            last_refresh_at: now,
            stalled_since: None,
            zero_peer_since: None,
            window_peak_speed_bps: 0.0,
            consecutive_stalled_refreshes: 0,
        }
    }

    fn reset_after_auto_refresh(&mut self, now: Instant) {
        self.last_refresh_at = now;
        self.stalled_since = None;
        self.zero_peer_since = None;
        self.window_peak_speed_bps = 0.0;
        self.consecutive_stalled_refreshes = self.consecutive_stalled_refreshes.saturating_add(1).min(4);
    }

    fn reset_after_manual_refresh(&mut self, now: Instant) {
        self.last_refresh_at = now;
        self.stalled_since = None;
        self.zero_peer_since = None;
        self.window_peak_speed_bps = 0.0;
        self.consecutive_stalled_refreshes = 0;
    }

    fn backoff_multiplier(&self) -> u32 {
        1u32 << self.consecutive_stalled_refreshes.min(3)
    }
}

/// Evaluate whether an active, incomplete downloading torrent is genuinely stalled and should
/// perform a background swarm refresh to re-announce UDP/HTTP trackers & reset peer backoffs.
/// Healthy active transfers are NEVER interrupted so TCP/uTP congestion windows stay stable.
fn should_auto_refresh_swarm(
    health: &mut TorrentSwarmHealth,
    now: Instant,
    live_peers: u32,
    speed_bps: f64,
) -> bool {
    health.window_peak_speed_bps = health.window_peak_speed_bps.max(speed_bps);

    // Once throughput recovers to healthy levels, clear any stall backoff immediately.
    if live_peers > 0 && speed_bps >= TORRENT_HEALTHY_RECOVERY_BPS {
        health.consecutive_stalled_refreshes = 0;
    }

    if live_peers == 0 {
        health.zero_peer_since.get_or_insert(now);
    } else {
        health.zero_peer_since = None;
    }

    let collapsed_from_fast = health.window_peak_speed_bps >= TORRENT_COLLAPSE_PEAK_BPS
        && speed_bps < TORRENT_COLLAPSE_FLOOR_BPS;
    let is_stalled =
        live_peers == 0 || speed_bps < TORRENT_STALLED_SPEED_BPS || collapsed_from_fast;
    if is_stalled {
        health.stalled_since.get_or_insert(now);
    } else {
        health.stalled_since = None;
    }

    let since_refresh = now.saturating_duration_since(health.last_refresh_at);
    let mult = health.backoff_multiplier();

    // 1. Hard stall: 0 live peers continuously (30s -> 60s -> 120s -> 240s)
    if let Some(zero_at) = health.zero_peer_since {
        let wait = (TORRENT_ZERO_PEER_REFRESH_INTERVAL * mult).min(TORRENT_MAX_REFRESH_BACKOFF);
        if now.saturating_duration_since(zero_at) >= wait && since_refresh >= wait {
            return true;
        }
    }

    // 2. Sustained throughput stall or severe collapse (45s -> 90s -> 180s -> 300s)
    if let Some(stalled_at) = health.stalled_since {
        let wait = (TORRENT_STALLED_REFRESH_INTERVAL * mult).min(TORRENT_MAX_REFRESH_BACKOFF);
        if now.saturating_duration_since(stalled_at) >= wait && since_refresh >= wait {
            return true;
        }
    }

    false
}

/// Prevent already-completed torrents from regressing back into `Downloading` / `Probing` / `Paused`
/// (e.g., if `librqbit` fast-resume check fails after restart or a companion file was modified/moved).
/// Returns `(tid, info_hash)` pairs for regressed torrents that `librqbit` is actively trying to
/// re-download/re-check so the engine can immediately pause them.
fn enforce_sticky_completed(
    downloads: &mut [DownloadItem],
    was_completed: &HashMap<Uuid, (Option<u64>, u64)>,
    prev_torrent_bytes: &mut HashMap<usize, u64>,
) -> Vec<(usize, Option<String>)> {
    let mut to_pause = Vec::new();
    for item in downloads.iter_mut() {
        if !item.is_torrent() {
            continue;
        }
        let prev_entry = was_completed.get(&item.id).copied();
        let already_completed = prev_entry.is_some() || item.completed_at.is_some();
        if !already_completed || item.status == DownloadStatus::Completed {
            continue;
        }

        let needs_pause = matches!(
            item.status,
            DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Verifying
        );

        let (prev_total, prev_dl) = prev_entry.unwrap_or((item.total_bytes, item.downloaded_bytes));
        let full_bytes = item
            .total_bytes
            .or(prev_total)
            .unwrap_or(prev_dl)
            .max(prev_dl)
            .max(item.downloaded_bytes);
        if item.total_bytes.is_none() && prev_total.is_some() {
            item.total_bytes = prev_total;
        }
        item.downloaded_bytes = full_bytes;
        item.status = DownloadStatus::Completed;
        item.speed_bps = 0.0;
        item.eta_seconds = None;
        item.active_connections = 0;
        item.error_message = None;
        if item.completed_at.is_none() {
            item.completed_at = Some(Utc::now());
        }

        if let Some(t) = item.torrent.as_mut() {
            t.seeding = false;
            t.upload_speed_bps = 0.0;
            t.peers_live = 0;
            t.peers_seeders = 0;
            t.peers_leechers = 0;
            if let Some(tid) = t.rqbit_id {
                prev_torrent_bytes.insert(tid, full_bytes);
                if needs_pause {
                    to_pause.push((tid, t.info_hash.clone()));
                }
            }
        }
    }
    to_pause
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopSignal {
    Running,
    Pause,
    Cancel,
    FallbackSingleStream,
}

#[derive(Clone)]
pub struct EngineSnapshot {
    pub downloads: Vec<DownloadItem>,
    pub settings: AppSettings,
    pub stats: GlobalStats,
}

pub struct DownloadEngineHandle {
    pub cmd_tx: mpsc::UnboundedSender<EngineCommand>,
    pub snapshot: Arc<RwLock<EngineSnapshot>>,
    pub client: Client,
    pub bring_to_front: Arc<AtomicBool>,
    pub pending_ui_targets: Arc<Mutex<VecDeque<String>>>,
}

impl DownloadEngineHandle {
    pub fn send(&self, cmd: EngineCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn get_snapshot(&self) -> EngineSnapshot {
        self.snapshot.read().unwrap().clone()
    }

    pub fn take_bring_to_front(&self) -> bool {
        self.bring_to_front.swap(false, Ordering::SeqCst)
    }

    pub fn push_pending_ui_targets(&self, targets: impl IntoIterator<Item = String>) {
        let mut q = self.pending_ui_targets.lock().unwrap();
        let mut any = false;
        for t in targets {
            if !t.trim().is_empty() {
                q.push_back(t);
                any = true;
            }
        }
        if any {
            self.bring_to_front.store(true, Ordering::SeqCst);
        }
    }

    pub fn take_pending_ui_targets(&self) -> Vec<String> {
        let mut q = self.pending_ui_targets.lock().unwrap();
        q.drain(..).collect()
    }
}

struct SharedState {
    downloads: RwLock<Vec<DownloadItem>>,
    settings: RwLock<AppSettings>,
    stats: RwLock<GlobalStats>,
    snapshot: Arc<RwLock<EngineSnapshot>>,
    storage: AppStorage,
    client: Client,
    bucket: Arc<TokenBucket>,
    torrent_mgr: Arc<TorrentManager>,
    active_signals: Mutex<HashMap<Uuid, watch::Sender<StopSignal>>>,
    /// Fast atomic byte trackers for active downloads (id -> downloaded_bytes)
    live_bytes: Mutex<HashMap<Uuid, Arc<AtomicU64>>>,
    live_conns: Mutex<HashMap<Uuid, Arc<AtomicUsize>>>,
    /// Shared live segments for active downloads so UI visualizes real-time segment bars
    live_segments: Mutex<HashMap<Uuid, Arc<Mutex<Vec<SegmentState>>>>>,
    /// Guards preventing `sync_stats` from resurrecting deleted torrents or flickering mid-pause/refresh
    deleting_tids: Mutex<HashSet<usize>>,
    deleting_hashes: Mutex<HashSet<String>>,
    cancelled_item_ids: Mutex<HashSet<Uuid>>,
    pending_pause_tids: Mutex<HashSet<usize>>,
    refreshing_tids: Mutex<HashSet<usize>>,
    last_swarm_refresh: Mutex<HashMap<usize, Instant>>,
    bring_to_front: Arc<AtomicBool>,
    pending_ui_targets: Arc<Mutex<VecDeque<String>>>,
    session_bytes: AtomicU64,
    repaint_cb: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl SharedState {
    fn refresh_snapshot(&self) {
        let downloads = self.downloads.read().unwrap().clone();
        let settings = self.settings.read().unwrap().clone();
        let stats = self.stats.read().unwrap().clone();
        let mut snap = self.snapshot.write().unwrap();
        snap.downloads = downloads;
        snap.settings = settings;
        snap.stats = stats;
        drop(snap);
        if let Some(cb) = &self.repaint_cb {
            cb();
        }
    }

    fn persist_state(&self) {
        let downloads = self.downloads.read().unwrap().clone();
        let _ = self.storage.save_downloads(&downloads);
    }
}

pub fn build_http_client() -> Client {
    Client::builder()
        .user_agent(format!("VortexDM/{} (Windows NT; Rust)", APP_VERSION))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(45))
        .tcp_nodelay(true)
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .pool_max_idle_per_host(32)
        .pool_idle_timeout(Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .expect("Failed to construct HTTP client")
}

/// Probe a URL using `HEAD` with automatic `GET Range: bytes=0-0` fallback for servers that reject HEAD.
pub async fn probe_url(client: &Client, url: &str) -> Result<ProbeInfo, String> {
    let mut final_url = url.to_string();
    let mut filename: Option<String> = None;
    let mut total_bytes: Option<u64> = None;
    let mut supports_ranges = false;
    let mut etag: Option<String> = None;
    let mut last_modified: Option<String> = None;
    let mut content_type: Option<String> = None;

    let mut need_range_probe = false;

    match client.head(url).send().await {
        Ok(resp) if resp.status().is_success() => {
            final_url = resp.url().to_string();
            let headers = resp.headers();

            if let Some(cd) = headers.get(CONTENT_DISPOSITION).and_then(|v| v.to_str().ok()) {
                filename = parse_content_disposition(cd);
            }
            if let Some(cl) = headers
                .get(CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
            {
                total_bytes = Some(cl);
            }
            if let Some(ar) = headers.get(ACCEPT_RANGES).and_then(|v| v.to_str().ok()) {
                supports_ranges = ar.to_ascii_lowercase().contains("bytes");
            } else {
                need_range_probe = true;
            }
            etag = headers
                .get(ETAG)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            last_modified = headers
                .get(LAST_MODIFIED)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            content_type = headers
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
        }
        _ => {
            need_range_probe = true;
        }
    }

    if need_range_probe {
        let resp = client
            .get(&final_url)
            .header(RANGE, "bytes=0-0")
            .send()
            .await
            .map_err(|e| format!("Network error probing URL: {}", e))?;

        if !resp.status().is_success() && resp.status() != StatusCode::PARTIAL_CONTENT {
            return Err(format!("Server returned HTTP {}", resp.status()));
        }

        final_url = resp.url().to_string();
        let status = resp.status();
        let headers = resp.headers();

        if filename.is_none() {
            if let Some(cd) = headers.get(CONTENT_DISPOSITION).and_then(|v| v.to_str().ok()) {
                filename = parse_content_disposition(cd);
            }
        }
        if etag.is_none() {
            etag = headers
                .get(ETAG)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
        }
        if last_modified.is_none() {
            last_modified = headers
                .get(LAST_MODIFIED)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
        }
        if content_type.is_none() {
            content_type = headers
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
        }

        if status == StatusCode::PARTIAL_CONTENT {
            supports_ranges = true;
            if let Some(cr) = headers.get(CONTENT_RANGE).and_then(|v| v.to_str().ok()) {
                if let Some(total) = parse_content_range_total(cr) {
                    total_bytes = Some(total);
                }
            }
        } else {
            if let Some(ar) = headers.get(ACCEPT_RANGES).and_then(|v| v.to_str().ok()) {
                if ar.to_ascii_lowercase().contains("bytes") {
                    supports_ranges = true;
                }
            }
            if total_bytes.is_none() {
                if let Some(cl) = headers
                    .get(CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    total_bytes = Some(cl);
                }
            }
        }
        // Dropping `resp` immediately closes the probe stream without reading body bytes.
    }

    let resolved_name = filename.unwrap_or_else(|| filename_from_url(&final_url));
    Ok(ProbeInfo {
        final_url,
        filename: resolved_name,
        total_bytes,
        supports_ranges,
        etag,
        last_modified,
        content_type,
    })
}

/// Parse total file length from `Content-Range: bytes 0-0/1048576`.
pub fn parse_content_range_total(header_val: &str) -> Option<u64> {
    let slash_idx = header_val.rfind('/')?;
    let total_str = header_val[slash_idx + 1..].trim();
    if total_str == "*" {
        None
    } else {
        total_str.parse::<u64>().ok()
    }
}

/// Start the background Tokio download engine (HTTP + BitTorrent) and return a thread-safe handle.
pub fn start_engine(
    rt_handle: &tokio::runtime::Handle,
    storage: AppStorage,
    repaint_cb: Option<Arc<dyn Fn() + Send + Sync>>,
) -> DownloadEngineHandle {
    start_engine_with_options(rt_handle, storage, true, true, repaint_cb)
}

/// Start the download engine with explicit torrent DHT/persistence toggles (useful for isolated tests).
pub fn start_engine_with_options(
    rt_handle: &tokio::runtime::Handle,
    storage: AppStorage,
    torrent_dht: bool,
    torrent_persistence: bool,
    repaint_cb: Option<Arc<dyn Fn() + Send + Sync>>,
) -> DownloadEngineHandle {
    let settings = storage.load_settings();
    let downloads = storage.load_downloads();
    let bucket = Arc::new(TokenBucket::new(settings.global_speed_limit_bps));
    let client = build_http_client();
    let torrent_mgr = Arc::new(TorrentManager::new(
        settings.download_dir.clone(),
        storage.data_dir(),
        torrent_dht,
        torrent_persistence,
    ));

    let has_persisted_torrents = downloads.iter().any(|d| d.is_torrent())
        || storage.data_dir().join("torrents").join("session.json").exists();
    let initial_limit = settings.global_speed_limit_bps;

    let snapshot = Arc::new(RwLock::new(EngineSnapshot {
        downloads: downloads.clone(),
        settings: settings.clone(),
        stats: GlobalStats::default(),
    }));

    let bring_to_front = Arc::new(AtomicBool::new(false));
    let pending_ui_targets = Arc::new(Mutex::new(VecDeque::new()));
    let state = Arc::new(SharedState {
        downloads: RwLock::new(downloads),
        settings: RwLock::new(settings),
        stats: RwLock::new(GlobalStats::default()),
        snapshot: snapshot.clone(),
        storage,
        client: client.clone(),
        bucket,
        torrent_mgr: torrent_mgr.clone(),
        active_signals: Mutex::new(HashMap::new()),
        live_bytes: Mutex::new(HashMap::new()),
        live_conns: Mutex::new(HashMap::new()),
        live_segments: Mutex::new(HashMap::new()),
        deleting_tids: Mutex::new(HashSet::new()),
        deleting_hashes: Mutex::new(HashSet::new()),
        cancelled_item_ids: Mutex::new(HashSet::new()),
        pending_pause_tids: Mutex::new(HashSet::new()),
        refreshing_tids: Mutex::new(HashSet::new()),
        last_swarm_refresh: Mutex::new(HashMap::new()),
        bring_to_front: bring_to_front.clone(),
        pending_ui_targets: pending_ui_targets.clone(),
        session_bytes: AtomicU64::new(0),
        repaint_cb,
    });

    // If user previously had active/persisted torrents, warm up librqbit Session immediately in background
    if has_persisted_torrents {
        let tm = torrent_mgr.clone();
        rt_handle.spawn(async move {
            let _ = tm.init(initial_limit).await;
        });
    }

    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<EngineCommand>();

    // Spawn EWMA Telemetry & Queue Scheduler ticker (250 ms)
    let tick_state = state.clone();
    let tick_cmd_tx = cmd_tx.clone();
    rt_handle.spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        let mut prev_bytes: HashMap<Uuid, u64> = HashMap::new();
        let mut prev_torrent_bytes: HashMap<usize, u64> = HashMap::new();
        let mut swarm_health: HashMap<usize, TorrentSwarmHealth> = HashMap::new();
        let mut guard_paused_tids: HashMap<usize, Instant> = HashMap::new();
        let mut last_tick = Instant::now();
        let mut persist_counter: u32 = 0;

        loop {
            interval.tick().await;
            let now = Instant::now();
            let dt = now.duration_since(last_tick).as_secs_f64().max(0.05);
            last_tick = now;

            // Drain any `.torrent` / `magnet:?` / URL targets forwarded by a secondary Windows launch
            // and hand them to the UI confirmation dialog before starting transfers.
            let ipc_targets = tick_state.storage.drain_ipc_inbox();
            if !ipc_targets.is_empty() {
                let mut pushed_any = false;
                {
                    let mut q = tick_state.pending_ui_targets.lock().unwrap();
                    for raw in ipc_targets {
                        if let Some(target) = normalize_cli_target(&raw) {
                            q.push_back(target);
                            pushed_any = true;
                        }
                    }
                }
                if pushed_any {
                    tick_state.bring_to_front.store(true, Ordering::SeqCst);
                }
            }

            let torrent_session = tick_state.torrent_mgr.get_session().await;

            let mut total_speed = 0.0f64;
            let mut total_upload_speed = 0.0f64;
            let mut active_count = 0usize;
            let mut queued_count = 0usize;
            let mut paused_count = 0usize;
            let mut completed_count = 0usize;
            let mut failed_count = 0usize;
            let mut torrent_count = 0usize;
            let mut total_conns = 0usize;
            let mut any_active = false;
            let mut active_downloading_torrents: Vec<(Uuid, usize, Option<String>, u32, f64)> =
                Vec::new();

            {
                let live_bytes_map = tick_state.live_bytes.lock().unwrap();
                let live_conns_map = tick_state.live_conns.lock().unwrap();
                let live_segs_map = tick_state.live_segments.lock().unwrap();
                let mut downloads = tick_state.downloads.write().unwrap();

                // Sync live BitTorrent stats from librqbit into unified DownloadItem list
                if let Some(sess) = &torrent_session {
                    let default_dl_dir = tick_state.settings.read().unwrap().download_dir.clone();
                    let del_tids = tick_state.deleting_tids.lock().unwrap().clone();
                    let del_hashes = tick_state.deleting_hashes.lock().unwrap().clone();
                    let refreshing = tick_state.refreshing_tids.lock().unwrap().clone();
                    // Remember which torrents were already Completed before syncing librqbit state
                    let was_completed: HashMap<Uuid, (Option<u64>, u64)> = downloads
                        .iter()
                        .filter(|d| {
                            d.is_torrent()
                                && (d.status == DownloadStatus::Completed
                                    || d.completed_at.is_some())
                        })
                        .map(|d| (d.id, (d.total_bytes, d.downloaded_bytes)))
                        .collect();
                    let mut pend_pause = tick_state.pending_pause_tids.lock().unwrap();
                    let (_t_dl, t_ul, _t_peers, t_delta) = TorrentManager::sync_stats(
                        sess,
                        &default_dl_dir,
                        &mut downloads,
                        &mut prev_torrent_bytes,
                        &del_tids,
                        &del_hashes,
                        &mut pend_pause,
                        &refreshing,
                        SPEED_HISTORY_LEN,
                    );
                    drop(pend_pause);
                    total_upload_speed += t_ul;
                    tick_state.session_bytes.fetch_add(t_delta, Ordering::Relaxed);

                    // Sticky Completed: a finished torrent must never fall back into downloading
                    // (e.g. librqbit re-verifying files on startup, or files moved/edited on disk).
                    let regressed = enforce_sticky_completed(
                        &mut downloads,
                        &was_completed,
                        &mut prev_torrent_bytes,
                    );
                    let regressed_tids: HashSet<usize> =
                        regressed.iter().map(|(tid, _)| *tid).collect();
                    guard_paused_tids.retain(|tid, _| regressed_tids.contains(tid));
                    for (tid, hash) in regressed {
                        let should_pause_now = guard_paused_tids
                            .get(&tid)
                            .is_none_or(|prev| now.saturating_duration_since(*prev) >= Duration::from_secs(2));
                        if should_pause_now {
                            guard_paused_tids.insert(tid, now);
                            let st = tick_state.clone();
                            tokio::spawn(async move {
                                let _ = st.torrent_mgr.pause(Some(tid), hash.as_deref()).await;
                            });
                        }
                    }
                }

                for item in downloads.iter_mut() {
                    if item.is_torrent() {
                        torrent_count += 1;
                        let is_seeding = item.torrent.as_ref().is_some_and(|t| t.seeding);
                        match item.status {
                            DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Verifying => {
                                active_count += 1;
                                any_active = true;
                                total_conns += item.active_connections;
                                total_speed += item.speed_bps;
                                if matches!(
                                    item.status,
                                    DownloadStatus::Downloading | DownloadStatus::Probing
                                ) && !is_seeding
                                    && item.completed_at.is_none()
                                {
                                    if let Some(tor) = item.torrent.as_ref() {
                                        if let Some(tid) = tor.rqbit_id {
                                            active_downloading_torrents.push((
                                                item.id,
                                                tid,
                                                tor.info_hash.clone(),
                                                tor.peers_live,
                                                item.speed_bps,
                                            ));
                                        }
                                    }
                                }
                            }
                            DownloadStatus::Queued => queued_count += 1,
                            DownloadStatus::Paused => paused_count += 1,
                            DownloadStatus::Completed => {
                                completed_count += 1;
                                if is_seeding {
                                    total_conns += item.active_connections;
                                }
                            }
                            DownloadStatus::Failed | DownloadStatus::Cancelled => failed_count += 1,
                        }
                        continue;
                    }

                    if let Some(counter) = live_bytes_map.get(&item.id) {
                        let cur = counter.load(Ordering::Relaxed);
                        item.downloaded_bytes = cur;
                    }
                    if let Some(conns) = live_conns_map.get(&item.id) {
                        item.active_connections = conns.load(Ordering::Relaxed);
                    } else if !item.status.is_active() {
                        item.active_connections = 0;
                    }
                    if let Some(seg_arc) = live_segs_map.get(&item.id) {
                        if let Ok(segs) = seg_arc.try_lock() {
                            item.segments = segs.clone();
                        }
                    }

                    match item.status {
                        DownloadStatus::Downloading | DownloadStatus::Probing | DownloadStatus::Verifying => {
                            active_count += 1;
                            any_active = true;
                            total_conns += item.active_connections;

                            let prev = prev_bytes.insert(item.id, item.downloaded_bytes);
                            let delta = match prev {
                                Some(p) => item.downloaded_bytes.saturating_sub(p),
                                None => 0,
                            };
                            tick_state.session_bytes.fetch_add(delta, Ordering::Relaxed);

                            let raw_speed = (delta as f64) / dt;
                            if item.status == DownloadStatus::Downloading {
                                if item.speed_bps <= 0.1 {
                                    item.speed_bps = raw_speed;
                                } else {
                                    item.speed_bps =
                                        EWMA_ALPHA * raw_speed + (1.0 - EWMA_ALPHA) * item.speed_bps;
                                }
                                if item.speed_bps < 1.0 && delta == 0 {
                                    item.speed_bps = 0.0;
                                }
                                if item.speed_bps > item.peak_speed_bps {
                                    item.peak_speed_bps = item.speed_bps;
                                }
                                item.eta_seconds = match item.total_bytes {
                                    Some(total) if item.speed_bps > 10.0 => {
                                        let rem = total.saturating_sub(item.downloaded_bytes);
                                        Some(rem as f64 / item.speed_bps)
                                    }
                                    _ => None,
                                };
                            } else {
                                item.speed_bps = 0.0;
                                item.eta_seconds = None;
                            }

                            if item.speed_history.len() >= SPEED_HISTORY_LEN {
                                item.speed_history.pop_front();
                            }
                            item.speed_history.push_back(item.speed_bps);
                            total_speed += item.speed_bps;
                        }
                        DownloadStatus::Queued => {
                            queued_count += 1;
                            item.speed_bps = 0.0;
                            item.eta_seconds = None;
                            prev_bytes.remove(&item.id);
                        }
                        DownloadStatus::Paused => {
                            paused_count += 1;
                            item.speed_bps = 0.0;
                            item.eta_seconds = None;
                            prev_bytes.remove(&item.id);
                        }
                        DownloadStatus::Completed => {
                            completed_count += 1;
                            item.speed_bps = 0.0;
                            item.eta_seconds = None;
                            prev_bytes.remove(&item.id);
                        }
                        DownloadStatus::Failed | DownloadStatus::Cancelled => {
                            failed_count += 1;
                            item.speed_bps = 0.0;
                            item.eta_seconds = None;
                            prev_bytes.remove(&item.id);
                        }
                    }
                }
            }

            {
                let mut stats = tick_state.stats.write().unwrap();
                stats.total_speed_bps = total_speed;
                stats.total_upload_speed_bps = total_upload_speed;
                stats.active_count = active_count;
                stats.queued_count = queued_count;
                stats.paused_count = paused_count;
                stats.completed_count = completed_count;
                stats.failed_count = failed_count;
                stats.torrent_count = torrent_count;
                stats.total_connections = total_conns;
                stats.session_downloaded_bytes = tick_state.session_bytes.load(Ordering::Relaxed);
                if stats.speed_history.len() >= SPEED_HISTORY_LEN {
                    stats.speed_history.pop_front();
                }
                stats.speed_history.push_back(total_speed);
            }

            // Evaluate automatic seamless swarm refresh for active downloading torrents
            if torrent_session.is_some() {
                let active_tids: HashSet<usize> = active_downloading_torrents
                    .iter()
                    .map(|(_, tid, _, _, _)| *tid)
                    .collect();
                swarm_health.retain(|tid, _| active_tids.contains(tid));

                if !active_downloading_torrents.is_empty() {
                    let manual_refreshes = tick_state.last_swarm_refresh.lock().unwrap().clone();
                    let refreshing_now = tick_state.refreshing_tids.lock().unwrap().clone();

                    for (item_id, tid, info_hash, live_peers, speed_bps) in
                        active_downloading_torrents
                    {
                        if refreshing_now.contains(&tid) {
                            continue;
                        }
                        let health = swarm_health
                            .entry(tid)
                            .or_insert_with(|| TorrentSwarmHealth::new(now));
                        if let Some(&manual_ts) = manual_refreshes.get(&tid) {
                            if manual_ts > health.last_refresh_at {
                                health.reset_after_manual_refresh(manual_ts);
                            }
                        }
                        if should_auto_refresh_swarm(health, now, live_peers, speed_bps)
                            && spawn_torrent_swarm_refresh(
                                tick_state.clone(),
                                item_id,
                                tid,
                                info_hash,
                            )
                        {
                            health.reset_after_auto_refresh(now);
                        }
                    }
                }
            }

            // Trigger queue scheduling if there are queued HTTP items & open concurrency slots
            if queued_count > 0 {
                schedule_queued(&tick_state, &tick_cmd_tx);
            }

            tick_state.refresh_snapshot();

            // Periodically checkpoint state to disk (~every 2s while active) for crash recovery
            if any_active {
                persist_counter += 1;
                if persist_counter >= 8 {
                    persist_counter = 0;
                    tick_state.persist_state();
                }
            }
        }
    });

    // Spawn Command Loop
    let cmd_state = state.clone();
    let loop_cmd_tx = cmd_tx.clone();
    rt_handle.spawn(async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                EngineCommand::Add(req) => {
                    // Automatically route magnet:? links and .torrent URLs/paths into BitTorrent engine!
                    if is_torrent_input(&req.url) {
                        let source = if std::path::Path::new(req.url.trim()).exists() {
                            TorrentSource::FilePath(std::path::PathBuf::from(req.url.trim()))
                        } else {
                            TorrentSource::MagnetOrUrl(req.url.trim().to_string())
                        };
                        let st = cmd_state.clone();
                        tokio::spawn(async move {
                            handle_add_torrent(
                                st,
                                NewTorrentRequest {
                                    source,
                                    save_dir: req.save_dir,
                                    start_paused: req.start_paused,
                                },
                            )
                            .await;
                        });
                    } else {
                        handle_add(&cmd_state, req);
                        schedule_queued(&cmd_state, &loop_cmd_tx);
                        cmd_state.persist_state();
                        cmd_state.refresh_snapshot();
                    }
                }
                EngineCommand::AddTorrent(req) => {
                    let st = cmd_state.clone();
                    tokio::spawn(async move {
                        handle_add_torrent(st, req).await;
                    });
                }
                EngineCommand::Pause(id) => {
                    let torrent_ref = {
                        let mut downloads = cmd_state.downloads.write().unwrap();
                        if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                            if let Some(t) = item.torrent.as_mut() {
                                if item.status != DownloadStatus::Completed
                                    && item.completed_at.is_none()
                                {
                                    item.status = DownloadStatus::Paused;
                                }
                                item.speed_bps = 0.0;
                                item.eta_seconds = None;
                                item.active_connections = 0;
                                t.seeding = false;
                                if let Some(tid) = t.rqbit_id {
                                    cmd_state.pending_pause_tids.lock().unwrap().insert(tid);
                                }
                                Some((t.rqbit_id, t.info_hash.clone()))
                            } else {
                                if matches!(
                                    item.status,
                                    DownloadStatus::Queued
                                        | DownloadStatus::Probing
                                        | DownloadStatus::Downloading
                                ) {
                                    item.status = DownloadStatus::Paused;
                                    item.speed_bps = 0.0;
                                    item.eta_seconds = None;
                                }
                                None
                            }
                        } else {
                            None
                        }
                    };
                    if let Some((tid, hash)) = torrent_ref {
                        let st = cmd_state.clone();
                        tokio::spawn(async move {
                            let _ = st.torrent_mgr.pause(tid, hash.as_deref()).await;
                            if let Some(t) = tid {
                                if !st.refreshing_tids.lock().unwrap().contains(&t) {
                                    st.pending_pause_tids.lock().unwrap().remove(&t);
                                }
                            }
                            st.persist_state();
                            st.refresh_snapshot();
                        });
                    } else {
                        signal_download(&cmd_state, id, StopSignal::Pause);
                    }
                    cmd_state.persist_state();
                    cmd_state.refresh_snapshot();
                }
                EngineCommand::Resume(id) => {
                    let torrent_action = {
                        let mut downloads = cmd_state.downloads.write().unwrap();
                        if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                            if item.status == DownloadStatus::Completed
                                || item.completed_at.is_some()
                            {
                                None
                            } else if let Some(t) = item.torrent.as_ref() {
                                item.error_message = None;
                                item.status = DownloadStatus::Probing;
                                if let Some(tid) = t.rqbit_id {
                                    cmd_state.pending_pause_tids.lock().unwrap().remove(&tid);
                                }
                                Some((
                                    t.rqbit_id,
                                    t.info_hash.clone(),
                                    t.source.clone(),
                                    item.save_dir.clone(),
                                ))
                            } else {
                                if matches!(
                                    item.status,
                                    DownloadStatus::Paused
                                        | DownloadStatus::Failed
                                        | DownloadStatus::Cancelled
                                ) {
                                    item.status = DownloadStatus::Queued;
                                    item.error_message = None;
                                }
                                None
                            }
                        } else {
                            None
                        }
                    };
                    if let Some((rqbit_id, info_hash, source, save_dir)) = torrent_action {
                        if rqbit_id.is_some() || info_hash.is_some() {
                            let st = cmd_state.clone();
                            tokio::spawn(async move {
                                let _ = st.torrent_mgr.unpause(rqbit_id, info_hash.as_deref()).await;
                                st.persist_state();
                                st.refresh_snapshot();
                            });
                        } else {
                            // Re-add persisted torrent into librqbit session if not yet bound
                            let st = cmd_state.clone();
                            tokio::spawn(async move {
                                let src = if std::path::Path::new(&source).exists() {
                                    TorrentSource::FilePath(std::path::PathBuf::from(&source))
                                } else {
                                    TorrentSource::MagnetOrUrl(source)
                                };
                                let limit = st.settings.read().unwrap().global_speed_limit_bps;
                                if let Ok((tid, hash, name, _)) =
                                    st.torrent_mgr.add_torrent(&src, Some(save_dir), false, limit).await
                                {
                                    let mut dls = st.downloads.write().unwrap();
                                    if let Some(item) = dls.iter_mut().find(|d| d.id == id) {
                                        item.filename = name;
                                        item.status = DownloadStatus::Downloading;
                                        if let Some(t) = item.torrent.as_mut() {
                                            t.rqbit_id = Some(tid);
                                            t.info_hash = Some(hash);
                                        }
                                    }
                                }
                                st.refresh_snapshot();
                            });
                        }
                    } else {
                        schedule_queued(&cmd_state, &loop_cmd_tx);
                    }
                    cmd_state.persist_state();
                    cmd_state.refresh_snapshot();
                }
                EngineCommand::Cancel(id) => {
                    signal_download(&cmd_state, id, StopSignal::Cancel);
                    let (part_path, torrent_ref) = {
                        let mut downloads = cmd_state.downloads.write().unwrap();
                        if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                            let tref = item
                                .torrent
                                .as_ref()
                                .map(|t| (t.rqbit_id, t.info_hash.clone()));
                            if let Some((Some(tid), _)) = &tref {
                                cmd_state.pending_pause_tids.lock().unwrap().insert(*tid);
                            }
                            item.status = DownloadStatus::Cancelled;
                            item.speed_bps = 0.0;
                            item.eta_seconds = None;
                            item.downloaded_bytes = 0;
                            item.segments.clear();
                            (if tref.is_none() { Some(item.part_path()) } else { None }, tref)
                        } else {
                            (None, None)
                        }
                    };
                    if let Some((tid, hash)) = torrent_ref {
                        let st = cmd_state.clone();
                        tokio::spawn(async move {
                            let _ = st.torrent_mgr.pause(tid, hash.as_deref()).await;
                            if let Some(t) = tid {
                                if !st.refreshing_tids.lock().unwrap().contains(&t) {
                                    st.pending_pause_tids.lock().unwrap().remove(&t);
                                }
                            }
                            st.refresh_snapshot();
                        });
                    }
                    if let Some(p) = part_path {
                        let _ = std::fs::remove_file(p);
                    }
                    cmd_state.persist_state();
                    cmd_state.refresh_snapshot();
                }
                EngineCommand::Remove { id, delete_file } => {
                    signal_download(&cmd_state, id, StopSignal::Cancel);
                    cmd_state.cancelled_item_ids.lock().unwrap().insert(id);
                    let removed = {
                        let mut downloads = cmd_state.downloads.write().unwrap();
                        if let Some(idx) = downloads.iter().position(|d| d.id == id) {
                            Some(downloads.remove(idx))
                        } else {
                            None
                        }
                    };
                    if let Some(item) = removed {
                        if let Some(tor) = item.torrent.as_ref() {
                            let tid = tor.rqbit_id;
                            let hash = tor.info_hash.clone();
                            if let Some(t) = tid {
                                cmd_state.deleting_tids.lock().unwrap().insert(t);
                                cmd_state.pending_pause_tids.lock().unwrap().remove(&t);
                                cmd_state.last_swarm_refresh.lock().unwrap().remove(&t);
                            }
                            if let Some(h) = &hash {
                                cmd_state
                                    .deleting_hashes
                                    .lock()
                                    .unwrap()
                                    .insert(h.to_ascii_lowercase());
                            }
                            let st = cmd_state.clone();
                            tokio::spawn(async move {
                                let _ = st
                                    .torrent_mgr
                                    .delete(tid, hash.as_deref(), delete_file)
                                    .await;
                                if let Some(t) = tid {
                                    st.deleting_tids.lock().unwrap().remove(&t);
                                }
                                if let Some(h) = &hash {
                                    st.deleting_hashes
                                        .lock()
                                        .unwrap()
                                        .remove(&h.to_ascii_lowercase());
                                }
                                st.persist_state();
                                st.refresh_snapshot();
                            });
                        } else {
                            let _ = std::fs::remove_file(item.part_path());
                            if delete_file {
                                let _ = std::fs::remove_file(item.target_path());
                            }
                        }
                    }
                    cmd_state.persist_state();
                    cmd_state.refresh_snapshot();
                }
                EngineCommand::RemoveMany { ids, delete_file } => {
                    if !ids.is_empty() {
                        let id_set: HashSet<Uuid> = ids.iter().copied().collect();
                        for &id in &id_set {
                            signal_download(&cmd_state, id, StopSignal::Cancel);
                            cmd_state.cancelled_item_ids.lock().unwrap().insert(id);
                        }
                        let removed_items: Vec<DownloadItem> = {
                            let mut downloads = cmd_state.downloads.write().unwrap();
                            let mut out = Vec::new();
                            let mut i = 0;
                            while i < downloads.len() {
                                if id_set.contains(&downloads[i].id) {
                                    out.push(downloads.remove(i));
                                } else {
                                    i += 1;
                                }
                            }
                            out
                        };
                        let mut torrent_deletes: Vec<(Option<usize>, Option<String>)> = Vec::new();
                        for item in removed_items {
                            if let Some(tor) = item.torrent.as_ref() {
                                let tid = tor.rqbit_id;
                                let hash = tor.info_hash.clone();
                                if let Some(t) = tid {
                                    cmd_state.deleting_tids.lock().unwrap().insert(t);
                                    cmd_state.pending_pause_tids.lock().unwrap().remove(&t);
                                    cmd_state.last_swarm_refresh.lock().unwrap().remove(&t);
                                }
                                if let Some(h) = &hash {
                                    cmd_state
                                        .deleting_hashes
                                        .lock()
                                        .unwrap()
                                        .insert(h.to_ascii_lowercase());
                                }
                                torrent_deletes.push((tid, hash));
                            } else {
                                let _ = std::fs::remove_file(item.part_path());
                                if delete_file {
                                    let _ = std::fs::remove_file(item.target_path());
                                }
                            }
                        }
                        if !torrent_deletes.is_empty() {
                            let st = cmd_state.clone();
                            tokio::spawn(async move {
                                for (tid, hash) in torrent_deletes {
                                    let _ = st
                                        .torrent_mgr
                                        .delete(tid, hash.as_deref(), delete_file)
                                        .await;
                                    if let Some(t) = tid {
                                        st.deleting_tids.lock().unwrap().remove(&t);
                                    }
                                    if let Some(h) = &hash {
                                        st.deleting_hashes
                                            .lock()
                                            .unwrap()
                                            .remove(&h.to_ascii_lowercase());
                                    }
                                }
                                st.persist_state();
                                st.refresh_snapshot();
                            });
                        }
                        cmd_state.persist_state();
                        cmd_state.refresh_snapshot();
                    }
                }
                EngineCommand::PauseAll => {
                    let (http_ids, torrent_refs): (Vec<Uuid>, Vec<(Option<usize>, Option<String>)>) = {
                        let mut downloads = cmd_state.downloads.write().unwrap();
                        let mut h_ids = Vec::new();
                        let mut t_refs = Vec::new();
                        for d in downloads.iter_mut() {
                            if let Some(t) = d.torrent.as_mut() {
                                if d.status.is_active() || t.seeding {
                                    if d.status != DownloadStatus::Completed
                                        && d.completed_at.is_none()
                                    {
                                        d.status = DownloadStatus::Paused;
                                    }
                                    d.speed_bps = 0.0;
                                    d.eta_seconds = None;
                                    d.active_connections = 0;
                                    t.seeding = false;
                                    if let Some(tid) = t.rqbit_id {
                                        cmd_state.pending_pause_tids.lock().unwrap().insert(tid);
                                    }
                                    t_refs.push((t.rqbit_id, t.info_hash.clone()));
                                }
                            } else if matches!(
                                d.status,
                                DownloadStatus::Queued
                                    | DownloadStatus::Probing
                                    | DownloadStatus::Downloading
                            ) {
                                d.status = DownloadStatus::Paused;
                                d.speed_bps = 0.0;
                                d.eta_seconds = None;
                                h_ids.push(d.id);
                            }
                        }
                        (h_ids, t_refs)
                    };
                    for id in http_ids {
                        signal_download(&cmd_state, id, StopSignal::Pause);
                    }
                    if !torrent_refs.is_empty() {
                        let st = cmd_state.clone();
                        tokio::spawn(async move {
                            for (tid, hash) in torrent_refs {
                                let _ = st.torrent_mgr.pause(tid, hash.as_deref()).await;
                                if let Some(t) = tid {
                                    if !st.refreshing_tids.lock().unwrap().contains(&t) {
                                        st.pending_pause_tids.lock().unwrap().remove(&t);
                                    }
                                }
                            }
                            st.persist_state();
                            st.refresh_snapshot();
                        });
                    }
                    cmd_state.persist_state();
                    cmd_state.refresh_snapshot();
                }
                EngineCommand::ResumeAll => {
                    let torrent_refs: Vec<(Option<usize>, Option<String>)> = {
                        let mut downloads = cmd_state.downloads.write().unwrap();
                        let mut t_refs = Vec::new();
                        for item in downloads.iter_mut() {
                            if let Some(t) = item.torrent.as_ref() {
                                if item.status == DownloadStatus::Paused {
                                    if let Some(tid) = t.rqbit_id {
                                        cmd_state.pending_pause_tids.lock().unwrap().remove(&tid);
                                    }
                                    item.status = DownloadStatus::Probing;
                                    t_refs.push((t.rqbit_id, t.info_hash.clone()));
                                }
                            } else if matches!(
                                item.status,
                                DownloadStatus::Paused | DownloadStatus::Failed
                            ) {
                                item.status = DownloadStatus::Queued;
                                item.error_message = None;
                            }
                        }
                        t_refs
                    };
                    if !torrent_refs.is_empty() {
                        let st = cmd_state.clone();
                        tokio::spawn(async move {
                            for (tid, hash) in torrent_refs {
                                let _ = st.torrent_mgr.unpause(tid, hash.as_deref()).await;
                            }
                            st.persist_state();
                            st.refresh_snapshot();
                        });
                    }
                    schedule_queued(&cmd_state, &loop_cmd_tx);
                    cmd_state.persist_state();
                    cmd_state.refresh_snapshot();
                }
                EngineCommand::ClearCompleted => {
                    let done_trefs: Vec<(Option<usize>, Option<String>)> = {
                        let mut downloads = cmd_state.downloads.write().unwrap();
                        let mut trefs = Vec::new();
                        downloads.retain(|d| {
                            let remove = matches!(
                                d.status,
                                DownloadStatus::Completed | DownloadStatus::Cancelled
                            );
                            if remove {
                                if let Some(t) = d.torrent.as_ref() {
                                    if let Some(tid) = t.rqbit_id {
                                        cmd_state.deleting_tids.lock().unwrap().insert(tid);
                                        cmd_state.last_swarm_refresh.lock().unwrap().remove(&tid);
                                    }
                                    if let Some(h) = &t.info_hash {
                                        cmd_state
                                            .deleting_hashes
                                            .lock()
                                            .unwrap()
                                            .insert(h.to_ascii_lowercase());
                                    }
                                    trefs.push((t.rqbit_id, t.info_hash.clone()));
                                }
                            }
                            !remove
                        });
                        trefs
                    };
                    if !done_trefs.is_empty() {
                        let st = cmd_state.clone();
                        tokio::spawn(async move {
                            for (tid, hash) in done_trefs {
                                let _ = st.torrent_mgr.delete(tid, hash.as_deref(), false).await;
                                if let Some(t) = tid {
                                    st.deleting_tids.lock().unwrap().remove(&t);
                                }
                                if let Some(h) = &hash {
                                    st.deleting_hashes
                                        .lock()
                                        .unwrap()
                                        .remove(&h.to_ascii_lowercase());
                                }
                            }
                            st.persist_state();
                            st.refresh_snapshot();
                        });
                    }
                    cmd_state.persist_state();
                    cmd_state.refresh_snapshot();
                }
                EngineCommand::RefreshTorrentPeers(id) => {
                    let target = {
                        let downloads = cmd_state.downloads.read().unwrap();
                        downloads.iter().find(|d| d.id == id).and_then(|item| {
                            if !matches!(
                                item.status,
                                DownloadStatus::Downloading | DownloadStatus::Probing
                            ) {
                                return None;
                            }
                            item.torrent.as_ref().and_then(|t| {
                                if t.seeding {
                                    None
                                } else {
                                    t.rqbit_id.map(|tid| (tid, t.info_hash.clone()))
                                }
                            })
                        })
                    };
                    if let Some((tid, info_hash)) = target {
                        spawn_torrent_swarm_refresh(cmd_state.clone(), id, tid, info_hash);
                    }
                }
                EngineCommand::VerifySha256 { id, expected } => {
                    let st = cmd_state.clone();
                    tokio::spawn(async move {
                        verify_existing_download(st, id, expected).await;
                    });
                }
                EngineCommand::UpdateSettings(new_settings) => {
                    cmd_state.bucket.set_rate(new_settings.global_speed_limit_bps);
                    cmd_state
                        .torrent_mgr
                        .set_download_limit(new_settings.global_speed_limit_bps)
                        .await;
                    let _ = platform::set_windows_autostart(
                        new_settings.start_with_windows,
                        new_settings.start_minimized,
                    );
                    let _ = cmd_state.storage.save_settings(&new_settings);
                    *cmd_state.settings.write().unwrap() = new_settings;
                    schedule_queued(&cmd_state, &loop_cmd_tx);
                    cmd_state.refresh_snapshot();
                }
                EngineCommand::SetGlobalSpeedLimit(bps) => {
                    cmd_state.bucket.set_rate(bps);
                    cmd_state.torrent_mgr.set_download_limit(bps).await;
                    {
                        let mut s = cmd_state.settings.write().unwrap();
                        s.global_speed_limit_bps = bps;
                        let _ = cmd_state.storage.save_settings(&s);
                    }
                    cmd_state.refresh_snapshot();
                }
                EngineCommand::Shutdown => {
                    let all_ids: Vec<Uuid> = {
                        let signals = cmd_state.active_signals.lock().unwrap();
                        signals.keys().copied().collect()
                    };
                    for id in all_ids {
                        signal_download(&cmd_state, id, StopSignal::Pause);
                    }
                    cmd_state.torrent_mgr.shutdown().await;
                    tokio::time::sleep(Duration::from_millis(120)).await;
                    cmd_state.persist_state();
                    break;
                }
            }
        }
    });

    DownloadEngineHandle {
        cmd_tx,
        snapshot,
        client,
        bring_to_front,
        pending_ui_targets,
    }
}

fn is_torrent_refresh_allowed(state: &SharedState, item_id: Uuid, tid: usize) -> bool {
    if state.deleting_tids.lock().unwrap().contains(&tid) {
        return false;
    }
    if state.pending_pause_tids.lock().unwrap().contains(&tid) {
        return false;
    }
    let downloads = state.downloads.read().unwrap();
    downloads.iter().any(|d| {
        d.id == item_id
            && d.completed_at.is_none()
            && matches!(d.status, DownloadStatus::Downloading | DownloadStatus::Probing)
            && d.torrent
                .as_ref()
                .is_some_and(|t| t.rqbit_id == Some(tid) && !t.seeding)
    })
}

fn spawn_torrent_swarm_refresh(
    state: Arc<SharedState>,
    item_id: Uuid,
    tid: usize,
    info_hash: Option<String>,
) -> bool {
    if !is_torrent_refresh_allowed(&state, item_id, tid) {
        return false;
    }
    {
        let mut refreshing = state.refreshing_tids.lock().unwrap();
        if !refreshing.insert(tid) {
            return false;
        }
    }

    tokio::spawn(async move {
        let check_state = state.clone();
        let refresh_res = state
            .torrent_mgr
            .refresh_peers_if(Some(tid), info_hash.as_deref(), move || {
                is_torrent_refresh_allowed(&check_state, item_id, tid)
            })
            .await;

        let refreshed = matches!(refresh_res, Ok(true));
        let still_allowed = is_torrent_refresh_allowed(&state, item_id, tid);

        if refreshed && !still_allowed {
            // User paused/cancelled/removed while `unpause` was in-flight — user intent wins.
            let is_deleting = state.deleting_tids.lock().unwrap().contains(&tid);
            if !is_deleting {
                let _ = state.torrent_mgr.pause(Some(tid), info_hash.as_deref()).await;
            }
            state.pending_pause_tids.lock().unwrap().remove(&tid);
        } else if !still_allowed {
            state.pending_pause_tids.lock().unwrap().remove(&tid);
        } else if refreshed {
            let now_utc = Utc::now();
            state
                .last_swarm_refresh
                .lock()
                .unwrap()
                .insert(tid, Instant::now());
            let mut downloads = state.downloads.write().unwrap();
            if let Some(item) = downloads.iter_mut().find(|d| d.id == item_id) {
                if let Some(t) = item.torrent.as_mut() {
                    t.last_swarm_refresh_at = Some(now_utc);
                }
            }
        }

        state.refreshing_tids.lock().unwrap().remove(&tid);
        state.refresh_snapshot();
    });

    true
}

async fn handle_add_torrent(state: Arc<SharedState>, req: NewTorrentRequest) {
    let settings = state.settings.read().unwrap().clone();
    let mut save_dir = req.save_dir.unwrap_or_else(|| settings.download_dir.clone());
    if settings.organize_by_category {
        save_dir = save_dir.join(FileCategory::Torrents.folder_name());
    }
    let _ = std::fs::create_dir_all(&save_dir);

    let (source_str, placeholder_name) = match &req.source {
        TorrentSource::MagnetOrUrl(u) => (
            u.clone(),
            if u.to_ascii_lowercase().starts_with("magnet:") {
                magnet_display_name(u)
            } else {
                filename_from_url(u)
            },
        ),
        TorrentSource::FilePath(p) => (
            p.display().to_string(),
            p.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Torrent")
                .to_string(),
        ),
    };

    let placeholder = DownloadItem::new_torrent(
        source_str,
        placeholder_name,
        save_dir.clone(),
        None,
        None,
        req.start_paused,
    );
    let item_id = placeholder.id;
    {
        let mut downloads = state.downloads.write().unwrap();
        downloads.insert(0, placeholder);
    }
    state.refresh_snapshot();

    match state
        .torrent_mgr
        .add_torrent(
            &req.source,
            Some(save_dir),
            req.start_paused,
            settings.global_speed_limit_bps,
        )
        .await
    {
        Ok((tid, info_hash, display_name, out_dir)) => {
            // If user already removed this item while it was still connecting/resolving, clean it up immediately
            if state.cancelled_item_ids.lock().unwrap().remove(&item_id) {
                state.deleting_tids.lock().unwrap().insert(tid);
                let _ = state.torrent_mgr.delete(Some(tid), Some(&info_hash), false).await;
                state.deleting_tids.lock().unwrap().remove(&tid);
                state.persist_state();
                state.refresh_snapshot();
                return;
            }

            let mut need_pause = false;
            {
                let mut downloads = state.downloads.write().unwrap();
                // Avoid duplicate entry if torrent with same info_hash was already present
                let dup = downloads.iter().position(|d| {
                    d.id != item_id
                        && d.torrent
                            .as_ref()
                            .and_then(|t| t.info_hash.as_deref())
                            .is_some_and(|h| h.eq_ignore_ascii_case(&info_hash))
                });
                if let Some(dup_idx) = dup {
                    downloads.retain(|d| d.id != item_id);
                    let target_idx = dup_idx.min(downloads.len().saturating_sub(1));
                    if let Some(existing) = downloads.get_mut(target_idx) {
                        if !req.start_paused && existing.status == DownloadStatus::Paused {
                            existing.status = DownloadStatus::Downloading;
                        }
                        if let Some(t) = existing.torrent.as_mut() {
                            t.rqbit_id = Some(tid);
                        }
                    }
                } else if let Some(item) = downloads.iter_mut().find(|d| d.id == item_id) {
                    let user_paused_during_connect =
                        !req.start_paused && item.status == DownloadStatus::Paused;
                    if user_paused_during_connect {
                        need_pause = true;
                        state.pending_pause_tids.lock().unwrap().insert(tid);
                    }
                    item.filename = display_name;
                    item.save_dir = out_dir;
                    item.status = if req.start_paused || user_paused_during_connect {
                        DownloadStatus::Paused
                    } else {
                        DownloadStatus::Downloading
                    };
                    if let Some(t) = item.torrent.as_mut() {
                        t.rqbit_id = Some(tid);
                        t.info_hash = Some(info_hash.clone());
                    }
                }
            }
            if need_pause {
                let _ = state.torrent_mgr.pause(Some(tid), Some(&info_hash)).await;
                state.pending_pause_tids.lock().unwrap().remove(&tid);
            }
        }
        Err(err_msg) => {
            let mut downloads = state.downloads.write().unwrap();
            if let Some(item) = downloads.iter_mut().find(|d| d.id == item_id) {
                item.status = DownloadStatus::Failed;
                item.error_message = Some(err_msg);
            }
        }
    }

    state.persist_state();
    state.refresh_snapshot();
}

fn handle_add(state: &Arc<SharedState>, req: NewDownloadRequest) {
    let settings = state.settings.read().unwrap().clone();
    let initial_name = req
        .custom_filename
        .filter(|s| !s.trim().is_empty())
        .map(|s| sanitize_filename(&s))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| filename_from_url(&req.url));

    let category = FileCategory::from_filename(&initial_name);
    let mut save_dir = req.save_dir.unwrap_or_else(|| settings.download_dir.clone());
    if settings.organize_by_category {
        save_dir = save_dir.join(category.folder_name());
    }
    let _ = std::fs::create_dir_all(&save_dir);

    let existing_names: Vec<String> = {
        let downloads = state.downloads.read().unwrap();
        downloads
            .iter()
            .filter(|d| d.save_dir == save_dir)
            .map(|d| d.filename.clone())
            .collect()
    };

    let unique_name = deduplicate_filename(&save_dir, &initial_name, &existing_names);
    let max_conns = req
        .max_connections
        .unwrap_or(settings.default_connections)
        .clamp(1, 32);

    let item = DownloadItem::new(
        req.url.trim().to_string(),
        unique_name,
        save_dir,
        max_conns,
        req.expected_sha256,
        req.start_paused,
    );

    let mut downloads = state.downloads.write().unwrap();
    downloads.insert(0, item);
}

fn signal_download(state: &Arc<SharedState>, id: Uuid, sig: StopSignal) {
    let signals = state.active_signals.lock().unwrap();
    if let Some(tx) = signals.get(&id) {
        let _ = tx.send(sig);
    }
}

fn schedule_queued(state: &Arc<SharedState>, _cmd_tx: &mpsc::UnboundedSender<EngineCommand>) {
    let max_concurrent = state.settings.read().unwrap().max_concurrent_downloads;
    let to_start: Vec<Uuid> = {
        let mut downloads = state.downloads.write().unwrap();
        let active_now = downloads
            .iter()
            .filter(|d| !d.is_torrent() && d.status.is_active())
            .count();
        let open_slots = max_concurrent.saturating_sub(active_now);
        if open_slots == 0 {
            return;
        }
        let mut started = Vec::new();
        // Start oldest queued HTTP items first (torrents are scheduled by librqbit)
        for item in downloads.iter_mut().rev() {
            if started.len() >= open_slots {
                break;
            }
            if !item.is_torrent() && item.status == DownloadStatus::Queued {
                item.status = DownloadStatus::Probing;
                item.error_message = None;
                started.push(item.id);
            }
        }
        started
    };

    for id in to_start {
        let st = state.clone();
        tokio::spawn(async move {
            run_download_task(st, id).await;
        });
    }
}

async fn run_download_task(state: Arc<SharedState>, id: Uuid) {
    let (stop_tx, mut stop_rx) = watch::channel(StopSignal::Running);
    {
        state.active_signals.lock().unwrap().insert(id, stop_tx.clone());
    }

    let result = run_download_inner(&state, id, &stop_tx, &mut stop_rx).await;

    // Clean up live runtime handles
    {
        state.active_signals.lock().unwrap().remove(&id);
        state.live_bytes.lock().unwrap().remove(&id);
        state.live_conns.lock().unwrap().remove(&id);
        state.live_segments.lock().unwrap().remove(&id);
    }

    match result {
        Ok(TaskOutcome::Completed) => {
            let auto_verify = state.settings.read().unwrap().auto_verify_sha256;
            let (should_verify, target_path, expected) = {
                let downloads = state.downloads.read().unwrap();
                if let Some(item) = downloads.iter().find(|d| d.id == id) {
                    (
                        auto_verify || item.expected_sha256.is_some(),
                        item.target_path(),
                        item.expected_sha256.clone(),
                    )
                } else {
                    (false, std::path::PathBuf::new(), None)
                }
            };

            if should_verify && target_path.exists() {
                {
                    let mut downloads = state.downloads.write().unwrap();
                    if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                        item.status = DownloadStatus::Verifying;
                        item.active_connections = 0;
                    }
                }
                state.refresh_snapshot();

                let hash_res = tokio::task::spawn_blocking(move || compute_file_sha256(&target_path))
                    .await
                    .ok()
                    .and_then(|r| r.ok());

                let mut downloads = state.downloads.write().unwrap();
                if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                    if let Some(hash) = hash_res {
                        let verified = expected.as_ref().map(|e| e.eq_ignore_ascii_case(&hash));
                        item.computed_sha256 = Some(hash);
                        item.sha256_verified = verified;
                        if verified == Some(false) {
                            item.error_message = Some("SHA-256 checksum mismatch!".to_string());
                        }
                    }
                    item.status = DownloadStatus::Completed;
                    item.completed_at = Some(Utc::now());
                    item.speed_bps = 0.0;
                    item.eta_seconds = None;
                }
            } else {
                let mut downloads = state.downloads.write().unwrap();
                if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                    item.status = DownloadStatus::Completed;
                    item.completed_at = Some(Utc::now());
                    item.active_connections = 0;
                    item.speed_bps = 0.0;
                    item.eta_seconds = None;
                }
            }
        }
        Ok(TaskOutcome::Paused) => {
            let mut downloads = state.downloads.write().unwrap();
            if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                if item.status != DownloadStatus::Cancelled {
                    item.status = DownloadStatus::Paused;
                }
                item.active_connections = 0;
                item.speed_bps = 0.0;
                item.eta_seconds = None;
            }
        }
        Ok(TaskOutcome::Cancelled) => {
            let part_path = {
                let mut downloads = state.downloads.write().unwrap();
                if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                    item.status = DownloadStatus::Cancelled;
                    item.active_connections = 0;
                    item.speed_bps = 0.0;
                    item.eta_seconds = None;
                    Some(item.part_path())
                } else {
                    None
                }
            };
            if let Some(p) = part_path {
                let _ = std::fs::remove_file(p);
            }
        }
        Err(err_msg) => {
            let mut downloads = state.downloads.write().unwrap();
            if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                item.status = DownloadStatus::Failed;
                item.error_message = Some(err_msg);
                item.active_connections = 0;
                item.speed_bps = 0.0;
                item.eta_seconds = None;
            }
        }
    }

    state.persist_state();
    state.refresh_snapshot();
}

enum TaskOutcome {
    Completed,
    Paused,
    Cancelled,
}

async fn run_download_inner(
    state: &Arc<SharedState>,
    id: Uuid,
    stop_tx: &watch::Sender<StopSignal>,
    stop_rx: &mut watch::Receiver<StopSignal>,
) -> Result<TaskOutcome, String> {
    let (url, existing_total, existing_etag, existing_lm, existing_segments, max_conns, max_retries) = {
        let downloads = state.downloads.read().unwrap();
        let item = downloads
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| "Download entry removed".to_string())?;
        let retries = state.settings.read().unwrap().max_retries;
        (
            item.url.clone(),
            item.total_bytes,
            item.etag.clone(),
            item.last_modified.clone(),
            item.segments.clone(),
            item.max_connections,
            retries,
        )
    };

    if *stop_rx.borrow() == StopSignal::Pause {
        return Ok(TaskOutcome::Paused);
    }
    if *stop_rx.borrow() == StopSignal::Cancel {
        return Ok(TaskOutcome::Cancelled);
    }

    // 1. Pre-flight probe (HEAD + Range 0-0 fallback)
    let probe = probe_url(&state.client, &url).await?;

    if *stop_rx.borrow() == StopSignal::Pause {
        return Ok(TaskOutcome::Paused);
    }
    if *stop_rx.borrow() == StopSignal::Cancel {
        return Ok(TaskOutcome::Cancelled);
    }

    // Determine whether we can safely resume existing segments
    let resource_unchanged = match (&existing_etag, &probe.etag) {
        (Some(old), Some(new)) => old == new,
        _ => match (&existing_lm, &probe.last_modified) {
            (Some(old), Some(new)) => old == new,
            _ => existing_total.is_some() && existing_total == probe.total_bytes,
        },
    };

    let (part_path, target_path, save_dir, can_resume_segments) = {
        let mut downloads = state.downloads.write().unwrap();
        let existing_names: Vec<String> = downloads
            .iter()
            .filter(|d| d.id != id)
            .map(|d| d.filename.clone())
            .collect();

        let item = downloads
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| "Download removed".to_string())?;

        // Update filename from server Content-Disposition if this is a fresh download (no progress yet)
        if item.downloaded_bytes == 0 && !probe.filename.is_empty() {
            let unique = deduplicate_filename(&item.save_dir, &probe.filename, &existing_names);
            item.filename = unique;
            item.category = FileCategory::from_filename(&item.filename);
        }

        item.final_url = Some(probe.final_url.clone());
        item.total_bytes = probe.total_bytes;
        item.supports_ranges = probe.supports_ranges;
        item.etag = probe.etag.clone();
        item.last_modified = probe.last_modified.clone();
        item.content_type = probe.content_type.clone();
        item.status = DownloadStatus::Downloading;

        let part = item.part_path();
        let target = item.target_path();
        let part_matches_size = match probe.total_bytes {
            Some(total) => part
                .metadata()
                .map(|m| m.len() == total)
                .unwrap_or(false),
            None => false,
        };

        let resume_ok = probe.supports_ranges
            && probe.total_bytes.is_some()
            && resource_unchanged
            && !existing_segments.is_empty()
            && part_matches_size;

        if !resume_ok {
            item.downloaded_bytes = 0;
            item.segments.clear();
        }

        (part, target, item.save_dir.clone(), resume_ok)
    };

    // 2. Disk Space Pre-Check
    if let Some(total) = probe.total_bytes {
        if !can_resume_segments {
            if let Some(free) = platform::available_disk_space(&save_dir) {
                // Keep 16 MB safety headroom
                if total.saturating_add(16 * 1024 * 1024) > free {
                    return Err(format!(
                        "Insufficient disk space: need {}, only {} free",
                        format_bytes(total),
                        format_bytes(free)
                    ));
                }
            }
        }
    }

    // Handle zero-byte files cleanly
    if probe.total_bytes == Some(0) {
        let _ = std::fs::create_dir_all(&save_dir);
        File::create(&target_path).map_err(|e| format!("Failed creating empty file: {}", e))?;
        return Ok(TaskOutcome::Completed);
    }

    // 3. Choose segmented multi-connection vs single-stream mode
    let effective_conns = recommended_connections(probe.total_bytes, probe.supports_ranges, max_conns);

    if let (Some(total_bytes), true) = (probe.total_bytes, probe.supports_ranges && effective_conns >= 1) {
        let seg_outcome = run_segmented_download(
            state,
            id,
            &probe,
            total_bytes,
            effective_conns,
            can_resume_segments,
            &part_path,
            &target_path,
            max_retries,
            stop_tx,
        )
        .await?;

        match seg_outcome {
            SegmentOutcome::Completed => return Ok(TaskOutcome::Completed),
            SegmentOutcome::Paused => return Ok(TaskOutcome::Paused),
            SegmentOutcome::Cancelled => return Ok(TaskOutcome::Cancelled),
            SegmentOutcome::FallbackSingleStream => {
                // Reset stop signal so single-stream runs fresh
                let _ = stop_tx.send(StopSignal::Running);
                {
                    let mut downloads = state.downloads.write().unwrap();
                    if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                        item.supports_ranges = false;
                        item.downloaded_bytes = 0;
                        item.segments.clear();
                    }
                }
            }
        }
    }

    // 4. Single-stream sequential download (unknown Content-Length, no Range support, or 200 OK fallback)
    run_single_stream_download(
        state,
        id,
        &probe.final_url,
        probe.total_bytes,
        &part_path,
        &target_path,
        max_retries,
        stop_rx,
    )
    .await
}

enum SegmentOutcome {
    Completed,
    Paused,
    Cancelled,
    FallbackSingleStream,
}

#[allow(clippy::too_many_arguments)]
async fn run_segmented_download(
    state: &Arc<SharedState>,
    id: Uuid,
    probe: &ProbeInfo,
    total_bytes: u64,
    num_conns: usize,
    can_resume: bool,
    part_path: &std::path::Path,
    target_path: &std::path::Path,
    max_retries: u32,
    stop_tx: &watch::Sender<StopSignal>,
) -> Result<SegmentOutcome, String> {
    let file = open_or_preallocate_part_file(part_path, total_bytes, can_resume)
        .map_err(|e| format!("Failed allocating disk space: {}", e))?;
    let shared_file = Arc::new(file);

    // Initialize or restore segments
    let initial_segments: Vec<SegmentState> = if can_resume {
        let downloads = state.downloads.read().unwrap();
        let mut segs = downloads
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.segments.clone())
            .unwrap_or_default();
        for s in &mut segs {
            if !s.is_complete() {
                s.status = SegmentStatus::Idle;
                s.retries = 0;
            } else {
                s.status = SegmentStatus::Completed;
            }
        }
        segs
    } else {
        create_initial_segments(total_bytes, num_conns)
    };

    let already_downloaded: u64 = initial_segments.iter().map(|s| s.downloaded).sum();
    let bytes_counter = Arc::new(AtomicU64::new(already_downloaded));
    let conns_counter = Arc::new(AtomicUsize::new(0));
    let segments_arc = Arc::new(Mutex::new(initial_segments));

    {
        state.live_bytes.lock().unwrap().insert(id, bytes_counter.clone());
        state.live_conns.lock().unwrap().insert(id, conns_counter.clone());
        state.live_segments.lock().unwrap().insert(id, segments_arc.clone());
    }

    let if_range_token = probe
        .etag
        .clone()
        .or_else(|| probe.last_modified.clone());

    let mut handles = Vec::with_capacity(num_conns);
    for _worker_idx in 0..num_conns {
        let client = state.client.clone();
        let bucket = state.bucket.clone();
        let file_ref = shared_file.clone();
        let segs_ref = segments_arc.clone();
        let bytes_ref = bytes_counter.clone();
        let conns_ref = conns_counter.clone();
        let url = probe.final_url.clone();
        let if_range = if_range_token.clone();
        let worker_stop_tx = stop_tx.clone();
        let mut worker_stop_rx = stop_tx.subscribe();

        let h = tokio::spawn(async move {
            loop {
                if *worker_stop_rx.borrow() != StopSignal::Running {
                    return Ok::<(), String>(());
                }

                // Acquire an idle segment or dynamically bisect the largest remaining active segment!
                let seg_idx = match acquire_or_steal_segment(&segs_ref, total_bytes) {
                    Some(idx) => idx,
                    None => return Ok(()),
                };

                conns_ref.fetch_add(1, Ordering::Relaxed);
                let worker_res = download_segment_loop(
                    &client,
                    &bucket,
                    &file_ref,
                    &segs_ref,
                    seg_idx,
                    &url,
                    if_range.as_deref(),
                    &bytes_ref,
                    max_retries,
                    &worker_stop_tx,
                    &mut worker_stop_rx,
                )
                .await;
                conns_ref.fetch_sub(1, Ordering::Relaxed);

                if let Err(e) = worker_res {
                    return Err(e);
                }
            }
        });
        handles.push(h);
    }

    let mut first_err: Option<String> = None;
    for h in handles {
        match h.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            Err(join_err) => {
                if first_err.is_none() {
                    first_err = Some(format!("Worker panicked: {}", join_err));
                }
            }
        }
    }

    // Snapshot final segment progress into DownloadItem before closing file handle
    let (all_complete, final_segs, total_written) = {
        let segs = segments_arc.lock().unwrap();
        let complete = !segs.is_empty() && segs.iter().all(|s| s.is_complete());
        let written: u64 = segs.iter().map(|s| s.downloaded).sum();
        (complete, segs.clone(), written)
    };

    {
        let mut downloads = state.downloads.write().unwrap();
        if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
            item.segments = final_segs;
            item.downloaded_bytes = total_written;
        }
    }

    // Flush & drop the shared File handle before renaming
    let _ = shared_file.sync_data();
    drop(shared_file);

    match *stop_tx.borrow() {
        StopSignal::Pause => return Ok(SegmentOutcome::Paused),
        StopSignal::Cancel => return Ok(SegmentOutcome::Cancelled),
        StopSignal::FallbackSingleStream => return Ok(SegmentOutcome::FallbackSingleStream),
        StopSignal::Running => {}
    }

    if let Some(err) = first_err {
        return Err(err);
    }

    if !all_complete || total_written != total_bytes {
        return Err(format!(
            "Incomplete transfer: received {} of {} bytes",
            total_written, total_bytes
        ));
    }

    finalize_part_file(part_path, target_path)
        .map_err(|e| format!("Failed finalizing downloaded file: {}", e))?;

    Ok(SegmentOutcome::Completed)
}

/// Create initial uniform byte segments `[start, end]` covering `[0, total_bytes - 1]`.
pub fn create_initial_segments(total_bytes: u64, num_conns: usize) -> Vec<SegmentState> {
    let n = num_conns.max(1) as u64;
    let chunk = total_bytes / n;
    let mut segs = Vec::with_capacity(num_conns);
    for i in 0..n {
        let start = i * chunk;
        let end = if i == n - 1 {
            total_bytes - 1
        } else {
            (i + 1) * chunk - 1
        };
        if start <= end {
            segs.push(SegmentState {
                id: i as usize,
                start,
                end,
                downloaded: 0,
                status: SegmentStatus::Idle,
                retries: 0,
            });
        }
    }
    segs
}

/// Claim an idle segment, OR dynamically steal the upper half of the largest remaining active segment (IDM rule).
pub fn acquire_or_steal_segment(
    segs_arc: &Arc<Mutex<Vec<SegmentState>>>,
    total_bytes: u64,
) -> Option<usize> {
    let mut segs = segs_arc.lock().unwrap();

    // 1. Prefer any unfinished Idle segment first
    if let Some(idx) = segs
        .iter()
        .position(|s| s.status == SegmentStatus::Idle && !s.is_complete())
    {
        segs[idx].status = SegmentStatus::Active;
        return Some(idx);
    }

    // 2. Dynamic In-Half Work Stealing: bisect the active segment with largest remaining byte span
    // Cap total segment count at 64 so UI and server stay tidy
    if segs.len() >= 64 {
        return None;
    }

    // Adaptive minimum split size: 64 KB for smaller files (< 2 MB), 256 KB otherwise
    let min_split = if total_bytes < 2 * 1024 * 1024 {
        64 * 1024
    } else {
        MIN_DYNAMIC_SPLIT_BYTES
    };

    let best = segs
        .iter()
        .enumerate()
        .filter(|(_, s)| s.status == SegmentStatus::Active && s.remaining() >= min_split * 2)
        .max_by_key(|(_, s)| s.remaining())
        .map(|(i, _)| i);

    if let Some(victim_idx) = best {
        let cur = segs[victim_idx].current_offset();
        let old_end = segs[victim_idx].end;
        let rem = old_end.saturating_sub(cur) + 1;
        let half = rem / 2;
        if half >= min_split {
            let new_victim_end = cur + half - 1;
            let stolen_start = new_victim_end + 1;
            segs[victim_idx].end = new_victim_end;

            let new_id = segs.len();
            segs.push(SegmentState {
                id: new_id,
                start: stolen_start,
                end: old_end,
                downloaded: 0,
                status: SegmentStatus::Active,
                retries: 0,
            });
            return Some(new_id);
        }
    }

    None
}

#[allow(clippy::too_many_arguments)]
async fn download_segment_loop(
    client: &Client,
    bucket: &Arc<TokenBucket>,
    file: &Arc<File>,
    segs_arc: &Arc<Mutex<Vec<SegmentState>>>,
    seg_idx: usize,
    url: &str,
    if_range: Option<&str>,
    bytes_counter: &Arc<AtomicU64>,
    max_retries: u32,
    stop_tx: &watch::Sender<StopSignal>,
    stop_rx: &mut watch::Receiver<StopSignal>,
) -> Result<(), String> {
    let mut attempt: u32 = 0;

    loop {
        if *stop_rx.borrow() != StopSignal::Running {
            let mut segs = segs_arc.lock().unwrap();
            if segs[seg_idx].status == SegmentStatus::Active && !segs[seg_idx].is_complete() {
                segs[seg_idx].status = SegmentStatus::Idle;
            }
            return Ok(());
        }

        let (req_start, req_end) = {
            let mut segs = segs_arc.lock().unwrap();
            let seg = &mut segs[seg_idx];
            if seg.is_complete() {
                seg.status = SegmentStatus::Completed;
                return Ok(());
            }
            (seg.current_offset(), seg.end)
        };

        let mut req_builder = client
            .get(url)
            .header(RANGE, format!("bytes={}-{}", req_start, req_end));
        if let Some(token) = if_range {
            req_builder = req_builder.header(IF_RANGE, token);
        }

        let resp_res = tokio::select! {
            res = req_builder.send() => res,
            _ = stop_rx.changed() => {
                let mut segs = segs_arc.lock().unwrap();
                if !segs[seg_idx].is_complete() {
                    segs[seg_idx].status = SegmentStatus::Idle;
                }
                return Ok(());
            }
        };

        let mut resp = match resp_res {
            Ok(r) => r,
            Err(e) => {
                attempt += 1;
                {
                    let mut segs = segs_arc.lock().unwrap();
                    segs[seg_idx].retries = attempt;
                }
                if attempt > max_retries {
                    let mut segs = segs_arc.lock().unwrap();
                    segs[seg_idx].status = SegmentStatus::Failed;
                    return Err(format!("Segment {} failed after {} retries: {}", seg_idx, max_retries, e));
                }
                let backoff = Duration::from_secs((1u64 << attempt.min(4)).min(16));
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => continue,
                    _ = stop_rx.changed() => return Ok(()),
                }
            }
        };

        match resp.status() {
            StatusCode::PARTIAL_CONTENT => {
                // Buffer small network chunks into 128 KB batches to minimize Win32 WriteFile
                // syscall jitter while honoring dynamic `seg.end` shrinking from work-stealing.
                let mut write_buf: Vec<u8> = Vec::with_capacity(HTTP_WRITE_BUFFER_BYTES);
                let flush_buf = |buf: &mut Vec<u8>| -> Result<bool, String> {
                    if buf.is_empty() {
                        return Ok(false);
                    }
                    let (write_offset, to_commit) = {
                        let segs = segs_arc.lock().unwrap();
                        let seg = &segs[seg_idx];
                        let rem = seg.remaining() as usize;
                        (seg.current_offset(), buf.len().min(rem))
                    };
                    if to_commit > 0 {
                        write_all_at_offset(file, &buf[..to_commit], write_offset)
                            .map_err(|e| format!("Disk write error: {}", e))?;
                        bytes_counter.fetch_add(to_commit as u64, Ordering::Relaxed);
                    }
                    buf.clear();
                    let done = {
                        let mut segs = segs_arc.lock().unwrap();
                        let seg = &mut segs[seg_idx];
                        seg.downloaded += to_commit as u64;
                        if seg.is_complete() {
                            seg.status = SegmentStatus::Completed;
                            true
                        } else {
                            false
                        }
                    };
                    Ok(done)
                };

                loop {
                    if *stop_rx.borrow() != StopSignal::Running {
                        let _ = flush_buf(&mut write_buf);
                        let mut segs = segs_arc.lock().unwrap();
                        if !segs[seg_idx].is_complete() {
                            segs[seg_idx].status = SegmentStatus::Idle;
                        }
                        return Ok(());
                    }

                    let chunk_res = tokio::select! {
                        c = resp.chunk() => c,
                        _ = stop_rx.changed() => {
                            let _ = flush_buf(&mut write_buf);
                            let mut segs = segs_arc.lock().unwrap();
                            if !segs[seg_idx].is_complete() {
                                segs[seg_idx].status = SegmentStatus::Idle;
                            }
                            return Ok(());
                        }
                    };

                    match chunk_res {
                        Ok(Some(chunk)) => {
                            if chunk.is_empty() {
                                continue;
                            }

                            // Determine how many bytes of this chunk still belong to our segment
                            // beyond what is already staged in `write_buf`.
                            let to_take = {
                                let segs = segs_arc.lock().unwrap();
                                let seg = &segs[seg_idx];
                                let rem = seg
                                    .remaining()
                                    .saturating_sub(write_buf.len() as u64)
                                    as usize;
                                chunk.len().min(rem)
                            };

                            if to_take == 0 {
                                let _ = flush_buf(&mut write_buf)?;
                                let mut segs = segs_arc.lock().unwrap();
                                segs[seg_idx].status = SegmentStatus::Completed;
                                return Ok(());
                            }

                            // Enforce global token-bucket bandwidth limit
                            bucket.consume(to_take as u64).await;

                            if *stop_rx.borrow() != StopSignal::Running {
                                let _ = flush_buf(&mut write_buf);
                                let mut segs = segs_arc.lock().unwrap();
                                if !segs[seg_idx].is_complete() {
                                    segs[seg_idx].status = SegmentStatus::Idle;
                                }
                                return Ok(());
                            }

                            write_buf.extend_from_slice(&chunk[..to_take]);

                            let need_flush = write_buf.len() >= HTTP_WRITE_BUFFER_BYTES || {
                                let segs = segs_arc.lock().unwrap();
                                write_buf.len() as u64 >= segs[seg_idx].remaining()
                            };
                            if need_flush && flush_buf(&mut write_buf)? {
                                return Ok(());
                            }
                        }
                        Ok(None) => {
                            // Stream ended — flush buffered bytes before checking completion
                            if flush_buf(&mut write_buf)? {
                                return Ok(());
                            }
                            let done = {
                                let mut segs = segs_arc.lock().unwrap();
                                let seg = &mut segs[seg_idx];
                                if seg.is_complete() {
                                    seg.status = SegmentStatus::Completed;
                                    true
                                } else {
                                    false
                                }
                            };
                            if done {
                                return Ok(());
                            }
                            // Premature EOF -> retry remaining range
                            attempt += 1;
                            if attempt > max_retries {
                                let mut segs = segs_arc.lock().unwrap();
                                segs[seg_idx].status = SegmentStatus::Failed;
                                return Err(format!("Segment {} ended prematurely", seg_idx));
                            }
                            break;
                        }
                        Err(e) => {
                            let _ = flush_buf(&mut write_buf);
                            attempt += 1;
                            {
                                let mut segs = segs_arc.lock().unwrap();
                                segs[seg_idx].retries = attempt;
                            }
                            if attempt > max_retries {
                                let mut segs = segs_arc.lock().unwrap();
                                segs[seg_idx].status = SegmentStatus::Failed;
                                return Err(format!("Stream read error: {}", e));
                            }
                            let backoff = Duration::from_secs((1u64 << attempt.min(4)).min(16));
                            tokio::select! {
                                _ = tokio::time::sleep(backoff) => break,
                                _ = stop_rx.changed() => return Ok(()),
                            }
                        }
                    }
                }
            }
            StatusCode::OK => {
                // Server either ignored `Range` OR rejected `If-Range` because the remote file changed!
                // Signal immediate clean fallback to single-stream download to prevent file corruption.
                let _ = stop_tx.send(StopSignal::FallbackSingleStream);
                return Ok(());
            }
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE | StatusCode::BAD_GATEWAY | StatusCode::GATEWAY_TIMEOUT => {
                attempt += 1;
                {
                    let mut segs = segs_arc.lock().unwrap();
                    segs[seg_idx].retries = attempt;
                }
                if attempt > max_retries {
                    let mut segs = segs_arc.lock().unwrap();
                    segs[seg_idx].status = SegmentStatus::Failed;
                    return Err(format!("Server returned HTTP {} repeatedly", resp.status()));
                }
                let backoff = Duration::from_secs((1u64 << attempt.min(4)).min(16));
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => continue,
                    _ = stop_rx.changed() => return Ok(()),
                }
            }
            other => {
                let mut segs = segs_arc.lock().unwrap();
                segs[seg_idx].status = SegmentStatus::Failed;
                return Err(format!("Unexpected HTTP status {} for range request", other));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_single_stream_download(
    state: &Arc<SharedState>,
    id: Uuid,
    url: &str,
    expected_total: Option<u64>,
    part_path: &std::path::Path,
    target_path: &std::path::Path,
    max_retries: u32,
    stop_rx: &mut watch::Receiver<StopSignal>,
) -> Result<TaskOutcome, String> {
    let bytes_counter = Arc::new(AtomicU64::new(0));
    let conns_counter = Arc::new(AtomicUsize::new(1));
    {
        state.live_bytes.lock().unwrap().insert(id, bytes_counter.clone());
        state.live_conns.lock().unwrap().insert(id, conns_counter.clone());
    }

    let mut attempt = 0u32;
    loop {
        if *stop_rx.borrow() == StopSignal::Pause {
            return Ok(TaskOutcome::Paused);
        }
        if *stop_rx.borrow() == StopSignal::Cancel {
            return Ok(TaskOutcome::Cancelled);
        }

        if let Some(parent) = part_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(part_path)
            .map_err(|e| format!("Failed opening output file: {}", e))?;

        bytes_counter.store(0, Ordering::Relaxed);

        let resp_res = tokio::select! {
            r = state.client.get(url).send() => r,
            _ = stop_rx.changed() => {
                return Ok(match *stop_rx.borrow() {
                    StopSignal::Cancel => TaskOutcome::Cancelled,
                    _ => TaskOutcome::Paused,
                });
            }
        };

        let mut resp = match resp_res {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                attempt += 1;
                if attempt > max_retries {
                    return Err(format!("HTTP error {}", r.status()));
                }
                tokio::time::sleep(Duration::from_secs(1u64 << attempt.min(4))).await;
                continue;
            }
            Err(e) => {
                attempt += 1;
                if attempt > max_retries {
                    return Err(format!("Connection failed: {}", e));
                }
                tokio::time::sleep(Duration::from_secs(1u64 << attempt.min(4))).await;
                continue;
            }
        };

        // Update total_bytes if server provides Content-Length on GET
        let content_len = resp.content_length().or(expected_total);
        if let Some(cl) = content_len {
            let mut downloads = state.downloads.write().unwrap();
            if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                item.total_bytes = Some(cl);
            }
        }

        let mut written: u64 = 0;
        let mut stream_err: Option<String> = None;

        loop {
            let chunk_res = tokio::select! {
                c = resp.chunk() => c,
                _ = stop_rx.changed() => {
                    return Ok(match *stop_rx.borrow() {
                        StopSignal::Cancel => TaskOutcome::Cancelled,
                        _ => TaskOutcome::Paused,
                    });
                }
            };

            match chunk_res {
                Ok(Some(chunk)) => {
                    state.bucket.consume(chunk.len() as u64).await;
                    file.write_all(&chunk)
                        .map_err(|e| format!("Disk write error: {}", e))?;
                    written += chunk.len() as u64;
                    bytes_counter.store(written, Ordering::Relaxed);
                }
                Ok(None) => break,
                Err(e) => {
                    stream_err = Some(e.to_string());
                    break;
                }
            }
        }

        if let Some(e) = stream_err {
            attempt += 1;
            if attempt > max_retries {
                return Err(format!("Stream interrupted: {}", e));
            }
            tokio::time::sleep(Duration::from_secs(1u64 << attempt.min(4))).await;
            continue;
        }

        let _ = file.sync_data();
        drop(file);

        {
            let mut downloads = state.downloads.write().unwrap();
            if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
                item.downloaded_bytes = written;
                if item.total_bytes.is_none() {
                    item.total_bytes = Some(written);
                }
            }
        }

        finalize_part_file(part_path, target_path)
            .map_err(|e| format!("Failed finalizing file: {}", e))?;
        return Ok(TaskOutcome::Completed);
    }
}

async fn verify_existing_download(state: Arc<SharedState>, id: Uuid, expected: Option<String>) {
    let (target_path, clean_expected) = {
        let mut downloads = state.downloads.write().unwrap();
        let Some(item) = downloads.iter_mut().find(|d| d.id == id) else {
            return;
        };
        if item.status != DownloadStatus::Completed {
            return;
        }
        let clean = expected
            .and_then(|s| {
                let t = s.trim().to_ascii_lowercase();
                if t.is_empty() { None } else { Some(t) }
            })
            .or_else(|| item.expected_sha256.clone());
        item.expected_sha256 = clean.clone();
        item.status = DownloadStatus::Verifying;
        (item.target_path(), clean)
    };
    state.refresh_snapshot();

    let hash_res = tokio::task::spawn_blocking(move || compute_file_sha256(&target_path)).await;

    {
        let mut downloads = state.downloads.write().unwrap();
        if let Some(item) = downloads.iter_mut().find(|d| d.id == id) {
            item.status = DownloadStatus::Completed;
            match hash_res {
                Ok(Ok(hash)) => {
                    let verified = clean_expected.as_ref().map(|e| e.eq_ignore_ascii_case(&hash));
                    item.computed_sha256 = Some(hash);
                    item.sha256_verified = verified;
                    if verified == Some(false) {
                        item.error_message = Some("SHA-256 checksum mismatch!".to_string());
                    } else {
                        item.error_message = None;
                    }
                }
                _ => {
                    item.error_message = Some("Could not read file for SHA-256 verification".to_string());
                }
            }
        }
    }
    state.persist_state();
    state.refresh_snapshot();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swarm_auto_refresh_stalled_and_zero_peers() {
        let t0 = Instant::now();
        let mut health = TorrentSwarmHealth::new(t0);

        // First observation at t0 with 0 peers: starts zero-peer timer, does not refresh immediately
        assert!(!should_auto_refresh_swarm(&mut health, t0, 0, 0.0));
        // Still under 30s zero-peer threshold
        assert!(!should_auto_refresh_swarm(
            &mut health,
            t0 + Duration::from_secs(29),
            0,
            0.0
        ));
        // At 30s with 0 peers -> stall recovery triggers
        assert!(should_auto_refresh_swarm(
            &mut health,
            t0 + Duration::from_secs(30),
            0,
            0.0
        ));

        // Progressive backoff: next 0-peer refresh waits 60s (2x)
        let t1 = t0 + Duration::from_secs(30);
        health.reset_after_auto_refresh(t1);
        assert!(!should_auto_refresh_swarm(&mut health, t1, 0, 0.0));
        assert!(!should_auto_refresh_swarm(
            &mut health,
            t1 + Duration::from_secs(45),
            0,
            0.0
        ));
        assert!(should_auto_refresh_swarm(
            &mut health,
            t1 + Duration::from_secs(60),
            0,
            0.0
        ));

        // Recovery above 64 KB/s resets backoff; subsequent sustained < 15 KB/s stall triggers at 45s
        let t2 = t1 + Duration::from_secs(60);
        health.reset_after_auto_refresh(t2);
        assert!(!should_auto_refresh_swarm(
            &mut health,
            t2 + Duration::from_secs(1),
            4,
            200.0 * 1024.0
        ));
        assert_eq!(health.consecutive_stalled_refreshes, 0);
        let t3 = t2 + Duration::from_secs(5);
        assert!(!should_auto_refresh_swarm(&mut health, t3, 3, 5.0 * 1024.0));
        assert!(!should_auto_refresh_swarm(
            &mut health,
            t3 + Duration::from_secs(44),
            3,
            5.0 * 1024.0
        ));
        assert!(should_auto_refresh_swarm(
            &mut health,
            t3 + Duration::from_secs(45),
            3,
            5.0 * 1024.0
        ));
    }

    #[test]
    fn test_swarm_auto_refresh_preserves_healthy_and_detects_collapse() {
        let t0 = Instant::now();
        let mut health = TorrentSwarmHealth::new(t0);

        // Healthy modest swarm (4 peers @ 100 KB/s or 9 peers @ 2.3 MB/s): NEVER interrupted!
        assert!(!should_auto_refresh_swarm(
            &mut health,
            t0 + Duration::from_secs(120),
            4,
            100.0 * 1024.0
        ));
        assert!(!should_auto_refresh_swarm(
            &mut health,
            t0 + Duration::from_secs(600),
            9,
            2.3 * 1024.0 * 1024.0
        ));

        // Severe collapse from 2.3 MB/s down to 20 KB/s continuously for 45s -> triggers recovery
        let t_collapse = t0 + Duration::from_secs(601);
        assert!(!should_auto_refresh_swarm(
            &mut health,
            t_collapse,
            4,
            20.0 * 1024.0
        ));
        assert!(!should_auto_refresh_swarm(
            &mut health,
            t_collapse + Duration::from_secs(44),
            4,
            20.0 * 1024.0
        ));
        assert!(should_auto_refresh_swarm(
            &mut health,
            t_collapse + Duration::from_secs(45),
            4,
            20.0 * 1024.0
        ));
    }

    #[test]
    fn test_sticky_completed_prevents_torrent_redownload_regression() {
        let mut item = DownloadItem::new_torrent(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_string(),
            "Show.S01E07.1080p.mkv".to_string(),
            std::path::PathBuf::from("C:\\Downloads"),
            Some(7),
            Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            false,
        );
        item.total_bytes = Some(3_360_000_000);
        item.downloaded_bytes = 3_353_280_000; // simulated 99.8% regression from librqbit
        item.status = DownloadStatus::Downloading;
        item.speed_bps = 60_000.0;
        item.active_connections = 6;
        item.completed_at = Some(Utc::now());

        let mut downloads = vec![item.clone()];
        let mut was_completed = HashMap::new();
        was_completed.insert(item.id, (Some(3_360_000_000), 3_360_000_000));
        let mut prev_torrent_bytes = HashMap::new();

        let to_pause = enforce_sticky_completed(&mut downloads, &was_completed, &mut prev_torrent_bytes);

        assert_eq!(to_pause.len(), 1);
        assert_eq!(to_pause[0].0, 7);
        assert_eq!(downloads[0].status, DownloadStatus::Completed);
        assert_eq!(downloads[0].downloaded_bytes, 3_360_000_000);
        assert_eq!(downloads[0].speed_bps, 0.0);
        assert_eq!(downloads[0].active_connections, 0);
        assert!(!downloads[0].torrent.as_ref().unwrap().seeding);

        // Once librqbit pauses it (status becomes Paused), sticky completed keeps it Completed
        // and does NOT need to issue another pause call.
        downloads[0].status = DownloadStatus::Paused;
        let to_pause_after = enforce_sticky_completed(&mut downloads, &was_completed, &mut prev_torrent_bytes);
        assert!(to_pause_after.is_empty());
        assert_eq!(downloads[0].status, DownloadStatus::Completed);
    }
}
