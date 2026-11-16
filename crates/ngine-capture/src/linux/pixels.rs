use crate::error::CaptureError;
use crate::frame::{CpuFrame, PixelFormat, Size};

const BYTES_PER_PIXEL: usize = 4;
const OPAQUE: u8 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Layout {
    Bgrx,
    Bgra,
    Rgbx,
    Rgba,
    Nv12,
}

impl Layout {
    pub(super) fn pixel_format(self) -> PixelFormat {
        match self {
            Self::Bgrx | Self::Bgra => PixelFormat::Bgra8,
            Self::Rgbx | Self::Rgba => PixelFormat::Rgba8,
            Self::Nv12 => PixelFormat::Nv12,
        }
    }
}

pub(super) fn preference(wanted: PixelFormat) -> Vec<Layout> {
    let blue_first = [Layout::Bgrx, Layout::Bgra];
    let red_first = [Layout::Rgbx, Layout::Rgba];
    match wanted {
        PixelFormat::Rgba8 => [red_first, blue_first].concat(),
        PixelFormat::Nv12 => [&[Layout::Nv12][..], &blue_first, &red_first].concat(),
        _ => [blue_first, red_first].concat(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Chunk {
    pub(super) offset: usize,
    pub(super) size: usize,
    pub(super) stride: i32,
}

pub(super) fn frame_from_mapped(
    layout: Layout,
    size: Size,
    chunk: Chunk,
    mapped: &[u8],
) -> Option<CpuFrame> {
    let width = usize::try_from(size.width).ok()?;
    let height = usize::try_from(size.height).ok()?;
    let (narrowest_stride, rows) = match layout {
        Layout::Nv12 => (width + width % 2, height + height.div_ceil(2)),
        _ => (width.checked_mul(BYTES_PER_PIXEL)?, height),
    };
    let stride = usize::try_from(chunk.stride)
        .ok()
        .filter(|stride| *stride > 0)
        .unwrap_or(narrowest_stride);
    let filled = if chunk.size == 0 {
        mapped.len()
    } else {
        chunk.offset.checked_add(chunk.size)?.min(mapped.len())
    };
    let wanted = chunk.offset.checked_add(stride.checked_mul(rows)?)?;
    let pixels = mapped.get(chunk.offset..wanted.min(filled))?;
    CpuFrame::new(size, layout.pixel_format(), stride, pixels.to_vec())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct XImageLayout {
    pub(super) bits_per_pixel: u8,
    pub(super) most_significant_byte_first: bool,
    pub(super) red_mask: u32,
    pub(super) blue_mask: u32,
}

pub(super) fn packed_from_ximage(
    layout: XImageLayout,
    size: Size,
    mut data: Vec<u8>,
) -> Result<(PixelFormat, Vec<u8>), CaptureError> {
    let format = match (layout.bits_per_pixel, layout.red_mask, layout.blue_mask) {
        (32, 0x00ff_0000, 0x0000_00ff) => PixelFormat::Bgra8,
        (32, 0x0000_00ff, 0x00ff_0000) => PixelFormat::Rgba8,
        (bits, red, _) => {
            return Err(CaptureError::Unsupported(format!(
                "an X11 screen with {bits} bits per pixel and red mask {red:#x}"
            )));
        }
    };
    let needed = usize::try_from(u64::from(size.width) * u64::from(size.height))
        .ok()
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
        .filter(|needed| *needed <= data.len())
        .ok_or_else(|| CaptureError::Os("the X server sent a short image".to_owned()))?;
    data.truncate(needed);
    for pixel in data.as_chunks_mut::<BYTES_PER_PIXEL>().0 {
        if layout.most_significant_byte_first {
            pixel.reverse();
        }
        pixel[3] = OPAQUE;
    }
    Ok((format, data))
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Cursor<'a> {
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) width: u16,
    pub(super) premultiplied_argb: &'a [u32],
}

fn over(cursor: u8, alpha: u8, below: u8) -> u8 {
    let kept = u16::from(below) * u16::from(OPAQUE - alpha) / u16::from(OPAQUE);
    u8::try_from(u16::from(cursor) + kept).unwrap_or(OPAQUE)
}

pub(super) fn draw_cursor(pixels: &mut [u8], size: Size, format: PixelFormat, cursor: &Cursor<'_>) {
    let red_index = match format {
        PixelFormat::Bgra8 => 2,
        PixelFormat::Rgba8 => 0,
        _ => return,
    };
    let Ok(frame_width) = usize::try_from(size.width) else {
        return;
    };
    let cursor_width = usize::from(cursor.width.max(1));
    for (index, argb) in cursor.premultiplied_argb.iter().enumerate() {
        let column = i32::try_from(index % cursor_width).ok();
        let row = i32::try_from(index / cursor_width).ok();
        let x = column.and_then(|column| usize::try_from(cursor.left.checked_add(column)?).ok());
        let y = row.and_then(|row| usize::try_from(cursor.top.checked_add(row)?).ok());
        let (Some(x), Some(y)) = (x, y) else {
            continue;
        };
        if x >= frame_width {
            continue;
        }
        let start = (y * frame_width + x) * BYTES_PER_PIXEL;
        let Some(pixel) = pixels.get_mut(start..start + BYTES_PER_PIXEL) else {
            continue;
        };
        let [alpha, red, green, blue] = argb.to_be_bytes();
        pixel[red_index] = over(red, alpha, pixel[red_index]);
        pixel[1] = over(green, alpha, pixel[1]);
        pixel[2 - red_index] = over(blue, alpha, pixel[2 - red_index]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP: XImageLayout = XImageLayout {
        bits_per_pixel: 32,
        most_significant_byte_first: false,
        red_mask: 0x00ff_0000,
        blue_mask: 0x0000_00ff,
    };

    #[test]
    fn the_asked_format_is_offered_first_and_the_others_stay_as_fallbacks() {
        assert_eq!(
            preference(PixelFormat::Bgra8),
            [Layout::Bgrx, Layout::Bgra, Layout::Rgbx, Layout::Rgba]
        );
        assert_eq!(preference(PixelFormat::Rgba8)[0], Layout::Rgbx);
        assert_eq!(preference(PixelFormat::Nv12)[0], Layout::Nv12);
        assert_eq!(preference(PixelFormat::Nv12).len(), 5);
        assert_eq!(Layout::Bgrx.pixel_format(), PixelFormat::Bgra8);
        assert_eq!(Layout::Rgba.pixel_format(), PixelFormat::Rgba8);
        assert_eq!(Layout::Nv12.pixel_format(), PixelFormat::Nv12);
    }

    #[test]
    fn a_mapped_buffer_is_copied_from_its_chunk_with_its_stride() {
        let mut mapped = vec![0_u8; 4 + 12 * 2 + 8];
        mapped[4..28].fill(7);
        let chunk = Chunk {
            offset: 4,
            size: 24,
            stride: 12,
        };

        let frame = frame_from_mapped(Layout::Bgrx, Size::new(2, 2), chunk, &mapped).unwrap();

        assert_eq!(frame.stride(), 12);
        assert_eq!(frame.format(), PixelFormat::Bgra8);
        assert_eq!(frame.data(), [7_u8; 24]);
    }

    #[test]
    fn a_chunk_without_a_stride_or_size_uses_the_tightest_row() {
        let chunk = Chunk {
            offset: 0,
            size: 0,
            stride: 0,
        };

        let packed = frame_from_mapped(Layout::Rgba, Size::new(3, 2), chunk, &[1; 40]).unwrap();
        let planar = frame_from_mapped(Layout::Nv12, Size::new(3, 3), chunk, &[1; 40]).unwrap();

        assert_eq!((packed.stride(), packed.data().len()), (12, 24));
        assert_eq!((planar.stride(), planar.data().len()), (4, 20));
    }

    #[test]
    fn a_buffer_too_short_for_its_size_is_no_frame() {
        let short = Chunk {
            offset: 0,
            size: 12,
            stride: 8,
        };
        let outside = Chunk {
            offset: 64,
            size: 16,
            stride: 8,
        };

        assert!(frame_from_mapped(Layout::Bgra, Size::new(2, 2), short, &[0; 64]).is_none());
        assert!(frame_from_mapped(Layout::Bgra, Size::new(2, 2), outside, &[0; 64]).is_none());
    }

    #[test]
    fn an_x11_image_becomes_opaque_bgra_or_rgba() {
        let image = vec![1, 2, 3, 0, 4, 5, 6, 0, 99];
        let red_low = XImageLayout {
            red_mask: 0x0000_00ff,
            blue_mask: 0x00ff_0000,
            ..DESKTOP
        };

        assert_eq!(
            packed_from_ximage(DESKTOP, Size::new(2, 1), image.clone()).unwrap(),
            (PixelFormat::Bgra8, vec![1, 2, 3, 255, 4, 5, 6, 255])
        );
        assert_eq!(
            packed_from_ximage(red_low, Size::new(2, 1), image)
                .unwrap()
                .0,
            PixelFormat::Rgba8
        );
    }

    #[test]
    fn an_image_from_a_big_endian_server_has_its_bytes_swapped() {
        let big_endian = XImageLayout {
            most_significant_byte_first: true,
            ..DESKTOP
        };

        assert_eq!(
            packed_from_ximage(big_endian, Size::new(1, 1), vec![0, 3, 2, 1]).unwrap(),
            (PixelFormat::Bgra8, vec![1, 2, 3, 255])
        );
    }

    #[test]
    fn an_unknown_or_short_x11_image_is_an_error() {
        let sixteen_bit = XImageLayout {
            bits_per_pixel: 16,
            red_mask: 0xf800,
            blue_mask: 0x001f,
            ..DESKTOP
        };

        assert!(matches!(
            packed_from_ximage(sixteen_bit, Size::new(1, 1), vec![0; 4]),
            Err(CaptureError::Unsupported(_))
        ));
        assert!(matches!(
            packed_from_ximage(DESKTOP, Size::new(2, 2), vec![0; 15]),
            Err(CaptureError::Os(_))
        ));
    }

    #[test]
    fn the_cursor_is_blended_over_the_frame_and_clipped_at_its_edges() {
        let size = Size::new(2, 2);
        let mut pixels = [10, 20, 30, 255].repeat(4);
        let solid_red = 0xffff_0000;
        let half_green = 0x8000_8000;
        let cursor = Cursor {
            left: 1,
            top: 0,
            width: 2,
            premultiplied_argb: &[solid_red, solid_red, half_green, 0],
        };

        draw_cursor(&mut pixels, size, PixelFormat::Bgra8, &cursor);

        assert_eq!(pixels[0..4], [10, 20, 30, 255]);
        assert_eq!(pixels[4..8], [0, 0, 255, 255]);
        assert_eq!(pixels[8..12], [10, 20, 30, 255]);
        assert_eq!(pixels[12..16], [4, 137, 14, 255]);
    }

    #[test]
    fn the_cursor_keeps_its_colours_in_an_rgba_frame_and_skips_other_formats() {
        let mut rgba = vec![0, 0, 0, 255];
        let mut planar = vec![9, 9, 9, 9];
        let cursor = Cursor {
            left: 0,
            top: 0,
            width: 1,
            premultiplied_argb: &[0xff11_2233],
        };
        let off_screen = Cursor { left: -5, ..cursor };

        draw_cursor(&mut rgba, Size::new(1, 1), PixelFormat::Rgba8, &cursor);
        draw_cursor(&mut planar, Size::new(1, 1), PixelFormat::Nv12, &cursor);
        draw_cursor(
            &mut planar,
            Size::new(1, 1),
            PixelFormat::Bgra8,
            &off_screen,
        );

        assert_eq!(rgba, [0x11, 0x22, 0x33, 255]);
        assert_eq!(planar, [9, 9, 9, 9]);
    }
}
