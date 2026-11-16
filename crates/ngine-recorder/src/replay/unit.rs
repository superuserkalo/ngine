use std::sync::Arc;

use super::disk::{DiskUnit, DiskUnitReader};
use super::error::ReplayError;
use super::packet::Packet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sequenced {
    pub(crate) sequence: u64,
    pub(crate) packet: Packet,
}

#[derive(Debug, Clone)]
pub(crate) enum StoredUnit {
    Memory(Arc<[Sequenced]>),
    Disk(DiskUnit),
}

impl StoredUnit {
    pub(crate) fn reader(&self) -> Result<UnitReader, ReplayError> {
        match self {
            Self::Memory(packets) => Ok(UnitReader::Memory {
                packets: Arc::clone(packets),
                next: 0,
            }),
            Self::Disk(unit) => unit.reader().map(UnitReader::Disk),
        }
    }
}

#[derive(Debug)]
pub(crate) enum UnitReader {
    Memory {
        packets: Arc<[Sequenced]>,
        next: usize,
    },
    Disk(DiskUnitReader),
}

impl Iterator for UnitReader {
    type Item = Result<Sequenced, ReplayError>;

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
