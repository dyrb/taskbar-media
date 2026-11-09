//! Windows 10 DeskBand. Registration is explicit; no loader entry point performs initialization.
use crate::platform::com::Com;
use crate::platform::ffi::*;
use crate::platform::{Error, Result};
use crate::ui::{self, api::*};
use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering, fence};

pub const BAND_CHANGED: u32 = 0x8000 + 46;
pub const CLSID: Guid = Guid::from_u128(0x6a215d25_d7e0_4597_a2bf_e8de78172984);
const CLSID_TEXT: &str = "{6A215D25-D7E0-4597-A2BF-E8DE78172984}";
const CLASS_FACTORY: Guid = Guid::from_u128(0x00000001_0000_0000_c000_000000000046);
const OLE_WINDOW: Guid = Guid::from_u128(0x00000114_0000_0000_c000_000000000046);
const DOCKING: Guid = Guid::from_u128(0x012dd920_7b26_11d0_8ca9_00a0c92dbfe8);
const DESK_BAND: Guid = Guid::from_u128(0xeb0fe172_1a3a_11d0_89b3_00a0c90a90ac);
const DESK_BAND2: Guid = Guid::from_u128(0x79d16de4_abee_4021_8d9d_9169b261d657);
const SITE: Guid = Guid::from_u128(0xfc4801a3_2ba9_11cf_a229_00aa003d7352);
const PERSIST: Guid = Guid::from_u128(0x0000010c_0000_0000_c000_000000000046);
const PERSIST_STREAM: Guid = Guid::from_u128(0x00000109_0000_0000_c000_000000000046);
const INPUT: Guid = Guid::from_u128(0x68284faa_6a48_11d0_8c78_00c04fd918b4);
const INPUT_SITE: Guid = Guid::from_u128(0xf1db8392_7331_11d0_8c99_00a0c92dbfe8);
const TRAY_CLASS: Guid = Guid::from_u128(0xe6442437_6c68_4f52_94dd_2cfed267efb9);
const TRAY_IID: Guid = Guid::from_u128(0x6d67e846_5b9c_4db8_9cbc_dde12f4254f1);
const DBIMF_VARIABLEHEIGHT: u32 = 0x0008;
const DBIMF_TOPALIGN: u32 = 0x0400;
const DBIMF_NOMARGINS: u32 = 0x2000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Registration {
    User,
    Machine,
}
impl Registration {
    fn root(self) -> Raw {
        (match self {
            Self::User => -2147483647isize,
            Self::Machine => -2147483646isize,
        }) as Raw
    }
    pub fn automatic() -> Result<Self> {
        let mut lua = 1u32;
        let mut bytes = size_of::<u32>() as u32;
        // SAFETY: read one DWORD only; no UAC setting is modified.
        let code = unsafe {
            RegGetValueW(
                Self::Machine.root(),
                wide("Software\\Microsoft\\Windows\\CurrentVersion\\Policies\\System").as_ptr(),
                wide("EnableLUA").as_ptr(),
                0x10,
                null_mut(),
                (&mut lua as *mut u32).cast(),
                &mut bytes,
            )
        };
        if code == 2 || code == 0 && lua != 0 {
            return Ok(Self::User);
        }
        if code != 0 {
            return Err(Error::new("read COM registration environment", (0x80070000u32 | code as u32) as i32));
        }
        let mut token = null_mut();
        // SAFETY: query this process's elevation, close the acquired token on every path.
        unsafe {
            if OpenProcessToken(-1isize as Raw, 8, &mut token) == 0 {
                return Err(Error::last("query registration privileges"));
            }
            let mut elevated = 0u32;
            let ok = GetTokenInformation(token, 20, (&mut elevated as *mut u32).cast(), 4, &mut bytes);
            let error = if ok == 0 { Some(Error::last("query registration privileges")) } else { None };
            CloseHandle(token);
            if let Some(error) = error {
                return Err(error);
            }
            Ok(if elevated != 0 { Self::Machine } else { Self::User })
        }
    }
}
#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: Raw, access: u32, token: *mut Raw) -> i32;
    fn GetTokenInformation(token: Raw, kind: u32, data: Raw, size: u32, written: *mut u32) -> i32;
}
#[repr(C)]
struct TrayVtbl {
    unknown: UnknownVtbl,
    show: unsafe extern "system" fn(Raw, *const Guid) -> i32,
    hide: unsafe extern "system" fn(Raw, *const Guid) -> i32,
    shown: unsafe extern "system" fn(Raw, *const Guid) -> i32,
    changed: unsafe extern "system" fn(Raw) -> i32,
}

struct Tray {
    inner: Com,
    _apartment: crate::platform::com::Apartment,
}
impl Tray {
    fn connect() -> Result<Self> {
        if windows_build()? >= 22000 {
            return Err(Error::new("this taskbar integration requires Windows 10", 0x80004001u32 as i32));
        }
        let apartment = crate::platform::com::Apartment::sta()?;
        let mut raw = null_mut();
        // SAFETY: SDK's ITrayDeskBand is implemented by Explorer as a local COM server.
        unsafe {
            crate::platform::check(
                CoCreateInstance(&TRAY_CLASS, null_mut(), 4, &TRAY_IID, &mut raw),
                "connect to Windows taskbar",
            )?;
            Ok(Self { inner: Com::owned(raw, "taskbar host")?, _apartment: apartment })
        }
    }
    fn shown(&self) -> Result<bool> {
        // SAFETY: SDK HRESULT is S_OK for shown and S_FALSE for hidden; only our CLSID is queried.
        let hr = unsafe { (self.inner.vt::<TrayVtbl>().shown)(self.inner.raw(), &CLSID) };
        crate::platform::check(hr, "query taskbar band")?;
        Ok(hr == 0)
    }
    fn changed(&self) -> Result<()> {
        // SAFETY: refresh the documented band registration cache, without restarting the shell.
        crate::platform::check(
            unsafe { (self.inner.vt::<TrayVtbl>().changed)(self.inner.raw()) },
            "refresh taskbar registration",
        )
    }
}

pub fn enable(scope: Registration) -> Result<bool> {
    let tray = Tray::connect()?;
    register(scope)?;
    tray.changed()?;
    // SAFETY: request only our registered CLSID; Explorer creates and sizes its child.
    // IsDeskBandShown may fail before the shell has instantiated the registered band.
    let hr = unsafe { (tray.inner.vt::<TrayVtbl>().show)(tray.inner.raw(), &CLSID) };
    crate::platform::check(hr, "show taskbar band")?;
    // Shell may still be awaiting its own compatibility/confirmation UI.
    tray.shown()
}
pub fn disable() -> Result<()> {
    let tray = Tray::connect()?;
    // SAFETY: hiding is restricted to our band, including one not yet instantiated.
    let hr = unsafe { (tray.inner.vt::<TrayVtbl>().hide)(tray.inner.raw(), &CLSID) };
    crate::platform::check(hr, "hide taskbar band")
}
pub fn is_shown() -> Result<bool> {
    Tray::connect()?.shown()
}
static OBJECTS: AtomicU32 = AtomicU32::new(0);
static LOCKS: AtomicU32 = AtomicU32::new(0);

pub(crate) fn view_destroyed(identity: Raw, hwnd: Raw) {
    let _ = boundary(|| {
        // SAFETY: the view borrows this live Band; close/final Drop clear the identity first.
        let band = unsafe { owner(identity) };
        if let Ok(mut state) = band.state.try_borrow_mut()
            && state.hwnd == hwnd
        {
            state.hwnd = null_mut();
        }
        0
    });
}

#[repr(C)]
struct Entry {
    vt: *const (),
    owner: *mut Band,
}
#[repr(C)]
struct Band {
    desk: Entry,
    site: Entry,
    persist: Entry,
    input: Entry,
    refs: AtomicU32,
    state: RefCell<BandState>,
}
struct BandState {
    site: Option<Com>,
    hwnd: Raw,
    id: u32,
    composited: bool,
}
impl Drop for Band {
    fn drop(&mut self) {
        // SAFETY: shell holds apartment-threaded interfaces; child was created on this thread.
        unsafe {
            if !self.state.get_mut().hwnd.is_null() {
                ui::set_band_identity(self.state.get_mut().hwnd, null_mut());
                DestroyWindow(self.state.get_mut().hwnd);
            }
        }
        OBJECTS.fetch_sub(1, Ordering::Release);
    }
}

#[repr(C)]
pub struct BandInfo {
    pub mask: u32,
    pub min: Point,
    pub max: Point,
    pub integral: Point,
    pub actual: Point,
    pub title: [u16; 256],
    pub mode: u32,
    pub background: u32,
}
#[repr(C)]
struct DeskVtbl {
    unknown: UnknownVtbl,
    window: unsafe extern "system" fn(Raw, *mut Raw) -> i32,
    help: unsafe extern "system" fn(Raw, i32) -> i32,
    show: unsafe extern "system" fn(Raw, i32) -> i32,
    close: unsafe extern "system" fn(Raw, u32) -> i32,
    resize: unsafe extern "system" fn(Raw, *const Rect, Raw, u32) -> i32,
    info: unsafe extern "system" fn(Raw, u32, u32, *mut BandInfo) -> i32,
    can_compose: unsafe extern "system" fn(Raw, *mut i32) -> i32,
    set_compose: unsafe extern "system" fn(Raw, i32) -> i32,
    get_compose: unsafe extern "system" fn(Raw, *mut i32) -> i32,
}
#[repr(C)]
struct SiteVtbl {
    unknown: UnknownVtbl,
    set: unsafe extern "system" fn(Raw, Raw) -> i32,
    get: unsafe extern "system" fn(Raw, *const Guid, *mut Raw) -> i32,
}
#[repr(C)]
struct PersistVtbl {
    unknown: UnknownVtbl,
    class: unsafe extern "system" fn(Raw, *mut Guid) -> i32,
    dirty: unsafe extern "system" fn(Raw) -> i32,
    load: unsafe extern "system" fn(Raw, Raw) -> i32,
    save: unsafe extern "system" fn(Raw, Raw, i32) -> i32,
    size: unsafe extern "system" fn(Raw, *mut u64) -> i32,
}
#[repr(C)]
struct InputVtbl {
    unknown: UnknownVtbl,
    activate: unsafe extern "system" fn(Raw, i32, *const Msg) -> i32,
    focus: unsafe extern "system" fn(Raw) -> i32,
    translate: unsafe extern "system" fn(Raw, *mut Msg) -> i32,
}
const fn unknown() -> UnknownVtbl {
    UnknownVtbl { query, add_ref: add, release }
}
static DESK: DeskVtbl = DeskVtbl {
    unknown: unknown(),
    window,
    help,
    show,
    close,
    resize,
    info,
    can_compose,
    get_compose: composition,
    set_compose: set_composition,
};
static SITE_VT: SiteVtbl = SiteVtbl { unknown: unknown(), set: set_site, get: get_site };
static PERSIST_VT: PersistVtbl = PersistVtbl { unknown: unknown(), class: get_class, dirty, load, save, size };
static INPUT_VT: InputVtbl = InputVtbl { unknown: unknown(), activate, focus, translate };

fn boundary(action: impl FnOnce() -> i32) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).unwrap_or(E_FAIL)
}
unsafe fn owner<'a>(this: Raw) -> &'a Band {
    // SAFETY: every supported interface starts with Entry and shares the same live owner.
    unsafe { &*(*(this as *const Entry)).owner }
}
unsafe extern "system" fn query(this: Raw, iid: *const Guid, out: *mut Raw) -> i32 {
    boundary(|| {
        if iid.is_null() || out.is_null() {
            return E_POINTER;
        }
        // SAFETY: valid COM inputs; all returned subobjects belong to this same Band.
        unsafe {
            *out = null_mut();
            let band = owner(this);
            let entry = if [IID_IUNKNOWN, OLE_WINDOW, DOCKING, DESK_BAND, DESK_BAND2].contains(&*iid) {
                &band.desk
            } else if *iid == SITE {
                &band.site
            } else if *iid == PERSIST || *iid == PERSIST_STREAM {
                &band.persist
            } else if *iid == INPUT {
                &band.input
            } else {
                return E_NOINTERFACE;
            };
            add(this);
            *out = entry as *const Entry as Raw;
        }
        0
    })
}
unsafe extern "system" fn add(this: Raw) -> u32 {
    // SAFETY: live COM reference, atomic count shared by every interface.
    unsafe { owner(this) }.refs.fetch_add(1, Ordering::Relaxed) + 1
}
unsafe extern "system" fn release(this: Raw) -> u32 {
    // SAFETY: caller owns a reference. Last release reclaims the Box once.
    let band = unsafe { (*(this as *const Entry)).owner };
    // SAFETY: the owner is live until its count reaches zero.
    let refs = unsafe { &*band }.refs.fetch_sub(1, Ordering::Release) - 1;
    if refs == 0 {
        fence(Ordering::Acquire);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: unique final ownership of the Box.
            drop(unsafe { Box::from_raw(band) });
        }));
    }
    refs
}
unsafe extern "system" fn window(this: Raw, out: *mut Raw) -> i32 {
    boundary(|| {
        if out.is_null() {
            return E_POINTER;
        }
        // SAFETY: live apartment-threaded band, writable output.
        unsafe {
            *out = owner(this).state.borrow().hwnd;
            if (*out).is_null() {
                return E_FAIL;
            }
        }
        0
    })
}
unsafe extern "system" fn help(_: Raw, _: i32) -> i32 {
    0x80004001u32 as i32
}
unsafe extern "system" fn show(this: Raw, visible: i32) -> i32 {
    boundary(|| {
        // SAFETY: child HWND belongs to this apartment; ShowWindow does not transfer ownership.
        unsafe {
            let hwnd = owner(this).state.borrow().hwnd;
            ShowWindow(hwnd, if visible != 0 { 5 } else { 0 });
            ui::set_band_shown(hwnd, visible != 0);
        }
        0
    })
}
unsafe extern "system" fn close(this: Raw, _: u32) -> i32 {
    boundary(|| {
        // SAFETY: remove HWND from state before reentrant destruction.
        let hwnd = unsafe { owner(this) }.state.borrow_mut().hwnd;
        // SAFETY: current thread owns this child; lifecycle is cancelled by WM_NCDESTROY.
        unsafe {
            owner(this).state.borrow_mut().hwnd = null_mut();
            if !hwnd.is_null() {
                ui::set_band_identity(hwnd, null_mut());
                DestroyWindow(hwnd);
            }
        }
        0
    })
}
unsafe extern "system" fn resize(_: Raw, _: *const Rect, _: Raw, _: u32) -> i32 {
    0x80004001u32 as i32
}
unsafe extern "system" fn info(this: Raw, id: u32, mode: u32, out: *mut BandInfo) -> i32 {
    boundary(|| {
        if out.is_null() {
            return E_POINTER;
        }
        // SAFETY: SDK's in/out DESKBANDINFO, only requested fields are written.
        unsafe {
            let band = owner(this);
            let mut state = band.state.borrow_mut();
            state.id = id;
            ui::set_band_id(state.hwnd, id);
            let dpi = if state.hwnd.is_null() { 96 } else { GetDpiForWindow(state.hwnd).max(96) };
            let scale = |v: i32| ((v as i64 * dpi as i64 + 48) / 96) as i32;
            let (width, _) = ui::band_preferences(state.hwnd).unwrap_or((360, false));
            let width = width as i32;
            let mut parent = Rect::default();
            if !state.hwnd.is_null() {
                GetClientRect(GetParent(state.hwnd), &mut parent);
            }
            let b = &mut *out;
            let vertical = mode & 1 != 0;
            if b.mask & 1 != 0 {
                b.min = Point { x: scale(if vertical { 32 } else { 160 }), y: scale(if vertical { 90 } else { 32 }) };
            }
            if b.mask & 2 != 0 {
                b.max = Point { x: -1, y: -1 };
            }
            if b.mask & 4 != 0 {
                b.integral = Point { x: 1, y: 1 };
            }
            if b.mask & 8 != 0 {
                b.actual = Point {
                    x: if vertical && parent.width() > 0 {
                        parent.width().max(scale(32))
                    } else {
                        scale(if vertical { 64 } else { width })
                    },
                    y: if !vertical && parent.height() > 0 {
                        parent.height().max(scale(32))
                    } else {
                        scale(if vertical { 180 } else { 40 })
                    },
                };
            }
            if b.mask & 16 != 0 {
                b.mask &= !16;
            }
            if b.mask & 32 != 0 {
                // Preserve Windows' sizing grip. The view follows the available row in both layouts.
                b.mode = DBIMF_TOPALIGN | DBIMF_NOMARGINS | DBIMF_VARIABLEHEIGHT;
            }
            b.mask &= !64;
        }
        0
    })
}
unsafe extern "system" fn can_compose(_: Raw, out: *mut i32) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: valid writable BOOL output. The view uses alpha-correct buffered GDI painting.
    unsafe {
        *out = 1;
    }
    0
}
unsafe extern "system" fn composition(this: Raw, out: *mut i32) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    boundary(|| {
        // SAFETY: apartment-owned band and writable SDK BOOL.
        unsafe {
            *out = i32::from(owner(this).state.borrow().composited);
        }
        0
    })
}
unsafe extern "system" fn set_composition(this: Raw, enabled: i32) -> i32 {
    boundary(|| {
        // SAFETY: state and the child view belong to the calling shell apartment.
        unsafe {
            let hwnd = {
                let mut state = owner(this).state.borrow_mut();
                state.composited = enabled != 0;
                state.hwnd
            };
            ui::set_band_composition(hwnd, enabled != 0);
        }
        0
    })
}
#[repr(C)]
struct OleWindowVtbl {
    unknown: UnknownVtbl,
    window: unsafe extern "system" fn(Raw, *mut Raw) -> i32,
}
unsafe extern "system" fn set_site(this: Raw, site: Raw) -> i32 {
    boundary(|| {
        // SAFETY: caller supplies IUnknown or null. Retain before storing; IOleWindow gives the container.
        unsafe {
            close(this, 0);
            let band = owner(this);
            band.state.borrow_mut().site = None;
            if site.is_null() {
                return 0;
            }
            let vt = &**(site as *const *const UnknownVtbl);
            (vt.add_ref)(site);
            let site = match Com::owned(site, "band site") {
                Ok(s) => s,
                Err(e) => return e.code,
            };
            let ole = match site.query(&OLE_WINDOW) {
                Ok(s) => s,
                Err(e) => return e.code,
            };
            let mut parent = null_mut();
            let hr = (ole.vt::<OleWindowVtbl>().window)(ole.raw(), &mut parent);
            if hr < 0 {
                return hr;
            }
            if parent.is_null() {
                return E_FAIL;
            }
            match ui::create(parent) {
                Ok(hwnd) => {
                    let mut state = band.state.borrow_mut();
                    state.hwnd = hwnd;
                    state.site = Some(site);
                    drop(state);
                    ui::set_band_identity(hwnd, &band.desk as *const Entry as Raw);
                    ui::set_band_composition(hwnd, band.state.borrow().composited);
                    ui::set_band_site(hwnd, band.state.borrow().site.as_ref(), band.state.borrow().id);
                    0
                }
                Err(e) => e.code,
            }
        }
    })
}
unsafe extern "system" fn get_site(this: Raw, iid: *const Guid, out: *mut Raw) -> i32 {
    boundary(|| {
        if iid.is_null() || out.is_null() {
            return E_POINTER;
        }
        // SAFETY: query the retained site directly, preserving its COM identity and reference count.
        unsafe {
            *out = null_mut();
            let state = owner(this).state.borrow();
            match &state.site {
                Some(s) => (s.vt::<UnknownVtbl>().query)(s.raw(), iid, out),
                None => E_FAIL,
            }
        }
    })
}
unsafe extern "system" fn get_class(_: Raw, out: *mut Guid) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable CLSID output.
    unsafe {
        *out = CLSID;
    }
    0
}
// Configuration is portable, not an opaque Explorer stream. A zero-byte persistence format is intentional.
unsafe extern "system" fn dirty(_: Raw) -> i32 {
    1
}
unsafe extern "system" fn load(_: Raw, stream: Raw) -> i32 {
    if stream.is_null() { E_POINTER } else { 0 }
}
unsafe extern "system" fn save(_: Raw, stream: Raw, _: i32) -> i32 {
    if stream.is_null() { E_POINTER } else { 0 }
}
unsafe extern "system" fn size(_: Raw, out: *mut u64) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: zero bytes are written by Save.
    unsafe {
        *out = 0;
    }
    0
}
unsafe extern "system" fn activate(this: Raw, active: i32, _: *const Msg) -> i32 {
    boundary(|| {
        // SAFETY: keyboard activation belongs to this UI thread and retained site.
        unsafe {
            let (hwnd, site) = {
                let state = owner(this).state.borrow();
                (state.hwnd, state.site.clone())
            };
            if active != 0 {
                let mut target = GetWindow(hwnd, 5);
                while !target.is_null() && IsWindowVisible(target) == 0 {
                    target = GetWindow(target, 2);
                }
                SetFocus(if target.is_null() { hwnd } else { target });
            }
            if let Some(site) = &site
                && let Ok(input) = site.query(&INPUT_SITE)
            {
                #[repr(C)]
                struct InputSite {
                    unknown: UnknownVtbl,
                    changed: unsafe extern "system" fn(Raw, Raw, i32) -> i32,
                }
                (input.vt::<InputSite>().changed)(input.raw(), &owner(this).desk as *const Entry as Raw, active);
            }
        }
        0
    })
}
unsafe extern "system" fn focus(this: Raw) -> i32 {
    boundary(|| {
        // SAFETY: focus query on the owning UI thread.
        unsafe {
            let hwnd = owner(this).state.borrow().hwnd;
            let focused = GetFocus();
            if focused == hwnd || IsChild(hwnd, focused) != 0 { 0 } else { 1 }
        }
    })
}
unsafe extern "system" fn translate(this: Raw, msg: *mut Msg) -> i32 {
    boundary(|| {
        if msg.is_null() {
            return E_POINTER;
        }
        // SAFETY: valid MSG from shell keyboard routing; only this child participates.
        unsafe {
            let hwnd = owner(this).state.borrow().hwnd;
            if IsDialogMessageW(hwnd, msg) != 0 { 0 } else { 1 }
        }
    })
}

#[repr(C)]
struct Factory {
    vt: &'static FactoryVtbl,
    refs: AtomicU32,
}
#[repr(C)]
struct FactoryVtbl {
    unknown: UnknownVtbl,
    create: unsafe extern "system" fn(Raw, Raw, *const Guid, *mut Raw) -> i32,
    lock: unsafe extern "system" fn(Raw, i32) -> i32,
}
static FACTORY: FactoryVtbl = FactoryVtbl {
    unknown: UnknownVtbl { query: factory_query, add_ref: factory_add, release: factory_release },
    create: factory_create,
    lock: factory_lock,
};
unsafe extern "system" fn factory_query(this: Raw, iid: *const Guid, out: *mut Raw) -> i32 {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    // SAFETY: standard IClassFactory identity and owned output.
    unsafe {
        *out = null_mut();
        if *iid != IID_IUNKNOWN && *iid != CLASS_FACTORY {
            return E_NOINTERFACE;
        }
        factory_add(this);
        *out = this;
    }
    0
}
unsafe extern "system" fn factory_add(this: Raw) -> u32 {
    // SAFETY: live factory object retained by COM.
    unsafe { &*(this as *const Factory) }.refs.fetch_add(1, Ordering::Relaxed) + 1
}
unsafe extern "system" fn factory_release(this: Raw) -> u32 {
    // SAFETY: one owned reference consumed; last Release destroys the allocation.
    let count = unsafe { &*(this as *const Factory) }.refs.fetch_sub(1, Ordering::Release) - 1;
    if count == 0 {
        fence(Ordering::Acquire);
        // SAFETY: sole remaining owner, allocated by DllGetClassObject.
        drop(unsafe { Box::from_raw(this as *mut Factory) });
        OBJECTS.fetch_sub(1, Ordering::Release);
    }
    count
}
unsafe extern "system" fn factory_create(_: Raw, outer: Raw, iid: *const Guid, out: *mut Raw) -> i32 {
    boundary(|| {
        if out.is_null() {
            return E_POINTER;
        }
        // SAFETY: writable output is initialized even for aggregation errors.
        unsafe {
            *out = null_mut();
        }
        if !outer.is_null() {
            return 0x80040110u32 as i32;
        }
        let empty = || Entry { vt: null(), owner: null_mut() };
        let mut band = Box::new(Band {
            desk: empty(),
            site: empty(),
            persist: empty(),
            input: empty(),
            refs: AtomicU32::new(1),
            state: RefCell::new(BandState { site: None, hwnd: null_mut(), id: 0, composited: false }),
        });
        let pointer = &mut *band as *mut Band;
        band.desk = Entry { vt: &DESK as *const DeskVtbl as *const (), owner: pointer };
        band.site = Entry { vt: &SITE_VT as *const SiteVtbl as *const (), owner: pointer };
        band.persist = Entry { vt: &PERSIST_VT as *const PersistVtbl as *const (), owner: pointer };
        band.input = Entry { vt: &INPUT_VT as *const InputVtbl as *const (), owner: pointer };
        OBJECTS.fetch_add(1, Ordering::Relaxed);
        let raw = Box::into_raw(band);
        // SAFETY: initial reference is exchanged for the interface requested by the caller.
        unsafe {
            let hr = query(raw.cast(), iid, out);
            release(raw.cast());
            hr
        }
    })
}
unsafe extern "system" fn factory_lock(_: Raw, lock: i32) -> i32 {
    if lock != 0 {
        LOCKS.fetch_add(1, Ordering::Relaxed);
    } else {
        let _ = LOCKS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
    }
    0
}

/// # Safety
/// Standard COM entry point: GUID inputs and output must be valid for the call.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(class: *const Guid, iid: *const Guid, out: *mut Raw) -> i32 {
    boundary(|| {
        if class.is_null() || iid.is_null() || out.is_null() {
            return E_POINTER;
        }
        // SAFETY: validated non-null standard COM entry-point arguments.
        unsafe {
            *out = null_mut();
            if *class != CLSID {
                return 0x80040111u32 as i32;
            }
            let factory = Box::into_raw(Box::new(Factory { vt: &FACTORY, refs: AtomicU32::new(1) }));
            OBJECTS.fetch_add(1, Ordering::Relaxed);
            let hr = factory_query(factory.cast(), iid, out);
            factory_release(factory.cast());
            hr
        }
    })
}
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> i32 {
    if OBJECTS.load(Ordering::Acquire) == 0 && LOCKS.load(Ordering::Acquire) == 0 && crate::platform::callbacks() == 0 {
        ui::unregister_class();
        0
    } else {
        1
    }
}

pub fn register(scope: Registration) -> Result<()> {
    if windows_build()? >= 22000 {
        return Err(Error::new("Windows 11 has no supported DeskBand taskbar host", 0x80004001u32 as i32));
    }
    let dll = std::env::current_exe()
        .map_err(|_| Error::new("locate program", E_FAIL))?
        .with_file_name("taskbar_media_band.dll");
    if !dll.is_file() {
        return Err(Error::new("taskbar_media_band.dll must be beside the executable", E_FAIL));
    }
    let root = format!("Software\\Classes\\CLSID\\{CLSID_TEXT}");
    let created = set_registry(scope, &root, "", "Taskbar Media")?;
    let result = (|| {
        set_registry(scope, &format!("{root}\\InprocServer32"), "ThreadingModel", "Apartment")?;
        set_registry(
            scope,
            &format!("{root}\\Implemented Categories\\{{00021492-0000-0000-C000-000000000046}}"),
            "",
            "",
        )?;
        // Publish the path last so a failed update leaves the previous server path usable.
        set_registry(scope, &format!("{root}\\InprocServer32"), "", &dll.to_string_lossy()).map(|_| ())
    })();
    if result.is_err() && created {
        let _ = remove_registration(scope);
    }
    result
}
pub fn unregister(scope: Registration) -> Result<()> {
    disable()?;
    remove_registration(scope)?;
    Tray::connect()?.changed()
}
fn remove_registration(scope: Registration) -> Result<()> {
    // SAFETY: delete only this application's fixed CLSID tree in the selected scope.
    let code =
        unsafe { RegDeleteTreeW(scope.root(), wide(&format!("Software\\Classes\\CLSID\\{CLSID_TEXT}")).as_ptr()) };
    if code == 0 || code == 2 {
        Ok(())
    } else {
        Err(Error::new("remove DeskBand registration", (0x80070000u32 | code as u32) as i32))
    }
}
fn set_registry(scope: Registration, path: &str, name: &str, value: &str) -> Result<bool> {
    let mut key = null_mut();
    let mut disposition = 0;
    // SAFETY: fixed application subtree; terminated strings; handle is closed on every result.
    unsafe {
        let code = RegCreateKeyExW(
            scope.root(),
            wide(path).as_ptr(),
            0,
            null_mut(),
            0,
            2,
            null_mut(),
            &mut key,
            &mut disposition,
        );
        if code != 0 {
            return Err(Error::new("create DeskBand registration", (0x80070000u32 | code as u32) as i32));
        }
        let value = wide(value);
        let code = RegSetValueExW(key, wide(name).as_ptr(), 0, 1, value.as_ptr().cast(), (value.len() * 2) as u32);
        RegCloseKey(key);
        if code != 0 {
            Err(Error::new("write DeskBand registration", (0x80070000u32 | code as u32) as i32))
        } else {
            Ok(disposition == 1)
        }
    }
}
fn windows_build() -> Result<u32> {
    #[repr(C)]
    struct Version {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        service: [u16; 128],
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(version: *mut Version) -> i32;
    }
    let mut version =
        Version { size: size_of::<Version>() as u32, major: 0, minor: 0, build: 0, platform: 0, service: [0; 128] };
    // SAFETY: documented RTL_OSVERSIONINFOW layout and size.
    if unsafe { RtlGetVersion(&mut version) } < 0 {
        Err(Error::new("read Windows version", E_FAIL))
    } else {
        Ok(version.build)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires explicit registration of the release DLL"]
    fn registered_dll_activation() {
        #[repr(C)]
        struct TestSite {
            vt: &'static OleWindowVtbl,
            refs: AtomicU32,
            hwnd: Raw,
        }
        unsafe extern "system" fn test_query(this: Raw, iid: *const Guid, out: *mut Raw) -> i32 {
            // SAFETY: the local test calls with valid COM arguments and keeps the stack site alive.
            unsafe {
                *out = null_mut();
                if *iid != IID_IUNKNOWN && *iid != OLE_WINDOW {
                    return E_NOINTERFACE;
                }
                test_add(this);
                *out = this;
            }
            0
        }
        unsafe extern "system" fn test_add(this: Raw) -> u32 {
            // SAFETY: this points to the live local TestSite.
            unsafe { &*(this as *const TestSite) }.refs.fetch_add(1, Ordering::Relaxed) + 1
        }
        unsafe extern "system" fn test_release(this: Raw) -> u32 {
            // SAFETY: the stack site outlives every retained reference in this test.
            unsafe { &*(this as *const TestSite) }.refs.fetch_sub(1, Ordering::Relaxed) - 1
        }
        unsafe extern "system" fn test_window(this: Raw, out: *mut Raw) -> i32 {
            // SAFETY: writable output and live site from this test.
            unsafe { *out = (*(this as *const TestSite)).hwnd };
            0
        }
        let _apartment = crate::platform::com::Apartment::sta().unwrap();
        let mut raw = null_mut();
        // SAFETY: explicitly registered in-process server; no Explorer or window is involved.
        unsafe {
            crate::platform::check(
                CoCreateInstance(&CLSID, null_mut(), 1, &DESK_BAND2, &mut raw),
                "activate registered DeskBand",
            )
            .unwrap();
            let band = Com::owned(raw, "registered band").unwrap();
            let persist = band.query(&PERSIST_STREAM).unwrap();
            let mut class = IID_IUNKNOWN;
            assert_eq!((persist.vt::<PersistVtbl>().class)(persist.raw(), &mut class), 0);
            assert_eq!(class, CLSID);
            let parent = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("Band lifecycle test").as_ptr(),
                0,
                0,
                0,
                480,
                40,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            );
            assert!(!parent.is_null());
            let site = TestSite {
                vt: &OleWindowVtbl {
                    unknown: UnknownVtbl { query: test_query, add_ref: test_add, release: test_release },
                    window: test_window,
                },
                refs: AtomicU32::new(1),
                hwnd: parent,
            };
            let site_interface = band.query(&SITE).unwrap();
            assert_eq!((site_interface.vt::<SiteVtbl>().set)(site_interface.raw(), &site as *const TestSite as Raw), 0);
            let desk = band.vt::<DeskVtbl>();
            let mut child = null_mut();
            assert_eq!((desk.window)(band.raw(), &mut child), 0);
            assert!(!child.is_null());
            let mut info: BandInfo = std::mem::zeroed();
            info.mask = 127;
            assert_eq!((desk.info)(band.raw(), 3, 0, &mut info), 0);
            assert!(info.actual.x >= info.min.x);
            assert_eq!(info.mode & 0x2408, 0x2408, "top-aligned variable-height band without margins");
            assert_eq!(info.mode & 0x0800, 0, "native sizing grip must remain available");
            assert_eq!((desk.show)(band.raw(), 0), 0);
            assert_eq!((desk.close)(band.raw(), 0), 0);
            assert_eq!((site_interface.vt::<SiteVtbl>().set)(site_interface.raw(), null_mut()), 0);
            assert_eq!(site.refs.load(Ordering::Relaxed), 1);
            DestroyWindow(parent);
        }
    }
    #[test]
    fn com_identity_and_persistence_lifecycle_without_explorer() {
        // SAFETY: exercise our real exported factory in-process, without registry or taskbar changes.
        unsafe {
            let mut raw = null_mut();
            assert_eq!(DllGetClassObject(&CLSID, &CLASS_FACTORY, &mut raw), 0);
            let factory = Com::owned(raw, "factory").unwrap();
            let vt = factory.vt::<FactoryVtbl>();
            assert_eq!((vt.create)(factory.raw(), null_mut(), &DESK_BAND2, &mut raw), 0);
            let band = Com::owned(raw, "band").unwrap();
            for iid in [SITE, PERSIST_STREAM, INPUT, OLE_WINDOW, DESK_BAND] {
                let p = band.query(&iid).unwrap();
                assert!(band.same_object(&p).unwrap());
            }
            let persist = band.query(&PERSIST_STREAM).unwrap();
            let desk = band.vt::<DeskVtbl>();
            let mut composed = 0;
            assert_eq!((desk.can_compose)(band.raw(), &mut composed), 0);
            assert_eq!(composed, 1);
            assert_eq!((desk.set_compose)(band.raw(), 1), 0);
            assert_eq!((desk.get_compose)(band.raw(), &mut composed), 0);
            assert_eq!(composed, 1);
            assert_eq!((desk.set_compose)(band.raw(), 0), 0);
            assert_eq!((desk.get_compose)(band.raw(), &mut composed), 0);
            assert_eq!(composed, 0);
            let mut bytes = 1;
            assert_eq!((persist.vt::<PersistVtbl>().size)(persist.raw(), &mut bytes), 0);
            assert_eq!(bytes, 0);
            assert_eq!(std::mem::offset_of!(DeskVtbl, info), 8 * 8);
            assert_eq!(size_of::<BandInfo>(), 556);
        }
    }
}
