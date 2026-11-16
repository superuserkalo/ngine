use crate::frame::PixelFormat;
use crate::target::{AudioApp, Target};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SystemAudio {
    #[default]
    Off,
    /// Everything the computer plays, apart from this process.
    Everything,
    App(AudioApp),
}

impl SystemAudio {
    /// The sound that belongs to a target: its own app where the system knows
    /// it, everything otherwise.
    pub fn of(target: &Target) -> Self {
        target
            .audio_app()
            .map_or(Self::Everything, |app| Self::App(app.clone()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Microphone {
    #[default]
    Off,
    /// The input device the system currently prefers.
    SystemDefault,
    /// A [`MicrophoneDevice::id`](crate::MicrophoneDevice) from
    /// [`Capturer::microphones`](crate::Capturer::microphones).
    Device(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SessionOptions {
    pub show_cursor: bool,
    /// The most frames per second the session delivers.
    pub frame_rate: u32,
    /// A preference. A backend that cannot deliver it uses its own format, and
    /// each frame says which format it has.
    pub pixel_format: PixelFormat,
    pub system_audio: SystemAudio,
    pub microphone: Microphone,
    /// Ask for frames that stay on the GPU even where they then cannot be
    /// copied to ordinary memory. Only Linux has such frames: with this set,
    /// a compositor that can share a DMA-BUF delivers
    /// [`Surface::DmaBuf`](crate::Surface), and
    /// [`VideoFrame::to_cpu`](crate::VideoFrame::to_cpu) fails for it. Set
    /// it only when the encoder imports DMA-BUFs.
    pub gpu_surfaces: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            show_cursor: true,
            frame_rate: 60,
            pixel_format: PixelFormat::Bgra8,
            system_audio: SystemAudio::Off,
            microphone: Microphone::Off,
            gpu_surfaces: false,
        }
    }
}

impl SessionOptions {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_cursor(mut self, show_cursor: bool) -> Self {
        self.show_cursor = show_cursor;
        self
    }

    #[must_use]
    pub fn with_frame_rate(mut self, frame_rate: u32) -> Self {
        self.frame_rate = frame_rate;
        self
    }

    #[must_use]
    pub fn with_pixel_format(mut self, pixel_format: PixelFormat) -> Self {
        self.pixel_format = pixel_format;
        self
    }

    #[must_use]
    pub fn with_system_audio(mut self, system_audio: SystemAudio) -> Self {
        self.system_audio = system_audio;
        self
    }

    #[must_use]
    pub fn with_microphone(mut self, microphone: Microphone) -> Self {
        self.microphone = microphone;
        self
    }

    #[must_use]
    pub fn with_gpu_surfaces(mut self, gpu_surfaces: bool) -> Self {
        self.gpu_surfaces = gpu_surfaces;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Size;
    use crate::target::{App, Screen, ScreenId};

    #[test]
    fn the_sound_of_an_app_is_that_app_and_the_sound_of_a_screen_is_everything() {
        let app = Target::from(App::new(7, "Player"));
        let screen = Target::from(Screen::new(ScreenId(1), "Main", Size::new(1920, 1080)));

        assert_eq!(
            SystemAudio::of(&app),
            SystemAudio::App(AudioApp::new(7, "Player"))
        );
        assert_eq!(SystemAudio::of(&screen), SystemAudio::Everything);
    }
}
