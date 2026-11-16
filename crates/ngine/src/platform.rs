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

pub use os::{LISTS_CAPTURE_TARGETS, SAMPLE_BUFFER_FOLDER, SAMPLE_OUTPUT_FOLDER, VIDEOS_FOLDER};

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
