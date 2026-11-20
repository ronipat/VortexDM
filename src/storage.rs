use crate::models::{AppSettings, DownloadItem, DownloadStatus};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::fs::FileExt;
#[cfg(unix)]
use std::os::unix::fs::FileExt;

pub struct AppStorage {
    data_dir: PathBuf,
}

impl AppStorage {
    pub fn default_app_storage() -> Self {
        let data_dir = directories::ProjectDirs::from("com", "VortexDM", "VortexDM")
            .map(|p| p.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".vortexdm"));
        let _ = fs::create_dir_all(&data_dir);
        Self { data_dir }
    }

    pub fn with_dir(data_dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&data_dir);
        Self { data_dir }
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn state_path(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }

    pub fn settings_path(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    pub fn load_settings(&self) -> AppSettings {
        let path = self.settings_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            let clean = raw.trim_start_matches('\u{feff}');
            if let Ok(mut settings) = serde_json::from_str::<AppSettings>(clean) {
                settings.default_connections = settings.default_connections.clamp(1, 32);
                settings.max_concurrent_downloads = settings.max_concurrent_downloads.clamp(1, 10);
                return settings;
            }
        }
        AppSettings::default()
    }

    pub fn save_settings(&self, settings: &AppSettings) -> io::Result<()> {
        let json = serde_json::to_string_pretty(settings)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        atomic_write(&self.settings_path(), json.as_bytes())
    }

    pub fn load_downloads(&self) -> Vec<DownloadItem> {
        let path = self.state_path();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(mut items) = serde_json::from_str::<Vec<DownloadItem>>(&raw) {
                // Any downloads that were actively running when app exited/crashed become Paused
                // so the user can cleanly resume them without unexpected bandwidth spikes.
                // However, any download that already finished (`completed_at.is_some()`) stays
                // Completed so finished torrents never regress into re-downloading on restart.
                for item in &mut items {
                    if item.completed_at.is_some() || item.status == DownloadStatus::Completed {
                        item.status = DownloadStatus::Completed;
                        if let Some(total) = item.total_bytes {
                            item.downloaded_bytes = item.downloaded_bytes.max(total);
                        }
                        if let Some(t) = item.torrent.as_mut() {
                            t.seeding = false;
                            t.upload_speed_bps = 0.0;
                            t.peers_live = 0;
                            t.peers_seeders = 0;
                            t.peers_leechers = 0;
                        }
                    } else if matches!(
                        item.status,
                        DownloadStatus::Downloading
                            | DownloadStatus::Probing
                            | DownloadStatus::Verifying
                    ) {
                        item.status = DownloadStatus::Paused;
                    }
                    item.active_connections = 0;
                    item.speed_bps = 0.0;
                    item.eta_seconds = None;
                }
                return items;
            }
        }
        Vec::new()
    }

    pub fn save_downloads(&self, items: &[DownloadItem]) -> io::Result<()> {
        let json = serde_json::to_string_pretty(items)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        atomic_write(&self.state_path(), json.as_bytes())
    }

    pub fn inbox_dir(&self) -> PathBuf {
        self.data_dir.join("inbox")
    }

    /// Atomically enqueue a `.torrent` path, `magnet:?` URI, or HTTP URL for an already-running
    /// VortexDM instance to pick up immediately and start downloading.
    pub fn enqueue_ipc_target(&self, target: &str) -> io::Result<()> {
        let dir = self.inbox_dir();
        fs::create_dir_all(&dir)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let uid = uuid::Uuid::new_v4().simple().to_string();
        let tmp = dir.join(format!("{:020}_{}_{}.tmp", nanos, pid, &uid[..8]));
        let req = tmp.with_extension("req");
        fs::write(&tmp, target.as_bytes())?;
        fs::rename(&tmp, &req)?;
        Ok(())
    }

    /// Drain all pending `.req` IPC targets written by secondary instances (FIFO order).
    pub fn drain_ipc_inbox(&self) -> Vec<String> {
        let dir = self.inbox_dir();
        if !dir.exists() {
            return Vec::new();
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut req_files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("req"))
            })
            .collect();
        if req_files.is_empty() {
            return Vec::new();
        }
        req_files.sort();
        let mut out = Vec::with_capacity(req_files.len());
        for path in req_files {
            if let Ok(raw) = fs::read_to_string(&path) {
                let cleaned = raw.trim().to_string();
                if !cleaned.is_empty() {
                    out.push(cleaned);
                }
            }
            let _ = fs::remove_file(&path);
        }
        out
    }
}

/// Write bytes atomically using temp file + rename so crashes never corrupt persisted JSON.
pub fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    // On Windows, fs::rename may fail if destination exists on older filesystems; replace safely.
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Open or create a `.vdm.part` file and pre-allocate `total_bytes` to prevent NTFS fragmentation.
pub fn open_or_preallocate_part_file(part_path: &Path, total_bytes: u64, resume: bool) -> io::Result<File> {
    if let Some(parent) = part_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(!resume)
        .open(part_path)?;

    let current_len = file.metadata()?.len();
    if !resume || current_len != total_bytes {
        file.set_len(total_bytes)?;
    }
    Ok(file)
}

/// Lock-free positional write at exact byte offset (Win32 `WriteFile` with OVERLAPPED offset).
pub fn write_all_at_offset(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        #[cfg(windows)]
        let written = file.seek_write(buf, offset)?;
        #[cfg(unix)]
        let written = file.write_at(buf, offset)?;

        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "Failed to write bytes to disk at offset",
            ));
        }
        buf = &buf[written..];
        offset += written as u64;
    }
    Ok(())
}

/// Rename `.vdm.part` to final path with brief retry (handles transient Windows Defender scan locks).
pub fn finalize_part_file(part_path: &Path, target_path: &Path) -> io::Result<()> {
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut last_err = None;
    for attempt in 0..8 {
        if target_path.exists() {
            let _ = fs::remove_file(target_path);
        }
        match fs::rename(part_path, target_path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(60 * (attempt + 1)));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::new(io::ErrorKind::Other, "Rename failed")))
}

/// Streaming SHA-256 digest computation using a 64 KB buffer (constant low memory for multi-GB files).
pub fn compute_file_sha256(path: &Path) -> io::Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}
