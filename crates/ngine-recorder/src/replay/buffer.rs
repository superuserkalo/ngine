use std::collections::VecDeque;
use std::fmt;
use std::iter;
use std::mem;
use std::path::PathBuf;
use std::time::Duration;

use super::budget::{
    BackendChoice, BackendKind, ReplayLength, choose_backend, expected_size_bytes,
};
use super::disk::{DiskStore, DiskUnit};
use super::error::ReplayError;
use super::packet::Packet;
use super::save::{EpochId, ReplaySave, ReplaySlice};
use super::time::Ticks;
use super::unit::{Sequenced, StoredUnit};

/// A slice carries the audio packets that end later than this long before
/// its start, so a decoder has primed before the clip starts.
pub const AUDIO_PRE_ROLL: Ticks = Ticks::from_millis(80);

/// A save starts on the video keyframe at or before the asked start when
/// that keyframe is at most this much older, so a clip is at most this much
/// longer than asked. With an older keyframe, as after a long still picture,
/// the slice starts exactly where asked and carries the video from the
/// keyframe for the decoder only.
pub const LONGEST_KEYFRAME_SNAP: Ticks = Ticks::from_secs(2);

const AUDIO_UNIT_SPAN: Ticks = Ticks::from_secs(1);
const AUDIO_UNIT_PACKETS: usize = 512;
const AUDIO_KEPT_BEFORE_VIDEO: Ticks = Ticks::from_secs(1);
const OPEN_UNIT_BYTES: u64 = 4 * 1024 * 1024;
const HELD_OVERHEAD_PER_PACKET_BYTES: u64 = 64;
const SMALLEST_SIZE_LIMIT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayConfig {
    pub length: ReplayLength,
    /// The cap the encoder is held to, all streams together. With `length`
    /// it gives the expected size, which [`BackendChoice::Auto`] compares to
    /// its budget and which sets [`ReplayBuffer::size_limit_bytes`].
    pub max_bitrate_bits_per_second: u64,
    pub backend: BackendChoice,
    /// Where the buffer keeps its files when it is on disk. It is created
    /// when missing, and the buffer only touches the `ngine-replay` folder
    /// inside. Without one, [`BackendChoice::Disk`] is an error and
    /// [`BackendChoice::Auto`] stays in memory when it passes its budget.
    pub disk_folder: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Stored,
    /// A video packet was dropped because no decoder could use it: no video
    /// keyframe has arrived since the start, the last clear, the last epoch
    /// change or the last refused video packet.
    WaitingForKeyframe,
    /// The newest stretch alone passed [`ReplayBuffer::size_limit_bytes`],
    /// so it was dropped with this packet. Video waits for a keyframe.
    DroppedOverSizeLimit,
}

#[derive(Debug)]
#[must_use]
pub struct Pushed {
    pub outcome: PushOutcome,
    /// Set when writing to the disk folder failed since the last push. No
    /// packet was lost: what could not be written stays in memory, and the
    /// buffer tries the disk again with the next group.
    pub write_failure: Option<ReplayError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveRange {
    /// At least this much, counted back from the end of the newest packet,
    /// or everything when the buffer holds less.
    Last(Duration),
    Everything,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterSave {
    KeepBuffer,
    ClearBuffer,
}

/// Keeps the last stretch of encoded packets, in memory or on disk.
///
/// One thread owns it and pushes. [`Self::save`] runs on that thread, only
/// takes a snapshot and never touches the disk; the [`ReplaySave`] it returns
/// is read on any thread.
///
/// Time is measured over all streams, so a still picture that delivers no
/// video, or an encoder that sends no keyframe, lets neither audio nor
/// anything else pile up: audio older than the stretch a save can ask for is
/// dropped on its own, and the whole buffer never passes
/// [`Self::size_limit_bytes`].
pub struct ReplayBuffer {
    length: Ticks,
    size_limit: u64,
    storage: Storage,
    closed_runs: VecDeque<Run>,
    current: Run,
    epoch: EpochId,
    next_sequence: u64,
}

struct Storage {
    disk: Option<DiskStore>,
    disk_folder: Option<PathBuf>,
    memory_budget: Option<u64>,
    writes_to_disk: bool,
    last_write_failed: bool,
    write_failure: Option<ReplayError>,
    held: Held,
}

#[derive(Debug, Default)]
struct Held {
    weight: u64,
    size_bytes: u64,
    memory_bytes: u64,
    units_in_memory: usize,
}

struct Unit {
    end: Ticks,
    size_bytes: u64,
    packet_count: u64,
    stored: StoredUnit,
}

struct OpenUnit {
    start: Ticks,
    end: Ticks,
    size_bytes: u64,
    packets: Vec<Sequenced>,
}

struct Group {
    keyframe: Ticks,
    units: Vec<Unit>,
}

struct OpenVideo {
    keyframe: Ticks,
    continues_newest_group: bool,
    unit: OpenUnit,
}

struct Run {
    epoch: EpochId,
    groups: VecDeque<Group>,
    open_video: Option<OpenVideo>,
    audio: VecDeque<Unit>,
    open_audio: Option<OpenUnit>,
    newest: Option<Ticks>,
    newest_keyframe: Option<Ticks>,
    served_from: Option<Ticks>,
}

fn weight(size_bytes: u64, packet_count: u64) -> u64 {
    size_bytes + packet_count * HELD_OVERHEAD_PER_PACKET_BYTES
}

impl OpenUnit {
    fn starting_at(start: Ticks) -> Self {
        Self {
            start,
            end: start,
            size_bytes: 0,
            packets: Vec::new(),
        }
    }

    fn push(&mut self, sequence: u64, packet: Packet) {
        self.end = self.end.max(packet.end());
        self.size_bytes += packet.size_bytes();
        self.packets.push(Sequenced { sequence, packet });
    }

    fn packet_count(&self) -> u64 {
        u64::try_from(self.packets.len()).unwrap_or(u64::MAX)
    }

    fn snapshot(&self) -> StoredUnit {
        StoredUnit::Memory(self.packets.as_slice().into())
    }
}

impl Storage {
    fn took(&mut self, packet: &Packet) {
        self.held.size_bytes += packet.size_bytes();
        self.held.memory_bytes += packet.size_bytes();
        self.held.weight += weight(packet.size_bytes(), 1);
    }

    fn seal(&mut self, open: OpenUnit) -> Unit {
        let packet_count = open.packet_count();
        let on_disk = if self.writes_to_disk {
            self.write(&open.packets)
        } else {
            None
        };
        let stored = if let Some(unit) = on_disk {
            self.held.memory_bytes -= open.size_bytes;
            StoredUnit::Disk(unit)
        } else {
            self.held.units_in_memory += 1;
            StoredUnit::Memory(open.packets.into())
        };
        Unit {
            end: open.end,
            size_bytes: open.size_bytes,
            packet_count,
            stored,
        }
    }

    fn write(&mut self, packets: &[Sequenced]) -> Option<DiskUnit> {
        if self.disk.is_none() {
            match DiskStore::create(self.disk_folder.as_ref()?) {
                Ok(disk) => self.disk = Some(disk),
                Err(error) => {
                    self.write_failed(error);
                    return None;
                }
            }
        }
        match self.disk.as_mut()?.write(packets) {
            Ok(unit) => {
                self.last_write_failed = false;
                Some(unit)
            }
            Err(error) => {
                self.write_failed(error);
                None
            }
        }
    }

    fn write_failed(&mut self, error: ReplayError) {
        self.last_write_failed = true;
        self.write_failure = Some(error);
    }

    fn forgot_unit(&mut self, unit: &Unit) {
        self.held.size_bytes -= unit.size_bytes;
        self.held.weight -= weight(unit.size_bytes, unit.packet_count);
        if matches!(unit.stored, StoredUnit::Memory(_)) {
            self.held.memory_bytes -= unit.size_bytes;
            self.held.units_in_memory -= 1;
        }
    }

    fn forgot_open(&mut self, open: &OpenUnit) {
        self.held.size_bytes -= open.size_bytes;
        self.held.memory_bytes -= open.size_bytes;
        self.held.weight -= weight(open.size_bytes, open.packet_count());
    }

    fn forgot_group(&mut self, group: &Group) {
        for unit in &group.units {
            self.forgot_unit(unit);
        }
    }

    fn forgot_run(&mut self, run: &Run) {
        for group in &run.groups {
            self.forgot_group(group);
        }
        for unit in &run.audio {
            self.forgot_unit(unit);
        }
        if let Some(open) = &run.open_video {
            self.forgot_open(&open.unit);
        }
        if let Some(open) = &run.open_audio {
            self.forgot_open(open);
        }
    }

    fn is_over_memory_budget(&self) -> bool {
        self.memory_budget
            .is_some_and(|budget| self.held.memory_bytes > budget)
    }

    fn write_to_disk_once_over_budget(&mut self) {
        let has_somewhere_to_write = self.disk.is_some() || self.disk_folder.is_some();
        if !self.writes_to_disk && has_somewhere_to_write && self.is_over_memory_budget() {
            self.writes_to_disk = true;
        }
    }
}

impl Run {
    fn new(epoch: EpochId) -> Self {
        Self {
            epoch,
            groups: VecDeque::new(),
            open_video: None,
            audio: VecDeque::new(),
            open_audio: None,
            newest: None,
            newest_keyframe: None,
            served_from: None,
        }
    }

    fn group_count(&self) -> usize {
        let open_group = self
            .open_video
            .as_ref()
            .is_some_and(|open| !open.continues_newest_group);
        self.groups.len() + usize::from(open_group)
    }

    fn group_start(&self, index: usize) -> Option<Ticks> {
        match self.groups.get(index) {
            Some(group) => Some(group.keyframe),
            None => self
                .open_video
                .as_ref()
                .filter(|open| !open.continues_newest_group && index == self.groups.len())
                .map(|open| open.keyframe),
        }
    }

    fn start(&self) -> Option<Ticks> {
        let oldest_keyframe = self.group_start(0)?;
        Some(self.served_from.map_or(oldest_keyframe, |served_from| {
            served_from.max(oldest_keyframe)
        }))
    }

    fn covered(&self) -> Ticks {
        match (self.newest, self.start()) {
            (Some(newest), Some(start)) => (newest - start).max(Ticks::ZERO),
            _ => Ticks::ZERO,
        }
    }

    fn took(&mut self, packet: &Packet) {
        let end = packet.end();
        self.newest = Some(self.newest.map_or(end, |newest| newest.max(end)));
    }

    fn drop_oldest_group(&mut self, storage: &mut Storage) {
        if let Some(group) = self.groups.pop_front() {
            storage.forgot_group(&group);
        }
        self.drop_audio_no_save_can_ask_for(storage);
    }

    fn drop_audio_no_save_can_ask_for(&mut self, storage: &mut Storage) {
        let Some(newest) = self.newest else {
            return;
        };
        let needed_from = self.start().unwrap_or(newest - AUDIO_KEPT_BEFORE_VIDEO) - AUDIO_PRE_ROLL;
        while let Some(oldest) = self.audio.front()
            && oldest.end <= needed_from
        {
            storage.forgot_unit(oldest);
            self.audio.pop_front();
        }
    }

    fn seal_open_video(&mut self, storage: &mut Storage) {
        let Some(open) = self.open_video.take() else {
            return;
        };
        if open.unit.packets.is_empty() {
            return;
        }
        let unit = storage.seal(open.unit);
        match self.groups.back_mut() {
            Some(group) if open.continues_newest_group => group.units.push(unit),
            _ => self.groups.push_back(Group {
                keyframe: open.keyframe,
                units: vec![unit],
            }),
        }
    }

    fn seal_open_audio(&mut self, storage: &mut Storage) {
        if let Some(open) = self.open_audio.take() {
            self.audio.push_back(storage.seal(open));
        }
    }

    fn units_mut(&mut self) -> impl Iterator<Item = &mut Unit> {
        self.groups
            .iter_mut()
            .flat_map(|group| group.units.iter_mut())
            .chain(self.audio.iter_mut())
    }

    fn slice(&self, still_asked: &mut Option<Ticks>) -> Option<ReplaySlice> {
        if *still_asked == Some(Ticks::ZERO) {
            return None;
        }
        let newest = self.newest?;
        let run_start = self.start()?;
        let asked_start = match *still_asked {
            Some(asked) if asked < self.covered() => newest - asked,
            _ => run_start,
        };
        let group = (0..self.group_count()).rev().find(|&index| {
            self.group_start(index)
                .is_some_and(|keyframe| keyframe <= asked_start)
        })?;
        let keyframe = self.group_start(group)?;
        let snaps_to_keyframe =
            keyframe >= run_start && asked_start - keyframe <= LONGEST_KEYFRAME_SNAP;
        let start = if snaps_to_keyframe {
            keyframe
        } else {
            asked_start
        };
        let duration = (newest - start).max(Ticks::ZERO);
        if let Some(asked) = still_asked {
            *asked = (*asked - duration).max(Ticks::ZERO);
        }
        let video_units = || {
            self.groups
                .iter()
                .skip(group)
                .flat_map(|group| &group.units)
        };
        let audio_units = || {
            self.audio
                .iter()
                .filter(|unit| unit.end > start - AUDIO_PRE_ROLL)
        };
        let open_video = self.open_video.as_ref().map(|open| &open.unit);
        let stored_bytes = video_units()
            .chain(audio_units())
            .map(|unit| unit.size_bytes)
            .chain(open_video.iter().map(|open| open.size_bytes))
            .chain(self.open_audio.iter().map(|open| open.size_bytes))
            .sum();
        Some(ReplaySlice {
            epoch: self.epoch,
            start,
            video_start: keyframe,
            duration,
            stored_bytes,
            video: video_units()
                .map(|unit| unit.stored.clone())
                .chain(open_video.map(OpenUnit::snapshot))
                .collect(),
            audio: audio_units()
                .map(|unit| unit.stored.clone())
                .chain(self.open_audio.as_ref().map(OpenUnit::snapshot))
                .collect(),
        })
    }
}

impl fmt::Debug for ReplayBuffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplayBuffer")
            .field("backend", &self.backend_kind())
            .field("length", &self.length)
            .field("covered_duration", &self.covered_duration())
            .field("held", &self.storage.held)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

impl ReplayBuffer {
    /// A buffer that starts on disk first removes what crashed runs left in
    /// the folder.
    pub fn new(config: &ReplayConfig) -> Result<Self, ReplayError> {
        let expected = expected_size_bytes(config.max_bitrate_bits_per_second, config.length);
        let disk = match (
            choose_backend(expected, config.backend),
            &config.disk_folder,
        ) {
            (BackendKind::Disk, Some(folder)) => Some(DiskStore::create(folder)?),
            (BackendKind::Disk, None) if config.backend == BackendChoice::Disk => {
                return Err(ReplayError::NoDiskFolder);
            }
            (BackendKind::Memory | BackendKind::Disk, _) => None,
        };
        Ok(Self::assemble(config, disk))
    }

    pub(crate) fn assemble(config: &ReplayConfig, disk: Option<DiskStore>) -> Self {
        let expected = expected_size_bytes(config.max_bitrate_bits_per_second, config.length);
        let memory_budget = match config.backend {
            BackendChoice::Auto {
                memory_budget_bytes,
            } => Some(memory_budget_bytes),
            BackendChoice::Memory | BackendChoice::Disk => None,
        };
        Self {
            length: Ticks::from_duration_rounded_up(config.length.get()),
            size_limit: expected.saturating_mul(2).max(SMALLEST_SIZE_LIMIT_BYTES),
            storage: Storage {
                writes_to_disk: disk.is_some(),
                disk,
                disk_folder: memory_budget.and(config.disk_folder.clone()),
                memory_budget,
                last_write_failed: false,
                write_failure: None,
                held: Held::default(),
            },
            closed_runs: VecDeque::new(),
            current: Run::new(EpochId::default()),
            epoch: EpochId::default(),
            next_sequence: 0,
        }
    }

    /// Where finished groups go now. A buffer built with
    /// [`BackendChoice::Auto`] changes from memory to disk, once and for
    /// good, when [`Self::memory_bytes`] passes its budget and it has a disk
    /// folder. It then moves what it holds in memory to disk, one group per
    /// push, without losing or reordering a packet.
    pub fn backend_kind(&self) -> BackendKind {
        if self.storage.writes_to_disk {
            BackendKind::Disk
        } else {
            BackendKind::Memory
        }
    }

    /// The payload bytes of the packets the buffer holds now, in memory and
    /// on disk together.
    pub fn size_bytes(&self) -> u64 {
        self.storage.held.size_bytes
    }

    /// The payload bytes the buffer itself holds in memory.
    ///
    /// On disk this is what is still being collected, at most about 8 MiB,
    /// plus whatever a failed write left in memory. In memory with a budget
    /// it stays at or under the budget plus that much, unless there is no
    /// disk folder or the disk fails. A [`ReplaySave`] that is still alive
    /// keeps the memory groups it was taken from on top of this.
    pub fn memory_bytes(&self) -> u64 {
        self.storage.held.memory_bytes
    }

    /// True while a buffer built with [`BackendChoice::Auto`] holds more in
    /// memory than its budget: for a moment while it moves to disk, and for
    /// as long as it has no disk folder or the disk fails.
    pub fn is_over_memory_budget(&self) -> bool {
        self.storage.is_over_memory_budget()
    }

    /// The buffer never holds more than this, whatever arrives: twice the
    /// expected size, and at least 1 MiB. Each packet counts with its
    /// payload plus 64 bytes. To stay under it the buffer drops its oldest
    /// keyframe groups early, and as a last resort everything it holds.
    pub fn size_limit_bytes(&self) -> u64 {
        self.size_limit
    }

    /// How far back a save can reach: from the oldest point still served to
    /// the end of the newest packet of any stream, added up over the
    /// stretches held. Once it has reached the configured length it stays
    /// between that and [`LONGEST_KEYFRAME_SNAP`] more while packets keep
    /// arriving in one epoch and the size limit is not hit.
    pub fn covered_duration(&self) -> Ticks {
        self.closed_runs
            .iter()
            .chain(iter::once(&self.current))
            .fold(Ticks::ZERO, |total, run| total + run.covered())
    }

    pub fn current_epoch(&self) -> EpochId {
        self.epoch
    }

    /// Takes one packet. Packets of all streams arrive interleaved in the
    /// order a muxer would write them, with timestamps on one clock.
    ///
    /// An error means the packet was refused and not stored. After a refused
    /// video packet nothing that follows could be decoded, so video is
    /// dropped until the next keyframe; what was stored before stays.
    pub fn push(&mut self, packet: Packet) -> Result<Pushed, ReplayError> {
        let is_video = packet.stream.is_video();
        if let Err(refusal) = self.check(&packet) {
            if is_video {
                self.current.seal_open_video(&mut self.storage);
            }
            return Err(refusal);
        }
        let outcome = if is_video {
            self.take_video(packet)
        } else {
            self.take_audio(packet);
            PushOutcome::Stored
        };
        self.drop_what_is_older_than_the_length();
        let dropped_everything = self.stay_within_the_size_limit();
        self.storage.write_to_disk_once_over_budget();
        self.move_one_unit_to_disk();
        Ok(Pushed {
            outcome: if dropped_everything {
                PushOutcome::DroppedOverSizeLimit
            } else {
                outcome
            },
            write_failure: self.storage.write_failure.take(),
        })
    }

    /// Call it when stream parameters change or timestamps restart. Packets
    /// pushed before and after are never handed back in one slice. Video
    /// then waits for the next keyframe.
    pub fn begin_epoch(&mut self) -> EpochId {
        self.epoch = EpochId(self.epoch.0 + 1);
        self.current.seal_open_video(&mut self.storage);
        self.current.seal_open_audio(&mut self.storage);
        let closed = mem::replace(&mut self.current, Run::new(self.epoch));
        if closed.group_count() > 0 {
            self.closed_runs.push_back(closed);
        } else {
            self.storage.forgot_run(&closed);
        }
        self.epoch
    }

    /// A disk write failure that no [`Pushed`] has reported yet, as after
    /// [`Self::begin_epoch`].
    pub fn take_write_failure(&mut self) -> Option<ReplayError> {
        self.storage.write_failure.take()
    }

    /// Snapshots the asked range without reading or writing anything, so it
    /// returns at once and cannot fail.
    pub fn save(&mut self, range: SaveRange, after: AfterSave) -> ReplaySave {
        let mut still_asked = match range {
            SaveRange::Last(asked) => Some(Ticks::from_duration_rounded_up(asked)),
            SaveRange::Everything => None,
        };
        let mut slices: Vec<ReplaySlice> = iter::once(&self.current)
            .chain(self.closed_runs.iter().rev())
            .filter_map(|run| run.slice(&mut still_asked))
            .collect();
        slices.reverse();
        if after == AfterSave::ClearBuffer {
            self.clear();
        }
        ReplaySave { slices }
    }

    /// Drops everything held. Saves taken earlier stay readable. Video then
    /// waits for the next keyframe.
    pub fn clear(&mut self) {
        self.closed_runs.clear();
        self.current = Run::new(self.epoch);
        self.storage.held = Held::default();
    }

    #[cfg(test)]
    pub(crate) fn held_weight(&self) -> u64 {
        self.storage.held.weight
    }

    #[cfg(test)]
    pub(crate) fn units_in_memory(&self) -> usize {
        self.storage.held.units_in_memory
    }

    fn check(&self, packet: &Packet) -> Result<(), ReplayError> {
        if packet.duration < Ticks::ZERO {
            return Err(ReplayError::NegativeDuration {
                stream: packet.stream,
                duration: packet.duration,
            });
        }
        match self.current.newest_keyframe {
            Some(previous) if packet.stream.is_video() && packet.keyframe => {
                if packet.pts > previous {
                    Ok(())
                } else {
                    Err(ReplayError::KeyframeNotAfterPrevious {
                        previous,
                        pts: packet.pts,
                    })
                }
            }
            _ => Ok(()),
        }
    }

    fn take_video(&mut self, packet: Packet) -> PushOutcome {
        let run = &mut self.current;
        if packet.keyframe {
            run.seal_open_video(&mut self.storage);
            run.newest_keyframe = Some(packet.pts);
            run.open_video = Some(OpenVideo {
                keyframe: packet.pts,
                continues_newest_group: false,
                unit: OpenUnit::starting_at(packet.pts),
            });
        }
        let Some(open) = run.open_video.as_mut() else {
            return PushOutcome::WaitingForKeyframe;
        };
        self.storage.took(&packet);
        let end = packet.end();
        open.unit.push(self.next_sequence, packet);
        self.next_sequence += 1;
        let keyframe = open.keyframe;
        let is_large = open.unit.size_bytes >= OPEN_UNIT_BYTES;
        run.newest = Some(run.newest.map_or(end, |newest| newest.max(end)));
        if is_large {
            run.seal_open_video(&mut self.storage);
            run.open_video = Some(OpenVideo {
                keyframe,
                continues_newest_group: true,
                unit: OpenUnit::starting_at(keyframe),
            });
        }
        PushOutcome::Stored
    }

    fn take_audio(&mut self, packet: Packet) {
        let run = &mut self.current;
        self.storage.took(&packet);
        run.took(&packet);
        let open = run
            .open_audio
            .get_or_insert_with(|| OpenUnit::starting_at(packet.pts));
        open.push(self.next_sequence, packet);
        self.next_sequence += 1;
        let is_full = open.end - open.start >= AUDIO_UNIT_SPAN
            || open.packets.len() >= AUDIO_UNIT_PACKETS
            || open.size_bytes >= OPEN_UNIT_BYTES;
        if is_full {
            run.seal_open_audio(&mut self.storage);
        }
    }

    fn drop_what_is_older_than_the_length(&mut self) {
        loop {
            let excess = self.covered_duration() - self.length;
            if excess <= Ticks::ZERO {
                break;
            }
            let oldest_is_closed = !self.closed_runs.is_empty();
            let run = self.closed_runs.front_mut().unwrap_or(&mut self.current);
            let Some(run_start) = run.start() else {
                break;
            };
            let next_group = run.group_start(1);
            let freed = match next_group {
                Some(next_keyframe) => Some(next_keyframe - run_start),
                None if oldest_is_closed => Some(run.covered()),
                None => None,
            };
            if freed.is_none_or(|freed| freed > excess) {
                if excess > LONGEST_KEYFRAME_SNAP {
                    run.served_from = Some(run_start + (excess - LONGEST_KEYFRAME_SNAP));
                }
                break;
            }
            if next_group.is_some() {
                run.drop_oldest_group(&mut self.storage);
            } else if let Some(run) = self.closed_runs.pop_front() {
                self.storage.forgot_run(&run);
            }
        }
        if let Some(oldest) = self.closed_runs.front_mut() {
            oldest.drop_audio_no_save_can_ask_for(&mut self.storage);
        }
        self.current
            .drop_audio_no_save_can_ask_for(&mut self.storage);
    }

    fn stay_within_the_size_limit(&mut self) -> bool {
        while self.storage.held.weight > self.size_limit {
            if let Some(oldest) = self.closed_runs.front_mut() {
                if oldest.group_start(1).is_some() {
                    oldest.drop_oldest_group(&mut self.storage);
                } else if let Some(run) = self.closed_runs.pop_front() {
                    self.storage.forgot_run(&run);
                }
            } else if self.current.group_start(1).is_some() {
                self.current.drop_oldest_group(&mut self.storage);
            } else {
                let dropped = mem::replace(&mut self.current, Run::new(self.epoch));
                self.storage.forgot_run(&dropped);
                return true;
            }
        }
        false
    }

    fn move_one_unit_to_disk(&mut self) {
        let storage = &mut self.storage;
        if !storage.writes_to_disk || storage.last_write_failed || storage.held.units_in_memory == 0
        {
            return;
        }
        let in_memory = self
            .closed_runs
            .iter_mut()
            .chain(iter::once(&mut self.current))
            .flat_map(Run::units_mut)
            .find_map(|unit| match &unit.stored {
                StoredUnit::Memory(packets) => Some((packets.clone(), unit)),
                StoredUnit::Disk(_) => None,
            });
        if let Some((packets, unit)) = in_memory
            && let Some(on_disk) = storage.write(&packets)
        {
            unit.stored = StoredUnit::Disk(on_disk);
            storage.held.memory_bytes -= unit.size_bytes;
            storage.held.units_in_memory -= 1;
        }
    }
}
