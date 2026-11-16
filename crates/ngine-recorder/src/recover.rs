use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ngine_media::{Demuxer, Mp4Layout, Mp4Writer};

use crate::error::RecorderError;
use crate::library::{invalid, tracks_of};
use crate::markers::{append_box, journal_path, read_markers};

const FRAGMENT: [u8; 4] = *b"moof";
const BOX_HEADER_BYTES: u64 = 8;

fn has_top_level_box(file: &mut File, wanted: [u8; 4]) -> io::Result<bool> {
    let length = file.metadata()?.len();
    let mut at = 0_u64;
    while at + BOX_HEADER_BYTES <= length {
        file.seek(SeekFrom::Start(at))?;
        let mut header = [0_u8; 8];
        file.read_exact(&mut header)?;
        let [s0, s1, s2, s3, kind @ ..] = header;
        if kind == wanted {
            return Ok(true);
        }
        let size = match u32::from_be_bytes([s0, s1, s2, s3]) {
            0 => return Ok(false),
            1 => {
                let mut large = [0_u8; 8];
                file.read_exact(&mut large)?;
                u64::from_be_bytes(large)
            }
            size => u64::from(size),
        };
        if size < BOX_HEADER_BYTES {
            return Ok(false);
        }
        at = at.saturating_add(size);
    }
    Ok(false)
}

/// Whether the file is a recording that was cut off before it was completed:
/// it is still made of the fragments written while recording.
pub fn is_unfinished(path: &Path) -> io::Result<bool> {
    has_top_level_box(&mut File::open(path)?, FRAGMENT)
}

pub(crate) fn temporary_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.part"))
}

fn copy_streams(source: &Path, target: &Path) -> Result<u64, RecorderError> {
    let mut demuxer = Demuxer::open(source)?;
    let tracks = tracks_of(demuxer.streams())?;
    let kept: Vec<usize> = std::iter::once(tracks.video).chain(tracks.audio).collect();
    let streams: Vec<_> = kept
        .iter()
        .map(|index| demuxer.streams()[*index].clone())
        .collect();
    let mut writer = Mp4Writer::create(target, &streams, Mp4Layout::Recording)?;
    let mut copied = 0_u64;
    loop {
        let mut packet = match demuxer.read() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(_) if copied > 0 => break,
            Err(error) => return Err(error.into()),
        };
        let Some(index) = kept.iter().position(|source| *source == packet.stream) else {
            continue;
        };
        packet.stream = index;
        writer.write(&packet)?;
        copied += 1;
    }
    writer.finish()?;
    Ok(copied)
}

fn rebuild(recording: &Path, temporary: &Path) -> Result<(), RecorderError> {
    let markers = read_markers(recording).unwrap_or_default();
    if copy_streams(recording, temporary)? == 0 {
        return Err(invalid("the unfinished recording holds nothing to keep"));
    }
    if !markers.is_empty() {
        let micros: Vec<u64> = markers
            .iter()
            .map(|marker| u64::try_from(marker.as_micros()).unwrap_or(u64::MAX))
            .collect();
        append_box(temporary, &micros)
            .map_err(|error| RecorderError::file("store the markers in", temporary, error))?;
    }
    let complete = !is_unfinished(temporary)
        .map_err(|error| RecorderError::file("check", temporary, error))?;
    if !complete {
        return Err(invalid("the rebuilt recording is still unfinished"));
    }
    Demuxer::open(temporary).map(drop).map_err(Into::into)
}

/// Turns a recording that was cut off into a normal MP4 by copying its
/// streams, without re-encoding. The original is replaced only once the copy
/// is complete; on any failure it stays as it was.
pub fn finish_recording(recording: &Path) -> Result<(), RecorderError> {
    let temporary = temporary_path(recording);
    let replaced = rebuild(recording, &temporary).and_then(|()| {
        fs::rename(&temporary, recording)
            .map_err(|error| RecorderError::file("replace", recording, error))
    });
    if replaced.is_err() {
        let _already_gone = fs::remove_file(&temporary);
        return replaced;
    }
    match fs::remove_file(journal_path(recording)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(RecorderError::file(
            "remove the marker notes of",
            recording,
            error,
        )),
        _ => Ok(()),
    }
}

#[derive(Debug)]
pub struct Recovery {
    pub recording: PathBuf,
    pub outcome: Result<(), RecorderError>,
}

fn settled(path: &Path, for_at_least: Duration) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age >= for_at_least)
}

/// Finishes every recording in `folder` that was cut off, and says how each
/// went. A file that changed less than `settled_for` ago is left alone,
/// because something may still be writing it.
pub fn finish_unfinished_recordings(folder: &Path, settled_for: Duration) -> Vec<Recovery> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let hidden = path
                .file_name()
                .is_none_or(|name| name.to_string_lossy().starts_with('.'));
            let video = path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"));
            video && !hidden && is_unfinished(path).unwrap_or(false) && settled(path, settled_for)
        })
        .map(|recording| Recovery {
            outcome: finish_recording(&recording),
            recording,
        })
        .collect()
}
