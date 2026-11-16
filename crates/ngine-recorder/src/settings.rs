use ngine_media::{AudioCodec, EncoderInfo, VideoCodec};

use crate::error::RecorderError;

pub const AUDIO_TRACK_BIT_RATE: u32 = 128_000;
pub const HIGHEST_FRAME_RATE: u32 = 240;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Quality {
    Low,
    Medium,
    High,
    Highest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tracks {
    /// The mix, then system sound, then the microphone. The two separate
    /// tracks are written as captured, whatever the volumes are, so the clip
    /// editor can rebalance them; the volumes shape the mix only.
    Three,
    MixOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoSettings {
    pub codec: VideoCodec,
    pub quality: Quality,
    pub frame_rate: u32,
    /// The name `FFmpeg` gives one encoder, to use it instead of the first
    /// working encoder for `codec`.
    pub encoder: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioSettings {
    /// The volume of system sound in the mix, or `None` to leave it out.
    pub system_gain: Option<f32>,
    /// The volume of the microphone in the mix, or `None` to leave it out.
    pub microphone_gain: Option<f32>,
    pub tracks: Tracks,
    pub codec: AudioCodec,
}

impl AudioSettings {
    /// No audio track is written when both sources are off.
    pub fn track_count(&self) -> usize {
        match (
            self.system_gain.is_some() || self.microphone_gain.is_some(),
            self.tracks,
        ) {
            (false, _) => 0,
            (true, Tracks::MixOnly) => 1,
            (true, Tracks::Three) => 3,
        }
    }
}

/// The most bits per second the video encoder may spend, measured over one
/// second. It depends on the preset, the codec and the frame rate only, so
/// the cost of a recording is known before it starts.
pub fn video_bit_rate_cap(codec: VideoCodec, quality: Quality, frame_rate: u32) -> u64 {
    let base_kbps: u64 = match quality {
        Quality::Low => 5_600,
        Quality::Medium => 8_600,
        Quality::High => 11_600,
        Quality::Highest => 19_600,
    };
    let codec_percent = match codec {
        VideoCodec::H264 => 100,
        VideoCodec::Hevc => 70,
        VideoCodec::Av1 => 60,
    };
    let rate_percent = 20 + u64::from(frame_rate.min(HIGHEST_FRAME_RATE)) * 80 / 60;
    base_kbps * codec_percent / 100 * rate_percent / 100 * 1000
}

/// The capped bit rate of video and all audio tracks together.
pub fn bit_rate_cap(video: &VideoSettings, audio: &AudioSettings) -> u64 {
    let tracks = u64::try_from(audio.track_count()).unwrap_or(3);
    video_bit_rate_cap(video.codec, video.quality, video.frame_rate)
        + tracks * u64::from(AUDIO_TRACK_BIT_RATE)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChosenEncoder {
    pub name: String,
}

pub(crate) fn choose_encoder(
    video: &VideoSettings,
    working: &[EncoderInfo],
) -> Result<ChosenEncoder, RecorderError> {
    if video.frame_rate == 0 || video.frame_rate > HIGHEST_FRAME_RATE {
        return Err(RecorderError::FrameRateOutOfRange(video.frame_rate));
    }
    let found = match &video.encoder {
        Some(name) => working.iter().find(|encoder| &encoder.name == name),
        None => working.iter().find(|encoder| encoder.codec == video.codec),
    };
    found
        .map(|encoder| ChosenEncoder {
            name: encoder.name.clone(),
        })
        .ok_or_else(|| match &video.encoder {
            Some(name) => RecorderError::EncoderUnavailable(name.clone()),
            None => RecorderError::CodecUnavailable(video.codec),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn working() -> Vec<EncoderInfo> {
        vec![
            EncoderInfo {
                name: "h264_hardware".into(),
                codec: VideoCodec::H264,
                hardware: true,
            },
            EncoderInfo {
                name: "libopenh264".into(),
                codec: VideoCodec::H264,
                hardware: false,
            },
        ]
    }

    fn video(codec: VideoCodec, encoder: Option<&str>) -> VideoSettings {
        VideoSettings {
            codec,
            quality: Quality::High,
            frame_rate: 60,
            encoder: encoder.map(str::to_owned),
        }
    }

    #[test]
    fn the_first_working_encoder_of_the_codec_is_chosen_unless_one_is_named() {
        let first = choose_encoder(&video(VideoCodec::H264, None), &working()).unwrap();
        assert_eq!(first.name, "h264_hardware");
        let named =
            choose_encoder(&video(VideoCodec::H264, Some("libopenh264")), &working()).unwrap();
        assert_eq!(named.name, "libopenh264");
    }

    #[test]
    fn a_codec_the_machine_cannot_encode_is_refused() {
        assert!(matches!(
            choose_encoder(&video(VideoCodec::Av1, None), &working()),
            Err(RecorderError::CodecUnavailable(VideoCodec::Av1))
        ));
        assert!(matches!(
            choose_encoder(&video(VideoCodec::H264, Some("h264_nvenc")), &working()),
            Err(RecorderError::EncoderUnavailable(_))
        ));
    }

    #[test]
    fn a_higher_preset_costs_more_and_the_default_matches_the_settings_screen() {
        let caps: Vec<u64> = [
            Quality::Low,
            Quality::Medium,
            Quality::High,
            Quality::Highest,
        ]
        .into_iter()
        .map(|quality| video_bit_rate_cap(VideoCodec::H264, quality, 60))
        .collect();
        assert!(caps.is_sorted());
        assert_eq!(caps[2], 11_600_000);
        let audio = AudioSettings {
            system_gain: Some(1.0),
            microphone_gain: Some(1.0),
            tracks: Tracks::Three,
            codec: AudioCodec::Aac,
        };
        let total = bit_rate_cap(&video(VideoCodec::H264, None), &audio);
        assert_eq!(total, 11_600_000 + 3 * 128_000);
    }
}
