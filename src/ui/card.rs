//! On-demand owned popup. No audio analyser, polling loop or retained hidden artwork.
use super::{Buffer, GetMonitorInfoW, MonitorFromWindow, MonitorInfo, api::*, fill, make_cover, text};
use crate::platform::{
    Error, Result,
    ffi::{E_FAIL, GetLastError, Raw},
};
use std::cell::RefCell;
use std::ptr::{null, null_mut};

const CLASS: &str = "TaskbarMedia.Card.1";
type Key = (Vec<u16>, Vec<u16>, Vec<u16>, usize, u32, (u32, u32, u32), (i32, i32));
struct Data {
    owner: Raw,
    key: Option<Key>,
    cover: Option<(Buffer, Rect)>,
    text: Vec<(text::Raster, Rect)>,
    buffer: Option<Buffer>,
    background: u32,
    pinned: bool,
    tracking: bool,
    _activity: crate::platform::Activity,
}
pub(super) struct Card {
    hwnd: Raw,
}
impl Card {
    pub fn hwnd(&self) -> Raw {
        self.hwnd
    }
    pub fn new(owner: Raw) -> Result<Self> {
        let module = super::instance()?;
        let class = WndClass {
            size: size_of::<WndClass>() as u32,
            style: 0,
            proc: window_proc,
            class_extra: 0,
            window_extra: 0,
            instance: module,
            icon: null_mut(),
            cursor: null_mut(),
            background: null_mut(),
            menu: null(),
            name: null(),
            small_icon: null_mut(),
        };
        // Keep the class name buffer alive for registration.
        let name = wide(CLASS);
        let class = WndClass { name: name.as_ptr(), ..class };
        // SAFETY: registering our module's private popup class; user data transfers in WM_NCCREATE.
        unsafe {
            if RegisterClassExW(&class) == 0 && GetLastError() != 1410 {
                return Err(Error::last("register track card"));
            }
            let mut context = Some(Box::new(RefCell::new(Data {
                owner,
                key: None,
                cover: None,
                text: Vec::new(),
                buffer: None,
                background: 0,
                pinned: false,
                tracking: false,
                _activity: crate::platform::Activity::new(),
            })));
            let hwnd = CreateWindowExW(
                0x08000080,
                name.as_ptr(),
                wide("Taskbar Media").as_ptr(),
                0x80000000,
                0,
                0,
                0,
                0,
                owner,
                null_mut(),
                module,
                (&mut context as *mut Option<Box<RefCell<Data>>>).cast(),
            );
            if hwnd.is_null() {
                return Err(Error::last("create track card"));
            }
            Ok(Self { hwnd })
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        engine: &text::Engine,
        title: &[u16],
        artist: &[u16],
        album: &[u16],
        image: Option<&std::sync::Arc<crate::Image>>,
        anchor: Rect,
        dpi: u32,
        colors: (u32, u32, u32),
    ) -> Result<()> {
        let scale = |n| ((n as i64 * dpi as i64 + 48) / 96) as i32;
        // SAFETY: the popup belongs to this thread and is retained by this Card owner.
        unsafe {
            let pointer = GetWindowLongPtrW(self.hwnd, -21) as *const RefCell<Data>;
            if pointer.is_null() {
                return Err(Error::new("track card closed", E_FAIL));
            }
            let mut data = (*pointer).borrow_mut();
            let mut info = MonitorInfo {
                size: size_of::<MonitorInfo>() as u32,
                monitor: Rect::default(),
                work: Rect::default(),
                flags: 0,
            };
            if GetMonitorInfoW(MonitorFromWindow(data.owner, 2), &mut info) == 0 {
                return Err(Error::last("locate card monitor"));
            }
            let width = scale(208).min(info.work.width()).max(1);
            let pad = scale(10).min(width / 4).min(info.work.height() / 4);
            let gap = scale(4);
            let key = (
                title.to_vec(),
                artist.to_vec(),
                album.to_vec(),
                image.map_or(0, |i| std::sync::Arc::as_ptr(i) as usize),
                dpi,
                colors,
                (width, info.work.height()),
            );
            if data.key.as_ref() != Some(&key) {
                let mut rows = Vec::with_capacity(3);
                let mut body = 0;
                for (value, size, weight, color, max_height) in
                    [(title, 14., 600, colors.1, 40), (artist, 12., 400, colors.2, 34), (album, 11., 400, colors.2, 16)]
                {
                    if value.len() < 2 {
                        continue;
                    }
                    let spacing = if rows.is_empty() { 0 } else { gap };
                    let available = info.work.height() - pad * 2 - body - spacing;
                    if available <= 0 || width <= pad * 2 {
                        break;
                    }
                    let area =
                        Rect { left: 0, top: 0, right: width - pad * 2, bottom: scale(max_height).min(available) };
                    let raster =
                        engine.raster_fit(value, area, text::Style { dpi, size, weight, color, wrap: true })?;
                    body += spacing + raster.buffer.height;
                    rows.push((raster, Rect::default()));
                }
                let cover_gap = if body > 0 { scale(8) } else { 0 };
                let side = if image.is_some() {
                    (width - pad * 2).min((info.work.height() - pad * 2 - body - cover_gap).max(0))
                } else {
                    0
                };
                let height = (pad * 2 + side + body + if side > 0 { cover_gap } else { 0 }).max(1);
                let area = Rect { left: (width - side) / 2, top: pad, right: (width + side) / 2, bottom: pad + side };
                let cover = if let Some(image) = image.filter(|_| side > 0) {
                    Some((make_cover(image, area)?, area))
                } else {
                    None
                };
                let mut y = if side > 0 { area.bottom + cover_gap } else { pad };
                for (raster, area) in &mut rows {
                    *area = Rect { left: pad, top: y, right: width - pad, bottom: y + raster.buffer.height };
                    y = area.bottom + gap;
                }
                if data.buffer.as_ref().is_none_or(|b| b.width != width || b.height != height) {
                    data.buffer =
                        Some(Buffer::new(null_mut(), width, height).ok_or(Error::new("allocate card pixels", E_FAIL))?);
                }
                data.cover = cover;
                data.text = rows;
                data.background = colors.0;
                data.key = Some(key);
                SetWindowTextW(self.hwnd, title.as_ptr());
                let dark = i32::from(colors.0 & 255 < 128);
                DwmSetWindowAttribute(self.hwnd, 20, &dark, 4);
                InvalidateRect(self.hwnd, null(), 0);
            }
            let height = data.buffer.as_ref().map_or(1, |b| b.height);
            let bounds = place(anchor, info.work, width, height, scale(8));
            drop(data);
            let mut previous = Rect::default();
            if GetWindowRect(self.hwnd, &mut previous) == 0 || previous != bounds {
                SetWindowPos(self.hwnd, (-1isize) as Raw, bounds.left, bounds.top, width, height, 0x10);
            }
            Ok(())
        }
    }
    pub fn show(hwnd: Raw, pinned: bool) {
        // SAFETY: explicit click may activate this popup; hover never takes keyboard focus.
        unsafe {
            let pointer = GetWindowLongPtrW(hwnd, -21) as *const RefCell<Data>;
            if pointer.is_null() {
                return;
            }
            (*pointer).borrow_mut().pinned = pinned;
            ShowWindow(hwnd, if pinned { 5 } else { 4 });
            if pinned {
                SetForegroundWindow(hwnd);
                SetFocus(hwnd);
            }
        }
    }
    pub fn contains(&self, point: Point) -> bool {
        // SAFETY: synchronous screen bounds query, never a borrowed window-data pointer.
        unsafe {
            let mut r = Rect::default();
            GetWindowRect(self.hwnd, &mut r) != 0
                && point.x >= r.left
                && point.x < r.right
                && point.y >= r.top
                && point.y < r.bottom
        }
    }
}
impl Drop for Card {
    fn drop(&mut self) {
        // SAFETY: this owner destroys its popup on the same UI thread.
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}
pub(super) fn place(anchor: Rect, work: Rect, width: i32, height: i32, gap: i32) -> Rect {
    let mut x = anchor.left;
    let mut y = anchor.top - height - gap;
    if anchor.right <= work.left {
        x = work.left + gap;
        y = anchor.top;
    } else if anchor.left >= work.right {
        x = work.right - width - gap;
        y = anchor.top;
    } else if y < work.top {
        y = anchor.bottom + gap;
    }
    x = x.clamp(work.left, (work.right - width).max(work.left));
    y = y.clamp(work.top, (work.bottom - height).max(work.top));
    Rect { left: x, top: y, right: x + width, bottom: y + height }
}
unsafe extern "system" fn window_proc(hwnd: Raw, message: u32, w: usize, l: isize) -> isize {
    std::panic::catch_unwind(|| dispatch(hwnd, message, w, l)).unwrap_or(0)
}
fn dispatch(hwnd: Raw, message: u32, w: usize, l: isize) -> isize {
    // SAFETY: the class owns one Box<RefCell<Data>> between WM_NCCREATE and WM_NCDESTROY.
    unsafe {
        if message == 0x0081 {
            let create = &*(l as *const Create);
            let context = &mut *(create.params as *mut Option<Box<RefCell<Data>>>);
            if let Some(data) = context.take() {
                SetWindowLongPtrW(hwnd, -21, Box::into_raw(data) as isize);
                return 1;
            }
            return 0;
        }
        let pointer = GetWindowLongPtrW(hwnd, -21) as *const RefCell<Data>;
        if pointer.is_null() {
            return DefWindowProcW(hwnd, message, w, l);
        }
        if message == 0x0082 {
            SetWindowLongPtrW(hwnd, -21, 0);
            drop(Box::from_raw(pointer.cast_mut()));
            return DefWindowProcW(hwnd, message, w, l);
        }
        let Ok(mut data) = (*pointer).try_borrow_mut() else {
            return DefWindowProcW(hwnd, message, w, l);
        };
        match message {
            0x0014 => 1,
            0x0021 if !data.pinned => 3,
            0x0200 => {
                if !data.tracking {
                    let mut track = MouseTrack { size: size_of::<MouseTrack>() as u32, flags: 2, hwnd, hover: 0 };
                    data.tracking = TrackMouseEvent(&mut track) != 0;
                    PostMessageW(data.owner, super::WM_CARD_ARM, hwnd as usize, 0);
                }
                0
            }
            0x02a3 => {
                data.tracking = false;
                PostMessageW(data.owner, super::WM_CARD_LEAVE, hwnd as usize, 0);
                0
            }
            0x0201 => {
                PostMessageW(data.owner, super::WM_CARD_PIN, 0, 0);
                0
            }
            0x0100 if w == 27 => {
                PostMessageW(data.owner, super::WM_CARD_CLOSE, hwnd as usize, 0);
                0
            }
            0x0010 => {
                PostMessageW(data.owner, super::WM_CARD_CLOSE, hwnd as usize, 0);
                0
            }
            0x0006 if w & 0xffff == 0 && data.pinned => {
                PostMessageW(data.owner, super::WM_CARD_CLOSE, hwnd as usize, 0);
                0
            }
            0x000f => {
                let mut paint: Paint = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut paint);
                if !dc.is_null() {
                    let mut area = Rect::default();
                    GetClientRect(hwnd, &mut area);
                    let target = data.buffer.as_ref().map_or(dc, |b| b.dc);
                    fill(target, area, data.background);
                    if let Some((cover, rect)) = &data.cover {
                        AlphaBlend(
                            target,
                            rect.left,
                            rect.top,
                            cover.width,
                            cover.height,
                            cover.dc,
                            0,
                            0,
                            cover.width,
                            cover.height,
                            0x01ff0000,
                        );
                    }
                    for (raster, rect) in &data.text {
                        raster.draw(target, *rect);
                    }
                    if target != dc {
                        BitBlt(dc, 0, 0, area.width(), area.height(), target, 0, 0, 0x00cc0020);
                    }
                    EndPaint(hwnd, &paint);
                }
                0
            }
            _ => {
                drop(data);
                DefWindowProcW(hwnd, message, w, l)
            }
        }
    }
}
pub(super) fn unregister(module: Raw) {
    // SAFETY: called only when the module has no live windows or callbacks.
    unsafe {
        UnregisterClassW(wide(CLASS).as_ptr(), module);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn placement_stays_on_the_selected_monitor_for_every_taskbar_edge() {
        let work = Rect { left: -1920, top: 0, right: 0, bottom: 1040 };
        for anchor in [
            Rect { left: -1900, top: 1040, right: -1500, bottom: 1080 },
            Rect { left: -1900, top: -40, right: -1500, bottom: 0 },
            Rect { left: -1960, top: 500, right: -1920, bottom: 800 },
            Rect { left: 0, top: 500, right: 40, bottom: 800 },
        ] {
            let r = place(anchor, work, 280, 320, 8);
            assert!(r.left >= work.left && r.right <= work.right && r.top >= work.top && r.bottom <= work.bottom);
        }
    }
}
