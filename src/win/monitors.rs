//! Monitor enumeration with *stable* identities (`QueryDisplayConfig` device
//! paths survive re-plugging and DPI changes; GDI names like `\\.\DISPLAY2` don't).

use std::collections::HashMap;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW,
    MonitorFromPoint, MonitorFromRect,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::BOOL;

use crate::util::from_wide;

#[derive(Clone, Debug)]
pub struct Monitor {
    pub handle: HMONITOR,
    pub id: String,
    pub work: RECT,
    pub bounds: RECT,
    pub dpi: u32,
    pub primary: bool,
}

impl Monitor {
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
}

/// GDI device name → monitor device path.
fn device_paths() -> HashMap<String, String> {
    let mut map = HashMap::new();
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).is_err() {
            return map;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None)
            .is_err()
        {
            return map;
        }
        for p in paths.iter().take(np as usize) {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            src.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            src.header.adapterId = p.sourceInfo.adapterId;
            src.header.id = p.sourceInfo.id;
            let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            tgt.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            tgt.header.adapterId = p.targetInfo.adapterId;
            tgt.header.id = p.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut src.header) == 0 && DisplayConfigGetDeviceInfo(&mut tgt.header) == 0 {
                let gdi = from_wide(&src.viewGdiDeviceName);
                let path = from_wide(&tgt.monitorDevicePath);
                if !path.is_empty() {
                    map.entry(gdi).or_insert(path);
                }
            }
        }
    }
    map
}

pub fn enumerate() -> Vec<Monitor> {
    let paths = device_paths();
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe extern "system" fn cb(h: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        unsafe { (*(lp.0 as *mut Vec<HMONITOR>)).push(h) };
        BOOL(1)
    }
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut handles as *mut _ as isize));
    }
    handles.into_iter().filter_map(|h| describe(h, &paths)).collect()
}

fn describe(h: HMONITOR, paths: &HashMap<String, String>) -> Option<Monitor> {
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    unsafe {
        if !GetMonitorInfoW(h, &mut mi as *mut _ as *mut MONITORINFO).as_bool() {
            return None;
        }
    }
    let gdi = from_wide(&mi.szDevice);
    let (mut dx, mut dy) = (96u32, 96u32);
    unsafe {
        let _ = GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    }
    Some(Monitor {
        handle: h,
        id: paths.get(&gdi).cloned().unwrap_or(gdi),
        work: mi.monitorInfo.rcWork,
        bounds: mi.monitorInfo.rcMonitor,
        dpi: dx.clamp(96, 960),
        primary: mi.monitorInfo.dwFlags & 1 != 0,
    })
}

pub fn primary(list: &[Monitor]) -> Option<&Monitor> {
    list.iter().find(|m| m.primary).or(list.first())
}

pub fn by_id<'a>(list: &'a [Monitor], id: &str) -> Option<&'a Monitor> {
    if id.is_empty() {
        return None;
    }
    list.iter().find(|m| m.id == id)
}

pub fn for_rect<'a>(list: &'a [Monitor], r: &RECT) -> Option<&'a Monitor> {
    let h = unsafe { MonitorFromRect(r, MONITOR_DEFAULTTONEAREST) };
    list.iter().find(|m| m.handle == h).or_else(|| primary(list))
}

pub fn for_point(list: &[Monitor], p: POINT) -> Option<&Monitor> {
    let h = unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST) };
    list.iter().find(|m| m.handle == h).or_else(|| primary(list))
}

/// Converts a stored DIP rect (relative to the work area) to screen pixels and
/// keeps at least the title bar on-screen.
pub fn to_screen(m: &Monitor, r: [f32; 4]) -> RECT {
    let s = m.scale();
    let ww = m.work.right - m.work.left;
    let wh = m.work.bottom - m.work.top;
    let w = ((r[2] * s).round() as i32).clamp(1, ww.max(1));
    let h = ((r[3] * s).round() as i32).clamp(1, wh.max(1));
    let mut x = m.work.left + (r[0] * s).round() as i32;
    let mut y = m.work.top + (r[1] * s).round() as i32;
    let keep = (40.0 * s) as i32;
    x = x.clamp(m.work.left - w + keep, m.work.right - keep);
    y = y.clamp(m.work.top, m.work.bottom - keep);
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

/// Screen pixels → DIP rect relative to the monitor's work area.
pub fn to_dip(m: &Monitor, r: &RECT) -> [f32; 4] {
    let s = m.scale();
    [
        (r.left - m.work.left) as f32 / s,
        (r.top - m.work.top) as f32 / s,
        (r.right - r.left) as f32 / s,
        (r.bottom - r.top) as f32 / s,
    ]
}
