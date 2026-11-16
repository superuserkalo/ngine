use std::io;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
#[path = "platform/macos.rs"]
mod os;
#[cfg(target_os = "linux")]
#[path = "platform/linux.rs"]
mod os;
#[cfg(target_os = "windows")]
#[path = "platform/windows.rs"]
mod os;

#[cfg(target_os = "windows")]
#[path = "platform/windows_console.rs"]
mod console;

#[cfg(target_os = "linux")]
#[path = "platform/freedesktop_notifications.rs"]
mod desktop_notifications;

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the window's palette is connected to it next")
)]
#[path = "platform/omarchy_theme.rs"]
pub mod omarchy_theme;

#[cfg(target_os = "linux")]
#[expect(dead_code, reason = "the window's palette is connected to it next")]
#[path = "platform/omarchy_theme_watch.rs"]
pub mod omarchy_theme_watch;

use crate::notification::Notification;

/// Shows the notification through the desktop's notification service, where
/// ngine talks to one. Elsewhere ngine's own overlay is all there is.
pub fn notify(notification: Notification) {
    #[cfg(target_os = "linux")]
    desktop_notifications::show(notification);
    #[cfg(not(target_os = "linux"))]
    drop(notification);
}

/// Makes what ngine prints appear in the terminal it was started from. On
/// Windows ngine is a windowed program, which has no console of its own, so
/// it joins its parent's when there is one. Elsewhere there is nothing to
/// do. <https://learn.microsoft.com/windows/console/attachconsole>
pub fn print_to_the_terminal_that_started_ngine() {
    #[cfg(target_os = "windows")]
    let _started_without_a_terminal = !console::attach_to_the_parent();
}

pub use os::{
    FILE_MANAGER, LISTS_CAPTURE_TARGETS, SAMPLE_BUFFER_FOLDER, SAMPLE_OUTPUT_FOLDER,
    SYSTEM_SETTINGS, TRASH, VIDEOS_FOLDER,
};

use crate::engine::Permission;

pub const BACKGROUND_FLAG: &str = "--background";

pub struct Folders<'a> {
    #[cfg_attr(
        any(target_os = "linux", target_os = "windows"),
        expect(
            dead_code,
            reason = "only macOS keeps its login entry under the home folder"
        )
    )]
    pub home: &'a Path,
    #[cfg_attr(
        target_os = "macos",
        expect(
            dead_code,
            reason = "macOS keeps its login entry under the home folder"
        )
    )]
    pub config: &'a Path,
}

fn start(mut command: std::process::Command) -> io::Result<()> {
    let mut started = command.spawn()?;
    std::thread::Builder::new()
        .name("ngine-opened".to_owned())
        .spawn(move || {
            let _opened_program_closed = started.wait();
        })
        .map(drop)
}

/// Shows a file in the system's file manager.
pub fn reveal(file: &Path) -> io::Result<()> {
    start(os::reveal(file))
}

/// Opens a folder in the system's file manager.
pub fn open_folder(folder: &Path) -> io::Result<()> {
    start(os::open_folder(folder))
}

/// Whether this system has a settings page where `permission` is granted.
pub fn has_privacy_settings(permission: Permission) -> bool {
    os::privacy_settings(permission).is_some()
}

/// Opens the system's settings page where `permission` is granted.
pub fn open_privacy_settings(permission: Permission) -> io::Result<()> {
    match os::privacy_settings(permission) {
        Some(command) => start(command),
        None => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this system has no page for it",
        )),
    }
}

/// The file that makes the operating system start ngine when the user logs
/// in, and what it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Autostart {
    pub file: PathBuf,
    pub contents: String,
}

impl Autostart {
    pub fn of(folders: &Folders<'_>, program: &Path) -> Self {
        os::autostart(folders, program)
    }

    pub fn set(&self, enabled: bool) -> io::Result<()> {
        if enabled {
            if let Some(folder) = self.file.parent() {
                std::fs::create_dir_all(folder)?;
            }
            return std::fs::write(&self.file, &self.contents);
        }
        match std::fs::remove_file(&self.file) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(test: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ngine-autostart-{test}-{}", std::process::id()))
    }

    #[test]
    fn the_entry_starts_this_program_in_the_background() {
        let root = temp_folder("contents");
        let folders = Folders {
            home: &root,
            config: &root.join("config").join("ngine"),
        };
        let entry = Autostart::of(&folders, Path::new("/opt/ngine & co/ngine"));
        assert!(entry.file.starts_with(&root));
        assert!(entry.contents.contains("ngine"));
        assert!(entry.contents.contains(BACKGROUND_FLAG));
    }

    #[test]
    fn switching_it_on_writes_the_file_and_off_removes_it() {
        let root = temp_folder("switch");
        let folders = Folders {
            home: &root,
            config: &root.join("config").join("ngine"),
        };
        let entry = Autostart::of(&folders, Path::new("/opt/ngine/ngine"));
        entry.set(true).expect("the entry is written");
        assert_eq!(
            std::fs::read_to_string(&entry.file).expect("the entry exists"),
            entry.contents
        );
        entry.set(false).expect("the entry is removed");
        assert!(!entry.file.exists());
        entry.set(false).expect("removing it twice is fine");
        std::fs::remove_dir_all(&root).expect("temp folder removed");
    }
}
