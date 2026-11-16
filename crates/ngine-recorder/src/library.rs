use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ngine_media::{Codec, Demuxer, StreamInfo, VideoCodec};

use crate::error::RecorderError;
use crate::markers::{journal_path, read_markers};
use crate::recover::is_unfinished;

const EXTENSION: &str = "mp4";
const FORBIDDEN: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
pub(crate) const MOST_AUDIO_TRACKS: usize = 3;

/// What a recording or clip holds, read from its header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaInfo {
    pub length: Duration,
    pub width: u32,
    pub height: u32,
    pub video_codec: Option<VideoCodec>,
    pub audio_tracks: usize,
}

impl MediaInfo {
    /// Whether system sound and the microphone are stored apart from the mix,
    /// so a clip can mute or rebalance them.
    pub fn has_separate_sound(&self) -> bool {
        self.audio_tracks >= MOST_AUDIO_TRACKS
    }
}

pub(crate) struct Tracks {
    pub video: usize,
    pub audio: Vec<usize>,
}

pub(crate) fn tracks_of(streams: &[StreamInfo]) -> Result<Tracks, RecorderError> {
    let video = streams
        .iter()
        .position(|stream| stream.parameters.is_video())
        .ok_or_else(|| invalid("the file holds no video"))?;
    let audio = streams
        .iter()
        .enumerate()
        .filter(|(_, stream)| stream.parameters.is_audio())
        .map(|(index, _)| index)
        .take(MOST_AUDIO_TRACKS)
        .collect();
    Ok(Tracks { video, audio })
}

pub(crate) fn invalid(message: &str) -> RecorderError {
    RecorderError::Media(ngine_media::Error::Invalid(message.to_owned()))
}

pub(crate) fn info_of(demuxer: &Demuxer) -> Result<MediaInfo, RecorderError> {
    let tracks = tracks_of(demuxer.streams())?;
    let video = &demuxer.streams()[tracks.video].parameters;
    let micros = demuxer.duration_micros().unwrap_or(0).max(0);
    Ok(MediaInfo {
        length: Duration::from_micros(micros.unsigned_abs()),
        width: video.width(),
        height: video.height(),
        video_codec: match video.codec() {
            Some(Codec::Video(codec)) => Some(codec),
            _ => None,
        },
        audio_tracks: tracks.audio.len(),
    })
}

pub fn probe(path: &Path) -> Result<MediaInfo, RecorderError> {
    info_of(&Demuxer::open(path)?)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryItem {
    pub path: PathBuf,
    /// The file name without its extension.
    pub name: String,
    pub bytes: u64,
    pub modified: SystemTime,
    /// `None` when the file cannot be read as a video.
    pub info: Option<MediaInfo>,
    pub markers: usize,
    /// A recording that was cut off before it was completed.
    pub unfinished: bool,
}

fn is_listed(path: &Path) -> bool {
    let hidden = path
        .file_name()
        .is_none_or(|name| name.to_string_lossy().starts_with('.'));
    let video = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION));
    video && !hidden
}

fn item(path: PathBuf) -> io::Result<LibraryItem> {
    let metadata = fs::metadata(&path)?;
    Ok(LibraryItem {
        name: path
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        bytes: metadata.len(),
        modified: metadata.modified()?,
        info: probe(&path).ok(),
        markers: read_markers(&path).map_or(0, |markers| markers.len()),
        unfinished: is_unfinished(&path).unwrap_or(false),
        path,
    })
}

/// Every recording and clip in `folder`, newest first. A folder that does not
/// exist yet holds nothing.
pub fn scan(folder: &Path) -> io::Result<Vec<LibraryItem>> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut items = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if is_listed(&path)
            && path.is_file()
            && let Ok(item) = item(path)
        {
            items.push(item);
        }
    }
    items.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.name.cmp(&b.name)));
    Ok(items)
}

#[derive(Debug)]
pub enum RenameError {
    Empty,
    Forbidden(char),
    Taken,
    File(io::Error),
}

impl fmt::Display for RenameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("A name cannot be empty"),
            Self::Forbidden(character) => {
                write!(
                    formatter,
                    "A name cannot hold {character:?} or start with a dot"
                )
            }
            Self::Taken => formatter.write_str("Another file already has this name"),
            Self::File(error) => write!(formatter, "The file could not be renamed: {error}"),
        }
    }
}

impl std::error::Error for RenameError {}

fn checked_name(name: &str) -> Result<&str, RenameError> {
    let name = name.trim();
    let name = name
        .len()
        .checked_sub(EXTENSION.len() + 1)
        .and_then(|at| name.split_at_checked(at))
        .filter(|(_, tail)| tail.eq_ignore_ascii_case(".mp4"))
        .map_or(name, |(stem, _)| stem.trim_end());
    if name.is_empty() {
        return Err(RenameError::Empty);
    }
    if name.starts_with('.') {
        return Err(RenameError::Forbidden('.'));
    }
    match name
        .chars()
        .find(|character| character.is_control() || FORBIDDEN.contains(character))
    {
        Some(character) => Err(RenameError::Forbidden(character)),
        None => Ok(name),
    }
}

pub(crate) fn move_without_replacing(from: &Path, to: &Path) -> io::Result<()> {
    match fs::hard_link(from, to) {
        Ok(()) => fs::remove_file(from),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(error),
        Err(_) if to.exists() => Err(io::ErrorKind::AlreadyExists.into()),
        Err(_) => fs::rename(from, to),
    }
}

/// Gives a recording or clip a new name in its folder and returns its new
/// path. A name another file already has is refused, so nothing is replaced.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, RenameError> {
    let name = checked_name(new_name)?;
    let target = path.with_file_name(format!("{name}.{EXTENSION}"));
    if target == path {
        return Ok(target);
    }
    let only_case_differs = target
        .to_string_lossy()
        .eq_ignore_ascii_case(&path.to_string_lossy());
    let moved = if only_case_differs {
        fs::rename(path, &target)
    } else {
        move_without_replacing(path, &target)
    };
    match moved {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(RenameError::Taken);
        }
        Err(error) => return Err(RenameError::File(error)),
    }
    let journal = journal_path(path);
    if journal.exists() {
        fs::rename(journal, journal_path(&target)).map_err(RenameError::File)?;
    }
    Ok(target)
}
