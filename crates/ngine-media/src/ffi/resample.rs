use std::ffi::{CStr, c_int};
use std::ptr::{self, NonNull};

use super::{ChannelLayout, Frame, check, out_of_memory, sys, to_int};
use crate::error::{Error, Result};

pub const OUTPUT_SAMPLE_RATE: u32 = 48_000;
pub const OUTPUT_CHANNELS: u16 = 2;
const MAX_SAMPLE_RATE: u32 = 384_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioLayout {
    pub sample_rate: u32,
    pub channels: u16,
}

pub(super) struct Converter {
    context: NonNull<sys::SwrContext>,
    output_channels: usize,
}

// SAFETY: a resampling context has no thread affinity and is used through
// `&mut self` only.
unsafe impl Send for Converter {}

impl Converter {
    pub(super) fn new(
        input_layout: *const sys::AVChannelLayout,
        input_format: sys::AVSampleFormat,
        input_rate: c_int,
        output: AudioLayout,
        options: &[(&CStr, &CStr)],
    ) -> Result<Self> {
        let output_layout = ChannelLayout::default_for(output.channels)?;
        let mut context = ptr::null_mut();
        // SAFETY: `context` is a live out-pointer holding null, the output
        // layout is valid, and callers pass a valid input layout.
        let status = unsafe {
            sys::swr_alloc_set_opts2(
                &raw mut context,
                output_layout.as_ptr(),
                sys::AV_SAMPLE_FMT_FLT,
                to_int(output.sample_rate, "the sample rate")?,
                input_layout,
                input_format,
                input_rate,
                0,
                ptr::null_mut(),
            )
        };
        check(status, "configure the resampler")?;
        let context = NonNull::new(context).ok_or_else(|| out_of_memory("allocate a resampler"))?;
        let converter = Self {
            context,
            output_channels: usize::from(output.channels),
        };
        for (name, value) in options {
            // SAFETY: the context is valid and both strings are
            // NUL-terminated.
            let status = unsafe {
                sys::av_opt_set(context.as_ptr().cast(), name.as_ptr(), value.as_ptr(), 0)
            };
            check(status, "configure the resampler")?;
        }
        // SAFETY: the context has all of its parameters set.
        let status = unsafe { sys::swr_init(context.as_ptr()) };
        check(status, "start the resampler")?;
        Ok(converter)
    }

    fn announce_timestamp(&mut self, timestamp: i64) {
        // SAFETY: the context is initialised; swr_next_pts only adjusts its
        // compensation state.
        let _output_timestamp = unsafe { sys::swr_next_pts(self.context.as_ptr(), timestamp) };
    }

    fn convert_raw(&mut self, input: *const *const u8, input_frames: c_int) -> Result<Vec<f32>> {
        // SAFETY: the context is initialised; this only reads its state.
        let capacity = unsafe { sys::swr_get_out_samples(self.context.as_ptr(), input_frames) };
        let capacity = check(capacity, "size the resampler output")?;
        let frames = usize::try_from(capacity).unwrap_or(0);
        let mut output = vec![0.0_f32; frames * self.output_channels];
        let planes = [output.as_mut_ptr().cast::<u8>()];
        // SAFETY: the output is one packed float plane with room for
        // `capacity` frames; the callers guarantee that `input` is null with
        // zero frames, or holds one plane pointer per input plane with
        // `input_frames` frames in each.
        let written = unsafe {
            sys::swr_convert(
                self.context.as_ptr(),
                planes.as_ptr(),
                capacity,
                input,
                input_frames,
            )
        };
        let written = check(written, "resample sound")?;
        output.truncate(usize::try_from(written).unwrap_or(0) * self.output_channels);
        Ok(output)
    }

    fn convert_packed(&mut self, samples: &[f32], channels: usize) -> Result<Vec<f32>> {
        let frames = to_int(samples.len() / channels, "the sample count")?;
        let planes = [samples.as_ptr().cast::<u8>()];
        self.convert_raw(planes.as_ptr(), frames)
    }

    pub(super) fn convert_frame(&mut self, frame: &Frame) -> Result<Vec<f32>> {
        let raw = frame.raw();
        self.convert_raw(raw.extended_data.cast_const().cast(), raw.nb_samples)
    }

    pub(super) fn flush(&mut self) -> Result<Vec<f32>> {
        self.convert_raw(ptr::null(), 0)
    }
}

impl Drop for Converter {
    fn drop(&mut self) {
        let mut context = self.context.as_ptr();
        // SAFETY: allocated by swr_alloc_set_opts2 and not used again.
        unsafe { sys::swr_free(&raw mut context) }
    }
}

/// Turns interleaved float sound of any rate and channel count into 48 kHz
/// stereo.
pub struct Resampler {
    converter: Converter,
    input: AudioLayout,
    synchronized: bool,
}

impl Resampler {
    /// A plain converter: what goes in comes out, at the new rate.
    pub fn new(input: AudioLayout) -> Result<Self> {
        Self::build(input, false)
    }

    /// A converter that follows the capture clock. Its output is one unbroken
    /// stream whose first sample is at time zero of that clock: a late start
    /// or a gap becomes silence, and a source clock that runs fast or slow is
    /// stretched back by up to half a percent.
    pub fn synchronized(input: AudioLayout) -> Result<Self> {
        Self::build(input, true)
    }

    fn build(input: AudioLayout, synchronized: bool) -> Result<Self> {
        super::init();
        if input.sample_rate == 0 || input.sample_rate > MAX_SAMPLE_RATE {
            return Err(Error::invalid(format!(
                "a sample rate of {} is outside 1..={MAX_SAMPLE_RATE}",
                input.sample_rate
            )));
        }
        ChannelLayout::default_for(input.channels)?;
        let layout = ChannelLayout::default_for(input.channels.max(OUTPUT_CHANNELS))?;
        let stretch = format!("{}", input.sample_rate / 200);
        let stretch = super::c_string(&stretch)?;
        let timed: [(&CStr, &CStr); 3] = [
            (c"flags", c"res"),
            (c"first_pts", c"0"),
            (c"async", &stretch),
        ];
        let converter = Converter::new(
            layout.as_ptr(),
            sys::AV_SAMPLE_FMT_FLT,
            to_int(input.sample_rate, "the sample rate")?,
            AudioLayout {
                sample_rate: OUTPUT_SAMPLE_RATE,
                channels: OUTPUT_CHANNELS,
            },
            if synchronized { &timed } else { &[] },
        )?;
        Ok(Self {
            converter,
            input,
            synchronized,
        })
    }

    fn check_whole_frames(&self, samples: &[f32]) -> Result<()> {
        if samples
            .len()
            .is_multiple_of(usize::from(self.input.channels))
        {
            Ok(())
        } else {
            Err(Error::invalid(
                "the sample count is not a multiple of the channel count",
            ))
        }
    }

    fn convert(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        if self.input.channels == 1 {
            let both_ears = samples
                .iter()
                .flat_map(|sample| [*sample, *sample])
                .collect::<Vec<_>>();
            self.converter
                .convert_packed(&both_ears, usize::from(OUTPUT_CHANNELS))
        } else {
            self.converter
                .convert_packed(samples, usize::from(self.input.channels))
        }
    }

    /// Converts the next samples of the stream, in order. Mono sound goes to
    /// both channels at full level.
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        self.check_whole_frames(samples)?;
        self.convert(samples)
    }

    /// Converts samples whose first frame was captured `start_micros` after
    /// time zero of the capture clock. Only for a synchronized resampler.
    pub fn push_at(&mut self, start_micros: i64, samples: &[f32]) -> Result<Vec<f32>> {
        if !self.synchronized {
            return Err(Error::invalid(
                "only a synchronized resampler takes capture times",
            ));
        }
        self.check_whole_frames(samples)?;
        let units_per_second = i128::from(self.input.sample_rate) * i128::from(OUTPUT_SAMPLE_RATE);
        let timestamp = i128::from(start_micros) * units_per_second / 1_000_000;
        let timestamp = i64::try_from(timestamp)
            .map_err(|_| Error::invalid("the capture time is out of range"))?;
        self.converter.announce_timestamp(timestamp);
        self.convert(samples)
    }

    /// Returns the samples still inside the resampler.
    pub fn finish(&mut self) -> Result<Vec<f32>> {
        self.converter.flush()
    }
}
