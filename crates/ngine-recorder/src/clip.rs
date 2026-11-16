use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use ngine_media::{ClipRange, ClipSummary, ClipWriter, Rational, StreamInfo};

use crate::error::RecorderError;
use crate::event::{Events, RecorderEvent};
use crate::replay::{EpochId, LONGEST_KEYFRAME_SNAP, ReplaySave, ReplaySlice};
use crate::time::{VIDEO_TIME_BASE, media_packet, micros_from_ticks, video_units_from_ticks};

pub(crate) const MOST_SAVES_AT_ONCE: usize = 2;
const NAME_ATTEMPTS: u32 = 10_000;

pub(crate) type EpochStreams = (EpochId, Arc<[StreamInfo]>);

pub(crate) struct ClipJob {
    pub save: ReplaySave,
    pub epochs: Vec<EpochStreams>,
    pub folder: PathBuf,
    pub name: String,
}

/// The first path in `folder` named `name`, `name (2)`, `name (3)` and so on
/// with the extension `mp4` that no file has yet.
pub fn unused_path(folder: &Path, name: &str) -> PathBuf {
    let numbered = (2..NAME_ATTEMPTS).map(|number| format!("{name} ({number}).mp4"));
    std::iter::once(format!("{name}.mp4"))
        .chain(numbered)
        .map(|file| folder.join(file))
        .find(|path| !path.exists())
        .unwrap_or_else(|| folder.join(format!("{name} ({NAME_ATTEMPTS}).mp4")))
}

struct RunningSave(Arc<AtomicUsize>);

impl Drop for RunningSave {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) fn spawn(
    job: ClipJob,
    running: &Arc<AtomicUsize>,
    events: Events,
) -> Result<(), RecorderError> {
    if running.fetch_add(1, Ordering::AcqRel) >= MOST_SAVES_AT_ONCE {
        running.fetch_sub(1, Ordering::AcqRel);
        return Err(RecorderError::Busy);
    }
    let guard = RunningSave(Arc::clone(running));
    thread::Builder::new()
        .name("ngine-replay-save".to_owned())
        .spawn(move || {
            write_clips(&job, &events);
            drop(guard);
        })
        .map(drop)
        .map_err(|_| RecorderError::Busy)
}

fn write_clips(job: &ClipJob, events: &Events) {
    if let Err(error) = fs::create_dir_all(&job.folder) {
        let error = RecorderError::file("create the folder", &job.folder, error);
        return events.send(RecorderEvent::ReplaySaveFailed { error });
    }
    for slice in job.save.slices() {
        let streams = job
            .epochs
            .iter()
            .find(|(epoch, _)| *epoch == slice.epoch())
            .map(|(_, streams)| streams);
        let Some(streams) = streams else {
            continue;
        };
        let target = unused_path(&job.folder, &job.name);
        let event = match write_clip(slice, streams, &target) {
            Ok(summary) => RecorderEvent::ReplaySaved {
                file: target,
                length: Duration::from_micros(u64::try_from(summary.duration_micros).unwrap_or(0)),
            },
            Err(error) => RecorderEvent::ReplaySaveFailed { error },
        };
        events.send(event);
    }
}

fn start_micros(slice: &ReplaySlice) -> i64 {
    if slice.start() == slice.video_start() {
        VIDEO_TIME_BASE
            .rescale(video_units_from_ticks(slice.start()), Rational::MICROS)
            .unwrap_or_else(|| micros_from_ticks(slice.start()))
    } else {
        micros_from_ticks(slice.start())
    }
}

fn temporary_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.part"))
}

fn write_clip(
    slice: &ReplaySlice,
    streams: &[StreamInfo],
    target: &Path,
) -> Result<ClipSummary, RecorderError> {
    let temporary = temporary_path(target);
    let written = mux(slice, streams, &temporary).and_then(|summary| {
        fs::rename(&temporary, target)
            .map(|()| summary)
            .map_err(|error| RecorderError::file("move the clip into place", target, error))
    });
    if written.is_err() {
        let _already_gone = fs::remove_file(&temporary);
    }
    written
}

fn mux(
    slice: &ReplaySlice,
    streams: &[StreamInfo],
    temporary: &Path,
) -> Result<ClipSummary, RecorderError> {
    let tracks: Vec<usize> = (0..streams.len()).collect();
    let range = ClipRange {
        start_micros: Some(start_micros(slice)),
        end_micros: None,
    };
    let mut writer = ClipWriter::create(temporary, streams, &tracks, range)?
        .starting_exactly_after_a_long_gap(micros_from_ticks(LONGEST_KEYFRAME_SNAP));
    for stored in slice.packets().flatten() {
        writer.push(media_packet(&stored))?;
    }
    Ok(writer.finish()?)
}
