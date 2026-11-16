use std::io;
use std::mem::MaybeUninit;
use std::path::PathBuf;
use std::thread;

use rustix::fs::inotify::{self, CreateFlags, Reader, WatchFlags};
use rustix::io::Errno;

use super::omarchy_theme::{self, ThemeColors};

const EVENT_BUFFER: usize = 4096;
const THEME_ENTRIES: [&[u8]; 3] = [b"theme", b"theme.name", b"theme.generation"];

/// Calls `changed` with the new colours each time Omarchy switches its
/// theme. Omarchy builds the new theme beside the current one and moves it
/// into place, so the watch is on the folder that holds `theme`. A thread
/// sleeps in the kernel until that folder changes, so it costs nothing in
/// between. Fails with [`io::ErrorKind::NotFound`] when this is not Omarchy.
pub fn watch(
    home: PathBuf,
    state_home: Option<PathBuf>,
    changed: impl Fn(ThemeColors) + Send + 'static,
) -> io::Result<()> {
    let events = inotify::init(CreateFlags::CLOEXEC)?;
    let watched = omarchy_theme::current_folders(&home, state_home.as_deref())
        .iter()
        .filter(|folder| {
            inotify::add_watch(
                &events,
                folder.as_path(),
                WatchFlags::CREATE | WatchFlags::MOVED_TO | WatchFlags::CLOSE_WRITE,
            )
            .is_ok()
        })
        .count();
    if watched == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "there is no Omarchy theme to follow",
        ));
    }
    thread::Builder::new()
        .name("ngine-omarchy-theme".into())
        .spawn(move || {
            let mut shown = omarchy_theme::read(&home, state_home.as_deref());
            let mut buffer = [MaybeUninit::<u8>::uninit(); EVENT_BUFFER];
            let mut reader = Reader::new(events, &mut buffer);
            loop {
                let about_the_theme = match reader.next() {
                    Ok(event) => event
                        .file_name()
                        .is_some_and(|name| THEME_ENTRIES.contains(&name.to_bytes())),
                    Err(Errno::INTR) => false,
                    Err(error) => {
                        eprintln!("ngine: the Omarchy theme is no longer followed: {error}");
                        return;
                    }
                };
                if !about_the_theme {
                    continue;
                }
                let now = omarchy_theme::read(&home, state_home.as_deref());
                if now != shown {
                    if let Some(colors) = now {
                        changed(colors);
                    }
                    shown = now;
                }
            }
        })
        .map(drop)
}
