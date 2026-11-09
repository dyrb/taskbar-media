//! Win32 ABI used by the native view and DeskBand host (Windows SDK 10.0.26100.0).
#![allow(non_snake_case)]
use crate::platform::ffi::Raw;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl Rect {
    pub fn intersects(self, other: Self) -> bool {
        self.left < other.right && self.right > other.left && self.top < other.bottom && self.bottom > other.top
    }
    pub fn width(self) -> i32 {
        (self.right - self.left).max(0)
    }
    pub fn height(self) -> i32 {
        (self.bottom - self.top).max(0)
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}
#[repr(C)]
pub struct Msg {
    pub hwnd: Raw,
    pub message: u32,
    pub wparam: usize,
    pub lparam: isize,
    pub time: u32,
    pub pt: Point,
    pub private: u32,
}
#[repr(C)]
pub struct WndClass {
    pub size: u32,
    pub style: u32,
    pub proc: unsafe extern "system" fn(Raw, u32, usize, isize) -> isize,
    pub class_extra: i32,
    pub window_extra: i32,
    pub instance: Raw,
    pub icon: Raw,
    pub cursor: Raw,
    pub background: Raw,
    pub menu: *const u16,
    pub name: *const u16,
    pub small_icon: Raw,
}
#[repr(C)]
pub struct Create {
    pub params: Raw,
    pub instance: Raw,
    pub menu: Raw,
    pub parent: Raw,
    pub cy: i32,
    pub cx: i32,
    pub y: i32,
    pub x: i32,
    pub style: i32,
    pub name: *const u16,
    pub class: *const u16,
    pub ex_style: u32,
}
#[repr(C)]
pub struct Paint {
    pub dc: Raw,
    pub erase: i32,
    pub rect: Rect,
    pub restore: i32,
    pub update: i32,
    pub reserved: [u8; 32],
}
#[repr(C)]
pub struct DrawItem {
    pub kind: u32,
    pub id: u32,
    pub item: u32,
    pub action: u32,
    pub state: u32,
    pub hwnd: Raw,
    pub dc: Raw,
    pub rect: Rect,
    pub data: usize,
}
#[repr(C)]
pub struct BitmapInfo {
    pub size: u32,
    pub width: i32,
    pub height: i32,
    pub planes: u16,
    pub bits: u16,
    pub compression: u32,
    pub image: u32,
    pub x: i32,
    pub y: i32,
    pub used: u32,
    pub important: u32,
    pub colors: [u32; 1],
}
#[repr(C)]
pub struct HighContrast {
    pub size: u32,
    pub flags: u32,
    pub scheme: *mut u16,
}
#[repr(C)]
pub struct ChooseColor {
    pub size: u32,
    pub owner: Raw,
    pub instance: Raw,
    pub result: u32,
    pub custom: *mut u32,
    pub flags: u32,
    pub data: isize,
    pub hook: Raw,
    pub template: *const u16,
}
#[link(name = "comdlg32")]
unsafe extern "system" {
    pub fn ChooseColorW(dialog: *mut ChooseColor) -> i32;
    pub fn CommDlgExtendedError() -> u32;
}
#[repr(C)]
pub struct MinMax {
    pub reserved: Point,
    pub max_size: Point,
    pub max_position: Point,
    pub min_track: Point,
    pub max_track: Point,
}

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
#[link(name = "user32")]
unsafe extern "system" {
    pub fn RegisterClassExW(class: *const WndClass) -> u16;
    pub fn UnregisterClassW(class: *const u16, instance: Raw) -> i32;
    pub fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Raw,
        menu: Raw,
        instance: Raw,
        param: Raw,
    ) -> Raw;
    pub fn DefWindowProcW(hwnd: Raw, msg: u32, w: usize, l: isize) -> isize;
    pub fn GetWindowLongPtrW(hwnd: Raw, index: i32) -> isize;
    pub fn SetWindowLongPtrW(hwnd: Raw, index: i32, value: isize) -> isize;
    pub fn DestroyWindow(hwnd: Raw) -> i32;
    pub fn ShowWindow(hwnd: Raw, show: i32) -> i32;
    #[cfg(test)]
    pub fn MoveWindow(hwnd: Raw, x: i32, y: i32, w: i32, h: i32, repaint: i32) -> i32;
    pub fn SetWindowPos(hwnd: Raw, after: Raw, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    pub fn BeginDeferWindowPos(count: i32) -> Raw;
    pub fn DeferWindowPos(batch: Raw, hwnd: Raw, after: Raw, x: i32, y: i32, w: i32, h: i32, flags: u32) -> Raw;
    pub fn EndDeferWindowPos(batch: Raw) -> i32;
    pub fn GetClientRect(hwnd: Raw, rect: *mut Rect) -> i32;
    pub fn GetDC(hwnd: Raw) -> Raw;
    pub fn ReleaseDC(hwnd: Raw, dc: Raw) -> i32;
    pub fn GetWindowRect(hwnd: Raw, rect: *mut Rect) -> i32;
    pub fn GetAncestor(hwnd: Raw, flags: u32) -> Raw;
    pub fn GetParent(hwnd: Raw) -> Raw;
    pub fn GetClassNameW(hwnd: Raw, text: *mut u16, count: i32) -> i32;
    pub fn SendMessageW(hwnd: Raw, message: u32, w: usize, l: isize) -> isize;
    pub fn GetCapture() -> Raw;
    pub fn GetWindow(hwnd: Raw, command: u32) -> Raw;
    pub fn IsWindowVisible(hwnd: Raw) -> i32;
    pub fn IsIconic(hwnd: Raw) -> i32;
    pub fn EnableWindow(hwnd: Raw, enabled: i32) -> i32;
    pub fn SetWindowTextW(hwnd: Raw, text: *const u16) -> i32;
    pub fn PostMessageW(hwnd: Raw, msg: u32, w: usize, l: isize) -> i32;
    pub fn GetMessageW(msg: *mut Msg, hwnd: Raw, min: u32, max: u32) -> i32;
    pub fn TranslateMessage(msg: *const Msg) -> i32;
    pub fn DispatchMessageW(msg: *const Msg) -> isize;
    pub fn IsDialogMessageW(hwnd: Raw, msg: *mut Msg) -> i32;
    pub fn PostQuitMessage(code: i32);
    pub fn BeginPaint(hwnd: Raw, paint: *mut Paint) -> Raw;
    pub fn EndPaint(hwnd: Raw, paint: *const Paint) -> i32;
    pub fn InvalidateRect(hwnd: Raw, rect: *const Rect, erase: i32) -> i32;
    pub fn FillRect(dc: Raw, rect: *const Rect, brush: Raw) -> i32;
    pub fn DrawTextW(dc: Raw, text: *const u16, count: i32, rect: *mut Rect, format: u32) -> i32;
    pub fn DrawFocusRect(dc: Raw, rect: *const Rect) -> i32;
    pub fn LoadCursorW(instance: Raw, name: *const u16) -> Raw;
    pub fn LoadIconW(instance: Raw, name: *const u16) -> Raw;
    pub fn GetDpiForWindow(hwnd: Raw) -> u32;
    pub fn SetProcessDpiAwarenessContext(context: isize) -> i32;
    pub fn GetSysColor(index: i32) -> u32;
    pub fn SystemParametersInfoW(action: u32, param: u32, value: Raw, flags: u32) -> i32;
    pub fn CreatePopupMenu() -> Raw;
    pub fn AppendMenuW(menu: Raw, flags: u32, id: usize, text: *const u16) -> i32;
    pub fn DestroyMenu(menu: Raw) -> i32;
    pub fn TrackPopupMenuEx(menu: Raw, flags: u32, x: i32, y: i32, hwnd: Raw, params: Raw) -> u32;
    pub fn GetCursorPos(point: *mut Point) -> i32;
    pub fn MessageBoxW(hwnd: Raw, text: *const u16, title: *const u16, kind: u32) -> i32;
    pub fn GetFocus() -> Raw;
    pub fn SetTimer(hwnd: Raw, id: usize, interval: u32, callback: Raw) -> usize;
    pub fn KillTimer(hwnd: Raw, id: usize) -> i32;
    pub fn RegisterSuspendResumeNotification(hwnd: Raw, flags: u32) -> Raw;
    pub fn UnregisterSuspendResumeNotification(handle: Raw) -> i32;
    pub fn SetFocus(hwnd: Raw) -> Raw;
    pub fn IsChild(parent: Raw, child: Raw) -> i32;
}
pub type SubclassProc = unsafe extern "system" fn(Raw, u32, usize, isize, usize, usize) -> isize;
#[link(name = "comctl32")]
unsafe extern "system" {
    pub fn SetWindowSubclass(hwnd: Raw, callback: SubclassProc, id: usize, data: usize) -> i32;
    pub fn RemoveWindowSubclass(hwnd: Raw, callback: SubclassProc, id: usize) -> i32;
    pub fn DefSubclassProc(hwnd: Raw, message: u32, w: usize, l: isize) -> isize;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    pub fn GdiFlush() -> i32;
    pub fn CreateCompatibleDC(dc: Raw) -> Raw;
    pub fn DeleteDC(dc: Raw) -> i32;
    pub fn CreateDIBSection(
        dc: Raw,
        info: *const BitmapInfo,
        usage: u32,
        bits: *mut Raw,
        section: Raw,
        offset: u32,
    ) -> Raw;
    pub fn DeleteObject(object: Raw) -> i32;
    pub fn SelectObject(dc: Raw, object: Raw) -> Raw;
    pub fn GetStockObject(index: i32) -> Raw;
    pub fn SetDCBrushColor(dc: Raw, color: u32) -> u32;
    pub fn SetDCPenColor(dc: Raw, color: u32) -> u32;
    pub fn SetTextColor(dc: Raw, color: u32) -> u32;
    pub fn SetBkMode(dc: Raw, mode: i32) -> i32;
    pub fn BitBlt(dst: Raw, x: i32, y: i32, w: i32, h: i32, src: Raw, sx: i32, sy: i32, rop: u32) -> i32;
    pub fn MoveToEx(dc: Raw, x: i32, y: i32, old: *mut Point) -> i32;
    pub fn LineTo(dc: Raw, x: i32, y: i32) -> i32;
    pub fn Polygon(dc: Raw, points: *const Point, count: i32) -> i32;
    pub fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike: u32,
        charset: u32,
        out: u32,
        clip: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> Raw;
}
#[link(name = "msimg32")]
unsafe extern "system" {
    pub fn AlphaBlend(
        dc: Raw,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: Raw,
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
        blend: u32,
    ) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut Raw) -> i32;
    pub fn GetUserDefaultUILanguage() -> u16;
    pub fn LoadLibraryExW(name: *const u16, file: Raw, flags: u32) -> Raw;
    pub fn GetProcAddress(module: Raw, name: *const u8) -> Raw;
    pub fn FreeLibrary(module: Raw) -> i32;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    pub fn RegGetValueW(
        key: Raw,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: Raw,
        size: *mut u32,
    ) -> i32;
    pub fn RegCreateKeyExW(
        key: Raw,
        subkey: *const u16,
        reserved: u32,
        class: *mut u16,
        options: u32,
        access: u32,
        attributes: Raw,
        out: *mut Raw,
        disposition: *mut u32,
    ) -> i32;
    pub fn RegSetValueExW(key: Raw, name: *const u16, reserved: u32, kind: u32, data: *const u8, size: u32) -> i32;
    pub fn RegDeleteTreeW(key: Raw, subkey: *const u16) -> i32;
    pub fn RegCloseKey(key: Raw) -> i32;
}
#[link(name = "dwmapi")]
unsafe extern "system" {
    pub fn DwmSetWindowAttribute(hwnd: Raw, attribute: u32, value: *const i32, size: u32) -> i32;
}
#[link(name = "uxtheme")]
unsafe extern "system" {
    pub fn BufferedPaintInit() -> i32;
    pub fn BufferedPaintUnInit() -> i32;
    pub fn BeginBufferedPaint(dc: Raw, rect: *const Rect, format: i32, params: Raw, out: *mut Raw) -> Raw;
    pub fn BufferedPaintSetAlpha(buffer: Raw, rect: *const Rect, alpha: u8) -> i32;
    pub fn BufferedPaintClear(buffer: Raw, rect: *const Rect) -> i32;
    pub fn GetBufferedPaintBits(buffer: Raw, bits: *mut Raw, stride: *mut i32) -> i32;
    pub fn DrawThemeParentBackground(hwnd: Raw, dc: Raw, rect: *const Rect) -> i32;
    pub fn EndBufferedPaint(buffer: Raw, update: i32) -> i32;
}
