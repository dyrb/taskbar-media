use super::ffi::*;
use super::{Error, Result, check};
use std::marker::PhantomData;
use std::ptr::{NonNull, null_mut};
use std::rc::Rc;

/// An owned interface restricted to its apartment. There is deliberately no Send/Sync.
pub struct Com {
    raw: NonNull<std::ffi::c_void>,
    _apartment: PhantomData<Rc<()>>,
}

impl Com {
    /// # Safety
    /// A non-null pointer must carry one owned COM reference.
    pub unsafe fn owned(raw: Raw, operation: &'static str) -> Result<Self> {
        let raw = NonNull::new(raw).ok_or(Error::new(operation, E_POINTER))?;
        Ok(Self { raw, _apartment: PhantomData })
    }

    pub fn raw(&self) -> Raw {
        self.raw.as_ptr()
    }

    /// # Safety
    /// The interface must expose precisely the vtable prefix T.
    pub unsafe fn vt<T>(&self) -> &T {
        // SAFETY: guaranteed by this function's caller.
        unsafe { &**(self.raw() as *const *const T) }
    }

    pub fn query(&self, iid: &Guid) -> Result<Self> {
        let mut result = null_mut();
        // SAFETY: every COM interface starts with IUnknown; result is writable.
        unsafe {
            check((self.vt::<UnknownVtbl>().query)(self.raw(), iid, &mut result), "QueryInterface")?;
            Self::owned(result, "QueryInterface")
        }
    }

    pub fn same_object(&self, other: &Self) -> Result<bool> {
        Ok(self.query(&IID_IUNKNOWN)?.raw() == other.query(&IID_IUNKNOWN)?.raw())
    }
}

impl Clone for Com {
    fn clone(&self) -> Self {
        // SAFETY: valid owned interface; AddRef creates the clone's reference.
        unsafe { (self.vt::<UnknownVtbl>().add_ref)(self.raw()) };
        Self { raw: self.raw, _apartment: PhantomData }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: this owner releases its one reference on the owning apartment.
        unsafe { (self.vt::<UnknownVtbl>().release)(self.raw()) };
    }
}

pub struct Apartment(PhantomData<Rc<()>>);

impl Apartment {
    pub fn sta() -> Result<Self> {
        // SAFETY: shell automation and native windows belong to the calling STA thread.
        check(unsafe { RoInitialize(0) }, "RoInitialize STA")?;
        Ok(Self(PhantomData))
    }
    pub fn mta() -> Result<Self> {
        // SAFETY: called on the engine's thread before any WinRT objects are used.
        check(unsafe { RoInitialize(1) }, "RoInitialize")?;
        Ok(Self(PhantomData))
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: successful RoInitialize, including S_FALSE, is balanced on this thread.
        unsafe { RoUninitialize() };
    }
}

pub struct HString(Raw);

impl HString {
    pub fn new(text: &str) -> Result<Self> {
        let text: Vec<u16> = text.encode_utf16().collect();
        let mut raw = null_mut();
        // SAFETY: Windows copies the supplied UTF-16 units.
        check(unsafe { WindowsCreateString(text.as_ptr(), text.len() as u32, &mut raw) }, "WindowsCreateString")?;
        Ok(Self(raw))
    }

    /// # Safety
    /// raw is null or an owned HSTRING returned by a Windows API.
    pub unsafe fn owned(raw: Raw) -> Self {
        Self(raw)
    }

    pub fn raw(&self) -> Raw {
        self.0
    }

    pub fn text(&self, limit: usize) -> String {
        let mut count = 0;
        // SAFETY: the buffer belongs to the HSTRING and remains valid until it is deleted.
        let raw = unsafe { WindowsGetStringRawBuffer(self.0, &mut count) };
        if count == 0 || raw.is_null() {
            return String::new();
        }
        // SAFETY: the API returned count readable units, truncated before allocation.
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw, (count as usize).min(limit)) })
    }
}

impl Drop for HString {
    fn drop(&mut self) {
        // SAFETY: WindowsDeleteString accepts null as an empty string.
        unsafe { WindowsDeleteString(self.0) };
    }
}
