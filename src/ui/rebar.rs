//! Resize only the native rebar band containing our own view, on its owning thread.
use super::GetWindowThreadProcessId;
use super::api::*;
use crate::platform::ffi::Raw;
use crate::platform::{Error, Result};

#[repr(C)]
#[derive(Default)]
struct BandInfo {
    size: u32,
    mask: u32,
    style: u32,
    foreground: u32,
    background: u32,
    text: *mut u16,
    text_len: u32,
    image: i32,
    child: Raw,
    min_width: u32,
    min_height: u32,
    width: u32,
    bitmap: Raw,
    id: u32,
    height: u32,
    max_height: u32,
    integral: u32,
    ideal: u32,
    param: isize,
    header: u32,
    chevron: Rect,
    chevron_state: u32,
}
const GET_INFO: u32 = 0x0400 + 28;
const SET_INFO: u32 = 0x0400 + 11;
const GET_COUNT: u32 = 0x0400 + 12;
const GET_RECT: u32 = 0x0400 + 9;
const CHILD: u32 = 0x10;
const SIZE: u32 = 0x40;
const IDEAL: u32 = 0x200;
const HEADER: u32 = 0x800;
const CHILD_SIZE: u32 = 0x20;

pub(super) fn parent(hwnd: Raw) -> Option<Raw> {
    // SAFETY: bounded parent traversal, no subclasses or changes to other bands.
    unsafe {
        let mut parent = GetParent(hwnd);
        for _ in 0..8 {
            if parent.is_null() {
                break;
            }
            let mut name = [0u16; 32];
            let n = GetClassNameW(parent, name.as_mut_ptr(), name.len() as i32);
            if n > 0 && name[..n as usize].iter().copied().eq("ReBarWindow32".encode_utf16()) {
                return Some(parent);
            }
            parent = GetParent(parent);
        }
    }
    None
}

pub(super) fn dragging(hwnd: Raw) -> bool {
    // SAFETY: capture query on this view's thread, with no retained application borrow.
    parent(hwnd).is_some_and(|p| unsafe { GetCapture() == p && GetWindowLongPtrW(p, -16) & 0x80 == 0 })
}

pub(super) fn set_size(hwnd: Raw, logical_width: Option<u32>) -> Result<()> {
    let parent = parent(hwnd).ok_or(Error::new("taskbar rebar is unavailable", 0x80004002u32 as i32))?;
    // SAFETY: stack pointers are passed only to our same-process, same-thread rebar.
    unsafe {
        let mut process = 0;
        if GetWindowThreadProcessId(parent, &mut process) != GetCurrentThreadId() || process != GetCurrentProcessId() {
            return Err(Error::new("taskbar container thread mismatch", 0x80004005u32 as i32));
        }
        let count = SendMessageW(parent, GET_COUNT, 0, 0);
        if !(0..=1024).contains(&count) {
            return Err(Error::new("invalid taskbar band count", 0x80004005u32 as i32));
        }
        for index in 0..count as usize {
            let mut info = BandInfo {
                size: std::mem::offset_of!(BandInfo, chevron) as u32,
                mask: CHILD | CHILD_SIZE | SIZE | HEADER,
                ..BandInfo::default()
            };
            if SendMessageW(parent, GET_INFO, index, (&mut info as *mut BandInfo) as isize) == 0 {
                return Err(Error::new("read taskbar band", 0x80004005u32 as i32));
            }
            if info.child != hwnd {
                continue;
            }
            let mut band = Rect::default();
            let mut client = Rect::default();
            GetClientRect(hwnd, &mut client);
            let vertical = GetWindowLongPtrW(parent, -16) & 0x80 != 0;
            let measured = SendMessageW(parent, GET_RECT, index, (&mut band as *mut Rect) as isize) != 0;
            let overhead = if measured && client.width() > 0 {
                if vertical { (band.height() - client.height()).max(0) } else { (band.width() - client.width()).max(0) }
            } else {
                info.header as i32
            };
            let mut container = Rect::default();
            let mut root = Rect::default();
            GetClientRect(parent, &mut container);
            GetClientRect(GetAncestor(hwnd, 2), &mut root);
            let cross =
                if vertical { container.width().min(root.width()) } else { container.height().min(root.height()) };
            let border = if measured {
                if vertical { (band.width() - client.width()).max(0) } else { (band.height() - client.height()).max(0) }
            } else {
                0
            };
            info.mask = CHILD_SIZE;
            info.height = (cross - border).max(1) as u32;
            info.min_height = info.height;
            info.max_height = info.height;
            info.integral = 1;
            if let Some(logical_width) = logical_width {
                let width = ((logical_width as u64 * GetDpiForWindow(hwnd).max(96) as u64 + 48) / 96) as u32;
                info.mask |= SIZE | IDEAL;
                info.width = width.saturating_add(overhead as u32);
                info.ideal = info.width;
            }
            if SendMessageW(parent, SET_INFO, index, (&mut info as *mut BandInfo) as isize) == 0 {
                return Err(Error::new("resize taskbar band", 0x80004005u32 as i32));
            }
            return Ok(());
        }
    }
    Err(Error::new("view is not attached to a taskbar band", 0x80004005u32 as i32))
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::null_mut;
    #[test]
    fn native_rebar_resizes_only_the_owning_band() {
        #[repr(C)]
        struct Init {
            size: u32,
            classes: u32,
        }
        #[link(name = "comctl32")]
        unsafe extern "system" {
            fn InitCommonControlsEx(init: *const Init) -> i32;
        }
        // SAFETY: real controls in an isolated hidden container, never the user's Explorer.
        unsafe {
            assert_ne!(InitCommonControlsEx(&Init { size: 8, classes: 0x400 }), 0);
            let owner = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("").as_ptr(),
                0x80000000,
                0,
                0,
                1200,
                80,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            );
            assert!(!owner.is_null());
            let bar = CreateWindowExW(
                0,
                wide("ReBarWindow32").as_ptr(),
                wide("").as_ptr(),
                0x50000000 | 0x4 | 0x8 | 0x40 | 0x200,
                0,
                0,
                1200,
                80,
                owner,
                null_mut(),
                null_mut(),
                null_mut(),
            );
            assert!(!bar.is_null());
            let mut children = [null_mut(); 2];
            for (i, child) in children.iter_mut().enumerate() {
                *child = CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    wide("").as_ptr(),
                    0x50000000,
                    0,
                    0,
                    160,
                    40,
                    bar,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                );
                assert!(!child.is_null());
                let mut info = BandInfo {
                    size: std::mem::offset_of!(BandInfo, chevron) as u32,
                    mask: CHILD | CHILD_SIZE | SIZE | IDEAL | 0x100 | 1,
                    child: *child,
                    min_width: 80,
                    min_height: 40,
                    width: 180,
                    ideal: 180,
                    height: 40,
                    max_height: 40,
                    integral: 1,
                    id: i as u32,
                    style: 0x80 | 0x40,
                    ..BandInfo::default()
                };
                assert_ne!(SendMessageW(bar, 0x400 + 10, i, (&mut info as *mut BandInfo) as isize), 0);
            }
            for desired in [280, 480, 360] {
                set_size(children[0], Some(desired)).unwrap();
                let mut actual = Rect::default();
                GetClientRect(children[0], &mut actual);
                assert_eq!(actual.width(), desired as i32);
                let mut other = BandInfo {
                    size: std::mem::offset_of!(BandInfo, chevron) as u32,
                    mask: IDEAL,
                    ..BandInfo::default()
                };
                assert_ne!(SendMessageW(bar, GET_INFO, 1, (&mut other as *mut BandInfo) as isize), 0);
                assert_eq!(other.ideal, 180);
            }
            for height in [80, 120, 40, 80] {
                assert_ne!(SetWindowPos(owner, null_mut(), 0, 0, 1200, height, 0x14), 0);
                assert_ne!(SetWindowPos(bar, null_mut(), 0, 0, 1200, height, 0x14), 0);
                for _ in 0..3 {
                    set_size(children[0], None).unwrap();
                    let mut actual = Rect::default();
                    GetClientRect(children[0], &mut actual);
                    assert_eq!(actual.height(), height, "row height must follow the container without feedback growth");
                }
            }
            assert_eq!(size_of::<BandInfo>(), 128);
            DestroyWindow(owner);
        }
    }
}
