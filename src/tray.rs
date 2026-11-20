use crate::icon::make_tray_icon;
use crate::models::{format_speed, GlobalStats, TrayAction, TRAY_POPUP_TITLE, WINDOW_TITLE};
use crate::platform;
use crate::tray_popup::{popup_viewport_id, TrayPopupShared};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub struct TrayController {
    tray_icon: TrayIcon,
    last_active_badge: bool,
    last_tooltip: String,
}

impl TrayController {
    /// Creates the Windows System Tray icon and spawns its event listener.
    /// Left-click & right-click toggle our custom frameless Tray Popup Flyout positioned
    /// right above the taskbar; double-click opens the full VortexDM Manager window.
    pub fn create(
        popup_shared: Arc<TrayPopupShared>,
        egui_ctx: egui::Context,
    ) -> Result<Self, String> {
        let icon = make_tray_icon(false).map_err(|e| format!("Failed creating tray icon: {}", e))?;
        let initial_tooltip = "VortexDM — Ready in System Tray".to_string();

        let tray_icon = TrayIconBuilder::new()
            .with_tooltip(&initial_tooltip)
            .with_icon(icon)
            .build()
            .map_err(|e| format!("Failed building Windows tray icon: {}", e))?;

        let popup_id = popup_viewport_id();

        std::thread::spawn(move || {
            let tray_rx = TrayIconEvent::receiver();
            let mut last_toggle = Instant::now() - Duration::from_secs(5);

            loop {
                while let Ok(ev) = tray_rx.try_recv() {
                    match ev {
                        TrayIconEvent::Click {
                            button: MouseButton::Left | MouseButton::Right,
                            button_state: MouseButtonState::Up,
                            position,
                            ..
                        } => {
                            if last_toggle.elapsed() < Duration::from_millis(160) {
                                continue;
                            }
                            last_toggle = Instant::now();

                            let popup_hwnd = platform::resolve_hwnd(
                                popup_shared.popup_hwnd.load(Ordering::Relaxed),
                                TRAY_POPUP_TITLE,
                            );
                            if popup_hwnd != 0 {
                                popup_shared.popup_hwnd.store(popup_hwnd, Ordering::Relaxed);
                                if platform::is_window_visible(popup_hwnd) {
                                    platform::hide_window_by_hwnd(popup_hwnd);
                                } else {
                                    popup_shared
                                        .autostart_cached
                                        .store(platform::is_windows_autostart_enabled(), Ordering::Relaxed);
                                    popup_shared.had_focus.store(false, Ordering::SeqCst);
                                    popup_shared
                                        .opened_at_ms
                                        .store(platform::now_millis(), Ordering::SeqCst);
                                    platform::show_popup_near_tray(
                                        popup_hwnd,
                                        Some(position.x as i32),
                                        Some(position.y as i32),
                                    );
                                    egui_ctx.request_repaint_of(popup_id);
                                }
                            } else {
                                // Fallback: if popup HWND wasn't resolved yet, show full window
                                let main_hwnd = platform::resolve_hwnd(
                                    popup_shared.main_hwnd.load(Ordering::Relaxed),
                                    WINDOW_TITLE,
                                );
                                platform::show_window_by_hwnd(main_hwnd);
                                if let Ok(tx) = popup_shared.action_tx.lock() {
                                    let _ = tx.send(TrayAction::ShowWindow);
                                }
                                egui_ctx.request_repaint();
                            }
                        }
                        TrayIconEvent::DoubleClick {
                            button: MouseButton::Left,
                            ..
                        } => {
                            popup_shared.hide_popup();
                            let main_hwnd = platform::resolve_hwnd(
                                popup_shared.main_hwnd.load(Ordering::Relaxed),
                                WINDOW_TITLE,
                            );
                            if main_hwnd != 0 {
                                popup_shared.main_hwnd.store(main_hwnd, Ordering::Relaxed);
                                platform::show_window_by_hwnd(main_hwnd);
                            }
                            if let Ok(tx) = popup_shared.action_tx.lock() {
                                let _ = tx.send(TrayAction::ShowWindow);
                            }
                            egui_ctx.request_repaint();
                        }
                        _ => {}
                    }
                }

                std::thread::sleep(Duration::from_millis(30));
            }
        });

        Ok(Self {
            tray_icon,
            last_active_badge: false,
            last_tooltip: initial_tooltip,
        })
    }

    /// Synchronize system tray tooltip and active emerald indicator badge with live downloads.
    pub fn sync_status(&mut self, stats: &GlobalStats) {
        let is_active = stats.active_count > 0;
        let tooltip = if is_active {
            format!(
                "VortexDM — {} active ({} sockets) | {}",
                stats.active_count,
                stats.total_connections,
                format_speed(stats.total_speed_bps)
            )
        } else if stats.paused_count > 0 {
            format!("VortexDM — {} paused | Ready in Tray", stats.paused_count)
        } else {
            "VortexDM — Ready in System Tray".to_string()
        };

        if tooltip != self.last_tooltip {
            let _ = self.tray_icon.set_tooltip(Some(&tooltip));
            self.last_tooltip = tooltip;
        }

        if is_active != self.last_active_badge {
            if let Ok(icon) = make_tray_icon(is_active) {
                let _ = self.tray_icon.set_icon(Some(icon));
                self.last_active_badge = is_active;
            }
        }
    }
}
