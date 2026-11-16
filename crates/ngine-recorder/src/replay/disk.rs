use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::backend::{Backend, StoredGroup};
use super::budget::BackendKind;
use super::error::ReplayError;
use super::packet::{Packet, StreamIndex};
use super::session::Session;
use super::time::Ticks;

const WRITE_BUFFER_BYTES: usize = 64 * 1024;

pub(crate) type Sink = Box<dyn Write + Send>;
pub(crate) type CreateSink = Box<dyn FnMut(&Path) -> io::Result<Sink> + Send>;

pub(crate) struct DiskBackend {
    create_sink: CreateSink,
    next_group_number: u64,
    open: Option<OpenGroupFile>,
    session: Arc<Session>,
}

struct OpenGroupFile {
    writer: BufWriter<Sink>,
    entries: Vec<PacketEntry>,
    file: Arc<GroupFile>,
}

#[derive(Debug)]
struct GroupFile {
    path: PathBuf,
    _session: Arc<Session>,
}

impl Drop for GroupFile {
    fn drop(&mut self) {
        drop(fs::remove_file(&self.path));
    }
}

#[derive(Debug, Clone, Copy)]
struct PacketEntry {
    stream: StreamIndex,
    pts: Ticks,
    dts: Ticks,
    duration: Ticks,
    keyframe: bool,
    len: usize,
}

impl DiskBackend {
    pub(crate) fn create(folder: &Path) -> Result<Self, ReplayError> {
        Self::with_sink(folder, Box::new(create_file_sink))
    }

    pub(crate) fn with_sink(folder: &Path, create_sink: CreateSink) -> Result<Self, ReplayError> {
        Ok(Self {
            create_sink,
            next_group_number: 0,
            open: None,
            session: Arc::new(Session::create(folder)?),
        })
    }
}

fn create_file_sink(path: &Path) -> io::Result<Sink> {
    Ok(Box::new(File::create_new(path)?))
}

fn no_open_group() -> ReplayError {
    ReplayError::storage(
        "use a replay group file before opening one",
        PathBuf::new(),
        io::Error::from(io::ErrorKind::NotFound),
    )
}

impl Backend for DiskBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Disk
    }

    fn open_group(&mut self) -> Result<(), ReplayError> {
        self.open = None;
        let path = self.session.group_path(self.next_group_number);
        self.next_group_number += 1;
        match (self.create_sink)(&path) {
            Ok(sink) => {
                self.open = Some(OpenGroupFile {
                    writer: BufWriter::with_capacity(WRITE_BUFFER_BYTES, sink),
                    entries: Vec::new(),
                    file: Arc::new(GroupFile {
                        path,
                        _session: Arc::clone(&self.session),
                    }),
                });
                Ok(())
            }
            Err(error) => Err(ReplayError::storage(
                "create a replay group file",
                path,
                error,
            )),
        }
    }

    fn append(&mut self, packet: &Packet) -> Result<(), ReplayError> {
        let open = self.open.as_mut().ok_or_else(no_open_group)?;
        open.writer.write_all(&packet.data).map_err(|error| {
            ReplayError::storage("write to a replay group file", &open.file.path, error)
        })?;
        open.entries.push(PacketEntry {
            stream: packet.stream,
            pts: packet.pts,
            dts: packet.dts,
            duration: packet.duration,
            keyframe: packet.keyframe,
            len: packet.data.len(),
        });
        Ok(())
    }

    fn snapshot_open_group(&mut self) -> Result<StoredGroup, ReplayError> {
        let open = self.open.as_mut().ok_or_else(no_open_group)?;
        open.writer.flush().map_err(|error| {
            ReplayError::storage("write to a replay group file", &open.file.path, error)
        })?;
        Ok(StoredGroup::Disk(DiskGroup {
            entries: open.entries.as_slice().into(),
            file: Arc::clone(&open.file),
        }))
    }

    fn seal_open_group(&mut self) -> Result<StoredGroup, ReplayError> {
        let OpenGroupFile {
            mut writer,
            entries,
            file,
        } = self.open.take().ok_or_else(no_open_group)?;
        writer.flush().map_err(|error| {
            ReplayError::storage("write to a replay group file", &file.path, error)
        })?;
        drop(writer);
        Ok(StoredGroup::Disk(DiskGroup {
            entries: entries.into(),
            file,
        }))
    }

    fn discard_open_group(&mut self) {
        self.open = None;
    }
}

#[derive(Clone)]
pub(crate) struct DiskGroup {
    entries: Arc<[PacketEntry]>,
    file: Arc<GroupFile>,
}

impl fmt::Debug for DiskGroup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiskGroup")
            .field("path", &self.file.path)
            .field("packets", &self.entries.len())
            .finish()
    }
}

impl DiskGroup {
    pub(crate) fn reader(&self) -> Result<DiskGroupReader, ReplayError> {
        let file = File::open(&self.file.path).map_err(|error| {
            ReplayError::storage("open a replay group file", &self.file.path, error)
        })?;
        Ok(DiskGroupReader {
            reader: BufReader::new(file),
            next: 0,
            group: self.clone(),
        })
    }
}

#[derive(Debug)]
pub(crate) struct DiskGroupReader {
    reader: BufReader<File>,
    next: usize,
    group: DiskGroup,
}

impl Iterator for DiskGroupReader {
    type Item = Result<Packet, ReplayError>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = *self.group.entries.get(self.next)?;
        self.next += 1;
        let mut data = vec![0; entry.len];
        Some(match self.reader.read_exact(&mut data) {
            Ok(()) => Ok(Packet {
                stream: entry.stream,
                pts: entry.pts,
                dts: entry.dts,
                duration: entry.duration,
                keyframe: entry.keyframe,
                data: data.into(),
            }),
            Err(error) => Err(ReplayError::storage(
                "read a replay group file",
                &self.group.file.path,
                error,
            )),
        })
    }
}
