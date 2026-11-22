//! Build script: embeds the application manifest and the application icon
//! without rc.exe or any build dependency.
//!
//! The icon (an amber tile with a white crate outline) is rasterised here with a
//! tiny signed-distance renderer and written straight into a `.res` file that
//! the MSVC linker accepts as an input.

use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = root.join("res").join("deskcrate.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:NO");

    let res = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("deskcrate.res");
    std::fs::write(&res, icon_res(&[16, 20, 24, 32, 40, 48, 64, 128])).expect("write .res");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

// ---- icon artwork ------------------------------------------------------------

fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

fn sd_segment(px: f32, py: f32, x0: f32, x1: f32, y: f32) -> f32 {
    let cx = px.clamp(x0, x1);
    ((px - cx).powi(2) + (py - y).powi(2)).sqrt()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Straight-alpha BGRA, top-down.
fn render(size: u32) -> Vec<u8> {
    let s = size as f32;
    let c = s / 2.0;
    let margin = (s * 0.06).max(0.5);
    let tile_half = c - margin;
    let tile_r = s * 0.22;
    let stroke = (s * 0.075).max(1.2);
    let box_half = s * 0.25;
    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let tile_a = (0.5 - sd_round_rect(fx, fy, c, c, tile_half, tile_half, tile_r)).clamp(0.0, 1.0);
            if tile_a <= 0.0 {
                continue;
            }
            // Amber gradient: #FBBF24 (top) -> #D97706 (bottom).
            let t = (fy / s).clamp(0.0, 1.0);
            let (mut r, mut g, mut b) = (lerp(251.0, 217.0, t), lerp(191.0, 119.0, t), lerp(36.0, 6.0, t));
            // White crate outline: box, lid line, handle slot.
            let d_box =
                sd_round_rect(fx, fy, c, c + s * 0.03, box_half, box_half * 0.92, s * 0.07).abs() - stroke / 2.0;
            let d_lid = sd_segment(fx, fy, c - box_half, c + box_half, c - s * 0.08) - stroke / 2.0;
            let d_handle = sd_segment(fx, fy, c - s * 0.08, c + s * 0.08, c + s * 0.12) - stroke / 2.0;
            let glyph_a = (0.5 - d_box.min(d_lid).min(d_handle)).clamp(0.0, 1.0);
            r = lerp(r, 255.0, glyph_a);
            g = lerp(g, 255.0, glyph_a);
            b = lerp(b, 255.0, glyph_a);
            let i = ((y * size + x) * 4) as usize;
            px[i] = b.round() as u8;
            px[i + 1] = g.round() as u8;
            px[i + 2] = r.round() as u8;
            px[i + 3] = (tile_a * 255.0).round() as u8;
        }
    }
    px
}

/// One RT_ICON image: BITMAPINFOHEADER + bottom-up 32-bpp XOR + 1-bpp AND mask.
fn icon_image(size: u32) -> Vec<u8> {
    let px = render(size);
    let mask_stride = (size.div_ceil(32) * 4) as usize;
    let mut v = Vec::new();
    for d in [40u32, size, size * 2, 1 | (32 << 16), 0, size * size * 4 + mask_stride as u32 * size, 0, 0, 0, 0] {
        v.extend_from_slice(&d.to_le_bytes());
    }
    for y in (0..size).rev() {
        let row = (y * size * 4) as usize;
        v.extend_from_slice(&px[row..row + (size * 4) as usize]);
    }
    for y in (0..size).rev() {
        let mut row = vec![0u8; mask_stride];
        for x in 0..size {
            if px[((y * size + x) * 4 + 3) as usize] == 0 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        v.extend_from_slice(&row);
    }
    v
}

fn res_entry(out: &mut Vec<u8>, ty: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes()); // header size
    for w in [0xFFFFu16, ty, 0xFFFF, id] {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // data version
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0x0409u16.to_le_bytes()); // language
    out.extend_from_slice(&0u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // characteristics
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// `.res` with RT_ICON images 1..n and RT_GROUP_ICON #1 (the executable icon).
fn icon_res(sizes: &[u32]) -> Vec<u8> {
    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    let mut out = Vec::new();
    res_entry(&mut out, 0, 0, 0, &[]); // mandatory empty leading entry
    let mut group = Vec::new();
    for w in [0u16, 1, sizes.len() as u16] {
        group.extend_from_slice(&w.to_le_bytes());
    }
    for (i, &s) in sizes.iter().enumerate() {
        let img = icon_image(s);
        let id = (i + 1) as u16;
        res_entry(&mut out, RT_ICON, id, 0x1010, &img);
        group.push(if s >= 256 { 0 } else { s as u8 });
        group.push(if s >= 256 { 0 } else { s as u8 });
        group.extend_from_slice(&[0, 0]);
        group.extend_from_slice(&1u16.to_le_bytes());
        group.extend_from_slice(&32u16.to_le_bytes());
        group.extend_from_slice(&(img.len() as u32).to_le_bytes());
        group.extend_from_slice(&id.to_le_bytes());
    }
    res_entry(&mut out, RT_GROUP_ICON, 1, 0x1030, &group);
    out
}
