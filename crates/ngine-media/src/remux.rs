use std::path::Path;

use crate::error::{Error, Result};
use crate::ffi::{Demuxer, Mp4Layout, Mp4Writer, StreamInfo};
use crate::nal::starts_closed_group;
use crate::types::{AudioCodec, Codec, Packet, Rational, VideoCodec};

const OPUS_PRE_ROLL_MICROS: i64 = 80_000;
const AAC_PRE_ROLL_MICROS: i64 = 44_000;
const UNCOMMITTED_AUDIO_MICROS: i64 = 1_000_000;
const SEEK_BACK_MICROS: i64 = 1_000;

/// The part of the source to keep, in microseconds on the source's own
/// timeline. The clip starts at the last clean keyframe at or before
/// `start_micros`, or at the first one when that is `None`, and stops before
/// the first video frame at or after `end_micros`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipRange {
    pub start_micros: Option<i64>,
    pub end_micros: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipSummary {
    /// Where the clip really starts on the source's timeline.
    pub source_start_micros: i64,
    pub duration_micros: i64,
    pub video_frames: u64,
}

/// Cuts a clip out of a file without re-encoding. `tracks` and `range` mean
/// what they mean for [`ClipWriter`].
pub fn cut_clip(
    source: &Path,
    target: &Path,
    tracks: &[usize],
    range: ClipRange,
) -> Result<ClipSummary> {
    let mut demuxer = Demuxer::open(source)?;
    let video = *tracks
        .first()
        .ok_or_else(|| Error::invalid("a clip needs a video track"))?;
    let time_base = demuxer
        .streams()
        .get(video)
        .ok_or_else(|| Error::invalid(format!("the source has no stream {video}")))?
        .time_base;
    if let Some(start) = range.start_micros {
        demuxer.seek(start)?;
        let keyframe_micros = loop {
            let packet = demuxer
                .read()?
                .ok_or_else(|| Error::invalid("the clip starts after the end of the file"))?;
            if packet.stream == video {
                break packet
                    .pts
                    .and_then(|pts| time_base.rescale(pts, Rational::MICROS))
                    .unwrap_or(start);
            }
        };
        let before_keyframe = keyframe_micros.min(start) - SEEK_BACK_MICROS;
        demuxer.seek(before_keyframe.max(0))?;
    }
    let mut writer = ClipWriter::create(target, demuxer.streams(), tracks, range)?;
    while !writer.is_complete()
        && let Some(packet) = demuxer.read()?
    {
        writer.push(packet)?;
    }
    writer.finish()
}

struct Track {
    source: usize,
    time_base: Rational,
    pre_roll_micros: i64,
    video: Option<(VideoCodec, Vec<u8>)>,
    offset: i64,
    ended: bool,
}

#[derive(Clone, Copy)]
struct Origin {
    micros: i64,
    video_pts: i64,
}

pub struct ClipWriter {
    writer: Mp4Writer,
    tracks: Vec<Track>,
    range: ClipRange,
    waiting: Vec<(Packet, i64)>,
    candidate: Option<Origin>,
    origin: Option<Origin>,
    end_micros: i64,
    video_frames: u64,
    complete: bool,
}

impl ClipWriter {
    /// `tracks` lists the source streams to keep, in output order: the video
    /// stream first, then up to three audio streams. Packets of other streams
    /// are ignored.
    pub fn create(
        path: &Path,
        streams: &[StreamInfo],
        tracks: &[usize],
        range: ClipRange,
    ) -> Result<Self> {
        let mut kept = Vec::new();
        let mut selected = Vec::new();
        for &source in tracks {
            let stream = streams
                .get(source)
                .ok_or_else(|| Error::invalid(format!("the source has no stream {source}")))?;
            let codec = stream.parameters.codec();
            kept.push(Track {
                source,
                time_base: stream.time_base,
                pre_roll_micros: match codec {
                    Some(Codec::Audio(AudioCodec::Opus)) => OPUS_PRE_ROLL_MICROS,
                    Some(Codec::Audio(AudioCodec::Aac)) => AAC_PRE_ROLL_MICROS,
                    _ => 0,
                },
                video: match codec {
                    Some(Codec::Video(codec)) => {
                        Some((codec, stream.parameters.extradata().to_vec()))
                    }
                    _ => None,
                },
                offset: 0,
                ended: false,
            });
            selected.push(stream.clone());
        }
        if kept.first().is_none_or(|track| track.video.is_none()) {
            return Err(Error::invalid(
                "a clip's first track must be H.264, HEVC or AV1 video",
            ));
        }
        Ok(Self {
            writer: Mp4Writer::create(path, &selected, Mp4Layout::Clip)?,
            tracks: kept,
            range,
            waiting: Vec::new(),
            candidate: None,
            origin: None,
            end_micros: 0,
            video_frames: 0,
            complete: false,
        })
    }

    /// Whether the end of the range was reached, so further packets change
    /// nothing.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Takes the source's next packet, in the order the source holds them.
    pub fn push(&mut self, mut packet: Packet) -> Result<()> {
        if self.complete {
            return Ok(());
        }
        let Some(index) = self
            .tracks
            .iter()
            .position(|track| track.source == packet.stream)
        else {
            return Ok(());
        };
        packet.stream = index;
        let track = &self.tracks[index];
        let pts = packet.pts.ok_or_else(|| {
            Error::invalid("a packet without a timestamp cannot be placed in a clip")
        })?;
        let micros = track
            .time_base
            .rescale(pts, Rational::MICROS)
            .ok_or_else(|| Error::invalid("a packet's timestamp is out of range"))?;

        if self.origin.is_some() {
            return self.write(packet, micros);
        }
        let Some((codec, extradata)) = &track.video else {
            self.waiting.push((packet, micros));
            if self.candidate.is_none() {
                self.waiting
                    .retain(|(_, queued)| *queued > micros - UNCOMMITTED_AUDIO_MICROS);
            }
            return Ok(());
        };
        let clean = packet.keyframe && starts_closed_group(*codec, extradata, &packet.data);
        let here = Origin {
            micros,
            video_pts: pts,
        };
        let past_start = self.range.start_micros.is_none_or(|start| micros > start);
        match (past_start, clean, self.candidate) {
            (true, _, Some(candidate)) => {
                self.commit(candidate)?;
                self.write(packet, micros)
            }
            (true, true, None) => {
                self.commit(here)?;
                self.write(packet, micros)
            }
            (false, true, _) => {
                self.candidate = Some(here);
                let tracks = &self.tracks;
                self.waiting.retain(|(queued, queued_micros)| {
                    let track = &tracks[queued.stream];
                    track.video.is_none() && *queued_micros > micros - 2 * track.pre_roll_micros
                });
                self.waiting.push((packet, micros));
                Ok(())
            }
            (false, false, Some(_)) => {
                self.waiting.push((packet, micros));
                Ok(())
            }
            (_, false, None) => Ok(()),
        }
    }

    fn commit(&mut self, origin: Origin) -> Result<()> {
        self.origin = Some(origin);
        self.end_micros = origin.micros;
        for track in &mut self.tracks {
            track.offset = if track.video.is_some() {
                origin.video_pts
            } else {
                Rational::MICROS
                    .rescale(origin.micros, track.time_base)
                    .ok_or_else(|| Error::invalid("the clip's start is out of range"))?
            };
        }
        for (packet, micros) in std::mem::take(&mut self.waiting) {
            self.write(packet, micros)?;
        }
        Ok(())
    }

    fn write(&mut self, mut packet: Packet, micros: i64) -> Result<()> {
        let Some(origin) = self.origin else {
            return Ok(());
        };
        let track = &self.tracks[packet.stream];
        let duration_micros = track
            .time_base
            .rescale(packet.duration, Rational::MICROS)
            .unwrap_or(0);
        if self.range.end_micros.is_some_and(|end| micros >= end) {
            self.tracks[packet.stream].ended = true;
            self.complete = self.tracks.iter().all(|track| track.ended);
            return Ok(());
        }
        if track.ended {
            return Ok(());
        }
        if track.video.is_some() {
            self.video_frames += 1;
            self.end_micros = self.end_micros.max(micros + duration_micros);
        } else if micros + duration_micros <= origin.micros - track.pre_roll_micros {
            return Ok(());
        }
        packet.pts = packet.pts.map(|pts| pts - track.offset);
        packet.dts = packet.dts.map(|dts| dts - track.offset);
        self.writer.write(&packet)
    }

    /// Completes the file. Fails when the source held no clean keyframe to
    /// start on.
    pub fn finish(mut self) -> Result<ClipSummary> {
        if self.origin.is_none()
            && let Some(candidate) = self.candidate
        {
            self.commit(candidate)?;
        }
        let origin = self.origin.ok_or_else(|| {
            Error::invalid("the packets held no keyframe that a clip can start on")
        })?;
        self.writer.finish()?;
        Ok(ClipSummary {
            source_start_micros: origin.micros,
            duration_micros: self.end_micros - origin.micros,
            video_frames: self.video_frames,
        })
    }
}
