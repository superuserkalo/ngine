use crate::error::{Error, Result};
use crate::ffi::{VideoEncoder, VideoEncoderSettings, VideoInput, VideoSource, has_encoder};
use crate::types::{PixelFormat, PixelView, PlaneView, Rational, VideoCodec};

const TRIAL_WIDTH: u32 = 640;
const TRIAL_HEIGHT: u32 = 360;
const TRIAL_FRAMES: i64 = 5;

/// The software H.264 encoder, which works without any GPU.
pub const SOFTWARE_H264: &str = "libopenh264";

#[derive(Clone, Copy)]
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

const CANDIDATES: [Candidate; 19] = [
    hardware("h264_videotoolbox", VideoCodec::H264),
    hardware("h264_vaapi", VideoCodec::H264),
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
    hardware("hevc_vaapi", VideoCodec::Hevc),
    hardware("hevc_nvenc", VideoCodec::Hevc),
    hardware("hevc_amf", VideoCodec::Hevc),
    hardware("hevc_qsv", VideoCodec::Hevc),
    hardware("hevc_mf", VideoCodec::Hevc),
    hardware("av1_videotoolbox", VideoCodec::Av1),
    hardware("av1_vaapi", VideoCodec::Av1),
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

/// What the trial encode of one encoder showed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trial {
    Works,
    /// The bundled `FFmpeg` was built without it, as it is for the encoders
    /// of the other operating systems.
    NotInThisBuild,
    Failed(Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbedEncoder {
    pub encoder: EncoderInfo,
    pub trial: Trial,
}

fn probe_with(
    is_built: impl Fn(&str) -> bool,
    trial: impl Fn(&str) -> Result<()>,
) -> Vec<ProbedEncoder> {
    CANDIDATES
        .iter()
        .map(|candidate| ProbedEncoder {
            encoder: EncoderInfo {
                name: candidate.name.to_owned(),
                codec: candidate.codec,
                hardware: candidate.hardware,
            },
            trial: if !is_built(candidate.name) {
                Trial::NotInThisBuild
            } else if let Err(error) = trial(candidate.name) {
                Trial::Failed(error)
            } else {
                Trial::Works
            },
        })
        .collect()
}

fn working(probed: Vec<ProbedEncoder>) -> Vec<EncoderInfo> {
    probed
        .into_iter()
        .filter(|probed| probed.trial == Trial::Works)
        .map(|probed| probed.encoder)
        .collect()
}

/// Runs the trial encode of every encoder ngine knows and keeps the reason
/// each one failed. The order is the order of preference within a codec:
/// `VideoToolbox`, VA-API, NVENC, AMF, Quick Sync, Media Foundation, then
/// software.
///
/// This opens every hardware encoder, which can take a moment.
pub fn probe_video_encoders() -> Vec<ProbedEncoder> {
    probe_with(has_encoder, try_video_encoder)
}

/// Encodes a few small frames with every encoder ngine knows and returns the
/// ones that produced a keyframe and global headers, grouped by codec with
/// hardware encoders before the software one.
///
/// This opens every hardware encoder, which can take a moment, so call it
/// once and keep the answer.
pub fn working_video_encoders() -> Vec<EncoderInfo> {
    working(probe_video_encoders())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn names(encoders: &[EncoderInfo]) -> Vec<&str> {
        encoders
            .iter()
            .map(|encoder| encoder.name.as_str())
            .collect()
    }

    fn broken(name: &str) -> Result<()> {
        Err(Error::unavailable(format!("{name} has no device")))
    }

    #[test]
    fn hardware_is_preferred_in_one_fixed_order_and_software_comes_last() {
        let everything = working(probe_with(|_| true, |_| Ok(())));

        assert_eq!(
            names(&everything)[..7],
            [
                "h264_videotoolbox",
                "h264_vaapi",
                "h264_nvenc",
                "h264_amf",
                "h264_qsv",
                "h264_mf",
                SOFTWARE_H264
            ]
        );
        for codec in VideoCodec::ALL {
            let of_codec: Vec<&EncoderInfo> = everything
                .iter()
                .filter(|encoder| encoder.codec == codec)
                .collect();
            let families: Vec<&str> = of_codec
                .iter()
                .filter(|encoder| encoder.hardware)
                .filter_map(|encoder| encoder.name.rsplit_once('_'))
                .map(|(_, family)| family)
                .collect();
            assert_eq!(
                families,
                ["videotoolbox", "vaapi", "nvenc", "amf", "qsv", "mf"],
                "{codec:?}"
            );
            assert!(
                of_codec
                    .iter()
                    .is_sorted_by_key(|encoder| !encoder.hardware)
            );
        }
    }

    #[test]
    fn a_windows_pc_with_an_amd_gpu_skips_the_encoders_whose_trial_fails() {
        let built = |name: &str| !name.ends_with("_videotoolbox") && !name.ends_with("_vaapi");
        let trial = |name: &str| {
            if name.ends_with("_amf") || name.ends_with("_mf") || name == SOFTWARE_H264 {
                Ok(())
            } else {
                broken(name)
            }
        };

        let found = working(probe_with(built, trial));

        assert_eq!(
            names(&found),
            [
                "h264_amf",
                "h264_mf",
                SOFTWARE_H264,
                "hevc_amf",
                "hevc_mf",
                "av1_amf",
                "av1_mf"
            ]
        );
        assert_eq!(working_video_codecs(&found), VideoCodec::ALL);
    }

    #[test]
    fn a_linux_pc_prefers_va_api_and_keeps_nvenc_behind_it() {
        let built = |name: &str| {
            name.ends_with("_vaapi") || name.ends_with("_nvenc") || name == SOFTWARE_H264
        };

        let found = working(probe_with(built, |name| {
            if name.starts_with("av1_") {
                broken(name)
            } else {
                Ok(())
            }
        }));

        assert_eq!(
            names(&found),
            [
                "h264_vaapi",
                "h264_nvenc",
                SOFTWARE_H264,
                "hevc_vaapi",
                "hevc_nvenc"
            ]
        );
    }

    #[test]
    fn a_machine_without_a_gpu_is_left_with_software_h264_and_the_reasons() {
        let probed = probe_with(
            |name| name.ends_with("_nvenc") || name == SOFTWARE_H264,
            |name| {
                if name == SOFTWARE_H264 {
                    Ok(())
                } else {
                    broken(name)
                }
            },
        );

        let nvenc = probed
            .iter()
            .find(|probed| probed.encoder.name == "h264_nvenc")
            .unwrap();
        let vaapi = probed
            .iter()
            .find(|probed| probed.encoder.name == "h264_vaapi")
            .unwrap();
        assert!(matches!(nvenc.trial, Trial::Failed(Error::Unavailable(_))));
        assert_eq!(vaapi.trial, Trial::NotInThisBuild);
        assert_eq!(names(&working(probed)), [SOFTWARE_H264]);
    }

    #[test]
    fn this_machine_can_at_least_encode_h264() {
        let found = working_video_encoders();

        assert!(
            found
                .iter()
                .any(|encoder| encoder.codec == VideoCodec::H264),
            "{found:?}"
        );
    }
}
