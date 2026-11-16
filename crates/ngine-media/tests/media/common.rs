use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ngine_media::{
    AudioCodec, AudioDecoder, AudioEncoder, AudioEncoderSettings, AudioFrame, Demuxer, Mp4Layout,
    Mp4Writer, Packet, PixelFormat, PixelView, PlaneView, Rational, SOFTWARE_H264, StreamInfo,
    VideoCodec, VideoDecoder, VideoDecoderOptions, VideoEncoder, VideoEncoderSettings, VideoFrame,
    VideoInput, VideoSource, has_encoder, working_video_encoders,
};

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;
pub const FRAME_RATE: i32 = 30;
pub const SAMPLE_RATE: usize = 48_000;
pub const SYSTEM_TONE: f32 = 440.0;
pub const MICROPHONE_TONE: f32 = 1_000.0;
pub const TONE_LEVEL: f32 = 0.4;
pub const VIDEO_TIME_BASE: Rational = Rational::new(1, 90_000);
const TICKS_PER_FRAME: i64 = 3_000;

pub fn h264_encoder() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        if has_encoder(SOFTWARE_H264) {
            return SOFTWARE_H264.to_owned();
        }
        working_video_encoders()
            .into_iter()
            .find(|encoder| encoder.codec == VideoCodec::H264)
            .map(|encoder| encoder.name)
            .expect("the tests need libopenh264 or a working hardware H.264 encoder")
    })
}

pub struct TempFile(PathBuf);

impl TempFile {
    pub fn new(name: &str) -> Self {
        let file = format!("ngine-media-{}-{name}", std::process::id());
        Self(std::env::temp_dir().join(file))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _missing_is_fine = std::fs::remove_file(&self.0);
    }
}

pub fn float(count: usize) -> f64 {
    f64::from(u32::try_from(count).unwrap())
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "test signals are computed in f64 and stored as the f32 samples the API takes"
)]
pub fn narrow(value: f64) -> f32 {
    value as f32
}

pub fn gray_level(frame: i64) -> u8 {
    u8::try_from(40 + (frame * 2) % 160).unwrap()
}

pub fn gray_bgra(frame: i64) -> Vec<u8> {
    let level = gray_level(frame);
    [level, level, level, 255].repeat((WIDTH * HEIGHT) as usize)
}

pub fn tone(frequency: f32, first_frame: usize, frames: usize) -> Vec<f32> {
    (first_frame..first_frame + frames)
        .flat_map(|frame| {
            let seconds = float(frame % SAMPLE_RATE) / float(SAMPLE_RATE);
            let turns = f64::from(frequency) * seconds;
            let sample = narrow(f64::from(TONE_LEVEL) * (std::f64::consts::TAU * turns).sin());
            [sample, sample]
        })
        .collect()
}

pub fn video_settings(encoder: &str, keyframe_interval: u32) -> VideoEncoderSettings {
    VideoEncoderSettings {
        encoder: encoder.to_owned(),
        width: WIDTH,
        height: HEIGHT,
        time_base: VIDEO_TIME_BASE,
        frame_rate: Rational::new(FRAME_RATE, 1),
        bitrate: 1_000_000,
        max_bitrate: 1_500_000,
        keyframe_interval,
    }
}

pub fn frame_pts(frame: i64) -> i64 {
    frame * TICKS_PER_FRAME
}

pub fn encode_gray_frame(encoder: &mut VideoEncoder, frame: i64) -> Vec<Packet> {
    let pixels = gray_bgra(frame);
    let view = PixelView::bgra(
        WIDTH,
        HEIGHT,
        PlaneView {
            data: &pixels,
            stride: (WIDTH * 4) as usize,
        },
    )
    .unwrap();
    encoder
        .encode(VideoInput::Cpu(&view), frame_pts(frame))
        .unwrap()
}

pub struct Source {
    pub streams: Vec<StreamInfo>,
    pub packets: Vec<Packet>,
}

pub fn encode_source(
    seconds: i64,
    keyframe_interval: u32,
    audio_codec: AudioCodec,
    audio: &[Vec<f32>],
) -> Source {
    let mut video = VideoEncoder::open(
        &video_settings(h264_encoder(), keyframe_interval),
        VideoSource::Cpu(PixelFormat::Bgra),
    )
    .unwrap();
    let mut encoders = audio
        .iter()
        .map(|_| {
            AudioEncoder::open(AudioEncoderSettings {
                codec: audio_codec,
                bitrate: 128_000,
            })
            .unwrap()
        })
        .collect::<Vec<_>>();
    let mut streams = vec![video.stream_info().clone()];
    streams.extend(encoders.iter().map(|encoder| encoder.stream_info().clone()));

    let samples_per_frame = SAMPLE_RATE / FRAME_RATE as usize * 2;
    let mut packets = Vec::new();
    for frame in 0..seconds * i64::from(FRAME_RATE) {
        packets.extend(encode_gray_frame(&mut video, frame));
        let start = usize::try_from(frame).unwrap() * samples_per_frame;
        for (index, encoder) in encoders.iter_mut().enumerate() {
            let chunk = &audio[index][start..start + samples_per_frame];
            packets.extend(on_stream(index + 1, encoder.encode(chunk).unwrap()));
        }
    }
    packets.extend(video.finish().unwrap());
    for (index, encoder) in encoders.iter_mut().enumerate() {
        packets.extend(on_stream(index + 1, encoder.finish().unwrap()));
    }
    Source { streams, packets }
}

fn on_stream(stream: usize, packets: Vec<Packet>) -> impl Iterator<Item = Packet> {
    packets.into_iter().map(move |mut packet| {
        packet.stream = stream;
        packet
    })
}

pub fn write_recording(path: &Path, source: &Source) {
    let mut writer = Mp4Writer::create(path, &source.streams, Mp4Layout::Recording).unwrap();
    for packet in &source.packets {
        writer.write(packet).unwrap();
    }
    writer.finish().unwrap();
}

pub struct Decoded {
    pub streams: Vec<StreamInfo>,
    pub video: Vec<VideoFrame>,
    pub audio: Vec<Vec<AudioFrame>>,
    pub duration_micros: Option<i64>,
}

impl Decoded {
    pub fn video_micros(&self) -> Vec<i64> {
        let time_base = self.streams[0].time_base;
        self.video
            .iter()
            .map(|frame| {
                time_base
                    .rescale(frame.pts.unwrap(), Rational::MICROS)
                    .unwrap()
            })
            .collect()
    }

    pub fn audio_start_micros(&self, track: usize) -> i64 {
        let time_base = self.streams[track + 1].time_base;
        let first = self.audio[track].first().unwrap().pts.unwrap();
        time_base.rescale(first, Rational::MICROS).unwrap()
    }

    pub fn left_channel(&self, track: usize) -> Vec<f32> {
        self.audio[track]
            .iter()
            .flat_map(|frame| frame.samples.iter().step_by(usize::from(frame.channels)))
            .copied()
            .collect()
    }
}

pub fn decode_file(path: &Path, output: PixelFormat, hardware: bool) -> Decoded {
    let mut demuxer = Demuxer::open(path).unwrap();
    let streams = demuxer.streams().to_vec();
    let mut video_decoder =
        VideoDecoder::open(&streams[0], VideoDecoderOptions { output, hardware }).unwrap();
    let mut audio_decoders = streams[1..]
        .iter()
        .map(|stream| AudioDecoder::open(stream).unwrap())
        .collect::<Vec<_>>();
    let mut video = Vec::new();
    let mut audio = vec![Vec::new(); audio_decoders.len()];
    while let Some(packet) = demuxer.read().unwrap() {
        if packet.stream == 0 {
            video.extend(video_decoder.decode(&packet).unwrap());
        } else {
            let track = packet.stream - 1;
            audio[track].extend(audio_decoders[track].decode(&packet).unwrap());
        }
    }
    video.extend(video_decoder.finish().unwrap());
    for (track, decoder) in audio_decoders.iter_mut().enumerate() {
        audio[track].extend(decoder.finish().unwrap());
    }
    Decoded {
        duration_micros: demuxer.duration_micros(),
        streams,
        video,
        audio,
    }
}

pub fn mean_gray(frame: &VideoFrame) -> f64 {
    let plane = &frame.planes[0];
    let step = if frame.format == PixelFormat::Bgra {
        4
    } else {
        1
    };
    let (sum, count) = plane
        .data
        .iter()
        .step_by(step)
        .fold((0.0, 0.0), |(sum, count), value| {
            (sum + f64::from(*value), count + 1.0)
        });
    sum / count
}

pub fn tone_level(samples: &[f32], frequency: f32) -> f32 {
    let (mut sine, mut cosine) = (0.0_f64, 0.0_f64);
    for (index, sample) in samples.iter().enumerate() {
        let angle = f64::from(TAU * frequency) * float(index) / float(SAMPLE_RATE);
        sine += f64::from(*sample) * angle.sin();
        cosine += f64::from(*sample) * angle.cos();
    }
    narrow(2.0 * sine.hypot(cosine) / float(samples.len()))
}

pub fn top_level_boxes(path: &Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap();
    let mut boxes = Vec::new();
    let mut at = 0_usize;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        boxes.push(String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned());
        let size = match size {
            0 => bytes.len() - at,
            1 => usize::try_from(u64::from_be_bytes(
                bytes[at + 8..at + 16].try_into().unwrap(),
            ))
            .unwrap(),
            size => size,
        };
        at += size.max(8);
    }
    boxes
}
