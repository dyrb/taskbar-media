//! Owned Windows resources and the ABI used by the media and audio engines.

pub mod async_op;
pub mod com;
pub mod event;
pub mod ffi;

use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
static CALLBACKS: AtomicU32 = AtomicU32::new(0);
pub(crate) struct Activity;
impl Activity {
    pub(crate) fn new() -> Self {
        CALLBACKS.fetch_add(1, Ordering::Relaxed);
        Self
    }
    pub(crate) fn callback() -> Result<Self> {
        let mut count = CALLBACKS.load(Ordering::Acquire);
        while count < 128 {
            match CALLBACKS.compare_exchange_weak(count, count + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Ok(Self),
                Err(actual) => count = actual,
            }
        }
        Err(Error::new("too many outstanding system callbacks", ffi::E_BOUNDS))
    }
}
impl Drop for Activity {
    fn drop(&mut self) {
        CALLBACKS.fetch_sub(1, Ordering::Release);
    }
}
pub(crate) fn callbacks() -> u32 {
    CALLBACKS.load(Ordering::Acquire)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub operation: &'static str,
    pub code: i32,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub const fn new(operation: &'static str, code: i32) -> Self {
        Self { operation, code }
    }

    pub fn last(operation: &'static str) -> Self {
        // SAFETY: GetLastError has no pointer parameters.
        let code = unsafe { ffi::GetLastError() };
        Self::new(operation, (0x80070000 | (code & 0xffff)) as i32)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = [0u16; 1024];
        // SAFETY: the output array is writable for the supplied character count.
        let n = unsafe {
            ffi::FormatMessageW(
                0x1200,
                std::ptr::null(),
                self.code as u32,
                0,
                text.as_mut_ptr(),
                text.len() as u32,
                std::ptr::null(),
            )
        };
        write!(f, "{} (0x{:08X})", self.operation, self.code as u32)?;
        if n != 0 {
            write!(f, ": {}", String::from_utf16_lossy(&text[..n as usize]).trim())?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

pub(crate) fn check(code: i32, operation: &'static str) -> Result<()> {
    if code < 0 { Err(Error::new(operation, code)) } else { Ok(()) }
}
