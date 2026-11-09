//! Signatures checked against Windows SDK 10.0.26100.0.

use std::ffi::c_void;

pub type Raw = *mut c_void;
pub type HResult = i32;

pub const E_POINTER: i32 = 0x80004003u32 as i32;
pub const E_NOINTERFACE: i32 = 0x80004002u32 as i32;
pub const E_ABORT: i32 = 0x80004004u32 as i32;
pub const E_FAIL: i32 = 0x80004005u32 as i32;
pub const E_BOUNDS: i32 = 0x8000000bu32 as i32;
pub const E_INVALIDARG: i32 = 0x80070057u32 as i32;
pub const ERROR_TIMEOUT: i32 = 0x800705b4u32 as i32;
pub const INFINITE: u32 = u32::MAX;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const fn from_u128(value: u128) -> Self {
        Self {
            data1: (value >> 96) as u32,
            data2: (value >> 80) as u16,
            data3: (value >> 64) as u16,
            data4: (value as u64).to_be_bytes(),
        }
    }
}

pub const IID_IUNKNOWN: Guid = Guid::from_u128(0x00000000_0000_0000_c000_000000000046);
pub const IID_IAGILE_OBJECT: Guid = Guid::from_u128(0x94ea2b94_e9cc_49e0_c0ff_ee64ca8f5b90);
pub const IID_IASYNC_INFO: Guid = Guid::from_u128(0x00000036_0000_0000_c000_000000000046);

#[repr(C)]
pub struct UnknownVtbl {
    pub query: unsafe extern "system" fn(Raw, *const Guid, *mut Raw) -> HResult,
    pub add_ref: unsafe extern "system" fn(Raw) -> u32,
    pub release: unsafe extern "system" fn(Raw) -> u32,
}

#[repr(C)]
pub struct InspectableVtbl {
    pub unknown: UnknownVtbl,
    pub get_iids: unsafe extern "system" fn(Raw, *mut u32, *mut *mut Guid) -> HResult,
    pub get_class_name: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
    pub get_trust_level: unsafe extern "system" fn(Raw, *mut i32) -> HResult,
}

#[repr(C)]
pub struct AsyncInfoVtbl {
    pub inspectable: InspectableVtbl,
    pub get_id: unsafe extern "system" fn(Raw, *mut u32) -> HResult,
    pub get_status: unsafe extern "system" fn(Raw, *mut i32) -> HResult,
    pub get_error: unsafe extern "system" fn(Raw, *mut i32) -> HResult,
    pub cancel: unsafe extern "system" fn(Raw) -> HResult,
    pub close: unsafe extern "system" fn(Raw) -> HResult,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetLastError() -> u32;
    pub fn CloseHandle(handle: Raw) -> i32;
    pub fn CreateEventW(attributes: Raw, manual: i32, initial: i32, name: *const u16) -> Raw;
    pub fn SetEvent(handle: Raw) -> i32;
    pub fn ResetEvent(handle: Raw) -> i32;
    pub fn WaitForSingleObject(handle: Raw, milliseconds: u32) -> u32;
    pub fn WaitForMultipleObjects(count: u32, handles: *const Raw, all: i32, milliseconds: u32) -> u32;
    pub fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
    pub fn FormatMessageW(
        flags: u32,
        source: *const c_void,
        id: u32,
        language: u32,
        buffer: *mut u16,
        size: u32,
        arguments: *const c_void,
    ) -> u32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    pub fn CoCreateInstance(class: *const Guid, outer: Raw, context: u32, iid: *const Guid, out: *mut Raw) -> i32;
    pub fn CoTaskMemFree(memory: *const c_void);
}

#[link(name = "runtimeobject")]
unsafe extern "system" {
    pub fn RoInitialize(kind: u32) -> i32;
    pub fn RoUninitialize();
    pub fn RoGetActivationFactory(name: Raw, iid: *const Guid, out: *mut Raw) -> i32;
    pub fn WindowsCreateString(text: *const u16, length: u32, out: *mut Raw) -> i32;
    pub fn WindowsDeleteString(text: Raw) -> i32;
    pub fn WindowsGetStringRawBuffer(text: Raw, length: *mut u32) -> *const u16;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_abi_layouts() {
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(align_of::<Guid>(), 4);
        assert_eq!(size_of::<UnknownVtbl>(), 3 * size_of::<Raw>());
        assert_eq!(size_of::<InspectableVtbl>(), 6 * size_of::<Raw>());
        assert_eq!(
            Guid::from_u128(0x12345678_1234_abcd_8123_1234567890ab).data4,
            [0x81, 0x23, 0x12, 0x34, 0x56, 0x78, 0x90, 0xab]
        );
    }
}
