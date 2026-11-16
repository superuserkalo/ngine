use super::{Frame, sys};
use crate::error::Result;
use crate::types::PixelFormat;

#[derive(Debug)]
pub enum GpuFramePool {}

#[derive(Debug)]
pub enum GpuSurface {}

impl GpuFramePool {
    pub(super) const SURFACE_FORMAT: sys::AVPixelFormat = sys::AV_PIX_FMT_NONE;

    pub fn width(&self) -> u32 {
        match *self {}
    }

    pub fn height(&self) -> u32 {
        match *self {}
    }

    pub fn format(&self) -> PixelFormat {
        match *self {}
    }

    pub(super) fn new_frames_reference(&self) -> Result<*mut sys::AVBufferRef> {
        match *self {}
    }
}

impl GpuSurface {
    pub fn width(&self) -> u32 {
        match *self {}
    }

    pub fn height(&self) -> u32 {
        match *self {}
    }

    pub(super) fn new_frame_reference(&self) -> Result<Frame> {
        match *self {}
    }
}
