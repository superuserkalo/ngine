use std::fs::{self, DirEntry, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use super::error::ReplayError;

pub(crate) const ROOT_DIRECTORY: &str = "ngine-replay";
pub(crate) const ROOT_LOCK: &str = "root.lock";
pub(crate) const OWNER_LOCK: &str = "owner.lock";
const SESSION_PREFIX: &str = "session-";
const GROUP_PREFIX: &str = "group-";
const GROUP_SUFFIX: &str = ".bin";
const SESSION_NAME_ATTEMPTS: u32 = 1000;

#[derive(Debug)]
pub(crate) struct Session {
    directory: PathBuf,
    owner_lock: Option<File>,
}

impl Session {
    pub(crate) fn create(folder: &Path) -> Result<Self, ReplayError> {
        let root = folder.join(ROOT_DIRECTORY);
        match fs::create_dir(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(ReplayError::storage(
                    "create the replay folder",
                    root,
                    error,
                ));
            }
        }
        let _root_lock = lock_root(&root)?;
        remove_dead_sessions(&root)?;
        Self::create_under_locked_root(&root)
    }

    fn create_under_locked_root(root: &Path) -> Result<Self, ReplayError> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since_epoch| since_epoch.as_nanos());
        for attempt in 0..SESSION_NAME_ATTEMPTS {
            let directory = root.join(format!(
                "{SESSION_PREFIX}{}-{stamp}-{attempt}",
                process::id()
            ));
            match fs::create_dir(&directory) {
                Ok(()) => return Self::own(directory),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(ReplayError::storage(
                        "create a replay session folder",
                        directory,
                        error,
                    ));
                }
            }
        }
        Err(ReplayError::storage(
            "find an unused replay session name",
            root,
            io::Error::from(io::ErrorKind::AlreadyExists),
        ))
    }

    fn own(directory: PathBuf) -> Result<Self, ReplayError> {
        let lock_path = directory.join(OWNER_LOCK);
        let locked = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .and_then(|file| match file.try_lock() {
                Ok(()) => Ok(file),
                Err(TryLockError::WouldBlock) => Err(io::Error::from(io::ErrorKind::WouldBlock)),
                Err(TryLockError::Error(error)) => Err(error),
            });
        match locked {
            Ok(owner_lock) => Ok(Self {
                directory,
                owner_lock: Some(owner_lock),
            }),
            Err(error) => {
                drop(fs::remove_file(&lock_path));
                drop(fs::remove_dir(&directory));
                Err(ReplayError::storage(
                    "lock a replay session folder",
                    lock_path,
                    error,
                ))
            }
        }
    }

    pub(crate) fn group_path(&self, group_number: u64) -> PathBuf {
        self.directory
            .join(format!("{GROUP_PREFIX}{group_number}{GROUP_SUFFIX}"))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        drop(self.owner_lock.take());
        drop(fs::remove_file(self.directory.join(OWNER_LOCK)));
        drop(fs::remove_dir(&self.directory));
    }
}

/// What [`remove_leftovers`] deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RemovedLeftovers {
    pub sessions: usize,
    pub files: usize,
    pub bytes: u64,
}

/// Deletes the buffer files that crashed runs left in `folder`.
///
/// Call it at start-up with the folder replay buffers are given, also when
/// replay is off or in memory. It only looks inside the `ngine-replay`
/// folder under `folder`, skips every session whose owner still runs, in this
/// process or another, and deletes only files named like buffer files, so a
/// file somebody else put there stays, together with its folder.
pub fn remove_leftovers(folder: &Path) -> Result<RemovedLeftovers, ReplayError> {
    let root = folder.join(ROOT_DIRECTORY);
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(RemovedLeftovers::default()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RemovedLeftovers::default());
        }
        Err(error) => {
            return Err(ReplayError::storage(
                "look at the replay folder",
                root,
                error,
            ));
        }
    }
    let _root_lock = lock_root(&root)?;
    remove_dead_sessions(&root)
}

fn lock_root(root: &Path) -> Result<File, ReplayError> {
    let lock_path = root.join(ROOT_LOCK);
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .and_then(|file| file.lock().map(|()| file))
        .map_err(|error| ReplayError::storage("lock the replay folder", lock_path, error))
}

fn remove_dead_sessions(root: &Path) -> Result<RemovedLeftovers, ReplayError> {
    let list_error = |error| ReplayError::storage("list the replay folder", root, error);
    let mut removed = RemovedLeftovers::default();
    for entry in fs::read_dir(root).map_err(list_error)? {
        let entry = entry.map_err(list_error)?;
        if !is_session_directory(&entry) {
            continue;
        }
        let directory = entry.path();
        if let Owner::Dead(released_lock) = owner_of(&directory) {
            remove_group_files(&directory, &mut removed);
            drop(released_lock);
            drop(fs::remove_file(directory.join(OWNER_LOCK)));
            if fs::remove_dir(&directory).is_ok() {
                removed.sessions += 1;
            }
        }
    }
    Ok(removed)
}

enum Owner {
    Alive,
    Dead(Option<File>),
}

fn owner_of(session_directory: &Path) -> Owner {
    match OpenOptions::new()
        .write(true)
        .open(session_directory.join(OWNER_LOCK))
    {
        Ok(file) => match file.try_lock() {
            Ok(()) => Owner::Dead(Some(file)),
            Err(_) => Owner::Alive,
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Owner::Dead(None),
        Err(_) => Owner::Alive,
    }
}

fn remove_group_files(session_directory: &Path, removed: &mut RemovedLeftovers) {
    let Ok(entries) = fs::read_dir(session_directory) else {
        return;
    };
    for entry in entries.flatten() {
        let is_group_file = entry.file_type().is_ok_and(|kind| kind.is_file())
            && entry.file_name().to_str().is_some_and(is_group_file_name);
        if !is_group_file {
            continue;
        }
        let bytes = entry.metadata().map_or(0, |metadata| metadata.len());
        if fs::remove_file(entry.path()).is_ok() {
            removed.files += 1;
            removed.bytes += bytes;
        }
    }
}

fn is_session_directory(entry: &DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_dir())
        && entry.file_name().to_str().is_some_and(is_session_name)
}

fn is_session_name(name: &str) -> bool {
    name.strip_prefix(SESSION_PREFIX).is_some_and(|numbers| {
        let mut parts = numbers.split('-');
        let all_numbers = parts
            .by_ref()
            .take(3)
            .filter(|part| is_number(part))
            .count()
            == 3;
        all_numbers && parts.next().is_none()
    })
}

fn is_group_file_name(name: &str) -> bool {
    name.strip_prefix(GROUP_PREFIX)
        .and_then(|rest| rest.strip_suffix(GROUP_SUFFIX))
        .is_some_and(is_number)
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}
