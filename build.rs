use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let assets_dir = manifest_dir.join("assets");
    let _ = fs::create_dir_all(&assets_dir);

    let ico_path = assets_dir.join("vortexdm.ico");
    let ico_bytes = generate_vortex_ico(&[16, 32, 48, 256]);
    fs::write(&ico_path, ico_bytes).expect("Failed to write assets/vortexdm.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        let rc_path = out_dir.join("vortexdm.rc");
        let res_obj = out_dir.join("vortexdm_res.o");

        let ico_escaped = ico_path.to_string_lossy().replace('\\', "\\\\");
        let rc_content = format!(
            r#"1 ICON "{ico}"

1 VERSIONINFO
FILEVERSION 1,1,0,0
PRODUCTVERSION 1,1,0,0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "VortexDM"
            VALUE "FileDescription", "VortexDM — High-Speed Segmented Download Manager"
            VALUE "FileVersion", "1.1.0.0"
            VALUE "InternalName", "vortexdm"
            VALUE "OriginalFilename", "vortexdm.exe"
            VALUE "ProductName", "VortexDM"
            VALUE "ProductVersion", "1.1.0.0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#,
            ico = ico_escaped
        );

        fs::write(&rc_path, rc_content).expect("Failed to write resource script");

        let status = Command::new("windres")
            .arg(&rc_path)
            .arg("-O")
            .arg("coff")
            .arg("-o")
            .arg(&res_obj)
            .status();

        match status {
            Ok(s) if s.success() => {
                println!("cargo:rustc-link-arg={}", res_obj.display());
            }
            Ok(s) => {
                println!(
                    "cargo:warning=windres exited with status {}; continuing without embedded PE icon",
                    s
                );
            }
            Err(e) => {
                println!(
                    "cargo:warning=windres not available ({}); continuing without embedded PE icon",
                    e
                );
            }
        }
    }
}

/// Renders a crisp, antialiased VortexDM icon at `size x size` RGBA.
fn render_icon_rgba(size: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let ss = 4; // 4x4 supersampling for smooth anti-aliasing

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
                    let (r, g, b, a) = sample_icon(u, v);
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

fn sample_icon(u: f32, v: f32) -> (f32, f32, f32, f32) {
    // 9Router-inspired warm terracotta-coral rounded squircle (#E56A4A -> #CC5236 -> #A64027)
    let d_bg = sdf_rounded_box(u, v, 0.90, 0.90, 0.34);
    if d_bg > 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }

    let t = ((u + v) * 0.25 + 0.5).clamp(0.0, 1.0);
    let mut r = 0.90 - t * 0.24; // #E56A4A -> #A64027
    let mut g = 0.42 - t * 0.17;
    let mut b = 0.29 - t * 0.14;

    // Subtle warm top-edge inner sheen
    if d_bg > -0.065 && v < 0.0 {
        let sheen = ((-v).clamp(0.0, 1.0)) * 0.22;
        r = (r + sheen).min(1.0);
        g = (g + sheen * 0.85).min(1.0);
        b = (b + sheen * 0.75).min(1.0);
    }

    // Crisp pure-white geometric download arrow + connected segment nodes
    let in_shaft = u.abs() <= 0.13 && v >= -0.46 && v <= 0.12;
    let in_head = v >= 0.02 && v <= 0.44 && u.abs() <= (0.44 - v) * 1.12;
    let in_tray = sdf_rounded_box(u, v - 0.56, 0.46, 0.075, 0.038) <= 0.0;

    // Side acceleration node dots (echoing 9Router's connected-node emblem)
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

fn generate_vortex_ico(sizes: &[u32]) -> Vec<u8> {
    let mut images: Vec<(u32, Vec<u8>)> = Vec::with_capacity(sizes.len());
    for &sz in sizes {
        let rgba = render_icon_rgba(sz);
        let dib = rgba_to_ico_dib(sz, &rgba);
        images.push((sz, dib));
    }

    let count = images.len() as u16;
    let mut out = Vec::new();
    // ICONDIR
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type = 1 (ICO)
    out.extend_from_slice(&count.to_le_bytes());

    let mut offset = 6u32 + (count as u32) * 16;
    for (sz, dib) in &images {
        let dim = if *sz >= 256 { 0u8 } else { *sz as u8 };
        out.push(dim); // width
        out.push(dim); // height
        out.push(0); // color count
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bit count
        out.extend_from_slice(&(dib.len() as u32).to_le_bytes()); // bytes in res
        out.extend_from_slice(&offset.to_le_bytes()); // image offset
        offset += dib.len() as u32;
    }

    for (_, dib) in images {
        out.extend_from_slice(&dib);
    }

    out
}

fn rgba_to_ico_dib(size: u32, rgba: &[u8]) -> Vec<u8> {
    let and_row_bytes = (((size + 31) / 32) * 4) as usize;
    let xor_size = (size * size * 4) as usize;
    let and_size = and_row_bytes * size as usize;
    let mut dib = Vec::with_capacity(40 + xor_size + and_size);

    // BITMAPINFOHEADER (40 bytes)
    dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
    dib.extend_from_slice(&(size as i32).to_le_bytes()); // biWidth
    dib.extend_from_slice(&((size * 2) as i32).to_le_bytes()); // biHeight (XOR + AND)
    dib.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    dib.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    dib.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    dib.extend_from_slice(&((xor_size + and_size) as u32).to_le_bytes()); // biSizeImage
    dib.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    dib.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant

    // XOR mask: BGRA, bottom-up
    for y in (0..size).rev() {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            let r = rgba[idx];
            let g = rgba[idx + 1];
            let b = rgba[idx + 2];
            let a = rgba[idx + 3];
            dib.push(b);
            dib.push(g);
            dib.push(r);
            dib.push(a);
        }
    }

    // AND mask: 1 bit per pixel, bottom-up, padded to 32-bit boundary
    for y in (0..size).rev() {
        let mut row = vec![0u8; and_row_bytes];
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            if rgba[idx + 3] < 128 {
                row[(x / 8) as usize] |= 1 << (7 - (x % 8));
            }
        }
        dib.extend_from_slice(&row);
    }

    dib
}
