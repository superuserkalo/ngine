use std::fmt;
use std::io::{self, BufRead, BufReader, Write};
use std::thread;

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

fn answer(stream: transport::Stream, handler: &impl Fn(Command) -> Reply) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.is_empty() {
        return Ok(());
    }
    let reply = match Command::decode(&line) {
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

/// Keeps the address claimed for as long as it lives.
pub struct Server {
    address: Address,
}

impl Drop for Server {
    fn drop(&mut self) {
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
    thread::Builder::new()
        .name("ngine-commands".into())
        .spawn(move || {
            loop {
                let served =
                    transport::accept(&listener).and_then(|stream| answer(stream, &handler));
                if let Err(error) = served {
                    eprintln!("ngine: a command could not be answered: {error}");
                }
            }
        })?;
    Ok(Server {
        address: address.clone(),
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
    fn a_server_that_ended_frees_its_address() {
        let address = Address::for_test("freed");
        drop(serve(&address, echo).expect("the server starts"));
        let _server = serve(&address, echo).expect("the address is free again");
        assert!(send(&address, Command::Status).is_ok());
    }
}
