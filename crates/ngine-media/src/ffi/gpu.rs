use super::graph::{Converter, Step};
use super::hw::{BufferRef, FrameShape, HwFrames};
use super::{Frame, check, sys};
use crate::error::{Error, Result};
use crate::types::PixelFormat;

/// Captured GPU surfaces go in, NV12 surfaces that a hardware encoder reads
/// come out, without the pixels passing through ordinary memory.
///
/// On Linux a DMA-BUF is imported into VA-API and converted by its video
/// processor. On Windows a Direct3D 11 texture is converted by the device's
/// video processor into a texture this pool owns, so the captured texture
/// goes back to the capture as soon as [`GpuFramePool::import`] returns.
pub struct GpuFramePool {
    input: HwFrames,
    converter: Converter,
    shape: FrameShape,
    matrix: sys::AVColorSpace,
    #[cfg(target_os = "windows")]
    device_address: usize,
}

impl std::fmt::Debug for GpuFramePool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GpuFramePool")
            .field("width", &self.shape.width)
            .field("height", &self.shape.height)
            .finish_non_exhaustive()
    }
}

impl GpuFramePool {
    fn converting(
        input: HwFrames,
        shape: FrameShape,
        steps: &[Step],
        matrix: sys::AVColorSpace,
    ) -> Result<Self> {
        let converter = Converter::new(&input, shape, steps)?;
        if converter.output_pixel_format() != sys::AV_PIX_FMT_NV12 {
            return Err(Error::unavailable("converting captured surfaces to NV12"));
        }
        Ok(Self {
            input,
            converter,
            shape,
            matrix,
            #[cfg(target_os = "windows")]
            device_address: 0,
        })
    }

    pub fn width(&self) -> u32 {
        self.shape.width
    }

    pub fn height(&self) -> u32 {
        self.shape.height
    }

    /// The layout of the converted surfaces.
    pub fn format(&self) -> PixelFormat {
        PixelFormat::Nv12
    }

    pub(super) fn surface_format(&self) -> sys::AVPixelFormat {
        self.converter.output_surface_format()
    }

    pub(super) fn color_matrix(&self) -> sys::AVColorSpace {
        self.matrix
    }

    pub(super) fn frames(&self) -> &BufferRef {
        self.converter.output_frames()
    }

    fn converted(&mut self, frame: Frame) -> Result<GpuSurface> {
        let frame = self.converter.convert(frame)?;
        Ok(GpuSurface {
            frame,
            width: self.shape.width,
            height: self.shape.height,
        })
    }
}

/// One converted surface, kept alive until the surface and every encoder
/// that still reads it are done.
pub struct GpuSurface {
    frame: Frame,
    width: u32,
    height: u32,
}

impl std::fmt::Debug for GpuSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GpuSurface")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl GpuSurface {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub(super) fn new_frame_reference(&self) -> Result<Frame> {
        let reference = Frame::new()?;
        // SAFETY: `reference` is blank and `self.frame` is reference-counted,
        // so av_frame_ref shares the surface without copying it.
        let status = unsafe { sys::av_frame_ref(reference.as_ptr(), self.frame.as_ptr()) };
        check(status, "reference a GPU surface")?;
        Ok(reference)
    }
}

#[cfg(target_os = "linux")]
mod dma_buf {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    use ngine_capture::DmaBufSurface;

    use super::super::drm::{self, DrmPicture, DrmPlane};
    use super::super::hw::{self, HwFrames};
    use super::super::{graph::Step, sys};
    use super::{GpuFramePool, GpuSurface};
    use crate::error::{Error, Result};

    const IMPORT_AND_CONVERT: [Step; 2] = [
        Step {
            filter: c"hwmap",
            options: c"mode=read:derive_device=vaapi",
        },
        Step {
            filter: c"scale_vaapi",
            options: c"format=nv12:out_color_matrix=bt709:out_range=tv",
        },
    ];

    fn with_picture<T>(
        surface: &DmaBufSurface,
        then: impl FnOnce(&DrmPicture<'_>) -> Result<T>,
    ) -> Result<T> {
        let planes: Vec<DrmPlane<'_>> = surface
            .planes()
            .iter()
            .map(|plane| DrmPlane {
                fd: plane.fd(),
                offset: plane.offset(),
                stride: plane.stride(),
            })
            .collect();
        then(&DrmPicture {
            width: surface.size().width,
            height: surface.size().height,
            fourcc: surface.drm_format(),
            modifier: surface.modifier(),
            planes: &planes,
        })
    }

    impl GpuFramePool {
        /// Prepares to import surfaces like `surface` on the first GPU that
        /// takes them. An error means this computer cannot, and the caller
        /// encodes CPU pixels instead.
        pub fn for_dma_buf(surface: &DmaBufSurface) -> Result<Self> {
            with_picture(surface, |picture| {
                let shape = drm::shape_of(picture)?;
                let mut last = None;
                for node in hw::system_render_nodes() {
                    let Ok(path) = CString::new(node.as_os_str().as_bytes()) else {
                        continue;
                    };
                    let opened =
                        hw::open_device(sys::AV_HWDEVICE_TYPE_DRM, Some(&path), "open the GPU")
                            .and_then(|device| {
                                HwFrames::on(&device, shape, "describe the captured surfaces")
                            })
                            .and_then(|input| {
                                Self::converting(
                                    input,
                                    shape,
                                    &IMPORT_AND_CONVERT,
                                    sys::AVCOL_SPC_BT709,
                                )
                            });
                    match opened {
                        Ok(pool) => return Ok(pool),
                        Err(error) => last = Some(error),
                    }
                }
                Err(hw::no_device(
                    "importing captured surfaces into VA-API",
                    last,
                ))
            })
        }

        /// Imports one captured surface and converts it. The result owns its
        /// own surface, so `surface` can be dropped.
        pub fn import(&mut self, surface: &DmaBufSurface) -> Result<GpuSurface> {
            let frame = with_picture(surface, |picture| {
                let shape = drm::shape_of(picture)?;
                if shape.width != self.shape.width
                    || shape.height != self.shape.height
                    || shape.pixel_format != self.shape.pixel_format
                {
                    return Err(Error::invalid(
                        "the captured surface does not match the pool's format and size",
                    ));
                }
                drm::frame(&self.input, picture)
            })?;
            self.converted(frame)
        }
    }
}

#[cfg(target_os = "windows")]
mod d3d11 {
    use std::ffi::c_void;
    use std::ptr;

    use ngine_capture::{D3d11Surface, Size};
    use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Multithread};
    use windows::core::Interface;

    use super::super::hw::{BufferRef, FrameShape, HwFrames};
    use super::super::{Frame, check, graph::Step, out_of_memory, sys, to_int};
    use super::{GpuFramePool, GpuSurface};
    use crate::error::{Error, Result};

    const CONVERT: [Step; 1] = [Step {
        filter: c"scale_d3d11",
        options: c"format=nv12",
    }];

    #[repr(C)]
    struct DeviceContextStart {
        device: *mut c_void,
    }

    fn protect_from_other_threads(device: &ID3D11Device) {
        if let Ok(multithread) = device.cast::<ID3D11Multithread>() {
            // SAFETY: the interface was just queried from a live device; the
            // call only switches on the lock around its immediate context.
            let _was_protected = unsafe { multithread.SetMultithreadProtected(true) };
        }
    }

    fn device_of(surface: &D3d11Surface) -> Result<BufferRef> {
        super::super::init();
        let borrowed = surface.device_ptr();
        // SAFETY: a D3d11Surface holds a valid ID3D11Device for as long as it
        // lives, and it is borrowed for this call.
        let device = unsafe { ID3D11Device::from_raw_borrowed(&borrowed) }
            .ok_or_else(|| Error::invalid("the captured frame has no device"))?
            .clone();
        protect_from_other_threads(&device);
        // SAFETY: av_hwdevice_ctx_alloc has no preconditions.
        let context = unsafe { sys::av_hwdevice_ctx_alloc(sys::AV_HWDEVICE_TYPE_D3D11VA) };
        let context = BufferRef::from_new(context, "allocate a Direct3D 11 device")?;
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "FFmpeg allocated this buffer as an AVHWDeviceContext, so it is aligned for one"
        )]
        let generic = context.data().cast::<sys::AVHWDeviceContext>();
        // SAFETY: a buffer from av_hwdevice_ctx_alloc holds an
        // AVHWDeviceContext that nothing else refers to before init.
        let start = unsafe { &*generic }.hwctx.cast::<DeviceContextStart>();
        let owned_by_ffmpeg = DeviceContextStart {
            device: device.into_raw(),
        };
        // SAFETY: for a Direct3D 11 device `hwctx` points at an
        // AVD3D11VADeviceContext, whose first field is the ID3D11Device
        // pointer and the only one its user must set; the pointer written
        // owns one reference, which FFmpeg releases when the device context
        // is freed. https://ffmpeg.org/doxygen/trunk/hwcontext__d3d11va_8h.html
        unsafe { ptr::write(start, owned_by_ffmpeg) };
        // SAFETY: the device context has its device and is not initialised.
        let status = unsafe { sys::av_hwdevice_ctx_init(context.as_ptr()) };
        check(status, "use the capture's Direct3D 11 device")?;
        Ok(context)
    }

    unsafe extern "C" fn return_to_the_capture(_opaque: *mut c_void, data: *mut u8) {
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "the pointer came from a Box<D3d11Surface>, so it is aligned for one"
        )]
        let lent = data.cast::<D3d11Surface>();
        // SAFETY: FFmpeg calls this once, when the last reference to the
        // buffer created in `frame` goes away, with the pointer `frame` got
        // from Box::into_raw; dropping the box ends the loan of the texture.
        drop(unsafe { Box::from_raw(lent) });
    }

    fn frame(frames: &HwFrames, surface: &D3d11Surface, shape: FrameShape) -> Result<Frame> {
        let mut frame = Frame::new()?;
        let width = to_int(shape.width, "the width")?;
        let height = to_int(shape.height, "the height")?;
        let frames_reference = frames.reference().new_reference()?;
        frame.raw_mut().hw_frames_ctx = frames_reference;
        let lent = Box::into_raw(Box::new(surface.clone()));
        // SAFETY: `lent` points at a live D3d11Surface; FFmpeg only stores
        // the pointer and calls `return_to_the_capture` with it when the
        // last reference is dropped.
        let holder = unsafe {
            sys::av_buffer_create(
                lent.cast(),
                size_of::<D3d11Surface>(),
                Some(return_to_the_capture),
                ptr::null_mut(),
                sys::AV_BUFFER_FLAG_READONLY.cast_signed(),
            )
        };
        if holder.is_null() {
            // SAFETY: av_buffer_create failed, so nothing else holds `lent`,
            // which came from Box::into_raw just above.
            drop(unsafe { Box::from_raw(lent) });
            return Err(out_of_memory("wrap a Direct3D 11 texture"));
        }
        let slice = usize::try_from(surface.subresource_index())
            .map_err(|_| Error::invalid("the texture slice does not fit a pointer"))?;
        let raw = frame.raw_mut();
        raw.buf[0] = holder;
        raw.data[0] = surface.texture_ptr().cast();
        raw.data[1] = ptr::without_provenance_mut(slice);
        raw.format = sys::AV_PIX_FMT_D3D11.0;
        raw.width = width;
        raw.height = height;
        Ok(frame)
    }

    impl GpuFramePool {
        /// Prepares to convert BGRA textures of `size` on the device that
        /// captured `surface`. An error means this computer cannot, and the
        /// caller encodes CPU pixels instead.
        pub fn for_d3d11(surface: &D3d11Surface, size: Size) -> Result<Self> {
            let shape = FrameShape {
                surface_format: sys::AV_PIX_FMT_D3D11,
                pixel_format: sys::AV_PIX_FMT_BGRA,
                width: size.width,
                height: size.height,
            };
            let device = device_of(surface)?;
            let input = HwFrames::on(&device, shape, "describe the captured textures")?;
            let mut pool = Self::converting(input, shape, &CONVERT, sys::AVCOL_SPC_SMPTE170M)?;
            pool.device_address = surface.device_ptr().addr();
            Ok(pool)
        }

        /// Converts one captured texture into a texture of this pool. The
        /// captured texture is no longer in use when this returns, so
        /// `surface` can be dropped and the capture goes on.
        pub fn import(&mut self, surface: &D3d11Surface) -> Result<GpuSurface> {
            if surface.device_ptr().addr() != self.device_address {
                return Err(Error::invalid(
                    "the captured texture belongs to another device than the pool",
                ));
            }
            let frame = frame(&self.input, surface, self.shape)?;
            self.converted(frame)
        }
    }
}
