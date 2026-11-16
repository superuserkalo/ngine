use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Autostart, BACKGROUND_FLAG, Folders};
use crate::engine::Permission;

pub const VIDEOS_FOLDER: &str = "Videos";
pub const SAMPLE_OUTPUT_FOLDER: &str = "~/Videos/ngine";
pub const SAMPLE_BUFFER_FOLDER: &str = "~/.cache/ngine/replay";
pub const FILE_MANAGER: &str = "file manager";
pub const TRASH: &str = "Trash";
pub const SYSTEM_SETTINGS: &str = "the system settings";
pub const LISTS_CAPTURE_TARGETS: bool = false;
pub const ERASE_KEY: &str = "Backspace";

/// Keys Linux desktops commonly keep for themselves, with what they do
/// there.
pub const RESERVED_SHORTCUTS: &[(&str, &str)] = &[
    ("alt+f4", "closing a window"),
    ("alt+tab", "switching windows"),
    ("ctrl+alt+delete", "logging out"),
    ("ctrl+alt+t", "opening a terminal"),
    ("super+l", "locking the screen"),
    ("super+tab", "switching apps"),
    ("ctrl+alt+left", "the workspace to the left"),
    ("ctrl+alt+right", "the workspace to the right"),
];

const ENTRY_FILE: &str = "autostart/ngine.desktop";

fn quoted(program: &Path) -> String {
    let escaped = program
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`");
    format!("\"{escaped}\"")
}

fn program_to_start(running: &Path, appimage: Option<OsString>) -> PathBuf {
    appimage
        .filter(|path| !path.is_empty())
        .map_or_else(|| running.to_path_buf(), PathBuf::from)
}

/// The window draws its own titlebar: on Windows the caption buttons sit
/// over ngine's own top strip; elsewhere the system's frame stays.
pub fn window_frame<S>(app: rune::App<S>) -> rune::App<S> {
    app.titlebar(crate::ui::titlebar())
}

pub fn reveal(file: &Path) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(file.parent().unwrap_or(file));
    command
}

pub fn open_folder(folder: &Path) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(folder);
    command
}

pub fn privacy_settings(_: Permission) -> Option<Command> {
    None
}

pub fn autostart(folders: &Folders<'_>, running: &Path) -> Autostart {
    let program = program_to_start(running, std::env::var_os("APPIMAGE"));
    let config_home = folders.config.parent().unwrap_or(folders.config);
    Autostart {
        file: config_home.join(ENTRY_FILE),
        contents: format!(
            "[Desktop Entry]\nType=Application\nName=ngine\nExec={} {BACKGROUND_FLAG}\n",
            quoted(&program)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_appimage_starts_from_its_own_file_not_its_temporary_mount() {
        let running = Path::new("/tmp/.mount_ngineAbc/usr/bin/ngine");
        let appimage = Some(OsString::from("/home/me/Apps/ngine.AppImage"));
        assert_eq!(
            program_to_start(running, appimage),
            PathBuf::from("/home/me/Apps/ngine.AppImage")
        );
    }

    #[test]
    fn an_installed_program_starts_from_where_it_runs() {
        let running = Path::new("/usr/lib/ngine/ngine");
        assert_eq!(program_to_start(running, None), running);
        assert_eq!(program_to_start(running, Some(OsString::new())), running);
    }
}
