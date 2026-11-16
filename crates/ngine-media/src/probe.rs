use crate::error::{Error, Result};
use crate::ffi::{VideoEncoder, VideoEncoderSettings, VideoInput, VideoSource, has_encoder};
use crate::types::{PixelFormat, PixelView, PlaneView, Rational, VideoCodec};

const TRIAL_WIDTH: u32 = 640;
const TRIAL_HEIGHT: u32 = 360;
const TRIAL_FRAMES: i64 = 5;

/// The software H.264 encoder, which works without any GPU.
pub const SOFTWARE_H264: &str = "libopenh264";

struct Candidate {
    name: &'static str,
    codec: VideoCodec,
    hardware: bool,
}

const fn hardware(name: &'static str, codec: VideoCodec) -> Candidate {
    Candidate {
        name,
        codec,
        hardware: true,
    }
}

const CANDIDATES: [Candidate; 16] = [
    hardware("h264_videotoolbox", VideoCodec::H264),
    hardware("h264_nvenc", VideoCodec::H264),
    hardware("h264_amf", VideoCodec::H264),
    hardware("h264_qsv", VideoCodec::H264),
    hardware("h264_mf", VideoCodec::H264),
    Candidate {
        name: SOFTWARE_H264,
        codec: VideoCodec::H264,
        hardware: false,
    },
    hardware("hevc_videotoolbox", VideoCodec::Hevc),
    hardware("hevc_nvenc", VideoCodec::Hevc),
    hardware("hevc_amf", VideoCodec::Hevc),
    hardware("hevc_qsv", VideoCodec::Hevc),
    hardware("hevc_mf", VideoCodec::Hevc),
    hardware("av1_videotoolbox", VideoCodec::Av1),
    hardware("av1_nvenc", VideoCodec::Av1),
    hardware("av1_amf", VideoCodec::Av1),
    hardware("av1_qsv", VideoCodec::Av1),
    hardware("av1_mf", VideoCodec::Av1),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderInfo {
    /// The name `FFmpeg` gives the encoder, to put in `VideoEncoderSettings`.
    pub name: String,
    pub codec: VideoCodec,
    pub hardware: bool,
}

/// Encodes a few small frames with every encoder ngine knows and returns the
/// ones that produced a keyframe and global headers, grouped by codec with
/// hardware encoders before the software one.
///
/// This opens every hardware encoder, which can take a moment, so call it
/// once and keep the answer.
pub fn working_video_encoders() -> Vec<EncoderInfo> {
    CANDIDATES
        .iter()
        .filter(|candidate| has_encoder(candidate.name))
        .filter(|candidate| try_video_encoder(candidate.name).is_ok())
        .map(|candidate| EncoderInfo {
            name: candidate.name.to_owned(),
            codec: candidate.codec,
            hardware: candidate.hardware,
        })
        .collect()
}

/// The codecs that at least one working encoder produces.
pub fn working_video_codecs(encoders: &[EncoderInfo]) -> Vec<VideoCodec> {
    VideoCodec::ALL
        .into_iter()
        .filter(|codec| encoders.iter().any(|encoder| encoder.codec == *codec))
        .collect()
}

/// Runs the trial encode for one encoder and says why it failed.
pub fn try_video_encoder(name: &str) -> Result<()> {
    let settings = VideoEncoderSettings {
        encoder: name.to_owned(),
        width: TRIAL_WIDTH,
        height: TRIAL_HEIGHT,
        time_base: Rational::new(1, 30),
        frame_rate: Rational::new(30, 1),
        bitrate: 1_000_000,
        max_bitrate: 1_500_000,
        keyframe_interval: 30,
    };
    let mut encoder = VideoEncoder::open(&settings, VideoSource::Cpu(PixelFormat::Nv12))?;
    let luma = vec![64_u8; (TRIAL_WIDTH * TRIAL_HEIGHT) as usize];
    let chroma = vec![128_u8; (TRIAL_WIDTH * TRIAL_HEIGHT / 2) as usize];
    let stride = TRIAL_WIDTH as usize;
    let pixels = PixelView::nv12(
        TRIAL_WIDTH,
        TRIAL_HEIGHT,
        PlaneView {
            data: &luma,
            stride,
        },
        PlaneView {
            data: &chroma,
            stride,
        },
    )?;
    let mut packets = Vec::new();
    for pts in 0..TRIAL_FRAMES {
        packets.extend(encoder.encode(VideoInput::Cpu(&pixels), pts)?);
    }
    packets.extend(encoder.finish()?);
    let starts_with_keyframe = packets.first().is_some_and(|packet| packet.keyframe);
    if !starts_with_keyframe {
        return Err(Error::unavailable(format!(
            "{name} did not start with a keyframe"
        )));
    }
    if encoder.stream_info().parameters.extradata().is_empty() {
        return Err(Error::unavailable(format!(
            "{name} did not provide global headers"
        )));
    }
    Ok(())
}
