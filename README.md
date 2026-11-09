# Taskbar Media

[![Build](https://github.com/dyrb/taskbar-media/actions/workflows/ci.yml/badge.svg)](https://github.com/dyrb/taskbar-media/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/dyrb/taskbar-media)](https://github.com/dyrb/taskbar-media/releases/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)
![Taskbar](https://img.shields.io/badge/taskbar-Windows%2010-0078D6)
![Rust](https://img.shields.io/badge/made%20with-Rust-B7410E)

Media controls and an audio visualiser inside the Windows 10 taskbar. Shows the current
track and artist, optional album artwork, a waveform or a frequency spectrum. Music plays
in your usual player. Written in Rust with a native Windows interface.

[Русская версия](README.ru.md)

## Features

- Track information and previous, play/pause and next buttons from Windows media sessions.
- Capture the current player's process tree, or explicitly select all output audio.
- Waveform and 32-band spectrum, solid or separated drawing, 15 or 30 FPS.
- Amplitude and release-time settings, native colour picker with 16 saved custom colours.
- Adjustable width, compact or full taskbar height, optional artwork and transparent background.
- Hide playback and settings buttons independently; right-click settings remain available.
- System, light and dark themes, high contrast, DPI scaling, English and Russian.
- Event-driven media updates and audio capture. Capture and FFT stop when playback is paused,
  the widget is hidden, visualisation is disabled, or there is no room to display it.
- Settings beside the program. No image cache, log files or startup tasks.
- A standalone window and CLI for media commands and audio analysis.

## Requirements

Windows x64. Taskbar integration uses the Windows 10 DeskBand mechanism and has been tested
on Windows 10 22H2, build 19045. The native Windows 11 taskbar does not support this band;
use the standalone window there. Media sessions require Windows 10 1809 or later.

The player must expose a Windows media session. **Audio source > Player only** identifies
its process tree from the session's executable name or application identity. This requires
Windows process-loopback support; it has been exercised on build 19045. If capture is
unavailable or the identity is ambiguous, the widget reports an error. Choose **All device
audio** to capture the output mix, including other applications and notifications.

Sounds inside the same process tree are included. Browser tabs sharing that tree are not
isolated, and protected audio may be unavailable. Audio-device removal, sleep/resume and
multi-monitor taskbar transitions still need broader hardware testing.

## Usage

Download the ZIP from [Releases](https://github.com/dyrb/taskbar-media/releases/latest),
extract all three binaries into one folder and run `taskbar-media-ui.exe` on Windows 10.
It registers the component and asks Windows to show **Taskbar Media**. Complete any Windows
toolbar prompt. The band is also available in the taskbar's **Toolbars** menu.

Right-click the widget or press its three-dot button to open settings. Unlock the taskbar
to drag the standard sizing grip, or select a width preset. **Height > Full taskbar height**
follows the taskbar's height; the compact mode aligns content to the top. Artwork fills its
frame with a centre crop. **Colour...** opens the Windows colour dialog with RGB controls
and a custom palette. Artwork is off by default.

Tab moves between buttons, Space activates a button, and Shift+F10 opens settings.
Unsupported playback commands are disabled. Controls that do not fit are hidden; audio
analysis stops when the visualisation area is empty and resumes when space returns.
Media-session notifications remain active so that the track information stays current.

For a separate window, run:

```powershell
.\taskbar-media-ui.exe --window
```

### Files and registration

| File | Purpose |
| --- | --- |
| `taskbar-media-ui.exe` | Enable the taskbar widget, or open a standalone window with `--window` |
| `taskbar-media.exe` | CLI and registration commands |
| `taskbar_media_band.dll` | Windows 10 taskbar component |
| `TaskbarMedia.ini` | Created beside the program when settings change |

Keep the binaries in their registered folder. Settings use an atomic `TaskbarMedia.ini.tmp`
replacement. An unwritable folder produces an error; there is no AppData fallback.
Unchanged startup and shutdown do not write a settings file. Artwork stays in memory.

Registration normally uses the current user's COM registry. With UAC disabled and an
elevated process, automatic registration uses the machine registry. `--user` or `--machine`
select the scope explicitly; machine registration requires administrator rights. Windows
can remember an enabled toolbar across sign-ins. The program adds no startup entry.

Disable the band before an update. Explorer can retain the DLL after hiding it; if it is
still locked, replace it after signing out or restarting Explorer yourself. The program
does not restart Explorer automatically. To remove registration, use the same scope as
when enabling it:

```powershell
.\taskbar-media.exe disable
.\taskbar-media.exe unregister
```

### Command line

```powershell
# Show the current track and available Windows media sessions.
.\taskbar-media.exe status
.\taskbar-media.exe sessions

# Follow media-session changes for 30 seconds.
.\taskbar-media.exe watch --seconds 30

# Analyse the default output mix for 10 seconds.
.\taskbar-media.exe meter --seconds 10 --spectrum

# List audio output identifiers or query the toolbar state.
.\taskbar-media.exe devices
.\taskbar-media.exe band-status
```

`meter --process PID` captures a process tree; `meter --device ID` selects an output listed
by `devices`. `play`, `pause`, `toggle`, `next`, `previous` and `stop` control the current
media session. Ctrl+C stops monitoring. `artwork` prints the system thumbnail's dimensions
without saving an image. Run `taskbar-media.exe help` for all options. CLI messages are English.

## Build from source

Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/) with
**Desktop development with C++** and a Windows SDK, then install Rust using
[rustup](https://rustup.rs/) with the MSVC toolchain. Rust 1.88 or later is required.
Download the source ZIP or clone this repository, open PowerShell in the project folder:

```powershell
cargo build --release --locked
```

The three binaries are in `target\release`. There are no third-party Cargo dependencies;
the native resources are generated by `build.rs`. The project uses two build jobs and a
static C runtime. For a ZIP and SHA-256 checksum with build paths remapped, run:

```powershell
.\scripts\build-release.ps1
```

The script writes to `dist` and does not register or publish the component.

### Development checks

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Tests cover Windows ABI layouts, COM ownership, cancellation, audio formats, FFT, spectrum
bands, WIC decoding, settings and DPI geometry. Two integration tests are opt-in; ordinary
tests neither register a band in Explorer nor change playback. CI runs checks on Windows;
release tags build the ZIP and attach a GitHub build attestation.

## Implementation

Windows media-session notifications supply metadata and controls. WASAPI supplies audio
through events. The analyser uses fixed-size buffers and a 1024-point FFT; the UI consumes
the latest frame rather than queueing old frames. Disjoint Mel-spaced spectrum bands use
a fixed -60..0 dB display range. Rendering uses native GDI and buffered painting.

Artwork is decoded by Windows Imaging Component, with limits of 8 MiB encoded input,
16 million source pixels and 512x512 output. Hiding the visualisation cancels capture;
an operation already in progress finishes cooperatively. The media engine remains available
for track updates and controls. The DeskBand runs inside Explorer, so its resource use is
part of that process.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).
