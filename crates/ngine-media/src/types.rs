use crate::error::{Error, Result};

pub const MAX_DIMENSION: u32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rational {
    pub num: i32,
    pub den: i32,
}

impl Rational {
    pub const MICROS: Self = Self::new(1, 1_000_000);

    pub const fn new(num: i32, den: i32) -> Self {
        Self { num, den }
    }

    pub const fn is_positive(self) -> bool {
        self.num > 0 && self.den > 0
    }

    /// Converts `value` from units of `self` to units of `to`, rounding to the
    /// nearest unit. Returns `None` unless both are positive fractions and the
    /// result fits.
    pub fn rescale(self, value: i64, to: Self) -> Option<i64> {
        if !self.is_positive() || !to.is_positive() {
            return None;
        }
        let numerator = i128::from(value) * i128::from(self.num) * i128::from(to.den);
        let denominator = i128::from(self.den) * i128::from(to.num);
        let half = denominator / 2;
        let rounded = if numerator >= 0 {
            (numerator + half) / denominator
        } else {
            (numerator - half) / denominator
        };
        i64::try_from(rounded).ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

impl VideoCodec {
    pub const ALL: [Self; 3] = [Self::H264, Self::Hevc, Self::Av1];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioCodec {
    Aac,
    Opus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    Video(VideoCodec),
    Audio(AudioCodec),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    Bgra,
    Nv12,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlaneShape {
    pub row_bytes: usize,
    pub rows: usize,
}

impl PixelFormat {
    pub(crate) fn plane_shapes(self, width: u32, height: u32) -> Vec<PlaneShape> {
        let width = width as usize;
        let height = height as usize;
        match self {
            Self::Bgra => vec![PlaneShape {
                row_bytes: width * 4,
                rows: height,
            }],
            Self::Nv12 => vec![
                PlaneShape {
                    row_bytes: width,
                    rows: height,
                },
                PlaneShape {
                    row_bytes: width.div_ceil(2) * 2,
                    rows: height.div_ceil(2),
                },
            ],
        }
    }
}

pub(crate) fn check_dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(Error::invalid(format!(
            "a picture of {width}x{height} is outside 1..={MAX_DIMENSION}"
        )));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct PlaneView<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

/// Borrowed CPU pixels. Construction checks that every plane holds all of its
/// rows, so a view always describes a whole picture.
#[derive(Clone, Debug)]
pub struct PixelView<'a> {
    format: PixelFormat,
    width: u32,
    height: u32,
    planes: Vec<PlaneView<'a>>,
}

impl<'a> PixelView<'a> {
    pub fn bgra(width: u32, height: u32, plane: PlaneView<'a>) -> Result<Self> {
        Self::new(PixelFormat::Bgra, width, height, vec![plane])
    }

    pub fn nv12(
        width: u32,
        height: u32,
        luma: PlaneView<'a>,
        chroma: PlaneView<'a>,
    ) -> Result<Self> {
        Self::new(PixelFormat::Nv12, width, height, vec![luma, chroma])
    }

    fn new(
        format: PixelFormat,
        width: u32,
        height: u32,
        planes: Vec<PlaneView<'a>>,
    ) -> Result<Self> {
        check_dimensions(width, height)?;
        for (plane, shape) in planes.iter().zip(format.plane_shapes(width, height)) {
            let needed = plane.stride * (shape.rows - 1) + shape.row_bytes;
            if plane.stride < shape.row_bytes || plane.data.len() < needed {
                return Err(Error::invalid(format!(
                    "a {format:?} plane of {width}x{height} needs {needed} bytes in rows of at least {} bytes, got {} bytes in rows of {}",
                    shape.row_bytes,
                    plane.data.len(),
                    plane.stride
                )));
            }
        }
        Ok(Self {
            format,
            width,
            height,
            planes,
        })
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn planes(&self) -> &[PlaneView<'a>] {
        &self.planes
    }

    pub(crate) fn rows(&self, plane: usize) -> impl Iterator<Item = &'a [u8]> + use<'a> {
        let shape = self.format.plane_shapes(self.width, self.height)[plane];
        let view = self.planes[plane];
        (0..shape.rows).map(move |row| {
            let start = row * view.stride;
            &view.data[start..start + shape.row_bytes]
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plane {
    pub data: Vec<u8>,
    pub stride: usize,
}

/// Decoded CPU pixels with tightly packed rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoFrame {
    pub format: PixelFormat,
    pub width: u32,
    pub height: u32,
    pub pts: Option<i64>,
    pub planes: Vec<Plane>,
}

impl VideoFrame {
    pub fn view(&self) -> Result<PixelView<'_>> {
        let planes = self
            .planes
            .iter()
            .map(|plane| PlaneView {
                data: &plane.data,
                stride: plane.stride,
            })
            .collect::<Vec<_>>();
        if planes.len() != self.format.plane_shapes(self.width, self.height).len() {
            return Err(Error::invalid("the frame has the wrong number of planes"));
        }
        PixelView::new(self.format, self.width, self.height, planes)
    }
}

/// Decoded sound: interleaved 32-bit float samples.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioFrame {
    pub pts: Option<i64>,
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl AudioFrame {
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }
}

/// One encoded packet as plain data. Timestamps are in the time base of the
/// stream it belongs to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Packet {
    pub stream: usize,
    pub pts: Option<i64>,
    pub dts: Option<i64>,
    pub duration: i64,
    pub keyframe: bool,
    /// The decoder must decode this packet and drop its output.
    pub discard: bool,
    /// Samples the decoder drops from the start of this packet's output.
    pub skip_samples_start: u32,
    /// Samples the decoder drops from the end of this packet's output.
    pub skip_samples_end: u32,
    pub data: Vec<u8>,
}

impl Packet {
    pub fn end(&self) -> Option<i64> {
        self.pts.map(|pts| pts.saturating_add(self.duration))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescale_rounds_to_the_nearest_unit_in_both_directions() {
        let frames = Rational::new(1, 30);
        assert_eq!(frames.rescale(1, Rational::MICROS), Some(33_333));
        assert_eq!(frames.rescale(2, Rational::MICROS), Some(66_667));
        assert_eq!(frames.rescale(-2, Rational::MICROS), Some(-66_667));
        assert_eq!(
            Rational::MICROS.rescale(1_000_000, Rational::new(1, 48_000)),
            Some(48_000)
        );
        assert_eq!(Rational::new(1, 0).rescale(1, Rational::MICROS), None);
    }

    #[test]
    fn a_view_rejects_a_plane_that_is_too_short() {
        let pixels = vec![0_u8; 4 * 4 * 3];
        let plane = PlaneView {
            data: &pixels,
            stride: 16,
        };
        assert!(PixelView::bgra(4, 3, plane).is_ok());
        assert!(PixelView::bgra(4, 4, plane).is_err());
        assert!(PixelView::bgra(0, 4, plane).is_err());
    }

    #[test]
    fn nv12_rows_cover_luma_and_half_height_chroma() {
        let luma = vec![1_u8; 6 * 4];
        let chroma = vec![2_u8; 6 * 2];
        let view = PixelView::nv12(
            6,
            4,
            PlaneView {
                data: &luma,
                stride: 6,
            },
            PlaneView {
                data: &chroma,
                stride: 6,
            },
        )
        .unwrap();
        assert_eq!(view.rows(0).count(), 4);
        assert_eq!(view.rows(1).count(), 2);
        assert!(view.rows(1).all(|row| row == [2; 6]));
    }
}
