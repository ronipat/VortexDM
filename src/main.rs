#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use vortexdm::engine::start_engine;
use vortexdm::icon::make_egui_icon;
use vortexdm::models::{normalize_cli_target, WINDOW_TITLE};
use vortexdm::platform::{self, SingleInstanceGuard};
use vortexdm::storage::AppStorage;
use vortexdm::ui::VortexApp;

fn main() -> eframe::Result<()> {
    // Parse CLI flags: `--minimized` (launch directly in Windows system tray) and optional URL/magnet/.torrent args.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let initial_targets: Vec<String> = args
        .iter()
        .filter_map(|a| normalize_cli_target(a))
        .collect();
    // If launched with a `.torrent`, `magnet:?`, or URL target, always show the window visibly
    let start_minimized =
        initial_targets.is_empty() && args.iter().any(|a| a == "--minimized" || a == "-m");

    let storage = AppStorage::default_app_storage();

    // Enforce single instance on Windows: if VortexDM is already running, forward any opened
    // `.torrent` / `magnet:?` / HTTP targets through the atomic IPC inbox and focus the window.
    let _instance_guard = match SingleInstanceGuard::acquire(WINDOW_TITLE) {
        Some(g) => g,
        None => {
            for target in &initial_targets {
                let _ = storage.enqueue_ipc_target(target);
            }
            let hwnd = platform::resolve_hwnd(0, WINDOW_TITLE);
            platform::show_window_by_hwnd(hwnd);
            return Ok(());
        }
    };

    // Build dedicated multi-threaded Tokio runtime scaled to host cores for smooth concurrent I/O.
    let worker_threads = std::thread::available_parallelism()
        .map(|n| n.get().clamp(4, 12))
        .unwrap_or(6);
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(worker_threads)
            .thread_name("vortexdm-engine")
            .build()
            .expect("Failed to create Tokio runtime"),
    );

    let viewport = egui::ViewportBuilder::default()
        .with_title(WINDOW_TITLE)
        .with_inner_size([960.0, 620.0])
        .with_min_inner_size([820.0, 520.0])
        .with_icon(Arc::new(make_egui_icon()));

    let native_options = eframe::NativeOptions {
        viewport,
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        WINDOW_TITLE,
        native_options,
        Box::new(move |cc| {
            let repaint_ctx = cc.egui_ctx.clone();
            let repaint_cb: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                repaint_ctx.request_repaint();
            });

            let engine = start_engine(runtime.handle(), storage, Some(repaint_cb));

            if !initial_targets.is_empty() {
                engine.push_pending_ui_targets(initial_targets);
            }

            Ok(Box::new(VortexApp::new(
                cc,
                runtime.clone(),
                engine,
                start_minimized,
            )))
        }),
    )
}
