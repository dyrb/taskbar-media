use std::io::{self, Write};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use taskbar_media_band::audio;
use taskbar_media_band::media::{Command, Manager, Snapshot};
use taskbar_media_band::platform::event::Event;
use taskbar_media_band::platform::ffi::{E_ABORT, SetConsoleCtrlHandler};

static CANCEL: OnceLock<Arc<Event>> = OnceLock::new();

unsafe extern "system" fn cancel(kind: u32) -> i32 {
    if kind <= 1 {
        if let Some(event) = CANCEL.get() {
            event.set();
        }
        1
    } else {
        0
    }
}

fn help() {
    println!(concat!("Taskbar Media ", env!("CARGO_PKG_VERSION")));
    println!(
        r#"
Usage: taskbar-media COMMAND

  gui          Open the native media view
  status       Show the current Windows media session
  sessions     List available media sessions
  watch        Print media changes until Ctrl+C
  devices      List active audio output identifiers
  meter        Analyse real output audio until Ctrl+C
  artwork      Read the system thumbnail and show its dimensions
  play         Request playback
  pause        Request pause
  toggle       Toggle playback/pause
  next         Request the next track
  previous     Request the previous track
  stop         Request stop
  register     Register the Windows 10 taskbar band
  enable       Register and show the Windows 10 taskbar band
  disable      Hide the taskbar band
  band-status  Query whether Windows shows the band
  unregister   Remove this program's taskbar registration
  help         Show this help

Options:
  watch/meter/gui --seconds N   Stop after N seconds (1..86400)
  meter --device ID            Output identifier from devices (default: system default)
  meter --process PID          Capture one process tree instead of the device mix
  meter --fps N                Analysis rate limit (1..60, default 15)
  meter --spectrum             Calculate the frequency spectrum
  enable/register/unregister --machine   Use machine COM registration (administrator)
  enable/register/unregister --user      Use per-user COM registration

Transport commands affect the current Windows media session only.
The GUI saves changed settings beside the program. Registration is explicit.
No logs, network requests, artwork files or startup tasks are created."#
    );
}

fn print_snapshot(snapshot: Option<&Snapshot>, out: &mut impl Write) -> io::Result<()> {
    match snapshot {
        None => writeln!(out, "No media session"),
        Some(s) => {
            writeln!(out, "Source: {}", s.source)?;
            writeln!(out, "State: {:?}", s.playback)?;
            writeln!(out, "Title: {}", s.title)?;
            writeln!(out, "Artist: {}", s.artist)?;
            if !s.album.is_empty() {
                writeln!(out, "Album: {}", s.album)?;
            }
            writeln!(
                out,
                "Position: {:.1} / {:.1} s",
                s.position as f64 / 10_000_000.0,
                s.duration as f64 / 10_000_000.0
            )
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let mut duration = None;
    let mut target = audio::Target::Default;
    let mut fps = 15;
    let mut spectrum = false;
    let mut registration = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--machine" | "--user" if matches!(command.as_str(), "enable" | "register" | "unregister") => {
                if registration.is_some() {
                    return Err("registration scope specified more than once".into());
                }
                registration = Some(if argument == "--machine" {
                    taskbar_media_band::host::Registration::Machine
                } else {
                    taskbar_media_band::host::Registration::User
                });
            }
            "--seconds" if matches!(command.as_str(), "watch" | "meter" | "gui") => {
                let seconds: u64 = args.next().ok_or("--seconds needs a value")?.parse()?;
                if !(1..=86400).contains(&seconds) {
                    return Err("seconds must be 1..86400".into());
                }
                duration = Some(Duration::from_secs(seconds));
            }
            "--fps" if command == "meter" => {
                fps = args.next().ok_or("--fps needs a value")?.parse()?;
                if !(1..=60).contains(&fps) {
                    return Err("fps must be 1..60".into());
                }
            }
            "--device" if command == "meter" => {
                target = audio::Target::Device(args.next().ok_or("--device needs an identifier")?)
            }
            "--process" if command == "meter" => {
                let pid = args.next().ok_or("--process needs a process ID")?.parse()?;
                if pid == 0 {
                    return Err("process ID must be greater than zero".into());
                }
                target = audio::Target::Process(pid);
            }
            "--spectrum" if command == "meter" => spectrum = true,
            _ => return Err("unexpected argument; use taskbar-media help".into()),
        }
    }
    if matches!(command.as_str(), "help" | "--help" | "-h") {
        help();
        return Ok(());
    }
    match command.as_str() {
        "enable" => {
            let shown = taskbar_media_band::host::enable(
                registration.map_or_else(taskbar_media_band::host::Registration::automatic, Ok)?,
            )?;
            println!(
                "{}",
                if shown {
                    "Taskbar band enabled."
                } else {
                    "Show requested. Complete any Windows prompt, then use band-status to verify."
                }
            );
            return Ok(());
        }
        "disable" => {
            taskbar_media_band::host::disable()?;
            println!("Hide requested. Use band-status to verify the shell state.");
            return Ok(());
        }
        "band-status" => {
            println!("{}", if taskbar_media_band::host::is_shown()? { "shown" } else { "hidden" });
            return Ok(());
        }
        "gui" => return Ok(taskbar_media_band::ui::run_for(duration)?),
        "register" => {
            taskbar_media_band::host::register(
                registration.map_or_else(taskbar_media_band::host::Registration::automatic, Ok)?,
            )?;
            println!("Registered. Enable Taskbar Media in taskbar > Toolbars. Explorer was not restarted.");
            return Ok(());
        }
        "unregister" => {
            taskbar_media_band::host::unregister(
                registration.map_or_else(taskbar_media_band::host::Registration::automatic, Ok)?,
            )?;
            println!("Band disabled and registration removed.");
            return Ok(());
        }
        _ => {}
    }
    let action = match command.as_str() {
        "status" | "sessions" | "watch" | "devices" | "meter" | "artwork" => None,
        "play" => Some(Command::Play),
        "pause" => Some(Command::Pause),
        "toggle" => Some(Command::Toggle),
        "next" => Some(Command::Next),
        "previous" => Some(Command::Previous),
        "stop" => Some(Command::Stop),
        _ => return Err("unknown command; use taskbar-media help".into()),
    };
    let stop = Arc::new(Event::new()?);
    CANCEL.set(stop.clone()).map_err(|_| "console cancellation already initialized")?;
    // SAFETY: process-lifetime callback and Event; no application object pointer is exposed.
    if unsafe { SetConsoleCtrlHandler(Some(cancel), 1) } == 0 {
        return Err(taskbar_media_band::platform::Error::last("SetConsoleCtrlHandler").into());
    }
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if command == "devices" {
        for id in audio::devices()? {
            writeln!(out, "{id}")?;
        }
        return Ok(());
    }
    if command == "meter" {
        let start = Instant::now();
        let mut peak = 0.0f32;
        let mut rms = 0.0f32;
        let mut frames = 0u64;
        let mut updates = 0u64;
        let mut gaps = 0u64;
        let mut strongest = 0.0f32;
        let mut capture_error = None;
        audio::monitor(&target, &stop, duration, fps, spectrum, |event| {
            match event {
                audio::Update::Ready { device, format } => {
                    capture_error = None;
                    writeln!(
                        out,
                        "Device: {device}\nFormat: {} Hz, {} channels",
                        format.sample_rate(),
                        format.channels()
                    )
                    .map_err(output_error)?;
                }
                audio::Update::Frame(frame) => {
                    peak = peak.max(frame.peak);
                    rms = frame.rms;
                    frames = frame.frames;
                    gaps = frame.discontinuities;
                    updates += 1;
                    strongest = frame.bands.iter().copied().fold(strongest, f32::max);
                }
                audio::Update::Unavailable(error) => capture_error = Some(error),
            }
            Ok(())
        })?;
        if let Some(error) = capture_error {
            return Err(error.into());
        }
        writeln!(
            out,
            "Elapsed: {:.3} s\nFrames: {frames}\nUpdates: {updates}\nPeak: {peak:.6}\nRMS: {rms:.6}\nDiscontinuities: {gaps}",
            start.elapsed().as_secs_f64()
        )?;
        if spectrum {
            writeln!(out, "Strongest spectrum amplitude: {strongest:.6}")?;
        }
        return Ok(());
    }
    let manager = Manager::connect(&stop)?;
    match command.as_str() {
        "artwork" => match manager.current()?.map(|s| s.artwork(&stop)).transpose()?.flatten() {
            Some(image) => writeln!(
                out,
                "System thumbnail: {} x {}, {} decoded bytes",
                image.width,
                image.height,
                image.pixels.len()
            )?,
            None => writeln!(out, "No system thumbnail")?,
        },
        "watch" => manager.watch(&stop, duration, |event| {
            let snapshot = match event {
                taskbar_media_band::media::Update::Changed(snapshot) => snapshot,
                taskbar_media_band::media::Update::Unavailable(error) => {
                    eprintln!("{error}");
                    return Ok(());
                }
            };
            print_snapshot(snapshot, &mut out)
                .and_then(|_| {
                    writeln!(out)?;
                    out.flush()
                })
                .map_err(output_error)
        })?,
        "sessions" => {
            let sessions = manager.sessions()?;
            if sessions.is_empty() {
                print_snapshot(None, &mut out)?;
            }
            for session in sessions {
                print_snapshot(Some(&session.snapshot(&stop)?), &mut out)?;
                writeln!(out)?;
            }
        }
        _ => {
            let current = manager.current()?;
            if let Some(action) = action {
                let session = current.ok_or("no current media session")?;
                if !session.command(action, &stop)? {
                    return Err("player declined the command".into());
                }
                writeln!(out, "Command accepted")?;
            } else {
                let snapshot = current.map(|s| s.snapshot(&stop)).transpose()?;
                print_snapshot(snapshot.as_ref(), &mut out)?;
            }
        }
    }
    Ok(())
}

fn output_error(_: io::Error) -> taskbar_media_band::platform::Error {
    taskbar_media_band::platform::Error::new("write output", 0x8007006du32 as i32)
}

fn main() {
    if let Err(error) = run() {
        if error.downcast_ref::<taskbar_media_band::platform::Error>().is_some_and(|e| e.code == E_ABORT) {
            std::process::exit(130);
        }
        eprintln!("{error}");
        std::process::exit(2);
    }
}
