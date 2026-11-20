/// Procedural antialiased RGBA icon renderer for VortexDM window & Windows system tray.

pub fn render_icon_rgba(size: u32, active_badge: bool) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let ss = 4; // 4x4 supersampling

    for y in 0..size {
        for x in 0..size {
            let mut acc_r = 0.0f32;
            let mut acc_g = 0.0f32;
            let mut acc_b = 0.0f32;
            let mut acc_a = 0.0f32;

            for sy in 0..ss {
                for sx in 0..ss {
                    let u = (x as f32 + (sx as f32 + 0.5) / ss as f32) / s * 2.0 - 1.0;
                    let v = (y as f32 + (sy as f32 + 0.5) / ss as f32) / s * 2.0 - 1.0;
                    let (r, g, b, a) = sample_icon(u, v, active_badge);
                    acc_r += r * a;
                    acc_g += g * a;
                    acc_b += b * a;
                    acc_a += a;
                }
            }

            let n = (ss * ss) as f32;
            let a = (acc_a / n).clamp(0.0, 1.0);
            let idx = ((y * size + x) * 4) as usize;
            if a > 0.001 {
                pixels[idx] = ((acc_r / acc_a).clamp(0.0, 1.0) * 255.0).round() as u8;
                pixels[idx + 1] = ((acc_g / acc_a).clamp(0.0, 1.0) * 255.0).round() as u8;
                pixels[idx + 2] = ((acc_b / acc_a).clamp(0.0, 1.0) * 255.0).round() as u8;
                pixels[idx + 3] = (a * 255.0).round() as u8;
            }
        }
    }

    pixels
}

fn sdf_rounded_box(px: f32, py: f32, bx: f32, by: f32, r: f32) -> f32 {
    let qx = px.abs() - bx + r;
    let qy = py.abs() - by + r;
    let out = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let inside = qx.max(qy).min(0.0);
    out + inside - r
}

fn sample_icon(u: f32, v: f32, active_badge: bool) -> (f32, f32, f32, f32) {
    // Active download emerald badge in bottom-right corner (#22C55E)
    if active_badge {
        let dx = u - 0.60;
        let dy = v - 0.60;
        let d_dot = (dx * dx + dy * dy).sqrt();
        if d_dot < 0.28 {
            return (0.133, 0.773, 0.369, 1.0);
        }
        if d_dot < 0.37 {
            return (0.10, 0.10, 0.10, 1.0);
        }
    }

    // Warm terracotta-coral rounded badge (#E56A4A -> #CC5236 -> #A64027)
    let d_bg = sdf_rounded_box(u, v, 0.90, 0.90, 0.34);
    if d_bg > 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }

    let t = ((u + v) * 0.25 + 0.5).clamp(0.0, 1.0);
    let mut r = 0.90 - t * 0.24;
    let mut g = 0.42 - t * 0.17;
    let mut b = 0.29 - t * 0.14;

    if d_bg > -0.065 && v < 0.0 {
        let sheen = ((-v).clamp(0.0, 1.0)) * 0.22;
        r = (r + sheen).min(1.0);
        g = (g + sheen * 0.85).min(1.0);
        b = (b + sheen * 0.75).min(1.0);
    }

    let in_shaft = u.abs() <= 0.13 && v >= -0.46 && v <= 0.12;
    let in_head = v >= 0.02 && v <= 0.44 && u.abs() <= (0.44 - v) * 1.12;
    let in_tray = sdf_rounded_box(u, v - 0.56, 0.46, 0.075, 0.038) <= 0.0;
    let d_left = ((u + 0.46).powi(2) + (v + 0.18).powi(2)).sqrt();
    let d_right = ((u - 0.46).powi(2) + (v + 0.18).powi(2)).sqrt();
    let in_nodes = d_left <= 0.105 || d_right <= 0.105;

    if in_shaft || in_head || in_tray || in_nodes {
        r = 1.0;
        g = 0.99;
        b = 0.98;
    }

    (r, g, b, 1.0)
}

pub fn make_tray_icon(active: bool) -> Result<tray_icon::Icon, tray_icon::BadIcon> {
    let size = 32;
    let rgba = render_icon_rgba(size, active);
    tray_icon::Icon::from_rgba(rgba, size, size)
}

pub fn make_egui_icon() -> egui::IconData {
    let size = 64;
    let rgba = render_icon_rgba(size, false);
    egui::IconData {
        rgba,
        width: size,
        height: size,
    }
}
