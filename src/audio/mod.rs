//! Event-driven shared-mode WASAPI loopback. Endpoint and capture COM objects stay on this thread.

use crate::platform::com::{Apartment, Com};
use crate::platform::event::{Event, Wake};
use crate::platform::ffi::*;
use crate::platform::{Error, Result, check};
use crate::visual::{Analyzer, Format, Frame, Smoothing};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering, fence};
use std::time::{Duration, Instant};
mod process;

const ENUMERATOR_CLASS: Guid = Guid::from_u128(0xbcde0395_e52f_467c_8e3d_c4579291692e);
const ENUMERATOR: Guid = Guid::from_u128(0xa95664d2_9614_4f35_a746_de8db63617e6);
const NOTIFICATION: Guid = Guid::from_u128(0x7991eec9_7e89_4d85_8390_6c703cec60c0);
const AUDIO_CLIENT: Guid = Guid::from_u128(0x1cb9ad4c_dbfa_4c32_b178_c2f568a703b2);
const CAPTURE_CLIENT: Guid = Guid::from_u128(0xc8adbd64_e71e_48a0_a4de_185c395cd317);
const DEVICE_CHANGED: u32 = 1;
const RETRY: u32 = 2;

#[repr(C)]
struct EnumeratorVtbl {
    unknown: UnknownVtbl,
    enumerate: unsafe extern "system" fn(Raw, i32, u32, *mut Raw) -> i32,
    default: unsafe extern "system" fn(Raw, i32, i32, *mut Raw) -> i32,
    device: unsafe extern "system" fn(Raw, *const u16, *mut Raw) -> i32,
    register: unsafe extern "system" fn(Raw, Raw) -> i32,
    unregister: unsafe extern "system" fn(Raw, Raw) -> i32,
}

#[repr(C)]
struct CollectionVtbl {
    unknown: UnknownVtbl,
    count: unsafe extern "system" fn(Raw, *mut u32) -> i32,
    item: unsafe extern "system" fn(Raw, u32, *mut Raw) -> i32,
}

#[repr(C)]
struct DeviceVtbl {
    unknown: UnknownVtbl,
    activate: unsafe extern "system" fn(Raw, *const Guid, u32, Raw, *mut Raw) -> i32,
    _property_store: usize,
    id: unsafe extern "system" fn(Raw, *mut *mut u16) -> i32,
    _state: usize,
}

#[repr(C)]
struct AudioVtbl {
    unknown: UnknownVtbl,
    initialize: unsafe extern "system" fn(Raw, i32, u32, i64, i64, Raw, *const Guid) -> i32,
    buffer_size: unsafe extern "system" fn(Raw, *mut u32) -> i32,
    _latency_padding_support: [usize; 3],
    mix_format: unsafe extern "system" fn(Raw, *mut Raw) -> i32,
    _period: usize,
    start: unsafe extern "system" fn(Raw) -> i32,
    stop: unsafe extern "system" fn(Raw) -> i32,
    _reset: usize,
    set_event: unsafe extern "system" fn(Raw, Raw) -> i32,
    service: unsafe extern "system" fn(Raw, *const Guid, *mut Raw) -> i32,
}

#[repr(C)]
struct CaptureVtbl {
    unknown: UnknownVtbl,
    get: unsafe extern "system" fn(Raw, *mut *mut u8, *mut u32, *mut u32, *mut u64, *mut u64) -> i32,
    release: unsafe extern "system" fn(Raw, u32) -> i32,
    next: unsafe extern "system" fn(Raw, *mut u32) -> i32,
}

#[repr(C)]
struct PropertyKey {
    format: Guid,
    id: u32,
}

#[repr(C)]
struct NotifyVtbl {
    unknown: UnknownVtbl,
    state: unsafe extern "system" fn(Raw, *const u16, u32) -> i32,
    added: unsafe extern "system" fn(Raw, *const u16) -> i32,
    removed: unsafe extern "system" fn(Raw, *const u16) -> i32,
    default: unsafe extern "system" fn(Raw, i32, i32, *const u16) -> i32,
    property: unsafe extern "system" fn(Raw, *const u16, PropertyKey) -> i32,
}

#[repr(C)]
struct Notify {
    vt: &'static NotifyVtbl,
    refs: AtomicU32,
    wake: Wake,
    _activity: crate::platform::Activity,
}

static NOTIFY: NotifyVtbl = NotifyVtbl {
    unknown: UnknownVtbl { query: notify_query, add_ref: notify_add, release: notify_release },
    state: notify_state,
    added: notify_device,
    removed: notify_device,
    default: notify_default,
    property: notify_property,
};

unsafe extern "system" fn notify_query(this: Raw, iid: *const Guid, out: *mut Raw) -> i32 {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM supplies valid pointers; this object implements this exact interface.
    unsafe {
        *out = null_mut();
        if ![IID_IUNKNOWN, IID_IAGILE_OBJECT, NOTIFICATION].contains(&*iid) {
            return E_NOINTERFACE;
        }
        notify_add(this);
        *out = this;
    }
    0
}

unsafe extern "system" fn notify_add(this: Raw) -> u32 {
    // SAFETY: a live Notify pointer owned by a COM reference.
    unsafe { &*(this as *const Notify) }.refs.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn notify_release(this: Raw) -> u32 {
    // SAFETY: the caller owns a reference.
    let refs = unsafe { &*(this as *const Notify) }.refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        fence(Ordering::Acquire);
        // SAFETY: last reference to an allocation made by Box::into_raw.
        drop(unsafe { Box::from_raw(this as *mut Notify) });
    }
    refs
}

unsafe extern "system" fn notify_state(this: Raw, id: *const u16, _: u32) -> i32 {
    // SAFETY: notification interface remains alive for this callback.
    unsafe { notify_device(this, id) }
}

unsafe extern "system" fn notify_device(this: Raw, _: *const u16) -> i32 {
    // SAFETY: only immutable/atomic state is accessed from this short callback.
    unsafe { &*(this as *const Notify) }.wake.signal(DEVICE_CHANGED);
    0
}

unsafe extern "system" fn notify_default(this: Raw, flow: i32, role: i32, id: *const u16) -> i32 {
    if flow == 0 && role == 1 {
        // SAFETY: forwarded within the same live notification callback.
        unsafe { notify_device(this, id) }
    } else {
        0
    }
}

unsafe extern "system" fn notify_property(_: Raw, _: *const u16, _: PropertyKey) -> i32 {
    // Friendly-name and volume property changes do not require restarting capture. A format
    // invalidation is reported by IAudioClient; state/default changes have dedicated callbacks.
    0
}

struct Registration {
    enumerator: Com,
    callback: Com,
}

impl Registration {
    fn new(enumerator: &Com, wake: &Wake) -> Result<Self> {
        let raw = Box::into_raw(Box::new(Notify {
            vt: &NOTIFY,
            refs: AtomicU32::new(1),
            wake: wake.clone(),
            _activity: crate::platform::Activity::callback()?,
        }));
        // SAFETY: exact IMMNotificationClient implementation with one owned reference.
        let callback = unsafe { Com::owned(raw.cast(), "audio notification client")? };
        // SAFETY: both interfaces are owned and expose the SDK's matching vtables.
        let hr = unsafe { (enumerator.vt::<EnumeratorVtbl>().register)(enumerator.raw(), callback.raw()) };
        check(hr, "subscribe to audio devices")?;
        Ok(Self { enumerator: enumerator.clone(), callback })
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: called on the owner thread, outside a notification callback; both objects live.
        unsafe { (self.enumerator.vt::<EnumeratorVtbl>().unregister)(self.enumerator.raw(), self.callback.raw()) };
    }
}

struct TaskMemory(Raw);

impl Drop for TaskMemory {
    fn drop(&mut self) {
        // SAFETY: APIs below return task-allocator memory, including null on an empty result.
        unsafe { CoTaskMemFree(self.0) };
    }
}

fn enumerator() -> Result<Com> {
    let mut raw = null_mut();
    // SAFETY: CLSID and IID from mmdeviceapi.h, on an initialized COM thread.
    unsafe {
        check(
            CoCreateInstance(&ENUMERATOR_CLASS, null_mut(), 1, &ENUMERATOR, &mut raw),
            "create audio device enumerator",
        )?;
        Com::owned(raw, "audio device enumerator")
    }
}

fn device_id(device: &Com) -> Result<String> {
    let mut raw = null_mut();
    // SAFETY: GetId returns a task-allocated, NUL-terminated endpoint ID.
    unsafe {
        check((device.vt::<DeviceVtbl>().id)(device.raw(), &mut raw), "audio device identifier")?;
        let _memory = TaskMemory(raw.cast());
        if raw.is_null() {
            return Err(Error::new("audio device identifier", E_POINTER));
        }
        let mut length = 0;
        while length < 32768 && *raw.add(length) != 0 {
            length += 1;
        }
        if length == 32768 {
            return Err(Error::new("audio device identifier is too long", E_BOUNDS));
        }
        Ok(String::from_utf16_lossy(std::slice::from_raw_parts(raw, length)))
    }
}

#[derive(Clone, Debug, Default)]
pub enum Target {
    #[default]
    Default,
    Device(String),
    Process(u32),
    Player(String),
}

impl Target {
    fn resolve(&self, enumerator: &Com) -> Result<Com> {
        let mut raw = null_mut();
        // SAFETY: valid enumerator; output is owned. Endpoint IDs are NUL-terminated UTF-16.
        unsafe {
            let vt = enumerator.vt::<EnumeratorVtbl>();
            let hr = match self {
                Self::Default => (vt.default)(enumerator.raw(), 0, 1, &mut raw),
                Self::Device(id) => {
                    if id.is_empty() || id.len() > 32767 || id.contains('\0') {
                        return Err(Error::new("invalid audio device identifier", E_INVALIDARG));
                    }
                    let id: Vec<u16> = id.encode_utf16().chain([0]).collect();
                    (vt.device)(enumerator.raw(), id.as_ptr(), &mut raw)
                }
                Self::Process(_) | Self::Player(_) => {
                    return Err(Error::new("process audio is not an endpoint", E_INVALIDARG));
                }
            };
            check(hr, "select audio output device")?;
            Com::owned(raw, "audio output device")
        }
    }
}

pub fn devices() -> Result<Vec<String>> {
    let _apartment = Apartment::mta()?;
    let enumerator = enumerator()?;
    // SAFETY: active rendering endpoints from IMMDeviceEnumerator and IMMDeviceCollection.
    unsafe {
        let mut raw = null_mut();
        check(
            (enumerator.vt::<EnumeratorVtbl>().enumerate)(enumerator.raw(), 0, 1, &mut raw),
            "enumerate audio outputs",
        )?;
        let collection = Com::owned(raw, "audio output collection")?;
        let vt = collection.vt::<CollectionVtbl>();
        let mut count = 0;
        check((vt.count)(collection.raw(), &mut count), "audio output count")?;
        if count > 256 {
            return Err(Error::new("too many audio output devices", E_BOUNDS));
        }
        let mut devices = Vec::with_capacity(count as usize);
        for index in 0..count {
            let mut raw = null_mut();
            check((vt.item)(collection.raw(), index, &mut raw), "audio output")?;
            devices.push(device_id(&Com::owned(raw, "audio output")?)?);
        }
        Ok(devices)
    }
}

struct Capture {
    capture: Com,
    client: Com,
    event: Event,
    format: Format,
    buffer_frames: u32,
    id: String,
}

impl Capture {
    fn open(device: &Com) -> Result<Self> {
        let id = device_id(device)?;
        // SAFETY: these are the exact IMMDevice/IAudioClient/IAudioCaptureClient SDK vtables.
        unsafe {
            let mut raw = null_mut();
            check(
                (device.vt::<DeviceVtbl>().activate)(device.raw(), &AUDIO_CLIENT, 1, null_mut(), &mut raw),
                "activate WASAPI",
            )?;
            let client = Com::owned(raw, "WASAPI client")?;
            let vt = client.vt::<AudioVtbl>();
            let mut raw = null_mut();
            check((vt.mix_format)(client.raw(), &mut raw), "audio mix format")?;
            let memory = TaskMemory(raw);
            if raw.is_null() {
                return Err(Error::new("audio mix format", E_POINTER));
            }
            // WAVEFORMATEX is packed and has an 18-byte base header.
            let pointer = raw as *const u8;
            let header = std::slice::from_raw_parts(pointer, 18);
            let extra = u16::from_le_bytes([header[16], header[17]]) as usize;
            if extra > 128 {
                return Err(Error::new("audio format extension is too large", E_BOUNDS));
            }
            let format = Format::parse(std::slice::from_raw_parts(pointer, 18 + extra))?;
            check(
                (vt.initialize)(client.raw(), 0, 0x00060000, 1_000_000, 0, memory.0, null()),
                "initialize loopback capture",
            )?;
            Self::start(client, format, id)
        }
    }

    fn open_process(pid: u32, stop: &Event) -> Result<Self> {
        if pid == 0 {
            return Err(Error::new("invalid player process", E_INVALIDARG));
        }
        let client = process::activate(pid, stop)?;
        let format_bytes = [3u8, 0, 2, 0, 128, 187, 0, 0, 0, 220, 5, 0, 8, 0, 32, 0, 0, 0];
        let format = Format::parse(&format_bytes)?;
        // SAFETY: packed WAVEFORMATEX, fixed float32 stereo 48 kHz, native shared-mode conversion.
        unsafe {
            check(
                (client.vt::<AudioVtbl>().initialize)(
                    client.raw(),
                    0,
                    0x80060000,
                    0,
                    0,
                    format_bytes.as_ptr() as Raw,
                    null(),
                ),
                "initialize player loopback",
            )?;
        }
        Self::start(client, format, format!("process:{pid}"))
    }

    fn start(client: Com, format: Format, id: String) -> Result<Self> {
        // SAFETY: initialized IAudioClient and owner-thread capture service.
        unsafe {
            let vt = client.vt::<AudioVtbl>();
            let event = Event::auto_reset()?;
            check((vt.set_event)(client.raw(), event.raw()), "set audio buffer event")?;
            let mut buffer_frames = 0;
            check((vt.buffer_size)(client.raw(), &mut buffer_frames), "audio buffer size")?;
            let mut raw = null_mut();
            check((vt.service)(client.raw(), &CAPTURE_CLIENT, &mut raw), "audio capture service")?;
            let capture = Com::owned(raw, "audio capture service")?;
            check((vt.start)(client.raw()), "start audio capture")?;
            Ok(Self { capture, client, event, format, buffer_frames, id })
        }
    }

    fn drain(&self, analyzer: &mut Analyzer, stop: &Event) -> Result<bool> {
        let mut received = false;
        // SAFETY: capture belongs to this thread, and all packets are released on this thread.
        unsafe {
            let vt = self.capture.vt::<CaptureVtbl>();
            for _ in 0..32 {
                if stop.wait(0)? {
                    return Ok(false);
                }
                let mut count = 0;
                check((vt.next)(self.capture.raw(), &mut count), "next audio packet")?;
                if count == 0 {
                    return Ok(received);
                }
                let mut raw = null_mut();
                let mut frames = 0;
                let mut flags = 0;
                check(
                    (vt.get)(self.capture.raw(), &mut raw, &mut frames, &mut flags, null_mut(), null_mut()),
                    "read audio packet",
                )?;
                if frames == 0 {
                    return Ok(received);
                }
                let packet = Packet { capture: &self.capture, frames, released: false };
                if frames > self.buffer_frames || (flags & 2 == 0 && raw.is_null()) {
                    return Err(Error::new("invalid WASAPI packet", E_BOUNDS));
                }
                let data = if flags & 2 != 0 {
                    None
                } else {
                    Some(std::slice::from_raw_parts(raw, frames as usize * self.format.frame_bytes))
                };
                analyzer.push(data, frames as usize, flags & 1 != 0)?;
                packet.release()?;
                received = true;
            }
            // Keep cancellation and endpoint notifications responsive even under continuous input.
            self.event.set();
            Ok(received)
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // SAFETY: the owner stops capture before releasing the service/client/event.
        unsafe { (self.client.vt::<AudioVtbl>().stop)(self.client.raw()) };
    }
}

struct Packet<'a> {
    capture: &'a Com,
    frames: u32,
    released: bool,
}

impl Packet<'_> {
    fn release(mut self) -> Result<()> {
        self.released = true;
        // SAFETY: one successful GetBuffer, same thread, exact packet frame count.
        check(
            unsafe { (self.capture.vt::<CaptureVtbl>().release)(self.capture.raw(), self.frames) },
            "release audio packet",
        )
    }
}

impl Drop for Packet<'_> {
    fn drop(&mut self) {
        if !self.released {
            // SAFETY: release the outstanding packet even on an error/unwind path.
            unsafe { (self.capture.vt::<CaptureVtbl>().release)(self.capture.raw(), self.frames) };
        }
    }
}

pub enum Update<'a> {
    Ready { device: &'a str, format: Format },
    Frame(&'a Frame),
    Unavailable(Error),
}

pub fn monitor(
    target: &Target,
    stop: &Event,
    duration: Option<Duration>,
    fps: u32,
    spectrum: bool,
    update: impl FnMut(Update<'_>) -> Result<()>,
) -> Result<()> {
    monitor_tuned(target, stop, duration, fps, spectrum, Duration::from_millis(180), update)
}

pub(crate) fn monitor_tuned(
    target: &Target,
    stop: &Event,
    duration: Option<Duration>,
    fps: u32,
    spectrum: bool,
    release: Duration,
    mut update: impl FnMut(Update<'_>) -> Result<()>,
) -> Result<()> {
    if stop.wait(0)? {
        return Ok(());
    }
    if !(1..=60).contains(&fps) {
        return Err(Error::new("frame rate must be between 1 and 60", E_INVALIDARG));
    }
    let _apartment = Apartment::mta()?;
    let _process = match target {
        Target::Player(source) => Some(process::source(source)?),
        _ => None,
    };
    let selected_process = _process.as_ref().map(|p| Target::Process(p.pid));
    let target = selected_process.as_ref().unwrap_or(target);
    let enumerator = enumerator()?;
    let wake = Wake::new()?;
    let _registration = Registration::new(&enumerator, &wake)?;
    let mut capture: Option<Capture> = None;
    let mut analyzer: Option<Analyzer> = None;
    let deadline = duration
        .map(|d| Instant::now().checked_add(d).ok_or(Error::new("capture duration is too large", E_INVALIDARG)))
        .transpose()?;
    let interval = Duration::from_secs_f64(1.0 / fps as f64);
    let mut next_frame = Instant::now();
    let mut last_frame = next_frame;
    let mut smoothing = Smoothing::new(Duration::from_millis(35), release)?;
    let mut dirty = false;
    let mut retried = false;
    wake.signal(DEVICE_CHANGED);
    loop {
        let now = Instant::now();
        if deadline.is_some_and(|d| now >= d) {
            return Ok(());
        }
        let frame_due = dirty.then_some(next_frame);
        let due = [deadline, frame_due].into_iter().flatten().min();
        let timeout = due.map_or(INFINITE, |d| {
            let wait = d.saturating_duration_since(now);
            wait.as_millis()
                .saturating_add(u128::from(wait.subsec_nanos() % 1_000_000 != 0))
                .min((INFINITE - 1) as u128) as u32
        });
        let handles = [stop.raw(), wake.raw(), capture.as_ref().map_or(null_mut(), |c| c.event.raw())];
        // SAFETY: use only the live handles, with stop first so cancellation takes precedence.
        let result =
            unsafe { WaitForMultipleObjects(if capture.is_some() { 3 } else { 2 }, handles.as_ptr(), 0, timeout) };
        match result {
            0 => return Ok(()),
            1 => {
                let flags = wake.take();
                if flags & DEVICE_CHANGED != 0 {
                    retried = false;
                }
                let replacement = match target {
                    Target::Process(pid) => {
                        if capture.is_some() {
                            continue;
                        }
                        Capture::open_process(*pid, stop)
                    }
                    _ => match target.resolve(&enumerator) {
                        Ok(device) => match device_id(&device) {
                            Ok(id) if capture.as_ref().is_some_and(|c| c.id == id) => continue,
                            Ok(_) => Capture::open(&device),
                            Err(e) => Err(e),
                        },
                        Err(e) => Err(e),
                    },
                };
                capture = None;
                analyzer = None;
                dirty = false;
                match replacement {
                    Ok(c) => {
                        update(Update::Ready { device: &c.id, format: c.format })?;
                        analyzer = Some(Analyzer::new(c.format));
                        capture = Some(c);
                        next_frame = Instant::now();
                        last_frame = next_frame - interval;
                        smoothing = Smoothing::new(Duration::from_millis(35), release)?;
                    }
                    Err(error) => update(Update::Unavailable(error))?,
                }
            }
            2 => {
                if let (Some(c), Some(a)) = (&capture, &mut analyzer) {
                    match c.drain(a, stop) {
                        Ok(received) => dirty |= received,
                        Err(error) => {
                            capture = None;
                            analyzer = None;
                            dirty = false;
                            update(Update::Unavailable(error))?;
                            // One immediate recovery; further attempts require a real device event.
                            if !retried {
                                retried = true;
                                wake.signal(RETRY);
                            }
                        }
                    }
                }
            }
            258 => {}
            _ => return Err(Error::last("wait for audio event")),
        }
        if dirty && Instant::now() >= next_frame {
            if stop.wait(0)? {
                return Ok(());
            }
            if let Some(analyzer) = &mut analyzer {
                let now = Instant::now();
                let mut frame = analyzer.frame(spectrum);
                if spectrum {
                    smoothing.apply(&mut frame.bands, now.duration_since(last_frame));
                }
                last_frame = now;
                update(Update::Frame(&frame))?;
            }
            dirty = false;
            next_frame = Instant::now() + interval;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_vtable_layouts_match_sdk() {
        assert_eq!(size_of::<PropertyKey>(), 20);
        assert_eq!(std::mem::offset_of!(AudioVtbl, mix_format), 8 * 8);
        assert_eq!(std::mem::offset_of!(AudioVtbl, set_event), 13 * 8);
        assert_eq!(std::mem::offset_of!(AudioVtbl, service), 14 * 8);
    }

    #[test]
    fn cancellation_before_start_does_not_open_or_report_an_audio_source() {
        let stop = Event::new().unwrap();
        stop.set();
        let mut called = false;
        monitor(&Target::Default, &stop, None, 30, true, |_| {
            called = true;
            Ok(())
        })
        .unwrap();
        assert!(!called);
    }
}
