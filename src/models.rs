use chrono::{DateTime, Utc};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use url::Url;
use uuid::Uuid;

pub const APP_NAME: &str = "VortexDM";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const WINDOW_TITLE: &str = "VortexDM — Download Manager";
pub const TRAY_POPUP_TITLE: &str = "VortexDM Tray Popup";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Language {
    #[default]
    English,
    Indonesian,
}

impl Language {
    pub fn tr<'a>(self, en: &'a str, id: &'a str) -> &'a str {
        match self {
            Language::English => en,
            Language::Indonesian => id,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Language::English => "EN",
            Language::Indonesian => "ID",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Indonesian => "Bahasa Indonesia",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadStatus {
    Queued,
    Probing,
    Downloading,
    Paused,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

impl DownloadStatus {
    pub fn label(&self) -> &'static str {
        self.label_tr(Language::English)
    }

    pub fn label_tr(&self, lang: Language) -> &'static str {
        match self {
            DownloadStatus::Queued => lang.tr("Queued", "Antrean"),
            DownloadStatus::Probing => lang.tr("Connecting", "Menghubungkan"),
            DownloadStatus::Downloading => lang.tr("Downloading", "Mengunduh"),
            DownloadStatus::Paused => lang.tr("Paused", "Dijeda"),
            DownloadStatus::Verifying => lang.tr("Verifying", "Memverifikasi"),
            DownloadStatus::Completed => lang.tr("Completed", "Selesai"),
            DownloadStatus::Failed => lang.tr("Failed", "Gagal"),
            DownloadStatus::Cancelled => lang.tr("Cancelled", "Dibatalkan"),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            DownloadStatus::Probing | DownloadStatus::Downloading | DownloadStatus::Verifying
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileCategory {
    Programs,
    Archives,
    Videos,
    Music,
    Documents,
    Images,
    Torrents,
    Other,
}

impl FileCategory {
    pub const ALL: [FileCategory; 8] = [
        FileCategory::Programs,
        FileCategory::Archives,
        FileCategory::Videos,
        FileCategory::Music,
        FileCategory::Documents,
        FileCategory::Images,
        FileCategory::Torrents,
        FileCategory::Other,
    ];

    pub fn label(&self) -> &'static str {
        self.label_tr(Language::English)
    }

    pub fn label_tr(&self, lang: Language) -> &'static str {
        match self {
            FileCategory::Programs => lang.tr("Programs", "Program"),
            FileCategory::Archives => lang.tr("Archives", "Arsip"),
            FileCategory::Videos => lang.tr("Videos", "Video"),
            FileCategory::Music => lang.tr("Music", "Musik"),
            FileCategory::Documents => lang.tr("Documents", "Dokumen"),
            FileCategory::Images => lang.tr("Images", "Gambar"),
            FileCategory::Torrents => lang.tr("Torrents", "Torrent"),
            FileCategory::Other => lang.tr("Other", "Lainnya"),
        }
    }

    pub fn folder_name(&self) -> &'static str {
        self.label()
    }

    pub fn from_filename(filename: &str) -> Self {
        let ext = Path::new(filename)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();

        match ext.as_str() {
            "exe" | "msi" | "msix" | "bat" | "cmd" | "ps1" | "apk" | "appimage" | "deb"
            | "rpm" | "dmg" | "pkg" | "iso" | "img" => FileCategory::Programs,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "tgz" | "cab" => {
                FileCategory::Archives
            }
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "ts" => {
                FileCategory::Videos
            }
            "mp3" | "flac" | "wav" | "aac" | "ogg" | "m4a" | "wma" | "opus" => {
                FileCategory::Music
            }
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "csv"
            | "epub" | "rtf" | "json" | "xml" => FileCategory::Documents,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "tiff" | "avif" => {
                FileCategory::Images
            }
            "torrent" => FileCategory::Torrents,
            _ => FileCategory::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DownloadKind {
    #[default]
    Http,
    Torrent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentFileEntry {
    pub relative_path: String,
    pub length: u64,
    pub downloaded: u64,
}

impl TorrentFileEntry {
    pub fn progress(&self) -> f32 {
        if self.length == 0 {
            1.0
        } else {
            (self.downloaded as f64 / self.length as f64).clamp(0.0, 1.0) as f32
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TorrentInfo {
    #[serde(skip)]
    pub rqbit_id: Option<usize>,
    pub info_hash: Option<String>,
    pub source: String,
    pub uploaded_bytes: u64,
    #[serde(skip)]
    pub upload_speed_bps: f64,
    #[serde(skip)]
    pub peers_live: u32,
    #[serde(skip)]
    pub peers_seeders: u32,
    #[serde(skip)]
    pub peers_leechers: u32,
    #[serde(skip)]
    pub peers_seen: u32,
    #[serde(skip)]
    pub peers_utp: u32,
    #[serde(skip)]
    pub last_swarm_refresh_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub files: Vec<TorrentFileEntry>,
    #[serde(default)]
    pub seeding: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentStatus {
    Idle,
    Active,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentState {
    pub id: usize,
    pub start: u64,
    /// Exclusive upper bound (bytes in `[start, end)`), or inclusive?
    /// We use inclusive `[start, end]` matching HTTP `Range: bytes=start-end`!
    pub end: u64,
    /// Number of bytes already written inside this segment (`0..=(end - start + 1)`).
    pub downloaded: u64,
    pub status: SegmentStatus,
    pub retries: u32,
}

impl SegmentState {
    pub fn total_len(&self) -> u64 {
        if self.end >= self.start {
            self.end - self.start + 1
        } else {
            0
        }
    }

    pub fn current_offset(&self) -> u64 {
        self.start.saturating_add(self.downloaded)
    }

    pub fn remaining(&self) -> u64 {
        self.total_len().saturating_sub(self.downloaded)
    }

    pub fn is_complete(&self) -> bool {
        self.downloaded >= self.total_len()
    }

    pub fn progress(&self) -> f32 {
        let len = self.total_len();
        if len == 0 {
            1.0
        } else {
            (self.downloaded as f64 / len as f64).clamp(0.0, 1.0) as f32
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadItem {
    pub id: Uuid,
    #[serde(default)]
    pub kind: DownloadKind,
    pub url: String,
    pub final_url: Option<String>,
    pub filename: String,
    pub save_dir: PathBuf,
    pub category: FileCategory,
    pub status: DownloadStatus,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub supports_ranges: bool,
    pub max_connections: usize,
    pub active_connections: usize,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
    pub segments: Vec<SegmentState>,
    #[serde(default)]
    pub torrent: Option<TorrentInfo>,
    pub expected_sha256: Option<String>,
    pub computed_sha256: Option<String>,
    pub sha256_verified: Option<bool>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,

    // Ephemeral runtime telemetry (not persisted)
    #[serde(skip)]
    pub speed_bps: f64,
    #[serde(skip)]
    pub peak_speed_bps: f64,
    #[serde(skip)]
    pub eta_seconds: Option<f64>,
    #[serde(skip)]
    pub speed_history: VecDeque<f64>,
}

impl DownloadItem {
    pub fn new(
        url: String,
        filename: String,
        save_dir: PathBuf,
        max_connections: usize,
        expected_sha256: Option<String>,
        start_paused: bool,
    ) -> Self {
        let category = FileCategory::from_filename(&filename);
        Self {
            id: Uuid::new_v4(),
            kind: DownloadKind::Http,
            url,
            final_url: None,
            filename,
            save_dir,
            category,
            status: if start_paused {
                DownloadStatus::Paused
            } else {
                DownloadStatus::Queued
            },
            total_bytes: None,
            downloaded_bytes: 0,
            supports_ranges: false,
            max_connections: max_connections.clamp(1, 32),
            active_connections: 0,
            etag: None,
            last_modified: None,
            content_type: None,
            segments: Vec::new(),
            torrent: None,
            expected_sha256: expected_sha256.and_then(|s| {
                let t = s.trim().to_ascii_lowercase();
                if t.is_empty() { None } else { Some(t) }
            }),
            computed_sha256: None,
            sha256_verified: None,
            error_message: None,
            created_at: Utc::now(),
            completed_at: None,
            speed_bps: 0.0,
            peak_speed_bps: 0.0,
            eta_seconds: None,
            speed_history: VecDeque::with_capacity(90),
        }
    }

    pub fn new_torrent(
        source: String,
        display_name: String,
        save_dir: PathBuf,
        rqbit_id: Option<usize>,
        info_hash: Option<String>,
        start_paused: bool,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: DownloadKind::Torrent,
            url: source.clone(),
            final_url: None,
            filename: display_name,
            save_dir,
            category: FileCategory::Torrents,
            status: if start_paused {
                DownloadStatus::Paused
            } else {
                DownloadStatus::Probing
            },
            total_bytes: None,
            downloaded_bytes: 0,
            supports_ranges: true,
            max_connections: 0,
            active_connections: 0,
            etag: None,
            last_modified: None,
            content_type: Some("application/x-bittorrent".to_string()),
            segments: Vec::new(),
            torrent: Some(TorrentInfo {
                rqbit_id,
                info_hash,
                source,
                ..Default::default()
            }),
            expected_sha256: None,
            computed_sha256: None,
            sha256_verified: None,
            error_message: None,
            created_at: Utc::now(),
            completed_at: None,
            speed_bps: 0.0,
            peak_speed_bps: 0.0,
            eta_seconds: None,
            speed_history: VecDeque::with_capacity(90),
        }
    }

    pub fn is_torrent(&self) -> bool {
        self.kind == DownloadKind::Torrent
    }

    pub fn target_path(&self) -> PathBuf {
        self.save_dir.join(&self.filename)
    }

    pub fn part_path(&self) -> PathBuf {
        self.save_dir.join(format!("{}.vdm.part", self.filename))
    }

    pub fn progress(&self) -> f32 {
        match self.status {
            DownloadStatus::Completed => 1.0,
            _ => match self.total_bytes {
                Some(total) if total > 0 => {
                    (self.downloaded_bytes as f64 / total as f64).clamp(0.0, 1.0) as f32
                }
                _ => 0.0,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProbeInfo {
    pub final_url: String,
    pub filename: String,
    pub total_bytes: Option<u64>,
    pub supports_ranges: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
}

fn default_dark_mode() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub download_dir: PathBuf,
    pub organize_by_category: bool,
    pub default_connections: usize,
    pub max_concurrent_downloads: usize,
    /// Global bandwidth limit in bytes/sec (0 = unlimited).
    pub global_speed_limit_bps: u64,
    pub close_to_tray: bool,
    pub start_with_windows: bool,
    pub start_minimized: bool,
    pub clipboard_monitoring: bool,
    pub auto_verify_sha256: bool,
    pub max_retries: u32,
    #[serde(default = "default_dark_mode")]
    pub dark_mode: bool,
    #[serde(default)]
    pub language: Language,
}

impl Default for AppSettings {
    fn default() -> Self {
        let download_dir = directories::UserDirs::new()
            .and_then(|u| u.download_dir().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from(r"C:\Downloads"));

        Self {
            download_dir,
            organize_by_category: false,
            default_connections: 8,
            max_concurrent_downloads: 3,
            global_speed_limit_bps: 0,
            close_to_tray: true,
            start_with_windows: false,
            start_minimized: true,
            clipboard_monitoring: true,
            auto_verify_sha256: false,
            max_retries: 5,
            dark_mode: true,
            language: Language::English,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GlobalStats {
    pub total_speed_bps: f64,
    pub total_upload_speed_bps: f64,
    pub active_count: usize,
    pub queued_count: usize,
    pub paused_count: usize,
    pub completed_count: usize,
    pub failed_count: usize,
    pub torrent_count: usize,
    pub total_connections: usize,
    pub session_downloaded_bytes: u64,
    pub speed_history: VecDeque<f64>,
}

#[derive(Debug, Clone)]
pub enum TorrentSource {
    MagnetOrUrl(String),
    FilePath(PathBuf),
}

#[derive(Debug, Clone)]
pub struct NewTorrentRequest {
    pub source: TorrentSource,
    pub save_dir: Option<PathBuf>,
    pub start_paused: bool,
}

#[derive(Debug, Clone)]
pub struct NewDownloadRequest {
    pub url: String,
    pub custom_filename: Option<String>,
    pub save_dir: Option<PathBuf>,
    pub max_connections: Option<usize>,
    pub expected_sha256: Option<String>,
    pub start_paused: bool,
}

#[derive(Debug, Clone)]
pub enum EngineCommand {
    Add(NewDownloadRequest),
    AddTorrent(NewTorrentRequest),
    Pause(Uuid),
    Resume(Uuid),
    Cancel(Uuid),
    Remove { id: Uuid, delete_file: bool },
    RemoveMany { ids: Vec<Uuid>, delete_file: bool },
    PauseAll,
    ResumeAll,
    ClearCompleted,
    RefreshTorrentPeers(Uuid),
    VerifySha256 { id: Uuid, expected: Option<String> },
    UpdateSettings(AppSettings),
    SetGlobalSpeedLimit(u64),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum TrayAction {
    ToggleWindow,
    ShowWindow,
    NewDownload,
    PauseAll,
    ResumeAll,
    SetSpeedLimit(u64),
    OpenDownloadsFolder,
    Quit,
}

/// Choose optimal connection count adaptively based on file size and user cap.
pub fn recommended_connections(total_bytes: Option<u64>, supports_ranges: bool, user_max: usize) -> usize {
    if !supports_ranges {
        return 1;
    }
    let cap = user_max.clamp(1, 32);
    match total_bytes {
        None => 1,
        Some(size) if size < 1_048_576 => 1,             // < 1 MB: single connection avoids handshake overhead
        Some(size) if size < 10 * 1_048_576 => cap.min(4), // 1 - 10 MB: up to 4
        Some(size) if size < 100 * 1_048_576 => cap.min(8), // 10 - 100 MB: up to 8
        Some(_) => cap,                                   // >= 100 MB: up to user cap (default 8, up to 32)
    }
}

/// Format bytes in human-readable binary units (KB, MB, GB, TB).
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;

    let b = bytes as f64;
    if b >= TB {
        format!("{:.2} TB", b / TB)
    } else if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

/// Format transfer rate in human-readable units.
pub fn format_speed(bps: f64) -> String {
    if bps <= 0.5 {
        return "0 B/s".to_string();
    }
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    if bps >= GB {
        format!("{:.2} GB/s", bps / GB)
    } else if bps >= MB {
        format!("{:.2} MB/s", bps / MB)
    } else if bps >= KB {
        format!("{:.1} KB/s", bps / KB)
    } else {
        format!("{:.0} B/s", bps)
    }
}

/// Format remaining time in concise `Hh Mm Ss` notation.
pub fn format_eta(seconds: Option<f64>) -> String {
    match seconds {
        None => "—".to_string(),
        Some(s) if !s.is_finite() || s < 0.0 => "—".to_string(),
        Some(s) if s > 86_400.0 * 7.0 => "> 7d".to_string(),
        Some(s) => {
            let total = s.ceil() as u64;
            let h = total / 3600;
            let m = (total % 3600) / 60;
            let sec = total % 60;
            if h > 0 {
                format!("{}h {:02}m {:02}s", h, m, sec)
            } else if m > 0 {
                format!("{}m {:02}s", m, sec)
            } else {
                format!("{}s", sec)
            }
        }
    }
}

/// Parse RFC 5987 (`filename*=UTF-8''...`) and standard `filename="..."` from Content-Disposition.
pub fn parse_content_disposition(header_val: &str) -> Option<String> {
    // 1. Check for RFC 5987 `filename*=`
    for part in header_val.split(';') {
        let trimmed = part.trim();
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("filename*=") {
            let orig_val = &trimmed[trimmed.len() - rest.len()..];
            let orig_val = orig_val.trim_matches('"');
            // Format: charset'lang'encoded_value
            if let Some(idx) = orig_val.rfind('\'') {
                let encoded = &orig_val[idx + 1..];
                if let Ok(decoded) = percent_decode_str(encoded).decode_utf8() {
                    let clean = sanitize_filename(&decoded);
                    if !clean.is_empty() {
                        return Some(clean);
                    }
                }
            }
        }
    }

    // 2. Standard `filename=`
    for part in header_val.split(';') {
        let trimmed = part.trim();
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("filename=") {
            let orig_val = &trimmed[trimmed.len() - rest.len()..];
            let unquoted = orig_val.trim().trim_matches('"').trim_matches('\'');
            let decoded = percent_decode_str(unquoted)
                .decode_utf8()
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| unquoted.to_string());
            let clean = sanitize_filename(&decoded);
            if !clean.is_empty() {
                return Some(clean);
            }
        }
    }

    None
}

/// Extract a sanitized filename from a URL path or generate a safe fallback.
pub fn filename_from_url(url_str: &str) -> String {
    if let Ok(parsed) = Url::parse(url_str) {
        if let Some(mut segments) = parsed.path_segments() {
            if let Some(last) = segments.next_back() {
                if !last.is_empty() {
                    let decoded = percent_decode_str(last)
                        .decode_utf8()
                        .map(|c| c.into_owned())
                        .unwrap_or_else(|_| last.to_string());
                    let clean = sanitize_filename(&decoded);
                    if !clean.is_empty() && clean != "." && clean != ".." {
                        return clean;
                    }
                }
            }
        }
    }
    format!("download_{}.bin", Utc::now().format("%Y%m%d_%H%M%S"))
}

/// Strip Windows-illegal filename characters and guard against reserved DOS device names.
pub fn sanitize_filename(raw: &str) -> String {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw)
        .trim();

    let mut cleaned: String = base
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();

    cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();

    if cleaned.is_empty() {
        return String::new();
    }

    // Check Windows reserved device names (CON, PRN, AUX, NUL, COM1..9, LPT1..9)
    let stem = Path::new(&cleaned)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&cleaned)
        .to_ascii_uppercase();

    let is_reserved = matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );

    if is_reserved {
        cleaned = format!("_{}", cleaned);
    }

    // Clamp max filename length safely below Windows MAX_PATH component limit
    if cleaned.len() > 180 {
        if let Some(ext) = Path::new(&cleaned).extension().and_then(|e| e.to_str()) {
            let max_stem = 175usize.saturating_sub(ext.len());
            let stem_slice: String = cleaned.chars().take(max_stem).collect();
            cleaned = format!("{}.{}", stem_slice, ext);
        } else {
            cleaned = cleaned.chars().take(180).collect();
        }
    }

    cleaned
}

/// Produce a non-colliding filename inside `dir` (e.g., `archive (1).zip`).
pub fn deduplicate_filename(dir: &Path, filename: &str, existing_names: &[String]) -> String {
    let candidate_exists = |name: &str| {
        dir.join(name).exists()
            || dir.join(format!("{}.vdm.part", name)).exists()
            || existing_names.iter().any(|e| e.eq_ignore_ascii_case(name))
    };

    if !candidate_exists(filename) {
        return filename.to_string();
    }

    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let ext = path.extension().and_then(|e| e.to_str());

    for i in 1..10_000 {
        let next = match ext {
            Some(e) if !e.is_empty() => format!("{} ({}).{}", stem, i, e),
            _ => format!("{} ({})", stem, i),
        };
        if !candidate_exists(&next) {
            return next;
        }
    }

    format!("{}_{}", Uuid::new_v4(), filename)
}

/// Detect whether a string is a BitTorrent magnet URI or a `.torrent` file/URL.
pub fn is_torrent_input(text: &str) -> bool {
    let cleaned = text.trim().trim_matches('"').trim();
    if cleaned.is_empty() {
        return false;
    }
    let lower = cleaned.to_ascii_lowercase();
    if lower.starts_with("magnet:?") || lower.ends_with(".torrent") {
        return true;
    }
    if let Ok(parsed) = Url::parse(cleaned) {
        if parsed.scheme().eq_ignore_ascii_case("magnet") {
            return true;
        }
        if parsed.path().to_ascii_lowercase().ends_with(".torrent") {
            return true;
        }
    }
    false
}

/// Normalize a Windows CLI argument (`.torrent` file path, `file:///` URI, `magnet:?`, or HTTP/HTTPS URL).
pub fn normalize_cli_target(raw: &str) -> Option<String> {
    let cleaned = raw.trim().trim_matches('"').trim();
    if cleaned.is_empty() || cleaned.starts_with('-') {
        return None;
    }
    let lower = cleaned.to_ascii_lowercase();
    if lower.starts_with("file://") {
        if let Ok(parsed) = Url::parse(cleaned) {
            if let Ok(path) = parsed.to_file_path() {
                let s = path.display().to_string();
                if is_torrent_input(&s) {
                    return Some(s);
                }
            }
        }
    }
    if lower.starts_with("http://") || lower.starts_with("https://") || is_torrent_input(cleaned) {
        return Some(cleaned.to_string());
    }
    let p = Path::new(cleaned);
    if p.exists()
        && p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
    {
        return Some(cleaned.to_string());
    }
    None
}

/// Extract display name (`dn=...`) or infohash summary from a BitTorrent magnet URI.
pub fn magnet_display_name(magnet: &str) -> String {
    if let Ok(parsed) = Url::parse(magnet) {
        let mut dn: Option<String> = None;
        let mut btih: Option<String> = None;
        for (k, v) in parsed.query_pairs() {
            if k == "dn" && !v.trim().is_empty() {
                dn = Some(sanitize_filename(&v));
            } else if k == "xt" {
                if let Some(hash) = v.strip_prefix("urn:btih:") {
                    btih = Some(hash.to_string());
                }
            }
        }
        if let Some(name) = dn.filter(|s| !s.is_empty()) {
            return name;
        }
        if let Some(hash) = btih {
            let short = &hash[..hash.len().min(12)];
            return format!("Magnet ({}...)", short);
        }
    }
    "BitTorrent Magnet".to_string()
}

/// Detect whether clipboard text looks like a direct downloadable HTTP/HTTPS file URL or Magnet URI.
pub fn is_likely_download_url(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() > 4096 || trimmed.contains('\n') || trimmed.contains(' ') {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("magnet:?") && lower.contains("xt=urn:btih:") {
        return true;
    }
    let Ok(parsed) = Url::parse(trimmed) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    let path = parsed.path().to_ascii_lowercase();
    let known_exts = [
        ".exe", ".msi", ".msix", ".zip", ".rar", ".7z", ".tar", ".gz", ".xz", ".zst", ".iso",
        ".img", ".dmg", ".deb", ".rpm", ".apk", ".pdf", ".mp4", ".mkv", ".webm", ".avi", ".mov",
        ".mp3", ".flac", ".wav", ".ogg", ".png", ".jpg", ".jpeg", ".webp", ".bin", ".csv",
        ".parquet", ".gguf", ".safetensors", ".torrent",
    ];
    known_exts.iter().any(|ext| path.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_disposition_rfc5987_and_standard() {
        assert_eq!(
            parse_content_disposition(
                "attachment; filename=\"fallback.zip\"; filename*=UTF-8''my%20report%202026.pdf"
            ),
            Some("my report 2026.pdf".to_string())
        );
        assert_eq!(
            parse_content_disposition("attachment; filename=\"installer_v1.exe\""),
            Some("installer_v1.exe".to_string())
        );
    }

    #[test]
    fn test_sanitize_windows_filename() {
        assert_eq!(sanitize_filename("my:file*name?.zip"), "my_file_name_.zip");
        assert_eq!(sanitize_filename("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_filename("NUL"), "_NUL");
        assert_eq!(sanitize_filename("trailing... "), "trailing");
    }

    #[test]
    fn test_adaptive_connections() {
        assert_eq!(recommended_connections(Some(500_000), true, 16), 1);
        assert_eq!(recommended_connections(Some(5_000_000), true, 16), 4);
        assert_eq!(recommended_connections(Some(50_000_000), true, 16), 8);
        assert_eq!(recommended_connections(Some(500_000_000), true, 16), 16);
        assert_eq!(recommended_connections(Some(500_000_000), false, 16), 1);
        assert_eq!(recommended_connections(None, true, 16), 1);
    }

    #[test]
    fn test_format_helpers() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(10 * 1024 * 1024), "10.00 MB");
        assert_eq!(format_speed(2.5 * 1024.0 * 1024.0), "2.50 MB/s");
        assert_eq!(format_eta(Some(65.0)), "1m 05s");
        assert_eq!(format_eta(Some(3661.0)), "1h 01m 01s");
    }

    #[test]
    fn test_torrent_and_magnet_detection() {
        let magnet = "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&dn=Ubuntu%2024.04.iso";
        let reordered_magnet = "magnet:?dn=Fedora&xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862";
        assert!(is_torrent_input(magnet));
        assert!(is_torrent_input(reordered_magnet));
        assert!(is_likely_download_url(magnet));
        assert!(is_likely_download_url(reordered_magnet));
        assert_eq!(magnet_display_name(magnet), "Ubuntu 24.04.iso");
        assert!(is_torrent_input("C:\\Downloads\\fedora.torrent"));
        assert!(is_torrent_input("\"C:\\Downloads\\fedora.torrent\""));
        assert!(is_torrent_input("https://tracker.test/dl/file.torrent?passkey=abc"));
        assert_eq!(
            normalize_cli_target("\"C:\\Downloads\\my show.torrent\""),
            Some("C:\\Downloads\\my show.torrent".to_string())
        );
        assert_eq!(normalize_cli_target("--minimized"), None);
        assert!(!is_torrent_input("https://example.com/archive.zip"));
    }
}
