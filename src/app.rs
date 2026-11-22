//! Application state and the hidden host window (tray, broadcasts, timers,
//! icon/watch notifications). Model (config) and HWNDs are decoupled so any
//! window can be re-created (e.g. after an Explorer crash).

use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use crate::config::{Appearance, Config, ConfigStore, Edge, GridCfg, Material, Mode, ThemePref};
use crate::layout::TileMetrics;
use crate::shell::icons::IconCache;
use crate::shell::menu::{self, Extra};
use crate::theme::{self, Rgba, Theme};
use crate::ui::grid::{self, Grid, Kind, Tab};
use crate::ui::render::{self, Renderer};
use crate::util::{loword, wide};
use crate::win::material::Applied;
use crate::win::{self, desktop, edge, hotkey, monitors, tray, watcher};

pub const HOST_CLASS: PCWSTR = w!("DeskCrateHost");

pub const WM_APP_TRAY: u32 = WM_APP + 1;
pub const WM_APP_ICON: u32 = WM_APP + 2;
pub const WM_APP_FS: u32 = WM_APP + 3;
pub const WM_APP_APPBAR: u32 = WM_APP + 4;

const HOTKEY_ID: i32 = 1;
const ABN_FULLSCREENAPP: usize = 2;

const TIMER_CONFIG: usize = 1;
const TIMER_DISPLAY: usize = 2;
const TIMER_THEME: usize = 3;
const TIMER_TRIM: usize = 4;
const TIMER_SORT: usize = 5;
const TIMER_SORT_PERIODIC: usize = 6;
const TIMER_FS_BASE: usize = 1000;

const CONFIG_TOKEN: u64 = u64::MAX;
const SORT_TOKEN_BASE: u64 = u64::MAX - 1000;
const SORT_PERIOD_MS: u32 = 6 * 3600 * 1000;
const ICON_CACHE_BUDGET: usize = 4 << 20;

const CMD_NEW: u32 = 1;
const CMD_REFRESH_ALL: u32 = 2;
const CMD_EDIT_CONFIG: u32 = 3;
const CMD_OPEN_CONFIG_DIR: u32 = 4;
const CMD_AUTOSTART: u32 = 5;
const CMD_ABOUT: u32 = 6;
const CMD_EXIT: u32 = 7;
const CMD_DRAWER_MODE: u32 = 8;
const CMD_SHOW_DRAWER: u32 = 9;
const CMD_SORT_NOW: u32 = 10;
const CMD_SORT_UNDO: u32 = 11;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("DeskCrate");

pub struct App {
    host: HWND,
    pub renderer: Renderer,
    pub icons: RefCell<IconCache>,
    pub store: RefCell<ConfigStore>,
    pub theme: RefCell<Theme>,
    pub metrics: Cell<TileMetrics>,
    pub grids: RefCell<Vec<Rc<Grid>>>,
    pub hide_ext: Cell<bool>,
    applied: Cell<Applied>,
    tray_icon: Cell<HICON>,
    next_token: Cell<u64>,
    taskbar_created: u32,
    last_look: RefCell<(Appearance, ThemePref)>,
    config_watch: Cell<Option<watcher::WatchId>>,
    drawer: RefCell<Option<Rc<Grid>>>,
    edge: Cell<HWND>,
    appbar: Cell<bool>,
    peeking: Cell<bool>,
    hotkey: RefCell<String>,
    sort_cfg: RefCell<Option<crate::sort::SortCfg>>,
    sort_watches: RefCell<Vec<watcher::WatchId>>,
    sort_notified: Cell<usize>,
}

/// `true` for windows that belong to this process (host, grids, dialogs…).
pub fn is_own_window(hwnd: HWND) -> bool {
    if hwnd.is_invalid() {
        return false;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid == std::process::id()
}

thread_local! {
    static APP: Cell<Option<&'static App>> = const { Cell::new(None) };
}

pub fn app() -> &'static App {
    APP.with(Cell::get).expect("app not initialised")
}

fn try_app() -> Option<&'static App> {
    APP.with(Cell::get)
}

pub fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).map(|h| HINSTANCE(h.0)).unwrap_or_default() }
}

fn register_classes() {
    unsafe {
        let hinst = hinstance();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let host = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(host_proc),
            hInstance: hinst,
            lpszClassName: HOST_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&host);
        let grid = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(grid::wndproc),
            hInstance: hinst,
            hCursor: cursor,
            lpszClassName: grid::CLASS,
            ..Default::default()
        };
        RegisterClassExW(&grid);
        edge::register_class(hinst);
    }
}

/// Builds the app and runs until exit. Returns the process exit code.
pub fn run(config_path: std::path::PathBuf) -> i32 {
    register_classes();
    let host = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            HOST_CLASS,
            w!("DeskCrate"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance()),
            None,
        )
    };
    let Ok(host) = host else {
        crate::log!("host window creation failed");
        return 1;
    };
    let store = ConfigStore::load_or_create(config_path);
    crate::util::set_log_enabled(store.cfg.general.log);
    crate::util::log_mem("host");
    let renderer = match Renderer::new() {
        Ok(r) => r,
        Err(e) => {
            crate::log!("renderer init failed: {e:?}");
            return 2;
        }
    };
    crate::util::log_mem("renderer");
    let look = (store.cfg.appearance.clone(), store.cfg.general.theme);
    let metrics = TileMetrics::new(&store.cfg.appearance);
    let app: &'static App = Box::leak(Box::new(App {
        host,
        renderer,
        icons: RefCell::new(IconCache::new(host, WM_APP_ICON, ICON_CACHE_BUDGET)),
        store: RefCell::new(store),
        theme: RefCell::new(Theme::new(true, Rgba::argb(0xFF4CC2FF), 0.18)),
        metrics: Cell::new(metrics),
        grids: RefCell::new(Vec::new()),
        hide_ext: Cell::new(crate::shell::hide_known_extensions()),
        applied: Cell::new(Applied::SystemBackdrop),
        tray_icon: Cell::new(HICON::default()),
        next_token: Cell::new(1),
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        last_look: RefCell::new(look),
        config_watch: Cell::new(None),
        drawer: RefCell::new(None),
        edge: Cell::new(HWND::default()),
        appbar: Cell::new(false),
        peeking: Cell::new(false),
        hotkey: RefCell::new(String::new()),
        sort_cfg: RefCell::new(None),
        sort_watches: RefCell::new(Vec::new()),
        sort_notified: Cell::new(0),
    }));
    APP.with(|a| a.set(Some(app)));
    app.start();
    message_loop()
}

fn message_loop() -> i32 {
    let mut msg = MSG::default();
    loop {
        // Alertable wait: directory-watch completion routines run here.
        unsafe {
            MsgWaitForMultipleObjectsEx(None, u32::MAX, QS_ALLINPUT, MWMO_ALERTABLE | MWMO_INPUTAVAILABLE);
        }
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                return msg.wParam.0 as i32;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

impl App {
    pub fn host(&self) -> HWND {
        self.host
    }

    pub fn cfg(&self) -> Ref<'_, Config> {
        Ref::map(self.store.borrow(), |s| &s.cfg)
    }

    fn start(&self) {
        crate::util::log_mem("app");
        self.refresh_theme();
        self.add_tray();
        crate::util::log_mem("tray");
        watcher::set_notify(self.host, WM_APP_FS);
        win::everything::allow_replies(self.host);
        self.watch_config();
        self.reconcile();
        crate::util::log_mem("grids");
        let autostart = self.cfg().general.autostart;
        sync_autostart(autostart);
        crate::log!("started: {} grid(s), material {:?}", self.grids.borrow().len(), self.applied.get());
    }

    fn watch_config(&self) {
        if let Some(w) = self.config_watch.take() {
            watcher::unwatch(w);
        }
        let dir = self.store.borrow().path.parent().map(|p| p.to_string_lossy().into_owned());
        if let Some(dir) = dir {
            self.config_watch.set(watcher::watch(&dir, CONFIG_TOKEN, false));
        }
    }

    fn add_tray(&self) {
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
        let old = self.tray_icon.get();
        if let Some(icon) = tray::make_icon(size, theme::system_shell_dark()) {
            self.tray_icon.set(icon);
            if !tray::add(self.host, WM_APP_TRAY, icon, "DeskCrate") {
                tray::set_icon(self.host, icon);
            }
            tray::destroy_icon(old);
        }
    }

    // ---- appearance -------------------------------------------------------

    fn refresh_theme(&self) {
        let (pref, tint) = {
            let c = self.cfg();
            (c.general.theme, c.appearance.tint)
        };
        let dark = match pref {
            ThemePref::System => theme::system_apps_dark(),
            ThemePref::Dark => true,
            ThemePref::Light => false,
        };
        let accent = theme::readable_accent(theme::system_accent(), dark);
        *self.theme.borrow_mut() = Theme::new(dark, accent, tint);
        menu::enable_dark_menus(dark);
    }

    /// Applies the configured DWM material to a window.
    pub fn apply_material(&self, hwnd: HWND) {
        let (material, corner) = {
            let c = self.cfg();
            (c.appearance.material, c.appearance.corner)
        };
        let dark = self.theme.borrow().dark;
        self.applied.set(win::material::apply(hwnd, material, corner, dark));
        // A window that was never activated has never told DWM it is "active",
        // so system backdrops stay on their solid fallback. Our WM_NCACTIVATE
        // handler always forwards TRUE; send one now (also refreshes the
        // backdrop after the user toggles transparency effects).
        unsafe {
            SendMessageW(hwnd, WM_NCACTIVATE, Some(WPARAM(1)), Some(LPARAM(0)));
        }
    }

    /// The tint layer depends on what DWM actually gave us.
    pub fn tint_color(&self, th: &Theme) -> Rgba {
        let tint = self.cfg().appearance.tint;
        match self.applied.get() {
            Applied::SystemBackdrop => th.tint,
            Applied::AccentBlur => th.tint.with_alpha(tint.max(0.45)),
            Applied::Solid => th.tint.with_alpha(0.94),
        }
    }

    fn restyle_all(&self) {
        self.refresh_theme();
        self.metrics.set(TileMetrics::new(&self.cfg().appearance));
        self.icons.borrow_mut().clear();
        self.renderer.clear_text_cache();
        for g in self.views() {
            let h = g.hwnd();
            if !h.is_invalid() {
                self.apply_material(h);
            }
            g.reset_visuals();
        }
    }

    // ---- grids --------------------------------------------------------------

    /// Desktop grids plus the drawer (if any).
    fn views(&self) -> Vec<Rc<Grid>> {
        let mut v = self.grids.borrow().clone();
        if let Some(d) = self.drawer.borrow().as_ref() {
            v.push(d.clone());
        }
        v
    }

    fn find_grid(&self, id: &str) -> Option<Rc<Grid>> {
        self.grids.borrow().iter().find(|g| g.id == id).cloned()
    }

    /// Makes the window set match the configuration (start-up, hot reload, edits).
    pub fn reconcile(&self) {
        let cfg = self.cfg().clone();
        let look = (cfg.appearance.clone(), cfg.general.theme);
        let look_changed = *self.last_look.borrow() != look;
        if look_changed {
            *self.last_look.borrow_mut() = look;
            self.restyle_all();
        }
        crate::util::set_log_enabled(cfg.general.log);
        sync_autostart(cfg.general.autostart);
        self.sync_hotkey(&cfg.general.hotkey);
        self.sync_sort(&cfg.sort);

        if cfg.general.mode == Mode::Drawer {
            // 100 % clean desktop: no desktop grids, one drawer on demand.
            self.end_peek();
            for g in std::mem::take(&mut *self.grids.borrow_mut()) {
                g.destroy_window();
            }
            self.ensure_drawer(&cfg);
            return;
        }
        self.teardown_drawer();

        // Removed grids.
        let existing = self.grids.borrow().clone();
        for g in &existing {
            if !cfg.grids.iter().any(|c| c.id == g.id) {
                self.grids.borrow_mut().retain(|x| x.id != g.id);
                g.destroy_window();
            }
        }
        // New / changed grids.
        let monitors = monitors::enumerate();
        for (index, gc) in cfg.grids.iter().enumerate() {
            match self.find_grid(&gc.id) {
                Some(g) => self.update_grid(&g, gc, &monitors),
                None => self.create_grid(gc.clone(), index, &monitors),
            }
        }
        // Appearance changes can alter which items are shown.
        if look_changed {
            for g in self.grids.borrow().clone() {
                g.reload();
            }
        }
    }

    fn placement(&self, gc: &GridCfg, index: usize, mons: &[monitors::Monitor]) -> (RECT, bool) {
        if let Some(r) = gc.rect
            && let Some(m) = monitors::by_id(mons, &gc.monitor).or_else(|| monitors::primary(mons))
        {
            return (monitors::to_screen(m, r), false);
        }
        let Some(m) = monitors::primary(mons) else {
            return (RECT { left: 100, top: 100, right: 380, bottom: 500 }, true);
        };
        let s = m.scale();
        let w = (theme::DEFAULT_W * s) as i32;
        let h = (theme::DEFAULT_H * s) as i32;
        let gap = (16.0 * s) as i32;
        let col = index as i32;
        let x = (m.work.right - (w + gap) * (col + 1)).max(m.work.left + gap);
        let y = m.work.top + gap;
        (RECT { left: x, top: y, right: x + w, bottom: y + h }, true)
    }

    fn create_grid(&self, gc: GridCfg, index: usize, mons: &[monitors::Monitor]) {
        let token = self.next_token.get();
        self.next_token.set(token + 1);
        let (rect, auto) = self.placement(&gc, index, mons);
        let g = Grid::new(gc, token, Kind::Desktop);
        if !g.create_window(rect) {
            return;
        }
        self.grids.borrow_mut().push(g.clone());
        g.reload();
        self.watch_grid(&g);
        if auto {
            g.persist_rect();
        }
    }

    pub fn watch_view(&self, g: &Grid) {
        self.watch_grid(g);
    }

    fn watch_grid(&self, g: &Grid) {
        let dir = g.st.borrow().dir.clone();
        let old = g.st.borrow_mut().watch.take();
        if let Some(w) = old {
            watcher::unwatch(w);
        }
        let id = watcher::watch(&dir, g.token, false);
        g.st.borrow_mut().watch = id;
    }

    fn update_grid(&self, g: &Rc<Grid>, gc: &GridCfg, mons: &[monitors::Monitor]) {
        let (path_changed, rect_changed, title_changed, mode_changed) = {
            let st = g.st.borrow();
            (
                st.cfg.path != gc.path,
                st.cfg.rect != gc.rect || st.cfg.monitor != gc.monitor,
                st.cfg.title != gc.title,
                st.cfg.collapse != gc.collapse
                    || (gc.collapse == crate::config::CollapseMode::Click && st.cfg.collapsed != gc.collapsed),
            )
        };
        {
            let mut st = g.st.borrow_mut();
            st.cfg = gc.clone();
            if path_changed {
                st.dir = crate::config::expand_path(&gc.path);
            }
        }
        if g.hwnd().is_invalid() {
            let (rect, _) = self.placement(gc, 0, mons);
            g.create_window(rect);
        } else if rect_changed && gc.rect.is_some() {
            let (r, _) = self.placement(gc, 0, mons);
            g.place(r);
        }
        if mode_changed {
            g.apply_mode();
        }
        if path_changed {
            g.reload();
            self.watch_grid(g);
        } else if title_changed {
            render::invalidate(g.hwnd());
        }
    }

    /// Re-applies stored positions (monitor plugged/unplugged, work area changed).
    fn replace_all(&self) {
        let mons = monitors::enumerate();
        if self.cfg().general.mode == Mode::Drawer {
            let cfg = self.cfg().clone();
            self.sync_edge(&cfg);
        }
        for g in self.grids.borrow().clone() {
            let gc = g.st.borrow().cfg.clone();
            let h = g.hwnd();
            if h.is_invalid() || gc.rect.is_none() {
                continue;
            }
            let (r, _) = self.placement(&gc, 0, &mons);
            g.place(r);
        }
    }

    /// Explorer restarted: re-attach (or re-create) every grid and the tray icon.
    fn on_explorer_restart(&self) {
        crate::log!("Explorer restarted — re-attaching");
        desktop::invalidate();
        self.add_tray();
        if self.cfg().general.mode == Mode::Drawer {
            let cfg = self.cfg().clone();
            self.sync_edge(&cfg);
        }
        let mons = monitors::enumerate();
        for (i, g) in self.grids.borrow().clone().iter().enumerate() {
            let h = g.hwnd();
            if h.is_invalid() || !unsafe { IsWindow(Some(h)).as_bool() } {
                let gc = g.st.borrow().cfg.clone();
                let (rect, _) = self.placement(&gc, i, &mons);
                g.create_window(rect);
            } else {
                desktop::attach(h);
            }
        }
    }

    // ---- auto filing ------------------------------------------------------------

    fn journal_path(&self) -> std::path::PathBuf {
        self.store.borrow().path.with_file_name("deskcrate-undo.log")
    }

    /// Watches the configured folders while auto filing is enabled.
    fn sync_sort(&self, sc: &crate::sort::SortCfg) {
        if self.sort_cfg.borrow().as_ref() == Some(sc) {
            return;
        }
        *self.sort_cfg.borrow_mut() = Some(sc.clone());
        for w in self.sort_watches.borrow_mut().drain(..) {
            watcher::unwatch(w);
        }
        unsafe {
            let _ = KillTimer(Some(self.host), TIMER_SORT_PERIODIC);
        }
        self.sort_notified.set(0);
        if !sc.enabled || sc.rules.is_empty() {
            return;
        }
        for (i, dir) in sc.watch.iter().enumerate() {
            let dir = crate::config::expand_path(dir);
            match watcher::watch(&dir, SORT_TOKEN_BASE + i as u64, false) {
                Some(id) => self.sort_watches.borrow_mut().push(id),
                None => crate::log!("sort: cannot watch {dir}"),
            }
        }
        unsafe {
            // First pass shortly after start/enable; idle-day rules need a slow re-check.
            SetTimer(Some(self.host), TIMER_SORT, (sc.settle_seconds * 1000).max(5000), None);
            if sc.rules.iter().any(|r| r.idle_days > 0) {
                SetTimer(Some(self.host), TIMER_SORT_PERIODIC, SORT_PERIOD_MS, None);
            }
        }
    }

    fn sort_plan(&self) -> Vec<crate::sort::Move> {
        let sc = self.cfg().sort.clone();
        crate::sort::plan(&sc, crate::config::expand_path)
    }

    /// Event-driven pass (only when `[sort] enabled = true`).
    fn auto_sort(&self) {
        let sc = self.cfg().sort.clone();
        if !sc.enabled {
            return;
        }
        let plan = self.sort_plan();
        if plan.is_empty() {
            self.sort_notified.set(0);
            return;
        }
        if sc.dry_run {
            if plan.len() != self.sort_notified.get() {
                self.sort_notified.set(plan.len());
                tray::balloon(
                    self.host,
                    "DeskCrate 归档预览",
                    &format!("{} 个文件符合归档规则（dry_run，未移动）。托盘 →「立即整理…」可查看并执行。", plan.len()),
                );
            }
            return;
        }
        let r = crate::sort::execute(&plan, &self.journal_path());
        crate::log!("sort: moved {}, skipped {}", r.moved.len(), r.skipped.len());
        if !r.moved.is_empty() {
            tray::balloon(
                self.host,
                "DeskCrate 已自动归档",
                &format!("已移动 {} 个文件，可在托盘「撤销上次归档」。", r.moved.len()),
            );
        }
    }

    /// Tray command: preview, confirm, execute.
    fn manual_sort(&self) {
        let plan = self.sort_plan();
        if plan.is_empty() {
            tray::balloon(self.host, "DeskCrate", "没有符合归档规则的文件。");
            return;
        }
        let mut text = String::from("将按规则移动以下文件（不会删除或覆盖任何文件，可撤销）：\n\n");
        for m in plan.iter().take(20) {
            let name = m.from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let dest = m.to.parent().map(|p| p.display().to_string()).unwrap_or_default();
            text.push_str(&format!("[{}] {name}  →  {dest}\n", m.rule));
        }
        if plan.len() > 20 {
            text.push_str(&format!("…以及另外 {} 个文件\n", plan.len() - 20));
        }
        text.push_str(&format!("\n共 {} 个文件，确定执行吗？", plan.len()));
        let r = unsafe {
            MessageBoxW(
                Some(self.host),
                &HSTRING::from(text),
                w!("DeskCrate · 整理预览"),
                MB_OKCANCEL | MB_ICONQUESTION,
            )
        };
        if r != IDOK {
            return;
        }
        let rep = crate::sort::execute(&plan, &self.journal_path());
        self.sort_notified.set(0);
        let mut msg = format!("已移动 {} 个文件。", rep.moved.len());
        if !rep.skipped.is_empty() {
            msg.push_str(&format!("跳过 {} 个（占用或已变化）。", rep.skipped.len()));
        }
        tray::balloon(self.host, "DeskCrate 整理完成", &msg);
    }

    fn undo_sort(&self) {
        let r = crate::sort::undo_last(&self.journal_path());
        let mut msg = format!("已移回 {} 个文件。", r.moved.len());
        if !r.skipped.is_empty() {
            msg.push_str(&format!("{} 个无法移回（已被移动或删除）。", r.skipped.len()));
        }
        tray::balloon(self.host, "DeskCrate 已撤销归档", &msg);
    }

    // ---- drawer mode ------------------------------------------------------------

    fn drawer_tabs(cfg: &Config) -> Vec<Tab> {
        cfg.grids
            .iter()
            .map(|g| {
                let dir = crate::config::expand_path(&g.path);
                let title = if g.title.is_empty() {
                    dir.rsplit('\\').find(|s| !s.is_empty()).unwrap_or(&dir).to_owned()
                } else {
                    g.title.clone()
                };
                Tab { title, path: g.path.clone() }
            })
            .collect()
    }

    fn ensure_drawer(&self, cfg: &Config) {
        let existing = self.drawer.borrow().clone();
        let d = match existing {
            Some(d) if !d.hwnd().is_invalid() => d,
            _ => {
                let token = self.next_token.get();
                self.next_token.set(token + 1);
                let gc = GridCfg {
                    id: "__drawer".into(),
                    title: String::new(),
                    path: cfg.grids.first().map(|g| g.path.clone()).unwrap_or_default(),
                    monitor: String::new(),
                    rect: None,
                    collapse: crate::config::CollapseMode::Expanded,
                    collapsed: false,
                    locked: true,
                };
                let d = Grid::new(gc, token, Kind::Drawer);
                let r = RECT { left: 0, top: 0, right: 380, bottom: 600 };
                if !d.create_window(r) {
                    return;
                }
                *self.drawer.borrow_mut() = Some(d.clone());
                d
            }
        };
        d.set_tabs(Self::drawer_tabs(cfg));
        self.sync_edge(cfg);
    }

    fn teardown_drawer(&self) {
        if let Some(d) = self.drawer.borrow_mut().take() {
            d.destroy_window();
        }
        self.sync_edge_off();
    }

    fn sync_edge_off(&self) {
        let e = self.edge.replace(HWND::default());
        if !e.is_invalid() {
            unsafe {
                let _ = DestroyWindow(e);
            }
        }
        if self.appbar.replace(false) {
            edge::appbar_unregister(self.host);
        }
    }

    /// (Re)creates the edge strip for the current monitor layout.
    fn sync_edge(&self, cfg: &Config) {
        self.sync_edge_off();
        let mons = monitors::enumerate();
        let Some(m) = monitors::primary(&mons) else { return };
        let Some(r) = edge::strip_rect(m, cfg.drawer.edge, cfg.drawer.hot_zone) else { return };
        if let Some(h) = edge::create(hinstance(), r) {
            self.edge.set(h);
            edge::appbar_register(self.host, WM_APP_APPBAR);
            self.appbar.set(true);
        }
    }

    fn drawer_rect(&self) -> (RECT, Edge) {
        let cfg = self.cfg();
        let edge = cfg.drawer.edge;
        let width = cfg.drawer.width;
        drop(cfg);
        let mons = monitors::enumerate();
        let mut cursor = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cursor);
        }
        let Some(m) = monitors::for_point(&mons, cursor) else {
            return (RECT { left: 100, top: 100, right: 480, bottom: 700 }, edge);
        };
        let s = m.scale();
        let gap = (8.0 * s) as i32;
        let wk = m.work;
        let w = ((width * s) as i32).min(wk.right - wk.left - 2 * gap);
        let r = match edge {
            Edge::Left => {
                RECT { left: wk.left + gap, top: wk.top + gap, right: wk.left + gap + w, bottom: wk.bottom - gap }
            }
            Edge::Top => {
                let tw = ((wk.right - wk.left) as f32 * 0.6) as i32;
                let x = wk.left + ((wk.right - wk.left) - tw) / 2;
                RECT {
                    left: x,
                    top: wk.top + gap,
                    right: x + tw,
                    bottom: wk.top + ((wk.bottom - wk.top) as f32 * 0.62) as i32,
                }
            }
            _ => RECT { left: wk.right - gap - w, top: wk.top + gap, right: wk.right - gap, bottom: wk.bottom - gap },
        };
        (r, edge)
    }

    pub fn show_drawer(&self, by_key: bool) {
        let Some(d) = self.drawer.borrow().clone() else { return };
        if d.is_shown() {
            if by_key {
                unsafe {
                    let _ = SetForegroundWindow(d.hwnd());
                }
            }
            return;
        }
        let (r, edge) = self.drawer_rect();
        d.reload();
        d.reveal(r, edge, by_key);
    }

    pub fn hide_drawer(&self) {
        if let Some(d) = self.drawer.borrow().clone() {
            d.conceal();
        }
    }

    fn toggle_drawer(&self) {
        let shown = self.drawer.borrow().as_ref().is_some_and(|d| d.is_shown());
        if shown { self.hide_drawer() } else { self.show_drawer(true) }
    }

    /// Desktop mode hotkey: lift every grid above all windows until focus leaves.
    fn toggle_peek(&self) {
        if self.peeking.get() {
            self.end_peek();
            return;
        }
        let grids = self.grids.borrow().clone();
        if grids.is_empty() {
            return;
        }
        self.peeking.set(true);
        for g in &grids {
            unsafe {
                let _ =
                    SetWindowPos(g.hwnd(), Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
        unsafe {
            let _ = SetForegroundWindow(grids[0].hwnd());
        }
    }

    fn end_peek(&self) {
        if !self.peeking.replace(false) {
            return;
        }
        for g in self.grids.borrow().clone() {
            let h = g.hwnd();
            unsafe {
                let _ = SetWindowPos(h, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            desktop::rebed(h);
        }
    }

    /// A grid lost focus to a foreign window.
    pub fn on_focus_left(&self) {
        self.end_peek();
    }

    fn sync_hotkey(&self, spec: &str) {
        if *self.hotkey.borrow() == spec {
            return;
        }
        hotkey::unregister(self.host, HOTKEY_ID);
        *self.hotkey.borrow_mut() = spec.to_owned();
        if spec.trim().is_empty() {
            return;
        }
        if !hotkey::register(self.host, HOTKEY_ID, spec) {
            crate::log!("hotkey '{spec}' unavailable");
            tray::balloon(
                self.host,
                "DeskCrate 热键不可用",
                &format!("“{spec}” 无效或已被其他程序占用，可在配置文件 [general] hotkey 中修改。"),
            );
        }
    }

    /// Edge/peer snapping while dragging a grid (screen px).
    pub fn snap_rect(&self, id: &str, r: RECT) -> RECT {
        let mons = monitors::enumerate();
        let Some(m) = monitors::for_rect(&mons, &r) else { return r };
        let s = m.scale();
        let th = (theme::SNAP_DISTANCE * s) as i32;
        let gap = (theme::SNAP_GAP * s).round() as i32;
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let mut xs = vec![m.work.left + gap, m.work.right - gap - w];
        let mut ys = vec![m.work.top + gap, m.work.bottom - gap - h];
        for g in self.grids.borrow().iter() {
            if g.id == id || g.hwnd().is_invalid() {
                continue;
            }
            let o = win::window_rect(g.hwnd());
            if o.top - th < r.bottom && r.top < o.bottom + th {
                xs.extend([o.right + gap, o.left - gap - w, o.left, o.right - w]);
            }
            if o.left - th < r.right && r.left < o.right + th {
                ys.extend([o.bottom + gap, o.top - gap - h, o.top, o.bottom - h]);
            }
        }
        let pick = |v: i32, c: &[i32]| {
            c.iter().copied().filter(|t| (t - v).abs() <= th).min_by_key(|t| (t - v).abs()).unwrap_or(v)
        };
        let x = pick(r.left, &xs);
        let y = pick(r.top, &ys);
        RECT { left: x, top: y, right: x + w, bottom: y + h }
    }

    pub fn confirm_remove_grid(&self, id: &str) {
        let Some(g) = self.find_grid(id) else { return };
        let text = format!(
            "移除格子「{}」？\n\n只移除桌面上的这个格子，文件夹「{}」及其中的文件保持不变。",
            g.title(),
            g.st.borrow().dir
        );
        let r = unsafe {
            MessageBoxW(Some(self.host), &HSTRING::from(text), w!("DeskCrate"), MB_OKCANCEL | MB_ICONQUESTION)
        };
        if r == IDOK {
            self.store.borrow_mut().remove_grid(id);
            self.reconcile();
        }
    }

    fn new_grid(&self) {
        let Some(path) = pick_folder(self.host) else { return };
        let title = path.rsplit('\\').find(|s| !s.is_empty()).unwrap_or(&path).to_owned();
        self.store.borrow_mut().add_grid(&title, &path);
        self.reconcile();
    }

    /// Releases transient memory a few seconds after the last paint.
    pub fn schedule_idle_trim(&self) {
        unsafe { SetTimer(Some(self.host), TIMER_TRIM, 4000, None) };
    }

    fn idle_trim(&self) {
        self.icons.borrow_mut().drop_bitmaps();
        for g in self.views() {
            g.drop_text_cache();
        }
        self.renderer.release();

        crate::util::log_mem(&format!("idle icons={}KB", self.icons.borrow().bytes() / 1024));
        unsafe {
            let _ = K32EmptyWorkingSet(GetCurrentProcess());
        }
    }

    pub fn shutdown(&self) {
        crate::log!("shutdown");
        let grids = std::mem::take(&mut *self.grids.borrow_mut());
        for g in grids {
            g.destroy_window();
        }
        self.teardown_drawer();
        hotkey::unregister(self.host, HOTKEY_ID);
        tray::remove(self.host);
        tray::destroy_icon(self.tray_icon.replace(HICON::default()));
        unsafe {
            let _ = DestroyWindow(self.host);
        }
    }

    // ---- host window --------------------------------------------------------

    fn tray_menu(&self) {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let autostart = self.cfg().general.autostart;
        let drawer_mode = self.cfg().general.mode == Mode::Drawer;
        let mut items = vec![
            Extra::item(CMD_NEW, "新建格子…"),
            Extra::item(CMD_REFRESH_ALL, "刷新全部"),
            Extra::Separator,
            Extra::check(CMD_DRAWER_MODE, "抽屉模式（桌面保持空白）", drawer_mode),
        ];
        if drawer_mode {
            items.push(Extra::item(CMD_SHOW_DRAWER, "打开抽屉"));
        }
        let has_rules = !self.cfg().sort.rules.is_empty();
        let undo_n = crate::sort::last_batch_len(&self.journal_path());
        if has_rules || undo_n > 0 {
            items.push(Extra::Separator);
        }
        if has_rules {
            items.push(Extra::item(CMD_SORT_NOW, "立即整理…（先预览）"));
        }
        if undo_n > 0 {
            items.push(Extra::item(CMD_SORT_UNDO, &format!("撤销上次归档（{undo_n} 个文件）")));
        }
        items.extend([
            Extra::Separator,
            Extra::item(CMD_EDIT_CONFIG, "编辑配置文件"),
            Extra::item(CMD_OPEN_CONFIG_DIR, "打开配置所在文件夹"),
            Extra::check(CMD_AUTOSTART, "开机自启", autostart),
            Extra::Separator,
            Extra::item(CMD_ABOUT, "关于 DeskCrate"),
            Extra::item(CMD_EXIT, "退出"),
        ]);
        match menu::track_plain(self.host, pt, &items) {
            CMD_NEW => self.new_grid(),
            CMD_REFRESH_ALL => {
                self.icons.borrow_mut().clear();
                for g in self.views() {
                    g.reload();
                }
            }
            CMD_DRAWER_MODE => {
                let next = if drawer_mode { "desktop" } else { "drawer" };
                self.store.borrow_mut().set_value("general", "mode", next.into());
                self.reconcile();
            }
            CMD_SHOW_DRAWER => self.show_drawer(true),
            CMD_SORT_NOW => self.manual_sort(),
            CMD_SORT_UNDO => self.undo_sort(),
            CMD_EDIT_CONFIG => {
                let p = self.store.borrow().path.to_string_lossy().into_owned();
                if !crate::shell::open_with_verb(&p, "edit") && !crate::shell::open_path(&p, None) {
                    let _ = std::process::Command::new("notepad.exe").arg(&p).spawn();
                }
            }
            CMD_OPEN_CONFIG_DIR => {
                let p = self.store.borrow().path.to_string_lossy().into_owned();
                let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{p}")).spawn();
            }
            CMD_AUTOSTART => {
                self.store.borrow_mut().set_value("general", "autostart", (!autostart).into());
                sync_autostart(!autostart);
            }
            CMD_ABOUT => {
                let text = format!(
                    "DeskCrate {}\n轻量、原生的 Windows 桌面文件收纳工具。\n\n\
                     基于 DeskBox（github.com/Tianyu199509/DeskBox，GPL-3.0-only）的非官方 Rust 重写，\
                     与原作者无关联，也未获其背书。\n许可证：GPL-3.0-only\n\n配置文件：{}",
                    env!("CARGO_PKG_VERSION"),
                    self.store.borrow().path.display()
                );
                unsafe {
                    MessageBoxW(
                        Some(self.host),
                        &HSTRING::from(text),
                        w!("关于 DeskCrate"),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
            }
            CMD_EXIT => self.shutdown(),
            _ => {}
        }
    }

    fn on_fs_events(&self) {
        for ev in watcher::take_events() {
            if ev.token == CONFIG_TOKEN {
                if ev.gone {
                    self.config_watch.set(None);
                    continue;
                }
                let hit = ev.overflow || ev.changes.iter().any(|(_, n)| n.eq_ignore_ascii_case("config.toml"));
                if hit {
                    unsafe { SetTimer(Some(self.host), TIMER_CONFIG, 250, None) };
                }
                continue;
            }
            if (SORT_TOKEN_BASE..CONFIG_TOKEN).contains(&ev.token) {
                let settle = self.cfg().sort.settle_seconds;
                // Debounce: run once the folder has been quiet for `settle` seconds.
                unsafe { SetTimer(Some(self.host), TIMER_SORT, settle * 1000 + 500, None) };
                continue;
            }
            let grid = self.views().into_iter().find(|g| g.token == ev.token);
            if let Some(g) = grid {
                if ev.gone {
                    g.st.borrow_mut().watch = None;
                }
                unsafe { SetTimer(Some(self.host), TIMER_FS_BASE + ev.token as usize, 150, None) };
            }
        }
    }

    fn on_timer(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.host), id);
        }
        match id {
            TIMER_CONFIG => {
                let r = self.store.borrow_mut().reload_if_changed();
                match r {
                    Ok(true) => {
                        crate::log!("config reloaded");
                        self.reconcile();
                    }
                    Ok(false) => {}
                    Err(e) => {
                        crate::log!("config reload failed: {e}");
                        tray::balloon(self.host, "DeskCrate 配置有误", &format!("已保留当前设置：{e}"));
                    }
                }
            }
            TIMER_DISPLAY => self.replace_all(),
            TIMER_THEME => {
                self.restyle_all();
                self.add_tray();
            }
            TIMER_TRIM => self.idle_trim(),
            TIMER_SORT => self.auto_sort(),
            TIMER_SORT_PERIODIC => {
                self.auto_sort();
                unsafe { SetTimer(Some(self.host), TIMER_SORT_PERIODIC, SORT_PERIOD_MS, None) };
            }
            t if t >= TIMER_FS_BASE => {
                let token = (t - TIMER_FS_BASE) as u64;
                let grid = self.views().into_iter().find(|g| g.token == token);
                if let Some(g) = grid {
                    g.reload();
                    if g.st.borrow().watch.is_none() && g.st.borrow().error.is_none() {
                        self.watch_grid(&g);
                    }
                }
            }
            _ => {}
        }
    }

    fn handle(&self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if let Some(r) = menu::handle_menu_msg(msg, wp, lp) {
            return Some(r);
        }
        if msg == self.taskbar_created && msg != 0 {
            self.on_explorer_restart();
            return Some(LRESULT(0));
        }
        match msg {
            WM_APP_TRAY => {
                let event = loword(lp.0 as usize) as u32;
                if event == WM_CONTEXTMENU || event == WM_USER || event == WM_LBUTTONUP {
                    self.tray_menu();
                }
                Some(LRESULT(0))
            }
            WM_APP_ICON => {
                if self.icons.borrow_mut().drain() {
                    for g in self.views() {
                        render::invalidate(g.hwnd());
                    }
                }
                Some(LRESULT(0))
            }
            WM_APP_FS => {
                self.on_fs_events();
                Some(LRESULT(0))
            }
            WM_HOTKEY => {
                if self.cfg().general.mode == Mode::Drawer {
                    self.toggle_drawer();
                } else {
                    self.toggle_peek();
                }
                Some(LRESULT(0))
            }
            WM_APP_APPBAR => {
                if wp.0 == ABN_FULLSCREENAPP {
                    edge::set_visible(self.edge.get(), lp.0 == 0);
                }
                Some(LRESULT(0))
            }
            WM_TIMER => {
                self.on_timer(wp.0);
                Some(LRESULT(0))
            }
            WM_DISPLAYCHANGE => {
                unsafe { SetTimer(Some(hwnd), TIMER_DISPLAY, 300, None) };
                Some(LRESULT(0))
            }
            WM_SETTINGCHANGE => {
                if wp.0 == SPI_SETWORKAREA.0 as usize {
                    unsafe { SetTimer(Some(hwnd), TIMER_DISPLAY, 300, None) };
                }
                if lp.0 != 0 {
                    let s = unsafe { crate::util::pcwstr_to_string(PCWSTR(lp.0 as *const u16)) };
                    if s == "ImmersiveColorSet" {
                        unsafe { SetTimer(Some(hwnd), TIMER_THEME, 200, None) };
                    }
                }
                let hide = crate::shell::hide_known_extensions();
                if hide != self.hide_ext.get() {
                    self.hide_ext.set(hide);
                    for g in self.grids.borrow().clone() {
                        g.reload();
                    }
                }
                Some(LRESULT(0))
            }
            WM_DWMCOLORIZATIONCOLORCHANGED => {
                unsafe { SetTimer(Some(hwnd), TIMER_THEME, 200, None) };
                Some(LRESULT(0))
            }
            WM_COPYDATA => {
                let cds = unsafe { &*(lp.0 as *const windows::Win32::System::DataExchange::COPYDATASTRUCT) };
                if cds.dwData & win::everything::REPLY_TAG_MASK == win::everything::REPLY_TAG {
                    let bytes = if cds.lpData.is_null() {
                        &[][..]
                    } else {
                        unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) }
                    };
                    let token = ((cds.dwData >> 12) & 0xFFF) as u64;
                    let qid = (cds.dwData & 0xFFF) as u32;
                    if let Some((total, hits)) = win::everything::parse_reply(bytes)
                        && let Some(g) = self.views().into_iter().find(|g| g.token & 0xFFF == token)
                    {
                        g.on_hits(qid, total, hits);
                    }
                    return Some(LRESULT(1));
                }
                if self.cfg().general.mode == Mode::Drawer {
                    self.show_drawer(true);
                } else {
                    tray::balloon(self.host, "DeskCrate", "DeskCrate 已在运行，右键托盘图标管理格子。");
                }
                Some(LRESULT(1))
            }
            WM_CLOSE => {
                self.shutdown();
                Some(LRESULT(0))
            }
            WM_QUERYENDSESSION => Some(LRESULT(1)),
            WM_ENDSESSION => {
                if wp.0 != 0 {
                    self.shutdown();
                }
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(app) = try_app()
        && hwnd == app.host
        && let Some(r) = app.handle(hwnd, msg, wp, lp)
    {
        return r;
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn pick_folder(owner: HWND) -> Option<String> {
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dlg.GetOptions().ok()?;
        dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST).ok()?;
        let _ = dlg.SetTitle(w!("选择要映射为格子的文件夹"));
        dlg.Show(Some(owner)).ok()?;
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn autostart_registered() -> bool {
    let mut size = 0u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size)).is_ok() }
}

/// The only registry write DeskCrate ever makes — and only when enabled.
fn sync_autostart(enabled: bool) {
    let registered = autostart_registered();
    if enabled && !registered {
        let Ok(exe) = std::env::current_exe() else { return };
        let cmd = wide(&format!("\"{}\"", exe.display()));
        unsafe {
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(cmd.as_ptr() as *const _),
                (cmd.len() * 2) as u32,
            );
        }
    } else if !enabled && registered {
        unsafe {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}

#[allow(dead_code)]
fn material_name(m: Material) -> &'static str {
    match m {
        Material::Mica => "mica",
        Material::MicaAlt => "mica_alt",
        Material::Acrylic => "acrylic",
        Material::Blur => "blur",
        Material::Solid => "solid",
    }
}
