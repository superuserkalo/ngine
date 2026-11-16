use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::error::ReplayError;
use super::packet::{Packet, StreamIndex};
use super::session::Session;
use super::time::Ticks;
use super::unit::Sequenced;

const WRITE_BUFFER_BYTES: usize = 64 * 1024;

pub(crate) type Sink = Box<dyn Write + Send>;
pub(crate) type CreateSink = Box<dyn FnMut(&Path) -> io::Result<Sink> + Send>;

pub(crate) struct DiskStore {
    create_sink: CreateSink,
    next_unit_number: u64,
    session: Arc<Session>,
}

#[derive(Debug)]
struct UnitFile {
    path: PathBuf,
    _session: Arc<Session>,
}

impl Drop for UnitFile {
    fn drop(&mut self) {
        drop(fs::remove_file(&self.path));
    }
}

#[derive(Debug, Clone, Copy)]
struct PacketEntry {
    sequence: u64,
    stream: StreamIndex,
    pts: Ticks,
    dts: Ticks,
    duration: Ticks,
    keyframe: bool,
    len: usize,
}

impl DiskStore {
    pub(crate) fn create(folder: &Path) -> Result<Self, ReplayError> {
        Self::with_sink(folder, Box::new(create_file_sink))
    }

    pub(crate) fn with_sink(folder: &Path, create_sink: CreateSink) -> Result<Self, ReplayError> {
        Ok(Self {
            create_sink,
            next_unit_number: 0,
            session: Arc::new(Session::create(folder)?),
        })
    }

    pub(crate) fn write(&mut self, packets: &[Sequenced]) -> Result<DiskUnit, ReplayError> {
        let path = self.session.unit_path(self.next_unit_number);
        self.next_unit_number += 1;
        let sink = (self.create_sink)(&path)
            .map_err(|error| ReplayError::storage("create a replay file", &path, error))?;
        let file = Arc::new(UnitFile {
            path,
            _session: Arc::clone(&self.session),
        });
        write_payloads(sink, packets)
            .map_err(|error| ReplayError::storage("write a replay file", &file.path, error))?;
        Ok(DiskUnit {
            entries: packets
                .iter()
                .map(|sequenced| PacketEntry {
                    sequence: sequenced.sequence,
                    stream: sequenced.packet.stream,
                    pts: sequenced.packet.pts,
                    dts: sequenced.packet.dts,
                    duration: sequenced.packet.duration,
                    keyframe: sequenced.packet.keyframe,
                    len: sequenced.packet.data.len(),
                })
                .collect(),
            file,
        })
    }
}

fn create_file_sink(path: &Path) -> io::Result<Sink> {
    Ok(Box::new(File::create_new(path)?))
}

fn write_payloads(sink: Sink, packets: &[Sequenced]) -> io::Result<()> {
    let mut writer = BufWriter::with_capacity(WRITE_BUFFER_BYTES, sink);
    for sequenced in packets {
        writer.write_all(&sequenced.packet.data)?;
    }
    writer.flush()
}

#[derive(Clone)]
pub(crate) struct DiskUnit {
    entries: Arc<[PacketEntry]>,
    file: Arc<UnitFile>,
}

impl fmt::Debug for DiskUnit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiskUnit")
            .field("path", &self.file.path)
            .field("packets", &self.entries.len())
            .finish()
    }
}

impl DiskUnit {
    pub(crate) fn reader(&self) -> Result<DiskUnitReader, ReplayError> {
        let file = File::open(&self.file.path)
            .map_err(|error| ReplayError::storage("open a replay file", &self.file.path, error))?;
        Ok(DiskUnitReader {
            reader: BufReader::new(file),
            next: 0,
            unit: self.clone(),
        })
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.file.path
    }
}

#[derive(Debug)]
pub(crate) struct DiskUnitReader {
    reader: BufReader<File>,
    next: usize,
    unit: DiskUnit,
}

impl Iterator for DiskUnitReader {
    type Item = Result<Sequenced, ReplayError>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = *self.unit.entries.get(self.next)?;
        self.next += 1;
        let mut data = vec![0; entry.len];
        Some(match self.reader.read_exact(&mut data) {
            Ok(()) => Ok(Sequenced {
                sequence: entry.sequence,
                packet: Packet {
                    stream: entry.stream,
                    pts: entry.pts,
                    dts: entry.dts,
                    duration: entry.duration,
                    keyframe: entry.keyframe,
                    data: data.into(),
                },
            }),
            Err(error) => {
                self.next = self.unit.entries.len();
                Err(ReplayError::storage(
                    "read a replay file",
                    &self.unit.file.path,
                    error,
                ))
            }
        })
    }
}
