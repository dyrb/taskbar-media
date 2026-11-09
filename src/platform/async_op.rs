//! WinRT delegates and cancellable waits on engine threads, never on Explorer's UI thread.

use super::com::Com;
use super::event::{Event, Wake};
use super::ffi::*;
use super::{Error, Result, check};
use std::ptr::null_mut;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering, fence};
use std::time::Duration;

#[repr(C)]
struct CompletionVtbl {
    unknown: UnknownVtbl,
    invoke: unsafe extern "system" fn(Raw, Raw, i32) -> i32,
}

#[repr(C)]
struct Completion {
    vt: &'static CompletionVtbl,
    refs: AtomicU32,
    iid: Guid,
    ready: Arc<Event>,
    _activity: crate::platform::Activity,
}

static COMPLETION: CompletionVtbl = CompletionVtbl {
    unknown: UnknownVtbl { query: complete_query, add_ref: complete_add, release: complete_release },
    invoke: complete_invoke,
};

unsafe extern "system" fn complete_query(this: Raw, iid: *const Guid, out: *mut Raw) -> i32 {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM supplies this object's live pointer and writable output.
    unsafe {
        *out = null_mut();
        let object = &*(this as *const Completion);
        if *iid != object.iid && *iid != IID_IUNKNOWN && *iid != IID_IAGILE_OBJECT {
            return E_NOINTERFACE;
        }
        complete_add(this);
        *out = this;
    }
    0
}

unsafe extern "system" fn complete_add(this: Raw) -> u32 {
    // SAFETY: this belongs to Completion until its last Release.
    unsafe { &*(this as *const Completion) }.refs.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn complete_release(this: Raw) -> u32 {
    // SAFETY: the caller owns a reference to this Completion.
    let refs = unsafe { &*(this as *const Completion) }.refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        fence(Ordering::Acquire);
        // SAFETY: last reference; the allocation was made by Box::into_raw.
        drop(unsafe { Box::from_raw(this as *mut Completion) });
    }
    refs
}

unsafe extern "system" fn complete_invoke(this: Raw, _: Raw, _: i32) -> i32 {
    // SAFETY: WinRT retains the delegate throughout this callback.
    unsafe { &*(this as *const Completion) }.ready.set();
    0
}

#[repr(C)]
struct EventVtbl {
    unknown: UnknownVtbl,
    invoke: unsafe extern "system" fn(Raw, Raw, Raw) -> i32,
}

#[repr(C)]
struct Changed {
    vt: &'static EventVtbl,
    refs: AtomicU32,
    iid: Guid,
    wake: Wake,
    bits: u32,
    invalidated: Option<Arc<Event>>,
    _activity: crate::platform::Activity,
}

static CHANGED: EventVtbl = EventVtbl {
    unknown: UnknownVtbl { query: changed_query, add_ref: changed_add, release: changed_release },
    invoke: changed_invoke,
};

unsafe extern "system" fn changed_query(this: Raw, iid: *const Guid, out: *mut Raw) -> i32 {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM supplies the object's pointer and output; its fields are immutable/atomic.
    unsafe {
        *out = null_mut();
        let object = &*(this as *const Changed);
        if *iid != object.iid && *iid != IID_IUNKNOWN && *iid != IID_IAGILE_OBJECT {
            return E_NOINTERFACE;
        }
        changed_add(this);
        *out = this;
    }
    0
}

unsafe extern "system" fn changed_add(this: Raw) -> u32 {
    // SAFETY: this belongs to Changed until the last Release.
    unsafe { &*(this as *const Changed) }.refs.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn changed_release(this: Raw) -> u32 {
    // SAFETY: the caller owns a reference.
    let refs = unsafe { &*(this as *const Changed) }.refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        fence(Ordering::Acquire);
        // SAFETY: last reference to the Box allocation.
        drop(unsafe { Box::from_raw(this as *mut Changed) });
    }
    refs
}

unsafe extern "system" fn changed_invoke(this: Raw, _: Raw, _: Raw) -> i32 {
    // SAFETY: a live delegate retained for this callback. No user code executes here.
    let object = unsafe { &*(this as *const Changed) };
    if let Some(event) = &object.invalidated {
        event.set();
    }
    object.wake.signal(object.bits);
    0
}

pub type AddEvent = unsafe extern "system" fn(Raw, Raw, *mut i64) -> i32;
pub type RemoveEvent = unsafe extern "system" fn(Raw, i64) -> i32;

pub struct Subscription {
    source: Com,
    remove: RemoveEvent,
    token: i64,
    _delegate: Com,
}

impl Subscription {
    /// # Safety
    /// add/remove and iid must describe the same event on source's interface.
    pub unsafe fn new(
        source: &Com,
        add: AddEvent,
        remove: RemoveEvent,
        iid: Guid,
        wake: &Wake,
        bits: u32,
    ) -> Result<Self> {
        // SAFETY: the caller's matching event contract is forwarded unchanged.
        unsafe { Self::new_cancellable(source, add, remove, iid, wake, bits, None) }
    }

    /// # Safety
    /// add/remove/iid must describe the same source event; invalidated cancels in-flight metadata.
    pub unsafe fn new_cancellable(
        source: &Com,
        add: AddEvent,
        remove: RemoveEvent,
        iid: Guid,
        wake: &Wake,
        bits: u32,
        invalidated: Option<Arc<Event>>,
    ) -> Result<Self> {
        let raw = Box::into_raw(Box::new(Changed {
            vt: &CHANGED,
            refs: AtomicU32::new(1),
            iid,
            wake: wake.clone(),
            bits,
            invalidated,
            _activity: crate::platform::Activity::callback()?,
        }));
        // SAFETY: the allocation owns one reference and exposes EventVtbl.
        let delegate = unsafe { Com::owned(raw.cast(), "event delegate")? };
        let mut token = 0;
        // SAFETY: guaranteed matching event ABI by caller.
        check(unsafe { add(source.raw(), delegate.raw(), &mut token) }, "subscribe to media event")?;
        Ok(Self { source: source.clone(), remove, token, _delegate: delegate })
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // SAFETY: the source and delegate remain owned until after unregistration.
        // A disconnected provider may return an error; Drop must still release our references.
        unsafe { (self.remove)(self.source.raw(), self.token) };
    }
}

#[repr(C)]
struct OperationVtbl<T> {
    inspectable: InspectableVtbl,
    put_completed: unsafe extern "system" fn(Raw, Raw) -> i32,
    get_completed: unsafe extern "system" fn(Raw, *mut Raw) -> i32,
    get_results: unsafe extern "system" fn(Raw, *mut T) -> i32,
}

pub struct Operation<T> {
    operation: Com,
    info: Com,
    ready: Arc<Event>,
    _result: std::marker::PhantomData<T>,
}

impl<T: Copy + Default> Operation<T> {
    /// # Safety
    /// operation is IAsyncOperation<T>, and handler_iid is its exact completed delegate IID.
    pub unsafe fn new(operation: Com, handler_iid: Guid) -> Result<Self> {
        let info = operation.query(&IID_IASYNC_INFO)?;
        let ready = Arc::new(Event::new()?);
        let raw = Box::into_raw(Box::new(Completion {
            vt: &COMPLETION,
            refs: AtomicU32::new(1),
            iid: handler_iid,
            ready: ready.clone(),
            _activity: crate::platform::Activity::callback()?,
        }));
        // SAFETY: allocation owns one COM reference.
        let handler = unsafe { Com::owned(raw.cast(), "completion delegate")? };
        // SAFETY: caller guarantees the exact operation/delegate ABI. WinRT retains the handler.
        let hr = unsafe { (operation.vt::<OperationVtbl<T>>().put_completed)(operation.raw(), handler.raw()) };
        check(hr, "set completion handler")?;
        Ok(Self { operation, info, ready, _result: std::marker::PhantomData })
    }

    pub fn wait(&self, stop: &Event, timeout: Duration) -> Result<T> {
        self.wait_cancellable(stop, None, timeout)
    }

    pub(crate) fn wait_cancellable(&self, stop: &Event, invalidated: Option<&Event>, timeout: Duration) -> Result<T> {
        let handles = [stop.raw(), invalidated.map_or(self.ready.raw(), Event::raw), self.ready.raw()];
        let timeout = timeout.as_millis().min((INFINITE - 1) as u128) as u32;
        // SAFETY: both handles remain owned; this runs on an MTA engine thread.
        match unsafe { WaitForMultipleObjects(if invalidated.is_some() { 3 } else { 2 }, handles.as_ptr(), 0, timeout) }
        {
            0 => return Err(Error::new("operation cancelled", E_ABORT)),
            1 if invalidated.is_some() => return Err(Error::new("media request superseded", E_ABORT)),
            1 | 2 => {}
            258 => return Err(Error::new("media operation timed out", ERROR_TIMEOUT)),
            _ => return Err(Error::last("wait for media operation")),
        }
        let mut status = 0;
        // SAFETY: info is the IAsyncInfo returned by QueryInterface.
        unsafe {
            let vt = self.info.vt::<AsyncInfoVtbl>();
            check((vt.get_status)(self.info.raw(), &mut status), "async status")?;
            if status != 1 {
                let mut error = if status == 2 { E_ABORT } else { E_FAIL };
                if status == 3 {
                    check((vt.get_error)(self.info.raw(), &mut error), "async error")?;
                }
                return Err(Error::new("media operation", error));
            }
            let mut value = T::default();
            check(
                (self.operation.vt::<OperationVtbl<T>>().get_results)(self.operation.raw(), &mut value),
                "async result",
            )?;
            Ok(value)
        }
    }
}

impl<T> Drop for Operation<T> {
    fn drop(&mut self) {
        // SAFETY: IAsyncInfo is owned. Cancellation is non-blocking; late completion owns its
        // own Event reference, never a pointer to this Operation or the UI.
        unsafe {
            let vt = self.info.vt::<AsyncInfoVtbl>();
            let mut status = 0;
            if (vt.get_status)(self.info.raw(), &mut status) >= 0 && status == 0 {
                (vt.cancel)(self.info.raw());
            } else {
                (vt.close)(self.info.raw());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_delegate_preserves_identity_and_owns_late_callback_state() {
        let wake = Wake::new().unwrap();
        let invalidated = Arc::new(Event::new().unwrap());
        let raw = Box::into_raw(Box::new(Changed {
            vt: &CHANGED,
            refs: AtomicU32::new(1),
            iid: IID_IUNKNOWN,
            wake: wake.clone(),
            bits: 4,
            invalidated: Some(invalidated.clone()),
            _activity: crate::platform::Activity::new(),
        }));
        // SAFETY: this test exercises the real delegate's COM lifetime directly.
        unsafe {
            let delegate = Com::owned(raw.cast(), "delegate").unwrap();
            let agile = delegate.query(&IID_IAGILE_OBJECT).unwrap();
            assert!(delegate.same_object(&agile).unwrap());
            drop(delegate);
            changed_invoke(agile.raw(), null_mut(), null_mut());
            assert!(invalidated.wait(0).unwrap());
            assert_eq!(wake.take(), 4);
            assert!(agile.query(&Guid::from_u128(1)).is_err());
        }
    }
}
