# Changelog

All notable architecture, engine, UI, OS integration, and installer changes made during the development of **VortexDM** are documented in this file.

## [1.1.0] — Development & Stabilization Release

### Bilingual Localization (English & Bahasa Indonesia) (`src/models.rs`, `src/ui.rs`, `src/tray_popup.rs`)
- **Language Model & Persistence (`Language` enum):** Added `Language::English` and `Language::Indonesian` with persisted `AppSettings::language` (`#[serde(default)]`) and localized `DownloadStatus::label_tr()` and `FileCategory::label_tr()` helpers.
- **Instant Toggle & Settings Selector:** Added a one-click **`EN` / `ID`** toggle pill in the top header bar and **Language / Bahasa** preset chips (`English` / `Bahasa Indonesia`) at the top of the **Settings** modal with live preview.
- **Full UI Coverage:** Localized header controls, sidebar navigation (`ANTREAN` / `KATEGORI` / `MESIN AKTIF`), KPI cards, clipboard banner, segmented workspace tabs (`Unduhan` / `Selesai`, `Jeda Semua`, `Lanjutkan Semua`, `Bersihkan Selesai`), sort/search/bulk bars, download cards, bottom inspector, status footer, confirmation modals, and the system tray popup flyout.

### Core HTTP/HTTPS Multi-Segment Engine (`src/engine.rs`, `src/throttle.rs`, `src/storage.rs`)
- **HTTP Probing & Filename Resolution:** Implemented `HEAD` probing with automatic `GET Range: bytes=0-0` fallback for `405 Method Not Allowed` / `501` / sneaky `200 OK` servers, RFC 5987 `Content-Disposition` decoding, and Windows disk space pre-checks (`GetDiskFreeSpaceExW`).
- **Dynamic In-Half Work Stealing & Adaptive Segmentation:** Adaptive segment sizing (`1–32` parallel byte-range connections) with IDM-style dynamic work stealing when active workers finish ahead of slower chunks.
- **Lock-Free Disk I/O & Integrity:** Added NTFS `.vdm.part` pre-allocation, parallel lock-free positional writes (`FileExt::seek_write`), `If-Range` (`ETag` / `Last-Modified`) resume safety, constant-memory streaming SHA-256 verification, and EWMA speed/ETA smoothing.
- **Global Bandwidth Limiter (`TokenBucketLimiter`):** Live rate-limiting (`Unlimited`, `512 KB/s`, `2 MB/s`, `5 MB/s`, `10 MB/s`, `25 MB/s`, or custom KB/s) shared cleanly across active HTTP segments.

### Native BitTorrent & Magnet Swarm Engine (`src/torrent.rs`, `src/engine.rs`)
- **Pure-Rust `librqbit 9.1` Stack:** Built native BitTorrent support (`default-tls`) with Kademlia DHT (BEP-5), HTTP/HTTPS & UDP Trackers (BEP-15), BEP-9 `ut_metadata` magnet resolution, uTP/TCP Peer Wire, automatic SHA-1 piece verification, fast-resume JSON persistence, and multi-file tree inspection.
- **Session Reliability & Swarm Auto-Recovery:**
  - Added serialized session initialization via `init_lock: AsyncMutex<()>` and automatic unpausing on `AlreadyManaged` duplicate-add responses.
  - Resolved torrents by ID with `info_hash` fallback (`resolve_tid`), metadata-guarded file deletion, and explicit non-blocking Tokio tasks for `pause`, `unpause`, and `delete`.
  - Fixed active-torrent Pause state flicker and deleted-torrent zombie re-adoption by tracking `deleting_tids`, `deleting_hashes`, `cancelled_item_ids`, and `pending_pause_tids` in `SharedState` across `TorrentManager::sync_stats`.
  - Added sticky-completed state protection (`completed_at.is_some()` / `status == DownloadStatus::Completed`) in `storage.rs` and `engine.rs` so finished torrents never regress into re-downloading on restart.
  - Added automatic swarm peer refresh (`RefreshTorrentPeers`) when stalled or collapsing (`test_swarm_auto_refresh_*`).

### 9Router-Inspired Dual-Theme Desktop UI (`src/ui.rs`, `src/theme.rs`, `src/icon.rs`)
- **Dual Theme Palette & Architectural Canvas:** Warm Charcoal Dark (`#1A1A1A` background, `#1F1F1E` panels, `#262626` surface) and Warm Paper Light (`#FCFBF9` background, `#F8FAFC` panels, `#FFFFFF` surface) paired with terracotta-coral accents (`#E56A4A`), vector icons, and pre-blended opaque `40px` architectural grid lines (`#241F1D` dark / `#F9F0EB` light) eliminating alpha-blending neon grid artifacts.
- **Responsive Main Workspace & Download Cards:**
  - Segmented `[Downloads] [Completed]` pill tab bar with live badges; scoped **`Pause All`** and **`Resume All`** controls directly to the right of the tab strip when viewing the **Downloads** tab, and **`Clear Completed`** on the **Completed** tab.
  - Cleaned top header bar (`render_header`) removing redundant `.torrent`, `Pause`, `Resume`, and `Clear` buttons in favor of streamlined navigation and primary `New Download` CTA.
  - Streamlined compact download rows without bulky category icon tiles, unbolded filenames (`body_font(12.5)`), dynamic window-width truncation with full-name hover tooltips, slim progress bars, and live HTTP/BitTorrent telemetry (`DL`, `UL`, peer `S/L` breakdown, `ETA`, `sockets`).
  - Multi-select workflow with a row-aligned tri-state **`Select All`** checkbox (`+10px` spacer matching card inner margin), keyboard shortcuts (`Ctrl+A`, `Esc`, `Delete`, `Shift+Delete`), and a contextual bulk action bar (`Pause`, `Resume`, `Delete (N)`).
- **Frameless Studio Modals (9Router Style):**
  - **New Download / Confirm Torrent Download:** Explicit confirmation dialog automatically opened when opening `.torrent` files, clicking `magnet:?` links, dragging `.torrent` files onto the window, submitting quick-URLs, or launching from a secondary process.
  - **Delete Download? / Delete N Downloads? (`render_delete_confirm_modal`):** Supports single and bulk deletion with an optional *"Also delete files from disk"* toggle (`Shift+Delete` pre-enables file deletion).
  - **Clear Completed Downloads? (`render_clear_completed_confirm_modal`):** Confirmation modal listing finished entries to be cleared from the list while preserving downloaded files on disk.
  - **Settings (`render_settings_modal`):** Custom coral sliders, pill toggle switches, theme switcher, global speed limit input/presets, and Windows logon autostart controls.
- **Fixed UI Regressions Resolved During Development:**
  - Fixed `egui 0.31` window sizing via `.min_width(w).max_width(w)` + explicit `ui.set_width(...)`.
  - Removed overlapping `frame_resp.response.interact(Sense::click())` in `render_download_card` so Pause/Delete/Inspect row buttons reliably receive clicks.
  - Passed explicit `RichText::new(...).color(p.text_subtle)` to `TextEdit::hint_text` so placeholders remain readable in Light Mode.

### Windows OS Integration, Single-Instance IPC & System Tray (`src/platform.rs`, `src/tray.rs`, `src/tray_popup.rs`, `src/storage.rs`)
- **Single-Instance & Atomic IPC Inbox:** Enforced single instance with `Local\VortexDM_SingleInstance_Mutex_v1`. Secondary launches forwarding `.torrent` paths, `magnet:?` URIs, or HTTP(S) URLs atomically enqueue `.req` files in `%APPDATA%\VortexDM\VortexDM\data\inbox\`, wake the running UI via `pending_ui_targets`, and restore the foreground window (`bring_to_front`).
- **Native Windows 11 Chrome & Icons (`platform.rs`, `build.rs`):**
  - Build script generates a multi-resolution (`16`, `32`, `48`, `256`) terracotta-coral `assets/vortexdm.ico` and embeds PE icon + `1.1.0.0` `VERSIONINFO` via `windres`.
  - Explicitly applies `WM_SETICON` (`ICON_BIG` / `ICON_SMALL`) and synchronizes the native Windows 11 DWM title bar via `DwmSetWindowAttribute` (`DWMWA_USE_IMMERSIVE_DARK_MODE`, `DWMWA_CAPTION_COLOR`, `DWMWA_BORDER_COLOR`, `DWMWA_TEXT_COLOR`).
- **System Tray & Frameless Flyout (`tray-icon 0.26`, `tray_popup.rs`):**
  - Fixed Windows 11 tray COM `E_FAIL` error by upgrading to `tray-icon = "0.26"`.
  - Pre-warmed frameless `276x354` always-on-top Tray Popup Flyout positioned dynamically above the Windows taskbar with live DL/UL readout, `+ New`, `Pause All`, `Resume All`, `Dark`/`Light` toggle, `Start with Windows` toggle, `Open Downloads`, and `Quit`.

### Installer & Verification (`installer/VortexDM.iss`, `build_installer.ps1`, `tests/engine_integration.rs`)
- **Inno Setup 6 Installer (`dist/VortexDM-Setup-1.1.0.exe`):**
  - Left *"Start VortexDM minimized in the Windows System Tray on logon"* **unchecked by default** (`Flags: unchecked` + `UsePreviousTasks=no`).
  - Registered HKCU `.torrent` (`VortexDM.Torrent`, `OpenWithProgids`, `Applications\vortexdm.exe`) and `magnet:` protocol associations with clean uninstall cleanup.
- **Automated Test Suite (`18/18` passing):**
  - **Unit tests (`10`):** HTTP `Content-Disposition` RFC 5987 & filename sanitization (`src/models.rs`), torrent/magnet detection (`src/models.rs`), token bucket throttling (`src/throttle.rs`), swarm auto-refresh & sticky-completed guards (`src/engine.rs`).
  - **Integration tests (`8` in `tests/engine_integration.rs`):** Multi-segment HTTP + SHA-256 verification, `HEAD 405` fallback + `200 OK` recovery, pause/resume byte integrity, `.torrent` add/pause/resume/remove lifecycle, IPC inbox `.torrent` auto-confirmation without zombie re-adoption, `RemoveMany` bulk delete, peer refresh without status flicker.
