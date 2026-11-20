use crate::models::{DownloadStatus, FileCategory};
use egui::{
    Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Painter,
    Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use std::sync::Arc;

/// Exact 9Router-inspired Dual Theme Palette (Warm Charcoal Dark + Warm Alabaster Light).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub bg_alt: Color32,
    pub surface: Color32,
    pub surface_2: Color32,
    pub border: Color32,
    pub border_subtle: Color32,
    pub text_main: Color32,
    pub text_muted: Color32,
    pub text_subtle: Color32,
    pub grid_line: Color32,

    // Signature terracotta / coral brand ramp (9Router)
    pub brand: Color32,
    pub brand_hover: Color32,
    pub brand_deep: Color32,
    pub brand_soft: Color32,

    // Semantic accents
    pub success: Color32,
    pub info: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub purple: Color32,
    pub pink: Color32,
    pub sky: Color32,
    pub teal: Color32,
}

impl Palette {
    pub const DARK: Self = Self {
        dark: true,
        bg: Color32::from_rgb(26, 26, 26),        // #1A1A1A
        bg_alt: Color32::from_rgb(31, 31, 30),    // #1F1F1E
        surface: Color32::from_rgb(38, 38, 38),   // #262626
        surface_2: Color32::from_rgb(48, 48, 48), // #303030
        border: Color32::from_rgb(56, 56, 56),    // #383838 (slightly crisp on desktop)
        border_subtle: Color32::from_rgb(42, 42, 42), // #2A2A2A
        text_main: Color32::from_rgb(237, 237, 237),  // #EDEDED
        text_muted: Color32::from_rgb(156, 163, 175), // #9CA3AF
        text_subtle: Color32::from_rgb(107, 114, 128), // #6B7280
        grid_line: Color32::from_rgb(36, 31, 29), // #1A1A1A + ~4.5% #E56A4A

        brand: Color32::from_rgb(229, 106, 74),       // #E56A4A
        brand_hover: Color32::from_rgb(204, 82, 54),  // #CC5236
        brand_deep: Color32::from_rgb(166, 64, 39),   // #A64027
        brand_soft: Color32::from_rgb(238, 141, 106), // #EE8D6A

        success: Color32::from_rgb(34, 197, 94),  // #22C55E
        info: Color32::from_rgb(59, 130, 246),    // #3B82F6
        warning: Color32::from_rgb(245, 158, 11), // #F59E0B
        danger: Color32::from_rgb(239, 68, 68),   // #EF4444
        purple: Color32::from_rgb(168, 85, 247),  // #A855F7
        pink: Color32::from_rgb(236, 72, 153),    // #EC4899
        sky: Color32::from_rgb(14, 165, 233),     // #0EA5E9
        teal: Color32::from_rgb(20, 184, 166),    // #14B8A6
    };

    pub const LIGHT: Self = Self {
        dark: false,
        bg: Color32::from_rgb(252, 251, 249),        // #FCFBF9 warm paper
        bg_alt: Color32::from_rgb(248, 250, 252),    // #F8FAFC
        surface: Color32::from_rgb(255, 255, 255),   // #FFFFFF
        surface_2: Color32::from_rgb(244, 244, 245), // #F4F4F5
        border: Color32::from_rgb(229, 231, 235),    // #E5E7EB
        border_subtle: Color32::from_rgb(241, 241, 243), // #F1F1F3
        text_main: Color32::from_rgb(24, 24, 27),    // #18181B crisp soft ink (less harsh than pure black)
        text_muted: Color32::from_rgb(107, 114, 128), // #6B7280
        text_subtle: Color32::from_rgb(156, 163, 175), // #9CA3AF
        grid_line: Color32::from_rgb(249, 240, 235), // #FCFBF9 + ~7.5% #E68A6E

        brand: Color32::from_rgb(229, 106, 74),
        brand_hover: Color32::from_rgb(204, 82, 54),
        brand_deep: Color32::from_rgb(166, 64, 39),
        brand_soft: Color32::from_rgb(238, 141, 106),

        success: Color32::from_rgb(22, 163, 74),  // slightly richer on light paper
        info: Color32::from_rgb(37, 99, 235),
        warning: Color32::from_rgb(217, 119, 6),
        danger: Color32::from_rgb(220, 38, 38),
        purple: Color32::from_rgb(147, 51, 234),
        pink: Color32::from_rgb(219, 39, 119),
        sky: Color32::from_rgb(2, 132, 199),
        teal: Color32::from_rgb(13, 148, 136),
    };

    pub fn for_mode(dark: bool) -> Self {
        if dark {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }

    pub fn alpha(color: Color32, a: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
    }

    pub fn status_color(&self, status: DownloadStatus) -> Color32 {
        match status {
            DownloadStatus::Downloading => self.brand,
            DownloadStatus::Probing | DownloadStatus::Verifying => self.info,
            DownloadStatus::Queued | DownloadStatus::Paused => self.warning,
            DownloadStatus::Completed => self.success,
            DownloadStatus::Failed | DownloadStatus::Cancelled => self.danger,
        }
    }

    pub fn category_color(&self, cat: FileCategory) -> Color32 {
        match cat {
            FileCategory::Programs => self.brand,
            FileCategory::Archives => self.warning,
            FileCategory::Videos => self.purple,
            FileCategory::Music => self.pink,
            FileCategory::Documents => self.info,
            FileCategory::Images => self.sky,
            FileCategory::Torrents => self.teal,
            FileCategory::Other => self.success,
        }
    }
}

pub fn bold_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn body_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn mono_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Load crisp native Windows system fonts (Segoe UI Regular/Semibold + Consolas) with safe fallback.
pub fn install_windows_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let mut has_regular = false;
    let mut has_emphasis = false;

    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        fonts.font_data.insert(
            "segoe_ui".to_owned(),
            Arc::new(FontData::from_owned(bytes)),
        );
        if let Some(fam) = fonts.families.get_mut(&FontFamily::Proportional) {
            fam.insert(0, "segoe_ui".to_owned());
        }
        has_regular = true;
    }

    // Prefer Segoe UI Semibold (weight 600) over heavy Bold (weight 700) so headings, tabs,
    // filenames, and status pills render light, crisp, and refined at desktop UI sizes.
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\seguisb.ttf")
        .or_else(|_| std::fs::read(r"C:\Windows\Fonts\segoeuib.ttf"))
    {
        fonts.font_data.insert(
            "segoe_ui_semibold".to_owned(),
            Arc::new(FontData::from_owned(bytes)),
        );
        has_emphasis = true;
    }

    // Register dedicated "bold" family backed by Segoe UI Semibold for crisp headings, tabs, and pills
    let mut bold_chain = Vec::new();
    if has_emphasis {
        bold_chain.push("segoe_ui_semibold".to_owned());
    }
    if has_regular {
        bold_chain.push("segoe_ui".to_owned());
    }
    if let Some(default_prop) = fonts.families.get(&FontFamily::Proportional) {
        for name in default_prop {
            if !bold_chain.contains(name) {
                bold_chain.push(name.clone());
            }
        }
    }
    fonts
        .families
        .insert(FontFamily::Name("bold".into()), bold_chain);

    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\consola.ttf") {
        fonts.font_data.insert(
            "consolas".to_owned(),
            Arc::new(FontData::from_owned(bytes)),
        );
        if let Some(fam) = fonts.families.get_mut(&FontFamily::Monospace) {
            fam.insert(0, "consolas".to_owned());
        }
    }

    ctx.set_fonts(fonts);
}

/// Apply 9Router visual tokens to egui's global visuals and spacing.
pub fn apply_theme(ctx: &egui::Context, p: &Palette) {
    let mut visuals = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    visuals.window_fill = p.surface;
    visuals.panel_fill = p.bg;
    visuals.extreme_bg_color = p.bg_alt;
    visuals.faint_bg_color = p.surface_2;
    visuals.override_text_color = Some(p.text_main);

    visuals.window_corner_radius = CornerRadius::same(14);
    visuals.window_stroke = Stroke::new(1.0_f32, p.border);
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 130 } else { 32 }),
    };

    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 120 } else { 28 }),
    };

    visuals.widgets.noninteractive.bg_fill = p.surface;
    visuals.widgets.noninteractive.weak_bg_fill = p.surface;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text_main);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(8);

    visuals.widgets.inactive.bg_fill = p.surface;
    visuals.widgets.inactive.weak_bg_fill = p.surface_2;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text_main);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(8);

    visuals.widgets.hovered.bg_fill = p.surface_2;
    visuals.widgets.hovered.weak_bg_fill = p.surface_2;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Palette::alpha(p.brand, 140));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.text_main);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(8);

    visuals.widgets.active.bg_fill = p.surface_2;
    visuals.widgets.active.weak_bg_fill = p.surface_2;
    visuals.widgets.active.bg_stroke = Stroke::new(1.2_f32, p.brand);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, p.text_main);
    visuals.widgets.active.corner_radius = CornerRadius::same(8);

    visuals.selection.bg_fill = Palette::alpha(p.brand, 68);
    visuals.selection.stroke = Stroke::new(1.2_f32, p.brand);
    visuals.hyperlink_color = p.brand;

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style.spacing.text_edit_width = 280.0;
    style.interaction.tooltip_delay = 0.25;
    ctx.set_style(style);
}

/// Paint 9Router's signature 40px x 40px architectural blueprint grid across `rect`.
pub fn paint_architectural_grid(painter: &Painter, rect: Rect, p: &Palette) {
    let step = 40.0_f32;
    let stroke = Stroke::new(1.0_f32, p.grid_line);

    let mut x = (rect.min.x / step).ceil() * step;
    while x < rect.max.x {
        painter.line_segment([Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)], stroke);
        x += step;
    }

    let mut y = (rect.min.y / step).ceil() * step;
    while y < rect.max.y {
        painter.line_segment([Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)], stroke);
        y += step;
    }
}

/// Paint the signature 9Router-style terracotta-coral brand emblem with crisp white arrow + nodes.
pub fn paint_brand_badge(painter: &Painter, rect: Rect, p: &Palette) {
    // Subtle warm glow underneath
    painter.rect_filled(
        rect.expand(1.5),
        CornerRadius::same(9),
        Palette::alpha(p.brand, 42),
    );
    // Primary coral rounded tile
    painter.rect_filled(rect, CornerRadius::same(8), p.brand);
    // Subtle lower-half depth tint
    let lower = Rect::from_min_max(
        Pos2::new(rect.min.x, rect.center().y),
        rect.max,
    );
    painter.rect_filled(
        lower,
        CornerRadius {
            nw: 0,
            ne: 0,
            sw: 8,
            se: 8,
        },
        Palette::alpha(p.brand_hover, 140),
    );
    // Crisp top specular highlight
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 42)),
        StrokeKind::Inside,
    );

    let c = rect.center();
    let s = rect.width() * 0.30;
    let white = Color32::WHITE;
    let st = Stroke::new(2.0_f32, white);

    // Downward arrow shaft + chevron + tray bar + twin side nodes
    painter.line_segment([Pos2::new(c.x, c.y - s * 0.95), Pos2::new(c.x, c.y + s * 0.45)], st);
    painter.line_segment(
        [
            Pos2::new(c.x - s * 0.55, c.y + s * 0.02),
            Pos2::new(c.x, c.y + s * 0.55),
        ],
        st,
    );
    painter.line_segment(
        [
            Pos2::new(c.x + s * 0.55, c.y + s * 0.02),
            Pos2::new(c.x, c.y + s * 0.55),
        ],
        st,
    );
    painter.line_segment(
        [
            Pos2::new(c.x - s * 0.85, c.y + s * 0.95),
            Pos2::new(c.x + s * 0.85, c.y + s * 0.95),
        ],
        Stroke::new(2.0_f32, white),
    );
    painter.circle_filled(Pos2::new(c.x - s * 0.85, c.y - s * 0.32), 2.1, white);
    painter.circle_filled(Pos2::new(c.x + s * 0.85, c.y - s * 0.32), 2.1, white);
}

#[derive(Debug, Clone, Copy)]
pub enum VectorIcon {
    Plus,
    Pause,
    Play,
    Clear,
    Gear,
    Tray,
    Sun,
    Moon,
    Folder,
    OpenExternal,
    Trash,
    Inspect,
    Shield,
    Speed,
    Link,
    Monitor,
    ChevronRight,
    Check,
    CloseX,
    Magnet,
}

/// Crisp Material-Outlined-style vector glyph painted via egui::Painter (zero missing emoji).
pub fn paint_vector_icon(painter: &Painter, center: Pos2, radius: f32, icon: VectorIcon, color: Color32) {
    let st = Stroke::new(1.6_f32, color);
    let r = radius;

    match icon {
        VectorIcon::Plus => {
            painter.line_segment([Pos2::new(center.x - r, center.y), Pos2::new(center.x + r, center.y)], Stroke::new(1.9_f32, color));
            painter.line_segment([Pos2::new(center.x, center.y - r), Pos2::new(center.x, center.y + r)], Stroke::new(1.9_f32, color));
        }
        VectorIcon::Pause => {
            let x_off = r * 0.38;
            painter.line_segment([Pos2::new(center.x - x_off, center.y - r * 0.78), Pos2::new(center.x - x_off, center.y + r * 0.78)], Stroke::new(2.0_f32, color));
            painter.line_segment([Pos2::new(center.x + x_off, center.y - r * 0.78), Pos2::new(center.x + x_off, center.y + r * 0.78)], Stroke::new(2.0_f32, color));
        }
        VectorIcon::Play => {
            let pts = vec![
                Pos2::new(center.x - r * 0.55, center.y - r * 0.78),
                Pos2::new(center.x + r * 0.80, center.y),
                Pos2::new(center.x - r * 0.55, center.y + r * 0.78),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        VectorIcon::Clear => {
            // Check-lines clear icon
            painter.line_segment([Pos2::new(center.x - r * 0.8, center.y - r * 0.5), Pos2::new(center.x + r * 0.8, center.y - r * 0.5)], st);
            painter.line_segment([Pos2::new(center.x - r * 0.8, center.y), Pos2::new(center.x + r * 0.4, center.y)], st);
            painter.line_segment([Pos2::new(center.x - r * 0.8, center.y + r * 0.5), Pos2::new(center.x + r * 0.1, center.y + r * 0.5)], st);
            painter.line_segment([Pos2::new(center.x + r * 0.35, center.y + r * 0.25), Pos2::new(center.x + r * 0.85, center.y + r * 0.75)], st);
            painter.line_segment([Pos2::new(center.x + r * 0.85, center.y + r * 0.25), Pos2::new(center.x + r * 0.35, center.y + r * 0.75)], st);
        }
        VectorIcon::Gear => {
            painter.circle_stroke(center, r * 0.42, st);
            painter.circle_stroke(center, r * 0.78, Stroke::new(1.2_f32, Palette::alpha(color, 180)));
            for i in 0..6 {
                let ang = (i as f32) * std::f32::consts::TAU / 6.0;
                let p0 = Pos2::new(center.x + ang.cos() * r * 0.72, center.y + ang.sin() * r * 0.72);
                let p1 = Pos2::new(center.x + ang.cos() * r * 1.05, center.y + ang.sin() * r * 1.05);
                painter.line_segment([p0, p1], Stroke::new(1.8_f32, color));
            }
        }
        VectorIcon::Tray => {
            // Down arrow into tray U-bracket
            painter.line_segment([Pos2::new(center.x, center.y - r * 0.85), Pos2::new(center.x, center.y + r * 0.28)], st);
            painter.line_segment([Pos2::new(center.x - r * 0.42, center.y - r * 0.10), Pos2::new(center.x, center.y + r * 0.32)], st);
            painter.line_segment([Pos2::new(center.x + r * 0.42, center.y - r * 0.10), Pos2::new(center.x, center.y + r * 0.32)], st);
            let pts = vec![
                Pos2::new(center.x - r * 0.82, center.y + r * 0.35),
                Pos2::new(center.x - r * 0.82, center.y + r * 0.82),
                Pos2::new(center.x + r * 0.82, center.y + r * 0.82),
                Pos2::new(center.x + r * 0.82, center.y + r * 0.35),
            ];
            painter.add(Shape::line(pts, st));
        }
        VectorIcon::Sun => {
            painter.circle_stroke(center, r * 0.45, st);
            for i in 0..8 {
                let ang = (i as f32) * std::f32::consts::TAU / 8.0;
                let p0 = Pos2::new(center.x + ang.cos() * r * 0.72, center.y + ang.sin() * r * 0.72);
                let p1 = Pos2::new(center.x + ang.cos() * r * 1.02, center.y + ang.sin() * r * 1.02);
                painter.line_segment([p0, p1], Stroke::new(1.5_f32, color));
            }
        }
        VectorIcon::Moon => {
            // Crisp crescent moon via arc polyline
            let mut pts = Vec::new();
            for i in 0..=16 {
                let ang = 0.65 + (i as f32 / 16.0) * (std::f32::consts::TAU * 0.68);
                pts.push(Pos2::new(center.x + ang.cos() * r * 0.82, center.y + ang.sin() * r * 0.82));
            }
            painter.add(Shape::line(pts, Stroke::new(1.8_f32, color)));
            // Small inner star dot
            painter.circle_filled(Pos2::new(center.x + r * 0.35, center.y - r * 0.35), 1.4, color);
        }
        VectorIcon::Folder => {
            let box_rect = Rect::from_min_max(
                Pos2::new(center.x - r * 0.85, center.y - r * 0.45),
                Pos2::new(center.x + r * 0.85, center.y + r * 0.72),
            );
            painter.rect_stroke(box_rect, CornerRadius::same(2), st, StrokeKind::Middle);
            painter.line_segment(
                [
                    Pos2::new(center.x - r * 0.85, center.y - r * 0.45),
                    Pos2::new(center.x - r * 0.20, center.y - r * 0.75),
                ],
                st,
            );
            painter.line_segment(
                [
                    Pos2::new(center.x - r * 0.20, center.y - r * 0.75),
                    Pos2::new(center.x + r * 0.05, center.y - r * 0.45),
                ],
                st,
            );
        }
        VectorIcon::OpenExternal => {
            let box_rect = Rect::from_min_max(
                Pos2::new(center.x - r * 0.78, center.y - r * 0.35),
                Pos2::new(center.x + r * 0.35, center.y + r * 0.78),
            );
            painter.rect_stroke(box_rect, CornerRadius::same(2), st, StrokeKind::Middle);
            painter.line_segment(
                [
                    Pos2::new(center.x - r * 0.05, center.y + r * 0.05),
                    Pos2::new(center.x + r * 0.82, center.y - r * 0.82),
                ],
                st,
            );
            painter.line_segment(
                [
                    Pos2::new(center.x + r * 0.28, center.y - r * 0.82),
                    Pos2::new(center.x + r * 0.82, center.y - r * 0.82),
                ],
                st,
            );
            painter.line_segment(
                [
                    Pos2::new(center.x + r * 0.82, center.y - r * 0.28),
                    Pos2::new(center.x + r * 0.82, center.y - r * 0.82),
                ],
                st,
            );
        }
        VectorIcon::Trash => {
            painter.line_segment([Pos2::new(center.x - r * 0.78, center.y - r * 0.55), Pos2::new(center.x + r * 0.78, center.y - r * 0.55)], st);
            painter.line_segment([Pos2::new(center.x - r * 0.28, center.y - r * 0.82), Pos2::new(center.x + r * 0.28, center.y - r * 0.82)], st);
            let body = Rect::from_min_max(
                Pos2::new(center.x - r * 0.58, center.y - r * 0.55),
                Pos2::new(center.x + r * 0.58, center.y + r * 0.82),
            );
            painter.rect_stroke(body, CornerRadius::same(2), st, StrokeKind::Middle);
        }
        VectorIcon::Inspect => {
            // Sliders / bars icon
            painter.line_segment([Pos2::new(center.x - r * 0.75, center.y - r * 0.45), Pos2::new(center.x + r * 0.75, center.y - r * 0.45)], st);
            painter.line_segment([Pos2::new(center.x - r * 0.75, center.y + r * 0.45), Pos2::new(center.x + r * 0.75, center.y + r * 0.45)], st);
            painter.circle_filled(Pos2::new(center.x - r * 0.25, center.y - r * 0.45), 2.3, color);
            painter.circle_filled(Pos2::new(center.x + r * 0.30, center.y + r * 0.45), 2.3, color);
        }
        VectorIcon::Shield => {
            let pts = vec![
                Pos2::new(center.x, center.y - r * 0.88),
                Pos2::new(center.x + r * 0.72, center.y - r * 0.50),
                Pos2::new(center.x + r * 0.60, center.y + r * 0.35),
                Pos2::new(center.x, center.y + r * 0.88),
                Pos2::new(center.x - r * 0.60, center.y + r * 0.35),
                Pos2::new(center.x - r * 0.72, center.y - r * 0.50),
                Pos2::new(center.x, center.y - r * 0.88),
            ];
            painter.add(Shape::line(pts, st));
        }
        VectorIcon::Speed => {
            // Speed gauge arc + needle
            let mut pts = Vec::new();
            for i in 0..=12 {
                let ang = std::f32::consts::PI + (i as f32 / 12.0) * std::f32::consts::PI;
                pts.push(Pos2::new(center.x + ang.cos() * r * 0.85, center.y + r * 0.35 + ang.sin() * r * 0.85));
            }
            painter.add(Shape::line(pts, st));
            painter.line_segment(
                [
                    Pos2::new(center.x, center.y + r * 0.35),
                    Pos2::new(center.x + r * 0.55, center.y - r * 0.25),
                ],
                Stroke::new(1.8_f32, color),
            );
        }
        VectorIcon::Link => {
            painter.circle_stroke(Pos2::new(center.x - r * 0.34, center.y), r * 0.42, st);
            painter.circle_stroke(Pos2::new(center.x + r * 0.34, center.y), r * 0.42, st);
        }
        VectorIcon::Monitor => {
            let screen = Rect::from_min_max(
                Pos2::new(center.x - r * 0.92, center.y - r * 0.78),
                Pos2::new(center.x + r * 0.92, center.y + r * 0.36),
            );
            painter.rect_stroke(screen, CornerRadius::same(3), Stroke::new(1.75_f32, color), StrokeKind::Middle);
            painter.line_segment(
                [Pos2::new(center.x, center.y + r * 0.36), Pos2::new(center.x, center.y + r * 0.78)],
                Stroke::new(1.75_f32, color),
            );
            painter.line_segment(
                [Pos2::new(center.x - r * 0.48, center.y + r * 0.80), Pos2::new(center.x + r * 0.48, center.y + r * 0.80)],
                Stroke::new(1.75_f32, color),
            );
        }
        VectorIcon::ChevronRight => {
            let s = Stroke::new(1.85_f32, color);
            painter.line_segment(
                [Pos2::new(center.x - r * 0.32, center.y - r * 0.62), Pos2::new(center.x + r * 0.38, center.y)],
                s,
            );
            painter.line_segment(
                [Pos2::new(center.x + r * 0.38, center.y), Pos2::new(center.x - r * 0.32, center.y + r * 0.62)],
                s,
            );
        }
        VectorIcon::Check => {
            let s = Stroke::new(2.0_f32, color);
            painter.line_segment(
                [Pos2::new(center.x - r * 0.65, center.y + r * 0.05), Pos2::new(center.x - r * 0.12, center.y + r * 0.56)],
                s,
            );
            painter.line_segment(
                [Pos2::new(center.x - r * 0.12, center.y + r * 0.56), Pos2::new(center.x + r * 0.72, center.y - r * 0.52)],
                s,
            );
        }
        VectorIcon::CloseX => {
            let s = Stroke::new(1.9_f32, color);
            painter.line_segment(
                [Pos2::new(center.x - r * 0.68, center.y - r * 0.68), Pos2::new(center.x + r * 0.68, center.y + r * 0.68)],
                s,
            );
            painter.line_segment(
                [Pos2::new(center.x + r * 0.68, center.y - r * 0.68), Pos2::new(center.x - r * 0.68, center.y + r * 0.68)],
                s,
            );
        }
        VectorIcon::Magnet => {
            // Crisp U-shaped magnet arc + pole tips
            let s = Stroke::new(1.9_f32, color);
            let mut pts = Vec::new();
            pts.push(Pos2::new(center.x - r * 0.58, center.y - r * 0.72));
            for i in 0..=10 {
                let ang = std::f32::consts::PI - (i as f32 / 10.0) * std::f32::consts::PI;
                pts.push(Pos2::new(
                    center.x + ang.cos() * r * 0.58,
                    center.y + r * 0.10 + ang.sin() * r * 0.60,
                ));
            }
            pts.push(Pos2::new(center.x + r * 0.58, center.y - r * 0.72));
            painter.add(Shape::line(pts, s));
            painter.line_segment(
                [Pos2::new(center.x - r * 0.78, center.y - r * 0.42), Pos2::new(center.x - r * 0.38, center.y - r * 0.42)],
                Stroke::new(1.5_f32, color),
            );
            painter.line_segment(
                [Pos2::new(center.x + r * 0.38, center.y - r * 0.42), Pos2::new(center.x + r * 0.78, center.y - r * 0.42)],
                Stroke::new(1.5_f32, color),
            );
        }
    }
}

/// Paint 9Router's iconic 38x38 tinted 10%-alpha rounded icon square for a file category.
pub fn paint_category_tile(ui: &mut Ui, cat: FileCategory, p: &Palette, size: f32) {
    let color = p.category_color(cat);
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(
        rect,
        CornerRadius::same(9),
        Palette::alpha(color, if p.dark { 28 } else { 22 }),
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(9),
        Stroke::new(1.0_f32, Palette::alpha(color, if p.dark { 62 } else { 52 })),
        StrokeKind::Inside,
    );

    let c = rect.center();
    let r = size * 0.24;
    let st = Stroke::new(1.65_f32, color);

    match cat {
        FileCategory::Programs => {
            // App window / terminal frame
            let frame = Rect::from_center_size(c, Vec2::splat(r * 1.9));
            painter.rect_stroke(frame, CornerRadius::same(3), st, StrokeKind::Middle);
            painter.line_segment(
                [
                    Pos2::new(frame.min.x, frame.min.y + r * 0.55),
                    Pos2::new(frame.max.x, frame.min.y + r * 0.55),
                ],
                st,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x - r * 0.35, c.y + r * 0.15),
                    Pos2::new(c.x - r * 0.05, c.y + r * 0.38),
                ],
                st,
            );
            painter.line_segment(
                [
                    Pos2::new(c.x - r * 0.05, c.y + r * 0.38),
                    Pos2::new(c.x - r * 0.35, c.y + r * 0.60),
                ],
                st,
            );
        }
        FileCategory::Archives => {
            // Archive package box + zipper spine
            let frame = Rect::from_center_size(c, Vec2::splat(r * 1.9));
            painter.rect_stroke(frame, CornerRadius::same(3), st, StrokeKind::Middle);
            painter.line_segment([Pos2::new(c.x, frame.min.y), Pos2::new(c.x, frame.max.y)], st);
            painter.line_segment([Pos2::new(c.x - r * 0.3, c.y - r * 0.35), Pos2::new(c.x + r * 0.3, c.y - r * 0.35)], st);
            painter.line_segment([Pos2::new(c.x - r * 0.3, c.y + r * 0.15), Pos2::new(c.x + r * 0.3, c.y + r * 0.15)], st);
        }
        FileCategory::Videos => {
            // Play triangle inside circle
            painter.circle_stroke(c, r * 0.98, st);
            let tri = vec![
                Pos2::new(c.x - r * 0.30, c.y - r * 0.45),
                Pos2::new(c.x + r * 0.50, c.y),
                Pos2::new(c.x - r * 0.30, c.y + r * 0.45),
            ];
            painter.add(Shape::convex_polygon(tri, color, Stroke::NONE));
        }
        FileCategory::Music => {
            // Audio waveform bars
            let heights = [0.42_f32, 0.88, 0.58, 0.96, 0.48];
            let spacing = r * 0.46;
            let start_x = c.x - spacing * 2.0;
            for (i, h) in heights.iter().enumerate() {
                let x = start_x + (i as f32) * spacing;
                painter.line_segment(
                    [Pos2::new(x, c.y - r * h), Pos2::new(x, c.y + r * h)],
                    Stroke::new(2.0_f32, color),
                );
            }
        }
        FileCategory::Documents => {
            // Page with 3 clean text lines
            let frame = Rect::from_center_size(c, Vec2::new(r * 1.55, r * 1.95));
            painter.rect_stroke(frame, CornerRadius::same(2), st, StrokeKind::Middle);
            painter.line_segment([Pos2::new(c.x - r * 0.45, c.y - r * 0.40), Pos2::new(c.x + r * 0.45, c.y - r * 0.40)], st);
            painter.line_segment([Pos2::new(c.x - r * 0.45, c.y), Pos2::new(c.x + r * 0.45, c.y)], st);
            painter.line_segment([Pos2::new(c.x - r * 0.45, c.y + r * 0.40), Pos2::new(c.x + r * 0.18, c.y + r * 0.40)], st);
        }
        FileCategory::Images => {
            // Landscape frame
            let frame = Rect::from_center_size(c, Vec2::new(r * 1.95, r * 1.65));
            painter.rect_stroke(frame, CornerRadius::same(3), st, StrokeKind::Middle);
            painter.circle_filled(Pos2::new(c.x - r * 0.45, c.y - r * 0.30), 1.8, color);
            let pts = vec![
                Pos2::new(frame.min.x + 2.0, frame.max.y - 2.0),
                Pos2::new(c.x + r * 0.10, c.y - r * 0.05),
                Pos2::new(frame.max.x - 2.0, frame.max.y - 2.0),
            ];
            painter.add(Shape::line(pts, st));
        }
        FileCategory::Torrents => {
            // P2P Swarm connected nodes (matching 9Router's connected-nodes visual language!)
            let n0 = Pos2::new(c.x, c.y - r * 0.72);
            let n1 = Pos2::new(c.x - r * 0.74, c.y + r * 0.56);
            let n2 = Pos2::new(c.x + r * 0.74, c.y + r * 0.56);
            painter.line_segment([c, n0], st);
            painter.line_segment([c, n1], st);
            painter.line_segment([c, n2], st);
            painter.circle_filled(c, 2.8, color);
            painter.circle_filled(n0, 2.3, color);
            painter.circle_filled(n1, 2.3, color);
            painter.circle_filled(n2, 2.3, color);
        }
        FileCategory::Other => {
            // Stacked layers diamond
            let top = vec![
                Pos2::new(c.x, c.y - r * 0.85),
                Pos2::new(c.x + r * 0.90, c.y - r * 0.25),
                Pos2::new(c.x, c.y + r * 0.30),
                Pos2::new(c.x - r * 0.90, c.y - r * 0.25),
                Pos2::new(c.x, c.y - r * 0.85),
            ];
            painter.add(Shape::line(top, st));
            let bot = vec![
                Pos2::new(c.x - r * 0.85, c.y + r * 0.28),
                Pos2::new(c.x, c.y + r * 0.82),
                Pos2::new(c.x + r * 0.85, c.y + r * 0.28),
            ];
            painter.add(Shape::line(bot, st));
        }
    }
}

/// Primary glowing coral CTA button matching 9Router's "Get Started" / "Start FREE Now" buttons.
pub fn brand_cta_button(ui: &mut Ui, label: &str, icon: Option<VectorIcon>, p: &Palette) -> Response {
    let text_galley = ui.painter().layout_no_wrap(
        label.to_string(),
        bold_font(12.5),
        Color32::WHITE,
    );
    let icon_w = if icon.is_some() { 16.0 } else { 0.0 };
    let desired = Vec2::new(text_galley.size().x + icon_w + 22.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());

    let bg = if resp.is_pointer_button_down_on() {
        p.brand_deep
    } else if resp.hovered() {
        p.brand_hover
    } else {
        p.brand
    };

    // Subtle warm terracotta glow halo (.shadow-glow)
    let glow_alpha = if resp.hovered() { 75 } else { 42 };
    painter_glow_rect(ui.painter(), rect, CornerRadius::same(9), Palette::alpha(p.brand, glow_alpha));
    ui.painter().rect_filled(rect, CornerRadius::same(8), bg);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 36)),
        StrokeKind::Inside,
    );

    let content_w = text_galley.size().x + icon_w;
    let start_x = rect.center().x - content_w * 0.5;
    if let Some(ic) = icon {
        paint_vector_icon(
            ui.painter(),
            Pos2::new(start_x + 5.0, rect.center().y),
            5.0,
            ic,
            Color32::WHITE,
        );
    }
    ui.painter().text(
        Pos2::new(start_x + icon_w, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        bold_font(12.5),
        Color32::WHITE,
    );

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    resp
}

/// Crimson destructive CTA button matching `brand_cta_button` proportions for delete confirmation dialogs.
pub fn danger_cta_button(ui: &mut Ui, label: &str, icon: Option<VectorIcon>, p: &Palette) -> Response {
    let text_galley = ui.painter().layout_no_wrap(
        label.to_string(),
        bold_font(12.5),
        Color32::WHITE,
    );
    let icon_w = if icon.is_some() { 16.0 } else { 0.0 };
    let desired = Vec2::new(text_galley.size().x + icon_w + 22.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());

    let bg = if resp.is_pointer_button_down_on() {
        Color32::from_rgb(185, 28, 28)
    } else if resp.hovered() {
        Color32::from_rgb(220, 38, 38)
    } else {
        p.danger
    };

    let glow_alpha = if resp.hovered() { 70 } else { 36 };
    painter_glow_rect(ui.painter(), rect, CornerRadius::same(9), Palette::alpha(p.danger, glow_alpha));
    ui.painter().rect_filled(rect, CornerRadius::same(8), bg);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 36)),
        StrokeKind::Inside,
    );

    let content_w = text_galley.size().x + icon_w;
    let start_x = rect.center().x - content_w * 0.5;
    if let Some(ic) = icon {
        paint_vector_icon(
            ui.painter(),
            Pos2::new(start_x + 5.0, rect.center().y),
            5.0,
            ic,
            Color32::WHITE,
        );
    }
    ui.painter().text(
        Pos2::new(start_x + icon_w, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        bold_font(12.5),
        Color32::WHITE,
    );

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    resp
}

fn painter_glow_rect(painter: &Painter, rect: Rect, radius: CornerRadius, color: Color32) {
    painter.rect_filled(rect.expand(2.5), radius, color);
}

/// Crisp bordered secondary toolbar button (surface fill + 1px border + coral border tint on hover).
pub fn secondary_button(ui: &mut Ui, label: &str, icon: Option<VectorIcon>, p: &Palette) -> Response {
    let text_galley = ui.painter().layout_no_wrap(
        label.to_string(),
        body_font(12.0),
        p.text_main,
    );
    let icon_w = if icon.is_some() { 15.0 } else { 0.0 };
    let desired = Vec2::new(text_galley.size().x + icon_w + 18.0, 29.0);
    let (rect, resp) = ui.allocate_exact_size(desired, Sense::click());

    let fill = if resp.hovered() { p.surface_2 } else { p.surface };
    let border = if resp.hovered() {
        Palette::alpha(p.brand, 145)
    } else {
        p.border
    };

    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(7),
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );

    let content_w = text_galley.size().x + icon_w;
    let start_x = rect.center().x - content_w * 0.5;
    if let Some(ic) = icon {
        paint_vector_icon(
            ui.painter(),
            Pos2::new(start_x + 4.5, rect.center().y),
            4.6,
            ic,
            if resp.hovered() { p.brand } else { p.text_muted },
        );
    }
    ui.painter().text(
        Pos2::new(start_x + icon_w, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        body_font(12.0),
        p.text_main,
    );

    resp
}

/// Square icon button (exact 9Router header Theme Toggle / Utility Icon button).
pub fn square_icon_button(ui: &mut Ui, icon: VectorIcon, tooltip: &str, p: &Palette, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let fill = if resp.hovered() { p.surface_2 } else { p.surface };
    let border = if resp.hovered() {
        Palette::alpha(p.brand, 155)
    } else {
        p.border
    };
    let fg = if resp.hovered() { p.brand } else { p.text_muted };

    let radius = if size <= 24.0 { 6 } else { 8 };
    ui.painter().rect_filled(rect, CornerRadius::same(radius), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );
    paint_vector_icon(ui.painter(), rect.center(), size * 0.23, icon, fg);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tooltip)
}

/// 9Router-style crisp uppercase pill badge (10-14% alpha tint + 30% border + crisp colored text).
pub fn status_pill(ui: &mut Ui, label: &str, accent: Color32, p: &Palette) {
    let galley = ui.painter().layout_no_wrap(label.to_string(), bold_font(10.0), accent);
    let desired = Vec2::new(galley.size().x + 14.0, 20.0);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());

    ui.painter().rect_filled(
        rect,
        CornerRadius::same(5),
        Palette::alpha(accent, if p.dark { 28 } else { 22 }),
    );
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(5),
        Stroke::new(1.0_f32, Palette::alpha(accent, if p.dark { 82 } else { 70 })),
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        bold_font(10.0),
        accent,
    );
}

/// Slimmer variant of `status_pill` tailored for compact download rows.
pub fn compact_status_pill(ui: &mut Ui, label: &str, accent: Color32, p: &Palette) {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), bold_font(9.5), accent);
    let desired = Vec2::new(galley.size().x + 11.0, 17.0);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());

    ui.painter().rect_filled(
        rect,
        CornerRadius::same(4),
        Palette::alpha(accent, if p.dark { 28 } else { 22 }),
    );
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0_f32, Palette::alpha(accent, if p.dark { 82 } else { 70 })),
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        bold_font(9.5),
        accent,
    );
}

/// Crisp 9Router multi-select square checkbox (supports unchecked, checked, and indeterminate dash).
pub fn custom_checkbox(ui: &mut Ui, checked: bool, indeterminate: bool, p: &Palette) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(17.0), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let active = checked || indeterminate;
    let fill = if active {
        if resp.hovered() {
            p.brand_hover
        } else {
            p.brand
        }
    } else if resp.hovered() {
        p.surface_2
    } else {
        p.bg_alt
    };

    let border = if active {
        p.brand
    } else if resp.hovered() {
        Palette::alpha(p.brand, 175)
    } else {
        p.border
    };

    ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(5),
        Stroke::new(1.2_f32, border),
        StrokeKind::Inside,
    );

    if checked {
        paint_vector_icon(ui.painter(), rect.center(), 4.6, VectorIcon::Check, Color32::WHITE);
    } else if indeterminate {
        let c = rect.center();
        ui.painter().line_segment(
            [Pos2::new(c.x - 4.0, c.y), Pos2::new(c.x + 4.0, c.y)],
            Stroke::new(2.0_f32, Color32::WHITE),
        );
    }

    resp
}

/// Modern crisp pill toggle switch (terracotta-coral ON state, bordered neutral OFF state).
pub fn toggle_switch(ui: &mut Ui, on: &mut bool, enabled: bool, p: &Palette) -> Response {
    let desired = Vec2::new(36.0, 20.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, mut resp) = ui.allocate_exact_size(desired, sense);

    if enabled && resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }

    let anim = ui.ctx().animate_bool_responsive(resp.id, *on);
    let track_bg = if !enabled {
        Palette::alpha(p.surface_2, 160)
    } else if *on {
        if resp.hovered() {
            p.brand_hover
        } else {
            p.brand
        }
    } else if resp.hovered() {
        p.surface_2
    } else if p.dark {
        Color32::from_rgb(58, 58, 58)
    } else {
        Color32::from_rgb(218, 220, 226)
    };

    let track_border = if !enabled {
        p.border_subtle
    } else if *on {
        p.brand
    } else if resp.hovered() {
        Palette::alpha(p.brand, 140)
    } else {
        p.border
    };

    ui.painter()
        .rect_filled(rect, CornerRadius::same(99), track_bg);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(99),
        Stroke::new(1.0_f32, track_border),
        StrokeKind::Inside,
    );

    let radius = 7.0_f32;
    let min_x = rect.min.x + 3.0 + radius;
    let max_x = rect.max.x - 3.0 - radius;
    let knob_x = egui::lerp(min_x..=max_x, anim);
    let knob_center = Pos2::new(knob_x, rect.center().y);

    let knob_col = if enabled {
        Color32::WHITE
    } else {
        p.text_subtle
    };
    ui.painter().circle_filled(knob_center, radius, knob_col);
    if !p.dark && !*on {
        ui.painter().circle_stroke(
            knob_center,
            radius,
            Stroke::new(0.8_f32, Color32::from_black_alpha(30)),
        );
    }

    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    resp
}

/// Custom high-contrast 9Router coral slider with visible rail, filled track, circular thumb, and aligned badge.
pub fn custom_slider_u32(
    ui: &mut Ui,
    value: &mut u32,
    min: u32,
    max: u32,
    width: f32,
    p: &Palette,
) -> Response {
    let mut changed = false;
    let mut out_resp = None;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (rect, mut resp) =
            ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::click_and_drag());

        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let pad = 8.0_f32;
        let rail_min_x = rect.min.x + pad;
        let rail_max_x = rect.max.x - pad;
        let rail_w = (rail_max_x - rail_min_x).max(1.0);

        if resp.clicked() || resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let t = ((pos.x - rail_min_x) / rail_w).clamp(0.0, 1.0);
                let next = (min as f32 + t * (max - min) as f32).round() as u32;
                let clamped = next.clamp(min, max);
                if clamped != *value {
                    *value = clamped;
                    changed = true;
                }
            }
        }

        let norm = if max > min {
            ((*value).clamp(min, max) - min) as f32 / (max - min) as f32
        } else {
            0.0
        };
        let thumb_x = rail_min_x + norm * rail_w;
        let cy = rect.center().y;

        // Visible background track rail (crisp in BOTH dark & light modes!)
        let rail_bg = if p.dark {
            Color32::from_rgb(60, 60, 60)
        } else {
            Color32::from_rgb(220, 223, 230)
        };
        let rail_rect = Rect::from_min_max(
            Pos2::new(rail_min_x, cy - 2.5),
            Pos2::new(rail_max_x, cy + 2.5),
        );
        ui.painter()
            .rect_filled(rail_rect, CornerRadius::same(99), rail_bg);

        // Filled terracotta-coral active track
        if thumb_x > rail_min_x {
            let fill_rect = Rect::from_min_max(
                Pos2::new(rail_min_x, cy - 2.5),
                Pos2::new(thumb_x, cy + 2.5),
            );
            ui.painter()
                .rect_filled(fill_rect, CornerRadius::same(99), p.brand);
        }

        // Thumb circle with soft hover halo
        let thumb_center = Pos2::new(thumb_x, cy);
        if resp.hovered() || resp.dragged() {
            ui.painter()
                .circle_filled(thumb_center, 10.5, Palette::alpha(p.brand, 55));
        }
        ui.painter().circle_filled(thumb_center, 6.8, Color32::WHITE);
        ui.painter().circle_stroke(
            thumb_center,
            6.8,
            Stroke::new(2.0_f32, p.brand),
        );

        // Crisp right-aligned monospace value pill
        let (val_rect, _) = ui.allocate_exact_size(Vec2::new(38.0, 24.0), Sense::hover());
        ui.painter()
            .rect_filled(val_rect, CornerRadius::same(6), p.bg_alt);
        ui.painter().rect_stroke(
            val_rect,
            CornerRadius::same(6),
            Stroke::new(1.0_f32, p.border),
            StrokeKind::Inside,
        );
        ui.painter().text(
            val_rect.center(),
            Align2::CENTER_CENTER,
            value.to_string(),
            mono_font(11.5),
            p.text_main,
        );

        if changed {
            resp.mark_changed();
        }
        out_resp = Some(resp);
    });

    out_resp.unwrap()
}

/// Crisp 9Router bordered text field with properly muted placeholder and coral focus ring.
pub fn styled_text_field(
    ui: &mut Ui,
    text: &mut String,
    hint: &str,
    mono: bool,
    width: f32,
    p: &Palette,
) -> Response {
    let mut inner_resp = None;
    let inner_w = if width.is_infinite() {
        (ui.available_width() - 18.0).max(40.0)
    } else {
        (width - 18.0).max(40.0)
    };
    let frame_resp = egui::Frame::new()
        .fill(p.bg_alt)
        .corner_radius(CornerRadius::same(8))
        .stroke(Stroke::new(1.0_f32, p.border))
        .inner_margin(Margin::symmetric(9, 5))
        .show(ui, |ui| {
            let font = if mono {
                mono_font(11.5)
            } else {
                body_font(12.0)
            };
            let edit = egui::TextEdit::singleline(text)
                .frame(false)
                .font(font)
                .hint_text(
                    egui::RichText::new(hint)
                        .font(if mono { mono_font(11.0) } else { body_font(11.5) })
                        .color(p.text_subtle),
                )
                .desired_width(inner_w);
            inner_resp = Some(ui.add(edit));
        });

    let resp = inner_resp.unwrap();
    if resp.has_focus() {
        ui.painter().rect_stroke(
            frame_resp.response.rect,
            CornerRadius::same(8),
            Stroke::new(1.4_f32, p.brand),
            StrokeKind::Inside,
        );
    } else if frame_resp.response.hovered() || resp.hovered() {
        ui.painter().rect_stroke(
            frame_resp.response.rect,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, Palette::alpha(p.brand, 120)),
            StrokeKind::Inside,
        );
    }
    resp
}

/// Compact selectable preset pill button (used for speed limit presets, etc.).
pub fn preset_chip(ui: &mut Ui, label: &str, selected: bool, p: &Palette) -> Response {
    let fg = if selected { p.brand } else { p.text_muted };
    let font = if selected { bold_font(10.5) } else { body_font(10.5) };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), fg);
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 12.0, 22.0), Sense::click());

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let bg = if selected {
        Palette::alpha(p.brand, if p.dark { 30 } else { 22 })
    } else if resp.hovered() {
        p.surface_2
    } else {
        p.bg_alt
    };
    let border = if selected {
        Palette::alpha(p.brand, 135)
    } else if resp.hovered() {
        Palette::alpha(p.brand, 90)
    } else {
        p.border
    };

    ui.painter().rect_filled(rect, CornerRadius::same(6), bg);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(6),
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        font,
        if resp.hovered() && !selected {
            p.text_main
        } else {
            fg
        },
    );
    resp
}

/// Full-screen subtle scrim behind modal windows.
pub fn paint_modal_scrim(ctx: &egui::Context, id: &str, p: &Palette) -> bool {
    let screen = ctx.screen_rect();
    let mut clicked_outside = false;
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            let scrim = if p.dark {
                Color32::from_rgba_unmultiplied(0, 0, 0, 148)
            } else {
                Color32::from_rgba_unmultiplied(15, 23, 42, 74)
            };
            ui.painter().rect_filled(rect, CornerRadius::ZERO, scrim);
            if resp.clicked() {
                clicked_outside = true;
            }
        });
    clicked_outside
}
