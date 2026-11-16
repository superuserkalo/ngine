use ngine_media::{
    AudioCodec, AudioEncoder, ClipRange, ClipWriter, PixelFormat, Rational, cut_clip,
};

use crate::common::{
    Decoded, FRAME_RATE, MICROPHONE_TONE, SAMPLE_RATE, SYSTEM_TONE, Source, TempFile, decode_file,
    encode_source, float, gray_level, mean_gray, tone, write_recording,
};

const SECONDS: i64 = 4;
const CLEAN_START_SAMPLES: usize = 4_800;

fn two_tone_source(codec: AudioCodec) -> Source {
    let frames = usize::try_from(SECONDS).unwrap() * SAMPLE_RATE;
    let audio = [
        tone(SYSTEM_TONE, 0, frames),
        tone(MICROPHONE_TONE, 0, frames),
    ];
    encode_source(SECONDS, 30, codec, &audio)
}

fn assert_starts_at_zero_on_frame(clip: &Decoded, first_frame: i64, frame_count: usize) {
    let micros = clip.video_micros();
    assert_eq!(micros[0], 0);
    assert_eq!(micros.len(), frame_count);
    let step = 1_000_000 / i64::from(FRAME_RATE);
    assert!(
        micros
            .windows(2)
            .all(|pair| (pair[1] - pair[0] - step).abs() <= 12)
    );
    let level = mean_gray(&clip.video[0]);
    let expected = f64::from(gray_level(first_frame));
    assert!((level - expected).abs() < 6.0, "{level} is not {expected}");
}

fn assert_sound_starts_clean(clip: &Decoded, track: usize, frequency: f32, source_second: usize) {
    let start = clip.audio_start_micros(track);
    assert!(start.abs() <= 1_000, "the sound starts at {start} µs");
    let heard = clip.left_channel(track);
    let expected = tone(frequency, source_second * SAMPLE_RATE, CLEAN_START_SAMPLES);
    let squared_error = heard[..CLEAN_START_SAMPLES]
        .iter()
        .zip(expected.iter().step_by(2))
        .map(|(heard, expected)| f64::from(heard - expected).powi(2))
        .sum::<f64>();
    let error = (squared_error / float(CLEAN_START_SAMPLES)).sqrt();
    assert!(
        error < 0.03,
        "the first tenth of a second is off by {error} RMS"
    );
}

fn cut_from_a_file(codec: AudioCodec) {
    let recording = TempFile::new(&format!("clip-source-{codec:?}.mp4"));
    let clip = TempFile::new(&format!("clip-{codec:?}.mp4"));
    write_recording(recording.path(), &two_tone_source(codec));

    let range = ClipRange {
        start_micros: Some(2_400_000),
        end_micros: Some(3_500_000),
    };
    let summary = cut_clip(recording.path(), clip.path(), &[0, 2], range).unwrap();
    assert_eq!(summary.source_start_micros, 2_000_000);
    assert_eq!(summary.video_frames, 45);
    assert!((summary.duration_micros - 1_500_000).abs() <= 12);

    let decoded = decode_file(clip.path(), PixelFormat::Bgra, false);
    assert_eq!(decoded.streams.len(), 2);
    assert_starts_at_zero_on_frame(&decoded, 60, 45);
    assert_sound_starts_clean(&decoded, 0, MICROPHONE_TONE, 2);
    let heard = decoded.left_channel(0).len();
    assert!(
        heard.abs_diff(SAMPLE_RATE * 3 / 2) <= 2_048,
        "the clip holds {heard} samples"
    );
}

#[test]
fn a_clip_cut_from_a_file_starts_at_zero_on_the_keyframe_before_the_cut_with_clean_aac() {
    cut_from_a_file(AudioCodec::Aac);
}

#[test]
fn a_clip_cut_from_a_file_starts_at_zero_on_the_keyframe_before_the_cut_with_clean_opus() {
    if !AudioEncoder::is_available(AudioCodec::Opus) {
        eprintln!("skipped: this FFmpeg build has no libopus");
        return;
    }
    cut_from_a_file(AudioCodec::Opus);
}

#[test]
fn a_replay_saved_from_buffered_packets_starts_at_its_first_keyframe() {
    let clip = TempFile::new("replay.mp4");
    let source = two_tone_source(AudioCodec::Aac);
    let first_video = source
        .packets
        .iter()
        .position(|packet| packet.stream == 0)
        .unwrap();
    let buffered = &source.packets[first_video + 1..];

    let mut writer =
        ClipWriter::create(clip.path(), &source.streams, &[0, 1], ClipRange::default()).unwrap();
    for packet in buffered {
        writer.push(packet.clone()).unwrap();
    }
    let summary = writer.finish().unwrap();
    assert_eq!(summary.source_start_micros, 1_000_000);
    assert_eq!(summary.video_frames, 90);

    let decoded = decode_file(clip.path(), PixelFormat::Bgra, false);
    assert_starts_at_zero_on_frame(&decoded, 30, 90);
    assert_sound_starts_clean(&decoded, 0, SYSTEM_TONE, 1);
}

#[test]
fn a_clip_after_a_long_gap_between_keyframes_starts_exactly_where_asked() {
    let frames = usize::try_from(SECONDS).unwrap() * SAMPLE_RATE;
    let audio = [tone(SYSTEM_TONE, 0, frames)];
    let one_keyframe = encode_source(SECONDS, 1_000, AudioCodec::Aac, &audio);
    let range = ClipRange {
        start_micros: Some(2_500_000),
        end_micros: None,
    };

    let snapped = TempFile::new("snapped.mp4");
    let mut writer =
        ClipWriter::create(snapped.path(), &one_keyframe.streams, &[0, 1], range).unwrap();
    for packet in &one_keyframe.packets {
        writer.push(packet.clone()).unwrap();
    }
    assert_eq!(writer.finish().unwrap().source_start_micros, 0);

    let exact = TempFile::new("exact.mp4");
    let mut writer = ClipWriter::create(exact.path(), &one_keyframe.streams, &[0, 1], range)
        .unwrap()
        .starting_exactly_after_a_long_gap(2_000_000);
    for packet in &one_keyframe.packets {
        writer.push(packet.clone()).unwrap();
    }
    let summary = writer.finish().unwrap();
    assert_eq!(summary.source_start_micros, 2_500_000);
    assert_eq!(summary.video_frames, 45);
    assert!((summary.duration_micros - 1_500_000).abs() <= 12);

    let decoded = decode_file(exact.path(), PixelFormat::Bgra, false);
    assert_starts_at_zero_on_frame(&decoded, 75, 45);
    let duration = decoded.duration_micros.unwrap();
    assert!((duration - 1_500_000).abs() < 50_000, "{duration} µs");
    let heard = decoded.left_channel(0).len();
    assert!(
        heard.abs_diff(SAMPLE_RATE * 3 / 2) <= 2_048,
        "{heard} samples"
    );
}

#[test]
fn packets_without_a_keyframe_cannot_become_a_clip() {
    let clip = TempFile::new("no-keyframe.mp4");
    let source = two_tone_source(AudioCodec::Aac);
    let mut writer =
        ClipWriter::create(clip.path(), &source.streams, &[0, 1], ClipRange::default()).unwrap();
    for packet in source.packets.iter().filter(|packet| !packet.keyframe) {
        writer.push(packet.clone()).unwrap();
    }
    assert!(writer.finish().is_err());
}

#[test]
fn a_clip_needs_video_as_its_first_track() {
    let clip = TempFile::new("audio-first.mp4");
    let source = two_tone_source(AudioCodec::Aac);
    let range = ClipRange::default();
    assert!(ClipWriter::create(clip.path(), &source.streams, &[1, 0], range).is_err());
    assert!(ClipWriter::create(clip.path(), &source.streams, &[0, 7], range).is_err());
    assert_eq!(
        source.streams[1].time_base,
        Rational::new(1, i32::try_from(SAMPLE_RATE).unwrap())
    );
}
