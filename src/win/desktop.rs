//! Desktop-layer pinning that survives `Win+D` (strategy audited from upstream
//! `WidgetLayerService`, see docs/AUDIT_AND_PLAN.md §3.1):
//!
//! * the grid window is *owned* by Explorer's desktop icon view
//!   (`SHELLDLL_DefView` under `Progman`/`WorkerW`) — Show Desktop raises the
//!   desktop, and owned windows always stay above their owner;
//! * it rests at `HWND_BOTTOM`, but the Z order is only rewritten when a foreign
//!   window actually sits below it (re-issuing `HWND_BOTTOM` repaints the owner
//!   band and flickers);
//! * clicks on a grid that is covered by another app must not activate it,
//!   because activating a DefView-owned window lifts the whole band.

use std::cell::Cell;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GW_HWNDNEXT, GW_HWNDPREV, GWL_STYLE, GWLP_HWNDPARENT, GetParent, GetShellWindow,
    GetWindow, GetWindowLongPtrW, GetWindowLongW, GetWindowThreadProcessId, HWND_BOTTOM, IsWindow, IsWindowVisible,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos, WS_MINIMIZE,
};
use windows::core::{BOOL, w};

use super::class_name;

thread_local! {
    static DEFVIEW: Cell<isize> = const { Cell::new(0) };
}

const MAX_WALK: usize = 256;

fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// `Progman`/`WorkerW` belonging to the shell process.
fn is_desktop_host(hwnd: HWND) -> bool {
    let class = class_name(hwnd);
    if class != "Progman" && class != "WorkerW" {
        return false;
    }
    let shell = unsafe { GetShellWindow() };
    if shell.is_invalid() {
        return class == "Progman";
    }
    hwnd == shell || pid_of(hwnd) == pid_of(shell)
}

fn is_shell_desktop_window(hwnd: HWND) -> bool {
    let mut cur = hwnd;
    for _ in 0..4 {
        if cur.is_invalid() {
            return false;
        }
        let class = class_name(cur);
        if class == "SHELLDLL_DefView" || class == "SysListView32" {
            cur = unsafe { GetParent(cur).unwrap_or_default() };
            continue;
        }
        return is_desktop_host(cur);
    }
    false
}

fn verified_defview(dv: HWND) -> bool {
    let alive = unsafe { IsWindow(Some(dv)).as_bool() };
    alive && class_name(dv) == "SHELLDLL_DefView" && unsafe { GetParent(dv) }.map(is_desktop_host).unwrap_or(false)
}

/// Explorer's desktop icon view. Never forces WorkerW creation (that races
/// Explorer's icon-layout restore at login).
pub fn find_defview() -> Option<HWND> {
    let cached = HWND(DEFVIEW.with(Cell::get) as *mut _);
    if !cached.is_invalid() && verified_defview(cached) {
        return Some(cached);
    }
    let mut found = HWND::default();
    unsafe {
        let shell = GetShellWindow();
        if !shell.is_invalid()
            && let Ok(dv) = FindWindowExW(Some(shell), None, w!("SHELLDLL_DefView"), None)
        {
            found = dv;
        }
        if found.is_invalid() {
            unsafe extern "system" fn enum_proc(hwnd: HWND, lp: LPARAM) -> BOOL {
                unsafe {
                    if is_desktop_host(hwnd)
                        && let Ok(dv) = FindWindowExW(Some(hwnd), None, w!("SHELLDLL_DefView"), None)
                    {
                        *(lp.0 as *mut HWND) = dv;
                        return BOOL(0);
                    }
                }
                BOOL(1)
            }
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut found as *mut HWND as isize));
        }
    }
    if found.is_invalid() {
        return None;
    }
    DEFVIEW.with(|c| c.set(found.0 as isize));
    Some(found)
}

pub fn invalidate() {
    DEFVIEW.with(|c| c.set(0));
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok() && cloaked != 0 }
}

#[derive(PartialEq)]
enum Kind {
    Own,
    Shell,
    Ignored,
    Foreign,
}

fn classify(hwnd: HWND, own_pid: u32) -> Kind {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool()
            || is_cloaked(hwnd)
            || (GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_MINIMIZE.0) != 0
        {
            return Kind::Ignored;
        }
    }
    if pid_of(hwnd) == own_pid {
        Kind::Own
    } else if is_shell_desktop_window(hwnd) {
        Kind::Shell
    } else {
        Kind::Foreign
    }
}

/// `true` when only our windows / shell desktop windows / invisible windows sit below.
pub fn is_bedded(hwnd: HWND) -> bool {
    let own = std::process::id();
    let mut cur = unsafe { GetWindow(hwnd, GW_HWNDNEXT).unwrap_or_default() };
    for _ in 0..MAX_WALK {
        if cur.is_invalid() {
            return true;
        }
        if classify(cur, own) == Kind::Foreign {
            return false;
        }
        cur = unsafe { GetWindow(cur, GW_HWNDNEXT).unwrap_or_default() };
    }
    true
}

/// Attaches `hwnd` to the desktop icon layer and beds it at the bottom.
/// Falls back to a plain `HWND_BOTTOM` window when Explorer is unavailable
/// (e.g. elevated Explorer) — `Win+D` may then hide it.
pub fn attach(hwnd: HWND) -> bool {
    let attached = match find_defview() {
        Some(dv) => unsafe {
            if GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT) != dv.0 as isize {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, dv.0 as isize);
            }
            let ok = GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT) == dv.0 as isize;
            if !ok {
                crate::log!("desktop: owner attach failed for {:?}", hwnd.0);
                invalidate();
            }
            ok
        },
        None => {
            crate::log!("desktop: SHELLDLL_DefView not found");
            false
        }
    };
    rebed(hwnd);
    attached
}

/// Puts the window back at the bottom only if something foreign is below it.
pub fn rebed(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    if !is_bedded(hwnd) {
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

fn intersects(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// `true` when a visible foreign window above `hwnd` overlaps it (taskbar excluded).
pub fn covered_by_foreign(hwnd: HWND) -> bool {
    let own = std::process::id();
    let me = super::window_rect(hwnd);
    let mut cur = unsafe { GetWindow(hwnd, GW_HWNDPREV).unwrap_or_default() };
    for _ in 0..MAX_WALK {
        if cur.is_invalid() {
            return false;
        }
        if classify(cur, own) == Kind::Foreign {
            let class = class_name(cur);
            if class != "Shell_TrayWnd"
                && class != "Shell_SecondaryTrayWnd"
                && intersects(&me, &super::window_rect(cur))
            {
                return true;
            }
        }
        cur = unsafe { GetWindow(cur, GW_HWNDPREV).unwrap_or_default() };
    }
    false
}
