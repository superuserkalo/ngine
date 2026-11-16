use std::time::Duration;

use crate::app::Action;
use crate::engine::Status;
use crate::format;
use crate::settings::{SHORT_SAVE_SECONDS, Settings};

#[cfg(target_os = "macos")]
#[path = "tray/macos.rs"]
mod backend;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "tray/notification_area.rs"]
mod backend;

pub use backend::Tray;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indicator {
    Idle,
    ReplayOn,
    Recording,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Item {
        action: Action,
        label: String,
        enabled: bool,
    },
    Separator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub indicator: Indicator,
    pub tooltip: String,
    pub entries: Vec<Entry>,
}

fn item(action: Action, label: impl Into<String>, enabled: bool) -> Entry {
    Entry::Item {
        action,
        label: label.into(),
        enabled,
    }
}

fn seconds(seconds: u16) -> String {
    format::length(Duration::from_secs(u64::from(seconds)))
}

impl Model {
    pub fn of(status: &Status, settings: &Settings) -> Self {
        let recording = status.recording.is_some();
        let replay = status.replay.is_some();
        let indicator = match (recording, replay) {
            (true, _) => Indicator::Recording,
            (false, true) => Indicator::ReplayOn,
            (false, false) => Indicator::Idle,
        };
        let tooltip = match indicator {
            Indicator::Idle => "ngine is idle",
            Indicator::ReplayOn => "ngine: instant replay is on",
            Indicator::Recording => "ngine is recording",
        };
        Self {
            indicator,
            tooltip: tooltip.into(),
            entries: vec![
                item(
                    Action::ToggleRecording,
                    if recording {
                        "Stop recording"
                    } else {
                        "Start recording"
                    },
                    true,
                ),
                item(Action::AddMarker, "Add marker", recording),
                Entry::Separator,
                item(
                    Action::ToggleReplay,
                    if replay {
                        "Turn instant replay off"
                    } else {
                        "Turn instant replay on"
                    },
                    true,
                ),
                item(
                    Action::SaveShort,
                    format!("Save last {}", seconds(SHORT_SAVE_SECONDS)),
                    replay,
                ),
                item(
                    Action::SaveCustom,
                    format!("Save last {}", seconds(settings.replay.save_seconds)),
                    replay,
                ),
                item(Action::SaveEverything, "Save everything", replay),
                Entry::Separator,
                item(Action::ShowWindow, "Open ngine", true),
                item(Action::Quit, "Quit ngine", true),
            ],
        }
    }
}

#[cfg(any(test, target_os = "linux", target_os = "windows"))]
mod icon {
    use super::Indicator;

    pub const SIDE: u32 = 32;
    const SAMPLES: u32 = 4;
    const CENTRE: i64 = (SIDE * SAMPLES / 2) as i64;
    const OUTER: i64 = 14 * SAMPLES as i64;
    const RING_INNER: i64 = 11 * SAMPLES as i64;
    const DOT: i64 = 6 * SAMPLES as i64;
    const WHITE: [u8; 3] = [255, 255, 255];
    const RED: [u8; 3] = [235, 64, 52];

    fn inked(indicator: Indicator, distance_squared: i64) -> bool {
        let within = |radius: i64| distance_squared <= radius * radius;
        match indicator {
            Indicator::Idle => within(OUTER) && !within(RING_INNER),
            Indicator::ReplayOn => (within(OUTER) && !within(RING_INNER)) || within(DOT),
            Indicator::Recording => within(OUTER),
        }
    }

    fn coverage(indicator: Indicator, x: u32, y: u32) -> u8 {
        let mut inside = 0u32;
        for sub_y in 0..SAMPLES {
            for sub_x in 0..SAMPLES {
                let dx = i64::from(x * SAMPLES + sub_x) - CENTRE;
                let dy = i64::from(y * SAMPLES + sub_y) - CENTRE;
                inside += u32::from(inked(indicator, dx * dx + dy * dy));
            }
        }
        u8::try_from(inside * 255 / (SAMPLES * SAMPLES)).unwrap_or(u8::MAX)
    }

    /// Straight RGBA rows, `SIDE` pixels each way.
    pub fn pixels(indicator: Indicator) -> Vec<u8> {
        let [red, green, blue] = match indicator {
            Indicator::Recording => RED,
            Indicator::Idle | Indicator::ReplayOn => WHITE,
        };
        (0..SIDE)
            .flat_map(|y| (0..SIDE).map(move |x| (x, y)))
            .flat_map(|(x, y)| [red, green, blue, coverage(indicator, x, y)])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Recording, Replay, Target, TargetKind};
    use std::path::PathBuf;

    fn settings() -> Settings {
        Settings::defaults(PathBuf::new(), PathBuf::new())
    }

    fn target() -> Target {
        Target {
            id: "screen".into(),
            kind: TargetKind::Screen,
            name: "Screen".into(),
            detail: String::new(),
        }
    }

    fn recording() -> Recording {
        Recording {
            target: target(),
            elapsed: Duration::from_secs(5),
            markers: 0,
        }
    }

    fn replay() -> Replay {
        Replay {
            target: target(),
            held: Duration::from_secs(5),
            length: Duration::from_secs(60),
            on_disk: false,
            bytes: 0,
        }
    }

    fn enabled(model: &Model, wanted: Action) -> bool {
        model.entries.iter().any(
            |entry| matches!(entry, Entry::Item { action, enabled: true, .. } if *action == wanted),
        )
    }

    fn label(model: &Model, wanted: Action) -> String {
        model
            .entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Item { action, label, .. } if *action == wanted => Some(label.clone()),
                _ => None,
            })
            .expect("the action is in the menu")
    }

    #[test]
    fn idle_offers_to_start_and_greys_out_what_needs_something_running() {
        let model = Model::of(&Status::default(), &settings());
        assert_eq!(model.indicator, Indicator::Idle);
        assert_eq!(label(&model, Action::ToggleRecording), "Start recording");
        assert_eq!(
            label(&model, Action::ToggleReplay),
            "Turn instant replay on"
        );
        assert!(!enabled(&model, Action::AddMarker));
        assert!(!enabled(&model, Action::SaveShort));
        assert!(enabled(&model, Action::Quit));
    }

    #[test]
    fn recording_wins_over_replay_in_the_indicator() {
        let status = Status {
            recording: Some(recording()),
            replay: Some(replay()),
            levels: None,
        };
        let model = Model::of(&status, &settings());
        assert_eq!(model.indicator, Indicator::Recording);
        assert_eq!(label(&model, Action::ToggleRecording), "Stop recording");
        assert!(enabled(&model, Action::AddMarker));
        assert!(enabled(&model, Action::SaveEverything));
    }

    #[test]
    fn replay_alone_shows_replay_and_names_the_save_lengths() {
        let status = Status {
            recording: None,
            replay: Some(replay()),
            levels: None,
        };
        let model = Model::of(&status, &settings());
        assert_eq!(model.indicator, Indicator::ReplayOn);
        assert_eq!(label(&model, Action::SaveShort), "Save last 30 s");
        assert_eq!(label(&model, Action::SaveCustom), "Save last 1 min");
    }

    #[test]
    fn the_three_icons_differ_and_have_the_right_size() {
        let icons = [Indicator::Idle, Indicator::ReplayOn, Indicator::Recording].map(icon::pixels);
        for pixels in &icons {
            assert_eq!(pixels.len(), (icon::SIDE * icon::SIDE * 4) as usize);
        }
        assert_ne!(icons[0], icons[1]);
        assert_ne!(icons[1], icons[2]);
    }
}
