use std::time::Duration;

use super::{AfterPick, Ngine};
use crate::bar::BarState;
use crate::cli::{Command, Reply};
use crate::engine::{Refusal, ReplaySpan, Target};
use crate::format;
use crate::settings::SHORT_SAVE_SECONDS;

const PICK_FIRST: &str = "Choose what to record in the system picker";
const NOTHING_TO_RECORD: &str = "There is nothing to record";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ToggleRecording,
    AddMarker,
    ToggleReplay,
    SaveShort,
    SaveCustom,
    SaveEverything,
    ShowWindow,
    Quit,
}

impl Action {
    pub const ALL: [Self; 8] = [
        Self::ToggleRecording,
        Self::AddMarker,
        Self::ToggleReplay,
        Self::SaveShort,
        Self::SaveCustom,
        Self::SaveEverything,
        Self::ShowWindow,
        Self::Quit,
    ];

    pub fn number(self) -> usize {
        Self::ALL
            .iter()
            .position(|action| *action == self)
            .unwrap_or(Self::ALL.len())
    }

    pub fn numbered(number: usize) -> Option<Self> {
        Self::ALL.get(number).copied()
    }
}

impl Ngine {
    fn target_or_pick(&mut self, then: AfterPick) -> Result<Target, String> {
        if let Some(target) = self.target.clone() {
            return Ok(target);
        }
        if self.capabilities.lists_targets {
            return Err(NOTHING_TO_RECORD.into());
        }
        self.pick();
        self.after_pick = Some(then);
        Err(PICK_FIRST.into())
    }

    fn refused(&mut self, refusal: &Refusal) -> String {
        if let Refusal::Blocked(problem) = refusal {
            self.raise(problem.clone());
        }
        refusal.to_string()
    }

    pub fn start_recording(&mut self) -> Reply {
        let target = match self.target_or_pick(AfterPick::Record) {
            Ok(target) => target,
            Err(waiting) if waiting == PICK_FIRST => return Ok(waiting),
            Err(nothing) => return Err(nothing),
        };
        let request = self.recording_request(target);
        match self.engine.start_recording(&request) {
            Ok(()) => Ok(format!("Recording {}", request.target.name)),
            Err(refusal) => Err(self.refused(&refusal)),
        }
    }

    pub fn stop_recording(&mut self) -> Reply {
        let elapsed = self
            .engine
            .status()
            .recording
            .map_or(Duration::ZERO, |recording| recording.elapsed);
        match self.engine.stop_recording() {
            Ok(()) => Ok(format!("Recording saved · {}", format::length(elapsed))),
            Err(refusal) => Err(self.refused(&refusal)),
        }
    }

    pub fn set_replay(&mut self, on: bool) -> Reply {
        if !on {
            return match self.engine.stop_replay() {
                Ok(()) => Ok("Instant replay is off".into()),
                Err(refusal) => Err(self.refused(&refusal)),
            };
        }
        let target = match self.target_or_pick(AfterPick::Replay) {
            Ok(target) => target,
            Err(waiting) if waiting == PICK_FIRST => return Ok(waiting),
            Err(nothing) => return Err(nothing),
        };
        let request = self.replay_request(target);
        match self.engine.start_replay(&request) {
            Ok(()) => Ok(format!(
                "Instant replay is on, keeping the last {}",
                format::length(request.length)
            )),
            Err(refusal) => Err(self.refused(&refusal)),
        }
    }

    pub fn save_replay(&mut self, span: ReplaySpan) -> Reply {
        match self.engine.save_replay(span) {
            Ok(()) => Ok(match span {
                ReplaySpan::Everything => "Saving everything the replay buffer holds".into(),
                ReplaySpan::Last(length) => {
                    format!("Saving the last {}", format::length(length))
                }
            }),
            Err(refusal) => Err(self.refused(&refusal)),
        }
    }

    pub fn add_marker(&mut self) -> Reply {
        match self.engine.add_marker() {
            Ok(at) => Ok(format!("Marker added at {}", format::timer(at))),
            Err(refusal) => Err(self.refused(&refusal)),
        }
    }

    pub fn status_line(&self) -> String {
        let status = self.engine.status();
        let recording = status.recording.map(|recording| {
            format!(
                "Recording {} for {}",
                recording.target.name,
                format::length(recording.elapsed)
            )
        });
        let replay = status.replay.map(|replay| {
            format!(
                "Instant replay is on, holding {} of {}",
                format::length(replay.held),
                format::length(replay.length)
            )
        });
        match (recording, replay) {
            (None, None) => "Nothing is running".into(),
            (Some(line), None) | (None, Some(line)) => line,
            (Some(recording), Some(replay)) => format!("{recording}. {replay}"),
        }
    }

    fn short_save() -> ReplaySpan {
        ReplaySpan::Last(Duration::from_secs(u64::from(SHORT_SAVE_SECONDS)))
    }

    fn custom_save(&self) -> ReplaySpan {
        ReplaySpan::Last(Duration::from_secs(u64::from(
            self.settings.replay.save_seconds,
        )))
    }

    fn run(&mut self, action: Action) -> Reply {
        let reply = match action {
            Action::ToggleRecording if self.status.recording.is_some() => self.stop_recording(),
            Action::ToggleRecording => self.start_recording(),
            Action::AddMarker => self.add_marker(),
            Action::ToggleReplay => self.set_replay(self.status.replay.is_none()),
            Action::SaveShort => self.save_replay(Self::short_save()),
            Action::SaveCustom => self.save_replay(self.custom_save()),
            Action::SaveEverything => self.save_replay(ReplaySpan::Everything),
            Action::ShowWindow => {
                if let Some(shell) = &self.shell {
                    shell.open_window();
                }
                Ok("The ngine window is in front".into())
            }
            Action::Quit => {
                self.save_settings();
                if let Some(shell) = &self.shell {
                    shell.quit();
                }
                Ok("ngine is closing".into())
            }
        };
        self.sync(self.clock);
        reply
    }

    pub fn note(&mut self, reply: &Reply) {
        let shown_as_problem = reply.as_ref().is_err_and(|text| {
            self.problems
                .iter()
                .any(|problem| problem.summary() == text)
        });
        match reply {
            Ok(text) => self.tell(text.clone(), false),
            Err(_) if shown_as_problem => {}
            Err(text) => self.tell(text.clone(), true),
        }
    }

    pub fn perform(&mut self, action: Action) {
        let reply = self.run(action);
        if !matches!(action, Action::ShowWindow | Action::Quit) {
            self.note(&reply);
        }
    }

    pub fn execute(&mut self, command: Command) -> Reply {
        let reply = match command {
            Command::RecordToggle if self.engine.status().recording.is_some() => {
                self.stop_recording()
            }
            Command::RecordStart | Command::RecordToggle => self.start_recording(),
            Command::RecordStop => self.stop_recording(),
            Command::ReplayOn => self.set_replay(true),
            Command::ReplayOff => self.set_replay(false),
            Command::ReplayToggle => self.set_replay(self.engine.status().replay.is_none()),
            Command::ReplaySave(span) => self.save_replay(span),
            Command::Marker => self.add_marker(),
            Command::Status => return Ok(self.status_line()),
            Command::RecordStatus if self.engine.status().recording.is_some() => {
                return Ok(self.status_line());
            }
            Command::RecordStatus => return Err(Refusal::NotRecording.to_string()),
            Command::State | Command::Watch => {
                return Ok(BarState::of(&self.engine.status()).encode());
            }
            Command::Show => return self.run(Action::ShowWindow),
        };
        self.sync(self.clock);
        self.note(&reply);
        reply
    }
}
