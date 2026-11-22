//! Event-driven directory watching without extra threads.
//!
//! `ReadDirectoryChangesW` completes into an APC completion routine that runs
//! on the UI thread while the message loop waits alertably
//! (`MsgWaitForMultipleObjectsEx(MWMO_ALERTABLE)`). Idle = zero wake-ups.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use windows::Win32::Foundation::{CloseHandle, ERROR_OPERATION_ABORTED, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::{CancelIoEx, OVERLAPPED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::HSTRING;

#[derive(Debug, Clone)]
pub struct Event {
    pub token: u64,
    /// Buffer overflowed — caller should rescan.
    pub overflow: bool,
    /// The watch died (directory removed / unreachable).
    pub gone: bool,
    pub changes: Vec<(u32, String)>,
}

struct Watch {
    ov: OVERLAPPED,
    handle: HANDLE,
    buf: Vec<u32>,
    token: u64,
    subtree: bool,
    filter: FILE_NOTIFY_CHANGE,
    closing: bool,
}

thread_local! {
    static WATCHES: RefCell<HashMap<usize, Box<Watch>>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<usize> = const { Cell::new(1) };
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static NOTIFY: Cell<(isize, u32)> = const { Cell::new((0, 0)) };
}

/// Window + message posted when events are queued.
pub fn set_notify(hwnd: HWND, msg: u32) {
    NOTIFY.with(|n| n.set((hwnd.0 as isize, msg)));
}

pub fn take_events() -> Vec<Event> {
    EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()))
}

fn push(ev: Event) {
    EVENTS.with(|e| e.borrow_mut().push(ev));
    let (hwnd, msg) = NOTIFY.with(Cell::get);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

/// Opaque watch id.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub struct WatchId(usize);

pub fn watch(path: &str, token: u64, subtree: bool) -> Option<WatchId> {
    let handle = unsafe {
        CreateFileW(
            &HSTRING::from(path),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            None,
        )
        .ok()?
    };
    let id = NEXT_ID.with(|n| {
        let v = n.get();
        n.set(v + 1);
        v
    });
    let mut w = Box::new(Watch {
        ov: OVERLAPPED::default(),
        handle,
        buf: vec![0u32; 8192],
        token,
        subtree,
        filter: FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_ATTRIBUTES
            | FILE_NOTIFY_CHANGE_SIZE
            | FILE_NOTIFY_CHANGE_LAST_WRITE,
        closing: false,
    });
    w.ov.hEvent = HANDLE(id as *mut _);
    if !issue(&mut w) {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return None;
    }
    WATCHES.with(|m| m.borrow_mut().insert(id, w));
    Some(WatchId(id))
}

/// Cancels the watch; resources are released in the completion routine.
pub fn unwatch(id: WatchId) {
    WATCHES.with(|m| {
        if let Some(w) = m.borrow_mut().get_mut(&id.0) {
            w.closing = true;
            unsafe {
                let _ = CancelIoEx(w.handle, Some(&w.ov));
            }
        }
    });
}

fn issue(w: &mut Watch) -> bool {
    let bytes = (w.buf.len() * 4) as u32;
    unsafe {
        ReadDirectoryChangesW(
            w.handle,
            w.buf.as_mut_ptr() as *mut _,
            bytes,
            w.subtree,
            w.filter,
            None,
            Some(&mut w.ov),
            Some(completion),
        )
        .is_ok()
    }
}

unsafe extern "system" fn completion(err: u32, bytes: u32, ov: *mut OVERLAPPED) {
    let id = unsafe { (*ov).hEvent.0 as usize };
    let mut finished: Option<(u64, bool)> = None;
    let mut event: Option<Event> = None;
    WATCHES.with(|m| {
        let mut map = m.borrow_mut();
        let Some(w) = map.get_mut(&id) else { return };
        if w.closing || err == ERROR_OPERATION_ABORTED.0 {
            finished = Some((w.token, false));
        } else if err != 0 {
            finished = Some((w.token, true));
        } else {
            let ev = if bytes == 0 {
                Event { token: w.token, overflow: true, gone: false, changes: Vec::new() }
            } else {
                Event { token: w.token, overflow: false, gone: false, changes: parse(&w.buf, bytes as usize) }
            };
            event = Some(ev);
            if !issue(w) {
                finished = Some((w.token, true));
            }
        }
        if finished.is_some()
            && let Some(w) = map.remove(&id)
        {
            unsafe {
                let _ = CloseHandle(w.handle);
            }
        }
    });
    if let Some(ev) = event {
        push(ev);
    }
    if let Some((token, true)) = finished {
        push(Event { token, overflow: false, gone: true, changes: Vec::new() });
    }
}

fn parse(buf: &[u32], len: usize) -> Vec<(u32, String)> {
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, len.min(buf.len() * 4)) };
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 12 <= bytes.len() {
        let rd = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let next = rd(off) as usize;
        let action = rd(off + 4);
        let name_len = rd(off + 8) as usize;
        let start = off + 12;
        if start + name_len > bytes.len() {
            break;
        }
        let name: Vec<u16> = bytes[start..start + name_len]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        out.push((action, String::from_utf16_lossy(&name)));
        if next == 0 {
            break;
        }
        off += next;
    }
    out
}
