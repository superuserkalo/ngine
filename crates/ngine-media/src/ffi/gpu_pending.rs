use super::hw::BufferRef;
use super::{Frame, sys};
use crate::error::Result;
use crate::types::PixelFormat;

#[derive(Debug)]
pub enum GpuFramePool {}

#[derive(Debug)]
pub enum GpuSurface {}

impl GpuFramePool {
    pub(super) fn surface_format(&self) -> sys::AVPixelFormat {
        match *self {}
    }

    pub(super) fn color_matrix(&self) -> sys::AVColorSpace {
        match *self {}
    }

    pub fn width(&self) -> u32 {
        match *self {}
    }

    pub fn height(&self) -> u32 {
        match *self {}
    }

    pub fn format(&self) -> PixelFormat {
        match *self {}
    }

    pub(super) fn frames(&self) -> &BufferRef {
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
