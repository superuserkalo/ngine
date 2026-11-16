use std::ffi::CStr;
use std::ptr::{self, NonNull};

use super::{Frame, check, out_of_memory, sys, to_int};
use crate::error::{Error, Result};
use crate::types::check_dimensions;

pub(super) struct BufferRef(NonNull<sys::AVBufferRef>);

// SAFETY: AVBufferRef reference counts are atomic and the hardware device and
// frame contexts held through it are documented as usable from any thread.
unsafe impl Send for BufferRef {}

impl BufferRef {
    pub(super) fn from_new(buffer: *mut sys::AVBufferRef, action: &'static str) -> Result<Self> {
        NonNull::new(buffer)
            .map(Self)
            .ok_or_else(|| out_of_memory(action))
    }

    pub(super) fn as_ptr(&self) -> *mut sys::AVBufferRef {
        self.0.as_ptr()
    }

    pub(super) fn data(&self) -> *mut u8 {
        // SAFETY: the reference is valid for as long as `self` lives.
        unsafe { self.0.as_ref() }.data
    }

    pub(super) fn new_reference(&self) -> Result<*mut sys::AVBufferRef> {
        // SAFETY: the buffer reference is valid; av_buffer_ref adds a
        // reference that the caller hands to an FFmpeg structure to own.
        let reference = unsafe { sys::av_buffer_ref(self.as_ptr()) };
        if reference.is_null() {
            Err(out_of_memory("reference a buffer"))
        } else {
            Ok(reference)
        }
    }

    pub(super) fn share(&self) -> Result<Self> {
        self.new_reference()
            .and_then(|reference| Self::from_new(reference, "reference a buffer"))
    }
}

impl Drop for BufferRef {
    fn drop(&mut self) {
        let mut buffer = self.0.as_ptr();
        // SAFETY: this reference came from FFmpeg and is not used again.
        unsafe { sys::av_buffer_unref(&raw mut buffer) }
    }
}

pub(super) fn open_device(
    kind: sys::AVHWDeviceType,
    path: Option<&CStr>,
    action: &'static str,
) -> Result<BufferRef> {
    super::init();
    let mut device = ptr::null_mut();
    // SAFETY: `device` is a live out-pointer, the path is null or a
    // NUL-terminated string that outlives the call, and no options are
    // passed.
    let status = unsafe {
        sys::av_hwdevice_ctx_create(
            &raw mut device,
            kind,
            path.map_or(ptr::null(), CStr::as_ptr),
            ptr::null_mut(),
            0,
        )
    };
    check(status, action)?;
    BufferRef::from_new(device, action)
}

#[derive(Clone, Copy)]
pub(super) struct FrameShape {
    pub(super) surface_format: sys::AVPixelFormat,
    pub(super) pixel_format: sys::AVPixelFormat,
    pub(super) width: u32,
    pub(super) height: u32,
}

pub(super) struct HwFrames {
    reference: BufferRef,
}

impl HwFrames {
    pub(super) fn on(device: &BufferRef, shape: FrameShape, action: &'static str) -> Result<Self> {
        check_dimensions(shape.width, shape.height)?;
        // SAFETY: `device` is a valid hardware device reference; the new
        // frames context takes its own reference to it.
        let frames = unsafe { sys::av_hwframe_ctx_alloc(device.as_ptr()) };
        let reference = BufferRef::from_new(frames, action)?;
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "FFmpeg allocated this buffer as an AVHWFramesContext, so it is aligned for one"
        )]
        let context = reference.data().cast::<sys::AVHWFramesContext>();
        // SAFETY: a buffer from av_hwframe_ctx_alloc holds an
        // AVHWFramesContext, and until av_hwframe_ctx_init nothing else
        // refers to it.
        let context = unsafe { &mut *context };
        context.format = shape.surface_format;
        context.sw_format = shape.pixel_format;
        context.width = to_int(shape.width, "the width")?;
        context.height = to_int(shape.height, "the height")?;
        // SAFETY: the frames context is fully described and not yet
        // initialised.
        let status = unsafe { sys::av_hwframe_ctx_init(reference.as_ptr()) };
        check(status, action)?;
        Ok(Self { reference })
    }

    pub(super) fn reference(&self) -> &BufferRef {
        &self.reference
    }

    pub(super) fn upload(&self, source: &Frame) -> Result<Frame> {
        let mut surface = Frame::new()?;
        // SAFETY: the frames context is initialised and `surface` is a blank
        // frame that receives a pooled surface.
        let status =
            unsafe { sys::av_hwframe_get_buffer(self.reference.as_ptr(), surface.as_ptr(), 0) };
        check(status, "take a surface from the hardware pool")?;
        // SAFETY: `surface` is a hardware frame from this pool and `source` is
        // a software frame; FFmpeg checks that its format and size fit.
        let status = unsafe { sys::av_hwframe_transfer_data(surface.as_ptr(), source.as_ptr(), 0) };
        check(status, "upload pixels to a hardware surface")?;
        let from = source.raw();
        let (colorspace, color_range) = (from.colorspace, from.color_range);
        let raw = surface.raw_mut();
        raw.colorspace = colorspace;
        raw.color_range = color_range;
        Ok(surface)
    }
}

pub(super) fn render_nodes(
    chosen: Option<std::ffi::OsString>,
    folder: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    if let Some(chosen) = chosen.filter(|chosen| !chosen.is_empty()) {
        return vec![std::path::PathBuf::from(chosen)];
    }
    let mut nodes: Vec<std::path::PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("renderD"))
                .is_some_and(|number| number.parse::<u32>().is_ok())
        })
        .collect();
    nodes.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("renderD"))
            .and_then(|number| number.parse::<u32>().ok())
    });
    nodes
}

pub(super) fn system_render_nodes() -> Vec<std::path::PathBuf> {
    render_nodes(
        std::env::var_os("NGINE_RENDER_NODE"),
        std::path::Path::new("/dev/dri"),
    )
}

pub(super) fn no_device(what: &str, last: Option<Error>) -> Error {
    match last {
        Some(error) => Error::unavailable(format!("{what}: {error}")),
        None => Error::unavailable(format!("{what}: this computer has no GPU render node")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_nodes_are_listed_in_the_order_of_their_numbers() {
        let folder =
            std::env::temp_dir().join(format!("ngine-render-nodes-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        for name in [
            "renderD129",
            "card0",
            "renderD128",
            "renderD1000",
            "renderDx",
        ] {
            std::fs::write(folder.join(name), []).unwrap();
        }

        let names: Vec<String> = render_nodes(None, &folder)
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        std::fs::remove_dir_all(&folder).unwrap();

        assert_eq!(names, ["renderD128", "renderD129", "renderD1000"]);
    }

    #[test]
    fn a_named_render_node_replaces_the_list_and_a_missing_folder_has_none() {
        let nowhere = std::path::Path::new("/nonexistent/ngine/dri");

        assert_eq!(
            render_nodes(None, nowhere),
            Vec::<std::path::PathBuf>::new()
        );
        assert_eq!(
            render_nodes(Some("/dev/dri/renderD130".into()), nowhere),
            [std::path::PathBuf::from("/dev/dri/renderD130")]
        );
        assert_eq!(
            render_nodes(Some("".into()), nowhere),
            Vec::<std::path::PathBuf>::new()
        );
    }

    #[test]
    fn a_device_this_build_does_not_have_is_an_error_and_not_a_crash() {
        let opened = [sys::AV_HWDEVICE_TYPE_VAAPI, sys::AV_HWDEVICE_TYPE_D3D11VA]
            .into_iter()
            .filter(|kind| open_device(*kind, None, "open a test device").is_ok())
            .count();

        assert!(opened <= 1, "no machine has both VA-API and Direct3D 11");
    }
}
