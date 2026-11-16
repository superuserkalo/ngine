use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::Arc;

use crate::error::CaptureError;
use crate::frame::{CpuFrame, Size};

#[derive(Clone, Debug)]
pub struct DmaBufPlane {
    fd: Arc<OwnedFd>,
    offset: u32,
    stride: u32,
}

impl DmaBufPlane {
    pub fn new(fd: OwnedFd, offset: u32, stride: u32) -> Self {
        Self {
            fd: Arc::new(fd),
            offset,
            stride,
        }
    }

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

#[derive(Clone, Debug)]
pub struct DmaBufSurface {
    size: Size,
    drm_format: u32,
    modifier: u64,
    planes: Vec<DmaBufPlane>,
}

impl DmaBufSurface {
    pub fn new(size: Size, drm_format: u32, modifier: u64, planes: Vec<DmaBufPlane>) -> Self {
        Self {
            size,
            drm_format,
            modifier,
            planes,
        }
    }

    pub fn size(&self) -> Size {
        self.size
    }

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
        let _ = self;
        Err(CaptureError::Unsupported(
            "copying a DMA-BUF into memory: it has to be mapped, and this crate has no unsafe code to map it"
                .to_owned(),
        ))
    }
}
