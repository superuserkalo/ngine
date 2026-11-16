use std::fs::{self, DirEntry, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use super::error::ReplayError;

pub(crate) const ROOT_DIRECTORY: &str = "ngine-replay";
pub(crate) const OWNER_LOCK: &str = "owner.lock";
const SESSION_PREFIX: &str = "session";
const UNIT_PREFIX: &str = "group-";
const UNIT_SUFFIX: &str = ".bin";
const SESSION_NAME_ATTEMPTS: u32 = 1000;
const HOST_NAME_BYTES_IN_A_SESSION_NAME: usize = 64;

#[derive(Debug)]
pub(crate) struct Session {
    directory: PathBuf,
    owner_lock: Option<File>,
}

impl Session {
    pub(crate) fn create(folder: &Path) -> Result<Self, ReplayError> {
        let root = folder.join(ROOT_DIRECTORY);
        fs::create_dir_all(&root)
            .map_err(|error| ReplayError::storage("create the replay folder", &root, error))?;
        drop(remove_dead_sessions(&root, &this_host()));
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since_epoch| since_epoch.as_nanos());
        for attempt in 0..SESSION_NAME_ATTEMPTS {
            let name = SessionName {
                host: this_host(),
                process: process::id(),
                stamp,
                attempt,
            };
            let directory = root.join(name.to_string());
            match fs::create_dir(&directory) {
                Ok(()) => return Ok(Self::own(directory)),
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

    fn own(directory: PathBuf) -> Self {
        let owner_lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(OWNER_LOCK))
            .ok()
            .filter(|file| file.try_lock().is_ok());
        Self {
            directory,
            owner_lock,
        }
    }

    pub(crate) fn unit_path(&self, unit_number: u64) -> PathBuf {
        self.directory
            .join(format!("{UNIT_PREFIX}{unit_number}{UNIT_SUFFIX}"))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        drop(self.owner_lock.take());
        drop(fs::remove_file(self.directory.join(OWNER_LOCK)));
        drop(fs::remove_dir(&self.directory));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionName {
    pub(crate) host: String,
    pub(crate) process: u32,
    pub(crate) stamp: u128,
    pub(crate) attempt: u32,
}

impl std::fmt::Display for SessionName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{SESSION_PREFIX}-{}-{}-{}-{}",
            self.host, self.process, self.stamp, self.attempt
        )
    }
}

impl SessionName {
    fn parse(name: &str) -> Option<Self> {
        let mut parts = name.split('-');
        let prefix = parts.next()?;
        let host = parts.next()?;
        let process = parts.next()?.parse().ok()?;
        let stamp = parts.next()?.parse().ok()?;
        let attempt = parts.next()?.parse().ok()?;
        let is_ours = prefix == SESSION_PREFIX
            && parts.next().is_none()
            && host.bytes().all(|byte| byte.is_ascii_hexdigit());
        is_ours.then(|| Self {
            host: host.to_owned(),
            process,
            stamp,
            attempt,
        })
    }
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn this_host() -> String {
    System::host_name().map_or_else(
        |_| String::new(),
        |name| {
            name.bytes()
                .take(HOST_NAME_BYTES_IN_A_SESSION_NAME)
                .flat_map(|byte| {
                    [
                        HEX_DIGITS[usize::from(byte >> 4)],
                        HEX_DIGITS[usize::from(byte & 15)],
                    ]
                })
                .map(char::from)
                .collect()
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerProcess {
    ThisOne,
    Running,
    Gone,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerLock {
    Held,
    NotHeld,
    Unknown,
}

pub(crate) fn is_proven_dead(
    session_host: &str,
    this_host: &str,
    owner: OwnerProcess,
    lock: impl FnOnce() -> OwnerLock,
) -> bool {
    !this_host.is_empty()
        && session_host == this_host
        && owner == OwnerProcess::Gone
        && lock() != OwnerLock::Held
}

pub(crate) fn owner_process(process: u32, session_created_nanos: u128) -> OwnerProcess {
    if process == process::id() {
        return OwnerProcess::ThisOne;
    }
    let Ok(mut system) = System::new() else {
        return OwnerProcess::Unknown;
    };
    let process = Pid::from_u32(process);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[process]),
        true,
        ProcessRefreshKind::nothing(),
    );
    let session_created_seconds = session_created_nanos / 1_000_000_000;
    match system.process(process) {
        Some(found) if u128::from(found.start_time()) > session_created_seconds => {
            OwnerProcess::Gone
        }
        Some(_) => OwnerProcess::Running,
        None => OwnerProcess::Gone,
    }
}

fn owner_lock(session_directory: &Path) -> OwnerLock {
    match OpenOptions::new()
        .write(true)
        .open(session_directory.join(OWNER_LOCK))
    {
        Ok(file) => match file.try_lock() {
            Ok(()) => OwnerLock::NotHeld,
            Err(TryLockError::WouldBlock) => OwnerLock::Held,
            Err(TryLockError::Error(_)) => OwnerLock::Unknown,
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => OwnerLock::NotHeld,
        Err(_) => OwnerLock::Unknown,
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
/// folder under `folder` and deletes a session only when all of this holds:
/// its name says this computer made it, the process that made it no longer
/// runs, and nobody holds its lock file. A session of this process, of
/// another computer sharing the folder, or one it cannot judge stays. It
/// deletes only files named like buffer files, so a file somebody else put
/// there stays, together with its folder.
pub fn remove_leftovers(folder: &Path) -> Result<RemovedLeftovers, ReplayError> {
    let root = folder.join(ROOT_DIRECTORY);
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.is_dir() => remove_dead_sessions(&root, &this_host()),
        Ok(_) => Ok(RemovedLeftovers::default()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(RemovedLeftovers::default()),
        Err(error) => Err(ReplayError::storage(
            "look at the replay folder",
            root,
            error,
        )),
    }
}

pub(crate) fn remove_dead_sessions(
    root: &Path,
    this_host: &str,
) -> Result<RemovedLeftovers, ReplayError> {
    let list_error = |error| ReplayError::storage("list the replay folder", root, error);
    let mut removed = RemovedLeftovers::default();
    for entry in fs::read_dir(root).map_err(list_error)? {
        let entry = entry.map_err(list_error)?;
        let Some(name) = session_name(&entry) else {
            continue;
        };
        let directory = entry.path();
        let owner = owner_process(name.process, name.stamp);
        if is_proven_dead(&name.host, this_host, owner, || owner_lock(&directory)) {
            remove_unit_files(&directory, &mut removed);
            drop(fs::remove_file(directory.join(OWNER_LOCK)));
            if fs::remove_dir(&directory).is_ok() {
                removed.sessions += 1;
            }
        }
    }
    Ok(removed)
}

fn remove_unit_files(session_directory: &Path, removed: &mut RemovedLeftovers) {
    let Ok(entries) = fs::read_dir(session_directory) else {
        return;
    };
    for entry in entries.flatten() {
        let is_unit_file = entry.file_type().is_ok_and(|kind| kind.is_file())
            && entry.file_name().to_str().is_some_and(is_unit_file_name);
        if !is_unit_file {
            continue;
        }
        let bytes = entry.metadata().map_or(0, |metadata| metadata.len());
        if fs::remove_file(entry.path()).is_ok() {
            removed.files += 1;
            removed.bytes += bytes;
        }
    }
}

fn session_name(entry: &DirEntry) -> Option<SessionName> {
    let is_directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
    let name = entry.file_name();
    is_directory
        .then(|| SessionName::parse(name.to_str()?))
        .flatten()
}

fn is_unit_file_name(name: &str) -> bool {
    name.strip_prefix(UNIT_PREFIX)
        .and_then(|rest| rest.strip_suffix(UNIT_SUFFIX))
        .is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}
