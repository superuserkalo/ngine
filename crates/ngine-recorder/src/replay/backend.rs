use std::sync::Arc;

use super::budget::BackendKind;
use super::disk::{DiskGroup, DiskGroupReader};
use super::error::ReplayError;
use super::packet::Packet;

pub(crate) trait Backend: Send {
    fn kind(&self) -> BackendKind;
    fn open_group(&mut self) -> Result<(), ReplayError>;
    fn append(&mut self, packet: &Packet) -> Result<(), ReplayError>;
    fn snapshot_open_group(&mut self) -> Result<StoredGroup, ReplayError>;
    fn seal_open_group(&mut self) -> Result<StoredGroup, ReplayError>;
    fn discard_open_group(&mut self);
}

#[derive(Debug, Clone)]
pub(crate) enum StoredGroup {
    Memory(Arc<[Packet]>),
    Disk(DiskGroup),
}

impl StoredGroup {
    pub(crate) fn reader(&self) -> Result<GroupReader, ReplayError> {
        match self {
            Self::Memory(packets) => Ok(GroupReader::Memory {
                packets: Arc::clone(packets),
                next: 0,
            }),
            Self::Disk(group) => group.reader().map(GroupReader::Disk),
        }
    }
}

#[derive(Debug)]
pub(crate) enum GroupReader {
    Memory { packets: Arc<[Packet]>, next: usize },
    Disk(DiskGroupReader),
}

impl Iterator for GroupReader {
    type Item = Result<Packet, ReplayError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Memory { packets, next } => {
                let packet = packets.get(*next)?.clone();
                *next += 1;
                Some(Ok(packet))
            }
            Self::Disk(reader) => reader.next(),
        }
    }
}
