use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ngine_media::{Demuxer, PixelFormat, VideoDecoder, VideoDecoderOptions, VideoFrame};

use crate::error::RecorderError;
use crate::library::{invalid, tracks_of};

const MAGIC: [u8; 4] = *b"NGT1";
const HEADER_BYTES: usize = 12;
const LONGEST_SIDE: u32 = 4096;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const MOST_PACKETS_FOR_ONE_FRAME: usize = 600;

/// A picture as tightly packed RGBA rows, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn fitted(width: u32, height: u32, bounds: (u32, u32)) -> (u32, u32) {
    let (most_wide, most_high) = (bounds.0.max(1), bounds.1.max(1));
    if width <= most_wide && height <= most_high {
        return (width.max(1), height.max(1));
    }
    let (wide, high) = (u64::from(width), u64::from(height));
    let by_width = wide * u64::from(most_high) >= high * u64::from(most_wide);
    let (scaled_wide, scaled_high) = if by_width {
        (u64::from(most_wide), high * u64::from(most_wide) / wide)
    } else {
        (wide * u64::from(most_high) / high, u64::from(most_high))
    };
    (
        u32::try_from(scaled_wide).unwrap_or(most_wide).max(1),
        u32::try_from(scaled_high).unwrap_or(most_high).max(1),
    )
}

fn rgba_of(luma: u8, blue_difference: u8, red_difference: u8) -> [u8; 4] {
    let level = (i32::from(luma) - 16) * 298 + 128;
    let blue = i32::from(blue_difference) - 128;
    let red = i32::from(red_difference) - 128;
    let part = |value: i32| u8::try_from((value >> 8).clamp(0, 255)).unwrap_or(u8::MAX);
    [
        part(level + 459 * red),
        part(level - 55 * blue - 136 * red),
        part(level + 541 * blue),
        255,
    ]
}

impl Picture {
    /// Shrinks a decoded BGRA frame to fit inside `bounds`, keeping its
    /// shape, by taking the nearest source pixel. A frame that already fits
    /// keeps its size.
    pub fn from_bgra(frame: &VideoFrame, bounds: (u32, u32)) -> Result<Self, RecorderError> {
        let plane = frame
            .planes
            .first()
            .filter(|_| frame.format == PixelFormat::Bgra)
            .ok_or_else(|| invalid("the decoded frame is not BGRA"))?;
        let (width, height) = fitted(frame.width, frame.height, bounds);
        let (source_wide, source_high) = (frame.width as usize, frame.height as usize);
        let (wide, high) = (width as usize, height as usize);
        if plane.stride < source_wide * 4 || plane.data.len() < plane.stride * source_high {
            return Err(invalid("the decoded frame is shorter than its size says"));
        }
        let columns: Vec<usize> = (0..wide)
            .map(|x| (x * source_wide / wide).min(source_wide - 1) * 4)
            .collect();
        let mut rgba = Vec::with_capacity(wide * high * 4);
        for y in 0..high {
            let row = (y * source_high / high).min(source_high - 1) * plane.stride;
            let line = &plane.data[row..row + source_wide * 4];
            for &at in &columns {
                rgba.extend_from_slice(&[line[at + 2], line[at + 1], line[at], 255]);
            }
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// Shrinks a decoded NV12 frame to fit inside `bounds` and turns it into
    /// RGBA, reading it as BT.709 video with the usual limited range. This is
    /// the fast path for a preview; a frame that already fits keeps its size.
    pub fn from_nv12(frame: &VideoFrame, bounds: (u32, u32)) -> Result<Self, RecorderError> {
        let [luma, chroma] = frame.planes.as_slice() else {
            return Err(invalid("the decoded frame is not NV12"));
        };
        let (width, height) = fitted(frame.width, frame.height, bounds);
        let (source_wide, source_high) = (frame.width as usize, frame.height as usize);
        let (wide, high) = (width as usize, height as usize);
        let chroma_high = source_high.div_ceil(2);
        if frame.format != PixelFormat::Nv12
            || luma.stride < source_wide
            || luma.data.len() < luma.stride * source_high
            || chroma.stride < source_wide.div_ceil(2) * 2
            || chroma.data.len() < chroma.stride * chroma_high
        {
            return Err(invalid("the decoded frame is shorter than its size says"));
        }
        let columns: Vec<usize> = (0..wide)
            .map(|x| (x * source_wide / wide).min(source_wide - 1))
            .collect();
        let mut rgba = Vec::with_capacity(wide * high * 4);
        for y in 0..high {
            let row = (y * source_high / high).min(source_high - 1);
            let lumas = &luma.data[row * luma.stride..][..source_wide];
            let chromas = &chroma.data[row / 2 * chroma.stride..][..chroma.stride];
            for &x in &columns {
                let pair = x / 2 * 2;
                rgba.extend_from_slice(&rgba_of(lumas[x], chromas[pair], chromas[pair + 1]));
            }
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    fn encoded(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_BYTES + self.rgba.len());
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&self.width.to_le_bytes());
        bytes.extend_from_slice(&self.height.to_le_bytes());
        bytes.extend_from_slice(&self.rgba);
        bytes
    }

    fn decoded(bytes: &[u8]) -> Option<Self> {
        let (header, rgba) = bytes.split_at_checked(HEADER_BYTES)?;
        let [m0, m1, m2, m3, w0, w1, w2, w3, h0, h1, h2, h3] = *header else {
            return None;
        };
        let width = u32::from_le_bytes([w0, w1, w2, w3]);
        let height = u32::from_le_bytes([h0, h1, h2, h3]);
        let sound = [m0, m1, m2, m3] == MAGIC
            && (1..=LONGEST_SIDE).contains(&width)
            && (1..=LONGEST_SIDE).contains(&height)
            && rgba.len() == width as usize * height as usize * 4;
        sound.then(|| Self {
            width,
            height,
            rgba: rgba.to_vec(),
        })
    }
}

/// Decodes the picture shown at `at`, or the last one before the end when the
/// video is shorter, on the CPU, and shrinks it to fit `bounds`. It starts
/// from the keyframe before `at` when `exact` is false.
pub fn picture_at(
    video: &Path,
    at: Duration,
    exact: bool,
    bounds: (u32, u32),
) -> Result<Picture, RecorderError> {
    let mut demuxer = Demuxer::open(video)?;
    let track = tracks_of(demuxer.streams())?.video;
    let stream = demuxer.streams()[track].clone();
    let options = VideoDecoderOptions {
        output: PixelFormat::Bgra,
        hardware: false,
    };
    let mut decoder = VideoDecoder::open(&stream, options)?;
    let wanted = i64::try_from(at.as_micros()).unwrap_or(i64::MAX);
    if wanted > 0 {
        demuxer.seek(wanted)?;
    }
    let reached = |frame: &VideoFrame| {
        !exact
            || frame
                .pts
                .and_then(|pts| stream.time_base.rescale(pts, ngine_media::Rational::MICROS))
                .is_none_or(|micros| micros >= wanted)
    };
    let mut last = None;
    let mut packets = 0_usize;
    while let Some(packet) = demuxer.read()? {
        if packet.stream != track {
            continue;
        }
        packets += 1;
        for frame in decoder.decode(&packet)? {
            if reached(&frame) {
                return Picture::from_bgra(&frame, bounds);
            }
            last = Some(frame);
        }
        if packets >= MOST_PACKETS_FOR_ONE_FRAME {
            break;
        }
    }
    last.or_else(|| decoder.finish().ok().and_then(|mut frames| frames.pop()))
        .ok_or_else(|| invalid("the video holds no picture"))
        .and_then(|frame| Picture::from_bgra(&frame, bounds))
}

/// Thumbnails kept as small files in one folder, named after the video's
/// path, size and date, so a changed video gets a new thumbnail.
#[derive(Clone, Debug)]
pub struct ThumbnailCache {
    folder: PathBuf,
    bounds: (u32, u32),
}

impl ThumbnailCache {
    pub fn new(folder: impl Into<PathBuf>, bounds: (u32, u32)) -> Self {
        Self {
            folder: folder.into(),
            bounds,
        }
    }

    pub fn file_for(&self, video: &Path, bytes: u64, modified: SystemTime) -> PathBuf {
        let stamp = modified
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let identity = format!(
            "{}|{bytes}|{stamp}|{}x{}",
            video.to_string_lossy(),
            self.bounds.0,
            self.bounds.1
        );
        let hash = identity.bytes().fold(FNV_OFFSET, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
        });
        self.folder.join(format!("{hash:016x}.thumb"))
    }

    /// The cached thumbnail, or a new one decoded a little way into the
    /// video and stored for the next time.
    pub fn thumbnail(
        &self,
        video: &Path,
        bytes: u64,
        modified: SystemTime,
        length: Duration,
    ) -> Result<Picture, RecorderError> {
        let file = self.file_for(video, bytes, modified);
        if let Some(cached) = fs::read(&file)
            .ok()
            .and_then(|bytes| Picture::decoded(&bytes))
        {
            return Ok(cached);
        }
        let picture = picture_at(video, length / 10, false, self.bounds)?;
        let stored =
            fs::create_dir_all(&self.folder).and_then(|()| store(&file, &picture.encoded()));
        stored.map_err(|error| RecorderError::file("store the thumbnail", &file, error))?;
        Ok(picture)
    }

    /// Removes cached thumbnails that none of `kept` names.
    pub fn prune(&self, kept: &[PathBuf]) -> io::Result<()> {
        let entries = match fs::read_dir(&self.folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            let ours = path.extension().is_some_and(|kind| kind == "thumb");
            if ours && !kept.contains(&path) {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }
}

fn store(file: &Path, bytes: &[u8]) -> io::Result<()> {
    let partial = file.with_extension("part");
    fs::write(&partial, bytes)?;
    fs::rename(partial, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_fitted_inside_its_bounds_without_changing_shape() {
        assert_eq!(fitted(1920, 1080, (320, 320)), (320, 180));
        assert_eq!(fitted(1080, 1920, (320, 320)), (180, 320));
        assert_eq!(fitted(100, 50, (320, 320)), (100, 50));
        assert_eq!(fitted(4000, 10, (320, 320)), (320, 1));
    }

    #[test]
    fn video_levels_become_full_range_colours() {
        assert_eq!(rgba_of(16, 128, 128), [0, 0, 0, 255]);
        assert_eq!(rgba_of(235, 128, 128), [255, 255, 255, 255]);
        assert_eq!(rgba_of(63, 102, 240), [255, 1, 0, 255]);
    }

    #[test]
    fn a_stored_picture_reads_back_and_a_damaged_one_does_not() {
        let picture = Picture {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
        };
        let bytes = picture.encoded();
        assert_eq!(Picture::decoded(&bytes), Some(picture));
        assert_eq!(Picture::decoded(&bytes[..bytes.len() - 1]), None);
        assert_eq!(Picture::decoded(b"nope"), None);
    }
}
