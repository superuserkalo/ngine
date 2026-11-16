//! # Linking on macOS
//!
//! The macOS backend links Swift code, and Cargo does not pass a linker
//! argument from a library on to the programs that use it. Every binary, test
//! and example that links this crate on macOS must therefore add
//! `-Wl,-rpath,/usr/lib/swift` to its own link, from a build script or from
//! `.cargo/config.toml`. Without it the program stops at launch with
//! `Library not loaded: @rpath/libswift_Concurrency.dylib`.
//!
//! Source: <https://doc.rust-lang.org/cargo/reference/build-scripts.html#rustc-link-arg>

mod audio;
mod backend;
mod capturer;
mod error;
mod event;
pub mod fake;
mod frame;
#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod options;
mod session;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod supervisor;
mod target;
mod time;
#[cfg(any(target_os = "windows", test))]
mod windows;

pub use audio::{AudioBuffer, AudioSource};
pub use capturer::{AudioAppLister, Capturer, TargetLister, TargetPicker};
pub use error::{CaptureError, Permission};
pub use event::{EndReason, Event};
pub use frame::{CpuFrame, PixelFormat, Size, Surface, VideoFrame};
#[cfg(target_os = "linux")]
pub use linux::{DmaBufPlane, DmaBufSurface};
#[cfg(target_os = "macos")]
pub use macos::CoreVideoSurface;
pub use options::{Microphone, SessionOptions, SystemAudio};
pub use session::{RecvError, Session, Stopper};
pub use target::{
    App, AudioApp, MicrophoneDevice, ParentWindow, PickedTarget, PickerOptions, RestoreToken,
    Screen, ScreenId, Target, TargetKind, Targets, Window, WindowId,
};
pub use time::Timestamp;
#[cfg(target_os = "windows")]
pub use windows::D3d11Surface;
