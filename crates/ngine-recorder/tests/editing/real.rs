use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ngine_media::{Demuxer, PixelFormat, Rational, VideoDecoder, VideoDecoderOptions};
use ngine_recorder::{
    ClipMix, Cut, ExportControl, ExportRequest, Picture, Player, PlayerOptions, export_clip,
    picture_at, probe, read_markers, working_video_encoders,
};

use crate::common::TempFolder;

const SMALL: (u32, u32) = (480, 480);
const WAIT: Duration = Duration::from_secs(30);

fn video_times(file: &Path) -> Vec<i64> {
    let mut demuxer = Demuxer::open(file).unwrap();
    let time_base = demuxer.streams()[0].time_base;
    let mut times = Vec::new();
    while let Some(packet) = demuxer.read().unwrap() {
        if packet.stream == 0 {
            times.push(
                time_base
                    .rescale(packet.pts.unwrap(), Rational::MICROS)
                    .unwrap(),
            );
        }
    }
    times.sort_unstable();
    times
}

fn decoded_frames(file: &Path, hardware: bool) -> usize {
    let mut demuxer = Demuxer::open(file).unwrap();
    let options = VideoDecoderOptions {
        output: PixelFormat::Nv12,
        hardware,
    };
    let mut decoder = VideoDecoder::open(&demuxer.streams()[0], options).unwrap();
    let mut frames = 0;
    while let Some(packet) = demuxer.read().unwrap() {
        if packet.stream == 0 {
            frames += decoder.decode(&packet).unwrap().len();
        }
    }
    frames + decoder.finish().unwrap().len()
}

fn difference(one: &Picture, other: &Picture) -> f64 {
    assert_eq!((one.width, one.height), (other.width, other.height));
    let total: u64 = one
        .rgba
        .iter()
        .zip(&other.rgba)
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    f64::from(u32::try_from(total / 1000).unwrap()) * 1000.0
        / f64::from(u32::try_from(one.rgba.len()).unwrap())
}

fn micros(time: i64) -> Duration {
    Duration::from_micros(time.unsigned_abs())
}

fn check_clip(source: &Path, clip: &Path, first: i64, last: i64, times: &[i64]) {
    let expected = times
        .iter()
        .filter(|time| **time >= first && **time <= last)
        .count();
    let clip_times = video_times(clip);
    println!(
        "  {} video frames (expected {expected}), first at {} µs, last at {} µs",
        clip_times.len(),
        clip_times[0],
        clip_times[clip_times.len() - 1]
    );
    assert_eq!(clip_times.len(), expected);
    assert_eq!(clip_times[0], 0);
    assert_eq!(decoded_frames(clip, false), expected, "software decoding");
    assert_eq!(decoded_frames(clip, true), expected, "hardware decoding");
    let info = probe(clip).unwrap();
    println!(
        "  length {:.3} s, {} sound track(s), {}x{}",
        info.length.as_secs_f64(),
        info.audio_tracks,
        info.width,
        info.height
    );
    assert!(info.audio_tracks <= 1);
    let wanted = micros(last - first).as_secs_f64();
    assert!((info.length.as_secs_f64() - wanted).abs() < 0.1);
    let middle = times[times.len() / 2].clamp(first, last);
    for (name, at) in [("first", first), ("middle", middle), ("last", last)] {
        let in_source = picture_at(source, micros(at), true, SMALL).unwrap();
        let in_clip = picture_at(clip, micros(at - first), true, SMALL).unwrap();
        let apart = difference(&in_source, &in_clip);
        println!("  the {name} frame differs from the source's by {apart:.2} of 255");
        assert!(
            apart < 8.0,
            "the {name} frame does not look like the source"
        );
    }
}

#[test]
#[ignore = "needs a real recording: set NGINE_REAL_RECORDING to its path"]
fn a_real_recording_exports_both_ways_and_plays() {
    let source = PathBuf::from(std::env::var_os("NGINE_REAL_RECORDING").unwrap());
    let info = probe(&source).unwrap();
    let times = video_times(&source);
    println!(
        "source: {:.3} s, {} frames, {}x{}, {:?}, {} sound tracks, markers {:?}",
        info.length.as_secs_f64(),
        times.len(),
        info.width,
        info.height,
        info.video_codec,
        info.audio_tracks,
        read_markers(&source).unwrap()
    );
    let start = times[times.len() * 2 / 5 + 3];
    let end = times[times.len() * 4 / 5];
    let folder = TempFolder::new("real");
    let encoders = working_video_encoders();
    println!(
        "encoders: {:?}",
        encoders.iter().map(|e| &e.name).collect::<Vec<_>>()
    );

    for (cut, mix) in [
        (Cut::Fast, ClipMix::default()),
        (Cut::Exact, ClipMix::default()),
        (
            Cut::Exact,
            ClipMix {
                system: 0.5,
                microphone: 0.0,
            },
        ),
    ] {
        let target = folder.file(&format!("{cut:?}-{}.mp4", mix.microphone));
        let request = ExportRequest {
            source: source.clone(),
            target: target.clone(),
            start: micros(start),
            end: micros(end),
            cut,
            mix,
            encoders: encoders.clone(),
        };
        let asked = Instant::now();
        let summary = export_clip(&request, &ExportControl::new()).unwrap();
        println!(
            "{cut:?} cut {mix:?} of {start}..={end} µs took {:?}: starts at {:?}, {:?} long, {} frames, {} bytes",
            asked.elapsed(),
            summary.source_start,
            summary.length,
            summary.video_frames,
            std::fs::metadata(&target).unwrap().len()
        );
        let first = i64::try_from(summary.source_start.as_micros()).unwrap();
        match cut {
            Cut::Fast => assert!(first <= start),
            Cut::Exact => assert_eq!(first, start),
        }
        check_clip(&source, &target, first, end, &times);
    }

    play(&source, start);
}

fn play(source: &Path, start: i64) {
    let options = PlayerOptions {
        picture_bounds: SMALL,
        hardware: true,
    };
    let player = Player::open(source, options).unwrap();
    player.seek(micros(start));
    let asked = Instant::now();
    let shown = loop {
        let playhead = player.advance();
        assert_eq!(playhead.failure, None);
        if let Some(picture) = playhead.picture
            && playhead.position == micros(start)
        {
            break picture;
        }
        assert!(asked.elapsed() < WAIT);
        std::thread::sleep(Duration::from_millis(2));
    };
    let wanted = picture_at(source, micros(start), true, SMALL).unwrap();
    println!(
        "the player showed the frame at {start} µs after {:?}, {:.2} of 255 from the software decoder's",
        asked.elapsed(),
        difference(&shown, &wanted)
    );
    let mut feed = player.sound();
    player.play();
    let mut block = [0.0_f32; 960];
    let mut loudest = 0.0_f32;
    let asked = Instant::now();
    while player.advance().position < micros(start) + Duration::from_secs(1) {
        feed.fill(&mut block);
        loudest = block
            .iter()
            .fold(loudest, |peak, sample| peak.max(sample.abs()));
        assert!(asked.elapsed() < WAIT, "playback stalled");
        std::thread::sleep(Duration::from_micros(500));
    }
    println!(
        "played one second of sound in {:?}, peak level {loudest:.4}, frame time {:?}",
        asked.elapsed(),
        player.advance().frame_time
    );
    drop(feed);
    player.pause();
    player.seek(micros(start));
    player.play();
    let (asked, mut shown) = (Instant::now(), 0_u32);
    while asked.elapsed() < Duration::from_secs(2) {
        shown += u32::from(player.advance().picture.is_some());
        std::thread::sleep(Duration::from_millis(4));
    }
    let moved = player.advance().position.saturating_sub(micros(start));
    println!("by the clock, two seconds showed {shown} pictures and moved {moved:?}");
    assert!(shown > 30, "the pictures do not keep up");
}
