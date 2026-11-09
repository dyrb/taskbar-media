#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(not(all(windows, target_arch = "x86_64")))]
compile_error!("taskbar-media currently supports Windows x64");

mod artwork;
pub use artwork::Image;
pub mod audio;
pub mod host;
pub mod media;
pub mod platform;
pub mod runtime;
pub mod settings;
pub mod ui;
pub mod visual;
