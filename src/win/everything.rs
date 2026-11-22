//! Everything (voidtools) IPC client — `WM_COPYDATA` + `EVERYTHING_IPC_QUERY2W`.
//! Nothing is bundled; when Everything is not running, searches stay local.
//! Replies arrive asynchronously as `WM_COPYDATA` on our host window.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, FindWindowW, MSGFLT_ALLOW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA,
};
use windows::core::w;

const COPYDATA_QUERY2W: usize = 18;
const REQUEST_FILE_NAME: u32 = 0x1;
const REQUEST_PATH: u32 = 0x2;
const REQUEST_FULL_PATH: u32 = 0x4;
const REQUEST_EXTENSION: u32 = 0x8;
const REQUEST_SIZE: u32 = 0x10;
const REQUEST_DATE_CREATED: u32 = 0x20;
const REQUEST_DATE_MODIFIED: u32 = 0x40;
const SORT_DATE_MODIFIED_DESC: u32 = 14;
const ITEM_FOLDER: u32 = 0x1;
const ITEM_DRIVE: u32 = 0x2;

/// Tag in `COPYDATASTRUCT::dwData` identifying our replies.
pub const REPLY_TAG: usize = 0xE700_0000;
pub const REPLY_TAG_MASK: usize = 0xFF00_0000;

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub name: String,
    pub dir: String,
    pub is_dir: bool,
}

impl Hit {
    pub fn full_path(&self) -> String {
        crate::shell::join(&self.dir, &self.name)
    }
}

/// Everything's IPC window (1.4, then 1.5 alpha instance name).
pub fn ipc_window() -> Option<HWND> {
    for class in [w!("EVERYTHING_TASKBAR_NOTIFICATION"), w!("EVERYTHING_TASKBAR_NOTIFICATION_(1.5a)")] {
        if let Ok(h) = unsafe { FindWindowW(class, None) }
            && !h.is_invalid()
        {
            return Some(h);
        }
    }
    None
}

/// Lets an elevated/unelevated Everything deliver replies to `reply`.
pub fn allow_replies(reply: HWND) {
    unsafe {
        let _ = ChangeWindowMessageFilterEx(reply, WM_COPYDATA, MSGFLT_ALLOW, None);
    }
}

pub fn encode_query(reply: HWND, tag: u32, text: &str, max: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(28 + (text.len() + 1) * 2);
    for d in [reply.0 as usize as u32, tag, 0, 0, max, REQUEST_FILE_NAME | REQUEST_PATH, SORT_DATE_MODIFIED_DESC] {
        v.extend_from_slice(&d.to_le_bytes());
    }
    for u in text.encode_utf16().chain(std::iter::once(0)) {
        v.extend_from_slice(&u.to_le_bytes());
    }
    v
}

/// Sends a query; the reply arrives later (or re-entrantly) as WM_COPYDATA.
pub fn query(everything: HWND, reply: HWND, tag: u32, text: &str, max: u32) -> bool {
    let packet = encode_query(reply, tag, text, max);
    let cds =
        COPYDATASTRUCT { dwData: COPYDATA_QUERY2W, cbData: packet.len() as u32, lpData: packet.as_ptr() as *mut _ };
    let mut result = 0usize;
    unsafe {
        SendMessageTimeoutW(
            everything,
            WM_COPYDATA,
            WPARAM(reply.0 as usize),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            1500,
            Some(&mut result),
        )
        .0 != 0
            && result != 0
    }
}

fn rd32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_str(b: &[u8], cursor: &mut usize) -> Option<String> {
    let len = rd32(b, *cursor)? as usize;
    if len > 32_768 {
        return None;
    }
    let start = *cursor + 4;
    let bytes = b.get(start..start + len * 2)?;
    *cursor = start + (len + 1) * 2;
    if *cursor > b.len() {
        return None;
    }
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some(String::from_utf16_lossy(&units))
}

/// Parses an `EVERYTHING_IPC_LIST2` reply: (total matches, hits).
pub fn parse_reply(b: &[u8]) -> Option<(u32, Vec<Hit>)> {
    let total = rd32(b, 0)?;
    let n = rd32(b, 4)? as usize;
    let flags = rd32(b, 12)?;
    if 20 + n * 8 > b.len() {
        return None;
    }
    let mut hits = Vec::with_capacity(n.min(1024));
    for i in 0..n {
        let item_flags = rd32(b, 20 + i * 8)?;
        let mut cur = rd32(b, 24 + i * 8)? as usize;
        let mut name = String::new();
        let mut dir = String::new();
        if flags & REQUEST_FILE_NAME != 0 {
            name = read_str(b, &mut cur)?;
        }
        if flags & REQUEST_PATH != 0 {
            dir = read_str(b, &mut cur)?;
        }
        if flags & REQUEST_FULL_PATH != 0 {
            read_str(b, &mut cur)?;
        }
        if flags & REQUEST_EXTENSION != 0 {
            read_str(b, &mut cur)?;
        }
        // Size / dates (8 bytes each) follow; we never request them.
        let _ = (REQUEST_SIZE, REQUEST_DATE_CREATED, REQUEST_DATE_MODIFIED);
        hits.push(Hit { name, dir, is_dir: item_flags & (ITEM_FOLDER | ITEM_DRIVE) != 0 });
    }
    Some((total, hits))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_str(v: &mut Vec<u8>, s: &str) {
        let u: Vec<u16> = s.encode_utf16().collect();
        v.extend_from_slice(&(u.len() as u32).to_le_bytes());
        for x in u.iter().chain(std::iter::once(&0)) {
            v.extend_from_slice(&x.to_le_bytes());
        }
    }

    #[test]
    fn parses_list2() {
        // Header (20) + 2 item headers (16) + data.
        let mut data = Vec::new();
        let off0 = 36u32;
        push_str(&mut data, "报告.docx");
        push_str(&mut data, "D:\\文档");
        let off1 = off0 + data.len() as u32;
        push_str(&mut data, "照片");
        push_str(&mut data, "E:\\");
        let mut b = Vec::new();
        for d in [57u32, 2, 0, REQUEST_FILE_NAME | REQUEST_PATH, 14, 0, off0, ITEM_FOLDER, off1] {
            b.extend_from_slice(&d.to_le_bytes());
        }
        b.extend_from_slice(&data);
        let (total, hits) = parse_reply(&b).unwrap();
        assert_eq!(total, 57);
        assert_eq!(hits[0], Hit { name: "报告.docx".into(), dir: "D:\\文档".into(), is_dir: false });
        assert!(hits[1].is_dir);
        assert_eq!(hits[1].full_path(), "E:\\照片");
        // Truncated buffers are rejected, never read out of bounds.
        assert!(parse_reply(&b[..b.len() - 3]).is_none());
    }

    #[test]
    fn encodes_query_header() {
        let q = encode_query(HWND(0x1234 as *mut _), 7, "ab", 30);
        assert_eq!(q.len(), 28 + 6);
        assert_eq!(rd32(&q, 0), Some(0x1234));
        assert_eq!(rd32(&q, 4), Some(7));
        assert_eq!(rd32(&q, 16), Some(30));
    }
}
