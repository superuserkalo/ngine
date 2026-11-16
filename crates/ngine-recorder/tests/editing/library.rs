use std::fs;
use std::time::{Duration, SystemTime};

use ngine_media::{Mp4Layout, Mp4Writer};
use ngine_recorder::{
    RenameError, ThumbnailCache, finish_unfinished_recordings, is_unfinished, journal_path,
    picture_at, probe, read_markers, rename, scan,
};

use crate::common::{
    HEIGHT, SECONDS, TempFolder, WIDTH, decode, frame_number, source, write_recording,
};

fn set_modified(file: &std::path::Path, seconds_ago: u64) {
    let time = SystemTime::now() - Duration::from_secs(seconds_ago);
    fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

#[test]
fn the_library_lists_videos_newest_first_with_length_size_and_date() {
    let folder = TempFolder::new("list");
    let older = folder.file("Recording 2026-11-14 09.00.00.mp4");
    let newer = folder.file("Clip.mp4");
    write_recording(&older, 3);
    write_recording(&newer, 1);
    set_modified(&older, 3_600);
    set_modified(&newer, 60);
    fs::write(folder.file("notes.txt"), "not a video").unwrap();
    fs::write(folder.file(".Clip.mp4.part"), "half written").unwrap();
    fs::write(folder.file("broken.mp4"), "not really a video").unwrap();
    set_modified(&folder.file("broken.mp4"), 7_200);

    let items = scan(folder.path()).unwrap();
    let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, ["Clip", "Recording 2026-11-14 09.00.00", "broken"]);

    let recording = &items[1];
    let info = recording.info.as_ref().unwrap();
    assert_eq!((info.width, info.height), (WIDTH, HEIGHT));
    assert_eq!(info.audio_tracks, 3);
    assert!(info.has_separate_sound());
    let seconds = info.length.as_secs_f64();
    assert!((seconds - 4.0).abs() < 0.1, "{seconds} s");
    assert_eq!(recording.bytes, fs::metadata(&older).unwrap().len());
    assert_eq!(
        recording.modified,
        fs::metadata(&older).unwrap().modified().unwrap()
    );
    assert!(!recording.unfinished);

    assert!(!items[0].info.as_ref().unwrap().has_separate_sound());
    assert_eq!(items[2].info, None);
    assert_eq!(scan(&folder.file("missing")).unwrap(), []);
}

#[test]
fn a_rename_moves_the_file_and_never_replaces_another() {
    let folder = TempFolder::new("rename");
    let first = folder.file("Recording.mp4");
    let second = folder.file("Other.mp4");
    fs::write(&first, "first").unwrap();
    fs::write(&second, "second").unwrap();
    fs::write(journal_path(&first), "1500000\n").unwrap();

    assert!(matches!(rename(&first, "Other"), Err(RenameError::Taken)));
    assert!(matches!(rename(&first, "  "), Err(RenameError::Empty)));
    assert!(matches!(
        rename(&first, "a/b"),
        Err(RenameError::Forbidden('/'))
    ));
    assert!(matches!(
        rename(&first, ".hidden"),
        Err(RenameError::Forbidden('.'))
    ));
    assert_eq!(fs::read_to_string(&second).unwrap(), "second");

    let renamed = rename(&first, " Boss fight.mp4 ").unwrap();
    assert_eq!(renamed, folder.file("Boss fight.mp4"));
    assert!(!first.exists());
    assert_eq!(fs::read_to_string(&renamed).unwrap(), "first");
    assert!(journal_path(&renamed).exists());
    assert_eq!(rename(&renamed, "Boss fight").unwrap(), renamed);
}

#[test]
fn a_thumbnail_is_decoded_once_and_then_read_from_the_cache() {
    let folder = TempFolder::new("thumbnail");
    let video = folder.file("Recording.mp4");
    write_recording(&video, 1);
    let metadata = fs::metadata(&video).unwrap();
    let (bytes, modified) = (metadata.len(), metadata.modified().unwrap());
    let cache = ThumbnailCache::new(folder.file("cache"), (160, 160));
    let length = Duration::from_secs(SECONDS.unsigned_abs());

    let made = cache.thumbnail(&video, bytes, modified, length).unwrap();
    assert_eq!((made.width, made.height), (160, 120));
    assert_eq!(made.rgba.len(), 160 * 120 * 4);
    assert_eq!(frame_number(&made.rgba, made.width, made.height), 0);

    let stored = cache.file_for(&video, bytes, modified);
    assert!(stored.exists());
    fs::remove_file(&video).unwrap();
    let cached = cache.thumbnail(&video, bytes, modified, length).unwrap();
    assert_eq!(cached, made);

    cache.prune(&[]).unwrap();
    assert!(!stored.exists());
}

#[test]
fn a_picture_can_be_taken_at_an_exact_frame() {
    let folder = TempFolder::new("picture");
    let video = folder.file("Recording.mp4");
    write_recording(&video, 1);
    let at = Duration::from_micros(crate::common::micros_of_frame(75).unsigned_abs());
    let exact = picture_at(&video, at, true, (640, 640)).unwrap();
    assert_eq!((exact.width, exact.height), (WIDTH, HEIGHT));
    assert_eq!(frame_number(&exact.rgba, exact.width, exact.height), 75);
    let keyframe = picture_at(&video, at, false, (640, 640)).unwrap();
    assert_eq!(
        frame_number(&keyframe.rgba, keyframe.width, keyframe.height),
        60
    );
}

#[test]
fn a_recording_cut_off_by_a_crash_is_finished_into_a_normal_file_with_its_markers() {
    let folder = TempFolder::new("recover");
    let writing = folder.file(".writing.mp4");
    let crashed = folder.file("Recording.mp4");
    let source = source(3);
    let mut writer = Mp4Writer::create(&writing, &source.streams, Mp4Layout::Recording).unwrap();
    let most = source.packets.len() * 4 / 5;
    for packet in &source.packets[..most] {
        writer.write(packet).unwrap();
    }
    fs::copy(&writing, &crashed).unwrap();
    drop(writer);
    fs::remove_file(&writing).unwrap();
    fs::write(journal_path(&crashed), "1500000\n2250000\n").unwrap();
    let finished = folder.file("Finished.mp4");
    write_recording(&finished, 1);

    assert!(is_unfinished(&crashed).unwrap());
    assert!(!is_unfinished(&finished).unwrap());
    assert!(
        scan(folder.path())
            .unwrap()
            .iter()
            .any(|item| item.unfinished)
    );
    let before = decode(&crashed).frame_micros.len();
    assert!(before >= 60, "only {before} frames were on disk");

    let left_alone = finish_unfinished_recordings(folder.path(), Duration::from_secs(3_600));
    assert!(left_alone.is_empty());
    assert!(is_unfinished(&crashed).unwrap());

    let recovered = finish_unfinished_recordings(folder.path(), Duration::ZERO);
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].recording, crashed);
    recovered[0].outcome.as_ref().unwrap();

    assert!(!is_unfinished(&crashed).unwrap());
    assert!(!journal_path(&crashed).exists());
    assert_eq!(
        read_markers(&crashed).unwrap(),
        [Duration::from_millis(1_500), Duration::from_millis(2_250)]
    );
    let after = decode(&crashed);
    assert_eq!(after.frame_micros.len(), before);
    assert_eq!(after.audio_tracks, 3);
    assert_eq!(after.frame_numbers[0], 0);
    assert_eq!(
        after.frame_numbers[before - 1],
        i64::try_from(before).unwrap() - 1
    );
    let info = probe(&crashed).unwrap();
    let expected = crate::common::float(before) / 30.0;
    assert!((info.length.as_secs_f64() - expected).abs() < 0.1);
    let leftovers: Vec<_> = fs::read_dir(folder.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".part"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert_eq!(
        scan(folder.path())
            .unwrap()
            .iter()
            .filter(|item| item.markers == 2)
            .count(),
        1
    );
}
