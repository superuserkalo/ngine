use std::sync::Arc;

use super::error::ReplayError;
use super::packet::Packet;
use super::time::Ticks;
use super::unit::{Sequenced, StoredUnit, UnitReader};

/// Counts the stretches between stream parameter changes, from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct EpochId(pub u64);

/// What a save hands back: one slice per uninterrupted stretch, oldest first
/// and newest last.
///
/// Packets of different epochs are never in one slice, so each slice becomes
/// its own clip. A stretch also ends where the buffer was cleared or had to
/// drop everything to stay within its size limit, so two slices can share an
/// epoch. The save stays readable, any number of times and from any thread,
/// while the buffer keeps taking packets, drops old ones or is cleared: it
/// keeps what it needs, in memory or on disk, until it is dropped.
#[derive(Debug, Clone, Default)]
pub struct ReplaySave {
    pub(crate) slices: Vec<ReplaySlice>,
}

impl ReplaySave {
    pub fn slices(&self) -> &[ReplaySlice] {
        &self.slices
    }

    /// The real length handed back, all slices together. It is at least what
    /// was asked whenever the buffer held that much, and at most
    /// [`LONGEST_KEYFRAME_SNAP`](super::LONGEST_KEYFRAME_SNAP) more.
    pub fn duration(&self) -> Ticks {
        self.slices
            .iter()
            .fold(Ticks::ZERO, |total, slice| total + slice.duration)
    }

    pub fn is_empty(&self) -> bool {
        self.slices.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct ReplaySlice {
    pub(crate) epoch: EpochId,
    pub(crate) start: Ticks,
    pub(crate) video_start: Ticks,
    pub(crate) duration: Ticks,
    pub(crate) stored_bytes: u64,
    pub(crate) video: Arc<[StoredUnit]>,
    pub(crate) audio: Arc<[StoredUnit]>,
}

impl ReplaySlice {
    pub fn epoch(&self) -> EpochId {
        self.epoch
    }

    /// Where the clip starts. Trim what comes before it with an edit list:
    /// the audio pre-roll always, and the video from [`Self::video_start`]
    /// when that is earlier.
    pub fn start(&self) -> Ticks {
        self.start
    }

    /// The pts of the video keyframe the video packets start on. It equals
    /// [`Self::start`] unless the keyframe is more than
    /// [`LONGEST_KEYFRAME_SNAP`](super::LONGEST_KEYFRAME_SNAP) older than the
    /// asked start, as after a long still picture.
    pub fn video_start(&self) -> Ticks {
        self.video_start
    }

    /// From [`Self::start`] to the end of the newest packet of any stream.
    pub fn duration(&self) -> Ticks {
        self.duration
    }

    /// The payload bytes behind this slice. Reading yields a little less,
    /// because audio older than the pre-roll is left out.
    pub fn stored_bytes(&self) -> u64 {
        self.stored_bytes
    }

    /// The packets in the order they were pushed: all video from the
    /// keyframe at [`Self::video_start`], and every audio packet that ends
    /// later than [`AUDIO_PRE_ROLL`](super::AUDIO_PRE_ROLL) before
    /// [`Self::start`].
    pub fn packets(&self) -> PacketReader {
        PacketReader {
            video: Track::new(&self.video, None),
            audio: Track::new(&self.audio, Some(self.start - super::AUDIO_PRE_ROLL)),
        }
    }
}

/// Reads a slice packet by packet, from disk where the buffer stored it.
///
/// An error item says the packets of one file could not be read. The reader
/// then goes on with what follows, and video continues at the next keyframe
/// group, so a caller can still write a clip from the rest.
#[derive(Debug)]
pub struct PacketReader {
    video: Track,
    audio: Track,
}

#[derive(Debug)]
struct Track {
    reader: Option<UnitReader>,
    ahead: Option<Sequenced>,
    next_unit: usize,
    ending_after: Option<Ticks>,
    units: Arc<[StoredUnit]>,
}

impl Track {
    fn new(units: &Arc<[StoredUnit]>, ending_after: Option<Ticks>) -> Self {
        Self {
            reader: None,
            ahead: None,
            next_unit: 0,
            ending_after,
            units: Arc::clone(units),
        }
    }

    fn next_sequence(&mut self) -> Result<Option<u64>, ReplayError> {
        while self.ahead.is_none() {
            let Some(reader) = self.reader.as_mut() else {
                let Some(unit) = self.units.get(self.next_unit) else {
                    return Ok(None);
                };
                self.next_unit += 1;
                self.reader = Some(unit.reader()?);
                continue;
            };
            match reader.next() {
                None => self.reader = None,
                Some(Err(error)) => {
                    self.reader = None;
                    return Err(error);
                }
                Some(Ok(sequenced)) => {
                    if self
                        .ending_after
                        .is_none_or(|too_old| sequenced.packet.end() > too_old)
                    {
                        self.ahead = Some(sequenced);
                    }
                }
            }
        }
        Ok(self.ahead.as_ref().map(|sequenced| sequenced.sequence))
    }
}

impl Iterator for PacketReader {
    type Item = Result<Packet, ReplayError>;

    fn next(&mut self) -> Option<Self::Item> {
        let video = match self.video.next_sequence() {
            Ok(sequence) => sequence,
            Err(error) => return Some(Err(error)),
        };
        let audio = match self.audio.next_sequence() {
            Ok(sequence) => sequence,
            Err(error) => return Some(Err(error)),
        };
        let track = match (video, audio) {
            (None, None) => return None,
            (Some(_), None) => &mut self.video,
            (Some(video), Some(audio)) if video < audio => &mut self.video,
            (_, Some(_)) => &mut self.audio,
        };
        track.ahead.take().map(|sequenced| Ok(sequenced.packet))
    }
}
