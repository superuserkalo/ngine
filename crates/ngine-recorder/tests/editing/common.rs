use std::f64::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use ngine_media::{
    AudioCodec, AudioDecoder, AudioEncoder, AudioEncoderSettings, Demuxer, Mp4Layout, Mp4Writer,
    Packet, PixelFormat, PixelView, PlaneView, Rational, SOFTWARE_H264, StreamInfo, VideoCodec,
    VideoDecoder, VideoDecoderOptions, VideoEncoder, VideoEncoderSettings, VideoInput, VideoSource,
    has_encoder, working_video_encoders,
};

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;
pub const FRAME_RATE: i64 = 30;
pub const SECONDS: i64 = 4;
pub const SAMPLE_RATE: usize = 48_000;
pub const SYSTEM_TONE: f64 = 440.0;
pub const MICROPHONE_TONE: f64 = 1_000.0;
pub const LEVEL: f64 = 0.3;
const VIDEO_TIME_BASE: Rational = Rational::new(1, 90_000);
const TICKS_PER_FRAME: i64 = 3_000;
const BANDS: u32 = 8;

static FOLDERS: AtomicU32 = AtomicU32::new(0);

pub struct TempFolder(PathBuf);

impl TempFolder {
    pub fn new(test: &str) -> Self {
        let number = FOLDERS.fetch_add(1, Ordering::Relaxed);
        let name = format!("ngine-editing-{}-{number}-{test}", std::process::id());
        let folder = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&folder).unwrap();
        Self(folder)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _already_gone = std::fs::remove_dir_all(&self.0);
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

pub fn h264_encoder() -> String {
    if has_encoder(SOFTWARE_H264) {
        return SOFTWARE_H264.to_owned();
    }
    working_video_encoders()
        .into_iter()
        .find(|encoder| encoder.codec == VideoCodec::H264)
        .map(|encoder| encoder.name)
        .expect("the tests need libopenh264 or a working hardware H.264 encoder")
}

pub fn micros_of_frame(frame: i64) -> i64 {
    VIDEO_TIME_BASE
        .rescale(frame * TICKS_PER_FRAME, Rational::MICROS)
        .unwrap()
}

fn bands(frame: i64) -> Vec<u8> {
    let band = (WIDTH / BANDS) as usize;
    let row: Vec<u8> = (0..WIDTH as usize)
        .flat_map(|x| {
            let lit = (frame >> (x / band)) & 1 == 1;
            let level = if lit { 220 } else { 40 };
            [level, level, level, 255]
        })
        .collect();
    row.repeat(HEIGHT as usize)
}

pub fn frame_number(pixels: &[u8], width: u32, height: u32) -> i64 {
    let (width, height) = (width as usize, height as usize);
    let row = &pixels[height / 2 * width * 4..][..width * 4];
    (0..BANDS as usize)
        .map(|band| {
            let x = (2 * band + 1) * width / (2 * BANDS as usize);
            i64::from(row[x * 4 + 1] > 128) << band
        })
        .sum()
}

pub fn tone(frequency: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let seconds = float(frame % SAMPLE_RATE) / float(SAMPLE_RATE);
            let sample = narrow(LEVEL * (TAU * frequency * seconds).sin());
            [sample, sample]
        })
        .collect()
}

pub struct Source {
    pub streams: Vec<StreamInfo>,
    pub packets: Vec<Packet>,
}

pub fn source(audio_tracks: usize) -> Source {
    let frames = usize::try_from(SECONDS).unwrap() * SAMPLE_RATE;
    let system = tone(SYSTEM_TONE, frames);
    let microphone = tone(MICROPHONE_TONE, frames);
    let mix: Vec<f32> = system.iter().zip(&microphone).map(|(a, b)| a + b).collect();
    let audio = [mix, system, microphone];
    let audio = &audio[..audio_tracks];

    let settings = VideoEncoderSettings {
        encoder: h264_encoder(),
        width: WIDTH,
        height: HEIGHT,
        time_base: VIDEO_TIME_BASE,
        frame_rate: Rational::new(30, 1),
        bitrate: 2_000_000,
        max_bitrate: 3_000_000,
        keyframe_interval: 30,
    };
    let mut video = VideoEncoder::open(&settings, VideoSource::Cpu(PixelFormat::Bgra)).unwrap();
    let mut encoders: Vec<AudioEncoder> = audio
        .iter()
        .map(|_| {
            AudioEncoder::open(AudioEncoderSettings {
                codec: AudioCodec::Aac,
                bitrate: 128_000,
            })
            .unwrap()
        })
        .collect();
    let mut streams = vec![video.stream_info().clone()];
    streams.extend(encoders.iter().map(|encoder| encoder.stream_info().clone()));

    let on_stream = |stream: usize, packets: Vec<Packet>| {
        packets
            .into_iter()
            .map(move |packet| Packet { stream, ..packet })
    };
    let samples_per_frame = SAMPLE_RATE / usize::try_from(FRAME_RATE).unwrap() * 2;
    let mut packets = Vec::new();
    for frame in 0..SECONDS * FRAME_RATE {
        let pixels = bands(frame);
        let plane = PlaneView {
            data: &pixels,
            stride: (WIDTH * 4) as usize,
        };
        let view = PixelView::bgra(WIDTH, HEIGHT, plane).unwrap();
        let encoded = video
            .encode(VideoInput::Cpu(&view), frame * TICKS_PER_FRAME)
            .unwrap();
        packets.extend(encoded);
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

pub fn write_recording(path: &Path, audio_tracks: usize) {
    let source = source(audio_tracks);
    let mut writer = Mp4Writer::create(path, &source.streams, Mp4Layout::Recording).unwrap();
    for packet in &source.packets {
        writer.write(packet).unwrap();
    }
    writer.finish().unwrap();
}

pub struct Decoded {
    pub audio_tracks: usize,
    pub frame_micros: Vec<i64>,
    pub frame_numbers: Vec<i64>,
    pub sound: Vec<f32>,
    pub length_micros: i64,
}

pub fn decode(path: &Path) -> Decoded {
    let mut demuxer = Demuxer::open(path).unwrap();
    let streams = demuxer.streams().to_vec();
    let options = VideoDecoderOptions {
        output: PixelFormat::Bgra,
        hardware: false,
    };
    let mut video = VideoDecoder::open(&streams[0], options).unwrap();
    let mut audio = streams
        .get(1)
        .map(|stream| AudioDecoder::open(stream).unwrap());
    let mut frames = Vec::new();
    let mut sound = Vec::new();
    while let Some(packet) = demuxer.read().unwrap() {
        if packet.stream == 0 {
            frames.extend(video.decode(&packet).unwrap());
        } else if packet.stream == 1
            && let Some(audio) = &mut audio
        {
            sound.extend(audio.decode(&packet).unwrap());
        }
    }
    frames.extend(video.finish().unwrap());
    if let Some(audio) = &mut audio {
        sound.extend(audio.finish().unwrap());
    }
    Decoded {
        audio_tracks: streams.len() - 1,
        frame_micros: frames
            .iter()
            .map(|frame| {
                streams[0]
                    .time_base
                    .rescale(frame.pts.unwrap(), Rational::MICROS)
                    .unwrap()
            })
            .collect(),
        frame_numbers: frames
            .iter()
            .map(|frame| frame_number(&frame.planes[0].data, frame.width, frame.height))
            .collect(),
        sound: sound
            .iter()
            .flat_map(|frame| frame.samples.iter().step_by(usize::from(frame.channels)))
            .copied()
            .collect(),
        length_micros: demuxer.duration_micros().unwrap_or(0),
    }
}

pub fn tone_level(samples: &[f32], frequency: f64) -> f64 {
    let (mut sine, mut cosine) = (0.0_f64, 0.0_f64);
    for (index, sample) in samples.iter().enumerate() {
        let angle = TAU * frequency * float(index) / float(SAMPLE_RATE);
        sine += f64::from(*sample) * angle.sin();
        cosine += f64::from(*sample) * angle.cos();
    }
    2.0 * sine.hypot(cosine) / float(samples.len())
}

pub fn assert_tone(heard: f64, expected: f64) {
    assert!(
        (heard - expected).abs() < 0.03,
        "a tone at {heard} instead of {expected}"
    );
}
