use std::fmt::Write as _;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use crate::engine::Status;
use crate::format;

const RECONNECT_WAIT: Duration = Duration::from_secs(5);
pub const RECORDING_ICON: &str = "\u{f0ec2}";
pub const REPLAY_ICON: &str = "\u{f02da}";

pub const USAGE: &str = "\
For a status bar such as Waybar:
  ngine status --waybar     Print the state as Waybar's JSON, once
  ngine status --waybar --watch
                            Print a line each time the state changes, and
                            keep going when ngine quits or starts again";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayFill {
    pub held_seconds: u64,
    pub length_seconds: u64,
}

/// What a status bar shows of the running ngine, to the whole second, so it
/// changes at most once a second.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BarState {
    pub recording_seconds: Option<u64>,
    pub replay: Option<ReplayFill>,
}

impl BarState {
    pub fn of(status: &Status) -> Self {
        Self {
            recording_seconds: status
                .recording
                .as_ref()
                .map(|recording| recording.elapsed.as_secs()),
            replay: status.replay.as_ref().map(|replay| ReplayFill {
                held_seconds: replay.held.min(replay.length).as_secs(),
                length_seconds: replay.length.as_secs(),
            }),
        }
    }

    pub fn encode(self) -> String {
        let mut words = Vec::new();
        if let Some(seconds) = self.recording_seconds {
            words.push(format!("recording {seconds}"));
        }
        if let Some(fill) = self.replay {
            words.push(format!(
                "replay {} {}",
                fill.held_seconds, fill.length_seconds
            ));
        }
        if words.is_empty() {
            "idle".into()
        } else {
            words.join(" ")
        }
    }

    pub fn decode(line: &str) -> Option<Self> {
        let words: Vec<&str> = line.split_whitespace().collect();
        let mut state = Self::default();
        let mut rest = words.as_slice();
        if rest == ["idle"] {
            return Some(state);
        }
        if let ["recording", seconds, after @ ..] = rest {
            state.recording_seconds = Some(seconds.parse().ok()?);
            rest = after;
        }
        if let ["replay", held, length, after @ ..] = rest {
            state.replay = Some(ReplayFill {
                held_seconds: held.parse().ok()?,
                length_seconds: length.parse().ok()?,
            });
            rest = after;
        }
        (rest.is_empty() && state != Self::default()).then_some(state)
    }
}

pub fn quoted(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            control if control.is_control() => {
                let _always_fits = write!(quoted, "\\u{:04x}", u32::from(control));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

struct Module {
    text: String,
    tooltip: String,
    classes: Vec<&'static str>,
    alt: &'static str,
    percentage: Option<u64>,
}

fn seconds(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

fn module(state: Option<BarState>) -> Module {
    let Some(state) = state else {
        return Module {
            text: String::new(),
            tooltip: "ngine is not running".into(),
            classes: vec!["stopped"],
            alt: "stopped",
            percentage: None,
        };
    };
    let replay_line = state.replay.map(|fill| {
        format!(
            "Instant replay is on, holding {} of {}",
            format::length(seconds(fill.held_seconds)),
            format::length(seconds(fill.length_seconds))
        )
    });
    let percentage = state.replay.and_then(|fill| {
        (fill.length_seconds > 0).then(|| (fill.held_seconds * 100 / fill.length_seconds).min(100))
    });
    match (state.recording_seconds, replay_line) {
        (Some(elapsed), replay_line) => {
            let timer = format::timer(seconds(elapsed));
            let mut tooltip = format!("Recording for {timer}\nClick to stop");
            let mut classes = vec!["recording"];
            if let Some(line) = replay_line {
                tooltip = format!("{tooltip}\n{line}");
                classes.push("replay");
            }
            Module {
                text: format!("{RECORDING_ICON} {timer}"),
                tooltip,
                classes,
                alt: "recording",
                percentage,
            }
        }
        (None, Some(line)) => Module {
            text: REPLAY_ICON.into(),
            tooltip: format!("{line}\nClick to save it"),
            classes: vec!["replay"],
            alt: "replay",
            percentage,
        },
        (None, None) => Module {
            text: String::new(),
            tooltip: "ngine is ready".into(),
            classes: vec!["idle"],
            alt: "idle",
            percentage: None,
        },
    }
}

/// One line of Waybar's custom-module JSON (`return-type: json`). `None`
/// means ngine is not running. Waybar hides the module while its text is
/// empty, so it shows only while ngine records or keeps a replay.
pub fn waybar(state: Option<BarState>) -> String {
    let module = module(state);
    let class = match module.classes.as_slice() {
        [one] => quoted(one),
        many => format!(
            "[{}]",
            many.iter()
                .map(|class| quoted(class))
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    let mut line = format!(
        "{{\"text\":{},\"tooltip\":{},\"class\":{class},\"alt\":{}",
        quoted(&module.text),
        quoted(&module.tooltip),
        quoted(module.alt)
    );
    if let Some(percentage) = module.percentage {
        let _always_fits = write!(line, ",\"percentage\":{percentage}");
    }
    line.push('}');
    line
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Once,
    Watch,
}

pub fn parse(options: &[String]) -> Result<Mode, String> {
    let mut waybar = false;
    let mut watch = false;
    for option in options {
        match option.as_str() {
            "--waybar" => waybar = true,
            "--watch" => watch = true,
            other => return Err(format!("ngine does not know the option \"{other}\"")),
        }
    }
    match (waybar, watch) {
        (true, false) => Ok(Mode::Once),
        (true, true) => Ok(Mode::Watch),
        (false, _) => Err("status --watch needs --waybar".into()),
    }
}

fn print(line: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{line}")?;
    out.flush()
}

/// Prints states as Waybar JSON. `connect` returns the states the running
/// ngine sends, or `None` when it is not running.
pub fn run<States: Iterator<Item = BarState>>(
    mode: Mode,
    mut connect: impl FnMut() -> Option<States>,
) -> Result<(), String> {
    let failed = |error: io::Error| format!("the state could not be printed: {error}");
    if mode == Mode::Once {
        let state = connect().and_then(|mut states| states.next());
        return print(&waybar(state)).map_err(failed);
    }
    let mut shown = None;
    loop {
        if let Some(states) = connect() {
            for state in states {
                let line = waybar(Some(state));
                if shown.as_ref() != Some(&line) {
                    print(&line).map_err(failed)?;
                    shown = Some(line);
                }
            }
        }
        let line = waybar(None);
        if shown.as_ref() != Some(&line) {
            print(&line).map_err(failed)?;
            shown = Some(line);
        }
        thread::sleep(RECONNECT_WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Recording, Replay, Target, TargetKind};

    fn target() -> Target {
        Target {
            id: "screen-1".into(),
            kind: TargetKind::Screen,
            name: "Built-in display".into(),
            detail: String::new(),
        }
    }

    fn recording(seconds: u64) -> BarState {
        BarState {
            recording_seconds: Some(seconds),
            replay: None,
        }
    }

    fn replay(held: u64, length: u64) -> BarState {
        BarState {
            recording_seconds: None,
            replay: Some(ReplayFill {
                held_seconds: held,
                length_seconds: length,
            }),
        }
    }

    #[test]
    fn the_state_comes_from_the_engine_status_in_whole_seconds() {
        let status = Status {
            recording: Some(Recording {
                target: target(),
                elapsed: Duration::from_millis(83_900),
                markers: 0,
            }),
            replay: Some(Replay {
                target: target(),
                held: Duration::from_secs(75),
                length: Duration::from_secs(60),
                on_disk: false,
                bytes: 0,
            }),
            levels: None,
        };
        let state = BarState::of(&status);
        assert_eq!(state.recording_seconds, Some(83));
        assert_eq!(
            state.replay,
            Some(ReplayFill {
                held_seconds: 60,
                length_seconds: 60
            })
        );
        assert_eq!(BarState::of(&Status::default()), BarState::default());
    }

    #[test]
    fn every_state_survives_the_wire() {
        let both = BarState {
            recording_seconds: Some(5),
            replay: Some(ReplayFill {
                held_seconds: 30,
                length_seconds: 60,
            }),
        };
        for state in [BarState::default(), recording(83), replay(30, 60), both] {
            assert_eq!(BarState::decode(&state.encode()), Some(state));
        }
        for bad in ["", "busy", "recording", "recording x", "replay 3", "idle 4"] {
            assert_eq!(BarState::decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn not_running_and_idle_hide_the_module() {
        assert_eq!(
            waybar(None),
            r#"{"text":"","tooltip":"ngine is not running","class":"stopped","alt":"stopped"}"#
        );
        assert_eq!(
            waybar(Some(BarState::default())),
            r#"{"text":"","tooltip":"ngine is ready","class":"idle","alt":"idle"}"#
        );
    }

    #[test]
    fn a_recording_shows_its_running_time() {
        assert_eq!(
            waybar(Some(recording(83))),
            format!(
                r#"{{"text":"{RECORDING_ICON} 01:23","tooltip":"Recording for 01:23\nClick to stop","class":"recording","alt":"recording"}}"#
            )
        );
    }

    #[test]
    fn replay_shows_how_full_its_buffer_is() {
        assert_eq!(
            waybar(Some(replay(30, 60))),
            format!(
                r#"{{"text":"{REPLAY_ICON}","tooltip":"Instant replay is on, holding 30 s of 1 min\nClick to save it","class":"replay","alt":"replay","percentage":50}}"#
            )
        );
    }

    #[test]
    fn recording_with_replay_on_carries_both_classes() {
        let state = BarState {
            recording_seconds: Some(3_725),
            replay: Some(ReplayFill {
                held_seconds: 60,
                length_seconds: 60,
            }),
        };
        let line = waybar(Some(state));
        assert!(line.contains(r#""class":["recording","replay"]"#), "{line}");
        assert!(line.contains("1:02:05"), "{line}");
        assert!(line.contains(r#""percentage":100"#), "{line}");
    }

    #[test]
    fn text_is_escaped_for_json() {
        assert_eq!(quoted("a \"b\" \\ c\n\u{1}"), r#""a \"b\" \\ c\n\u0001""#);
    }

    #[test]
    fn the_options_are_waybar_and_watch() {
        let options =
            |line: &str| -> Vec<String> { line.split_whitespace().map(str::to_owned).collect() };
        assert_eq!(parse(&options("--waybar")), Ok(Mode::Once));
        assert_eq!(parse(&options("--watch --waybar")), Ok(Mode::Watch));
        assert!(parse(&options("--watch")).is_err());
        assert!(parse(&options("--waybar --json")).is_err());
    }

    #[test]
    fn once_prints_the_first_state_or_not_running() {
        let mut asked = 0;
        let outcome = run(Mode::Once, || {
            asked += 1;
            Some(vec![recording(1), recording(2)].into_iter())
        });
        assert_eq!(outcome, Ok(()));
        assert_eq!(asked, 1);
        let nobody = run(Mode::Once, || None::<std::vec::IntoIter<BarState>>);
        assert_eq!(nobody, Ok(()));
    }
}
