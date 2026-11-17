use std::sync::mpsc;
use std::time::{Duration, Instant};

use ngine_capture::fake::{self, AudioShape, FakeCapture};
use ngine_capture::{
    App, AudioApp, AudioSource, CaptureError, EndReason, Event, MicrophoneDevice, Permission,
    PickedTarget, PickerOptions, RecvError, Region, RestoreToken, Screen, ScreenId, Session,
    SessionOptions, Size, SystemAudio, Target, TargetKind, Targets, Timestamp,
};

const FRAME: Size = Size::new(64, 36);

fn screen() -> Screen {
    Screen::new(ScreenId(1), "Fake screen", FRAME)
}

fn start(fake: FakeCapture) -> Session {
    fake.into_capturer()
        .start(&Target::from(screen()), &SessionOptions::new())
        .expect("a fake without a start error starts")
}

fn drain(session: &Session) -> Vec<Event> {
    std::iter::from_fn(|| session.recv()).collect()
}

fn system_sound() -> AudioShape {
    AudioShape {
        source: AudioSource::System,
        sample_rate: 48_000,
        channels: 2,
        frames_per_buffer: 480,
    }
}

#[test]
fn a_script_arrives_in_order_and_then_the_session_ends() {
    let frames = fake::solid_frames(fake::black(FRAME).unwrap(), 50, 3, Timestamp::from_nanos(0));
    let session = start(FakeCapture::new().with_script(frames));

    let events = drain(&session);

    let times: Vec<Option<u64>> = events
        .iter()
        .map(|event| event.timestamp().map(Timestamp::as_nanos))
        .collect();
    assert_eq!(times, [Some(0), Some(20_000_000), Some(40_000_000), None]);
    assert!(matches!(
        events.last(),
        Some(Event::Ended(EndReason::Stopped))
    ));
    assert!(session.recv().is_none());
}

#[test]
fn a_frame_gives_its_pixels_on_request() {
    let frames = fake::solid_frames(fake::black(FRAME).unwrap(), 60, 1, Timestamp::from_nanos(5));
    let session = start(FakeCapture::new().with_script(frames));

    let Some(Event::Video(frame)) = session.recv() else {
        panic!("the script starts with a video frame");
    };
    let pixels = frame.to_cpu().unwrap();

    assert_eq!(frame.size(), FRAME);
    assert_eq!(pixels.size(), FRAME);
    assert_eq!(pixels.data().len(), 64 * 36 * 4);
}

#[test]
fn audio_buffers_name_their_source_and_follow_one_another_on_the_clock() {
    let microphone = AudioShape {
        source: AudioSource::Microphone,
        sample_rate: 44_100,
        channels: 1,
        frames_per_buffer: 441,
    };
    let first = Timestamp::from_nanos(1_000);
    let script = fake::in_time_order(
        fake::audio_buffers(system_sound(), 3, first, |_frame| 0.25),
        fake::audio_buffers(microphone, 3, first, |_frame| -0.5),
    );
    let session = start(FakeCapture::new().with_script(script));

    let buffers: Vec<_> = drain(&session)
        .into_iter()
        .filter_map(|event| match event {
            Event::Audio(buffer) => Some(buffer),
            _ => None,
        })
        .collect();

    let described: Vec<_> = buffers
        .iter()
        .map(|buffer| (buffer.source(), buffer.timestamp().as_nanos()))
        .collect();
    assert_eq!(
        described,
        [
            (AudioSource::System, 1_000),
            (AudioSource::Microphone, 1_000),
            (AudioSource::System, 10_001_000),
            (AudioSource::Microphone, 10_001_000),
            (AudioSource::System, 20_001_000),
            (AudioSource::Microphone, 20_001_000),
        ]
    );
    assert_eq!(buffers[0].samples(), [0.25; 960]);
    assert_eq!(buffers[1].samples(), [-0.5; 441]);
    assert_eq!(buffers[1].sample_rate(), 44_100);
    assert_eq!(buffers[1].channels(), 1);
}

#[test]
fn a_break_and_a_restart_are_reported_between_the_frames_around_them() {
    let pixels = fake::black(FRAME).unwrap();
    let larger = fake::black(Size::new(128, 72)).unwrap();
    let script = fake::solid_frames(pixels, 60, 2, Timestamp::from_nanos(0))
        .chain([
            Event::Interrupted(CaptureError::Os("the display changed".to_owned())),
            Event::Resumed,
        ])
        .chain(fake::solid_frames(
            larger,
            60,
            2,
            Timestamp::from_nanos(1_000_000_000),
        ));
    let session = start(FakeCapture::new().with_script(script));

    let shape: Vec<&str> = drain(&session)
        .iter()
        .map(|event| match event {
            Event::Video(frame) if frame.size() == FRAME => "small frame",
            Event::Video(_) => "large frame",
            Event::Interrupted(_) => "interrupted",
            Event::Resumed => "resumed",
            Event::Ended(_) => "ended",
            _ => "other",
        })
        .collect();

    assert_eq!(
        shape,
        [
            "small frame",
            "small frame",
            "interrupted",
            "resumed",
            "large frame",
            "large frame",
            "ended"
        ]
    );
}

#[test]
fn a_script_can_end_the_session_with_a_failure() {
    let script = [
        Event::Interrupted(CaptureError::TargetGone),
        Event::Ended(EndReason::Failed(CaptureError::TargetGone)),
        Event::Resumed,
    ];
    let session = start(FakeCapture::new().with_script(script));

    let events = drain(&session);

    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[1],
        Event::Ended(EndReason::Failed(CaptureError::TargetGone))
    ));
}

#[test]
fn a_slow_reader_loses_nothing_from_a_fake() {
    let simulated_minutes = 5;
    let frames = 60 * 60 * simulated_minutes;
    let script = fake::in_time_order(
        fake::solid_frames(
            fake::black(FRAME).unwrap(),
            60,
            frames,
            Timestamp::from_nanos(0),
        ),
        fake::audio_buffers(
            system_sound(),
            100 * 60 * simulated_minutes,
            Timestamp::from_nanos(0),
            |_frame| 0.0,
        ),
    );
    let session = start(FakeCapture::new().with_script(script));
    std::thread::sleep(Duration::from_millis(50));

    let events = drain(&session);

    let videos = events
        .iter()
        .filter(|event| matches!(event, Event::Video(_)))
        .count();
    let in_order = events.iter().filter_map(Event::timestamp).is_sorted();
    assert_eq!(u64::try_from(videos).unwrap(), frames);
    assert_eq!(events.len(), 18_000 + 30_000 + 1);
    assert!(in_order);
    assert_eq!(session.dropped_video_frames(), 0);
    assert_eq!(session.dropped_audio_buffers(), 0);
}

#[test]
fn stopping_ends_a_script_that_would_never_end() {
    let endless = fake::solid_frames(
        fake::black(FRAME).unwrap(),
        60,
        u64::MAX,
        Timestamp::from_nanos(0),
    );
    let session = start(FakeCapture::new().with_script(endless));

    assert!(matches!(session.recv(), Some(Event::Video(_))));
    session.stop();
    let rest = drain(&session);

    assert!(matches!(
        rest.last(),
        Some(Event::Ended(EndReason::Stopped))
    ));
    assert!(session.recv().is_none());
}

#[test]
fn a_stopper_on_another_thread_wakes_a_reader_that_is_waiting() {
    let (hold_open, quiet) = mpsc::channel::<Event>();
    let session = start(FakeCapture::new().with_script(quiet));
    let stopper = session.stopper();

    let stopping = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        stopper.stop();
    });
    let woke_with = session.recv();
    stopping.join().unwrap();
    drop(hold_open);

    assert!(matches!(woke_with, Some(Event::Ended(EndReason::Stopped))));
}

#[test]
fn a_quiet_session_times_out_instead_of_blocking() {
    let (hold_open, quiet) = mpsc::channel::<Event>();
    let session = start(FakeCapture::new().with_script(quiet));
    let asked = Instant::now();

    let outcome = session.recv_timeout(Duration::from_millis(20));

    assert_eq!(outcome.unwrap_err(), RecvError::TimedOut);
    assert!(asked.elapsed() >= Duration::from_millis(20));
    drop(hold_open);
}

#[test]
fn a_missing_permission_is_an_error_value_from_start() {
    for permission in [Permission::ScreenRecording, Permission::Microphone] {
        let capturer = FakeCapture::new()
            .failing_to_start(CaptureError::PermissionDenied(permission))
            .into_capturer();

        let outcome = capturer.start(&Target::from(screen()), &SessionOptions::new());

        assert_eq!(
            outcome.unwrap_err(),
            CaptureError::PermissionDenied(permission)
        );
    }
}

#[test]
fn a_fake_offers_only_what_it_was_given() {
    let bare = FakeCapture::new().into_capturer();
    let targets = Targets::new(vec![screen()], Vec::new(), vec![App::new(7, "Player")]);
    let full = FakeCapture::new()
        .with_targets(targets.clone())
        .with_picker(Ok(None))
        .with_audio_apps(vec![AudioApp::new(31, "Player")])
        .with_microphones(vec![MicrophoneDevice::new("usb-1", "Desk mic", true)])
        .into_capturer();

    assert!(bare.lister().is_none());
    assert!(bare.picker().is_none());
    assert!(bare.audio_apps().is_none());
    assert_eq!(bare.microphones().unwrap(), []);
    assert_eq!(full.lister().unwrap().targets().unwrap(), targets);
    assert_eq!(
        full.audio_apps().unwrap().playing().unwrap(),
        [AudioApp::new(31, "Player")]
    );
    assert_eq!(
        full.microphones().unwrap(),
        [MicrophoneDevice::new("usb-1", "Desk mic", true)]
    );
}

#[test]
fn a_picker_hands_over_the_target_and_its_restore_token() {
    let picked = PickedTarget::new(
        Target::from(screen()),
        Some(RestoreToken::new("token-from-the-portal")),
    );
    let capturer = FakeCapture::new()
        .with_picker(Ok(Some(picked)))
        .into_capturer();
    let (answered, answer) = mpsc::channel();

    capturer
        .picker()
        .unwrap()
        .pick(PickerOptions::new(), move |outcome| {
            answered.send(outcome).unwrap();
        });
    let picked = answer.recv().unwrap().unwrap().unwrap();

    assert_eq!(picked.target.kind(), TargetKind::Screen);
    assert_eq!(picked.target.label(), "Fake screen");
    assert_eq!(
        picked.restore_token.unwrap().as_str(),
        "token-from-the-portal"
    );
}

#[test]
fn a_cancelled_picker_answers_with_no_target() {
    let capturer = FakeCapture::new().with_picker(Ok(None)).into_capturer();
    let (answered, answer) = mpsc::channel();

    capturer
        .picker()
        .unwrap()
        .pick(PickerOptions::new(), move |outcome| {
            answered.send(outcome).unwrap();
        });

    assert!(answer.recv().unwrap().unwrap().is_none());
}

#[test]
fn the_start_log_shows_what_the_code_under_test_asked_for() {
    let fake = FakeCapture::new();
    let log = fake.start_log();
    let capturer = fake.into_capturer();
    let app = Target::from(App::new(7, "Player"));
    let options = SessionOptions::new()
        .with_cursor(false)
        .with_system_audio(SystemAudio::of(&app));

    let session = capturer.start(&app, &options).unwrap();
    drop(session);

    let starts = log.starts();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].target.kind(), TargetKind::App);
    assert_eq!(starts[0].options, options);
    assert_eq!(
        starts[0].options.system_audio,
        SystemAudio::App(AudioApp::new(7, "Player"))
    );
}

#[test]
fn each_start_plays_the_next_script() {
    let first = fake::solid_frames(fake::black(FRAME).unwrap(), 60, 1, Timestamp::from_nanos(0));
    let second = fake::solid_frames(fake::black(FRAME).unwrap(), 60, 2, Timestamp::from_nanos(0));
    let capturer = FakeCapture::new()
        .with_script(first)
        .with_script(second)
        .into_capturer();
    let target = Target::from(screen());

    let lengths: Vec<usize> = (0..3)
        .map(|_| {
            let session = capturer.start(&target, &SessionOptions::new()).unwrap();
            drain(&session).len()
        })
        .collect();

    assert_eq!(lengths, [2, 3, 1]);
}

#[test]
fn a_region_of_a_screen_arrives_cropped_from_frames_that_can_be_copied() {
    let frames = fake::solid_frames(fake::black(FRAME).unwrap(), 30, 2, Timestamp::from_nanos(0));
    let fake = FakeCapture::new().with_script(frames);
    let log = fake.start_log();
    let target = Target::from(screen()).with_region(Region::new(8, 4, 32, 18));

    let session = fake
        .into_capturer()
        .start(&target, &SessionOptions::new().with_gpu_surfaces(true))
        .unwrap();
    let sizes: Vec<Size> = drain(&session)
        .iter()
        .filter_map(|event| match event {
            Event::Video(frame) => Some(frame.to_cpu().unwrap().size()),
            _ => None,
        })
        .collect();

    assert_eq!(sizes, [Size::new(32, 18), Size::new(32, 18)]);
    assert!(!log.starts()[0].options.gpu_surfaces);
}
