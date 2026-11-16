use std::sync::Arc;

use crate::error::CaptureError;
use crate::time::Timestamp;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PixelFormat {
    /// Four bytes per pixel: blue, green, red, alpha. Alpha is not meaningful
    /// for a captured screen.
    Bgra8,
    /// Four bytes per pixel: red, green, blue, alpha.
    Rgba8,
    /// A full-size luma plane, then a half-size plane of interleaved chroma.
    /// Video range, BT.709.
    Nv12,
}

impl PixelFormat {
    fn required_len(self, size: Size, stride: usize) -> Option<usize> {
        let width = usize::try_from(size.width).ok()?;
        let height = usize::try_from(size.height).ok()?;
        let (narrowest_stride, rows) = match self {
            Self::Bgra8 | Self::Rgba8 => (width.checked_mul(4)?, height),
            Self::Nv12 => (
                width.checked_add(width % 2)?,
                height.checked_add(height.div_ceil(2))?,
            ),
        };
        (stride >= narrowest_stride)
            .then(|| stride.checked_mul(rows))
            .flatten()
    }
}

/// Pixels in ordinary memory.
///
/// Rows are `stride` bytes apart. For [`PixelFormat::Nv12`] the chroma rows
/// follow the luma rows with the same stride.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuFrame {
    size: Size,
    format: PixelFormat,
    stride: usize,
    data: Vec<u8>,
}

impl CpuFrame {
    /// Returns `None` when `stride` is narrower than a row or `data` is
    /// shorter than the format, size and stride need.
    pub fn new(size: Size, format: PixelFormat, stride: usize, data: Vec<u8>) -> Option<Self> {
        let required = format.required_len(size, stride)?;
        (data.len() >= required).then_some(Self {
            size,
            format,
            stride,
            data,
        })
    }

    /// Returns `None` when the frame would not fit in memory.
    pub fn solid_bgra(size: Size, bgra: [u8; 4]) -> Option<Self> {
        let stride = usize::try_from(size.width).ok()?.checked_mul(4)?;
        let len = PixelFormat::Bgra8.required_len(size, stride)?;
        let data = bgra.iter().copied().cycle().take(len).collect();
        Self::new(size, PixelFormat::Bgra8, stride, data)
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Surface {
    #[cfg(target_os = "macos")]
    CoreVideo(crate::macos::CoreVideoSurface),
    #[cfg(target_os = "windows")]
    D3d11(crate::windows::D3d11Surface),
    #[cfg(target_os = "linux")]
    DmaBuf(crate::linux::DmaBufSurface),
    Cpu(Arc<CpuFrame>),
}

/// One captured picture.
///
/// Frames arrive when the picture changes, not at a fixed rate, and the size
/// can change from one frame to the next when the screen or window is resized.
#[derive(Clone, Debug)]
pub struct VideoFrame {
    timestamp: Timestamp,
    size: Size,
    format: PixelFormat,
    surface: Surface,
}

impl VideoFrame {
    pub fn from_cpu(timestamp: Timestamp, pixels: Arc<CpuFrame>) -> Self {
        Self {
            timestamp,
            size: pixels.size(),
            format: pixels.format(),
            surface: Surface::Cpu(pixels),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn from_surface(
        timestamp: Timestamp,
        size: Size,
        format: PixelFormat,
        surface: Surface,
    ) -> Self {
        Self {
            timestamp,
            size,
            format,
            surface,
        }
    }

    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    /// Copies the pixels into ordinary memory. This reads the surface back
    /// from the GPU, so call it for the frames that need it and not for every
    /// frame. The copy can be in a different format from the surface.
    pub fn to_cpu(&self) -> Result<Arc<CpuFrame>, CaptureError> {
        match &self.surface {
            #[cfg(target_os = "macos")]
            Surface::CoreVideo(surface) => surface.to_cpu().map(Arc::new),
            #[cfg(target_os = "windows")]
            Surface::D3d11(surface) => surface.to_cpu().map(Arc::new),
            #[cfg(target_os = "linux")]
            Surface::DmaBuf(surface) => surface.to_cpu().map(Arc::new),
            Surface::Cpu(pixels) => Ok(Arc::clone(pixels)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cpu_frame_needs_every_row_it_claims() {
        let size = Size::new(4, 2);

        assert!(CpuFrame::new(size, PixelFormat::Bgra8, 16, vec![0; 32]).is_some());
        assert!(CpuFrame::new(size, PixelFormat::Bgra8, 16, vec![0; 31]).is_none());
        assert!(CpuFrame::new(size, PixelFormat::Bgra8, 15, vec![0; 64]).is_none());
    }

    #[test]
    fn an_nv12_frame_holds_luma_rows_then_half_as_many_chroma_rows() {
        let size = Size::new(3, 3);

        assert!(CpuFrame::new(size, PixelFormat::Nv12, 4, vec![0; 20]).is_some());
        assert!(CpuFrame::new(size, PixelFormat::Nv12, 4, vec![0; 19]).is_none());
        assert!(CpuFrame::new(size, PixelFormat::Nv12, 3, vec![0; 64]).is_none());
    }

    #[test]
    fn a_solid_frame_repeats_its_colour() {
        let frame = CpuFrame::solid_bgra(Size::new(2, 2), [1, 2, 3, 255]).unwrap();

        assert_eq!(frame.stride(), 8);
        assert_eq!(frame.data().len(), 16);
        assert_eq!(frame.data(), [1, 2, 3, 255].repeat(4));
    }

    #[test]
    fn a_cpu_frame_copies_to_cpu_without_a_new_allocation() {
        let pixels = Arc::new(CpuFrame::solid_bgra(Size::new(2, 2), [0, 0, 0, 255]).unwrap());
        let frame = VideoFrame::from_cpu(Timestamp::from_nanos(7), Arc::clone(&pixels));

        assert!(Arc::ptr_eq(&frame.to_cpu().unwrap(), &pixels));
        assert_eq!(frame.size(), Size::new(2, 2));
        assert_eq!(frame.format(), PixelFormat::Bgra8);
    }
}
