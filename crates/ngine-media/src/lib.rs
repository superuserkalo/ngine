//! The only crate in the workspace that calls `FFmpeg`. Everything it exports is
//! safe to use.

mod error;
mod ffi;
mod mix;
mod nal;
mod probe;
mod remux;
mod splice;
mod types;

pub use error::{Error, Result};
pub use ffi::{
    AudioDecoder, AudioEncoder, AudioEncoderSettings, AudioLayout, CodecParameters, Demuxer,
    GpuFramePool, GpuSurface, Mp4Layout, Mp4Writer, Resampler, StreamInfo, VideoDecoder,
    VideoDecoderOptions, VideoEncoder, VideoEncoderSettings, VideoInput, VideoSource, has_encoder,
};
pub use mix::Mixer;
pub use probe::{
    EncoderInfo, SOFTWARE_H264, try_video_encoder, working_video_codecs, working_video_encoders,
};
pub use remux::{ClipRange, ClipSummary, ClipWriter, cut_clip};
pub use splice::{conform_to_track, same_parameter_sets, with_parameter_sets};
pub use types::{
    AudioCodec, AudioFrame, Codec, MAX_DIMENSION, Packet, PixelFormat, PixelView, Plane, PlaneView,
    Rational, VideoCodec, VideoFrame,
};
