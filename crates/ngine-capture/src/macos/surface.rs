use std::ffi::c_void;
use std::fmt;
use std::sync::Arc;

use screencapturekit::cm::{CMSampleBuffer, CMSampleBufferExt};
use screencapturekit::cv::CVPixelBuffer;
use screencapturekit::screenshot_manager::CGImageExt;

use super::convert;
use crate::error::CaptureError;
use crate::frame::{CpuFrame, PixelFormat, Size};

const BGRA_BYTES_PER_PIXEL: usize = 4;

#[derive(Clone)]
pub struct CoreVideoSurface {
    sample: Arc<CMSampleBuffer>,
    pixel_buffer: Arc<CVPixelBuffer>,
}

impl CoreVideoSurface {
    pub(super) fn new(sample: CMSampleBuffer, pixel_buffer: CVPixelBuffer) -> Self {
        Self {
            sample: Arc::new(sample),
            pixel_buffer: Arc::new(pixel_buffer),
        }
    }

    /// The `CVPixelBufferRef`, never null. It stays valid while this surface
    /// or a clone of it is alive. Call `CFRetain` on it to keep it for longer.
    pub fn pixel_buffer_ptr(&self) -> *mut c_void {
        self.pixel_buffer.as_ptr()
    }

    /// The `IOSurfaceRef` behind the pixel buffer, with the same lifetime as
    /// [`CoreVideoSurface::pixel_buffer_ptr`]. `None` when the pixel buffer
    /// has none.
    pub fn io_surface_ptr(&self) -> Option<*mut c_void> {
        self.pixel_buffer
            .io_surface()
            .map(|surface| surface.as_ptr())
            .filter(|pointer| !pointer.is_null())
    }

    pub(crate) fn to_cpu(&self) -> Result<CpuFrame, CaptureError> {
        let image = self.sample.cg_image().map_err(|status| {
            CaptureError::Os(format!(
                "the frame could not be read back (status {status})"
            ))
        })?;
        let pixels = image
            .bgra_data()
            .map_err(|error| convert::error_from(&error))?;
        let size = u32::try_from(image.width())
            .ok()
            .zip(u32::try_from(image.height()).ok())
            .map(|(width, height)| Size::new(width, height));
        size.and_then(|size| {
            let stride = image.width().checked_mul(BGRA_BYTES_PER_PIXEL)?;
            CpuFrame::new(size, PixelFormat::Bgra8, stride, pixels)
        })
        .ok_or_else(|| CaptureError::Os("the frame read back with an impossible size".to_owned()))
    }
}

impl fmt::Debug for CoreVideoSurface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CoreVideoSurface")
            .field("pixel_buffer", &self.pixel_buffer_ptr())
            .finish_non_exhaustive()
    }
}
