use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use ngine_media::{
    AudioCodec, AudioDecoder, AudioEncoder, AudioEncoderSettings, ClipRange, ClipWriter, Codec,
    Demuxer, EncoderInfo, Packet, PixelFormat, Rational, SOFTWARE_H264, StreamInfo, VideoCodec,
    VideoDecoder, VideoDecoderOptions, VideoEncoder, VideoEncoderSettings, VideoInput, VideoSource,
    conform_to_track, has_encoder, same_parameter_sets, with_parameter_sets,
};

use crate::error::RecorderError;
use crate::library::{MOST_AUDIO_TRACKS, invalid, move_without_replacing, tracks_of};
use crate::recover::temporary_path;
use crate::settings::AUDIO_TRACK_BIT_RATE;
use crate::sound::{ClipMix, TimelineMix};

const FRAME_TOLERANCE_MICROS: i64 = 500;
const FALLBACK_FRAME_MICROS: i64 = 16_667;
const FURTHEST_READ_PAST_THE_END_MICROS: i64 = 10_000_000;
const SEEK_BACK_MICROS: i64 = 1_000;
const LOWEST_EDGE_BIT_RATE: u64 = 6_000_000;
const HIGHEST_EDGE_BIT_RATE: u64 = 80_000_000;
const DONE: u32 = 1000;
const SYSTEM_TRACK: usize = 1;
const MICROPHONE_TRACK: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// Starts at the keyframe before the in point and copies everything.
    Fast,
    /// Starts on the in point: the video from there to the next keyframe is
    /// encoded again and the rest is copied.
    Exact,
}

#[derive(Clone, Debug)]
pub struct ExportRequest {
    pub source: PathBuf,
    /// The clip's file. It must not exist yet.
    pub target: PathBuf,
    /// When the first frame of the clip is shown in the source.
    pub start: Duration,
    /// When the last frame of the clip is shown in the source.
    pub end: Duration,
    pub cut: Cut,
    pub mix: ClipMix,
    /// The video encoders that work on this machine, for an exact cut.
    pub encoders: Vec<EncoderInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportSummary {
    pub file: PathBuf,
    /// Where the clip really starts in the source.
    pub source_start: Duration,
    pub length: Duration,
    pub video_frames: u64,
}

/// Shared with the thread that exports: how far it is, and a way to stop it.
#[derive(Debug, Default)]
pub struct ExportControl {
    permille: AtomicU32,
    cancelled: AtomicBool,
}

impl ExportControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// From 0 to 1.
    pub fn progress(&self) -> f32 {
        f32::from(u16::try_from(self.permille.load(Ordering::Relaxed).min(DONE)).unwrap_or(0))
            / 1000.0
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn reached(&self, micros: i64, from: i64, to: i64) {
        let span = (to - from).max(1);
        let done = ((micros - from).clamp(0, span) * i64::from(DONE - 1)) / span;
        self.permille
            .store(u32::try_from(done).unwrap_or(0), Ordering::Relaxed);
    }
}

fn micros_of(time: Duration) -> i64 {
    i64::try_from(time.as_micros()).unwrap_or(i64::MAX)
}

fn length_of(micros: i64) -> Duration {
    Duration::from_micros(micros.max(0).unsigned_abs())
}

enum Sound {
    Silent,
    Copied {
        source: usize,
        reached: i64,
    },
    Mixed {
        sources: [usize; 2],
        decoders: Box<[AudioDecoder; 2]>,
        time_bases: [Rational; 2],
        mix: TimelineMix,
        encoder: Box<AudioEncoder>,
        origin: Option<i64>,
        reached: [i64; 2],
    },
}

impl Sound {
    fn open(streams: &[StreamInfo], audio: &[usize], mix: ClipMix) -> Result<Self, RecorderError> {
        let Some(&first) = audio.first() else {
            return Ok(Self::Silent);
        };
        if mix.is_as_recorded() || audio.len() < MOST_AUDIO_TRACKS {
            return Ok(Self::Copied {
                source: first,
                reached: i64::MIN,
            });
        }
        let sources = [audio[SYSTEM_TRACK], audio[MICROPHONE_TRACK]];
        let wanted = match streams[first].parameters.codec() {
            Some(Codec::Audio(codec)) if AudioEncoder::is_available(codec) => codec,
            _ => AudioCodec::Aac,
        };
        Ok(Self::Mixed {
            sources,
            decoders: Box::new([
                AudioDecoder::open(&streams[sources[0]])?,
                AudioDecoder::open(&streams[sources[1]])?,
            ]),
            time_bases: [streams[sources[0]].time_base, streams[sources[1]].time_base],
            mix: TimelineMix::new(&mix.gains()),
            encoder: Box::new(AudioEncoder::open(AudioEncoderSettings {
                codec: wanted,
                bitrate: AUDIO_TRACK_BIT_RATE,
            })?),
            origin: None,
            reached: [i64::MIN; 2],
        })
    }

    fn read_until(&self) -> i64 {
        match self {
            Self::Silent => i64::MAX,
            Self::Copied { reached, .. } => *reached,
            Self::Mixed { reached, .. } => reached[0].min(reached[1]),
        }
    }

    fn stream(&self, streams: &[StreamInfo]) -> Option<StreamInfo> {
        match self {
            Self::Silent => None,
            Self::Copied { source, .. } => Some(streams[*source].clone()),
            Self::Mixed { encoder, .. } => Some(encoder.stream_info().clone()),
        }
    }

    fn encode_ready(
        mix: &mut TimelineMix,
        encoder: &mut AudioEncoder,
        origin: &mut Option<i64>,
        last: bool,
    ) -> Result<Vec<Packet>, RecorderError> {
        let mut packets = Vec::new();
        while let Some((at, samples)) = mix.take() {
            origin.get_or_insert(at);
            packets.extend(encoder.encode(&samples)?);
        }
        if last {
            packets.extend(encoder.finish()?);
        }
        let shift = origin.unwrap_or(0);
        for packet in &mut packets {
            packet.stream = 1;
            packet.pts = packet.pts.map(|pts| pts + shift);
            packet.dts = packet.dts.map(|dts| dts + shift);
        }
        Ok(packets)
    }

    fn take(&mut self, packet: Packet, micros: i64) -> Result<Vec<Packet>, RecorderError> {
        match self {
            Self::Silent => Ok(Vec::new()),
            Self::Copied { source, reached } => Ok(if packet.stream == *source {
                *reached = micros;
                vec![Packet {
                    stream: 1,
                    ..packet
                }]
            } else {
                Vec::new()
            }),
            Self::Mixed {
                sources,
                decoders,
                time_bases,
                mix,
                encoder,
                origin,
                reached,
            } => {
                let Some(line) = sources.iter().position(|source| *source == packet.stream) else {
                    return Ok(Vec::new());
                };
                reached[line] = micros;
                for frame in decoders[line].decode(&packet)? {
                    mix.push(line, &frame, time_bases[line])?;
                }
                Self::encode_ready(mix, encoder, origin, false)
            }
        }
    }

    fn finish(&mut self) -> Result<Vec<Packet>, RecorderError> {
        let Self::Mixed {
            decoders,
            time_bases,
            mix,
            encoder,
            origin,
            ..
        } = self
        else {
            return Ok(Vec::new());
        };
        for line in 0..decoders.len() {
            for frame in decoders[line].finish()? {
                mix.push(line, &frame, time_bases[line])?;
            }
            mix.end_line(line);
        }
        Self::encode_ready(mix, encoder, origin, true)
    }
}

struct Video {
    source: usize,
    stream: StreamInfo,
    codec: VideoCodec,
    frame_micros: i64,
}

impl Video {
    fn micros(&self, packet: &Packet) -> Result<i64, RecorderError> {
        packet
            .pts
            .and_then(|pts| self.stream.time_base.rescale(pts, Rational::MICROS))
            .ok_or_else(|| invalid("a video packet has no usable timestamp"))
    }

    fn frame_ticks(&self) -> i64 {
        Rational::MICROS
            .rescale(self.frame_micros, self.stream.time_base)
            .unwrap_or(1)
            .max(1)
    }
}

struct Edge {
    packets: Vec<Packet>,
    first_micros: i64,
    foreign: bool,
}

fn open_edge_encoder(
    video: &Video,
    size: (u32, u32),
    frames: usize,
    encoders: &[EncoderInfo],
) -> Result<VideoEncoder, RecorderError> {
    let mut names: Vec<&str> = encoders
        .iter()
        .filter(|encoder| encoder.codec == video.codec)
        .map(|encoder| encoder.name.as_str())
        .collect();
    if video.codec == VideoCodec::H264
        && has_encoder(SOFTWARE_H264)
        && !names.contains(&SOFTWARE_H264)
    {
        names.push(SOFTWARE_H264);
    }
    let per_second = (1_000_000 / video.frame_micros.max(1)).clamp(1, 240);
    let bitrate =
        (video.stream.parameters.bit_rate() * 2).clamp(LOWEST_EDGE_BIT_RATE, HIGHEST_EDGE_BIT_RATE);
    let mut fallback = None;
    for name in names {
        let settings = VideoEncoderSettings {
            encoder: name.to_owned(),
            width: size.0,
            height: size.1,
            time_base: video.stream.time_base,
            frame_rate: Rational::new(i32::try_from(per_second).unwrap_or(60), 1),
            bitrate,
            max_bitrate: bitrate + bitrate / 2,
            keyframe_interval: u32::try_from(frames + 1).unwrap_or(u32::MAX),
        };
        let Ok(encoder) = VideoEncoder::open(&settings, VideoSource::Cpu(PixelFormat::Nv12)) else {
            continue;
        };
        let matches = same_parameter_sets(
            video.codec,
            video.stream.parameters.extradata(),
            encoder.stream_info().parameters.extradata(),
        );
        if matches {
            return Ok(encoder);
        }
        fallback = fallback.or(Some(encoder));
    }
    fallback.ok_or(RecorderError::CodecUnavailable(video.codec))
}

fn encode_edge(
    video: &Video,
    group: &[Packet],
    window: (i64, i64),
    encoders: &[EncoderInfo],
) -> Result<Option<Edge>, RecorderError> {
    if video.codec == VideoCodec::Av1 {
        return Err(invalid(
            "an exact cut of AV1 video is not built yet; use the fast cut",
        ));
    }
    let options = VideoDecoderOptions {
        output: PixelFormat::Nv12,
        hardware: false,
    };
    let mut decoder = VideoDecoder::open(&video.stream, options)?;
    let mut frames = Vec::new();
    for packet in group {
        frames.extend(decoder.decode(packet)?);
    }
    frames.extend(decoder.finish()?);
    let time_base = video.stream.time_base;
    let shown_at = |pts: Option<i64>| pts.and_then(|pts| time_base.rescale(pts, Rational::MICROS));
    frames.retain(|frame| {
        shown_at(frame.pts).is_some_and(|micros| micros >= window.0 && micros <= window.1)
    });
    let Some(first) = frames.first() else {
        return Ok(None);
    };
    let first_micros = shown_at(first.pts).unwrap_or(window.0);
    let mut encoder =
        open_edge_encoder(video, (first.width, first.height), frames.len(), encoders)?;
    let mut fresh = Vec::new();
    for frame in &frames {
        let pts = frame.pts.unwrap_or(0);
        fresh.extend(encoder.encode(VideoInput::Cpu(&frame.view()?), pts)?);
    }
    fresh.extend(encoder.finish()?);
    let track = video.stream.parameters.extradata();
    let own = encoder.stream_info().parameters.extradata().to_vec();
    let ticks = video.frame_ticks();
    let mut packets = Vec::with_capacity(fresh.len());
    for packet in fresh {
        packets.push(Packet {
            stream: 0,
            dts: packet.dts.or(packet.pts),
            duration: if packet.duration > 0 {
                packet.duration
            } else {
                ticks
            },
            data: conform_to_track(video.codec, track, &own, &packet.data, packet.keyframe)?,
            ..packet
        });
    }
    if !packets.first().is_some_and(|packet| packet.keyframe) {
        return Err(invalid("the re-encoded start of the clip has no keyframe"));
    }
    Ok(Some(Edge {
        packets,
        first_micros,
        foreign: !same_parameter_sets(video.codec, track, &own),
    }))
}

struct Export<'a> {
    request: &'a ExportRequest,
    video: Video,
    streams: Vec<StreamInfo>,
    temporary: &'a Path,
    writer: Option<ClipWriter>,
    waiting: Vec<(i64, Packet)>,
    group: Vec<Packet>,
    group_closed: bool,
    needs_parameter_sets: bool,
    last_video_micros: i64,
    start: i64,
    end: i64,
}

impl Export<'_> {
    fn open_writer(&mut self, start: i64) -> Result<(), RecorderError> {
        let tracks: Vec<usize> = (0..self.streams.len()).collect();
        let range = ClipRange {
            start_micros: Some(start),
            end_micros: Some(self.end + FRAME_TOLERANCE_MICROS + self.video.frame_micros),
        };
        let mut writer = ClipWriter::create(self.temporary, &self.streams, &tracks, range)?;
        self.waiting.sort_by_key(|(micros, _)| *micros);
        for (_, packet) in self.waiting.drain(..) {
            writer.push(packet)?;
        }
        self.writer = Some(writer);
        Ok(())
    }

    fn push(&mut self, micros: i64, packet: Packet) -> Result<(), RecorderError> {
        if let Some(writer) = &mut self.writer {
            writer.push(packet)?;
        } else {
            self.waiting.push((micros, packet));
        }
        Ok(())
    }

    fn close_group(&mut self) -> Result<(), RecorderError> {
        self.group_closed = true;
        let group = std::mem::take(&mut self.group);
        let Some(first) = group.first() else {
            return Err(invalid("the clip starts after the end of the video"));
        };
        let first_micros = self.video.micros(first)?;
        let from = self.start - FRAME_TOLERANCE_MICROS;
        let until = self.end + FRAME_TOLERANCE_MICROS;
        if first_micros >= from {
            for packet in group {
                let micros = self.video.micros(&packet)?;
                if micros <= until {
                    self.waiting.push((
                        micros,
                        Packet {
                            stream: 0,
                            ..packet
                        },
                    ));
                }
            }
            return self.open_writer(first_micros);
        }
        let edge = encode_edge(&self.video, &group, (from, until), &self.request.encoders)?
            .ok_or_else(|| invalid("the clip holds no video frame"))?;
        self.needs_parameter_sets = edge.foreign;
        for packet in edge.packets {
            let micros = self.video.micros(&packet)?;
            self.waiting.push((micros, packet));
        }
        self.open_writer(edge.first_micros)
    }

    fn take_video(&mut self, mut packet: Packet) -> Result<(), RecorderError> {
        let micros = self.video.micros(&packet)?;
        if packet.duration > 0 && self.video.frame_micros == FALLBACK_FRAME_MICROS {
            self.video.frame_micros = self
                .video
                .stream
                .time_base
                .rescale(packet.duration, Rational::MICROS)
                .filter(|micros| *micros > 0)
                .unwrap_or(FALLBACK_FRAME_MICROS);
        }
        let reordered = micros < self.last_video_micros;
        self.last_video_micros = self.last_video_micros.max(micros);
        packet.stream = 0;
        if self.request.cut == Cut::Fast {
            return if micros > self.end + FRAME_TOLERANCE_MICROS {
                Ok(())
            } else {
                self.push(micros, packet)
            };
        }
        if reordered {
            return Err(invalid(
                "an exact cut of video with reordered frames is not built yet; use the fast cut",
            ));
        }
        if !self.group_closed {
            let opens_a_group = packet.keyframe;
            if opens_a_group && micros <= self.start + FRAME_TOLERANCE_MICROS {
                self.group.clear();
            } else if opens_a_group {
                self.close_group()?;
            }
            if !self.group_closed {
                if packet.keyframe || !self.group.is_empty() {
                    self.group.push(packet);
                }
                return Ok(());
            }
        }
        if micros > self.end + FRAME_TOLERANCE_MICROS {
            return Ok(());
        }
        if self.needs_parameter_sets && packet.keyframe {
            self.needs_parameter_sets = false;
            let track = self.video.stream.parameters.extradata();
            packet.data = with_parameter_sets(self.video.codec, track, &packet.data)?;
        }
        self.push(micros, packet)
    }
}

fn rewind_to_the_keyframe_before(
    demuxer: &mut Demuxer,
    video: usize,
    start: i64,
) -> Result<i64, RecorderError> {
    if start <= 0 {
        return Ok(0);
    }
    let time_base = demuxer.streams()[video].time_base;
    demuxer.seek(start)?;
    let keyframe = loop {
        let packet = demuxer
            .read()?
            .ok_or_else(|| invalid("the clip starts after the end of the video"))?;
        if packet.stream == video {
            break packet
                .pts
                .and_then(|pts| time_base.rescale(pts, Rational::MICROS))
                .unwrap_or(start);
        }
    };
    let read_from = (keyframe.min(start) - SEEK_BACK_MICROS).max(0);
    demuxer.seek(read_from)?;
    Ok(read_from)
}

fn write(
    request: &ExportRequest,
    control: &ExportControl,
    temporary: &Path,
) -> Result<ExportSummary, RecorderError> {
    let mut demuxer = Demuxer::open(&request.source)?;
    let tracks = tracks_of(demuxer.streams())?;
    let source_streams = demuxer.streams().to_vec();
    let stream = source_streams[tracks.video].clone();
    let Some(Codec::Video(codec)) = stream.parameters.codec() else {
        return Err(invalid("the video's codec is not one ngine cuts"));
    };
    let mut sound = Sound::open(&source_streams, &tracks.audio, request.mix)?;
    let mut streams = vec![stream.clone()];
    streams.extend(sound.stream(&source_streams));
    let (start, end) = (micros_of(request.start), micros_of(request.end));

    let read_from = rewind_to_the_keyframe_before(&mut demuxer, tracks.video, start)?;

    let mut export = Export {
        request,
        video: Video {
            source: tracks.video,
            stream,
            codec,
            frame_micros: FALLBACK_FRAME_MICROS,
        },
        streams,
        temporary,
        writer: None,
        waiting: Vec::new(),
        group: Vec::new(),
        group_closed: false,
        needs_parameter_sets: false,
        last_video_micros: i64::MIN,
        start,
        end,
    };
    if request.cut == Cut::Fast {
        export.open_writer(start)?;
    }
    let own_sound = export.streams.get(1).map(|own| own.time_base);
    while let Some(packet) = demuxer.read()? {
        if control.is_cancelled() {
            return Err(invalid("the export was cancelled"));
        }
        if packet.stream == export.video.source {
            let micros = export.video.micros(&packet)?;
            control.reached(micros, read_from, end);
            export.take_video(packet)?;
        } else {
            let read_at = packet
                .pts
                .and_then(|pts| {
                    source_streams[packet.stream]
                        .time_base
                        .rescale(pts, Rational::MICROS)
                })
                .unwrap_or(0);
            for ready in sound.take(packet, read_at)? {
                let micros = ready
                    .pts
                    .zip(own_sound)
                    .and_then(|(pts, base)| base.rescale(pts, Rational::MICROS))
                    .unwrap_or(read_at);
                export.push(micros, ready)?;
            }
        }
        let past_the_end = end + export.video.frame_micros + FRAME_TOLERANCE_MICROS;
        let seen = export.last_video_micros;
        if (seen > past_the_end && sound.read_until() > past_the_end)
            || seen > past_the_end + FURTHEST_READ_PAST_THE_END_MICROS
        {
            break;
        }
    }
    if request.cut == Cut::Exact && !export.group_closed {
        export.close_group()?;
    }
    for ready in sound.finish()? {
        export.push(i64::MAX, ready)?;
    }
    let writer = export
        .writer
        .take()
        .ok_or_else(|| invalid("the clip holds no video frame"))?;
    let summary = writer.finish()?;
    Ok(ExportSummary {
        file: request.target.clone(),
        source_start: length_of(summary.source_start_micros),
        length: length_of(summary.duration_micros),
        video_frames: summary.video_frames,
    })
}

/// Cuts a clip out of a recording into `request.target`. The clip is written
/// next to the target under a temporary name and moved into place once it is
/// complete, so a failed or cancelled export leaves nothing behind. The source
/// is only read.
pub fn export_clip(
    request: &ExportRequest,
    control: &ExportControl,
) -> Result<ExportSummary, RecorderError> {
    let target = &request.target;
    if request.end < request.start {
        return Err(invalid("the clip ends before it starts"));
    }
    if target.exists() {
        let taken = io::Error::from(io::ErrorKind::AlreadyExists);
        return Err(RecorderError::file("create", target, taken));
    }
    if let Some(folder) = target.parent() {
        fs::create_dir_all(folder)
            .map_err(|error| RecorderError::file("create the folder", folder, error))?;
    }
    let temporary = temporary_path(target);
    let written = write(request, control, &temporary).and_then(|summary| {
        move_without_replacing(&temporary, target)
            .map(|()| summary)
            .map_err(|error| RecorderError::file("move the clip into place", target, error))
    });
    if written.is_err() {
        let _already_gone = fs::remove_file(&temporary);
    } else {
        control.permille.store(DONE, Ordering::Relaxed);
    }
    written
}
