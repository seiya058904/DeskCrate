//! A grid ("crate") window: a live view of one real folder on the desktop layer,
//! foldable into a smart capsule.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_WEIGHT_NORMAL, DWRITE_HIT_TEST_METRICS, IDWriteTextLayout};
use windows::Win32::Graphics::Gdi::{BeginPaint, ClientToScreen, EndPaint, PAINTSTRUCT, ScreenToClient};
use windows::Win32::System::Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop};
use windows::Win32::UI::Input::Ime::{
    CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext, ImmSetCompositionWindow,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_HOVER, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_A,
    VK_BACK, VK_C, VK_CONTROL, VK_DELETE, VK_END, VK_ESCAPE, VK_F2, VK_F5, VK_HOME, VK_LEFT, VK_MENU, VK_RETURN,
    VK_RIGHT, VK_SHIFT, VK_V, VK_X,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::BOOL;

use crate::app::{self, app};
use crate::config::{CollapseMode, GridCfg};
use crate::layout::{GridLayout, RectF, TileMetrics};
use crate::shell;
use crate::shell::dragdrop::{DropHost, GridDropTarget};
use crate::shell::items::Item;
use crate::shell::menu::{self, EXTRA_BASE, Extra, Outcome};
use crate::theme::{self, Rgba};
use crate::ui::render::{self, Align, Canvas, Wrap};
use crate::ui::textinput::{TextInput, valid_file_name};
use crate::util::{x_lparam, y_lparam};
use crate::win::{self, desktop, monitors};

pub const CLASS: windows::core::PCWSTR = windows::core::w!("DeskCrateGrid");

const CMD_OPEN_FOLDER: u32 = EXTRA_BASE + 1;
const CMD_REFRESH: u32 = EXTRA_BASE + 2;
const CMD_LOCK: u32 = EXTRA_BASE + 3;
const CMD_REMOVE: u32 = EXTRA_BASE + 4;
const CMD_MODE_EXPANDED: u32 = EXTRA_BASE + 5;
const CMD_MODE_CLICK: u32 = EXTRA_BASE + 6;
const CMD_MODE_SMART: u32 = EXTRA_BASE + 7;

/// Not exported by windows-rs' WindowsAndMessaging module.
const WM_MOUSEHOVER: u32 = 0x02A1;
const WM_MOUSELEAVE: u32 = 0x02A3;

const TIMER_ANIM: usize = 1;
const TIMER_COLLAPSE: usize = 2;
const TIMER_SLIDE: usize = 3;
const TIMER_HOVER_HIDE: usize = 4;
const TIMER_QUERY: usize = 5;

/// "Everything" results section (DIP).
const HITS_HEADER_H: f32 = 26.0;
const HITS_ROW_H: f32 = 28.0;
const HITS_MAX: u32 = 40;

/// Desktop grid, or the aggregated drawer (one tab per configured grid).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Desktop,
    Drawer,
}

#[derive(Clone, Debug)]
pub struct Tab {
    pub title: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug)]
struct Slide {
    from: (i32, i32),
    to: (i32, i32),
    start: u64,
    dur: u32,
    hide: bool,
}

const GLYPH_FOLDER: char = '\u{E8B7}';
const GLYPH_MORE: char = '\u{E712}';
const GLYPH_UP: char = '\u{E70E}';
const GLYPH_DOWN: char = '\u{E70D}';
const GLYPH_SEARCH: char = '\u{E721}';
const GLYPH_CLEAR: char = '\u{E711}';

#[derive(Clone, Copy, PartialEq, Debug)]
enum Button {
    More,
    Fold,
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    None,
    Pending { x: i32, y: i32, index: Option<usize>, title: bool, button: Option<Button> },
    Move { cursor: POINT, rect: RECT },
    Marquee { x0: f32, y0: f32, x1: f32, y1: f32 },
    PendingHit { x: i32, y: i32, i: usize },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fold {
    Expanded,
    Collapsed,
    Animating { from: i32, to: i32, start: u64, dur: u32, collapsing: bool },
}

struct Edit {
    index: usize,
    input: TextInput,
    /// Re-appended on commit when Explorer hides the extension.
    hidden_ext: Option<String>,
    /// Last laid-out text + origin (DIP), for caret hit-testing.
    layout: Option<(IDWriteTextLayout, f32, f32)>,
    surrogate: Option<u16>,
}

pub struct GridState {
    pub cfg: GridCfg,
    pub dir: String,
    /// Everything in the folder; `items` is the (filtered) view.
    all_items: Vec<Item>,
    pub items: Vec<Item>,
    filter: Option<TextInput>,
    filter_surrogate: Option<u16>,
    hits: Vec<win::everything::Hit>,
    hits_total: u32,
    hit_sel: Option<usize>,
    hit_hover: Option<usize>,
    query_id: u32,
    labels: Vec<Option<IDWriteTextLayout>>,
    selected: Vec<bool>,
    anchor: Option<usize>,
    hover: Option<usize>,
    hover_button: Option<Button>,
    inside: bool,
    scroll: f32,
    pub dpi: u32,
    size: (i32, i32),
    drag: Drag,
    active: bool,
    drop_hl: Option<Option<usize>>,
    fold: Fold,
    anchor_bottom: bool,
    edit: Option<Edit>,
    pending_select: Option<String>,
    tabs: Vec<Tab>,
    tab: usize,
    hover_tab: Option<usize>,
    tab_rects: Vec<RectF>,
    slide: Option<Slide>,
    revealed_by_key: bool,
    outside_ticks: u8,
    pub watch: Option<win::watcher::WatchId>,
    pub error: Option<String>,
}

pub struct Grid {
    pub id: String,
    pub kind: Kind,
    pub token: u64,
    pub hwnd: Cell<HWND>,
    pub st: RefCell<GridState>,
    weak: RefCell<Weak<Grid>>,
}

fn key_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

impl Grid {
    pub fn new(cfg: GridCfg, token: u64, kind: Kind) -> Rc<Grid> {
        let dir = crate::config::expand_path(&cfg.path);
        let start_collapsed = match cfg.collapse {
            CollapseMode::Expanded => false,
            CollapseMode::Click => cfg.collapsed,
            CollapseMode::Smart => true,
        };
        let g = Rc::new(Grid {
            id: cfg.id.clone(),
            kind,
            token,
            hwnd: Cell::new(HWND::default()),
            st: RefCell::new(GridState {
                cfg,
                dir,
                all_items: Vec::new(),
                items: Vec::new(),
                filter: None,
                filter_surrogate: None,
                hits: Vec::new(),
                hits_total: 0,
                hit_sel: None,
                hit_hover: None,
                query_id: 0,
                labels: Vec::new(),
                selected: Vec::new(),
                anchor: None,
                hover: None,
                hover_button: None,
                inside: false,
                scroll: 0.0,
                dpi: 96,
                size: (0, 0),
                drag: Drag::None,
                active: false,
                drop_hl: None,
                fold: if start_collapsed { Fold::Collapsed } else { Fold::Expanded },
                anchor_bottom: false,
                edit: None,
                pending_select: None,
                tabs: Vec::new(),
                tab: 0,
                hover_tab: None,
                tab_rects: Vec::new(),
                slide: None,
                revealed_by_key: false,
                outside_ticks: 0,
                watch: None,
                error: None,
            }),
            weak: RefCell::new(Weak::new()),
        });
        *g.weak.borrow_mut() = Rc::downgrade(&g);
        g
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd.get()
    }

    /// Creates the native window for the *expanded* rect (screen px), folds it if
    /// needed, and attaches it to the desktop layer.
    pub fn create_window(self: &Rc<Self>, expanded: RECT) -> bool {
        let drawer = self.kind == Kind::Drawer;
        let raw = Rc::into_raw(self.clone());
        let ex = if drawer { WS_EX_TOOLWINDOW | WS_EX_TOPMOST } else { WS_EX_TOOLWINDOW };
        let hwnd = unsafe {
            CreateWindowExW(
                ex,
                CLASS,
                &windows::core::HSTRING::from(self.title()),
                WS_POPUP | WS_CLIPCHILDREN,
                expanded.left,
                expanded.top,
                expanded.right - expanded.left,
                expanded.bottom - expanded.top,
                None,
                None,
                Some(app::hinstance()),
                Some(raw as *const _),
            )
        };
        let Ok(hwnd) = hwnd else {
            unsafe { drop(Rc::from_raw(raw)) };
            crate::log!("grid {}: CreateWindowEx failed", self.id);
            return false;
        };
        self.st.borrow_mut().dpi = win::dpi_of(hwnd);
        app().apply_material(hwnd);
        let host: Weak<dyn DropHost> = self.weak.borrow().clone();
        let target: IDropTarget = GridDropTarget::create(hwnd, host);
        unsafe {
            if let Err(e) = RegisterDragDrop(hwnd, &target) {
                crate::log!("grid {}: RegisterDragDrop failed {e:?}", self.id);
            }
        }
        if drawer {
            // Shown on demand by `reveal`.
            return true;
        }
        self.place(expanded);
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        desktop::attach(hwnd);
        true
    }

    pub fn destroy_window(&self) {
        let h = self.hwnd();
        if !h.is_invalid() {
            unsafe {
                let _ = DestroyWindow(h);
            }
        }
    }

    pub fn title(&self) -> String {
        let st = self.st.borrow();
        Self::title_of(&st)
    }

    fn title_of(st: &GridState) -> String {
        if let Some(t) = st.tabs.get(st.tab) {
            return t.title.clone();
        }
        if !st.cfg.title.is_empty() {
            return st.cfg.title.clone();
        }
        st.dir.rsplit('\\').find(|s| !s.is_empty()).unwrap_or(&st.dir).to_owned()
    }

    /// Re-reads the folder, keeping selection by name.
    pub fn reload(&self) {
        let (dir, show_hidden) = {
            let st = self.st.borrow();
            (st.dir.clone(), app().cfg().appearance.show_hidden)
        };
        let result = shell::items::list(&dir, show_hidden, app().hide_ext.get());
        {
            let mut st = self.st.borrow_mut();
            let mut keep: std::collections::HashSet<String> =
                st.items.iter().zip(&st.selected).filter(|(_, s)| **s).map(|(i, _)| i.name.clone()).collect();
            if let Some(name) = st.pending_select.take() {
                keep.clear();
                keep.insert(name);
            }
            match result {
                Ok(items) => {
                    st.all_items = items;
                    st.error = None;
                    Self::refilter(&mut st);
                    st.selected = st.items.iter().map(|i| keep.contains(&i.name)).collect();
                }
                Err(code) => {
                    st.all_items.clear();
                    Self::refilter(&mut st);
                    st.error = Some(if code == 2 || code == 3 {
                        "找不到文件夹".to_owned()
                    } else {
                        format!("无法读取文件夹（错误 {code}）")
                    });
                }
            }
            st.hover = None;
            st.anchor = None;
            // A refresh under an open rename box would move items around it.
            st.edit = None;
        }
        self.clamp_scroll();
        render::invalidate(self.hwnd());
    }

    /// Drops cached text layouts (metrics / theme changed).
    pub fn reset_visuals(&self) {
        self.drop_text_cache();
        self.clamp_scroll();
        render::invalidate(self.hwnd());
    }

    /// Drops cached text layouts so the text stack can be released when idle.
    pub fn drop_text_cache(&self) {
        if let Ok(mut st) = self.st.try_borrow_mut() {
            let n = st.items.len();
            st.labels = vec![None; n];
            if let Some(e) = st.edit.as_mut() {
                e.layout = None;
            }
        }
    }

    // ---- folding (capsule) -----------------------------------------------------

    pub fn is_collapsed(&self) -> bool {
        !matches!(self.st.borrow().fold, Fold::Expanded)
    }

    fn capsule_px(st: &GridState) -> i32 {
        (theme::CAPSULE_H * Self::scale(st)).round() as i32
    }

    /// Expanded height in px (from the stored DIP rect, else the current size).
    fn expanded_px(st: &GridState) -> i32 {
        match st.cfg.rect {
            Some(r) => (r[3] * Self::scale(st)).round() as i32,
            None => (theme::DEFAULT_H * Self::scale(st)).round() as i32,
        }
    }

    /// Capsules docked in the lower half of a screen fold down and expand upward.
    fn decide_anchor(expanded: &RECT) -> bool {
        let mons = monitors::enumerate();
        let Some(m) = monitors::for_rect(&mons, expanded) else { return false };
        (expanded.top + expanded.bottom) / 2 > (m.work.top + m.work.bottom) / 2
    }

    /// Positions the window for an expanded rect, honouring the current fold.
    pub fn place(&self, expanded: RECT) {
        let hwnd = self.hwnd();
        if hwnd.is_invalid() {
            return;
        }
        let bottom = Self::decide_anchor(&expanded);
        let r = {
            let mut st = self.st.borrow_mut();
            st.anchor_bottom = bottom;
            match st.fold {
                Fold::Expanded => expanded,
                _ => {
                    st.fold = Fold::Collapsed;
                    let h = Self::capsule_px(&st);
                    if bottom {
                        RECT { top: expanded.bottom - h, ..expanded }
                    } else {
                        RECT { bottom: expanded.top + h, ..expanded }
                    }
                }
            }
        };
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_ANIM);
            let _ = SetWindowPos(
                hwnd,
                None,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// The expanded rect implied by the current window position.
    fn expanded_rect(&self) -> RECT {
        let r = win::window_rect(self.hwnd());
        let st = self.st.borrow();
        if matches!(st.fold, Fold::Expanded) {
            return r;
        }
        let eh = Self::expanded_px(&st);
        if st.anchor_bottom { RECT { top: r.bottom - eh, ..r } } else { RECT { bottom: r.top + eh, ..r } }
    }

    pub fn set_collapsed(&self, collapse: bool, animate: bool) {
        let hwnd = self.hwnd();
        if hwnd.is_invalid() || self.kind == Kind::Drawer {
            return;
        }
        let currently = self.is_collapsed();
        let animating = matches!(self.st.borrow().fold, Fold::Animating { .. });
        if currently == collapse && !animating {
            return;
        }
        let expanded = self.expanded_rect();
        let anchor = Self::decide_anchor(&expanded);
        let dur = app().cfg().capsule.animation_ms;
        let (from, to) = {
            let mut st = self.st.borrow_mut();
            if !matches!(st.fold, Fold::Animating { .. }) {
                st.anchor_bottom = anchor;
            }
            st.edit = None;
            st.hover = None;
            if collapse && st.filter.is_some() {
                st.filter = None;
                st.hits.clear();
                Self::refilter(&mut st);
            }
            (st.size.1, if collapse { Self::capsule_px(&st) } else { Self::expanded_px(&st) })
        };
        if animate && dur > 0 && from != to {
            self.st.borrow_mut().fold =
                Fold::Animating { from, to, start: crate::util::tick_ms(), dur, collapsing: collapse };
            unsafe { SetTimer(Some(hwnd), TIMER_ANIM, 10, None) };
        } else {
            self.st.borrow_mut().fold = if collapse { Fold::Collapsed } else { Fold::Expanded };
            self.apply_height(to);
        }
        let (id, mode) = {
            let st = self.st.borrow();
            (self.id.clone(), st.cfg.collapse)
        };
        if mode == CollapseMode::Click {
            self.st.borrow_mut().cfg.collapsed = collapse;
            app().store.borrow_mut().set_grid_flag(&id, "collapsed", collapse);
        }
        if collapse {
            self.drop_text_cache();
            app().schedule_idle_trim();
        }
    }

    fn apply_height(&self, h: i32) {
        let hwnd = self.hwnd();
        let r = win::window_rect(hwnd);
        let bottom = self.st.borrow().anchor_bottom;
        let top = if bottom { r.bottom - h } else { r.top };
        unsafe {
            let _ = SetWindowPos(hwnd, None, r.left, top, r.right - r.left, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    fn on_anim_tick(&self) {
        let fold = self.st.borrow().fold;
        let Fold::Animating { from, to, start, dur, collapsing } = fold else {
            unsafe {
                let _ = KillTimer(Some(self.hwnd()), TIMER_ANIM);
            }
            return;
        };
        let t = ((crate::util::tick_ms() - start) as f32 / dur.max(1) as f32).min(1.0);
        let h = from + ((to - from) as f32 * ease_out_cubic(t)).round() as i32;
        if t >= 1.0 {
            self.st.borrow_mut().fold = if collapsing { Fold::Collapsed } else { Fold::Expanded };
            unsafe {
                let _ = KillTimer(Some(self.hwnd()), TIMER_ANIM);
            }
        }
        self.apply_height(h);
        render::invalidate(self.hwnd());
    }

    fn smart(&self) -> bool {
        self.st.borrow().cfg.collapse == CollapseMode::Smart
    }

    fn arm_collapse_timer(&self) {
        if self.kind == Kind::Desktop && self.smart() && !self.is_collapsed() {
            let ms = app().cfg().capsule.collapse_delay_ms.max(50);
            unsafe { SetTimer(Some(self.hwnd()), TIMER_COLLAPSE, ms, None) };
        }
    }

    fn on_collapse_timer(&self) {
        let hwnd = self.hwnd();
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_COLLAPSE);
        }
        let busy = {
            let st = self.st.borrow();
            st.edit.is_some() || !matches!(st.drag, Drag::None) || st.drop_hl.is_some()
        };
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        let r = win::window_rect(hwnd);
        let over = p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom;
        if busy || over {
            self.arm_collapse_timer();
            return;
        }
        self.set_collapsed(true, true);
    }

    /// Mode changed by hot reload or menu.
    pub fn apply_mode(&self) {
        let mode = self.st.borrow().cfg.collapse;
        match mode {
            CollapseMode::Expanded => self.set_collapsed(false, true),
            CollapseMode::Smart => {
                if !self.st.borrow().inside {
                    self.set_collapsed(true, true);
                }
            }
            CollapseMode::Click => {
                let want = self.st.borrow().cfg.collapsed;
                self.set_collapsed(want, true);
            }
        }
        render::invalidate(self.hwnd());
    }

    // ---- instant filter + Everything ---------------------------------------------------

    /// Recomputes the visible items from `all_items` and the filter text.
    fn refilter(st: &mut GridState) {
        let f = st.filter.as_ref().map(|t| t.text().to_lowercase()).unwrap_or_default();
        st.items = if f.is_empty() {
            st.all_items.clone()
        } else {
            st.all_items
                .iter()
                .filter(|i| i.display.to_lowercase().contains(&f) || i.name.to_lowercase().contains(&f))
                .cloned()
                .collect()
        };
        let n = st.items.len();
        st.selected = vec![false; n];
        st.labels = vec![None; n];
        st.hover = None;
        st.anchor = None;
        st.edit = None;
    }

    fn filter_text(&self) -> Option<String> {
        self.st.borrow().filter.as_ref().map(TextInput::text)
    }

    fn on_filter_changed(&self) {
        let hwnd = self.hwnd();
        let text = {
            let mut st = self.st.borrow_mut();
            if st.filter.as_ref().is_some_and(TextInput::is_empty) {
                st.filter = None;
            }
            Self::refilter(&mut st);
            st.scroll = 0.0;
            st.hit_sel = None;
            st.hit_hover = None;
            st.query_id = st.query_id.wrapping_add(1);
            let text = st.filter.as_ref().map(TextInput::text).unwrap_or_default();
            if text.chars().count() < 2 {
                st.hits.clear();
                st.hits_total = 0;
            }
            if st.filter.is_some() && !st.items.is_empty() {
                st.selected[0] = true;
                st.anchor = Some(0);
            }
            text
        };
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_QUERY);
            if text.chars().count() >= 2 && win::everything::ipc_window().is_some() {
                SetTimer(Some(hwnd), TIMER_QUERY, 150, None);
            }
        }
        render::invalidate(hwnd);
    }

    pub fn clear_filter(&self) {
        if self.st.borrow().filter.is_none() {
            return;
        }
        self.st.borrow_mut().filter = None;
        self.on_filter_changed();
    }

    fn filter_insert(&self, s: &str) {
        {
            let mut st = self.st.borrow_mut();
            st.filter.get_or_insert_with(TextInput::default).insert(s);
        }
        self.on_filter_changed();
    }

    fn filter_char(&self, unit: u16) {
        let s = {
            let mut st = self.st.borrow_mut();
            if (0xD800..0xDC00).contains(&unit) {
                st.filter_surrogate = Some(unit);
                return;
            }
            if (0xDC00..0xE000).contains(&unit) {
                match st.filter_surrogate.take() {
                    Some(hi) => String::from_utf16_lossy(&[hi, unit]),
                    None => return,
                }
            } else if unit >= 0x20 && unit != 0x7F {
                // A leading space is not a search.
                if unit == 0x20 && st.filter.is_none() {
                    return;
                }
                String::from_utf16_lossy(&[unit])
            } else {
                return;
            }
        };
        self.filter_insert(&s);
    }

    fn run_query(&self) {
        let hwnd = self.hwnd();
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_QUERY);
        }
        let Some(text) = self.filter_text().filter(|t| t.chars().count() >= 2) else { return };
        let Some(ev) = win::everything::ipc_window() else { return };
        let qid = self.st.borrow().query_id & 0xFFF;
        let tag = win::everything::REPLY_TAG as u32 | (((self.token as u32) & 0xFFF) << 12) | qid;
        // The reply may arrive re-entrantly during this call; no borrows are held.
        if !win::everything::query(ev, app().host(), tag, &text, HITS_MAX) {
            crate::log!("everything: query failed");
        }
    }

    /// Everything reply routed here by the host window.
    pub fn on_hits(&self, qid: u32, total: u32, hits: Vec<win::everything::Hit>) {
        {
            let mut st = self.st.borrow_mut();
            if qid != st.query_id & 0xFFF || st.filter.is_none() {
                return;
            }
            let here = st.dir.trim_end_matches('\\').to_lowercase();
            st.hits = hits.into_iter().filter(|h| h.dir.trim_end_matches('\\').to_lowercase() != here).collect();
            st.hits_total = total;
            st.hit_sel = None;
        }
        self.clamp_scroll();
        render::invalidate(self.hwnd());
    }

    fn hits_height(st: &GridState) -> f32 {
        if st.hits.is_empty() { 0.0 } else { HITS_HEADER_H + st.hits.len() as f32 * HITS_ROW_H }
    }

    fn grid_height(st: &GridState, m: &TileMetrics) -> f32 {
        if st.items.is_empty() { 0.0 } else { Self::grid_layout(st, m).content_height() }
    }

    fn total_height(st: &GridState, m: &TileMetrics) -> f32 {
        Self::grid_height(st, m) + Self::hits_height(st)
    }

    fn hits_top(st: &GridState, m: &TileMetrics) -> f32 {
        Self::content_area(st).t + Self::grid_height(st, m) - st.scroll
    }

    fn hit_row(&self, x: f32, y: f32) -> Option<usize> {
        let m = app().metrics.get();
        let st = self.st.borrow();
        let area = Self::content_area(&st);
        if st.hits.is_empty() || Self::capsule_look(&st) || !area.contains(x, y) {
            return None;
        }
        let top = Self::hits_top(&st, &m) + HITS_HEADER_H;
        if y < top {
            return None;
        }
        let i = ((y - top) / HITS_ROW_H) as usize;
        (i < st.hits.len()).then_some(i)
    }

    fn filter_clear_rect(st: &GridState) -> RectF {
        let (w, _) = Self::dims(st);
        let r = w - theme::TITLE_PAD_R - if st.inside { theme::TITLE_BUTTON + 6.0 } else { 0.0 };
        RectF::new(r - 26.0, 8.0, r - 2.0, Self::title_h() - 10.0)
    }

    fn open_hit(&self, i: usize) {
        let hit = self.st.borrow().hits.get(i).cloned();
        if let Some(h) = hit {
            shell::open_path(&h.full_path(), Some(&h.dir));
        }
    }

    // ---- drawer ---------------------------------------------------------------------

    /// Replaces the drawer's tabs (one per configured grid), keeping the active tab.
    pub fn set_tabs(&self, tabs: Vec<Tab>) {
        let (changed, idx) = {
            let mut st = self.st.borrow_mut();
            let current = st.tabs.get(st.tab).map(|t| t.path.clone());
            let idx = current.and_then(|p| tabs.iter().position(|t| t.path == p)).unwrap_or(0);
            let changed = st.tabs.get(idx).map(|t| &t.path) != tabs.get(idx).map(|t| &t.path) || st.tabs.is_empty();
            st.tabs = tabs;
            (changed, idx)
        };
        self.switch_tab(idx, changed);
    }

    fn switch_tab(&self, i: usize, force: bool) {
        let path = {
            let mut st = self.st.borrow_mut();
            let Some(t) = st.tabs.get(i).cloned() else { return };
            if st.tab == i && !force {
                return;
            }
            st.tab = i;
            st.scroll = 0.0;
            st.edit = None;
            st.cfg.path = t.path.clone();
            st.dir = crate::config::expand_path(&t.path);
            st.items.clear();
            st.selected.clear();
            st.labels.clear();
            t.path
        };
        crate::log!("drawer: tab {i} -> {path}");
        self.reload();
        app().watch_view(self);
    }

    pub fn is_shown(&self) -> bool {
        let h = self.hwnd();
        !h.is_invalid() && unsafe { IsWindowVisible(h).as_bool() } && !self.st.borrow().slide.is_some_and(|s| s.hide)
    }

    /// Slides the drawer in to `target` (screen px) from `edge`.
    pub fn reveal(&self, target: RECT, edge: crate::config::Edge, by_key: bool) {
        let hwnd = self.hwnd();
        if hwnd.is_invalid() {
            return;
        }
        let (w, h) = (target.right - target.left, target.bottom - target.top);
        let from = match edge {
            crate::config::Edge::Left => (target.left - w / 3, target.top),
            crate::config::Edge::Top => (target.left, target.top - h / 4),
            _ => (target.left + w / 3, target.top),
        };
        {
            let mut st = self.st.borrow_mut();
            st.revealed_by_key = by_key;
            st.outside_ticks = 0;
            st.slide = Some(Slide {
                from,
                to: (target.left, target.top),
                start: crate::util::tick_ms(),
                dur: 160,
                hide: false,
            });
        }
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), from.0, from.1, w, h, SWP_SHOWWINDOW | SWP_NOACTIVATE);
            SetTimer(Some(hwnd), TIMER_SLIDE, 10, None);
            if by_key {
                let _ = SetForegroundWindow(hwnd);
            } else {
                SetTimer(Some(hwnd), TIMER_HOVER_HIDE, 250, None);
            }
        }
        render::invalidate(hwnd);
    }

    pub fn conceal(&self) {
        let hwnd = self.hwnd();
        if hwnd.is_invalid() || !unsafe { IsWindowVisible(hwnd).as_bool() } {
            return;
        }
        let r = win::window_rect(hwnd);
        {
            let mut st = self.st.borrow_mut();
            st.edit = None;
            if st.filter.take().is_some() {
                st.hits.clear();
                Self::refilter(&mut st);
            }
            let w = r.right - r.left;
            st.slide = Some(Slide {
                from: (r.left, r.top),
                to: (r.left + w / 4, r.top),
                start: crate::util::tick_ms(),
                dur: 120,
                hide: true,
            });
        }
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_HOVER_HIDE);
            SetTimer(Some(hwnd), TIMER_SLIDE, 10, None);
        }
    }

    fn on_slide_tick(&self) {
        let hwnd = self.hwnd();
        let Some(sl) = self.st.borrow().slide else {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_SLIDE);
            }
            return;
        };
        let t = ((crate::util::tick_ms() - sl.start) as f32 / sl.dur.max(1) as f32).min(1.0);
        let k = ease_out_cubic(t);
        let x = sl.from.0 + ((sl.to.0 - sl.from.0) as f32 * k).round() as i32;
        let y = sl.from.1 + ((sl.to.1 - sl.from.1) as f32 * k).round() as i32;
        unsafe {
            let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        if t >= 1.0 {
            self.st.borrow_mut().slide = None;
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_SLIDE);
                if sl.hide {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    app().schedule_idle_trim();
                }
            }
        }
    }

    /// Edge-revealed drawer: hide once the pointer has left it (unless focused).
    fn on_hover_hide_tick(&self) {
        let hwnd = self.hwnd();
        let (active, busy) = {
            let st = self.st.borrow();
            (st.active, st.edit.is_some() || !matches!(st.drag, Drag::None) || st.drop_hl.is_some())
        };
        if active || busy {
            self.st.borrow_mut().outside_ticks = 0;
            return;
        }
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        let r = win::window_rect(hwnd);
        let pad = (24.0 * win::scale(self.st.borrow().dpi)) as i32;
        let near = p.x >= r.left - pad && p.x < r.right + pad && p.y >= r.top - pad && p.y < r.bottom + pad;
        let ticks = {
            let mut st = self.st.borrow_mut();
            st.outside_ticks = if near { 0 } else { st.outside_ticks + 1 };
            st.outside_ticks
        };
        if ticks >= 2 {
            app().hide_drawer();
        }
    }

    fn rebed(&self) {
        if self.kind == Kind::Desktop {
            desktop::rebed(self.hwnd());
        }
    }

    fn hit_tab(&self, x: f32, y: f32) -> Option<usize> {
        let st = self.st.borrow();
        if self.kind != Kind::Drawer || y >= Self::title_h() {
            return None;
        }
        st.tab_rects.iter().position(|r| r.contains(x, y))
    }

    // ---- geometry (DIP) ---------------------------------------------------------

    fn scale(st: &GridState) -> f32 {
        win::scale(st.dpi)
    }

    fn dims(st: &GridState) -> (f32, f32) {
        let s = Self::scale(st);
        (st.size.0 as f32 / s, st.size.1 as f32 / s)
    }

    fn title_h() -> f32 {
        theme::TITLE_MIN_H
    }

    /// Draw the capsule look (title only) when folded or nearly folded.
    fn capsule_look(st: &GridState) -> bool {
        matches!(st.fold, Fold::Collapsed) || Self::dims(st).1 < Self::title_h() + 10.0
    }

    fn content_area(st: &GridState) -> RectF {
        let (w, h) = Self::dims(st);
        RectF::new(
            theme::CONTENT_PAD_H,
            Self::title_h() + theme::CONTENT_PAD_TOP,
            w - theme::CONTENT_PAD_H,
            h - theme::CONTENT_PAD_BOTTOM,
        )
    }

    fn grid_layout(st: &GridState, m: &TileMetrics) -> GridLayout {
        GridLayout::new(m, Self::content_area(st), st.items.len())
    }

    /// Scroll range uses the expanded viewport, so folding never loses position.
    fn max_scroll(st: &GridState, m: &TileMetrics) -> f32 {
        let area = Self::content_area(st);
        if matches!(st.fold, Fold::Expanded) { (Self::total_height(st, m) - area.h()).max(0.0) } else { st.scroll }
    }

    fn clamp_scroll(&self) {
        let m = app().metrics.get();
        if let Ok(mut st) = self.st.try_borrow_mut() {
            let max = Self::max_scroll(&st, &m);
            st.scroll = st.scroll.clamp(0.0, max.max(0.0));
        }
    }

    fn row_h(st: &GridState) -> f32 {
        if Self::capsule_look(st) { Self::dims(st).1 } else { Self::title_h() }
    }

    fn button_rect(st: &GridState, b: Button) -> RectF {
        let (w, _) = Self::dims(st);
        let size = theme::TITLE_BUTTON;
        let top = (Self::row_h(st) - size) / 2.0 - if Self::capsule_look(st) { 0.0 } else { 1.0 };
        let r = w - theme::TITLE_PAD_R + 4.0;
        let slot = match b {
            Button::More => 0.0,
            Button::Fold => size + 2.0,
        };
        RectF::new(r - size - slot, top, r - slot, top + size)
    }

    fn visible_buttons(st: &GridState) -> Vec<Button> {
        if !st.inside {
            return Vec::new();
        }
        let mut v = vec![Button::More];
        if st.cfg.collapse != CollapseMode::Expanded {
            v.push(Button::Fold);
        }
        v
    }

    fn to_dip(st: &GridState, x: i32, y: i32) -> (f32, f32) {
        let s = Self::scale(st);
        (x as f32 / s, y as f32 / s)
    }

    fn selected_names(st: &GridState) -> Vec<String> {
        st.items.iter().zip(&st.selected).filter(|(_, s)| **s).map(|(i, _)| i.name.clone()).collect()
    }

    // ---- painting ------------------------------------------------------------------

    fn paint(&self, hdc: windows::Win32::Graphics::Gdi::HDC) {
        let app = app();
        let theme = app.theme.borrow().clone();
        let ap = app.cfg().appearance.clone();
        let m = app.metrics.get();
        let Ok(mut st) = self.st.try_borrow_mut() else { return };
        let (pw, ph) = st.size;
        let dpi = st.dpi;
        let title = Self::title_of(&st);
        let st = &mut *st;
        let t0 = std::time::Instant::now();
        app.renderer.paint(hdc, pw, ph, dpi, |cv| {
            self.draw(cv, st, &theme, &ap, &m, &title);
        });
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        if ms > 12.0 {
            crate::log!("paint {} took {ms:.1} ms", self.id);
        }
        app.schedule_idle_trim();
    }

    fn draw(
        &self,
        cv: &Canvas,
        st: &mut GridState,
        th: &theme::Theme,
        ap: &crate::config::Appearance,
        m: &TileMetrics,
        title: &str,
    ) {
        let (w, h) = Self::dims(st);
        let s = Self::scale(st);
        let px = 1.0 / s;
        let (outer, inner) = theme::corner_radii(ap.corner);
        let full = RectF::new(0.0, 0.0, w, h);
        cv.fill_round(full, outer, app().tint_color(th));

        // ---- title row / capsule ----
        let capsule = Self::capsule_look(st);
        let row_h = Self::row_h(st);
        let icon_r = RectF::new(
            theme::TITLE_PAD_L,
            (row_h - 20.0) / 2.0,
            theme::TITLE_PAD_L + theme::TITLE_ICON,
            (row_h + 20.0) / 2.0,
        );
        if capsule || st.filter.is_none() {
            cv.glyph(GLYPH_FOLDER, icon_r, theme::TITLE_ICON, th.accent);
        }
        let text_l = icon_r.r + theme::TITLE_ICON_GAP;
        let buttons = Self::visible_buttons(st);
        let mut right = w - theme::TITLE_PAD_R;
        if let Some(last) = buttons.last() {
            right = Self::button_rect(st, *last).l - 4.0;
        }
        if capsule && st.error.is_none() {
            // Key info, like the upstream "关键信息" capsule content.
            let count = format!("{} 项", st.items.len());
            let cw = 46.0;
            cv.text(&count, RectF::new(right - cw, 0.0, right, row_h), 11.5, false, Align::Leading, th.text_tertiary);
            right -= cw + 2.0;
        }
        let text_r = right.max(text_l + 10.0);
        if !capsule && st.filter.is_some() {
            self.draw_filter_pill(cv, st, th, icon_r, inner, s);
        } else if self.kind == Kind::Drawer && !st.tabs.is_empty() {
            self.draw_tabs(cv, st, th, text_l, text_r, inner);
        } else {
            cv.text(
                title,
                RectF::new(text_l, 0.0, text_r, row_h - if capsule { 0.0 } else { 2.0 }),
                theme::TITLE_TEXT,
                false,
                Align::Leading,
                th.text,
            );
        }
        for b in &buttons {
            let br = Self::button_rect(st, *b);
            if st.hover_button == Some(*b) {
                cv.fill_round(br, inner, th.hover);
            }
            let glyph = match b {
                Button::More => GLYPH_MORE,
                Button::Fold if capsule => GLYPH_DOWN,
                Button::Fold => GLYPH_UP,
            };
            cv.glyph(glyph, br, if *b == Button::More { 14.0 } else { 11.0 }, th.text_secondary);
        }
        if let Some(None) = st.drop_hl {
            cv.stroke_round(full.inset(1.0, 1.0), (outer - 1.0).max(0.0), th.accent, 2.0);
        }
        if capsule {
            return;
        }
        let dy = Self::title_h() - theme::DIVIDER_MARGIN_B - px / 2.0;
        cv.line(theme::DIVIDER_MARGIN_H, dy, w - theme::DIVIDER_MARGIN_H, dy, th.divider, px);

        // ---- content ----
        let area = Self::content_area(st);
        if let Some(err) = &st.error {
            let msg = format!("{err}\n{}", st.dir);
            self.message(cv, area, &msg, th);
        } else if st.items.is_empty() && st.hits.is_empty() {
            let msg = match &st.filter {
                Some(f) => format!("没有与“{}”匹配的项目", f.text()),
                None => "空文件夹 — 把文件拖到这里".to_owned(),
            };
            self.message(cv, area, &msg, th);
        } else {
            if !st.items.is_empty() {
                self.draw_tiles(cv, st, th, ap, m, area, inner, s);
            }
            self.draw_hits(cv, st, th, m, area, inner, s);
            self.draw_scrollbar(cv, st, th, m, area);
        }
        if let Drag::Marquee { x0, y0, x1, y1 } = st.drag {
            let r = RectF::new(x0.min(x1), y0.min(y1) - st.scroll, x0.max(x1), y0.max(y1) - st.scroll);
            cv.clip(area);
            cv.fill_rect(r, th.accent.with_alpha(0.16));
            cv.stroke_round(r, 0.0, th.accent.with_alpha(0.7), px);
            cv.unclip();
        }
        self.draw_edit(cv, st, th, m, area, s);
    }

    fn draw_tabs(&self, cv: &Canvas, st: &mut GridState, th: &theme::Theme, left: f32, right: f32, inner: f32) {
        let r = cv.renderer();
        let Some(fmt) = r.text_format(theme::TITLE_TEXT, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading, Wrap::Line, false)
        else {
            return;
        };
        let row = Self::title_h();
        let mut x = left - 6.0;
        let mut rects = Vec::with_capacity(st.tabs.len());
        for (i, t) in st.tabs.iter().enumerate() {
            let Some(l) = r.layout(&t.title, &fmt, 160.0, row) else { continue };
            let w = render::text_width(&l).min(160.0) + 14.0;
            let tr = RectF::new(x, 7.0, (x + w).min(right), row - 9.0);
            if tr.w() < 12.0 {
                break;
            }
            if st.hover_tab == Some(i) && i != st.tab {
                cv.fill_round(tr, inner, th.hover);
            }
            let color = if i == st.tab { th.text } else { th.text_secondary };
            cv.text(
                &t.title,
                RectF::new(tr.l + 7.0, 0.0, tr.r - 4.0, row - 2.0),
                theme::TITLE_TEXT,
                i == st.tab,
                Align::Leading,
                color,
            );
            if i == st.tab {
                cv.fill_round(
                    RectF::new(tr.l + tr.w() / 2.0 - 8.0, row - 8.0, tr.l + tr.w() / 2.0 + 8.0, row - 5.0),
                    1.5,
                    th.accent,
                );
            }
            rects.push(tr);
            x += w + 2.0;
        }
        st.tab_rects = rects;
    }

    fn message(&self, cv: &Canvas, area: RectF, msg: &str, th: &theme::Theme) {
        let r = cv.renderer();
        if let Some(fmt) = r.text_format(12.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center, Wrap::Multi, false)
            && let Some(l) = r.layout(msg, &fmt, area.w() - 16.0, area.h())
        {
            let y = area.t + (area.h() / 2.0 - 24.0).max(0.0);
            cv.text_layout(&l, area.l + 8.0, y, th.text_tertiary);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_tiles(
        &self,
        cv: &Canvas,
        st: &mut GridState,
        th: &theme::Theme,
        ap: &crate::config::Appearance,
        m: &TileMetrics,
        area: RectF,
        inner: f32,
        s: f32,
    ) {
        let gl = Self::grid_layout(st, m);
        let renderer = cv.renderer();
        let label_fmt = renderer.text_format(m.text, DWRITE_FONT_WEIGHT_NORMAL, Align::Center, Wrap::Multi, false);
        let icon_px = (m.icon * s).round().max(16.0) as u32;
        let snap = |v: f32| (v * s).round() / s;
        let shadow = ap.text_shadow.then_some(th.shadow);
        let editing = st.edit.as_ref().map(|e| e.index);
        cv.clip(area);
        let (bmp_rt, generation) = cv.bitmap_target();
        let mut icons = app().icons.borrow_mut();
        for i in gl.visible(area.h(), st.scroll) {
            let r = gl.tile_rect(m, i, st.scroll);
            if r.b < area.t || r.t > area.b {
                continue;
            }
            let sel = st.selected.get(i).copied().unwrap_or(false);
            let hover = st.hover == Some(i) || st.drop_hl == Some(Some(i));
            let fill = match (sel, hover, st.active) {
                (true, true, true) => th.selected_hover,
                (true, false, true) => th.selected,
                (true, true, false) => th.selected_unfocused_hover,
                (true, false, false) => th.selected_unfocused,
                (false, true, _) => th.hover,
                _ => Rgba::argb(0),
            };
            cv.fill_round(r, inner, fill);
            if sel && st.active {
                cv.stroke_round(r.inset(0.5 / s, 0.5 / s), inner, th.selected_border, 1.0 / s);
            }
            let item = &st.items[i];
            let ix = snap(r.l + (r.w() - m.icon) / 2.0);
            let iy = snap(r.t + m.pad_y);
            let icon_rect = RectF::new(ix, iy, ix + icon_px as f32 / s, iy + icon_px as f32 / s);
            let key = item.icon_key(&st.dir);
            let sample = shell::join(&st.dir, &item.name);
            match icons.get(&key, icon_px, &sample, &bmp_rt, generation) {
                Some(bmp) => cv.bitmap(&bmp, icon_rect, if item.attrs & 0x2 != 0 { 0.55 } else { 1.0 }),
                None => cv.fill_round(icon_rect.inset(2.0, 2.0), 6.0, th.placeholder),
            }
            if m.lines > 0 && editing != Some(i) {
                if st.labels.get(i).is_some_and(Option::is_none)
                    && let Some(fmt) = &label_fmt
                {
                    st.labels[i] = renderer.layout(&item.display, fmt, m.label_w, m.line_h * m.lines as f32);
                }
                if let Some(Some(l)) = st.labels.get(i) {
                    let lx = r.l + (r.w() - m.label_w) / 2.0;
                    let ly = iy + m.icon + m.spacing;
                    cv.text_layout_shadowed(l, lx, ly, th.text, shadow);
                }
            }
        }
        drop(icons);
        cv.unclip();
    }

    fn draw_scrollbar(&self, cv: &Canvas, st: &GridState, th: &theme::Theme, m: &TileMetrics, area: RectF) {
        let content_h = Self::total_height(st, m);
        if content_h > area.h() + 0.5 {
            let (w, _) = Self::dims(st);
            let track = RectF::new(w - 6.0, area.t + 2.0, w - 6.0 + theme::SCROLLBAR_W, area.b - 2.0);
            let thumb_h = (track.h() * area.h() / content_h).max(24.0).min(track.h());
            let max = (content_h - area.h()).max(1.0);
            let t = track.t + (track.h() - thumb_h) * (st.scroll / max).clamp(0.0, 1.0);
            let alpha = if st.inside { 1.0 } else { 0.55 };
            cv.fill_round(
                RectF::new(track.l, t, track.r, t + thumb_h),
                theme::SCROLLBAR_W / 2.0,
                th.scrollbar.scale_alpha(alpha),
            );
        }
    }

    /// Search pill replacing the title while filtering.
    fn draw_filter_pill(&self, cv: &Canvas, st: &GridState, th: &theme::Theme, icon_r: RectF, inner: f32, s: f32) {
        let clear = Self::filter_clear_rect(st);
        let pill = RectF::new(icon_r.l - 6.0, 7.0, clear.r + 2.0, Self::title_h() - 9.0);
        cv.fill_round(pill, inner + 2.0, th.selected);
        cv.stroke_round(pill.inset(0.5 / s, 0.5 / s), inner + 2.0, th.accent.with_alpha(0.8), 1.0 / s);
        cv.glyph(GLYPH_SEARCH, icon_r, 13.0, th.accent);
        let text = st.filter.as_ref().map(TextInput::text).unwrap_or_default();
        let tl = icon_r.r + 6.0;
        let tr = clear.l - 2.0;
        let r = cv.renderer();
        let mut caret_x = tl;
        if let Some(fmt) =
            r.text_format(theme::TITLE_TEXT, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading, Wrap::Line, false)
            && let Some(l) = r.layout(&text, &fmt, (tr - tl).max(10.0), Self::title_h() - 2.0)
        {
            cv.text_layout(&l, tl, 0.0, th.text);
            caret_x = (tl + render::text_width(&l)).min(tr);
        }
        if st.active {
            cv.line(caret_x + 1.0, 12.0, caret_x + 1.0, Self::title_h() - 14.0, th.text, 1.0 / s);
        }
        cv.glyph(GLYPH_CLEAR, clear, 10.0, th.text_secondary);
        let hwnd = self.hwnd();
        unsafe {
            let himc = ImmGetContext(hwnd);
            if !himc.is_invalid() {
                let cf = COMPOSITIONFORM {
                    dwStyle: CFS_POINT,
                    ptCurrentPos: POINT { x: (caret_x * s) as i32, y: (8.0 * s) as i32 },
                    ..Default::default()
                };
                let _ = ImmSetCompositionWindow(himc, &cf);
                let _ = ImmReleaseContext(hwnd, himc);
            }
        }
    }

    /// "Everything" global results below the local tiles.
    #[allow(clippy::too_many_arguments)]
    fn draw_hits(
        &self,
        cv: &Canvas,
        st: &mut GridState,
        th: &theme::Theme,
        m: &TileMetrics,
        area: RectF,
        inner: f32,
        s: f32,
    ) {
        if st.hits.is_empty() {
            return;
        }
        let top = Self::hits_top(st, m);
        cv.clip(area);
        let header = RectF::new(area.l + 4.0, top + 4.0, area.r - 4.0, top + HITS_HEADER_H);
        cv.line(header.l, top + 2.0, header.r, top + 2.0, th.divider, 1.0 / s);
        let title = format!("Everything · 全盘结果  {}", st.hits_total);
        cv.text(&title, header, 11.0, true, Align::Leading, th.text_tertiary);
        let icon_px = (18.0 * s).round() as u32;
        let (bmp_rt, generation) = cv.bitmap_target();
        let mut icons = app().icons.borrow_mut();
        for (i, h) in st.hits.iter().enumerate() {
            let y = top + HITS_HEADER_H + i as f32 * HITS_ROW_H;
            if y + HITS_ROW_H < area.t || y > area.b {
                continue;
            }
            let row = RectF::new(area.l, y + 1.0, area.r - 4.0, y + HITS_ROW_H - 1.0);
            let fill = match (st.hit_sel == Some(i), st.hit_hover == Some(i)) {
                (true, true) => th.selected_hover,
                (true, false) => th.selected,
                (false, true) => th.hover,
                _ => Rgba::argb(0),
            };
            cv.fill_round(row, inner, fill);
            let ix = ((row.l + 6.0) * s).round() / s;
            let iy = ((row.t + (row.h() - 18.0) / 2.0) * s).round() / s;
            let icon_rect = RectF::new(ix, iy, ix + icon_px as f32 / s, iy + icon_px as f32 / s);
            let key = shell::items::key_for(&h.dir, &h.name, h.is_dir);
            match icons.get(&key, icon_px, &h.full_path(), &bmp_rt, generation) {
                Some(b) => cv.bitmap(&b, icon_rect, 1.0),
                None => cv.fill_round(icon_rect.inset(1.0, 1.0), 4.0, th.placeholder),
            }
            let name_r = RectF::new(row.l + 30.0, row.t, row.l + 30.0 + (row.w() - 34.0) * 0.55, row.b);
            cv.text(&h.name, name_r, 12.0, false, Align::Leading, th.text);
            let dir_r = RectF::new(name_r.r + 6.0, row.t, row.r - 4.0, row.b);
            cv.text(&h.dir, dir_r, 10.5, false, Align::Leading, th.text_tertiary);
        }
        drop(icons);
        cv.unclip();
    }

    /// Inline rename box over the edited tile's label.
    fn draw_edit(&self, cv: &Canvas, st: &mut GridState, th: &theme::Theme, m: &TileMetrics, area: RectF, s: f32) {
        let Some(index) = st.edit.as_ref().map(|e| e.index) else { return };
        let gl = Self::grid_layout(st, m);
        let r = gl.tile_rect(m, index, st.scroll);
        let box_w = (m.label_w * 1.7).max(m.tile_w + 12.0).min(area.w());
        let cx = r.l + r.w() / 2.0;
        let bl = (cx - box_w / 2.0).clamp(area.l, area.r - box_w);
        let bt = r.t + m.pad_y + m.icon + m.spacing - 3.0;
        let renderer = cv.renderer();
        let Some(fmt) = renderer.text_format(m.text, DWRITE_FONT_WEIGHT_NORMAL, Align::Center, Wrap::Multi, false)
        else {
            return;
        };
        let Some(edit) = st.edit.as_mut() else { return };
        let text = edit.input.text();
        let Some(layout) =
            renderer.layout(if text.is_empty() { " " } else { &text }, &fmt, box_w - 8.0, m.line_h * 6.0)
        else {
            return;
        };
        let mut tm = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut tm);
        }
        let bh = tm.height.max(m.line_h) + 6.0;
        let boxr = RectF::new(bl, bt, bl + box_w, bt + bh);
        let (bg, fg) = if th.dark {
            (Rgba::argb(0xFF2B2B2B), Rgba::argb(0xFFFFFFFF))
        } else {
            (Rgba::argb(0xFFFFFFFF), Rgba::argb(0xFF000000))
        };
        cv.fill_round(boxr, 3.0, bg);
        cv.stroke_round(boxr.inset(0.5 / s, 0.5 / s), 3.0, th.accent, 1.0 / s);
        let (tx, ty) = (bl + 4.0, bt + 3.0);
        // Selection.
        let (a, b) = edit.input.selection();
        if a != b {
            let start = edit.input.utf16_at(a);
            let len = edit.input.utf16_at(b) - start;
            let mut hits = [DWRITE_HIT_TEST_METRICS::default(); 8];
            let mut n = 0u32;
            unsafe {
                let _ = layout.HitTestTextRange(start, len, tx, ty, Some(&mut hits), &mut n);
            }
            for hm in hits.iter().take(n as usize) {
                cv.fill_rect(
                    RectF::new(hm.left, hm.top, hm.left + hm.width, hm.top + hm.height),
                    th.accent.with_alpha(0.45),
                );
            }
        }
        cv.text_layout(&layout, tx, ty, fg);
        // Caret.
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let mut hm = DWRITE_HIT_TEST_METRICS::default();
        let pos = edit.input.utf16_at(edit.input.caret);
        unsafe {
            let _ = layout.HitTestTextPosition(pos, false, &mut x, &mut y, &mut hm);
        }
        let caret_x = (tx + x).round_ties_even();
        cv.line(caret_x, ty + y, caret_x, ty + y + hm.height.max(m.line_h - 2.0), fg, 1.0 / s);
        edit.layout = Some((layout, tx, ty));
        // Keep the IME candidate window next to the caret.
        let hwnd = self.hwnd();
        unsafe {
            let himc = ImmGetContext(hwnd);
            if !himc.is_invalid() {
                let cf = COMPOSITIONFORM {
                    dwStyle: CFS_POINT,
                    ptCurrentPos: POINT { x: (caret_x * s) as i32, y: ((ty + y) * s) as i32 },
                    ..Default::default()
                };
                let _ = ImmSetCompositionWindow(himc, &cf);
                let _ = ImmReleaseContext(hwnd, himc);
            }
        }
    }

    // ---- input ----------------------------------------------------------------------

    fn hit_tile(&self, x: f32, y: f32) -> Option<usize> {
        let m = app().metrics.get();
        let st = self.st.borrow();
        if st.error.is_some() || Self::capsule_look(&st) || !Self::content_area(&st).contains(x, y) {
            return None;
        }
        Self::grid_layout(&st, &m).hit(&m, x, y, st.scroll)
    }

    fn hit_button(&self, x: f32, y: f32) -> Option<Button> {
        let st = self.st.borrow();
        Self::visible_buttons(&st).into_iter().find(|b| Self::button_rect(&st, *b).contains(x, y))
    }

    fn in_title(&self, y: f32) -> bool {
        let st = self.st.borrow();
        Self::capsule_look(&st) || y < Self::title_h()
    }

    fn edit_box_contains(&self, x: f32, y: f32) -> Option<u32> {
        let st = self.st.borrow();
        let (layout, tx, ty) = st.edit.as_ref()?.layout.clone()?;
        let mut trailing = BOOL(0);
        let mut inside = BOOL(0);
        let mut hm = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            layout.HitTestPoint(x - tx, y - ty, &mut trailing, &mut inside, &mut hm).ok()?;
        }
        let mut tm = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut tm);
        }
        let within = x >= tx - 4.0 && x <= tx + tm.layoutWidth + 4.0 && y >= ty - 3.0 && y <= ty + tm.height + 3.0;
        within.then_some(hm.textPosition + if trailing.as_bool() { hm.length } else { 0 })
    }

    fn on_lbutton_down(&self, x: i32, y: i32) {
        let hwnd = self.hwnd();
        let (dx, dy) = Self::to_dip(&self.st.borrow(), x, y);
        if self.st.borrow().edit.is_some() {
            if let Some(pos) = self.edit_box_contains(dx, dy) {
                let mut st = self.st.borrow_mut();
                if let Some(e) = st.edit.as_mut() {
                    let c = e.input.char_at_utf16(pos);
                    e.input.move_to(c, key_down(VK_SHIFT));
                }
                drop(st);
                render::invalidate(hwnd);
                return;
            }
            self.commit_rename();
        }
        if let Some(t) = self.hit_tab(dx, dy) {
            self.switch_tab(t, false);
            return;
        }
        let clear_hit = self.st.borrow().filter.is_some()
            && !self.is_collapsed()
            && Self::filter_clear_rect(&self.st.borrow()).contains(dx, dy);
        if clear_hit {
            self.clear_filter();
            return;
        }
        if let Some(i) = (!self.in_title(dy)).then(|| self.hit_row(dx, dy)).flatten() {
            {
                let mut st = self.st.borrow_mut();
                st.selected.iter_mut().for_each(|s| *s = false);
                st.hit_sel = Some(i);
                st.drag = Drag::PendingHit { x, y, i };
            }
            unsafe { SetCapture(hwnd) };
            render::invalidate(hwnd);
            return;
        }
        self.st.borrow_mut().hit_sel = None;
        let ctrl = key_down(VK_CONTROL);
        let shift = key_down(VK_SHIFT);
        let in_title = self.in_title(dy);
        let button = if in_title { self.hit_button(dx, dy) } else { None };
        let tile = if in_title { None } else { self.hit_tile(dx, dy) };
        {
            let mut st = self.st.borrow_mut();
            if in_title {
                st.drag = Drag::Pending { x, y, index: None, title: true, button };
            } else if let Some(i) = tile {
                if shift {
                    let a = st.anchor.unwrap_or(i);
                    if !ctrl {
                        st.selected.iter_mut().for_each(|s| *s = false);
                    }
                    st.selected[a.min(i)..=a.max(i)].iter_mut().for_each(|s| *s = true);
                } else if ctrl {
                    st.selected[i] = !st.selected[i];
                    st.anchor = Some(i);
                } else {
                    if !st.selected[i] {
                        st.selected.iter_mut().for_each(|s| *s = false);
                        st.selected[i] = true;
                    }
                    st.anchor = Some(i);
                }
                st.drag = Drag::Pending { x, y, index: Some(i), title: false, button: None };
            } else {
                if !ctrl {
                    st.selected.iter_mut().for_each(|s| *s = false);
                }
                let sy = dy + st.scroll;
                st.drag = Drag::Marquee { x0: dx, y0: sy, x1: dx, y1: sy };
            }
        }
        unsafe { SetCapture(hwnd) };
        render::invalidate(hwnd);
    }

    fn track_mouse(&self, hover: bool) {
        let ms = app().cfg().capsule.expand_delay_ms.max(1);
        let mut tme = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: if hover { TME_LEAVE | TME_HOVER } else { TME_LEAVE },
            hwndTrack: self.hwnd(),
            dwHoverTime: ms,
        };
        unsafe {
            let _ = TrackMouseEvent(&mut tme);
        }
    }

    fn on_mouse_move(&self, x: i32, y: i32) {
        let hwnd = self.hwnd();
        let first_enter = {
            let mut st = self.st.borrow_mut();
            let was = st.inside;
            st.inside = true;
            !was
        };
        if first_enter {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_COLLAPSE);
            }
            let want_hover = self.smart() && self.is_collapsed();
            self.track_mouse(want_hover);
            render::invalidate(hwnd);
        }
        let drag = self.st.borrow().drag;
        match drag {
            Drag::PendingHit { x: x0, y: y0, i } => {
                let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
                if (x - x0).abs() <= cx && (y - y0).abs() <= cy {
                    return;
                }
                self.st.borrow_mut().drag = Drag::None;
                unsafe {
                    let _ = ReleaseCapture();
                }
                let hit = self.st.borrow().hits.get(i).cloned();
                if let Some(h) = hit {
                    shell::dragdrop::start_drag(app().host(), &h.dir, std::slice::from_ref(&h.name));
                }
            }
            Drag::Pending { x: x0, y: y0, index, title, .. } => {
                let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
                if (x - x0).abs() <= cx && (y - y0).abs() <= cy {
                    return;
                }
                if title {
                    if self.st.borrow().cfg.locked || self.kind == Kind::Drawer {
                        return;
                    }
                    let mut cursor = POINT::default();
                    unsafe {
                        let _ = GetCursorPos(&mut cursor);
                    }
                    self.st.borrow_mut().drag = Drag::Move { cursor, rect: win::window_rect(hwnd) };
                } else if index.is_some() {
                    self.st.borrow_mut().drag = Drag::None;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    self.begin_ole_drag();
                }
            }
            Drag::Move { cursor, rect } => {
                let mut now = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut now);
                }
                let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
                let moved = RECT {
                    left: rect.left + now.x - cursor.x,
                    top: rect.top + now.y - cursor.y,
                    right: rect.left + now.x - cursor.x + w,
                    bottom: rect.top + now.y - cursor.y + h,
                };
                let snapped = app().snap_rect(&self.id, moved);
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        snapped.left,
                        snapped.top,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            Drag::Marquee { x0, y0, .. } => {
                let m = app().metrics.get();
                {
                    let mut st = self.st.borrow_mut();
                    let (dx, dy) = Self::to_dip(&st, x, y);
                    let sy = dy + st.scroll;
                    st.drag = Drag::Marquee { x0, y0, x1: dx, y1: sy };
                    let sel = RectF::new(x0.min(dx), y0.min(sy), x0.max(dx), y0.max(sy));
                    let gl = Self::grid_layout(&st, &m);
                    let ctrl = key_down(VK_CONTROL);
                    for i in 0..st.items.len() {
                        let hit = gl.tile_rect(&m, i, 0.0).intersects(&sel);
                        if hit {
                            st.selected[i] = true;
                        } else if !ctrl {
                            st.selected[i] = false;
                        }
                    }
                }
                render::invalidate(hwnd);
            }
            Drag::None => {
                let (dx, dy) = Self::to_dip(&self.st.borrow(), x, y);
                let tile = self.hit_tile(dx, dy);
                let button = self.hit_button(dx, dy);
                let tab = self.hit_tab(dx, dy);
                let row = self.hit_row(dx, dy);
                let changed = {
                    let mut st = self.st.borrow_mut();
                    let c = st.hover != tile || st.hover_button != button || st.hover_tab != tab || st.hit_hover != row;
                    st.hover = tile;
                    st.hover_button = button;
                    st.hover_tab = tab;
                    st.hit_hover = row;
                    c
                };
                if changed {
                    render::invalidate(hwnd);
                }
            }
        }
    }

    fn on_lbutton_up(&self, x: i32, y: i32) {
        let hwnd = self.hwnd();
        let drag = std::mem::replace(&mut self.st.borrow_mut().drag, Drag::None);
        unsafe {
            let _ = ReleaseCapture();
        }
        let (dx, dy) = Self::to_dip(&self.st.borrow(), x, y);
        match drag {
            Drag::Pending { index: Some(i), .. } => {
                if !key_down(VK_CONTROL) && !key_down(VK_SHIFT) {
                    let mut st = self.st.borrow_mut();
                    st.selected.iter_mut().for_each(|s| *s = false);
                    if let Some(s) = st.selected.get_mut(i) {
                        *s = true;
                    }
                }
            }
            Drag::Pending { title: true, button: Some(b), .. } if self.hit_button(dx, dy) == Some(b) => match b {
                Button::More => {
                    let (br, s) = {
                        let st = self.st.borrow();
                        (Self::button_rect(&st, Button::More), Self::scale(&st))
                    };
                    let mut p = POINT { x: (br.l * s) as i32, y: (br.b * s) as i32 };
                    unsafe {
                        let _ = ClientToScreen(hwnd, &mut p);
                    }
                    self.show_menu(p, None, true);
                }
                Button::Fold => {
                    let collapse = !self.is_collapsed();
                    self.set_collapsed(collapse, true);
                }
            },
            Drag::Pending { title: true, .. } => {
                // A click on a folded capsule opens it (any mode).
                if self.is_collapsed() {
                    self.set_collapsed(false, true);
                }
            }
            Drag::Move { .. } => {
                self.persist_rect();
                self.rebed();
            }
            _ => {}
        }
        render::invalidate(hwnd);
    }

    fn begin_ole_drag(&self) {
        let (dir, names) = {
            let st = self.st.borrow();
            (st.dir.clone(), Self::selected_names(&st))
        };
        if names.is_empty() {
            return;
        }
        let effect = shell::dragdrop::start_drag(app().host(), &dir, &names);
        crate::log!("drag out of {} finished, effect {:?}", self.id, effect.0);
    }

    fn on_double_click(&self, x: i32, y: i32) {
        let (dx, dy) = Self::to_dip(&self.st.borrow(), x, y);
        if self.in_title(dy) {
            if self.hit_button(dx, dy).is_none() && self.st.borrow().cfg.collapse != CollapseMode::Expanded {
                let collapse = !self.is_collapsed();
                self.set_collapsed(collapse, true);
            }
            return;
        }
        if self.st.borrow().edit.is_some() {
            if let Some(e) = self.st.borrow_mut().edit.as_mut() {
                e.input.select_all();
            }
            render::invalidate(self.hwnd());
            return;
        }
        if let Some(i) = self.hit_tile(dx, dy) {
            let (dir, name) = {
                let st = self.st.borrow();
                (st.dir.clone(), st.items[i].name.clone())
            };
            shell::open_path(&shell::join(&dir, &name), Some(&dir));
        } else if let Some(i) = self.hit_row(dx, dy) {
            self.open_hit(i);
        }
    }

    fn on_rbutton_up(&self, x: i32, y: i32) {
        let hwnd = self.hwnd();
        if self.st.borrow().edit.is_some() {
            self.commit_rename();
        }
        let (dx, dy) = Self::to_dip(&self.st.borrow(), x, y);
        let mut p = POINT { x, y };
        unsafe {
            let _ = ClientToScreen(hwnd, &mut p);
        }
        if self.in_title(dy) {
            self.show_menu(p, None, true);
            return;
        }
        if let Some(i) = self.hit_row(dx, dy) {
            let hit = {
                let mut st = self.st.borrow_mut();
                st.selected.iter_mut().for_each(|s| *s = false);
                st.hit_sel = Some(i);
                st.hits.get(i).cloned()
            };
            render::invalidate(hwnd);
            if let Some(h) = hit {
                menu::track(app().host(), &h.dir, std::slice::from_ref(&h.name), p, &[], key_down(VK_SHIFT), false);
            }
            return;
        }
        match self.hit_tile(dx, dy) {
            Some(i) => {
                {
                    let mut st = self.st.borrow_mut();
                    if !st.selected[i] {
                        st.selected.iter_mut().for_each(|s| *s = false);
                        st.selected[i] = true;
                        st.anchor = Some(i);
                    }
                }
                render::invalidate(hwnd);
                let names = Self::selected_names(&self.st.borrow());
                self.show_menu(p, Some(names), false);
            }
            None => self.show_menu(p, None, true),
        }
    }

    fn grid_extras(&self) -> Vec<Extra> {
        if self.kind == Kind::Drawer {
            return vec![Extra::item(CMD_OPEN_FOLDER, "在资源管理器中打开"), Extra::item(CMD_REFRESH, "刷新\tF5")];
        }
        let (locked, mode) = {
            let st = self.st.borrow();
            (st.cfg.locked, st.cfg.collapse)
        };
        vec![
            Extra::item(CMD_OPEN_FOLDER, "在资源管理器中打开"),
            Extra::item(CMD_REFRESH, "刷新\tF5"),
            Extra::Separator,
            Extra::check(CMD_MODE_EXPANDED, "常展开", mode == CollapseMode::Expanded),
            Extra::check(CMD_MODE_CLICK, "点击折叠为胶囊", mode == CollapseMode::Click),
            Extra::check(CMD_MODE_SMART, "智能胶囊（悬停展开，移出收起）", mode == CollapseMode::Smart),
            Extra::Separator,
            Extra::check(CMD_LOCK, "锁定位置和大小", locked),
            Extra::item(CMD_REMOVE, "移除此格子（文件保持不变）"),
        ]
    }

    /// `names = None` → folder background menu (+ grid commands).
    fn show_menu(&self, screen: POINT, names: Option<Vec<String>>, with_extras: bool) {
        let dir = self.st.borrow().dir.clone();
        let extras = if with_extras { self.grid_extras() } else { Vec::new() };
        let has_error = self.st.borrow().error.is_some();
        let extended = key_down(VK_SHIFT);
        let single = names.as_ref().is_some_and(|n| n.len() == 1);
        let owner = app().host();
        let outcome = if has_error {
            match menu::track_plain(owner, screen, &extras) {
                0 => Outcome::Nothing,
                id => Outcome::Extra(id),
            }
        } else {
            menu::track(owner, &dir, names.as_deref().unwrap_or(&[]), screen, &extras, extended, single)
        };
        match outcome {
            Outcome::Extra(id) => self.command(id),
            Outcome::Rename => {
                let idx = {
                    let st = self.st.borrow();
                    st.selected.iter().position(|s| *s)
                };
                if let Some(i) = idx {
                    self.begin_rename(i);
                }
            }
            Outcome::Invoked(verb) => crate::log!("grid {}: invoked '{verb}'", self.id),
            Outcome::Nothing => {}
        }
        if self.st.borrow().edit.is_none() {
            self.rebed();
        }
        self.arm_collapse_timer();
        render::invalidate(self.hwnd());
    }

    fn set_mode(&self, mode: CollapseMode) {
        let s = match mode {
            CollapseMode::Expanded => "expanded",
            CollapseMode::Click => "click",
            CollapseMode::Smart => "smart",
        };
        self.st.borrow_mut().cfg.collapse = mode;
        app().store.borrow_mut().set_grid_str(&self.id, "collapse", s);
        self.apply_mode();
    }

    fn command(&self, id: u32) {
        match id {
            CMD_OPEN_FOLDER => {
                let dir = self.st.borrow().dir.clone();
                shell::open_path(&dir, None);
            }
            CMD_REFRESH => self.reload(),
            CMD_LOCK => {
                let locked = !self.st.borrow().cfg.locked;
                self.st.borrow_mut().cfg.locked = locked;
                app().store.borrow_mut().set_grid_flag(&self.id, "locked", locked);
            }
            CMD_MODE_EXPANDED => self.set_mode(CollapseMode::Expanded),
            CMD_MODE_CLICK => self.set_mode(CollapseMode::Click),
            CMD_MODE_SMART => self.set_mode(CollapseMode::Smart),
            CMD_REMOVE => app().confirm_remove_grid(&self.id),
            _ => {}
        }
    }

    // ---- inline rename -------------------------------------------------------------------

    fn begin_rename(&self, index: usize) {
        let hwnd = self.hwnd();
        {
            let mut st = self.st.borrow_mut();
            let Some(item) = st.items.get(index) else { return };
            let ext_hidden = !item.is_dir && !item.ext.is_empty() && item.display != item.name;
            let shown = if ext_hidden { item.display.clone() } else { item.name.clone() };
            let mut input = TextInput::new(&shown);
            // Like Explorer: select the stem, not the extension.
            let stem_len = if !item.is_dir && !ext_hidden {
                shown.rfind('.').filter(|&d| d > 0).map(|d| shown[..d].chars().count()).unwrap_or(input.len())
            } else {
                input.len()
            };
            input.select(0, stem_len);
            st.edit = Some(Edit {
                index,
                input,
                hidden_ext: ext_hidden.then(|| item.ext.clone()).filter(|e| !e.is_empty()),
                layout: None,
                surrogate: None,
            });
            st.selected.iter_mut().for_each(|s| *s = false);
            if let Some(s) = st.selected.get_mut(index) {
                *s = true;
            }
        }
        self.ensure_visible(index);
        unsafe {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
        }
        render::invalidate(hwnd);
    }

    fn ensure_visible(&self, index: usize) {
        let m = app().metrics.get();
        let mut st = self.st.borrow_mut();
        let area = Self::content_area(&st);
        let gl = Self::grid_layout(&st, &m);
        let r = gl.tile_rect(&m, index, 0.0);
        let top = r.t - area.t;
        let bottom = r.b - area.t + m.line_h * 2.0;
        if top < st.scroll {
            st.scroll = top.max(0.0);
        } else if bottom > st.scroll + area.h() {
            st.scroll = bottom - area.h();
        }
        let max = Self::max_scroll(&st, &m);
        st.scroll = st.scroll.clamp(0.0, max.max(0.0));
    }

    fn cancel_rename(&self) {
        self.st.borrow_mut().edit = None;
        render::invalidate(self.hwnd());
    }

    fn commit_rename(&self) {
        let Some(edit) = self.st.borrow_mut().edit.take() else { return };
        let (dir, old) = {
            let st = self.st.borrow();
            match st.items.get(edit.index) {
                Some(i) => (st.dir.clone(), i.name.clone()),
                None => return,
            }
        };
        let text = edit.input.text().trim().to_owned();
        let new_name = match &edit.hidden_ext {
            Some(ext) => format!("{text}.{ext}"),
            None => text.clone(),
        };
        render::invalidate(self.hwnd());
        if text.is_empty() || new_name == old {
            return;
        }
        if !valid_file_name(&new_name) {
            unsafe {
                let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(MB_ICONWARNING);
            }
            return;
        }
        if shell::rename(app().host(), &shell::join(&dir, &old), &new_name) {
            self.st.borrow_mut().pending_select = Some(new_name);
        }
    }

    fn edit_key(&self, vk: u16) -> bool {
        let ctrl = key_down(VK_CONTROL);
        let shift = key_down(VK_SHIFT);
        match vk {
            v if v == VK_RETURN.0 => {
                self.commit_rename();
                return true;
            }
            v if v == VK_ESCAPE.0 => {
                self.cancel_rename();
                return true;
            }
            _ => {}
        }
        let mut handled = true;
        {
            let mut st = self.st.borrow_mut();
            let Some(e) = st.edit.as_mut() else { return false };
            let t = &mut e.input;
            match vk {
                v if v == VK_LEFT.0 => t.left(shift),
                v if v == VK_RIGHT.0 => t.right(shift),
                v if v == VK_HOME.0 => t.move_to(0, shift),
                v if v == VK_END.0 => t.move_to(t.len(), shift),
                v if v == VK_BACK.0 => t.backspace(),
                v if v == VK_DELETE.0 => t.delete(),
                v if v == VK_A.0 && ctrl => t.select_all(),
                v if v == VK_C.0 && ctrl => crate::win::clipboard_set_text(&t.selected_text()),
                v if v == VK_X.0 && ctrl => {
                    crate::win::clipboard_set_text(&t.selected_text());
                    t.backspace();
                }
                v if v == VK_V.0 && ctrl => {
                    if let Some(s) = crate::win::clipboard_text() {
                        t.insert(&s.replace(['\r', '\n'], ""));
                    }
                }
                _ => handled = false,
            }
        }
        if handled {
            render::invalidate(self.hwnd());
        }
        handled
    }

    fn edit_char(&self, unit: u16) {
        let mut st = self.st.borrow_mut();
        let Some(e) = st.edit.as_mut() else { return };
        if (0xD800..0xDC00).contains(&unit) {
            e.surrogate = Some(unit);
            return;
        }
        let s = if (0xDC00..0xE000).contains(&unit) {
            match e.surrogate.take() {
                Some(hi) => String::from_utf16_lossy(&[hi, unit]),
                None => return,
            }
        } else if unit >= 0x20 {
            String::from_utf16_lossy(&[unit])
        } else {
            return;
        };
        e.input.insert(&s);
        drop(st);
        render::invalidate(self.hwnd());
    }

    // ---- keyboard -----------------------------------------------------------------------------

    fn on_key(&self, vk: u16) -> bool {
        if self.st.borrow().edit.is_some() {
            return self.edit_key(vk);
        }
        if self.st.borrow().filter.is_some() && self.filter_key(vk) {
            return true;
        }
        let hwnd = self.hwnd();
        let ctrl = key_down(VK_CONTROL);
        let alt = key_down(VK_MENU);
        let (dir, names, first) = {
            let st = self.st.borrow();
            (st.dir.clone(), Self::selected_names(&st), st.selected.iter().position(|s| *s))
        };
        let owner = app().host();
        match vk {
            v if v == VK_DELETE.0 && !names.is_empty() => {
                menu::invoke_verb(owner, &dir, &names, "delete");
            }
            v if v == VK_F2.0 => {
                if let Some(i) = first {
                    self.begin_rename(i);
                }
            }
            v if v == VK_RETURN.0 && alt && !names.is_empty() => {
                menu::invoke_verb(owner, &dir, &names, "properties");
            }
            v if v == VK_RETURN.0 => {
                for n in names.iter().take(16) {
                    shell::open_path(&shell::join(&dir, n), Some(&dir));
                }
            }
            v if v == VK_A.0 && ctrl => {
                self.st.borrow_mut().selected.iter_mut().for_each(|s| *s = true);
                render::invalidate(hwnd);
            }
            v if v == VK_C.0 && ctrl && !names.is_empty() => {
                menu::invoke_verb(owner, &dir, &names, "copy");
            }
            v if v == VK_X.0 && ctrl && !names.is_empty() => {
                menu::invoke_verb(owner, &dir, &names, "cut");
            }
            v if v == VK_V.0 && ctrl => {
                menu::invoke_verb(owner, &dir, &[], "paste");
            }
            v if v == VK_F5.0 => self.reload(),
            v if v == VK_ESCAPE.0 => {
                let any = self.st.borrow().selected.iter().any(|s| *s);
                if !any && self.kind == Kind::Drawer {
                    app().hide_drawer();
                }
                self.st.borrow_mut().selected.iter_mut().for_each(|s| *s = false);
                render::invalidate(hwnd);
            }
            _ => return false,
        }
        true
    }

    /// Keys while the filter pill is active. Returns `true` when consumed.
    fn filter_key(&self, vk: u16) -> bool {
        let ctrl = key_down(VK_CONTROL);
        match vk {
            v if v == VK_BACK.0 => {
                if let Some(f) = self.st.borrow_mut().filter.as_mut() {
                    if ctrl {
                        f.select_all();
                    }
                    f.backspace();
                }
                self.on_filter_changed();
            }
            v if v == VK_ESCAPE.0 => self.clear_filter(),
            v if v == VK_V.0 && ctrl => {
                if let Some(t) = crate::win::clipboard_text() {
                    self.filter_insert(&t.replace(['\r', '\n'], " "));
                }
            }
            v if v == VK_RETURN.0 && !key_down(VK_MENU) => {
                let (hit, tile, has_hits) = {
                    let st = self.st.borrow();
                    (st.hit_sel, st.selected.iter().position(|s| *s), !st.hits.is_empty())
                };
                match (hit, tile) {
                    (Some(i), _) => self.open_hit(i),
                    (None, Some(i)) => {
                        let (dir, name) = {
                            let st = self.st.borrow();
                            (st.dir.clone(), st.items[i].name.clone())
                        };
                        shell::open_path(&shell::join(&dir, &name), Some(&dir));
                    }
                    (None, None) if has_hits => self.open_hit(0),
                    _ => {}
                }
            }
            _ => return false,
        }
        true
    }

    fn on_wheel(&self, delta: i32, screen: POINT) {
        if self.kind == Kind::Drawer {
            let mut p = screen;
            unsafe {
                let _ = ScreenToClient(self.hwnd(), &mut p);
            }
            let (_, dy) = Self::to_dip(&self.st.borrow(), p.x, p.y);
            if dy < Self::title_h() {
                let (tab, n) = {
                    let st = self.st.borrow();
                    (st.tab, st.tabs.len())
                };
                if n > 1 {
                    let next = if delta < 0 { (tab + 1) % n } else { (tab + n - 1) % n };
                    self.switch_tab(next, false);
                }
                return;
            }
        }
        let m = app().metrics.get();
        {
            let mut st = self.st.borrow_mut();
            if !matches!(st.fold, Fold::Expanded) {
                return;
            }
            st.edit = None;
            let max = Self::max_scroll(&st, &m);
            let step = m.cell_h() * 0.75;
            st.scroll = (st.scroll - delta as f32 / 120.0 * step).clamp(0.0, max);
        }
        render::invalidate(self.hwnd());
    }

    fn hit_test(&self, x: i32, y: i32) -> u32 {
        let st = self.st.borrow();
        if st.cfg.locked || !matches!(st.fold, Fold::Expanded) || self.kind == Kind::Drawer {
            return HTCLIENT;
        }
        let r = win::window_rect(self.hwnd());
        let e = (theme::RESIZE_EDGE * Self::scale(&st)) as i32;
        let left = x < r.left + e;
        let right = x >= r.right - e;
        let top = y < r.top + e / 2 + 1;
        let bottom = y >= r.bottom - e;
        match (left, right, top, bottom) {
            (true, _, true, _) => HTTOPLEFT,
            (_, true, true, _) => HTTOPRIGHT,
            (true, _, _, true) => HTBOTTOMLEFT,
            (_, true, _, true) => HTBOTTOMRIGHT,
            (true, ..) => HTLEFT,
            (_, true, ..) => HTRIGHT,
            (_, _, true, _) => HTTOP,
            (.., true) => HTBOTTOM,
            _ => HTCLIENT,
        }
    }

    /// Saves the *expanded* rect (DIP, monitor-relative) to config.toml.
    pub fn persist_rect(&self) {
        if self.kind == Kind::Drawer || matches!(self.st.borrow().fold, Fold::Animating { .. }) {
            return;
        }
        let r = self.expanded_rect();
        let mons = monitors::enumerate();
        let Some(m) = monitors::for_rect(&mons, &r) else { return };
        let dip = monitors::to_dip(m, &r);
        {
            let mut st = self.st.borrow_mut();
            st.cfg.rect = Some(dip);
            st.cfg.monitor = m.id.clone();
        }
        app().store.borrow_mut().set_grid_rect(&self.id, &m.id, dip);
    }

    // ---- window procedure -------------------------------------------------------------------------

    fn handle(&self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let hwnd = self.hwnd();
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
                self.paint(hdc);
                unsafe {
                    let _ = EndPaint(hwnd, &ps);
                }
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_SIZE => {
                if let Ok(mut st) = self.st.try_borrow_mut() {
                    st.size = ((lp.0 & 0xFFFF) as i32, ((lp.0 >> 16) & 0xFFFF) as i32);
                }
                self.clamp_scroll();
                render::invalidate(hwnd);
                Some(LRESULT(0))
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_ANIM => self.on_anim_tick(),
                    TIMER_COLLAPSE => self.on_collapse_timer(),
                    TIMER_SLIDE => self.on_slide_tick(),
                    TIMER_HOVER_HIDE => self.on_hover_hide_tick(),
                    TIMER_QUERY => self.run_query(),
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_DPICHANGED => {
                let dpi = crate::util::hiword(wp.0) as u32;
                if let Ok(mut st) = self.st.try_borrow_mut() {
                    st.dpi = dpi;
                }
                let moving = matches!(self.st.borrow().drag, Drag::Move { .. });
                let r = unsafe { *(lp.0 as *const RECT) };
                unsafe {
                    if moving {
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            0,
                            0,
                            r.right - r.left,
                            r.bottom - r.top,
                            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    } else {
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            r.left,
                            r.top,
                            r.right - r.left,
                            r.bottom - r.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
                self.reset_visuals();
                Some(LRESULT(0))
            }
            WM_GETMINMAXINFO => {
                let (s, folded) = match self.st.try_borrow() {
                    Ok(st) => (win::scale(st.dpi), !matches!(st.fold, Fold::Expanded)),
                    Err(_) => (1.0, true),
                };
                let mmi = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
                let min_h = if folded { 1.0 } else { theme::MIN_H * s };
                mmi.ptMinTrackSize = POINT { x: (theme::MIN_W * s) as i32, y: min_h as i32 };
                Some(LRESULT(0))
            }
            WM_NCHITTEST => Some(LRESULT(self.hit_test(x_lparam(lp), y_lparam(lp)) as isize)),
            WM_NCACTIVATE => {
                // Keep the DWM material in its active look even while unfocused.
                Some(unsafe { DefWindowProcW(hwnd, msg, WPARAM(1), lp) })
            }
            WM_MOUSEACTIVATE => {
                let covered = self.kind == Kind::Desktop && desktop::covered_by_foreign(hwnd);
                Some(LRESULT(if covered { MA_NOACTIVATE as isize } else { MA_ACTIVATE as isize }))
            }
            WM_ACTIVATE => {
                let active = (wp.0 & 0xFFFF) != 0;
                if let Ok(mut st) = self.st.try_borrow_mut() {
                    st.active = active;
                }
                if !active {
                    if self.st.borrow().edit.is_some() {
                        self.commit_rename();
                    }
                    // Focus moving to another DeskCrate window (menu host,
                    // dialogs, a sibling grid) is not "leaving".
                    let other = HWND(lp.0 as *mut _);
                    if !app::is_own_window(other) {
                        if self.kind == Kind::Drawer {
                            app().hide_drawer();
                        } else {
                            app().on_focus_left();
                            self.rebed();
                        }
                    }
                }
                render::invalidate(hwnd);
                Some(LRESULT(0))
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                    let mut p = POINT::default();
                    unsafe {
                        let _ = GetCursorPos(&mut p);
                        let _ = ScreenToClient(hwnd, &mut p);
                    }
                    let (dx, dy) = Self::to_dip(&self.st.borrow(), p.x, p.y);
                    let ibeam = self.edit_box_contains(dx, dy).is_some();
                    unsafe {
                        let _ = SetCursor(LoadCursorW(None, if ibeam { IDC_IBEAM } else { IDC_ARROW }).ok());
                    }
                    Some(LRESULT(1))
                } else {
                    None
                }
            }
            WM_LBUTTONDOWN => {
                self.on_lbutton_down(x_lparam(lp), y_lparam(lp));
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                self.on_mouse_move(x_lparam(lp), y_lparam(lp));
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                self.on_lbutton_up(x_lparam(lp), y_lparam(lp));
                Some(LRESULT(0))
            }
            WM_LBUTTONDBLCLK => {
                self.on_double_click(x_lparam(lp), y_lparam(lp));
                Some(LRESULT(0))
            }
            WM_RBUTTONUP => {
                self.on_rbutton_up(x_lparam(lp), y_lparam(lp));
                Some(LRESULT(0))
            }
            WM_MOUSEHOVER => {
                if self.smart() && self.is_collapsed() {
                    self.set_collapsed(false, true);
                }
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                if let Ok(mut st) = self.st.try_borrow_mut() {
                    st.inside = false;
                    st.hover = None;
                    st.hover_button = None;
                }
                self.arm_collapse_timer();
                render::invalidate(hwnd);
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED => {
                let was_move = match self.st.try_borrow_mut() {
                    Ok(mut st) => {
                        let m = matches!(st.drag, Drag::Move { .. });
                        let lost = lp.0 != hwnd.0 as isize;
                        if !matches!(st.drag, Drag::None) && lost {
                            st.drag = Drag::None;
                        }
                        m && lost
                    }
                    Err(_) => false,
                };
                if was_move {
                    self.persist_rect();
                }
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                self.on_wheel(crate::util::wheel_delta(wp), POINT { x: x_lparam(lp), y: y_lparam(lp) });
                Some(LRESULT(0))
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if self.on_key(wp.0 as u16) {
                    Some(LRESULT(0))
                } else {
                    None
                }
            }
            WM_CHAR => {
                if self.st.borrow().edit.is_some() {
                    self.edit_char(wp.0 as u16);
                    return Some(LRESULT(0));
                }
                if !self.is_collapsed() && !key_down(VK_CONTROL) {
                    self.filter_char(wp.0 as u16);
                    return Some(LRESULT(0));
                }
                None
            }
            WM_EXITSIZEMOVE => {
                self.persist_rect();
                self.rebed();
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                unsafe {
                    let _ = RevokeDragDrop(hwnd);
                    let _ = KillTimer(Some(hwnd), TIMER_ANIM);
                    let _ = KillTimer(Some(hwnd), TIMER_COLLAPSE);
                    let _ = KillTimer(Some(hwnd), TIMER_SLIDE);
                    let _ = KillTimer(Some(hwnd), TIMER_HOVER_HIDE);
                    let _ = KillTimer(Some(hwnd), TIMER_QUERY);
                }
                if let Some(w) = self.st.borrow_mut().watch.take() {
                    win::watcher::unwatch(w);
                }
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}

impl DropHost for Grid {
    fn drop_target_at(&self, screen: POINT) -> Option<(String, Option<usize>)> {
        let hwnd = self.hwnd();
        let mut p = screen;
        unsafe {
            let _ = ScreenToClient(hwnd, &mut p);
        }
        let (dx, dy, dir) = {
            let st = self.st.try_borrow().ok()?;
            if st.error.is_some() {
                return None;
            }
            let (dx, dy) = Self::to_dip(&st, p.x, p.y);
            (dx, dy, st.dir.clone())
        };
        if !self.in_title(dy)
            && let Some(i) = self.hit_tile(dx, dy)
        {
            let st = self.st.borrow();
            let item = &st.items[i];
            if item.is_dir {
                return Some((shell::join(&st.dir, &item.name), Some(i)));
            }
        }
        Some((dir, None))
    }

    fn drop_feedback(&self, target: Option<Option<usize>>) {
        let changed = match self.st.try_borrow_mut() {
            Ok(mut st) => {
                let c = st.drop_hl != target;
                st.drop_hl = target;
                c
            }
            Err(_) => false,
        };
        if target.is_some() && self.is_collapsed() {
            // Open a folded capsule so files can be dropped onto sub-folders.
            self.set_collapsed(false, true);
        } else if target.is_none() {
            self.arm_collapse_timer();
        }
        if changed {
            render::invalidate(self.hwnd());
        }
    }
}

pub unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            let ptr = cs.lpCreateParams as *const Grid;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
            (*ptr).hwnd.set(hwnd);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Grid;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            (*ptr).hwnd.set(HWND::default());
            drop(Rc::from_raw(ptr));
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        // Keep the grid alive for this message even if the handler removes it.
        Rc::increment_strong_count(ptr);
        let grid = Rc::from_raw(ptr);
        let r = grid.handle(msg, wp, lp);
        drop(grid);
        r.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
    }
}
