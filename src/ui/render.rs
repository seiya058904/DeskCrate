//! One shared Direct2D renderer for every window.
//!
//! A software `ID2D1DCRenderTarget` draws into a single 32-bpp premultiplied
//! DIB that grows on demand, then `BitBlt`s into the window. The DWM material
//! shows through transparent pixels. No GPU device or driver is loaded, and the
//! scratch DIB can be released when idle.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory,
    ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_LINE_SPACING_METHOD_UNIFORM,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS, DWRITE_TRIMMING,
    DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_EMERGENCY_BREAK, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWRITE_WORD_WRAPPING_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteInlineObject, IDWriteTextFormat,
    IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, HBITMAP, HDC, HGDIOBJ, SRCCOPY, SelectObject,
};
use windows::core::{HSTRING, Interface, Result, w};
use windows_numerics::Vector2;

use crate::layout::RectF;
use crate::theme::Rgba;

pub fn rect(r: RectF) -> D2D_RECT_F {
    D2D_RECT_F { left: r.l, top: r.t, right: r.r, bottom: r.b }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    Leading,
    Center,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wrap {
    /// Single line, ellipsis.
    Line,
    /// Wraps (breaking long words), ellipsis on the last line.
    Multi,
}

struct Scratch {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    w: i32,
    h: i32,
}

type FormatKey = (u32, i32, Align, Wrap, bool);

struct Inner {
    dw: IDWriteFactory,
    _d2d: ID2D1Factory,
    rt: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
}

/// Everything Direct2D/DirectWrite is created lazily and torn down when idle:
/// the D2D factory, render target and text layouts keep glyph/shaping caches
/// (measured ~12 MB with CJK labels) that `ClearResources` does not return.
pub struct Renderer {
    inner: RefCell<Option<Inner>>,
    generation: std::cell::Cell<u64>,
    scratch: RefCell<Option<Scratch>>,
    formats: RefCell<HashMap<FormatKey, IDWriteTextFormat>>,
    pub ui_font: HSTRING,
    pub icon_font: HSTRING,
}

fn dwrite_factory() -> Result<IDWriteFactory> {
    unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
}

impl Renderer {
    pub fn new() -> Result<Self> {
        let dw = dwrite_factory()?;
        let ui_font = pick_font(&dw, &["Segoe UI Variable Text", "Segoe UI"]);
        let icon_font = pick_font(&dw, &["Segoe Fluent Icons", "Segoe MDL2 Assets"]);
        Ok(Renderer {
            inner: RefCell::new(None),
            generation: std::cell::Cell::new(1),
            scratch: RefCell::new(None),
            formats: RefCell::new(HashMap::new()),
            ui_font,
            icon_font,
        })
    }

    fn create_inner() -> Option<Inner> {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let dw = dwrite_factory().ok()?;
            let rt = d2d.CreateDCRenderTarget(&props).ok()?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = rt.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None).ok()?;
            Some(Inner { dw, _d2d: d2d, rt, brush })
        }
    }

    fn parts(&self) -> Option<(IDWriteFactory, ID2D1DCRenderTarget, ID2D1SolidColorBrush)> {
        let mut inner = self.inner.borrow_mut();
        if inner.is_none() {
            *inner = Self::create_inner();
        }
        inner.as_ref().map(|i| (i.dw.clone(), i.rt.clone(), i.brush.clone()))
    }

    fn dw(&self) -> Option<IDWriteFactory> {
        self.parts().map(|p| p.0)
    }

    /// Bumped whenever the render target is recreated; device bitmaps from an
    /// older generation must be rebuilt.
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    fn ensure_scratch(&self, w: i32, h: i32) -> Option<HDC> {
        let mut s = self.scratch.borrow_mut();
        if let Some(sc) = s.as_ref()
            && sc.w >= w
            && sc.h >= h
        {
            return Some(sc.dc);
        }
        let (nw, nh) = match s.as_ref() {
            Some(sc) => (sc.w.max(w), sc.h.max(h)),
            None => (w, h),
        };
        if let Some(old) = s.take() {
            free_scratch(old);
        }
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: nw,
                    biHeight: -nh,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let old = SelectObject(dc, HGDIOBJ(bmp.0));
            *s = Some(Scratch { dc, bmp, old, w: nw, h: nh });
            Some(dc)
        }
    }

    /// Frees the scratch bitmap, text formats, render target and factories.
    /// Callers must also drop any cached `IDWriteTextLayout`s.
    pub fn release(&self) {
        if let Some(s) = self.scratch.borrow_mut().take() {
            free_scratch(s);
        }
        self.formats.borrow_mut().clear();
        if self.inner.borrow_mut().take().is_some() {
            self.generation.set(self.generation.get() + 1);
        }
    }

    /// Renders one frame of `w`×`h` pixels at `dpi` and copies it to `target`.
    pub fn paint(&self, target: HDC, w: i32, h: i32, dpi: u32, draw: impl FnOnce(&Canvas)) {
        if w <= 0 || h <= 0 {
            return;
        }
        let Some((_, rt, brush)) = self.parts() else { return };
        let Some(dc) = self.ensure_scratch(w, h) else { return };
        let bounds = RECT { left: 0, top: 0, right: w, bottom: h };
        unsafe {
            if rt.BindDC(dc, &bounds).is_err() {
                return;
            }
            rt.SetDpi(dpi as f32, dpi as f32);
            rt.BeginDraw();
            rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            let canvas = Canvas { r: self, rt: rt.clone(), brush, dpi };
            draw(&canvas);
            if rt.EndDraw(None, None).is_err() {
                crate::log!("render: EndDraw failed");
            }
            let _ = BitBlt(target, 0, 0, w, h, Some(dc), 0, 0, SRCCOPY);
        }
    }

    pub fn text_format(
        &self,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        align: Align,
        wrap: Wrap,
        icon: bool,
    ) -> Option<IDWriteTextFormat> {
        let key = ((size * 100.0) as u32, weight.0, align, wrap, icon);
        if let Some(f) = self.formats.borrow().get(&key) {
            return Some(f.clone());
        }
        let family = if icon { &self.icon_font } else { &self.ui_font };
        let dw = self.dw()?;
        let f = unsafe {
            let f = dw
                .CreateTextFormat(
                    family,
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!(""),
                )
                .ok()?;
            let _ = f.SetTextAlignment(match align {
                Align::Leading => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            } as DWRITE_TEXT_ALIGNMENT);
            match wrap {
                Wrap::Line => {
                    let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                    let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                }
                Wrap::Multi => {
                    if f.SetWordWrapping(DWRITE_WORD_WRAPPING_EMERGENCY_BREAK).is_err() {
                        let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
                    }
                    let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
                    let spacing = (size * 1.4).ceil();
                    let _ = f.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, spacing, spacing * 0.78);
                }
            }
            if !icon {
                let sign: Option<IDWriteInlineObject> = dw.CreateEllipsisTrimmingSign(&f).ok();
                let trimming = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = f.SetTrimming(&trimming, sign.as_ref());
            }
            f
        };
        self.formats.borrow_mut().insert(key, f.clone());
        Some(f)
    }

    pub fn layout(&self, text: &str, fmt: &IDWriteTextFormat, max_w: f32, max_h: f32) -> Option<IDWriteTextLayout> {
        let w: Vec<u16> = text.encode_utf16().collect();
        let dw = self.dw()?;
        unsafe { dw.CreateTextLayout(&w, fmt, max_w.max(1.0), max_h.max(1.0)).ok() }
    }

    pub fn clear_text_cache(&self) {
        self.formats.borrow_mut().clear();
    }
}

fn free_scratch(s: Scratch) {
    unsafe {
        SelectObject(s.dc, s.old);
        let _ = DeleteObject(HGDIOBJ(s.bmp.0));
        let _ = DeleteDC(s.dc);
    }
}

fn pick_font(dw: &IDWriteFactory, names: &[&str]) -> HSTRING {
    unsafe {
        let mut coll = None;
        if dw.GetSystemFontCollection(&mut coll, false).is_ok()
            && let Some(coll) = coll
        {
            for n in names {
                let mut idx = 0u32;
                let mut exists = windows::core::BOOL(0);
                if coll.FindFamilyName(&HSTRING::from(*n), &mut idx, &mut exists).is_ok() && exists.as_bool() {
                    return HSTRING::from(*n);
                }
            }
        }
    }
    HSTRING::from(names[names.len() - 1])
}

/// Drawing API handed to window painters (coordinates in DIP).
pub struct Canvas<'a> {
    r: &'a Renderer,
    rt: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    pub dpi: u32,
}

impl Canvas<'_> {
    fn brush(&self, c: Rgba) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&c.d2d()) };
        &self.brush
    }

    /// Target for device bitmaps, plus its generation.
    pub fn bitmap_target(&self) -> (ID2D1RenderTarget, u64) {
        (self.rt.cast().expect("render target"), self.r.generation())
    }

    pub fn renderer(&self) -> &Renderer {
        self.r
    }

    pub fn fill_round(&self, r: RectF, radius: f32, c: Rgba) {
        if c.a <= 0.0 {
            return;
        }
        let rr = D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius };
        unsafe { self.rt.FillRoundedRectangle(&rr, self.brush(c)) };
    }

    pub fn stroke_round(&self, r: RectF, radius: f32, c: Rgba, width: f32) {
        if c.a <= 0.0 {
            return;
        }
        let rr = D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius };
        unsafe { self.rt.DrawRoundedRectangle(&rr, self.brush(c), width, None::<&ID2D1StrokeStyle>) };
    }

    pub fn fill_rect(&self, r: RectF, c: Rgba) {
        if c.a <= 0.0 {
            return;
        }
        unsafe { self.rt.FillRectangle(&rect(r), self.brush(c)) };
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgba, width: f32) {
        unsafe { self.rt.DrawLine(pt(x0, y0), pt(x1, y1), self.brush(c), width, None::<&ID2D1StrokeStyle>) };
    }

    pub fn clip(&self, r: RectF) {
        unsafe { self.rt.PushAxisAlignedClip(&rect(r), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
    }

    pub fn unclip(&self) {
        unsafe { self.rt.PopAxisAlignedClip() };
    }

    pub fn bitmap(&self, b: &ID2D1Bitmap, r: RectF, opacity: f32) {
        unsafe { self.rt.DrawBitmap(b, Some(&rect(r)), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
    }

    pub fn text_layout(&self, l: &IDWriteTextLayout, x: f32, y: f32, c: Rgba) {
        unsafe { self.rt.DrawTextLayout(pt(x, y), l, self.brush(c), D2D1_DRAW_TEXT_OPTIONS_CLIP) };
    }

    /// Layout text with an optional soft shadow (readability on bright wallpapers).
    pub fn text_layout_shadowed(&self, l: &IDWriteTextLayout, x: f32, y: f32, c: Rgba, shadow: Option<Rgba>) {
        if let Some(s) = shadow {
            let d = 1.0 / crate::win::scale(self.dpi).max(1.0);
            unsafe {
                self.rt.DrawTextLayout(pt(x, y + d.max(0.75)), l, self.brush(s), D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
        }
        self.text_layout(l, x, y, c);
    }

    /// One-off text in a box (title, glyphs, messages).
    pub fn text(&self, s: &str, r: RectF, size: f32, semibold: bool, align: Align, c: Rgba) {
        self.text_ex(s, r, size, semibold, align, c, false);
    }

    pub fn glyph(&self, glyph: char, r: RectF, size: f32, c: Rgba) {
        self.text_ex(&glyph.to_string(), r, size, false, Align::Center, c, true);
    }

    #[allow(clippy::too_many_arguments)]
    fn text_ex(&self, s: &str, r: RectF, size: f32, semibold: bool, align: Align, c: Rgba, icon: bool) {
        let weight = if semibold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
        let Some(fmt) = self.r.text_format(size, weight, align, Wrap::Line, icon) else { return };
        if let Some(l) = self.r.layout(s, &fmt, r.w(), r.h()) {
            self.text_layout(&l, r.l, r.t, c);
        }
    }
}

pub fn text_width(l: &IDWriteTextLayout) -> f32 {
    let mut m = DWRITE_TEXT_METRICS::default();
    unsafe {
        let _ = l.GetMetrics(&mut m);
    }
    m.widthIncludingTrailingWhitespace
}

pub fn invalidate(hwnd: HWND) {
    // InvalidateRect(NULL) would repaint every window on the desktop.
    if hwnd.is_invalid() {
        return;
    }
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false);
    }
}
