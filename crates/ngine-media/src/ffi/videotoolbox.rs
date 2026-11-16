use std::ffi::c_void;
use std::ptr::{self, NonNull};

use ngine_capture::CoreVideoSurface;

use super::encode::frame_from_pixels;
use super::{Frame, check, out_of_memory, sys, to_int};
use crate::error::{Error, Result};
use crate::types::{PixelFormat, PixelView, check_dimensions};

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
    fn CFRetain(object: *const c_void) -> *const c_void;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferGetWidth(buffer: *const c_void) -> usize;
    fn CVPixelBufferGetHeight(buffer: *const c_void) -> usize;
}

struct BufferRef(NonNull<sys::AVBufferRef>);

// SAFETY: AVBufferRef reference counts are atomic and the hardware device and
// frame contexts held through it are documented as usable from any thread.
unsafe impl Send for BufferRef {}

impl BufferRef {
    fn from_new(buffer: *mut sys::AVBufferRef, action: &'static str) -> Result<Self> {
        NonNull::new(buffer)
            .map(Self)
            .ok_or_else(|| out_of_memory(action))
    }

    fn as_ptr(&self) -> *mut sys::AVBufferRef {
        self.0.as_ptr()
    }

    fn new_reference(&self) -> Result<*mut sys::AVBufferRef> {
        // SAFETY: the buffer reference is valid; av_buffer_ref adds a
        // reference that the caller hands to an FFmpeg structure to own.
        let reference = unsafe { sys::av_buffer_ref(self.as_ptr()) };
        if reference.is_null() {
            Err(out_of_memory("reference a buffer"))
        } else {
            Ok(reference)
        }
    }
}

impl Drop for BufferRef {
    fn drop(&mut self) {
        let mut buffer = self.0.as_ptr();
        // SAFETY: this reference came from FFmpeg and is not used again.
        unsafe { sys::av_buffer_unref(&raw mut buffer) }
    }
}

/// A pool of `CVPixelBuffer`s that the `VideoToolbox` encoders read without a
/// CPU copy.
pub struct GpuFramePool {
    frames: BufferRef,
    width: u32,
    height: u32,
    format: PixelFormat,
}

impl GpuFramePool {
    pub(super) const SURFACE_FORMAT: sys::AVPixelFormat = sys::AV_PIX_FMT_VIDEOTOOLBOX;

    pub fn new(width: u32, height: u32, format: PixelFormat) -> Result<Self> {
        super::init();
        check_dimensions(width, height)?;
        let mut device = ptr::null_mut();
        // SAFETY: `device` is a live out-pointer; VideoToolbox devices take no
        // device name and no options.
        let status = unsafe {
            sys::av_hwdevice_ctx_create(
                &raw mut device,
                sys::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
                ptr::null(),
                ptr::null_mut(),
                0,
            )
        };
        check(status, "create a VideoToolbox device")?;
        let device = BufferRef::from_new(device, "create a VideoToolbox device")?;
        // SAFETY: `device` is a valid hardware device reference; the new
        // frames context takes its own reference to it.
        let frames = unsafe { sys::av_hwframe_ctx_alloc(device.as_ptr()) };
        let frames = BufferRef::from_new(frames, "allocate a VideoToolbox frame pool")?;
        // SAFETY: the reference returned by av_hwframe_ctx_alloc is valid.
        let data = unsafe { frames.0.as_ref() }.data;
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "FFmpeg allocated this buffer as an AVHWFramesContext, so it is aligned for one"
        )]
        let context = data.cast::<sys::AVHWFramesContext>();
        // SAFETY: a buffer from av_hwframe_ctx_alloc holds an
        // AVHWFramesContext, and until av_hwframe_ctx_init nothing else
        // refers to it.
        let context = unsafe { &mut *context };
        context.format = Self::SURFACE_FORMAT;
        context.sw_format = format.raw();
        context.width = to_int(width, "the width")?;
        context.height = to_int(height, "the height")?;
        // SAFETY: the frames context is fully described and not yet
        // initialised.
        let status = unsafe { sys::av_hwframe_ctx_init(frames.as_ptr()) };
        check(status, "initialise a VideoToolbox frame pool")?;
        Ok(Self {
            frames,
            width,
            height,
            format,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub(super) fn new_frames_reference(&self) -> Result<*mut sys::AVBufferRef> {
        self.frames.new_reference()
    }

    pub fn upload(&self, pixels: &PixelView<'_>) -> Result<GpuSurface> {
        if pixels.format() != self.format
            || pixels.width() != self.width
            || pixels.height() != self.height
        {
            return Err(Error::invalid(
                "the picture does not match the pool's format and size",
            ));
        }
        let source = frame_from_pixels(pixels)?;
        let surface = Frame::new()?;
        // SAFETY: the frames context is initialised and `surface` is a blank
        // frame that receives a pooled pixel buffer.
        let status =
            unsafe { sys::av_hwframe_get_buffer(self.frames.as_ptr(), surface.as_ptr(), 0) };
        check(status, "take a surface from the VideoToolbox pool")?;
        // SAFETY: `surface` is a hardware frame from this pool and `source` is
        // a software frame in the pool's software format and size.
        let status = unsafe { sys::av_hwframe_transfer_data(surface.as_ptr(), source.as_ptr(), 0) };
        check(status, "upload pixels to a VideoToolbox surface")?;
        Ok(GpuSurface {
            frame: surface,
            width: self.width,
            height: self.height,
        })
    }
}

impl std::fmt::Debug for GpuFramePool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GpuFramePool")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

/// One `CVPixelBuffer`, kept alive until the surface and every encoder that
/// still reads it are done.
pub struct GpuSurface {
    frame: Frame,
    width: u32,
    height: u32,
}

impl std::fmt::Debug for GpuSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GpuSurface")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl GpuSurface {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub(super) fn new_frame_reference(&self) -> Result<Frame> {
        let reference = Frame::new()?;
        // SAFETY: `reference` is blank and `self.frame` is reference-counted,
        // so av_frame_ref shares the pixel buffer without copying it.
        let status = unsafe { sys::av_frame_ref(reference.as_ptr(), self.frame.as_ptr()) };
        check(status, "reference a GPU surface")?;
        Ok(reference)
    }

    /// Shares a captured frame's pixel buffer with the encoder without
    /// copying it. The surface keeps the pixel buffer alive on its own, so
    /// the captured frame can be dropped.
    pub fn from_capture(surface: &CoreVideoSurface) -> Result<Self> {
        Self::holding(RetainedPixelBuffer::of_capture(surface)?)
    }

    fn holding(buffer: RetainedPixelBuffer) -> Result<Self> {
        let (width, height) = buffer.size()?;
        let mut frame = blank_surface_frame(width, height)?;
        // SAFETY: av_buffer_create only records the pointer; with a size of
        // zero FFmpeg never reads through it, and `release_pixel_buffer`
        // gives back the one retain that `buffer` owns.
        let holder = unsafe {
            sys::av_buffer_create(
                buffer.as_ptr().cast(),
                0,
                Some(release_pixel_buffer),
                ptr::null_mut(),
                0,
            )
        };
        if holder.is_null() {
            return Err(out_of_memory("wrap a pixel buffer"));
        }
        let pixel_buffer = buffer.into_the_holders_keeping();
        let raw = frame.raw_mut();
        raw.buf[0] = holder;
        // An AV_PIX_FMT_VIDEOTOOLBOX frame carries its CVPixelBufferRef in
        // data[3]: https://ffmpeg.org/doxygen/trunk/pixfmt_8h.html
        raw.data[3] = pixel_buffer.as_ptr().cast();
        Ok(Self {
            frame,
            width,
            height,
        })
    }
}

struct RetainedPixelBuffer(NonNull<c_void>);

impl RetainedPixelBuffer {
    fn of_capture(surface: &CoreVideoSurface) -> Result<Self> {
        let buffer = NonNull::new(surface.pixel_buffer_ptr())
            .ok_or_else(|| Error::invalid("the captured frame has no pixel buffer"))?;
        // SAFETY: a CoreVideoSurface holds a valid CVPixelBufferRef for as
        // long as it lives, and it is borrowed for this call; CFRetain adds
        // the reference that the returned value owns and releases on drop.
        let _same_pointer = unsafe { CFRetain(buffer.as_ptr()) };
        Ok(Self(buffer))
    }

    #[cfg(test)]
    fn of_pooled(surface: &GpuSurface) -> Option<Self> {
        let buffer = NonNull::new(surface.frame.raw().data[3].cast::<c_void>())?;
        // SAFETY: data[3] of a VideoToolbox frame from a GpuFramePool is its
        // live CVPixelBufferRef, kept alive by the borrowed surface; CFRetain
        // adds the reference that the returned value owns.
        let _same_pointer = unsafe { CFRetain(buffer.as_ptr()) };
        Some(Self(buffer))
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    fn size(&self) -> Result<(u32, u32)> {
        // SAFETY: this value owns a retain on a valid CVPixelBufferRef.
        let width = unsafe { CVPixelBufferGetWidth(self.as_ptr()) };
        // SAFETY: as above.
        let height = unsafe { CVPixelBufferGetHeight(self.as_ptr()) };
        match (u32::try_from(width), u32::try_from(height)) {
            (Ok(width), Ok(height)) => Ok((width, height)),
            _ => Err(Error::invalid("the pixel buffer has an impossible size")),
        }
    }

    fn into_the_holders_keeping(self) -> NonNull<c_void> {
        std::mem::ManuallyDrop::new(self).0
    }
}

impl Drop for RetainedPixelBuffer {
    fn drop(&mut self) {
        // SAFETY: this value owns one retain on a valid CVPixelBufferRef and
        // gives it back exactly once.
        unsafe { CFRelease(self.as_ptr()) }
    }
}

fn blank_surface_frame(width: u32, height: u32) -> Result<Frame> {
    check_dimensions(width, height)?;
    let mut frame = Frame::new()?;
    let raw = frame.raw_mut();
    raw.format = GpuFramePool::SURFACE_FORMAT.0;
    raw.width = to_int(width, "the width")?;
    raw.height = to_int(height, "the height")?;
    Ok(frame)
}

unsafe extern "C" fn release_pixel_buffer(_opaque: *mut c_void, data: *mut u8) {
    // SAFETY: FFmpeg calls this once, when the last reference to the buffer
    // created in `GpuSurface::holding` goes away, with the pixel buffer
    // pointer that function stored; releasing it gives back the retain that
    // the RetainedPixelBuffer handed to the holder.
    unsafe { CFRelease(data.cast_const().cast()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::{VideoEncoder, VideoEncoderSettings, VideoInput, VideoSource};
    use crate::types::{PlaneView, Rational};

    fn settings(encoder: &str) -> VideoEncoderSettings {
        VideoEncoderSettings {
            encoder: encoder.to_owned(),
            width: 640,
            height: 360,
            time_base: Rational::new(1, 30),
            frame_rate: Rational::new(30, 1),
            bitrate: 2_000_000,
            max_bitrate: 3_000_000,
            keyframe_interval: 30,
        }
    }

    #[test]
    fn a_pixel_buffer_from_outside_the_pool_encodes_without_a_copy() {
        let Ok(pool) = GpuFramePool::new(640, 360, PixelFormat::Nv12) else {
            eprintln!("skipped: this machine has no VideoToolbox device");
            return;
        };
        let Ok(mut encoder) =
            VideoEncoder::open(&settings("h264_videotoolbox"), VideoSource::Gpu(&pool))
        else {
            eprintln!("skipped: this machine has no VideoToolbox H.264 encoder");
            return;
        };
        let luma = vec![90_u8; 640 * 360];
        let chroma = vec![128_u8; 640 * 180];
        let pixels = PixelView::nv12(
            640,
            360,
            PlaneView {
                data: &luma,
                stride: 640,
            },
            PlaneView {
                data: &chroma,
                stride: 640,
            },
        )
        .unwrap();

        let mut packets = Vec::new();
        for pts in 0..10 {
            let pooled = pool.upload(&pixels).unwrap();
            let retained = RetainedPixelBuffer::of_pooled(&pooled).unwrap();
            drop(pooled);
            let foreign = GpuSurface::holding(retained).unwrap();
            assert_eq!((foreign.width(), foreign.height()), (640, 360));
            packets.extend(encoder.encode(VideoInput::Gpu(&foreign), pts).unwrap());
        }
        packets.extend(encoder.finish().unwrap());

        assert_eq!(packets.len(), 10);
        assert!(packets[0].keyframe);
        assert_ne!(encoder.stream_info().parameters.extradata(), [0_u8; 0]);
    }
}
