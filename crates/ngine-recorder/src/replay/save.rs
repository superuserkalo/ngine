use std::sync::Arc;

use super::backend::{GroupReader, StoredGroup};
use super::error::ReplayError;
use super::packet::Packet;
use super::time::Ticks;

/// Counts the stretches between stream parameter changes, from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct EpochId(pub u64);

/// What a save hands back: one slice per uninterrupted stretch, oldest first
/// and newest last.
///
/// Packets of different epochs are never in one slice, so each slice becomes
/// its own clip. A stretch also ends where a failed write lost packets, so
/// two slices can share an epoch. The save stays readable, any number of
/// times and from any thread, while the buffer keeps taking packets, drops
/// old ones or is cleared: it keeps what it needs alive until it is dropped.
#[derive(Debug, Clone, Default)]
pub struct ReplaySave {
    pub(crate) slices: Vec<ReplaySlice>,
}

impl ReplaySave {
    pub fn slices(&self) -> &[ReplaySlice] {
        &self.slices
    }

    /// The real length handed back, all slices together. It is at least what
    /// was asked whenever the buffer held that much, and less than one
    /// keyframe interval more.
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
    pub(crate) duration: Ticks,
    pub(crate) packet_count: usize,
    pub(crate) size_bytes: u64,
    pub(crate) pre_roll: Arc<[Packet]>,
    pub(crate) groups: Arc<[StoredGroup]>,
}

impl ReplaySlice {
    pub fn epoch(&self) -> EpochId {
        self.epoch
    }

    /// The pts of the video keyframe the clip starts on. Audio packets that
    /// come before it are pre-roll for the decoder: trim them with an edit
    /// list that starts here.
    pub fn start(&self) -> Ticks {
        self.start
    }

    /// From [`Self::start`] to the end of the newest video packet.
    pub fn duration(&self) -> Ticks {
        self.duration
    }

    pub fn packet_count(&self) -> usize {
        self.packet_count
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// The packets in the order they were pushed: first the audio pre-roll,
    /// then everything from the starting video keyframe on.
    pub fn packets(&self) -> PacketReader {
        PacketReader {
            group_reader: None,
            next_pre_roll: 0,
            next_group: 0,
            failed: false,
            pre_roll: Arc::clone(&self.pre_roll),
            groups: Arc::clone(&self.groups),
        }
    }
}

/// Reads a slice packet by packet, from disk when the buffer lives there.
/// After the first error it yields nothing more.
#[derive(Debug)]
pub struct PacketReader {
    group_reader: Option<GroupReader>,
    next_pre_roll: usize,
    next_group: usize,
    failed: bool,
    pre_roll: Arc<[Packet]>,
    groups: Arc<[StoredGroup]>,
}

impl PacketReader {
    fn fail(&mut self, error: ReplayError) -> Result<Packet, ReplayError> {
        self.failed = true;
        self.group_reader = None;
        Err(error)
    }
}

impl Iterator for PacketReader {
    type Item = Result<Packet, ReplayError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        if let Some(packet) = self.pre_roll.get(self.next_pre_roll) {
            self.next_pre_roll += 1;
            return Some(Ok(packet.clone()));
        }
        loop {
            if let Some(reader) = self.group_reader.as_mut() {
                match reader.next() {
                    Some(Ok(packet)) => return Some(Ok(packet)),
                    Some(Err(error)) => return Some(self.fail(error)),
                    None => self.group_reader = None,
                }
            }
            let group = self.groups.get(self.next_group)?;
            self.next_group += 1;
            match group.reader() {
                Ok(reader) => self.group_reader = Some(reader),
                Err(error) => return Some(self.fail(error)),
            }
        }
    }
}
