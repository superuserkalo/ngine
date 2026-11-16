use std::sync::Arc;
use std::time::{Duration, Instant};

use ngine_recorder::{ClipMix, Picture, Player, PlayerOptions};

use crate::common::{
    LEVEL, MICROPHONE_TONE, SYSTEM_TONE, TempFolder, assert_tone, frame_number, micros_of_frame,
    tone_level, write_recording,
};

const WAIT: Duration = Duration::from_secs(20);
const OPTIONS: PlayerOptions = PlayerOptions {
    picture_bounds: (160, 160),
    hardware: false,
};

fn next_picture(player: &Player) -> Arc<Picture> {
    let asked = Instant::now();
    loop {
        let playhead = player.advance();
        assert_eq!(playhead.failure, None);
        if let Some(picture) = playhead.picture {
            return picture;
        }
        assert!(asked.elapsed() < WAIT, "no picture arrived");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn shown_frame(picture: &Picture) -> i64 {
    frame_number(&picture.rgba, picture.width, picture.height)
}

fn at_frame(frame: i64) -> Duration {
    Duration::from_micros(micros_of_frame(frame).unsigned_abs())
}

#[test]
fn a_player_shows_the_first_frame_and_the_frame_it_is_sent_to() {
    let folder = TempFolder::new("scrub");
    let video = folder.file("Recording.mp4");
    write_recording(&video, 3);
    let player = Player::open(&video, OPTIONS).unwrap();
    assert!(player.info().has_separate_sound());

    let first = next_picture(&player);
    assert_eq!((first.width, first.height), (160, 120));
    assert_eq!(frame_number(&first.rgba, first.width, first.height), 0);

    for frame in [75, 31, 119, 60] {
        player.seek(at_frame(frame));
        assert_eq!(shown_frame(&next_picture(&player)), frame);
        let playhead = player.advance();
        assert_eq!(playhead.position, at_frame(frame));
        assert!(!playhead.playing);
    }
}

#[test]
fn without_a_sound_device_playback_follows_the_clock_and_stops_at_the_end() {
    let folder = TempFolder::new("clock");
    let video = folder.file("Recording.mp4");
    write_recording(&video, 1);
    let player = Player::open(&video, OPTIONS).unwrap();
    next_picture(&player);
    player.seek(at_frame(105));
    assert_eq!(shown_frame(&next_picture(&player)), 105);

    player.play();
    let started = Instant::now();
    let mut shown = Vec::new();
    loop {
        let playhead = player.advance();
        if let Some(picture) = playhead.picture {
            shown.push(shown_frame(&picture));
        }
        if !playhead.playing {
            assert!(playhead.position >= at_frame(119));
            break;
        }
        assert!(started.elapsed() < WAIT, "playback never ended");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(started.elapsed() >= Duration::from_millis(400));
    assert!(shown.len() >= 5, "{shown:?}");
    assert!(shown.windows(2).all(|pair| pair[1] > pair[0]), "{shown:?}");
    assert_eq!(shown.last(), Some(&119));
}

fn pull(player: &Player, feed: &mut ngine_recorder::SoundFeed, seconds: f64) -> Vec<f32> {
    let mut heard = Vec::new();
    let mut block = [0.0_f32; 960];
    let asked = Instant::now();
    while crate::common::float(heard.len()) < seconds * 48_000.0 {
        let before = player.advance().position;
        feed.fill(&mut block);
        let moved = player.advance().position.saturating_sub(before);
        let frames = usize::try_from((moved.as_micros() * 48 + 500) / 1000).unwrap();
        heard.extend(block.iter().step_by(2).take(frames));
        if frames == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(asked.elapsed() < WAIT, "the sound stalled");
    }
    heard
}

#[test]
fn with_a_sound_device_the_position_follows_the_sound_that_was_played() {
    let folder = TempFolder::new("sound");
    let video = folder.file("Recording.mp4");
    write_recording(&video, 3);
    let player = Player::open(&video, OPTIONS).unwrap();
    next_picture(&player);
    let mut feed = player.sound();

    let mut silent = [1.0_f32; 64];
    feed.fill(&mut silent);
    assert_eq!(silent, [0.0; 64]);
    assert_eq!(player.advance().position, Duration::ZERO);

    player.seek(at_frame(30));
    next_picture(&player);
    player.play();
    let heard = pull(&player, &mut feed, 1.0);
    let position = player.advance().position.as_secs_f64();
    assert!(
        (position - 2.0).abs() < 0.05,
        "at {position} s after one second of sound"
    );
    assert_tone(tone_level(&heard[4_800..], SYSTEM_TONE), LEVEL);
    assert_tone(tone_level(&heard[4_800..], MICROPHONE_TONE), LEVEL);
    std::thread::sleep(Duration::from_millis(100));
    let waited = player.advance().position.as_secs_f64();
    assert!(
        (waited - position).abs() < 0.001,
        "the position moved without sound"
    );

    player.set_mix(ClipMix {
        system: 1.0,
        microphone: 0.0,
    });
    pull(&player, &mut feed, 0.6);
    let heard = pull(&player, &mut feed, 0.5);
    assert_tone(tone_level(&heard, SYSTEM_TONE), LEVEL);
    assert_tone(tone_level(&heard, MICROPHONE_TONE), 0.0);

    player.pause();
    drop(feed);
    assert!(!player.advance().playing);
}
