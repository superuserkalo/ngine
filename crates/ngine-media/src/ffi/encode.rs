use std::ffi::{CString, c_int};

use super::hw::{self, BufferRef, FrameShape, HwFrames};
use super::scale::{Scaler, tag_sdr_colors};
use super::{
    ChannelLayout, CodecContext, CodecParameters, CodecRef, Frame, GpuFramePool, GpuSurface,
    RawPacket, StreamInfo, codec_from_raw, sys, to_int,
};
use crate::error::{Error, Result};
use crate::types::{
    AudioCodec, Codec, Packet, PixelFormat, PixelView, Rational, VideoCodec, check_dimensions,
};

pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u16 = 2;
const AUDIO_TIME_BASE: Rational = Rational::new(1, 48_000);
const FALLBACK_AUDIO_FRAME: usize = 1024;

pub fn has_encoder(name: &str) -> bool {
    CodecRef::encoder(name).is_some()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoEncoderSettings {
    /// The name `FFmpeg` gives the encoder, as the probe reports it.
    pub encoder: String,
    pub width: u32,
    pub height: u32,
    /// The unit of the timestamps passed to `encode` and found on its packets.
    pub time_base: Rational,
    pub frame_rate: Rational,
    /// Average bits per second.
    pub bitrate: u64,
    /// The cap on bits per second, measured over one second.
    pub max_bitrate: u64,
    /// Frames from one keyframe to the next.
    pub keyframe_interval: u32,
}

/// What the encoder is opened for. An encoder takes one kind of input for its
/// whole life.
#[derive(Clone, Copy, Debug)]
pub enum VideoSource<'a> {
    Cpu(PixelFormat),
    Gpu(&'a GpuFramePool),
}

#[derive(Clone, Copy, Debug)]
pub enum VideoInput<'a> {
    Cpu(&'a PixelView<'a>),
    Gpu(&'a GpuSurface),
}

enum InputMode {
    Cpu {
        format: PixelFormat,
        encoder_format: sys::AVPixelFormat,
        scaler: Scaler,
    },
    Upload {
        format: PixelFormat,
        scaler: Scaler,
        surfaces: HwFrames,
    },
    Gpu,
}

struct Surfaces<'a> {
    frames: &'a BufferRef,
    surface_format: sys::AVPixelFormat,
    pixel_format: sys::AVPixelFormat,
    matrix: sys::AVColorSpace,
}

struct UploadTarget {
    device: sys::AVHWDeviceType,
    surface_format: sys::AVPixelFormat,
}

fn surfaces_it_needs_uploaded(codec: CodecRef) -> Option<UploadTarget> {
    let supported = codec.pixel_formats();
    let takes_cpu_pixels = supported.is_empty()
        || supported
            .iter()
            .any(|format| [sys::AV_PIX_FMT_NV12, sys::AV_PIX_FMT_YUV420P].contains(format));
    (!takes_cpu_pixels && supported.contains(&sys::AV_PIX_FMT_VAAPI)).then_some(UploadTarget {
        device: sys::AV_HWDEVICE_TYPE_VAAPI,
        surface_format: sys::AV_PIX_FMT_VAAPI,
    })
}

pub struct VideoEncoder {
    context: CodecContext,
    packet: RawPacket,
    mode: InputMode,
    codec: VideoCodec,
    name: String,
    hardware: bool,
    width: u32,
    height: u32,
    frame_duration: i64,
    last_pts: Option<i64>,
    keyframe_requested: bool,
    finished: bool,
    info: StreamInfo,
}

impl VideoEncoder {
    pub fn open(settings: &VideoEncoderSettings, source: VideoSource<'_>) -> Result<Self> {
        let codec = CodecRef::encoder(&settings.encoder)
            .ok_or_else(|| Error::unavailable(format!("the video encoder {}", settings.encoder)))?;
        let Some(Codec::Video(video_codec)) = codec_from_raw(codec.0.id) else {
            return Err(Error::invalid(format!(
                "{} does not encode H.264, HEVC or AV1",
                settings.encoder
            )));
        };
        validate_video(settings)?;
        let target = Target {
            settings,
            codec,
            video_codec,
        };
        match source {
            VideoSource::Gpu(pool) => {
                if pool.width() != settings.width || pool.height() != settings.height {
                    return Err(Error::invalid(
                        "the GPU frame pool and the encoder differ in size",
                    ));
                }
                let surfaces = Surfaces {
                    frames: pool.frames(),
                    surface_format: pool.surface_format(),
                    pixel_format: pool.format().raw(),
                    matrix: pool.color_matrix(),
                };
                target.open(InputMode::Gpu, Some(&surfaces))
            }
            VideoSource::Cpu(format) => match surfaces_it_needs_uploaded(codec) {
                None => target.open(
                    InputMode::Cpu {
                        format,
                        encoder_format: encoder_format(codec)?,
                        scaler: Scaler::new()?,
                    },
                    None,
                ),
                Some(kind) => target.open_uploading(format, &kind),
            },
        }
    }

    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_hardware(&self) -> bool {
        self.hardware
    }

    /// The stream to hand to a muxer or a decoder. Its parameters carry the
    /// global headers.
    pub fn stream_info(&self) -> &StreamInfo {
        &self.info
    }

    /// Makes the next encoded frame a keyframe.
    pub fn request_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    /// Encodes one frame. `pts` is in the settings' time base and must grow
    /// with every call. Returns the packets that are ready, which may be none.
    pub fn encode(&mut self, input: VideoInput<'_>, pts: i64) -> Result<Vec<Packet>> {
        if self.finished {
            return Err(Error::invalid("the video encoder is already finished"));
        }
        if self.last_pts.is_some_and(|last| pts <= last) {
            return Err(Error::invalid(format!(
                "video timestamp {pts} does not follow {:?}",
                self.last_pts
            )));
        }
        let mut frame = match (&mut self.mode, input) {
            (
                InputMode::Cpu {
                    format,
                    encoder_format,
                    scaler,
                },
                VideoInput::Cpu(pixels),
            ) => {
                if pixels.format() != *format
                    || pixels.width() != self.width
                    || pixels.height() != self.height
                {
                    return Err(Error::invalid(
                        "the picture does not match the format and size the encoder was opened with",
                    ));
                }
                let copied = frame_from_pixels(pixels)?;
                if format.raw() == *encoder_format {
                    copied
                } else {
                    scaler.convert(&copied, *encoder_format)?
                }
            }
            (
                InputMode::Upload {
                    format,
                    scaler,
                    surfaces,
                },
                VideoInput::Cpu(pixels),
            ) => {
                if pixels.format() != *format
                    || pixels.width() != self.width
                    || pixels.height() != self.height
                {
                    return Err(Error::invalid(
                        "the picture does not match the format and size the encoder was opened with",
                    ));
                }
                let copied = frame_from_pixels(pixels)?;
                let planar = if *format == PixelFormat::Nv12 {
                    copied
                } else {
                    scaler.convert(&copied, sys::AV_PIX_FMT_NV12)?
                };
                surfaces.upload(&planar)?
            }
            (InputMode::Gpu, VideoInput::Gpu(surface)) => {
                if surface.width() != self.width || surface.height() != self.height {
                    return Err(Error::invalid(
                        "the GPU surface does not match the size the encoder was opened with",
                    ));
                }
                surface.new_frame_reference()?
            }
            _ => {
                return Err(Error::invalid(
                    "the encoder was opened for the other kind of input",
                ));
            }
        };
        let raw = frame.raw_mut();
        raw.pts = pts;
        raw.duration = self.frame_duration;
        if self.keyframe_requested {
            raw.pict_type = sys::AV_PICTURE_TYPE_I;
        }
        self.context
            .send_frame(Some(&frame), "encode a video frame")?;
        self.keyframe_requested = false;
        self.last_pts = Some(pts);
        self.drain()
    }

    /// Returns the packets still inside the encoder. Nothing can be encoded
    /// afterwards.
    pub fn finish(&mut self) -> Result<Vec<Packet>> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;
        self.context.send_frame(None, "flush the video encoder")?;
        self.drain()
    }

    fn drain(&mut self) -> Result<Vec<Packet>> {
        let mut packets = Vec::new();
        while self
            .context
            .receive_packet(&mut self.packet, "read an encoded video packet")?
        {
            let mut packet = self.packet.to_plain();
            self.packet.unref();
            packet.stream = 0;
            if packet.duration == 0 {
                packet.duration = self.frame_duration;
            }
            packets.push(packet);
        }
        Ok(packets)
    }
}

struct Target<'a> {
    settings: &'a VideoEncoderSettings,
    codec: CodecRef,
    video_codec: VideoCodec,
}

impl Target<'_> {
    fn open_uploading(&self, format: PixelFormat, kind: &UploadTarget) -> Result<VideoEncoder> {
        let shape = FrameShape {
            surface_format: kind.surface_format,
            pixel_format: sys::AV_PIX_FMT_NV12,
            width: self.settings.width,
            height: self.settings.height,
        };
        let nodes = hw::system_render_nodes();
        let paths: Vec<Option<CString>> = if nodes.is_empty() {
            vec![None]
        } else {
            nodes
                .iter()
                .filter_map(|node| node.to_str())
                .filter_map(|node| CString::new(node).ok())
                .map(Some)
                .collect()
        };
        let mut last = None;
        for path in &paths {
            let opened = hw::open_device(kind.device, path.as_deref(), "open the GPU")
                .and_then(|device| HwFrames::on(&device, shape, "allocate encoder surfaces"))
                .and_then(|surfaces| {
                    let frames = surfaces.reference().share()?;
                    let described = Surfaces {
                        frames: &frames,
                        surface_format: shape.surface_format,
                        pixel_format: shape.pixel_format,
                        matrix: sys::AVCOL_SPC_BT709,
                    };
                    let mode = InputMode::Upload {
                        format,
                        scaler: Scaler::new()?,
                        surfaces,
                    };
                    self.open(mode, Some(&described))
                });
            match opened {
                Ok(encoder) => return Ok(encoder),
                Err(error) => last = Some(error),
            }
        }
        Err(hw::no_device(&self.settings.encoder, last))
    }

    fn open(&self, mode: InputMode, surfaces: Option<&Surfaces<'_>>) -> Result<VideoEncoder> {
        let settings = self.settings;
        let codec = self.codec;
        let mut context = CodecContext::new(codec)?;
        if let Some(surfaces) = surfaces {
            let frames = surfaces.frames.new_reference()?;
            let raw = context.unopened_mut();
            raw.hw_frames_ctx = frames;
            raw.sw_pix_fmt = surfaces.pixel_format;
        }
        let peak = i64::try_from(settings.max_bitrate)
            .map_err(|_| Error::invalid("the bitrate cap is too large"))?;
        let raw = context.unopened_mut();
        raw.pix_fmt = match (&mode, surfaces) {
            (InputMode::Cpu { encoder_format, .. }, _) => *encoder_format,
            (_, Some(surfaces)) => surfaces.surface_format,
            (_, None) => {
                return Err(Error::invalid(
                    "an encoder that reads surfaces needs their description",
                ));
            }
        };
        raw.width = to_int(settings.width, "the width")?;
        raw.height = to_int(settings.height, "the height")?;
        raw.time_base = settings.time_base.into();
        raw.framerate = settings.frame_rate.into();
        raw.bit_rate = i64::try_from(settings.bitrate)
            .map_err(|_| Error::invalid("the bitrate is too large"))?;
        raw.rc_max_rate = peak;
        raw.rc_buffer_size = c_int::try_from(peak).unwrap_or(c_int::MAX);
        raw.gop_size = to_int(settings.keyframe_interval, "the keyframe interval")?;
        raw.max_b_frames = 0;
        raw.flags |= sys::AV_CODEC_FLAG_GLOBAL_HEADER.cast_signed();
        raw.color_primaries = sys::AVCOL_PRI_BT709;
        raw.color_trc = sys::AVCOL_TRC_BT709;
        raw.colorspace = surfaces.map_or(sys::AVCOL_SPC_BT709, |surfaces| surfaces.matrix);
        raw.color_range = sys::AVCOL_RANGE_MPEG;
        if settings.encoder.ends_with("_videotoolbox") {
            context.set_private_option(c"realtime", c"1");
        }
        if settings.encoder.ends_with("_mf") {
            context.set_private_option(c"hw_encoding", c"1");
        }
        context.open(codec, "open the video encoder")?;

        let frame_duration = Rational::new(settings.frame_rate.den, settings.frame_rate.num)
            .rescale(1, settings.time_base)
            .unwrap_or(0);
        let info = StreamInfo {
            time_base: settings.time_base,
            parameters: CodecParameters::from_context(&context)?,
        };
        Ok(VideoEncoder {
            context,
            packet: RawPacket::new()?,
            mode,
            codec: self.video_codec,
            name: codec.name(),
            hardware: codec.is_hardware(),
            width: settings.width,
            height: settings.height,
            frame_duration,
            last_pts: None,
            keyframe_requested: false,
            finished: false,
            info,
        })
    }
}

fn validate_video(settings: &VideoEncoderSettings) -> Result<()> {
    check_dimensions(settings.width, settings.height)?;
    if !settings.width.is_multiple_of(2) || !settings.height.is_multiple_of(2) {
        return Err(Error::invalid("the encoders need an even width and height"));
    }
    if !settings.time_base.is_positive() || !settings.frame_rate.is_positive() {
        return Err(Error::invalid(
            "the time base and the frame rate must be positive fractions",
        ));
    }
    if settings.bitrate == 0 || settings.max_bitrate < settings.bitrate {
        return Err(Error::invalid(
            "the bitrate must be positive and no larger than its cap",
        ));
    }
    if settings.keyframe_interval == 0 {
        return Err(Error::invalid("the keyframe interval must be at least 1"));
    }
    Ok(())
}

fn encoder_format(codec: CodecRef) -> Result<sys::AVPixelFormat> {
    let supported = codec.pixel_formats();
    [sys::AV_PIX_FMT_NV12, sys::AV_PIX_FMT_YUV420P]
        .into_iter()
        .find(|format| supported.is_empty() || supported.contains(format))
        .ok_or_else(|| {
            Error::unavailable(format!(
                "{} takes neither NV12 nor planar 4:2:0 pixels",
                codec.name()
            ))
        })
}

pub(super) fn frame_from_pixels(pixels: &PixelView<'_>) -> Result<Frame> {
    let format = pixels.format();
    let mut frame = Frame::video(format.raw(), pixels.width(), pixels.height())?;
    tag_sdr_colors(&mut frame);
    let shapes = format.plane_shapes(pixels.width(), pixels.height());
    for (index, shape) in shapes.into_iter().enumerate() {
        let (bytes, stride) = frame.plane_mut(index, shape).ok_or_else(|| {
            Error::invalid("FFmpeg allocated a frame without the expected planes")
        })?;
        for (target, source) in bytes.chunks_mut(stride).zip(pixels.rows(index)) {
            target[..shape.row_bytes].copy_from_slice(source);
        }
    }
    Ok(frame)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioEncoderSettings {
    pub codec: AudioCodec,
    /// Bits per second.
    pub bitrate: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SampleLayout {
    Planar,
    Interleaved,
}

/// Encodes 48 kHz stereo sound. Packets are stamped in 1/48000 s, counted from
/// the first sample passed in.
pub struct AudioEncoder {
    context: CodecContext,
    packet: RawPacket,
    layout: ChannelLayout,
    sample_format: sys::AVSampleFormat,
    sample_layout: SampleLayout,
    frame_samples: usize,
    pending: Vec<f32>,
    next_pts: i64,
    finished: bool,
    info: StreamInfo,
}

impl AudioEncoder {
    pub fn encoder_name(codec: AudioCodec) -> &'static str {
        match codec {
            AudioCodec::Aac => "aac",
            AudioCodec::Opus => "libopus",
        }
    }

    pub fn is_available(codec: AudioCodec) -> bool {
        has_encoder(Self::encoder_name(codec))
    }

    pub fn open(settings: AudioEncoderSettings) -> Result<Self> {
        let name = Self::encoder_name(settings.codec);
        let codec = CodecRef::encoder(name)
            .ok_or_else(|| Error::unavailable(format!("the audio encoder {name}")))?;
        if settings.bitrate == 0 {
            return Err(Error::invalid("the audio bitrate must be positive"));
        }
        let supported = codec.sample_formats();
        let (sample_format, sample_layout) = [
            (sys::AV_SAMPLE_FMT_FLTP, SampleLayout::Planar),
            (sys::AV_SAMPLE_FMT_FLT, SampleLayout::Interleaved),
        ]
        .into_iter()
        .find(|(format, _)| supported.is_empty() || supported.contains(format))
        .ok_or_else(|| Error::unavailable(format!("{name} does not take float samples")))?;

        let layout = ChannelLayout::default_for(AUDIO_CHANNELS)?;
        let mut context = CodecContext::new(codec)?;
        let raw = context.unopened_mut();
        raw.sample_rate = to_int(AUDIO_SAMPLE_RATE, "the sample rate")?;
        raw.sample_fmt = sample_format;
        raw.bit_rate = i64::from(settings.bitrate);
        raw.time_base = AUDIO_TIME_BASE.into();
        raw.flags |= sys::AV_CODEC_FLAG_GLOBAL_HEADER.cast_signed();
        layout.copy_into(&mut raw.ch_layout)?;
        context.open(codec, "open the audio encoder")?;

        let frame_samples = context
            .read(|raw| usize::try_from(raw.frame_size).ok())
            .filter(|&samples| samples > 0)
            .unwrap_or(FALLBACK_AUDIO_FRAME);
        let info = StreamInfo {
            time_base: AUDIO_TIME_BASE,
            parameters: CodecParameters::from_context(&context)?,
        };
        Ok(Self {
            context,
            packet: RawPacket::new()?,
            layout,
            sample_format,
            sample_layout,
            frame_samples,
            pending: Vec::new(),
            next_pts: 0,
            finished: false,
            info,
        })
    }

    pub fn stream_info(&self) -> &StreamInfo {
        &self.info
    }

    /// Takes interleaved stereo samples at 48 kHz, any number of whole frames,
    /// and returns the packets that are ready.
    pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<Packet>> {
        if self.finished {
            return Err(Error::invalid("the audio encoder is already finished"));
        }
        let channels = usize::from(AUDIO_CHANNELS);
        if !samples.len().is_multiple_of(channels) {
            return Err(Error::invalid(
                "stereo sound needs an even number of samples",
            ));
        }
        self.pending.extend_from_slice(samples);
        let chunk = self.frame_samples * channels;
        let mut packets = Vec::new();
        let mut consumed = 0;
        while self.pending.len() - consumed >= chunk {
            let range = consumed..consumed + chunk;
            consumed += chunk;
            let outcome = self.encode_chunk(range);
            match outcome {
                Ok(mut ready) => packets.append(&mut ready),
                Err(error) => {
                    self.pending.drain(..consumed);
                    return Err(error);
                }
            }
        }
        self.pending.drain(..consumed);
        Ok(packets)
    }

    /// Encodes what is left, padded by the encoder, and returns the last
    /// packets. Nothing can be encoded afterwards.
    pub fn finish(&mut self) -> Result<Vec<Packet>> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;
        let mut packets = Vec::new();
        if !self.pending.is_empty() {
            packets = self.encode_chunk(0..self.pending.len())?;
            self.pending.clear();
        }
        self.context.send_frame(None, "flush the audio encoder")?;
        packets.append(&mut self.drain()?);
        Ok(packets)
    }

    fn encode_chunk(&mut self, range: std::ops::Range<usize>) -> Result<Vec<Packet>> {
        let channels = usize::from(AUDIO_CHANNELS);
        let samples = &self.pending[range];
        let count = samples.len() / channels;
        let mut frame = Frame::audio(
            self.sample_format,
            &self.layout,
            to_int(AUDIO_SAMPLE_RATE, "the sample rate")?,
            count,
        )?;
        let missing = || Error::invalid("FFmpeg allocated an audio frame without its planes");
        match self.sample_layout {
            SampleLayout::Planar => {
                for channel in 0..channels {
                    let plane = frame
                        .sample_plane_mut(channel, count * size_of::<f32>())
                        .ok_or_else(missing)?;
                    let source = samples.iter().skip(channel).step_by(channels);
                    for (target, sample) in plane.as_chunks_mut::<4>().0.iter_mut().zip(source) {
                        target.copy_from_slice(&sample.to_ne_bytes());
                    }
                }
            }
            SampleLayout::Interleaved => {
                let plane = frame
                    .sample_plane_mut(0, size_of_val(samples))
                    .ok_or_else(missing)?;
                for (target, sample) in plane.as_chunks_mut::<4>().0.iter_mut().zip(samples) {
                    target.copy_from_slice(&sample.to_ne_bytes());
                }
            }
        }
        frame.raw_mut().pts = self.next_pts;
        self.context.send_frame(Some(&frame), "encode sound")?;
        self.next_pts += i64::try_from(count).unwrap_or(0);
        self.drain()
    }

    fn drain(&mut self) -> Result<Vec<Packet>> {
        let mut packets = Vec::new();
        while self
            .context
            .receive_packet(&mut self.packet, "read an encoded audio packet")?
        {
            let mut packet = self.packet.to_plain();
            self.packet.unref();
            packet.stream = 0;
            packet.keyframe = true;
            packets.push(packet);
        }
        Ok(packets)
    }
}
