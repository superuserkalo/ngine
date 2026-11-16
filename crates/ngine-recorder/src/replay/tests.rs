use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use proptest::prelude::*;

use super::disk::{DiskStore, Sink};
use super::session::{
    OWNER_LOCK, OwnerLock, OwnerProcess, ROOT_DIRECTORY, SessionName, is_proven_dead,
    owner_process, remove_dead_sessions, this_host,
};
use super::unit::StoredUnit;
use super::{
    AUDIO_PRE_ROLL, AfterSave, BackendChoice, BackendKind, EpochId, LONGEST_KEYFRAME_SNAP, Packet,
    PushOutcome, RemovedLeftovers, ReplayBuffer, ReplayConfig, ReplayError, ReplayLength,
    ReplaySave, ReplaySlice, SaveRange, StreamIndex, Ticks, choose_backend, expected_size_bytes,
    installed_memory_bytes, memory_budget_bytes, remove_leftovers,
};

const CASES_PER_PROPERTY: u32 = 64;
const LENGTH: Ticks = Ticks::from_secs(15);
const SMALL_MEMORY_BUDGET_BYTES: u64 = 20_000;
const HELPER_FOLDER_VARIABLE: &str = "NGINE_REPLAY_TEST_HELPER_FOLDER";
const HELPER_TEST: &str = "replay::tests::helper_process_holds_a_disk_buffer_until_killed";
const HELPER_IS_HOLDING: &str = "helper holds its buffer";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    Memory,
    Disk,
    MemoryThatMovesToDisk,
}

const EVERY_SETUP: [Setup; 3] = [Setup::Memory, Setup::Disk, Setup::MemoryThatMovesToDisk];

fn config(setup: Setup, folder: &TempFolder) -> ReplayConfig {
    let (backend, disk_folder) = match setup {
        Setup::Memory => (BackendChoice::Memory, None),
        Setup::Disk => (BackendChoice::Disk, Some(folder.0.clone())),
        Setup::MemoryThatMovesToDisk => (
            BackendChoice::Auto {
                memory_budget_bytes: SMALL_MEMORY_BUDGET_BYTES,
            },
            Some(folder.0.clone()),
        ),
    };
    let fits_the_small_budget = 8_000;
    ReplayConfig {
        length: ReplayLength::SHORTEST,
        max_bitrate_bits_per_second: if setup == Setup::MemoryThatMovesToDisk {
            fits_the_small_budget
        } else {
            8_000_000
        },
        backend,
        disk_folder,
    }
}

fn new_buffer(setup: Setup, folder: &TempFolder) -> ReplayBuffer {
    ReplayBuffer::new(&config(setup, folder)).unwrap()
}

fn assert_nothing_left_on_disk(folder: &TempFolder) {
    if folder.root().exists() {
        assert_eq!(names_in(&folder.root()), Vec::<String>::new());
    }
}

#[derive(Debug, Clone)]
struct Stream {
    packets: Vec<Packet>,
}

#[derive(Debug, Clone)]
struct Shape {
    frame_millis: i64,
    keyframe_gaps: Vec<usize>,
    audio_tracks: u8,
    audio_millis: i64,
    audio_lead_millis: Vec<i64>,
    first_pts_millis: i64,
    payload_bytes: usize,
    b_frames: bool,
    zero_durations: bool,
}

fn decode_order(gap: usize, b_frames: bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..gap).collect();
    if b_frames {
        for pair in order[1..].as_chunks_mut::<2>().0 {
            pair.swap(0, 1);
        }
    }
    order
}

fn build_stream(shape: &Shape) -> Stream {
    let frame = Ticks::from_millis(shape.frame_millis);
    let first_pts = Ticks::from_millis(shape.first_pts_millis);
    let duration_of = |real: Ticks| {
        if shape.zero_durations {
            Ticks::ZERO
        } else {
            real
        }
    };
    let mut arrivals = Vec::new();
    let mut group_start = first_pts;
    for &gap in &shape.keyframe_gaps {
        for (slot, position) in decode_order(gap, shape.b_frames).into_iter().enumerate() {
            let decoded_at = group_start + Ticks(frame.0 * i64::try_from(slot).unwrap());
            let pts = group_start + Ticks(frame.0 * i64::try_from(position).unwrap());
            let dts = decoded_at - frame - frame;
            arrivals.push((
                decoded_at,
                StreamIndex::VIDEO,
                pts,
                dts,
                frame,
                position == 0,
            ));
        }
        group_start = group_start + Ticks(frame.0 * i64::try_from(gap).unwrap());
    }
    let video_end = group_start;
    let audio_duration = Ticks::from_millis(shape.audio_millis);
    for track in 0..shape.audio_tracks {
        let stream = StreamIndex::audio(track).unwrap();
        let lead = Ticks::from_millis(shape.audio_lead_millis[usize::from(track)]);
        let mut pts = first_pts - Ticks::from_millis(300);
        while pts < video_end {
            arrivals.push((pts - lead, stream, pts, pts, audio_duration, true));
            pts = pts + audio_duration;
        }
    }
    arrivals.sort_by_key(|&(arrival, stream, ..)| (arrival, stream));
    let packets = arrivals
        .into_iter()
        .enumerate()
        .map(
            |(number, (_, stream, pts, dts, duration, keyframe))| Packet {
                stream,
                pts,
                dts,
                duration: duration_of(duration),
                keyframe,
                data: payload(number, shape.payload_bytes),
            },
        )
        .collect();
    Stream { packets }
}

fn payload(number: usize, bytes: usize) -> Arc<[u8]> {
    let filler = bytes + number.wrapping_mul(7919) % 40;
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
        payload_bytes in 0usize..60,
        b_frames in any::<bool>(),
        zero_durations in prop::bool::weighted(0.2),
    ) -> Stream {
        build_stream(&Shape {
            frame_millis,
            keyframe_gaps,
            audio_tracks,
            audio_millis,
            audio_lead_millis,
            first_pts_millis,
            payload_bytes,
            b_frames,
            zero_durations,
        })
    }
}

fn steady_shape(seconds: usize) -> Shape {
    Shape {
        frame_millis: 100,
        keyframe_gaps: vec![10; seconds],
        audio_tracks: 2,
        audio_millis: 20,
        audio_lead_millis: vec![30, -30, 0],
        first_pts_millis: 1000,
        payload_bytes: 7,
        b_frames: false,
        zero_durations: false,
    }
}

fn steady_stream(seconds: usize) -> Stream {
    build_stream(&steady_shape(seconds))
}

fn heavy_stream(seconds: usize) -> Stream {
    build_stream(&Shape {
        payload_bytes: 300,
        ..steady_shape(seconds)
    })
}

#[derive(Debug, Clone)]
struct Run {
    epoch: EpochId,
    accepted: Vec<Packet>,
}

#[derive(Debug, Clone, Default)]
struct Reference {
    runs: Vec<Run>,
    run_has_ended: bool,
    write_failures: usize,
}

impl Reference {
    fn push(&mut self, buffer: &mut ReplayBuffer, packet: &Packet) -> PushOutcome {
        let pushed = buffer.push(packet.clone()).unwrap();
        self.write_failures += usize::from(pushed.write_failure.is_some());
        match pushed.outcome {
            PushOutcome::Stored => self.accepted(packet, buffer.current_epoch()),
            PushOutcome::WaitingForKeyframe => {
                assert!(packet.stream.is_video() && !packet.keyframe);
            }
            PushOutcome::DroppedOverSizeLimit => self.end_run(),
        }
        pushed.outcome
    }

    fn accepted(&mut self, packet: &Packet, epoch: EpochId) {
        if self.run_has_ended || self.runs.is_empty() {
            self.runs.push(Run {
                epoch,
                accepted: Vec::new(),
            });
            self.run_has_ended = false;
        }
        self.runs.last_mut().unwrap().accepted.push(packet.clone());
    }

    fn end_run(&mut self) {
        self.run_has_ended = true;
    }

    fn runs_with_video(&self) -> Vec<&Run> {
        self.runs
            .iter()
            .filter(|run| run.accepted.iter().any(is_video_keyframe))
            .collect()
    }
}

fn is_video_keyframe(packet: &Packet) -> bool {
    packet.stream.is_video() && packet.keyframe
}

fn newest(packets: &[Packet]) -> Ticks {
    packets.iter().map(Packet::end).max().unwrap()
}

fn keyframe_at_or_before(packets: &[Packet], time: Ticks) -> Option<Ticks> {
    packets
        .iter()
        .filter(|packet| is_video_keyframe(packet) && packet.pts <= time)
        .map(|packet| packet.pts)
        .max()
}

fn read(slice: &ReplaySlice) -> Vec<Packet> {
    slice.packets().collect::<Result<_, _>>().unwrap()
}

fn pushed_packets_in_range(run: &[Packet], slice: &ReplaySlice) -> Vec<Packet> {
    let first_video = run
        .iter()
        .position(|packet| is_video_keyframe(packet) && packet.pts == slice.video_start())
        .expect("the video of a slice starts on a stored keyframe");
    let audio_from = slice.start() - AUDIO_PRE_ROLL;
    run.iter()
        .enumerate()
        .filter(|(position, packet)| {
            if packet.stream.is_video() {
                *position >= first_video
            } else {
                packet.end() > audio_from
            }
        })
        .map(|(_, packet)| packet.clone())
        .collect()
}

fn assert_slice_matches(run: &Run, slice: &ReplaySlice) {
    assert_eq!(slice.epoch(), run.epoch);
    assert_eq!(slice.duration(), newest(&run.accepted) - slice.start());
    assert_eq!(
        Some(slice.video_start()),
        keyframe_at_or_before(&run.accepted, slice.start())
    );
    let packets = read(slice);
    assert!(
        packets == pushed_packets_in_range(&run.accepted, slice),
        "a slice holds exactly the pushed packets of its range, in order"
    );
    assert!(is_video_keyframe(
        packets
            .iter()
            .find(|packet| packet.stream.is_video())
            .unwrap()
    ));
    assert_eq!(read(slice).len(), packets.len(), "a slice reads twice");
}

fn assert_save_matches(reference: &Reference, save: &ReplaySave) {
    let runs = reference.runs_with_video();
    let slices = save.slices();
    assert!(slices.len() <= runs.len());
    let saved_runs = &runs[runs.len() - slices.len()..];
    for (position, (slice, run)) in slices.iter().zip(saved_runs).enumerate() {
        assert_slice_matches(run, slice);
        if position > 0 {
            let first_keyframe = run.accepted.iter().find(|packet| is_video_keyframe(packet));
            assert_eq!(slice.start(), first_keyframe.unwrap().pts);
        }
    }
}

fn save_everything(buffer: &mut ReplayBuffer) -> ReplaySave {
    buffer.save(SaveRange::Everything, AfterSave::KeepBuffer)
}

fn assert_start_follows_the_keyframe_rule(
    run: &Run,
    slice: &ReplaySlice,
    asked: Ticks,
    covered: Ticks,
) {
    let newest = newest(&run.accepted);
    let run_start = newest - covered;
    let asked_start = if asked < covered {
        newest - asked
    } else {
        run_start
    };
    let keyframe = keyframe_at_or_before(&run.accepted, asked_start).unwrap();
    let snaps = keyframe >= run_start && asked_start - keyframe <= LONGEST_KEYFRAME_SNAP;
    assert_eq!(slice.start(), if snaps { keyframe } else { asked_start });
    assert_eq!(slice.video_start(), keyframe);
}

fn stays_full_starts_on_a_keyframe_and_within_its_limits(setup: Setup, stream: &Stream) {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(setup, &folder);
    let mut reference = Reference::default();
    let mut was_full = false;
    let last = stream.packets.len() - 1;
    for (number, packet) in stream.packets.iter().enumerate() {
        assert_eq!(reference.push(&mut buffer, packet), PushOutcome::Stored);
        let covered = buffer.covered_duration();
        assert!(!was_full || covered >= LENGTH, "a full buffer went short");
        assert!(covered <= LENGTH + LONGEST_KEYFRAME_SNAP);
        was_full |= covered >= LENGTH;
        if setup == Setup::Memory {
            assert_eq!(buffer.memory_bytes(), buffer.size_bytes());
        }
        if number % 61 == 0 || number == last {
            let everything = save_everything(&mut buffer);
            assert_save_matches(&reference, &everything);
            assert_eq!(everything.duration(), covered);
            assert!(everything.slices().len() <= 1);
        }
    }
    assert_eq!(reference.write_failures, 0);
    drop(buffer);
    assert_nothing_left_on_disk(&folder);
}

fn save_of_the_last_stretch_is_long_enough(setup: Setup, stream: &Stream, millis: u64) {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(setup, &folder);
    let mut reference = Reference::default();
    for packet in &stream.packets {
        reference.push(&mut buffer, packet);
    }
    let asked = Ticks::from_millis(i64::try_from(millis).unwrap());
    let covered = buffer.covered_duration();
    let save = buffer.save(
        SaveRange::Last(Duration::from_millis(millis)),
        AfterSave::KeepBuffer,
    );
    assert_save_matches(&reference, &save);
    if millis == 0 {
        assert!(save.is_empty());
        return;
    }
    assert_eq!(save.slices().len(), 1);
    assert!(save.duration() >= asked.min(covered));
    assert!(save.duration() <= asked + LONGEST_KEYFRAME_SNAP);
    assert_start_follows_the_keyframe_rule(&reference.runs[0], &save.slices()[0], asked, covered);
}

fn save_survives_pushes_and_evictions(
    setup: Setup,
    stream: &Stream,
    saved_after: usize,
    range: SaveRange,
) {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(setup, &folder);
    let mut reference = Reference::default();
    let (before, after) = stream
        .packets
        .split_at(saved_after.min(stream.packets.len()));
    for packet in before {
        reference.push(&mut buffer, packet);
    }
    let save = buffer.save(range, AfterSave::KeepBuffer);
    let reference_at_save = reference.clone();
    let read_meanwhile = thread::scope(|scope| {
        let reader = scope.spawn(|| save.slices().iter().map(read).collect::<Vec<_>>());
        for packet in after {
            reference.push(&mut buffer, packet);
        }
        buffer.begin_epoch();
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
    assert_nothing_left_on_disk(&folder);
}

fn epochs_are_never_mixed(setup: Setup, streams: &[Stream], range: SaveRange, after: AfterSave) {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(setup, &folder);
    let mut reference = Reference::default();
    for (epoch, stream) in streams.iter().enumerate() {
        if epoch > 0 {
            assert_eq!(buffer.begin_epoch().0, u64::try_from(epoch).unwrap());
            reference.end_run();
        }
        for packet in &stream.packets {
            reference.push(&mut buffer, packet);
        }
    }
    let covered = buffer.covered_duration();
    assert!(covered <= LENGTH + LONGEST_KEYFRAME_SNAP);
    let save = buffer.save(range, after);
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
            assert!(save.duration() >= asked.min(covered));
            assert!(save.duration() <= asked + LONGEST_KEYFRAME_SNAP);
        }
        SaveRange::Everything => assert_eq!(save.duration(), covered),
    }
    if after == AfterSave::ClearBuffer {
        assert_eq!(buffer.covered_duration(), Ticks::ZERO);
        assert_eq!(buffer.size_bytes(), 0);
        assert_eq!(buffer.memory_bytes(), 0);
        assert!(save_everything(&mut buffer).is_empty());
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

prop_compose! {
    fn any_packet()(
        stream in 0u8..=3,
        pts_millis in -1000i64..=120_000,
        dts_millis in -1000i64..=120_000,
        duration_millis in prop_oneof![Just(0i64), 0i64..=200, Just(-1i64), Just(3_600_000i64)],
        keyframe in prop::bool::weighted(0.1),
        bytes in prop_oneof![Just(0usize), 0usize..3000],
    ) -> Packet {
        Packet {
            stream: StreamIndex::audio(stream).unwrap_or(StreamIndex::VIDEO),
            pts: Ticks::from_millis(pts_millis),
            dts: Ticks::from_millis(dts_millis),
            duration: Ticks::from_millis(duration_millis),
            keyframe,
            data: vec![0x5A; bytes].into(),
        }
    }
}

fn stays_within_its_size_limit(setup: Setup, packets: &[Packet]) {
    let folder = TempFolder::new();
    let mut buffer = ReplayBuffer::new(&ReplayConfig {
        max_bitrate_bits_per_second: 8,
        ..config(setup, &folder)
    })
    .unwrap();
    assert_eq!(buffer.size_limit_bytes(), 1024 * 1024);
    for packet in packets.iter().cycle().take(packets.len() * 2) {
        drop(buffer.push(packet.clone()));
        assert!(buffer.held_weight() <= buffer.size_limit_bytes());
        assert!(buffer.size_bytes() <= buffer.size_limit_bytes());
        assert!(buffer.memory_bytes() <= buffer.size_bytes());
    }
    for range in [
        SaveRange::Everything,
        SaveRange::Last(Duration::from_secs(15)),
    ] {
        let save = buffer.save(range, AfterSave::KeepBuffer);
        for slice in save.slices() {
            assert!(slice.packets().all(|packet| packet.is_ok()));
            assert!(slice.stored_bytes() <= buffer.size_limit_bytes());
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: CASES_PER_PROPERTY,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn buffer_starts_on_a_keyframe_drops_whole_groups_and_stays_full(stream in stream()) {
        for setup in EVERY_SETUP {
            stays_full_starts_on_a_keyframe_and_within_its_limits(setup, &stream);
        }
    }

    #[test]
    fn save_of_the_last_stretch_covers_it_and_at_most_the_keyframe_snap_more(
        stream in stream(),
        millis in 0u64..=25_000,
    ) {
        for setup in EVERY_SETUP {
            save_of_the_last_stretch_is_long_enough(setup, &stream, millis);
        }
    }

    #[test]
    fn save_taken_mid_stream_holds_exactly_its_range_after_everything_was_evicted(
        stream in stream(),
        saved_after in 0usize..3000,
        range in save_range(),
    ) {
        for setup in EVERY_SETUP {
            save_survives_pushes_and_evictions(setup, &stream, saved_after, range);
        }
    }

    #[test]
    fn save_across_an_epoch_boundary_gives_one_slice_per_epoch_newest_last(
        streams in prop::collection::vec(prop_oneof![stream(), short_stream()], 2..=3),
        range in save_range(),
        after in after_save(),
    ) {
        for setup in EVERY_SETUP {
            epochs_are_never_mixed(setup, &streams, range, after);
        }
    }

    #[test]
    fn buffer_stays_within_its_size_limit_whatever_arrives(
        packets in prop::collection::vec(any_packet(), 1..500),
    ) {
        for setup in EVERY_SETUP {
            stays_within_its_size_limit(setup, &packets);
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
            disk_folder: Some(folder.0.clone()),
        })
        .unwrap()
    };
    assert_eq!(build(expected).backend_kind(), BackendKind::Memory);
    assert!(!folder.root().exists());
    let on_disk = build(expected - 1);
    assert_eq!(on_disk.backend_kind(), BackendKind::Disk);
    assert_eq!(on_disk.size_limit_bytes(), 2 * expected);
    assert_eq!(names_in(&folder.root()).len(), 1);
}

#[test]
fn length_is_fifteen_seconds_to_thirty_minutes_and_one_minute_by_default() {
    assert_eq!(ReplayLength::default().get(), Duration::from_mins(1));
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

fn video_after_a_refused_packet_waits_for_a_keyframe(setup: Setup, refused: &Packet) {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(setup, &folder);
    let mut reference = Reference::default();
    let stream = steady_stream(4);
    let refused_at = stream
        .packets
        .iter()
        .position(|packet| packet.stream.is_video() && packet.pts == Ticks::from_millis(2300))
        .unwrap();
    let (before, after) = stream.packets.split_at(refused_at);
    for packet in before {
        reference.push(&mut buffer, packet);
    }
    assert!(buffer.push(refused.clone()).is_err());
    let next_keyframe = after.iter().position(is_video_keyframe).unwrap();
    for packet in &after[..next_keyframe] {
        let outcome = reference.push(&mut buffer, packet);
        if packet.stream.is_video() {
            assert_eq!(outcome, PushOutcome::WaitingForKeyframe);
        }
    }
    let without_the_broken_tail = save_everything(&mut buffer);
    assert_save_matches(&reference, &without_the_broken_tail);
    let newest_video = read(&without_the_broken_tail.slices()[0])
        .into_iter()
        .filter(|packet| packet.stream.is_video())
        .map(|packet| packet.pts)
        .max();
    assert_eq!(newest_video, Some(Ticks::from_millis(2200)));
    for packet in &after[next_keyframe..] {
        assert_eq!(reference.push(&mut buffer, packet), PushOutcome::Stored);
    }
    let everything = save_everything(&mut buffer);
    assert_save_matches(&reference, &everything);
    assert_eq!(everything.slices().len(), 1);
}

#[test]
fn nothing_that_depends_on_a_refused_video_packet_is_stored() {
    let stream = steady_stream(4);
    let open_group_keyframe = stream
        .packets
        .iter()
        .find(|packet| is_video_keyframe(packet) && packet.pts == Ticks::from_millis(2000))
        .unwrap();
    let delta = stream
        .packets
        .iter()
        .find(|packet| packet.stream.is_video() && packet.pts == Ticks::from_millis(2300))
        .unwrap();
    let delta_with_a_negative_duration = Packet {
        duration: Ticks(-1),
        ..delta.clone()
    };
    for setup in EVERY_SETUP {
        video_after_a_refused_packet_waits_for_a_keyframe(setup, open_group_keyframe);
        video_after_a_refused_packet_waits_for_a_keyframe(setup, &delta_with_a_negative_duration);
    }
}

#[test]
fn refused_audio_packet_leaves_video_alone() {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(Setup::Memory, &folder);
    let mut reference = Reference::default();
    let stream = steady_stream(3);
    let (before, after) = stream.packets.split_at(stream.packets.len() / 2);
    for packet in before {
        reference.push(&mut buffer, packet);
    }
    let audio = stream
        .packets
        .iter()
        .find(|packet| !packet.stream.is_video());
    let refused = buffer.push(Packet {
        duration: Ticks(-1),
        ..audio.unwrap().clone()
    });
    assert!(matches!(refused, Err(ReplayError::NegativeDuration { .. })));
    for packet in after {
        assert_eq!(reference.push(&mut buffer, packet), PushOutcome::Stored);
    }
    assert_save_matches(&reference, &save_everything(&mut buffer));
}

fn audio_before(slice: &ReplaySlice) -> usize {
    read(slice)
        .iter()
        .filter(|packet| !packet.stream.is_video() && packet.pts < slice.start())
        .count()
}

#[test]
fn first_group_of_every_stretch_has_audio_pre_roll() {
    for setup in EVERY_SETUP {
        let folder = TempFolder::new();
        let mut buffer = new_buffer(setup, &folder);
        let mut reference = Reference::default();
        for packet in &steady_stream(3).packets {
            reference.push(&mut buffer, packet);
        }
        buffer.begin_epoch();
        reference.end_run();
        for packet in &steady_stream(2).packets {
            reference.push(&mut buffer, packet);
        }
        let everything = save_everything(&mut buffer);
        assert_save_matches(&reference, &everything);
        assert_eq!(everything.slices().len(), 2);
        for slice in everything.slices() {
            assert_eq!(slice.start(), Ticks::from_millis(1000));
            assert_eq!(audio_before(slice), 8, "80 ms of two tracks at 20 ms");
        }
        let last_second = buffer.save(
            SaveRange::Last(Duration::from_secs(1)),
            AfterSave::ClearBuffer,
        );
        assert_eq!(audio_before(&last_second.slices()[0]), 8);
        for packet in steady_stream(2).packets {
            drop(buffer.push(packet).unwrap());
        }
        let after_a_clear = save_everything(&mut buffer);
        assert_eq!(audio_before(&after_a_clear.slices()[0]), 8);
    }
}

fn audio_packet(pts_millis: i64) -> Packet {
    Packet {
        stream: StreamIndex::audio(0).unwrap(),
        pts: Ticks::from_millis(pts_millis),
        dts: Ticks::from_millis(pts_millis),
        duration: Ticks::from_millis(20),
        keyframe: true,
        data: vec![1; 200].into(),
    }
}

fn video_packet(pts_millis: i64, keyframe: bool) -> Packet {
    Packet {
        stream: StreamIndex::VIDEO,
        pts: Ticks::from_millis(pts_millis),
        dts: Ticks::from_millis(pts_millis),
        duration: Ticks::from_millis(100),
        keyframe,
        data: vec![2; 500].into(),
    }
}

#[test]
fn hour_of_sound_over_a_still_picture_keeps_only_the_last_stretch() {
    for setup in EVERY_SETUP {
        let folder = TempFolder::new();
        let mut buffer = new_buffer(setup, &folder);
        for millis in (0..3000).step_by(100) {
            drop(
                buffer
                    .push(video_packet(millis, millis % 1000 == 0))
                    .unwrap(),
            );
        }
        let hour_millis = 3_600_000;
        let mut most_held = 0;
        for millis in (3000..hour_millis).step_by(20) {
            let pushed = buffer.push(audio_packet(millis)).unwrap();
            assert_eq!(pushed.outcome, PushOutcome::Stored);
            most_held = most_held.max(buffer.size_bytes());
        }
        let seconds_held = 15 + 2 + 2;
        assert!(most_held <= 30 * 500 + seconds_held * 50 * 200);
        assert_eq!(buffer.covered_duration(), LENGTH + LONGEST_KEYFRAME_SNAP);

        let save = buffer.save(
            SaveRange::Last(Duration::from_secs(15)),
            AfterSave::KeepBuffer,
        );
        let slice = &save.slices()[0];
        assert_eq!(save.duration(), LENGTH);
        assert_eq!(slice.start(), Ticks::from_millis(hour_millis - 15_000));
        assert_eq!(slice.video_start(), Ticks::from_millis(2000));
        let packets = read(slice);
        let video: Vec<_> = packets
            .iter()
            .filter(|packet| packet.stream.is_video())
            .collect();
        assert_eq!(video.len(), 10);
        assert!(is_video_keyframe(video[0]));
        let audio = packets.len() - video.len();
        assert_eq!(audio, 15_000 / 20 + 4);
    }
}

#[test]
fn encoder_that_never_sends_another_keyframe_cannot_fill_the_buffer() {
    for setup in EVERY_SETUP {
        let folder = TempFolder::new();
        let mut buffer = ReplayBuffer::new(&ReplayConfig {
            max_bitrate_bits_per_second: 8,
            ..config(setup, &folder)
        })
        .unwrap();
        let mut dropped_everything = 0;
        let mut video_stored_afterwards = 0;
        for millis in (0..3_600_000).step_by(100) {
            let video = buffer.push(video_packet(millis, millis == 0)).unwrap();
            let audio = buffer.push(audio_packet(millis)).unwrap();
            if dropped_everything > 0 && video.outcome == PushOutcome::Stored {
                video_stored_afterwards += 1;
            }
            for outcome in [video.outcome, audio.outcome] {
                dropped_everything += usize::from(outcome == PushOutcome::DroppedOverSizeLimit);
            }
            assert!(buffer.held_weight() <= buffer.size_limit_bytes());
        }
        assert_eq!(dropped_everything, 1);
        assert_eq!(video_stored_afterwards, 0);
        assert!(save_everything(&mut buffer).is_empty());
        let pushed = buffer.push(video_packet(3_600_000, true)).unwrap();
        assert_eq!(pushed.outcome, PushOutcome::Stored);
        assert_eq!(save_everything(&mut buffer).slices().len(), 1);
    }
}

#[test]
fn memory_buffer_over_its_budget_moves_to_disk_without_losing_or_reordering() {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(Setup::MemoryThatMovesToDisk, &folder);
    let mut reference = Reference::default();
    let stream = heavy_stream(12);
    let mut saved_in_memory = None;
    let mut most_in_memory = 0;
    for packet in &stream.packets {
        reference.push(&mut buffer, packet);
        most_in_memory = most_in_memory.max(buffer.memory_bytes());
        if buffer.backend_kind() == BackendKind::Memory {
            assert_eq!(buffer.memory_bytes(), buffer.size_bytes());
            assert!(!folder.root().exists());
            saved_in_memory = Some((save_everything(&mut buffer), reference.clone()));
        }
    }
    assert_eq!(buffer.backend_kind(), BackendKind::Disk);
    assert_eq!(reference.write_failures, 0);
    let one_group_and_one_audio_unit = 2 * 110 * 400;
    assert!(most_in_memory <= SMALL_MEMORY_BUDGET_BYTES + one_group_and_one_audio_unit);
    assert!(buffer.memory_bytes() <= one_group_and_one_audio_unit);
    assert!(buffer.size_bytes() > 10 * SMALL_MEMORY_BUDGET_BYTES);
    assert_eq!(buffer.units_in_memory(), 0);
    assert_save_matches(&reference, &save_everything(&mut buffer));
    let (save, reference_at_save) = saved_in_memory.unwrap();
    assert_save_matches(&reference_at_save, &save);
}

#[test]
fn memory_buffer_over_its_budget_without_a_folder_stays_and_says_so() {
    let folder = TempFolder::new();
    let mut buffer = ReplayBuffer::new(&ReplayConfig {
        disk_folder: None,
        ..config(Setup::MemoryThatMovesToDisk, &folder)
    })
    .unwrap();
    let mut reference = Reference::default();
    assert!(!buffer.is_over_memory_budget());
    for packet in &heavy_stream(6).packets {
        reference.push(&mut buffer, packet);
    }
    assert!(buffer.is_over_memory_budget());
    assert_eq!(buffer.backend_kind(), BackendKind::Memory);
    assert_eq!(buffer.memory_bytes(), buffer.size_bytes());
    assert_eq!(reference.write_failures, 0);
    assert_save_matches(&reference, &save_everything(&mut buffer));
}

#[test]
fn disk_buffer_creates_its_folder_and_needs_one() {
    let folder = TempFolder::new();
    let missing = folder.0.join("not").join("there").join("yet");
    let built = ReplayBuffer::new(&ReplayConfig {
        disk_folder: Some(missing.clone()),
        ..config(Setup::Disk, &folder)
    });
    assert!(built.is_ok());
    assert!(missing.join(ROOT_DIRECTORY).is_dir());
    let without_a_folder = ReplayBuffer::new(&ReplayConfig {
        disk_folder: None,
        ..config(Setup::Disk, &folder)
    });
    assert!(matches!(without_a_folder, Err(ReplayError::NoDiskFolder)));
    let too_large_for_its_budget = ReplayBuffer::new(&ReplayConfig {
        backend: BackendChoice::Auto {
            memory_budget_bytes: 1,
        },
        disk_folder: None,
        ..config(Setup::Disk, &folder)
    })
    .unwrap();
    assert_eq!(too_large_for_its_budget.backend_kind(), BackendKind::Memory);
    fs::write(folder.0.join("a-file"), "in the way").unwrap();
    let under_a_file = ReplayBuffer::new(&ReplayConfig {
        disk_folder: Some(folder.0.join("a-file")),
        ..config(Setup::Disk, &folder)
    });
    assert!(matches!(under_a_file, Err(ReplayError::Storage { .. })));
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
    let store = DiskStore::with_sink(
        &folder.0,
        Box::new(move |path| {
            let sink: Sink = Box::new(SinkOnAFailingDisk {
                file: File::create_new(path)?,
                disk_works: Arc::clone(&seen_by_sink),
            });
            Ok(sink)
        }),
    )
    .unwrap();
    (
        ReplayBuffer::assemble(&config(Setup::Disk, folder), Some(store)),
        disk_works,
    )
}

fn unit_files(folder: &TempFolder) -> usize {
    names_in(&folder.root())
        .iter()
        .flat_map(|session| names_in(&folder.root().join(session)))
        .filter(|name| name.starts_with("group-"))
        .count()
}

#[test]
fn failed_write_is_reported_and_loses_no_packet() {
    let folder = TempFolder::new();
    let (mut buffer, disk_works) = buffer_on_a_disk_that_can_fail(&folder);
    let mut reference = Reference::default();
    let stream = steady_stream(12);
    let thirds = stream.packets.len() / 3;
    for packet in &stream.packets[..thirds] {
        reference.push(&mut buffer, packet);
    }
    assert_eq!(reference.write_failures, 0);
    let files_before = unit_files(&folder);
    assert!(files_before > 0);
    let saved_before = save_everything(&mut buffer);
    let reference_before = reference.clone();

    disk_works.store(false, Ordering::Relaxed);
    let mut reported = Vec::new();
    for packet in &stream.packets[thirds..2 * thirds] {
        let pushed = buffer.push(packet.clone()).unwrap();
        assert_eq!(pushed.outcome, PushOutcome::Stored);
        reference.accepted(packet, buffer.current_epoch());
        reported.extend(pushed.write_failure);
    }
    assert!(reported.len() >= 3);
    assert!(reported.iter().all(|failure| matches!(failure,
        ReplayError::Storage { source, .. } if source.kind() == io::ErrorKind::StorageFull)));
    assert_eq!(
        unit_files(&folder),
        files_before,
        "no half-written file stays"
    );
    assert!(buffer.units_in_memory() > 0);
    let saved_while_failing = save_everything(&mut buffer);
    assert_save_matches(&reference, &saved_while_failing);
    assert_eq!(saved_while_failing.slices().len(), 1);
    assert_save_matches(&reference_before, &saved_before);

    disk_works.store(true, Ordering::Relaxed);
    for packet in &stream.packets[2 * thirds..] {
        reference.push(&mut buffer, packet);
    }
    assert_eq!(reference.write_failures, 0);
    assert_eq!(buffer.units_in_memory(), 0);
    assert!(unit_files(&folder) > files_before);
    let everything = save_everything(&mut buffer);
    assert_save_matches(&reference, &everything);
    assert_eq!(everything.slices().len(), 1);
    assert_eq!(everything.duration(), Ticks::from_secs(12));
    assert_save_matches(&reference_before, &saved_before);
}

#[test]
fn save_with_a_missing_file_still_gives_every_other_group() {
    let folder = TempFolder::new();
    let mut buffer = new_buffer(Setup::Disk, &folder);
    let mut reference = Reference::default();
    for packet in &steady_stream(6).packets {
        reference.push(&mut buffer, packet);
    }
    let save = save_everything(&mut buffer);
    let slice = &save.slices()[0];
    let complete = read(slice);
    let StoredUnit::Disk(third_group) = &slice.video[2] else {
        panic!("a disk buffer keeps its finished groups on disk");
    };
    let lost: Vec<Packet> = third_group
        .reader()
        .unwrap()
        .map(|sequenced| sequenced.unwrap().packet)
        .collect();
    assert_eq!(lost.len(), 10);
    fs::remove_file(third_group.path()).unwrap();

    let (read_back, failures): (Vec<_>, Vec<_>) = slice.packets().partition(Result::is_ok);
    let read_back: Vec<Packet> = read_back.into_iter().map(Result::unwrap).collect();
    assert_eq!(failures.len(), 1);
    assert!(matches!(failures[0], Err(ReplayError::Storage { .. })));
    let expected: Vec<Packet> = complete
        .into_iter()
        .filter(|packet| !lost.contains(packet))
        .collect();
    assert_eq!(read_back.len(), expected.len());
    assert_eq!(read_back, expected);
    let keyframes_read = read_back.iter().filter(|packet| is_video_keyframe(packet));
    assert_eq!(keyframes_read.count(), 5);
}

#[test]
fn session_is_dead_only_when_this_computer_made_it_and_its_process_is_gone() {
    let lock_is = |lock| move || lock;
    let never_asked = || -> OwnerLock { panic!("the lock of a live session is never touched") };
    assert!(is_proven_dead(
        "ab",
        "ab",
        OwnerProcess::Gone,
        lock_is(OwnerLock::NotHeld)
    ));
    assert!(is_proven_dead(
        "ab",
        "ab",
        OwnerProcess::Gone,
        lock_is(OwnerLock::Unknown)
    ));
    assert!(!is_proven_dead(
        "ab",
        "ab",
        OwnerProcess::Gone,
        lock_is(OwnerLock::Held)
    ));
    assert!(!is_proven_dead(
        "ab",
        "ab",
        OwnerProcess::ThisOne,
        never_asked
    ));
    assert!(!is_proven_dead(
        "ab",
        "ab",
        OwnerProcess::Running,
        never_asked
    ));
    assert!(!is_proven_dead(
        "ab",
        "ab",
        OwnerProcess::Unknown,
        never_asked
    ));
    assert!(!is_proven_dead("cd", "ab", OwnerProcess::Gone, never_asked));
    assert!(!is_proven_dead("", "", OwnerProcess::Gone, never_asked));
    assert!(!is_proven_dead("ab", "", OwnerProcess::Gone, never_asked));
}

fn spawn_helper(folder: Option<&Path>) -> Child {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([HELPER_TEST, "--exact", "--nocapture", "--test-threads=1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(folder) = folder {
        command.env(HELPER_FOLDER_VARIABLE, folder);
    } else {
        command.env_remove(HELPER_FOLDER_VARIABLE);
    }
    command.spawn().unwrap()
}

fn process_that_has_ended() -> u32 {
    let mut helper = spawn_helper(None);
    let process = helper.id();
    helper.wait().unwrap();
    process
}

#[test]
fn helper_process_holds_a_disk_buffer_until_killed() {
    let Some(folder) = std::env::var_os(HELPER_FOLDER_VARIABLE) else {
        return;
    };
    let mut buffer = ReplayBuffer::new(&ReplayConfig {
        length: ReplayLength::SHORTEST,
        max_bitrate_bits_per_second: 8_000_000,
        backend: BackendChoice::Disk,
        disk_folder: Some(PathBuf::from(folder)),
    })
    .unwrap();
    for packet in steady_stream(4).packets {
        drop(buffer.push(packet).unwrap());
    }
    println!("{HELPER_IS_HOLDING}");
    let mut until_the_parent_lets_go = Vec::new();
    drop(io::stdin().read_to_end(&mut until_the_parent_lets_go));
}

fn wait_until_the_helper_holds(helper: &mut Child) {
    let output = BufReader::new(helper.stdout.take().unwrap());
    let holding = output
        .lines()
        .map_while(Result::ok)
        .any(|line| line.contains(HELPER_IS_HOLDING));
    assert!(holding, "the helper process never built its buffer");
}

#[test]
fn files_of_another_running_process_stay_until_it_is_dead() {
    let folder = TempFolder::new();
    let mut helper = spawn_helper(Some(&folder.0));
    wait_until_the_helper_holds(&mut helper);
    assert_eq!(owner_process(helper.id()), OwnerProcess::Running);
    let sessions = names_in(&folder.root());
    assert_eq!(sessions.len(), 1);
    let session = folder.root().join(&sessions[0]);
    let files = names_in(&session);
    assert!(files.len() > 4);

    assert_eq!(
        remove_leftovers(&folder.0).unwrap(),
        RemovedLeftovers::default()
    );
    let neighbour = new_buffer(Setup::Disk, &folder);
    assert_eq!(names_in(&session), files);
    drop(neighbour);
    assert_eq!(names_in(&folder.root()), sessions);

    helper.kill().unwrap();
    helper.wait().unwrap();
    assert_eq!(owner_process(helper.id()), OwnerProcess::Gone);
    assert_eq!(
        names_in(&session),
        files,
        "a killed process cleans nothing up"
    );
    let removed = remove_leftovers(&folder.0).unwrap();
    assert_eq!(removed.sessions, 1);
    assert_eq!(removed.files, files.len() - 1);
    assert!(removed.bytes > 0);
    assert_nothing_left_on_disk(&folder);
}

#[test]
fn owner_process_tells_this_process_from_a_running_and_an_ended_one() {
    assert_eq!(owner_process(process::id()), OwnerProcess::ThisOne);
    assert_eq!(owner_process(process_that_has_ended()), OwnerProcess::Gone);
    #[cfg(unix)]
    assert_eq!(owner_process(1), OwnerProcess::Running);
}

fn leftover_session(root: &Path, name: &SessionName, files: &[(&str, usize)]) -> PathBuf {
    let directory = root.join(name.to_string());
    fs::create_dir(&directory).unwrap();
    for &(file, bytes) in files {
        fs::write(directory.join(file), vec![0; bytes]).unwrap();
    }
    directory
}

#[test]
fn crash_leftovers_are_removed_and_nothing_else_is() {
    let folder = TempFolder::new();
    let mut live = new_buffer(Setup::Disk, &folder);
    let mut reference = Reference::default();
    let stream = steady_stream(6);
    let (before, after) = stream.packets.split_at(stream.packets.len() / 2);
    for packet in before {
        reference.push(&mut live, packet);
    }
    let root = folder.root();
    let live_session = root.join(&names_in(&root)[0]);
    let live_files = names_in(&live_session);
    assert!(live_files.len() > 2);

    let ended = process_that_has_ended();
    let session_of = |host: String, process, stamp| SessionName {
        host,
        process,
        stamp,
        attempt: 0,
    };
    let crashed = leftover_session(
        &root,
        &session_of(this_host(), ended, 1),
        &[(OWNER_LOCK, 0), ("group-0.bin", 10), ("group-17.bin", 5)],
    );
    let crashed_while_starting = leftover_session(
        &root,
        &session_of(this_host(), ended, 2),
        &[("group-0.bin", 3)],
    );
    let crashed_with_a_guest = leftover_session(
        &root,
        &session_of(this_host(), ended, 3),
        &[(OWNER_LOCK, 0), ("group-0.bin", 4), ("holiday.mp4", 9)],
    );
    let of_another_computer = leftover_session(
        &root,
        &session_of(format!("{}00", this_host()), ended, 4),
        &[(OWNER_LOCK, 0), ("group-0.bin", 6)],
    );
    let of_an_unnamed_computer = leftover_session(
        &root,
        &session_of(String::new(), ended, 5),
        &[("group-0.bin", 6)],
    );
    let of_this_process = leftover_session(
        &root,
        &session_of(this_host(), process::id(), 6),
        &[("group-0.bin", 6)],
    );
    let not_a_session = root.join("session-backup");
    fs::create_dir(&not_a_session).unwrap();
    fs::write(not_a_session.join("group-0.bin"), "mine").unwrap();
    fs::write(root.join("notes.txt"), "mine").unwrap();
    fs::write(folder.0.join("clip.mp4"), "a saved clip").unwrap();

    assert_eq!(
        remove_dead_sessions(&root, "").unwrap(),
        RemovedLeftovers::default(),
        "a computer without a name proves nothing dead"
    );
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
    assert_eq!(names_in(&of_another_computer), ["group-0.bin", OWNER_LOCK]);
    assert_eq!(names_in(&of_an_unnamed_computer), ["group-0.bin"]);
    assert_eq!(names_in(&of_this_process), ["group-0.bin"]);
    assert_eq!(names_in(&not_a_session), ["group-0.bin"]);
    assert!(root.join("notes.txt").exists());
    assert!(folder.0.join("clip.mp4").exists());
    assert_eq!(names_in(&live_session), live_files);

    let crashed_again = leftover_session(
        &root,
        &session_of(this_host(), ended, 7),
        &[("group-3.bin", 1)],
    );
    let second_instance = new_buffer(Setup::Disk, &folder);
    assert!(!crashed_again.exists());
    assert_eq!(names_in(&live_session), live_files);
    drop(second_instance);

    for packet in after {
        reference.push(&mut live, packet);
    }
    assert_save_matches(&reference, &save_everything(&mut live));
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
