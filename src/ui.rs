use crate::engine::{probe_url, DownloadEngineHandle, EngineSnapshot};
use crate::models::*;
use crate::platform;
use crate::theme::{
    apply_theme, body_font, bold_font, brand_cta_button, compact_status_pill, custom_checkbox,
    custom_slider_u32, danger_cta_button, install_windows_fonts, mono_font,
    paint_architectural_grid, paint_brand_badge, paint_modal_scrim, paint_vector_icon, preset_chip,
    secondary_button, square_icon_button, status_pill, styled_text_field, toggle_switch, Palette,
    VectorIcon,
};
use crate::tray::TrayController;
use crate::tray_popup::{
    popup_viewport_builder, popup_viewport_id, render_tray_popup, TrayPopupShared,
};
use egui::{
    Align, Align2, Color32, CornerRadius, Layout, Margin, Pos2, Rect, RichText, Sense, Shape,
    Stroke, StrokeKind, Vec2,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarFilter {
    All,
    Active,
    Queued,
    Paused,
    Completed,
    Failed,
    Category(FileCategory),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortBy {
    Newest,
    Name,
    Size,
    Speed,
    Progress,
}

impl SortBy {
    fn label(&self) -> &'static str {
        match self {
            SortBy::Newest => "Newest",
            SortBy::Name => "Name",
            SortBy::Size => "Size",
            SortBy::Speed => "Speed",
            SortBy::Progress => "Progress",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorTab {
    Segments,
    Details,
}

struct AddModalState {
    open: bool,
    url: String,
    custom_filename: String,
    save_dir: PathBuf,
    connections: usize,
    expected_sha256: String,
    probing: bool,
    probe_result: Option<Result<ProbeInfo, String>>,
    probe_rx: Option<std_mpsc::Receiver<Result<ProbeInfo, String>>>,
    error: Option<String>,
}

impl AddModalState {
    fn new(default_dir: PathBuf, default_conns: usize) -> Self {
        Self {
            open: false,
            url: String::new(),
            custom_filename: String::new(),
            save_dir: default_dir,
            connections: default_conns,
            expected_sha256: String::new(),
            probing: false,
            probe_result: None,
            probe_rx: None,
            error: None,
        }
    }
}

#[derive(Clone)]
struct DeleteConfirmItem {
    id: Uuid,
    filename: String,
    is_torrent: bool,
    is_completed: bool,
}

struct DeleteConfirmState {
    items: Vec<DeleteConfirmItem>,
    delete_files: bool,
}

pub struct VortexApp {
    runtime: Arc<tokio::runtime::Runtime>,
    engine: DownloadEngineHandle,
    tray: Option<TrayController>,
    tray_error: Option<String>,
    tray_rx: std_mpsc::Receiver<TrayAction>,
    popup_shared: Arc<TrayPopupShared>,
    hwnd_holder: Arc<AtomicIsize>,
    force_quit: Arc<AtomicBool>,
    start_minimized_pending: bool,
    startup_frames: u8,
    window_hidden: bool,
    applied_dark_mode: bool,

    // UI State
    filter: SidebarFilter,
    sort_by: SortBy,
    search_query: String,
    quick_url_input: String,
    selected_id: Option<Uuid>,
    checked_ids: HashSet<Uuid>,
    inspector_tab: InspectorTab,
    verify_hash_input: String,

    // Modals
    add_modal: AddModalState,
    settings_open: bool,
    settings_draft: AppSettings,
    speed_limit_kb_draft: String,
    delete_confirm: Option<DeleteConfirmState>,
    clear_completed_confirm: bool,
    pending_confirm_urls: VecDeque<String>,

    // Background helpers
    clipboard_last_seen: String,
    clipboard_banner_url: Option<String>,
    last_clipboard_poll: Instant,
    last_disk_poll: Instant,
    cached_disk_free: Option<u64>,
    last_tray_sync: Instant,
    toast_notice: Option<(String, Instant)>,
}

impl VortexApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        runtime: Arc<tokio::runtime::Runtime>,
        engine: DownloadEngineHandle,
        start_minimized: bool,
    ) -> Self {
        install_windows_fonts(&cc.egui_ctx);

        let snap = engine.get_snapshot();
        let initial_dark = snap.settings.dark_mode;
        apply_theme(&cc.egui_ctx, &Palette::for_mode(initial_dark));

        let hwnd_holder = Arc::new(AtomicIsize::new(0));
        let force_quit = Arc::new(AtomicBool::new(false));
        let (tray_tx, tray_rx) = std_mpsc::channel();

        let autostart_now = platform::is_windows_autostart_enabled();
        let popup_shared = Arc::new(TrayPopupShared {
            engine_cmd_tx: engine.cmd_tx.clone(),
            engine_snapshot: engine.snapshot.clone(),
            action_tx: Mutex::new(tray_tx),
            main_hwnd: hwnd_holder.clone(),
            popup_hwnd: Arc::new(AtomicIsize::new(0)),
            force_quit: force_quit.clone(),
            opened_at_ms: Arc::new(AtomicU64::new(0)),
            had_focus: AtomicBool::new(false),
            autostart_cached: AtomicBool::new(autostart_now),
        });

        let (tray, tray_error) = match TrayController::create(
            popup_shared.clone(),
            cc.egui_ctx.clone(),
        ) {
            Ok(t) => (Some(t), None),
            Err(e) => {
                eprintln!("[VortexDM] Tray init error: {}", e);
                (None, Some(e))
            }
        };

        let mut settings_draft = snap.settings.clone();
        settings_draft.start_with_windows = autostart_now;
        let speed_limit_kb_draft = if settings_draft.global_speed_limit_bps > 0 {
            (settings_draft.global_speed_limit_bps / 1024).to_string()
        } else {
            "0".to_string()
        };

        let cached_disk_free = platform::available_disk_space(&snap.settings.download_dir);

        Self {
            runtime,
            engine,
            tray,
            tray_error,
            tray_rx,
            popup_shared,
            hwnd_holder,
            force_quit,
            start_minimized_pending: start_minimized,
            startup_frames: 0,
            window_hidden: start_minimized,
            applied_dark_mode: initial_dark,
            filter: SidebarFilter::All,
            sort_by: SortBy::Newest,
            search_query: String::new(),
            quick_url_input: String::new(),
            selected_id: None,
            checked_ids: HashSet::new(),
            inspector_tab: InspectorTab::Segments,
            verify_hash_input: String::new(),
            add_modal: AddModalState::new(
                snap.settings.download_dir.clone(),
                snap.settings.default_connections,
            ),
            settings_open: false,
            settings_draft,
            speed_limit_kb_draft,
            delete_confirm: None,
            clear_completed_confirm: false,
            pending_confirm_urls: VecDeque::new(),
            clipboard_last_seen: String::new(),
            clipboard_banner_url: None,
            last_clipboard_poll: Instant::now(),
            last_disk_poll: Instant::now(),
            cached_disk_free,
            last_tray_sync: Instant::now(),
            toast_notice: None,
        }
    }

    fn toggle_theme(&mut self, ctx: &egui::Context, snap: &EngineSnapshot) {
        let mut updated = snap.settings.clone();
        updated.dark_mode = !updated.dark_mode;
        self.applied_dark_mode = updated.dark_mode;
        apply_theme(ctx, &Palette::for_mode(updated.dark_mode));
        platform::apply_window_caption_theme(
            self.hwnd_holder.load(Ordering::Relaxed),
            updated.dark_mode,
        );
        self.engine.send(EngineCommand::UpdateSettings(updated));
    }

    fn open_add_modal(&mut self, settings: &AppSettings, prefill_url: Option<String>) {
        self.settings_open = false;
        self.delete_confirm = None;
        self.clear_completed_confirm = false;
        self.add_modal =
            AddModalState::new(settings.download_dir.clone(), settings.default_connections);
        self.add_modal.open = true;
        if let Some(u) = prefill_url {
            self.add_modal.url = u;
        } else if let Ok(mut cb) = arboard::Clipboard::new() {
            if let Ok(txt) = cb.get_text() {
                let t = txt.trim();
                if t.starts_with("http://") || t.starts_with("https://") || is_torrent_input(t) {
                    self.add_modal.url = t.to_string();
                }
            }
        }
    }

    fn open_torrent_file_picker(&mut self, settings: &AppSettings) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("BitTorrent Files", &["torrent"])
            .pick_file()
        {
            self.open_add_modal(settings, Some(path.display().to_string()));
        }
    }

    fn submit_quick_url(&mut self, settings: &AppSettings) {
        let url = self.quick_url_input.trim().to_string();
        if is_torrent_input(&url) {
            self.quick_url_input.clear();
            self.open_add_modal(settings, Some(url));
        } else if url.starts_with("http://") || url.starts_with("https://") {
            self.engine.send(EngineCommand::Add(NewDownloadRequest {
                url,
                custom_filename: None,
                save_dir: None,
                max_connections: None,
                expected_sha256: None,
                start_paused: false,
            }));
            self.quick_url_input.clear();
            self.notify("Download queued");
        } else if !url.is_empty() {
            self.notify("Enter an http://, https://, magnet:? link, or .torrent path");
        }
    }

    fn start_probe(&mut self, egui_ctx: egui::Context) {
        let url = self.add_modal.url.trim().to_string();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            self.add_modal.error = Some("Enter a valid http:// or https:// URL".to_string());
            return;
        }
        self.add_modal.error = None;
        self.add_modal.probing = true;
        self.add_modal.probe_result = None;

        let (tx, rx) = std_mpsc::channel();
        self.add_modal.probe_rx = Some(rx);

        let client = self.engine.client.clone();
        self.runtime.spawn(async move {
            let res = probe_url(&client, &url).await;
            let _ = tx.send(res);
            egui_ctx.request_repaint();
        });
    }

    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        let hwnd = platform::resolve_hwnd(self.hwnd_holder.load(Ordering::Relaxed), WINDOW_TITLE);
        if hwnd != 0 {
            self.hwnd_holder.store(hwnd, Ordering::Relaxed);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        platform::hide_window_by_hwnd(hwnd);
        self.window_hidden = true;
    }

    fn notify(&mut self, msg: impl Into<String>) {
        self.toast_notice = Some((msg.into(), Instant::now()));
    }

    fn set_filter(&mut self, next: SidebarFilter) {
        if self.filter != next {
            self.filter = next;
            self.checked_ids.clear();
        }
    }

    fn matches_filter(&self, d: &DownloadItem) -> bool {
        match self.filter {
            SidebarFilter::All => d.status != DownloadStatus::Completed,
            SidebarFilter::Active => d.status.is_active(),
            SidebarFilter::Queued => d.status == DownloadStatus::Queued,
            SidebarFilter::Paused => d.status == DownloadStatus::Paused,
            SidebarFilter::Completed => d.status == DownloadStatus::Completed,
            SidebarFilter::Failed => {
                matches!(d.status, DownloadStatus::Failed | DownloadStatus::Cancelled)
            }
            SidebarFilter::Category(c) => d.category == c,
        }
    }

    fn capture_hwnd(&mut self, frame: &eframe::Frame) {
        if self.hwnd_holder.load(Ordering::Relaxed) == 0 {
            if let Ok(handle) = frame.window_handle() {
                if let RawWindowHandle::Win32(win32) = handle.as_raw() {
                    let raw = win32.hwnd.get();
                    self.hwnd_holder.store(raw, Ordering::SeqCst);
                    platform::apply_window_taskbar_icon(raw);
                    platform::apply_window_caption_theme(raw, self.applied_dark_mode);
                }
            }
        }
    }

    fn poll_tray_actions(&mut self, ctx: &egui::Context, snap: &EngineSnapshot) {
        while let Ok(action) = self.tray_rx.try_recv() {
            match action {
                TrayAction::ShowWindow => {
                    self.window_hidden = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayAction::ToggleWindow => {
                    self.window_hidden = !self.window_hidden;
                }
                TrayAction::NewDownload => {
                    self.window_hidden = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.open_add_modal(&snap.settings, None);
                }
                TrayAction::PauseAll => {
                    self.engine.send(EngineCommand::PauseAll);
                    self.notify("Paused all active downloads");
                }
                TrayAction::ResumeAll => {
                    self.engine.send(EngineCommand::ResumeAll);
                    self.notify("Resumed downloads");
                }
                TrayAction::SetSpeedLimit(bps) => {
                    self.engine.send(EngineCommand::SetGlobalSpeedLimit(bps));
                    if bps == 0 {
                        self.notify("Bandwidth limit: Unlimited");
                    } else {
                        self.notify(format!("Bandwidth capped at {}", format_speed(bps as f64)));
                    }
                }
                TrayAction::OpenDownloadsFolder => {
                    platform::open_folder(&snap.settings.download_dir);
                }
                TrayAction::Quit => {
                    self.force_quit.store(true, Ordering::SeqCst);
                    self.engine.send(EngineCommand::Shutdown);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn poll_background_helpers(&mut self, snap: &EngineSnapshot) {
        if self.last_tray_sync.elapsed() >= Duration::from_millis(400) {
            self.last_tray_sync = Instant::now();
            if let Some(tray) = &mut self.tray {
                tray.sync_status(&snap.stats);
            }
        }

        if self.last_disk_poll.elapsed() >= Duration::from_secs(5) {
            self.last_disk_poll = Instant::now();
            self.cached_disk_free = platform::available_disk_space(&snap.settings.download_dir);
        }

        if let Some(rx) = &self.add_modal.probe_rx {
            if let Ok(res) = rx.try_recv() {
                self.add_modal.probing = false;
                if let Ok(ref info) = res {
                    if self.add_modal.custom_filename.trim().is_empty() {
                        self.add_modal.custom_filename = info.filename.clone();
                    }
                    self.add_modal.connections = recommended_connections(
                        info.total_bytes,
                        info.supports_ranges,
                        snap.settings.default_connections,
                    );
                }
                self.add_modal.probe_result = Some(res);
                self.add_modal.probe_rx = None;
            }
        }

        if snap.settings.clipboard_monitoring
            && self.last_clipboard_poll.elapsed() >= Duration::from_millis(1200)
        {
            self.last_clipboard_poll = Instant::now();
            if let Ok(mut cb) = arboard::Clipboard::new() {
                if let Ok(txt) = cb.get_text() {
                    let trimmed = txt.trim().to_string();
                    if !trimmed.is_empty() && trimmed != self.clipboard_last_seen {
                        self.clipboard_last_seen = trimmed.clone();
                        if is_likely_download_url(&trimmed) {
                            let already_queued = snap.downloads.iter().any(|d| d.url == trimmed);
                            if !already_queued {
                                self.clipboard_banner_url = Some(trimmed);
                            }
                        }
                    }
                }
            }
        }
    }
}

impl eframe::App for VortexApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        // Match active theme background so neither main window nor tray popup flashes black edges
        egui::Rgba::from(visuals.panel_fill).to_array()
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.capture_hwnd(frame);
        self.startup_frames = self.startup_frames.saturating_add(1);

        // Mount our pre-warmed deferred Tray Popup viewport (stays hidden at 0% CPU until clicked)
        let popup_shared = self.popup_shared.clone();
        ctx.show_viewport_deferred(
            popup_viewport_id(),
            popup_viewport_builder(),
            move |popup_ctx, _class| {
                render_tray_popup(popup_ctx, &popup_shared);
            },
        );
        if self.popup_shared.popup_hwnd.load(Ordering::Relaxed) == 0 {
            let ph = platform::resolve_hwnd(0, TRAY_POPUP_TITLE);
            if ph != 0 {
                self.popup_shared.popup_hwnd.store(ph, Ordering::SeqCst);
            }
        }

        if self.start_minimized_pending {
            if self.startup_frames >= 2 && self.hwnd_holder.load(Ordering::Relaxed) != 0 {
                self.start_minimized_pending = false;
                self.hide_to_tray(ctx);
                return;
            } else {
                ctx.request_repaint();
            }
        }

        let snap = self.engine.get_snapshot();

        // Keep active egui theme + Windows OS caption bar synchronized with settings
        if snap.settings.dark_mode != self.applied_dark_mode {
            self.applied_dark_mode = snap.settings.dark_mode;
            apply_theme(ctx, &Palette::for_mode(self.applied_dark_mode));
            platform::apply_window_caption_theme(
                self.hwnd_holder.load(Ordering::Relaxed),
                self.applied_dark_mode,
            );
        }
        let p = Palette::for_mode(self.applied_dark_mode);

        self.poll_tray_actions(ctx, &snap);
        self.poll_background_helpers(&snap);

        // Collect any incoming `.torrent` / `magnet:?` / URL targets from CLI or secondary IPC launches
        for incoming in self.engine.take_pending_ui_targets() {
            self.pending_confirm_urls.push_back(incoming);
        }

        // Bring window forward if a secondary launch forwarded a `.torrent` / `magnet:` / URL via IPC
        if self.engine.take_bring_to_front() {
            self.window_hidden = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            let hwnd = platform::resolve_hwnd(self.hwnd_holder.load(Ordering::Relaxed), WINDOW_TITLE);
            platform::show_window_by_hwnd(hwnd);
        }

        // Native Drag-and-Drop support for `.torrent` files dropped onto VortexDM -> opens confirmation modal
        let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped_files {
            if let Some(path) = file.path {
                if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
                {
                    self.pending_confirm_urls
                        .push_back(path.display().to_string());
                }
            }
        }

        // Pop the next queued torrent / link into the confirmation dialog when no modal is active
        if !self.add_modal.open
            && !self.settings_open
            && self.delete_confirm.is_none()
            && !self.clear_completed_confirm
        {
            if let Some(next_target) = self.pending_confirm_urls.pop_front() {
                self.open_add_modal(&snap.settings, Some(next_target));
            }
        }

        // Close-to-tray interception
        if ctx.input(|i| i.viewport().close_requested()) {
            let should_quit = self.force_quit.load(Ordering::SeqCst)
                || !snap.settings.close_to_tray
                || self.tray.is_none();
            if should_quit {
                self.engine.send(EngineCommand::Shutdown);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_to_tray(ctx);
                return;
            }
        }

        // Ctrl+N opens New Download modal
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::N)) && !self.add_modal.open {
            self.open_add_modal(&snap.settings, None);
        }

        let repaint_ms = if snap.stats.active_count > 0 { 200 } else { 800 };
        ctx.request_repaint_after(Duration::from_millis(repaint_ms));

        // --- 1. TOP STICKY-STYLE HEADER BAR (9Router Aesthetic) ---
        egui::TopBottomPanel::top("top_header")
            .frame(
                egui::Frame::new()
                    .fill(p.bg_alt)
                    .inner_margin(Margin::symmetric(14, 8))
                    .stroke(Stroke::new(1.0_f32, p.border)),
            )
            .show(ctx, |ui| {
                self.render_header(ui, &snap, &p);
            });

        // --- 2. BOTTOM STATUS FOOTER ---
        egui::TopBottomPanel::bottom("bottom_footer")
            .frame(
                egui::Frame::new()
                    .fill(p.bg_alt)
                    .inner_margin(Margin::symmetric(14, 5))
                    .stroke(Stroke::new(1.0_f32, p.border)),
            )
            .show(ctx, |ui| {
                self.render_footer(ui, &snap, &p);
            });

        // --- 3. LEFT NAVIGATION RAIL ---
        egui::SidePanel::left("nav_sidebar")
            .resizable(false)
            .exact_width(196.0)
            .frame(
                egui::Frame::new()
                    .fill(p.bg_alt)
                    .inner_margin(Margin::symmetric(10, 10))
                    .stroke(Stroke::new(1.0_f32, p.border)),
            )
            .show(ctx, |ui| {
                self.render_sidebar(ui, &snap, &p);
            });

        // --- 4. BOTTOM TERMINAL-STYLE INSPECTOR ---
        if let Some(sel_id) = self.selected_id {
            if let Some(item) = snap.downloads.iter().find(|d| d.id == sel_id).cloned() {
                egui::TopBottomPanel::bottom("terminal_inspector")
                    .resizable(true)
                    .default_height(205.0)
                    .min_height(155.0)
                    .max_height(340.0)
                    .frame(
                        egui::Frame::new()
                            .fill(p.bg_alt)
                            .inner_margin(Margin::symmetric(14, 10))
                            .stroke(Stroke::new(1.0_f32, p.border)),
                    )
                    .show(ctx, |ui| {
                        self.render_inspector(ui, &item, &p);
                    });
            } else {
                self.selected_id = None;
            }
        }

        // --- 5. CENTRAL WORKSPACE WITH 40PX ARCHITECTURAL GRID ---
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(14, 10)),
            )
            .show(ctx, |ui| {
                let full_rect = ui.max_rect().expand(24.0);
                paint_architectural_grid(ui.painter(), full_rect, &p);

                self.render_clipboard_banner(ui, &snap, &p);
                self.render_kpi_strip(ui, &snap, &p);
                ui.add_space(8.0);
                self.render_main_area(ui, &snap, &p);
            });

        // --- 6. MODALS ---
        if self.add_modal.open {
            self.render_add_modal(ctx, &snap, &p);
        }
        if self.settings_open {
            self.render_settings_modal(ctx, &p);
        }
        if self.delete_confirm.is_some() {
            self.render_delete_confirm_modal(ctx, &p);
        }
        if self.clear_completed_confirm {
            self.render_clear_completed_confirm_modal(ctx, &snap, &p);
        }
    }
}

impl VortexApp {
    fn render_header(&mut self, ui: &mut egui::Ui, snap: &EngineSnapshot, p: &Palette) {
        let compact_hdr = ui.available_width() < 860.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = if compact_hdr { 4.0 } else { 6.0 };
            // Left: 9Router-style terracotta brand badge + bold wordmark
            let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
            paint_brand_badge(ui.painter(), logo_rect, p);
            ui.add_space(2.0);

            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    ui.label(
                        RichText::new("VortexDM")
                            .font(bold_font(15.0))
                            .color(p.text_main),
                    );
                    status_pill(ui, &format!("v{}", APP_VERSION), p.brand, p);
                });
                ui.label(
                    RichText::new("HTTP & BitTorrent Engine")
                        .font(body_font(10.0))
                        .color(p.text_muted),
                );
            });

            // Right side: CTA button + utility icons + Theme toggle + Speed limit
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                if brand_cta_button(
                    ui,
                    if compact_hdr { "New" } else { "New Download" },
                    Some(VectorIcon::Plus),
                    p,
                )
                    .on_hover_text("Add a new download or magnet link (Ctrl+N)")
                    .clicked()
                {
                    self.open_add_modal(&snap.settings, None);
                }

                if self.tray.is_some()
                    && square_icon_button(
                        ui,
                        VectorIcon::Tray,
                        "Minimize to Windows System Tray",
                        p,
                        29.0,
                    )
                    .clicked()
                {
                    self.hide_to_tray(ui.ctx());
                }

                if square_icon_button(ui, VectorIcon::Gear, "Settings", p, 29.0).clicked() {
                    self.add_modal.open = false;
                    self.delete_confirm = None;
                    self.clear_completed_confirm = false;
                    self.settings_draft = snap.settings.clone();
                    self.settings_draft.start_with_windows =
                        platform::is_windows_autostart_enabled();
                    self.speed_limit_kb_draft = if self.settings_draft.global_speed_limit_bps > 0 {
                        (self.settings_draft.global_speed_limit_bps / 1024).to_string()
                    } else {
                        "0".to_string()
                    };
                    self.settings_open = true;
                }

                // Signature 9Router Dark/Light Theme Toggle Button
                let theme_icon = if p.dark {
                    VectorIcon::Sun
                } else {
                    VectorIcon::Moon
                };
                let theme_tip = if p.dark {
                    "Switch to Light Mode (Warm Paper)"
                } else {
                    "Switch to Dark Mode (Warm Charcoal)"
                };
                if square_icon_button(ui, theme_icon, theme_tip, p, 29.0).clicked() {
                    self.toggle_theme(ui.ctx(), snap);
                }

                // Bandwidth cap selector
                let cur_limit = snap.settings.global_speed_limit_bps;
                let limit_label = if cur_limit == 0 {
                    if compact_hdr {
                        "Cap: Unlim".to_string()
                    } else {
                        "Cap: Unlimited".to_string()
                    }
                } else {
                    format!("Cap: {}", format_speed(cur_limit as f64))
                };

                egui::ComboBox::from_id_salt("header_speed_limit")
                    .selected_text(
                        RichText::new(limit_label)
                            .font(mono_font(11.0))
                            .color(if cur_limit == 0 { p.text_muted } else { p.brand }),
                    )
                    .width(if compact_hdr { 86.0 } else { 112.0 })
                    .show_ui(ui, |ui| {
                        let presets: &[(&str, u64)] = &[
                            ("Unlimited", 0),
                            ("512 KB/s", 512 * 1024),
                            ("2 MB/s", 2 * 1024 * 1024),
                            ("5 MB/s", 5 * 1024 * 1024),
                            ("10 MB/s", 10 * 1024 * 1024),
                            ("25 MB/s", 25 * 1024 * 1024),
                        ];
                        for &(lbl, bps) in presets {
                            if ui.selectable_label(cur_limit == bps, lbl).clicked() {
                                self.engine.send(EngineCommand::SetGlobalSpeedLimit(bps));
                            }
                        }
                    });
            });
        });
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui, snap: &EngineSnapshot, p: &Palette) {
        let bottom_card_h = 122.0;
        let scroll_h = (ui.available_height() - bottom_card_h - 8.0).max(100.0);

        egui::ScrollArea::vertical()
            .id_salt("sidebar_nav_scroll")
            .max_height(scroll_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(
                    RichText::new("QUEUE")
                        .font(bold_font(10.0))
                        .color(p.text_subtle),
                );
                ui.add_space(2.0);

                let in_progress_count = snap
                    .downloads
                    .iter()
                    .filter(|d| d.status != DownloadStatus::Completed)
                    .count();

                sidebar_nav_row(
                    ui,
                    "Downloads",
                    in_progress_count,
                    p.brand,
                    self.filter == SidebarFilter::All,
                    p,
                    || self.set_filter(SidebarFilter::All),
                );
                sidebar_nav_row(
                    ui,
                    "Active",
                    snap.stats.active_count,
                    p.brand,
                    self.filter == SidebarFilter::Active,
                    p,
                    || self.set_filter(SidebarFilter::Active),
                );
                sidebar_nav_row(
                    ui,
                    "Queued",
                    snap.stats.queued_count,
                    p.info,
                    self.filter == SidebarFilter::Queued,
                    p,
                    || self.set_filter(SidebarFilter::Queued),
                );
                sidebar_nav_row(
                    ui,
                    "Paused",
                    snap.stats.paused_count,
                    p.warning,
                    self.filter == SidebarFilter::Paused,
                    p,
                    || self.set_filter(SidebarFilter::Paused),
                );
                sidebar_nav_row(
                    ui,
                    "Completed",
                    snap.stats.completed_count,
                    p.success,
                    self.filter == SidebarFilter::Completed,
                    p,
                    || self.set_filter(SidebarFilter::Completed),
                );
                sidebar_nav_row(
                    ui,
                    "Failed / Stopped",
                    snap.stats.failed_count,
                    p.danger,
                    self.filter == SidebarFilter::Failed,
                    p,
                    || self.set_filter(SidebarFilter::Failed),
                );

                ui.add_space(12.0);
                ui.label(
                    RichText::new("CATEGORIES")
                        .font(bold_font(10.0))
                        .color(p.text_subtle),
                );
                ui.add_space(2.0);

                for cat in FileCategory::ALL {
                    let count = snap.downloads.iter().filter(|d| d.category == cat).count();
                    sidebar_nav_row(
                        ui,
                        cat.label(),
                        count,
                        p.category_color(cat),
                        self.filter == SidebarFilter::Category(cat),
                        p,
                        || self.set_filter(SidebarFilter::Category(cat)),
                    );
                }
            });

        // Bottom Live Throughput Card (explicitly top-down inside bottom-pinned slot!)
        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            egui::Frame::new()
                .fill(p.surface)
                .corner_radius(CornerRadius::same(10))
                .stroke(Stroke::new(1.0_f32, p.border))
                .inner_margin(Margin::same(9))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("LIVE ENGINE")
                                    .font(bold_font(9.5))
                                    .color(p.text_subtle),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!("{} sockets", snap.stats.total_connections))
                                        .font(mono_font(10.0))
                                        .color(p.brand),
                                );
                            });
                        });

                        ui.label(
                            RichText::new(format_speed(snap.stats.total_speed_bps))
                                .font(bold_font(16.0))
                                .color(if snap.stats.total_speed_bps > 1.0 {
                                    p.brand
                                } else {
                                    p.text_main
                                }),
                        );

                        paint_sparkline(ui, &snap.stats.speed_history, 36.0, p.brand, p);

                        ui.label(
                            RichText::new(format!(
                                "Session: {}",
                                format_bytes(snap.stats.session_downloaded_bytes)
                            ))
                            .font(mono_font(10.0))
                            .color(p.text_muted),
                        );
                    });
                });
        });
    }

    fn render_kpi_strip(&mut self, ui: &mut egui::Ui, snap: &EngineSnapshot, p: &Palette) {
        let free_str = self
            .cached_disk_free
            .map(format_bytes)
            .unwrap_or_else(|| "—".to_string());

        ui.columns(4, |cols| {
            kpi_card(
                &mut cols[0],
                "LIVE THROUGHPUT",
                &format_speed(snap.stats.total_speed_bps),
                if snap.settings.global_speed_limit_bps > 0 {
                    format!(
                        "Capped at {}",
                        format_speed(snap.settings.global_speed_limit_bps as f64)
                    )
                } else {
                    "EWMA • Unlimited".to_string()
                },
                p.brand,
                p,
            );
            kpi_card(
                &mut cols[1],
                "ACTIVE TRANSFERS",
                &format!("{} active", snap.stats.active_count),
                format!(
                    "{} queued • {} parallel sockets",
                    snap.stats.queued_count, snap.stats.total_connections
                ),
                p.info,
                p,
            );
            kpi_card(
                &mut cols[2],
                "SESSION VOLUME",
                &format_bytes(snap.stats.session_downloaded_bytes),
                format!("{} completed total", snap.stats.completed_count),
                p.success,
                p,
            );
            kpi_card(
                &mut cols[3],
                "DISK HEADROOM",
                &free_str,
                snap.settings
                    .download_dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| format!("Target: {}", s))
                    .unwrap_or_else(|| "Download drive".to_string()),
                p.purple,
                p,
            );
        });
    }

    fn render_clipboard_banner(&mut self, ui: &mut egui::Ui, snap: &EngineSnapshot, p: &Palette) {
        if let Some(url) = self.clipboard_banner_url.clone() {
            let is_tor = is_torrent_input(&url);
            let banner_col = if is_tor { p.teal } else { p.brand };
            egui::Frame::new()
                .fill(Palette::alpha(banner_col, if p.dark { 26 } else { 18 }))
                .stroke(Stroke::new(1.0_f32, Palette::alpha(banner_col, 140)))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::symmetric(14, 9))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (ic_r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        paint_vector_icon(
                            ui.painter(),
                            ic_r.center(),
                            6.5,
                            if is_tor {
                                VectorIcon::Magnet
                            } else {
                                VectorIcon::Link
                            },
                            banner_col,
                        );

                        ui.label(
                            RichText::new(if is_tor {
                                "Torrent link in clipboard"
                            } else {
                                "Link in clipboard"
                            })
                            .font(bold_font(12.2))
                            .color(banner_col),
                        );

                        let preview_w = (ui.available_width() - 285.0).max(60.0);
                        ui.allocate_ui_with_layout(
                            Vec2::new(preview_w, 18.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.set_max_width(preview_w);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&url)
                                            .font(mono_font(11.2))
                                            .color(p.text_main),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&url);
                            },
                        );

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if secondary_button(ui, "Dismiss", None, p).clicked() {
                                self.clipboard_banner_url = None;
                            }
                            if !is_tor && secondary_button(ui, "Configure", None, p).clicked() {
                                self.clipboard_banner_url = None;
                                self.open_add_modal(&snap.settings, Some(url.clone()));
                            }
                            let cta_lbl = if is_tor {
                                "Review & Download"
                            } else {
                                "Download Now"
                            };
                            if brand_cta_button(ui, cta_lbl, Some(VectorIcon::Plus), p).clicked() {
                                self.clipboard_banner_url = None;
                                if is_tor {
                                    self.open_add_modal(&snap.settings, Some(url.clone()));
                                } else {
                                    self.engine.send(EngineCommand::Add(NewDownloadRequest {
                                        url: url.clone(),
                                        custom_filename: None,
                                        save_dir: None,
                                        max_connections: None,
                                        expected_sha256: None,
                                        start_paused: false,
                                    }));
                                }
                            }
                        });
                    });
                });
            ui.add_space(10.0);
        }
    }

    fn render_main_area(&mut self, ui: &mut egui::Ui, snap: &EngineSnapshot, p: &Palette) {
        if snap.downloads.is_empty() {
            self.checked_ids.clear();
            self.render_empty_hero(ui, snap, p);
            return;
        }

        let in_progress_count = snap
            .downloads
            .iter()
            .filter(|d| d.status != DownloadStatus::Completed)
            .count();
        let completed_count = snap.stats.completed_count;
        let is_completed_tab = self.filter == SidebarFilter::Completed;
        let is_downloads_tab = matches!(
            self.filter,
            SidebarFilter::All
                | SidebarFilter::Active
                | SidebarFilter::Queued
                | SidebarFilter::Paused
                | SidebarFilter::Failed
        );

        // Segmented top tab strip: Downloads vs Completed (+ sub-filter chip & Clear Completed action)
        let tab_bar_w = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new(tab_bar_w, 33.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(tab_bar_w);
                ui.set_max_width(tab_bar_w);
                ui.spacing_mut().item_spacing.x = 8.0;

                egui::Frame::new()
                    .fill(p.bg_alt)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(9))
                    .inner_margin(Margin::same(3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            if tab_segment(
                                ui,
                                "Downloads",
                                in_progress_count,
                                p.brand,
                                is_downloads_tab,
                                p,
                            )
                            .on_hover_text("Show active, queued, paused, and failed downloads")
                            .clicked()
                            {
                                self.set_filter(SidebarFilter::All);
                            }

                            if tab_segment(
                                ui,
                                "Completed",
                                completed_count,
                                p.success,
                                is_completed_tab,
                                p,
                            )
                            .on_hover_text("Show finished downloads")
                            .clicked()
                            {
                                self.set_filter(SidebarFilter::Completed);
                            }
                        });
                    });

                if is_downloads_tab {
                    if secondary_button(ui, "Pause All", Some(VectorIcon::Pause), p)
                        .on_hover_text("Pause all active downloads")
                        .clicked()
                    {
                        self.engine.send(EngineCommand::PauseAll);
                        self.notify("Paused all active downloads");
                    }
                    if secondary_button(ui, "Resume All", Some(VectorIcon::Play), p)
                        .on_hover_text("Resume all paused downloads")
                        .clicked()
                    {
                        self.engine.send(EngineCommand::ResumeAll);
                        self.notify("Resumed downloads");
                    }
                }

                // Subtle dismissible chip when viewing a sidebar sub-filter or category
                let sub_filter_label: Option<String> = match self.filter {
                    SidebarFilter::Active => Some("Active ×".to_string()),
                    SidebarFilter::Queued => Some("Queued ×".to_string()),
                    SidebarFilter::Paused => Some("Paused ×".to_string()),
                    SidebarFilter::Failed => Some("Failed / Stopped ×".to_string()),
                    SidebarFilter::Category(cat) => Some(format!("{} ×", cat.label())),
                    SidebarFilter::All | SidebarFilter::Completed => None,
                };
                if let Some(lbl) = sub_filter_label {
                    if preset_chip(ui, &lbl, true, p)
                        .on_hover_text("Reset filter to all in-progress downloads")
                        .clicked()
                    {
                        self.set_filter(SidebarFilter::All);
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if is_completed_tab && completed_count > 0 {
                        if secondary_button(ui, "Clear Completed", Some(VectorIcon::Clear), p)
                            .on_hover_text(
                                "Remove all completed downloads from list (confirmation required)",
                            )
                            .clicked()
                        {
                            self.add_modal.open = false;
                            self.settings_open = false;
                            self.delete_confirm = None;
                            self.clear_completed_confirm = true;
                        }
                    }
                });
            },
        );

        ui.add_space(6.0);

        // Prune checked IDs that were removed from engine or moved out of the current tab/filter
        let active_filter = self.filter;
        self.checked_ids.retain(|id| {
            snap.downloads.iter().any(|d| {
                d.id == *id
                    && match active_filter {
                        SidebarFilter::All => d.status != DownloadStatus::Completed,
                        SidebarFilter::Active => d.status.is_active(),
                        SidebarFilter::Queued => d.status == DownloadStatus::Queued,
                        SidebarFilter::Paused => d.status == DownloadStatus::Paused,
                        SidebarFilter::Completed => d.status == DownloadStatus::Completed,
                        SidebarFilter::Failed => {
                            matches!(d.status, DownloadStatus::Failed | DownloadStatus::Cancelled)
                        }
                        SidebarFilter::Category(c) => d.category == c,
                    }
            })
        });

        let query = self.search_query.trim().to_ascii_lowercase();
        let mut filtered: Vec<&DownloadItem> = snap
            .downloads
            .iter()
            .filter(|d| self.matches_filter(d))
            .filter(|d| {
                query.is_empty()
                    || d.filename.to_ascii_lowercase().contains(&query)
                    || d.url.to_ascii_lowercase().contains(&query)
            })
            .collect();

        match self.sort_by {
            SortBy::Newest => filtered.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
            SortBy::Name => {
                filtered.sort_by(|a, b| a.filename.to_lowercase().cmp(&b.filename.to_lowercase()))
            }
            SortBy::Size => filtered.sort_by(|a, b| {
                b.total_bytes
                    .unwrap_or(b.downloaded_bytes)
                    .cmp(&a.total_bytes.unwrap_or(a.downloaded_bytes))
            }),
            SortBy::Speed => filtered.sort_by(|a, b| {
                b.speed_bps
                    .partial_cmp(&a.speed_bps)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            SortBy::Progress => filtered.sort_by(|a, b| {
                b.progress()
                    .partial_cmp(&a.progress())
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
        }

        let no_modal_open = !self.add_modal.open
            && !self.settings_open
            && self.delete_confirm.is_none()
            && !self.clear_completed_confirm;
        let wants_kb = ui.ctx().wants_keyboard_input();

        // Power-user keyboard shortcuts for multi-select (Ctrl+A, Esc, Delete)
        if no_modal_open && !wants_kb {
            if ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::A)) && !filtered.is_empty()
            {
                for d in &filtered {
                    self.checked_ids.insert(d.id);
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !self.checked_ids.is_empty() {
                self.checked_ids.clear();
            }
            if ui.input(|i| i.key_pressed(egui::Key::Delete)) && !self.checked_ids.is_empty() {
                let items: Vec<DeleteConfirmItem> = snap
                    .downloads
                    .iter()
                    .filter(|d| self.checked_ids.contains(&d.id))
                    .map(|d| DeleteConfirmItem {
                        id: d.id,
                        filename: d.filename.clone(),
                        is_torrent: d.is_torrent(),
                        is_completed: d.status == DownloadStatus::Completed,
                    })
                    .collect();
                if !items.is_empty() {
                    self.clear_completed_confirm = false;
                    self.delete_confirm = Some(DeleteConfirmState {
                        items,
                        delete_files: ui.input(|i| i.modifiers.shift),
                    });
                }
            }
        }

        let filtered_count = filtered.len();
        let checked_in_filtered = filtered
            .iter()
            .filter(|d| self.checked_ids.contains(&d.id))
            .count();
        let all_selected = filtered_count > 0 && checked_in_filtered == filtered_count;
        let some_selected = checked_in_filtered > 0 && !all_selected;

        // Toolbar row: Tri-state Select All (pixel-aligned with row checkboxes at +11px) + Search + Sort
        let toolbar_w = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new(toolbar_w, 28.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(toolbar_w);
                ui.set_max_width(toolbar_w);
                ui.spacing_mut().item_spacing.x = 10.0;

                // Match the download card's 11px left inner margin so Select All aligns cleanly with row checkboxes
                ui.add_space(10.0);

                let sel_resp = custom_checkbox(ui, all_selected, some_selected, p).on_hover_text(
                    if all_selected {
                        "Deselect all visible downloads"
                    } else {
                        "Select all visible downloads (Ctrl+A)"
                    },
                );
                if sel_resp.clicked() {
                    if all_selected {
                        for d in &filtered {
                            self.checked_ids.remove(&d.id);
                        }
                    } else {
                        for d in &filtered {
                            self.checked_ids.insert(d.id);
                        }
                    }
                }

                let search_w = (ui.available_width() - 185.0).clamp(140.0, 320.0);
                let search_hint = if self.filter == SidebarFilter::Completed {
                    "Filter completed downloads by filename or URL..."
                } else {
                    "Filter downloads by filename or URL..."
                };
                // Search input card
                egui::Frame::new()
                    .fill(p.surface)
                    .corner_radius(CornerRadius::same(9))
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .inner_margin(Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.search_query)
                                    .frame(false)
                                    .hint_text(RichText::new(search_hint).color(p.text_subtle))
                                    .desired_width(search_w),
                            );
                            if !self.search_query.is_empty() && ui.small_button("Clear").clicked() {
                                self.search_query.clear();
                            }
                        });
                    });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("sort_downloads")
                        .selected_text(
                            RichText::new(format!("Sort: {}", self.sort_by.label()))
                                .font(body_font(12.5))
                                .color(p.text_main),
                        )
                        .width(125.0)
                        .show_ui(ui, |ui| {
                            for opt in [
                                SortBy::Newest,
                                SortBy::Name,
                                SortBy::Size,
                                SortBy::Speed,
                                SortBy::Progress,
                            ] {
                                ui.selectable_value(&mut self.sort_by, opt, opt.label());
                            }
                        });
                });
            },
        );

        // Contextual Bulk Action Bar (appears cleanly whenever 1+ downloads are checked)
        let total_checked = self.checked_ids.len();
        if total_checked > 0 {
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(Palette::alpha(p.brand, if p.dark { 24 } else { 15 }))
                .stroke(Stroke::new(1.0_f32, Palette::alpha(p.brand, 115)))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        status_pill(ui, &format!("{} SELECTED", total_checked), p.brand, p);

                        if !all_selected && filtered_count > 0 {
                            if preset_chip(
                                ui,
                                &format!("Select all ({})", filtered_count),
                                false,
                                p,
                            )
                            .clicked()
                            {
                                for d in &filtered {
                                    self.checked_ids.insert(d.id);
                                }
                            }
                        }

                        if preset_chip(ui, "Clear", false, p).clicked() {
                            self.checked_ids.clear();
                        }

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if danger_cta_button(
                                ui,
                                &format!("Delete ({})", total_checked),
                                Some(VectorIcon::Trash),
                                p,
                            )
                            .on_hover_text("Delete all selected downloads (confirmation required)")
                            .clicked()
                            {
                                let items: Vec<DeleteConfirmItem> = snap
                                    .downloads
                                    .iter()
                                    .filter(|d| self.checked_ids.contains(&d.id))
                                    .map(|d| DeleteConfirmItem {
                                        id: d.id,
                                        filename: d.filename.clone(),
                                        is_torrent: d.is_torrent(),
                                        is_completed: d.status == DownloadStatus::Completed,
                                    })
                                    .collect();
                                if !items.is_empty() {
                                    self.add_modal.open = false;
                                    self.settings_open = false;
                                    self.clear_completed_confirm = false;
                                    self.delete_confirm = Some(DeleteConfirmState {
                                        items,
                                        delete_files: ui.input(|i| i.modifiers.shift),
                                    });
                                }
                            }

                            if secondary_button(ui, "Resume", Some(VectorIcon::Play), p)
                                .on_hover_text("Resume all selected paused downloads")
                                .clicked()
                            {
                                for id in self.checked_ids.iter().copied().collect::<Vec<_>>() {
                                    self.engine.send(EngineCommand::Resume(id));
                                }
                                self.notify(format!("Resumed {} selected", total_checked));
                            }

                            if secondary_button(ui, "Pause", Some(VectorIcon::Pause), p)
                                .on_hover_text("Pause all selected active downloads")
                                .clicked()
                            {
                                for id in self.checked_ids.iter().copied().collect::<Vec<_>>() {
                                    self.engine.send(EngineCommand::Pause(id));
                                }
                                self.notify(format!("Paused {} selected", total_checked));
                            }
                        });
                    });
                });
        }

        ui.add_space(6.0);

        if filtered.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(46.0);
                    if !query.is_empty() {
                        ui.label(
                            RichText::new("No matching downloads")
                                .font(bold_font(16.0))
                                .color(p.text_main),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("Try clearing your search query or switching tabs.")
                                .font(body_font(12.0))
                                .color(p.text_muted),
                        );
                        ui.add_space(10.0);
                        if secondary_button(ui, "Clear Search", None, p).clicked() {
                            self.search_query.clear();
                        }
                    } else if self.filter == SidebarFilter::All {
                        ui.label(
                            RichText::new("All downloads completed!")
                                .font(bold_font(16.0))
                                .color(p.text_main),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("Everything in your queue has finished downloading.")
                                .font(body_font(12.0))
                                .color(p.text_muted),
                        );
                        if completed_count > 0 {
                            ui.add_space(10.0);
                            if secondary_button(
                                ui,
                                &format!("View Completed ({})", completed_count),
                                Some(VectorIcon::Check),
                                p,
                            )
                            .clicked()
                            {
                                self.set_filter(SidebarFilter::Completed);
                            }
                        }
                    } else if self.filter == SidebarFilter::Completed {
                        ui.label(
                            RichText::new("No completed downloads yet")
                                .font(bold_font(16.0))
                                .color(p.text_main),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("Finished transfers will appear here automatically.")
                                .font(body_font(12.0))
                                .color(p.text_muted),
                        );
                        if in_progress_count > 0 {
                            ui.add_space(10.0);
                            if secondary_button(
                                ui,
                                &format!("View Downloads ({})", in_progress_count),
                                None,
                                p,
                            )
                            .clicked()
                            {
                                self.set_filter(SidebarFilter::All);
                            }
                        }
                    } else {
                        let empty_title = match self.filter {
                            SidebarFilter::Active => "No active downloads".to_string(),
                            SidebarFilter::Queued => "No queued downloads".to_string(),
                            SidebarFilter::Paused => "No paused downloads".to_string(),
                            SidebarFilter::Failed => "No failed or stopped downloads".to_string(),
                            SidebarFilter::Category(cat) => {
                                format!("No downloads in {}", cat.label())
                            }
                            SidebarFilter::All | SidebarFilter::Completed => {
                                "No matching downloads".to_string()
                            }
                        };
                        ui.label(
                            RichText::new(empty_title)
                                .font(bold_font(16.0))
                                .color(p.text_main),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("Switch back to all downloads or start a new transfer.")
                                .font(body_font(12.0))
                                .color(p.text_muted),
                        );
                        ui.add_space(10.0);
                        if secondary_button(ui, "Show All Downloads", None, p).clicked() {
                            self.set_filter(SidebarFilter::All);
                        }
                    }
                });
            });
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for item in filtered {
                    self.render_download_card(ui, item, p);
                }
            });
    }

    /// 9Router-inspired architectural Hero Empty State with instant Quick-URL input & 3 numbered cards.
    fn render_empty_hero(&mut self, ui: &mut egui::Ui, snap: &EngineSnapshot, p: &Palette) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let avail_w = ui.available_width();
            let content_w = avail_w.min(820.0);
            let side_pad = ((avail_w - content_w) * 0.5).max(0.0);

            ui.horizontal(|ui| {
                ui.add_space(side_pad);
                ui.allocate_ui(Vec2::new(content_w, ui.available_height()), |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);

                        // Compact, snug translucent coral eyebrow pill (exact 9Router hero pill)
                        let pill_text = "Multi-Segmented HTTP  •  BitTorrent DHT & Magnet  •  SHA-256";
                        let pill_galley = ui.painter().layout_no_wrap(
                            pill_text.to_string(),
                            bold_font(10.0),
                            p.brand,
                        );
                        let (pill_rect, _) = ui.allocate_exact_size(
                            Vec2::new(pill_galley.size().x + 22.0, 22.0),
                            Sense::hover(),
                        );
                        ui.painter().rect_filled(
                            pill_rect,
                            CornerRadius::same(99),
                            Palette::alpha(p.brand, if p.dark { 26 } else { 18 }),
                        );
                        ui.painter().rect_stroke(
                            pill_rect,
                            CornerRadius::same(99),
                            Stroke::new(1.0_f32, Palette::alpha(p.brand, 92)),
                            StrokeKind::Inside,
                        );
                        ui.painter().text(
                            pill_rect.center(),
                            Align2::CENTER_CENTER,
                            pill_text,
                            bold_font(10.0),
                            p.brand,
                        );

                        ui.add_space(10.0);

                        // Two-tone editorial headline (exact 9Router "One Router. 60+ AI Providers." feel)
                        ui.label(
                            RichText::new("One Manager.")
                                .font(bold_font(28.0))
                                .color(p.text_main),
                        );
                        ui.label(
                            RichText::new("Maximum Speed. Zero Bottlenecks.")
                                .font(bold_font(24.0))
                                .color(p.brand),
                        );

                        ui.add_space(5.0);
                        ui.label(
                            RichText::new(
                                "Paste an HTTP/HTTPS URL or magnet:? link below, or open a .torrent file to accelerate downloads.",
                            )
                            .font(body_font(12.5))
                            .color(p.text_muted),
                        );

                        ui.add_space(14.0);

                        // Inline Quick-Add URL / Magnet Card (perfectly centered and vertically aligned)
                        let bar_w = content_w.min(660.0);
                        let bar_pad = ((content_w - bar_w) * 0.5).max(0.0);
                        ui.horizontal(|ui| {
                            ui.add_space(bar_pad);
                            ui.allocate_ui(Vec2::new(bar_w, 48.0), |ui| {
                                egui::Frame::new()
                                    .fill(p.surface)
                                    .corner_radius(CornerRadius::same(12))
                                    .stroke(Stroke::new(1.0_f32, p.border))
                                    .inner_margin(Margin::symmetric(10, 8))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let input_w = (ui.available_width() - 286.0).max(140.0);
                                            egui::Frame::new()
                                                .fill(p.bg_alt)
                                                .corner_radius(CornerRadius::same(7))
                                                .stroke(Stroke::new(1.0_f32, p.border))
                                                .inner_margin(Margin::symmetric(8, 6))
                                                .show(ui, |ui| {
                                                    let resp = ui.add(
                                                        egui::TextEdit::singleline(&mut self.quick_url_input)
                                                            .frame(false)
                                                            .font(mono_font(12.0))
                                                            .hint_text(
                                                                RichText::new("https://... or magnet:?xt=urn:btih:...")
                                                                    .font(mono_font(11.5))
                                                                    .color(p.text_subtle),
                                                            )
                                                            .desired_width(input_w),
                                                    );
                                                    if resp.lost_focus()
                                                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                                    {
                                                        self.submit_quick_url(&snap.settings);
                                                    }
                                                });

                                            if secondary_button(ui, "Paste", None, p).clicked() {
                                                if let Ok(mut cb) = arboard::Clipboard::new() {
                                                    if let Ok(txt) = cb.get_text() {
                                                        self.quick_url_input = txt.trim().to_string();
                                                    }
                                                }
                                            }

                                            if secondary_button(ui, ".torrent", Some(VectorIcon::Magnet), p)
                                                .on_hover_text("Open a local .torrent file")
                                                .clicked()
                                            {
                                                self.open_torrent_file_picker(&snap.settings);
                                            }

                                            if brand_cta_button(ui, "Start", Some(VectorIcon::Plus), p).clicked() {
                                                if self.quick_url_input.trim().is_empty() {
                                                    self.open_add_modal(&snap.settings, None);
                                                } else {
                                                    self.submit_quick_url(&snap.settings);
                                                }
                                            }
                                        });
                                    });
                            });
                        });

                        ui.add_space(16.0);

                        // 3-Column Feature Cards with 9Router numbered circular coral step badges
                        ui.columns(3, |cols| {
                            hero_feature_card(
                                &mut cols[0],
                                "1",
                                "Dynamic HTTP Segments",
                                "Splits transfers up to 32 parallel byte-range streams and steals work from slow chunks.",
                                "If-Range + SHA-256",
                                p,
                            );
                            hero_feature_card(
                                &mut cols[1],
                                "2",
                                "BitTorrent & Magnet",
                                "Native Rust swarm engine with Kademlia DHT, HTTP/UDP trackers, uTP, and fast-resume.",
                                "magnet:? + .torrent",
                                p,
                            );
                            hero_feature_card(
                                &mut cols[2],
                                "3",
                                "Native Tray Flyout",
                                "Runs silently in your Windows tray with live popup controls and instant bandwidth caps.",
                                "vortexdm --minimized",
                                p,
                            );
                        });
                    });
                });
            });
        });
    }

    fn render_download_card(&mut self, ui: &mut egui::Ui, item: &DownloadItem, p: &Palette) {
        let is_selected = self.selected_id == Some(item.id);
        let is_checked = self.checked_ids.contains(&item.id);
        let accent = p.status_color(item.status);
        let mut any_action_interacted = false;
        let card_outer_w = ui.available_width().max(260.0);

        let card_fill = if is_checked {
            if p.dark {
                Color32::from_rgb(44, 34, 31)
            } else {
                Color32::from_rgb(255, 248, 245)
            }
        } else {
            p.surface
        };
        let card_stroke = if is_selected || is_checked {
            Stroke::new(1.4_f32, p.brand)
        } else {
            Stroke::new(1.0_f32, p.border)
        };

        // Pre-allocate card background interaction cleanly (never steal clicks from inner buttons!)
        let frame_resp = egui::Frame::new()
            .fill(card_fill)
            .corner_radius(CornerRadius::same(10))
            .stroke(card_stroke)
            .inner_margin(Margin::symmetric(10, 4))
            .show(ui, |ui| {
                let content_w = (card_outer_w - 20.0).max(230.0);
                ui.set_min_width(content_w);
                ui.set_max_width(content_w);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;

                    // Multi-select checkbox (17x17, optically centered vertically in the compact row)
                    let mut cb_hovered = false;
                    let mut cb_clicked = false;
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add_space(9.0);
                        let cb_resp = custom_checkbox(ui, is_checked, false, p).on_hover_text(
                            if is_checked {
                                "Deselect download"
                            } else {
                                "Select for bulk delete / pause / resume"
                            },
                        );
                        cb_hovered = cb_resp.hovered();
                        cb_clicked = cb_resp.clicked();
                    });
                    if cb_hovered {
                        any_action_interacted = true;
                    }
                    if cb_clicked {
                        any_action_interacted = true;
                        if is_checked {
                            self.checked_ids.remove(&item.id);
                        } else {
                            self.checked_ids.insert(item.id);
                        }
                    }

                    // Main info column strictly bounded to remaining card width so long filenames never overflow
                    let info_w = (content_w - 27.0).max(180.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(info_w, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_min_width(info_w);
                            ui.set_max_width(info_w);
                            ui.spacing_mut().item_spacing.y = 1.0;

                            // Build status/metadata pills upfront so we can size filename accurately
                            let mut pills: Vec<(String, Color32)> = Vec::with_capacity(4);
                            pills.push((item.status.label().to_uppercase(), accent));
                            if let Some(tor) = &item.torrent {
                                pills.push(("TORRENT".to_string(), p.teal));
                                if tor.seeding {
                                    pills.push(("SEEDING".to_string(), p.success));
                                }
                            } else if item.supports_ranges {
                                pills.push((
                                    format!(
                                        "{}X SEG",
                                        item.segments.len().max(item.max_connections)
                                    ),
                                    p.info,
                                ));
                            }
                            if let Some(ok) = item.sha256_verified {
                                if ok {
                                    pills.push(("SHA-256 OK".to_string(), p.success));
                                } else {
                                    pills.push(("HASH MISMATCH".to_string(), p.danger));
                                }
                            }

                            let btn_size = 20.0_f32;
                            let btn_gap = 4.0_f32;
                            let btn_count: usize = if item.status == DownloadStatus::Completed {
                                let seeding_btn =
                                    usize::from(item.torrent.as_ref().is_some_and(|t| t.seeding));
                                let open_btn = usize::from(!item.is_torrent());
                                3 + seeding_btn + open_btn
                            } else {
                                3
                            };
                            let actions_w = (btn_count as f32) * btn_size
                                + ((btn_count.saturating_sub(1)) as f32) * btn_gap
                                + 8.0;
                            let pills_w: f32 = pills
                                .iter()
                                .map(|(lbl, col)| {
                                    ui.painter()
                                        .layout_no_wrap(lbl.clone(), bold_font(9.5), *col)
                                        .size()
                                        .x
                                        + 11.0
                                        + btn_gap
                                })
                                .sum();

                            // Top row: responsive auto-truncating filename + compact pills + right-aligned action buttons
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = btn_gap;
                                ui.set_min_width(info_w);
                                ui.set_max_width(info_w);

                                let max_name_w = (info_w - pills_w - actions_w - 4.0).max(70.0);
                                let natural_name_w = ui
                                    .painter()
                                    .layout_no_wrap(
                                        item.filename.clone(),
                                        body_font(12.5),
                                        p.text_main,
                                    )
                                    .size()
                                    .x;
                                let name_w = natural_name_w.min(max_name_w);
                                let is_truncated = natural_name_w > max_name_w + 1.0;

                                ui.allocate_ui_with_layout(
                                    Vec2::new(name_w, 20.0),
                                    Layout::left_to_right(Align::Center),
                                    |ui| {
                                        ui.set_min_width(name_w);
                                        ui.set_max_width(name_w);
                                        let mut name_resp = ui.add(
                                            egui::Label::new(
                                                RichText::new(&item.filename)
                                                    .font(body_font(12.5))
                                                    .color(p.text_main),
                                            )
                                            .truncate()
                                            .sense(Sense::click()),
                                        );
                                        if is_truncated {
                                            name_resp = name_resp.on_hover_text(&item.filename);
                                        }
                                        if name_resp.hovered() {
                                            any_action_interacted = true;
                                        }
                                        if name_resp.clicked() {
                                            any_action_interacted = true;
                                            self.selected_id = Some(item.id);
                                            self.verify_hash_input =
                                                item.expected_sha256.clone().unwrap_or_default();
                                        }
                                    },
                                );

                                for (lbl, col) in &pills {
                                    compact_status_pill(ui, lbl, *col, p);
                                }

                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.spacing_mut().item_spacing.x = btn_gap;
                                    let trash_resp = square_icon_button(
                                        ui,
                                        VectorIcon::Trash,
                                        "Delete download...",
                                        p,
                                        btn_size,
                                    );
                                    if trash_resp.hovered() {
                                        any_action_interacted = true;
                                    }
                                    if trash_resp.clicked() {
                                        any_action_interacted = true;
                                        self.add_modal.open = false;
                                        self.settings_open = false;
                                        self.clear_completed_confirm = false;
                                        self.delete_confirm = Some(DeleteConfirmState {
                                            items: vec![DeleteConfirmItem {
                                                id: item.id,
                                                filename: item.filename.clone(),
                                                is_torrent: item.is_torrent(),
                                                is_completed: item.status
                                                    == DownloadStatus::Completed,
                                            }],
                                            delete_files: ui.input(|i| i.modifiers.shift),
                                        });
                                    }

                                    let inspect_resp = square_icon_button(
                                        ui,
                                        VectorIcon::Inspect,
                                        if is_selected {
                                            "Hide Inspector"
                                        } else {
                                            "Inspect Segments & Details"
                                        },
                                        p,
                                        btn_size,
                                    );
                                    if inspect_resp.hovered() {
                                        any_action_interacted = true;
                                    }
                                    if inspect_resp.clicked() {
                                        any_action_interacted = true;
                                        if is_selected {
                                            self.selected_id = None;
                                        } else {
                                            self.selected_id = Some(item.id);
                                            self.verify_hash_input =
                                                item.expected_sha256.clone().unwrap_or_default();
                                        }
                                    }

                                    if item.status == DownloadStatus::Completed {
                                        if item.torrent.as_ref().is_some_and(|t| t.seeding) {
                                            let r = square_icon_button(
                                                ui,
                                                VectorIcon::Pause,
                                                "Pause seeding",
                                                p,
                                                btn_size,
                                            );
                                            if r.hovered() {
                                                any_action_interacted = true;
                                            }
                                            if r.clicked() {
                                                any_action_interacted = true;
                                                self.engine.send(EngineCommand::Pause(item.id));
                                            }
                                        }
                                        let folder_r = square_icon_button(
                                            ui,
                                            VectorIcon::Folder,
                                            "Reveal in Windows Explorer",
                                            p,
                                            btn_size,
                                        );
                                        if folder_r.hovered() {
                                            any_action_interacted = true;
                                        }
                                        if folder_r.clicked() {
                                            any_action_interacted = true;
                                            if item.is_torrent() {
                                                platform::open_folder(&item.save_dir);
                                            } else {
                                                platform::reveal_in_explorer(&item.target_path());
                                            }
                                        }
                                        if !item.is_torrent() {
                                            let open_r = square_icon_button(
                                                ui,
                                                VectorIcon::OpenExternal,
                                                "Open downloaded file",
                                                p,
                                                btn_size,
                                            );
                                            if open_r.hovered() {
                                                any_action_interacted = true;
                                            }
                                            if open_r.clicked() {
                                                any_action_interacted = true;
                                                platform::open_file(&item.target_path());
                                            }
                                        }
                                    } else if matches!(
                                        item.status,
                                        DownloadStatus::Downloading
                                            | DownloadStatus::Probing
                                            | DownloadStatus::Queued
                                    ) {
                                        let pause_r = square_icon_button(
                                            ui,
                                            VectorIcon::Pause,
                                            "Pause download",
                                            p,
                                            btn_size,
                                        );
                                        if pause_r.hovered() {
                                            any_action_interacted = true;
                                        }
                                        if pause_r.clicked() {
                                            any_action_interacted = true;
                                            self.engine.send(EngineCommand::Pause(item.id));
                                            self.notify("Download paused");
                                        }
                                    } else if matches!(
                                        item.status,
                                        DownloadStatus::Paused
                                            | DownloadStatus::Failed
                                            | DownloadStatus::Cancelled
                                    ) {
                                        let play_r = square_icon_button(
                                            ui,
                                            VectorIcon::Play,
                                            "Resume download",
                                            p,
                                            btn_size,
                                        );
                                        if play_r.hovered() {
                                            any_action_interacted = true;
                                        }
                                        if play_r.clicked() {
                                            any_action_interacted = true;
                                            self.engine.send(EngineCommand::Resume(item.id));
                                            self.notify("Download resumed");
                                        }
                                    }
                                });
                            });

                            let progress = item.progress();

                            // Second line: inline slim progress bar + monospace telemetry (wraps on narrow windows)
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                let bar_w = (info_w * 0.16).clamp(64.0, 128.0);
                                let (bar_slot, _) =
                                    ui.allocate_exact_size(Vec2::new(bar_w, 13.0), Sense::hover());
                                let bar_rect = Rect::from_center_size(
                                    bar_slot.center(),
                                    Vec2::new(bar_w, 4.0),
                                );
                                ui.painter()
                                    .rect_filled(bar_rect, CornerRadius::same(99), p.surface_2);
                                if progress > 0.0 {
                                    let filled = Rect::from_min_size(
                                        bar_rect.min,
                                        Vec2::new(
                                            (bar_rect.width() * progress).max(4.0),
                                            bar_rect.height(),
                                        ),
                                    );
                                    ui.painter()
                                        .rect_filled(filled, CornerRadius::same(99), accent);
                                }
                                ui.add_space(2.0);
                                let sep = || RichText::new("•").font(body_font(10.5)).color(p.border);
                                let size_text = match item.total_bytes {
                                    Some(total) => format!(
                                        "{} / {} ({:.1}%)",
                                        format_bytes(item.downloaded_bytes),
                                        format_bytes(total),
                                        progress * 100.0
                                    ),
                                    None => {
                                        format!(
                                            "{} / Unknown size",
                                            format_bytes(item.downloaded_bytes)
                                        )
                                    }
                                };
                                ui.label(
                                    RichText::new(size_text)
                                        .font(mono_font(10.5))
                                        .color(p.text_muted),
                                );

                                if let Some(tor) = &item.torrent {
                                    if item.status == DownloadStatus::Downloading || tor.seeding {
                                        let leechers = tor.peers_leechers;
                                        ui.label(sep());
                                        ui.label(
                                            RichText::new(format!(
                                                "DL {}",
                                                format_speed(item.speed_bps)
                                            ))
                                            .font(mono_font(10.5))
                                            .color(p.brand),
                                        );
                                        ui.label(sep());
                                        ui.label(
                                            RichText::new(format!(
                                                "UL {}",
                                                format_speed(tor.upload_speed_bps)
                                            ))
                                            .font(mono_font(10.5))
                                            .color(p.teal),
                                        );
                                        ui.label(sep());
                                        ui.label(
                                            RichText::new(format!(
                                                "{} peers ({}S/{}L)",
                                                tor.peers_live, tor.peers_seeders, leechers
                                            ))
                                            .font(mono_font(10.5))
                                            .color(p.info),
                                        );
                                        if item.status == DownloadStatus::Downloading {
                                            ui.label(sep());
                                            ui.label(
                                                RichText::new(format!(
                                                    "ETA {}",
                                                    format_eta(item.eta_seconds)
                                                ))
                                                .font(mono_font(10.5))
                                                .color(p.text_main),
                                            );
                                        }
                                    } else if let Some(err) = &item.error_message {
                                        ui.label(sep());
                                        ui.label(
                                            RichText::new(err)
                                                .font(body_font(10.8))
                                                .color(p.danger),
                                        );
                                    } else if item.status == DownloadStatus::Probing {
                                        ui.label(sep());
                                        ui.label(
                                            RichText::new(
                                                "Discovering DHT/tracker peers & fetching metadata...",
                                            )
                                            .font(mono_font(10.8))
                                            .color(p.teal),
                                        );
                                    } else if item.status == DownloadStatus::Completed {
                                        if let Some(done_at) = item.completed_at {
                                            ui.label(sep());
                                            ui.label(
                                                RichText::new(format!(
                                                    "Finished {}",
                                                    done_at
                                                        .with_timezone(&chrono::Local)
                                                        .format("%H:%M:%S")
                                                ))
                                                .font(mono_font(10.8))
                                                .color(p.success),
                                            );
                                        }
                                    }
                                } else if item.status == DownloadStatus::Downloading {
                                    ui.label(sep());
                                    ui.label(
                                        RichText::new(format_speed(item.speed_bps))
                                            .font(mono_font(10.8))
                                            .color(p.brand),
                                    );
                                    ui.label(sep());
                                    ui.label(
                                        RichText::new(format!(
                                            "ETA {}",
                                            format_eta(item.eta_seconds)
                                        ))
                                        .font(mono_font(10.8))
                                        .color(p.text_main),
                                    );
                                    ui.label(sep());
                                    ui.label(
                                        RichText::new(format!(
                                            "{} sockets",
                                            item.active_connections
                                        ))
                                        .font(mono_font(10.8))
                                        .color(p.info),
                                    );
                                } else if let Some(err) = &item.error_message {
                                    ui.label(sep());
                                    ui.label(
                                        RichText::new(err)
                                            .font(body_font(10.8))
                                            .color(p.danger),
                                    );
                                } else if item.status == DownloadStatus::Completed {
                                    if let Some(done_at) = item.completed_at {
                                        ui.label(sep());
                                        ui.label(
                                            RichText::new(format!(
                                                "Finished {}",
                                                done_at
                                                    .with_timezone(&chrono::Local)
                                                    .format("%H:%M:%S")
                                            ))
                                            .font(mono_font(10.8))
                                            .color(p.success),
                                        );
                                    }
                                }
                            });
                        },
                    );
                });
            });

        // Subtle warm coral border highlight on card hover (matching 9Router .card-hover)
        if frame_resp.response.hovered() && !is_selected {
            ui.painter().rect_stroke(
                frame_resp.response.rect,
                CornerRadius::same(10),
                Stroke::new(1.0_f32, Palette::alpha(p.brand, 130)),
                StrokeKind::Inside,
            );
        }

        // Select card when clicking empty card background, WITHOUT ever intercepting inner action buttons!
        if !any_action_interacted
            && !self.add_modal.open
            && !self.settings_open
            && self.delete_confirm.is_none()
            && frame_resp.response.hovered()
            && ui.input(|i| i.pointer.primary_clicked())
        {
            self.selected_id = Some(item.id);
            self.verify_hash_input = item.expected_sha256.clone().unwrap_or_default();
        }
    }

    /// 9Router macOS/Terminal-style bottom Inspector with 3 colored dots, monospace header, and code chips.
    fn render_inspector(&mut self, ui: &mut egui::Ui, item: &DownloadItem, p: &Palette) {
        egui::Frame::new()
            .fill(p.surface)
            .corner_radius(CornerRadius::same(12))
            .stroke(Stroke::new(1.0_f32, p.border))
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // Terminal-style titlebar (exact 9Router terminal window motif — responsive to window width)
                ui.horizontal(|ui| {
                    let bar_w = ui.available_width();
                    let (dots_rect, _) = ui.allocate_exact_size(Vec2::new(46.0, 14.0), Sense::hover());
                    let cy = dots_rect.center().y;
                    ui.painter().circle_filled(Pos2::new(dots_rect.min.x + 6.0, cy), 4.5, p.danger);
                    ui.painter().circle_filled(Pos2::new(dots_rect.min.x + 21.0, cy), 4.5, p.warning);
                    ui.painter().circle_filled(Pos2::new(dots_rect.min.x + 36.0, cy), 4.5, p.success);

                    let title_str = format!("inspector — {}", item.filename);
                    let max_title_w = (bar_w - 460.0).max(90.0);
                    let nat_title_w = ui
                        .painter()
                        .layout_no_wrap(title_str.clone(), mono_font(12.0), p.text_muted)
                        .size()
                        .x;
                    let title_w = nat_title_w.min(max_title_w);
                    ui.allocate_ui_with_layout(
                        Vec2::new(title_w, 18.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.set_min_width(title_w);
                            ui.set_max_width(title_w);
                            let mut lbl = ui.add(
                                egui::Label::new(
                                    RichText::new(&title_str)
                                        .font(mono_font(12.0))
                                        .color(p.text_muted),
                                )
                                .truncate(),
                            );
                            if nat_title_w > max_title_w + 1.0 {
                                lbl = lbl.on_hover_text(&item.filename);
                            }
                            let _ = lbl;
                        },
                    );

                    ui.add_space(10.0);
                    ui.selectable_value(
                        &mut self.inspector_tab,
                        InspectorTab::Segments,
                        if item.is_torrent() {
                            "Swarm & Files"
                        } else {
                            "Segments & Speed"
                        },
                    );
                    ui.selectable_value(
                        &mut self.inspector_tab,
                        InspectorTab::Details,
                        if item.is_torrent() {
                            "Infohash & Details"
                        } else {
                            "Metadata & SHA-256"
                        },
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Close").clicked() {
                            self.selected_id = None;
                        }
                        if ui
                            .small_button("Delete + Files")
                            .on_hover_text("Stop transfer, remove from list, AND delete files from disk")
                            .clicked()
                        {
                            self.add_modal.open = false;
                            self.settings_open = false;
                            self.delete_confirm = Some(DeleteConfirmState {
                                items: vec![DeleteConfirmItem {
                                    id: item.id,
                                    filename: item.filename.clone(),
                                    is_torrent: item.is_torrent(),
                                    is_completed: item.status == DownloadStatus::Completed,
                                }],
                                delete_files: true,
                            });
                        }
                    });
                });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(8.0);

                match self.inspector_tab {
                    InspectorTab::Segments => {
                        if let Some(tor) = &item.torrent {
                            ui.columns(2, |cols| {
                                cols[0].horizontal(|ui| {
                                    ui.label(
                                        RichText::new("BITTORRENT SWARM & FILES")
                                            .font(bold_font(10.5))
                                            .color(p.text_subtle),
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if item.status == DownloadStatus::Downloading {
                                            let refresh_tip = match tor.last_swarm_refresh_at {
                                                Some(ts) => format!(
                                                    "Auto-refresh active (last refreshed {}) — click to refresh peers/trackers now",
                                                    ts.with_timezone(&chrono::Local).format("%H:%M:%S")
                                                ),
                                                None => "Auto-refresh active — click to refresh peers/trackers now".to_string(),
                                            };
                                            if preset_chip(ui, "Refresh Peers", false, p)
                                                .on_hover_text(refresh_tip)
                                                .clicked()
                                            {
                                                self.engine.send(EngineCommand::RefreshTorrentPeers(item.id));
                                                self.notify("Refreshing torrent peers & trackers...");
                                            }
                                        }
                                        ui.label(
                                            RichText::new(format!(
                                                "{} live ({}S/{}L, {} uTP) • {} seen • UL {}",
                                                tor.peers_live,
                                                tor.peers_seeders,
                                                tor.peers_leechers,
                                                tor.peers_utp,
                                                tor.peers_seen,
                                                format_bytes(tor.uploaded_bytes)
                                            ))
                                            .font(mono_font(10.5))
                                            .color(p.teal),
                                        );
                                    });
                                });
                                cols[0].add_space(6.0);

                                egui::ScrollArea::vertical()
                                    .max_height(124.0)
                                    .show(&mut cols[0], |ui| {
                                        if tor.files.is_empty() {
                                            ui.label(
                                                RichText::new(
                                                    "Resolving torrent file tree from peers via BEP-9 ut_metadata...",
                                                )
                                                .font(mono_font(11.0))
                                                .color(p.text_muted),
                                            );
                                        } else {
                                            egui::Grid::new("tor_files_grid")
                                                .striped(true)
                                                .spacing([12.0, 4.0])
                                                .show(ui, |ui| {
                                                    ui.label(RichText::new("FILE").font(bold_font(10.0)).color(p.text_subtle));
                                                    ui.label(RichText::new("SIZE").font(bold_font(10.0)).color(p.text_subtle));
                                                    ui.label(RichText::new("DONE").font(bold_font(10.0)).color(p.text_subtle));
                                                    ui.end_row();
                                                    let file_col_w = (ui.available_width() - 120.0).max(110.0);
                                                    for f in &tor.files {
                                                        ui.allocate_ui_with_layout(
                                                            Vec2::new(file_col_w, 16.0),
                                                            Layout::left_to_right(Align::Center),
                                                            |ui| {
                                                                ui.set_max_width(file_col_w);
                                                                ui.add(
                                                                    egui::Label::new(
                                                                        RichText::new(&f.relative_path)
                                                                            .font(mono_font(11.0))
                                                                            .color(p.text_main),
                                                                    )
                                                                    .truncate(),
                                                                )
                                                                .on_hover_text(&f.relative_path);
                                                            },
                                                        );
                                                        ui.label(
                                                            RichText::new(format_bytes(f.length))
                                                                .font(mono_font(11.0))
                                                                .color(p.text_muted),
                                                        );
                                                        ui.label(
                                                            RichText::new(format!("{:.0}%", f.progress() * 100.0))
                                                                .font(mono_font(11.0))
                                                                .color(if f.progress() >= 0.999 { p.success } else { p.teal }),
                                                        );
                                                        ui.end_row();
                                                    }
                                                });
                                        }
                                    });

                                cols[1].horizontal(|ui| {
                                    ui.label(
                                        RichText::new("SWARM THROUGHPUT")
                                            .font(bold_font(10.5))
                                            .color(p.text_subtle),
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(format!("UL {}", format_speed(tor.upload_speed_bps)))
                                                .font(mono_font(11.0))
                                                .color(p.teal),
                                        );
                                        ui.label(
                                            RichText::new(format!("DL {}", format_speed(item.speed_bps)))
                                                .font(mono_font(11.0))
                                                .color(p.brand),
                                        );
                                    });
                                });
                                cols[1].add_space(5.0);
                                paint_sparkline(&mut cols[1], &item.speed_history, 114.0, p.teal, p);
                            });
                        } else {
                        ui.columns(2, |cols| {
                            cols[0].horizontal(|ui| {
                                ui.label(
                                    RichText::new("DYNAMIC SEGMENTATION MAP")
                                        .font(bold_font(10.5))
                                        .color(p.text_subtle),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(
                                        RichText::new(format!("{} segments", item.segments.len()))
                                            .font(mono_font(11.0))
                                            .color(p.brand),
                                    );
                                });
                            });
                            cols[0].add_space(5.0);
                            paint_segment_map(&mut cols[0], item, p);
                            cols[0].add_space(8.0);

                            egui::ScrollArea::vertical()
                                .max_height(98.0)
                                .show(&mut cols[0], |ui| {
                                    if item.segments.is_empty() {
                                        ui.label(
                                            RichText::new(if item.supports_ranges {
                                                "Waiting to partition dynamic byte ranges..."
                                            } else {
                                                "Single-stream transfer (server does not expose byte ranges)"
                                            })
                                            .font(mono_font(11.0))
                                            .color(p.text_muted),
                                        );
                                    } else {
                                        egui::Grid::new("seg_grid_9r")
                                            .striped(true)
                                            .spacing([14.0, 4.0])
                                            .show(ui, |ui| {
                                                ui.label(RichText::new("SEG").font(bold_font(10.0)).color(p.text_subtle));
                                                ui.label(RichText::new("BYTE RANGE").font(bold_font(10.0)).color(p.text_subtle));
                                                ui.label(RichText::new("DONE").font(bold_font(10.0)).color(p.text_subtle));
                                                ui.label(RichText::new("STATE").font(bold_font(10.0)).color(p.text_subtle));
                                                ui.end_row();

                                                for seg in &item.segments {
                                                    ui.label(RichText::new(format!("#{}", seg.id + 1)).font(mono_font(11.0)));
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "{} – {}",
                                                            format_bytes(seg.start),
                                                            format_bytes(seg.end)
                                                        ))
                                                        .font(mono_font(11.0))
                                                        .color(p.text_muted),
                                                    );
                                                    ui.label(
                                                        RichText::new(format!("{:.0}%", seg.progress() * 100.0))
                                                            .font(mono_font(11.0))
                                                            .color(p.brand),
                                                    );
                                                    let st_col = match seg.status {
                                                        SegmentStatus::Completed => p.success,
                                                        SegmentStatus::Active => p.brand,
                                                        SegmentStatus::Idle => p.warning,
                                                        SegmentStatus::Failed => p.danger,
                                                    };
                                                    ui.label(
                                                        RichText::new(format!("{:?}", seg.status))
                                                            .font(mono_font(11.0))
                                                            .color(st_col),
                                                    );
                                                    ui.end_row();
                                                }
                                            });
                                    }
                                });

                            // Right column: Live EWMA sparkline
                            cols[1].horizontal(|ui| {
                                ui.label(
                                    RichText::new("EWMA SPEED TELEMETRY")
                                        .font(bold_font(10.5))
                                        .color(p.text_subtle),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(
                                        RichText::new(format!("Peak {}", format_speed(item.peak_speed_bps)))
                                            .font(mono_font(11.0))
                                            .color(p.success),
                                    );
                                    ui.label(
                                        RichText::new(format!("Now {}", format_speed(item.speed_bps)))
                                            .font(mono_font(11.0))
                                            .color(p.brand),
                                    );
                                });
                            });
                            cols[1].add_space(5.0);
                            paint_sparkline(&mut cols[1], &item.speed_history, 114.0, p.brand, p);
                        });
                        }
                    }
                    InspectorTab::Details => {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            egui::Grid::new("details_grid_9r")
                                .num_columns(2)
                                .spacing([14.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label(RichText::new(if item.is_torrent() { "Magnet / Source" } else { "Source URL" }).font(bold_font(11.5)).color(p.text_muted));
                                    ui.horizontal(|ui| {
                                        let short_u = if item.url.len() > 82 {
                                            format!("{}...", &item.url[..82])
                                        } else {
                                            item.url.clone()
                                        };
                                        code_chip(ui, &short_u, p.info, p);
                                        if ui.small_button("Copy").clicked() {
                                            if let Ok(mut cb) = arboard::Clipboard::new() {
                                                let _ = cb.set_text(item.url.clone());
                                            }
                                        }
                                    });
                                    ui.end_row();

                                    ui.label(RichText::new("Save Path").font(bold_font(11.5)).color(p.text_muted));
                                    ui.horizontal(|ui| {
                                        let p_display = if item.is_torrent() {
                                            item.save_dir.display().to_string()
                                        } else {
                                            item.target_path().display().to_string()
                                        };
                                        code_chip(ui, &p_display, p.text_main, p);
                                        if ui.small_button("Reveal").clicked() {
                                            if item.is_torrent() {
                                                platform::open_folder(&item.save_dir);
                                            } else {
                                                platform::reveal_in_explorer(&item.target_path());
                                            }
                                        }
                                    });
                                    ui.end_row();

                                    if let Some(tor) = &item.torrent {
                                        ui.label(RichText::new("Infohash (SHA-1)").font(bold_font(11.5)).color(p.text_muted));
                                        ui.horizontal(|ui| {
                                            let hash = tor.info_hash.as_deref().unwrap_or("Resolving...");
                                            code_chip(ui, hash, p.teal, p);
                                            if let Some(h) = &tor.info_hash {
                                                if ui.small_button("Copy").clicked() {
                                                    if let Ok(mut cb) = arboard::Clipboard::new() {
                                                        let _ = cb.set_text(h.clone());
                                                    }
                                                }
                                            }
                                        });
                                        ui.end_row();

                                        ui.label(RichText::new("Swarm Protocol").font(bold_font(11.5)).color(p.text_muted));
                                        code_chip(
                                            ui,
                                            "BitTorrent v1 (BEP-3) • Kademlia DHT (BEP-5) • Magnet ut_metadata (BEP-9) • uTP",
                                            p.text_muted,
                                            p,
                                        );
                                        ui.end_row();
                                    } else {
                                    ui.label(RichText::new("HTTP Headers").font(bold_font(11.5)).color(p.text_muted));
                                    code_chip(
                                        ui,
                                        &format!(
                                            "Accept-Ranges: {}   |   ETag: {}",
                                            if item.supports_ranges { "bytes" } else { "none" },
                                            item.etag.as_deref().unwrap_or("—")
                                        ),
                                        p.text_muted,
                                        p,
                                    );
                                    ui.end_row();

                                    ui.label(RichText::new("SHA-256 Digest").font(bold_font(11.5)).color(p.text_muted));
                                    ui.horizontal(|ui| {
                                        match &item.computed_sha256 {
                                            Some(h) => {
                                                let col = match item.sha256_verified {
                                                    Some(true) => p.success,
                                                    Some(false) => p.danger,
                                                    None => p.brand,
                                                };
                                                code_chip(ui, h, col, p);
                                                if ui.small_button("Copy").clicked() {
                                                    if let Ok(mut cb) = arboard::Clipboard::new() {
                                                        let _ = cb.set_text(h.clone());
                                                    }
                                                }
                                            }
                                            None => {
                                                ui.label(
                                                    RichText::new("Not computed yet")
                                                        .font(mono_font(11.5))
                                                        .color(p.text_subtle),
                                                );
                                            }
                                        }
                                    });
                                    ui.end_row();

                                    ui.label(RichText::new("Verify Hash").font(bold_font(11.5)).color(p.text_muted));
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::TextEdit::singleline(&mut self.verify_hash_input)
                                                .font(mono_font(11.5))
                                                .hint_text("Optional expected 64-char SHA-256 hex...")
                                                .desired_width(350.0),
                                        );
                                        let can_verify = item.status == DownloadStatus::Completed;
                                        ui.add_enabled_ui(can_verify, |ui| {
                                            if brand_cta_button(ui, "Verify SHA-256", Some(VectorIcon::Shield), p).clicked() {
                                                let exp = if self.verify_hash_input.trim().is_empty() {
                                                    None
                                                } else {
                                                    Some(self.verify_hash_input.trim().to_string())
                                                };
                                                self.engine.send(EngineCommand::VerifySha256 {
                                                    id: item.id,
                                                    expected: exp,
                                                });
                                            }
                                        });
                                    });
                                    ui.end_row();
                                    }
                                });
                        });
                    }
                }
            });
    }

    fn render_add_modal(&mut self, ctx: &egui::Context, _snap: &EngineSnapshot, p: &Palette) {
        let mut cancel_clicked = paint_modal_scrim(ctx, "vortex_add_scrim", p);
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel_clicked = true;
        }

        let mut should_submit: Option<bool> = None;
        let mut trigger_probe = false;
        let is_torrent = is_torrent_input(&self.add_modal.url);
        let accent = if is_torrent { p.teal } else { p.brand };
        let max_body_h = (ctx.screen_rect().height() - 135.0).clamp(220.0, 440.0);

        egui::Window::new("New Download / Torrent")
            .id(egui::Id::new("vortex_add_modal"))
            .title_bar(false)
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(564.0)
            .max_width(564.0)
            .frame(modal_window_frame(p))
            .show(ctx, |ui| {
                ui.set_width(562.0);
                ui.spacing_mut().item_spacing.y = 0.0;

                // Custom 9Router studio header band
                let close_x = modal_header(
                    ui,
                    if is_torrent {
                        VectorIcon::Magnet
                    } else {
                        VectorIcon::Plus
                    },
                    accent,
                    if is_torrent {
                        "Confirm Torrent Download"
                    } else {
                        "New Download / Torrent"
                    },
                    if is_torrent {
                        "Review destination folder and confirm to start BitTorrent swarm"
                    } else {
                        "Add an HTTP/HTTPS stream, magnet:? link, or local .torrent file"
                    },
                    Some((
                        if is_torrent { "BITTORRENT" } else { "HTTP / P2P" },
                        accent,
                    )),
                    p,
                );
                if close_x {
                    cancel_clicked = true;
                }

                // Scrollable body cards
                egui::ScrollArea::vertical()
                    .id_salt("add_modal_scroll")
                    .max_height(max_body_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 10.0;

                                // Card 1: Source URL / Magnet / .torrent
                                modal_section_card(
                                    ui,
                                    "SOURCE LINK OR TORRENT FILE",
                                    accent,
                                    p,
                                    |ui| {
                                        ui.spacing_mut().item_spacing.y = 8.0;
                                        let url_w = ui.available_width();
                                        let url_resp = styled_text_field(
                                            ui,
                                            &mut self.add_modal.url,
                                            "https://example.com/archive.zip  or  magnet:?xt=urn:btih:...",
                                            true,
                                            url_w,
                                            p,
                                        );
                                        if url_resp.lost_focus()
                                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                        {
                                            should_submit = Some(false);
                                        }

                                        // Action strip: Paste / Open .torrent / Probe Headers
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 6.0;
                                            if secondary_button(ui, "Paste", Some(VectorIcon::Link), p)
                                                .on_hover_text("Paste URL or magnet link from clipboard")
                                                .clicked()
                                            {
                                                if let Ok(mut cb) = arboard::Clipboard::new() {
                                                    if let Ok(txt) = cb.get_text() {
                                                        self.add_modal.url = txt.trim().to_string();
                                                    }
                                                }
                                            }
                                            if secondary_button(
                                                ui,
                                                "Open .torrent",
                                                Some(VectorIcon::Magnet),
                                                p,
                                            )
                                            .on_hover_text("Pick a local .torrent file from disk")
                                            .clicked()
                                            {
                                                if let Some(path) = rfd::FileDialog::new()
                                                    .add_filter("BitTorrent Files", &["torrent"])
                                                    .pick_file()
                                                {
                                                    self.add_modal.url = path.display().to_string();
                                                }
                                            }
                                            if !is_torrent {
                                                ui.add_enabled_ui(!self.add_modal.probing, |ui| {
                                                    if secondary_button(
                                                        ui,
                                                        "Probe",
                                                        Some(VectorIcon::Speed),
                                                        p,
                                                    )
                                                    .on_hover_text(
                                                        "Inspect file size, filename, and byte-range resume support",
                                                    )
                                                    .clicked()
                                                    {
                                                        trigger_probe = true;
                                                    }
                                                });
                                            }

                                            if !self.add_modal.url.is_empty() {
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        if preset_chip(ui, "Clear", false, p)
                                                            .clicked()
                                                        {
                                                            self.add_modal.url.clear();
                                                            self.add_modal.probe_result = None;
                                                            self.add_modal.error = None;
                                                        }
                                                    },
                                                );
                                            }
                                        });

                                        // Live protocol / probe status banner
                                        if is_torrent {
                                            let trimmed_url = self.add_modal.url.trim().trim_matches('"');
                                            let tor_display = if trimmed_url
                                                .to_ascii_lowercase()
                                                .starts_with("magnet:")
                                            {
                                                magnet_display_name(trimmed_url)
                                            } else {
                                                std::path::Path::new(trimmed_url)
                                                    .file_name()
                                                    .and_then(|s| s.to_str())
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| filename_from_url(trimmed_url))
                                            };
                                            egui::Frame::new()
                                                .fill(Palette::alpha(
                                                    p.teal,
                                                    if p.dark { 24 } else { 16 },
                                                ))
                                                .stroke(Stroke::new(
                                                    1.0_f32,
                                                    Palette::alpha(p.teal, 85),
                                                ))
                                                .corner_radius(CornerRadius::same(8))
                                                .inner_margin(Margin::symmetric(10, 7))
                                                .show(ui, |ui| {
                                                    ui.set_width(ui.available_width());
                                                    ui.vertical(|ui| {
                                                        ui.spacing_mut().item_spacing.y = 2.0;
                                                        ui.horizontal(|ui| {
                                                            let (ic, _) = ui.allocate_exact_size(
                                                                Vec2::splat(14.0),
                                                                Sense::hover(),
                                                            );
                                                            paint_vector_icon(
                                                                ui.painter(),
                                                                ic.center(),
                                                                5.5,
                                                                VectorIcon::Magnet,
                                                                p.teal,
                                                            );
                                                            let max_w =
                                                                (ui.available_width() - 8.0)
                                                                    .max(120.0);
                                                            ui.allocate_ui_with_layout(
                                                                Vec2::new(max_w, 16.0),
                                                                Layout::left_to_right(Align::Center),
                                                                |ui| {
                                                                    ui.set_max_width(max_w);
                                                                    ui.add(
                                                                        egui::Label::new(
                                                                            RichText::new(
                                                                                &tor_display,
                                                                            )
                                                                            .font(bold_font(11.5))
                                                                            .color(p.text_main),
                                                                        )
                                                                        .truncate(),
                                                                    )
                                                                    .on_hover_text(&tor_display);
                                                                },
                                                            );
                                                        });
                                                        ui.label(
                                                            RichText::new(
                                                                "BitTorrent Swarm  •  Kademlia DHT + Trackers + Auto Piece Verification",
                                                            )
                                                            .font(body_font(10.5))
                                                            .color(p.teal),
                                                        );
                                                    });
                                                });
                                        } else if self.add_modal.probing {
                                            egui::Frame::new()
                                                .fill(Palette::alpha(
                                                    p.brand,
                                                    if p.dark { 22 } else { 14 },
                                                ))
                                                .stroke(Stroke::new(
                                                    1.0_f32,
                                                    Palette::alpha(p.brand, 75),
                                                ))
                                                .corner_radius(CornerRadius::same(8))
                                                .inner_margin(Margin::symmetric(10, 6))
                                                .show(ui, |ui| {
                                                    ui.set_width(ui.available_width());
                                                    ui.horizontal(|ui| {
                                                        ui.spinner();
                                                        ui.label(
                                                            RichText::new(
                                                                "Probing remote HTTP headers...",
                                                            )
                                                            .font(mono_font(11.2))
                                                            .color(p.brand),
                                                        );
                                                    });
                                                });
                                        } else if let Some(res) = &self.add_modal.probe_result {
                                            match res {
                                                Ok(info) => {
                                                    egui::Frame::new()
                                                        .fill(Palette::alpha(
                                                            p.success,
                                                            if p.dark { 24 } else { 16 },
                                                        ))
                                                        .stroke(Stroke::new(
                                                            1.0_f32,
                                                            Palette::alpha(p.success, 85),
                                                        ))
                                                        .corner_radius(CornerRadius::same(8))
                                                        .inner_margin(Margin::symmetric(10, 6))
                                                        .show(ui, |ui| {
                                                            ui.set_width(ui.available_width());
                                                            ui.horizontal(|ui| {
                                                                let sz = info
                                                                    .total_bytes
                                                                    .map(format_bytes)
                                                                    .unwrap_or_else(|| {
                                                                        "Unknown size".to_string()
                                                                    });
                                                                ui.label(
                                                                    RichText::new(format!(
                                                                        "Size: {}",
                                                                        sz
                                                                    ))
                                                                    .font(bold_font(11.5))
                                                                    .color(p.success),
                                                                );
                                                                ui.label(
                                                                    RichText::new("•")
                                                                        .color(p.border),
                                                                );
                                                                ui.label(
                                                                    RichText::new(
                                                                        if info.supports_ranges {
                                                                            "Multi-Segment & Resume Supported"
                                                                        } else {
                                                                            "Single-Stream Only"
                                                                        },
                                                                    )
                                                                    .font(body_font(11.5))
                                                                    .color(if info.supports_ranges {
                                                                        p.brand
                                                                    } else {
                                                                        p.warning
                                                                    }),
                                                                );
                                                            });
                                                        });
                                                }
                                                Err(e) => {
                                                    egui::Frame::new()
                                                        .fill(Palette::alpha(
                                                            p.danger,
                                                            if p.dark { 26 } else { 16 },
                                                        ))
                                                        .stroke(Stroke::new(
                                                            1.0_f32,
                                                            Palette::alpha(p.danger, 90),
                                                        ))
                                                        .corner_radius(CornerRadius::same(8))
                                                        .inner_margin(Margin::symmetric(10, 6))
                                                        .show(ui, |ui| {
                                                            ui.set_width(ui.available_width());
                                                            ui.label(
                                                                RichText::new(format!(
                                                                    "Probe error: {}",
                                                                    e
                                                                ))
                                                                .font(body_font(11.2))
                                                                .color(p.danger),
                                                            );
                                                        });
                                                }
                                            }
                                        }
                                    },
                                );

                                // Card 2: Destination & Transfer Options
                                modal_section_card(
                                    ui,
                                    if is_torrent {
                                        "DESTINATION FOLDER"
                                    } else {
                                        "DESTINATION & TRANSFER OPTIONS"
                                    },
                                    p.brand,
                                    p,
                                    |ui| {
                                        ui.spacing_mut().item_spacing.y = 6.0;

                                        // Save Directory
                                        modal_field_label(ui, "Save directory", None, p);
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 8.0;
                                            let mut dir_str =
                                                self.add_modal.save_dir.display().to_string();
                                            let input_w = (ui.available_width() - 86.0).max(160.0);
                                            if styled_text_field(
                                                ui,
                                                &mut dir_str,
                                                r"C:\Users\...\Downloads",
                                                true,
                                                input_w,
                                                p,
                                            )
                                            .changed()
                                            {
                                                self.add_modal.save_dir = PathBuf::from(dir_str);
                                            }
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    if secondary_button(
                                                        ui,
                                                        "Browse",
                                                        Some(VectorIcon::Folder),
                                                        p,
                                                    )
                                                    .clicked()
                                                    {
                                                        if let Some(folder) = rfd::FileDialog::new()
                                                            .set_directory(&self.add_modal.save_dir)
                                                            .pick_folder()
                                                        {
                                                            self.add_modal.save_dir = folder;
                                                        }
                                                    }
                                                },
                                            );
                                        });

                                        if !is_torrent {
                                            modal_hairline(ui, p);

                                            // Custom Filename
                                            modal_field_label(
                                                ui,
                                                "Custom filename",
                                                Some("Optional"),
                                                p,
                                            );
                                            let fn_w = ui.available_width();
                                            styled_text_field(
                                                ui,
                                                &mut self.add_modal.custom_filename,
                                                "Leave blank to auto-detect from Content-Disposition or URL",
                                                false,
                                                fn_w,
                                                p,
                                            );

                                            modal_hairline(ui, p);

                                            // Parallel Connections Slider
                                            let mut conns = self.add_modal.connections as u32;
                                            modal_slider_row(
                                                ui,
                                                "Parallel connections",
                                                Some("HTTP byte-range streams (1–32)"),
                                                &mut conns,
                                                1,
                                                32,
                                                170.0,
                                                p,
                                            );
                                            self.add_modal.connections = conns as usize;

                                            modal_hairline(ui, p);

                                            // Expected SHA-256 Checksum
                                            modal_field_label(
                                                ui,
                                                "Expected SHA-256 checksum",
                                                Some("Optional"),
                                                p,
                                            );
                                            let sha_w = ui.available_width();
                                            styled_text_field(
                                                ui,
                                                &mut self.add_modal.expected_sha256,
                                                "64-char hex digest — auto-verifies cryptographic integrity on completion",
                                                true,
                                                sha_w,
                                                p,
                                            );
                                        }
                                    },
                                );

                                // Validation error alert banner
                                if let Some(err) = &self.add_modal.error {
                                    egui::Frame::new()
                                        .fill(Palette::alpha(
                                            p.danger,
                                            if p.dark { 28 } else { 18 },
                                        ))
                                        .stroke(Stroke::new(1.0_f32, Palette::alpha(p.danger, 110)))
                                        .corner_radius(CornerRadius::same(9))
                                        .inner_margin(Margin::symmetric(12, 8))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.label(
                                                RichText::new(err)
                                                    .font(bold_font(11.5))
                                                    .color(p.danger),
                                            );
                                        });
                                }
                            });
                    });

                // Custom 9Router studio footer band
                modal_footer(ui, p, |ui| {
                    if secondary_button(ui, "Cancel", None, p).clicked() {
                        cancel_clicked = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let cta_label = if is_torrent {
                            "Start Torrent"
                        } else {
                            "Start Download"
                        };
                        if brand_cta_button(ui, cta_label, Some(VectorIcon::Play), p).clicked() {
                            should_submit = Some(false);
                        }
                        if secondary_button(ui, "Add Paused", Some(VectorIcon::Pause), p).clicked()
                        {
                            should_submit = Some(true);
                        }
                    });
                });
            });

        if cancel_clicked {
            self.add_modal.open = false;
        }

        if trigger_probe {
            self.start_probe(ctx.clone());
        }

        if let Some(start_paused) = should_submit {
            let url = self.add_modal.url.trim().to_string();
            let valid_http = url.starts_with("http://") || url.starts_with("https://");
            let valid_torrent = is_torrent_input(&url);
            if !valid_http && !valid_torrent {
                self.add_modal.error = Some(
                    "Enter an http://, https://, magnet:? link, or select a .torrent file"
                        .to_string(),
                );
                self.add_modal.open = true;
            } else {
                let custom_name = if self.add_modal.custom_filename.trim().is_empty() {
                    None
                } else {
                    Some(self.add_modal.custom_filename.trim().to_string())
                };
                let expected = if self.add_modal.expected_sha256.trim().is_empty() {
                    None
                } else {
                    Some(self.add_modal.expected_sha256.trim().to_string())
                };
                self.engine.send(EngineCommand::Add(NewDownloadRequest {
                    url,
                    custom_filename: custom_name,
                    save_dir: Some(self.add_modal.save_dir.clone()),
                    max_connections: Some(self.add_modal.connections),
                    expected_sha256: expected,
                    start_paused,
                }));
                self.add_modal.open = false;
            }
        }
    }

    fn render_settings_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        let mut cancel_clicked = paint_modal_scrim(ctx, "vortex_settings_scrim", p);
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel_clicked = true;
        }
        let mut save_clicked = false;
        let max_body_h = (ctx.screen_rect().height() - 108.0).clamp(220.0, 515.0);

        egui::Window::new("VortexDM Settings")
            .id(egui::Id::new("vortex_settings_modal"))
            .title_bar(false)
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(576.0)
            .max_width(576.0)
            .frame(modal_window_frame(p))
            .show(ctx, |ui| {
                ui.set_width(574.0);
                ui.spacing_mut().item_spacing.y = 0.0;

                // Custom 9Router studio header band
                let close_x = modal_header(
                    ui,
                    VectorIcon::Gear,
                    p.brand,
                    "Settings",
                    "Storage paths, transfer engine, and Windows tray behavior",
                    None,
                    p,
                );
                if close_x {
                    cancel_clicked = true;
                }

                // Scrollable body cards (all 3 cards fit cleanly with zero scroll at 960x620)
                egui::ScrollArea::vertical()
                    .id_salt("settings_modal_scroll")
                    .max_height(max_body_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(Margin::symmetric(14, 8))
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 7.0;

                                // Card 1: Storage & Destination
                                modal_section_card(ui, "STORAGE & DESTINATION", p.brand, p, |ui| {
                                    ui.spacing_mut().item_spacing.y = 4.0;

                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        let mut dir_str = self
                                            .settings_draft
                                            .download_dir
                                            .display()
                                            .to_string();
                                        let input_w = (ui.available_width() - 86.0).max(160.0);
                                        if styled_text_field(
                                            ui,
                                            &mut dir_str,
                                            r"C:\Users\...\Downloads",
                                            true,
                                            input_w,
                                            p,
                                        )
                                        .changed()
                                        {
                                            self.settings_draft.download_dir =
                                                PathBuf::from(dir_str);
                                        }
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if secondary_button(
                                                    ui,
                                                    "Browse",
                                                    Some(VectorIcon::Folder),
                                                    p,
                                                )
                                                .clicked()
                                                {
                                                    if let Some(folder) = rfd::FileDialog::new()
                                                        .set_directory(
                                                            &self.settings_draft.download_dir,
                                                        )
                                                        .pick_folder()
                                                    {
                                                        self.settings_draft.download_dir = folder;
                                                    }
                                                }
                                            },
                                        );
                                    });

                                    modal_hairline(ui, p);

                                    modal_toggle_row(
                                        ui,
                                        "Auto-sort into category subfolders",
                                        Some("Programs, Archives, Videos, Torrents, Documents..."),
                                        &mut self.settings_draft.organize_by_category,
                                        true,
                                        false,
                                        p,
                                    );
                                });

                                // Card 2: Transfer Engine & Bandwidth
                                modal_section_card(
                                    ui,
                                    "TRANSFER ENGINE & BANDWIDTH",
                                    p.brand,
                                    p,
                                    |ui| {
                                        ui.spacing_mut().item_spacing.y = 4.0;

                                        let mut conns =
                                            self.settings_draft.default_connections as u32;
                                        modal_slider_row(
                                            ui,
                                            "Connections per download",
                                            None,
                                            &mut conns,
                                            1,
                                            32,
                                            175.0,
                                            p,
                                        );
                                        self.settings_draft.default_connections = conns as usize;

                                        let mut max_conc =
                                            self.settings_draft.max_concurrent_downloads as u32;
                                        modal_slider_row(
                                            ui,
                                            "Max simultaneous downloads",
                                            None,
                                            &mut max_conc,
                                            1,
                                            10,
                                            175.0,
                                            p,
                                        );
                                        self.settings_draft.max_concurrent_downloads =
                                            max_conc as usize;

                                        let mut retries = self.settings_draft.max_retries;
                                        modal_slider_row(
                                            ui,
                                            "Retry attempts per segment",
                                            None,
                                            &mut retries,
                                            0,
                                            15,
                                            175.0,
                                            p,
                                        );
                                        self.settings_draft.max_retries = retries;

                                        modal_hairline(ui, p);

                                        // Global Speed Cap with quick preset pills + inline KB/s input
                                        let cur_kb = self
                                            .speed_limit_kb_draft
                                            .trim()
                                            .parse::<u64>()
                                            .unwrap_or(0);
                                        ui.horizontal(|ui| {
                                            ui.vertical(|ui| {
                                                ui.spacing_mut().item_spacing.y = 0.0;
                                                ui.label(
                                                    RichText::new("Global speed cap")
                                                        .font(body_font(12.0))
                                                        .color(p.text_main),
                                                );
                                                ui.label(
                                                    RichText::new(
                                                        "HTTP & BitTorrent (0 = Unlimited)",
                                                    )
                                                    .font(body_font(10.0))
                                                    .color(p.text_muted),
                                                );
                                            });

                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    ui.spacing_mut().item_spacing.x = 5.0;
                                                    ui.label(
                                                        RichText::new("KB/s")
                                                            .font(mono_font(10.5))
                                                            .color(p.text_subtle),
                                                    );
                                                    styled_text_field(
                                                        ui,
                                                        &mut self.speed_limit_kb_draft,
                                                        "0",
                                                        true,
                                                        68.0,
                                                        p,
                                                    );
                                                    ui.add_space(3.0);
                                                    for (label, kb_val) in [
                                                        ("10M", 10240u64),
                                                        ("5M", 5120u64),
                                                        ("2M", 2048u64),
                                                        ("Unlim", 0u64),
                                                    ] {
                                                        if preset_chip(
                                                            ui,
                                                            label,
                                                            cur_kb == kb_val,
                                                            p,
                                                        )
                                                        .clicked()
                                                        {
                                                            self.speed_limit_kb_draft =
                                                                kb_val.to_string();
                                                        }
                                                    }
                                                },
                                            );
                                        });
                                    },
                                );

                                // Card 3: Windows Tray & Automation
                                modal_section_card(
                                    ui,
                                    "WINDOWS TRAY & AUTOMATION",
                                    p.brand,
                                    p,
                                    |ui| {
                                        ui.spacing_mut().item_spacing.y = 3.0;

                                        modal_toggle_row(
                                            ui,
                                            "Minimize to System Tray when closing window (X)",
                                            None,
                                            &mut self.settings_draft.close_to_tray,
                                            true,
                                            false,
                                            p,
                                        );

                                        modal_hairline(ui, p);

                                        modal_toggle_row(
                                            ui,
                                            "Start VortexDM automatically on Windows logon",
                                            None,
                                            &mut self.settings_draft.start_with_windows,
                                            true,
                                            false,
                                            p,
                                        );

                                        modal_toggle_row(
                                            ui,
                                            "Launch minimized directly in System Tray",
                                            None,
                                            &mut self.settings_draft.start_minimized,
                                            self.settings_draft.start_with_windows,
                                            true,
                                            p,
                                        );

                                        modal_hairline(ui, p);

                                        modal_toggle_row(
                                            ui,
                                            "Clipboard link guard (detect copied HTTP & magnet:? links)",
                                            None,
                                            &mut self.settings_draft.clipboard_monitoring,
                                            true,
                                            false,
                                            p,
                                        );

                                        modal_hairline(ui, p);

                                        modal_toggle_row(
                                            ui,
                                            "Automatically compute SHA-256 checksum on completion",
                                            None,
                                            &mut self.settings_draft.auto_verify_sha256,
                                            true,
                                            false,
                                            p,
                                        );
                                    },
                                );
                            });
                    });

                // Custom 9Router studio footer band
                modal_footer(ui, p, |ui| {
                    ui.label(
                        RichText::new("Esc to discard")
                            .font(body_font(11.0))
                            .color(p.text_subtle),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if brand_cta_button(ui, "Save Settings", Some(VectorIcon::Check), p)
                            .clicked()
                        {
                            save_clicked = true;
                        }
                        if secondary_button(ui, "Cancel", None, p).clicked() {
                            cancel_clicked = true;
                        }
                    });
                });
            });

        if cancel_clicked {
            self.settings_open = false;
        }

        if save_clicked {
            let kb = self
                .speed_limit_kb_draft
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
            self.settings_draft.global_speed_limit_bps = kb.saturating_mul(1024);
            self.settings_draft.dark_mode = self.applied_dark_mode;
            self.popup_shared
                .autostart_cached
                .store(self.settings_draft.start_with_windows, Ordering::Relaxed);
            self.engine
                .send(EngineCommand::UpdateSettings(self.settings_draft.clone()));
            self.settings_open = false;
            self.notify("Settings saved");
        }
    }

    fn render_delete_confirm_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut state) = self.delete_confirm.take() else {
            return;
        };
        if state.items.is_empty() {
            return;
        }

        let mut cancel_clicked = paint_modal_scrim(ctx, "vortex_delete_confirm_scrim", p);
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel_clicked = true;
        }
        let mut confirm_clicked = false;
        let count = state.items.len();
        let is_multi = count > 1;
        let modal_w = if is_multi { 460.0 } else { 436.0 };

        let header_title = if is_multi {
            format!("Delete {} Downloads?", count)
        } else if state.items[0].is_torrent {
            "Delete Torrent?".to_string()
        } else {
            "Delete Download?".to_string()
        };
        let header_sub = if is_multi {
            "Selected transfers will be stopped and removed"
        } else {
            "This transfer will be stopped and removed from VortexDM"
        };

        egui::Window::new("Delete Download")
            .id(egui::Id::new("vortex_delete_confirm_modal"))
            .title_bar(false)
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(modal_w)
            .max_width(modal_w)
            .frame(modal_window_frame(p))
            .show(ctx, |ui| {
                ui.set_width(modal_w - 2.0);
                ui.spacing_mut().item_spacing.y = 0.0;

                let close_x = modal_header(
                    ui,
                    VectorIcon::Trash,
                    p.danger,
                    &header_title,
                    header_sub,
                    if is_multi {
                        Some(("BULK DELETE", p.danger))
                    } else {
                        None
                    },
                    p,
                );
                if close_x {
                    cancel_clicked = true;
                }

                egui::Frame::new()
                    .inner_margin(Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 10.0;

                        egui::Frame::new()
                            .fill(p.surface)
                            .corner_radius(CornerRadius::same(11))
                            .stroke(Stroke::new(1.0_f32, p.border))
                            .inner_margin(Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 7.0;

                                ui.label(
                                    RichText::new(if is_multi {
                                        format!("SELECTED TRANSFERS ({})", count)
                                    } else {
                                        "SELECTED TRANSFER".to_string()
                                    })
                                    .font(bold_font(9.5))
                                    .color(p.text_subtle),
                                );

                                // Single filename pill or compact scrollable list for multi-delete
                                egui::Frame::new()
                                    .fill(p.bg_alt)
                                    .corner_radius(CornerRadius::same(8))
                                    .stroke(Stroke::new(1.0_f32, p.border))
                                    .inner_margin(Margin::symmetric(10, 7))
                                    .show(ui, |ui| {
                                        let w = ui.available_width();
                                        ui.set_min_width(w);
                                        ui.set_max_width(w);
                                        if !is_multi {
                                            let name = &state.items[0].filename;
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(name)
                                                        .font(mono_font(11.5))
                                                        .color(p.text_main),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(name);
                                        } else {
                                            egui::ScrollArea::vertical()
                                                .id_salt("multi_delete_list")
                                                .max_height(112.0)
                                                .auto_shrink([false, true])
                                                .show(ui, |ui| {
                                                    ui.spacing_mut().item_spacing.y = 4.0;
                                                    let row_w = (w - 8.0).max(120.0);
                                                    for entry in &state.items {
                                                        ui.horizontal(|ui| {
                                                            ui.spacing_mut().item_spacing.x = 6.0;
                                                            let (dot, _) = ui.allocate_exact_size(
                                                                Vec2::splat(7.0),
                                                                Sense::hover(),
                                                            );
                                                            ui.painter().circle_filled(
                                                                dot.center(),
                                                                2.6,
                                                                if entry.is_torrent {
                                                                    p.teal
                                                                } else {
                                                                    p.brand
                                                                },
                                                            );
                                                            let text_w = (row_w - 16.0).max(80.0);
                                                            ui.allocate_ui_with_layout(
                                                                Vec2::new(text_w, 16.0),
                                                                Layout::left_to_right(Align::Center),
                                                                |ui| {
                                                                    ui.set_max_width(text_w);
                                                                    ui.add(
                                                                        egui::Label::new(
                                                                            RichText::new(
                                                                                &entry.filename,
                                                                            )
                                                                            .font(mono_font(11.0))
                                                                            .color(p.text_main),
                                                                        )
                                                                        .truncate(),
                                                                    )
                                                                    .on_hover_text(&entry.filename);
                                                                },
                                                            );
                                                        });
                                                    }
                                                });
                                        }
                                    });

                                modal_hairline(ui, p);

                                let all_completed = state.items.iter().all(|it| it.is_completed);
                                let sub_text = if all_completed {
                                    "Permanently removes downloaded data from disk"
                                } else {
                                    "Also removes downloaded or partial data from disk"
                                };
                                modal_toggle_row(
                                    ui,
                                    "Also delete files from disk",
                                    Some(sub_text),
                                    &mut state.delete_files,
                                    true,
                                    false,
                                    p,
                                );

                                if state.delete_files {
                                    egui::Frame::new()
                                        .fill(Palette::alpha(
                                            p.danger,
                                            if p.dark { 26 } else { 16 },
                                        ))
                                        .stroke(Stroke::new(
                                            1.0_f32,
                                            Palette::alpha(p.danger, 95),
                                        ))
                                        .corner_radius(CornerRadius::same(8))
                                        .inner_margin(Margin::symmetric(10, 6))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.label(
                                                RichText::new(if is_multi {
                                                    "All selected files will be permanently removed from your folder."
                                                } else {
                                                    "Deleted files will be permanently removed from your folder."
                                                })
                                                .font(body_font(11.0))
                                                .color(p.danger),
                                            );
                                        });
                                }
                            });
                    });

                modal_footer(ui, p, |ui| {
                    ui.label(
                        RichText::new("Esc to cancel")
                            .font(body_font(11.0))
                            .color(p.text_subtle),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let del_label = match (is_multi, state.delete_files) {
                            (true, true) => format!("Delete + Files ({})", count),
                            (true, false) => format!("Remove ({})", count),
                            (false, true) => "Delete + Files".to_string(),
                            (false, false) => "Remove".to_string(),
                        };
                        if danger_cta_button(ui, &del_label, Some(VectorIcon::Trash), p).clicked()
                        {
                            confirm_clicked = true;
                        }
                        if secondary_button(ui, "Cancel", None, p).clicked() {
                            cancel_clicked = true;
                        }
                    });
                });
            });

        if confirm_clicked {
            let delete_file = state.delete_files;
            let ids: Vec<Uuid> = state.items.iter().map(|it| it.id).collect();
            for id in &ids {
                self.checked_ids.remove(id);
                if self.selected_id == Some(*id) {
                    self.selected_id = None;
                }
            }
            if ids.len() == 1 {
                self.engine.send(EngineCommand::Remove {
                    id: ids[0],
                    delete_file,
                });
                self.notify(if delete_file {
                    "Removed & deleted files"
                } else {
                    "Removed download"
                });
            } else {
                let n = ids.len();
                self.engine.send(EngineCommand::RemoveMany {
                    ids,
                    delete_file,
                });
                self.notify(if delete_file {
                    format!("Removed & deleted files for {} downloads", n)
                } else {
                    format!("Removed {} downloads", n)
                });
            }
        } else if !cancel_clicked {
            self.delete_confirm = Some(state);
        }
    }

    fn render_clear_completed_confirm_modal(
        &mut self,
        ctx: &egui::Context,
        snap: &EngineSnapshot,
        p: &Palette,
    ) {
        let completed_items: Vec<&DownloadItem> = snap
            .downloads
            .iter()
            .filter(|d| d.status == DownloadStatus::Completed)
            .collect();
        if completed_items.is_empty() {
            self.clear_completed_confirm = false;
            return;
        }

        let mut cancel_clicked = paint_modal_scrim(ctx, "vortex_clear_completed_scrim", p);
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel_clicked = true;
        }
        let mut confirm_clicked = false;
        let count = completed_items.len();
        let is_multi = count > 1;
        let modal_w = 450.0;

        let header_title = if is_multi {
            format!("Clear {} Completed Downloads?", count)
        } else {
            "Clear Completed Download?".to_string()
        };
        let header_sub = "Finished entries will be removed from your completed list";

        egui::Window::new("Clear Completed")
            .id(egui::Id::new("vortex_clear_completed_modal"))
            .title_bar(false)
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(modal_w)
            .max_width(modal_w)
            .frame(modal_window_frame(p))
            .show(ctx, |ui| {
                ui.set_width(modal_w - 2.0);
                ui.spacing_mut().item_spacing.y = 0.0;

                let close_x = modal_header(
                    ui,
                    VectorIcon::Clear,
                    p.brand,
                    &header_title,
                    header_sub,
                    None,
                    p,
                );
                if close_x {
                    cancel_clicked = true;
                }

                egui::Frame::new()
                    .inner_margin(Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 10.0;

                        egui::Frame::new()
                            .fill(p.surface)
                            .corner_radius(CornerRadius::same(11))
                            .stroke(Stroke::new(1.0_f32, p.border))
                            .inner_margin(Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 7.0;

                                ui.label(
                                    RichText::new(format!("COMPLETED TRANSFERS ({})", count))
                                        .font(bold_font(9.5))
                                        .color(p.text_subtle),
                                );

                                egui::Frame::new()
                                    .fill(p.bg_alt)
                                    .corner_radius(CornerRadius::same(8))
                                    .stroke(Stroke::new(1.0_f32, p.border))
                                    .inner_margin(Margin::symmetric(10, 7))
                                    .show(ui, |ui| {
                                        let w = ui.available_width();
                                        ui.set_min_width(w);
                                        ui.set_max_width(w);
                                        if !is_multi {
                                            let name = &completed_items[0].filename;
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(name)
                                                        .font(mono_font(11.5))
                                                        .color(p.text_main),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(name);
                                        } else {
                                            egui::ScrollArea::vertical()
                                                .id_salt("clear_completed_list")
                                                .max_height(112.0)
                                                .auto_shrink([false, true])
                                                .show(ui, |ui| {
                                                    ui.spacing_mut().item_spacing.y = 4.0;
                                                    let row_w = (w - 8.0).max(120.0);
                                                    for entry in &completed_items {
                                                        ui.horizontal(|ui| {
                                                            ui.spacing_mut().item_spacing.x = 6.0;
                                                            let (dot, _) = ui.allocate_exact_size(
                                                                Vec2::splat(7.0),
                                                                Sense::hover(),
                                                            );
                                                            ui.painter().circle_filled(
                                                                dot.center(),
                                                                2.6,
                                                                if entry.is_torrent() {
                                                                    p.teal
                                                                } else {
                                                                    p.success
                                                                },
                                                            );
                                                            let text_w = (row_w - 16.0).max(80.0);
                                                            ui.allocate_ui_with_layout(
                                                                Vec2::new(text_w, 16.0),
                                                                Layout::left_to_right(Align::Center),
                                                                |ui| {
                                                                    ui.set_max_width(text_w);
                                                                    ui.add(
                                                                        egui::Label::new(
                                                                            RichText::new(
                                                                                &entry.filename,
                                                                            )
                                                                            .font(mono_font(11.0))
                                                                            .color(p.text_main),
                                                                        )
                                                                        .truncate(),
                                                                    )
                                                                    .on_hover_text(&entry.filename);
                                                                },
                                                            );
                                                        });
                                                    }
                                                });
                                        }
                                    });

                                modal_hairline(ui, p);

                                egui::Frame::new()
                                    .fill(Palette::alpha(
                                        p.success,
                                        if p.dark { 22 } else { 15 },
                                    ))
                                    .stroke(Stroke::new(
                                        1.0_f32,
                                        Palette::alpha(p.success, 90),
                                    ))
                                    .corner_radius(CornerRadius::same(8))
                                    .inner_margin(Margin::symmetric(10, 6))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.label(
                                            RichText::new(
                                                "Your downloaded files on disk will not be deleted.",
                                            )
                                            .font(body_font(11.0))
                                            .color(p.success),
                                        );
                                    });
                            });
                    });

                modal_footer(ui, p, |ui| {
                    ui.label(
                        RichText::new("Esc to cancel")
                            .font(body_font(11.0))
                            .color(p.text_subtle),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let cta_label = if is_multi {
                            format!("Clear Completed ({})", count)
                        } else {
                            "Clear Completed".to_string()
                        };
                        if brand_cta_button(ui, &cta_label, Some(VectorIcon::Clear), p).clicked() {
                            confirm_clicked = true;
                        }
                        if secondary_button(ui, "Cancel", None, p).clicked() {
                            cancel_clicked = true;
                        }
                    });
                });
            });

        if confirm_clicked {
            for item in &completed_items {
                self.checked_ids.remove(&item.id);
                if self.selected_id == Some(item.id) {
                    self.selected_id = None;
                }
            }
            self.engine.send(EngineCommand::ClearCompleted);
            self.clear_completed_confirm = false;
            self.notify("Cleared completed downloads");
        } else if cancel_clicked {
            self.clear_completed_confirm = false;
        }
    }

    fn render_footer(&mut self, ui: &mut egui::Ui, snap: &EngineSnapshot, p: &Palette) {
        ui.horizontal(|ui| {
            let tray_ok = self.tray.is_some();
            let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
            ui.painter().circle_filled(
                dot.center(),
                3.6,
                if tray_ok { p.success } else { p.warning },
            );
            let tray_lbl = ui.label(
                RichText::new(if tray_ok {
                    "System Tray Active"
                } else {
                    "Tray Unavailable"
                })
                .font(body_font(11.5))
                .color(p.text_muted),
            );
            if let Some(err) = &self.tray_error {
                tray_lbl.on_hover_text(err);
            }

            if snap.settings.clipboard_monitoring {
                ui.label(RichText::new("•").color(p.border));
                ui.label(
                    RichText::new("Clipboard Guard ON")
                        .font(body_font(11.5))
                        .color(p.text_subtle),
                );
            }

            if let Some((msg, at)) = &self.toast_notice {
                if at.elapsed() < Duration::from_secs(4) {
                    ui.label(RichText::new("•").color(p.border));
                    ui.label(RichText::new(msg).font(bold_font(11.5)).color(p.brand));
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("VortexDM v{}", APP_VERSION))
                        .font(mono_font(11.0))
                        .color(p.text_subtle),
                );
                if let Some(free) = self.cached_disk_free {
                    ui.label(RichText::new("•").color(p.border));
                    ui.label(
                        RichText::new(format!("Free: {}", format_bytes(free)))
                            .font(mono_font(11.0))
                            .color(p.text_muted),
                    );
                }
                ui.label(RichText::new("•").color(p.border));
                ui.label(
                    RichText::new(format!(
                        "{} active  /  {} total",
                        snap.stats.active_count,
                        snap.downloads.len()
                    ))
                    .font(mono_font(11.0))
                    .color(p.text_muted),
                );
            });
        });
    }
}

fn kpi_card(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    sub: String,
    accent: Color32,
    p: &Palette,
) {
    egui::Frame::new()
        .fill(p.surface)
        .corner_radius(CornerRadius::same(9))
        .stroke(Stroke::new(1.0_f32, p.border))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(
                RichText::new(label)
                    .font(bold_font(9.0))
                    .color(p.text_subtle),
            );
            ui.label(
                RichText::new(value)
                    .font(bold_font(14.5))
                    .color(accent),
            );
            ui.add(
                egui::Label::new(RichText::new(sub).font(body_font(9.8)).color(p.text_muted))
                    .truncate(),
            );
        });
}

fn hero_feature_card(
    ui: &mut egui::Ui,
    step_num: &str,
    title: &str,
    desc: &str,
    chip: &str,
    p: &Palette,
) {
    let resp = egui::Frame::new()
        .fill(p.surface)
        .corner_radius(CornerRadius::same(11))
        .stroke(Stroke::new(1.0_f32, p.border))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(106.0);

            // Signature 9Router circular coral numbered badge
            ui.horizontal(|ui| {
                let (badge_rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                ui.painter().circle_filled(badge_rect.center(), 11.0, p.brand);
                ui.painter().text(
                    badge_rect.center(),
                    Align2::CENTER_CENTER,
                    step_num,
                    bold_font(11.0),
                    Color32::WHITE,
                );
                ui.add_space(3.0);
                ui.label(RichText::new(title).font(bold_font(12.5)).color(p.text_main));
            });

            ui.add_space(5.0);
            ui.label(
                RichText::new(desc)
                    .font(body_font(11.0))
                    .color(p.text_muted),
            );

            ui.add_space(7.0);
            code_chip(ui, chip, p.brand, p);
        });

    if resp.response.hovered() {
        ui.painter().rect_stroke(
            resp.response.rect,
            CornerRadius::same(11),
            Stroke::new(1.0_f32, Palette::alpha(p.brand, 130)),
            StrokeKind::Inside,
        );
    }
}

fn code_chip(ui: &mut egui::Ui, text: &str, fg: Color32, p: &Palette) {
    egui::Frame::new()
        .fill(p.bg_alt)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).font(mono_font(11.0)).color(fg));
        });
}

fn sidebar_nav_row(
    ui: &mut egui::Ui,
    label: &str,
    count: usize,
    dot_color: Color32,
    selected: bool,
    p: &Palette,
    on_click: impl FnOnce(),
) {
    let desired = Vec2::new(ui.available_width(), 23.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());

    if selected {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(8),
            Palette::alpha(p.brand, if p.dark { 28 } else { 20 }),
        );
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, Palette::alpha(p.brand, 95)),
            StrokeKind::Inside,
        );
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), p.surface);
    }

    ui.painter().circle_filled(
        Pos2::new(rect.min.x + 12.0, rect.center().y),
        3.5,
        if selected { p.brand } else { dot_color },
    );

    let text_col = if selected { p.brand } else { p.text_main };
    ui.painter().text(
        Pos2::new(rect.min.x + 24.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        if selected { bold_font(12.5) } else { body_font(12.5) },
        text_col,
    );

    if count > 0 {
        let count_str = count.to_string();
        let pill_rect = Rect::from_center_size(
            Pos2::new(rect.max.x - 16.0, rect.center().y),
            Vec2::new(22.0, 17.0),
        );
        ui.painter().rect_filled(
            pill_rect,
            CornerRadius::same(5),
            if selected {
                Palette::alpha(p.brand, 38)
            } else {
                p.surface_2
            },
        );
        ui.painter().text(
            pill_rect.center(),
            Align2::CENTER_CENTER,
            count_str,
            mono_font(10.5),
            if selected { p.brand } else { p.text_muted },
        );
    }

    if resp.clicked() {
        on_click();
    }
}

/// Crisp 9Router segmented tab button with status dot, label, and live count badge.
fn tab_segment(
    ui: &mut egui::Ui,
    label: &str,
    count: usize,
    accent: Color32,
    active: bool,
    p: &Palette,
) -> egui::Response {
    let font = if active {
        bold_font(12.0)
    } else {
        body_font(12.0)
    };
    let text_col = if active { p.text_main } else { p.text_muted };
    let text_galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), text_col);

    let count_str = count.to_string();
    let count_galley = ui
        .painter()
        .layout_no_wrap(count_str.clone(), mono_font(10.0), accent);
    let badge_w = (count_galley.size().x + 10.0).max(20.0);

    let total_w = 10.0 + 6.0 + 6.0 + text_galley.size().x + 7.0 + badge_w + 8.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, 25.0), Sense::click());

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    if active {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(7), p.surface);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(7),
            Stroke::new(
                1.0_f32,
                Palette::alpha(accent, if p.dark { 130 } else { 115 }),
            ),
            StrokeKind::Inside,
        );
    } else if resp.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(7),
            Palette::alpha(p.surface, if p.dark { 170 } else { 200 }),
        );
    }

    let cy = rect.center().y;
    let dot_x = rect.min.x + 13.0;
    let dot_col = if active || count > 0 {
        accent
    } else {
        p.text_subtle
    };
    ui.painter().circle_filled(Pos2::new(dot_x, cy), 3.0, dot_col);

    let label_x = dot_x + 9.0;
    let fg = if active || resp.hovered() {
        p.text_main
    } else {
        p.text_muted
    };
    ui.painter()
        .text(Pos2::new(label_x, cy), Align2::LEFT_CENTER, label, font, fg);

    let badge_rect = Rect::from_center_size(
        Pos2::new(rect.max.x - 8.0 - badge_w * 0.5, cy),
        Vec2::new(badge_w, 16.0),
    );
    let badge_bg = if active {
        Palette::alpha(accent, if p.dark { 36 } else { 24 })
    } else {
        p.surface_2
    };
    let badge_fg = if active { accent } else { p.text_muted };
    ui.painter()
        .rect_filled(badge_rect, CornerRadius::same(5), badge_bg);
    ui.painter().text(
        badge_rect.center(),
        Align2::CENTER_CENTER,
        count_str,
        mono_font(10.0),
        badge_fg,
    );

    resp
}

/// Custom-painted smooth warm-coral EWMA speed sparkline.
fn paint_sparkline(
    ui: &mut egui::Ui,
    samples: &VecDeque<f64>,
    height: f32,
    color: Color32,
    p: &Palette,
) {
    let width = ui.available_width().max(80.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, CornerRadius::same(8), p.bg_alt);
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0_f32, p.border_subtle),
        StrokeKind::Inside,
    );

    // Subtle horizontal grid baseline inside chart
    let mid_y = rect.center().y;
    painter.line_segment(
        [Pos2::new(rect.min.x + 4.0, mid_y), Pos2::new(rect.max.x - 4.0, mid_y)],
        Stroke::new(1.0_f32, p.border_subtle),
    );

    if samples.len() < 2 {
        return;
    }

    let max_val = samples
        .iter()
        .copied()
        .fold(1024.0f64, |acc, v| acc.max(v))
        * 1.15;

    let pad = 5.0;
    let inner_w = (rect.width() - pad * 2.0).max(1.0);
    let inner_h = (rect.height() - pad * 2.0).max(1.0);
    let step_x = inner_w / (samples.len() - 1) as f32;

    let mut pts: Vec<Pos2> = Vec::with_capacity(samples.len());
    for (i, &v) in samples.iter().enumerate() {
        let x = rect.min.x + pad + (i as f32) * step_x;
        let norm = (v / max_val).clamp(0.0, 1.0) as f32;
        let y = rect.max.y - pad - norm * inner_h;
        pts.push(Pos2::new(x, y));
    }

    let fill_col = Palette::alpha(color, if p.dark { 42 } else { 32 });
    for w in pts.windows(2) {
        let p0 = w[0];
        let p1 = w[1];
        let poly = vec![
            p0,
            p1,
            Pos2::new(p1.x, rect.max.y - pad),
            Pos2::new(p0.x, rect.max.y - pad),
        ];
        painter.add(Shape::convex_polygon(poly, fill_col, Stroke::NONE));
    }

    painter.add(Shape::line(pts, Stroke::new(1.8_f32, color)));
}

/// Real-time color-coded multi-segment byte map in 9Router's warm coral / emerald palette.
fn paint_segment_map(ui: &mut egui::Ui, item: &DownloadItem, p: &Palette) {
    let width = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, CornerRadius::same(6), p.bg_alt);

    if let Some(total) = item.total_bytes {
        if total > 0 && !item.segments.is_empty() {
            for seg in &item.segments {
                let x0 = rect.min.x + (seg.start as f64 / total as f64) as f32 * rect.width();
                let x_cur = rect.min.x
                    + (seg.current_offset().min(seg.end + 1) as f64 / total as f64) as f32
                        * rect.width();

                let fill_color = match seg.status {
                    SegmentStatus::Completed => p.success,
                    SegmentStatus::Active => p.brand,
                    SegmentStatus::Idle => Palette::alpha(p.warning, 190),
                    SegmentStatus::Failed => p.danger,
                };

                if x_cur > x0 {
                    let seg_filled = Rect::from_min_max(
                        Pos2::new(x0, rect.min.y + 1.5),
                        Pos2::new(x_cur.max(x0 + 1.5), rect.max.y - 1.5),
                    );
                    painter.rect_filled(seg_filled, CornerRadius::same(2), fill_color);
                }

                painter.line_segment(
                    [Pos2::new(x0, rect.min.y), Pos2::new(x0, rect.max.y)],
                    Stroke::new(1.0_f32, p.surface),
                );
            }
        } else {
            let filled_w = rect.width() * item.progress();
            if filled_w > 0.0 {
                let r = Rect::from_min_size(rect.min, Vec2::new(filled_w, rect.height()));
                painter.rect_filled(r, CornerRadius::same(6), p.status_color(item.status));
            }
        }
    }

    painter.rect_stroke(
        rect,
        CornerRadius::same(6),
        Stroke::new(1.0_f32, p.border),
        StrokeKind::Inside,
    );
}

/// Outer frameless 9Router modal card frame with crisp border & elevated drop shadow.
fn modal_window_frame(p: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(p.bg)
        .corner_radius(CornerRadius::same(14))
        .stroke(Stroke::new(1.0_f32, p.border))
        .inner_margin(Margin::same(1))
        .shadow(egui::epaint::Shadow {
            offset: [0, 14],
            blur: 36,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 170 } else { 46 }),
        })
}

/// Custom frameless 9Router modal header band with tinted icon tile, title/subtitle, optional badge, and close button.
fn modal_header(
    ui: &mut egui::Ui,
    icon: VectorIcon,
    accent: Color32,
    title: &str,
    subtitle: &str,
    badge: Option<(&str, Color32)>,
    p: &Palette,
) -> bool {
    let mut close_clicked = false;
    egui::Frame::new()
        .fill(p.bg_alt)
        .corner_radius(CornerRadius {
            nw: 13,
            ne: 13,
            sw: 0,
            se: 0,
        })
        .inner_margin(Margin::symmetric(15, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;

                // 30x30 tinted icon tile
                let (tile_rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                ui.painter().rect_filled(
                    tile_rect,
                    CornerRadius::same(8),
                    Palette::alpha(accent, if p.dark { 30 } else { 20 }),
                );
                ui.painter().rect_stroke(
                    tile_rect,
                    CornerRadius::same(8),
                    Stroke::new(1.0_f32, Palette::alpha(accent, if p.dark { 95 } else { 78 })),
                    StrokeKind::Inside,
                );
                paint_vector_icon(ui.painter(), tile_rect.center(), 6.2, icon, accent);

                // Title + subtitle stack
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(RichText::new(title).font(bold_font(14.0)).color(p.text_main));
                    ui.label(
                        RichText::new(subtitle)
                            .font(body_font(10.8))
                            .color(p.text_muted),
                    );
                });

                // Right-aligned optional pill + crisp X close button
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if square_icon_button(ui, VectorIcon::CloseX, "Close (Esc)", p, 27.0).clicked()
                    {
                        close_clicked = true;
                    }
                    if let Some((badge_txt, badge_col)) = badge {
                        status_pill(ui, badge_txt, badge_col, p);
                    }
                });
            });
        });

    // Crisp 1px header bottom border
    let (line_rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(line_rect, CornerRadius::ZERO, p.border);

    close_clicked
}

/// Custom frameless 9Router modal footer band with top hairline and action bar.
fn modal_footer(ui: &mut egui::Ui, p: &Palette, add_contents: impl FnOnce(&mut egui::Ui)) {
    let (line_rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(line_rect, CornerRadius::ZERO, p.border);

    egui::Frame::new()
        .fill(p.bg_alt)
        .corner_radius(CornerRadius {
            nw: 0,
            ne: 0,
            sw: 13,
            se: 13,
        })
        .inner_margin(Margin::symmetric(15, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                add_contents(ui);
            });
        });
}

/// Elevated 9Router settings/form section card with uppercase eyebrow label.
fn modal_section_card(
    ui: &mut egui::Ui,
    eyebrow: &str,
    accent: Color32,
    p: &Palette,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(p.surface)
        .corner_radius(CornerRadius::same(11))
        .stroke(Stroke::new(1.0_f32, p.border))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(eyebrow).font(bold_font(9.5)).color(accent));
            ui.add_space(4.0);
            add_contents(ui);
        });
}

/// Subtle 1px full-width divider inside modal section cards.
fn modal_hairline(ui: &mut egui::Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 4.0), Sense::hover());
    let y = rect.center().y;
    ui.painter().line_segment(
        [Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)],
        Stroke::new(1.0_f32, p.border_subtle),
    );
}

/// Clean field label with optional subtle "Optional" pill tag.
fn modal_field_label(ui: &mut egui::Ui, label: &str, tag: Option<&str>, p: &Palette) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(RichText::new(label).font(bold_font(11.5)).color(p.text_main));
        if let Some(t) = tag {
            let galley = ui
                .painter()
                .layout_no_wrap(t.to_string(), body_font(9.8), p.text_subtle);
            let (r, _) =
                ui.allocate_exact_size(Vec2::new(galley.size().x + 10.0, 16.0), Sense::hover());
            ui.painter()
                .rect_filled(r, CornerRadius::same(4), p.bg_alt);
            ui.painter().rect_stroke(
                r,
                CornerRadius::same(4),
                Stroke::new(1.0_f32, p.border_subtle),
                StrokeKind::Inside,
            );
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                t,
                body_font(9.8),
                p.text_subtle,
            );
        }
    });
}

/// Clean modern toggle row with optional subtitle, indented sub-option accent rail, and right-aligned pill switch.
fn modal_toggle_row(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: Option<&str>,
    value: &mut bool,
    enabled: bool,
    indented: bool,
    p: &Palette,
) {
    ui.horizontal(|ui| {
        if indented {
            let (rail_rect, _) = ui.allocate_exact_size(Vec2::new(14.0, 20.0), Sense::hover());
            let rail_x = rail_rect.min.x + 6.0;
            let rail_col = if enabled && *value {
                Palette::alpha(p.brand, 180)
            } else {
                p.border
            };
            ui.painter().line_segment(
                [
                    Pos2::new(rail_x, rail_rect.min.y + 1.0),
                    Pos2::new(rail_x, rail_rect.max.y - 1.0),
                ],
                Stroke::new(2.0_f32, rail_col),
            );
        }

        let title_col = if enabled {
            p.text_main
        } else {
            p.text_subtle
        };
        let sub_col = if enabled {
            p.text_muted
        } else {
            p.text_subtle
        };

        if let Some(sub) = subtitle {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new(title).font(body_font(12.0)).color(title_col));
                ui.label(RichText::new(sub).font(body_font(10.2)).color(sub_col));
            });
        } else {
            ui.label(RichText::new(title).font(body_font(11.5)).color(title_col));
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            toggle_switch(ui, value, enabled, p);
        });
    });
}

/// Perfectly aligned slider row: fixed left label column + right-aligned coral custom slider + monospace value badge.
fn modal_slider_row(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: Option<&str>,
    value: &mut u32,
    min: u32,
    max: u32,
    slider_w: f32,
    p: &Palette,
) {
    let total_w = ui.available_width();
    let right_w = slider_w + 10.0 + 38.0;
    let left_w = (total_w - right_w - 8.0).max(120.0);
    let row_h = if subtitle.is_some() { 28.0 } else { 24.0 };

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let layout = if subtitle.is_some() {
            Layout::top_down(Align::Min)
        } else {
            Layout::left_to_right(Align::Center)
        };
        ui.allocate_ui_with_layout(Vec2::new(left_w, row_h), layout, |ui| {
            ui.set_min_width(left_w);
            ui.set_max_width(left_w);
            if let Some(sub) = subtitle {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new(title).font(body_font(12.0)).color(p.text_main));
                ui.label(RichText::new(sub).font(body_font(10.0)).color(p.text_muted));
            } else {
                ui.label(RichText::new(title).font(body_font(12.0)).color(p.text_main));
            }
        });
        custom_slider_u32(ui, value, min, max, slider_w, p);
    });
}
