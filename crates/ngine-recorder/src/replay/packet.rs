use std::sync::Arc;

use super::time::Ticks;

/// Which stream of the encoder a packet belongs to: the one video stream, or
/// one of up to three audio streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamIndex(u8);

impl StreamIndex {
    pub const VIDEO: Self = Self(0);
    pub const MAX_AUDIO_STREAMS: u8 = 3;

    /// `track` counts audio streams from zero. Returns `None` for a fourth
    /// track or beyond.
    pub fn audio(track: u8) -> Option<Self> {
        (track < Self::MAX_AUDIO_STREAMS).then_some(Self(track + 1))
    }

    /// Zero for video, one to three for audio.
    pub fn get(self) -> u8 {
        self.0
    }

    pub fn is_video(self) -> bool {
        self == Self::VIDEO
    }
}

/// One encoded packet. Every time is in [`Ticks`], whatever the encoder used.
///
/// `keyframe` on a video packet says a decoder can start there. A video
/// packet must carry its real `duration`, because the buffer measures what it
/// covers up to the end of the newest video packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub stream: StreamIndex,
    pub pts: Ticks,
    pub dts: Ticks,
    pub duration: Ticks,
    pub keyframe: bool,
    pub data: Arc<[u8]>,
}

impl Packet {
    pub(crate) fn end(&self) -> Ticks {
        self.pts + self.duration
    }

    pub(crate) fn size_bytes(&self) -> u64 {
        u64::try_from(self.data.len()).unwrap_or(u64::MAX)
    }
}
