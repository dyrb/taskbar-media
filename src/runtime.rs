//! Owned engine threads and bounded UI delivery. No media or driver calls on the UI thread.

use crate::platform::event::{Event, Wake};
use crate::platform::ffi::*;
use crate::platform::{Error, Result};
use crate::{
    audio, media,
    settings::{Settings, Visual},
    visual::Frame,
};
use std::collections::VecDeque;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

pub const MESSAGE: u32 = 0x8000 + 44;
pub const FINISHED: u32 = 0x8000 + 47;
const COMMAND: u32 = 1 << 16;
const QUEUE_LIMIT: usize = 8;
static NEXT_COOKIE: AtomicUsize = AtomicUsize::new(1);
static ACTIVE: AtomicBool = AtomicBool::new(false);
struct StartGuard;
impl Drop for StartGuard {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Default)]
pub struct State {
    pub snapshot: Option<Arc<media::Snapshot>>,
    pub frame: Frame,
    pub error: Option<Error>,
    pub audio_error: Option<Error>,
    pub artwork: Option<Arc<crate::artwork::Image>>,
    pub artwork_error: Option<Error>,
    pub busy: bool,
    generation: u64,
}

struct AudioConfig {
    settings: Settings,
    source: String,
    generation: u64,
    visible: bool,
    playing: bool,
    segment: Arc<Event>,
}

impl AudioConfig {
    fn enabled(&self) -> bool {
        self.visible && self.playing && self.settings.visual != Visual::Off
    }
}

struct Shared {
    state: Mutex<State>,
    queue: Mutex<VecDeque<(u64, media::Command)>>,
    audio: Mutex<AudioConfig>,
    audio_wake: Wake,
    audio_shutdown: Event,
    wake: Wake,
    stop: Event,
    done: Arc<Event>,
    hwnd: AtomicUsize,
    pending: AtomicBool,
    cookie: usize,
    _activity: crate::platform::Activity,
}

impl Shared {
    fn notify(&self) {
        let hwnd = self.hwnd.load(Ordering::Acquire);
        if hwnd != 0 && !self.pending.swap(true, Ordering::AcqRel) {
            // SAFETY: message carries only a cookie, never a pointer into the runtime.
            if unsafe { PostMessageW(hwnd as Raw, MESSAGE, self.cookie, 0) } == 0 {
                self.pending.store(false, Ordering::Release);
            }
        }
    }

    fn playing(&self, playing: bool) {
        let mut audio = lock(&self.audio);
        if audio.playing != playing {
            audio.playing = playing;
            audio.segment.set();
            self.audio_wake.signal(1);
            lock(&self.state).frame = Frame::default();
        }
    }

    fn cancel(&self) {
        self.hwnd.store(0, Ordering::Release);
        self.stop.set();
        lock(&self.audio).segment.set();
        self.audio_wake.signal(1);
    }
}

pub struct Runtime {
    shared: Arc<Shared>,
}

impl Runtime {
    pub fn start(hwnd: Raw, settings: &Settings) -> Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State { busy: true, ..State::default() }),
            queue: Mutex::new(VecDeque::with_capacity(QUEUE_LIMIT)),
            audio: Mutex::new(AudioConfig {
                settings: settings.clone(),
                source: String::new(),
                generation: 0,
                visible: false,
                playing: false,
                segment: Arc::new(Event::new()?),
            }),
            audio_wake: Wake::new()?,
            audio_shutdown: Event::new()?,
            wake: Wake::new()?,
            stop: Event::new()?,
            done: Arc::new(Event::new()?),
            hwnd: AtomicUsize::new(hwnd as usize),
            pending: AtomicBool::new(false),
            cookie: NEXT_COOKIE.fetch_add(1, Ordering::Relaxed),
            _activity: crate::platform::Activity::new(),
        });
        // Never pile up replacements behind a synchronous COM call or driver that will not return.
        if ACTIVE.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return Err(Error::new("another media engine is still active", 0x800700aau32 as i32));
        }
        let guard = StartGuard;
        let mut module = null_mut();
        // SAFETY: FROM_ADDRESS retains the containing module before a worker can execute its code.
        if unsafe { GetModuleHandleExW(4, worker as *const () as *const u16, &mut module) } == 0 {
            return Err(Error::last("retain engine module"));
        }
        let argument = Box::into_raw(Box::new((shared.clone(), module as usize)));
        // SAFETY: worker consumes exactly one Box; all shared state is Send + Sync.
        let thread = unsafe { CreateThread(null_mut(), 0, worker, argument.cast(), 0, null_mut()) };
        if thread.is_null() {
            let error = Error::last("start engine thread");
            // SAFETY: creation failed, ownership has not transferred.
            unsafe {
                drop(Box::from_raw(argument));
                FreeLibrary(module);
            }
            return Err(error);
        }
        // SAFETY: the thread holds its module and state; no UI join or detached borrowed pointer.
        unsafe {
            CloseHandle(thread);
        }
        std::mem::forget(guard);
        Ok(Self { shared })
    }

    pub fn cookie(&self) -> usize {
        self.shared.cookie
    }
    pub fn stop(&self) {
        self.shared.cancel();
    }
    pub(crate) fn is_stopping(&self) -> bool {
        self.shared.stop.wait(0).unwrap_or(true)
    }
    pub(crate) fn request_restart(&self) {
        self.shared.stop.set();
        lock(&self.shared.audio).segment.set();
        self.shared.audio_wake.signal(1);
    }
    /// Wait only from a non-UI owner or an explicit lifecycle check.
    pub fn wait_stopped(&self, timeout: Duration) -> Result<bool> {
        self.shared.done.wait(timeout.as_millis().min((INFINITE - 1) as u128) as u32)
    }
    #[cfg(test)]
    pub(crate) fn done(&self) -> Arc<Event> {
        self.shared.done.clone()
    }

    pub fn read(&self) -> State {
        let state = lock(&self.shared.state);
        // Reset under the state lock, before producers can publish a new state.
        self.shared.pending.store(false, Ordering::Release);
        state.clone()
    }

    pub fn configure(&self, settings: &Settings, visible: bool) {
        let mut audio = lock(&self.shared.audio);
        let artwork_changed = audio.settings.artwork != settings.artwork;
        let source_changed = audio.settings.player_audio != settings.player_audio;
        let capture_changed = audio.settings.fps != settings.fps
            || source_changed
            || audio.settings.visual != settings.visual
            || audio.settings.release_ms != settings.release_ms
            || audio.visible != visible;
        audio.settings = settings.clone();
        if capture_changed {
            audio.visible = visible;
            audio.segment.set();
            self.shared.audio_wake.signal(1);
            lock(&self.shared.state).frame = Frame::default();
        }
        if source_changed {
            lock(&self.shared.state).audio_error = None;
            self.shared.notify();
        }
        if artwork_changed {
            self.shared.wake.signal(COMMAND | media::MEDIA);
        }
    }

    pub fn command(&self, command: media::Command) -> Result<()> {
        if self.shared.stop.wait(0)? {
            return Err(Error::new("engine has stopped", E_ABORT));
        }
        let generation = lock(&self.shared.state).generation;
        let mut queue = lock(&self.shared.queue);
        if queue.len() == QUEUE_LIMIT {
            return Err(Error::new("command queue is full", E_BOUNDS));
        }
        queue.push_back((generation, command));
        self.shared.wake.signal(COMMAND);
        Ok(())
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shared.cancel();
    }
}

unsafe extern "system" fn worker(argument: Raw) -> u32 {
    // SAFETY: exactly the allocation transferred by successful CreateThread.
    let (shared, module) = *unsafe { Box::from_raw(argument as *mut (Arc<Shared>, usize)) };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&shared)));
    if !matches!(result, Ok(Ok(()))) && !shared.stop.wait(0).unwrap_or(true) {
        let mut state = lock(&shared.state);
        state.busy = false;
        state.snapshot = None;
        state.error = Some(match result {
            Ok(Err(e)) => e,
            _ => Error::new("engine thread failed", E_FAIL),
        });
        drop(state);
        shared.playing(false);
        shared.notify();
    }
    shared.stop.set();
    ACTIVE.store(false, Ordering::Release);
    let hwnd = shared.hwnd.load(Ordering::Acquire);
    if hwnd != 0 {
        // SAFETY: completion carries only a generation cookie. A destroyed view clears its HWND.
        unsafe {
            PostMessageW(hwnd as Raw, FINISHED, shared.cookie, 0);
        }
    }
    let done = shared.done.clone();
    drop(shared);
    done.set();
    drop(done);
    // SAFETY: no Rust objects remain on this stack. Release the retained module and exit atomically.
    unsafe { FreeLibraryAndExitThread(module as Raw, 0) }
}

struct AudioWorker {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for AudioWorker {
    fn drop(&mut self) {
        self.shared.audio_shutdown.set();
        lock(&self.shared.audio).segment.set();
        self.shared.audio_wake.signal(1);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(shared: &Arc<Shared>) -> Result<()> {
    let audio_state = shared.clone();
    let thread = std::thread::Builder::new()
        .name("media-audio".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| audio_run(&audio_state)));
            if let Err(e) = result.unwrap_or(Err(Error::new("audio worker failed", E_FAIL))) {
                lock(&audio_state.state).audio_error = Some(e);
                audio_state.notify();
            }
        })
        .map_err(|_| Error::new("start audio worker", E_FAIL))?;
    let _audio = AudioWorker { shared: shared.clone(), thread: Some(thread) };
    recover_media(
        &shared.stop,
        &[250, 500, 1000, 2000, 4000],
        || run_media(shared),
        |_| {
            {
                let mut state = lock(&shared.state);
                state.busy = true;
                state.error = None;
                state.snapshot = None;
                state.artwork = None;
                state.artwork_error = None;
                state.frame = Frame::default();
                state.generation = state.generation.wrapping_add(1);
            }
            lock(&shared.queue).clear();
            shared.playing(false);
            shared.notify();
        },
    )
}

fn recover_media(
    stop: &Event,
    delays: &[u32],
    mut connect: impl FnMut() -> Result<()>,
    mut retrying: impl FnMut(Error),
) -> Result<()> {
    let mut attempt = 0;
    loop {
        if stop.wait(0)? {
            return Ok(());
        }
        match connect() {
            Err(error)
                if matches!(error.code as u32, 0x80040154 | 0x80010108 | 0x800706ba) && attempt < delays.len() =>
            {
                // The previous manager and subscriptions have been dropped before another attempt.
                // Never retry a timed-out asynchronous request: its provider may still be running.
                retrying(error);
                if stop.wait(delays[attempt])? {
                    return Ok(());
                }
                attempt += 1;
            }
            result => return result,
        }
    }
}

fn run_media(shared: &Arc<Shared>) -> Result<()> {
    let manager = media::Manager::connect(&shared.stop)?;
    let mut artwork_key = (0u64, false);
    manager.watch_controlled(
        &shared.stop,
        None,
        &shared.wake,
        |session, changed| {
            if changed {
                let mut state = lock(&shared.state);
                state.generation = state.generation.wrapping_add(1);
            }
            // One command per turn: a stream of clicks cannot starve media notifications.
            let item = lock(&shared.queue).pop_front();
            if let Some((generation, command)) = item
                && generation == lock(&shared.state).generation
            {
                let result = match session {
                    Some(s) => s.command(command, &shared.stop).and_then(|accepted| {
                        if accepted { Ok(()) } else { Err(Error::new("player declined command", E_FAIL)) }
                    }),
                    None => Err(Error::new("no current media session", E_FAIL)),
                };
                lock(&shared.state).error = result.err();
                shared.notify();
            }
            if !lock(&shared.queue).is_empty() {
                shared.wake.signal(COMMAND);
            }
            Ok(())
        },
        |update| {
            let (playing, source, generation) = {
                let mut state = lock(&shared.state);
                state.busy = false;
                match update {
                    media::Update::Changed(snapshot) => {
                        state.snapshot = snapshot.cloned().map(Arc::new);
                        state.error = None;
                    }
                    media::Update::Unavailable(e) => {
                        state.error = Some(e);
                        state.snapshot = None;
                    }
                }
                (
                    state.snapshot.as_ref().is_some_and(|s| s.playback == media::Playback::Playing),
                    state.snapshot.as_ref().map(|s| s.source.clone()).unwrap_or_default(),
                    state.generation,
                )
            };
            {
                let mut config = lock(&shared.audio);
                let source_changed = config.source != source || config.generation != generation;
                let playing_changed = config.playing != playing;
                config.playing = playing;
                if source_changed {
                    config.source = source;
                    config.generation = generation;
                }
                if playing_changed || source_changed && config.settings.player_audio {
                    config.segment.set();
                    lock(&shared.state).frame = Frame::default();
                    shared.audio_wake.signal(1);
                }
            }
            shared.notify();
            Ok(())
        },
        |properties, invalidated, revision| {
            let enabled = lock(&shared.audio).settings.artwork;
            let key = (revision, enabled);
            if key != artwork_key {
                artwork_key = key;
                {
                    let mut state = lock(&shared.state);
                    state.artwork = None;
                    state.artwork_error = None;
                }
                shared.notify();
                if enabled && let Some(properties) = properties {
                    let image = media::thumbnail(properties).and_then(|reference| {
                        reference
                            .as_ref()
                            .map(|r| crate::artwork::read_cancellable(r, &shared.stop, Some(invalidated)))
                            .transpose()
                    });
                    if !shared.stop.wait(0)? && !invalidated.wait(0)? {
                        let mut state = lock(&shared.state);
                        match image {
                            Ok(image) => state.artwork = image.map(Arc::new),
                            Err(e) => state.artwork_error = Some(e),
                        }
                    }
                    shared.notify();
                }
            }
            Ok(())
        },
    )
}

fn audio_run(shared: &Shared) -> Result<()> {
    shared.audio_wake.signal(1);
    loop {
        let handles = [shared.audio_shutdown.raw(), shared.audio_wake.raw()];
        // SAFETY: owned events outlive this worker; no polling when capture is disabled.
        match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) } {
            0 => return Ok(()),
            1 => {}
            _ => return Err(Error::last("wait for audio configuration")),
        }
        shared.audio_wake.take();
        let (settings, stop, enabled, source) = {
            let mut config = lock(&shared.audio);
            config.segment = Arc::new(Event::new()?);
            (config.settings.clone(), config.segment.clone(), config.enabled(), config.source.clone())
        };
        if !enabled {
            continue;
        }
        let target = if settings.player_audio { audio::Target::Player(source) } else { audio::Target::Default };
        let result = audio::monitor_tuned(
            &target,
            &stop,
            None,
            settings.fps,
            settings.visual == Visual::Spectrum,
            Duration::from_millis(settings.release_ms as u64),
            |update| {
                if stop.wait(0)? {
                    return Ok(());
                }
                {
                    let mut state = lock(&shared.state);
                    match update {
                        audio::Update::Frame(frame) => state.frame = frame.clone(),
                        audio::Update::Ready { .. } => state.audio_error = None,
                        audio::Update::Unavailable(e) => {
                            state.audio_error = Some(e);
                            state.frame = Frame::default();
                        }
                    }
                }
                shared.notify();
                Ok(())
            },
        );
        if let Err(e) = result
            && !stop.wait(0).unwrap_or(true)
        {
            lock(&shared.state).audio_error = Some(e);
            shared.notify();
        }
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut Raw) -> i32;
    fn FreeLibrary(module: Raw) -> i32;
    fn FreeLibraryAndExitThread(module: Raw, code: u32) -> !;
    fn CreateThread(
        attributes: Raw,
        stack: usize,
        entry: unsafe extern "system" fn(Raw) -> u32,
        argument: Raw,
        flags: u32,
        id: *mut u32,
    ) -> Raw;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn PostMessageW(hwnd: Raw, message: u32, wparam: usize, lparam: isize) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_requires_visible_space_playback_and_a_visual_mode() {
        let mut config = AudioConfig {
            settings: Settings::default(),
            source: String::new(),
            generation: 0,
            visible: false,
            playing: false,
            segment: Arc::new(Event::new().unwrap()),
        };
        for visible in [false, true] {
            for playing in [false, true] {
                for visual in [Visual::Off, Visual::Wave, Visual::Spectrum] {
                    config.visible = visible;
                    config.playing = playing;
                    config.settings.visual = visual;
                    assert_eq!(config.enabled(), visible && playing && visual != Visual::Off);
                }
            }
        }
    }

    #[test]
    fn media_recovery_is_bounded_and_does_not_retry_timeouts() {
        let stop = Event::new().unwrap();
        let transient = Error::new("media class not ready", 0x80040154u32 as i32);
        let mut calls = 0;
        recover_media(
            &stop,
            &[0, 0],
            || {
                calls += 1;
                if calls < 3 { Err(transient) } else { Ok(()) }
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(calls, 3);
        calls = 0;
        assert_eq!(
            recover_media(
                &stop,
                &[0, 0],
                || {
                    calls += 1;
                    Err(transient)
                },
                |_| {}
            ),
            Err(transient)
        );
        assert_eq!(calls, 3);
        calls = 0;
        let timeout = Error::new("pending provider request", ERROR_TIMEOUT);
        assert_eq!(
            recover_media(
                &stop,
                &[0, 0],
                || {
                    calls += 1;
                    Err(timeout)
                },
                |_| {}
            ),
            Err(timeout)
        );
        assert_eq!(calls, 1);
        let permanent = Error::new("access denied", 0x80070005u32 as i32);
        assert_eq!(
            recover_media(&stop, &[0, 0], || Err(permanent), |_| panic!("permanent errors must not retry")),
            Err(permanent)
        );
    }

    #[test]
    fn cancellation_interrupts_media_recovery_before_another_attempt() {
        let stop = Event::new().unwrap();
        let mut calls = 0;
        recover_media(
            &stop,
            &[30_000],
            || {
                calls += 1;
                Err(Error::new("media class not ready", 0x80040154u32 as i32))
            },
            |_| stop.set(),
        )
        .unwrap();
        assert_eq!(calls, 1);
    }
}
