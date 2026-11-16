use std::path::Path;
use std::time::Duration;

use ngine_media::{EncoderInfo, VideoCodec};
use ngine_recorder::{ClipMix, Cut, ExportControl, ExportRequest, ExportSummary, export_clip};

use crate::common::{
    Decoded, LEVEL, MICROPHONE_TONE, SAMPLE_RATE, SYSTEM_TONE, TempFolder, assert_tone, decode,
    float, h264_encoder, micros_of_frame, tone_level, write_recording,
};

const FRAME_MICROS: i64 = 33_333;

fn at_frame(frame: i64) -> Duration {
    Duration::from_micros(micros_of_frame(frame).unsigned_abs())
}

fn request(source: &Path, target: &Path, frames: (i64, i64), cut: Cut) -> ExportRequest {
    ExportRequest {
        source: source.to_owned(),
        target: target.to_owned(),
        start: at_frame(frames.0),
        end: at_frame(frames.1),
        cut,
        mix: ClipMix::default(),
        encoders: vec![EncoderInfo {
            name: h264_encoder(),
            codec: VideoCodec::H264,
            hardware: false,
        }],
    }
}

fn export(request: &ExportRequest) -> (ExportSummary, Decoded) {
    let control = ExportControl::new();
    let summary = export_clip(request, &control).unwrap();
    assert!((control.progress() - 1.0).abs() < f32::EPSILON);
    (summary, decode(&request.target))
}

fn assert_runs_from_zero_over(clip: &Decoded, first: i64, last: i64) {
    let count = usize::try_from(last - first + 1).unwrap();
    assert_eq!(clip.frame_micros.len(), count);
    assert_eq!(clip.frame_micros[0], 0);
    assert!(
        clip.frame_micros
            .windows(2)
            .all(|pair| (pair[1] - pair[0] - FRAME_MICROS).abs() <= 12),
        "{:?}",
        clip.frame_micros
    );
    let expected: Vec<i64> = (first..=last).collect();
    assert_eq!(clip.frame_numbers, expected);
    let expected = i64::try_from(count).unwrap() * FRAME_MICROS;
    assert!(
        (clip.length_micros - expected).abs() < 40_000,
        "the clip is {} µs long instead of {expected}",
        clip.length_micros
    );
    assert_eq!(clip.audio_tracks, 1);
    let heard = float(clip.sound.len()) / float(SAMPLE_RATE) * 1_000_000.0;
    let wanted = float(count) * 33_333.0;
    assert!(
        (heard - wanted).abs() < 70_000.0,
        "{heard} µs of sound for {wanted} µs of video"
    );
}

#[test]
fn the_fast_cut_starts_at_the_keyframe_before_the_in_point_and_ends_on_the_out_frame() {
    let folder = TempFolder::new("fast");
    let recording = folder.file("Recording.mp4");
    write_recording(&recording, 3);
    let original = std::fs::read(&recording).unwrap();

    let request = request(&recording, &folder.file("Clip.mp4"), (72, 105), Cut::Fast);
    let (summary, clip) = export(&request);
    assert_eq!(summary.source_start, Duration::from_secs(2));
    assert_eq!(summary.video_frames, 46);
    assert_runs_from_zero_over(&clip, 60, 105);
    assert_tone(tone_level(&clip.sound, SYSTEM_TONE), LEVEL);
    assert_tone(tone_level(&clip.sound, MICROPHONE_TONE), LEVEL);
    assert_eq!(std::fs::read(&recording).unwrap(), original);
}

#[test]
fn the_exact_cut_starts_and_ends_on_the_chosen_frames() {
    let folder = TempFolder::new("exact");
    let recording = folder.file("Recording.mp4");
    write_recording(&recording, 3);

    let request = request(&recording, &folder.file("Clip.mp4"), (72, 105), Cut::Exact);
    let (summary, clip) = export(&request);
    assert_eq!(summary.source_start, at_frame(72));
    assert_eq!(summary.video_frames, 34);
    assert_runs_from_zero_over(&clip, 72, 105);
    assert_tone(tone_level(&clip.sound, SYSTEM_TONE), LEVEL);
    assert_tone(tone_level(&clip.sound, MICROPHONE_TONE), LEVEL);
}

#[test]
fn an_exact_cut_inside_one_group_and_one_that_starts_on_a_keyframe_are_exact_too() {
    let folder = TempFolder::new("exact-edges");
    let recording = folder.file("Recording.mp4");
    write_recording(&recording, 1);

    let inside = request(&recording, &folder.file("Inside.mp4"), (66, 80), Cut::Exact);
    assert_runs_from_zero_over(&export(&inside).1, 66, 80);

    let on_keyframe = request(
        &recording,
        &folder.file("Keyframe.mp4"),
        (60, 110),
        Cut::Exact,
    );
    assert_runs_from_zero_over(&export(&on_keyframe).1, 60, 110);

    let from_the_start = request(&recording, &folder.file("Start.mp4"), (0, 45), Cut::Exact);
    assert_runs_from_zero_over(&export(&from_the_start).1, 0, 45);

    let to_the_end = request(&recording, &folder.file("End.mp4"), (90, 119), Cut::Exact);
    assert_runs_from_zero_over(&export(&to_the_end).1, 90, 119);
}

#[test]
fn a_clip_holds_one_sound_track_mixed_as_asked() {
    let folder = TempFolder::new("mix");
    let recording = folder.file("Recording.mp4");
    write_recording(&recording, 3);

    let mut muted = request(&recording, &folder.file("Muted.mp4"), (72, 115), Cut::Exact);
    muted.mix = ClipMix {
        system: 1.0,
        microphone: 0.0,
    };
    let clip = export(&muted).1;
    assert_runs_from_zero_over(&clip, 72, 115);
    assert_tone(tone_level(&clip.sound, SYSTEM_TONE), LEVEL);
    assert_tone(tone_level(&clip.sound, MICROPHONE_TONE), 0.0);

    let mut rebalanced = request(&recording, &folder.file("Quiet.mp4"), (72, 115), Cut::Fast);
    rebalanced.mix = ClipMix {
        system: 0.5,
        microphone: 1.0,
    };
    let clip = export(&rebalanced).1;
    assert_runs_from_zero_over(&clip, 60, 115);
    assert_tone(tone_level(&clip.sound, SYSTEM_TONE), LEVEL / 2.0);
    assert_tone(tone_level(&clip.sound, MICROPHONE_TONE), LEVEL);
}

#[test]
fn a_recording_with_only_the_mix_keeps_it_whatever_the_mix_asks() {
    let folder = TempFolder::new("mix-only");
    let recording = folder.file("Recording.mp4");
    write_recording(&recording, 1);
    let mut request = request(&recording, &folder.file("Clip.mp4"), (30, 90), Cut::Fast);
    request.mix = ClipMix {
        system: 0.0,
        microphone: 0.0,
    };
    let clip = export(&request).1;
    assert_eq!(clip.audio_tracks, 1);
    assert_tone(tone_level(&clip.sound, SYSTEM_TONE), LEVEL);
}

#[test]
fn an_export_never_replaces_a_file_and_leaves_nothing_behind_when_it_stops() {
    let folder = TempFolder::new("refuse");
    let recording = folder.file("Recording.mp4");
    write_recording(&recording, 1);
    let taken = folder.file("Taken.mp4");
    std::fs::write(&taken, "mine").unwrap();
    let control = ExportControl::new();

    let onto_a_file = request(&recording, &taken, (10, 50), Cut::Fast);
    assert!(export_clip(&onto_a_file, &control).is_err());
    assert_eq!(std::fs::read_to_string(&taken).unwrap(), "mine");

    let backwards = request(
        &recording,
        &folder.file("Backwards.mp4"),
        (50, 10),
        Cut::Fast,
    );
    assert!(export_clip(&backwards, &control).is_err());

    let cancelled = request(
        &recording,
        &folder.file("Cancelled.mp4"),
        (10, 110),
        Cut::Exact,
    );
    control.cancel();
    assert!(export_clip(&cancelled, &control).is_err());

    let mut names: Vec<String> = std::fs::read_dir(folder.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["Recording.mp4", "Taken.mp4"]);
}
