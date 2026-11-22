//! Visual tokens. Values are taken from the upstream XAML/C# so the look matches
//! the original (see docs/AUDIT_AND_PLAN.md §2).

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::w;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    /// `0xAARRGGBB`
    pub const fn argb(v: u32) -> Self {
        Rgba {
            a: ((v >> 24) & 0xFF) as f32 / 255.0,
            r: ((v >> 16) & 0xFF) as f32 / 255.0,
            g: ((v >> 8) & 0xFF) as f32 / 255.0,
            b: (v & 0xFF) as f32 / 255.0,
        }
    }
    pub const fn with_alpha(self, a: f32) -> Self {
        Rgba { a, ..self }
    }
    pub fn scale_alpha(self, k: f32) -> Self {
        Rgba { a: (self.a * k).clamp(0.0, 1.0), ..self }
    }
    pub fn d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F { r: self.r, g: self.g, b: self.b, a: self.a }
    }
}

// ---- Geometry (DIP) -------------------------------------------------------
pub const TITLE_MIN_H: f32 = 40.0;
pub const TITLE_PAD_L: f32 = 14.0;
pub const TITLE_PAD_R: f32 = 12.0;
pub const TITLE_ICON: f32 = 16.0;
pub const TITLE_ICON_GAP: f32 = 8.0;
pub const TITLE_TEXT: f32 = 13.0;
pub const TITLE_BUTTON: f32 = 26.0;
pub const DIVIDER_MARGIN_H: f32 = 12.0;
pub const DIVIDER_MARGIN_B: f32 = 4.0;
pub const DIVIDER_OPACITY: f32 = 0.62;
pub const CONTENT_PAD_H: f32 = 8.0;
pub const CONTENT_PAD_TOP: f32 = 2.0;
pub const CONTENT_PAD_BOTTOM: f32 = 8.0;
pub const RESIZE_EDGE: f32 = 6.0;
pub const SCROLLBAR_W: f32 = 3.0;
pub const SNAP_GAP: f32 = 5.0;
pub const SNAP_DISTANCE: f32 = 8.0;
pub const CAPSULE_H: f32 = 42.0;
pub const MIN_W: f32 = 140.0;
pub const MIN_H: f32 = 90.0;
pub const DEFAULT_W: f32 = 280.0;
pub const DEFAULT_H: f32 = 400.0;

/// Corner radii for the four upstream presets: (outer, inner).
pub fn corner_radii(corner: crate::config::Corner) -> (f32, f32) {
    use crate::config::Corner::*;
    match corner {
        Square => (0.0, 0.0),
        Small => (4.0, 2.0),
        Round => (8.0, 4.0),
        Large => (16.0, 8.0),
    }
}

/// Resolved palette for one theme.
#[derive(Clone, Debug)]
pub struct Theme {
    pub dark: bool,
    pub accent: Rgba,
    pub text: Rgba,
    pub text_secondary: Rgba,
    pub text_tertiary: Rgba,
    pub divider: Rgba,
    pub tint: Rgba,
    pub shadow: Rgba,
    pub hover: Rgba,
    pub selected: Rgba,
    pub selected_hover: Rgba,
    pub selected_unfocused: Rgba,
    pub selected_unfocused_hover: Rgba,
    pub selected_border: Rgba,
    pub placeholder: Rgba,
    pub scrollbar: Rgba,
}

impl Theme {
    /// `tint_alpha` is the extra overlay drawn on top of the DWM material.
    pub fn new(dark: bool, accent: Rgba, tint_alpha: f32) -> Self {
        // Neutral state layers (upstream FileItemSurfaceStyleCache): white on dark,
        // black on light, alpha per state.
        let layer = |a_dark: u32, a_light: u32| {
            if dark { Rgba::argb((a_dark << 24) | 0x00FF_FFFF) } else { Rgba::argb(a_light << 24) }
        };
        if dark {
            Theme {
                dark,
                accent,
                text: Rgba::argb(0xFFF5F5F5),
                text_secondary: Rgba::argb(0xE8FFFFFF),
                text_tertiary: Rgba::argb(0xCCFFFFFF),
                divider: Rgba::argb(0x66FFFFFF).scale_alpha(DIVIDER_OPACITY),
                tint: Rgba::argb(0xFF1C1C1C).with_alpha(tint_alpha),
                shadow: Rgba::argb(0x8C000000),
                hover: layer(0x14, 0x0F),
                selected: layer(0x28, 0x20),
                selected_hover: layer(0x38, 0x2D),
                selected_unfocused: layer(0x10, 0x0C),
                selected_unfocused_hover: layer(0x1C, 0x15),
                selected_border: Rgba::argb(0x8BFFFFFF),
                placeholder: Rgba::argb(0x1FFFFFFF),
                scrollbar: Rgba::argb(0x66FFFFFF),
            }
        } else {
            Theme {
                dark,
                accent,
                text: Rgba::argb(0xE4000000),
                text_secondary: Rgba::argb(0xB0000000),
                text_tertiary: Rgba::argb(0x8A000000),
                divider: Rgba::argb(0x33000000).scale_alpha(DIVIDER_OPACITY),
                tint: Rgba::argb(0xFFF3F3F3).with_alpha(tint_alpha),
                shadow: Rgba::argb(0x59FFFFFF),
                hover: layer(0x14, 0x0F),
                selected: layer(0x28, 0x20),
                selected_hover: layer(0x38, 0x2D),
                selected_unfocused: layer(0x10, 0x0C),
                selected_unfocused_hover: layer(0x1C, 0x15),
                selected_border: Rgba::argb(0x72000000),
                placeholder: Rgba::argb(0x14000000),
                scrollbar: Rgba::argb(0x66000000),
            }
        }
    }
}

fn reg_dword(subkey: windows::core::PCWSTR, value: windows::core::PCWSTR) -> Option<u32> {
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r.is_ok().then_some(data)
}

/// Apps theme (registry read only).
pub fn system_apps_dark() -> bool {
    reg_dword(w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("AppsUseLightTheme"))
        .map(|v| v == 0)
        .unwrap_or(true)
}

/// Taskbar / system theme — used for the tray icon colour.
pub fn system_shell_dark() -> bool {
    reg_dword(w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), w!("SystemUsesLightTheme"))
        .map(|v| v == 0)
        .unwrap_or(true)
}

/// System accent colour (stored as 0xAABBGGRR).
pub fn system_accent() -> Rgba {
    match reg_dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor")) {
        Some(abgr) => {
            let r = abgr & 0xFF;
            let g = (abgr >> 8) & 0xFF;
            let b = (abgr >> 16) & 0xFF;
            Rgba::argb(0xFF00_0000 | (r << 16) | (g << 8) | b)
        }
        None => Rgba::argb(0xFF4CC2FF),
    }
}

/// Lightens a dark accent so it stays readable on dark material.
pub fn readable_accent(accent: Rgba, dark: bool) -> Rgba {
    let lum = 0.2126 * accent.r + 0.7152 * accent.g + 0.0722 * accent.b;
    if dark && lum < 0.45 {
        let k = 0.45;
        Rgba {
            r: accent.r + (1.0 - accent.r) * k,
            g: accent.g + (1.0 - accent.g) * k,
            b: accent.b + (1.0 - accent.b) * k,
            a: 1.0,
        }
    } else if !dark && lum > 0.6 {
        let k = 0.35;
        Rgba { r: accent.r * (1.0 - k), g: accent.g * (1.0 - k), b: accent.b * (1.0 - k), a: 1.0 }
    } else {
        accent
    }
}
