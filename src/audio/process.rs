//! Process-tree loopback and conservative GSMTC source identity resolution.
use super::*;
use std::sync::{Arc, Mutex, Weak};

struct Handle(Raw);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: uniquely owned process/snapshot handle.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
pub(super) struct Process {
    pub pid: u32,
    _handle: Handle,
}
#[repr(C)]
struct Entry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    name: [u16; 260],
}
pub(super) fn source(source: &str) -> Result<Process> {
    if source.is_empty() || source.len() > 4096 || source.contains('\0') {
        return Err(Error::new("player process identity unavailable", E_INVALIDARG));
    }
    let id: Vec<u16> = source.encode_utf16().collect();
    let executable = !source.contains('!') && source.to_ascii_lowercase().ends_with(".exe");
    // SAFETY: bounded snapshot iteration and fixed output arrays; handles are closed on every path.
    unsafe {
        let raw = CreateToolhelp32Snapshot(2, 0);
        if raw == -1isize as Raw {
            return Err(Error::last("snapshot player processes"));
        }
        let snapshot = Handle(raw);
        let mut entry: Entry = std::mem::zeroed();
        entry.size = size_of::<Entry>() as u32;
        let mut more = Process32FirstW(snapshot.0, &mut entry);
        let mut candidates = Vec::new();
        while more != 0 {
            let len = entry.name.iter().position(|c| *c == 0).unwrap_or(entry.name.len());
            let matches = if executable {
                CompareStringOrdinal(entry.name.as_ptr(), len as i32, id.as_ptr(), id.len() as i32, 1) == 2
            } else {
                let raw = OpenProcess(0x1000, 0, entry.pid);
                if raw.is_null() {
                    false
                } else {
                    let process = Handle(raw);
                    let mut app = [0u16; 512];
                    let mut len = app.len() as u32;
                    GetApplicationUserModelId(process.0, &mut len, app.as_mut_ptr()) == 0
                        && len > 0
                        && len as usize <= app.len()
                        && app[..len as usize - 1] == id
                }
            };
            if matches {
                if candidates.len() == 256 {
                    return Err(Error::new("too many matching player processes", E_BOUNDS));
                }
                candidates.push((entry.pid, entry.parent));
            }
            more = Process32NextW(snapshot.0, &mut entry);
        }
        if GetLastError() != 18 {
            return Err(Error::last("enumerate player processes"));
        }
        let mut roots = candidates.iter().filter(|(_, parent)| !candidates.iter().any(|(pid, _)| pid == parent));
        let pid = roots.next().map(|p| p.0).ok_or(Error::new("player process not found", 0x80070490u32 as i32))?;
        if roots.next().is_some() {
            return Err(Error::new("player process identity is ambiguous", E_INVALIDARG));
        }
        let raw = OpenProcess(0x101000, 0, pid);
        if raw.is_null() {
            return Err(Error::last("retain player process"));
        }
        Ok(Process { pid, _handle: Handle(raw) })
    }
}

const COMPLETION: Guid = Guid::from_u128(0x41d949ab_9862_444a_80f6_c261334da5eb);
const MARSHAL: Guid = Guid::from_u128(0x00000003_0000_0000_c000_000000000046);
static ACTIVATING: Mutex<Option<Weak<Activation>>> = Mutex::new(None);
#[repr(C)]
struct Params {
    kind: u32,
    pid: u32,
    mode: u32,
}
#[repr(C)]
struct Blob {
    len: u32,
    pointer: usize,
}
#[repr(C)]
struct Variant {
    kind: u16,
    reserved: [u16; 3],
    blob: Blob,
}
struct Activation {
    _params: Box<Params>,
    variant: Variant,
    done: Event,
    result: Mutex<Option<(i32, usize)>>,
}
impl Drop for Activation {
    fn drop(&mut self) {
        if let Some((_, raw)) = self.result.get_mut().unwrap_or_else(|e| e.into_inner()).take() {
            // SAFETY: unclaimed GetActivateResult reference, including late/cancelled completion.
            unsafe {
                release_raw(raw);
            }
        }
    }
}
unsafe fn release_raw(raw: usize) {
    if raw != 0 {
        // SAFETY: caller transfers one owned IUnknown reference.
        unsafe {
            ((**(raw as *const *const UnknownVtbl)).release)(raw as Raw);
        }
    }
}
#[repr(C)]
struct CompletionVtbl {
    unknown: UnknownVtbl,
    complete: unsafe extern "system" fn(Raw, Raw) -> i32,
}
#[repr(C)]
struct OperationVtbl {
    unknown: UnknownVtbl,
    result: unsafe extern "system" fn(Raw, *mut i32, *mut Raw) -> i32,
}
#[repr(C)]
struct Handler {
    vt: &'static CompletionVtbl,
    refs: AtomicU32,
    state: Arc<Activation>,
    marshal: usize,
    _activity: crate::platform::Activity,
}
impl Drop for Handler {
    fn drop(&mut self) {
        // SAFETY: uniquely owned nondelegating IUnknown of the aggregated free-threaded marshaler.
        unsafe {
            release_raw(self.marshal);
        }
        self.state.done.set();
    }
}
static HANDLER: CompletionVtbl = CompletionVtbl { unknown: UnknownVtbl { query, add_ref: add, release }, complete };
unsafe extern "system" fn query(this: Raw, iid: *const Guid, out: *mut Raw) -> i32 {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: standard COM query; the FTM handles IMarshal and preserves outer identity.
    unsafe {
        *out = null_mut();
        let handler = &*(this as *const Handler);
        if *iid == MARSHAL && handler.marshal != 0 {
            let raw = handler.marshal as Raw;
            return ((**(raw as *const *const UnknownVtbl)).query)(raw, iid, out);
        }
        if ![IID_IUNKNOWN, IID_IAGILE_OBJECT, COMPLETION].contains(&*iid) {
            return E_NOINTERFACE;
        }
        add(this);
        *out = this;
    }
    0
}
unsafe extern "system" fn add(this: Raw) -> u32 {
    // SAFETY: live COM reference.
    unsafe { &*(this as *const Handler) }.refs.fetch_add(1, Ordering::Relaxed) + 1
}
unsafe extern "system" fn release(this: Raw) -> u32 {
    // SAFETY: caller releases its reference; atomic final ownership reclaims the allocation once.
    let refs = unsafe { &*(this as *const Handler) }.refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        fence(Ordering::Acquire);
        // SAFETY: unique final reference.
        drop(unsafe { Box::from_raw(this as *mut Handler) });
    }
    refs
}
unsafe extern "system" fn complete(this: Raw, operation: Raw) -> i32 {
    if operation.is_null() {
        return E_POINTER;
    }
    // SAFETY: SDK completion runs in the MTA, shared with the waiting audio worker.
    unsafe {
        let handler = &*(this as *const Handler);
        let mut status = E_FAIL;
        let mut raw = null_mut();
        let hr = ((**(operation as *const *const OperationVtbl)).result)(operation, &mut status, &mut raw);
        let mut result = crate::runtime::lock(&handler.state.result);
        if let Some((_, previous)) = result.replace((if hr < 0 { hr } else { status }, raw as usize)) {
            release_raw(previous);
        }
        drop(result);
        handler.state.done.set();
    }
    0
}
fn wait_for_activation(done: &Event, stop: &Event, deadline: Instant) -> Result<()> {
    let handles = [stop.raw(), done.raw()];
    let remaining = deadline.saturating_duration_since(Instant::now());
    let timeout = remaining.as_millis().saturating_add(1).min((INFINITE - 1) as u128) as u32;
    // SAFETY: both events are retained for the cancellable, bounded wait.
    match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout) } {
        0 => Err(Error::new("process audio cancelled", E_ABORT)),
        1 => Ok(()),
        258 => Err(Error::new("player audio activation timed out", ERROR_TIMEOUT)),
        _ => Err(Error::last("wait for player audio activation")),
    }
}
fn reserve_activation(
    pending: &Mutex<Option<Weak<Activation>>>,
    state: &Arc<Activation>,
    stop: &Event,
    deadline: Instant,
) -> Result<()> {
    loop {
        if stop.wait(0)? {
            return Err(Error::new("process audio cancelled", E_ABORT));
        }
        if Instant::now() >= deadline {
            return Err(Error::new("player audio activation timed out", ERROR_TIMEOUT));
        }
        let previous = {
            let mut active = crate::runtime::lock(pending);
            match active.as_ref().and_then(Weak::upgrade) {
                Some(previous) if !previous.done.wait(0)? => previous,
                _ => {
                    *active = Some(Arc::downgrade(state));
                    return Ok(());
                }
            }
        };
        // A cancelled request can still complete in Windows. Wait for its callback instead of
        // rejecting the next playback transition or starting another outstanding activation.
        wait_for_activation(&previous.done, stop, deadline)?;
    }
}
pub(super) fn activate(pid: u32, stop: &Event) -> Result<Com> {
    if stop.wait(0)? {
        return Err(Error::new("process audio cancelled", E_ABORT));
    }
    let mut params = Box::new(Params { kind: 1, pid, mode: 0 });
    let variant = Variant {
        kind: 65,
        reserved: [0; 3],
        blob: Blob { len: size_of::<Params>() as u32, pointer: (&mut *params as *mut Params) as usize },
    };
    let state = Arc::new(Activation { _params: params, variant, done: Event::new()?, result: Mutex::new(None) });
    let activity = crate::platform::Activity::callback()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    reserve_activation(&ACTIVATING, &state, stop, deadline)?;
    let raw = Box::into_raw(Box::new(Handler {
        vt: &HANDLER,
        refs: AtomicU32::new(1),
        state: state.clone(),
        marshal: 0,
        _activity: activity,
    }));
    // SAFETY: retained handler owns all parameters through late completion, including cancellation.
    unsafe {
        let handler = Com::owned(raw.cast(), "audio activation callback")?;
        let mut marshaler = null_mut();
        check(CoCreateFreeThreadedMarshaler(raw.cast(), &mut marshaler), "create audio callback marshaler")?;
        (*raw).marshal = marshaler as usize;
        let path: Vec<u16> = "VAD\\Process_Loopback".encode_utf16().chain(Some(0)).collect();
        let mut operation = null_mut();
        let hr =
            ActivateAudioInterfaceAsync(path.as_ptr(), &AUDIO_CLIENT, &state.variant, handler.raw(), &mut operation);
        if hr < 0 {
            state.done.set();
            release_raw(operation as usize);
            return Err(Error::new("activate player audio", hr));
        }
        let _operation = Com::owned(operation, "player audio activation")?;
        wait_for_activation(&state.done, stop, deadline)?;
        let (hr, raw) =
            crate::runtime::lock(&state.result).take().ok_or(Error::new("missing audio activation result", E_FAIL))?;
        let result = Com::owned(raw as Raw, "player audio client");
        check(hr, "activate player audio")?;
        result?.query(&AUDIO_CLIENT)
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Raw;
    fn Process32FirstW(snapshot: Raw, entry: *mut Entry) -> i32;
    fn Process32NextW(snapshot: Raw, entry: *mut Entry) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Raw;
    fn GetApplicationUserModelId(process: Raw, length: *mut u32, name: *mut u16) -> i32;
    fn CompareStringOrdinal(a: *const u16, an: i32, b: *const u16, bn: i32, ignore_case: i32) -> i32;
}
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoCreateFreeThreadedMarshaler(outer: Raw, inner: *mut Raw) -> i32;
}
#[link(name = "mmdevapi")]
unsafe extern "system" {
    fn ActivateAudioInterfaceAsync(
        path: *const u16,
        iid: *const Guid,
        params: *const Variant,
        handler: Raw,
        operation: *mut Raw,
    ) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn activation() -> Arc<Activation> {
        Arc::new(Activation {
            _params: Box::new(Params { kind: 1, pid: 1, mode: 0 }),
            variant: Variant { kind: 65, reserved: [0; 3], blob: Blob { len: 0, pointer: 0 } },
            done: Event::new().unwrap(),
            result: Mutex::new(None),
        })
    }
    #[test]
    fn next_activation_waits_for_late_completion_and_respects_cancellation() {
        let old = activation();
        let next = activation();
        let pending = Mutex::new(Some(Arc::downgrade(&old)));
        let stop = Event::new().unwrap();
        stop.set();
        let deadline = Instant::now() + Duration::from_secs(2);
        assert_eq!(reserve_activation(&pending, &next, &stop, deadline).unwrap_err().code, E_ABORT);
        stop.reset();
        assert_eq!(reserve_activation(&pending, &next, &stop, Instant::now()).unwrap_err().code, ERROR_TIMEOUT);
        let ready = Event::new().unwrap();
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                ready.set();
                reserve_activation(&pending, &next, &stop, deadline)
            });
            assert!(ready.wait(1000).unwrap());
            old.done.set();
            waiter.join().unwrap().unwrap();
        });
        assert!(Arc::ptr_eq(&crate::runtime::lock(&pending).as_ref().unwrap().upgrade().unwrap(), &next));
        // Windows may retain the previous handler after completing the operation.
        assert_eq!(Arc::strong_count(&old), 1);
    }
    #[test]
    fn process_activation_abi_and_identity_are_bounded() {
        assert_eq!(size_of::<Entry>(), 568);
        assert_eq!(size_of::<Params>(), 12);
        assert_eq!(size_of::<Variant>(), 24);
        assert_eq!(std::mem::offset_of!(Variant, blob), 8);
        assert!(source("").is_err());
        let exe = std::env::current_exe().unwrap();
        let process = source(exe.file_name().unwrap().to_str().unwrap()).unwrap();
        assert_eq!(process.pid, std::process::id());
        let stop = Event::new().unwrap();
        stop.set();
        assert_eq!(activate(process.pid, &stop).err().unwrap().code, E_ABORT);
    }
}
