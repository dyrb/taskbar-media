# Changelog

## 0.1.2

- Native Segoe MDL2 playback icons rasterised with DirectWrite and cached for button states.
- Tighter transport-button spacing, including consistent order on a vertical taskbar.
- Row-aware band height to prevent growth when toolbars wrap onto multiple rows.
- Restore the configured width and clear a carried row break when moving the band; notify the host and verify its resulting order.

## 0.1.1

- Larger cached DirectWrite text with single-line and two-line layouts.
- Destination-sized WIC artwork resampling and square artwork geometry.
- Persistent element order, Alt+drag and native rebar position commands.
- Optional two-colour waveform/spectrum gradient with native colour selection.
- Event-driven hover/click track card with on-demand artwork and no extra audio capture.
- Compact 208-DIP card with measured text height and consistent artwork/text spacing.
- Cancel-safe handoff between process-audio activations; bounded recovery from transient audio failures.
- Manual Explorer restart script for applying a rebuilt DeskBand.

## 0.1.0

- Windows 10 taskbar widget and standalone media window.
- Windows media-session track information, playback controls and optional artwork.
- Player-process or output-device audio capture, waveform and 32-band spectrum.
- Solid fill, amplitude, release time, 15/30 FPS and a native colour picker with saved custom colours.
- Adjustable width, compact/full height, transparency and independently hidden controls.
- System/light/dark themes, high contrast, DPI scaling and English/Russian interface.
- Event-driven updates; capture stops on pause, hiding, disabled visualisation or insufficient space.
- Portable settings, CLI, Windows CI and release ZIP with SHA-256.

Taskbar integration targets Windows 10; Windows 11 supports the standalone window only.
