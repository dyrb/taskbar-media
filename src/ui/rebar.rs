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

fn available_cross(hwnd: Raw, parent: Raw, index: usize, vertical: bool) -> i32 {
    // SAFETY: caller validated the rebar's owning thread; these are synchronous geometry queries.
    unsafe {
        let mut container = Rect::default();
        let mut root = Rect::default();
        GetClientRect(parent, &mut container);
        GetClientRect(GetAncestor(hwnd, 2), &mut root);
        let extent = if vertical { container.width().min(root.width()) } else { container.height().min(root.height()) };
        let row = SendMessageW(parent, 0x400 + 14, index, 0) as i32;
        if SendMessageW(parent, 0x400 + 13, 0, 0) > 1 && row > 0 { extent.min(row).max(1) } else { extent.max(1) }
    }
}

pub(super) fn row_extent(hwnd: Raw, vertical: bool) -> Option<i32> {
    let parent = parent(hwnd)?;
    // SAFETY: never send pointer messages across a process or thread boundary.
    unsafe {
        let mut process = 0;
        if GetWindowThreadProcessId(parent, &mut process) != GetCurrentThreadId() || process != GetCurrentProcessId() {
            return None;
        }
        let count = SendMessageW(parent, GET_COUNT, 0, 0);
        if !(1..=1024).contains(&count) {
            return None;
        }
        for index in 0..count as usize {
            let mut info =
                BandInfo { size: std::mem::offset_of!(BandInfo, chevron) as u32, mask: CHILD, ..BandInfo::default() };
            if SendMessageW(parent, GET_INFO, index, (&mut info as *mut BandInfo) as isize) == 0 {
                return None;
            }
            if info.child == hwnd {
                return Some(available_cross(hwnd, parent, index, vertical));
            }
        }
    }
    None
}

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

/// Move only our band through the rebar's documented ordering message, never screen coordinates.
pub(super) fn move_band(hwnd: Raw, direction: u32, logical_width: u32) -> Result<()> {
    let parent = parent(hwnd).ok_or(Error::new("taskbar rebar is unavailable", 0x80004002u32 as i32))?;
    // SAFETY: validate the same process and thread before passing stack buffers to the container.
    unsafe {
        let mut process = 0;
        if GetWindowThreadProcessId(parent, &mut process) != GetCurrentThreadId() || process != GetCurrentProcessId() {
            return Err(Error::new("taskbar container thread mismatch", 0x80004005u32 as i32));
        }
        let count = SendMessageW(parent, GET_COUNT, 0, 0);
        if !(1..=1024).contains(&count) {
            return Err(Error::new("invalid taskbar band count", 0x80004005u32 as i32));
        }
        for index in 0..count as usize {
            let mut info = BandInfo {
                size: std::mem::offset_of!(BandInfo, chevron) as u32,
                mask: CHILD | 1,
                ..BandInfo::default()
            };
            if SendMessageW(parent, GET_INFO, index, (&mut info as *mut BandInfo) as isize) == 0 {
                return Err(Error::new("read taskbar band", 0x80004005u32 as i32));
            }
            if info.child != hwnd {
                continue;
            }
            if info.style & 0x100 != 0 || GetWindowLongPtrW(parent, -16) & 0x800 != 0 {
                return Err(Error::new("taskbar bands are locked", 0x80070005u32 as i32));
            }
            let target = match direction {
                0 => 0,
                1 => index.saturating_sub(1),
                2 => (index + 1).min(count as usize - 1),
                _ => count as usize - 1,
            };
            if target == index {
                return Ok(());
            }
            // A row break belongs to the old position. Carrying it to the new index creates an
            // extra row even when the destination has room for this band.
            if info.style & 1 != 0 {
                info.mask = 1;
                info.style &= !1;
                if SendMessageW(parent, SET_INFO, index, (&mut info as *mut BandInfo) as isize) == 0 {
                    return Err(Error::new("clear taskbar band row break", 0x80004005u32 as i32));
                }
            }
            if SendMessageW(parent, 0x400 + 39, index, target as isize) == 0 {
                return Err(Error::new("move taskbar band", 0x80004005u32 as i32));
            }
            // The last band can occupy all spare space. Do not carry that stretched width to
            // the beginning of the row, where it would push the application band to another row.
            set_size(hwnd, Some(logical_width))?;
            #[repr(C)]
            struct Header {
                hwnd: Raw,
                id: usize,
                code: u32,
            }
            let owner = GetParent(parent);
            if !owner.is_null()
                && GetWindowThreadProcessId(owner, &mut process) == GetCurrentThreadId()
                && process == GetCurrentProcessId()
            {
                let header =
                    Header { hwnd: parent, id: GetWindowLongPtrW(parent, -12) as usize, code: (-833i32) as u32 };
                // Programmatic reordering must also tell the shell to reconcile/save its layout.
                SendMessageW(owner, 0x004e, header.id, (&header as *const Header) as isize);
            }
            let mut actual =
                BandInfo { size: std::mem::offset_of!(BandInfo, chevron) as u32, mask: CHILD, ..BandInfo::default() };
            if SendMessageW(parent, GET_INFO, target, (&mut actual as *mut BandInfo) as isize) == 0
                || actual.child != hwnd
            {
                return Err(Error::new("taskbar host rejected band position", 0x80004005u32 as i32));
            }
            return Ok(());
        }
    }
    Err(Error::new("view is not attached to a taskbar band", 0x80004005u32 as i32))
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
            let cross = available_cross(hwnd, parent, index, vertical);
            let border = if measured {
                if vertical { (band.width() - client.width()).max(0) } else { (band.height() - client.height()).max(0) }
            } else {
                0
            };
            info.mask = CHILD_SIZE;
            info.height = (cross - border).max(1) as u32;
            info.min_height = info.height.min((32 * GetDpiForWindow(hwnd).max(96) + 48) / 96);
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
            let mut children = [null_mut(); 3];
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
            let mut wrap = BandInfo {
                size: std::mem::offset_of!(BandInfo, chevron) as u32,
                mask: 1,
                style: 0x80 | 0x40 | 1,
                ..BandInfo::default()
            };
            assert_ne!(SendMessageW(bar, SET_INFO, 1, (&mut wrap as *mut BandInfo) as isize), 0);
            SetWindowPos(owner, null_mut(), 0, 0, 1200, 240, 0x14);
            SetWindowPos(bar, null_mut(), 0, 0, 1200, 240, 0x14);
            assert!(SendMessageW(bar, 0x400 + 13, 0, 0) > 1);
            let row_height = SendMessageW(bar, 0x400 + 14, 0, 0) as i32;
            for _ in 0..4 {
                set_size(children[0], None).unwrap();
                let mut actual = Rect::default();
                GetClientRect(children[0], &mut actual);
                assert!(actual.height() <= row_height, "another row must not increase this band's required height");
            }
            move_band(children[0], 3, 360).unwrap();
            let mut moved =
                BandInfo { size: std::mem::offset_of!(BandInfo, chevron) as u32, mask: CHILD, ..BandInfo::default() };
            assert_ne!(SendMessageW(bar, GET_INFO, 2, (&mut moved as *mut BandInfo) as isize), 0);
            assert_eq!(moved.child, children[0]);
            move_band(children[0], 1, 360).unwrap();
            assert_ne!(SendMessageW(bar, GET_INFO, 1, (&mut moved as *mut BandInfo) as isize), 0);
            assert_eq!(moved.child, children[0]);
            move_band(children[0], 2, 360).unwrap();
            assert_ne!(SendMessageW(bar, GET_INFO, 2, (&mut moved as *mut BandInfo) as isize), 0);
            assert_eq!(moved.child, children[0]);
            move_band(children[0], 0, 360).unwrap();
            assert_ne!(SendMessageW(bar, GET_INFO, 0, (&mut moved as *mut BandInfo) as isize), 0);
            assert_eq!(moved.child, children[0]);
            wrap.style = 0x80 | 0x40 | 1;
            assert_ne!(SendMessageW(bar, SET_INFO, 0, (&mut wrap as *mut BandInfo) as isize), 0);
            move_band(children[0], 3, 360).unwrap();
            moved.mask = CHILD | 1;
            assert_ne!(SendMessageW(bar, GET_INFO, 2, (&mut moved as *mut BandInfo) as isize), 0);
            assert_eq!(moved.style & 1, 0, "an explicit move must not carry the old row break");
            wrap.style |= 0x100;
            assert_ne!(SendMessageW(bar, SET_INFO, 2, (&mut wrap as *mut BandInfo) as isize), 0);
            assert_eq!(move_band(children[0], 0, 360).unwrap_err().code, 0x80070005u32 as i32);
            DestroyWindow(owner);
        }
    }
}
