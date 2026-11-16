use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ngine_recorder::{
    LibraryItem, MediaInfo, Picture, ThumbnailCache, VideoCodec, finish_unfinished_recordings,
    journal_path, scan,
};
use rune::Pixels;

use crate::format;
use crate::work::{Job, Progress};

pub const THUMBNAIL_PIXELS: (u32, u32) = (256, 144);
const SETTLED_FOR: Duration = Duration::from_secs(10);
const SAMPLE_DAY: u64 = 86_400;

pub struct Entry {
    pub item: LibraryItem,
    pub date: String,
    pub thumbnail: Pixels,
}

struct Scanned {
    finished: usize,
    not_finished: Vec<String>,
    items: io::Result<Vec<LibraryItem>>,
}

/// The recordings and clips in the output folder, as the window lists them.
pub struct Library {
    pub entries: Vec<Entry>,
    pub loading: bool,
    pub problem: Option<String>,
    news: Vec<String>,
    cache: Option<ThumbnailCache>,
    scanning: Option<Job<Scanned>>,
    scan_again: Option<PathBuf>,
    waits_for_thumbnails: bool,
}

fn scanned(folder: &Path) -> Scanned {
    let recoveries = finish_unfinished_recordings(folder, SETTLED_FOR);
    let not_finished = recoveries
        .iter()
        .filter_map(|recovery| {
            let error = recovery.outcome.as_ref().err()?;
            Some(format!(
                "{} could not be finished: {error}",
                recovery.recording.display()
            ))
        })
        .collect::<Vec<_>>();
    Scanned {
        finished: recoveries.len() - not_finished.len(),
        not_finished,
        items: scan(folder),
    }
}

fn paint(thumbnail: &Pixels, picture: Picture) {
    thumbnail.set(picture.width, picture.height, picture.rgba);
}

struct Wanted {
    video: PathBuf,
    bytes: u64,
    modified: SystemTime,
    length: Duration,
    thumbnail: Pixels,
}

fn make_thumbnails(cache: &ThumbnailCache, wanted: Vec<Wanted>, kept: &[PathBuf]) {
    for one in wanted {
        match cache.thumbnail(&one.video, one.bytes, one.modified, one.length) {
            Ok(picture) => paint(&one.thumbnail, picture),
            Err(error) => eprintln!("ngine: no thumbnail for {}: {error}", one.video.display()),
        }
    }
    if let Err(error) = cache.prune(kept) {
        eprintln!("ngine: old thumbnails could not be removed: {error}");
    }
}

impl Library {
    /// A library that never reads the disk. It lists what it is given.
    pub fn offline() -> Self {
        Self {
            entries: Vec::new(),
            loading: false,
            problem: None,
            news: Vec::new(),
            cache: None,
            scanning: None,
            scan_again: None,
            waits_for_thumbnails: false,
        }
    }

    pub fn on_disk(thumbnails: PathBuf) -> Self {
        Self {
            cache: Some(ThumbnailCache::new(thumbnails, THUMBNAIL_PIXELS)),
            ..Self::offline()
        }
    }

    pub fn is_busy(&self) -> bool {
        self.scanning.is_some()
    }

    /// Lists `folder` again on another thread, after finishing any recording
    /// in it that was cut off.
    pub fn refresh(&mut self, folder: &Path) {
        if self.cache.is_none() {
            return;
        }
        if self.scanning.is_some() {
            self.scan_again = Some(folder.to_owned());
            return;
        }
        let scanned_folder = folder.to_owned();
        match Job::spawn("ngine-library", move || scanned(&scanned_folder)) {
            Ok(job) => {
                self.scanning = Some(job);
                self.loading = true;
            }
            Err(error) => self.problem = Some(format!("The library could not be read: {error}")),
        }
    }

    /// Lists `folder` again and waits for it and for its thumbnails.
    pub fn load_now(&mut self, folder: &Path) {
        self.waits_for_thumbnails = true;
        self.absorb(scanned(folder));
        self.waits_for_thumbnails = false;
    }

    /// Takes in a finished listing. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(job) = &self.scanning else {
            return false;
        };
        match job.progress() {
            Progress::Running => return false,
            Progress::Done(scanned) => {
                self.scanning = None;
                self.absorb(scanned);
            }
            Progress::Lost => {
                self.scanning = None;
                self.loading = false;
                self.problem = Some("The library could not be read".into());
            }
        }
        if let Some(folder) = self.scan_again.take() {
            self.refresh(&folder);
        }
        true
    }

    pub fn take_news(&mut self) -> Vec<String> {
        std::mem::take(&mut self.news)
    }

    fn absorb(&mut self, scanned: Scanned) {
        self.loading = false;
        match scanned.finished {
            0 => {}
            1 => self
                .news
                .push("A recording that was cut off has been finished".into()),
            many => self.news.push(format!(
                "{many} recordings that were cut off have been finished"
            )),
        }
        self.news.extend(scanned.not_finished);
        match scanned.items {
            Ok(items) => {
                self.problem = None;
                self.show(items);
            }
            Err(error) => self.problem = Some(format!("The library could not be read: {error}")),
        }
    }

    fn show(&mut self, items: Vec<LibraryItem>) {
        let mut known = std::mem::take(&mut self.entries);
        let mut wanted = Vec::new();
        for item in items {
            let seen_before = known.iter().position(|entry| {
                entry.item.path == item.path
                    && entry.item.bytes == item.bytes
                    && entry.item.modified == item.modified
            });
            let thumbnail = if let Some(at) = seen_before {
                known.swap_remove(at).thumbnail
            } else {
                let thumbnail = Pixels::new();
                if let Some(info) = &item.info {
                    wanted.push(Wanted {
                        video: item.path.clone(),
                        bytes: item.bytes,
                        modified: item.modified,
                        length: info.length,
                        thumbnail: thumbnail.clone(),
                    });
                }
                thumbnail
            };
            self.entries.push(Entry {
                date: format::date(&format::local(item.modified)),
                item,
                thumbnail,
            });
        }
        let Some(cache) = self.cache.clone() else {
            return;
        };
        let kept: Vec<PathBuf> = self
            .entries
            .iter()
            .map(|entry| cache.file_for(&entry.item.path, entry.item.bytes, entry.item.modified))
            .collect();
        if self.waits_for_thumbnails {
            return make_thumbnails(&cache, wanted, &kept);
        }
        let made = std::thread::Builder::new()
            .name("ngine-thumbnails".to_owned())
            .spawn(move || make_thumbnails(&cache, wanted, &kept));
        if let Err(error) = made {
            eprintln!("ngine: thumbnails could not be made: {error}");
        }
    }

    pub fn path(&self, index: usize) -> Option<PathBuf> {
        self.entries.get(index).map(|entry| entry.item.path.clone())
    }

    pub fn renamed(&mut self, from: &Path, to: PathBuf) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.item.path == from)
        {
            entry.item.name = to
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            entry.item.path = to;
        }
    }

    pub fn forget(&mut self, file: &Path) {
        self.entries.retain(|entry| entry.item.path != file);
    }

    /// A library with a few made-up entries, for snapshots and tests.
    pub fn sample(folder: &Path) -> Self {
        let sample = |name, seconds, bytes, ago, markers, colour| Sample {
            name,
            seconds,
            bytes,
            ago,
            markers,
            colour,
        };
        let samples = [
            sample(
                "Recording 2026-11-16 14.03.22",
                252,
                310_400_000,
                0,
                2,
                [38, 70, 120],
            ),
            sample(
                "Replay 2026-11-16 13.41.07",
                31,
                38_900_000,
                1_335,
                0,
                [120, 52, 60],
            ),
            sample(
                "Clip 2026-11-15 22.18.40",
                14,
                17_300_000,
                56_682,
                0,
                [46, 104, 82],
            ),
            sample(
                "Boss fight",
                1_930,
                2_340_000_000,
                2 * SAMPLE_DAY,
                5,
                [96, 78, 40],
            ),
            sample(
                "Recording 2026-11-12 09.15.00",
                65,
                81_000_000,
                4 * SAMPLE_DAY,
                0,
                [70, 60, 110],
            ),
        ];
        let newest = 1_794_834_202;
        let entries = samples
            .into_iter()
            .map(|sample| {
                let Sample {
                    name,
                    seconds,
                    bytes,
                    ago,
                    markers,
                    colour,
                } = sample;
                let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(newest - ago);
                let thumbnail = Pixels::new();
                paint(&thumbnail, sample_picture(THUMBNAIL_PIXELS, colour));
                let since_epoch = i64::try_from((newest - ago) * 1000).unwrap_or(0);
                Entry {
                    date: format::date(&rune::LocalTime::at_offset(since_epoch, 3_600)),
                    thumbnail,
                    item: LibraryItem {
                        path: folder.join(format!("{name}.mp4")),
                        name: name.to_owned(),
                        bytes,
                        modified,
                        info: Some(MediaInfo {
                            length: Duration::from_secs(seconds),
                            width: 2560,
                            height: 1440,
                            video_codec: Some(VideoCodec::H264),
                            audio_tracks: 3,
                        }),
                        markers,
                        unfinished: false,
                    },
                }
            })
            .collect();
        Self {
            entries,
            ..Self::offline()
        }
    }
}

struct Sample {
    name: &'static str,
    seconds: u64,
    bytes: u64,
    ago: u64,
    markers: usize,
    colour: [u8; 3],
}

/// A made-up picture: a soft slope of one colour with a lighter band.
pub fn sample_picture(size: (u32, u32), colour: [u8; 3]) -> Picture {
    let (width, height) = size;
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let slope = (x + 2 * y) * 60 / (width + 2 * height).max(1);
            let band = if (y * 10 / height.max(1)) == 6 { 26 } else { 0 };
            let lift = u8::try_from(slope + band).unwrap_or(u8::MAX);
            rgba.extend(colour.map(|part| part.saturating_add(lift)));
            rgba.push(255);
        }
    }
    Picture {
        width,
        height,
        rgba,
    }
}

/// Moves a recording or clip, and the marker notes beside an unfinished one,
/// to the system's trash.
pub fn move_to_trash(file: &Path) -> Result<(), String> {
    trash::delete(file).map_err(|error| error.to_string())?;
    let notes = journal_path(file);
    if notes.exists() {
        trash::delete(notes).map_err(|error| error.to_string())?;
    }
    Ok(())
}
