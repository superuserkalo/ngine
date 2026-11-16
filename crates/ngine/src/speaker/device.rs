use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use ngine_recorder::SoundFeed;

const FEED_RATE: f32 = 48_000.0;
const CHUNK_SAMPLES: usize = 512;

pub struct Speaker {
    _stream: Stream,
}

struct Source {
    feed: SoundFeed,
    chunk: [f32; CHUNK_SAMPLES],
    at: usize,
    earlier: [f32; 2],
    later: [f32; 2],
    between: f32,
    step: f32,
    same_rate: bool,
}

impl Source {
    fn feed_frame(&mut self) -> [f32; 2] {
        if self.at >= CHUNK_SAMPLES {
            self.feed.fill(&mut self.chunk);
            self.at = 0;
        }
        let frame = [self.chunk[self.at], self.chunk[self.at + 1]];
        self.at += 2;
        frame
    }

    fn device_frame(&mut self) -> [f32; 2] {
        if self.same_rate {
            return self.feed_frame();
        }
        self.between += self.step;
        while self.between >= 1.0 {
            self.between -= 1.0;
            self.earlier = self.later;
            self.later = self.feed_frame();
        }
        let blend = |channel: usize| {
            self.earlier[channel] + (self.later[channel] - self.earlier[channel]) * self.between
        };
        [blend(0), blend(1)]
    }

    fn write(&mut self, out: &mut [f32], channels: usize) {
        for frame in out.chunks_mut(channels.max(1)) {
            let [left, right] = self.device_frame();
            match frame {
                [only] => *only = f32::midpoint(left, right),
                [first, second, others @ ..] => {
                    *first = left;
                    *second = right;
                    others.fill(0.0);
                }
                [] => {}
            }
        }
    }
}

impl Speaker {
    pub fn open(feed: SoundFeed) -> Option<Self> {
        let device = cpal::default_host().default_output_device()?;
        let supported = device.default_output_config().ok()?;
        if supported.sample_format() != SampleFormat::F32 {
            return None;
        }
        let config = supported.config();
        let channels = usize::from(config.channels);
        let rate = f32::from(u16::try_from(config.sample_rate / 10).ok()?) * 10.0;
        let mut source = Source {
            feed,
            chunk: [0.0; CHUNK_SAMPLES],
            at: CHUNK_SAMPLES,
            earlier: [0.0; 2],
            later: [0.0; 2],
            between: 0.0,
            step: FEED_RATE / rate.max(1.0),
            same_rate: config.sample_rate == 48_000,
        };
        let stream = device
            .build_output_stream(
                config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| source.write(out, channels),
                |error| eprintln!("ngine: the sound output had trouble: {error}"),
                None,
            )
            .ok()?;
        stream.play().ok()?;
        Some(Self { _stream: stream })
    }
}
