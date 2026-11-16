use std::ffi::c_int;
use std::ptr;

use super::resample::{AudioLayout, Converter};
use super::scale::Scaler;
use super::{CodecContext, CodecRef, Frame, RawPacket, StreamInfo, check, optional_timestamp, sys};
use crate::error::{Error, Result};
use crate::types::{AudioFrame, Packet, PixelFormat, Plane, VideoFrame};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoDecoderOptions {
    pub output: PixelFormat,
    /// Decode on the GPU when `FFmpeg` and the machine can, and on the CPU
    /// otherwise.
    pub hardware: bool,
}

fn open_decoder(stream: &StreamInfo, video: bool) -> Result<(CodecContext, CodecRef)> {
    let parameters = stream.parameters.raw();
    if video != stream.parameters.is_video() || video == stream.parameters.is_audio() {
        return Err(Error::invalid(
            "the stream is not the kind this decoder takes",
        ));
    }
    let codec = CodecRef::decoder(parameters.codec_id)
        .ok_or_else(|| Error::unavailable(format!("a decoder for {:?}", stream.parameters)))?;
    let mut context = CodecContext::new(codec)?;
    // SAFETY: the context is new and unopened and the parameters are valid.
    let status =
        unsafe { sys::avcodec_parameters_to_context(context.as_ptr(), ptr::from_ref(parameters)) };
    check(status, "configure the decoder")?;
    context.unopened_mut().pkt_timebase = stream.time_base.into();
    Ok((context, codec))
}

fn hardware_device_type(
    codec: *const sys::AVCodec,
    format: sys::AVPixelFormat,
) -> Option<sys::AVHWDeviceType> {
    hardware_configs(codec)
        .find(|config| config.pix_fmt == format)
        .map(|config| config.device_type)
}

fn hardware_configs(
    codec: *const sys::AVCodec,
) -> impl Iterator<Item = &'static sys::AVCodecHWConfig> {
    (0..)
        .map_while(move |index| {
            // SAFETY: the codec is a registered codec and any index is
            // allowed.
            let config = unsafe { sys::avcodec_get_hw_config(codec, index) };
            // SAFETY: FFmpeg returns null past the last configuration and a
            // pointer to static data before it.
            unsafe { config.as_ref() }
        })
        .filter(|config| {
            i64::from(config.methods) & i64::from(sys::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX.0)
                != 0
        })
}

fn device_type_of(device: &sys::AVBufferRef) -> Option<sys::AVHWDeviceType> {
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "FFmpeg allocated this buffer as an AVHWDeviceContext, so it is aligned for one"
    )]
    let context = device.data.cast::<sys::AVHWDeviceContext>();
    // SAFETY: the data of a hardware device reference is a live
    // AVHWDeviceContext whose type never changes.
    unsafe { context.as_ref() }.map(|context| context.type_)
}

unsafe extern "C" fn prefer_hardware_format(
    context: *mut sys::AVCodecContext,
    formats: *const sys::AVPixelFormat,
) -> sys::AVPixelFormat {
    // SAFETY: libavcodec calls this with the decoder's own live context and
    // does not touch it while the callback runs.
    let decoder = unsafe { &*context };
    let codec = decoder.codec;
    // SAFETY: the device reference set in `VideoDecoder::open` stays alive as
    // long as the context.
    let device = unsafe { decoder.hw_device_ctx.as_ref() };
    let device_type = device.and_then(device_type_of);
    let mut software = sys::AV_PIX_FMT_NONE;
    for offset in 0.. {
        // SAFETY: libavcodec passes a list terminated by AV_PIX_FMT_NONE and
        // the loop stops at that terminator, so the offset stays inside it.
        let entry = unsafe { formats.add(offset) };
        // SAFETY: as above, `entry` points at an element of the list.
        let format = unsafe { *entry };
        if format == sys::AV_PIX_FMT_NONE {
            break;
        }
        // SAFETY: av_pix_fmt_desc_get accepts any format value.
        let descriptor = unsafe { sys::av_pix_fmt_desc_get(format) };
        // SAFETY: the descriptor is null or points at static data.
        let descriptor = unsafe { descriptor.as_ref() };
        let is_hardware = descriptor.is_some_and(|descriptor| {
            descriptor.flags & u64::from(sys::AV_PIX_FMT_FLAG_HWACCEL) != 0
        });
        if !is_hardware {
            if software == sys::AV_PIX_FMT_NONE {
                software = format;
            }
        } else if device_type.is_some() && hardware_device_type(codec, format) == device_type {
            return format;
        }
    }
    software
}

/// Decodes H.264, HEVC or AV1 into CPU pixels.
pub struct VideoDecoder {
    context: CodecContext,
    frame: Frame,
    downloaded: Frame,
    scaler: Scaler,
    output: PixelFormat,
    hardware: bool,
}

impl VideoDecoder {
    pub fn open(stream: &StreamInfo, options: VideoDecoderOptions) -> Result<Self> {
        let (mut context, codec) = open_decoder(stream, true)?;
        let mut hardware = false;
        if options.hardware {
            for config in hardware_configs(codec.as_ptr()) {
                let mut device = ptr::null_mut();
                // SAFETY: `device` is a live out-pointer and the type comes
                // from the codec's own configuration list.
                let status = unsafe {
                    sys::av_hwdevice_ctx_create(
                        &raw mut device,
                        config.device_type,
                        ptr::null(),
                        ptr::null_mut(),
                        0,
                    )
                };
                if status >= 0 && !device.is_null() {
                    let raw = context.unopened_mut();
                    raw.hw_device_ctx = device;
                    raw.get_format = Some(prefer_hardware_format);
                    hardware = true;
                    break;
                }
            }
        }
        context.open(codec, "open the video decoder")?;
        Ok(Self {
            context,
            frame: Frame::new()?,
            downloaded: Frame::new()?,
            scaler: Scaler::new()?,
            output: options.output,
            hardware,
        })
    }

    /// Whether a hardware device was attached when the decoder was opened.
    pub fn has_hardware_device(&self) -> bool {
        self.hardware
    }

    /// Feeds one packet and returns the pictures that are ready, in display
    /// order, stamped in the stream's time base.
    pub fn decode(&mut self, packet: &Packet) -> Result<Vec<VideoFrame>> {
        let raw = RawPacket::from_plain(packet)?;
        self.context
            .send_packet(Some(&raw), "decode a video packet")?;
        self.drain()
    }

    /// Returns the pictures still inside the decoder at the end of a stream.
    pub fn finish(&mut self) -> Result<Vec<VideoFrame>> {
        self.context.send_packet(None, "flush the video decoder")?;
        let frames = self.drain();
        self.context.flush();
        frames
    }

    /// Forgets everything buffered. Call it after a seek.
    pub fn reset(&mut self) {
        self.context.flush();
    }

    fn drain(&mut self) -> Result<Vec<VideoFrame>> {
        let mut frames = Vec::new();
        while self
            .context
            .receive_frame(&mut self.frame, "read a decoded picture")?
        {
            let pts = optional_timestamp(self.frame.raw().best_effort_timestamp);
            let software = if self.frame.raw().hw_frames_ctx.is_null() {
                &self.frame
            } else {
                self.downloaded.unref();
                // SAFETY: the source is a hardware frame from the decoder and
                // the target is blank, so FFmpeg picks a software format and
                // allocates the target's buffers.
                let status = unsafe {
                    sys::av_hwframe_transfer_data(self.downloaded.as_ptr(), self.frame.as_ptr(), 0)
                };
                check(status, "download a decoded picture from the GPU")?;
                let source = self.frame.raw();
                let (colorspace, color_range) = (source.colorspace, source.color_range);
                let target = self.downloaded.raw_mut();
                target.colorspace = colorspace;
                target.color_range = color_range;
                &self.downloaded
            };
            let converted;
            let ready = if software.raw().format == self.output.raw().0 {
                software
            } else {
                converted = self.scaler.convert(software, self.output.raw())?;
                &converted
            };
            frames.push(copy_out(ready, self.output, pts)?);
            self.frame.unref();
        }
        Ok(frames)
    }
}

fn copy_out(frame: &Frame, format: PixelFormat, pts: Option<i64>) -> Result<VideoFrame> {
    let raw = frame.raw();
    let size = |value: c_int| {
        u32::try_from(value).map_err(|_| Error::invalid("the decoder produced a negative size"))
    };
    let (width, height) = (size(raw.width)?, size(raw.height)?);
    crate::types::check_dimensions(width, height)?;
    let mut planes = Vec::new();
    for (index, shape) in format.plane_shapes(width, height).into_iter().enumerate() {
        let (bytes, stride) = frame
            .plane(index, shape)
            .ok_or_else(|| Error::invalid("the decoded picture is missing a plane"))?;
        let mut data = Vec::with_capacity(shape.row_bytes * shape.rows);
        for row in bytes.chunks(stride) {
            data.extend_from_slice(&row[..shape.row_bytes]);
        }
        planes.push(Plane {
            data,
            stride: shape.row_bytes,
        });
    }
    Ok(VideoFrame {
        format,
        width,
        height,
        pts,
        planes,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SoundShape {
    format: c_int,
    sample_rate: c_int,
    channels: c_int,
}

/// Decodes AAC or Opus into interleaved float samples at the stream's own
/// rate and channel count.
pub struct AudioDecoder {
    context: CodecContext,
    frame: Frame,
    converter: Option<(SoundShape, AudioLayout, Converter)>,
}

impl AudioDecoder {
    pub fn open(stream: &StreamInfo) -> Result<Self> {
        let (mut context, codec) = open_decoder(stream, false)?;
        context.open(codec, "open the audio decoder")?;
        Ok(Self {
            context,
            frame: Frame::new()?,
            converter: None,
        })
    }

    /// Feeds one packet and returns the sound that is ready, stamped in the
    /// stream's time base. Samples that the packet marks as skipped, such as
    /// encoder priming, are already removed.
    pub fn decode(&mut self, packet: &Packet) -> Result<Vec<AudioFrame>> {
        let raw = RawPacket::from_plain(packet)?;
        self.context
            .send_packet(Some(&raw), "decode an audio packet")?;
        self.drain()
    }

    /// Returns the sound still inside the decoder at the end of a stream.
    pub fn finish(&mut self) -> Result<Vec<AudioFrame>> {
        self.context.send_packet(None, "flush the audio decoder")?;
        let frames = self.drain();
        self.context.flush();
        frames
    }

    /// Forgets everything buffered. Call it after a seek.
    pub fn reset(&mut self) {
        self.context.flush();
    }

    fn drain(&mut self) -> Result<Vec<AudioFrame>> {
        let mut frames = Vec::new();
        while self
            .context
            .receive_frame(&mut self.frame, "read decoded sound")?
        {
            let raw = self.frame.raw();
            let shape = SoundShape {
                format: raw.format,
                sample_rate: raw.sample_rate,
                channels: raw.ch_layout.nb_channels,
            };
            if self
                .converter
                .as_ref()
                .is_none_or(|(known, ..)| *known != shape)
            {
                let layout = AudioLayout {
                    sample_rate: u32::try_from(shape.sample_rate)
                        .map_err(|_| Error::invalid("the decoder produced a negative rate"))?,
                    channels: u16::try_from(shape.channels)
                        .map_err(|_| Error::invalid("the decoder produced too many channels"))?,
                };
                let converter = Converter::new(
                    &raw const raw.ch_layout,
                    sys::AVSampleFormat(shape.format),
                    shape.sample_rate,
                    layout,
                    &[],
                )?;
                self.converter = Some((shape, layout, converter));
            }
            let Some((_, layout, converter)) = self.converter.as_mut() else {
                return Err(Error::invalid("the audio converter is missing"));
            };
            let samples = converter.convert_frame(&self.frame)?;
            frames.push(AudioFrame {
                pts: optional_timestamp(self.frame.raw().best_effort_timestamp),
                sample_rate: layout.sample_rate,
                channels: layout.channels,
                samples,
            });
            self.frame.unref();
        }
        Ok(frames)
    }
}
