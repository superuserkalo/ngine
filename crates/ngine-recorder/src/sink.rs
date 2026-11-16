use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use ngine_media::{Mp4Layout, Mp4Writer, Packet, StreamInfo};

use crate::error::RecorderError;
use crate::event::{Events, RecorderEvent};
use crate::markers::Journal;
use crate::replay::ReplaySlice;
use crate::time::{Rebase, duration_from_video_units, media_packet};

const QUEUED_PACKETS: usize = 2048;

enum Message {
    Packet(Packet),
    Slice { slice: ReplaySlice, rebase: Rebase },
    Marker(u64),
    Sync(SyncSender<()>),
    Finish { continues: bool },
}

pub(crate) struct Sink {
    sender: SyncSender<Message>,
    failed: Arc<AtomicBool>,
    path: PathBuf,
    thread: JoinHandle<()>,
}

impl Sink {
    pub(crate) fn create(
        path: &Path,
        streams: &[StreamInfo],
        events: Events,
    ) -> Result<Self, RecorderError> {
        let writer = Mp4Writer::create(path, streams, Mp4Layout::Recording)?;
        let (sender, receiver) = mpsc::sync_channel(QUEUED_PACKETS);
        let failed = Arc::new(AtomicBool::new(false));
        let file = OpenFile {
            writer: Some(writer),
            journal: Journal::new(path),
            path: path.to_owned(),
            video_end: 0,
            failed: Arc::clone(&failed),
            events,
        };
        let thread = thread::Builder::new()
            .name("ngine-file-writer".to_owned())
            .spawn(move || file.run(&receiver))
            .map_err(|error| RecorderError::file("start the writer thread for", path, error))?;
        Ok(Self {
            sender,
            failed,
            path: path.to_owned(),
            thread,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    fn send(&self, message: Message) {
        if self.sender.send(message).is_err() {
            self.failed.store(true, Ordering::Release);
        }
    }

    pub(crate) fn packet(&self, packet: Packet) {
        self.send(Message::Packet(packet));
    }

    pub(crate) fn slice(&self, slice: ReplaySlice, rebase: Rebase) {
        self.send(Message::Slice { slice, rebase });
    }

    pub(crate) fn marker(&self, micros: u64) {
        self.send(Message::Marker(micros));
    }

    pub(crate) fn wait_until_written(&self) {
        let (done, wait) = mpsc::sync_channel(1);
        self.send(Message::Sync(done));
        let _writer_gone = wait.recv();
    }

    pub(crate) fn finish(self, continues: bool) -> JoinHandle<()> {
        self.send(Message::Finish { continues });
        self.thread
    }
}

struct OpenFile {
    writer: Option<Mp4Writer>,
    journal: Journal,
    path: PathBuf,
    video_end: i64,
    failed: Arc<AtomicBool>,
    events: Events,
}

impl OpenFile {
    fn fail(&mut self, error: RecorderError) {
        if self.writer.take().is_some() {
            self.failed.store(true, Ordering::Release);
            self.events.send(RecorderEvent::RecordingFailed {
                file: self.path.clone(),
                length: duration_from_video_units(self.video_end),
                error,
            });
        }
    }

    fn write(&mut self, packet: &Packet) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        match writer.write(packet) {
            Ok(()) => {
                if packet.stream == 0
                    && let Some(end) = packet.end()
                {
                    self.video_end = self.video_end.max(end);
                }
            }
            Err(error) => self.fail(error.into()),
        }
    }

    fn write_slice(&mut self, slice: &ReplaySlice, rebase: Rebase) {
        for stored in slice.packets().flatten() {
            if let Some(packet) = rebase.apply(media_packet(&stored)) {
                self.write(&packet);
            }
        }
    }

    fn add_marker(&mut self, micros: u64) {
        if let Err(error) = self.journal.add(micros) {
            eprintln!(
                "ngine: a marker could not be stored for {}: {error}",
                self.path.display()
            );
        }
    }

    fn finish(mut self, continues: bool) {
        let Some(writer) = self.writer.take() else {
            return;
        };
        let length = duration_from_video_units(self.video_end);
        if let Err(error) = writer.finish() {
            self.failed.store(true, Ordering::Release);
            self.events.send(RecorderEvent::RecordingFailed {
                file: self.path,
                length,
                error: error.into(),
            });
            return;
        }
        if let Err(error) = self.journal.seal_into(&self.path) {
            eprintln!(
                "ngine: the markers of {} stay in their own file: {error}",
                self.path.display()
            );
        }
        self.events.send(RecorderEvent::RecordingFinished {
            file: self.path,
            length,
            continues,
        });
    }

    fn run(mut self, messages: &Receiver<Message>) {
        for message in messages {
            match message {
                Message::Packet(packet) => self.write(&packet),
                Message::Slice { slice, rebase } => self.write_slice(&slice, rebase),
                Message::Marker(micros) => self.add_marker(micros),
                Message::Sync(done) => {
                    let _nobody_waits = done.send(());
                }
                Message::Finish { continues } => return self.finish(continues),
            }
        }
        self.finish(false);
    }
}
