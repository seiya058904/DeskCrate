//! DWM materials: Mica / Mica Alt / Acrylic system backdrops (Win11 22H2+),
//! accent-policy blur fallback (Win10 1803+ and older Win11), rounded corners,
//! dark-mode frame. All composition happens inside DWM — zero cost for us.

use std::ffi::c_void;

use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWM_WINDOW_CORNER_PREFERENCE, DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMSBT_TABBEDWINDOW,
    DWMSBT_TRANSIENTWINDOW, DWMWA_BORDER_COLOR, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND, DWMWCP_ROUNDSMALL, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::Controls::MARGINS;
use windows::core::{s, w};

use crate::config::{Corner, Material};

/// What the window actually got — the renderer adapts the tint layer to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    SystemBackdrop,
    AccentBlur,
    Solid,
}

const DWMWA_COLOR_DEFAULT: u32 = 0xFFFF_FFFF;

fn set_attr<T>(hwnd: HWND, attr: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE, v: &T) -> bool {
    unsafe {
        DwmSetWindowAttribute(hwnd, attr, v as *const T as *const c_void, std::mem::size_of::<T>() as u32).is_ok()
    }
}

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionData {
    attrib: u32,
    data: *mut c_void,
    size: usize,
}

const WCA_ACCENT_POLICY: u32 = 19;
const ACCENT_DISABLED: u32 = 0;
const ACCENT_ENABLE_BLURBEHIND: u32 = 3;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;

/// Undocumented but stable since Windows 10 1803 (used by the taskbar itself).
fn set_accent(hwnd: HWND, state: u32, abgr: u32) -> bool {
    unsafe {
        let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else { return false };
        let Some(f) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else { return false };
        let f: unsafe extern "system" fn(HWND, *mut CompositionData) -> i32 = std::mem::transmute(f);
        let mut policy = AccentPolicy { state, flags: 2, gradient: abgr, animation: 0 };
        let mut data = CompositionData {
            attrib: WCA_ACCENT_POLICY,
            data: &mut policy as *mut _ as *mut c_void,
            size: std::mem::size_of::<AccentPolicy>(),
        };
        f(hwnd, &mut data) != 0
    }
}

pub fn apply(hwnd: HWND, material: Material, corner: Corner, dark: bool) -> Applied {
    let build = super::os_build();
    let win11 = build >= 22000;
    let dark_flag: i32 = dark.into();
    set_attr(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark_flag);

    if win11 {
        let pref: DWM_WINDOW_CORNER_PREFERENCE = match corner {
            Corner::Square => DWMWCP_DONOTROUND,
            Corner::Small => DWMWCP_ROUNDSMALL,
            Corner::Round | Corner::Large => DWMWCP_ROUND,
        };
        set_attr(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &pref);
        // Thin neutral system border, like the upstream widgets.
        set_attr(hwnd, DWMWA_BORDER_COLOR, &COLORREF(DWMWA_COLOR_DEFAULT));
    }

    // Material shows through every transparent client pixel.
    let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }

    let backdrop: Option<DWM_SYSTEMBACKDROP_TYPE> = match material {
        Material::Mica => Some(DWMSBT_MAINWINDOW),
        Material::MicaAlt => Some(DWMSBT_TABBEDWINDOW),
        Material::Acrylic => Some(DWMSBT_TRANSIENTWINDOW),
        Material::Blur | Material::Solid => None,
    };

    if build >= 22621 {
        if let Some(b) = backdrop {
            set_accent(hwnd, ACCENT_DISABLED, 0);
            if set_attr(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &b) {
                return Applied::SystemBackdrop;
            }
        } else {
            set_attr(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &DWMSBT_NONE);
        }
    }
    if material == Material::Solid {
        set_accent(hwnd, ACCENT_DISABLED, 0);
        return Applied::Solid;
    }
    // Blur requested, or no system backdrop available (Win10 / Win11 21H2).
    let state = if material == Material::Acrylic && build >= 17134 && !win11 {
        ACCENT_ENABLE_ACRYLICBLURBEHIND
    } else {
        ACCENT_ENABLE_BLURBEHIND
    };
    if set_accent(hwnd, state, 0x01000000) { Applied::AccentBlur } else { Applied::Solid }
}
