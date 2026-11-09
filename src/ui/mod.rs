//! One native view for the standalone window and the taskbar container.
pub(crate) mod api;
mod lang;
mod rebar;
use crate::media::{Command, Playback};
use crate::platform::ffi::{E_FAIL, Raw};
use crate::platform::{Error, Result};
use crate::runtime::{Runtime, State};
use crate::settings::{self, Language, Settings, Theme, Visual};
use api::*;
use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

const CLASS: &str = "TaskbarMedia.View.1";
const WM_REFRESH: u32 = 0x8000 + 45;
const WM_NOTICE: u32 = 0x8000 + 48;
const WM_FOCUS_CHANGED: u32 = 0x8000 + 49;
const WM_BAND_SHOWN: u32 = 0x8000 + 50;
const WM_BAND_RESIZE: u32 = 0x8000 + 51;
const WM_PICK_COLOR: u32 = 0x8000 + 52;
const SAVE_RESIZE_TIMER: usize = 7002;
static CLASS_MODULE: OnceLock<usize> = OnceLock::new();

#[derive(Clone, Copy, Default)]
struct Layout {
    text: Rect,
    visual: Rect,
    artwork: Rect,
    buttons: [Rect; 4],
}
impl Layout {
    fn has_visual(&self) -> bool {
        self.visual.width() > 0 && self.visual.height() > 0
    }
    fn new(w: i32, h: i32, dpi: u32, settings: &Settings) -> Self {
        if w <= 0 || h <= 0 {
            return Self::default();
        }
        let scale = |v: i32| ((v as i64 * dpi as i64 + 48) / 96) as i32;
        let gap = scale(6);
        let row = h.min(scale(40));
        let side = scale(30).min(row).min(w);
        let enabled = |i| if i == 3 { settings.menu_button } else { settings.controls };
        let mut r = Self::default();
        if w < scale(120) && h > scale(80) {
            let mut x = 0;
            for i in [1, 3, 0, 2].into_iter().filter(|&i| enabled(i)) {
                if x + side > w {
                    break;
                }
                r.buttons[i] = Rect { left: x, top: h - side, right: x + side, bottom: h };
                x += side;
            }
            let bottom = if x > 0 { h - side - gap } else { h };
            let mut top = 0;
            let size = w.min((bottom - scale(32) - gap).max(0));
            if settings.artwork && size >= scale(24) {
                r.artwork = Rect { left: 0, top: 0, right: w, bottom: size };
                top = size + gap;
            }
            r.text = Rect { left: gap, top, right: w - gap, bottom: (top + scale(32)).min(bottom) };
            if settings.visual != Visual::Off && bottom > r.text.bottom + gap {
                r.visual = Rect { left: 0, top: r.text.bottom + gap, right: w, bottom };
            }
            return r.bounded(w, h);
        }
        let min_content = scale(64).min(w);
        let mut selected = [false; 4];
        let mut count = 0;
        for i in [1, 3, 0, 2].into_iter().filter(|&i| enabled(i)) {
            if (count + 1) * side > w - min_content {
                break;
            }
            selected[i] = true;
            count += 1;
        }
        let right = w - count * side;
        let mut x = right;
        for (i, &show) in selected.iter().enumerate() {
            if show {
                let y = (row - side) / 2;
                r.buttons[i] = Rect { left: x, top: y, right: x + side, bottom: y + side };
                x += side;
            }
        }
        let mut left = 0;
        if settings.artwork && right >= scale(64) {
            let cover_width = h.min((right - scale(96) - gap).max(scale(16)));
            r.artwork = Rect { left: 0, top: 0, right: cover_width, bottom: h };
            left = cover_width + gap;
        }
        let available = (right - left).max(0);
        let vw = if settings.visual != Visual::Off && available >= scale(144) {
            (available / 3).clamp(scale(48), scale(144))
        } else {
            0
        };
        let text_right = right - if vw > 0 { vw + gap } else { 0 };
        let text_height = row.min(scale(32));
        let text_top = (row - text_height) / 2;
        r.text = Rect { left, top: text_top, right: text_right, bottom: text_top + text_height };
        if vw > 0 {
            r.visual = Rect { left: right - vw, top: 0, right, bottom: if settings.full_height { h } else { row } };
        }
        r.bounded(w, h)
    }
    fn bounded(mut self, w: i32, h: i32) -> Self {
        for r in [&mut self.text, &mut self.visual, &mut self.artwork].into_iter().chain(self.buttons.iter_mut()) {
            r.left = r.left.clamp(0, w);
            r.top = r.top.clamp(0, h);
            r.right = r.right.clamp(r.left, w);
            r.bottom = r.bottom.clamp(r.top, h);
        }
        self
    }
    fn fit_text(&mut self, width: i32, dpi: u32) {
        if self.visual.width() > 0 && self.text.right < self.visual.left {
            let gap = ((4 * dpi + 48) / 96) as i32;
            self.text.right = (self.text.left + width.max(0)).min(self.text.right);
            self.visual.left = (self.text.right + gap).min(self.visual.right);
        }
    }
}

fn solid_wave(
    wave: &[crate::visual::Range; crate::visual::POINTS],
    area: Rect,
    gain: f32,
) -> [Point; crate::visual::POINTS * 2] {
    const N: usize = crate::visual::POINTS;
    let mut points = [Point::default(); N * 2];
    let center = (area.top + area.bottom) / 2;
    let amplitude = (area.height().saturating_sub(1)) as f32 * 0.5;
    for (i, range) in wave.iter().enumerate() {
        let x = area.left + i as i32 * area.width() / (N - 1) as i32;
        let upper = center - ((range.high * gain).clamp(-1.0, 1.0) * amplitude) as i32;
        let lower = center - ((range.low * gain).clamp(-1.0, 1.0) * amplitude) as i32 + 1;
        points[i] = Point { x, y: upper.clamp(area.top, area.bottom) };
        points[N * 2 - 1 - i] = Point { x, y: lower.clamp(area.top, area.bottom) };
    }
    points
}

fn artwork_crop(width: i32, height: i32, target: Rect) -> Rect {
    if width <= 0 || height <= 0 || target.width() == 0 || target.height() == 0 {
        return Rect::default();
    }
    let (w, h) = if width as i64 * target.height() as i64 > height as i64 * target.width() as i64 {
        (((height as i64 * target.width() as i64 / target.height() as i64) as i32).clamp(1, width), height)
    } else {
        (width, ((width as i64 * target.height() as i64 / target.width() as i64) as i32).clamp(1, height))
    };
    let left = (width - w) / 2;
    let top = (height - h) / 2;
    Rect { left, top, right: left + w, bottom: top + h }
}

struct Buffer {
    dc: Raw,
    bitmap: Raw,
    old: Raw,
    width: i32,
    height: i32,
    bits: Raw,
}
struct CompositedPaint {
    handle: Raw,
    dc: Raw,
    rect: Rect,
    transparent: bool,
    pixels: Raw,
    stride: i32,
}
impl CompositedPaint {
    fn new(dc: Raw, rect: Rect) -> Option<Self> {
        if rect.width() == 0 || rect.height() == 0 || rect.width() as i64 * rect.height() as i64 > 2_097_152 {
            return None;
        }
        let mut target = null_mut();
        // SAFETY: bounded rect and a live target DC; UxTheme owns and reuses its thread-local buffers.
        let handle = unsafe { BeginBufferedPaint(dc, &rect, 2, null_mut(), &mut target) };
        if handle.is_null() {
            None
        } else {
            Some(Self { handle, dc: target, rect, transparent: false, pixels: null_mut(), stride: 0 })
        }
    }
    fn clear_transparent(&mut self) -> bool {
        let mut bits = null_mut();
        let mut stride = 0;
        // SAFETY: validate access to the owned top-down DIB before using the alpha path.
        unsafe {
            if GetBufferedPaintBits(self.handle, &mut bits, &mut stride) < 0
                || bits.is_null()
                || stride < self.rect.width()
                || BufferedPaintClear(self.handle, null()) < 0
            {
                return false;
            }
        }
        self.transparent = true;
        self.pixels = bits;
        self.stride = stride;
        true
    }
    fn colorize(&self, rect: Rect, color: u32) {
        if !self.transparent {
            return;
        }
        let rect = Rect {
            left: rect.left.max(self.rect.left),
            top: rect.top.max(self.rect.top),
            right: rect.right.min(self.rect.right),
            bottom: rect.bottom.min(self.rect.bottom),
        };
        if rect.width() == 0 || rect.height() == 0 {
            return;
        }
        // SAFETY: flush GDI before CPU access, clip to this bounded DIB and respect its pixel stride.
        unsafe {
            GdiFlush();
            for y in rect.top..rect.bottom {
                let start = (y - self.rect.top) as usize * self.stride as usize + (rect.left - self.rect.left) as usize;
                let row = std::slice::from_raw_parts_mut(self.pixels.cast::<u32>().add(start), rect.width() as usize);
                for pixel in row {
                    // White grayscale glyphs/lines encode coverage, including fully black requested colours.
                    let a = *pixel & 255;
                    let r = ((color & 255) * a + 127) / 255;
                    let g = (((color >> 8) & 255) * a + 127) / 255;
                    let b = (((color >> 16) & 255) * a + 127) / 255;
                    *pixel = (a << 24) | (r << 16) | (g << 8) | b;
                }
            }
        }
    }
}
impl Drop for CompositedPaint {
    fn drop(&mut self) {
        // SAFETY: GDI does not maintain alpha. Make the completed opaque surface valid for composition.
        unsafe {
            if !self.transparent {
                BufferedPaintSetAlpha(self.handle, null(), 255);
            }
            EndBufferedPaint(self.handle, 1);
        }
    }
}
impl Buffer {
    fn new(dc: Raw, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 || width as i64 * height as i64 > 2_097_152 {
            return None;
        }
        // SAFETY: GDI allocations are owned here; compatible memory DC selects one bounded DIB.
        unsafe {
            let memory = CreateCompatibleDC(dc);
            if memory.is_null() {
                return None;
            }
            let info = bitmap_info(width, height);
            let mut bits = null_mut();
            let bitmap = CreateDIBSection(dc, &info, 0, &mut bits, null_mut(), 0);
            if bitmap.is_null() {
                DeleteDC(memory);
                return None;
            }
            let old = SelectObject(memory, bitmap);
            Some(Self { dc: memory, bitmap, old, width, height, bits })
        }
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: restore the previous bitmap before deleting the selected DIB and its DC.
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}
fn bitmap_info(w: i32, h: i32) -> BitmapInfo {
    BitmapInfo {
        size: 40,
        width: w,
        height: -h,
        planes: 1,
        bits: 32,
        compression: 0,
        image: 0,
        x: 0,
        y: 0,
        used: 0,
        important: 0,
        colors: [0],
    }
}

struct View {
    hwnd: Raw,
    standalone: bool,
    band_enabled: bool,
    composited: bool,
    buttons: [Raw; 4],
    runtime: Option<Runtime>,
    restart_pending: bool,
    state: State,
    settings: Settings,
    path: std::path::PathBuf,
    dpi: u32,
    layout: Layout,
    buffer: Option<Buffer>,
    cover: Option<(std::sync::Arc<crate::Image>, Buffer)>,
    font: Raw,
    dark: bool,
    contrast: bool,
    visible: bool,
    suspended: bool,
    locked: bool,
    hook: Raw,
    power: Raw,
    title: Vec<u16>,
    artist: Vec<u16>,
    caption: String,
    notice: Option<String>,
    band_site: Option<crate::platform::com::Com>,
    band_id: u32,
    host_size: (i32, i32),
    resize_save_pending: bool,
    // Borrowed identity: Band destroys this window before its allocation is released.
    band_identity: Raw,
    _activity: crate::platform::Activity,
}

impl Drop for View {
    fn drop(&mut self) {
        self.runtime.take();
        self.buffer.take();
        self.cover.take();
        // SAFETY: owned hook/font; unhook before the view pointer is destroyed.
        unsafe {
            if !self.hook.is_null() {
                UnhookWinEvent(self.hook);
            }
            WTSUnRegisterSessionNotification(self.hwnd);
            if !self.power.is_null() {
                UnregisterSuspendResumeNotification(self.power);
            }
            if !self.font.is_null() {
                DeleteObject(self.font);
            }
            BufferedPaintUnInit();
        }
        VIEWS.with(|v| v.borrow_mut().retain(|&hwnd| hwnd != self.hwnd));
    }
}
thread_local! {static VIEWS:RefCell<Vec<Raw>>=const {RefCell::new(Vec::new())};}

fn instance() -> Result<Raw> {
    let mut module = null_mut();
    // SAFETY: resolve this implementation's module, including when loaded into Explorer.
    if unsafe { GetModuleHandleExW(4 | 2, window_proc as *const () as *const u16, &mut module) } == 0 {
        return Err(Error::last("view module"));
    }
    let class = wide(CLASS);
    // SAFETY: callbacks are static, name is consumed by RegisterClassExW.
    unsafe {
        let wc = WndClass {
            size: size_of::<WndClass>() as u32,
            style: 3,
            proc: window_proc,
            class_extra: 0,
            window_extra: 0,
            instance: module,
            icon: LoadIconW(module, std::ptr::dangling::<u16>()),
            cursor: LoadCursorW(null_mut(), 32512usize as *const u16),
            background: null_mut(),
            menu: null(),
            name: class.as_ptr(),
            small_icon: null_mut(),
        };
        if RegisterClassExW(&wc) == 0 && crate::platform::ffi::GetLastError() != 1410 {
            return Err(Error::last("register media view"));
        }
    }
    let _ = CLASS_MODULE.set(module as usize);
    Ok(module)
}

/// Creates a real view. A non-null parent is a container-owned child, not a positioned overlay.
pub(crate) fn create(parent: Raw) -> Result<Raw> {
    let module = instance()?;
    let path = settings::path().map_err(|_| Error::new("locate portable settings", E_FAIL))?;
    let settings = Settings::load(&path).map_err(|_| Error::new("read portable settings", E_FAIL))?;
    let standalone = parent.is_null();
    // SAFETY: one initialization per view, balanced on its owning UI thread by View::drop.
    crate::platform::check(unsafe { BufferedPaintInit() }, "initialize buffered painting")?;
    let view = Box::new(RefCell::new(View {
        hwnd: null_mut(),
        standalone,
        band_enabled: standalone,
        composited: false,
        buttons: [null_mut(); 4],
        runtime: None,
        restart_pending: false,
        state: State::default(),
        settings: settings.clone(),
        path,
        dpi: 96,
        layout: Layout::default(),
        buffer: None,
        cover: None,
        font: null_mut(),
        dark: false,
        contrast: false,
        visible: false,
        suspended: false,
        locked: false,
        hook: null_mut(),
        power: null_mut(),
        title: Vec::new(),
        artist: Vec::new(),
        caption: String::new(),
        notice: None,
        band_site: None,
        band_id: 0,
        host_size: (0, 0),
        resize_save_pending: false,
        band_identity: null_mut(),
        _activity: crate::platform::Activity::new(),
    }));
    let mut context = Some(view);
    let style = if standalone { 0x00cf0000 | 0x02000000 } else { 0x40000000 | 0x02000000 | 0x04000000 };
    // SAFETY: WM_NCCREATE transfers the Box to the window; WM_NCDESTROY owns its destruction.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            wide(CLASS).as_ptr(),
            wide("Taskbar Media").as_ptr(),
            style,
            if standalone { i32::MIN } else { 0 },
            if standalone { i32::MIN } else { 0 },
            settings.width as i32,
            if standalone { 120 } else { 40 },
            parent,
            null_mut(),
            module,
            (&mut context as *mut Option<Box<RefCell<View>>>).cast(),
        )
    };
    if hwnd.is_null() {
        return Err(Error::last("create media view"));
    }
    Ok(hwnd)
}

pub fn run() -> Result<()> {
    run_for(None)
}

pub fn report_startup_error(error: &(dyn std::error::Error + 'static)) {
    let language = Settings::load(&settings::path().unwrap_or_default()).unwrap_or_default().language;
    let t = lang::text(language);
    let details =
        error.downcast_ref::<Error>().map_or_else(|| error.to_string(), |e| format!("0x{:08X}", e.code as u32));
    // SAFETY: terminated labels remain alive for the synchronous native message box.
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(&format!("{}\n{}", t.startup_error, details)).as_ptr(),
            wide("Taskbar Media").as_ptr(),
            0x10,
        );
    }
}

/// Optional shutdown deadline for explicit lifecycle checks; no periodic frame timer.
pub fn run_for(duration: Option<std::time::Duration>) -> Result<()> {
    // SAFETY: set only in the standalone process, before creating its first HWND.
    unsafe {
        SetProcessDpiAwarenessContext(-4);
    }
    let hwnd = create(null_mut())?;
    // SAFETY: the window is owned by this UI thread; standard message dispatch.
    unsafe {
        if let Some(duration) = duration {
            let ms = duration.as_millis().clamp(1, u32::MAX as u128) as u32;
            if SetTimer(hwnd, 7001, ms, null_mut()) == 0 {
                DestroyWindow(hwnd);
                return Err(Error::last("set window deadline"));
            }
        }
        ShowWindow(hwnd, 5);
        let mut msg: Msg = std::mem::zeroed();
        loop {
            match GetMessageW(&mut msg, null_mut(), 0, 0) {
                -1 => return Err(Error::last("UI message loop")),
                0 => break,
                _ => {}
            }
            if IsDialogMessageW(hwnd, &mut msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    Ok(())
}

unsafe extern "system" fn window_proc(hwnd: Raw, message: u32, w: usize, l: isize) -> isize {
    let result = std::panic::catch_unwind(|| dispatch(hwnd, message, w, l));
    match result {
        Ok(value) => value,
        Err(_) => {
            // SAFETY: schedule disposal rather than unwind through Windows or abort Explorer.
            unsafe {
                PostMessageW(hwnd, 0x0010, 0, 0);
                DefWindowProcW(hwnd, message, w, l)
            }
        }
    }
}
unsafe extern "system" fn button_proc(hwnd: Raw, message: u32, w: usize, l: isize, id: usize, _: usize) -> isize {
    // SAFETY: only our owning-thread buttons are subclassed; native input/accessibility are preserved.
    unsafe {
        if message == 0x0014 {
            return 1; // Owner draw supplies the complete background, including transparent pixels.
        }
        if message == 0x0082 {
            RemoveWindowSubclass(hwnd, button_proc, id);
        }
        DefSubclassProc(hwnd, message, w, l)
    }
}
fn dispatch(hwnd: Raw, message: u32, w: usize, l: isize) -> isize {
    // SAFETY: USERDATA is initialized with a Box<RefCell<View>> during WM_NCCREATE.
    unsafe {
        if message == 0x0081 {
            let create = &*(l as *const Create);
            let context = &mut *(create.params as *mut Option<Box<RefCell<View>>>);
            let Some(view) = context.take() else {
                return 0;
            };
            SetWindowLongPtrW(hwnd, -21, Box::into_raw(view) as isize);
            return 1;
        }
        let pointer = GetWindowLongPtrW(hwnd, -21) as *mut RefCell<View>;
        if pointer.is_null() {
            return DefWindowProcW(hwnd, message, w, l);
        }
        if message == 0x0082 {
            SetWindowLongPtrW(hwnd, -21, 0);
            let standalone = (*pointer).borrow().standalone;
            let identity = (*pointer).borrow().band_identity;
            if !identity.is_null() {
                crate::host::view_destroyed(identity, hwnd);
            }
            drop(Box::from_raw(pointer));
            if standalone {
                PostQuitMessage(0);
            }
            return DefWindowProcW(hwnd, message, w, l);
        }
        if message == 0x0010 {
            DestroyWindow(hwnd);
            return 0;
        }
        if message == WM_NOTICE {
            let notice = (*pointer).borrow_mut().notice.take();
            if let Some(notice) = notice {
                MessageBoxW(hwnd, wide(&notice).as_ptr(), wide("Taskbar Media").as_ptr(), 0x10);
            }
            return 0;
        }
        if message == WM_PICK_COLOR {
            let (accent, mut colors) = {
                let view = (*pointer).borrow();
                (view.settings.accent, view.settings.custom_colors)
            };
            let mut dialog = ChooseColor {
                size: size_of::<ChooseColor>() as u32,
                owner: hwnd,
                instance: null_mut(),
                result: accent,
                custom: colors.as_mut_ptr(),
                flags: 0x103,
                data: 0,
                hook: null_mut(),
                template: null(),
            };
            // No View borrow crosses the dialog's nested message loop.
            let selected = ChooseColorW(&mut dialog) != 0;
            let error = if selected { 0 } else { CommDlgExtendedError() };
            if GetWindowLongPtrW(hwnd, -21) as *mut RefCell<View> != pointer {
                return 0;
            }
            if let Ok(mut view) = (*pointer).try_borrow_mut() {
                if error != 0 {
                    view.notice = Some(format!("{} (0x{error:08X})", lang::text(view.settings.language).color_error));
                    PostMessageW(hwnd, WM_NOTICE, 0, 0);
                } else if colors != view.settings.custom_colors || selected && dialog.result != view.settings.accent {
                    view.settings.custom_colors = colors;
                    if selected {
                        view.settings.accent = dialog.result & 0xffffff;
                    }
                    view.save_settings();
                    InvalidateRect(hwnd, &view.layout.visual, 0);
                    for &button in &view.buttons {
                        InvalidateRect(button, null(), 0);
                    }
                }
            }
            return 0;
        }
        if matches!(message, WM_FOCUS_CHANGED | WM_BAND_RESIZE) || message == crate::host::BAND_CHANGED {
            let (site, id, identity, width, standalone) = {
                let view = (*pointer).borrow();
                let identity = if view.band_identity.is_null() {
                    None
                } else {
                    use crate::platform::ffi::UnknownVtbl;
                    let vt = &**(view.band_identity as *const *const UnknownVtbl);
                    (vt.add_ref)(view.band_identity);
                    crate::platform::com::Com::owned(view.band_identity, "band identity").ok()
                };
                (view.band_site.clone(), view.band_id, identity, view.settings.width, view.standalone)
            };
            // Shell calls can destroy the view synchronously; no View borrow crosses them.
            if message == WM_FOCUS_CHANGED {
                View::focus_changed(site.as_ref(), identity.as_ref(), hwnd);
            } else if standalone && message == WM_BAND_RESIZE {
                let mut rect = Rect::default();
                GetWindowRect(hwnd, &mut rect);
                let pixels = ((width as u64 * GetDpiForWindow(hwnd).max(96) as u64 + 48) / 96) as i32;
                SetWindowPos(hwnd, null_mut(), 0, 0, pixels, rect.height(), 0x16);
            } else {
                let notification = View::band_changed(site.as_ref(), id);
                if site.is_some() {
                    let result = rebar::set_size(hwnd, (message == WM_BAND_RESIZE).then_some(width));
                    if let Err(resize_error) = result
                        && message == WM_BAND_RESIZE
                    {
                        let error = notification.err().unwrap_or(resize_error);
                        let pointer = GetWindowLongPtrW(hwnd, -21) as *const RefCell<View>;
                        if !pointer.is_null()
                            && let Ok(mut view) = (*pointer).try_borrow_mut()
                        {
                            let t = lang::text(view.settings.language);
                            view.notice = Some(format!("{} (0x{:08X})", t.resize_error, error.code as u32));
                            PostMessageW(hwnd, WM_NOTICE, 0, 0);
                        }
                    }
                }
            }
            return 0;
        }
        if message == 0x0007 || message == 0x0008 || message == 0x0111 && matches!((w >> 16) & 0xffff, 6 | 7) {
            PostMessageW(hwnd, WM_FOCUS_CHANGED, 0, 0);
            return DefWindowProcW(hwnd, message, w, l);
        }
        if message == 0x007b || (message == 0x0111 && w & 0xffff == 104 && (w >> 16) & 0xffff == 0) {
            let settings = (*pointer).borrow().settings.clone();
            let standalone = (*pointer).borrow().standalone;
            let mut point = Point::default();
            if message == 0x007b && l != -1 {
                point.x = l as i16 as i32;
                point.y = (l >> 16) as i16 as i32;
            } else {
                GetCursorPos(&mut point);
            }
            let command = popup(hwnd, &settings, point, standalone);
            // A nested menu loop can process WM_CLOSE; never reuse its old allocation afterwards.
            let pointer = GetWindowLongPtrW(hwnd, -21) as *const RefCell<View>;
            if !pointer.is_null()
                && let Ok(mut view) = (*pointer).try_borrow_mut()
            {
                view.setting(command);
            }
            return 0;
        }
        let Ok(mut view) = (*pointer).try_borrow_mut() else {
            return DefWindowProcW(hwnd, message, w, l);
        };
        match message {
            0x0001 => {
                view.hwnd = hwnd;
                VIEWS.with(|v| v.borrow_mut().push(hwnd));
                view.style();
                for i in 0..4 {
                    view.buttons[i] = CreateWindowExW(
                        0,
                        wide("BUTTON").as_ptr(),
                        wide("").as_ptr(),
                        0x5001400b,
                        0,
                        0,
                        0,
                        0,
                        hwnd,
                        (101 + i) as Raw,
                        CLASS_MODULE.get().copied().unwrap_or(0) as Raw,
                        null_mut(),
                    );
                    if view.buttons[i].is_null() || SetWindowSubclass(view.buttons[i], button_proc, 1, 0) == 0 {
                        return -1;
                    }
                }
                if view.standalone {
                    match Runtime::start(hwnd, &view.settings) {
                        Ok(runtime) => view.runtime = Some(runtime),
                        Err(e) => view.state.error = Some(e),
                    }
                }
                view.update_text();
                view.resize();
                let root = GetAncestor(hwnd, 2);
                let mut process = 0;
                let thread = GetWindowThreadProcessId(root, &mut process);
                view.hook = SetWinEventHook(0x8002, 0x800b, null_mut(), visibility_event, process, thread, 0);
                WTSRegisterSessionNotification(hwnd, 0);
                view.power = RegisterSuspendResumeNotification(hwnd, 0);
                0
            }
            crate::runtime::MESSAGE => {
                if let Some(runtime) = &view.runtime
                    && runtime.cookie() == w
                {
                    let next = runtime.read();
                    let chrome_changed = view.state.snapshot != next.snapshot
                        || view.state.error != next.error
                        || view.state.audio_error != next.audio_error
                        || view.state.busy != next.busy
                        || view.state.artwork_error != next.artwork_error;
                    let artwork_changed = match (&view.state.artwork, &next.artwork) {
                        (Some(a), Some(b)) => !std::sync::Arc::ptr_eq(a, b),
                        (None, None) => false,
                        _ => true,
                    };
                    view.state = next;
                    if chrome_changed || artwork_changed {
                        view.update_text();
                        view.resize();
                        view.visibility();
                        InvalidateRect(hwnd, null(), 0);
                    } else {
                        InvalidateRect(hwnd, &view.layout.visual, 0);
                    }
                }
                0
            }
            crate::runtime::FINISHED => {
                if view.runtime.as_ref().is_some_and(|r| r.cookie() == w) {
                    let restart = view.restart_pending && view.band_enabled;
                    view.restart_pending = false;
                    view.runtime.take();
                    if restart {
                        view.start_runtime();
                    }
                }
                0
            }
            WM_BAND_SHOWN => {
                view.band_enabled = w != 0;
                if view.band_enabled {
                    if view.runtime.is_none() {
                        view.start_runtime();
                    } else if view.runtime.as_ref().is_some_and(|r| r.is_stopping()) {
                        view.restart_pending = true;
                    }
                    view.visibility();
                    PostMessageW(hwnd, WM_BAND_RESIZE, 0, 0);
                } else {
                    view.restart_pending = false;
                    if let Some(runtime) = &view.runtime {
                        runtime.request_restart();
                    }
                    view.state = State::default();
                    view.cover.take();
                    view.buffer.take();
                    view.update_text();
                }
                0
            }
            0x0005 => {
                view.buffer.take();
                view.resize();
                view.visibility();
                if !view.standalone && rebar::dragging(hwnd) {
                    let mut rect = Rect::default();
                    GetClientRect(hwnd, &mut rect);
                    let width = ((rect.width() as u64 * 96 + view.dpi as u64 / 2) / view.dpi as u64) as u32;
                    if (160..=1200).contains(&width) && view.settings.width != width {
                        view.settings.width = width;
                        view.resize_save_pending = true;
                        if SetTimer(hwnd, SAVE_RESIZE_TIMER, 200, null_mut()) == 0 {
                            view.save_settings();
                        }
                    }
                }
                InvalidateRect(hwnd, null(), 0);
                0
            }
            0x0047 | 0x0018 | WM_REFRESH => {
                view.visibility();
                if !view.standalone {
                    let mut parent = Rect::default();
                    GetClientRect(GetParent(hwnd), &mut parent);
                    let size = (parent.width(), parent.height());
                    if view.host_size != size {
                        view.host_size = size;
                        PostMessageW(hwnd, crate::host::BAND_CHANGED, 0, 0);
                    }
                }
                drop(view);
                DefWindowProcW(hwnd, message, w, l)
            }
            0x02e0 => {
                view.style();
                view.buffer.take();
                if view.standalone {
                    let r = &*(l as *const Rect);
                    SetWindowPos(hwnd, null_mut(), r.left, r.top, r.width(), r.height(), 0x14);
                }
                view.resize();
                InvalidateRect(hwnd, null(), 0);
                0
            }
            0x02e3 => {
                view.style();
                view.resize();
                if !view.standalone {
                    PostMessageW(hwnd, WM_BAND_RESIZE, 0, 0);
                }
                InvalidateRect(hwnd, null(), 0);
                0
            }
            0x001a | 0x031a | 0x0015 => {
                view.style();
                view.update_text();
                InvalidateRect(hwnd, null(), 0);
                0
            }
            0x0218 => {
                if w == 4 {
                    view.suspended = true;
                } else if w == 7 || w == 18 {
                    view.suspended = false;
                }
                view.visibility();
                1
            }
            0x02b1 => {
                if matches!(w, 2 | 4 | 7) {
                    view.locked = true;
                } else if matches!(w, 1 | 3 | 8) {
                    view.locked = false;
                }
                view.visibility();
                0
            }
            0x0111 => {
                if (w >> 16) & 0xffff != 0 {
                    return 0;
                }
                let command = match w & 0xffff {
                    101 => Some(Command::Previous),
                    102 => Some(view.toggle()),
                    103 => Some(Command::Next),
                    _ => {
                        if l == 0 {
                            view.setting((w & 0xffff) as u32);
                        }
                        None
                    }
                };
                if let (Some(command), Some(runtime)) = (command, &view.runtime)
                    && let Err(e) = runtime.command(command)
                {
                    view.state.error = Some(e);
                    view.update_text();
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            0x002b => {
                view.draw_button(&*(l as *const DrawItem));
                1
            }
            0x0014 => 1,
            0x0113 if w == 7001 => {
                PostMessageW(hwnd, 0x10, 0, 0);
                0
            }
            0x0113 if w == SAVE_RESIZE_TIMER => {
                if rebar::dragging(hwnd) {
                    SetTimer(hwnd, SAVE_RESIZE_TIMER, 200, null_mut());
                } else {
                    KillTimer(hwnd, SAVE_RESIZE_TIMER);
                    if view.resize_save_pending {
                        view.save_settings();
                    }
                }
                0
            }
            0x000f => {
                view.paint();
                0
            }
            0x0318 => {
                let mut r = Rect::default();
                GetClientRect(hwnd, &mut r);
                let mut buffered = if view.composited { CompositedPaint::new(w as Raw, r) } else { None };
                if view.transparent()
                    && let Some(paint) = &mut buffered
                {
                    paint.clear_transparent();
                }
                view.scene(buffered.as_ref().map_or(w as Raw, |b| b.dc), r, buffered.as_ref());
                0
            }
            0x0024 if view.standalone => {
                let m = &mut *(l as *mut MinMax);
                m.min_track.x = (160 * view.dpi / 96) as i32;
                m.min_track.y = (75 * view.dpi / 96) as i32;
                0
            }
            _ => {
                drop(view);
                DefWindowProcW(hwnd, message, w, l)
            }
        }
    }
}

impl View {
    fn start_runtime(&mut self) {
        match Runtime::start(self.hwnd, &self.settings) {
            Ok(runtime) => {
                self.state = State::default();
                self.state.busy = true;
                runtime.configure(&self.settings, self.visible && self.layout.has_visual());
                self.runtime = Some(runtime);
            }
            Err(e) => {
                self.state.error = Some(e);
                self.state.busy = false;
            }
        }
        self.update_text();
        // SAFETY: asynchronous repaint of this owned view.
        unsafe {
            InvalidateRect(self.hwnd, null(), 0);
        }
    }
    fn focus_changed(
        site: Option<&crate::platform::com::Com>,
        identity: Option<&crate::platform::com::Com>,
        hwnd: Raw,
    ) {
        use crate::platform::ffi::{Guid, UnknownVtbl};
        #[repr(C)]
        struct InputSite {
            unknown: UnknownVtbl,
            changed: unsafe extern "system" fn(Raw, Raw, i32) -> i32,
        }
        if let Some(site) = site
            && let Ok(input) = site.query(&Guid::from_u128(0xf1db8392_7331_11d0_8c99_00a0c92dbfe8))
        {
            // SAFETY: retained IInputObjectSite and band identity; both belong to this apartment.
            unsafe {
                let focused = GetFocus();
                let has = focused == hwnd || IsChild(hwnd, focused) != 0;
                if let Some(identity) = identity {
                    (input.vt::<InputSite>().changed)(input.raw(), identity.raw(), i32::from(has));
                }
            }
        }
    }
    fn band_changed(site: Option<&crate::platform::com::Com>, id: u32) -> Result<()> {
        use crate::platform::ffi::{Guid, UnknownVtbl};
        #[repr(C)]
        struct Target {
            unknown: UnknownVtbl,
            _status: usize,
            exec: unsafe extern "system" fn(Raw, *const Guid, u32, u32, *const Variant, Raw) -> i32,
        }
        #[repr(C)]
        struct Variant {
            kind: u16,
            reserved: [u16; 3],
            value: [u64; 2],
        }
        if let Some(site) = site
            && let Ok(target) = site.query(&Guid::from_u128(0xb722bccb_4e68_101b_a2bc_00aa00404770))
        {
            let variant = Variant { kind: 3, reserved: [0; 3], value: [id as u64, 0] };
            // SAFETY: IOleCommandTarget::Exec with CGID_DeskBand, DBID_BANDINFOCHANGED and VT_I4 band id.
            unsafe {
                crate::platform::check(
                    (target.vt::<Target>().exec)(
                        target.raw(),
                        &Guid::from_u128(0xeb0fe172_1a3a_11d0_89b3_00a0c90a90ac),
                        0,
                        0,
                        &variant,
                        null_mut(),
                    ),
                    "update taskbar band information",
                )?;
            }
        }
        Ok(())
    }
    fn toggle(&self) -> Command {
        let Some(s) = &self.state.snapshot else {
            return Command::Toggle;
        };
        if s.controls.supports(Command::Toggle) {
            Command::Toggle
        } else if s.playback == Playback::Playing {
            Command::Pause
        } else {
            Command::Play
        }
    }
    fn style(&mut self) {
        // SAFETY: all settings/color queries are synchronous; only owned font is replaced.
        unsafe {
            self.dpi = GetDpiForWindow(self.hwnd).max(96);
            let mut contrast = HighContrast { size: size_of::<HighContrast>() as u32, flags: 0, scheme: null_mut() };
            SystemParametersInfoW(0x0042, contrast.size, (&mut contrast as *mut HighContrast).cast(), 0);
            self.contrast = contrast.flags & 1 != 0;
            let mut light = 1u32;
            let mut size = 4;
            RegGetValueW(
                (-2147483647isize) as Raw,
                wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
                wide(if self.standalone { "AppsUseLightTheme" } else { "SystemUsesLightTheme" }).as_ptr(),
                0x10,
                null_mut(),
                (&mut light as *mut u32).cast(),
                &mut size,
            );
            self.dark = match self.settings.theme {
                Theme::Dark => true,
                Theme::Light => false,
                Theme::System => light == 0,
            };
            let theme = LoadLibraryExW(wide("uxtheme.dll").as_ptr(), null_mut(), 0x800);
            if !theme.is_null() {
                let allow = GetProcAddress(theme, 133usize as *const u8);
                if !allow.is_null() {
                    // SAFETY: Windows 10 1809+ ordinal; optional, never required for rendering.
                    let allow: unsafe extern "system" fn(Raw, bool) -> bool = std::mem::transmute(allow);
                    allow(self.hwnd, self.dark && !self.contrast);
                }
                // PreferredAppMode is process-wide: an embedded band must not change Explorer's mode.
                if self.standalone {
                    let preferred = GetProcAddress(theme, 135usize as *const u8);
                    if !preferred.is_null() {
                        let preferred: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(preferred);
                        preferred(if self.contrast {
                            0
                        } else if self.dark {
                            2
                        } else {
                            3
                        });
                    }
                }
                FreeLibrary(theme);
            }
            let font = CreateFontW(
                -(((12 * self.dpi + 48) / 96) as i32),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                1,
                0,
                0,
                4,
                0,
                wide("Segoe UI").as_ptr(),
            );
            if !font.is_null() {
                if !self.font.is_null() {
                    DeleteObject(self.font);
                }
                self.font = font;
            }
            if self.standalone {
                let dark = i32::from(self.dark);
                DwmSetWindowAttribute(self.hwnd, 20, &dark, 4);
            }
        }
    }
    fn colors(&self) -> (u32, u32, u32, u32) {
        if self.contrast {
            // SAFETY: system COLORREF values in high contrast.
            unsafe { (GetSysColor(5), GetSysColor(8), GetSysColor(8), GetSysColor(13)) }
        } else if self.dark {
            (0x222020, 0xf8f6f3, 0xaaa7a1, self.settings.accent)
        } else {
            (0xf7f7f7, 0x242424, 0x686868, self.settings.accent)
        }
    }
    fn resize(&mut self) {
        let previous = self.layout;
        let mut rect = Rect::default();
        // SAFETY: this window and its native child controls are owned by the current thread.
        unsafe {
            GetClientRect(self.hwnd, &mut rect);
            let mut options = self.settings.clone();
            options.artwork &= self.state.artwork.is_some();
            self.layout = Layout::new(rect.width(), rect.height(), self.dpi, &options);
            let dc = GetDC(self.hwnd);
            if !dc.is_null() {
                let old = SelectObject(dc, self.font);
                let mut width = 0;
                for text in [&self.title, &self.artist] {
                    if text.len() < 2 {
                        continue;
                    }
                    let mut bounds = Rect::default();
                    DrawTextW(
                        dc,
                        text.as_ptr(),
                        text.len().saturating_sub(1) as i32,
                        &mut bounds,
                        0x400 | 0x20 | 0x800,
                    );
                    width = width.max(bounds.width());
                }
                SelectObject(dc, old);
                ReleaseDC(self.hwnd, dc);
                self.layout.fit_text(width + ((2 * self.dpi + 48) / 96) as i32, self.dpi);
            }
            let changed: [bool; 4] = std::array::from_fn(|i| {
                let hwnd = self.buttons[i];
                let r = self.layout.buttons[i];
                let visible = r.width() > 0 && r.height() > 0;
                previous.buttons[i] != r || (GetWindowLongPtrW(hwnd, -16) & 0x10000000 != 0) != visible
            });
            let count = changed.iter().filter(|&&v| v).count() as i32;
            let mut batch = if count > 0 { BeginDeferWindowPos(count) } else { null_mut() };
            for (i, (&hwnd, r)) in self.buttons.iter().zip(self.layout.buttons).enumerate() {
                if changed[i] && !batch.is_null() {
                    let flags = 0x011c | if r.width() > 0 && r.height() > 0 { 0x40 } else { 0x80 };
                    batch = DeferWindowPos(batch, hwnd, null_mut(), r.left, r.top, r.width(), r.height(), flags);
                }
            }
            let applied = !batch.is_null() && EndDeferWindowPos(batch) != 0;
            for (i, (&hwnd, r)) in self.buttons.iter().zip(self.layout.buttons).enumerate() {
                if changed[i] {
                    if !applied {
                        let flags = 0x011c | if r.width() > 0 && r.height() > 0 { 0x40 } else { 0x80 };
                        SetWindowPos(hwnd, null_mut(), r.left, r.top, r.width(), r.height(), flags);
                    }
                    InvalidateRect(hwnd, null(), 0);
                }
            }
            let focus = GetFocus();
            if IsChild(self.hwnd, focus) != 0 && IsWindowVisible(focus) == 0 {
                SetFocus(self.hwnd);
            }
        }
    }
    fn update_text(&mut self) {
        let t = lang::text(self.settings.language);
        let title = if self.state.error.is_some() {
            t.error
        } else if self.state.busy {
            t.waiting
        } else {
            self.state.snapshot.as_ref().map_or(t.empty, |s| s.title.as_str())
        };
        self.title = wide(title);
        self.artist = wide(if self.state.audio_error.is_some() {
            if self.settings.player_audio { t.player_audio_error } else { t.audio_error }
        } else if self.state.artwork_error.is_some() {
            t.artwork_error
        } else {
            self.state.snapshot.as_ref().map_or("", |s| s.artist.as_str())
        });
        self.caption = match self.state.error.or(self.state.audio_error).or(self.state.artwork_error) {
            Some(error) => format!("{title} ({:08X})", error.code as u32),
            None => format!("{} - {}", title, self.state.snapshot.as_ref().map_or("", |s| s.artist.as_str())),
        };
        let playing = self.state.snapshot.as_ref().is_some_and(|s| s.playback == Playback::Playing);
        let captions = [t.previous, if playing { t.pause } else { t.play }, t.next, t.options];
        let commands = [Command::Previous, self.toggle(), Command::Next];
        // SAFETY: native buttons expose captions and enabled state to keyboard/accessibility clients.
        unsafe {
            SetWindowTextW(self.hwnd, wide(&self.caption).as_ptr());
            for i in 0..4 {
                SetWindowTextW(self.buttons[i], wide(captions[i]).as_ptr());
                if i < 3 {
                    EnableWindow(
                        self.buttons[i],
                        i32::from(self.state.snapshot.as_ref().is_some_and(|s| s.controls.supports(commands[i]))),
                    );
                }
                InvalidateRect(self.buttons[i], null(), 0);
            }
        }
    }
    fn visibility(&mut self) {
        // SAFETY: visibility queries and monitor geometry contain no borrowed pointers after return.
        let visible = unsafe {
            let root = GetAncestor(self.hwnd, 2);
            let mut r = Rect::default();
            GetWindowRect(root, &mut r);
            let mut monitor = MonitorInfo {
                size: size_of::<MonitorInfo>() as u32,
                monitor: Rect::default(),
                work: Rect::default(),
                flags: 0,
            };
            GetMonitorInfoW(MonitorFromWindow(root, 2), &mut monitor);
            let overlap_x = (r.right.min(monitor.monitor.right) - r.left.max(monitor.monitor.left)).max(0);
            let overlap_y = (r.bottom.min(monitor.monitor.bottom) - r.top.max(monitor.monitor.top)).max(0);
            IsWindowVisible(self.hwnd) != 0
                && IsIconic(root) == 0
                && overlap_x >= r.width().min(8)
                && overlap_y >= r.height().min(8)
                && !self.suspended
                && !self.locked
        };
        self.visible = visible;
        if let Some(runtime) = &self.runtime {
            runtime.configure(&self.settings, visible && self.layout.has_visual());
        }
    }
    fn paint(&mut self) {
        // SAFETY: matching BeginPaint/EndPaint; GDI objects are restored before deletion.
        unsafe {
            let mut paint: Paint = std::mem::zeroed();
            let dc = BeginPaint(self.hwnd, &mut paint);
            if dc.is_null() {
                return;
            }
            let mut r = Rect::default();
            GetClientRect(self.hwnd, &mut r);
            match &self.state.artwork {
                Some(image)
                    if self.cover.as_ref().is_none_or(|(previous, _)| !std::sync::Arc::ptr_eq(image, previous)) =>
                {
                    self.cover = Buffer::new(dc, image.width as i32, image.height as i32).map(|buffer| {
                        std::ptr::copy_nonoverlapping(
                            image.pixels.as_ptr(),
                            buffer.bits.cast::<u8>(),
                            image.pixels.len(),
                        );
                        (image.clone(), buffer)
                    });
                }
                None => self.cover = None,
                _ => {}
            }
            let mut composed = if self.composited { CompositedPaint::new(dc, paint.rect) } else { None };
            if self.transparent()
                && let Some(buffer) = &mut composed
            {
                buffer.clear_transparent();
            }
            if composed.is_some() {
                self.buffer.take();
            } else if self.buffer.as_ref().is_none_or(|b| b.width != r.width() || b.height != r.height()) {
                self.buffer = Buffer::new(dc, r.width(), r.height());
            }
            let target = composed.as_ref().map_or_else(|| self.buffer.as_ref().map_or(dc, |b| b.dc), |b| b.dc);
            self.scene(target, paint.rect, composed.as_ref());
            if composed.is_none() && target != dc {
                BitBlt(
                    dc,
                    paint.rect.left,
                    paint.rect.top,
                    paint.rect.width(),
                    paint.rect.height(),
                    target,
                    paint.rect.left,
                    paint.rect.top,
                    0x00cc0020,
                );
            }
            drop(composed);
            EndPaint(self.hwnd, &paint);
        }
    }
    fn transparent(&self) -> bool {
        !self.standalone && !self.contrast && self.settings.transparent
    }
    fn background(&self, hwnd: Raw, dc: Raw, dirty: Rect, alpha: bool) {
        if alpha {
            return;
        }
        fill(dc, dirty, self.colors().0);
        if self.transparent() {
            // SAFETY: classic/non-composited fallback uses the actual container background.
            unsafe {
                DrawThemeParentBackground(hwnd, dc, &dirty);
            }
        }
    }
    fn scene(&self, dc: Raw, dirty: Rect, paint: Option<&CompositedPaint>) {
        let (_, foreground, muted, accent) = self.colors();
        let alpha = paint.is_some_and(|p| p.transparent);
        self.background(self.hwnd, dc, dirty, alpha);
        // SAFETY: local selections are restored; text uses terminated UTF-16 slices.
        unsafe {
            let old = SelectObject(dc, self.font);
            SetBkMode(dc, 1);
            if let Some((image, buffer)) = &self.cover
                && self.layout.artwork.intersects(dirty)
            {
                let area = self.layout.artwork;
                if area.width() > 0 && area.height() > 0 {
                    let source = artwork_crop(image.width as i32, image.height as i32, area);
                    AlphaBlend(
                        dc,
                        area.left,
                        area.top,
                        area.width(),
                        area.height(),
                        buffer.dc,
                        source.left,
                        source.top,
                        source.width(),
                        source.height(),
                        0x01ff0000,
                    );
                }
            }
            let text = self.layout.text;
            let two = text.height() >= (28 * self.dpi / 96) as i32;
            let mut title = text;
            if two {
                title.bottom = title.top + title.height() / 2;
            }
            if title.intersects(dirty) {
                label(dc, &self.title, title, if alpha { 0xffffff } else { foreground });
                if let Some(paint) = paint {
                    paint.colorize(title, foreground);
                }
            }
            if two && (Rect { top: title.bottom, ..text }).intersects(dirty) {
                let area = Rect { top: title.bottom, ..text };
                label(dc, &self.artist, area, if alpha { 0xffffff } else { muted });
                if let Some(paint) = paint {
                    paint.colorize(area, muted);
                }
            }
            let visual = self.layout.visual;
            if visual.width() > 0 && visual.height() > 0 && visual.intersects(dirty) {
                let old_pen = SelectObject(dc, GetStockObject(19));
                let draw_color = if alpha { 0xffffff } else { accent };
                SetDCPenColor(dc, draw_color);
                let gain = self.settings.gain as f32 / 25.;
                match self.settings.visual {
                    Visual::Wave if self.settings.solid => {
                        let points = solid_wave(&self.state.frame.wave, visual, gain);
                        let previous_pen = SelectObject(dc, GetStockObject(8));
                        let previous_brush = SelectObject(dc, GetStockObject(18));
                        SetDCBrushColor(dc, draw_color);
                        Polygon(dc, points.as_ptr(), points.len() as i32);
                        SelectObject(dc, previous_pen);
                        SelectObject(dc, previous_brush);
                    }
                    Visual::Wave => {
                        for (i, range) in self.state.frame.wave.iter().enumerate() {
                            let x = visual.left + (i as i32 * visual.width() / 64);
                            let center = (visual.top + visual.bottom) / 2;
                            let a = (visual.height() - 1) as f32 * 0.5;
                            let low = (range.low * gain).clamp(-1., 1.);
                            let high = (range.high * gain).clamp(-1., 1.);
                            MoveToEx(dc, x, center - (high * a) as i32, null_mut());
                            LineTo(dc, x, center - (low * a) as i32 + 1);
                        }
                    }
                    Visual::Spectrum => {
                        for (i, band) in self.state.frame.bands.iter().enumerate() {
                            let x = visual.left + i as i32 * visual.width() / 32;
                            let right = visual.left + (i as i32 + 1) * visual.width() / 32;
                            let height = (crate::visual::spectrum_height(*band, gain) * visual.height() as f32) as i32;
                            fill(
                                dc,
                                Rect {
                                    left: x,
                                    top: visual.bottom - height,
                                    right: (right - i32::from(!self.settings.solid)).max(x + 1).min(visual.right),
                                    bottom: visual.bottom,
                                },
                                draw_color,
                            );
                        }
                    }
                    Visual::Off => {}
                }
                SelectObject(dc, old_pen);
                if let Some(paint) = paint {
                    paint.colorize(visual, accent);
                }
            }
            SelectObject(dc, old);
        }
    }
    fn draw_button(&self, item: &DrawItem) {
        let mut buffered = if self.composited { CompositedPaint::new(item.dc, item.rect) } else { None };
        if self.transparent()
            && let Some(buffer) = &mut buffered
        {
            buffer.clear_transparent();
        }
        let item = &DrawItem { dc: buffered.as_ref().map_or(item.dc, |b| b.dc), ..*item };
        let (_, fg, muted, accent) = self.colors();
        let active = item.state & 1 != 0;
        let color = if item.state & 4 != 0 {
            muted
        } else if active {
            accent
        } else {
            fg
        };
        let alpha = buffered.as_ref().is_some_and(|p| p.transparent);
        self.background(item.hwnd, item.dc, item.rect, alpha);
        let actual_color = color;
        let color = if alpha { 0xffffff } else { color };
        let cx = (item.rect.left + item.rect.right) / 2;
        let cy = (item.rect.top + item.rect.bottom) / 2;
        let d = ((5 * self.dpi + 48) / 96) as i32;
        // SAFETY: owner-draw for real native buttons. Stock objects are restored, never deleted.
        unsafe {
            let old_pen = SelectObject(item.dc, GetStockObject(19));
            let old_brush = SelectObject(item.dc, GetStockObject(18));
            SetDCPenColor(item.dc, color);
            SetDCBrushColor(item.dc, color);
            let triangle = |a: Point, b: Point, c: Point| {
                Polygon(item.dc, [a, b, c].as_ptr(), 3);
            };
            match item.id {
                101 | 103 => {
                    let sign = if item.id == 101 { -1 } else { 1 };
                    triangle(
                        Point { x: cx - sign * d, y: cy - d },
                        Point { x: cx - sign * d, y: cy + d },
                        Point { x: cx + sign * (d - 1), y: cy },
                    );
                    fill(
                        item.dc,
                        Rect { left: cx + sign * d, top: cy - d, right: cx + sign * d + 2, bottom: cy + d + 1 },
                        color,
                    );
                }
                102 if self.state.snapshot.as_ref().is_some_and(|s| s.playback == Playback::Playing) => {
                    for x in [cx - d, cx + 1] {
                        fill(item.dc, Rect { left: x, top: cy - d, right: x + d - 1, bottom: cy + d }, color);
                    }
                }
                102 => triangle(
                    Point { x: cx - d + 1, y: cy - d },
                    Point { x: cx - d + 1, y: cy + d },
                    Point { x: cx + d, y: cy },
                ),
                104 => {
                    for x in [cx - d, cx, cx + d] {
                        fill(item.dc, Rect { left: x - 1, top: cy - 1, right: x + 1, bottom: cy + 1 }, color);
                    }
                }
                _ => {}
            }
            SelectObject(item.dc, old_pen);
            SelectObject(item.dc, old_brush);
            if item.state & 16 != 0 {
                let mut r = item.rect;
                r.left += 2;
                r.right -= 2;
                r.top += 2;
                r.bottom -= 2;
                DrawFocusRect(item.dc, &r);
            }
        }
        if let Some(paint) = &buffered {
            paint.colorize(item.rect, actual_color);
        }
    }
    fn setting(&mut self, id: u32) {
        let old = self.settings.clone();
        let width_requested = matches!(id, 260..=262);
        match id {
            200..=202 => self.settings.visual = [Visual::Off, Visual::Wave, Visual::Spectrum][(id - 200) as usize],
            210..=212 => self.settings.theme = [Theme::System, Theme::Light, Theme::Dark][(id - 210) as usize],
            220..=222 => {
                self.settings.language = [Language::System, Language::English, Language::Russian][(id - 220) as usize]
            }
            230 => self.settings.fps = 15,
            231 => self.settings.fps = 30,
            240..=244 => self.settings.gain = [100, 200, 400, 800, 1600][(id - 240) as usize],
            250..=252 => self.settings.release_ms = [80, 180, 400][(id - 250) as usize],
            260..=262 => self.settings.width = [280, 360, 480][(id - 260) as usize],
            270 => {
                // SAFETY: open the modal dialog after releasing this mutable View borrow.
                unsafe {
                    PostMessageW(self.hwnd, WM_PICK_COLOR, 0, 0);
                }
                return;
            }
            280 => self.settings.artwork = !self.settings.artwork,
            300 => self.settings.full_height = false,
            301 => self.settings.full_height = true,
            302 => self.settings.transparent = !self.settings.transparent,
            303 => self.settings.controls = !self.settings.controls,
            304 => self.settings.menu_button = !self.settings.menu_button,
            305 => self.settings.solid = !self.settings.solid,
            306 => self.settings.player_audio = true,
            307 => self.settings.player_audio = false,
            290 => {
                if self.runtime.as_ref().is_some_and(|r| r.wait_stopped(std::time::Duration::ZERO).unwrap_or(false)) {
                    self.runtime.take();
                    self.start_runtime();
                } else if let Some(runtime) = &self.runtime {
                    self.restart_pending = true;
                    runtime.request_restart();
                    self.state = State::default();
                    self.state.busy = true;
                    self.update_text();
                } else {
                    self.start_runtime();
                }
            }
            291 => {
                // SAFETY: close is delivered through the owning UI thread's message queue.
                unsafe {
                    PostMessageW(self.hwnd, 0x10, 0, 0);
                }
                return;
            }
            _ => return,
        }
        if self.settings != old {
            self.save_settings();
            if self.settings.theme != old.theme {
                self.style();
                // SAFETY: theme changes invalidate owner-drawn buttons without erasing them.
                unsafe {
                    for &hwnd in &self.buttons {
                        InvalidateRect(hwnd, null(), 0);
                    }
                }
            }
            let language_changed = self.settings.language != old.language;
            if language_changed {
                self.update_text();
            }
            if language_changed
                || self.settings.full_height != old.full_height
                || self.settings.artwork != old.artwork
                || self.settings.controls != old.controls
                || self.settings.menu_button != old.menu_button
            {
                self.resize();
            }
            self.visibility();
            if !width_requested && !self.standalone && self.settings.full_height != old.full_height {
                // SAFETY: notify outside the current View borrow.
                unsafe {
                    PostMessageW(self.hwnd, super::host::BAND_CHANGED, 0, 0);
                }
            }
        }
        if width_requested {
            // SAFETY: a repeated preset must apply too; notify outside the current View borrow.
            unsafe {
                PostMessageW(self.hwnd, WM_BAND_RESIZE, 0, 0);
            }
        }
        // SAFETY: repaint is scheduled; no frame timer is installed.
        unsafe {
            InvalidateRect(self.hwnd, null(), 0);
        }
    }
    fn save_settings(&mut self) {
        self.resize_save_pending = false;
        if let Err(error) = self.settings.save(&self.path) {
            let text = lang::text(self.settings.language);
            self.notice = Some(format!("{}\n{}", text.save_error, error));
            // SAFETY: defer modal dispatch until no mutable View borrow is held.
            unsafe {
                PostMessageW(self.hwnd, WM_NOTICE, 0, 0);
            }
        }
    }
}

pub(crate) fn band_preferences(hwnd: Raw) -> Option<(u32, bool)> {
    // SAFETY: called only with this implementation's live view HWND on its owner thread.
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, -21) as *const RefCell<View>;
        if pointer.is_null() {
            None
        } else {
            (*pointer).try_borrow().ok().map(|v| (v.settings.width, v.settings.full_height))
        }
    }
}
pub(crate) fn set_band_shown(hwnd: Raw, shown: bool) {
    // SAFETY: the shell's ShowDW controls this owned view; delivery never waits for COM/driver shutdown.
    unsafe {
        PostMessageW(hwnd, WM_BAND_SHOWN, usize::from(shown), 0);
    }
}
pub(crate) fn set_band_composition(hwnd: Raw, composited: bool) {
    if hwnd.is_null() {
        return;
    }
    // SAFETY: called synchronously on the band's apartment, without a retained View borrow.
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, -21) as *const RefCell<View>;
        if !pointer.is_null()
            && let Ok(mut view) = (*pointer).try_borrow_mut()
        {
            view.composited = composited;
            view.buffer.take();
            InvalidateRect(hwnd, null(), 0);
            for button in view.buttons {
                InvalidateRect(button, null(), 0);
            }
        }
    }
}
pub(crate) fn set_band_site(hwnd: Raw, site: Option<&crate::platform::com::Com>, id: u32) {
    // SAFETY: owner-thread access to the view stored in USERDATA.
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, -21) as *const RefCell<View>;
        if !pointer.is_null() {
            let mut view = (*pointer).borrow_mut();
            view.band_site = site.cloned();
            view.band_id = id;
        }
    }
}
pub(crate) fn set_band_identity(hwnd: Raw, identity: Raw) {
    // SAFETY: owner-thread assignment; it adds no COM reference or ownership cycle.
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, -21) as *const RefCell<View>;
        if !pointer.is_null()
            && let Ok(mut view) = (*pointer).try_borrow_mut()
        {
            view.band_identity = identity;
        }
    }
}
pub(crate) fn set_band_id(hwnd: Raw, id: u32) {
    // SAFETY: owner-thread access to this module's view, including reentrant band-info queries.
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, -21) as *const RefCell<View>;
        if !pointer.is_null()
            && let Ok(mut view) = (*pointer).try_borrow_mut()
        {
            view.band_id = id;
        }
    }
}
pub(crate) fn unregister_class() {
    if let Some(&module) = CLASS_MODULE.get() {
        // SAFETY: called by DllCanUnloadNow only when no window/callback activity remains.
        unsafe {
            UnregisterClassW(wide(CLASS).as_ptr(), module as Raw);
        }
    }
}

fn fill(dc: Raw, r: Rect, color: u32) {
    if r.width() == 0 || r.height() == 0 {
        return;
    }
    // SAFETY: stock brush color belongs to this DC; the brush itself is not deleted.
    unsafe {
        SetDCBrushColor(dc, color);
        FillRect(dc, &r, GetStockObject(18));
    }
}
fn label(dc: Raw, text: &[u16], mut r: Rect, color: u32) {
    if text.len() < 2 || r.width() == 0 || r.height() == 0 {
        return;
    }
    // SAFETY: text includes a trailing NUL, explicit count excludes it; RECT is writable.
    unsafe {
        SetTextColor(dc, color);
        DrawTextW(dc, text.as_ptr(), (text.len() - 1) as i32, &mut r, 0x20 | 0x4 | 0x8000 | 0x800);
    }
}

struct Menu(Raw);
impl Menu {
    fn new() -> Self {
        // SAFETY: owned popup handle, destroyed by Drop.
        Self(unsafe { CreatePopupMenu() })
    }
    fn item(&self, id: u32, text: &str, checked: bool) {
        // SAFETY: menu copies label immediately.
        unsafe {
            AppendMenuW(self.0, if checked { 8 } else { 0 }, id as usize, wide(text).as_ptr());
        }
    }
    fn submenu(&self, text: &str, menu: Menu) {
        // SAFETY: parent takes ownership only after successful append.
        if unsafe { AppendMenuW(self.0, 16, menu.0 as usize, wide(text).as_ptr()) } != 0 {
            std::mem::forget(menu);
        }
    }
}
impl Drop for Menu {
    fn drop(&mut self) {
        // SAFETY: owns this tree of menus.
        unsafe {
            DestroyMenu(self.0);
        }
    }
}
fn popup(hwnd: Raw, s: &Settings, point: Point, standalone: bool) -> u32 {
    let t = lang::text(s.language);
    let menu = Menu::new();
    let visual = Menu::new();
    for (i, label) in [t.off, t.wave, t.spectrum].iter().enumerate() {
        visual.item(200 + i as u32, label, s.visual as usize == i);
    }
    visual.item(305, t.solid, s.solid);
    menu.submenu(t.visual, visual);
    let audio = Menu::new();
    audio.item(306, t.player_audio, s.player_audio);
    audio.item(307, t.system_audio, !s.player_audio);
    menu.submenu(t.audio_source, audio);
    let theme = Menu::new();
    for (i, label) in [t.system, t.light, t.dark].iter().enumerate() {
        theme.item(210 + i as u32, label, s.theme as usize == i);
    }
    menu.submenu(t.theme, theme);
    let language = Menu::new();
    for (i, label) in [t.system, "English", "Русский"].iter().enumerate() {
        language.item(220 + i as u32, label, s.language as usize == i);
    }
    menu.submenu(t.language, language);
    let height = Menu::new();
    height.item(300, t.compact, !s.full_height);
    height.item(301, t.full_height, s.full_height);
    menu.submenu(t.height, height);
    if !standalone {
        menu.item(302, t.transparent, s.transparent);
    }
    menu.item(303, t.controls, s.controls);
    menu.item(304, t.menu_button, s.menu_button);
    for (name, base, values, current) in [
        ("FPS", 230, &[15, 30][..], s.fps),
        (t.amplitude, 240, &[100, 200, 400, 800, 1600][..], s.gain),
        (t.smoothing, 250, &[80, 180, 400][..], s.release_ms),
        (t.width, 260, &[280, 360, 480][..], s.width),
    ] {
        let sub = Menu::new();
        for (i, n) in values.iter().enumerate() {
            sub.item(
                base + i as u32,
                &format!(
                    "{n}{}",
                    if base == 240 {
                        "%"
                    } else if base == 250 {
                        " ms"
                    } else {
                        ""
                    }
                ),
                *n == current,
            );
        }
        menu.submenu(name, sub);
    }
    menu.item(270, &format!("{}...", t.color), false);
    menu.item(280, t.artwork, s.artwork);
    menu.item(290, t.reconnect, false);
    if standalone {
        menu.item(291, t.close, false);
    }
    // SAFETY: no View borrow is held during the nested menu message loop.
    unsafe { TrackPopupMenuEx(menu.0, 0x100 | 2, point.x, point.y, hwnd, null_mut()) }
}

#[repr(C)]
struct MonitorInfo {
    size: u32,
    monitor: Rect,
    work: Rect,
    flags: u32,
}
type WinEvent = unsafe extern "system" fn(Raw, u32, Raw, i32, i32, u32, u32);
unsafe extern "system" fn visibility_event(_: Raw, _: u32, hwnd: Raw, object: i32, _: i32, _: u32, _: u32) {
    let _ = std::panic::catch_unwind(|| {
        if object != 0 {
            return;
        }
        VIEWS.with(|v| {
            for &view in v.borrow().iter() {
                // SAFETY: out-of-context hooks execute on this UI thread; callbacks only post a refresh.
                unsafe {
                    if GetAncestor(view, 2) == hwnd {
                        PostMessageW(view, WM_REFRESH, 0, 0);
                    }
                }
            }
        });
    });
}
#[link(name = "user32")]
unsafe extern "system" {
    fn SetWinEventHook(
        min: u32,
        max: u32,
        module: Raw,
        callback: WinEvent,
        process: u32,
        thread: u32,
        flags: u32,
    ) -> Raw;
    fn UnhookWinEvent(hook: Raw) -> i32;
    fn GetWindowThreadProcessId(hwnd: Raw, process: *mut u32) -> u32;
    fn MonitorFromWindow(hwnd: Raw, flags: u32) -> Raw;
    fn GetMonitorInfoW(monitor: Raw, info: *mut MonitorInfo) -> i32;
}
#[link(name = "wtsapi32")]
unsafe extern "system" {
    fn WTSRegisterSessionNotification(hwnd: Raw, flags: u32) -> i32;
    fn WTSUnRegisterSessionNotification(hwnd: Raw) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn color_dialog_matches_the_windows_sdk_abi() {
        assert_eq!(size_of::<ChooseColor>(), if size_of::<usize>() == 8 { 72 } else { 36 });
        assert_eq!(std::mem::offset_of!(ChooseColor, custom), if size_of::<usize>() == 8 { 32 } else { 16 });
    }
    #[test]
    fn narrow_and_empty_layouts_disable_visualisation_at_every_dpi() {
        let settings = Settings::default();
        for dpi in [96, 120, 144, 192, 288] {
            let px = |n| n * dpi as i32 / 96;
            assert!(!Layout::new(px(160), px(40), dpi, &settings).has_visual());
            assert!(Layout::new(px(480), px(40), dpi, &settings).has_visual());
            assert!(!Layout::new(px(480), 0, dpi, &settings).has_visual());
            let disabled = Settings { visual: Visual::Off, ..settings.clone() };
            assert!(!Layout::new(px(480), px(40), dpi, &disabled).has_visual());
        }
    }
    #[test]
    fn solid_wave_forms_a_bounded_continuous_envelope() {
        let area = Rect { left: 13, top: 7, right: 201, bottom: 47 };
        let wave = [crate::visual::Range { low: -0.5, high: 0.75 }; crate::visual::POINTS];
        for gain in [0.0, 1.0, 64.0] {
            let points = solid_wave(&wave, area, gain);
            let n = crate::visual::POINTS;
            assert_eq!(points[0].x, area.left);
            assert_eq!(points[n - 1].x, area.right);
            assert_eq!(points[n].x, area.right);
            assert_eq!(points[n * 2 - 1].x, area.left);
            assert!(points[..n].windows(2).all(|p| p[0].x <= p[1].x));
            for (i, p) in points[..n].iter().enumerate() {
                let lower = points[n * 2 - 1 - i];
                assert_eq!(p.x, lower.x);
                assert!(p.x >= area.left && p.x <= area.right);
                assert!(p.y >= area.top && p.y < lower.y && lower.y <= area.bottom);
            }
        }
    }
    #[test]
    fn cover_fills_the_row_and_crops_without_distorting_the_source() {
        for dpi in [96, 144, 192] {
            let h = 60 * dpi / 96;
            for full_height in [false, true] {
                let options =
                    Settings { artwork: true, controls: false, menu_button: false, full_height, ..Settings::default() };
                let layout = Layout::new(480 * dpi / 96, h, dpi as u32, &options);
                assert_eq!(layout.artwork.top, 0);
                assert_eq!(layout.artwork.bottom, h);
                assert!(layout.text.left > layout.artwork.right);
                assert!(!layout.text.intersects(layout.visual));
                for (w, height) in [(150, 83), (83, 150), (256, 256), (1, 1)] {
                    let crop = artwork_crop(w, height, layout.artwork);
                    assert!(crop.left >= 0 && crop.top >= 0 && crop.right <= w && crop.bottom <= height);
                    assert!(crop.width() > 0 && crop.height() > 0);
                    let error = (crop.width() as i64 * layout.artwork.height() as i64
                        - crop.height() as i64 * layout.artwork.width() as i64)
                        .abs();
                    assert!(error <= layout.artwork.width().max(layout.artwork.height()) as i64);
                }
            }
        }
    }
    #[test]
    fn transparent_paint_keeps_holes_and_black_glyphs_with_nonzero_origin() {
        // SAFETY: private DIBs only; verify alpha, premultiplication and clipping off the desktop.
        unsafe {
            assert!(BufferedPaintInit() >= 0);
            let target = Buffer::new(null_mut(), 8, 8).unwrap();
            let rect = Rect { left: 2, top: 3, right: 7, bottom: 7 };
            let mut paint = CompositedPaint::new(target.dc, rect).unwrap();
            assert!(paint.clear_transparent());
            let glyph = Rect { left: 3, top: 4, right: 5, bottom: 6 };
            fill(paint.dc, glyph, 0xffffff);
            paint.colorize(glyph, 0);
            let antialias = Rect { left: 6, top: 3, right: 7, bottom: 4 };
            fill(paint.dc, antialias, 0x808080);
            paint.colorize(antialias, 0x004080ff);
            drop(paint);
            assert_ne!(GdiFlush(), 0);
            let pixels = std::slice::from_raw_parts(target.bits.cast::<u32>(), 64);
            for y in 0..8 {
                for x in 0..8 {
                    let expected = if (3..5).contains(&x) && (4..6).contains(&y) {
                        0xff000000
                    } else if x == 6 && y == 3 {
                        0x80804020
                    } else {
                        0
                    };
                    assert_eq!(pixels[y * 8 + x], expected, "{x},{y}");
                }
            }
            assert!(BufferedPaintUnInit() >= 0);
        }
    }
    #[test]
    fn full_height_and_hidden_controls_reclaim_the_container() {
        let mut settings =
            Settings { artwork: false, full_height: true, controls: false, menu_button: false, ..Settings::default() };
        let full = Layout::new(360, 160, 96, &settings);
        assert!(full.buttons.iter().all(|r| r.width() == 0 && r.height() == 0));
        assert_eq!(full.text.top, 4);
        assert_eq!(full.visual.top, 0);
        assert_eq!(full.visual.bottom, 160);
        assert_eq!(full.visual.right, 360);
        settings.controls = true;
        settings.menu_button = true;
        let controls = Layout::new(360, 160, 96, &settings);
        assert!(controls.buttons.iter().all(|r| r.top == 5 && r.height() == 30));
        assert!(full.text.width() + full.visual.width() > controls.text.width() + controls.visual.width());
        settings.full_height = false;
        let compact = Layout::new(360, 160, 96, &settings);
        assert_eq!(compact.visual.bottom, 40);
        assert_eq!(compact.text.top, 4);
    }
    #[test]
    fn composited_paint_preserves_color_and_sets_opaque_alpha() {
        // SAFETY: only private memory DCs and tiny DIBs; no desktop window or shell modification.
        unsafe {
            assert!(BufferedPaintInit() >= 0);
            let target = Buffer::new(null_mut(), 8, 8).unwrap();
            let rect = Rect { left: 0, top: 0, right: 8, bottom: 8 };
            let paint = CompositedPaint::new(target.dc, rect).unwrap();
            fill(paint.dc, rect, 0x00112233);
            drop(paint);
            assert_ne!(GdiFlush(), 0);
            for pixel in std::slice::from_raw_parts(target.bits.cast::<u8>(), 8 * 8 * 4).chunks_exact(4) {
                assert_eq!(pixel, &[0x11, 0x22, 0x33, 255]);
            }
            assert!(BufferedPaintUnInit() >= 0);
        }
    }
    #[test]
    #[ignore = "requires an interactive Windows desktop; never registers a band or changes playback"]
    fn native_view_resize_theme_and_shutdown() {
        // SAFETY: an isolated hidden native container in the test process, not Explorer.
        unsafe {
            let parent = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("Media view lifecycle check").as_ptr(),
                0,
                0,
                0,
                480,
                120,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            );
            assert!(!parent.is_null());
            let hwnd = create(parent).unwrap();
            let pointer = GetWindowLongPtrW(hwnd, -21) as *const RefCell<View>;
            (*pointer).borrow_mut().start_runtime();
            let done = (*pointer).borrow().runtime.as_ref().unwrap().done();
            for (w, h) in [(480, 40), (200, 120), (48, 180), (800, 120), (360, 40)] {
                assert_ne!(MoveWindow(hwnd, 0, 0, w, h, 0), 0);
                let view = (*pointer).borrow();
                for button in view.layout.buttons {
                    assert!(button.right <= w && button.bottom <= h);
                }
            }
            for width in [280, 480, 360] {
                {
                    let mut view = (*pointer).borrow_mut();
                    view.standalone = true;
                    view.settings.width = width;
                }
                // Exercise deferred window sizing without writing the user's settings.
                dispatch(hwnd, WM_BAND_RESIZE, 0, 0);
                let mut actual = Rect::default();
                GetClientRect(hwnd, &mut actual);
                let view = (*pointer).borrow();
                assert_eq!(actual.width(), (width * view.dpi / 96) as i32);
                assert_eq!(
                    view.layout.buttons[3].right,
                    actual.width(),
                    "WM_SIZE must update the grid before returning"
                );
            }
            (*pointer).borrow_mut().standalone = false;
            for theme in [Theme::Dark, Theme::Light, Theme::System] {
                let mut view = (*pointer).borrow_mut();
                view.settings.theme = theme;
                view.style();
            }
            // Paint the actual transparent scene and native button glyphs into an isolated DIB.
            set_band_composition(hwnd, true);
            let target = Buffer::new(null_mut(), 360, 120).unwrap();
            fill(target.dc, Rect { left: 0, top: 0, right: 30, bottom: 30 }, 0x123456);
            GdiFlush();
            let background = *target.bits.cast::<u32>();
            assert_ne!(background & 0xffffff, 0xffffff);
            assert_eq!(SendMessageW((*pointer).borrow().buttons[0], 0x0014, target.dc as usize, 0), 1);
            GdiFlush();
            assert_eq!(
                *target.bits.cast::<u32>(),
                background,
                "native button erase must not flash the system face colour"
            );
            {
                let mut view = (*pointer).borrow_mut();
                view.settings.full_height = true;
                view.settings.controls = false;
                view.settings.menu_button = false;
                view.settings.transparent = true;
                view.resize();
                assert!(view.layout.buttons.iter().all(|r| r.width() == 0));
                let mut paint =
                    CompositedPaint::new(target.dc, Rect { left: 0, top: 0, right: 360, bottom: 120 }).unwrap();
                assert!(paint.clear_transparent());
                view.scene(paint.dc, paint.rect, Some(&paint));
            }
            assert_ne!(GdiFlush(), 0);
            for pixel in std::slice::from_raw_parts(target.bits.cast::<u32>(), 360 * 120) {
                let alpha = pixel >> 24;
                assert!((pixel & 255) <= alpha && ((pixel >> 8) & 255) <= alpha && ((pixel >> 16) & 255) <= alpha);
            }
            {
                let view = (*pointer).borrow();
                for id in 101..=104 {
                    view.draw_button(&DrawItem {
                        kind: 4,
                        id,
                        item: 0,
                        action: 1,
                        state: 16,
                        hwnd: view.buttons[(id - 101) as usize],
                        dc: target.dc,
                        rect: Rect { left: 0, top: 0, right: 30, bottom: 30 },
                        data: 0,
                    });
                }
            }
            assert!(!(*pointer).borrow().visible);
            assert_ne!(DestroyWindow(hwnd), 0);
            assert_ne!(DestroyWindow(parent), 0);
            assert!(done.wait(5000).unwrap(), "engine must cancel and join audio before completing shutdown");
        }
    }
    #[test]
    fn layout_stays_inside_container_at_all_sizes_and_dpis() {
        for dpi in [96, 120, 144, 192, 288] {
            for w in [0, 1, 20, 40, 96, 160, 280, 360, 480, 1200] {
                for h in [0, 1, 24, 40, 64, 120, 600] {
                    for mode in 0..16 {
                        let settings = Settings {
                            artwork: mode & 1 != 0,
                            full_height: mode & 2 != 0,
                            controls: mode & 4 != 0,
                            menu_button: mode & 8 != 0,
                            ..Settings::default()
                        };
                        let l = Layout::new(w, h, dpi, &settings);
                        for r in [l.text, l.visual, l.artwork].into_iter().chain(l.buttons) {
                            assert!(
                                r.left >= 0
                                    && r.top >= 0
                                    && r.right <= w
                                    && r.bottom <= h
                                    && r.right >= r.left
                                    && r.bottom >= r.top,
                                "{w}x{h} {dpi}: {r:?}"
                            );
                        }
                        let areas =
                            [l.text, l.visual, l.artwork, l.buttons[0], l.buttons[1], l.buttons[2], l.buttons[3]];
                        for (i, a) in areas.iter().enumerate().filter(|(_, r)| r.width() > 0 && r.height() > 0) {
                            for b in areas[i + 1..].iter().filter(|r| r.width() > 0 && r.height() > 0) {
                                assert!(!a.intersects(*b), "overlap at {w}x{h} {dpi}: {a:?}, {b:?}");
                            }
                        }
                    }
                }
            }
        }
    }
}
