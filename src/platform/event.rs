use super::ffi::*;
use super::{Error, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

pub const STOP: u32 = 1 << 31;

pub struct Event(Raw);

// SAFETY: Windows event handles may be signalled and waited on from different threads.
unsafe impl Send for Event {}
// SAFETY: event operations are synchronized by the kernel; CloseHandle runs at last ownership.
unsafe impl Sync for Event {}

impl Event {
    pub fn new() -> Result<Self> {
        Self::create(true)
    }

    pub fn auto_reset() -> Result<Self> {
        Self::create(false)
    }

    fn create(manual: bool) -> Result<Self> {
        // SAFETY: unnamed event with the requested reset mode and no initial signal.
        let raw = unsafe { CreateEventW(std::ptr::null_mut(), manual as i32, 0, std::ptr::null()) };
        if raw.is_null() { Err(Error::last("CreateEventW")) } else { Ok(Self(raw)) }
    }

    pub fn raw(&self) -> Raw {
        self.0
    }

    pub fn set(&self) {
        // SAFETY: the owned event remains open for the duration of this call.
        unsafe { SetEvent(self.0) };
    }

    pub fn reset(&self) {
        // SAFETY: the owned event remains open for the duration of this call.
        unsafe { ResetEvent(self.0) };
    }

    pub fn wait(&self, timeout: u32) -> Result<bool> {
        // SAFETY: a live event handle; no ownership is transferred.
        match unsafe { WaitForSingleObject(self.0, timeout) } {
            0 => Ok(true),
            258 => Ok(false),
            _ => Err(Error::last("WaitForSingleObject")),
        }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: this is the sole owner of the unnamed kernel handle.
        unsafe { CloseHandle(self.0) };
    }
}

struct State {
    event: Event,
    flags: AtomicU32,
}

#[derive(Clone)]
pub struct Wake(Arc<State>);

impl Wake {
    pub fn new() -> Result<Self> {
        Ok(Self(Arc::new(State { event: Event::new()?, flags: AtomicU32::new(0) })))
    }

    pub fn signal(&self, flags: u32) {
        if self.0.flags.fetch_or(flags, Ordering::Release) == 0 {
            self.0.event.set();
        }
    }

    pub fn take(&self) -> u32 {
        // Reset before consuming: a producer after the swap must leave the event signalled.
        self.0.event.reset();
        self.0.flags.swap(0, Ordering::Acquire)
    }

    pub fn flags(&self) -> u32 {
        self.0.flags.load(Ordering::Acquire)
    }

    pub fn raw(&self) -> Raw {
        self.0.event.raw()
    }

    pub fn wait(&self, timeout: u32) -> Result<bool> {
        self.0.event.wait(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_flags_are_coalesced_and_reset() {
        let wake = Wake::new().unwrap();
        wake.signal(1);
        wake.signal(2);
        wake.signal(1);
        assert!(wake.wait(0).unwrap());
        assert_eq!(wake.take(), 3);
        assert!(!wake.wait(0).unwrap());
        let producer = wake.clone();
        std::thread::spawn(move || producer.signal(STOP)).join().unwrap();
        assert!(wake.wait(1000).unwrap());
        assert_eq!(wake.take(), STOP);
    }
}
