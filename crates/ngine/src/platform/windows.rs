use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use super::{Autostart, BACKGROUND_FLAG, Folders};
use crate::engine::Permission;

pub const VIDEOS_FOLDER: &str = "Videos";
pub const SAMPLE_OUTPUT_FOLDER: &str = "~\\Videos\\ngine";
pub const SAMPLE_BUFFER_FOLDER: &str = "~\\AppData\\Local\\ngine\\Cache\\replay";
pub const FILE_MANAGER: &str = "Explorer";
pub const TRASH: &str = "Recycle Bin";
pub const SYSTEM_SETTINGS: &str = "Settings";
pub const LISTS_CAPTURE_TARGETS: bool = true;

const STARTUP_FILE: &str = "Microsoft\\Windows\\Start Menu\\Programs\\Startup\\ngine.cmd";

pub fn reveal(file: &Path) -> Command {
    let mut selected = OsString::from("/select,");
    selected.push(file);
    let mut command = Command::new("explorer");
    command.arg(selected);
    command
}

pub fn open_folder(folder: &Path) -> Command {
    let mut command = Command::new("explorer");
    command.arg(folder);
    command
}

pub fn privacy_settings(permission: Permission) -> Option<Command> {
    match permission {
        Permission::ScreenRecording => None,
        Permission::Microphone => {
            let mut command = Command::new("explorer");
            command.arg("ms-settings:privacy-microphone");
            Some(command)
        }
    }
}

pub fn autostart(folders: &Folders<'_>, program: &Path) -> Autostart {
    let roaming = folders.config.parent().unwrap_or(folders.config);
    Autostart {
        file: roaming.join(STARTUP_FILE),
        contents: format!(
            "@echo off\r\nstart \"\" \"{}\" {BACKGROUND_FLAG}\r\n",
            program.display()
        ),
    }
}
