use std::path::Path;

use super::{Autostart, BACKGROUND_FLAG, Folders};

pub const VIDEOS_FOLDER: &str = "Videos";
pub const LISTS_CAPTURE_TARGETS: bool = true;

const STARTUP_FILE: &str = "Microsoft\\Windows\\Start Menu\\Programs\\Startup\\ngine.cmd";

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
