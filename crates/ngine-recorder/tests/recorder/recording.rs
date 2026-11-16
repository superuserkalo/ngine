use std::sync::Arc;
use std::time::Duration;

use ngine_capture::{AudioSource, CaptureError, CpuFrame, Event, Size};
use ngine_media::{AudioCodec, Codec, Demuxer, VideoCodec};
use ngine_recorder::{RecorderEvent, RecordingStart, Tracks, journal_path, read_markers};

use crate::common::{
    FRAME_RATE, File, LEVEL, MICROPHONE_TONE, Rig, SAMPLE_RATE, SIZE, SYSTEM_TONE, Sound,
    TempFolder, config, float, frames, gray, mean_luma, merged, sound, tone, tone_level,
};

const FLASH_AT: f64 = 2.0;
const FLASH_SECONDS: f64 = 0.2;

fn black_with_a_flash(size: Size) -> impl Fn(f64) -> Arc<CpuFrame> + Send + 'static {
    let (black, white) = (gray(size, 16), gray(size, 235));
    move |seconds| {
        if (FLASH_AT..FLASH_AT + FLASH_SECONDS).contains(&seconds) {
            Arc::clone(&white)
        } else {
            Arc::clone(&black)
        }
    }
}

fn tone_with_a_louder_beep(frequency: f64) -> impl Fn(f64) -> f64 + Send + 'static {
    move |time| {
        let gain = if (FLASH_AT..FLASH_AT + FLASH_SECONDS).contains(&time) {
            2.0
        } else {
            0.25
        };
        gain * tone(frequency)(time)
    }
}

fn picture_and_two_sources(seconds: f64) -> Vec<Event> {
    merged(vec![
        Box::new(frames(FRAME_RATE, 0.0, seconds, black_with_a_flash(SIZE))),
        Box::new(sound(
            Sound::system(),
            0.0,
            seconds,
            tone_with_a_louder_beep(SYSTEM_TONE),
        )),
        Box::new(sound(
            Sound::microphone(),
            0.0,
            seconds,
            tone(MICROPHONE_TONE),
        )),
    ])
}

fn start(file: std::path::PathBuf) -> RecordingStart {
    RecordingStart {
        file,
        include_buffered: None,
    }
}

#[test]
fn a_recording_holds_the_mix_then_system_sound_then_the_microphone_in_sync_with_the_picture() {
    let folder = TempFolder::new("three-tracks");
    let mut rig = Rig::new(
        &config(FRAME_RATE, Tracks::Three),
        picture_and_two_sources(4.0),
    );
    rig.pipeline
        .start_recording(&start(folder.file("recording.mp4")))
        .unwrap();
    rig.run_to_the_end();
    let (path, length) = rig.wait_for_finished_file();

    let file = File::decode(&path);
    let codecs: Vec<_> = file
        .streams
        .iter()
        .map(|stream| stream.parameters.codec())
        .collect();
    assert_eq!(
        codecs,
        [
            Some(Codec::Video(VideoCodec::H264)),
            Some(Codec::Audio(AudioCodec::Aac)),
            Some(Codec::Audio(AudioCodec::Aac)),
            Some(Codec::Audio(AudioCodec::Aac)),
        ]
    );
    assert_eq!(file.video.len(), 120);
    assert_eq!(file.video_micros()[0], 0);
    assert!(length.abs_diff(Duration::from_secs(4)) < Duration::from_millis(50));

    let quiet = 0.25 * LEVEL;
    let expected = [(quiet, LEVEL), (quiet, 0.0), (0.0, LEVEL)];
    for (track, (system, microphone)) in expected.into_iter().enumerate() {
        assert!(file.audio_start_micros(track).abs() < 1_000);
        let samples = file.left_channel(track);
        assert!(
            samples.len().abs_diff(4 * SAMPLE_RATE) <= 4_096,
            "track {track} holds {} samples",
            samples.len()
        );
        let steady = &samples[SAMPLE_RATE / 2..SAMPLE_RATE * 3 / 2];
        let found = (
            tone_level(steady, SYSTEM_TONE),
            tone_level(steady, MICROPHONE_TONE),
        );
        assert!(
            (found.0 - system).abs() < 0.03 && (found.1 - microphone).abs() < 0.05,
            "track {track} holds the tones at {found:?}, not ({system}, {microphone})"
        );
    }

    let flash = file.bright_onset(0.0, 128.0).unwrap();
    let beep = file.sound_onset(1, 1.0, 0.3).unwrap();
    assert!((flash - FLASH_AT).abs() < 0.005, "the flash is at {flash}");
    assert!(
        (beep - flash).abs() < 0.010,
        "the beep at {beep} and the flash at {flash} are apart"
    );
}

#[test]
fn the_mix_only_setting_writes_one_audio_track() {
    let folder = TempFolder::new("mix-only");
    let mut rig = Rig::new(
        &config(FRAME_RATE, Tracks::MixOnly),
        picture_and_two_sources(2.0),
    );
    rig.pipeline
        .start_recording(&start(folder.file("recording.mp4")))
        .unwrap();
    rig.run_to_the_end();
    let (path, _) = rig.wait_for_finished_file();

    let file = File::decode(&path);
    assert_eq!(file.streams.len(), 2);
    let steady = &file.left_channel(0)[SAMPLE_RATE / 2..SAMPLE_RATE];
    assert!(tone_level(steady, SYSTEM_TONE) > 0.05);
    assert!(tone_level(steady, MICROPHONE_TONE) > 0.3);
}

#[test]
fn a_recording_whose_process_dies_plays_up_to_its_last_keyframe() {
    let folder = TempFolder::new("killed");
    let mut rig = Rig::new(
        &config(FRAME_RATE, Tracks::Three),
        picture_and_two_sources(7.0),
    );
    let path = folder.file("recording.mp4");
    rig.pipeline.start_recording(&start(path.clone())).unwrap();
    rig.run_until(6.5);
    rig.pipeline.wait_until_written();
    let killed = folder.file("as-the-kill-left-it.mp4");
    std::fs::copy(&path, &killed).unwrap();

    let file = File::decode(&killed);
    assert_eq!(file.streams.len(), 4);
    assert_eq!(file.video_micros()[0], 0);
    let seconds = file.video.len() / FRAME_RATE as usize;
    assert!(
        (4..=6).contains(&seconds),
        "{} frames survived the kill",
        file.video.len()
    );
    for track in 0..3 {
        let samples = file.left_channel(track).len();
        assert!(
            samples + 4_096 >= seconds * SAMPLE_RATE,
            "track {track} kept {samples} samples for {seconds} s of picture"
        );
    }
    let flash = file.bright_onset(0.0, 128.0).unwrap();
    let beep = file.sound_onset(1, 1.0, 0.3).unwrap();
    assert!((beep - flash).abs() < 0.010);

    rig.run_to_the_end();
    rig.wait_for_finished_file();
    assert_eq!(File::decode(&path).video.len(), 210);
}

#[test]
fn markers_dropped_during_a_recording_are_read_back_from_the_file() {
    let folder = TempFolder::new("markers");
    let mut rig = Rig::new(
        &config(FRAME_RATE, Tracks::MixOnly),
        picture_and_two_sources(5.0),
    );
    let path = folder.file("recording.mp4");
    rig.pipeline.start_recording(&start(path.clone())).unwrap();
    rig.run_until(1.5);
    let first = rig.pipeline.add_marker().unwrap();
    rig.run_until(4.0);
    let second = rig.pipeline.add_marker().unwrap();
    assert_eq!(rig.pipeline.status().recording.unwrap().markers, 2);
    rig.pipeline.wait_until_written();

    let close_to = |marker: Duration, seconds: f64| {
        marker.abs_diff(Duration::from_secs_f64(seconds)) < Duration::from_millis(40)
    };
    assert!(close_to(first, 1.5), "{first:?}");
    assert!(close_to(second, 4.0), "{second:?}");
    assert_eq!(read_markers(&path).unwrap(), [first, second]);
    assert!(journal_path(&path).exists());

    rig.run_to_the_end();
    rig.wait_for_finished_file();
    assert_eq!(read_markers(&path).unwrap(), [first, second]);
    assert!(!journal_path(&path).exists());
    assert_eq!(File::decode(&path).video.len(), 150);
}

#[test]
fn a_new_picture_size_continues_the_recording_in_a_second_part() {
    let folder = TempFolder::new("resized");
    let small = Size::new(160, 120);
    let script = merged(vec![
        Box::new(frames(FRAME_RATE, 0.0, 2.0, black_with_a_flash(SIZE))),
        Box::new(frames(FRAME_RATE, 2.0, 4.0, {
            let picture = gray(small, 120);
            move |_| Arc::clone(&picture)
        })),
        Box::new(sound(Sound::system(), 0.0, 4.0, tone(SYSTEM_TONE))),
    ]);
    let mut rig = Rig::new(&config(FRAME_RATE, Tracks::Three), script);
    let path = folder.file("recording.mp4");
    rig.pipeline.start_recording(&start(path.clone())).unwrap();
    rig.run_to_the_end();

    let (first, first_length) = rig.wait_for_finished_file();
    let (second, second_length) = rig.wait_for_finished_file();
    assert_eq!(first, path);
    assert_eq!(second, folder.file("recording (part 2).mp4"));
    for (file, length, size) in [
        (&first, first_length, SIZE),
        (&second, second_length, small),
    ] {
        let decoded = File::decode(file);
        assert_eq!(decoded.streams[0].parameters.width(), size.width);
        assert_eq!(decoded.streams[0].parameters.height(), size.height);
        assert_eq!(decoded.video.len(), 60);
        assert!(length.abs_diff(Duration::from_secs(2)) < Duration::from_millis(50));
        let heard = decoded.left_channel(1).len();
        assert!(heard.abs_diff(2 * SAMPLE_RATE) <= 4_096, "{heard} samples");
    }
    let continued: Vec<bool> = rig
        .seen
        .iter()
        .filter_map(|event| match event {
            RecorderEvent::RecordingFinished { continues, .. } => Some(*continues),
            _ => None,
        })
        .collect();
    assert_eq!(continued, [true, false]);
}

#[test]
fn an_interruption_stays_in_the_file_as_a_still_picture_with_silence() {
    let folder = TempFolder::new("interrupted");
    let picture = gray(SIZE, 90);
    let later = Arc::clone(&picture);
    let mut script = merged(vec![
        Box::new(frames(FRAME_RATE, 0.0, 2.0, move |_| Arc::clone(&picture))),
        Box::new(sound(Sound::system(), 0.0, 2.0, tone(SYSTEM_TONE))),
    ]);
    script.push(Event::Interrupted(CaptureError::Os("display slept".into())));
    script.push(Event::Resumed);
    script.extend(merged(vec![
        Box::new(frames(FRAME_RATE, 5.0, 7.0, move |_| Arc::clone(&later))),
        Box::new(sound(Sound::system(), 5.0, 7.0, tone(SYSTEM_TONE))),
    ]));
    let mut rig = Rig::new(&config(FRAME_RATE, Tracks::MixOnly), script);
    let path = folder.file("recording.mp4");
    rig.pipeline.start_recording(&start(path.clone())).unwrap();
    rig.run_to_the_end();
    let (_, length) = rig.wait_for_finished_file();

    assert!(length.abs_diff(Duration::from_secs(7)) < Duration::from_millis(50));
    assert!(rig.seen.iter().any(|event| matches!(
        event,
        RecorderEvent::CaptureInterrupted(CaptureError::Os(_))
    )));
    assert!(
        rig.seen
            .iter()
            .any(|event| matches!(event, RecorderEvent::CaptureResumed))
    );
    let file = File::decode(&path);
    let micros = file.video_micros();
    let longest_step = micros.windows(2).map(|pair| pair[1] - pair[0]).max();
    assert!(longest_step.unwrap() <= 1_000_000, "{longest_step:?}");
    let heard = file.left_channel(0);
    assert!(heard.len().abs_diff(7 * SAMPLE_RATE) <= 4_096);
    let during = &heard[SAMPLE_RATE * 5 / 2..SAMPLE_RATE * 9 / 2];
    assert!(during.iter().all(|sample| sample.abs() < 0.01));
    let after = &heard[SAMPLE_RATE * 11 / 2..SAMPLE_RATE * 13 / 2];
    assert!(tone_level(after, SYSTEM_TONE) > 0.3);
}

#[test]
fn a_clock_that_jumps_ten_hours_ahead_is_cut_out_instead_of_filled() {
    let folder = TempFolder::new("clock-jump");
    let picture = gray(SIZE, 90);
    let later = Arc::clone(&picture);
    let ten_hours = 36_000.0;
    let mut script = merged(vec![
        Box::new(frames(FRAME_RATE, 0.0, 2.0, move |_| Arc::clone(&picture))),
        Box::new(sound(Sound::system(), 0.0, 2.0, tone(SYSTEM_TONE))),
    ]);
    script.extend(merged(vec![
        Box::new(sound(
            Sound::system(),
            ten_hours - 0.5,
            ten_hours + 2.0,
            tone(SYSTEM_TONE),
        )),
        Box::new(frames(FRAME_RATE, ten_hours, ten_hours + 2.0, move |_| {
            Arc::clone(&later)
        })),
    ]));
    let mut rig = Rig::new(&config(FRAME_RATE, Tracks::MixOnly), script);
    let path = folder.file("recording.mp4");
    rig.pipeline.start_recording(&start(path.clone())).unwrap();
    rig.run_to_the_end();
    let (_, length) = rig.wait_for_finished_file();

    assert!(
        length.abs_diff(Duration::from_secs(4)) < Duration::from_millis(150),
        "{length:?}"
    );
    let status = rig.pipeline.status();
    assert!(status.frames_repeated < 10, "{}", status.frames_repeated);
    let file = File::decode(&path);
    assert!(
        (118..=124).contains(&file.video.len()),
        "{}",
        file.video.len()
    );
    let heard = file.left_channel(0);
    assert!(
        heard.len().abs_diff(4 * SAMPLE_RATE) <= 8_192,
        "{}",
        heard.len()
    );
}

#[test]
fn a_still_picture_keeps_a_steady_cadence_of_frames() {
    let folder = TempFolder::new("still");
    let picture = gray(SIZE, 200);
    let script = merged(vec![
        Box::new(frames(FRAME_RATE, 0.0, 0.03, move |_| Arc::clone(&picture))),
        Box::new(sound(Sound::system(), 0.0, 5.0, tone(SYSTEM_TONE))),
    ]);
    let mut rig = Rig::new(&config(FRAME_RATE, Tracks::MixOnly), script);
    let path = folder.file("recording.mp4");
    rig.pipeline.start_recording(&start(path.clone())).unwrap();
    rig.run_to_the_end();
    rig.wait_for_finished_file();

    let status = rig.pipeline.status();
    assert_eq!(status.frames_captured, 1);
    let file = File::decode(&path);
    assert!(
        (145..=150).contains(&file.video.len()),
        "{} frames",
        file.video.len()
    );
    let micros = file.video_micros();
    assert!(
        micros
            .windows(2)
            .all(|pair| (pair[1] - pair[0] - 33_333).abs() <= 12)
    );
    assert!(
        file.video
            .iter()
            .all(|frame| (mean_luma(frame) - 188.0).abs() < 8.0)
    );
}

fn pulse_every_second(time: f64) -> f64 {
    let into_second = time - time.floor();
    if into_second < 0.005 {
        LEVEL
            * (std::f64::consts::TAU * 1_000.0 * into_second)
                .sin()
                .signum()
    } else {
        0.0
    }
}

fn pulse_onsets(path: &std::path::Path, track: usize, from_seconds: i64) -> Vec<f64> {
    let mut demuxer = Demuxer::open(path).unwrap();
    let stream = demuxer.streams()[track + 1].clone();
    let mut decoder = ngine_media::AudioDecoder::open(&stream).unwrap();
    demuxer.seek(from_seconds * 1_000_000).unwrap();
    let mut onsets = Vec::new();
    let mut last_loud = f64::NEG_INFINITY;
    while let Some(packet) = demuxer.read().unwrap() {
        if packet.stream != track + 1 {
            continue;
        }
        for frame in decoder.decode(&packet).unwrap() {
            #[expect(
                clippy::cast_precision_loss,
                reason = "timestamps in a test stay far below 2^52"
            )]
            let starts = frame.pts.unwrap() as f64 / float(SAMPLE_RATE);
            for (index, sample) in frame
                .samples
                .iter()
                .step_by(usize::from(frame.channels))
                .enumerate()
            {
                if sample.abs() > 0.1 {
                    let time = starts + float(index) / float(SAMPLE_RATE);
                    if time - last_loud > 0.5 {
                        onsets.push(time);
                    }
                    last_loud = time;
                }
            }
        }
    }
    onsets.into_iter().skip(1).collect()
}

#[test]
fn sources_whose_clocks_drift_apart_stay_within_20_ms_over_thirty_minutes() {
    const MINUTES: f64 = 30.0;
    const SLOW_FRAME_RATE: u32 = 5;
    let folder = TempFolder::new("drift");
    let seconds = MINUTES * 60.0;
    let small = Size::new(160, 120);
    let picture = gray(small, 60);
    let fast_microphone = Sound {
        source: AudioSource::Microphone,
        nominal_rate: 48_000,
        real_rate: 48_010.0,
        channels: 1,
    };
    let script = ngine_capture::fake::in_time_order(
        frames(SLOW_FRAME_RATE, 0.0, seconds, move |_| Arc::clone(&picture)),
        ngine_capture::fake::in_time_order(
            sound(Sound::system(), 0.0, seconds, pulse_every_second),
            sound(fast_microphone, 0.0, seconds, pulse_every_second),
        ),
    );
    let mut rig = Rig::new(&config(SLOW_FRAME_RATE, Tracks::Three), script);
    let path = folder.file("recording.mp4");
    rig.pipeline.start_recording(&start(path.clone())).unwrap();
    rig.run_to_the_end();
    let (_, length) = rig.wait_for_finished_file();
    assert!(length.abs_diff(Duration::from_mins(30)) < Duration::from_millis(300));

    let system = pulse_onsets(&path, 1, 1_740);
    let microphone = pulse_onsets(&path, 2, 1_740);
    let mut compared = 0;
    for system in &system {
        let expected = system.round();
        assert!(
            (system - expected).abs() < 0.020,
            "the system pulse of second {expected} is at {system}"
        );
        let Some(microphone) = microphone
            .iter()
            .find(|microphone| (microphone.round() - expected).abs() < 0.5)
        else {
            continue;
        };
        assert!(
            (microphone - system).abs() < 0.020,
            "the microphone pulse at {microphone} and the system pulse at {system} are apart"
        );
        compared += 1;
    }
    assert!(compared >= 50, "{compared} seconds were compared");
}
