//! Small helpers: UTF-16 conversion, logging, message-parameter decoding.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::core::PCWSTR;

/// NUL-terminated UTF-16 buffer.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Converts a (possibly NUL-terminated) UTF-16 slice to a `String`.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Reads a NUL-terminated wide string from a raw pointer.
///
/// # Safety
/// `p` must be null or point to a valid NUL-terminated UTF-16 string.
pub unsafe fn pcwstr_to_string(p: PCWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { String::from_utf16_lossy(p.as_wide()) }
}

#[inline]
pub fn loword(v: usize) -> u16 {
    (v & 0xFFFF) as u16
}

#[inline]
pub fn hiword(v: usize) -> u16 {
    ((v >> 16) & 0xFFFF) as u16
}

#[inline]
pub fn x_lparam(l: LPARAM) -> i32 {
    (l.0 & 0xFFFF) as i16 as i32
}

#[inline]
pub fn y_lparam(l: LPARAM) -> i32 {
    ((l.0 >> 16) & 0xFFFF) as i16 as i32
}

#[inline]
pub fn wheel_delta(w: WPARAM) -> i32 {
    ((w.0 >> 16) & 0xFFFF) as i16 as i32
}

/// FNV-1a 64-bit hash — used to recognise our own config writes.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

static LOG_ENABLED: AtomicBool = AtomicBool::new(cfg!(debug_assertions));
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn init_log(path: PathBuf, enabled: bool) {
    let _ = LOG_PATH.set(path);
    set_log_enabled(enabled);
}

pub fn set_log_enabled(enabled: bool) {
    LOG_ENABLED.store(enabled || cfg!(debug_assertions), Ordering::Relaxed);
}

pub fn log_line(msg: &str) {
    let w = wide(&format!("[DeskCrate] {msg}\n"));
    unsafe { OutputDebugStringW(PCWSTR(w.as_ptr())) };
    if !LOG_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if let Some(path) = LOG_PATH.get() {
        // Keep the log bounded: truncate when it grows past 1 MB.
        let truncate = std::fs::metadata(path).map(|m| m.len() > 1 << 20).unwrap_or(false);
        let file = OpenOptions::new().create(true).append(!truncate).write(true).truncate(truncate).open(path);
        if let Ok(mut f) = file {
            let _ = writeln!(f, "{} {msg}", tick_ms());
        }
    }
}

/// (private bytes, working set) of this process in MB — for budget logging.
pub fn mem_mb() -> (f64, f64) {
    use windows::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
        .as_bool()
    };
    if !ok {
        return (0.0, 0.0);
    }
    (c.PrivateUsage as f64 / 1048576.0, c.WorkingSetSize as f64 / 1048576.0)
}

pub fn log_mem(stage: &str) {
    let (p, w) = mem_mb();
    log_line(&format!("mem[{stage}] private={p:.1}MB ws={w:.1}MB"));
}

pub fn tick_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::util::log_line(&format!($($arg)*)) };
}
