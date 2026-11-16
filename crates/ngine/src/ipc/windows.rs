use std::fs::{self, OpenOptions};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use interprocess::local_socket::traits::{Listener as _, Stream as _};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Name, ToNsName};
use interprocess::os::windows::local_socket::ListenerOptionsExt;
use interprocess::os::windows::security_descriptor::SecurityDescriptor;
use widestring::U16CString;

pub type Stream = interprocess::local_socket::Stream;
pub type Listener = interprocess::local_socket::Listener;

const OWNER_ONLY: &str = "D:P(A;;GA;;;OW)";
const SECRET_FILE: &str = "command-pipe";
const SECRET_LENGTH: usize = 32;

#[derive(Clone, Debug)]
pub enum Address {
    /// The pipe's name ends in a secret kept in a file in this folder, which
    /// must be one that only the current user can read.
    SecretIn(PathBuf),
    #[cfg(test)]
    Named(String),
}

impl Address {
    pub fn in_folder(folder: &Path) -> Self {
        Self::SecretIn(folder.to_owned())
    }

    #[cfg(test)]
    pub fn for_test(name: &str) -> Self {
        Self::Named(format!("ngine-test-{}-{name}", std::process::id()))
    }
}

fn is_secret(text: &str) -> bool {
    text.len() == SECRET_LENGTH && text.chars().all(|character| character.is_ascii_hexdigit())
}

fn new_secret() -> String {
    let half = || RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", half(), half())
}

fn read_secret(file: &Path) -> io::Result<String> {
    let secret = fs::read_to_string(file)?.trim().to_owned();
    if is_secret(&secret) {
        Ok(secret)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the command pipe's secret is damaged",
        ))
    }
}

fn write_secret(file: &Path) -> io::Result<String> {
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder)?;
    }
    match fs::remove_file(file) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let secret = new_secret();
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)?
        .write_all(secret.as_bytes())?;
    Ok(secret)
}

fn pipe_name(address: &Address, serving: bool) -> io::Result<Name<'static>> {
    let name = match address {
        Address::SecretIn(folder) => {
            let file = folder.join(SECRET_FILE);
            let secret = if serving {
                write_secret(&file)?
            } else {
                read_secret(&file)?
            };
            format!("ngine-{secret}")
        }
        #[cfg(test)]
        Address::Named(name) => name.clone(),
    };
    name.to_ns_name::<GenericNamespaced>()
}

fn owner_only() -> io::Result<SecurityDescriptor> {
    let rule = U16CString::from_str(OWNER_ONLY)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    SecurityDescriptor::deserialize(&rule)
}

fn served_from_this_session(stream: &Stream) -> io::Result<bool> {
    let Stream::NamedPipe(pipe) = stream;
    let pipe = pipe.inner();
    Ok(pipe.server_session_id()? == pipe.client_session_id()?)
}

/// Connects only to the pipe named by the secret in the user's own folder,
/// and refuses a server from another logon session.
pub fn connect(address: &Address) -> io::Result<Stream> {
    let stream = Stream::connect(pipe_name(address, false)?)?;
    if served_from_this_session(&stream)? {
        Ok(stream)
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the command pipe belongs to another session",
        ))
    }
}

/// Creates the pipe under a fresh secret name, for the current user only.
/// interprocess creates it as the first instance, so a name that already
/// exists is an error, and it rejects remote clients.
pub fn listen(address: &Address) -> io::Result<Listener> {
    if matches!(address, Address::SecretIn(_)) && connect(address).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "another ngine already answers commands",
        ));
    }
    ListenerOptions::new()
        .name(pipe_name(address, true)?)
        .security_descriptor(owner_only()?)
        .create_sync()
        .map_err(|error| match error.kind() {
            io::ErrorKind::PermissionDenied => io::Error::new(io::ErrorKind::AddrInUse, error),
            _ => error,
        })
}

pub fn accept(listener: &Listener) -> io::Result<Stream> {
    let stream = listener.accept()?;
    if served_from_this_session(&stream)? {
        Ok(stream)
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "a command came from another session",
        ))
    }
}

pub fn release(address: &Address) {
    if let Address::SecretIn(folder) = address
        && let Err(error) = fs::remove_file(folder.join(SECRET_FILE))
    {
        eprintln!("ngine: the command pipe's secret could not be removed: {error}");
    }
}
