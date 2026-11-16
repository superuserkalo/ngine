use ngine_media::{
    AudioCodec, AudioDecoder, AudioEncoder, AudioEncoderSettings, AudioLayout, Resampler,
};

use crate::common::{SAMPLE_RATE, SYSTEM_TONE, TONE_LEVEL, narrow, tone, tone_level};

const STEREO_48K: AudioLayout = AudioLayout {
    sample_rate: 48_000,
    channels: 2,
};
const BUFFER_FRAMES: usize = 480;
const TWENTY_MILLISECONDS: usize = 960;

fn left(samples: &[f32]) -> Vec<f32> {
    samples.iter().step_by(2).copied().collect()
}

fn round_trip(codec: AudioCodec) {
    let mut encoder = AudioEncoder::open(AudioEncoderSettings {
        codec,
        bitrate: 128_000,
    })
    .unwrap();
    let mut packets = encoder.encode(&tone(SYSTEM_TONE, 0, SAMPLE_RATE)).unwrap();
    packets.extend(encoder.finish().unwrap());
    let stream = encoder.stream_info();
    assert_eq!(stream.parameters.sample_rate(), 48_000);
    assert_eq!(stream.parameters.channels(), 2);
    assert_ne!(stream.parameters.extradata(), [0_u8; 0]);
    assert!(packets.iter().all(|packet| packet.keyframe));
    let padding = i64::from(stream.parameters.initial_padding());
    assert_eq!(packets[0].pts, Some(-padding));

    let mut decoder = AudioDecoder::open(stream).unwrap();
    let mut frames = Vec::new();
    for packet in &packets {
        frames.extend(decoder.decode(packet).unwrap());
    }
    frames.extend(decoder.finish().unwrap());
    let heard = frames
        .iter()
        .flat_map(|frame| left(&frame.samples))
        .collect::<Vec<_>>();
    assert!(
        heard.len() >= SAMPLE_RATE,
        "{} samples came back",
        heard.len()
    );
    let steady = &heard[SAMPLE_RATE / 4..SAMPLE_RATE * 3 / 4];
    let level = tone_level(steady, SYSTEM_TONE);
    assert!((level - TONE_LEVEL).abs() < 0.05, "the tone is at {level}");
}

#[test]
fn aac_survives_encoding_and_decoding() {
    round_trip(AudioCodec::Aac);
}

#[test]
fn opus_survives_encoding_and_decoding() {
    if !AudioEncoder::is_available(AudioCodec::Opus) {
        eprintln!("skipped: this FFmpeg build has no libopus");
        return;
    }
    round_trip(AudioCodec::Opus);
}

#[test]
fn a_mono_44_1_khz_source_becomes_48_khz_stereo_of_the_same_length_and_pitch() {
    let mut resampler = Resampler::new(AudioLayout {
        sample_rate: 44_100,
        channels: 1,
    })
    .unwrap();
    let captured = (0..44_100)
        .map(|frame| {
            let turns = f64::from(SYSTEM_TONE) * f64::from(frame) / 44_100.0;
            narrow(f64::from(TONE_LEVEL) * (std::f64::consts::TAU * turns).sin())
        })
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    for buffer in captured.chunks(441) {
        output.extend(resampler.push(buffer).unwrap());
    }
    output.extend(resampler.finish().unwrap());

    assert!(
        (output.len() / 2).abs_diff(SAMPLE_RATE) <= 64,
        "{} frames came out",
        output.len() / 2
    );
    let steady = &output[SAMPLE_RATE / 2..SAMPLE_RATE * 3 / 2];
    assert!((tone_level(&left(steady), SYSTEM_TONE) - TONE_LEVEL).abs() < 0.02);
    assert!(
        output
            .chunks(2)
            .all(|frame| frame[0].to_bits() == frame[1].to_bits())
    );
}

fn worst_drift_over_thirty_minutes(frames_per_real_second: usize, synchronized: bool) -> usize {
    let mut resampler = if synchronized {
        Resampler::synchronized(STEREO_48K).unwrap()
    } else {
        Resampler::new(STEREO_48K).unwrap()
    };
    let one_buffer = tone(SYSTEM_TONE, 0, BUFFER_FRAMES);
    let buffers = 30 * 60 * frames_per_real_second / BUFFER_FRAMES;
    let mut produced = 0_usize;
    let mut worst_drift = 0_usize;
    for buffer in 0..buffers {
        let first_frame = buffer * BUFFER_FRAMES;
        let captured_micros = first_frame * 1_000_000 / frames_per_real_second;
        let output = if synchronized {
            resampler
                .push_at(i64::try_from(captured_micros).unwrap(), &one_buffer)
                .unwrap()
        } else {
            resampler.push(&one_buffer).unwrap()
        };
        produced += output.len() / 2;
        let due = (first_frame + BUFFER_FRAMES) * SAMPLE_RATE / frames_per_real_second;
        if buffer > 100 {
            worst_drift = worst_drift.max(produced.abs_diff(due));
        }
    }
    worst_drift
}

#[test]
fn a_source_whose_clock_runs_fast_or_slow_stays_within_20_ms_of_the_capture_clock_over_30_minutes()
{
    for frames_per_real_second in [48_010, 47_990] {
        let uncorrected = worst_drift_over_thirty_minutes(frames_per_real_second, false);
        assert!(uncorrected > 10 * TWENTY_MILLISECONDS);

        let corrected = worst_drift_over_thirty_minutes(frames_per_real_second, true);
        assert!(
            corrected <= TWENTY_MILLISECONDS,
            "a clock at {frames_per_real_second} Hz drifted by {corrected} frames"
        );
    }
}

#[test]
fn a_late_start_and_a_gap_become_silence_in_a_synchronized_stream() {
    let mut resampler = Resampler::synchronized(STEREO_48K).unwrap();
    let half_second = SAMPLE_RATE / 2;
    let mut output = resampler
        .push_at(500_000, &tone(SYSTEM_TONE, 0, half_second))
        .unwrap();
    output.extend(
        resampler
            .push_at(2_000_000, &tone(SYSTEM_TONE, 0, half_second))
            .unwrap(),
    );
    output.extend(resampler.finish().unwrap());
    let heard = left(&output);

    assert!(
        heard.len().abs_diff(SAMPLE_RATE * 5 / 2) <= 64,
        "{} frames came out",
        heard.len()
    );
    let quiet = |from: usize, to: usize| heard[from..to].iter().all(|sample| sample.abs() < 0.01);
    let loud = |from: usize, to: usize| tone_level(&heard[from..to], SYSTEM_TONE) > 0.3;
    assert!(quiet(0, 23_000));
    assert!(loud(25_000, 47_000));
    assert!(quiet(49_000, 95_000));
    assert!(loud(97_000, 119_000));
}

#[test]
fn a_plain_resampler_takes_no_capture_times() {
    let mut resampler = Resampler::new(STEREO_48K).unwrap();
    assert!(resampler.push_at(0, &[0.0, 0.0]).is_err());
    assert!(resampler.push(&[0.0; 3]).is_err());
    assert!(
        Resampler::new(AudioLayout {
            sample_rate: 0,
            channels: 2
        })
        .is_err()
    );
}
