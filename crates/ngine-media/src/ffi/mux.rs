use std::ffi::CStr;
use std::path::Path;
use std::ptr::{self, NonNull};

use super::{RawPacket, StreamInfo, c_path, check, out_of_memory, sys};
use crate::error::{Error, Result};
use crate::types::{Codec, Packet, VideoCodec};

pub const MAX_AUDIO_TRACKS: usize = 3;

const HVC1: u32 = u32::from_le_bytes(*b"hvc1");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mp4Layout {
    /// For a recording. The file is fragmented while it grows, so it plays up
    /// to its last fragment if the process dies, and `finish` turns it into a
    /// regular MP4. A fragment starts at every video keyframe.
    Recording,
    /// For a clip that is written in one go: a regular MP4 whose edit lists
    /// hide audio that starts before the first video frame.
    Clip,
}

impl Mp4Layout {
    fn options(self) -> &'static [(&'static CStr, &'static CStr)] {
        match self {
            Self::Recording => &[
                (c"movflags", c"hybrid_fragmented+frag_keyframe+delay_moov"),
                (c"flush_packets", c"1"),
            ],
            Self::Clip => &[],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Open,
    Failed,
    Closed,
}

/// Writes one video track and up to three audio tracks into an MP4 file.
///
/// Dropping a writer that was not finished finishes it, unless a write has
/// failed: then the file is left as the fragments already on disk.
pub struct Mp4Writer {
    context: NonNull<sys::AVFormatContext>,
    streams: Vec<(NonNull<sys::AVStream>, sys::AVRational)>,
    state: State,
}

// SAFETY: a muxer context has no thread affinity and is used through
// `&mut self` only.
unsafe impl Send for Mp4Writer {}

impl Mp4Writer {
    /// `streams[0]` must be video and the rest audio. A packet's `stream`
    /// field indexes this list, and its timestamps are in that stream's time
    /// base.
    pub fn create(path: &Path, streams: &[StreamInfo], layout: Mp4Layout) -> Result<Self> {
        super::init();
        let (video, audio) = streams
            .split_first()
            .ok_or_else(|| Error::invalid("an MP4 needs a video track"))?;
        if !video.parameters.is_video()
            || audio.len() > MAX_AUDIO_TRACKS
            || audio.iter().any(|stream| !stream.parameters.is_audio())
        {
            return Err(Error::invalid(
                "an MP4 holds one video track followed by at most three audio tracks",
            ));
        }
        if streams.iter().any(|stream| !stream.time_base.is_positive()) {
            return Err(Error::invalid("every stream needs a positive time base"));
        }
        let path = c_path(path)?;

        let mut context = ptr::null_mut();
        // SAFETY: `context` is a live out-pointer and both strings are
        // NUL-terminated.
        let status = unsafe {
            sys::avformat_alloc_output_context2(
                &raw mut context,
                ptr::null(),
                c"mp4".as_ptr(),
                path.as_ptr(),
            )
        };
        if status < 0 {
            return Err(Error::unavailable("the MP4 muxer"));
        }
        let context =
            NonNull::new(context).ok_or_else(|| out_of_memory("allocate an MP4 writer"))?;
        let mut writer = Self {
            context,
            streams: Vec::with_capacity(streams.len()),
            state: State::Closed,
        };
        for stream in streams {
            writer.add_stream(stream)?;
        }

        // SAFETY: the context has no I/O context yet, so avio_open stores a
        // new one in the slot, and the path is NUL-terminated.
        let status = unsafe {
            sys::avio_open(
                writer.io_slot(),
                path.as_ptr(),
                sys::AVIO_FLAG_WRITE.cast_signed(),
            )
        };
        check(status, "create the MP4 file")?;

        let mut options = ptr::null_mut();
        for (name, value) in layout.options() {
            // SAFETY: `options` is a live dictionary pointer and both strings
            // are NUL-terminated; FFmpeg copies them.
            let status =
                unsafe { sys::av_dict_set(&raw mut options, name.as_ptr(), value.as_ptr(), 0) };
            if status < 0 {
                // SAFETY: `options` is null or a dictionary this function
                // owns.
                unsafe { sys::av_dict_free(&raw mut options) };
                return Err(super::failure(status, "set the MP4 layout"));
            }
        }
        // SAFETY: the context has its streams and an open I/O context;
        // unrecognised options stay in `options`.
        let status = unsafe { sys::avformat_write_header(context.as_ptr(), &raw mut options) };
        // SAFETY: `options` is null or a dictionary this function owns.
        unsafe { sys::av_dict_free(&raw mut options) };
        check(status, "write the MP4 header")?;
        writer.state = State::Open;
        Ok(writer)
    }

    fn io_slot(&self) -> *mut *mut sys::AVIOContext {
        // SAFETY: the context is valid until Drop; this only takes the address
        // of its `pb` field.
        unsafe { &raw mut (*self.context.as_ptr()).pb }
    }

    fn add_stream(&mut self, info: &StreamInfo) -> Result<()> {
        // SAFETY: the context is a valid output context that has not written
        // its header.
        let stream = unsafe { sys::avformat_new_stream(self.context.as_ptr(), ptr::null()) };
        let stream = NonNull::new(stream).ok_or_else(|| out_of_memory("add an MP4 track"))?;
        // SAFETY: a new stream is valid, owned by the context, and nothing
        // else refers to it before the header is written.
        let raw = unsafe { &mut *stream.as_ptr() };
        // SAFETY: a new stream has allocated parameters and the source is a
        // valid parameter set.
        let status = unsafe {
            sys::avcodec_parameters_copy(raw.codecpar, ptr::from_ref(info.parameters.raw()))
        };
        check(status, "describe an MP4 track")?;
        // SAFETY: `codecpar` was allocated with the stream and just filled.
        let parameters = unsafe { &mut *raw.codecpar };
        parameters.codec_tag = match info.parameters.codec() {
            Some(Codec::Video(VideoCodec::Hevc)) => HVC1,
            _ => 0,
        };
        raw.time_base = info.time_base.into();
        self.streams.push((stream, info.time_base.into()));
        Ok(())
    }

    /// Writes one packet. Packets of each stream must arrive in decoding
    /// order; the writer interleaves the streams.
    pub fn write(&mut self, packet: &Packet) -> Result<()> {
        if self.state != State::Open {
            return Err(Error::invalid(
                "the MP4 writer is closed or has failed, so it takes no more packets",
            ));
        }
        let &(stream, source_time_base) = self
            .streams
            .get(packet.stream)
            .ok_or_else(|| Error::invalid(format!("the MP4 has no stream {}", packet.stream)))?;
        if packet.pts.is_none() {
            return Err(Error::invalid("a packet needs a timestamp to be written"));
        }
        let mut raw = RawPacket::from_plain(packet)?;
        if packet.dts.is_none() {
            raw.raw_mut().dts = raw.raw().pts;
        }
        // SAFETY: the stream belongs to the live context; after the header is
        // written the muxer only reads its time base.
        let target_time_base = unsafe { (*stream.as_ptr()).time_base };
        // SAFETY: the packet is valid and both time bases are positive.
        unsafe { sys::av_packet_rescale_ts(raw.as_ptr(), source_time_base, target_time_base) };
        // SAFETY: the header is written, the packet's stream index is in
        // range, and the muxer takes ownership of the packet's data.
        let status =
            unsafe { sys::av_interleaved_write_frame(self.context.as_ptr(), raw.as_ptr()) };
        if status < 0 {
            self.state = State::Failed;
        }
        check(status, "write a packet to the MP4").map(drop)
    }

    /// Completes the file as a regular MP4.
    pub fn finish(mut self) -> Result<()> {
        self.close()
    }

    fn close(&mut self) -> Result<()> {
        let was_open = self.state == State::Open;
        self.state = State::Closed;
        let mut outcome = Ok(());
        if was_open {
            // SAFETY: the header was written and no write has failed, so the
            // muxer is in the state av_write_trailer expects.
            let status = unsafe { sys::av_write_trailer(self.context.as_ptr()) };
            outcome = check(status, "finish the MP4").map(drop);
        }
        // SAFETY: avio_closep flushes and closes the I/O context in the slot
        // if there is one and nulls the slot.
        let status = unsafe { sys::avio_closep(self.io_slot()) };
        outcome.and(check(status, "close the MP4 file").map(drop))
    }
}

impl Drop for Mp4Writer {
    fn drop(&mut self) {
        let _unreported = self.close();
        // SAFETY: the context came from avformat_alloc_output_context2, its
        // I/O context is closed, and it is not used again.
        unsafe { sys::avformat_free_context(self.context.as_ptr()) }
    }
}
