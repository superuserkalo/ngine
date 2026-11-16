#[cfg(any(target_os = "macos", target_os = "windows"))]
#[path = "speaker/device.rs"]
mod os;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[path = "speaker/silent.rs"]
mod os;

/// The default sound output, playing one player's sound for as long as it
/// lives. Opening it fails quietly when there is no device: the player then
/// follows the clock without sound.
pub use os::Speaker;
