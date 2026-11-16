use std::path::Path;
use std::ptr::{self, NonNull};

use super::{CodecParameters, RawPacket, StreamInfo, c_path, check, optional_timestamp, sys};
use crate::error::Result;
use crate::types::Packet;

/// Reads the packets of an MP4, MOV or Matroska file.
pub struct Demuxer {
    context: NonNull<sys::AVFormatContext>,
    packet: RawPacket,
    streams: Vec<StreamInfo>,
}

// SAFETY: a demuxer context has no thread affinity and is used through
// `&mut self` only.
unsafe impl Send for Demuxer {}

impl Demuxer {
    pub fn open(path: &Path) -> Result<Self> {
        super::init();
        let path = c_path(path)?;
        let packet = RawPacket::new()?;
        let mut context = ptr::null_mut();
        // SAFETY: `context` is a live out-pointer holding null, so FFmpeg
        // allocates the context, and the path is NUL-terminated.
        let status = unsafe {
            sys::avformat_open_input(
                &raw mut context,
                path.as_ptr(),
                ptr::null(),
                ptr::null_mut(),
            )
        };
        check(status, "open the file")?;
        let Some(context) = NonNull::new(context) else {
            return Err(super::out_of_memory("open the file"));
        };
        let mut demuxer = Self {
            context,
            packet,
            streams: Vec::new(),
        };
        // SAFETY: the context is an open input.
        let status = unsafe { sys::avformat_find_stream_info(context.as_ptr(), ptr::null_mut()) };
        check(status, "read the file's streams")?;
        for stream in demuxer.raw_streams() {
            // SAFETY: every entry of an open input's stream list is a valid
            // stream owned by the context.
            let stream = unsafe { stream.as_ref() };
            // SAFETY: a demuxer stream always has allocated parameters.
            let parameters = unsafe { stream.and_then(|stream| stream.codecpar.as_ref()) };
            let (Some(stream), Some(parameters)) = (stream, parameters) else {
                return Err(super::out_of_memory("read the file's streams"));
            };
            demuxer.streams.push(StreamInfo {
                time_base: stream.time_base.into(),
                parameters: CodecParameters::copy_of(parameters)?,
            });
        }
        Ok(demuxer)
    }

    fn raw(&self) -> &sys::AVFormatContext {
        // SAFETY: the context is valid until Drop and FFmpeg only touches it
        // inside calls made through `&mut self`.
        unsafe { self.context.as_ref() }
    }

    fn raw_streams(&self) -> Vec<*mut sys::AVStream> {
        let raw = self.raw();
        let count = usize::try_from(raw.nb_streams).unwrap_or(0);
        if raw.streams.is_null() || count == 0 {
            return Vec::new();
        }
        // SAFETY: `streams` points at `nb_streams` stream pointers owned by
        // the context.
        unsafe { std::slice::from_raw_parts(raw.streams, count) }.to_vec()
    }

    /// The file's streams. A packet's `stream` field indexes this list.
    pub fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    pub fn duration_micros(&self) -> Option<i64> {
        optional_timestamp(self.raw().duration)
    }

    /// The next packet in file order, or `None` at the end of the file.
    pub fn read(&mut self) -> Result<Option<Packet>> {
        loop {
            self.packet.unref();
            // SAFETY: the context is an open input and the packet is blank.
            let status = unsafe { sys::av_read_frame(self.context.as_ptr(), self.packet.as_ptr()) };
            if status == sys::NGINE_AVERROR_EOF {
                return Ok(None);
            }
            check(status, "read a packet")?;
            let packet = self.packet.to_plain();
            if packet.stream < self.streams.len() {
                return Ok(Some(packet));
            }
        }
    }

    pub fn packets(&mut self) -> impl Iterator<Item = Result<Packet>> + '_ {
        std::iter::from_fn(move || self.read().transpose())
    }

    /// Moves to the last keyframe at or before `micros`, so the next video
    /// packet read is a keyframe. Decoders must be reset before they are fed
    /// again.
    pub fn seek(&mut self, micros: i64) -> Result<()> {
        // SAFETY: the context is an open input; stream -1 makes the timestamp
        // microseconds.
        let status = unsafe {
            sys::av_seek_frame(
                self.context.as_ptr(),
                -1,
                micros,
                sys::AVSEEK_FLAG_BACKWARD.cast_signed(),
            )
        };
        check(status, "seek").map(drop)
    }
}

impl Drop for Demuxer {
    fn drop(&mut self) {
        let mut context = self.context.as_ptr();
        // SAFETY: the context came from avformat_open_input and is not used
        // again.
        unsafe { sys::avformat_close_input(&raw mut context) }
    }
}
