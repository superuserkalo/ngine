use std::ffi::CStr;
use std::ptr::{self, NonNull};

use super::hw::{BufferRef, FrameShape, HwFrames};
use super::{Frame, check, out_of_memory, sys, to_int};
use crate::error::{Error, Result};

const TIME_BASE: sys::AVRational = sys::AVRational {
    num: 1,
    den: 90_000,
};

#[derive(Clone, Copy)]
pub(super) struct Step {
    pub(super) filter: &'static CStr,
    pub(super) options: &'static CStr,
}

struct Graph(NonNull<sys::AVFilterGraph>);

impl Drop for Graph {
    fn drop(&mut self) {
        let mut graph = self.0.as_ptr();
        // SAFETY: allocated by avfilter_graph_alloc and not used again; this
        // also frees every filter that was created in the graph.
        unsafe { sys::avfilter_graph_free(&raw mut graph) }
    }
}

fn filter_named(name: &CStr) -> Result<NonNull<sys::AVFilter>> {
    // SAFETY: `name` is a NUL-terminated string that outlives the call.
    let filter = unsafe { sys::avfilter_get_by_name(name.as_ptr()) };
    NonNull::new(filter.cast_mut())
        .ok_or_else(|| Error::unavailable(format!("the filter {}", name.to_string_lossy())))
}

impl Graph {
    fn new() -> Result<Self> {
        super::init();
        // SAFETY: avfilter_graph_alloc has no preconditions.
        let mut graph = NonNull::new(unsafe { sys::avfilter_graph_alloc() })
            .map(Self)
            .ok_or_else(|| out_of_memory("allocate a filter graph"))?;
        // SAFETY: the graph was just allocated and nothing else refers to it.
        unsafe { graph.0.as_mut() }.nb_threads = 1;
        Ok(graph)
    }

    fn add(
        &mut self,
        filter: &CStr,
        instance: &CStr,
        options: Option<&CStr>,
    ) -> Result<NonNull<sys::AVFilterContext>> {
        let description = filter_named(filter)?;
        let mut context = ptr::null_mut();
        // SAFETY: the graph and the filter description are valid, the strings
        // are NUL-terminated and outlive the call, and `context` is a live
        // out-pointer. The graph owns the filter it creates.
        let status = unsafe {
            sys::avfilter_graph_create_filter(
                &raw mut context,
                description.as_ptr(),
                instance.as_ptr(),
                options.map_or(ptr::null(), CStr::as_ptr),
                ptr::null_mut(),
                self.0.as_ptr(),
            )
        };
        check(status, "create a filter")?;
        NonNull::new(context).ok_or_else(|| out_of_memory("create a filter"))
    }

    fn add_source(
        &mut self,
        frames: &HwFrames,
        shape: FrameShape,
    ) -> Result<NonNull<sys::AVFilterContext>> {
        let description = filter_named(c"buffer")?;
        // SAFETY: the graph and the filter description are valid and the
        // instance name is a NUL-terminated literal. The graph owns the
        // filter it allocates.
        let source = unsafe {
            sys::avfilter_graph_alloc_filter(self.0.as_ptr(), description.as_ptr(), c"in".as_ptr())
        };
        let source =
            NonNull::new(source).ok_or_else(|| out_of_memory("allocate the frame source"))?;
        // SAFETY: av_buffersrc_parameters_alloc has no preconditions.
        let parameters = unsafe { sys::av_buffersrc_parameters_alloc() };
        let Some(mut parameters) = NonNull::new(parameters) else {
            return Err(out_of_memory("describe the frame source"));
        };
        let described = to_int(shape.width, "the width")
            .and_then(|width| Ok((width, to_int(shape.height, "the height")?)))
            .map(|(width, height)| {
                // SAFETY: the parameters were just allocated and are only
                // reachable through this pointer.
                let raw = unsafe { parameters.as_mut() };
                raw.format = shape.surface_format.0;
                raw.time_base = TIME_BASE;
                raw.width = width;
                raw.height = height;
                raw.hw_frames_ctx = frames.reference().as_ptr();
            });
        let status = described.map(|()| {
            // SAFETY: the source is an uninitialised buffer filter and the
            // parameters are valid; FFmpeg copies them and takes its own
            // reference to the frames context.
            unsafe { sys::av_buffersrc_parameters_set(source.as_ptr(), parameters.as_ptr()) }
        });
        // SAFETY: the parameters came from av_buffersrc_parameters_alloc and
        // are not used again; the frames context they pointed at is borrowed
        // and stays with `frames`.
        unsafe { sys::av_free(parameters.as_ptr().cast()) };
        check(status?, "describe the frame source")?;
        // SAFETY: the source has its parameters and is not yet initialised.
        let status = unsafe { sys::avfilter_init_dict(source.as_ptr(), ptr::null_mut()) };
        check(status, "start the frame source")?;
        Ok(source)
    }

    fn link(from: NonNull<sys::AVFilterContext>, to: NonNull<sys::AVFilterContext>) -> Result<()> {
        // SAFETY: both filters belong to this graph and each has one pad on
        // the side that is linked.
        let status = unsafe { sys::avfilter_link(from.as_ptr(), 0, to.as_ptr(), 0) };
        check(status, "link two filters").map(drop)
    }

    fn configure(&mut self) -> Result<()> {
        // SAFETY: the graph is valid and every filter in it is linked.
        let status = unsafe { sys::avfilter_graph_config(self.0.as_ptr(), ptr::null_mut()) };
        check(status, "configure the filter graph").map(drop)
    }
}

pub(super) struct Converter {
    source: NonNull<sys::AVFilterContext>,
    sink: NonNull<sys::AVFilterContext>,
    output: BufferRef,
    _graph: Graph,
}

// SAFETY: a filter graph has no thread affinity; it is used through
// `&mut self` only, and the frames context reference is atomic.
unsafe impl Send for Converter {}

impl Converter {
    pub(super) fn new(input: &HwFrames, shape: FrameShape, steps: &[Step]) -> Result<Self> {
        let mut graph = Graph::new()?;
        let source = graph.add_source(input, shape)?;
        let mut last = source;
        for step in steps {
            let next = graph.add(step.filter, step.filter, Some(step.options))?;
            Graph::link(last, next)?;
            last = next;
        }
        let sink = graph.add(c"buffersink", c"out", None)?;
        Graph::link(last, sink)?;
        graph.configure()?;
        // SAFETY: the sink belongs to a configured graph; the function
        // returns null or a reference the sink keeps owning.
        let frames = unsafe { sys::av_buffersink_get_hw_frames_ctx(sink.as_ptr()) };
        if frames.is_null() {
            return Err(Error::unavailable(
                "the filter chain does not end in hardware frames",
            ));
        }
        // SAFETY: `frames` is a valid reference owned by the sink; av_buffer_ref
        // adds the reference that `output` owns.
        let output = BufferRef::from_new(
            unsafe { sys::av_buffer_ref(frames) },
            "reference the converted frames",
        )?;
        Ok(Self {
            source,
            sink,
            output,
            _graph: graph,
        })
    }

    pub(super) fn output_frames(&self) -> &BufferRef {
        &self.output
    }

    pub(super) fn output_pixel_format(&self) -> sys::AVPixelFormat {
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "FFmpeg allocated this buffer as an AVHWFramesContext, so it is aligned for one"
        )]
        let context = self.output.data().cast::<sys::AVHWFramesContext>();
        // SAFETY: the reference came from a configured sink, so its data is
        // an initialised AVHWFramesContext that no one mutates any more.
        unsafe { &*context }.sw_format
    }

    pub(super) fn output_surface_format(&self) -> sys::AVPixelFormat {
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "FFmpeg allocated this buffer as an AVHWFramesContext, so it is aligned for one"
        )]
        let context = self.output.data().cast::<sys::AVHWFramesContext>();
        // SAFETY: as in `output_pixel_format`.
        unsafe { &*context }.format
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "FFmpeg moves the frame's buffers into the graph, so the caller must not keep the frame"
    )]
    pub(super) fn convert(&mut self, frame: Frame) -> Result<Frame> {
        // SAFETY: the source belongs to the configured graph and the frame is
        // valid; FFmpeg moves the frame's references into the graph and
        // leaves `frame` blank, which its Drop then frees.
        let status =
            unsafe { sys::av_buffersrc_add_frame_flags(self.source.as_ptr(), frame.as_ptr(), 0) };
        check(status, "hand a frame to the GPU converter")?;
        let converted = Frame::new()?;
        // SAFETY: the sink belongs to the configured graph and `converted` is
        // a blank frame.
        let status =
            unsafe { sys::av_buffersink_get_frame(self.sink.as_ptr(), converted.as_ptr()) };
        check(status, "convert a frame on the GPU")?;
        Ok(converted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filter_this_build_lacks_is_reported_by_name() {
        let Err(error) = filter_named(c"no_such_filter") else {
            panic!("FFmpeg has no filter of that name");
        };

        assert_eq!(
            error,
            Error::Unavailable("the filter no_such_filter".to_owned())
        );
        assert!(filter_named(c"buffer").is_ok());
        assert!(filter_named(c"buffersink").is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_chain_converts_hardware_frames_on_the_gpu_and_describes_what_comes_out() {
        use super::super::hw;

        let Ok(device) = hw::open_device(sys::AV_HWDEVICE_TYPE_VIDEOTOOLBOX, None, "open a GPU")
        else {
            eprintln!("skipped: this machine has no VideoToolbox device");
            return;
        };
        let shape = FrameShape {
            surface_format: sys::AV_PIX_FMT_VIDEOTOOLBOX,
            pixel_format: sys::AV_PIX_FMT_NV12,
            width: 640,
            height: 360,
        };
        let frames = HwFrames::on(&device, shape, "describe test frames").unwrap();
        let halve = [Step {
            filter: c"scale_vt",
            options: c"w=320:h=180",
        }];
        let mut converter = Converter::new(&frames, shape, &halve).unwrap();

        assert!(converter.output_surface_format() == sys::AV_PIX_FMT_VIDEOTOOLBOX);
        assert!(converter.output_pixel_format() == sys::AV_PIX_FMT_NV12);
        assert!(!converter.output_frames().as_ptr().is_null());
        for _ in 0..3 {
            let pixels = Frame::video(sys::AV_PIX_FMT_NV12, 640, 360).unwrap();
            let surface = frames.upload(&pixels).unwrap();
            let halved = converter.convert(surface).unwrap();
            let raw = halved.raw();
            assert_eq!((raw.width, raw.height), (320, 180));
            assert_eq!(raw.format, sys::AV_PIX_FMT_VIDEOTOOLBOX.0);
            assert!(!raw.hw_frames_ctx.is_null());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_chain_with_a_filter_of_another_system_fails_before_any_frame() {
        use super::super::hw;

        let Ok(device) = hw::open_device(sys::AV_HWDEVICE_TYPE_VIDEOTOOLBOX, None, "open a GPU")
        else {
            eprintln!("skipped: this machine has no VideoToolbox device");
            return;
        };
        let shape = FrameShape {
            surface_format: sys::AV_PIX_FMT_VIDEOTOOLBOX,
            pixel_format: sys::AV_PIX_FMT_NV12,
            width: 640,
            height: 360,
        };
        let frames = HwFrames::on(&device, shape, "describe test frames").unwrap();
        let of_linux = [Step {
            filter: c"scale_vaapi",
            options: c"format=nv12",
        }];

        assert!(matches!(
            Converter::new(&frames, shape, &of_linux),
            Err(Error::Unavailable(_))
        ));
    }
}
