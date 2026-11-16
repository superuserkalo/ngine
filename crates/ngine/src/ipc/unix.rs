use std::fs::{self, DirBuilder, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKET_NAME: &str = "ngine.sock";
const OWNER_ONLY_FOLDER: u32 = 0o700;
const OWNER_ONLY_SOCKET: u32 = 0o600;

pub type Stream = UnixStream;
pub type Listener = UnixListener;

#[derive(Clone, Debug)]
pub struct Address {
    folder: PathBuf,
}

impl Address {
    /// The folder must belong to the current user. It is closed to everyone
    /// else before the socket in it is used.
    pub fn in_folder(folder: &Path) -> Self {
        Self {
            folder: folder.to_owned(),
        }
    }

    #[cfg(test)]
    pub fn for_test(name: &str) -> Self {
        let folder = std::env::temp_dir().join(format!("ngine-{}-{name}", std::process::id()));
        Self::in_folder(&folder)
    }

    fn socket(&self) -> PathBuf {
        self.folder.join(SOCKET_NAME)
    }
}

fn refuse(folder: &Path, why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{} {why}", folder.display()),
    )
}

fn close_to_others(folder: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(folder)?.is_dir() {
        return Err(refuse(folder, "is not a folder of its own"));
    }
    fs::set_permissions(folder, Permissions::from_mode(OWNER_ONLY_FOLDER))
        .map_err(|_| refuse(folder, "belongs to someone else"))
}

fn with_timeouts(stream: UnixStream) -> io::Result<UnixStream> {
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
    stream.set_write_timeout(Some(REPLY_TIMEOUT))?;
    Ok(stream)
}

pub fn connect(address: &Address) -> io::Result<Stream> {
    close_to_others(&address.folder)?;
    with_timeouts(UnixStream::connect(address.socket())?)
}

fn bind(socket: &Path) -> io::Result<Listener> {
    let listener = UnixListener::bind(socket)?;
    fs::set_permissions(socket, Permissions::from_mode(OWNER_ONLY_SOCKET))?;
    Ok(listener)
}

pub fn listen(address: &Address) -> io::Result<Listener> {
    DirBuilder::new()
        .recursive(true)
        .mode(OWNER_ONLY_FOLDER)
        .create(&address.folder)?;
    close_to_others(&address.folder)?;
    let socket = address.socket();
    match bind(&socket) {
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            if UnixStream::connect(&socket).is_ok() {
                return Err(error);
            }
            fs::remove_file(&socket)?;
            bind(&socket)
        }
        bound => bound,
    }
}

pub fn accept(listener: &Listener) -> io::Result<Stream> {
    let (stream, _) = listener.accept()?;
    with_timeouts(stream)
}

pub fn release(address: &Address) {
    if let Err(error) = fs::remove_file(address.socket()) {
        eprintln!("ngine: the command socket could not be removed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path)
            .expect("the path exists")
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn the_socket_and_its_folder_are_closed_to_other_users() {
        let address = Address::for_test("private");
        fs::create_dir_all(&address.folder).expect("a temp folder");
        fs::set_permissions(&address.folder, Permissions::from_mode(0o755))
            .expect("the folder starts open");
        let _listener = listen(&address).expect("the socket is bound");
        assert_eq!(mode(&address.folder), OWNER_ONLY_FOLDER);
        assert_eq!(mode(&address.socket()), OWNER_ONLY_SOCKET);
        release(&address);
    }

    #[test]
    fn a_folder_that_is_a_link_to_somewhere_else_is_refused() {
        let real = Address::for_test("link-target");
        fs::create_dir_all(&real.folder).expect("a temp folder");
        let _listener = listen(&real).expect("the socket is bound");
        let linked = Address::for_test("link");
        symlink(&real.folder, &linked.folder).expect("a symbolic link");

        let refused = |error: io::Error| error.kind() == io::ErrorKind::PermissionDenied;
        assert!(connect(&linked).is_err_and(refused));
        assert!(listen(&linked).is_err_and(refused));
        fs::remove_file(&linked.folder).expect("the link is removed");
        release(&real);
    }
}
