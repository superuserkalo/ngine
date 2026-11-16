use std::fmt;
use std::time::Duration;

use crate::engine::ReplaySpan;

pub const USAGE: &str = "\
Usage: ngine [command]

With no command, ngine opens its window, or brings the window of the running
ngine forward.

Commands for the running ngine:
  record start              Start recording
  record stop               Stop recording and keep the file
  record toggle             Start recording, or stop the running recording
  record status             Succeed while a recording runs, fail otherwise
  replay on                 Turn instant replay on
  replay off                Turn instant replay off
  replay toggle             Turn instant replay on or off
  replay save               Save everything the replay buffer holds
  replay save --last 30s    Save the last 30 seconds (also 90, 2m, 1m30s)
  marker                    Drop a marker in the running recording
  status                    Say what is running

Options:
  --background              Start without opening the window
  --help                    Show this text";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    RecordStart,
    RecordStop,
    RecordToggle,
    RecordStatus,
    ReplayOn,
    ReplayOff,
    ReplayToggle,
    ReplaySave(ReplaySpan),
    Marker,
    Status,
    State,
    Watch,
    Show,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsageError {
    UnknownCommand(String),
    MissingLength,
    BadLength(String),
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(command) => {
                write!(f, "ngine does not know the command \"{command}\"")
            }
            Self::MissingLength => f.write_str("--last needs a length, such as 30s"),
            Self::BadLength(length) => write!(
                f,
                "\"{length}\" is not a length; write it like 30s, 90, 2m or 1m30s"
            ),
        }
    }
}

pub type Reply = Result<String, String>;

pub fn parse_length(text: &str) -> Option<Duration> {
    let mut seconds: u64 = 0;
    let mut digits = String::new();
    for character in text.chars() {
        match character {
            '0'..='9' => digits.push(character),
            'm' | 's' if !digits.is_empty() => {
                let number: u64 = digits.parse().ok()?;
                let scale = if character == 'm' { 60 } else { 1 };
                seconds = seconds.checked_add(number.checked_mul(scale)?)?;
                digits.clear();
            }
            _ => return None,
        }
    }
    if !digits.is_empty() {
        seconds = seconds.checked_add(digits.parse().ok()?)?;
    }
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

fn parse_save(options: &[&str]) -> Result<Command, UsageError> {
    match options {
        [] => Ok(Command::ReplaySave(ReplaySpan::Everything)),
        ["--last"] => Err(UsageError::MissingLength),
        ["--last", length] => parse_length(length)
            .map(|length| Command::ReplaySave(ReplaySpan::Last(length)))
            .ok_or_else(|| UsageError::BadLength((*length).to_owned())),
        _ => Err(UsageError::UnknownCommand(format!(
            "replay save {}",
            options.join(" ")
        ))),
    }
}

pub fn parse(arguments: &[String]) -> Result<Command, UsageError> {
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["record", "start"] => Ok(Command::RecordStart),
        ["record", "stop"] => Ok(Command::RecordStop),
        ["record", "toggle"] => Ok(Command::RecordToggle),
        ["record", "status"] => Ok(Command::RecordStatus),
        ["replay", "on"] => Ok(Command::ReplayOn),
        ["replay", "off"] => Ok(Command::ReplayOff),
        ["replay", "toggle"] => Ok(Command::ReplayToggle),
        ["replay", "save", options @ ..] => parse_save(options),
        ["marker"] => Ok(Command::Marker),
        ["status"] => Ok(Command::Status),
        _ => Err(UsageError::UnknownCommand(words.join(" "))),
    }
}

impl Command {
    pub fn encode(self) -> String {
        match self {
            Self::RecordStart => "record start".into(),
            Self::RecordStop => "record stop".into(),
            Self::RecordToggle => "record toggle".into(),
            Self::RecordStatus => "record status".into(),
            Self::ReplayOn => "replay on".into(),
            Self::ReplayOff => "replay off".into(),
            Self::ReplayToggle => "replay toggle".into(),
            Self::ReplaySave(ReplaySpan::Everything) => "replay save all".into(),
            Self::ReplaySave(ReplaySpan::Last(length)) => {
                format!("replay save {}", length.as_secs())
            }
            Self::Marker => "marker".into(),
            Self::Status => "status".into(),
            Self::State => "state".into(),
            Self::Watch => "watch".into(),
            Self::Show => "show".into(),
        }
    }

    pub fn decode(line: &str) -> Option<Self> {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["record", "start"] => Some(Self::RecordStart),
            ["record", "stop"] => Some(Self::RecordStop),
            ["record", "toggle"] => Some(Self::RecordToggle),
            ["record", "status"] => Some(Self::RecordStatus),
            ["replay", "on"] => Some(Self::ReplayOn),
            ["replay", "off"] => Some(Self::ReplayOff),
            ["replay", "toggle"] => Some(Self::ReplayToggle),
            ["replay", "save", "all"] => Some(Self::ReplaySave(ReplaySpan::Everything)),
            ["replay", "save", seconds] => seconds
                .parse()
                .ok()
                .filter(|seconds| *seconds > 0)
                .map(|seconds| Self::ReplaySave(ReplaySpan::Last(Duration::from_secs(seconds)))),
            ["marker"] => Some(Self::Marker),
            ["status"] => Some(Self::Status),
            ["state"] => Some(Self::State),
            ["watch"] => Some(Self::Watch),
            ["show"] => Some(Self::Show),
            _ => None,
        }
    }
}

pub fn encode_reply(reply: &Reply) -> String {
    let (word, message) = match reply {
        Ok(message) => ("ok", message),
        Err(message) => ("error", message),
    };
    format!("{word} {}", message.replace('\n', " "))
}

pub fn decode_reply(line: &str) -> Option<Reply> {
    let line = line.trim_end();
    if let Some(message) = line.strip_prefix("ok") {
        return Some(Ok(message.trim_start().to_owned()));
    }
    line.strip_prefix("error")
        .map(|message| Err(message.trim_start().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    const EVERY_COMMAND: [Command; 14] = [
        Command::RecordStart,
        Command::RecordStop,
        Command::RecordToggle,
        Command::RecordStatus,
        Command::ReplayOn,
        Command::ReplayOff,
        Command::ReplayToggle,
        Command::ReplaySave(ReplaySpan::Everything),
        Command::ReplaySave(ReplaySpan::Last(Duration::from_secs(30))),
        Command::Marker,
        Command::Status,
        Command::State,
        Command::Watch,
        Command::Show,
    ];

    #[test]
    fn the_documented_commands_parse() {
        let last_thirty = Command::ReplaySave(ReplaySpan::Last(Duration::from_secs(30)));
        for (line, command) in [
            ("record start", Command::RecordStart),
            ("record stop", Command::RecordStop),
            ("record toggle", Command::RecordToggle),
            ("record status", Command::RecordStatus),
            ("replay on", Command::ReplayOn),
            ("replay off", Command::ReplayOff),
            ("replay toggle", Command::ReplayToggle),
            ("replay save --last 30s", last_thirty),
            ("replay save", Command::ReplaySave(ReplaySpan::Everything)),
            ("marker", Command::Marker),
            ("status", Command::Status),
        ] {
            assert_eq!(parse(&arguments(line)), Ok(command), "{line}");
        }
    }

    #[test]
    fn lengths_take_seconds_and_minutes() {
        assert_eq!(parse_length("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_length("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_length("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_length("1m30s"), Some(Duration::from_secs(90)));
        for bad in [
            "",
            "0",
            "0s",
            "s",
            "30x",
            "-5",
            "1h",
            "99999999999999999999m",
        ] {
            assert_eq!(parse_length(bad), None, "{bad}");
        }
    }

    #[test]
    fn mistakes_are_named() {
        assert_eq!(
            parse(&arguments("replay save --last")),
            Err(UsageError::MissingLength)
        );
        assert_eq!(
            parse(&arguments("replay save --last soon")),
            Err(UsageError::BadLength("soon".into()))
        );
        assert_eq!(
            parse(&arguments("record pause")),
            Err(UsageError::UnknownCommand("record pause".into()))
        );
    }

    #[test]
    fn every_command_survives_the_wire() {
        for command in EVERY_COMMAND {
            assert_eq!(Command::decode(&command.encode()), Some(command));
        }
        assert_eq!(Command::decode("replay save 0"), None);
        assert_eq!(Command::decode("dance"), None);
    }

    #[test]
    fn replies_survive_the_wire() {
        for reply in [
            Ok("Recording started".to_owned()),
            Err("No recording is running".to_owned()),
            Ok(String::new()),
        ] {
            assert_eq!(decode_reply(&encode_reply(&reply)), Some(reply));
        }
        assert_eq!(decode_reply("maybe"), None);
    }
}
