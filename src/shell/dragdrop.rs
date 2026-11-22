//! Standard OLE drag & drop.
//!
//! Incoming drops are *delegated* to the target folder's own shell
//! `IDropTarget`, so move/copy/link semantics, conflict dialogs, progress,
//! undo and the right-drag menu behave exactly like Explorer. We only do
//! hit-testing (folder vs. sub-folder tile) and highlight feedback.

use std::cell::RefCell;
use std::rc::Weak;

use windows::Win32::Foundation::{HWND, POINT, POINTL};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IDataObject};
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropSource, IDropTarget,
    IDropTarget_Impl,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, IDropTargetHelper, IShellFolder, SHBindToParent, SHDoDragDrop};
use windows::core::{Ref, Result, implement};

/// Implemented by windows that accept drops.
pub trait DropHost {
    /// Folder that should receive a drop at this screen point, plus the tile
    /// index when the point is over a sub-folder tile.
    fn drop_target_at(&self, screen: POINT) -> Option<(String, Option<usize>)>;
    /// Highlight feedback: `Some(None)` = whole grid, `Some(Some(i))` = tile i, `None` = off.
    fn drop_feedback(&self, target: Option<Option<usize>>);
}

/// The shell's own drop target for a directory.
pub fn shell_drop_target(owner: HWND, path: &str) -> Option<IDropTarget> {
    let pidl = super::parse_pidl(path)?;
    unsafe {
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let parent: IShellFolder = SHBindToParent(pidl.0, Some(&mut child)).ok()?;
        parent.GetUIObjectOf::<IDropTarget>(owner, &[child as *const _], None).ok()
    }
}

struct State {
    data: Option<IDataObject>,
    path: Option<String>,
    target: Option<IDropTarget>,
    helper: Option<IDropTargetHelper>,
}

#[implement(IDropTarget)]
pub struct GridDropTarget {
    hwnd: isize,
    host: Weak<dyn DropHost>,
    st: RefCell<State>,
}

impl GridDropTarget {
    pub fn create(hwnd: HWND, host: Weak<dyn DropHost>) -> IDropTarget {
        let helper = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok() };
        GridDropTarget {
            hwnd: hwnd.0 as isize,
            host,
            st: RefCell::new(State { data: None, path: None, target: None, helper }),
        }
        .into()
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }

    /// Re-targets to the folder under the cursor; returns the shell's effect.
    fn retarget(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT, entering: bool) {
        let screen = POINT { x: pt.x, y: pt.y };
        let host = self.host.upgrade();
        let resolved = host.as_ref().and_then(|h| h.drop_target_at(screen));
        let mut st = self.st.borrow_mut();
        let new_path = resolved.as_ref().map(|(p, _)| p.clone());
        let allowed = unsafe { *effect };
        if new_path != st.path || entering {
            if let Some(t) = st.target.take() {
                unsafe {
                    let _ = t.DragLeave();
                }
            }
            st.path = new_path.clone();
            st.target = new_path.as_deref().and_then(|p| shell_drop_target(self.hwnd(), p));
            if let (Some(t), Some(data)) = (&st.target, &st.data) {
                unsafe {
                    *effect = allowed;
                    if t.DragEnter(data, keys, *pt, effect).is_err() {
                        *effect = DROPEFFECT_NONE;
                    }
                }
            } else {
                unsafe { *effect = DROPEFFECT_NONE };
            }
        } else if let Some(t) = &st.target {
            unsafe {
                if t.DragOver(keys, *pt, effect).is_err() {
                    *effect = DROPEFFECT_NONE;
                }
            }
        } else {
            unsafe { *effect = DROPEFFECT_NONE };
        }
        drop(st);
        if let Some(h) = host {
            let fb = resolved.filter(|_| unsafe { *effect } != DROPEFFECT_NONE).map(|(_, tile)| tile);
            h.drop_feedback(fb);
        }
    }
}

impl IDropTarget_Impl for GridDropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let data = data.cloned();
        {
            let mut st = self.st.borrow_mut();
            st.data = data.clone();
            if let (Some(h), Some(d)) = (&st.helper, &data) {
                unsafe {
                    let _ = h.DragEnter(self.hwnd(), d, &POINT { x: pt.x, y: pt.y }, *effect);
                }
            }
        }
        self.retarget(keys, pt, effect, true);
        Ok(())
    }

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        self.retarget(keys, pt, effect, false);
        let st = self.st.borrow();
        if let Some(h) = &st.helper {
            unsafe {
                let _ = h.DragOver(&POINT { x: pt.x, y: pt.y }, *effect);
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        let mut st = self.st.borrow_mut();
        if let Some(t) = st.target.take() {
            unsafe {
                let _ = t.DragLeave();
            }
        }
        if let Some(h) = &st.helper {
            unsafe {
                let _ = h.DragLeave();
            }
        }
        st.data = None;
        st.path = None;
        drop(st);
        if let Some(h) = self.host.upgrade() {
            h.drop_feedback(None);
        }
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        // Make sure we target what is under the cursor right now.
        self.retarget(keys, pt, effect, false);
        let (target, helper) = {
            let mut st = self.st.borrow_mut();
            let t = st.target.take();
            st.data = None;
            st.path = None;
            (t, st.helper.clone())
        };
        if let Some(h) = &helper
            && let Some(d) = data.as_ref()
        {
            unsafe {
                let _ = h.Drop(d, &POINT { x: pt.x, y: pt.y }, *effect);
            }
        }
        if let (Some(t), Some(d)) = (target, data.as_ref()) {
            unsafe {
                if t.Drop(d, keys, *pt, effect).is_err() {
                    *effect = DROPEFFECT_NONE;
                }
            }
        } else {
            unsafe { *effect = DROPEFFECT_NONE };
        }
        if let Some(h) = self.host.upgrade() {
            h.drop_feedback(None);
        }
        Ok(())
    }
}

/// Starts an OLE drag of `names` (inside `folder`). The shell builds the data
/// object and the drag image; returns the performed effect.
pub fn start_drag(owner: HWND, folder: &str, names: &[String]) -> DROPEFFECT {
    let Some(sf) = super::folder_of(folder) else { return DROPEFFECT_NONE };
    let pidls = super::child_pidls(&sf, owner, names);
    if pidls.is_empty() {
        return DROPEFFECT_NONE;
    }
    let ptrs: Vec<_> = pidls.iter().map(|p| p.0 as *const _).collect();
    unsafe {
        let Ok(data) = sf.GetUIObjectOf::<IDataObject>(owner, &ptrs, None) else { return DROPEFFECT_NONE };
        SHDoDragDrop(Some(owner), &data, None::<&IDropSource>, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK)
            .unwrap_or(DROPEFFECT_NONE)
    }
}
