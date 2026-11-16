use std::path::PathBuf;
use std::time::Duration;

use super::{Action, Ngine, demo};
use crate::cli::Command;
use crate::engine::{Permission, Problem, ReplaySpan};
use crate::settings::Store;

fn pass(app: &mut Ngine, time: Duration) {
    if let Some(simulator) = &app.simulator {
        simulator.advance(time);
    }
    app.sync(app.clock);
}

fn temp_file(test: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("ngine-app-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&folder).expect("a temp folder");
    folder.join("settings.json")
}

#[test]
fn commands_drive_a_recording_and_say_what_happened() {
    let mut app = demo::idle();
    assert_eq!(
        app.execute(Command::Status),
        Ok("Nothing is running".into())
    );
    assert_eq!(
        app.execute(Command::Marker),
        Err("No recording is running".into())
    );
    assert_eq!(
        app.execute(Command::RecordStart),
        Ok("Recording Built-in display".into())
    );
    assert_eq!(
        app.execute(Command::RecordStart),
        Err("A recording is already running".into())
    );
    pass(&mut app, Duration::from_secs(65));
    assert_eq!(
        app.execute(Command::Marker),
        Ok("Marker added at 01:05".into())
    );
    assert_eq!(
        app.execute(Command::Status),
        Ok("Recording Built-in display for 1 min 5 s".into())
    );
    assert_eq!(
        app.execute(Command::RecordStop),
        Ok("Recording saved · 1 min 5 s".into())
    );
    assert_eq!(
        app.execute(Command::RecordStop),
        Err("No recording is running".into())
    );
}

#[test]
fn commands_drive_replay_and_its_saves() {
    let mut app = demo::idle();
    let last_thirty = Command::ReplaySave(ReplaySpan::Last(Duration::from_secs(30)));
    assert_eq!(
        app.execute(last_thirty),
        Err("Instant replay is off".into())
    );
    assert_eq!(
        app.execute(Command::ReplayOn),
        Ok("Instant replay is on, keeping the last 1 min".into())
    );
    assert_eq!(
        app.execute(Command::ReplaySave(ReplaySpan::Everything)),
        Err("The replay buffer is still empty".into())
    );
    pass(&mut app, Duration::from_secs(45));
    assert_eq!(app.execute(last_thirty), Ok("Saving the last 30 s".into()));
    pass(&mut app, Duration::from_secs(1));
    let confirmation = app.confirmation.clone().expect("a confirmation");
    assert_eq!(confirmation.text, "Replay saved · 30 s");
    assert_eq!(
        app.execute(Command::Status),
        Ok("Instant replay is on, holding 46 s of 1 min".into())
    );
    assert_eq!(
        app.execute(Command::ReplayOff),
        Ok("Instant replay is off".into())
    );
}

#[test]
fn the_confirmation_and_the_notice_go_away_by_themselves() {
    let mut app = demo::idle();
    app.perform(Action::ToggleReplay);
    pass(&mut app, Duration::from_secs(10));
    app.perform(Action::SaveEverything);
    pass(&mut app, Duration::from_secs(1));
    assert!(app.confirmation.is_some());
    assert!(app.notice.is_some());
    app.sync(2.0);
    assert!(app.confirmation.is_some());
    assert!(app.sync(10.0));
    assert_eq!(app.confirmation, None);
    assert_eq!(app.notice, None);
}

#[test]
fn a_blocked_start_raises_the_problem_once() {
    let mut app = demo::idle();
    let simulator = app.simulator.clone().expect("a simulated engine");
    let blocked = Problem::PermissionMissing(Permission::ScreenRecording);
    simulator.block_starts_with(Some(blocked.clone()));
    app.perform(Action::ToggleRecording);
    app.perform(Action::ToggleRecording);
    assert_eq!(app.problems, vec![blocked]);
    assert_eq!(app.status.recording, None);

    simulator.block_starts_with(None);
    app.perform(Action::ToggleRecording);
    assert!(app.status.recording.is_some());
    assert_eq!(app.problems, Vec::new());
}

#[test]
fn changed_settings_reach_the_file_after_a_short_wait() {
    let file = temp_file("saved");
    let mut app = demo::with(true, Store::open(&file));
    app.edit(|settings| settings.replay.length_seconds = 300);
    app.sync(0.0);
    assert!(!file.exists());
    app.sync(1.0);
    let written = std::fs::read_to_string(&file).expect("the settings file");
    assert!(
        written.contains("\"replay.length_seconds\": 300"),
        "{written}"
    );

    let reopened = demo::with(true, Store::open(&file));
    assert_eq!(reopened.settings.replay.length_seconds, 300);
    assert!(!reopened.settings_were_reset);
    std::fs::remove_file(&file).expect("temp file removed");
}

#[test]
fn replay_starts_with_the_app_when_the_setting_says_so() {
    let file = temp_file("replay-at-start");
    std::fs::write(&file, br#"{ "replay.start_with_app": true }"#).expect("a settings file");
    let app = demo::with(true, Store::open(&file));
    assert!(app.status.replay.is_some());
    std::fs::remove_file(&file).expect("temp file removed");
}

#[test]
fn a_new_replay_length_applies_to_the_running_buffer() {
    let mut app = demo::idle();
    app.perform(Action::ToggleReplay);
    pass(&mut app, Duration::from_secs(5));
    app.edit(|settings| settings.replay.length_seconds = 300);
    pass(&mut app, Duration::from_secs(1));
    let replay = app.status.replay.clone().expect("replay is on");
    assert_eq!(replay.length, Duration::from_secs(300));
    assert_eq!(replay.held, Duration::from_secs(6));
}

#[test]
fn a_codec_the_engine_lacks_falls_back_to_one_it_has() {
    let file = temp_file("codec");
    std::fs::write(&file, br#"{ "video.codec": "av1" }"#).expect("a settings file");
    let app = demo::with(true, Store::open(&file));
    assert_eq!(app.settings.video.codec, crate::engine::Codec::H264);
    std::fs::remove_file(&file).expect("temp file removed");
}
