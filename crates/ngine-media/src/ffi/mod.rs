#![expect(
    unsafe_code,
    reason = "FFmpeg is a C library, so every call into it is unsafe; this module tree is the only place that makes those calls"
)]

mod decode;
mod demux;
mod encode;
mod hw;
mod mux;
mod resample;
mod scale;

#[cfg(any(target_os = "linux", all(test, unix)))]
mod drm;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod gpu;
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod gpu_pending;
#[cfg(any(target_os = "linux", target_os = "windows", test))]
mod graph;
#[cfg(target_os = "macos")]
mod videotoolbox;

#[expect(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    clippy::all,
    clippy::pedantic,
    clippy::allow_attributes,
    clippy::allow_attributes_without_reason,
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    reason = "bindgen writes this module from the FFmpeg headers at build time and its output follows C naming and bindgen's own style"
)]
mod sys {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

use std::ffi::{CStr, CString, c_char, c_int};
use std::path::Path;
use std::ptr::{self, NonNull};
use std::sync::{Arc, Once};

use crate::error::{Error, Result};
use crate::types::{AudioCodec, Codec, Packet, PixelFormat, PlaneShape, Rational, VideoCodec};

pub use decode::{AudioDecoder, VideoDecoder, VideoDecoderOptions};
pub use demux::Demuxer;
pub use encode::{
    AudioEncoder, AudioEncoderSettings, VideoEncoder, VideoEncoderSettings, VideoInput,
    VideoSource, has_encoder,
};
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use gpu::{GpuFramePool, GpuSurface};
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub use gpu_pending::{GpuFramePool, GpuSurface};
pub use mux::{Mp4Layout, Mp4Writer};
pub use resample::{AudioLayout, Resampler};
#[cfg(target_os = "macos")]
pub use videotoolbox::{GpuFramePool, GpuSurface};

const SKIP_SAMPLES_SIZE: usize = 10;

fn init() {
    static QUIET: Once = Once::new();
    QUIET.call_once(|| {
        // SAFETY: av_log_set_level stores one integer and accepts any value.
        unsafe { sys::av_log_set_level(sys::AV_LOG_ERROR.cast_signed()) }
    });
}

fn failure(code: c_int, action: &'static str) -> Error {
    let mut text = [0 as c_char; 128];
    // SAFETY: the pointer and length describe `text`, which av_strerror fills
    // with a NUL-terminated string no longer than the length it is given.
    let described = unsafe { sys::av_strerror(code, text.as_mut_ptr(), text.len()) };
    let message = if described < 0 {
        String::from("unknown error")
    } else {
        let bytes = text
            .iter()
            .map(|unit| unit.to_ne_bytes()[0])
            .take_while(|&byte| byte != 0)
            .collect::<Vec<u8>>();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    Error::Ffmpeg {
        action,
        code,
        message,
    }
}

fn check(code: c_int, action: &'static str) -> Result<c_int> {
    if code < 0 {
        Err(failure(code, action))
    } else {
        Ok(code)
    }
}

fn out_of_memory(action: &'static str) -> Error {
    Error::Ffmpeg {
        action,
        code: 0,
        message: String::from("out of memory"),
    }
}

fn to_int(value: impl TryInto<c_int>, what: &'static str) -> Result<c_int> {
    value
        .try_into()
        .map_err(|_| Error::invalid(format!("{what} does not fit FFmpeg's int")))
}

fn c_string(text: &str) -> Result<CString> {
    CString::new(text).map_err(|_| Error::invalid(format!("{text:?} contains a NUL byte")))
}

fn c_path(path: &Path) -> Result<CString> {
    let text = path.to_str().ok_or_else(|| {
        Error::invalid(format!(
            "FFmpeg takes paths as UTF-8 and {} is not",
            path.display()
        ))
    })?;
    c_string(text)
}

fn optional_timestamp(value: i64) -> Option<i64> {
    (value != sys::NGINE_NOPTS_VALUE).then_some(value)
}

fn raw_timestamp(value: Option<i64>) -> i64 {
    value.unwrap_or(sys::NGINE_NOPTS_VALUE)
}

impl From<sys::AVRational> for Rational {
    fn from(value: sys::AVRational) -> Self {
        Self::new(value.num, value.den)
    }
}

impl From<Rational> for sys::AVRational {
    fn from(value: Rational) -> Self {
        Self {
            num: value.num,
            den: value.den,
        }
    }
}

impl PixelFormat {
    fn raw(self) -> sys::AVPixelFormat {
        match self {
            Self::Bgra => sys::AV_PIX_FMT_BGRA,
            Self::Nv12 => sys::AV_PIX_FMT_NV12,
        }
    }
}

impl VideoCodec {
    fn raw(self) -> sys::AVCodecID {
        match self {
            Self::H264 => sys::AV_CODEC_ID_H264,
            Self::Hevc => sys::AV_CODEC_ID_HEVC,
            Self::Av1 => sys::AV_CODEC_ID_AV1,
        }
    }
}

impl AudioCodec {
    fn raw(self) -> sys::AVCodecID {
        match self {
            Self::Aac => sys::AV_CODEC_ID_AAC,
            Self::Opus => sys::AV_CODEC_ID_OPUS,
        }
    }
}

fn codec_from_raw(id: sys::AVCodecID) -> Option<Codec> {
    VideoCodec::ALL
        .into_iter()
        .find(|codec| codec.raw() == id)
        .map(Codec::Video)
        .or_else(|| {
            [AudioCodec::Aac, AudioCodec::Opus]
                .into_iter()
                .find(|codec| codec.raw() == id)
                .map(Codec::Audio)
        })
}

#[derive(Clone, Copy)]
struct CodecRef(&'static sys::AVCodec);

impl CodecRef {
    fn from_ptr(codec: *const sys::AVCodec) -> Option<Self> {
        // SAFETY: FFmpeg returns either null or a pointer to one of its
        // registered codec descriptions, which are immutable statics that live
        // as long as the library is loaded.
        unsafe { codec.as_ref() }.map(Self)
    }

    fn encoder(name: &str) -> Option<Self> {
        init();
        let name = CString::new(name).ok()?;
        // SAFETY: `name` is a NUL-terminated string that outlives the call.
        Self::from_ptr(unsafe { sys::avcodec_find_encoder_by_name(name.as_ptr()) })
    }

    fn decoder(id: sys::AVCodecID) -> Option<Self> {
        init();
        // SAFETY: avcodec_find_decoder accepts any codec id.
        Self::from_ptr(unsafe { sys::avcodec_find_decoder(id) })
    }

    fn as_ptr(self) -> *const sys::AVCodec {
        ptr::from_ref(self.0)
    }

    fn name(self) -> String {
        // SAFETY: every registered codec has a static NUL-terminated name.
        unsafe { CStr::from_ptr(self.0.name) }
            .to_string_lossy()
            .into_owned()
    }

    fn is_hardware(self) -> bool {
        self.0.capabilities & sys::AV_CODEC_CAP_HARDWARE.cast_signed() != 0
    }

    fn supported<T: Copy>(self, config: sys::AVCodecConfig) -> Vec<T> {
        let mut list: *const std::ffi::c_void = ptr::null();
        let mut count: c_int = 0;
        // SAFETY: the codec pointer is valid, a null context is allowed, and
        // both out-pointers refer to live locals.
        let status = unsafe {
            sys::avcodec_get_supported_config(
                ptr::null(),
                self.as_ptr(),
                config,
                0,
                &raw mut list,
                &raw mut count,
            )
        };
        let Ok(count) = usize::try_from(count) else {
            return Vec::new();
        };
        if status < 0 || list.is_null() {
            return Vec::new();
        }
        // SAFETY: on success FFmpeg returns `count` elements of the type that
        // belongs to `config`, stored in static memory; the two callers below
        // pair each config with its element type.
        unsafe { std::slice::from_raw_parts(list.cast::<T>(), count) }.to_vec()
    }

    fn pixel_formats(self) -> Vec<sys::AVPixelFormat> {
        self.supported(sys::AV_CODEC_CONFIG_PIX_FORMAT)
    }

    fn sample_formats(self) -> Vec<sys::AVSampleFormat> {
        self.supported(sys::AV_CODEC_CONFIG_SAMPLE_FORMAT)
    }
}

struct Frame(NonNull<sys::AVFrame>);

// SAFETY: an AVFrame and the reference-counted buffers it holds have no thread
// affinity; FFmpeg's buffer reference counts are atomic.
unsafe impl Send for Frame {}

impl Frame {
    fn new() -> Result<Self> {
        // SAFETY: av_frame_alloc has no preconditions.
        NonNull::new(unsafe { sys::av_frame_alloc() })
            .map(Self)
            .ok_or_else(|| out_of_memory("allocate a frame"))
    }

    fn video(format: sys::AVPixelFormat, width: u32, height: u32) -> Result<Self> {
        let mut frame = Self::new()?;
        let raw = frame.raw_mut();
        raw.format = format.0;
        raw.width = to_int(width, "the width")?;
        raw.height = to_int(height, "the height")?;
        frame.allocate_buffers()?;
        Ok(frame)
    }

    fn audio(
        format: sys::AVSampleFormat,
        layout: &ChannelLayout,
        sample_rate: c_int,
        samples: usize,
    ) -> Result<Self> {
        let mut frame = Self::new()?;
        let raw = frame.raw_mut();
        raw.format = format.0;
        raw.sample_rate = sample_rate;
        raw.nb_samples = to_int(samples, "the sample count")?;
        layout.copy_into(&mut raw.ch_layout)?;
        frame.allocate_buffers()?;
        Ok(frame)
    }

    fn allocate_buffers(&mut self) -> Result<()> {
        // SAFETY: the frame is valid and its format and size fields are set by
        // the two constructors above, which is all av_frame_get_buffer needs.
        let status = unsafe { sys::av_frame_get_buffer(self.as_ptr(), 0) };
        check(status, "allocate frame buffers").map(drop)
    }

    fn as_ptr(&self) -> *mut sys::AVFrame {
        self.0.as_ptr()
    }

    fn raw(&self) -> &sys::AVFrame {
        // SAFETY: the pointer came from av_frame_alloc, is freed only in Drop,
        // and FFmpeg keeps no other reference to the frame between calls.
        unsafe { self.0.as_ref() }
    }

    fn raw_mut(&mut self) -> &mut sys::AVFrame {
        // SAFETY: as in `raw`, and `&mut self` makes this the only reference.
        unsafe { self.0.as_mut() }
    }

    fn unref(&mut self) {
        // SAFETY: the frame is valid; av_frame_unref releases its buffers and
        // resets its fields.
        unsafe { sys::av_frame_unref(self.as_ptr()) }
    }

    fn plane_len(&self, index: usize, shape: PlaneShape) -> Option<(usize, usize)> {
        let raw = self.raw();
        let stride = usize::try_from(*raw.linesize.get(index)?).ok()?;
        if raw.data[index].is_null()
            || raw.buf[0].is_null()
            || shape.rows == 0
            || shape.row_bytes > stride
        {
            return None;
        }
        Some((stride, stride * (shape.rows - 1) + shape.row_bytes))
    }

    fn plane(&self, index: usize, shape: PlaneShape) -> Option<(&[u8], usize)> {
        let (stride, len) = self.plane_len(index, shape)?;
        // SAFETY: FFmpeg allocates at least `linesize * height` bytes for each
        // plane of a software frame, callers pass a shape derived from the
        // frame's own format and size, and the data stays alive and unchanged
        // while `self` is borrowed.
        let bytes = unsafe { std::slice::from_raw_parts(self.raw().data[index], len) };
        Some((bytes, stride))
    }

    fn plane_mut(&mut self, index: usize, shape: PlaneShape) -> Option<(&mut [u8], usize)> {
        let (stride, len) = self.plane_len(index, shape)?;
        // SAFETY: as in `plane`; the frame was allocated by one of the
        // constructors above and never shared, so its buffers are writable and
        // `&mut self` makes this the only view of them.
        let bytes = unsafe { std::slice::from_raw_parts_mut(self.raw().data[index], len) };
        Some((bytes, stride))
    }

    fn sample_plane_mut(&mut self, index: usize, len: usize) -> Option<&mut [u8]> {
        let raw = self.raw();
        let capacity = usize::try_from(raw.linesize[0]).ok()?;
        let data = *raw.data.get(index)?;
        if data.is_null() || len > capacity {
            return None;
        }
        // SAFETY: for audio, linesize[0] is the size of every plane buffer,
        // `len` fits in it, and the frame's buffers are owned by `self` alone.
        Some(unsafe { std::slice::from_raw_parts_mut(data, len) })
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        let mut frame = self.0.as_ptr();
        // SAFETY: the frame came from av_frame_alloc and is not used again.
        unsafe { sys::av_frame_free(&raw mut frame) }
    }
}

struct RawPacket(NonNull<sys::AVPacket>);

// SAFETY: an AVPacket and its reference-counted data have no thread affinity.
unsafe impl Send for RawPacket {}

impl RawPacket {
    fn new() -> Result<Self> {
        // SAFETY: av_packet_alloc has no preconditions.
        NonNull::new(unsafe { sys::av_packet_alloc() })
            .map(Self)
            .ok_or_else(|| out_of_memory("allocate a packet"))
    }

    fn as_ptr(&self) -> *mut sys::AVPacket {
        self.0.as_ptr()
    }

    fn raw(&self) -> &sys::AVPacket {
        // SAFETY: the pointer came from av_packet_alloc, is freed only in
        // Drop, and FFmpeg keeps no reference to the packet between calls.
        unsafe { self.0.as_ref() }
    }

    fn raw_mut(&mut self) -> &mut sys::AVPacket {
        // SAFETY: as in `raw`, and `&mut self` makes this the only reference.
        unsafe { self.0.as_mut() }
    }

    fn unref(&mut self) {
        // SAFETY: the packet is valid; av_packet_unref releases its data.
        unsafe { sys::av_packet_unref(self.as_ptr()) }
    }

    fn from_plain(plain: &Packet) -> Result<Self> {
        let mut packet = Self::new()?;
        let size = to_int(plain.data.len(), "the packet size")?;
        // SAFETY: the packet is valid and blank; av_new_packet allocates
        // `size` bytes plus FFmpeg's zeroed padding.
        let status = unsafe { sys::av_new_packet(packet.as_ptr(), size) };
        check(status, "allocate packet data")?;
        if !plain.data.is_empty() {
            // SAFETY: av_new_packet succeeded, so `data` points at
            // `plain.data.len()` writable bytes owned by this packet alone.
            let bytes =
                unsafe { std::slice::from_raw_parts_mut(packet.raw().data, plain.data.len()) };
            bytes.copy_from_slice(&plain.data);
        }
        let mut flags = 0;
        if plain.keyframe {
            flags |= sys::AV_PKT_FLAG_KEY.cast_signed();
        }
        if plain.discard {
            flags |= sys::AV_PKT_FLAG_DISCARD.cast_signed();
        }
        let raw = packet.raw_mut();
        raw.stream_index = to_int(plain.stream, "the stream index")?;
        raw.pts = raw_timestamp(plain.pts);
        raw.dts = raw_timestamp(plain.dts);
        raw.duration = plain.duration;
        raw.flags = flags;
        if plain.skip_samples_start != 0 || plain.skip_samples_end != 0 {
            packet.set_skip_samples(plain.skip_samples_start, plain.skip_samples_end)?;
        }
        Ok(packet)
    }

    fn set_skip_samples(&mut self, start: u32, end: u32) -> Result<()> {
        // SAFETY: the packet is valid; FFmpeg allocates the side data and
        // attaches it to the packet.
        let data = unsafe {
            sys::av_packet_new_side_data(
                self.as_ptr(),
                sys::AV_PKT_DATA_SKIP_SAMPLES,
                SKIP_SAMPLES_SIZE,
            )
        };
        if data.is_null() {
            return Err(out_of_memory("allocate packet side data"));
        }
        // SAFETY: av_packet_new_side_data returned SKIP_SAMPLES_SIZE writable
        // bytes that belong to this packet.
        let bytes = unsafe { std::slice::from_raw_parts_mut(data, SKIP_SAMPLES_SIZE) };
        bytes[..4].copy_from_slice(&start.to_le_bytes());
        bytes[4..8].copy_from_slice(&end.to_le_bytes());
        bytes[8..].fill(0);
        Ok(())
    }

    fn skip_samples(&self) -> (u32, u32) {
        let mut size = 0_usize;
        // SAFETY: the packet is valid and `size` is a live local.
        let data = unsafe {
            sys::av_packet_get_side_data(
                self.as_ptr(),
                sys::AV_PKT_DATA_SKIP_SAMPLES,
                &raw mut size,
            )
        };
        if data.is_null() || size < SKIP_SAMPLES_SIZE {
            return (0, 0);
        }
        // SAFETY: FFmpeg returned `size` readable bytes of side data that live
        // as long as the packet is not modified, which `&self` prevents.
        let bytes = unsafe { std::slice::from_raw_parts(data, size) };
        let word = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        (word(0), word(4))
    }

    fn to_plain(&self) -> Packet {
        let raw = self.raw();
        let data = match usize::try_from(raw.size) {
            Ok(size) if size > 0 && !raw.data.is_null() => {
                // SAFETY: a packet's `data` points at `size` readable bytes
                // that stay alive while the packet is borrowed.
                unsafe { std::slice::from_raw_parts(raw.data, size) }.to_vec()
            }
            _ => Vec::new(),
        };
        let (skip_samples_start, skip_samples_end) = self.skip_samples();
        Packet {
            stream: usize::try_from(raw.stream_index).unwrap_or(0),
            pts: optional_timestamp(raw.pts),
            dts: optional_timestamp(raw.dts),
            duration: raw.duration,
            keyframe: raw.flags & sys::AV_PKT_FLAG_KEY.cast_signed() != 0,
            discard: raw.flags & sys::AV_PKT_FLAG_DISCARD.cast_signed() != 0,
            skip_samples_start,
            skip_samples_end,
            data,
        }
    }
}

impl Drop for RawPacket {
    fn drop(&mut self) {
        let mut packet = self.0.as_ptr();
        // SAFETY: the packet came from av_packet_alloc and is not used again.
        unsafe { sys::av_packet_free(&raw mut packet) }
    }
}

struct ChannelLayout(sys::AVChannelLayout);

// SAFETY: the layouts built here are native-order masks that own no pointers.
unsafe impl Send for ChannelLayout {}

impl ChannelLayout {
    fn default_for(channels: u16) -> Result<Self> {
        if !(1..=8).contains(&channels) {
            return Err(Error::invalid(format!(
                "{channels} channels is outside 1..=8"
            )));
        }
        // SAFETY: an all-zero AVChannelLayout is the documented blank state.
        let mut layout = unsafe { std::mem::zeroed::<sys::AVChannelLayout>() };
        // SAFETY: `layout` is a live, blank layout and the count is positive.
        unsafe { sys::av_channel_layout_default(&raw mut layout, c_int::from(channels)) };
        Ok(Self(layout))
    }

    fn as_ptr(&self) -> *const sys::AVChannelLayout {
        &raw const self.0
    }

    fn copy_into(&self, target: &mut sys::AVChannelLayout) -> Result<()> {
        // SAFETY: both layouts are valid; the target is blank or holds a
        // layout that av_channel_layout_copy frees before overwriting.
        let status = unsafe { sys::av_channel_layout_copy(ptr::from_mut(target), self.as_ptr()) };
        check(status, "copy a channel layout").map(drop)
    }
}

struct OwnedParameters(NonNull<sys::AVCodecParameters>);

// SAFETY: the parameters are plain data plus heap blocks they own, and nothing
// mutates them after construction, so sharing and sending them is sound.
unsafe impl Send for OwnedParameters {}
// SAFETY: as above; shared access is read-only.
unsafe impl Sync for OwnedParameters {}

impl Drop for OwnedParameters {
    fn drop(&mut self) {
        let mut parameters = self.0.as_ptr();
        // SAFETY: allocated by avcodec_parameters_alloc and not used again.
        unsafe { sys::avcodec_parameters_free(&raw mut parameters) }
    }
}

/// What a decoder or a muxer needs to know about one stream, including the
/// codec's global headers. Cloning is cheap.
#[derive(Clone)]
pub struct CodecParameters(Arc<OwnedParameters>);

impl CodecParameters {
    fn allocate() -> Result<OwnedParameters> {
        // SAFETY: avcodec_parameters_alloc has no preconditions.
        NonNull::new(unsafe { sys::avcodec_parameters_alloc() })
            .map(OwnedParameters)
            .ok_or_else(|| out_of_memory("allocate codec parameters"))
    }

    fn from_context(context: &CodecContext) -> Result<Self> {
        let owned = Self::allocate()?;
        // SAFETY: both pointers are valid and the context is open, so its
        // fields, extradata included, are final.
        let status =
            unsafe { sys::avcodec_parameters_from_context(owned.0.as_ptr(), context.as_ptr()) };
        check(status, "read the encoder's parameters")?;
        Ok(Self(Arc::new(owned)))
    }

    fn copy_of(source: &sys::AVCodecParameters) -> Result<Self> {
        let owned = Self::allocate()?;
        // SAFETY: the destination was just allocated and the source is a valid
        // parameter set borrowed for the call.
        let status =
            unsafe { sys::avcodec_parameters_copy(owned.0.as_ptr(), ptr::from_ref(source)) };
        check(status, "copy codec parameters")?;
        Ok(Self(Arc::new(owned)))
    }

    fn raw(&self) -> &sys::AVCodecParameters {
        // SAFETY: the allocation lives as long as the Arc and is never mutated
        // after construction.
        unsafe { self.0.0.as_ref() }
    }

    pub fn codec(&self) -> Option<Codec> {
        codec_from_raw(self.raw().codec_id)
    }

    pub fn is_video(&self) -> bool {
        self.raw().codec_type == sys::AVMEDIA_TYPE_VIDEO
    }

    pub fn is_audio(&self) -> bool {
        self.raw().codec_type == sys::AVMEDIA_TYPE_AUDIO
    }

    pub fn width(&self) -> u32 {
        u32::try_from(self.raw().width).unwrap_or(0)
    }

    pub fn height(&self) -> u32 {
        u32::try_from(self.raw().height).unwrap_or(0)
    }

    pub fn sample_rate(&self) -> u32 {
        u32::try_from(self.raw().sample_rate).unwrap_or(0)
    }

    pub fn channels(&self) -> u16 {
        u16::try_from(self.raw().ch_layout.nb_channels).unwrap_or(0)
    }

    /// Samples per packet, or 0 when the codec's packets vary in length.
    pub fn frame_size(&self) -> u32 {
        u32::try_from(self.raw().frame_size).unwrap_or(0)
    }

    /// Samples the encoder put in front of the sound, which a player skips.
    pub fn initial_padding(&self) -> u32 {
        u32::try_from(self.raw().initial_padding).unwrap_or(0)
    }

    pub fn bit_rate(&self) -> u64 {
        u64::try_from(self.raw().bit_rate).unwrap_or(0)
    }

    /// The codec's global headers, empty when the stream has none.
    pub fn extradata(&self) -> &[u8] {
        let raw = self.raw();
        match usize::try_from(raw.extradata_size) {
            Ok(size) if size > 0 && !raw.extradata.is_null() => {
                // SAFETY: extradata points at extradata_size readable bytes
                // owned by the parameters, which outlive the returned borrow.
                unsafe { std::slice::from_raw_parts(raw.extradata, size) }
            }
            _ => &[],
        }
    }
}

impl std::fmt::Debug for CodecParameters {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodecParameters")
            .field("codec", &self.codec())
            .field("width", &self.width())
            .field("height", &self.height())
            .field("sample_rate", &self.sample_rate())
            .field("channels", &self.channels())
            .field("extradata_len", &self.extradata().len())
            .finish()
    }
}

/// One stream of a file or of an encoder: its parameters and the time base of
/// its packets' timestamps.
#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub time_base: Rational,
    pub parameters: CodecParameters,
}

struct CodecContext(NonNull<sys::AVCodecContext>);

// SAFETY: a codec context may be used from any one thread at a time; the
// encoders and decoders ngine uses keep no thread-local state.
unsafe impl Send for CodecContext {}

impl CodecContext {
    fn new(codec: CodecRef) -> Result<Self> {
        // SAFETY: the codec pointer is a registered codec.
        NonNull::new(unsafe { sys::avcodec_alloc_context3(codec.as_ptr()) })
            .map(Self)
            .ok_or_else(|| out_of_memory("allocate a codec context"))
    }

    fn as_ptr(&self) -> *mut sys::AVCodecContext {
        self.0.as_ptr()
    }

    fn unopened_mut(&mut self) -> &mut sys::AVCodecContext {
        // SAFETY: the context is valid and, before avcodec_open2, nothing else
        // holds a pointer to it.
        unsafe { self.0.as_mut() }
    }

    fn set_private_option(&mut self, name: &CStr, value: &CStr) {
        // SAFETY: the context is valid, both strings are NUL-terminated, and
        // av_opt_set reports an unknown option as an error without side
        // effects.
        let _status = unsafe {
            sys::av_opt_set(
                self.as_ptr().cast(),
                name.as_ptr(),
                value.as_ptr(),
                sys::AV_OPT_SEARCH_CHILDREN.cast_signed(),
            )
        };
    }

    fn open(&mut self, codec: CodecRef, action: &'static str) -> Result<()> {
        // SAFETY: the context was allocated for this codec and is not open.
        let status = unsafe { sys::avcodec_open2(self.as_ptr(), codec.as_ptr(), ptr::null_mut()) };
        check(status, action).map(drop)
    }

    fn read<T>(&self, field: impl FnOnce(&sys::AVCodecContext) -> T) -> T {
        // SAFETY: the context is valid; the closure only copies plain fields
        // that the codec sets during avcodec_open2 and leaves alone afterwards.
        field(unsafe { self.0.as_ref() })
    }

    fn send_frame(&mut self, frame: Option<&Frame>, action: &'static str) -> Result<()> {
        let frame = frame.map_or(ptr::null(), |frame| frame.as_ptr().cast_const());
        // SAFETY: the context is open as an encoder and the frame is either
        // null, which starts draining, or a valid frame the encoder copies or
        // references.
        let status = unsafe { sys::avcodec_send_frame(self.as_ptr(), frame) };
        check(status, action).map(drop)
    }

    fn receive_packet(&mut self, packet: &mut RawPacket, action: &'static str) -> Result<bool> {
        // SAFETY: the context is open as an encoder and the packet is valid;
        // FFmpeg unrefs it before writing.
        let status = unsafe { sys::avcodec_receive_packet(self.as_ptr(), packet.as_ptr()) };
        if status == sys::NGINE_AVERROR_EAGAIN || status == sys::NGINE_AVERROR_EOF {
            return Ok(false);
        }
        check(status, action).map(|_| true)
    }

    fn send_packet(&mut self, packet: Option<&RawPacket>, action: &'static str) -> Result<()> {
        let packet = packet.map_or(ptr::null(), |packet| packet.as_ptr().cast_const());
        // SAFETY: the context is open as a decoder and the packet is either
        // null, which starts draining, or valid with padded data.
        let status = unsafe { sys::avcodec_send_packet(self.as_ptr(), packet) };
        check(status, action).map(drop)
    }

    fn receive_frame(&mut self, frame: &mut Frame, action: &'static str) -> Result<bool> {
        // SAFETY: the context is open as a decoder and the frame is valid;
        // FFmpeg unrefs it before writing.
        let status = unsafe { sys::avcodec_receive_frame(self.as_ptr(), frame.as_ptr()) };
        if status == sys::NGINE_AVERROR_EAGAIN || status == sys::NGINE_AVERROR_EOF {
            return Ok(false);
        }
        check(status, action).map(|_| true)
    }

    fn flush(&mut self) {
        // SAFETY: the context is open; flushing drops its buffered frames.
        unsafe { sys::avcodec_flush_buffers(self.as_ptr()) }
    }
}

impl Drop for CodecContext {
    fn drop(&mut self) {
        let mut context = self.0.as_ptr();
        // SAFETY: allocated by avcodec_alloc_context3 and not used again.
        unsafe { sys::avcodec_free_context(&raw mut context) }
    }
}
