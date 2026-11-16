use std::fmt;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::Arc;

use crate::error::CaptureError;
use crate::frame::CpuFrame;

type CpuCopy = dyn Fn() -> Result<CpuFrame, CaptureError> + Send + Sync;

#[derive(Clone, Debug)]
pub struct DmaBufPlane {
    fd: Arc<OwnedFd>,
    offset: u32,
    stride: u32,
}

impl DmaBufPlane {
    /// The descriptor stays open while this plane or a clone of it is alive.
    /// Duplicate it to keep it for longer.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    pub fn offset(&self) -> u32 {
        self.offset
    }

    pub fn stride(&self) -> u32 {
        self.stride
    }
}

#[derive(Clone)]
pub struct DmaBufSurface {
    planes: Vec<DmaBufPlane>,
    drm_format: u32,
    modifier: u64,
    cpu_copy: Arc<CpuCopy>,
}

impl DmaBufSurface {
    pub fn planes(&self) -> &[DmaBufPlane] {
        &self.planes
    }

    /// The DRM four-character code of the pixel layout.
    pub fn drm_format(&self) -> u32 {
        self.drm_format
    }

    pub fn modifier(&self) -> u64 {
        self.modifier
    }

    pub(crate) fn to_cpu(&self) -> Result<CpuFrame, CaptureError> {
        (self.cpu_copy)()
    }
}

impl fmt::Debug for DmaBufSurface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DmaBufSurface")
            .field("planes", &self.planes)
            .field("drm_format", &self.drm_format)
            .field("modifier", &self.modifier)
            .finish_non_exhaustive()
    }
}
