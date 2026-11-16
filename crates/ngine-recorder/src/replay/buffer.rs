use std::collections::VecDeque;
use std::fmt;
use std::iter;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::backend::{Backend, StoredGroup};
use super::budget::{
    BackendChoice, BackendKind, ReplayLength, choose_backend, expected_size_bytes,
};
use super::disk::DiskBackend;
use super::error::ReplayError;
use super::memory::MemoryBackend;
use super::packet::Packet;
use super::save::{EpochId, ReplaySave, ReplaySlice};
use super::time::Ticks;

/// A slice carries the audio packets that end later than this long before
/// its first video keyframe, so a decoder has primed before the clip starts.
pub const AUDIO_PRE_ROLL: Ticks = Ticks::from_millis(80);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayConfig {
    pub length: ReplayLength,
    /// The cap the encoder is held to, all streams together. With `length`
    /// it gives the size that [`BackendChoice::Auto`] compares to its budget.
    pub max_bitrate_bits_per_second: u64,
    pub backend: BackendChoice,
    /// Where a disk buffer keeps its files. It must exist. The buffer only
    /// touches the `ngine-replay` folder it creates inside.
    pub disk_folder: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Stored,
    /// The packet was dropped because the buffer starts on a video keyframe
    /// and none has arrived since the start, the last clear, the last epoch
    /// change or the last failed write.
    WaitingForKeyframe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveRange {
    /// At least this much, counted back from the end of the newest video
    /// packet, or everything when the buffer holds less.
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
/// One thread owns it and pushes. [`Self::save`] runs on that thread and only
/// takes a snapshot; the [`ReplaySave`] it returns is read on any thread.
pub struct ReplayBuffer {
    length: Ticks,
    backend: Box<dyn Backend>,
    closed_runs: VecDeque<ClosedRun>,
    current_run: Option<CurrentRun>,
    epoch: EpochId,
    recent_audio: VecDeque<Packet>,
    size_bytes: u64,
}

#[derive(Debug, Clone)]
struct GroupSpan {
    start: Ticks,
    end: Ticks,
    size_bytes: u64,
    packet_count: usize,
    pre_roll: Arc<[Packet]>,
}

#[derive(Debug, Clone)]
struct SealedGroup {
    span: GroupSpan,
    stored: StoredGroup,
}

struct ClosedRun {
    epoch: EpochId,
    groups: VecDeque<SealedGroup>,
}

struct CurrentRun {
    epoch: EpochId,
    sealed: VecDeque<SealedGroup>,
    open: GroupSpan,
    newest_video_pts: Ticks,
}

impl fmt::Debug for ReplayBuffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplayBuffer")
            .field("backend", &self.backend.kind())
            .field("length", &self.length)
            .field("covered_duration", &self.covered_duration())
            .field("size_bytes", &self.size_bytes)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

impl ReplayBuffer {
    /// A disk buffer first removes what crashed runs left in the folder.
    pub fn new(config: &ReplayConfig) -> Result<Self, ReplayError> {
        let expected = expected_size_bytes(config.max_bitrate_bits_per_second, config.length);
        let backend: Box<dyn Backend> = match choose_backend(expected, config.backend) {
            BackendKind::Memory => Box::new(MemoryBackend::default()),
            BackendKind::Disk => Box::new(DiskBackend::create(&config.disk_folder)?),
        };
        Ok(Self::with_backend(config.length, backend))
    }

    pub(crate) fn with_backend(length: ReplayLength, backend: Box<dyn Backend>) -> Self {
        Self {
            length: Ticks::from_duration_rounded_up(length.get()),
            backend,
            closed_runs: VecDeque::new(),
            current_run: None,
            epoch: EpochId::default(),
            recent_audio: VecDeque::new(),
            size_bytes: 0,
        }
    }

    pub fn backend_kind(&self) -> BackendKind {
        self.backend.kind()
    }

    /// The payload bytes of the packets the buffer holds now.
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// From the oldest video keyframe held to the end of the newest video
    /// packet, added up over the stretches the buffer holds. Once it has
    /// reached the configured length it stays at or above it while packets
    /// keep arriving in one epoch.
    pub fn covered_duration(&self) -> Ticks {
        let closed = self
            .closed_runs
            .iter()
            .filter_map(|run| Some(run.groups.back()?.span.end - run.groups.front()?.span.start))
            .fold(Ticks::ZERO, |total, run| total + run);
        match &self.current_run {
            Some(run) => closed + (run.open.end - run.start()),
            None => closed,
        }
    }

    pub fn current_epoch(&self) -> EpochId {
        self.epoch
    }

    /// Takes one packet. Packets of all streams arrive interleaved in the
    /// order a muxer would write them.
    ///
    /// An error means this packet was not stored. After a
    /// [`ReplayError::Storage`] error the group that was being written is
    /// lost as well, everything older is kept, and the buffer takes packets
    /// again from the next video keyframe.
    pub fn push(&mut self, packet: Packet) -> Result<PushOutcome, ReplayError> {
        if packet.duration < Ticks::ZERO {
            return Err(ReplayError::NegativeDuration {
                stream: packet.stream,
                duration: packet.duration,
            });
        }
        if packet.stream.is_video() && packet.keyframe {
            self.start_group(&packet)?;
        }
        if self.current_run.is_none() {
            return Ok(PushOutcome::WaitingForKeyframe);
        }
        self.append(packet)?;
        self.evict_while_longer_than_needed();
        Ok(PushOutcome::Stored)
    }

    /// Call it when stream parameters change or timestamps restart. Packets
    /// pushed before and after are never handed back in one slice. The
    /// buffer then waits for the next video keyframe.
    ///
    /// The new epoch has begun even when this returns an error, which says
    /// that the newest group of the old epoch could not be written.
    pub fn begin_epoch(&mut self) -> Result<EpochId, ReplayError> {
        self.epoch = EpochId(self.epoch.0 + 1);
        self.close_current_run()?;
        Ok(self.epoch)
    }

    /// Snapshots the asked range. Each slice starts at the video keyframe at
    /// or before the asked start, so the save is never shorter than asked
    /// unless the buffer holds less.
    ///
    /// It does not read packet data, so it returns quickly. After a
    /// [`ReplayError::Storage`] error the newest group is lost and a second
    /// call saves what is left.
    pub fn save(&mut self, range: SaveRange, after: AfterSave) -> Result<ReplaySave, ReplayError> {
        let open_group = self.snapshot_open_group()?;
        let mut still_asked = match range {
            SaveRange::Last(asked) => Some(Ticks::from_duration_rounded_up(asked)),
            SaveRange::Everything => None,
        };
        let mut slices = Vec::new();
        if let (Some(run), Some(open_group)) = (&self.current_run, &open_group) {
            let groups = run.sealed.iter().chain(iter::once(open_group));
            slices.extend(take_slice(run.epoch, groups, &mut still_asked));
        }
        for run in self.closed_runs.iter().rev() {
            slices.extend(take_slice(run.epoch, run.groups.iter(), &mut still_asked));
        }
        slices.reverse();
        if after == AfterSave::ClearBuffer {
            self.clear();
        }
        Ok(ReplaySave { slices })
    }

    /// Drops everything held. Saves taken earlier stay readable. The buffer
    /// then waits for the next video keyframe.
    pub fn clear(&mut self) {
        self.backend.discard_open_group();
        self.closed_runs.clear();
        self.current_run = None;
        self.recent_audio.clear();
        self.size_bytes = 0;
    }

    fn start_group(&mut self, keyframe: &Packet) -> Result<(), ReplayError> {
        let (epoch, sealed, previous_end, pre_roll) = match self.current_run.take() {
            Some(run) if keyframe.pts <= run.open.start => {
                let previous = run.open.start;
                self.current_run = Some(run);
                return Err(ReplayError::KeyframeNotAfterPrevious {
                    previous,
                    pts: keyframe.pts,
                });
            }
            Some(run) => match self.backend.seal_open_group() {
                Ok(stored) => {
                    let previous_end = run.open.end;
                    let mut sealed = run.sealed;
                    sealed.push_back(SealedGroup {
                        span: run.open,
                        stored,
                    });
                    let pre_roll = self.pre_roll_before(keyframe.pts);
                    (run.epoch, sealed, previous_end, pre_roll)
                }
                Err(error) => {
                    self.lose_open_group(run);
                    return Err(error);
                }
            },
            None => (self.epoch, VecDeque::new(), keyframe.pts, Arc::default()),
        };
        if let Err(error) = self.backend.open_group() {
            self.backend.discard_open_group();
            self.keep_as_closed_run(epoch, sealed);
            return Err(error);
        }
        self.current_run = Some(CurrentRun {
            epoch,
            sealed,
            open: GroupSpan {
                start: keyframe.pts,
                end: previous_end.max(keyframe.pts),
                size_bytes: 0,
                packet_count: 0,
                pre_roll,
            },
            newest_video_pts: keyframe.pts,
        });
        Ok(())
    }

    fn append(&mut self, packet: Packet) -> Result<(), ReplayError> {
        let Some(mut run) = self.current_run.take() else {
            return Ok(());
        };
        if let Err(error) = self.backend.append(&packet) {
            self.lose_open_group(run);
            return Err(error);
        }
        run.open.size_bytes += packet.size_bytes();
        run.open.packet_count += 1;
        self.size_bytes += packet.size_bytes();
        if packet.stream.is_video() {
            run.open.end = run.open.end.max(packet.end());
            run.newest_video_pts = run.newest_video_pts.max(packet.pts);
        } else {
            self.recent_audio.push_back(packet);
        }
        let too_old_for_pre_roll = run.newest_video_pts - AUDIO_PRE_ROLL;
        while self
            .recent_audio
            .front()
            .is_some_and(|oldest| oldest.end() <= too_old_for_pre_roll)
        {
            self.recent_audio.pop_front();
        }
        self.current_run = Some(run);
        Ok(())
    }

    fn pre_roll_before(&self, keyframe_pts: Ticks) -> Arc<[Packet]> {
        let too_old = keyframe_pts - AUDIO_PRE_ROLL;
        self.recent_audio
            .iter()
            .filter(|packet| packet.end() > too_old)
            .cloned()
            .collect()
    }

    fn snapshot_open_group(&mut self) -> Result<Option<SealedGroup>, ReplayError> {
        let Some(run) = self.current_run.take() else {
            return Ok(None);
        };
        match self.backend.snapshot_open_group() {
            Ok(stored) => {
                let open_group = SealedGroup {
                    span: run.open.clone(),
                    stored,
                };
                self.current_run = Some(run);
                Ok(Some(open_group))
            }
            Err(error) => {
                self.lose_open_group(run);
                Err(error)
            }
        }
    }

    fn close_current_run(&mut self) -> Result<(), ReplayError> {
        let Some(run) = self.current_run.take() else {
            return Ok(());
        };
        match self.backend.seal_open_group() {
            Ok(stored) => {
                let mut groups = run.sealed;
                groups.push_back(SealedGroup {
                    span: run.open,
                    stored,
                });
                self.keep_as_closed_run(run.epoch, groups);
                Ok(())
            }
            Err(error) => {
                self.lose_open_group(run);
                Err(error)
            }
        }
    }

    fn lose_open_group(&mut self, run: CurrentRun) {
        self.backend.discard_open_group();
        self.size_bytes -= run.open.size_bytes;
        self.keep_as_closed_run(run.epoch, run.sealed);
    }

    fn keep_as_closed_run(&mut self, epoch: EpochId, groups: VecDeque<SealedGroup>) {
        self.recent_audio.clear();
        if !groups.is_empty() {
            self.closed_runs.push_back(ClosedRun { epoch, groups });
        }
    }

    fn evict_while_longer_than_needed(&mut self) {
        while let Some(freed) = self.duration_freed_by_dropping_oldest_group() {
            if self.covered_duration() - freed < self.length {
                break;
            }
            self.drop_oldest_group();
        }
    }

    fn duration_freed_by_dropping_oldest_group(&self) -> Option<Ticks> {
        if let Some(run) = self.closed_runs.front() {
            let oldest = &run.groups.front()?.span;
            let next_start = run
                .groups
                .get(1)
                .map_or(run.groups.back()?.span.end, |next| next.span.start);
            return Some(next_start - oldest.start);
        }
        let run = self.current_run.as_ref()?;
        let oldest = &run.sealed.front()?.span;
        let next_start = run
            .sealed
            .get(1)
            .map_or(run.open.start, |next| next.span.start);
        Some(next_start - oldest.start)
    }

    fn drop_oldest_group(&mut self) {
        let dropped = if let Some(run) = self.closed_runs.front_mut() {
            let dropped = run.groups.pop_front();
            if run.groups.is_empty() {
                self.closed_runs.pop_front();
            }
            dropped
        } else {
            self.current_run
                .as_mut()
                .and_then(|run| run.sealed.pop_front())
        };
        if let Some(dropped) = dropped {
            self.size_bytes -= dropped.span.size_bytes;
        }
    }
}

impl CurrentRun {
    fn start(&self) -> Ticks {
        self.sealed
            .front()
            .map_or(self.open.start, |oldest| oldest.span.start)
    }
}

fn take_slice<'a, Groups>(
    epoch: EpochId,
    groups: Groups,
    still_asked: &mut Option<Ticks>,
) -> Option<ReplaySlice>
where
    Groups: DoubleEndedIterator<Item = &'a SealedGroup> + Clone,
{
    if *still_asked == Some(Ticks::ZERO) {
        return None;
    }
    let end = groups.clone().next_back()?.span.end;
    let run_start = groups.clone().next()?.span.start;
    let asked_start = still_asked
        .filter(|&asked| asked < end - run_start)
        .map(|asked| end - asked);
    let mut taken = Vec::new();
    for group in groups.rev() {
        taken.push(group);
        if asked_start.is_some_and(|asked_start| group.span.start <= asked_start) {
            break;
        }
    }
    taken.reverse();
    let first = taken.first()?;
    let start = first.span.start;
    let duration = end - start;
    if let Some(asked) = still_asked {
        *asked = (*asked - duration).max(Ticks::ZERO);
    }
    let pre_roll = Arc::clone(&first.span.pre_roll);
    Some(ReplaySlice {
        epoch,
        start,
        duration,
        packet_count: pre_roll.len()
            + taken
                .iter()
                .map(|group| group.span.packet_count)
                .sum::<usize>(),
        size_bytes: pre_roll.iter().map(Packet::size_bytes).sum::<u64>()
            + taken.iter().map(|group| group.span.size_bytes).sum::<u64>(),
        pre_roll,
        groups: taken.iter().map(|group| group.stored.clone()).collect(),
    })
}
