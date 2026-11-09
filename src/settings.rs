//! Portable settings: one bounded UTF-8 file beside the module, written only on changes.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    System,
    English,
    Russian,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Visual {
    Off,
    Wave,
    #[default]
    Spectrum,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub theme: Theme,
    pub language: Language,
    pub visual: Visual,
    pub fps: u32,
    pub gain: u32,
    pub release_ms: u32,
    pub width: u32,
    pub accent: u32,
    pub custom_colors: [u32; 16],
    pub artwork: bool,
    pub full_height: bool,
    pub transparent: bool,
    pub controls: bool,
    pub menu_button: bool,
    pub solid: bool,
    pub player_audio: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            language: Language::System,
            visual: Visual::Spectrum,
            fps: 15,
            gain: 400,
            release_ms: 180,
            width: 360,
            accent: 0x00d7b85b,
            custom_colors: [0xffffff; 16],
            artwork: false,
            full_height: false,
            transparent: true,
            controls: true,
            menu_button: true,
            solid: true,
            player_audio: true,
        }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Self {
        let text = text.trim_start_matches('\u{feff}');
        let mut s = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.trim().split_once('=') else { continue };
            let v = value.trim();
            match key.trim() {
                "theme" => {
                    s.theme = match v {
                        "dark" => Theme::Dark,
                        "light" => Theme::Light,
                        _ => Theme::System,
                    }
                }
                "language" => {
                    s.language = match v {
                        "en" => Language::English,
                        "ru" => Language::Russian,
                        _ => Language::System,
                    }
                }
                "visual" => {
                    s.visual = match v {
                        "off" => Visual::Off,
                        "wave" => Visual::Wave,
                        _ => Visual::Spectrum,
                    }
                }
                "fps" => {
                    if let Ok(n @ (15 | 30)) = v.parse() {
                        s.fps = n
                    }
                }
                "gain" => {
                    if let Ok(n @ 25..=1600) = v.parse() {
                        s.gain = n
                    }
                }
                "release_ms" => {
                    if let Ok(n @ 50..=1000) = v.parse() {
                        s.release_ms = n
                    }
                }
                "width" => {
                    if let Ok(n @ 160..=1200) = v.parse() {
                        s.width = n
                    }
                }
                "accent" => {
                    if let Ok(n @ 0..=0xffffff) = u32::from_str_radix(v, 16) {
                        s.accent = n
                    }
                }
                "custom_colors" => {
                    let mut colors = [0xffffff; 16];
                    let mut values = v.split(',');
                    let valid = colors.iter_mut().all(|color| {
                        match values.next().and_then(|s| u32::from_str_radix(s.trim(), 16).ok()) {
                            Some(value @ 0..=0xffffff) => {
                                *color = value;
                                true
                            }
                            _ => false,
                        }
                    });
                    if valid && values.next().is_none() {
                        s.custom_colors = colors;
                    }
                }
                "artwork" => s.artwork = v == "1",
                "full_height" if matches!(v, "0" | "1") => s.full_height = v == "1",
                "transparent" if matches!(v, "0" | "1") => s.transparent = v == "1",
                "controls" if matches!(v, "0" | "1") => s.controls = v == "1",
                "menu_button" if matches!(v, "0" | "1") => s.menu_button = v == "1",
                "solid" if matches!(v, "0" | "1") => s.solid = v == "1",
                "player_audio" if matches!(v, "0" | "1") => s.player_audio = v == "1",
                _ => {}
            }
        }
        s
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        match std::fs::metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Ok(m) if m.len() > 16384 => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "settings exceed 16 KiB"));
            }
            Err(e) => return Err(e),
            _ => {}
        }
        // Take limits the read even if another process grows the file after metadata().
        use io::Read;
        let mut text = String::new();
        std::fs::File::open(path)?.take(16385).read_to_string(&mut text)?;
        if text.len() > 16384 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "settings exceed 16 KiB"));
        }
        Ok(Self::parse(&text))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        use std::fmt::Write as _;
        let mut colors = String::with_capacity(111);
        for (i, color) in self.custom_colors.iter().enumerate() {
            if i != 0 {
                colors.push(',');
            }
            write!(&mut colors, "{color:06x}").expect("write color into string");
        }
        let text = format!(
            "theme={}\nlanguage={}\nvisual={}\nfps={}\ngain={}\nrelease_ms={}\nwidth={}\naccent={:06x}\nartwork={}\nfull_height={}\ntransparent={}\ncontrols={}\nmenu_button={}\nsolid={}\nplayer_audio={}\ncustom_colors={}\n",
            match self.theme {
                Theme::System => "system",
                Theme::Light => "light",
                Theme::Dark => "dark",
            },
            match self.language {
                Language::System => "system",
                Language::English => "en",
                Language::Russian => "ru",
            },
            match self.visual {
                Visual::Off => "off",
                Visual::Wave => "wave",
                Visual::Spectrum => "spectrum",
            },
            self.fps,
            self.gain,
            self.release_ms,
            self.width,
            self.accent,
            u8::from(self.artwork),
            u8::from(self.full_height),
            u8::from(self.transparent),
            u8::from(self.controls),
            u8::from(self.menu_button),
            u8::from(self.solid),
            u8::from(self.player_audio),
            colors
        );
        if std::fs::read(path).ok().as_deref() == Some(text.as_bytes()) {
            return Ok(());
        }
        let temporary = path.with_extension("ini.tmp");
        use std::io::Write;
        use std::os::windows::fs::OpenOptionsExt;
        // Deny concurrent writers, but allow our atomic rename while the handle remains owned.
        let mut file =
            std::fs::OpenOptions::new().write(true).create(true).truncate(true).share_mode(4).open(&temporary)?;
        if let Err(error) = file.write_all(text.as_bytes()) {
            drop(file);
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: terminated paths; replace atomically in the same directory. No fallback location.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8) } == 0 {
            let e = io::Error::last_os_error();
            drop(file);
            let _ = std::fs::remove_file(temporary);
            return Err(e);
        }
        drop(file);
        Ok(())
    }
}

pub fn path() -> io::Result<PathBuf> {
    use crate::platform::ffi::Raw;
    use std::os::windows::ffi::OsStringExt;
    let mut module: Raw = std::ptr::null_mut();
    let mut buffer = [0u16; 32768];
    // SAFETY: FROM_ADDRESS | UNCHANGED_REFCOUNT identifies this code's EXE or DLL, not Explorer.
    unsafe {
        if GetModuleHandleExW(4 | 2, path as *const () as *const u16, &mut module) == 0 {
            return Err(io::Error::last_os_error());
        }
        let n = GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) as usize;
        if n == 0 || n >= buffer.len() {
            return Err(io::Error::last_os_error());
        }
        Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..n])).with_file_name("TaskbarMedia.ini"))
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut crate::platform::ffi::Raw) -> i32;
    fn GetModuleFileNameW(module: crate::platform::ffi::Raw, path: *mut u16, size: u32) -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_replace_atomically_and_leave_unchanged_files_alone() {
        use std::os::windows::fs::OpenOptionsExt;
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("settings-check-{}.ini", std::process::id()));
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
                let _ = std::fs::remove_file(self.0.with_extension("ini.tmp"));
            }
        }
        let _cleanup = Cleanup(path.clone());
        let mut s = Settings::default();
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);
        // A reader denies deletion: an unchanged save must not attempt atomic replacement.
        let reader = std::fs::OpenOptions::new().read(true).share_mode(1 | 2).open(&path).unwrap();
        s.save(&path).unwrap();
        s.artwork = true;
        s.full_height = true;
        s.transparent = false;
        s.controls = false;
        s.menu_button = false;
        s.solid = false;
        s.player_audio = false;
        s.accent = 0;
        s.custom_colors = std::array::from_fn(|i| i as u32 * 0x010203);
        assert!(s.save(&path).is_err());
        assert!(!Settings::load(&path).unwrap().artwork);
        assert!(!path.with_extension("ini.tmp").exists());
        drop(reader);
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);
    }
    #[test]
    fn invalid_settings_do_not_escape_resource_limits() {
        let s = Settings::parse(
            "fps=999\nwidth=4294967295\ngain=0\nrelease_ms=-1\naccent=ffffffff\nartwork=1\nlanguage=ru",
        );
        assert_eq!(s.fps, 15);
        assert_eq!(s.width, 360);
        assert_eq!(s.gain, 400);
        assert_eq!(s.release_ms, 180);
        assert!(s.artwork);
        assert_eq!(s.language, Language::Russian);
        let old = Settings::parse("width=280\nartwork=1");
        assert!(old.controls && old.menu_button && old.transparent);
        assert!(!old.full_height);
        assert!(old.solid && old.player_audio);
        let invalid =
            Settings::parse("controls=no\nmenu_button=-1\ntransparent=x\nfull_height=2\nsolid=x\nplayer_audio=no");
        assert_eq!(invalid, Settings::default());
        for colors in [
            "",
            "000000",
            "ffffff,ffffff",
            "1000000,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0",
            "0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0",
        ] {
            assert_eq!(Settings::parse(&format!("custom_colors={colors}")).custom_colors, [0xffffff; 16]);
        }
    }
}
