//! Folder enumeration (FindFirstFileEx, no COM) and Explorer-like ordering.

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM,
    FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
    WIN32_FIND_DATAW,
};
use windows::Win32::UI::Shell::StrCmpLogicalW;
use windows::core::PCWSTR;

use super::icons::IconKey;
use crate::util::{from_wide, wide};

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub display: String,
    pub is_dir: bool,
    pub attrs: u32,
    pub ext: String,
    /// Last write time (FILETIME ticks); drives the capsule's "latest" summary.
    pub mtime: u64,
}

/// Icon key for an arbitrary path (search results; attributes unknown).
pub fn key_for(dir: &str, name: &str, is_dir: bool) -> IconKey {
    if is_dir {
        return IconKey::Folder;
    }
    let (_, ext) = display_name(name, false, false);
    if ext.is_empty() || PER_FILE_ICON.contains(&ext.as_str()) {
        IconKey::Path(super::join(dir, name))
    } else {
        IconKey::Ext(ext)
    }
}

const ALWAYS_HIDE_EXT: &[&str] = &["lnk", "url", "pif", "appref-ms", "website"];
const PER_FILE_ICON: &[&str] =
    &["exe", "lnk", "ico", "url", "cur", "ani", "scr", "msc", "cpl", "appref-ms", "website", "pif"];

impl Item {
    /// Icon key; with `thumbs`, media and files with a shell thumbnail
    /// provider get a per-file thumbnail (upstream default), like Explorer.
    pub fn icon_key_with(&self, dir: &str, thumbs: bool) -> IconKey {
        if thumbs && !self.is_dir && super::icons::has_thumbnail(&self.ext) {
            return IconKey::Thumb(super::join(dir, &self.name));
        }
        self.icon_key(dir)
    }

    pub fn icon_key(&self, dir: &str) -> IconKey {
        if self.is_dir {
            // Explorer marks customised folders (desktop.ini) read-only or system.
            if self.attrs & (FILE_ATTRIBUTE_READONLY.0 | FILE_ATTRIBUTE_SYSTEM.0) != 0 {
                IconKey::Path(super::join(dir, &self.name))
            } else {
                IconKey::Folder
            }
        } else if self.ext.is_empty() || PER_FILE_ICON.contains(&self.ext.as_str()) {
            IconKey::Path(super::join(dir, &self.name))
        } else {
            IconKey::Ext(self.ext.clone())
        }
    }
}

pub fn display_name(name: &str, is_dir: bool, hide_ext: bool) -> (String, String) {
    if is_dir {
        return (name.to_owned(), String::new());
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => {
            let ext = name[dot + 1..].to_ascii_lowercase();
            let hide = hide_ext || ALWAYS_HIDE_EXT.contains(&ext.as_str());
            let shown = if hide { name[..dot].to_owned() } else { name.to_owned() };
            (shown, ext)
        }
        _ => (name.to_owned(), String::new()),
    }
}

/// Lists `dir`. Errors carry the Win32 error code.
pub fn list(dir: &str, show_hidden: bool, hide_ext: bool) -> Result<Vec<Item>, u32> {
    let pattern = wide(&super::join(dir, "*"));
    let mut data = WIN32_FIND_DATAW::default();
    let handle = unsafe {
        FindFirstFileExW(
            PCWSTR(pattern.as_ptr()),
            FindExInfoBasic,
            &mut data as *mut _ as *mut _,
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    let handle = match handle {
        Ok(h) => h,
        Err(e) => {
            let code = (e.code().0 as u32) & 0xFFFF;
            return if code == ERROR_FILE_NOT_FOUND.0 { Ok(Vec::new()) } else { Err(code) };
        }
    };
    let mut out = Vec::new();
    loop {
        let name = from_wide(&data.cFileName);
        let attrs = data.dwFileAttributes;
        let skip = name == "."
            || name == ".."
            || (!show_hidden && attrs & FILE_ATTRIBUTE_HIDDEN.0 != 0)
            || name.eq_ignore_ascii_case("desktop.ini")
            || name.eq_ignore_ascii_case("thumbs.db");
        if !skip {
            let is_dir = attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
            let (display, ext) = display_name(&name, is_dir, hide_ext);
            let t = data.ftLastWriteTime;
            let mtime = ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
            out.push(Item { display, ext, is_dir, attrs, name, mtime });
        }
        if let Err(e) = unsafe { FindNextFileW(handle, &mut data) } {
            let code = (e.code().0 as u32) & 0xFFFF;
            if code != ERROR_NO_MORE_FILES.0 {
                crate::log!("list {dir}: FindNextFile error {code}");
            }
            break;
        }
    }
    unsafe {
        let _ = FindClose(handle);
    }
    sort(&mut out);
    Ok(out)
}

/// Folders first, then Explorer's natural ("logical") order.
pub fn sort(items: &mut [Item]) {
    let keys: Vec<Vec<u16>> = items.iter().map(|i| wide(&i.display)).collect();
    let mut idx: Vec<usize> = (0..items.len()).collect();
    idx.sort_by(|&a, &b| {
        items[b].is_dir.cmp(&items[a].is_dir).then_with(|| {
            let r = unsafe { StrCmpLogicalW(PCWSTR(keys[a].as_ptr()), PCWSTR(keys[b].as_ptr())) };
            r.cmp(&0)
        })
    });
    let sorted: Vec<Item> = idx.iter().map(|&i| items[i].clone()).collect();
    items.clone_from_slice(&sorted);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names() {
        assert_eq!(display_name("App.lnk", false, false).0, "App");
        assert_eq!(display_name("a.pdf", false, false).0, "a.pdf");
        assert_eq!(display_name("a.pdf", false, true).0, "a");
        assert_eq!(display_name(".gitignore", false, true).0, ".gitignore");
        assert_eq!(display_name("dir.v2", true, true).0, "dir.v2");
    }

    #[test]
    fn natural_order_folders_first() {
        let mk = |n: &str, d: bool| Item {
            name: n.into(),
            display: n.into(),
            is_dir: d,
            attrs: 0,
            ext: String::new(),
            mtime: 0,
        };
        let mut v = vec![mk("file10", false), mk("file2", false), mk("zdir", true)];
        sort(&mut v);
        let names: Vec<_> = v.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["zdir", "file2", "file10"]);
    }
}
