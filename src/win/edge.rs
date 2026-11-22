//! Screen-edge trigger for drawer mode: a 1-px, alpha-1 layered strip on part
//! of one edge. Hovering it briefly reveals the drawer. While a full-screen app
//! runs (AppBar `ABN_FULLSCREENAPP`, event-driven) the strip is hidden so games
//! and videos are never disturbed.

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_HOVER, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::{ABM_NEW, ABM_REMOVE, APPBARDATA, SHAppBarMessage};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::config::Edge;
use crate::win::monitors::Monitor;

pub const CLASS: PCWSTR = w!("DeskCrateEdge");
const WM_MOUSEHOVER: u32 = 0x02A1;
const HOVER_MS: u32 = 120;

pub fn register_class(hinst: windows::Win32::Foundation::HINSTANCE) {
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(edge_proc),
        hInstance: hinst,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        lpszClassName: CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassExW(&wc) };
}

/// Strip rectangle (screen px) for `edge` on monitor `m`.
pub fn strip_rect(m: &Monitor, edge: Edge, zone: (f32, f32)) -> Option<(i32, i32, i32, i32)> {
    let b = m.bounds;
    let (w, h) = (b.right - b.left, b.bottom - b.top);
    match edge {
        Edge::None => None,
        Edge::Left | Edge::Right => {
            let y0 = b.top + (h as f32 * zone.0) as i32;
            let y1 = b.top + (h as f32 * zone.1) as i32;
            let x = if edge == Edge::Left { b.left } else { b.right - 1 };
            Some((x, y0, 1, (y1 - y0).max(1)))
        }
        Edge::Top => {
            let x0 = b.left + (w as f32 * zone.0) as i32;
            let x1 = b.left + (w as f32 * zone.1) as i32;
            Some((x0, b.top, (x1 - x0).max(1), 1))
        }
    }
}

pub fn create(hinst: windows::Win32::Foundation::HINSTANCE, r: (i32, i32, i32, i32)) -> Option<HWND> {
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            CLASS,
            w!("DeskCrate edge"),
            WS_POPUP,
            r.0,
            r.1,
            r.2,
            r.3,
            None,
            None,
            Some(hinst),
            None,
        )
        .ok()?;
        // Alpha 1: invisible, but (unlike alpha 0) still receives the pointer.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Some(hwnd)
    }
}

pub fn set_visible(hwnd: HWND, visible: bool) {
    if !hwnd.is_invalid() {
        unsafe {
            let _ = ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            if visible {
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }
}

/// Registers `host` for AppBar notifications (full-screen apps) without
/// reserving any screen space.
pub fn appbar_register(host: HWND, msg: u32) {
    let mut abd = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: host,
        uCallbackMessage: msg,
        ..Default::default()
    };
    unsafe { SHAppBarMessage(ABM_NEW, &mut abd) };
}

pub fn appbar_unregister(host: HWND) {
    let mut abd = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: host, ..Default::default() };
    unsafe { SHAppBarMessage(ABM_REMOVE, &mut abd) };
}

unsafe extern "system" fn edge_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_HOVER | TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: HOVER_MS,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            LRESULT(0)
        }
        WM_MOUSEHOVER => {
            crate::app::app().show_drawer(false);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
