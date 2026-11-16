use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{Autostart, BACKGROUND_FLAG, Folders};

pub const VIDEOS_FOLDER: &str = "Videos";
pub const SAMPLE_OUTPUT_FOLDER: &str = "~/Videos/ngine";
pub const SAMPLE_BUFFER_FOLDER: &str = "~/.cache/ngine/replay";
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

fn program_to_start(running: &Path, appimage: Option<OsString>) -> PathBuf {
    appimage
        .filter(|path| !path.is_empty())
        .map_or_else(|| running.to_path_buf(), PathBuf::from)
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
