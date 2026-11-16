use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use proptest::prelude::*;

use super::disk::{DiskBackend, Sink};
use super::session::{OWNER_LOCK, ROOT_DIRECTORY, ROOT_LOCK};
use super::{
    AUDIO_PRE_ROLL, AfterSave, BackendChoice, BackendKind, EpochId, Packet, PushOutcome,
    RemovedLeftovers, ReplayBuffer, ReplayConfig, ReplayError, ReplayLength, ReplaySave,
    ReplaySlice, SaveRange, StreamIndex, Ticks, choose_backend, expected_size_bytes,
    installed_memory_bytes, memory_budget_bytes, remove_leftovers,
};

const CASES_PER_PROPERTY: u32 = 96;
const BOTH_BACKENDS: [BackendChoice; 2] = [BackendChoice::Memory, BackendChoice::Disk];
const LENGTH: Ticks = Ticks::from_secs(15);

struct TempFolder(PathBuf);

impl TempFolder {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ngine-recorder-test-{}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn root(&self) -> PathBuf {
        self.0.join(ROOT_DIRECTORY)
    }

    fn names_under_root(&self) -> Vec<String> {
        names_in(&self.root())
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.0));
    }
}

fn names_in(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn new_buffer(choice: BackendChoice, folder: &TempFolder) -> ReplayBuffer {
    ReplayBuffer::new(&ReplayConfig {
        length: ReplayLength::SHORTEST,
        max_bitrate_bits_per_second: 8_000_000,
        backend: choice,
        disk_folder: folder.0.clone(),
    })
    .unwrap()
}

fn assert_nothing_left_on_disk(choice: BackendChoice, folder: &TempFolder) {
    if choice == BackendChoice::Disk {
        assert_eq!(folder.names_under_root(), [ROOT_LOCK]);
    }
}

#[derive(Debug, Clone)]
struct Stream {
    packets: Vec<Packet>,
    longest_keyframe_interval: Ticks,
}

fn build_stream(
    frame_millis: i64,
    keyframe_gaps: &[usize],
    audio_tracks: u8,
    audio_millis: i64,
    audio_lead_millis: &[i64],
    first_pts_millis: i64,
    payload_seed: usize,
) -> Stream {
    let frame = Ticks::from_millis(frame_millis);
    let first_pts = Ticks::from_millis(first_pts_millis);
    let mut arrivals = Vec::new();
    let mut pts = first_pts;
    for &gap in keyframe_gaps {
        for position in 0..gap {
            arrivals.push((pts, StreamIndex::VIDEO, pts, frame, position == 0));
            pts = pts + frame;
        }
    }
    let video_end = pts;
    let audio_duration = Ticks::from_millis(audio_millis);
    for track in 0..audio_tracks {
        let stream = StreamIndex::audio(track).unwrap();
        let lead = Ticks::from_millis(audio_lead_millis[usize::from(track)]);
        let mut pts = first_pts - Ticks::from_millis(300);
        while pts < video_end {
            arrivals.push((pts - lead, stream, pts, audio_duration, true));
            pts = pts + audio_duration;
        }
    }
    arrivals.sort_by_key(|&(arrival, stream, ..)| (arrival, stream));
    let packets = arrivals
        .into_iter()
        .enumerate()
        .map(|(number, (_, stream, pts, duration, keyframe))| Packet {
            stream,
            pts,
            dts: pts,
            duration,
            keyframe,
            data: payload(number, payload_seed),
        })
        .collect();
    let longest_gap = keyframe_gaps.iter().copied().max().unwrap_or(1);
    Stream {
        packets,
        longest_keyframe_interval: Ticks(frame.0 * i64::try_from(longest_gap).unwrap()),
    }
}

fn payload(number: usize, seed: usize) -> Arc<[u8]> {
    let filler = (seed.wrapping_add(number.wrapping_mul(7919))) % 40;
    let mut data = number.to_le_bytes().to_vec();
    data.extend(std::iter::repeat_n(0xA5, filler));
    data.into()
}

prop_compose! {
    fn stream()(
        frame_millis in 100i64..=400,
        keyframe_gaps in prop::collection::vec(1usize..=12, 30..80),
        audio_tracks in 0u8..=3,
        audio_millis in 20i64..=120,
        audio_lead_millis in prop::collection::vec(-150i64..=150, 3),
        first_pts_millis in 0i64..=5000,
        payload_seed in 0usize..1000,
    ) -> Stream {
        build_stream(
            frame_millis,
            &keyframe_gaps,
            audio_tracks,
            audio_millis,
            &audio_lead_millis,
            first_pts_millis,
            payload_seed,
        )
    }
}

fn steady_stream(seconds: usize) -> Stream {
    build_stream(100, &vec![10; seconds], 2, 20, &[30, -30, 0], 1000, 7)
}

#[derive(Debug, Clone, Default)]
struct Reference {
    runs: Vec<Vec<Packet>>,
    epochs: Vec<EpochId>,
    run_is_open: bool,
}

impl Reference {
    fn push(&mut self, buffer: &mut ReplayBuffer, packet: &Packet) {
        match buffer.push(packet.clone()) {
            Ok(PushOutcome::Stored) => self.stored(packet, buffer.current_epoch()),
            Ok(PushOutcome::WaitingForKeyframe) => assert!(!self.run_is_open),
            Err(error) => {
                assert!(matches!(error, ReplayError::Storage { .. }), "{error}");
                self.lost_newest_group();
            }
        }
    }

    fn stored(&mut self, packet: &Packet, epoch: EpochId) {
        if !self.run_is_open {
            self.runs.push(Vec::new());
            self.epochs.push(epoch);
            self.run_is_open = true;
        }
        self.runs.last_mut().unwrap().push(packet.clone());
    }

    fn end_run(&mut self) {
        self.run_is_open = false;
    }

    fn lost_newest_group(&mut self) {
        if !self.run_is_open {
            return;
        }
        let run = self.runs.last_mut().unwrap();
        let newest_keyframe = run.iter().rposition(is_video_keyframe).unwrap();
        run.truncate(newest_keyframe);
        if run.is_empty() {
            self.runs.pop();
            self.epochs.pop();
        }
        self.run_is_open = false;
    }
}

fn is_video_keyframe(packet: &Packet) -> bool {
    packet.stream.is_video() && packet.keyframe
}

fn video_end(packets: &[Packet]) -> Ticks {
    packets
        .iter()
        .filter(|packet| packet.stream.is_video())
        .map(Packet::end)
        .max()
        .unwrap()
}

fn read(slice: &ReplaySlice) -> Vec<Packet> {
    slice.packets().collect::<Result<_, _>>().unwrap()
}

fn bytes_of(packets: &[Packet]) -> u64 {
    packets.iter().map(Packet::size_bytes).sum()
}

fn assert_save_matches(reference: &Reference, save: &ReplaySave) {
    let slices = save.slices();
    assert!(slices.len() <= reference.runs.len());
    assert!(slices.is_sorted_by_key(ReplaySlice::epoch));
    let oldest_saved_run = reference.runs.len() - slices.len();
    let runs = &reference.runs[oldest_saved_run..];
    for (position, (slice, run)) in slices.iter().zip(runs).enumerate() {
        assert_eq!(slice.epoch(), reference.epochs[oldest_saved_run + position]);
        let packets = read(slice);
        let first_kept = run
            .iter()
            .position(|packet| is_video_keyframe(packet) && packet.pts == slice.start())
            .expect("a slice starts on a stored video keyframe");
        if position > 0 {
            assert_eq!(first_kept, 0, "only the oldest slice may start mid-run");
        }
        let too_old = slice.start() - AUDIO_PRE_ROLL;
        let mut expected: Vec<Packet> = run[..first_kept]
            .iter()
            .filter(|packet| !packet.stream.is_video() && packet.end() > too_old)
            .cloned()
            .collect();
        assert_eq!(slice.pre_roll.len(), expected.len());
        expected.extend_from_slice(&run[first_kept..]);
        assert!(packets == expected, "a slice holds exactly its range");
        assert_eq!(slice.packet_count(), packets.len());
        assert_eq!(slice.size_bytes(), bytes_of(&packets));
        assert_eq!(slice.duration(), video_end(run) - slice.start());
        assert_eq!(read(slice).len(), packets.len(), "a slice reads twice");
    }
}

fn held_bytes(save: &ReplaySave) -> u64 {
    save.slices()
        .iter()
        .map(|slice| slice.size_bytes() - bytes_of(&slice.pre_roll))
        .sum()
}

fn starts_on_a_keyframe_drops_whole_groups_and_stays_full(choice: BackendChoice, stream: &Stream) {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(choice, &folder);
    let mut reference = Reference::default();
    let mut was_full = false;
    let last = stream.packets.len() - 1;
    for (number, packet) in stream.packets.iter().enumerate() {
        reference.push(&mut buffer, packet);
        let covered = buffer.covered_duration();
        assert!(!was_full || covered >= LENGTH, "a full buffer went short");
        assert!(covered < LENGTH + stream.longest_keyframe_interval);
        was_full |= covered >= LENGTH;
        if number % 61 == 0 || number == last {
            let everything = buffer
                .save(SaveRange::Everything, AfterSave::KeepBuffer)
                .unwrap();
            assert_save_matches(&reference, &everything);
            assert_eq!(everything.duration(), covered);
            assert_eq!(held_bytes(&everything), buffer.size_bytes());
        }
    }
    drop(buffer);
    assert_nothing_left_on_disk(choice, &folder);
}

fn save_of_the_last_stretch_is_long_enough(choice: BackendChoice, stream: &Stream, millis: i64) {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(choice, &folder);
    let mut reference = Reference::default();
    for packet in &stream.packets {
        reference.push(&mut buffer, packet);
    }
    let asked = Ticks::from_millis(millis);
    let covered = buffer.covered_duration();
    let save = buffer
        .save(
            SaveRange::Last(Duration::from_millis(millis.unsigned_abs())),
            AfterSave::KeepBuffer,
        )
        .unwrap();
    assert_save_matches(&reference, &save);
    assert!(save.slices().len() <= 1);
    if covered >= asked {
        assert!(save.duration() >= asked);
        assert!(save.duration() < asked + stream.longest_keyframe_interval);
    } else {
        assert_eq!(save.duration(), covered);
    }
}

fn save_survives_pushes_and_evictions(
    choice: BackendChoice,
    stream: &Stream,
    saved_after: usize,
    range: SaveRange,
) {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(choice, &folder);
    let mut reference = Reference::default();
    let (before, after) = stream
        .packets
        .split_at(saved_after.min(stream.packets.len()));
    for packet in before {
        reference.push(&mut buffer, packet);
    }
    let save = buffer.save(range, AfterSave::KeepBuffer).unwrap();
    let reference_at_save = reference.clone();
    let read_meanwhile = thread::scope(|scope| {
        let reader = scope.spawn(|| save.slices().iter().map(read).collect::<Vec<_>>());
        for packet in after {
            reference.push(&mut buffer, packet);
        }
        buffer.begin_epoch().unwrap();
        reference.end_run();
        for packet in &steady_stream(40).packets {
            reference.push(&mut buffer, packet);
        }
        reader.join().unwrap()
    });
    assert_save_matches(&reference_at_save, &save);
    let read_afterwards: Vec<_> = save.slices().iter().map(read).collect();
    assert_eq!(read_meanwhile, read_afterwards);
    drop(buffer);
    assert_save_matches(&reference_at_save, &save);
    drop(save);
    assert_nothing_left_on_disk(choice, &folder);
}

fn epochs_are_never_mixed(
    choice: BackendChoice,
    streams: &[Stream],
    range: SaveRange,
    after: AfterSave,
) {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(choice, &folder);
    let mut reference = Reference::default();
    for (epoch, stream) in streams.iter().enumerate() {
        if epoch > 0 {
            assert_eq!(
                buffer.begin_epoch().unwrap().0,
                u64::try_from(epoch).unwrap()
            );
            reference.end_run();
        }
        for packet in &stream.packets {
            reference.push(&mut buffer, packet);
        }
    }
    let covered = buffer.covered_duration();
    let longest_keyframe_interval = streams
        .iter()
        .map(|stream| stream.longest_keyframe_interval)
        .max()
        .unwrap();
    assert!(covered < LENGTH + longest_keyframe_interval);
    let save = buffer.save(range, after).unwrap();
    assert_save_matches(&reference, &save);
    assert!(
        save.slices()
            .windows(2)
            .all(|pair| pair[0].epoch() < pair[1].epoch()),
        "every slice has an epoch of its own"
    );
    match range {
        SaveRange::Last(asked) => {
            let asked = Ticks::from_duration_rounded_up(asked);
            if covered >= asked {
                assert!(save.duration() >= asked);
                assert!(save.duration() < asked + longest_keyframe_interval);
            } else {
                assert_eq!(save.duration(), covered);
            }
        }
        SaveRange::Everything => assert_eq!(save.duration(), covered),
    }
    if after == AfterSave::ClearBuffer {
        assert_eq!(buffer.covered_duration(), Ticks::ZERO);
        assert_eq!(buffer.size_bytes(), 0);
        let nothing = buffer
            .save(SaveRange::Everything, AfterSave::KeepBuffer)
            .unwrap();
        assert!(nothing.is_empty());
        assert_save_matches(&reference, &save);
    }
}

fn short_stream() -> impl Strategy<Value = Stream> {
    (stream(), 1usize..400).prop_map(|(mut stream, kept)| {
        stream.packets.truncate(kept);
        stream
    })
}

fn save_range() -> impl Strategy<Value = SaveRange> {
    prop_oneof![
        Just(SaveRange::Everything),
        (0u64..=25_000).prop_map(|millis| SaveRange::Last(Duration::from_millis(millis))),
    ]
}

fn after_save() -> impl Strategy<Value = AfterSave> {
    prop_oneof![Just(AfterSave::KeepBuffer), Just(AfterSave::ClearBuffer)]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: CASES_PER_PROPERTY,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn buffer_starts_on_a_keyframe_drops_whole_groups_and_stays_full(stream in stream()) {
        for choice in BOTH_BACKENDS {
            starts_on_a_keyframe_drops_whole_groups_and_stays_full(choice, &stream);
        }
    }

    #[test]
    fn save_of_the_last_stretch_covers_it_and_less_than_a_keyframe_interval_more(
        stream in stream(),
        millis in 0i64..=25_000,
    ) {
        for choice in BOTH_BACKENDS {
            save_of_the_last_stretch_is_long_enough(choice, &stream, millis);
        }
    }

    #[test]
    fn save_taken_mid_stream_holds_exactly_its_range_after_everything_was_evicted(
        stream in stream(),
        saved_after in 0usize..3000,
        range in save_range(),
    ) {
        for choice in BOTH_BACKENDS {
            save_survives_pushes_and_evictions(choice, &stream, saved_after, range);
        }
    }

    #[test]
    fn save_across_an_epoch_boundary_gives_one_slice_per_epoch_newest_last(
        streams in prop::collection::vec(prop_oneof![stream(), short_stream()], 2..=3),
        range in save_range(),
        after in after_save(),
    ) {
        for choice in BOTH_BACKENDS {
            epochs_are_never_mixed(choice, &streams, range, after);
        }
    }

    #[test]
    fn backend_changes_at_the_budget(expected in 1u64..=u64::MAX - 1, budget in any::<u64>()) {
        let auto = |memory_budget_bytes| BackendChoice::Auto { memory_budget_bytes };
        let chosen = choose_backend(expected, auto(budget));
        prop_assert_eq!(chosen == BackendKind::Memory, expected <= budget);
        prop_assert_eq!(choose_backend(expected, auto(expected)), BackendKind::Memory);
        prop_assert_eq!(choose_backend(expected, auto(expected - 1)), BackendKind::Disk);
        prop_assert_eq!(choose_backend(expected, BackendChoice::Memory), BackendKind::Memory);
        prop_assert_eq!(choose_backend(expected, BackendChoice::Disk), BackendKind::Disk);
    }

    #[test]
    fn budget_is_three_percent_between_its_floor_and_ceiling(installed in any::<u64>()) {
        let budget = memory_budget_bytes(installed);
        prop_assert!((256 * MEBIBYTE..=2048 * MEBIBYTE).contains(&budget));
        if (256 * MEBIBYTE + 1..2048 * MEBIBYTE).contains(&budget) {
            prop_assert_eq!(budget, installed / 100 * 3);
        }
    }
}

const MEBIBYTE: u64 = 1024 * 1024;
const GIBIBYTE: u64 = 1024 * MEBIBYTE;

#[test]
fn budget_for_common_machines() {
    assert_eq!(memory_budget_bytes(8 * GIBIBYTE), 256 * MEBIBYTE);
    assert_eq!(memory_budget_bytes(16 * GIBIBYTE) / MEBIBYTE, 491);
    assert_eq!(memory_budget_bytes(32 * GIBIBYTE) / MEBIBYTE, 983);
    assert_eq!(memory_budget_bytes(128 * GIBIBYTE), 2 * GIBIBYTE);
    assert_eq!(memory_budget_bytes(16_000_000_000), 480_000_000);
    assert_eq!(memory_budget_bytes(32_000_000_000), 960_000_000);
    assert_eq!(memory_budget_bytes(0), 256 * MEBIBYTE);
}

#[test]
fn this_machine_reports_its_installed_memory() {
    assert!(installed_memory_bytes() > 256 * MEBIBYTE);
}

#[test]
fn buffer_is_built_in_memory_at_the_budget_and_on_disk_one_byte_over() {
    let folder = TempFolder::new();
    let length = ReplayLength::DEFAULT;
    let bitrate = 40_000_000;
    let expected = expected_size_bytes(bitrate, length);
    assert_eq!(expected, 300_000_000);
    let build = |memory_budget_bytes| {
        ReplayBuffer::new(&ReplayConfig {
            length,
            max_bitrate_bits_per_second: bitrate,
            backend: BackendChoice::Auto {
                memory_budget_bytes,
            },
            disk_folder: folder.0.clone(),
        })
        .unwrap()
        .backend_kind()
    };
    assert_eq!(build(expected), BackendKind::Memory);
    assert!(!folder.root().exists());
    assert_eq!(build(expected - 1), BackendKind::Disk);
    assert_eq!(folder.names_under_root(), [ROOT_LOCK]);
}

#[test]
fn length_is_fifteen_seconds_to_thirty_minutes_and_one_minute_by_default() {
    assert_eq!(ReplayLength::default().get(), Duration::from_secs(60));
    assert!(ReplayLength::new(Duration::from_secs(15)).is_ok());
    assert!(ReplayLength::new(Duration::from_mins(30)).is_ok());
    assert!(ReplayLength::new(Duration::from_millis(14_999)).is_err());
    assert!(ReplayLength::new(Duration::from_millis(1_800_001)).is_err());
}

#[test]
fn ticks_convert_without_shortening_a_request() {
    assert_eq!(
        Ticks::from_duration_rounded_up(Duration::from_secs(30)),
        Ticks::from_secs(30)
    );
    assert_eq!(
        Ticks::from_duration_rounded_up(Duration::from_nanos(1)),
        Ticks(1)
    );
    assert_eq!(Ticks::from_secs(30).to_duration(), Duration::from_secs(30));
    assert_eq!(Ticks(-5).to_duration(), Duration::ZERO);
    assert_eq!(Ticks::PER_SECOND % 90_000, 0);
    assert_eq!(Ticks::PER_SECOND % 48_000, 0);
    assert_eq!(Ticks::PER_SECOND % 44_100, 0);
}

#[test]
fn there_are_one_video_and_three_audio_streams() {
    assert_eq!(StreamIndex::VIDEO.get(), 0);
    assert_eq!(StreamIndex::audio(2).map(StreamIndex::get), Some(3));
    assert_eq!(StreamIndex::audio(3), None);
}

#[test]
fn buffer_moves_to_the_encoder_thread_and_a_save_to_any_thread() {
    fn moves_between_threads<T: Send>() {}
    fn is_shared_between_threads<T: Send + Sync>() {}
    moves_between_threads::<ReplayBuffer>();
    is_shared_between_threads::<ReplaySave>();
}

#[test]
fn packets_that_break_the_timeline_are_refused_and_the_buffer_goes_on() {
    for choice in BOTH_BACKENDS {
        let folder = TempFolder::new();
        let mut buffer = new_buffer(choice, &folder);
        let mut reference = Reference::default();
        let stream = steady_stream(3);
        let (before, after) = stream.packets.split_at(stream.packets.len() / 2);
        for packet in before {
            reference.push(&mut buffer, packet);
        }
        let first_keyframe = stream
            .packets
            .iter()
            .find(|packet| is_video_keyframe(packet));
        let repeated = buffer.push(first_keyframe.unwrap().clone());
        assert!(matches!(
            repeated,
            Err(ReplayError::KeyframeNotAfterPrevious { .. })
        ));
        let negative = Packet {
            duration: Ticks(-1),
            ..before[0].clone()
        };
        assert!(matches!(
            buffer.push(negative),
            Err(ReplayError::NegativeDuration { .. })
        ));
        for packet in after {
            reference.push(&mut buffer, packet);
        }
        let everything = buffer
            .save(SaveRange::Everything, AfterSave::KeepBuffer)
            .unwrap();
        assert_save_matches(&reference, &everything);
        assert_eq!(everything.slices().len(), 1);
    }
}

fn crashed_session(root: &Path, name: &str, files: &[(&str, usize)]) -> PathBuf {
    let directory = root.join(name);
    fs::create_dir(&directory).unwrap();
    for &(file, bytes) in files {
        fs::write(directory.join(file), vec![0; bytes]).unwrap();
    }
    directory
}

#[test]
fn crash_leftovers_are_removed_and_nothing_else_is() {
    let folder = TempFolder::new();
    let mut live = new_buffer(BackendChoice::Disk, &folder);
    let mut reference = Reference::default();
    let stream = steady_stream(6);
    let (before, after) = stream.packets.split_at(stream.packets.len() / 2);
    for packet in before {
        reference.push(&mut live, packet);
    }
    let root = folder.root();
    let live_session = root.join(
        folder
            .names_under_root()
            .into_iter()
            .find(|name| name.starts_with("session-"))
            .unwrap(),
    );
    let live_files = names_in(&live_session);
    assert!(live_files.len() > 2);

    let crashed = crashed_session(
        &root,
        "session-4242-1-0",
        &[(OWNER_LOCK, 0), ("group-0.bin", 10), ("group-17.bin", 5)],
    );
    let crashed_while_starting = crashed_session(&root, "session-4242-2-0", &[("group-0.bin", 3)]);
    let crashed_with_a_guest = crashed_session(
        &root,
        "session-4242-3-0",
        &[(OWNER_LOCK, 0), ("group-0.bin", 4), ("holiday.mp4", 9)],
    );
    let not_a_session = crashed_session(&root, "session-backup", &[("group-0.bin", 2)]);
    fs::write(root.join("notes.txt"), "mine").unwrap();
    fs::write(root.join("session-4242-9-0"), "a file, not a folder").unwrap();
    fs::write(folder.0.join("clip.mp4"), "a saved clip").unwrap();

    let removed = remove_leftovers(&folder.0).unwrap();

    assert_eq!(
        removed,
        RemovedLeftovers {
            sessions: 2,
            files: 4,
            bytes: 22,
        }
    );
    assert!(!crashed.exists());
    assert!(!crashed_while_starting.exists());
    assert_eq!(names_in(&crashed_with_a_guest), ["holiday.mp4"]);
    assert_eq!(names_in(&not_a_session), ["group-0.bin"]);
    assert!(root.join("notes.txt").exists());
    assert!(root.join("session-4242-9-0").is_file());
    assert!(folder.0.join("clip.mp4").exists());
    assert_eq!(names_in(&live_session), live_files);

    let crashed_again = crashed_session(&root, "session-4242-4-0", &[("group-3.bin", 1)]);
    let second_instance = new_buffer(BackendChoice::Disk, &folder);
    assert!(!crashed_again.exists());
    assert_eq!(names_in(&live_session), live_files);
    assert_eq!(
        remove_leftovers(&folder.0).unwrap(),
        RemovedLeftovers::default()
    );
    drop(second_instance);

    for packet in after {
        reference.push(&mut live, packet);
    }
    let everything = live
        .save(SaveRange::Everything, AfterSave::KeepBuffer)
        .unwrap();
    assert_save_matches(&reference, &everything);
    drop(everything);
    drop(live);
    assert!(!live_session.exists());
}

#[test]
fn leftovers_of_a_folder_without_replay_files_are_none() {
    let folder = TempFolder::new();
    assert_eq!(
        remove_leftovers(&folder.0).unwrap(),
        RemovedLeftovers::default()
    );
    assert_eq!(
        remove_leftovers(&folder.0.join("missing")).unwrap(),
        RemovedLeftovers::default()
    );
    assert_eq!(names_in(&folder.0), Vec::<String>::new());
}

#[test]
fn disk_buffer_in_a_missing_folder_is_an_error() {
    let folder = TempFolder::new();
    let built = ReplayBuffer::new(&ReplayConfig {
        length: ReplayLength::SHORTEST,
        max_bitrate_bits_per_second: 8_000_000,
        backend: BackendChoice::Disk,
        disk_folder: folder.0.join("unplugged").join("drive"),
    });
    assert!(matches!(built, Err(ReplayError::Storage { .. })));
}

struct SinkOnAFailingDisk {
    file: File,
    disk_works: Arc<AtomicBool>,
}

fn disk_full() -> io::Error {
    io::Error::new(io::ErrorKind::StorageFull, "injected: no space left")
}

impl Write for SinkOnAFailingDisk {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.disk_works.load(Ordering::Relaxed) {
            self.file.write(bytes)
        } else {
            Err(disk_full())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.disk_works.load(Ordering::Relaxed) {
            self.file.flush()
        } else {
            Err(disk_full())
        }
    }
}

fn buffer_on_a_disk_that_can_fail(folder: &TempFolder) -> (ReplayBuffer, Arc<AtomicBool>) {
    let disk_works = Arc::new(AtomicBool::new(true));
    let seen_by_sink = Arc::clone(&disk_works);
    let backend = DiskBackend::with_sink(
        &folder.0,
        Box::new(move |path| {
            if !seen_by_sink.load(Ordering::Relaxed) {
                return Err(disk_full());
            }
            let sink: Sink = Box::new(SinkOnAFailingDisk {
                file: File::create_new(path)?,
                disk_works: Arc::clone(&seen_by_sink),
            });
            Ok(sink)
        }),
    )
    .unwrap();
    (
        ReplayBuffer::with_backend(ReplayLength::SHORTEST, Box::new(backend)),
        disk_works,
    )
}

#[test]
fn failed_write_is_an_error_that_keeps_older_packets_and_earlier_saves() {
    let folder = TempFolder::new();
    let (mut buffer, disk_works) = buffer_on_a_disk_that_can_fail(&folder);
    let mut reference = Reference::default();
    let stream = steady_stream(12);
    let thirds = stream.packets.len() / 3;
    for packet in &stream.packets[..thirds] {
        reference.push(&mut buffer, packet);
    }
    let saved_before = buffer
        .save(SaveRange::Everything, AfterSave::KeepBuffer)
        .unwrap();
    let reference_before = reference.clone();
    let size_before = buffer.size_bytes();

    disk_works.store(false, Ordering::Relaxed);
    let larger_than_the_write_buffer = Packet {
        keyframe: false,
        data: vec![7; 100 * 1024].into(),
        ..stream.packets[thirds].clone()
    };
    let failed = buffer.push(larger_than_the_write_buffer);
    assert!(
        matches!(&failed, Err(ReplayError::Storage { source, .. })
            if source.kind() == io::ErrorKind::StorageFull),
        "{failed:?}"
    );
    reference.lost_newest_group();
    assert!(buffer.size_bytes() < size_before);

    let mut errors = 0;
    for packet in &stream.packets[thirds..2 * thirds] {
        match buffer.push(packet.clone()) {
            Ok(outcome) => assert_eq!(outcome, PushOutcome::WaitingForKeyframe),
            Err(error) => {
                assert!(matches!(error, ReplayError::Storage { .. }));
                assert!(is_video_keyframe(packet));
                errors += 1;
            }
        }
    }
    assert!(errors > 0);
    assert_save_matches(&reference_before, &saved_before);

    disk_works.store(true, Ordering::Relaxed);
    for packet in &stream.packets[2 * thirds..] {
        reference.push(&mut buffer, packet);
    }
    let everything = buffer
        .save(SaveRange::Everything, AfterSave::KeepBuffer)
        .unwrap();
    assert_save_matches(&reference, &everything);
    assert_eq!(everything.slices().len(), 2);
    assert_eq!(
        everything.slices()[0].epoch(),
        everything.slices()[1].epoch()
    );
    assert_eq!(held_bytes(&everything), buffer.size_bytes());
    assert_save_matches(&reference_before, &saved_before);
}

#[test]
fn failed_flush_at_a_keyframe_or_a_save_loses_only_the_newest_group() {
    let folder = TempFolder::new();
    let (mut buffer, disk_works) = buffer_on_a_disk_that_can_fail(&folder);
    let mut reference = Reference::default();
    let stream = steady_stream(8);
    let half = stream.packets.len() / 2;
    for packet in &stream.packets[..half] {
        reference.push(&mut buffer, packet);
    }

    disk_works.store(false, Ordering::Relaxed);
    let failed_save = buffer.save(SaveRange::Everything, AfterSave::KeepBuffer);
    assert!(matches!(failed_save, Err(ReplayError::Storage { .. })));
    reference.lost_newest_group();
    let what_is_left = buffer
        .save(SaveRange::Everything, AfterSave::KeepBuffer)
        .unwrap();
    assert_save_matches(&reference, &what_is_left);
    assert!(!what_is_left.is_empty());

    disk_works.store(true, Ordering::Relaxed);
    let mut failed_at_a_keyframe = false;
    for packet in &stream.packets[half..] {
        if is_video_keyframe(packet) && reference.run_is_open && !failed_at_a_keyframe {
            disk_works.store(false, Ordering::Relaxed);
            assert!(matches!(
                buffer.push(packet.clone()),
                Err(ReplayError::Storage { .. })
            ));
            reference.lost_newest_group();
            disk_works.store(true, Ordering::Relaxed);
            failed_at_a_keyframe = true;
        } else {
            reference.push(&mut buffer, packet);
        }
    }
    assert!(failed_at_a_keyframe);
    assert!(buffer.begin_epoch().is_ok());
    let everything = buffer
        .save(SaveRange::Everything, AfterSave::KeepBuffer)
        .unwrap();
    assert_save_matches(&reference, &everything);
    assert_eq!(everything.slices().len(), 2);
    assert_eq!(held_bytes(&everything), buffer.size_bytes());
}
