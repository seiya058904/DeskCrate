//! A grid ("crate") window: a live view of one real folder on the desktop layer,
//! foldable into a smart capsule.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_WEIGHT_NORMAL, DWRITE_HIT_TEST_METRICS, IDWriteTextLayout};
use windows::Win32::Graphics::Gdi::{BeginPaint, ClientToScreen, EndPaint, PAINTSTRUCT, ScreenToClient, UpdateWindow};
use windows::Win32::System::Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop};
use windows::Win32::UI::Input::Ime::{
    CFS_POINT, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext, ImmSetCompositionWindow,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_HOVER, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_A,
    VK_APPS, VK_BACK, VK_C, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F2, VK_F5, VK_F10, VK_HOME, VK_LEFT,
    VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP, VK_V, VK_X,
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
use crate::ui::motion::{self, Spring, Tween};
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
const CMD_RENAME_TITLE: u32 = EXTRA_BASE + 8;
const CMD_LOCK_SIZE: u32 = EXTRA_BASE + 9;

/// Not exported by windows-rs' WindowsAndMessaging module.
const WM_MOUSEHOVER: u32 = 0x02A1;
const WM_MOUSELEAVE: u32 = 0x02A3;

const TIMER_COLLAPSE: usize = 2;
const TIMER_HOVER_HIDE: usize = 4;
const TIMER_QUERY: usize = 5;
/// Fades the scrollbar out after inactivity (upstream `ScrollBarAutoHideDelay` 3 s).
const TIMER_BAR: usize = 6;
/// Marquee auto-scroll near the edges (upstream `InteractiveAutoScrollInterval` 32 ms).
const TIMER_AUTOSCROLL: usize = 7;
/// Deferred name expansion / slow-click rename (one double-click interval).
const TIMER_EXPAND: usize = 8;
const TIMER_SLOW_RENAME: usize = 9;
/// Re-check desktop bedding after a launch (upstream: 600 ms after open).
const TIMER_REBED: usize = 10;
/// Tooltip show (one double-click interval, like `TTDT_INITIAL`) and auto-pop.
const TIMER_TIP: usize = 11;
const TIMER_TIP_HIDE: usize = 12;
const BAR_HIDE_MS: u32 = 3000;
const AUTOSCROLL_ZONE: f32 = 28.0;

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
    x: Spring,
    y: Spring,
    hide: bool,
}

/// Something with an animated hover highlight.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fx {
    Tile(usize),
    Button(Button),
    Tab(usize),
    Hit(usize),
    /// Press depth of a tile / title button (drives the press scale).
    Press(usize),
    PressButton(Button),
    /// Capsule drag grip (replaces the identity icon while hovering its left edge).
    Grip,
    /// Selected-name expansion fade (upstream 0.55 → 1 over 90 ms).
    Expand(usize),
}

/// Hover highlight fade (ms): quick in, softer out — Fluent "fast in, slow out".
const FX_IN_MS: f32 = 83.0;
const FX_OUT_MS: f32 = 167.0;
/// Press: sinks almost instantly, springs back slower (iOS-like).
const PRESS_IN_MS: f32 = 60.0;
const PRESS_OUT_MS: f32 = 260.0;
const PRESS_SCALE: f32 = 0.05;
/// Toast lifetime (fade in 120 ms, hold, fade out 220 ms).
const TOAST_MS: f64 = 1800.0;
/// Wheel scroll glide.
const SCROLL_MS: f32 = 180.0;

const GLYPH_UP: char = '\u{E70E}';
const GLYPH_DOWN: char = '\u{E70D}';
const GLYPH_SEARCH: char = '\u{E721}';
const GLYPH_CLEAR: char = '\u{E711}';

#[derive(Clone, Copy, PartialEq, Debug)]
enum Button {
    /// Upstream `AddButton` (default hover action): new grid.
    Add,
    More,
    Fold,
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    None,
    Pending { x: i32, y: i32, index: Option<usize>, title: bool, button: Option<Button> },
    Move { cursor: POINT, rect: RECT, snap: crate::app::SnapState },
    Marquee { x0: f32, y0: f32, x1: f32, y1: f32 },
    PendingHit { x: i32, y: i32, i: usize },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fold {
    Expanded,
    Collapsed,
    /// Window height (px) driven by a spring; reversible at any frame.
    Animating {
        spring: Spring,
        collapsing: bool,
    },
}

struct Edit {
    index: usize,
    /// Editing the grid title (double-click on the title, upstream `StartTitleRename`).
    title: bool,
    input: TextInput,
    /// Re-appended on commit when Explorer hides the extension.
    hidden_ext: Option<String>,
    /// Last laid-out text + origin (DIP), for caret hit-testing.
    layout: Option<(IDWriteTextLayout, f32, f32)>,
    surrogate: Option<u16>,
}

impl GridState {
    fn last_pt_dip(&self) -> (f32, f32) {
        let s = win::scale(self.dpi);
        (self.last_pt.0 as f32 / s, self.last_pt.1 as f32 / s)
    }
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
    fx: Vec<(Fx, Tween)>,
    scroll_tw: Option<Tween>,
    /// Scrollbar visibility (auto-hides after inactivity).
    bar: Tween,
    /// Keyboard focus item (lead of a Shift range; `anchor` is the other end).
    lead: Option<usize>,
    /// Sole selection and when it became sole (slow-click rename gating).
    sole_since: Option<(usize, f64)>,
    /// Name expansion may show for this item once the time has passed.
    expand_ready: Option<(usize, f64)>,
    /// After an open, hover stays off until the pointer actually moves.
    suppress_hover: Option<(i32, i32)>,
    /// Last pointer position (client px) for auto-scroll.
    last_pt: (i32, i32),
    tooltip: HWND,
    tip_for: Option<usize>,
    /// Snap edge glow (left, top, right, bottom).
    glow: [Tween; 4],
    /// Transient in-grid feedback (upstream `WidgetFeedbackPresenter`): text, shown at.
    toast: Option<(String, f64)>,
    /// Set by a paint that left animations running; consumed by `frame`.
    wants_frame: bool,
}

pub struct Grid {
    pub id: String,
    pub kind: Kind,
    pub token: u64,
    pub hwnd: Cell<HWND>,
    pub st: RefCell<GridState>,
    weak: RefCell<Weak<Grid>>,
    /// Per-grid tile metrics when the icon size is overridden (Ctrl+wheel).
    m_override: Cell<Option<TileMetrics>>,
}

fn key_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
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
                fx: Vec::new(),
                scroll_tw: None,
                bar: Tween::at(0.0),
                lead: None,
                sole_since: None,
                expand_ready: None,
                suppress_hover: None,
                last_pt: (0, 0),
                tooltip: HWND::default(),
                tip_for: None,
                glow: [Tween::at(0.0); 4],
                toast: None,
                wants_frame: false,
            }),
            weak: RefCell::new(Weak::new()),
            m_override: Cell::new(None),
        });
        g.sync_metrics();
        *g.weak.borrow_mut() = Rc::downgrade(&g);
        g
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd.get()
    }

    /// Tile metrics for this grid (global appearance + optional icon-size override).
    fn metrics(&self) -> TileMetrics {
        self.m_override.get().unwrap_or_else(|| app().metrics.get())
    }

    fn sync_metrics(&self) {
        let size = self.st.borrow().cfg.icon_size;
        self.m_override.set(size.map(|px| {
            let mut a = app().cfg().appearance.clone();
            a.icon_size = px;
            TileMetrics::new(&a)
        }));
    }

    /// Shows a short feedback pill at the bottom of the grid.
    fn toast(&self, text: String) {
        self.st.borrow_mut().toast = Some((text, motion::now_ms()));
        self.st.borrow_mut().wants_frame = true;
        render::invalidate(self.hwnd());
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
                        crate::i18n::tr("grid.not_found")
                    } else {
                        crate::i18n::tr_n("grid.read_error", code as usize)
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

    /// Where the fold is heading: collapsed, or animating toward collapsed.
    fn heading_collapsed(&self) -> bool {
        matches!(self.st.borrow().fold, Fold::Collapsed | Fold::Animating { collapsing: true, .. })
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
        if self.heading_collapsed() == collapse {
            return;
        }
        let expanded = self.expanded_rect();
        let anchor = Self::decide_anchor(&expanded);
        let base = app().cfg().capsule.animation_ms as f32;
        let now = motion::now_ms();
        let to = {
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
            let to = if collapse { Self::capsule_px(&st) } else { Self::expanded_px(&st) } as f32;
            // Spring tuned so ~90 % of the travel lands within the upstream
            // SoftOut window; closing is 0.78× like upstream. Slight underdamping kills the\n            // slow tail; any overshoot is clamped to the capsule/expanded size.
            let (resp, damp) = if collapse { (base * 0.78 * 0.00075, 0.9) } else { (base * 0.00075, 0.86) };
            if base > 0.0 && animate {
                let spring = match st.fold {
                    // Interrupted: keep position and velocity, just change the goal.
                    Fold::Animating { mut spring, .. } => {
                        spring.target = to;
                        spring.tune(resp, damp);
                        spring
                    }
                    _ => Spring::new(st.size.1 as f32, to, resp, damp, now),
                };
                st.fold = Fold::Animating { spring, collapsing: collapse };
            } else {
                st.fold = if collapse { Fold::Collapsed } else { Fold::Expanded };
            }
            to
        };
        if matches!(self.st.borrow().fold, Fold::Animating { .. }) {
            self.step_fold(now);
        } else {
            self.apply_height(to.round() as i32);
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

    /// Advances the fold spring; `true` while it still runs.
    fn step_fold(&self, now: f64) -> bool {
        let (h, done) = {
            let mut st = self.st.borrow_mut();
            let Fold::Animating { mut spring, collapsing } = st.fold else { return false };
            let done = spring.step(now);
            st.fold = if done {
                if collapsing { Fold::Collapsed } else { Fold::Expanded }
            } else {
                Fold::Animating { spring, collapsing }
            };
            // Overshoot of an expanding spring never grows the window past its size.
            let max = Self::expanded_px(&st) as f32;
            (spring.x.clamp(Self::capsule_px(&st) as f32, max).round() as i32, done)
        };
        if h != self.st.borrow().size.1 {
            self.apply_height(h);
        }
        // Paint synchronously so the new size and its content reach DWM in the same frame.
        render::invalidate(self.hwnd());
        unsafe {
            let _ = UpdateWindow(self.hwnd());
        }
        !done
    }

    /// Body transition while folding, as a function of the *current height*
    /// (not time), so reversing mid-flight is seamless: (opacity, dy, scale).
    fn body_transition(st: &GridState) -> Option<(f32, f32, f32)> {
        let Fold::Animating { .. } = st.fold else { return None };
        let (cap, exp) = (Self::capsule_px(st) as f32, Self::expanded_px(st) as f32);
        let p = ((st.size.1 as f32 - cap) / (exp - cap).max(1.0)).clamp(0.0, 1.0);
        let o = motion::smoothstep(motion::track(p, 0.30, 0.85));
        let dir = if st.anchor_bottom { 1.0 } else { -1.0 };
        Some((o, dir * 4.0 * (1.0 - o), motion::lerp(0.96, 1.0, motion::out_cubic(o))))
    }

    pub fn is_busy(&self) -> bool {
        let st = self.st.borrow();
        st.edit.is_some() || !matches!(st.drag, Drag::None) || st.drop_hl.is_some() || st.slide.is_some()
    }

    /// One display frame: steps fold, slide, scroll and hover animations.
    pub fn frame(&self, now: f64) -> bool {
        let hwnd = self.hwnd();
        if hwnd.is_invalid() {
            return false;
        }
        let mut any = self.step_fold(now);
        any |= self.step_slide(now);
        let repaint = {
            let mut st = self.st.borrow_mut();
            let mut r = std::mem::take(&mut st.wants_frame);
            if let Some(tw) = st.scroll_tw {
                st.scroll = tw.value(now);
                if !tw.active(now) {
                    st.scroll_tw = None;
                }
                r = true;
            }
            r
        };
        if repaint {
            render::invalidate(hwnd);
            unsafe {
                let _ = UpdateWindow(hwnd);
            }
            // The paint re-arms `wants_frame` if hover fades are still running.
            any |= self.st.borrow().wants_frame || self.st.borrow().scroll_tw.is_some();
        }
        {
            let mut st = self.st.borrow_mut();
            let mut more = st.bar.active(now);
            if let Some((_, t)) = st.toast {
                if now - t > TOAST_MS {
                    st.toast = None;
                    more = true;
                } else {
                    more = true;
                }
            }
            if more {
                st.wants_frame = true;
                any = true;
            }
        }
        any
    }

    /// Retargets hover fades from the current hover state; called from paint.
    fn sync_fx(st: &mut GridState, now: f64) {
        let press = match st.drag {
            Drag::Pending { index: Some(i), .. } if st.hover == Some(i) => Some(Fx::Press(i)),
            Drag::Pending { button: Some(b), .. } if st.hover_button == Some(b) => Some(Fx::PressButton(b)),
            _ => None,
        };
        let grip = (Self::capsule_look(st)
            && st.inside
            && st.last_pt_dip().0 < theme::CAPSULE_PAD_L + 38.0
            && matches!(st.drag, Drag::None | Drag::Pending { .. }))
        .then_some(Fx::Grip);
        let expand = Self::name_expansion(st, now).map(Fx::Expand);
        let mut hot: [Option<Fx>; 7] = [
            grip,
            expand,
            press,
            st.hover.map(Fx::Tile).or(match st.drop_hl {
                Some(Some(i)) => Some(Fx::Tile(i)),
                _ => None,
            }),
            st.hover_button.map(Fx::Button),
            st.hover_tab.map(Fx::Tab),
            st.hit_hover.map(Fx::Hit),
        ];
        if !matches!(st.drag, Drag::None | Drag::Pending { .. } | Drag::PendingHit { .. }) {
            hot = [None, None, None, None, None, None, None];
        }
        let ms = |k: &Fx, on: bool| match (k, on) {
            (Fx::Press(_) | Fx::PressButton(_), true) => PRESS_IN_MS,
            (Fx::Press(_) | Fx::PressButton(_), false) => PRESS_OUT_MS,
            (Fx::Expand(_), true) => 90.0,
            (Fx::Expand(_), false) => 1.0,
            (Fx::Grip, true) => 140.0,
            (Fx::Grip, false) => 200.0,
            (_, true) => FX_IN_MS,
            (_, false) => FX_OUT_MS,
        };
        for (k, tw) in st.fx.iter_mut() {
            if !hot.contains(&Some(*k)) {
                tw.set(0.0, ms(k, false), now);
            }
        }
        for k in hot.into_iter().flatten() {
            match st.fx.iter_mut().find(|(x, _)| *x == k) {
                Some((_, tw)) => tw.set(1.0, ms(&k, true), now),
                None => {
                    let mut tw = Tween::at(0.0);
                    tw.set(1.0, ms(&k, true), now);
                    st.fx.push((k, tw));
                }
            }
        }
        st.fx.retain(|(_, tw)| tw.active(now) || tw.target() > 0.0);
        if st.fx.iter().any(|(_, tw)| tw.active(now)) {
            st.wants_frame = true;
        }
    }

    fn bar_show(st: &mut GridState, hwnd: HWND) {
        let now = motion::now_ms();
        st.bar.set(1.0, 120.0, now);
        st.wants_frame = true;
        unsafe { SetTimer(Some(hwnd), TIMER_BAR, BAR_HIDE_MS, None) };
    }

    fn fx(st: &GridState, k: Fx, now: f64) -> f32 {
        st.fx.iter().find(|(x, _)| *x == k).map_or(0.0, |(_, tw)| tw.value(now))
    }

    fn smart(&self) -> bool {
        self.st.borrow().cfg.collapse == CollapseMode::Smart
    }

    fn arm_collapse_timer(&self) {
        if self.kind == Kind::Desktop && self.smart() && !self.heading_collapsed() {
            // Leaving while it is still opening reads as "never mind": fold back sooner.
            let opening = matches!(self.st.borrow().fold, Fold::Animating { .. });
            let delay = app().cfg().capsule.collapse_delay_ms;
            let ms = if opening { delay.min(80) } else { delay }.max(50);
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
            st.scroll_tw = None;
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
        let m = self.metrics();
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
        let r = match Self::visible_buttons(st).last() {
            Some(b) => Self::button_rect(st, *b).l - 4.0,
            None => w - theme::TITLE_PAD_R,
        };
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
            st.scroll_tw = None;
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
        let now = motion::now_ms();
        let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
        let from = {
            let mut st = self.st.borrow_mut();
            st.revealed_by_key = by_key;
            st.outside_ticks = 0;
            // Re-revealed while sliding out: turn around from where it is, keeping momentum.
            let (mut x, mut y) = match st.slide {
                Some(sl) if visible => (sl.x, sl.y),
                _ => (Spring::new(from.0 as f32, 0.0, 0.0, 0.0, now), Spring::new(from.1 as f32, 0.0, 0.0, 0.0, now)),
            };
            for (sp, t) in [(&mut x, target.left), (&mut y, target.top)] {
                sp.target = t as f32;
                sp.tune(0.32, 0.86);
            }
            st.slide = Some(Slide { x, y, hide: false });
            (x.x.round() as i32, y.x.round() as i32)
        };
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), from.0, from.1, w, h, SWP_SHOWWINDOW | SWP_NOACTIVATE);
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
            let now = motion::now_ms();
            let (mut x, mut y) = match st.slide {
                Some(sl) => (sl.x, sl.y),
                None => (Spring::new(r.left as f32, 0.0, 0.0, 0.0, now), Spring::new(r.top as f32, 0.0, 0.0, 0.0, now)),
            };
            let (tx, ty) = Self::offscreen_target(&r, app().cfg().drawer.edge);
            x.target = tx as f32;
            y.target = ty as f32;
            x.tune(0.28, 1.0);
            y.tune(0.28, 1.0);
            st.slide = Some(Slide { x, y, hide: true });
        }
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_HOVER_HIDE);
        }
    }

    /// Where a concealing drawer slides to: fully past its screen edge when no
    /// monitor lies beyond (so it never appears on a neighbour), else a third.
    fn offscreen_target(r: &RECT, edge: crate::config::Edge) -> (i32, i32) {
        use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let cy = (r.top + r.bottom) / 2;
        let cx = (r.left + r.right) / 2;
        let (probe, full, part) = match edge {
            crate::config::Edge::Left => {
                (POINT { x: r.left - 2, y: cy }, (r.left - w - 8, r.top), (r.left - w / 3, r.top))
            }
            crate::config::Edge::Top => {
                (POINT { x: cx, y: r.top - 2 }, (r.left, r.top - h - 8), (r.left, r.top - h / 4))
            }
            _ => (POINT { x: r.right + 2, y: cy }, (r.right + 8, r.top), (r.left + w / 3, r.top)),
        };
        let neighbour = unsafe { !MonitorFromPoint(probe, MONITOR_DEFAULTTONULL).is_invalid() };
        if neighbour { part } else { full }
    }

    fn step_slide(&self, now: f64) -> bool {
        let hwnd = self.hwnd();
        let Some(mut sl) = self.st.borrow().slide else { return false };
        let done = sl.x.step(now) & sl.y.step(now);
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                sl.x.x.round() as i32,
                sl.y.x.round() as i32,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        if done {
            self.st.borrow_mut().slide = None;
            if sl.hide {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                app().schedule_idle_trim();
            }
            return false;
        }
        self.st.borrow_mut().slide = Some(sl);
        true
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
            theme::CONTENT_PAD_L,
            Self::title_h() + theme::CONTENT_PAD_TOP,
            w - theme::CONTENT_PAD_R,
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
        let m = self.metrics();
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
        let capsule = Self::capsule_look(st);
        let size = if capsule { theme::CAPSULE_BUTTON } else { theme::TITLE_BUTTON };
        let top = ((Self::row_h(st) - size) / 2.0 - if capsule { 0.0 } else { 1.0 }).round();
        let r = w - if capsule { theme::CAPSULE_PAD_R } else { theme::TITLE_PAD_R };
        // Laid out right-to-left: [Add] [More] [Fold] like upstream's action strip.
        let list = Self::visible_buttons(st);
        let slot = list.iter().rev().position(|x| *x == b).unwrap_or(0) as f32;
        let right = r - slot * (size + theme::TITLE_BUTTON_GAP);
        RectF::new(right - size, top, right, top + size)
    }

    fn visible_buttons(st: &GridState) -> Vec<Button> {
        if Self::capsule_look(st) {
            // Capsule: only the expand chevron (upstream `CompactExpandButton`).
            return vec![Button::Fold];
        }
        let mut v = Vec::new();
        if st.tabs.is_empty() {
            v.push(Button::Add);
        }
        v.push(Button::More);
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
        let m = self.metrics();
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
        let now = motion::now_ms();
        Self::sync_fx(st, now);
        if matches!(st.fold, Fold::Animating { .. }) {
            st.wants_frame = true;
        }

        // ---- title row / capsule ----
        let capsule = Self::capsule_look(st);
        if capsule {
            self.draw_capsule(cv, st, th, title, now);
            self.draw_buttons(cv, st, th, inner, now);
            if let Some(None) = st.drop_hl {
                cv.stroke_round(full.inset(1.0, 1.0), (outer - 1.0).max(0.0), th.neutral_line, 1.5);
            }
            Self::draw_glow(cv, st, th, now);
            return;
        }
        let row_h = Self::row_h(st);
        let icon_r = RectF::new(
            theme::TITLE_PAD_L,
            ((row_h - theme::TITLE_ICON) / 2.0).round(),
            theme::TITLE_PAD_L + theme::TITLE_ICON,
            ((row_h - theme::TITLE_ICON) / 2.0).round() + theme::TITLE_ICON,
        );
        if st.filter.is_none() {
            cv.vector(crate::ui::vector::LAPTOP, 20.0, icon_r);
        }
        let text_l = icon_r.r + theme::TITLE_ICON_GAP;
        let buttons = Self::visible_buttons(st);
        let mut right = w - theme::TITLE_PAD_R;
        if let Some(last) = buttons.last() {
            right = Self::button_rect(st, *last).l - 4.0;
        }
        let text_r = right.max(text_l + 10.0);
        let editing_title = st.edit.as_ref().is_some_and(|e| e.title);
        if editing_title {
            self.draw_edit(cv, st, th, m, RectF::new(text_l, 0.0, text_r, row_h), s);
        } else if st.filter.is_some() {
            self.draw_filter_pill(cv, st, th, icon_r, inner, s);
        } else if self.kind == Kind::Drawer && !st.tabs.is_empty() {
            self.draw_tabs(cv, st, th, text_l, text_r, inner);
        } else {
            cv.text(
                title,
                RectF::new(text_l, 0.0, text_r, row_h - 2.0),
                theme::TITLE_TEXT,
                false,
                Align::Leading,
                th.text,
            );
        }
        let _ = buttons;
        self.draw_buttons(cv, st, th, inner, now);
        if let Some(None) = st.drop_hl {
            cv.stroke_round(full.inset(1.0, 1.0), (outer - 1.0).max(0.0), th.neutral_line, 1.5);
        }
        let dy = Self::title_h() - theme::DIVIDER_MARGIN_B - px / 2.0;
        cv.line(theme::DIVIDER_MARGIN_H, dy, w - theme::DIVIDER_MARGIN_H, dy, th.divider, px);

        // ---- content ----
        let area = Self::content_area(st);
        let group = Self::body_transition(st)
            .map(|(o, dy, k)| cv.push_group(RectF::new(0.0, Self::title_h(), w, h), o, dy, k))
            .unwrap_or(false);
        if let Some(err) = &st.error {
            let msg = format!("{err}\n{}", st.dir);
            self.message(cv, area, &msg, th);
        } else if st.items.is_empty() && st.hits.is_empty() {
            let msg = match &st.filter {
                Some(f) => crate::i18n::trf("grid.no_match", &[&f.text()]),
                None => crate::i18n::tr("grid.empty"),
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
            // Upstream marquee uses the neutral interaction palette, not the accent.
            cv.fill_rect(r, th.neutral_fill);
            cv.stroke_round(r, 0.0, th.neutral_line, px);
            cv.unclip();
        }
        if st.edit.as_ref().is_some_and(|e| !e.title) {
            self.draw_edit(cv, st, th, m, area, s);
        }
        if group {
            cv.pop_group();
        }
        self.draw_toast(cv, st, th, now);
        Self::draw_glow(cv, st, th, now);
    }

    /// Upstream snap highlight: an accent band fading inward from the snapped
    /// edge, its tips fading out along the edge.
    fn draw_glow(cv: &Canvas, st: &mut GridState, th: &theme::Theme, now: f64) {
        let (w, h) = Self::dims(st);
        let band = theme::SNAP_GLOW;
        let a = th.accent;
        let stops = [
            (0.0, a),
            (0.22, a.scale_alpha(180.0 / 255.0)),
            (0.55, a.scale_alpha(70.0 / 255.0)),
            (1.0, a.with_alpha(0.0)),
        ];
        for (i, tw) in st.glow.iter().enumerate() {
            let k = tw.value(now);
            if k <= 0.001 {
                continue;
            }
            let st_k: Vec<(f32, Rgba)> = stops.iter().map(|&(p, c)| (p, c.scale_alpha(k))).collect();
            let vertical = i % 2 == 0;
            let len = if vertical { h } else { w };
            // Tip fades along the edge (0–12 % and 88–100 %), drawn as slices.
            const N: usize = 10;
            for j in 0..N {
                let (t0, t1) = (j as f32 / N as f32, (j + 1) as f32 / N as f32);
                let mid = (t0 + t1) / 2.0;
                let tip = motion::smoothstep((mid / 0.12).min((1.0 - mid) / 0.12).min(1.0));
                let tk: Vec<(f32, Rgba)> = st_k.iter().map(|&(p, c)| (p, c.scale_alpha(tip))).collect();
                let (r, from, to) = match i {
                    0 => (RectF::new(0.0, len * t0, band, len * t1), (0.0, 0.0), (band, 0.0)),
                    2 => (RectF::new(w - band, len * t0, w, len * t1), (w, 0.0), (w - band, 0.0)),
                    1 => (RectF::new(len * t0, 0.0, len * t1, band), (0.0, 0.0), (0.0, band)),
                    _ => (RectF::new(len * t0, h - band, len * t1, h), (0.0, h), (0.0, h - band)),
                };
                cv.fill_gradient(r, from, to, &tk);
            }
            if tw.active(now) {
                st.wants_frame = true;
            }
        }
    }

    fn set_glow(&self, edges: [bool; 4]) {
        let now = motion::now_ms();
        let mut st = self.st.borrow_mut();
        let mut changed = false;
        for (tw, on) in st.glow.iter_mut().zip(edges) {
            let target = if on { 1.0 } else { 0.0 };
            if (tw.target() - target).abs() > 0.5 {
                tw.set(target, if on { 160.0 } else { 100.0 }, now);
                changed = true;
            }
        }
        if changed {
            st.wants_frame = true;
            drop(st);
            render::invalidate(self.hwnd());
        }
    }

    fn draw_toast(&self, cv: &Canvas, st: &GridState, th: &theme::Theme, now: f64) {
        let Some((text, t0)) = &st.toast else { return };
        let age = now - t0;
        let o = (motion::track(age as f32, 0.0, 120.0)
            * (1.0 - motion::track(age as f32, (TOAST_MS - 220.0) as f32, TOAST_MS as f32)))
        .clamp(0.0, 1.0);
        if o <= 0.0 {
            return;
        }
        let (w, h) = Self::dims(st);
        let renderer = cv.renderer();
        let tw = renderer
            .text_format(12.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading, Wrap::Line, false)
            .and_then(|f| renderer.layout(text, &f, w - 40.0, 30.0))
            .map_or(60.0, |l| render::text_width(&l));
        let pw = (tw + 24.0).min(w - 16.0);
        let rise = 6.0 * (1.0 - motion::out_cubic(motion::track(age as f32, 0.0, 160.0)));
        let r = RectF::new((w - pw) / 2.0, h - 42.0 + rise, (w + pw) / 2.0, h - 14.0 + rise);
        let (bg, line) = if th.dark {
            (Rgba::argb(0xF22C2C2C), Rgba::argb(0x24FFFFFF))
        } else {
            (Rgba::argb(0xF5FBFBFB), Rgba::argb(0x17000000))
        };
        cv.fill_round(r, 6.0, bg.scale_alpha(o));
        cv.stroke_round(r.inset(0.5, 0.5), 6.0, line.scale_alpha(o), 1.0);
        cv.text(text, r.inset(12.0, 0.0), 12.0, false, Align::Center, th.text.scale_alpha(o));
    }

    fn draw_buttons(&self, cv: &Canvas, st: &GridState, th: &theme::Theme, inner: f32, now: f64) {
        let capsule = Self::capsule_look(st);
        for b in Self::visible_buttons(st) {
            let br = Self::button_rect(st, b);
            let kp = Self::fx(st, Fx::PressButton(b), now);
            let fill = th.hover.scale_alpha(Self::fx(st, Fx::Button(b), now)).mix(th.pressed, kp);
            cv.fill_round(br, inner, fill);
            let restore =
                (kp > 0.001).then(|| cv.scale_about(br.l + br.w() / 2.0, br.t + br.h() / 2.0, 1.0 - 0.1 * kp));
            let icon = theme::TITLE_ACTION_ICON;
            let ir = RectF::new(
                br.l + (br.w() - icon) / 2.0,
                br.t + (br.h() - icon) / 2.0,
                br.l + (br.w() + icon) / 2.0,
                br.t + (br.h() + icon) / 2.0,
            );
            match b {
                Button::Add => cv.glyph_fit(&crate::ui::vector::ADD, ir, th.text),
                Button::More => cv.glyph_fit(&crate::ui::vector::MORE, ir, th.text),
                Button::Fold if capsule => cv.glyph(GLYPH_DOWN, br, 12.0, th.text),
                Button::Fold => cv.glyph(GLYPH_UP, br, icon, th.text),
            }
            if let Some(m) = restore {
                cv.set_transform(m);
            }
        }
    }

    /// Upstream "Smart" capsule: identity icon, title · latest item · count,
    /// expand chevron; the left edge turns into a drag grip on hover.
    fn draw_capsule(&self, cv: &Canvas, st: &GridState, th: &theme::Theme, title: &str, now: f64) {
        let (w, h) = Self::dims(st);
        let s = Self::scale(st);
        let snap = |v: f32| (v * s).round() / s;
        let id_l = theme::CAPSULE_PAD_L;
        let grip = Self::fx(st, Fx::Grip, now);
        let ic = theme::CAPSULE_ICON;
        let ix = snap(id_l + (theme::CAPSULE_IDENTITY_W - ic) / 2.0);
        let iy = snap((h - ic) / 2.0);
        if grip < 0.999 {
            let icon_r = RectF::new(ix, iy, ix + ic, iy + ic);
            if grip > 0.001 {
                cv.push_group(icon_r.inset(-1.0, -1.0), 1.0 - grip, 0.0, 1.0);
                cv.vector(crate::ui::vector::LAPTOP, 20.0, icon_r);
                cv.pop_group();
            } else {
                cv.vector(crate::ui::vector::LAPTOP, 20.0, icon_r);
            }
        }
        if grip > 0.001 {
            // 2 × 3 dots, 2.5 DIP with 3 DIP gaps (upstream `CompactLeftDragGrip`).
            let (dw, gap) = (2.5, 3.0);
            let gx = snap(id_l + 8.0);
            let gy = snap((h - (dw * 3.0 + gap * 2.0)) / 2.0);
            let c = th.text.scale_alpha(0.9 * grip);
            for row in 0..3 {
                for col in 0..2 {
                    let x = gx + col as f32 * (dw + gap);
                    let y = gy + row as f32 * (dw + gap);
                    cv.fill_round(RectF::new(x, y, x + dw, y + dw), dw / 2.0, c);
                }
            }
        }
        let text_l = id_l + theme::CAPSULE_IDENTITY_W + 4.0;
        let right =
            Self::visible_buttons(st).first().map_or(w - theme::CAPSULE_PAD_R, |b| Self::button_rect(st, *b).l) - 6.0;
        if right <= text_l + 8.0 {
            return;
        }
        let renderer = cv.renderer();
        let title_w = renderer
            .text_format(theme::CAPSULE_TITLE, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading, Wrap::Line, false)
            .and_then(|f| renderer.layout(title, &f, right - text_l, h))
            .map_or(0.0, |l| render::text_width(&l))
            .min(right - text_l);
        cv.text(title, RectF::new(text_l, 0.0, right, h), theme::CAPSULE_TITLE, false, Align::Leading, th.text);
        // Summary only in Standard/Wide tiers (upstream hides it below 210 DIP).
        let summary = Self::capsule_summary(st);
        let sx = text_l + title_w + 6.0;
        if w >= 210.0 && !summary.is_empty() && sx + 3.0 + 6.0 + 24.0 < right {
            let dot = RectF::new(sx, (h - 3.0) / 2.0, sx + 3.0, (h + 3.0) / 2.0);
            cv.fill_round(dot, 1.5, th.text_tertiary);
            let tx = dot.r + 6.0;
            cv.text(
                &summary,
                RectF::new(tx, 0.0, right, h + 1.0),
                theme::CAPSULE_SUMMARY,
                false,
                Align::Leading,
                th.text_secondary,
            );
        }
    }

    /// "latest item · N 项" (upstream `CreateFileCompactPresentation`, Smart mode).
    fn capsule_summary(st: &GridState) -> String {
        if st.error.is_some() {
            return String::new();
        }
        let count = crate::i18n::tr_n("capsule.count", st.all_items.len());
        match st.all_items.iter().filter(|i| i.mtime != 0).max_by_key(|i| i.mtime) {
            Some(latest) => format!("{} · {count}", latest.display),
            None => count,
        }
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
            if i != st.tab {
                cv.fill_round(tr, inner, th.hover.scale_alpha(Self::fx(st, Fx::Tab(i), motion::now_ms())));
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
        let editing = st.edit.as_ref().filter(|e| !e.title).map(|e| e.index);
        let now = motion::now_ms();
        let radius = theme::tile_radius(inner);
        let expanded = Self::name_expansion(st, now);
        cv.clip(area);
        let (bmp_rt, generation) = cv.bitmap_target();
        let mut icons = app().icons.borrow_mut();
        for i in gl.visible(area.h(), st.scroll) {
            let r = gl.tile_rect(m, i, st.scroll);
            if r.b < area.t || r.t > area.b {
                continue;
            }
            let sel = st.selected.get(i).copied().unwrap_or(false);
            let k = Self::fx(st, Fx::Tile(i), now);
            let kp = Self::fx(st, Fx::Press(i), now);
            let restore =
                (kp > 0.001).then(|| cv.scale_about(r.l + r.w() / 2.0, r.t + r.h() / 2.0, 1.0 - PRESS_SCALE * kp));
            let fill = match (sel, st.active) {
                (false, _) if kp > 0.0 => th.hover.scale_alpha(k).mix(th.pressed, kp),
                (true, true) => th.selected.mix(th.selected_hover, k),
                (true, false) => th.selected_unfocused.mix(th.selected_unfocused_hover, k),
                (false, _) => th.hover.scale_alpha(k),
            };
            let is_expanded = expanded == Some(i);
            if !is_expanded {
                cv.fill_round(r, radius, fill);
                if sel && st.active {
                    cv.stroke_round(r.inset(0.5 / s, 0.5 / s), radius, th.selected_border, 1.0 / s);
                }
            }
            let item = &st.items[i];
            let ix = snap(r.l + (r.w() - m.icon) / 2.0);
            let iy = snap(r.t + m.pad_y);
            let icon_rect = RectF::new(ix, iy, ix + icon_px as f32 / s, iy + icon_px as f32 / s);
            let key = item.icon_key_with(&st.dir, ap.thumbnails);
            let sample = shell::join(&st.dir, &item.name);
            match icons.get(&key, icon_px, &sample, &bmp_rt, generation) {
                Some(bmp) => cv.bitmap_fit(&bmp, icon_rect, if item.attrs & 0x2 != 0 { 0.55 } else { 1.0 }),
                None => {
                    // Upstream loading placeholder: 36-unit box, 2 margin, radius 8, 1 px card stroke.
                    let k = icon_rect.w() / 36.0;
                    let pr = icon_rect.inset(2.0 * k, 2.0 * k);
                    cv.fill_round(pr, 8.0 * k, th.placeholder);
                    cv.stroke_round(pr.inset(0.5 / s, 0.5 / s), 8.0 * k, th.placeholder_stroke, 1.0 / s);
                }
            }
            if m.lines > 0 && editing != Some(i) && !is_expanded {
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
            if let Some(m) = restore {
                cv.set_transform(m);
            }
        }
        drop(icons);
        if let Some(i) = expanded {
            self.draw_name_expansion(cv, st, th, m, area, radius, s, i, now, label_fmt.as_ref(), shadow);
        }
        cv.unclip();
    }

    /// Windows-desktop-style selected-name expansion: a sole, focused selection
    /// whose label is trimmed grows downward to show the full name (upstream
    /// `SelectedNameExpansionPolicy`). Returns the expanded item, if any.
    fn name_expansion(st: &GridState, now: f64) -> Option<usize> {
        if !st.active || st.edit.is_some() || !matches!(st.drag, Drag::None | Drag::Pending { .. }) {
            return None;
        }
        let mut it = st.selected.iter().enumerate().filter(|(_, s)| **s);
        let (i, _) = it.next()?;
        if it.next().is_some() {
            return None;
        }
        match st.expand_ready {
            Some((j, t)) if j == i && now < t => None,
            _ => Some(i),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_name_expansion(
        &self,
        cv: &Canvas,
        st: &mut GridState,
        th: &theme::Theme,
        m: &TileMetrics,
        area: RectF,
        radius: f32,
        s: f32,
        i: usize,
        now: f64,
        fmt: Option<&windows::Win32::Graphics::DirectWrite::IDWriteTextFormat>,
        shadow: Option<Rgba>,
    ) {
        let gl = Self::grid_layout(st, m);
        let r = gl.tile_rect(m, i, st.scroll);
        let snap = |v: f32| (v * s).round() / s;
        let ly = snap(r.t + m.pad_y) + m.icon + m.spacing;
        let lx = r.l + (r.w() - m.label_w) / 2.0;
        let Some(fmt) = fmt else { return };
        let Some(full) = st.items.get(i).and_then(|it| cv.renderer().layout(&it.display, fmt, m.label_w, 4000.0))
        else {
            return;
        };
        let mut tm = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = full.GetMetrics(&mut tm);
        }
        let trimmed = tm.height > m.line_h * m.lines.max(1) as f32 + 0.5;
        let k = Self::fx(st, Fx::Expand(i), now);
        // Not trimmed: the normal selected look stays (drawn as usual next frame).
        let (fill, ring) =
            if st.active { (th.selected, th.selected_border) } else { (th.selected_unfocused, Rgba::argb(0)) };
        let bottom = if trimmed { (ly + tm.height + 3.0).min(area.b - 2.0).max(r.b) } else { r.b };
        let o = r.l;
        let box_r = RectF::new(o, r.t, r.r, bottom);
        let alpha = if trimmed { motion::lerp(0.55, 1.0, k) } else { 1.0 };
        cv.fill_round(box_r, radius, fill.scale_alpha(alpha));
        cv.stroke_round(box_r.inset(0.5 / s, 0.5 / s), radius, ring.scale_alpha(alpha), 1.0 / s);
        if trimmed {
            let max_h = (bottom - ly).max(m.line_h);
            if let Some(l) = cv.renderer().layout(&st.items[i].display, fmt, m.label_w, max_h) {
                cv.text_layout_shadowed(&l, lx, ly, th.text.scale_alpha(alpha), shadow);
            }
        } else if let Some(Some(l)) = st.labels.get(i) {
            cv.text_layout_shadowed(l, lx, ly, th.text, shadow);
        }
    }

    fn draw_scrollbar(&self, cv: &Canvas, st: &GridState, th: &theme::Theme, m: &TileMetrics, area: RectF) {
        let content_h = Self::total_height(st, m);
        if content_h > area.h() + 0.5 {
            let (w, _) = Self::dims(st);
            let track = RectF::new(w - 11.5, area.t + 2.0, w - 11.5 + theme::SCROLLBAR_W, area.b - 2.0);
            let thumb_h = (track.h() * area.h() / content_h).max(24.0).min(track.h());
            let max = (content_h - area.h()).max(1.0);
            let t = track.t + (track.h() - thumb_h) * (st.scroll / max).clamp(0.0, 1.0);
            let alpha = st.bar.value(motion::now_ms());
            if alpha <= 0.001 {
                return;
            }
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
        let title = crate::i18n::tr_n("grid.everything", st.hits_total as usize);
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
            let k = Self::fx(st, Fx::Hit(i), motion::now_ms());
            let fill =
                if st.hit_sel == Some(i) { th.selected.mix(th.selected_hover, k) } else { th.hover.scale_alpha(k) };
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
        let Some((index, title)) = st.edit.as_ref().map(|e| (e.index, e.title)) else { return };
        let renderer = cv.renderer();
        let (bl, bt, box_w, fmt, line_h) = if title {
            // `area` is the title text slot; the box hugs it like upstream's title editor.
            let lh = (theme::TITLE_TEXT * 1.4).ceil();
            let Some(fmt) =
                renderer.text_format(theme::TITLE_TEXT, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading, Wrap::Multi, false)
            else {
                return;
            };
            (area.l - 4.0, ((area.h() - lh - 6.0) / 2.0).round() - 1.0, area.w() + 4.0, fmt, lh)
        } else {
            let gl = Self::grid_layout(st, m);
            let r = gl.tile_rect(m, index, st.scroll);
            let box_w = (m.label_w * 1.7).max(m.tile_w + 12.0).min(area.w());
            let cx = r.l + r.w() / 2.0;
            let bl = (cx - box_w / 2.0).clamp(area.l, area.r - box_w);
            let bt = r.t + m.pad_y + m.icon + m.spacing - 3.0;
            let Some(fmt) = renderer.text_format(m.text, DWRITE_FONT_WEIGHT_NORMAL, Align::Center, Wrap::Multi, false)
            else {
                return;
            };
            (bl, bt, box_w, fmt, m.line_h)
        };
        let Some(edit) = st.edit.as_mut() else { return };
        let text = edit.input.text();
        let Some(layout) = renderer.layout(if text.is_empty() { " " } else { &text }, &fmt, box_w - 8.0, line_h * 6.0)
        else {
            return;
        };
        let mut tm = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut tm);
        }
        let bh = tm.height.max(line_h) + 6.0;
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
        cv.line(caret_x, ty + y, caret_x, ty + y + hm.height.max(line_h - 2.0), fg, 1.0 / s);
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
        let m = self.metrics();
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
        // Slow click on the label of an item that already was the settled sole
        // selection starts rename after one double-click interval (Explorer/upstream).
        if let Some(i) = tile
            && !ctrl
            && !shift
        {
            let now = motion::now_ms();
            let dbl = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() } as f64;
            let armed = {
                let st = self.st.borrow();
                let sole =
                    st.selected.iter().filter(|s| **s).count() == 1 && st.selected.get(i).copied().unwrap_or(false);
                let settled = matches!(st.sole_since, Some((j, t)) if j == i && now - t >= dbl);
                let m = self.metrics();
                let r = Self::grid_layout(&st, &m).tile_rect(&m, i, st.scroll);
                let on_label = dy >= r.t + m.pad_y + m.icon + m.spacing - 2.0;
                st.active && sole && settled && on_label
            };
            if armed {
                unsafe { SetTimer(Some(hwnd), TIMER_SLOW_RENAME, dbl as u32, None) };
            }
        }
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
        let capsule_hover_change = {
            let mut st = self.st.borrow_mut();
            let before = Self::capsule_look(&st) && st.last_pt_dip().0 < theme::CAPSULE_PAD_L + 38.0;
            st.last_pt = (x, y);
            let after = Self::capsule_look(&st) && st.last_pt_dip().0 < theme::CAPSULE_PAD_L + 38.0;
            before != after
        };
        if capsule_hover_change {
            render::invalidate(hwnd);
        }
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
            // Back over a capsule that is still folding away: the intent is
            // unambiguous, so turn around now instead of waiting for hover.
            let folding = matches!(self.st.borrow().fold, Fold::Animating { collapsing: true, .. });
            if self.smart() && folding {
                self.set_collapsed(false, true);
            }
            let want_hover = self.smart() && self.heading_collapsed();
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
                    if self.st.borrow().cfg.lock_position || self.kind == Kind::Drawer {
                        return;
                    }
                    let mut cursor = POINT::default();
                    unsafe {
                        let _ = GetCursorPos(&mut cursor);
                    }
                    self.st.borrow_mut().drag = Drag::Move { cursor, rect: win::window_rect(hwnd), snap: [None, None] };
                } else if index.is_some() {
                    self.st.borrow_mut().drag = Drag::None;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    self.begin_ole_drag();
                }
            }
            Drag::Move { cursor, rect, snap } => {
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
                let (snapped, state, edges) = app().snap_move(&self.id, moved, snap);
                {
                    let mut st = self.st.borrow_mut();
                    st.drag = Drag::Move { cursor, rect, snap: state };
                }
                self.set_glow(edges);
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
                let m = self.metrics();
                {
                    let mut st = self.st.borrow_mut();
                    let (dx, dy) = Self::to_dip(&st, x, y);
                    let sy = dy + st.scroll;
                    st.drag = Drag::Marquee { x0, y0, x1: dx, y1: sy };
                    let area = Self::content_area(&st);
                    if dy < area.t + AUTOSCROLL_ZONE || dy > area.b - AUTOSCROLL_ZONE {
                        unsafe { SetTimer(Some(hwnd), TIMER_AUTOSCROLL, 32, None) };
                    }
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
                {
                    // After an open the hover stays off until the pointer really moves.
                    let mut st = self.st.borrow_mut();
                    match st.suppress_hover {
                        Some(p) if p == (x, y) => return,
                        Some(_) => st.suppress_hover = None,
                        None => {}
                    }
                }
                let (dx, dy) = Self::to_dip(&self.st.borrow(), x, y);
                if !self.is_collapsed() && Self::content_area(&self.st.borrow()).contains(dx, dy) {
                    let shown = self.st.borrow().bar.target() > 0.5;
                    if !shown {
                        Self::bar_show(&mut self.st.borrow_mut(), hwnd);
                    }
                }
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
                    self.update_tooltip(tile);
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
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_AUTOSCROLL);
        }
        match drag {
            Drag::Pending { index: Some(i), .. } => {
                if !key_down(VK_CONTROL) && !key_down(VK_SHIFT) {
                    let mut st = self.st.borrow_mut();
                    st.selected.iter_mut().for_each(|s| *s = false);
                    if let Some(s) = st.selected.get_mut(i) {
                        *s = true;
                    }
                    st.lead = Some(i);
                }
                // A fresh pointer selection expands its name only after a
                // double-click interval, so a double-click never flashes it.
                let dbl = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() } as f64;
                let now = motion::now_ms();
                let mut st = self.st.borrow_mut();
                if !matches!(st.expand_ready, Some((j, _)) if j == i) {
                    st.expand_ready = Some((i, now + dbl));
                    unsafe { SetTimer(Some(hwnd), TIMER_EXPAND, dbl as u32 + 10, None) };
                }
                Self::note_selection(&mut st, now);
            }
            Drag::Pending { title: true, button: Some(b), .. } if self.hit_button(dx, dy) == Some(b) => match b {
                Button::Add => app().new_grid(),
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
                    let collapse = !self.heading_collapsed();
                    self.set_collapsed(collapse, true);
                }
            },
            Drag::Pending { title: true, .. } => {
                // A click on a folded capsule opens it (any mode), also mid-collapse.
                if self.heading_collapsed() {
                    self.set_collapsed(false, true);
                }
            }
            Drag::Move { .. } => {
                self.set_glow([false; 4]);
                self.persist_rect();
                self.rebed();
            }
            Drag::Marquee { .. } => Self::note_selection(&mut self.st.borrow_mut(), motion::now_ms()),
            _ => {}
        }
        render::invalidate(hwnd);
    }

    /// Records when the selection became a single item (slow-click rename gate).
    fn note_selection(st: &mut GridState, now: f64) {
        let mut it = st.selected.iter().enumerate().filter(|(_, s)| **s).map(|(i, _)| i);
        st.sole_since = match (it.next(), it.next()) {
            (Some(i), None) => match st.sole_since {
                Some((j, t)) if j == i => Some((j, t)),
                _ => Some((i, now)),
            },
            _ => None,
        };
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
        unsafe {
            let _ = KillTimer(Some(self.hwnd()), TIMER_SLOW_RENAME);
        }
        if self.in_title(dy) {
            if self.hit_button(dx, dy).is_some() {
                return;
            }
            // Upstream: double-clicking the title identity renames the grid.
            let on_title = {
                let st = self.st.borrow();
                !Self::capsule_look(&st) && self.kind == Kind::Desktop && dx < Self::title_text_right(&st)
            };
            if on_title {
                self.begin_title_rename();
            } else if self.st.borrow().cfg.collapse != CollapseMode::Expanded {
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
            self.after_open(&[i], (x, y));
        } else if let Some(i) = self.hit_row(dx, dy) {
            self.open_hit(i);
        }
    }

    /// Upstream `ClearOpenedItemSelectionAfterDispatch`: the opened items are
    /// deselected, their hover cleared until the pointer moves, and the desktop
    /// bedding re-checked once the launched window has settled.
    fn after_open(&self, opened: &[usize], pt: (i32, i32)) {
        {
            let mut st = self.st.borrow_mut();
            for &i in opened {
                if let Some(s) = st.selected.get_mut(i) {
                    *s = false;
                }
            }
            st.hover = None;
            st.suppress_hover = Some(pt);
            st.sole_since = None;
            st.expand_ready = None;
        }
        unsafe { SetTimer(Some(self.hwnd()), TIMER_REBED, 600, None) };
        render::invalidate(self.hwnd());
    }

    /// Right edge of the title text (hit area for title rename).
    fn title_text_right(st: &GridState) -> f32 {
        let list = Self::visible_buttons(st);
        list.first().map_or(Self::dims(st).0 - theme::TITLE_PAD_R, |b| Self::button_rect(st, *b).l - 4.0)
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
        use crate::i18n::tr;
        if self.kind == Kind::Drawer {
            return vec![
                Extra::item(CMD_OPEN_FOLDER, &tr("menu.open_in_explorer")),
                Extra::item(CMD_REFRESH, &format!("{}\tF5", tr("menu.refresh"))),
            ];
        }
        let (lock_pos, lock_size, mode) = {
            let st = self.st.borrow();
            (st.cfg.lock_position, st.cfg.lock_size, st.cfg.collapse)
        };
        // Upstream `CreateMoreFlyout` order: rename, expand behaviour, locks, delete.
        vec![
            Extra::item(CMD_RENAME_TITLE, &tr("menu.rename")),
            Extra::Separator,
            Extra::check(CMD_MODE_EXPANDED, &tr("collapse.expanded"), mode == CollapseMode::Expanded),
            Extra::check(CMD_MODE_CLICK, &tr("collapse.click"), mode == CollapseMode::Click),
            Extra::check(CMD_MODE_SMART, &tr("collapse.smart"), mode == CollapseMode::Smart),
            Extra::Separator,
            Extra::check(CMD_LOCK, &tr("menu.lock_position"), lock_pos),
            Extra::check(CMD_LOCK_SIZE, &tr("menu.lock_size"), lock_size),
            Extra::Separator,
            Extra::item(CMD_OPEN_FOLDER, &tr("menu.open_in_explorer")),
            Extra::item(CMD_REFRESH, &format!("{}\tF5", tr("menu.refresh"))),
            Extra::Separator,
            Extra::item(CMD_REMOVE, &tr("menu.delete_grid")),
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
                let v = !self.st.borrow().cfg.lock_position;
                self.st.borrow_mut().cfg.lock_position = v;
                let mut store = app().store.borrow_mut();
                store.remove_grid_key(&self.id, "locked");
                store.set_grid_flag(&self.id, "lock_position", v);
            }
            CMD_LOCK_SIZE => {
                let v = !self.st.borrow().cfg.lock_size;
                self.st.borrow_mut().cfg.lock_size = v;
                let mut store = app().store.borrow_mut();
                store.remove_grid_key(&self.id, "locked");
                store.set_grid_flag(&self.id, "lock_size", v);
            }
            CMD_RENAME_TITLE => self.begin_title_rename(),
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
                title: false,
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

    fn begin_title_rename(&self) {
        let hwnd = self.hwnd();
        if self.kind != Kind::Desktop || self.is_collapsed() {
            return;
        }
        {
            let mut st = self.st.borrow_mut();
            let mut input = TextInput::new(&Self::title_of(&st));
            input.select_all();
            st.edit =
                Some(Edit { index: usize::MAX, title: true, input, hidden_ext: None, layout: None, surrogate: None });
        }
        unsafe {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
        }
        render::invalidate(hwnd);
    }

    fn commit_title(&self, text: &str) {
        let folder = {
            let st = self.st.borrow();
            st.dir.rsplit('\\').find(|s| !s.is_empty()).unwrap_or(&st.dir).to_owned()
        };
        // An empty title (or the folder's own name) means "follow the folder name".
        let title = if text.is_empty() || text == folder { String::new() } else { text.to_owned() };
        self.st.borrow_mut().cfg.title = title.clone();
        app().store.borrow_mut().set_grid_str(&self.id, "title", &title);
        render::invalidate(self.hwnd());
    }

    fn ensure_visible(&self, index: usize) {
        let m = self.metrics();
        let mut st = self.st.borrow_mut();
        let area = Self::content_area(&st);
        let gl = Self::grid_layout(&st, &m);
        let r = gl.tile_rect(&m, index, 0.0);
        let top = r.t - area.t;
        let bottom = r.b - area.t + m.line_h * 2.0;
        st.scroll_tw = None;
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
        if edit.title {
            self.commit_title(edit.input.text().trim());
            return;
        }
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
                let opened: Vec<usize> = {
                    let st = self.st.borrow();
                    st.selected.iter().enumerate().filter(|(_, s)| **s).map(|(i, _)| i).collect()
                };
                let pt = self.st.borrow().last_pt;
                self.after_open(&opened, pt);
            }
            v if v == VK_A.0 && ctrl => {
                let mut st = self.st.borrow_mut();
                st.selected.iter_mut().for_each(|s| *s = true);
                Self::note_selection(&mut st, motion::now_ms());
                drop(st);
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
            v if v == VK_APPS.0 || (v == VK_F10.0 && key_down(VK_SHIFT)) => self.keyboard_menu(),
            v if [VK_LEFT.0, VK_RIGHT.0, VK_UP.0, VK_DOWN.0, VK_HOME.0, VK_END.0, VK_PRIOR.0, VK_NEXT.0]
                .contains(&v) =>
            {
                self.navigate(v);
            }
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

    /// Arrow / Home / End / Page keys move the focus item; Shift extends from
    /// the anchor, Ctrl moves without selecting (ListView semantics).
    fn navigate(&self, vk: u16) {
        let m = self.metrics();
        let hwnd = self.hwnd();
        let next = {
            let st = self.st.borrow();
            let n = st.items.len();
            if n == 0 || self.is_collapsed() {
                return;
            }
            let gl = Self::grid_layout(&st, &m);
            let cols = gl.cols.max(1);
            let page = ((Self::content_area(&st).h() / m.cell_h()).floor() as usize).max(1) * cols;
            let cur = st.lead.filter(|&i| i < n).or_else(|| st.selected.iter().position(|s| *s));
            match cur {
                None => 0,
                Some(c) => match vk {
                    v if v == VK_LEFT.0 => c.saturating_sub(1),
                    v if v == VK_RIGHT.0 => (c + 1).min(n - 1),
                    v if v == VK_UP.0 => c.checked_sub(cols).unwrap_or(c),
                    v if v == VK_DOWN.0 => {
                        if c + cols < n {
                            c + cols
                        } else {
                            c
                        }
                    }
                    v if v == VK_HOME.0 => 0,
                    v if v == VK_END.0 => n - 1,
                    v if v == VK_PRIOR.0 => c.saturating_sub(page),
                    _ => (c + page).min(n - 1),
                },
            }
        };
        {
            let mut st = self.st.borrow_mut();
            let shift = key_down(VK_SHIFT);
            let ctrl = key_down(VK_CONTROL);
            if shift {
                let a = st.anchor.unwrap_or(next);
                st.selected.iter_mut().for_each(|s| *s = false);
                st.selected[a.min(next)..=a.max(next)].iter_mut().for_each(|s| *s = true);
                st.anchor = Some(a);
            } else if !ctrl {
                st.selected.iter_mut().for_each(|s| *s = false);
                st.selected[next] = true;
                st.anchor = Some(next);
            }
            st.lead = Some(next);
            st.expand_ready = None;
            Self::note_selection(&mut st, motion::now_ms());
        }
        self.ensure_visible(next);
        render::invalidate(hwnd);
    }

    /// Context menu from the keyboard (Apps / Shift+F10), anchored on the focus item.
    fn keyboard_menu(&self) {
        let m = self.metrics();
        let (names, anchor) = {
            let st = self.st.borrow();
            let names = Self::selected_names(&st);
            let lead = st
                .lead
                .filter(|&i| st.selected.get(i).copied().unwrap_or(false))
                .or_else(|| st.selected.iter().position(|s| *s));
            let s = Self::scale(&st);
            let pt = match lead {
                Some(i) => {
                    let r = Self::grid_layout(&st, &m).tile_rect(&m, i, st.scroll);
                    POINT { x: ((r.l + r.w() / 2.0) * s) as i32, y: ((r.t + r.h() / 2.0) * s) as i32 }
                }
                None => POINT { x: (24.0 * s) as i32, y: (Self::title_h() * s) as i32 },
            };
            (names, pt)
        };
        let mut p = anchor;
        unsafe {
            let _ = ClientToScreen(self.hwnd(), &mut p);
        }
        if names.is_empty() { self.show_menu(p, None, true) } else { self.show_menu(p, Some(names), false) }
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
        if key_down(VK_CONTROL) && self.kind == Kind::Desktop {
            self.zoom_icons(delta);
            return;
        }
        let m = self.metrics();
        let hwnd = self.hwnd();
        {
            let mut st = self.st.borrow_mut();
            if !matches!(st.fold, Fold::Expanded) {
                return;
            }
            if st.edit.as_ref().is_some_and(|e| !e.title) {
                st.edit = None;
            }
            Self::bar_show(&mut st, hwnd);
            let max = Self::max_scroll(&st, &m);
            let step = m.cell_h() * 0.75;
            let now = motion::now_ms();
            // Glide toward the accumulated target; successive notches retarget smoothly.
            let mut tw = st.scroll_tw.unwrap_or(Tween::at(st.scroll));
            let target = (tw.target() - delta as f32 / 120.0 * step).clamp(0.0, max);
            tw.set(target, SCROLL_MS, now);
            st.scroll_tw = Some(tw);
            st.wants_frame = true;
        }
        render::invalidate(self.hwnd());
    }

    /// Marquee near the top/bottom edge scrolls, faster the closer it gets.
    fn autoscroll_tick(&self) {
        let hwnd = self.hwnd();
        let m = self.metrics();
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
            let _ = ScreenToClient(hwnd, &mut p);
        }
        let keep = {
            let mut st = self.st.borrow_mut();
            let Drag::Marquee { x0, y0, .. } = st.drag else { return self.stop_autoscroll() };
            let (dx, dy) = Self::to_dip(&st, p.x, p.y);
            let area = Self::content_area(&st);
            let v = if dy < area.t + AUTOSCROLL_ZONE {
                -(area.t + AUTOSCROLL_ZONE - dy).min(60.0)
            } else if dy > area.b - AUTOSCROLL_ZONE {
                (dy - (area.b - AUTOSCROLL_ZONE)).min(60.0)
            } else {
                0.0
            };
            let max = Self::max_scroll(&st, &m);
            let before = st.scroll;
            st.scroll_tw = None;
            st.scroll = (st.scroll + v * 0.35).clamp(0.0, max);
            let sy = dy.clamp(area.t, area.b) + st.scroll;
            st.drag = Drag::Marquee { x0, y0, x1: dx, y1: sy };
            let sel = RectF::new(x0.min(dx), y0.min(sy), x0.max(dx), y0.max(sy));
            let gl = Self::grid_layout(&st, &m);
            let ctrl = key_down(VK_CONTROL);
            for i in 0..st.items.len() {
                if gl.tile_rect(&m, i, 0.0).intersects(&sel) {
                    st.selected[i] = true;
                } else if !ctrl {
                    st.selected[i] = false;
                }
            }
            v != 0.0 && (st.scroll - before).abs() > 0.01
        };
        render::invalidate(hwnd);
        if !keep {
            self.stop_autoscroll();
        }
    }

    fn stop_autoscroll(&self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd()), TIMER_AUTOSCROLL);
        }
    }

    /// Path tooltip for the hovered tile (upstream `ShowFileItemPathTooltips`).
    /// A *tracked* tooltip shown by our own timer: a subclassing tooltip calls
    /// `TrackMouseEvent` on the grid and cancels the capsule's hover detection.
    fn update_tooltip(&self, tile: Option<usize>) {
        let hwnd = self.hwnd();
        {
            let mut st = self.st.borrow_mut();
            if st.tip_for == tile {
                return;
            }
            st.tip_for = tile;
        }
        self.hide_tooltip();
        if tile.is_some() {
            let ms = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
            unsafe { SetTimer(Some(hwnd), TIMER_TIP, ms, None) };
        }
    }

    fn hide_tooltip(&self) {
        use windows::Win32::UI::Controls::{TTM_TRACKACTIVATE, TTTOOLINFOW};
        let hwnd = self.hwnd();
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_TIP);
        }
        let tip = self.st.borrow().tooltip;
        if !tip.is_invalid() {
            let mut ti =
                TTTOOLINFOW { cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32, hwnd, uId: 1, ..Default::default() };
            unsafe {
                SendMessageW(tip, TTM_TRACKACTIVATE, Some(WPARAM(0)), Some(LPARAM(&mut ti as *mut _ as isize)));
            }
        }
    }

    fn show_tooltip(&self) {
        use windows::Win32::UI::Controls::{
            TOOLTIPS_CLASSW, TTF_ABSOLUTE, TTF_TRACK, TTM_ADDTOOLW, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE,
            TTM_TRACKPOSITION, TTM_UPDATE, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
        };
        let hwnd = self.hwnd();
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_TIP);
        }
        let ok = {
            let st = self.st.borrow();
            st.inside
                && st.tip_for.is_some()
                && matches!(st.drag, Drag::None)
                && st.edit.is_none()
                && !Self::capsule_look(&st)
        };
        if !ok {
            return;
        }
        let s = Self::scale(&self.st.borrow());
        if self.st.borrow().tooltip.is_invalid() {
            let tip = unsafe {
                CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT,
                    TOOLTIPS_CLASSW,
                    None,
                    WINDOW_STYLE(WS_POPUP.0 | TTS_ALWAYSTIP | TTS_NOPREFIX),
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    Some(hwnd),
                    None,
                    None,
                    None,
                )
                .unwrap_or_default()
            };
            if tip.is_invalid() {
                return;
            }
            let mut ti = TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_TRACK | TTF_ABSOLUTE,
                hwnd,
                uId: 1,
                lpszText: windows::core::PWSTR(-1isize as *mut u16), // LPSTR_TEXTCALLBACK
                ..Default::default()
            };
            unsafe {
                SendMessageW(tip, TTM_ADDTOOLW, None, Some(LPARAM(&mut ti as *mut _ as isize)));
                SendMessageW(tip, TTM_SETMAXTIPWIDTH, None, Some(LPARAM((360.0 * s) as isize)));
            }
            self.st.borrow_mut().tooltip = tip;
        }
        let tip = self.st.borrow().tooltip;
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        // Below the pointer like a native tooltip (never under it: that would
        // generate WM_MOUSELEAVE for the grid).
        let y = p.y + (22.0 * s) as i32;
        let mut ti =
            TTTOOLINFOW { cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32, hwnd, uId: 1, ..Default::default() };
        unsafe {
            SendMessageW(tip, TTM_TRACKPOSITION, None, Some(LPARAM(((y as u16 as isize) << 16) | p.x as u16 as isize)));
            SendMessageW(tip, TTM_UPDATE, None, None);
            SendMessageW(tip, TTM_TRACKACTIVATE, Some(WPARAM(1)), Some(LPARAM(&mut ti as *mut _ as isize)));
            SetTimer(Some(hwnd), TIMER_TIP_HIDE, 8000, None);
        }
    }

    fn on_notify(&self, lp: LPARAM) -> Option<LRESULT> {
        use windows::Win32::UI::Controls::{NMHDR, NMTTDISPINFOW, TTN_GETDISPINFOW};
        let hdr = unsafe { &*(lp.0 as *const NMHDR) };
        if hdr.code != TTN_GETDISPINFOW {
            return None;
        }
        let path = {
            let st = self.st.borrow();
            if hdr.hwndFrom != st.tooltip {
                return None;
            }
            let path = st.tip_for.and_then(|i| st.items.get(i)).map(|it| shell::join(&st.dir, &it.name));
            match path {
                Some(p) if matches!(st.drag, Drag::None) && st.edit.is_none() => p,
                _ => String::new(),
            }
        };
        thread_local! {
            static TIP: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
        }
        let di = unsafe { &mut *(lp.0 as *mut NMTTDISPINFOW) };
        TIP.with(|t| {
            *t.borrow_mut() = crate::util::wide(&path);
            di.lpszText = windows::core::PWSTR(t.borrow_mut().as_mut_ptr());
        });
        Some(LRESULT(0))
    }

    /// Ctrl+wheel steps this grid's icon size through upstream's
    /// `FileWidgetIconSizePolicy` (24…56) and confirms it with a toast.
    fn zoom_icons(&self, delta: i32) {
        const STEPS: [f32; 7] = [24.0, 28.0, 32.0, 36.0, 40.0, 48.0, 56.0];
        if delta == 0 {
            return;
        }
        let cur = self.metrics().icon;
        let next = if delta > 0 {
            STEPS.iter().copied().find(|s| *s > cur + 0.01).unwrap_or(STEPS[6])
        } else {
            STEPS.iter().rev().copied().find(|s| *s < cur - 0.01).unwrap_or(STEPS[0])
        };
        if (next - cur).abs() < 0.01 {
            return;
        }
        self.st.borrow_mut().cfg.icon_size = Some(next);
        app().store.borrow_mut().set_grid_value(&self.id, "icon_size", toml_edit::value(next as i64));
        self.sync_metrics();
        self.drop_text_cache();
        self.clamp_scroll();
        self.toast(crate::i18n::trf("toast.icon_size", &[&format!("{next:.0}")]));
    }

    fn hit_test(&self, x: i32, y: i32) -> u32 {
        let st = self.st.borrow();
        if st.cfg.lock_size || !matches!(st.fold, Fold::Expanded) || self.kind == Kind::Drawer {
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
                    TIMER_COLLAPSE => self.on_collapse_timer(),
                    TIMER_HOVER_HIDE => self.on_hover_hide_tick(),
                    TIMER_QUERY => self.run_query(),
                    TIMER_BAR => {
                        unsafe {
                            let _ = KillTimer(Some(hwnd), TIMER_BAR);
                        }
                        let inside_drag = matches!(self.st.borrow().drag, Drag::Marquee { .. });
                        if inside_drag {
                            Self::bar_show(&mut self.st.borrow_mut(), hwnd);
                        } else {
                            let mut st = self.st.borrow_mut();
                            st.bar.set(0.0, 300.0, motion::now_ms());
                            st.wants_frame = true;
                        }
                    }
                    TIMER_AUTOSCROLL => self.autoscroll_tick(),
                    TIMER_TIP => self.show_tooltip(),
                    TIMER_TIP_HIDE => {
                        unsafe {
                            let _ = KillTimer(Some(hwnd), TIMER_TIP_HIDE);
                        }
                        self.hide_tooltip();
                    }
                    TIMER_EXPAND => {
                        unsafe {
                            let _ = KillTimer(Some(hwnd), TIMER_EXPAND);
                        }
                        render::invalidate(hwnd);
                    }
                    TIMER_SLOW_RENAME => {
                        unsafe {
                            let _ = KillTimer(Some(hwnd), TIMER_SLOW_RENAME);
                        }
                        let sole = {
                            let st = self.st.borrow();
                            let mut it = st.selected.iter().enumerate().filter(|(_, s)| **s);
                            match (it.next(), it.next()) {
                                (Some((i, _)), None) if matches!(st.drag, Drag::None) => Some(i),
                                _ => None,
                            }
                        };
                        if let Some(i) = sole {
                            self.begin_rename(i);
                        }
                    }
                    TIMER_REBED => {
                        unsafe {
                            let _ = KillTimer(Some(hwnd), TIMER_REBED);
                        }
                        self.rebed();
                    }
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
                self.hide_tooltip();
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
                if self.smart() && self.heading_collapsed() {
                    self.set_collapsed(false, true);
                }
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                self.hide_tooltip();
                if let Ok(mut st) = self.st.try_borrow_mut() {
                    st.tip_for = None;
                    st.inside = false;
                    st.hover = None;
                    st.hover_button = None;
                    st.last_pt = (-1000, -1000);
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
                self.hide_tooltip();
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
            WM_NOTIFY => self.on_notify(lp),
            WM_SIZING => {
                let r = unsafe { &mut *(lp.0 as *mut RECT) };
                let moving = match wp.0 as u32 {
                    WMSZ_LEFT => [true, false, false, false],
                    WMSZ_RIGHT => [false, false, true, false],
                    WMSZ_TOP => [false, true, false, false],
                    WMSZ_BOTTOM => [false, false, false, true],
                    WMSZ_TOPLEFT => [true, true, false, false],
                    WMSZ_TOPRIGHT => [false, true, true, false],
                    WMSZ_BOTTOMLEFT => [true, false, false, true],
                    _ => [false, false, true, true],
                };
                let s = win::scale(self.st.borrow().dpi);
                let min = ((theme::MIN_W * s) as i32, (theme::MIN_H * s) as i32);
                let (snapped, glow) = app().snap_size(&self.id, *r, moving, min);
                *r = snapped;
                self.set_glow(glow);
                Some(LRESULT(1))
            }
            WM_EXITSIZEMOVE => {
                self.set_glow([false; 4]);
                self.persist_rect();
                self.rebed();
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                unsafe {
                    let _ = RevokeDragDrop(hwnd);
                    let _ = KillTimer(Some(hwnd), TIMER_COLLAPSE);
                    let _ = KillTimer(Some(hwnd), TIMER_HOVER_HIDE);
                    let _ = KillTimer(Some(hwnd), TIMER_QUERY);
                    for t in [
                        TIMER_BAR,
                        TIMER_AUTOSCROLL,
                        TIMER_EXPAND,
                        TIMER_SLOW_RENAME,
                        TIMER_REBED,
                        TIMER_TIP,
                        TIMER_TIP_HIDE,
                    ] {
                        let _ = KillTimer(Some(hwnd), t);
                    }
                    let tip = std::mem::take(&mut self.st.borrow_mut().tooltip);
                    if !tip.is_invalid() {
                        let _ = DestroyWindow(tip);
                    }
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
