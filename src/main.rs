//! DeskCrate — a lean, native Windows desktop file organizer.
//!
//! Unofficial Rust re-implementation inspired by DeskBox
//! (github.com/Tianyu199509/DeskBox, GPL-3.0-only). Licensed GPL-3.0-only.

#![windows_subsystem = "windows"]

mod app;
mod config;
mod i18n;
mod layout;
mod shell;
mod sort;
mod theme;
mod ui;
mod util;
mod win;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA};
use windows::core::w;

fn main() {
    // One instance per user session; a second launch pings the first and exits.
    let _mutex = unsafe { CreateMutexW(None, true, w!("Local\\DeskCrate.SingleInstance")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if let Ok(h) = unsafe { FindWindowW(app::HOST_CLASS, None) } {
            let cds = COPYDATASTRUCT { dwData: 1, cbData: 0, lpData: std::ptr::null_mut() };
            unsafe {
                SendMessageTimeoutW(
                    h,
                    WM_COPYDATA,
                    WPARAM(0),
                    LPARAM(&cds as *const _ as isize),
                    SMTO_ABORTIFHUNG,
                    2000,
                    None,
                );
            }
        }
        return;
    }

    if unsafe { OleInitialize(None) }.is_err() {
        return;
    }
    // Tooltip class (comctl32 v6 via the manifest).
    unsafe {
        let icc = windows::Win32::UI::Controls::INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<windows::Win32::UI::Controls::INITCOMMONCONTROLSEX>() as u32,
            dwICC: windows::Win32::UI::Controls::ICC_WIN95_CLASSES,
        };
        let _ = windows::Win32::UI::Controls::InitCommonControlsEx(&icc);
    }
    let config_path = config::ConfigStore::locate();
    util::init_log(config_path.with_file_name("deskcrate.log"), false);
    util::log_line(&format!("DeskCrate {} starting (build {})", env!("CARGO_PKG_VERSION"), win::os_build()));
    let code = app::run(config_path);
    unsafe { OleUninitialize() };
    std::process::exit(code);
}
