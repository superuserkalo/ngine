use std::sync::Arc;

use crate::error::CaptureError;
use crate::target::Region;
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

    /// A copy of `region` of these pixels. `None` when the region does not
    /// lie wholly inside them. For [`PixelFormat::Nv12`] the region shrinks
    /// to even pixels first, as its half-size chroma needs.
    pub fn cropped(&self, region: Region) -> Option<Self> {
        if region.within(self.size)? != region {
            return None;
        }
        let (region, bytes_per_pixel) = match self.format {
            PixelFormat::Nv12 => (region.on_even_pixels()?, 1),
            PixelFormat::Bgra8 | PixelFormat::Rgba8 => (region, 4),
        };
        let left = usize::try_from(region.x)
            .ok()?
            .checked_mul(bytes_per_pixel)?;
        let row_len = usize::try_from(region.width)
            .ok()?
            .checked_mul(bytes_per_pixel)?;
        let top = usize::try_from(region.y).ok()?;
        let rows = usize::try_from(region.height).ok()?;
        let mut data = Vec::with_capacity(row_len.checked_mul(rows)?);
        self.copy_rows(&mut data, 0, top..top + rows, left, row_len)?;
        if self.format == PixelFormat::Nv12 {
            let chroma = self
                .stride
                .checked_mul(usize::try_from(self.size.height).ok()?)?;
            let bottom = top + rows;
            self.copy_rows(&mut data, chroma, top / 2..bottom / 2, left, row_len)?;
        }
        Self::new(region.size(), self.format, row_len, data)
    }

    fn copy_rows(
        &self,
        into: &mut Vec<u8>,
        plane: usize,
        rows: std::ops::Range<usize>,
        left: usize,
        row_len: usize,
    ) -> Option<()> {
        for row in rows {
            let start = plane
                .checked_add(row.checked_mul(self.stride)?)?
                .checked_add(left)?;
            into.extend_from_slice(self.data.get(start..start.checked_add(row_len)?)?);
        }
        Some(())
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
    crop: Option<Region>,
}

impl VideoFrame {
    pub fn from_cpu(timestamp: Timestamp, pixels: Arc<CpuFrame>) -> Self {
        Self {
            timestamp,
            size: pixels.size(),
            format: pixels.format(),
            surface: Surface::Cpu(pixels),
            crop: None,
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
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
            crop: None,
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

    /// The part of [`VideoFrame::surface`] this frame shows, when it shows
    /// only part of it. [`VideoFrame::size`] is the size of that part, and
    /// [`VideoFrame::to_cpu`] copies only that part.
    pub fn crop(&self) -> Option<Region> {
        self.crop
    }

    /// The same frame showing only `region` of what it shows now. `None`
    /// when none of the region lies on it. A part of the region outside the
    /// frame is left out, and for [`PixelFormat::Nv12`] the region shrinks to
    /// even pixels.
    #[must_use]
    pub fn cropped(self, region: Region) -> Option<Self> {
        let inside = region.within(self.size)?;
        let shown = self
            .crop
            .unwrap_or(Region::new(0, 0, self.size.width, self.size.height));
        let on_surface = Region::new(
            shown.x.checked_add(inside.x)?,
            shown.y.checked_add(inside.y)?,
            inside.width,
            inside.height,
        );
        let on_surface = match self.format {
            PixelFormat::Nv12 => on_surface.on_even_pixels()?,
            PixelFormat::Bgra8 | PixelFormat::Rgba8 => on_surface,
        };
        Some(Self {
            size: on_surface.size(),
            crop: Some(on_surface),
            ..self
        })
    }

    /// Copies the pixels into ordinary memory. This reads the surface back
    /// from the GPU, so call it for the frames that need it and not for every
    /// frame. The copy can be in a different format from the surface.
    pub fn to_cpu(&self) -> Result<Arc<CpuFrame>, CaptureError> {
        let whole = match &self.surface {
            #[cfg(target_os = "macos")]
            Surface::CoreVideo(surface) => surface.to_cpu().map(Arc::new),
            #[cfg(target_os = "windows")]
            Surface::D3d11(surface) => surface.to_cpu().map(Arc::new),
            #[cfg(target_os = "linux")]
            Surface::DmaBuf(surface) => surface.to_cpu().map(Arc::new),
            Surface::Cpu(pixels) => Ok(Arc::clone(pixels)),
        }?;
        match self.crop {
            None => Ok(whole),
            Some(region) => whole.cropped(region).map(Arc::new).ok_or_else(|| {
                CaptureError::Os("the frame read back smaller than the part to keep".to_owned())
            }),
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

    fn numbered(size: Size) -> CpuFrame {
        let stride = size.width as usize * 4;
        let data = (0..size.height)
            .flat_map(|y| {
                (0..size.width)
                    .flat_map(move |x| [u8::try_from(x).unwrap(), u8::try_from(y).unwrap(), 0, 255])
            })
            .collect();
        CpuFrame::new(size, PixelFormat::Bgra8, stride, data).unwrap()
    }

    fn pixel(frame: &CpuFrame, x: usize, y: usize) -> &[u8] {
        let start = y * frame.stride() + x * 4;
        &frame.data()[start..start + 4]
    }

    #[test]
    fn a_cropped_bgra_frame_holds_the_pixels_of_its_region() {
        let frame = numbered(Size::new(8, 6));

        let cropped = frame.cropped(Region::new(3, 2, 4, 3)).unwrap();

        assert_eq!(cropped.size(), Size::new(4, 3));
        assert_eq!(cropped.stride(), 16);
        assert_eq!(pixel(&cropped, 0, 0), [3, 2, 0, 255]);
        assert_eq!(pixel(&cropped, 3, 2), [6, 4, 0, 255]);
    }

    #[test]
    fn a_crop_that_reaches_past_the_pixels_is_refused() {
        let frame = numbered(Size::new(8, 6));

        assert!(frame.cropped(Region::new(6, 0, 4, 2)).is_none());
    }

    #[test]
    fn a_cropped_nv12_frame_keeps_its_chroma_rows_with_its_luma_rows() {
        let size = Size::new(8, 4);
        let luma: Vec<u8> = (0..32).collect();
        let chroma: Vec<u8> = (100..116).collect();
        let frame = CpuFrame::new(size, PixelFormat::Nv12, 8, [luma, chroma].concat()).unwrap();

        let cropped = frame.cropped(Region::new(2, 2, 4, 2)).unwrap();

        assert_eq!(cropped.size(), Size::new(4, 2));
        assert_eq!(
            cropped.data(),
            [18, 19, 20, 21, 26, 27, 28, 29, 110, 111, 112, 113]
        );
    }

    #[test]
    fn a_cropped_video_frame_has_the_size_and_pixels_of_its_region() {
        let pixels = Arc::new(numbered(Size::new(16, 10)));
        let frame = VideoFrame::from_cpu(Timestamp::from_nanos(1), pixels);

        let cropped = frame.cropped(Region::new(4, 2, 6, 6)).unwrap();
        let twice = cropped
            .clone()
            .cropped(Region::new(2, 1, 100, 100))
            .unwrap();

        assert_eq!(cropped.size(), Size::new(6, 6));
        assert_eq!(cropped.crop(), Some(Region::new(4, 2, 6, 6)));
        assert_eq!(pixel(&cropped.to_cpu().unwrap(), 0, 0), [4, 2, 0, 255]);
        assert_eq!(twice.crop(), Some(Region::new(6, 3, 4, 5)));
        assert_eq!(pixel(&twice.to_cpu().unwrap(), 0, 0), [6, 3, 0, 255]);
    }

    #[test]
    fn a_region_off_the_frame_crops_to_nothing() {
        let pixels = Arc::new(numbered(Size::new(16, 10)));
        let frame = VideoFrame::from_cpu(Timestamp::from_nanos(1), pixels);

        assert!(frame.cropped(Region::new(16, 0, 4, 4)).is_none());
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
