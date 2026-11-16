use std::path::Path;

use super::{Autostart, BACKGROUND_FLAG, Folders};

pub const VIDEOS_FOLDER: &str = "Videos";
pub const LISTS_CAPTURE_TARGETS: bool = false;

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

pub fn autostart(folders: &Folders<'_>, program: &Path) -> Autostart {
    let config_home = folders.config.parent().unwrap_or(folders.config);
    Autostart {
        file: config_home.join(ENTRY_FILE),
        contents: format!(
            "[Desktop Entry]\nType=Application\nName=ngine\nExec={} {BACKGROUND_FLAG}\n",
            quoted(program)
        ),
    }
}
