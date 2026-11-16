use std::error::Error;
use std::path::Path;

use ngine_media::{
    AudioDecoder, Demuxer, PixelFormat, Rational, StreamInfo, VideoDecoder, VideoDecoderOptions,
    VideoFrame,
};
use ngine_recorder::read_markers;

const LUMA_SAMPLE_STEP: usize = 16;
const FLASH_RISE: f64 = 25.0;
const BEEP_LEVEL: f32 = 0.05;
const QUIET_BEFORE_A_BEEP_SECONDS: f64 = 0.3;
const FARTHEST_PAIR_SECONDS: f64 = 0.4;

#[expect(
    clippy::cast_precision_loss,
    reason = "timestamps and sample counts of a recording stay far below 2^52"
)]
fn real(value: i64) -> f64 {
    value as f64
}

fn count(value: usize) -> f64 {
    real(i64::try_from(value).unwrap_or(i64::MAX))
}

fn seconds(stream: &StreamInfo, pts: i64) -> f64 {
    stream
        .time_base
        .rescale(pts, Rational::MICROS)
        .map_or(0.0, |micros| real(micros) / 1e6)
}

fn mean_luma(frame: &VideoFrame) -> f64 {
    let Some(plane) = frame.planes.first() else {
        return 0.0;
    };
    let width = frame.width as usize;
    let mut sum = 0.0;
    let mut sampled = 0_usize;
    for row in plane
        .data
        .chunks(plane.stride.max(1))
        .step_by(LUMA_SAMPLE_STEP)
    {
        for value in row.iter().take(width).step_by(LUMA_SAMPLE_STEP) {
            sum += f64::from(*value);
            sampled += 1;
        }
    }
    sum / count(sampled.max(1))
}

#[derive(Default)]
struct Picture {
    frames: usize,
    first: Option<f64>,
    last: f64,
    longest_step: f64,
    previous_luma: Option<f64>,
    flashes: Vec<f64>,
}

impl Picture {
    fn take(&mut self, time: f64, luma: f64) {
        if self.first.is_none() {
            self.first = Some(time);
        } else {
            self.longest_step = self.longest_step.max(time - self.last);
        }
        if self
            .previous_luma
            .is_some_and(|previous| luma - previous > FLASH_RISE)
        {
            self.flashes.push(time);
        }
        self.previous_luma = Some(luma);
        self.last = time;
        self.frames += 1;
    }
}

#[derive(Default)]
struct Sound {
    samples: usize,
    first: Option<f64>,
    square_sum: f64,
    peak: f32,
    last_loud: f64,
    beeps: Vec<f64>,
}

impl Sound {
    fn take(&mut self, starts: f64, rate: u32, channels: u16, samples: &[f32]) {
        if self.first.is_none() {
            self.first = Some(starts);
            self.last_loud = f64::NEG_INFINITY;
        }
        for (index, frame) in samples.chunks(usize::from(channels.max(1))).enumerate() {
            let loudest = frame
                .iter()
                .fold(0.0_f32, |peak, value| peak.max(value.abs()));
            self.square_sum += f64::from(loudest) * f64::from(loudest);
            self.peak = self.peak.max(loudest);
            if loudest > BEEP_LEVEL {
                let time = starts + count(index) / f64::from(rate.max(1));
                if time - self.last_loud > QUIET_BEFORE_A_BEEP_SECONDS {
                    self.beeps.push(time);
                }
                self.last_loud = time;
            }
            self.samples += 1;
        }
    }

    fn rms_decibels(&self) -> f64 {
        let rms = (self.square_sum / count(self.samples.max(1))).sqrt();
        20.0 * rms.max(1e-9).log10()
    }
}

fn print_sync(picture: &Picture, sound: &Sound) {
    let mut offsets = Vec::new();
    for flash in &picture.flashes {
        let nearest = sound
            .beeps
            .iter()
            .map(|beep| beep - flash)
            .min_by(|one, other| one.abs().total_cmp(&other.abs()));
        if let Some(offset) = nearest.filter(|offset| offset.abs() <= FARTHEST_PAIR_SECONDS) {
            println!(
                "  flash at {flash:8.3} s, sound {:+7.1} ms from it",
                offset * 1e3
            );
            offsets.push(offset * 1e3);
        }
    }
    if offsets.is_empty() {
        println!("  no flash had a sound within {FARTHEST_PAIR_SECONDS} s");
        return;
    }
    let mean = offsets.iter().sum::<f64>() / count(offsets.len());
    let worst = offsets
        .iter()
        .fold(0.0_f64, |worst, offset| worst.max(offset.abs()));
    println!(
        "  {} pairs: sound is on average {mean:+.1} ms from the picture, at worst {worst:.1} ms",
        offsets.len()
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let path = arguments
        .next()
        .ok_or("usage: inspect FILE [--sync TRACK]")?;
    let sync_track: Option<usize> = match arguments.next().as_deref() {
        Some("--sync") => Some(arguments.next().ok_or("--sync needs a track")?.parse()?),
        _ => None,
    };
    let path = Path::new(&path);

    let mut demuxer = Demuxer::open(path)?;
    let streams = demuxer.streams().to_vec();
    let video_stream = streams.first().ok_or("the file has no streams")?;
    let options = VideoDecoderOptions {
        output: PixelFormat::Nv12,
        hardware: true,
    };
    let mut video_decoder = VideoDecoder::open(video_stream, options)?;
    let mut audio_decoders = streams[1..]
        .iter()
        .map(AudioDecoder::open)
        .collect::<Result<Vec<_>, _>>()?;
    let mut picture = Picture::default();
    let mut sounds: Vec<Sound> = audio_decoders.iter().map(|_| Sound::default()).collect();

    let see = |frames: Vec<VideoFrame>, picture: &mut Picture| {
        for frame in frames {
            let time = seconds(video_stream, frame.pts.unwrap_or(0));
            picture.take(time, mean_luma(&frame));
        }
    };
    while let Some(packet) = demuxer.read()? {
        if packet.stream == 0 {
            see(video_decoder.decode(&packet)?, &mut picture);
        } else if let Some(decoder) = audio_decoders.get_mut(packet.stream - 1) {
            let stream = &streams[packet.stream];
            for frame in decoder.decode(&packet)? {
                let starts = seconds(stream, frame.pts.unwrap_or(0));
                sounds[packet.stream - 1].take(
                    starts,
                    frame.sample_rate,
                    frame.channels,
                    &frame.samples,
                );
            }
        }
    }
    see(video_decoder.finish()?, &mut picture);

    let parameters = &video_stream.parameters;
    println!(
        "video: {:?} {}x{}, {} frames from {:.3} s to {:.3} s, {:.2} frames per second, longest step {:.1} ms",
        parameters.codec(),
        parameters.width(),
        parameters.height(),
        picture.frames,
        picture.first.unwrap_or(0.0),
        picture.last,
        count(picture.frames.saturating_sub(1))
            / (picture.last - picture.first.unwrap_or(0.0)).max(1e-9),
        picture.longest_step * 1e3
    );
    for (track, (sound, stream)) in sounds.iter().zip(&streams[1..]).enumerate() {
        let rate = stream.parameters.sample_rate();
        println!(
            "audio {track}: {:?} {} Hz {} ch, {:.3} s from {:.3} s, level {:.1} dBFS, peak {:.3}",
            stream.parameters.codec(),
            rate,
            stream.parameters.channels(),
            count(sound.samples) / f64::from(rate.max(1)),
            sound.first.unwrap_or(0.0),
            sound.rms_decibels(),
            sound.peak
        );
    }
    if let Some(micros) = demuxer.duration_micros() {
        println!("container: {:.3} s", real(micros) / 1e6);
    }
    let markers = read_markers(path)?;
    println!("markers: {markers:?}");
    if let Some(track) = sync_track {
        let sound = sounds.get(track).ok_or("there is no such audio track")?;
        println!(
            "sync: {} flashes, {} sounds on audio {track}",
            picture.flashes.len(),
            sound.beeps.len()
        );
        print_sync(&picture, sound);
    }
    Ok(())
}
