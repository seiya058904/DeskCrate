//! Native shell icons with a bounded LRU cache.
//!
//! Extraction runs on one STA worker thread (slow shell extensions never block
//! the UI). The worker blocks on a channel, so it costs no CPU when idle.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::Foundation::{HWND, LPARAM, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{D2D1_BITMAP_PROPERTIES, ID2D1Bitmap, ID2D1RenderTarget};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HGDIOBJ,
    ReleaseDC,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, IBindCtx};
use windows::Win32::UI::Shell::{
    IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_ICONONLY, SIIGBF_RESIZETOFIT,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{HSTRING, Interface};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IconKey {
    /// Shared by every file with this (lower-case) extension.
    Ext(String),
    /// Per-file icon (exe, lnk, custom folders…).
    Path(String),
    /// Plain folder.
    Folder,
    /// Per-file thumbnail (images, videos, documents with a thumbnail handler).
    Thumb(String),
}

/// Media types upstream always previews (`IconHelper.IsMediaFile`).
const MEDIA: &[&str] = &[
    "jpg", "jpeg", "jfif", "png", "gif", "bmp", "webp", "heic", "heif", "avif", "tif", "tiff", "ico", "jxl", "dng",
    "mp4", "m4v", "mov", "mkv", "avi", "wmv", "webm", "mpg", "mpeg", "3gp",
];

thread_local! {
    static THUMB_EXT: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
}

/// `true` when files of `ext` get a shell thumbnail: media, or any type with a
/// registered `IThumbnailProvider` (PDF, PSD, Office…), like upstream does.
pub fn has_thumbnail(ext: &str) -> bool {
    if ext.is_empty() {
        return false;
    }
    if MEDIA.contains(&ext) {
        return true;
    }
    THUMB_EXT.with(|m| {
        if let Some(v) = m.borrow().get(ext) {
            return *v;
        }
        let v = thumbnail_handler_registered(ext);
        m.borrow_mut().insert(ext.to_owned(), v);
        v
    })
}

fn thumbnail_handler_registered(ext: &str) -> bool {
    use windows::Win32::System::Registry::{HKEY, HKEY_CLASSES_ROOT, KEY_READ, RegCloseKey, RegOpenKeyExW};
    const IID: &str = r"ShellEx\{E357FCCD-A995-4576-B01F-234630154E96}";
    let exists = |sub: String| unsafe {
        let mut k = HKEY::default();
        let ok = RegOpenKeyExW(HKEY_CLASSES_ROOT, &HSTRING::from(sub), Some(0), KEY_READ, &mut k).is_ok();
        if ok {
            let _ = RegCloseKey(k);
        }
        ok
    };
    exists(format!(r".{ext}\{IID}")) || exists(format!(r"SystemFileAssociations\.{ext}\{IID}"))
}

struct Request {
    key: IconKey,
    px: u32,
    path: String,
}

struct Reply {
    key: IconKey,
    px: u32,
    image: Option<(u32, u32, Vec<u8>)>,
}

enum Slot {
    Pending,
    Failed,
    /// Premultiplied pixels are the source of truth; the device bitmap is
    /// rebuilt whenever the (idle-released) render target is recreated.
    Ready {
        pixels: Vec<u8>,
        w: u32,
        h: u32,
        bmp: Option<(u64, ID2D1Bitmap)>,
        used: u64,
    },
}

pub struct IconCache {
    map: HashMap<(IconKey, u32), Slot>,
    bytes: usize,
    budget: usize,
    clock: u64,
    tx: Sender<Request>,
    rx: Receiver<Reply>,
}

impl IconCache {
    /// Starts the worker. `notify` receives `msg` whenever replies are ready.
    pub fn new(notify: HWND, msg: u32, budget_bytes: usize) -> Self {
        let (tx, worker_rx) = channel::<Request>();
        let (worker_tx, rx) = channel::<Reply>();
        let notify = notify.0 as isize;
        std::thread::Builder::new()
            .name("icons".into())
            .stack_size(256 * 1024)
            .spawn(move || worker(worker_rx, worker_tx, notify, msg))
            .expect("spawn icon worker");
        IconCache { map: HashMap::new(), bytes: 0, budget: budget_bytes, clock: 0, tx, rx }
    }

    /// Returns the bitmap if cached; otherwise queues extraction and returns `None`.
    pub fn get(
        &mut self,
        key: &IconKey,
        px: u32,
        sample_path: &str,
        rt: &ID2D1RenderTarget,
        generation: u64,
    ) -> Option<ID2D1Bitmap> {
        self.clock += 1;
        let k = (key.clone(), px);
        match self.map.get_mut(&k) {
            Some(Slot::Ready { pixels, w, h, bmp, used }) => {
                *used = self.clock;
                match bmp {
                    Some((g, b)) if *g == generation => Some(b.clone()),
                    _ => {
                        let b = create_bitmap(rt, *w, *h, pixels)?;
                        *bmp = Some((generation, b.clone()));
                        Some(b)
                    }
                }
            }
            Some(_) => None,
            None => {
                self.map.insert(k, Slot::Pending);
                let _ = self.tx.send(Request { key: key.clone(), px, path: sample_path.to_owned() });
                None
            }
        }
    }

    /// Stores finished extractions. Returns `true` if any arrived.
    pub fn drain(&mut self) -> bool {
        let mut any = false;
        while let Ok(reply) = self.rx.try_recv() {
            any = true;
            let k = (reply.key, reply.px);
            let slot = match reply.image {
                Some((w, h, pixels)) => {
                    self.bytes += pixels.len();
                    Slot::Ready { pixels, w, h, bmp: None, used: self.clock }
                }
                None => Slot::Failed,
            };
            if let Some(Slot::Ready { pixels, .. }) = self.map.insert(k, slot) {
                self.bytes -= pixels.len();
            }
        }
        if self.bytes > self.budget {
            self.evict(self.budget * 3 / 4);
        }
        any
    }

    /// Releases device bitmaps (pixels stay cached).
    pub fn drop_bitmaps(&mut self) {
        for s in self.map.values_mut() {
            if let Slot::Ready { bmp, .. } = s {
                *bmp = None;
            }
        }
    }

    fn evict(&mut self, target: usize) {
        let mut ready: Vec<(u64, (IconKey, u32))> = self
            .map
            .iter()
            .filter_map(|(k, s)| match s {
                Slot::Ready { used, .. } => Some((*used, k.clone())),
                _ => None,
            })
            .collect();
        ready.sort_by_key(|(u, _)| *u);
        for (_, k) in ready {
            if self.bytes <= target {
                break;
            }
            if let Some(Slot::Ready { pixels, .. }) = self.map.remove(&k) {
                self.bytes -= pixels.len();
            }
        }
    }

    /// Drops every cached bitmap (e.g. on DPI/theme change, collapse, or idle trim).
    pub fn clear(&mut self) {
        self.map.retain(|_, s| matches!(s, Slot::Pending));
        self.bytes = 0;
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

fn create_bitmap(rt: &ID2D1RenderTarget, w: u32, h: u32, px: &[u8]) -> Option<ID2D1Bitmap> {
    let props = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    unsafe { rt.CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(px.as_ptr() as *const _), w * 4, &props).ok() }
}

fn worker(rx: Receiver<Request>, tx: Sender<Reply>, notify: isize, msg: u32) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
    while let Ok(req) = rx.recv() {
        let flags = if matches!(req.key, IconKey::Thumb(_)) { SIIGBF_RESIZETOFIT } else { SIIGBF_ICONONLY };
        let image = extract(&req.path, req.px, flags).or_else(|| {
            // No thumbnail (corrupt file, missing codec): fall back to the type icon.
            (flags != SIIGBF_ICONONLY).then(|| extract(&req.path, req.px, SIIGBF_ICONONLY)).flatten()
        });
        if tx.send(Reply { key: req.key, px: req.px, image }).is_err() {
            break;
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(notify as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

/// Extracts the icon of `path` at `px` pixels as premultiplied, top-down BGRA.
fn extract(path: &str, px: u32, flags: SIIGBF) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None::<&IBindCtx>).ok()?;
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let hbm = factory.GetImage(SIZE { cx: px as i32, cy: px as i32 }, flags).ok()?;
        let mut bm = BITMAP::default();
        let got = GetObjectW(HGDIOBJ(hbm.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _));
        let result = (|| {
            if got == 0 || bm.bmWidth <= 0 || bm.bmHeight == 0 {
                return None;
            }
            let (w, h) = (bm.bmWidth as u32, bm.bmHeight.unsigned_abs());
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let hdc = GetDC(None);
            let lines = GetDIBits(hdc, hbm, 0, h, Some(buf.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
            ReleaseDC(None, hdc);
            if lines == 0 {
                return None;
            }
            fix_alpha(&mut buf);
            Some((w, h, buf))
        })();
        let _ = DeleteObject(HGDIOBJ(hbm.0));
        result
    }
}

/// Shell bitmaps come either without alpha (all zero), with straight alpha, or
/// already premultiplied. Normalise to premultiplied.
pub fn fix_alpha(bgra: &mut [u8]) {
    let mut any_alpha = false;
    let mut straight = false;
    for p in bgra.as_chunks::<4>().0 {
        if p[3] != 0 {
            any_alpha = true;
        }
        if p[0] > p[3] || p[1] > p[3] || p[2] > p[3] {
            straight = true;
        }
    }
    if !any_alpha {
        for p in bgra.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
    } else if straight {
        for p in bgra.as_chunks_mut::<4>().0 {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fix_alpha;

    #[test]
    fn alpha_normalisation() {
        let mut opaque = vec![10, 20, 30, 0, 40, 50, 60, 0];
        fix_alpha(&mut opaque);
        assert_eq!(opaque[3], 255);
        let mut straight = vec![200, 200, 200, 128];
        fix_alpha(&mut straight);
        assert_eq!(straight, vec![100, 100, 100, 128]);
        let mut pre = vec![100, 100, 100, 128];
        fix_alpha(&mut pre);
        assert_eq!(pre, vec![100, 100, 100, 128]);
    }
}
