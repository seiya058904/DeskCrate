//! Native Explorer context menus (never owner-drawn by us).
//!
//! The shell's `IContextMenu` fills a real `HMENU`; our own commands are merged
//! on top. `IContextMenu2/3` messages are forwarded so "Send to", "Open with" and
//! "New" sub-menus populate exactly like in Explorer.

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    CMF_CANRENAME, CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
    GCS_VERBW, IContextMenu, IContextMenu2, IContextMenu3,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, HMENU, InsertMenuW, MF_BYPOSITION, MF_CHECKED, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, PostMessageW, SW_SHOWNORMAL, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
    WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WM_NULL,
};
use windows::core::{Interface, PCSTR, PCWSTR, PSTR};

use crate::util::{from_wide, wide};

/// Same value as SEE_MASK_UNICODE (not exported by windows-rs).
const CMIC_MASK_UNICODE: u32 = 0x4000;
const ID_FIRST: u32 = 1;
const ID_LAST: u32 = 0x6FFF;
/// Our own command ids start here (never overlap the shell range).
pub const EXTRA_BASE: u32 = 0x7000;

thread_local! {
    static ACTIVE: RefCell<Option<(Option<IContextMenu2>, Option<IContextMenu3>)>> = const { RefCell::new(None) };
}

/// Forward owner-window messages to the menu being tracked.
pub fn handle_menu_msg(msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if !matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        return None;
    }
    let menus = ACTIVE.with(|a| a.borrow().clone())?;
    unsafe {
        if let Some(cm3) = &menus.1 {
            let mut res = LRESULT(0);
            if cm3.HandleMenuMsg2(msg, wp, lp, Some(&mut res)).is_ok() {
                return Some(res);
            }
        }
        if let Some(cm2) = &menus.0
            && cm2.HandleMenuMsg(msg, wp, lp).is_ok()
        {
            return Some(LRESULT(0));
        }
    }
    None
}

#[derive(Clone, Debug)]
pub enum Extra {
    Item { id: u32, label: String, checked: bool, enabled: bool },
    Separator,
}

impl Extra {
    pub fn item(id: u32, label: &str) -> Self {
        Extra::Item { id, label: label.to_owned(), checked: false, enabled: true }
    }
    pub fn check(id: u32, label: &str, checked: bool) -> Self {
        Extra::Item { id, label: label.to_owned(), checked, enabled: true }
    }
}

pub enum Outcome {
    Nothing,
    Extra(u32),
    Rename,
    Invoked(String),
}

fn context_menu(owner: HWND, folder: &str, names: &[String]) -> Option<IContextMenu> {
    let sf = super::folder_of(folder)?;
    unsafe {
        if names.is_empty() {
            sf.CreateViewObject::<IContextMenu>(owner).ok()
        } else {
            let pidls = super::child_pidls(&sf, owner, names);
            if pidls.is_empty() {
                return None;
            }
            let ptrs: Vec<_> = pidls.iter().map(|p| p.0 as *const _).collect();
            sf.GetUIObjectOf::<IContextMenu>(owner, &ptrs, None).ok()
        }
    }
}

pub fn append_extras(menu: HMENU, extras: &[Extra]) {
    unsafe {
        for e in extras {
            match e {
                Extra::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
                Extra::Item { id, label, checked, enabled } => {
                    let w = wide(label);
                    let mut flags = MF_STRING;
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    let _ = AppendMenuW(menu, flags, *id as usize, PCWSTR(w.as_ptr()));
                }
            }
        }
    }
}

fn verb_of(cm: &IContextMenu, offset: u32) -> String {
    let mut buf = [0u16; 128];
    let ok = unsafe {
        cm.GetCommandString(offset as usize, GCS_VERBW, None, PSTR(buf.as_mut_ptr() as *mut u8), buf.len() as u32)
    };
    if ok.is_ok() { from_wide(&buf).to_ascii_lowercase() } else { String::new() }
}

fn invoke(cm: &IContextMenu, owner: HWND, verb_a: PCSTR, verb_w: PCWSTR, pt: Option<POINT>, dir: &str) -> bool {
    let dirw = wide(dir);
    let mut mask = CMIC_MASK_UNICODE;
    if pt.is_some() {
        mask |= CMIC_MASK_PTINVOKE;
    }
    let info = CMINVOKECOMMANDINFOEX {
        cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: mask,
        hwnd: owner,
        lpVerb: verb_a,
        lpVerbW: verb_w,
        lpDirectoryW: PCWSTR(dirw.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ptInvoke: pt.unwrap_or_default(),
        ..Default::default()
    };
    unsafe { cm.InvokeCommand(&info as *const _ as *const CMINVOKECOMMANDINFO).is_ok() }
}

/// Shows the shell menu for `names` in `folder` (or the folder background when
/// `names` is empty), with `extras` merged at the top.
pub fn track(
    owner: HWND,
    folder: &str,
    names: &[String],
    pt: POINT,
    extras: &[Extra],
    extended: bool,
    can_rename: bool,
) -> Outcome {
    let cm = context_menu(owner, folder, names);
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return Outcome::Nothing };
        append_extras(menu, extras);
        if let Some(cm) = &cm {
            let pos = extras.len() as u32;
            if pos > 0 {
                let _ = InsertMenuW(menu, pos, MF_BYPOSITION | MF_SEPARATOR, 0, PCWSTR::null());
            }
            let mut flags = CMF_NORMAL;
            if can_rename {
                flags |= CMF_CANRENAME;
            }
            if extended {
                flags |= CMF_EXTENDEDVERBS;
            }
            let _ = cm.QueryContextMenu(menu, pos + u32::from(pos > 0), ID_FIRST, ID_LAST, flags);
            ACTIVE.with(|a| *a.borrow_mut() = Some((cm.cast().ok(), cm.cast().ok())));
        }
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, pt.x, pt.y, owner, None).0 as u32;
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        ACTIVE.with(|a| *a.borrow_mut() = None);
        let outcome = if cmd == 0 {
            Outcome::Nothing
        } else if cmd >= EXTRA_BASE {
            Outcome::Extra(cmd)
        } else if let Some(cm) = &cm {
            let offset = cmd - ID_FIRST;
            let verb = verb_of(cm, offset);
            if verb == "rename" {
                Outcome::Rename
            } else {
                let id = offset as usize;
                invoke(cm, owner, PCSTR(id as *const u8), PCWSTR(id as *const u16), Some(pt), folder);
                Outcome::Invoked(verb)
            }
        } else {
            Outcome::Nothing
        };
        let _ = DestroyMenu(menu);
        outcome
    }
}

/// Invokes a canonical verb (`delete`, `copy`, `cut`, `paste`, `properties`, `open`…)
/// without showing a menu — used for keyboard shortcuts.
pub fn invoke_verb(owner: HWND, folder: &str, names: &[String], verb: &str) -> bool {
    let Some(cm) = context_menu(owner, folder, names) else { return false };
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return false };
        let _ = cm.QueryContextMenu(menu, 0, ID_FIRST, ID_LAST, CMF_NORMAL);
        let a: Vec<u8> = verb.bytes().chain(std::iter::once(0)).collect();
        let w = wide(verb);
        let ok = invoke(&cm, owner, PCSTR(a.as_ptr()), PCWSTR(w.as_ptr()), None, folder);
        let _ = DestroyMenu(menu);
        ok
    }
}

/// A plain native popup with only our own items. Returns the chosen id (0 = none).
/// Lets native popup menus follow the dark app theme, like Explorer does.
/// Uses uxtheme's `SetPreferredAppMode` (ordinal 135, Windows 10 1903+).
pub fn enable_dark_menus(dark: bool) {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::w;
    if crate::win::os_build() < 18362 {
        return;
    }
    unsafe {
        let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(f) = GetProcAddress(ux, PCSTR(135usize as *const u8)) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_mode(if dark { 2 } else { 3 }); // ForceDark / ForceLight
        }
        if let Some(f) = GetProcAddress(ux, PCSTR(136usize as *const u8)) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

pub fn track_plain(owner: HWND, pt: POINT, extras: &[Extra]) -> u32 {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        append_extras(menu, extras);
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, pt.x, pt.y, owner, None).0 as u32;
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd
    }
}
