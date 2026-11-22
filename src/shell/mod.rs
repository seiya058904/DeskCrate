//! Shell integration: folder enumeration, icons, native context menus, OLE drag & drop.

pub mod dragdrop;
pub mod icons;
pub mod items;
pub mod menu;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, ILFree, IShellFolder, KF_FLAG_DEFAULT, SEE_MASK_NOASYNC,
    SHELLEXECUTEINFOW, SHGetDesktopFolder, SHGetKnownFolderPath, SHParseDisplayName, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{GUID, HSTRING, PCWSTR};

use crate::util::{from_wide, wide};

#[derive(Clone, Copy)]
pub enum KnownFolder {
    Desktop,
    Downloads,
    Documents,
}

pub fn known_folder(k: KnownFolder) -> Option<String> {
    let id: &GUID = match k {
        KnownFolder::Desktop => &FOLDERID_Desktop,
        KnownFolder::Downloads => &FOLDERID_Downloads,
        KnownFolder::Documents => &FOLDERID_Documents,
    };
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

pub fn expand_env(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let src = wide(s);
    let mut buf = vec![0u16; 1024];
    let n = unsafe { ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() {
        return s.to_owned();
    }
    from_wide(&buf)
}

/// Owned absolute PIDL.
pub struct Pidl(pub *mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ILFree(Some(self.0)) };
        }
    }
}

pub fn parse_pidl(path: &str) -> Option<Pidl> {
    let mut p: *mut ITEMIDLIST = std::ptr::null_mut();
    unsafe { SHParseDisplayName(&HSTRING::from(path), None::<&IBindCtx>, &mut p, 0, None).ok()? };
    (!p.is_null()).then_some(Pidl(p))
}

/// `IShellFolder` for a file-system directory.
pub fn folder_of(path: &str) -> Option<IShellFolder> {
    let pidl = parse_pidl(path)?;
    unsafe {
        let desktop = SHGetDesktopFolder().ok()?;
        desktop.BindToObject::<_, IShellFolder>(pidl.0, None::<&IBindCtx>).ok()
    }
}

/// Folder-relative PIDLs for `names` (all inside `folder`).
pub fn child_pidls(folder: &IShellFolder, owner: HWND, names: &[String]) -> Vec<Pidl> {
    let mut out = Vec::with_capacity(names.len());
    for n in names {
        let w = wide(n);
        let mut p: *mut ITEMIDLIST = std::ptr::null_mut();
        let ok = unsafe {
            folder.ParseDisplayName(owner, None::<&IBindCtx>, PCWSTR(w.as_ptr()), None, &mut p, std::ptr::null_mut())
        };
        if ok.is_ok() && !p.is_null() {
            out.push(Pidl(p));
        }
    }
    out
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('\\') { format!("{dir}{name}") } else { format!("{dir}\\{name}") }
}

/// Opens a file/folder with its default verb (same as double-click in Explorer).
pub fn open_path(path: &str, dir: Option<&str>) -> bool {
    let file = wide(path);
    let dirw = dir.map(wide);
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpFile: PCWSTR(file.as_ptr()),
        lpDirectory: dirw.as_ref().map(|d| PCWSTR(d.as_ptr())).unwrap_or(PCWSTR::null()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut sei).is_ok() }
}

pub fn open_with_verb(path: &str, verb: &str) -> bool {
    let file = wide(path);
    let v = wide(verb);
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(v.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut sei).is_ok() }
}

/// Renames via `IFileOperation`: undoable from Explorer (Ctrl+Z) and shows the
/// native conflict / error UI. Returns `true` when the operation ran.
pub fn rename(owner: HWND, path: &str, new_name: &str) -> bool {
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::Win32::UI::Shell::{
        FOF_ALLOWUNDO, FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName,
    };
    unsafe {
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(path), None::<&IBindCtx>) else {
            return false;
        };
        let Ok(op) = CoCreateInstance::<_, IFileOperation>(&FileOperation, None, CLSCTX_ALL) else { return false };
        let _ = op.SetOwnerWindow(owner);
        let _ = op.SetOperationFlags(FOF_ALLOWUNDO);
        if op.RenameItem(&item, &HSTRING::from(new_name), None).is_err() {
            return false;
        }
        op.PerformOperations().is_ok() && !op.GetAnyOperationsAborted().map(|b| b.as_bool()).unwrap_or(false)
    }
}
