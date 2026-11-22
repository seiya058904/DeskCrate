//! Notification-area icon. The glyph (a crate) is rasterised at runtime with a
//! tiny signed-distance renderer — no image assets, crisp at any DPI.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HGDIOBJ,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO};

const UID: u32 = 1;

fn base(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: UID,
        ..Default::default()
    }
}

fn copy_into(dst: &mut [u16], s: &str) {
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().chain(std::iter::once(0))) {
        *d = c;
    }
    if let Some(last) = dst.last_mut() {
        *last = 0;
    }
}

pub fn add(hwnd: HWND, callback_msg: u32, icon: HICON, tip: &str) -> bool {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = callback_msg;
    nid.hIcon = icon;
    copy_into(&mut nid.szTip, tip);
    unsafe {
        let ok = Shell_NotifyIconW(NIM_ADD, &nid).as_bool();
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        ok
    }
}

pub fn set_tip(hwnd: HWND, tip: &str) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_TIP | NIF_SHOWTIP;
    copy_into(&mut nid.szTip, tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn set_icon(hwnd: HWND, icon: HICON) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_ICON;
    nid.hIcon = icon;
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn balloon(hwnd: HWND, title: &str, text: &str) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_INFO;
    copy_into(&mut nid.szInfoTitle, title);
    copy_into(&mut nid.szInfo, text);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn remove(hwnd: HWND) {
    let nid = base(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

fn sd_segment(px: f32, py: f32, x0: f32, x1: f32, y: f32) -> f32 {
    let cx = px.clamp(x0, x1);
    ((px - cx).powi(2) + (py - y).powi(2)).sqrt()
}

/// Straight-alpha BGRA pixels of the crate glyph.
pub fn glyph_pixels(size: i32, light_glyph: bool) -> Vec<u8> {
    let s = size as f32;
    let stroke = (s / 12.0).max(1.25);
    let (cr, cg, cb) = if light_glyph { (255u8, 255u8, 255u8) } else { (28u8, 28u8, 28u8) };
    let mut px = vec![0u8; (size * size * 4) as usize];
    let c = s / 2.0;
    let half = s * 0.40;
    let radius = s * 0.18;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Box outline.
            let d_box = sd_round_rect(fx, fy, c, c + s * 0.03, half, half * 0.92, radius).abs() - stroke / 2.0;
            // Lid line and handle slot.
            let lid_y = c - s * 0.12;
            let d_lid = sd_segment(fx, fy, c - half, c + half, lid_y) - stroke / 2.0;
            let d_handle = sd_segment(fx, fy, c - s * 0.12, c + s * 0.12, c + s * 0.14) - stroke / 2.0;
            let d = d_box.min(d_lid).min(d_handle);
            let a = (0.5 - d).clamp(0.0, 1.0);
            let i = ((y * size + x) * 4) as usize;
            px[i] = cb;
            px[i + 1] = cg;
            px[i + 2] = cr;
            px[i + 3] = (a * 255.0).round() as u8;
        }
    }
    px
}

pub fn make_icon(size: i32, light_glyph: bool) -> Option<HICON> {
    let pixels = glyph_pixels(size, light_glyph);
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
        let mask = CreateBitmap(size, size, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon
    }
}

pub fn destroy_icon(icon: HICON) {
    if !icon.is_invalid() {
        unsafe {
            let _ = DestroyIcon(icon);
        }
    }
}
