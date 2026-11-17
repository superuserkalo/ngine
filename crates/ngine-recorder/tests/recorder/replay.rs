use std::sync::Arc;
use std::time::Duration;

use ngine_capture::fake::FakeCapture;
use ngine_capture::{
    CaptureError, CpuFrame, EndReason, Event, Permission, Screen, ScreenId, Target,
};
use ngine_recorder::replay::{BackendChoice, ReplayLength, SaveRange};
use ngine_recorder::{
    AudioSettings, CaptureSetup, Recorder, RecorderError, RecorderEvent, RecordingStart,
    ReplaySaveRequest, ReplaySetup, Tracks,
};

use crate::common::{
    FRAME_RATE, File, Rig, SAMPLE_RATE, SIZE, SYSTEM_TONE, Sound, TempFolder, config, frames, gray,
    mean_luma, merged, sound, tone,
};

const BEEP_SECONDS: f64 = 0.2;

fn replay(backend: BackendChoice, folder: &TempFolder) -> ReplaySetup {
    ReplaySetup {
        length: ReplayLength::new(Duration::from_mins(1)).unwrap(),
        backend,
        disk_folder: Some(folder.file("buffer")),
        clear_after_save: false,
    }
}

fn level_of_second(seconds: f64) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "test times are small positive numbers of seconds"
    )]
    let second = (seconds.floor() as u32 % 36) as u8;
    30 + 6 * second
}

fn luma_of(level: u8) -> f64 {
    16.0 + f64::from(level) * 219.0 / 255.0
}

fn a_brighter_picture_every_second() -> impl Fn(f64) -> Arc<CpuFrame> + Send + 'static {
    let pictures: Vec<Arc<CpuFrame>> = (0_u8..36)
        .map(|second| gray(SIZE, level_of_second(f64::from(second))))
        .collect();
    move |seconds| Arc::clone(&pictures[usize::from((level_of_second(seconds) - 30) / 6)])
}

fn quiet_tone_with_a_beep_at(beep: f64) -> impl Fn(f64) -> f64 + Send + 'static {
    move |time| {
        let gain = if (beep..beep + BEEP_SECONDS).contains(&time) {
            2.0
        } else {
            0.25
        };
        gain * tone(SYSTEM_TONE)(time)
    }
}

fn picture_and_sound(from: f64, until: f64, beep: f64) -> Vec<Event> {
    merged(vec![
        Box::new(frames(
            FRAME_RATE,
            from,
            until,
            a_brighter_picture_every_second(),
        )),
        Box::new(sound(
            Sound::system(),
            from,
            until,
            quiet_tone_with_a_beep_at(beep),
        )),
    ])
}

fn save(folder: &TempFolder, name: &str, range: SaveRange) -> ReplaySaveRequest {
    ReplaySaveRequest {
        range,
        folder: folder.path().to_owned(),
        name: name.to_owned(),
    }
}

fn assert_picture_and_beep_agree(file: &File, second: u8, expected_at: f64) {
    let brighter = luma_of(level_of_second(f64::from(second))) - 2.0;
    let picture = file.bright_onset(0.0, brighter).unwrap();
    let beep = file.sound_onset(0, 0.5, 0.3).unwrap();
    assert!(
        (picture - expected_at).abs() < 0.005,
        "second {second} begins at {picture}, not {expected_at}"
    );
    assert!(
        (beep - picture).abs() < 0.010,
        "the beep at {beep} and the picture at {picture} are apart"
    );
}

fn the_last_ten_seconds_save_as_a_clip_of_ten_to_eleven_seconds(backend: BackendChoice) {
    let folder = TempFolder::new(&format!("save-{backend:?}"));
    let mut rig = Rig::new(
        &config(FRAME_RATE, Tracks::Three),
        picture_and_sound(0.0, 27.0, 22.0),
    );
    rig.pipeline
        .set_replay(Some(replay(backend, &folder)))
        .unwrap();
    assert!(matches!(
        rig.pipeline
            .save_replay(save(&folder, "early", SaveRange::Everything)),
        Err(RecorderError::NothingHeld)
    ));
    rig.run_until(25.4);
    let status = rig.pipeline.status().replay.unwrap();
    assert_eq!(status.on_disk, backend == BackendChoice::Disk);
    assert!(status.held.abs_diff(Duration::from_secs_f64(25.4)) < Duration::from_millis(300));

    rig.pipeline
        .save_replay(save(
            &folder,
            "clip",
            SaveRange::Last(Duration::from_secs(10)),
        ))
        .unwrap();
    let (path, length) = rig.wait_for_saved_clip();
    assert_eq!(path, folder.file("clip.mp4"));
    assert!(
        (Duration::from_secs(10)..=Duration::from_secs(11)).contains(&length),
        "the clip is {length:?} long"
    );

    let file = File::decode(&path);
    assert_eq!(file.streams.len(), 4);
    assert_eq!(file.video_micros()[0], 0);
    let frame_count = file.video.len();
    assert!(
        (300..=330).contains(&frame_count),
        "the clip holds {frame_count} frames"
    );
    let first_second = (mean_luma(&file.video[0]) - 16.0) * 255.0 / 219.0;
    assert!(
        (first_second - f64::from(level_of_second(15.0))).abs() < 3.0,
        "the clip starts on a picture of level {first_second}"
    );
    assert_picture_and_beep_agree(&file, 22, 7.0);
    let heard = file.left_channel(0).len();
    assert!(
        heard.abs_diff(frame_count * SAMPLE_RATE / FRAME_RATE as usize) <= 12_000,
        "{heard} samples for {frame_count} frames"
    );

    rig.pipeline
        .save_replay(save(&folder, "clip", SaveRange::Everything))
        .unwrap();
    let (everything, length) = rig.wait_for_saved_clip();
    assert_eq!(everything, folder.file("clip (2).mp4"));
    assert!(length.abs_diff(Duration::from_secs_f64(25.4)) < Duration::from_millis(300));
    rig.run_to_the_end();
    assert!(!folder.file(".clip.mp4.part").exists());
}

#[test]
fn a_save_from_memory_is_as_long_as_asked_to_a_second_more_and_in_sync() {
    the_last_ten_seconds_save_as_a_clip_of_ten_to_eleven_seconds(BackendChoice::Memory);
}

#[test]
fn a_save_from_disk_is_as_long_as_asked_to_a_second_more_and_in_sync() {
    the_last_ten_seconds_save_as_a_clip_of_ten_to_eleven_seconds(BackendChoice::Disk);
}

#[test]
fn a_recording_that_includes_the_buffer_starts_with_the_buffered_frames() {
    let folder = TempFolder::new("include-buffer");
    let mut rig = Rig::new(
        &config(FRAME_RATE, Tracks::Three),
        picture_and_sound(0.0, 13.0, 11.0),
    );
    rig.pipeline
        .set_replay(Some(replay(BackendChoice::Memory, &folder)))
        .unwrap();
    rig.run_until(10.3);
    let path = folder.file("recording.mp4");
    rig.pipeline
        .start_recording(&RecordingStart {
            file: path.clone(),
            include_buffered: Some(Duration::from_secs(5)),
        })
        .unwrap();
    rig.run_to_the_end();
    let (_, length) = rig.wait_for_finished_file();

    assert!(length.abs_diff(Duration::from_secs(8)) < Duration::from_millis(50));
    let file = File::decode(&path);
    assert_eq!(file.video.len(), 240);
    assert_eq!(file.video_micros()[0], 0);
    let first_level = (mean_luma(&file.video[0]) - 16.0) * 255.0 / 219.0;
    assert!(
        (first_level - f64::from(level_of_second(5.0))).abs() < 3.0,
        "the file starts on a picture of level {first_level}"
    );
    assert_picture_and_beep_agree(&file, 11, 6.0);
    for track in 0..3 {
        assert!(file.audio_start_micros(track).abs() < 1_000);
        let heard = file.left_channel(track).len();
        assert!(heard.abs_diff(8 * SAMPLE_RATE) <= 4_096, "{heard} samples");
    }
}

#[test]
fn a_recording_started_without_the_buffer_starts_at_that_moment() {
    let folder = TempFolder::new("exclude-buffer");
    let mut rig = Rig::new(
        &config(FRAME_RATE, Tracks::Three),
        picture_and_sound(0.0, 13.0, 11.0),
    );
    rig.pipeline
        .set_replay(Some(replay(BackendChoice::Memory, &folder)))
        .unwrap();
    rig.run_until(10.3);
    let path = folder.file("recording.mp4");
    rig.pipeline
        .start_recording(&RecordingStart {
            file: path.clone(),
            include_buffered: None,
        })
        .unwrap();
    rig.run_to_the_end();
    let (_, length) = rig.wait_for_finished_file();

    assert!(length.abs_diff(Duration::from_secs_f64(2.7)) < Duration::from_millis(50));
    let file = File::decode(&path);
    assert_eq!(file.video.len(), 81);
    assert_picture_and_beep_agree(&file, 11, 0.7);
}

#[test]
fn replay_keeps_what_it_had_when_capture_is_replaced_by_a_new_session() {
    let folder = TempFolder::new("restarted");
    let mut script = picture_and_sound(0.0, 3.0, 1.0);
    script.extend(picture_and_sound(20.0, 22.0, 21.0));
    let mut rig = Rig::new(&config(FRAME_RATE, Tracks::MixOnly), script);
    rig.pipeline
        .set_replay(Some(replay(BackendChoice::Memory, &folder)))
        .unwrap();
    rig.run_until(3.0);
    rig.pipeline.capture_restarted();
    rig.run_until(22.0);
    rig.pipeline
        .save_replay(save(&folder, "clip", SaveRange::Everything))
        .unwrap();
    let (path, length) = rig.wait_for_saved_clip();

    assert!(
        length.abs_diff(Duration::from_secs(5)) < Duration::from_millis(100),
        "{length:?}"
    );
    let frame_count = File::decode(&path).video.len();
    assert!((150..=152).contains(&frame_count), "{frame_count} frames");
    rig.run_to_the_end();
}

fn screen() -> Target {
    Target::from(Screen::new(ScreenId(1), "Fake screen", SIZE))
}

fn setup() -> CaptureSetup {
    CaptureSetup {
        target: screen(),
        show_cursor: true,
        app_sound_only: false,
        sound_of: None,
        microphone: None,
    }
}

fn events_until_stopped(recorder: &Recorder) -> Vec<RecorderEvent> {
    let mut events = Vec::new();
    loop {
        let event = recorder
            .wait_event(Duration::from_secs(30))
            .expect("the recorder ends with a stopped event");
        let stopped = matches!(event, RecorderEvent::Stopped);
        events.push(event);
        if stopped {
            return events;
        }
    }
}

#[test]
fn the_recorder_restarts_a_broken_capture_and_keeps_the_recording_in_one_file() {
    let folder = TempFolder::new("recorder-restart");
    let broken = CaptureError::Os("the display went away".into());
    let mut first = picture_and_sound(0.0, 2.0, 1.0);
    first.push(Event::Ended(EndReason::Failed(broken)));
    let (recording_began, began) = std::sync::mpsc::channel::<()>();
    let after_the_recording_begins = std::iter::once_with(move || began.recv())
        .filter_map(|_| None)
        .chain(first);
    let capturer = FakeCapture::new()
        .with_script(after_the_recording_begins)
        .with_script(picture_and_sound(50.0, 52.0, 51.0))
        .into_capturer();
    let recorder = Recorder::start(
        Arc::new(capturer),
        setup(),
        &config(FRAME_RATE, Tracks::MixOnly),
    )
    .unwrap();
    let path = folder.file("recording.mp4");
    recorder
        .start_recording(RecordingStart {
            file: path.clone(),
            include_buffered: None,
        })
        .unwrap();
    recording_began.send(()).unwrap();

    let events = events_until_stopped(&recorder);
    let names: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            RecorderEvent::RecordingStarted { .. } => Some("started"),
            RecorderEvent::CaptureInterrupted(_) => Some("interrupted"),
            RecorderEvent::CaptureResumed => Some("resumed"),
            RecorderEvent::RecordingFinished { .. } => Some("finished"),
            RecorderEvent::Stopped => Some("stopped"),
            _ => None,
        })
        .collect();
    assert_eq!(
        names,
        ["started", "interrupted", "resumed", "finished", "stopped"]
    );
    let file = File::decode(&path);
    let duration = file.duration_micros.unwrap();
    assert!(
        (120..=122).contains(&file.video.len()),
        "{} frames over {duration} µs",
        file.video.len()
    );
    assert!((duration - 4_000_000).abs() < 150_000, "{duration} µs");
}

#[test]
fn a_capture_that_cannot_start_is_reported_and_the_recorder_stops() {
    let denied = CaptureError::PermissionDenied(Permission::ScreenRecording);
    let capturer = FakeCapture::new()
        .failing_to_start(denied.clone())
        .into_capturer();
    let recorder = Recorder::start(
        Arc::new(capturer),
        setup(),
        &config(FRAME_RATE, Tracks::MixOnly),
    )
    .unwrap();

    let events = events_until_stopped(&recorder);
    assert!(matches!(
        events.as_slice(),
        [RecorderEvent::CaptureLost(Some(error)), RecorderEvent::Stopped] if *error == denied
    ));
}

#[test]
fn a_save_right_after_a_source_went_quiet_has_sound_to_the_end_of_its_picture() {
    let folder = TempFolder::new("quiet-source");
    let picture = gray(SIZE, 120);
    let script = merged(vec![
        Box::new(frames(FRAME_RATE, 0.0, 6.0, move |_| Arc::clone(&picture))),
        Box::new(sound(Sound::system(), 0.0, 6.0, tone(SYSTEM_TONE))),
        Box::new(sound(Sound::microphone(), 0.0, 5.7, tone(SYSTEM_TONE))),
    ]);
    let mut rig = Rig::new(&config(FRAME_RATE, Tracks::Three), script);
    rig.pipeline
        .set_replay(Some(replay(BackendChoice::Memory, &folder)))
        .unwrap();
    rig.run_until(6.0);
    rig.pipeline
        .save_replay(save(&folder, "clip", SaveRange::Everything))
        .unwrap();
    let (path, _) = rig.wait_for_saved_clip();

    let file = File::decode(&path);
    let picture_ends = file.video_micros().last().copied().unwrap() + 33_333;
    let frames_heard = file.left_channel(0).len();
    let sound_ends =
        file.audio_start_micros(0) + i64::try_from(frames_heard * 1_000_000 / SAMPLE_RATE).unwrap();
    assert!(
        picture_ends - sound_ends < 80_000,
        "the sound ends {} µs before the picture",
        picture_ends - sound_ends
    );
    rig.run_to_the_end();
}

#[test]
fn another_target_and_other_volumes_keep_the_recorder_and_its_recording() {
    let folder = TempFolder::new("change-capture");
    let fake = FakeCapture::new()
        .with_script(picture_and_sound(0.0, 2.0, 1.0))
        .with_script(picture_and_sound(50.0, 52.0, 51.0));
    let log = fake.start_log();
    let settings = config(FRAME_RATE, Tracks::MixOnly);
    let recorder = Recorder::start(Arc::new(fake.into_capturer()), setup(), &settings).unwrap();
    let path = folder.file("recording.mp4");
    recorder
        .start_recording(RecordingStart {
            file: path.clone(),
            include_buffered: None,
        })
        .unwrap();
    let second_screen = Target::from(Screen::new(ScreenId(2), "Second screen", SIZE));
    let without_microphone = AudioSettings {
        system_gain: Some(0.5),
        microphone_gain: None,
        ..settings.audio
    };
    recorder
        .change_capture(
            CaptureSetup {
                target: second_screen,
                ..setup()
            },
            without_microphone,
        )
        .unwrap();

    let events = events_until_stopped(&recorder);

    let starts = log.starts();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_eq!(starts[1].target.label(), "Second screen");
    assert_eq!(starts[1].options.microphone, ngine_capture::Microphone::Off);
    assert_ne!(starts[0].options.microphone, ngine_capture::Microphone::Off);
    let finished: Vec<bool> = events
        .iter()
        .filter_map(|event| match event {
            RecorderEvent::RecordingFinished { continues, .. } => Some(*continues),
            _ => None,
        })
        .collect();
    assert_eq!(finished, [false]);
    assert!(!events.iter().any(|event| matches!(
        event,
        RecorderEvent::CaptureInterrupted(_) | RecorderEvent::CaptureLost(_)
    )));
    assert_ne!(File::decode(&path).video.len(), 0);
}
