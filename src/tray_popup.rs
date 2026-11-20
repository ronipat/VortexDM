use crate::engine::EngineSnapshot;
use crate::models::{
    format_bytes, format_speed, EngineCommand, TrayAction, TRAY_POPUP_TITLE, WINDOW_TITLE,
};
use crate::platform;
use crate::theme::{
    apply_theme, body_font, bold_font, mono_font, paint_brand_badge, paint_vector_icon, Palette,
    VectorIcon,
};
use egui::{
    Align2, Color32, CornerRadius, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
    Vec2, ViewportBuilder, ViewportId,
};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;

pub const POPUP_WIDTH: f32 = 276.0;
pub const POPUP_HEIGHT: f32 = 354.0;

pub fn popup_viewport_id() -> ViewportId {
    ViewportId::from_hash_of("vortexdm_tray_popup")
}

pub fn popup_viewport_builder() -> ViewportBuilder {
    ViewportBuilder::default()
        .with_title(TRAY_POPUP_TITLE)
        .with_inner_size([POPUP_WIDTH, POPUP_HEIGHT])
        .with_min_inner_size([POPUP_WIDTH, POPUP_HEIGHT])
        .with_max_inner_size([POPUP_WIDTH, POPUP_HEIGHT])
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_visible(false)
}

pub struct TrayPopupShared {
    pub engine_cmd_tx: tokio_mpsc::UnboundedSender<EngineCommand>,
    pub engine_snapshot: Arc<RwLock<EngineSnapshot>>,
    pub action_tx: Mutex<std_mpsc::Sender<TrayAction>>,
    pub main_hwnd: Arc<AtomicIsize>,
    pub popup_hwnd: Arc<AtomicIsize>,
    pub force_quit: Arc<AtomicBool>,
    pub opened_at_ms: Arc<AtomicU64>,
    pub had_focus: AtomicBool,
    pub autostart_cached: AtomicBool,
}

impl TrayPopupShared {
    pub fn hide_popup(&self) {
        let hwnd = platform::resolve_hwnd(self.popup_hwnd.load(Ordering::Relaxed), TRAY_POPUP_TITLE);
        if hwnd != 0 {
            self.popup_hwnd.store(hwnd, Ordering::Relaxed);
            platform::hide_window_by_hwnd(hwnd);
        }
    }

    fn send_action(&self, action: TrayAction) {
        if let Ok(tx) = self.action_tx.lock() {
            let _ = tx.send(action);
        }
    }
}

/// Deferred viewport callback rendering the compact frameless Tray Popup in 9Router's theme.
pub fn render_tray_popup(ctx: &egui::Context, shared: &Arc<TrayPopupShared>) {
    if shared.popup_hwnd.load(Ordering::Relaxed) == 0 {
        let h = platform::resolve_hwnd(0, TRAY_POPUP_TITLE);
        if h != 0 {
            shared.popup_hwnd.store(h, Ordering::SeqCst);
        }
    }

    let popup_hwnd = shared.popup_hwnd.load(Ordering::Relaxed);
    let visible = platform::is_window_visible(popup_hwnd);

    // Auto-dismiss on Escape, OR when the user clicks outside after the popup received focus.
    if visible {
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        if focused {
            shared.had_focus.store(true, Ordering::Relaxed);
        }
        let had_focus = shared.had_focus.load(Ordering::Relaxed);
        let opened_at = shared.opened_at_ms.load(Ordering::Relaxed);
        let elapsed_ms = platform::now_millis().saturating_sub(opened_at);

        if escape || (had_focus && !focused && elapsed_ms > 220) {
            shared.had_focus.store(false, Ordering::Relaxed);
            shared.hide_popup();
            return;
        }

        ctx.request_repaint_after(Duration::from_millis(250));
    }

    let snap = shared.engine_snapshot.read().unwrap().clone();
    let p = Palette::for_mode(snap.settings.dark_mode);
    apply_theme(ctx, &p);

    if popup_hwnd != 0 {
        platform::apply_popup_window_chrome(popup_hwnd, snap.settings.dark_mode);
    }

    // Fill edge-to-edge with `p.bg` so no transparent/black swapchain pixels ever bleed at the
    // 4 edges or corners; native Windows 11 DWM (`DWMWCP_ROUND` + `DWMWA_BORDER_COLOR`) clips the
    // rounded outer frame and draws the crisp themed border & drop shadow.
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(p.bg))
        .show(ctx, |ui| {
            let outer_rect = ui.max_rect();
            let inner_rect = outer_rect.shrink(11.0);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner_rect));
            child.spacing_mut().item_spacing.y = 6.0;

            render_popup_contents(&mut child, ctx, shared, &snap, &p);
        });
}

fn render_popup_contents(
    ui: &mut Ui,
    ctx: &egui::Context,
    shared: &Arc<TrayPopupShared>,
    snap: &EngineSnapshot,
    p: &Palette,
) {
    let lang = snap.settings.language;
    let is_active = snap.stats.active_count > 0;

    // --- 1. COMPACT HEADER ROW ---
    ui.horizontal(|ui| {
        let (badge_rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
        paint_brand_badge(ui.painter(), badge_rect, p);
        ui.add_space(2.0);

        ui.label(
            RichText::new("VortexDM")
                .font(bold_font(13.5))
                .color(p.text_main),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let status_txt = if is_active {
                lang.tr("Live", "Aktif")
            } else {
                lang.tr("Ready", "Siap")
            };
            let (pill_rect, _) = ui.allocate_exact_size(Vec2::new(58.0, 22.0), Sense::hover());
            ui.painter()
                .rect_filled(pill_rect, CornerRadius::same(99), p.surface);
            ui.painter().rect_stroke(
                pill_rect,
                CornerRadius::same(99),
                Stroke::new(
                    1.0_f32,
                    if is_active {
                        Palette::alpha(p.success, 110)
                    } else {
                        p.border
                    },
                ),
                StrokeKind::Inside,
            );
            ui.painter().text(
                Pos2::new(pill_rect.min.x + 11.0, pill_rect.center().y),
                Align2::LEFT_CENTER,
                status_txt,
                body_font(10.5),
                p.text_main,
            );
            ui.painter().circle_filled(
                Pos2::new(pill_rect.max.x - 10.0, pill_rect.center().y),
                3.4,
                p.success,
            );
        });
    });

    // --- 2. COMPACT TELEMETRY CARD ---
    egui::Frame::new()
        .fill(p.surface)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0_f32, p.border))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;

            ui.columns(2, |cols| {
                cols[0].spacing_mut().item_spacing.y = 1.0;
                cols[0].label(
                    RichText::new(lang.tr("SPEED (DL)", "KECEPATAN"))
                        .font(bold_font(9.0))
                        .color(p.text_muted),
                );
                cols[0].label(
                    RichText::new(format_speed(snap.stats.total_speed_bps))
                        .font(bold_font(14.5))
                        .color(if is_active { p.success } else { p.brand }),
                );

                cols[1].spacing_mut().item_spacing.y = 1.0;
                cols[1].label(
                    RichText::new(lang.tr("ACTIVE / SOCKETS", "AKTIF / SOKET"))
                        .font(bold_font(9.0))
                        .color(p.text_muted),
                );
                cols[1].label(
                    RichText::new(format!(
                        "{} / {}",
                        snap.stats.active_count, snap.stats.total_connections
                    ))
                    .font(bold_font(14.5))
                    .color(p.info),
                );
            });

            let (div_rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), Sense::hover());
            ui.painter().line_segment(
                [
                    Pos2::new(div_rect.min.x, div_rect.center().y),
                    Pos2::new(div_rect.max.x, div_rect.center().y),
                ],
                Stroke::new(1.0_f32, p.border_subtle),
            );

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        lang.tr("Session:", "Sesi:"),
                        format_bytes(snap.stats.session_downloaded_bytes)
                    ))
                    .font(mono_font(10.0))
                    .color(p.text_muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let right_text = if snap.stats.total_upload_speed_bps > 1.0 {
                        format!("UL {}", format_speed(snap.stats.total_upload_speed_bps))
                    } else if snap.stats.paused_count > 0 {
                        format!(
                            "{} {} • {} {}",
                            snap.stats.completed_count,
                            lang.tr("Done", "Selesai"),
                            snap.stats.paused_count,
                            lang.tr("Paused", "Dijeda")
                        )
                    } else {
                        format!(
                            "{} {}",
                            snap.stats.completed_count,
                            lang.tr("Done", "Selesai")
                        )
                    };
                    ui.label(
                        RichText::new(right_text)
                            .font(mono_font(10.0))
                            .color(if snap.stats.total_upload_speed_bps > 1.0 {
                                p.teal
                            } else {
                                p.text_subtle
                            }),
                    );
                });
            });
        });

    // --- 3. OPEN FULL MANAGER ROW CARD ---
    if action_row_card(
        ui,
        VectorIcon::Monitor,
        p.brand,
        lang.tr("Open Full Manager", "Buka Manajer Utama"),
        p.text_main,
        Some(VectorIcon::ChevronRight),
        p.brand,
        p.brand,
        p,
    )
    .clicked()
    {
        shared.hide_popup();
        let main_hwnd =
            platform::resolve_hwnd(shared.main_hwnd.load(Ordering::Relaxed), WINDOW_TITLE);
        if main_hwnd != 0 {
            shared.main_hwnd.store(main_hwnd, Ordering::Relaxed);
            platform::show_window_by_hwnd(main_hwnd);
        }
        shared.send_action(TrayAction::ShowWindow);
        ctx.request_repaint_of(ViewportId::ROOT);
    }

    // --- 4. QUICK CONTROLS SEGMENTED CARD ---
    egui::Frame::new()
        .fill(p.surface)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0_f32, p.border))
        .inner_margin(Margin::symmetric(7, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.columns(3, |cols| {
                if segmented_pill_btn(
                    &mut cols[0],
                    lang.tr("+ New", "+ Baru"),
                    false,
                    Some(p.brand),
                    p,
                )
                .clicked()
                {
                    shared.hide_popup();
                    let main_hwnd = platform::resolve_hwnd(
                        shared.main_hwnd.load(Ordering::Relaxed),
                        WINDOW_TITLE,
                    );
                    if main_hwnd != 0 {
                        shared.main_hwnd.store(main_hwnd, Ordering::Relaxed);
                        platform::show_window_by_hwnd(main_hwnd);
                    }
                    shared.send_action(TrayAction::NewDownload);
                    ctx.request_repaint_of(ViewportId::ROOT);
                }

                if segmented_pill_btn(&mut cols[1], lang.tr("Pause", "Jeda"), false, None, p)
                    .clicked()
                {
                    let _ = shared.engine_cmd_tx.send(EngineCommand::PauseAll);
                }

                if segmented_pill_btn(&mut cols[2], lang.tr("Resume", "Lanjut"), false, None, p)
                    .clicked()
                {
                    let _ = shared.engine_cmd_tx.send(EngineCommand::ResumeAll);
                }
            });
        });

    // --- 5. THEME SWITCHER CARD ---
    egui::Frame::new()
        .fill(p.surface)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0_f32, p.border))
        .inner_margin(Margin::symmetric(9, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.label(
                RichText::new(lang.tr("Theme", "Tema"))
                    .font(body_font(10.5))
                    .color(p.text_muted),
            );
            ui.columns(2, |cols| {
                let is_dark = snap.settings.dark_mode;
                if segmented_pill_btn(&mut cols[0], lang.tr("Dark", "Gelap"), is_dark, None, p)
                    .clicked()
                    && !is_dark
                {
                    let mut updated = snap.settings.clone();
                    updated.dark_mode = true;
                    let _ = shared
                        .engine_cmd_tx
                        .send(EngineCommand::UpdateSettings(updated));
                    ctx.request_repaint_of(ViewportId::ROOT);
                }
                if segmented_pill_btn(
                    &mut cols[1],
                    lang.tr("Light", "Terang"),
                    !is_dark,
                    None,
                    p,
                )
                .clicked()
                    && is_dark
                {
                    let mut updated = snap.settings.clone();
                    updated.dark_mode = false;
                    let _ = shared
                        .engine_cmd_tx
                        .send(EngineCommand::UpdateSettings(updated));
                    ctx.request_repaint_of(ViewportId::ROOT);
                }
            });
        });

    // --- 6. START WITH WINDOWS TOGGLE CARD ---
    let autostart_on = shared.autostart_cached.load(Ordering::Relaxed);
    if toggle_row_card(
        ui,
        lang.tr("Start with Windows", "Mulai bersama Windows"),
        autostart_on,
        lang.tr("On", "Aktif"),
        lang.tr("Off", "Mati"),
        p,
    )
    .clicked()
    {
        let next = !autostart_on;
        shared.autostart_cached.store(next, Ordering::Relaxed);
        let mut updated = snap.settings.clone();
        updated.start_with_windows = next;
        let _ = shared
            .engine_cmd_tx
            .send(EngineCommand::UpdateSettings(updated));
        ctx.request_repaint_of(ViewportId::ROOT);
    }

    // --- 7. QUIT ROW CARD ---
    if action_row_card(
        ui,
        VectorIcon::CloseX,
        p.danger,
        lang.tr("Quit", "Keluar"),
        p.danger,
        None,
        p.danger,
        p.danger,
        p,
    )
    .clicked()
    {
        shared.hide_popup();
        shared.force_quit.store(true, Ordering::SeqCst);
        let _ = shared.engine_cmd_tx.send(EngineCommand::Shutdown);
        shared.send_action(TrayAction::Quit);
        let main_hwnd =
            platform::resolve_hwnd(shared.main_hwnd.load(Ordering::Relaxed), WINDOW_TITLE);
        platform::post_close_by_hwnd(main_hwnd);
        ctx.request_repaint_of(ViewportId::ROOT);
    }
}

#[allow(clippy::too_many_arguments)]
fn action_row_card(
    ui: &mut Ui,
    left_icon: VectorIcon,
    left_color: Color32,
    label: &str,
    label_color: Color32,
    right_icon: Option<VectorIcon>,
    right_color: Color32,
    hover_accent: Color32,
    p: &Palette,
) -> egui::Response {
    let desired = Vec2::new(ui.available_width(), 33.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());

    let fill = if resp.hovered() { p.surface_2 } else { p.surface };
    let border = if resp.hovered() {
        Palette::alpha(hover_accent, 150)
    } else {
        p.border
    };

    ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(9),
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );

    paint_vector_icon(
        ui.painter(),
        Pos2::new(rect.min.x + 18.0, rect.center().y),
        6.0,
        left_icon,
        left_color,
    );

    ui.painter().text(
        Pos2::new(rect.min.x + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        body_font(12.0),
        label_color,
    );

    if let Some(ric) = right_icon {
        paint_vector_icon(
            ui.painter(),
            Pos2::new(rect.max.x - 14.0, rect.center().y),
            4.5,
            ric,
            right_color,
        );
    }

    resp
}

fn segmented_pill_btn(
    ui: &mut Ui,
    label: &str,
    active: bool,
    accent_text: Option<Color32>,
    p: &Palette,
) -> egui::Response {
    let desired = Vec2::new(ui.available_width(), 24.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());

    let fill = if active {
        p.surface_2
    } else if resp.hovered() {
        Palette::alpha(p.surface_2, 180)
    } else {
        p.bg_alt
    };

    let border = if active {
        Palette::alpha(p.brand, 175)
    } else if resp.hovered() {
        Palette::alpha(p.brand, 110)
    } else {
        p.border
    };

    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(7),
        Stroke::new(if active { 1.2_f32 } else { 1.0_f32 }, border),
        StrokeKind::Inside,
    );

    let fg = if let Some(col) = accent_text {
        col
    } else if active {
        p.text_main
    } else {
        p.text_muted
    };

    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        if active || accent_text.is_some() {
            bold_font(11.0)
        } else {
            body_font(11.0)
        },
        fg,
    );

    resp
}

fn toggle_row_card(
    ui: &mut Ui,
    label: &str,
    checked: bool,
    on_label: &str,
    off_label: &str,
    p: &Palette,
) -> egui::Response {
    let desired = Vec2::new(ui.available_width(), 33.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());

    let fill = if resp.hovered() { p.surface_2 } else { p.surface };
    let border = if resp.hovered() {
        Palette::alpha(p.brand, 145)
    } else {
        p.border
    };

    ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(9),
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );

    let box_rect = Rect::from_center_size(
        Pos2::new(rect.min.x + 18.0, rect.center().y),
        Vec2::splat(15.0),
    );
    if checked {
        ui.painter()
            .rect_filled(box_rect, CornerRadius::same(4), p.brand);
        paint_vector_icon(
            ui.painter(),
            box_rect.center(),
            4.5,
            VectorIcon::Check,
            Color32::WHITE,
        );
    } else {
        ui.painter().rect_stroke(
            box_rect,
            CornerRadius::same(4),
            Stroke::new(1.4_f32, p.text_subtle),
            StrokeKind::Inside,
        );
    }

    ui.painter().text(
        Pos2::new(rect.min.x + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        body_font(12.0),
        p.text_main,
    );

    ui.painter().text(
        Pos2::new(rect.max.x - 13.0, rect.center().y),
        Align2::RIGHT_CENTER,
        if checked { on_label } else { off_label },
        if checked { bold_font(11.0) } else { body_font(11.0) },
        if checked { p.success } else { p.text_muted },
    );

    resp
}
