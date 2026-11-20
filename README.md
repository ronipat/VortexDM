# VortexDM — High-Performance Windows Download & BitTorrent Manager (Rust)

**VortexDM v1.1.0** is a native Windows download & BitTorrent manager written in **100% Rust** (`tokio`, `reqwest`, `librqbit`, `eframe`/`egui`, `tray-icon`, `windows-sys`) with a dual-theme 9Router-inspired UI (Warm Charcoal Dark & Warm Paper Light), custom Windows System Tray Popup Flyout, dynamic HTTP multi-segmentation, and full BitTorrent / Magnet Link support.

See [CHANGELOG.md](CHANGELOG.md) for a detailed breakdown of all development changes in v1.1.0.

## Features

- **Dynamic Multi-Segment HTTP/HTTPS Engine**:
  - `HEAD` + `GET Range: bytes=0-0` fallback probing, RFC 5987 `Content-Disposition` decoding, and Windows disk space pre-checks (`GetDiskFreeSpaceExW`).
  - Adaptive connection scaling (`1–32` streams) with IDM-style **dynamic in-half work stealing**, NTFS `.vdm.part` pre-allocation, lock-free Win32 `FileExt::seek_write`, and `If-Range` (`ETag`/`Last-Modified`) resume validation.
  - EWMA speed smoothing, stable ETA, live sparkline telemetry, interactive **Multi-Segment Byte Map**, and constant-memory streaming **SHA-256 verification**.
- **Native BitTorrent & Magnet Engine (`librqbit 9.1`)**:
  - Supports `magnet:?xt=urn:btih:...` links and local/remote `.torrent` files (including drag-and-drop and Windows `.torrent` & `magnet:` protocol association via the installer).
  - Decentralized **Kademlia DHT (BEP-5)**, HTTP/HTTPS & **UDP Trackers (BEP-15)**, **BEP-9 `ut_metadata`**, **uTP/TCP Peer Wire**, automatic piece SHA-1 verification, fast-resume persistence, live DL/UL speeds, peer/seeder counts, and multi-file tree inspection.
  - Automatic confirmation modal when opening `.torrent` files or `magnet:?` links, plus sticky-completed and non-blocking pause/resume/delete synchronization.
- **Streamlined Workspace, Multi-Select & Confirmation Modals**:
  - Segmented **Downloads / Completed** view with scoped **`Pause All`** / **`Resume All`** controls on the **Downloads** tab and confirmed **`Clear Completed`** cleanup on the **Completed** tab.
  - Compact download rows with responsive auto-truncating filenames, inline progress bars, and aligned multi-select (`Select All`, `Ctrl+A`, `Esc`, `Delete`, `Shift+Delete`) with a contextual bulk action bar.
  - Confirmation modals for **New Download / Confirm Torrent Download**, **Delete Download(s)** (with optional disk file deletion), and **Clear Completed Downloads**.
- **Bilingual UI (English & Bahasa Indonesia)**:
  - Instant header **`EN` / `ID`** language switch and **Settings** selector translating the main workspace, modals, status pills, inspector, and system tray flyout between **English** and **Bahasa Indonesia**.
- **Global Bandwidth Throttling**:
  - Live rate limits (`Unlimited`, `512 KB/s`, `2 MB/s`, `5 MB/s`, `10 MB/s`, `25 MB/s`, or custom KB/s) applied across active transfers.
- **Custom Windows System Tray Flyout & Single-Instance IPC**:
  - Frameless, rounded (`276x354`) always-on-top tray popup flyout synchronized with your Dark/Light theme and Windows 11 DWM window chrome, featuring live DL/UL speed, quick controls (`+ New`, `Pause All`, `Resume All`), `Dark`/`Light` switch, `Start with Windows` toggle, and `Quit`.
  - Close-to-tray interception, `--minimized` startup flag (autostart unchecked by default in installer), clipboard URL/magnet auto-detection, and Win32 single-instance enforcement with an atomic file IPC inbox forwarding secondary `.torrent` / `magnet:?` / URL launches to the running window.

## Building from Source

```powershell
# Run unit & integration tests (18 tests covering HTTP + BitTorrent + IPC)
cargo test

# Build release executable + Windows Setup Installer (dist\VortexDM-Setup-1.1.0.exe)
.\build_installer.ps1
```

## CLI Arguments

- `--minimized` / `-m`: Launches silently into the Windows System Tray.
- `<http(s)://url | magnet:?xt=... | path\to\file.torrent>`: Opens in the confirmation modal (forwarded via single-instance IPC inbox if VortexDM is already running).
