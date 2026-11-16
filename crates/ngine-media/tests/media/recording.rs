use ngine_media::{
    AudioCodec, AudioLayout, Codec, Mixer, Mp4Layout, Mp4Writer, PixelFormat, Resampler, VideoCodec,
};

use crate::common::{
    FRAME_RATE, HEIGHT, MICROPHONE_TONE, SAMPLE_RATE, SYSTEM_TONE, TONE_LEVEL, TempFile, WIDTH,
    decode_file, encode_source, float, gray_level, mean_gray, narrow, tone, tone_level,
    top_level_boxes, write_recording,
};

const SECONDS: i64 = 3;
const MICROPHONE_RATE: u32 = 44_100;

fn microphone_at_its_own_rate(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|frame| {
            let turns = f64::from(MICROPHONE_TONE) * float(frame) / f64::from(MICROPHONE_RATE);
            narrow(f64::from(TONE_LEVEL) * (std::f64::consts::TAU * turns).sin())
        })
        .collect()
}

fn three_tracks() -> Vec<Vec<f32>> {
    let frames = usize::try_from(SECONDS).unwrap() * SAMPLE_RATE;
    let system = tone(SYSTEM_TONE, 0, frames);

    let mut resampler = Resampler::new(AudioLayout {
        sample_rate: MICROPHONE_RATE,
        channels: 1,
    })
    .unwrap();
    let captured = microphone_at_its_own_rate(usize::try_from(SECONDS).unwrap() * 44_100);
    let mut microphone = resampler.push(&captured).unwrap();
    microphone.extend(resampler.finish().unwrap());
    microphone.resize(frames * 2, 0.0);

    let mut mixer = Mixer::new(2);
    mixer.push(0, &system).unwrap();
    mixer.push(1, &microphone).unwrap();
    let mix = mixer.mix(frames);
    vec![mix, system, microphone]
}

#[test]
fn a_recording_with_three_audio_tracks_decodes_back_with_its_frames_tracks_and_timestamps() {
    let file = TempFile::new("three-tracks.mp4");
    let source = encode_source(SECONDS, 30, AudioCodec::Aac, &three_tracks());
    write_recording(file.path(), &source);

    let boxes = top_level_boxes(file.path());
    assert!(boxes.contains(&"moov".to_owned()), "{boxes:?}");
    assert!(!boxes.contains(&"moof".to_owned()), "{boxes:?}");

    let decoded = decode_file(file.path(), PixelFormat::Bgra, false);
    let codecs = decoded
        .streams
        .iter()
        .map(|stream| stream.parameters.codec())
        .collect::<Vec<_>>();
    assert_eq!(
        codecs,
        [
            Some(Codec::Video(VideoCodec::H264)),
            Some(Codec::Audio(AudioCodec::Aac)),
            Some(Codec::Audio(AudioCodec::Aac)),
            Some(Codec::Audio(AudioCodec::Aac)),
        ]
    );
    assert_eq!(decoded.streams[0].parameters.width(), WIDTH);
    assert_eq!(decoded.streams[0].parameters.height(), HEIGHT);

    let frame_count = SECONDS * i64::from(FRAME_RATE);
    let expected_micros = (0..frame_count)
        .map(|frame| frame * 1_000_000 / i64::from(FRAME_RATE))
        .collect::<Vec<_>>();
    let micros = decoded.video_micros();
    assert_eq!(micros.len(), expected_micros.len());
    for (actual, expected) in micros.iter().zip(&expected_micros) {
        assert!(
            (actual - expected).abs() <= 12,
            "{actual} is not {expected}"
        );
    }
    for (index, frame) in decoded.video.iter().enumerate() {
        let expected = f64::from(gray_level(i64::try_from(index).unwrap()));
        let actual = mean_gray(frame);
        assert!(
            (actual - expected).abs() < 6.0,
            "frame {index} is {actual}, not {expected}"
        );
    }

    let duration = decoded.duration_micros.unwrap();
    assert!(
        (duration - SECONDS * 1_000_000).abs() < 50_000,
        "the file lasts {duration} µs"
    );

    let expected_levels = [
        (TONE_LEVEL, TONE_LEVEL),
        (TONE_LEVEL, 0.0),
        (0.0, TONE_LEVEL),
    ];
    for (track, (system, microphone)) in expected_levels.into_iter().enumerate() {
        assert_eq!(decoded.streams[track + 1].parameters.sample_rate(), 48_000);
        assert_eq!(decoded.streams[track + 1].parameters.channels(), 2);
        assert!(decoded.audio_start_micros(track).abs() < 1_000);
        let samples = decoded.left_channel(track);
        let expected_samples = usize::try_from(SECONDS).unwrap() * SAMPLE_RATE;
        assert!(
            samples.len().abs_diff(expected_samples) <= 2_048,
            "track {track} holds {} samples",
            samples.len()
        );
        let steady = &samples[SAMPLE_RATE / 2..SAMPLE_RATE * 2];
        let found = (
            tone_level(steady, SYSTEM_TONE),
            tone_level(steady, MICROPHONE_TONE),
        );
        assert!(
            (found.0 - system).abs() < 0.05 && (found.1 - microphone).abs() < 0.05,
            "track {track} holds the tones at {found:?}, not ({system}, {microphone})"
        );
    }
}

#[test]
fn a_recording_that_was_never_finished_decodes_up_to_its_last_fragment() {
    let file = TempFile::new("unfinished.mp4");
    let crashed = TempFile::new("unfinished-copy.mp4");
    let source = encode_source(SECONDS, 30, AudioCodec::Aac, &three_tracks());

    let mut writer = Mp4Writer::create(file.path(), &source.streams, Mp4Layout::Recording).unwrap();
    for packet in &source.packets {
        writer.write(packet).unwrap();
    }
    std::fs::copy(file.path(), crashed.path()).unwrap();
    drop(writer);

    let boxes = top_level_boxes(crashed.path());
    assert!(boxes.contains(&"moof".to_owned()), "{boxes:?}");
    let decoded = decode_file(crashed.path(), PixelFormat::Nv12, false);
    let whole_seconds = decoded.video.len() / usize::try_from(FRAME_RATE).unwrap();
    assert!(
        (1..usize::try_from(SECONDS).unwrap()).contains(&whole_seconds),
        "{} frames survived",
        decoded.video.len()
    );
    assert_eq!(decoded.video_micros()[0], 0);
    for track in 0..3 {
        let samples = decoded.left_channel(track);
        assert!(
            samples.len() >= whole_seconds * SAMPLE_RATE - 4_096,
            "track {track} kept {} samples for {whole_seconds} s of video",
            samples.len()
        );
    }

    let finished = decode_file(file.path(), PixelFormat::Nv12, false);
    assert_eq!(finished.video.len(), source_frame_count());
}

fn source_frame_count() -> usize {
    usize::try_from(SECONDS * i64::from(FRAME_RATE)).unwrap()
}
