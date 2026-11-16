use std::sync::Arc;

use ngine_capture::{CpuFrame, Size, VideoFrame};
use ngine_media::{
    Packet, PixelFormat, PixelView, PlaneView, Rational, StreamInfo, VideoEncoder,
    VideoEncoderSettings, VideoInput, VideoSource,
};

use crate::error::RecorderError;
use crate::settings::{ChosenEncoder, VideoSettings, video_bit_rate_cap};
use crate::time::{VIDEO_TIME_BASE, VIDEO_UNITS_PER_SECOND};

const AVERAGE_PERCENT_OF_CAP: u64 = 75;
const BACKSTOP_KEYFRAME_SECONDS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Geometry {
    size: Size,
    format: ngine_capture::PixelFormat,
    on_gpu: bool,
}

impl Geometry {
    pub(crate) fn of(frame: &VideoFrame, encoder: &ChosenEncoder) -> Self {
        Self {
            size: frame.size(),
            format: frame.format(),
            on_gpu: gpu::can_take(frame, encoder),
        }
    }

    fn encoded_size(self) -> Size {
        Size::new(self.size.width & !1, self.size.height & !1)
    }
}

enum Picture {
    Cpu(Arc<CpuFrame>),
    Gpu(gpu::Held),
}

pub(crate) struct VideoStage {
    encoder: VideoEncoder,
    _gpu: Option<gpu::Pool>,
    geometry: Geometry,
    last_picture: Option<Picture>,
    last_pts: Option<i64>,
    last_keyframe_request: i64,
}

fn encoder_settings(
    settings: &VideoSettings,
    encoder: &ChosenEncoder,
    size: Size,
) -> Result<VideoEncoderSettings, RecorderError> {
    let frame_rate = i32::try_from(settings.frame_rate)
        .map_err(|_| RecorderError::FrameRateOutOfRange(settings.frame_rate))?;
    let cap = video_bit_rate_cap(settings.codec, settings.quality, settings.frame_rate);
    Ok(VideoEncoderSettings {
        encoder: encoder.name.clone(),
        width: size.width,
        height: size.height,
        time_base: VIDEO_TIME_BASE,
        frame_rate: Rational::new(frame_rate, 1),
        bitrate: (cap * AVERAGE_PERCENT_OF_CAP / 100).max(1),
        max_bitrate: cap.max(1),
        keyframe_interval: settings
            .frame_rate
            .saturating_mul(BACKSTOP_KEYFRAME_SECONDS),
    })
}

fn media_format(format: ngine_capture::PixelFormat) -> PixelFormat {
    match format {
        ngine_capture::PixelFormat::Nv12 => PixelFormat::Nv12,
        _ => PixelFormat::Bgra,
    }
}

impl VideoStage {
    pub(crate) fn open(
        settings: &VideoSettings,
        encoder: &ChosenEncoder,
        geometry: Geometry,
    ) -> Result<Self, RecorderError> {
        let size = geometry.encoded_size();
        let encoder_settings = encoder_settings(settings, encoder, size)?;
        let format = media_format(geometry.format);
        if geometry.on_gpu
            && let Some((pool, opened)) = gpu::open(&encoder_settings, format)
        {
            return Ok(Self::with(opened, Some(pool), geometry));
        }
        let geometry = Geometry {
            on_gpu: false,
            ..geometry
        };
        let opened = VideoEncoder::open(&encoder_settings, VideoSource::Cpu(format))?;
        Ok(Self::with(opened, None, geometry))
    }

    fn with(encoder: VideoEncoder, pool: Option<gpu::Pool>, geometry: Geometry) -> Self {
        Self {
            encoder,
            _gpu: pool,
            geometry,
            last_picture: None,
            last_pts: None,
            last_keyframe_request: 0,
        }
    }

    pub(crate) fn takes(&self, frame: &VideoFrame) -> bool {
        frame.size() == self.geometry.size && frame.format() == self.geometry.format
    }

    pub(crate) fn stream_info(&self) -> &StreamInfo {
        self.encoder.stream_info()
    }

    pub(crate) fn encoder_name(&self) -> &str {
        self.encoder.name()
    }

    pub(crate) fn reads_the_gpu_surface(&self) -> bool {
        self.geometry.on_gpu
    }

    pub(crate) fn last_pts(&self) -> Option<i64> {
        self.last_pts
    }

    pub(crate) fn request_keyframe(&mut self) {
        self.encoder.request_keyframe();
    }

    fn keyframe_if_due(&mut self, pts: i64, interval: i64) {
        if pts - self.last_keyframe_request >= interval {
            self.encoder.request_keyframe();
            self.last_keyframe_request = pts;
        }
    }

    pub(crate) fn encode(
        &mut self,
        frame: &VideoFrame,
        pts: i64,
        keyframe_interval: i64,
    ) -> Result<Vec<Packet>, RecorderError> {
        let held = if self.geometry.on_gpu {
            gpu::hold(frame)
        } else {
            None
        };
        let picture = match held {
            Some(held) => Picture::Gpu(held?),
            None => Picture::Cpu(frame.to_cpu()?),
        };
        self.last_picture = Some(picture);
        self.repeat(pts, keyframe_interval)
    }

    pub(crate) fn repeat(
        &mut self,
        pts: i64,
        keyframe_interval: i64,
    ) -> Result<Vec<Packet>, RecorderError> {
        if self.last_picture.is_none() {
            return Ok(Vec::new());
        }
        self.keyframe_if_due(pts, keyframe_interval);
        let size = self.geometry.encoded_size();
        let packets = match &self.last_picture {
            None => Vec::new(),
            Some(Picture::Gpu(held)) => gpu::encode(&mut self.encoder, held, pts)?,
            Some(Picture::Cpu(pixels)) => encode_cpu(&mut self.encoder, pixels, size, pts)?,
        };
        self.last_pts = Some(pts);
        Ok(packets)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<Packet>, RecorderError> {
        Ok(self.encoder.finish()?)
    }
}

fn encode_cpu(
    encoder: &mut VideoEncoder,
    pixels: &CpuFrame,
    size: Size,
    pts: i64,
) -> Result<Vec<Packet>, RecorderError> {
    let stride = pixels.stride();
    let data = pixels.data();
    let packets = match pixels.format() {
        ngine_capture::PixelFormat::Nv12 => {
            let luma_len = stride * pixels.size().height as usize;
            let (luma, chroma) = data.split_at(luma_len.min(data.len()));
            let view = PixelView::nv12(
                size.width,
                size.height,
                PlaneView { data: luma, stride },
                PlaneView {
                    data: chroma,
                    stride,
                },
            )?;
            encoder.encode(VideoInput::Cpu(&view), pts)?
        }
        ngine_capture::PixelFormat::Rgba8 => {
            let swapped: Vec<u8> = data
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|[red, green, blue, alpha]| [*blue, *green, *red, *alpha])
                .collect();
            let plane = PlaneView {
                data: &swapped,
                stride,
            };
            let view = PixelView::bgra(size.width, size.height, plane)?;
            encoder.encode(VideoInput::Cpu(&view), pts)?
        }
        _ => {
            let view = PixelView::bgra(size.width, size.height, PlaneView { data, stride })?;
            encoder.encode(VideoInput::Cpu(&view), pts)?
        }
    };
    Ok(packets)
}

pub(crate) fn frame_period(frame_rate: u32) -> i64 {
    VIDEO_UNITS_PER_SECOND / i64::from(frame_rate.max(1))
}

#[cfg(target_os = "macos")]
mod gpu {
    use ngine_capture::{Surface, VideoFrame};
    use ngine_media::{
        GpuFramePool, GpuSurface, Packet, PixelFormat, VideoEncoder, VideoEncoderSettings,
        VideoInput, VideoSource,
    };

    use crate::error::RecorderError;
    use crate::settings::ChosenEncoder;

    pub(super) type Pool = GpuFramePool;
    pub(super) type Held = GpuSurface;

    pub(super) fn can_take(frame: &VideoFrame, encoder: &ChosenEncoder) -> bool {
        let size = frame.size();
        matches!(frame.surface(), Surface::CoreVideo(_))
            && encoder.name.ends_with("_videotoolbox")
            && size.width.is_multiple_of(2)
            && size.height.is_multiple_of(2)
    }

    pub(super) fn open(
        settings: &VideoEncoderSettings,
        format: PixelFormat,
    ) -> Option<(Pool, VideoEncoder)> {
        let pool = GpuFramePool::new(settings.width, settings.height, format).ok()?;
        let encoder = VideoEncoder::open(settings, VideoSource::Gpu(&pool)).ok()?;
        Some((pool, encoder))
    }

    pub(super) fn hold(frame: &VideoFrame) -> Option<Result<Held, RecorderError>> {
        match frame.surface() {
            Surface::CoreVideo(surface) => {
                Some(GpuSurface::from_capture(surface).map_err(RecorderError::from))
            }
            _ => None,
        }
    }

    pub(super) fn encode(
        encoder: &mut VideoEncoder,
        held: &Held,
        pts: i64,
    ) -> Result<Vec<Packet>, RecorderError> {
        Ok(encoder.encode(VideoInput::Gpu(held), pts)?)
    }
}

#[cfg(not(target_os = "macos"))]
mod gpu {
    use ngine_capture::VideoFrame;
    use ngine_media::{Packet, PixelFormat, VideoEncoder, VideoEncoderSettings};

    use crate::error::RecorderError;
    use crate::settings::ChosenEncoder;

    pub(super) enum Pool {}
    pub(super) enum Held {}

    pub(super) fn can_take(_frame: &VideoFrame, _encoder: &ChosenEncoder) -> bool {
        false
    }

    pub(super) fn open(
        _settings: &VideoEncoderSettings,
        _format: PixelFormat,
    ) -> Option<(Pool, VideoEncoder)> {
        None
    }

    pub(super) fn hold(_frame: &VideoFrame) -> Option<Result<Held, RecorderError>> {
        None
    }

    pub(super) fn encode(
        _encoder: &mut VideoEncoder,
        held: &Held,
        _pts: i64,
    ) -> Result<Vec<Packet>, RecorderError> {
        match *held {}
    }
}
