use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use crate::cli::{self, Command, Reply};

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "ipc/unix.rs"]
mod transport;
#[cfg(target_os = "windows")]
#[path = "ipc/windows.rs"]
mod transport;

pub use transport::Address;

#[derive(Debug)]
pub enum ClientError {
    NotRunning,
    Io(io::Error),
    BadReply(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => f.write_str("ngine is not running"),
            Self::Io(error) => write!(f, "ngine could not be reached: {error}"),
            Self::BadReply(reply) => {
                write!(f, "ngine gave an answer that makes no sense: {reply}")
            }
        }
    }
}

pub fn send(address: &Address, command: Command) -> Result<Reply, ClientError> {
    let mut stream = transport::connect(address).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => ClientError::NotRunning,
        _ => ClientError::Io(error),
    })?;
    writeln!(stream, "{}", command.encode()).map_err(ClientError::Io)?;
    stream.flush().map_err(ClientError::Io)?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(ClientError::Io)?;
    cli::decode_reply(&line).ok_or(ClientError::BadReply(line))
}

/// Asks the running ngine to send its state now and each time it changes.
/// The iterator ends when ngine quits or the connection breaks.
pub fn watch(address: &Address) -> Result<impl Iterator<Item = String>, ClientError> {
    let mut stream = transport::connect(address).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => ClientError::NotRunning,
        _ => ClientError::Io(error),
    })?;
    transport::wait_without_limit(&stream).map_err(ClientError::Io)?;
    writeln!(stream, "{}", Command::Watch.encode()).map_err(ClientError::Io)?;
    stream.flush().map_err(ClientError::Io)?;
    Ok(BufReader::new(stream)
        .lines()
        .map_while(Result::ok)
        .map_while(|line| cli::decode_reply(&line).and_then(Result::ok)))
}

#[derive(Default)]
struct Published {
    line: Option<String>,
    version: u64,
    stopping: bool,
}

/// The connections that asked to follow the state. Each one is written to
/// from a thread of its own, so a slow reader never holds up the app.
#[derive(Clone, Default)]
pub struct Watchers(Arc<(Mutex<Published>, Condvar)>);

impl Watchers {
    fn published(&self) -> MutexGuard<'_, Published> {
        self.0.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends the line to every watcher, unless it is the line sent last.
    pub fn publish(&self, line: String) {
        let mut published = self.published();
        if published.line.as_ref() == Some(&line) {
            return;
        }
        published.line = Some(line);
        published.version += 1;
        self.0.1.notify_all();
    }

    fn stop(&self) {
        self.published().stopping = true;
        self.0.1.notify_all();
    }

    fn follow(&self, mut stream: transport::Stream, first: &Reply) -> io::Result<()> {
        let mut seen = None;
        loop {
            let line = {
                let mut published = self.published();
                while !published.stopping && seen == Some(published.version) {
                    published = self
                        .0
                        .1
                        .wait(published)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                if published.stopping {
                    return Ok(());
                }
                seen = Some(published.version);
                published.line.clone().map_or_else(|| first.clone(), Ok)
            };
            writeln!(stream, "{}", cli::encode_reply(&line))?;
            stream.flush()?;
        }
    }

    fn follow_on_a_thread(&self, stream: transport::Stream, first: Reply) -> io::Result<()> {
        let watchers = self.clone();
        thread::Builder::new()
            .name("ngine-watcher".into())
            .spawn(move || {
                let _reader_left = watchers.follow(stream, &first);
            })
            .map(drop)
    }
}

fn answer(
    stream: transport::Stream,
    handler: &impl Fn(Command) -> Reply,
    watchers: &Watchers,
) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.is_empty() {
        return Ok(());
    }
    let reply = match Command::decode(&line) {
        Some(Command::Watch) => {
            return watchers.follow_on_a_thread(reader.into_inner(), handler(Command::Watch));
        }
        Some(command) => handler(command),
        None => Err(format!(
            "ngine does not know the command \"{}\"",
            line.trim()
        )),
    };
    let stream = reader.get_mut();
    writeln!(stream, "{}", cli::encode_reply(&reply))?;
    stream.flush()
}

/// Keeps the address claimed for as long as it lives. Dropping it stops the
/// listening thread, so every handle to the address is closed before the
/// drop returns.
pub struct Server {
    address: Address,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    watchers: Watchers,
}

impl Server {
    pub fn watchers(&self) -> Watchers {
        self.watchers.clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.watchers.stop();
        self.stopping.store(true, Ordering::SeqCst);
        let woken = transport::connect(&self.address);
        if woken.is_ok()
            && let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("ngine: the command listener ended badly");
        }
        drop(woken);
        transport::release(&self.address);
    }
}

/// Fails with [`io::ErrorKind::AddrInUse`] when another process answers at
/// the address.
pub fn serve(
    address: &Address,
    handler: impl Fn(Command) -> Reply + Send + 'static,
) -> io::Result<Server> {
    let listener = transport::listen(address)?;
    let stopping = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&stopping);
    let watchers = Watchers::default();
    let served = watchers.clone();
    let thread = thread::Builder::new()
        .name("ngine-commands".into())
        .spawn(move || {
            loop {
                let accepted = transport::accept(&listener);
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                if let Err(error) = accepted.and_then(|stream| answer(stream, &handler, &served)) {
                    eprintln!("ngine: a command could not be answered: {error}");
                }
            }
        })?;
    Ok(Server {
        address: address.clone(),
        stopping,
        thread: Some(thread),
        watchers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ReplaySpan;
    use std::time::Duration;

    fn echo(command: Command) -> Reply {
        match command {
            Command::Marker => Err("No recording is running".into()),
            other => Ok(other.encode()),
        }
    }

    #[test]
    fn a_command_reaches_the_server_and_its_reply_comes_back() {
        let address = Address::for_test("round-trip");
        let _server = serve(&address, echo).expect("the server starts");
        let save = Command::ReplaySave(ReplaySpan::Last(Duration::from_secs(30)));
        assert_eq!(
            send(&address, save).expect("a reply"),
            Ok("replay save 30".into())
        );
        assert_eq!(
            send(&address, Command::Status).expect("a reply"),
            Ok("status".into())
        );
        assert_eq!(
            send(&address, Command::Marker).expect("a reply"),
            Err("No recording is running".into())
        );
    }

    #[test]
    fn with_no_server_the_client_says_ngine_is_not_running() {
        let address = Address::for_test("nobody-home");
        assert!(matches!(
            send(&address, Command::Status),
            Err(ClientError::NotRunning)
        ));
    }

    #[test]
    fn a_second_server_at_the_same_address_is_refused() {
        let address = Address::for_test("taken");
        let _server = serve(&address, echo).expect("the server starts");
        let second = serve(&address, echo);
        assert_eq!(
            second.err().map(|error| error.kind()),
            Some(io::ErrorKind::AddrInUse)
        );
    }

    #[test]
    fn a_watcher_gets_the_state_now_and_on_each_change_until_ngine_quits() {
        let address = Address::for_test("watch");
        let server = serve(&address, |_| Ok("idle".into())).expect("the server starts");
        let watchers = server.watchers();
        let mut states = watch(&address).expect("a watch");
        assert_eq!(states.next().as_deref(), Some("idle"));
        watchers.publish("recording 1".into());
        assert_eq!(states.next().as_deref(), Some("recording 1"));
        watchers.publish("recording 1".into());
        watchers.publish("recording 2".into());
        assert_eq!(states.next().as_deref(), Some("recording 2"));
        assert_eq!(
            send(&address, Command::Status).expect("a reply"),
            Ok("idle".into())
        );
        drop(server);
        assert_eq!(states.next(), None);
    }

    #[test]
    fn a_watcher_that_joins_late_gets_the_last_state_published() {
        let address = Address::for_test("watch-late");
        let server = serve(&address, |_| Ok("idle".into())).expect("the server starts");
        server.watchers().publish("replay 4 60".into());
        let mut states = watch(&address).expect("a watch");
        assert_eq!(states.next().as_deref(), Some("replay 4 60"));
    }

    #[test]
    fn watching_with_no_server_says_ngine_is_not_running() {
        let address = Address::for_test("watch-nobody");
        assert!(matches!(watch(&address), Err(ClientError::NotRunning)));
    }

    #[test]
    fn a_server_that_ended_frees_its_address() {
        let address = Address::for_test("freed");
        drop(serve(&address, echo).expect("the server starts"));
        let _server = serve(&address, echo).expect("the address is free again");
        assert!(send(&address, Command::Status).is_ok());
    }
}
