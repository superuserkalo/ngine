use std::sync::Arc;

use ngine_capture::{CpuFrame, Size, Surface, VideoFrame};
use ngine_media::{
    Arrival, Packet, PixelFormat, PixelView, PlaneView, Rational, Route, StreamInfo, SurfaceKind,
    VideoEncoder, VideoEncoderSettings, VideoInput, VideoSource, routes,
};

use crate::error::RecorderError;
use crate::settings::{ChosenEncoder, VideoSettings, video_bit_rate_cap};
use crate::time::{VIDEO_TIME_BASE, VIDEO_UNITS_PER_SECOND};

const AVERAGE_PERCENT_OF_CAP: u64 = 75;
const BACKSTOP_KEYFRAME_SECONDS: u32 = 4;

pub(crate) fn surface_kind(frame: &VideoFrame) -> SurfaceKind {
    match frame.surface() {
        #[cfg(target_os = "macos")]
        Surface::CoreVideo(_) => SurfaceKind::CoreVideo,
        #[cfg(target_os = "windows")]
        Surface::D3d11(_) => SurfaceKind::D3d11,
        #[cfg(target_os = "linux")]
        Surface::DmaBuf(_) => SurfaceKind::DmaBuf,
        _ => SurfaceKind::Cpu,
    }
}

pub(crate) fn cannot_be_copied(surface: SurfaceKind) -> bool {
    surface == SurfaceKind::DmaBuf
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Geometry {
    size: Size,
    format: ngine_capture::PixelFormat,
    surface: SurfaceKind,
}

impl Geometry {
    pub(crate) fn of(frame: &VideoFrame) -> Self {
        Self {
            size: frame.size(),
            format: frame.format(),
            surface: surface_kind(frame),
        }
    }

    fn encoded_size(self) -> Size {
        Size::new(self.size.width & !1, self.size.height & !1)
    }

    fn arrival(self) -> Arrival {
        Arrival {
            surface: self.surface,
            is_bgra: self.format == ngine_capture::PixelFormat::Bgra8,
            has_even_size: self.size == self.encoded_size(),
        }
    }
}

pub(crate) fn routes_to_try(
    encoder: &str,
    arrival: Arrival,
    gpu_route_allowed: bool,
) -> Vec<Route> {
    routes(encoder, arrival)
        .into_iter()
        .filter(|route| gpu_route_allowed || *route != Route::GpuSurface)
        .collect()
}

enum Picture {
    Cpu(Arc<CpuFrame>),
    Gpu(gpu::Held),
}

pub(crate) struct VideoStage {
    encoder: VideoEncoder,
    gpu: Option<gpu::Pool>,
    geometry: Geometry,
    last_picture: Option<Picture>,
    last_pts: Option<i64>,
    last_keyframe_request: i64,
}

pub(crate) struct Opened {
    pub(crate) stage: VideoStage,
    pub(crate) gpu_route_failed: bool,
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
        frame: &VideoFrame,
        gpu_route_allowed: bool,
    ) -> Result<Opened, RecorderError> {
        let geometry = Geometry::of(frame);
        let encoder_settings = encoder_settings(settings, encoder, geometry.encoded_size())?;
        let mut gpu_route_failed = false;
        for route in routes_to_try(&encoder.name, geometry.arrival(), gpu_route_allowed) {
            match route {
                Route::GpuSurface => match gpu::open(&encoder_settings, frame) {
                    Some((pool, opened)) => {
                        return Ok(Opened {
                            stage: Self::with(opened, Some(pool), geometry),
                            gpu_route_failed,
                        });
                    }
                    None => gpu_route_failed = true,
                },
                Route::CpuPixels => {
                    let format = media_format(geometry.format);
                    let opened = VideoEncoder::open(&encoder_settings, VideoSource::Cpu(format))?;
                    return Ok(Opened {
                        stage: Self::with(opened, None, geometry),
                        gpu_route_failed,
                    });
                }
            }
        }
        Err(RecorderError::EncoderUnavailable(encoder.name.clone()))
    }

    fn with(encoder: VideoEncoder, pool: Option<gpu::Pool>, geometry: Geometry) -> Self {
        Self {
            encoder,
            gpu: pool,
            geometry,
            last_picture: None,
            last_pts: None,
            last_keyframe_request: 0,
        }
    }

    pub(crate) fn takes(&self, frame: &VideoFrame) -> bool {
        Geometry::of(frame) == self.geometry
    }

    pub(crate) fn stream_info(&self) -> &StreamInfo {
        self.encoder.stream_info()
    }

    pub(crate) fn encoder_name(&self) -> &str {
        self.encoder.name()
    }

    pub(crate) fn reads_the_gpu_surface(&self) -> bool {
        self.gpu.is_some()
    }

    pub(crate) fn surface(&self) -> SurfaceKind {
        self.geometry.surface
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
        let picture = match self.gpu.as_mut() {
            Some(pool) => Picture::Gpu(gpu::hold(pool, frame)?),
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

    pub(super) type Pool = GpuFramePool;
    pub(super) type Held = GpuSurface;

    pub(super) fn open(
        settings: &VideoEncoderSettings,
        frame: &VideoFrame,
    ) -> Option<(Pool, VideoEncoder)> {
        let format = match frame.format() {
            ngine_capture::PixelFormat::Nv12 => PixelFormat::Nv12,
            _ => PixelFormat::Bgra,
        };
        let pool = GpuFramePool::new(settings.width, settings.height, format).ok()?;
        let encoder = VideoEncoder::open(settings, VideoSource::Gpu(&pool)).ok()?;
        Some((pool, encoder))
    }

    pub(super) fn hold(_pool: &mut Pool, frame: &VideoFrame) -> Result<Held, RecorderError> {
        match frame.surface() {
            Surface::CoreVideo(surface) => Ok(GpuSurface::from_capture(surface)?),
            _ => Err(super::not_a_surface()),
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

#[cfg(target_os = "linux")]
mod gpu {
    use ngine_capture::{Surface, VideoFrame};
    use ngine_media::{
        GpuFramePool, GpuSurface, Packet, VideoEncoder, VideoEncoderSettings, VideoInput,
        VideoSource,
    };

    use crate::error::RecorderError;

    pub(super) type Pool = GpuFramePool;
    pub(super) type Held = GpuSurface;

    pub(super) fn open(
        settings: &VideoEncoderSettings,
        frame: &VideoFrame,
    ) -> Option<(Pool, VideoEncoder)> {
        let Surface::DmaBuf(surface) = frame.surface() else {
            return None;
        };
        let pool = GpuFramePool::for_dma_buf(surface).ok()?;
        let encoder = VideoEncoder::open(settings, VideoSource::Gpu(&pool)).ok()?;
        Some((pool, encoder))
    }

    pub(super) fn hold(pool: &mut Pool, frame: &VideoFrame) -> Result<Held, RecorderError> {
        match frame.surface() {
            Surface::DmaBuf(surface) => Ok(pool.import(surface)?),
            _ => Err(super::not_a_surface()),
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

#[cfg(target_os = "windows")]
mod gpu {
    use ngine_capture::{Surface, VideoFrame};
    use ngine_media::{
        GpuFramePool, GpuSurface, Packet, VideoEncoder, VideoEncoderSettings, VideoInput,
        VideoSource,
    };

    use crate::error::RecorderError;

    pub(super) type Pool = GpuFramePool;
    pub(super) type Held = GpuSurface;

    pub(super) fn open(
        settings: &VideoEncoderSettings,
        frame: &VideoFrame,
    ) -> Option<(Pool, VideoEncoder)> {
        let Surface::D3d11(surface) = frame.surface() else {
            return None;
        };
        let pool = GpuFramePool::for_d3d11(surface, frame.size()).ok()?;
        let encoder = VideoEncoder::open(settings, VideoSource::Gpu(&pool)).ok()?;
        Some((pool, encoder))
    }

    pub(super) fn hold(pool: &mut Pool, frame: &VideoFrame) -> Result<Held, RecorderError> {
        match frame.surface() {
            Surface::D3d11(surface) => Ok(pool.import(surface)?),
            _ => Err(super::not_a_surface()),
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

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod gpu {
    use ngine_capture::VideoFrame;
    use ngine_media::{Packet, VideoEncoder, VideoEncoderSettings};

    use crate::error::RecorderError;

    pub(super) enum Pool {}
    pub(super) enum Held {}

    pub(super) fn open(
        _settings: &VideoEncoderSettings,
        _frame: &VideoFrame,
    ) -> Option<(Pool, VideoEncoder)> {
        None
    }

    pub(super) fn hold(pool: &mut Pool, _frame: &VideoFrame) -> Result<Held, RecorderError> {
        match *pool {}
    }

    pub(super) fn encode(
        _encoder: &mut VideoEncoder,
        held: &Held,
        _pts: i64,
    ) -> Result<Vec<Packet>, RecorderError> {
        match *held {}
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn not_a_surface() -> RecorderError {
    RecorderError::Media(ngine_media::Error::Invalid(
        "the frame is not the kind of surface the encoder was opened for".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrival(surface: SurfaceKind) -> Arrival {
        Arrival {
            surface,
            is_bgra: true,
            has_even_size: true,
        }
    }

    #[test]
    fn a_gpu_route_that_failed_before_is_not_tried_again() {
        let on_the_gpu = arrival(SurfaceKind::D3d11);

        assert_eq!(
            routes_to_try("h264_nvenc", on_the_gpu, true),
            [Route::GpuSurface, Route::CpuPixels]
        );
        assert_eq!(
            routes_to_try("h264_nvenc", on_the_gpu, false),
            [Route::CpuPixels]
        );
        assert_eq!(
            routes_to_try("libopenh264", on_the_gpu, true),
            [Route::CpuPixels]
        );
    }

    #[test]
    fn only_a_dma_buf_has_no_copy_to_fall_back_on() {
        assert!(cannot_be_copied(SurfaceKind::DmaBuf));
        assert!(!cannot_be_copied(SurfaceKind::D3d11));
        assert!(!cannot_be_copied(SurfaceKind::CoreVideo));
        assert!(!cannot_be_copied(SurfaceKind::Cpu));
    }

    #[test]
    fn pixels_in_memory_open_a_stage_that_copies_and_report_no_failed_gpu_route() {
        let pixels = Arc::new(CpuFrame::solid_bgra(Size::new(64, 36), [0, 0, 0, 255]).unwrap());
        let frame = VideoFrame::from_cpu(ngine_capture::Timestamp::from_nanos(0), pixels);
        let settings = VideoSettings {
            codec: ngine_media::VideoCodec::H264,
            quality: crate::settings::Quality::Low,
            frame_rate: 30,
            encoder: None,
        };
        let software = ChosenEncoder {
            name: ngine_media::SOFTWARE_H264.to_owned(),
        };

        let opened = VideoStage::open(&settings, &software, &frame, true).unwrap();

        assert!(!opened.gpu_route_failed);
        assert!(!opened.stage.reads_the_gpu_surface());
        assert_eq!(opened.stage.surface(), SurfaceKind::Cpu);
        assert!(opened.stage.takes(&frame));
    }
}
