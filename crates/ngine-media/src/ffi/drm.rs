use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
use std::ptr;

use super::hw::{FrameShape, HwFrames};
use super::{Frame, out_of_memory, sys, to_int};
use crate::error::{Error, Result};

const fn fourcc(code: [u8; 4]) -> u32 {
    u32::from_le_bytes(code)
}

const XRGB8888: u32 = fourcc(*b"XR24");
const ARGB8888: u32 = fourcc(*b"AR24");
const XBGR8888: u32 = fourcc(*b"XB24");
const ABGR8888: u32 = fourcc(*b"AB24");

#[derive(Clone, Copy, Debug)]
pub(super) struct DrmPlane<'a> {
    pub(super) fd: BorrowedFd<'a>,
    pub(super) offset: u32,
    pub(super) stride: u32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DrmPicture<'a> {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) fourcc: u32,
    pub(super) modifier: u64,
    pub(super) planes: &'a [DrmPlane<'a>],
}

pub(super) fn pixel_format_of(fourcc: u32) -> Option<sys::AVPixelFormat> {
    match fourcc {
        XRGB8888 => Some(sys::AV_PIX_FMT_BGR0),
        ARGB8888 => Some(sys::AV_PIX_FMT_BGRA),
        XBGR8888 => Some(sys::AV_PIX_FMT_RGB0),
        ABGR8888 => Some(sys::AV_PIX_FMT_RGBA),
        _ => None,
    }
}

pub(super) fn shape_of(picture: &DrmPicture<'_>) -> Result<FrameShape> {
    let pixel_format = pixel_format_of(picture.fourcc).ok_or_else(|| {
        Error::unavailable(format!("importing the DRM format {:#010x}", picture.fourcc))
    })?;
    Ok(FrameShape {
        surface_format: sys::AV_PIX_FMT_DRM_PRIME,
        pixel_format,
        width: picture.width,
        height: picture.height,
    })
}

#[repr(C)]
struct OwnedDescriptor {
    descriptor: sys::AVDRMFrameDescriptor,
    keeps_the_buffer_open: OwnedFd,
}

fn length_of(buffer: OwnedFd) -> (OwnedFd, usize) {
    let mut file = File::from(buffer);
    let length = file
        .seek(SeekFrom::End(0))
        .ok()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0);
    (OwnedFd::from(file), length)
}

fn describe(picture: &DrmPicture<'_>) -> Result<Box<OwnedDescriptor>> {
    let [plane] = picture.planes else {
        return Err(Error::unavailable(format!(
            "importing a DMA-BUF of {} planes; VA-API takes one",
            picture.planes.len()
        )));
    };
    let duplicate = plane
        .fd
        .try_clone_to_owned()
        .map_err(|error| Error::invalid(format!("the DMA-BUF could not be duplicated: {error}")))?;
    let (duplicate, length) = length_of(duplicate);
    // SAFETY: AVDRMFrameDescriptor is a C struct of integers and arrays of
    // integers, for which all zeroes is a valid value.
    let mut descriptor = unsafe { std::mem::zeroed::<sys::AVDRMFrameDescriptor>() };
    descriptor.nb_objects = 1;
    descriptor.objects[0].fd = duplicate.as_raw_fd();
    descriptor.objects[0].size = length;
    descriptor.objects[0].format_modifier = picture.modifier;
    descriptor.nb_layers = 1;
    descriptor.layers[0].format = picture.fourcc;
    descriptor.layers[0].nb_planes = 1;
    descriptor.layers[0].planes[0].object_index = 0;
    descriptor.layers[0].planes[0].offset = isize::try_from(plane.offset)
        .map_err(|_| Error::invalid("the plane offset is too large"))?;
    descriptor.layers[0].planes[0].pitch = isize::try_from(plane.stride)
        .map_err(|_| Error::invalid("the plane stride is too large"))?;
    Ok(Box::new(OwnedDescriptor {
        descriptor,
        keeps_the_buffer_open: duplicate,
    }))
}

unsafe extern "C" fn release_descriptor(_opaque: *mut std::ffi::c_void, data: *mut u8) {
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "the pointer came from a Box<OwnedDescriptor>, so it is aligned for one"
    )]
    let owned = data.cast::<OwnedDescriptor>();
    // SAFETY: FFmpeg calls this once, when the last reference to the buffer
    // created in `frame` goes away, with the pointer that `frame` got from
    // Box::into_raw; rebuilding the box frees the descriptor and closes the
    // duplicated descriptor it owns.
    drop(unsafe { Box::from_raw(owned) });
}

pub(super) fn frame(frames: &HwFrames, picture: &DrmPicture<'_>) -> Result<Frame> {
    let mut frame = Frame::new()?;
    let width = to_int(picture.width, "the width")?;
    let height = to_int(picture.height, "the height")?;
    let frames_reference = frames.reference().new_reference()?;
    frame.raw_mut().hw_frames_ctx = frames_reference;
    let owned = Box::into_raw(describe(picture)?);
    // SAFETY: `owned` points at a live OwnedDescriptor of the size given;
    // FFmpeg only stores the pointer and calls `release_descriptor` with it
    // when the last reference is dropped.
    let holder = unsafe {
        sys::av_buffer_create(
            owned.cast(),
            size_of::<OwnedDescriptor>(),
            Some(release_descriptor),
            ptr::null_mut(),
            sys::AV_BUFFER_FLAG_READONLY.cast_signed(),
        )
    };
    if holder.is_null() {
        // SAFETY: av_buffer_create failed, so nothing else holds `owned`,
        // which came from Box::into_raw just above.
        drop(unsafe { Box::from_raw(owned) });
        return Err(out_of_memory("wrap a DMA-BUF"));
    }
    let raw = frame.raw_mut();
    raw.buf[0] = holder;
    raw.data[0] = owned.cast();
    raw.format = sys::AV_PIX_FMT_DRM_PRIME.0;
    raw.width = width;
    raw.height = height;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd;

    use super::*;

    fn temp_file(name: &str, bytes: usize) -> File {
        let path = std::env::temp_dir().join(format!("ngine-drm-{name}-{}", std::process::id()));
        std::fs::write(&path, vec![0_u8; bytes]).unwrap();
        let file = File::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        file
    }

    #[test]
    fn a_single_plane_becomes_one_object_and_one_layer_with_its_own_descriptor() {
        let buffer = temp_file("describe", 4096);
        let planes = [DrmPlane {
            fd: buffer.as_fd(),
            offset: 16,
            stride: 256,
        }];
        let picture = DrmPicture {
            width: 64,
            height: 4,
            fourcc: XRGB8888,
            modifier: 7,
            planes: &planes,
        };

        let owned = describe(&picture).unwrap();
        let descriptor = &owned.descriptor;

        assert_eq!((descriptor.nb_objects, descriptor.nb_layers), (1, 1));
        assert_eq!(
            descriptor.objects[0].fd,
            owned.keeps_the_buffer_open.as_raw_fd()
        );
        assert_ne!(descriptor.objects[0].fd, buffer.as_raw_fd());
        assert_eq!(descriptor.objects[0].size, 4096);
        assert_eq!(descriptor.objects[0].format_modifier, 7);
        assert_eq!(descriptor.layers[0].format, XRGB8888);
        assert_eq!(descriptor.layers[0].nb_planes, 1);
        assert_eq!(descriptor.layers[0].planes[0].offset, 16);
        assert_eq!(descriptor.layers[0].planes[0].pitch, 256);
        assert_eq!(
            ptr::from_ref(owned.as_ref()).cast::<u8>(),
            ptr::from_ref(descriptor).cast::<u8>()
        );
    }

    #[test]
    fn two_planes_and_unknown_formats_are_refused_so_the_caller_falls_back() {
        let buffer = temp_file("refuse", 16);
        let plane = DrmPlane {
            fd: buffer.as_fd(),
            offset: 0,
            stride: 8,
        };
        let planes = [plane, plane];
        let two_planes = DrmPicture {
            width: 2,
            height: 2,
            fourcc: XRGB8888,
            modifier: 0,
            planes: &planes,
        };
        let nv12 = DrmPicture {
            fourcc: fourcc(*b"NV12"),
            planes: &planes[..1],
            ..two_planes
        };

        assert!(matches!(describe(&two_planes), Err(Error::Unavailable(_))));
        assert!(matches!(shape_of(&nv12), Err(Error::Unavailable(_))));
        assert!(shape_of(&two_planes).unwrap().pixel_format == sys::AV_PIX_FMT_BGR0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_frame_carries_the_descriptor_and_releases_it_with_its_last_reference() {
        use super::super::hw;

        let Ok(device) = hw::open_device(sys::AV_HWDEVICE_TYPE_VIDEOTOOLBOX, None, "open a GPU")
        else {
            eprintln!("skipped: this machine has no VideoToolbox device");
            return;
        };
        let any_frames = HwFrames::on(
            &device,
            FrameShape {
                surface_format: sys::AV_PIX_FMT_VIDEOTOOLBOX,
                pixel_format: sys::AV_PIX_FMT_NV12,
                width: 64,
                height: 4,
            },
            "describe test frames",
        )
        .unwrap();
        let buffer = temp_file("frame", 1024);
        let planes = [DrmPlane {
            fd: buffer.as_fd(),
            offset: 0,
            stride: 256,
        }];
        let picture = DrmPicture {
            width: 64,
            height: 4,
            fourcc: ARGB8888,
            modifier: 0,
            planes: &planes,
        };

        let frame = frame(&any_frames, &picture).unwrap();
        let raw = frame.raw();

        assert_eq!(raw.format, sys::AV_PIX_FMT_DRM_PRIME.0);
        assert_eq!((raw.width, raw.height), (64, 4));
        assert!(!raw.buf[0].is_null() && !raw.hw_frames_ctx.is_null());
        assert!(!raw.data[0].is_null());
        drop(frame);
    }

    #[test]
    fn the_four_packed_rgb_formats_of_a_shared_screen_are_known() {
        assert!(pixel_format_of(XRGB8888) == Some(sys::AV_PIX_FMT_BGR0));
        assert!(pixel_format_of(ARGB8888) == Some(sys::AV_PIX_FMT_BGRA));
        assert!(pixel_format_of(XBGR8888) == Some(sys::AV_PIX_FMT_RGB0));
        assert!(pixel_format_of(ABGR8888) == Some(sys::AV_PIX_FMT_RGBA));
        assert!(pixel_format_of(0).is_none());
    }
}
