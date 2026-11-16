use std::ptr::NonNull;

use super::{Frame, check, out_of_memory, sys};
use crate::error::Result;

pub(super) struct Scaler(NonNull<sys::SwsContext>);

// SAFETY: a scaling context has no thread affinity and is used through
// `&mut self` only.
unsafe impl Send for Scaler {}

impl Scaler {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: sws_alloc_context has no preconditions.
        NonNull::new(unsafe { sys::sws_alloc_context() })
            .map(Self)
            .ok_or_else(|| out_of_memory("allocate a pixel converter"))
    }

    pub(super) fn convert(&mut self, source: &Frame, format: sys::AVPixelFormat) -> Result<Frame> {
        let mut target = Frame::new()?;
        let raw = target.raw_mut();
        raw.format = format.0;
        raw.width = source.raw().width;
        raw.height = source.raw().height;
        raw.pts = source.raw().pts;
        tag_sdr_colors(&mut target);
        // SAFETY: the context is valid, the source is a software frame with
        // data, and the target has its format and size set and no buffers, so
        // swscale allocates them and configures itself from the two frames.
        let status =
            unsafe { sys::sws_scale_frame(self.0.as_ptr(), target.as_ptr(), source.as_ptr()) };
        check(status, "convert pixels")?;
        Ok(target)
    }
}

pub(super) fn tag_sdr_colors(frame: &mut Frame) {
    let is_rgb = frame.raw().format == sys::AV_PIX_FMT_BGRA.0;
    let raw = frame.raw_mut();
    if is_rgb {
        raw.colorspace = sys::AVCOL_SPC_RGB;
        raw.color_range = sys::AVCOL_RANGE_JPEG;
    } else {
        raw.colorspace = sys::AVCOL_SPC_BT709;
        raw.color_range = sys::AVCOL_RANGE_MPEG;
    }
}

impl Drop for Scaler {
    fn drop(&mut self) {
        let mut context = self.0.as_ptr();
        // SAFETY: allocated by sws_alloc_context and not used again.
        unsafe { sys::sws_free_context(&raw mut context) }
    }
}
