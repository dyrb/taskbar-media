//! Windows GSMTC sessions. All interfaces are owned by the calling MTA thread.

use crate::platform::async_op::{AddEvent, Operation, RemoveEvent, Subscription};
use crate::platform::com::{Apartment, Com, HString};
use crate::platform::event::{Event, Wake};
use crate::platform::ffi::*;
use crate::platform::{Error, Result, check};
use std::ptr::null_mut;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SESSION: u32 = 1;
pub(crate) const MEDIA: u32 = 2;
const PLAYBACK: u32 = 4;
const TIMELINE: u32 = 8;
const MAX_SESSIONS: u32 = 64;
const TEXT_LIMIT: usize = 4096;
const TIMEOUT: Duration = Duration::from_secs(10);
const MANAGER_STATICS: Guid = Guid::from_u128(0x2050c4ee_11a0_57de_aed7_c97c70338245);
const COMPLETE_MANAGER: Guid = Guid::from_u128(0x10f0074e_923d_5510_8f4a_dde37754ca0e);
const COMPLETE_PROPERTIES: Guid = Guid::from_u128(0x84593a3d_951a_55b6_8353_5205e577797b);
const COMPLETE_BOOL: Guid = Guid::from_u128(0xc1d3d1a2_ae17_5a5f_b5a2_bdcc8844889a);
const CURRENT_CHANGED: Guid = Guid::from_u128(0x228bd0ed_1fa2_5e9b_a6ec_42566173103b);
const SESSIONS_CHANGED: Guid = Guid::from_u128(0x2e2a8630_dc8c_530a_9746_bc984d4b029e);
const MEDIA_CHANGED: Guid = Guid::from_u128(0x0f2ce2b7_afa7_5ed0_8cb6_8c40cf9b3a5f);
const PLAYBACK_CHANGED: Guid = Guid::from_u128(0x2bdf1426_d41f_5896_897f_efc0b0fa7392);
const TIMELINE_CHANGED: Guid = Guid::from_u128(0xe8bf62af_fac1_5fff_9053_0bf191ae777e);

type GetObject = unsafe extern "system" fn(Raw, *mut Raw) -> i32;
type GetString = unsafe extern "system" fn(Raw, *mut Raw) -> i32;

#[repr(C)]
struct StaticsVtbl {
    inspectable: InspectableVtbl,
    request: GetObject,
}

#[repr(C)]
struct ManagerVtbl {
    inspectable: InspectableVtbl,
    current: GetObject,
    sessions: GetObject,
    add_current: AddEvent,
    remove_current: RemoveEvent,
    add_sessions: AddEvent,
    remove_sessions: RemoveEvent,
}

#[repr(C)]
struct SessionsVtbl {
    inspectable: InspectableVtbl,
    get_at: unsafe extern "system" fn(Raw, u32, *mut Raw) -> i32,
    size: unsafe extern "system" fn(Raw, *mut u32) -> i32,
}

#[repr(C)]
struct SessionVtbl {
    inspectable: InspectableVtbl,
    source: GetString,
    properties: GetObject,
    timeline: GetObject,
    playback: GetObject,
    commands: [GetObject; 11],
    _change_repeat_rate_shuffle_position: [usize; 4],
    add_timeline: AddEvent,
    remove_timeline: RemoveEvent,
    add_playback: AddEvent,
    remove_playback: RemoveEvent,
    add_media: AddEvent,
    remove_media: RemoveEvent,
}

#[repr(C)]
struct PropertiesVtbl {
    inspectable: InspectableVtbl,
    title: GetString,
    _subtitle: GetString,
    _album_artist: GetString,
    artist: GetString,
    album: GetString,
    _track_genres_album_count_type: [usize; 4],
    thumbnail: GetObject,
}

#[repr(C)]
struct PlaybackVtbl {
    inspectable: InspectableVtbl,
    controls: GetObject,
    status: unsafe extern "system" fn(Raw, *mut i32) -> i32,
}

#[repr(C)]
struct ControlsVtbl {
    inspectable: InspectableVtbl,
    enabled: [unsafe extern "system" fn(Raw, *mut u8) -> i32; 15],
}

#[repr(C)]
struct TimelineVtbl {
    inspectable: InspectableVtbl,
    get: [unsafe extern "system" fn(Raw, *mut i64) -> i32; 6],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Playback {
    Closed,
    Opened,
    Changing,
    Stopped,
    Playing,
    Paused,
    Unknown(i32),
}

impl From<i32> for Playback {
    fn from(value: i32) -> Self {
        match value {
            0 => Self::Closed,
            1 => Self::Opened,
            2 => Self::Changing,
            3 => Self::Stopped,
            4 => Self::Playing,
            5 => Self::Paused,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    Toggle,
}

impl Command {
    fn method(self) -> usize {
        match self {
            Self::Play => 0,
            Self::Pause => 1,
            Self::Stop => 2,
            Self::Next => 6,
            Self::Previous => 7,
            Self::Toggle => 10,
        }
    }

    fn control(self) -> usize {
        self.method()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controls(u16);

impl Controls {
    pub fn supports(self, command: Command) -> bool {
        self.0 & (1 << command.control()) != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub source: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playback: Playback,
    pub controls: Controls,
    /// Windows TimeSpan values: 100-nanosecond units.
    pub position: i64,
    pub duration: i64,
}

pub struct Session {
    inner: Com,
    _apartment: Rc<Apartment>,
}

impl Session {
    pub fn source(&self) -> Result<String> {
        // SAFETY: Session owns the SDK's IGlobalSystemMediaTransportControlsSession.
        unsafe { read_string(&self.inner, self.inner.vt::<SessionVtbl>().source) }
    }

    pub fn snapshot(&self, stop: &Event) -> Result<Snapshot> {
        Ok(self.properties(stop, None)?.0)
    }

    fn properties(&self, stop: &Event, invalidated: Option<&Event>) -> Result<(Snapshot, Com)> {
        // SAFETY: all vtables below are returned by the corresponding typed SDK methods.
        unsafe {
            let properties = self.media_properties(stop, invalidated)?;
            let properties_vt = properties.vt::<PropertiesVtbl>();
            let (playback, controls) = self.playback()?;
            let (position, duration) = self.timeline()?;
            let snapshot = Snapshot {
                source: self.source()?,
                title: read_string(&properties, properties_vt.title)?,
                artist: read_string(&properties, properties_vt.artist)?,
                album: read_string(&properties, properties_vt.album)?,
                playback,
                controls,
                position,
                duration,
            };
            Ok((snapshot, properties))
        }
    }

    fn media_properties(&self, stop: &Event, invalidated: Option<&Event>) -> Result<Com> {
        // SAFETY: the session getter returns exactly IAsyncOperation<MediaProperties>.
        unsafe {
            let operation = object(&self.inner, self.inner.vt::<SessionVtbl>().properties, "read media properties")?;
            Com::owned(
                Operation::<Raw>::new(operation, COMPLETE_PROPERTIES)?.wait_cancellable(stop, invalidated, TIMEOUT)?,
                "media properties",
            )
        }
    }

    pub fn artwork(&self, stop: &Event) -> Result<Option<crate::Image>> {
        let properties = self.media_properties(stop, None)?;
        thumbnail(&properties)?.as_ref().map(|r| crate::artwork::read(r, stop)).transpose()
    }

    pub fn timeline(&self) -> Result<(i64, i64)> {
        // SAFETY: typed GetTimelineProperties result and SDK TimeSpan output layout.
        unsafe {
            let timeline = object(&self.inner, self.inner.vt::<SessionVtbl>().timeline, "read media timeline")?;
            let mut start = 0;
            let mut end = 0;
            let mut position = 0;
            let get = &timeline.vt::<TimelineVtbl>().get;
            check(get[0](timeline.raw(), &mut start), "timeline start")?;
            check(get[1](timeline.raw(), &mut end), "timeline end")?;
            check(get[4](timeline.raw(), &mut position), "timeline position")?;
            Ok((position, end.saturating_sub(start).max(0)))
        }
    }

    pub fn playback(&self) -> Result<(Playback, Controls)> {
        // SAFETY: interfaces are the typed results of Session/GetPlaybackInfo/GetControls.
        unsafe {
            let playback = object(&self.inner, self.inner.vt::<SessionVtbl>().playback, "read playback state")?;
            let vt = playback.vt::<PlaybackVtbl>();
            let controls = object(&playback, vt.controls, "read playback controls")?;
            let mut status = 0;
            check((vt.status)(playback.raw(), &mut status), "playback status")?;
            let mut supported = 0;
            for (i, get) in controls.vt::<ControlsVtbl>().enabled.iter().enumerate() {
                let mut enabled = 0;
                check(get(controls.raw(), &mut enabled), "playback control capability")?;
                if enabled != 0 {
                    supported |= 1 << i;
                }
            }
            Ok((Playback::from(status), Controls(supported)))
        }
    }

    pub fn command(&self, command: Command, stop: &Event) -> Result<bool> {
        if !self.playback()?.1.supports(command) {
            return Err(Error::new("player does not support this command", 0x80004001u32 as i32));
        }
        // SAFETY: commands without parameters have the SDK's IAsyncOperation<boolean> ABI.
        unsafe {
            let raw =
                object(&self.inner, self.inner.vt::<SessionVtbl>().commands[command.method()], "send media command")?;
            Ok(Operation::<u8>::new(raw, COMPLETE_BOOL)?.wait(stop, TIMEOUT)? != 0)
        }
    }

    fn subscribe(&self, wake: &Wake, invalidated: &Arc<Event>) -> Result<Vec<Subscription>> {
        // SAFETY: each event's delegate IID and add/remove pair are from windows.media.control.h.
        unsafe {
            let vt = self.inner.vt::<SessionVtbl>();
            Ok(vec![
                Subscription::new_cancellable(
                    &self.inner,
                    vt.add_media,
                    vt.remove_media,
                    MEDIA_CHANGED,
                    wake,
                    MEDIA,
                    Some(invalidated.clone()),
                )?,
                Subscription::new(&self.inner, vt.add_playback, vt.remove_playback, PLAYBACK_CHANGED, wake, PLAYBACK)?,
                Subscription::new(&self.inner, vt.add_timeline, vt.remove_timeline, TIMELINE_CHANGED, wake, TIMELINE)?,
            ])
        }
    }
}

pub enum Update<'a> {
    Changed(Option<&'a Snapshot>),
    Unavailable(Error),
}

pub struct Manager {
    manager: Com,
    // COM objects are destroyed before this thread's Windows Runtime apartment.
    _apartment: Rc<Apartment>,
}

impl Manager {
    pub fn connect(stop: &Event) -> Result<Self> {
        if stop.wait(0)? {
            return Err(Error::new("operation cancelled", E_ABORT));
        }
        let apartment = Rc::new(Apartment::mta()?);
        let class = HString::new("Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager")?;
        // SAFETY: exact factory IID and WinRT class name from the SDK; writable out parameters.
        unsafe {
            let mut raw = null_mut();
            check(RoGetActivationFactory(class.raw(), &MANAGER_STATICS, &mut raw), "activate media session manager")?;
            let factory = Com::owned(raw, "media session manager factory")?;
            let operation = object(&factory, factory.vt::<StaticsVtbl>().request, "request media session manager")?;
            let manager = Com::owned(
                Operation::<Raw>::new(operation, COMPLETE_MANAGER)?.wait(stop, TIMEOUT)?,
                "media session manager",
            )?;
            Ok(Self { manager, _apartment: apartment })
        }
    }

    pub fn current(&self) -> Result<Option<Session>> {
        // SAFETY: the manager owns the exact manager interface. Null is a valid empty result.
        unsafe {
            let mut raw = null_mut();
            check((self.manager.vt::<ManagerVtbl>().current)(self.manager.raw(), &mut raw), "current media session")?;
            if raw.is_null() {
                Ok(None)
            } else {
                Ok(Some(Session {
                    inner: Com::owned(raw, "current media session")?,
                    _apartment: self._apartment.clone(),
                }))
            }
        }
    }

    pub fn sessions(&self) -> Result<Vec<Session>> {
        // SAFETY: GetSessions returns IVectorView<GlobalSystemMediaTransportControlsSession>.
        unsafe {
            let sessions = object(&self.manager, self.manager.vt::<ManagerVtbl>().sessions, "list media sessions")?;
            let vt = sessions.vt::<SessionsVtbl>();
            let mut count = 0;
            check((vt.size)(sessions.raw(), &mut count), "media session count")?;
            if count > MAX_SESSIONS {
                return Err(Error::new("too many media sessions", E_BOUNDS));
            }
            let mut result = Vec::with_capacity(count as usize);
            for i in 0..count {
                let mut raw = null_mut();
                check((vt.get_at)(sessions.raw(), i, &mut raw), "media session")?;
                result.push(Session { inner: Com::owned(raw, "media session")?, _apartment: self._apartment.clone() });
            }
            Ok(result)
        }
    }

    pub fn watch(
        &self,
        stop: &Event,
        duration: Option<Duration>,
        changed: impl FnMut(Update<'_>) -> Result<()>,
    ) -> Result<()> {
        let wake = Wake::new()?;
        self.watch_controlled(stop, duration, &wake, |_, _| Ok(()), changed, |_, _, _| Ok(()))
    }

    pub(crate) fn watch_controlled(
        &self,
        stop: &Event,
        duration: Option<Duration>,
        wake: &Wake,
        mut commands: impl FnMut(Option<&Session>, bool) -> Result<()>,
        mut changed: impl FnMut(Update<'_>) -> Result<()>,
        mut properties_changed: impl FnMut(Option<&Com>, &Event, u64) -> Result<()>,
    ) -> Result<()> {
        let invalidated = Arc::new(Event::new()?);
        // SAFETY: exact manager event signatures and delegate IIDs from the SDK.
        let _manager_subscriptions = unsafe {
            let vt = self.manager.vt::<ManagerVtbl>();
            [
                Subscription::new_cancellable(
                    &self.manager,
                    vt.add_current,
                    vt.remove_current,
                    CURRENT_CHANGED,
                    wake,
                    SESSION,
                    Some(invalidated.clone()),
                )?,
                Subscription::new_cancellable(
                    &self.manager,
                    vt.add_sessions,
                    vt.remove_sessions,
                    SESSIONS_CHANGED,
                    wake,
                    SESSION,
                    Some(invalidated.clone()),
                )?,
            ]
        };
        let deadline = duration
            .map(|d| Instant::now().checked_add(d).ok_or(Error::new("watch duration is too large", E_INVALIDARG)))
            .transpose()?;
        let mut subscriptions = Vec::new();
        let mut current: Option<Session> = None;
        let mut snapshot: Option<Snapshot> = None;
        let mut properties = None;
        let mut properties_revision = 0u64;
        let mut first = true;
        let mut last_error = None;
        wake.signal(SESSION);
        loop {
            let timeout = deadline.map_or(INFINITE, |d| {
                d.saturating_duration_since(Instant::now()).as_millis().min((INFINITE - 1) as u128) as u32
            });
            let handles = [stop.raw(), wake.raw()];
            // SAFETY: handles outlive the wait, on an initialized MTA worker/CLI thread.
            match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout) } {
                0 | 258 => return Ok(()),
                1 => {}
                _ => return Err(Error::last("wait for media change")),
            }
            let bits = wake.take();
            let mut new_session = false;
            if bits & SESSION != 0 {
                let next = self.current()?;
                let same = match (&current, &next) {
                    (Some(a), Some(b)) => a.inner.same_object(&b.inner)?,
                    (None, None) => true,
                    _ => false,
                };
                if !same {
                    subscriptions.clear();
                    current = next;
                    snapshot = None;
                    properties = None;
                    properties_revision = properties_revision.wrapping_add(1);
                    new_session = true;
                    if let Some(session) = &current {
                        subscriptions = session.subscribe(wake, &invalidated)?;
                    }
                }
            }
            commands(current.as_ref(), new_session)?;
            let refreshed = (|| -> Result<bool> {
                let Some(session) = &current else { return Ok(new_session) };
                if snapshot.is_none() || bits & MEDIA != 0 {
                    invalidated.reset();
                    let (next, next_properties) = session.properties(stop, Some(&invalidated))?;
                    if invalidated.wait(0)? {
                        return Err(Error::new("media request superseded", E_ABORT));
                    }
                    // Asynchronous metadata must still belong to the current COM session.
                    let still_current = match self.current()? {
                        Some(s) => session.inner.same_object(&s.inner)?,
                        None => false,
                    };
                    if !still_current {
                        wake.signal(SESSION);
                        return Ok(false);
                    }
                    let differs = snapshot.as_ref() != Some(&next);
                    snapshot = Some(next);
                    properties = Some(next_properties);
                    properties_revision = properties_revision.wrapping_add(1);
                    return Ok(differs);
                }
                let playback = if bits & PLAYBACK != 0 { Some(session.playback()?) } else { None };
                let timeline = if bits & TIMELINE != 0 { Some(session.timeline()?) } else { None };
                let previous = snapshot.as_mut().expect("snapshot checked above");
                let mut differs = false;
                if let Some((status, controls)) = playback {
                    differs |= previous.playback != status || previous.controls != controls;
                    previous.playback = status;
                    previous.controls = controls;
                }
                if let Some((position, duration)) = timeline {
                    differs |= previous.position != position || previous.duration != duration;
                    previous.position = position;
                    previous.duration = duration;
                }
                Ok(differs)
            })();
            match refreshed {
                Ok(differs) => {
                    if first || differs || last_error.is_some() {
                        changed(Update::Changed(snapshot.as_ref()))?;
                        first = false;
                    }
                    last_error = None;
                }
                Err(error) if error.code == E_ABORT && stop.wait(0)? => return Ok(()),
                Err(error) if error.code == E_ABORT && invalidated.wait(0)? => continue,
                // Do not pile up new requests behind a provider which ignores cancellation.
                Err(error) if error.code == ERROR_TIMEOUT => return Err(error),
                Err(error) => {
                    if last_error != Some(error) {
                        changed(Update::Unavailable(error))?;
                    }
                    last_error = Some(error);
                }
            }
            properties_changed(properties.as_ref(), &invalidated, properties_revision)?;
        }
    }
}

pub(crate) fn thumbnail(properties: &Com) -> Result<Option<Com>> {
    // SAFETY: only cached, typed GSMTC media properties from the watcher are passed here.
    unsafe {
        let mut raw = null_mut();
        check((properties.vt::<PropertiesVtbl>().thumbnail)(properties.raw(), &mut raw), "read system thumbnail")?;
        if raw.is_null() { Ok(None) } else { Ok(Some(Com::owned(raw, "thumbnail reference")?)) }
    }
}

unsafe fn object(source: &Com, get: GetObject, operation: &'static str) -> Result<Com> {
    let mut raw = null_mut();
    // SAFETY: caller supplies the exact ABI and source interface for this getter.
    unsafe {
        check(get(source.raw(), &mut raw), operation)?;
        Com::owned(raw, operation)
    }
}

unsafe fn read_string(source: &Com, get: GetString) -> Result<String> {
    let mut raw = null_mut();
    // SAFETY: the getter returns an owned HSTRING, including null for an empty string.
    unsafe {
        check(get(source.raw(), &mut raw), "read media text")?;
        Ok(HString::owned(raw).text(TEXT_LIMIT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_vtable_offsets_match_windows_sdk() {
        assert_eq!(std::mem::offset_of!(SessionVtbl, commands), 10 * 8);
        assert_eq!(std::mem::offset_of!(SessionVtbl, add_timeline), 25 * 8);
        assert_eq!(std::mem::offset_of!(SessionVtbl, add_media), 29 * 8);
        assert_eq!(std::mem::offset_of!(ManagerVtbl, add_current), 8 * 8);
        assert_eq!(Command::Next.method(), 6);
        assert!(Controls((1 << 6) | 1).supports(Command::Next));
        assert!(!Controls(1).supports(Command::Pause));
    }
}
