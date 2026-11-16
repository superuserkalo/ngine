use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ngine_capture::{Capturer, PickerOptions, Target};
use ngine_recorder::replay::{BackendChoice, ReplayLength};
use ngine_recorder::{
    AudioCodec, AudioSettings, CaptureSetup, EncoderInfo, ProbedEncoder, Quality, Recorder,
    RecorderConfig, RecorderEvent, ReplaySetup, Status, SurfaceKind, Tracks, Trial, VideoCodec,
    VideoSettings, probe_video_encoders,
};

use crate::app;

pub const USAGE: &str = "\
Checking this computer:
  ngine doctor [options]    Say which capture backend and encoders work here,
                            then capture for a few seconds and say whether
                            frames stay on the GPU all the way to the encoder

Options for doctor:
  --codec h264|hevc|av1     The codec of the test capture (default: h264)
  --encoder NAME            Test this FFmpeg encoder instead of the preferred
                            one
  --seconds N               How long the test capture runs (default: 3)
  --no-capture              Only list the backend and the encoders";

const POLL: Duration = Duration::from_millis(50);
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(15);
const PICKER_WAIT: Duration = Duration::from_mins(2);

struct Request {
    codec: VideoCodec,
    encoder: Option<String>,
    seconds: u64,
    capture: bool,
}

impl Request {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut request = Self {
            codec: VideoCodec::H264,
            encoder: None,
            seconds: 3,
            capture: true,
        };
        let mut arguments = arguments.iter();
        while let Some(argument) = arguments.next() {
            let mut value = || {
                arguments
                    .next()
                    .map(String::as_str)
                    .ok_or_else(|| format!("{argument} needs a value"))
            };
            match argument.as_str() {
                "--codec" => {
                    request.codec = match value()? {
                        "h264" => VideoCodec::H264,
                        "hevc" => VideoCodec::Hevc,
                        "av1" => VideoCodec::Av1,
                        other => return Err(format!("\"{other}\" is not h264, hevc or av1")),
                    };
                }
                "--encoder" => request.encoder = Some(value()?.to_owned()),
                "--seconds" => {
                    let text = value()?;
                    request.seconds = text
                        .parse()
                        .ok()
                        .filter(|seconds| *seconds > 0)
                        .ok_or_else(|| format!("\"{text}\" is not a number of seconds"))?;
                }
                "--no-capture" => request.capture = false,
                other => return Err(format!("ngine does not know the option \"{other}\"")),
            }
        }
        Ok(request)
    }
}

fn encoder_lines(probed: &[ProbedEncoder]) -> Vec<String> {
    probed
        .iter()
        .filter(|probed| probed.trial != Trial::NotInThisBuild)
        .map(|probed| {
            let kind = if probed.encoder.hardware {
                "hardware"
            } else {
                "software"
            };
            match &probed.trial {
                Trial::Failed(error) => format!("  {:<18} FAILED: {error}", probed.encoder.name),
                _ => format!("  {:<18} works ({kind})", probed.encoder.name),
            }
        })
        .collect()
}

fn working(probed: &[ProbedEncoder]) -> Vec<EncoderInfo> {
    probed
        .iter()
        .filter(|probed| probed.trial == Trial::Works)
        .map(|probed| probed.encoder.clone())
        .collect()
}

fn preferred_lines(working: &[EncoderInfo]) -> Vec<String> {
    VideoCodec::ALL
        .into_iter()
        .map(
            |codec| match working.iter().find(|encoder| encoder.codec == codec) {
                Some(encoder) if encoder.hardware => {
                    format!("  {codec:?}: {} (hardware)", encoder.name)
                }
                Some(encoder) => format!("  {codec:?}: {} (software)", encoder.name),
                None => format!("  {codec:?}: no working encoder"),
            },
        )
        .collect()
}

fn surface_line(surface: Option<SurfaceKind>) -> &'static str {
    match surface {
        Some(SurfaceKind::CoreVideo) => "GPU surfaces (CVPixelBuffer)",
        Some(SurfaceKind::D3d11) => "GPU surfaces (Direct3D 11 textures)",
        Some(SurfaceKind::DmaBuf) => "GPU surfaces (DMA-BUF)",
        Some(SurfaceKind::Cpu) => "pixels in ordinary memory",
        None => "nothing: no frame arrived",
    }
}

fn capture_lines(status: &Status) -> Vec<String> {
    let encoder = status.encoder.as_deref().unwrap_or("none");
    let path = if status.zero_copy {
        "zero-copy: the encoder reads the GPU surface"
    } else {
        "CPU copy: each frame passes through ordinary memory"
    };
    vec![
        format!(
            "  frames arrive as: {}",
            surface_line(status.frames_arrive_as)
        ),
        format!("  encoder in use:   {encoder}"),
        format!("  path:             {path}"),
        format!(
            "  frames:           {} captured, {} repeated",
            status.frames_captured, status.frames_repeated
        ),
    ]
}

fn picked_target(capturer: &Capturer) -> Result<Target, String> {
    let picker = capturer
        .picker()
        .ok_or("this system can neither list nor pick what to capture")?;
    println!("Choose a screen in the system's dialog.");
    let (sender, choice) = mpsc::channel();
    picker.pick(PickerOptions::default(), move |outcome| {
        let _doctor_gave_up_waiting = sender.send(outcome);
    });
    match choice.recv_timeout(PICKER_WAIT) {
        Ok(Ok(Some(chosen))) => Ok(chosen.target),
        Ok(Ok(None)) => Err("nothing was chosen".into()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("nothing was chosen in time".into()),
    }
}

fn target(capturer: &Capturer) -> Result<Target, String> {
    let listed = capturer
        .lister()
        .map(|lister| lister.targets().map_err(|error| error.to_string()));
    match listed {
        Some(targets) => targets?
            .screens
            .into_iter()
            .next()
            .map(Target::from)
            .ok_or_else(|| "there is no screen to capture".to_owned()),
        None => picked_target(capturer),
    }
}

fn test_capture(
    capturer: Capturer,
    request: &Request,
    encoders: Vec<EncoderInfo>,
) -> Result<Status, String> {
    let target = target(&capturer)?;
    println!(
        "Test capture of {} for {} s with {:?}:",
        target.label(),
        request.seconds,
        request.codec
    );
    let config = RecorderConfig {
        video: VideoSettings {
            codec: request.codec,
            quality: Quality::High,
            frame_rate: 60,
            encoder: request.encoder.clone(),
        },
        audio: AudioSettings {
            system_gain: None,
            microphone_gain: None,
            tracks: Tracks::MixOnly,
            codec: AudioCodec::Aac,
        },
        encoders,
    };
    let setup = CaptureSetup {
        target,
        show_cursor: true,
        app_sound_only: false,
    };
    let recorder =
        Recorder::start(Arc::new(capturer), setup, &config).map_err(|error| error.to_string())?;
    recorder
        .set_replay(Some(ReplaySetup {
            length: ReplayLength::SHORTEST,
            backend: BackendChoice::Memory,
            disk_folder: None,
            clear_after_save: false,
        }))
        .map_err(|error| error.to_string())?;
    let asked = Instant::now();
    let mut first_frame: Option<Instant> = None;
    let mut failure = None;
    loop {
        if first_frame.is_none() && recorder.status().frames_captured > 0 {
            first_frame = Some(Instant::now());
        }
        let done = match first_frame {
            Some(started) => started.elapsed() >= Duration::from_secs(request.seconds),
            None => asked.elapsed() >= FIRST_FRAME_WAIT,
        };
        if done || failure.is_some() {
            break;
        }
        match recorder.wait_event(POLL) {
            Some(RecorderEvent::CaptureLost(error)) => {
                failure = Some(error.map_or_else(
                    || "capture was ended from outside".to_owned(),
                    |error| format!("capture was lost: {error}"),
                ));
            }
            Some(RecorderEvent::Failed(error)) => failure = Some(error.to_string()),
            Some(RecorderEvent::CaptureInterrupted(error)) => {
                println!("  capture was interrupted: {error}");
            }
            Some(RecorderEvent::Stopped) => failure = Some("the recorder stopped".to_owned()),
            _ => {}
        }
    }
    let status = recorder.status();
    drop(recorder.stop());
    match failure {
        Some(failure) => Err(failure),
        None => Ok(status),
    }
}

pub fn run(arguments: &[String]) -> Result<(), String> {
    let request = Request::parse(arguments).map_err(|error| format!("{error}\n\n{USAGE}"))?;
    println!(
        "{} {} on {} ({})",
        app::NAME,
        app::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let capturer = Capturer::system();
    match &capturer {
        Ok(capturer) => println!("Capture backend: {}", capturer.backend_name()),
        Err(error) => println!("Capture backend: none ({error})"),
    }
    println!("Encoders in this build, in the order ngine prefers them:");
    let probed = probe_video_encoders();
    for line in encoder_lines(&probed) {
        println!("{line}");
    }
    let working = working(&probed);
    println!("Used when nothing else is asked for:");
    for line in preferred_lines(&working) {
        println!("{line}");
    }
    if !request.capture {
        return Ok(());
    }
    let status = test_capture(
        capturer.map_err(|error| error.to_string())?,
        &request,
        working,
    )?;
    for line in capture_lines(&status) {
        println!("{line}");
    }
    if status.frames_captured == 0 {
        return Err("no frame arrived, so the path to the encoder was not tested".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ngine_recorder::MediaError;

    use super::*;

    fn probed(name: &str, codec: VideoCodec, hardware: bool, trial: Trial) -> ProbedEncoder {
        ProbedEncoder {
            encoder: EncoderInfo {
                name: name.to_owned(),
                codec,
                hardware,
            },
            trial,
        }
    }

    fn a_linux_pc_without_nvidia() -> Vec<ProbedEncoder> {
        let no_device = Trial::Failed(MediaError::Unavailable(
            "h264_nvenc has no device".to_owned(),
        ));
        vec![
            probed(
                "h264_videotoolbox",
                VideoCodec::H264,
                true,
                Trial::NotInThisBuild,
            ),
            probed("h264_vaapi", VideoCodec::H264, true, Trial::Works),
            probed("h264_nvenc", VideoCodec::H264, true, no_device),
            probed("libopenh264", VideoCodec::H264, false, Trial::Works),
            probed("hevc_vaapi", VideoCodec::Hevc, true, Trial::Works),
        ]
    }

    #[test]
    fn encoders_of_other_systems_are_left_out_and_failures_keep_their_reason() {
        let lines = encoder_lines(&a_linux_pc_without_nvidia());

        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("h264_vaapi") && lines[0].ends_with("works (hardware)"));
        assert!(lines[1].contains("h264_nvenc") && lines[1].contains("FAILED"));
        assert!(lines[1].contains("has no device"));
        assert!(lines[2].ends_with("works (software)"));
    }

    #[test]
    fn the_first_working_encoder_of_each_codec_is_the_one_used() {
        let lines = preferred_lines(&working(&a_linux_pc_without_nvidia()));

        assert_eq!(
            lines,
            [
                "  H264: h264_vaapi (hardware)",
                "  Hevc: hevc_vaapi (hardware)",
                "  Av1: no working encoder"
            ]
        );
    }

    #[test]
    fn the_capture_report_says_where_frames_live_and_whether_they_are_copied() {
        let zero_copy = Status {
            encoder: Some("h264_vaapi".into()),
            zero_copy: true,
            frames_captured: 180,
            frames_arrive_as: Some(SurfaceKind::DmaBuf),
            ..Status::default()
        };
        let copied = Status {
            zero_copy: false,
            frames_arrive_as: Some(SurfaceKind::Cpu),
            ..zero_copy.clone()
        };

        let lines = capture_lines(&zero_copy);
        assert!(lines[0].ends_with("GPU surfaces (DMA-BUF)"));
        assert!(lines[1].ends_with("h264_vaapi"));
        assert!(lines[2].contains("zero-copy"));
        assert!(lines[3].contains("180 captured"));
        let lines = capture_lines(&copied);
        assert!(lines[0].ends_with("pixels in ordinary memory"));
        assert!(lines[2].contains("CPU copy"));
    }

    #[test]
    fn the_options_parse_and_mistakes_are_named() {
        let arguments =
            |line: &str| -> Vec<String> { line.split_whitespace().map(str::to_owned).collect() };

        let request =
            Request::parse(&arguments("--codec hevc --encoder hevc_nvenc --seconds 5")).unwrap();
        assert_eq!(request.codec, VideoCodec::Hevc);
        assert_eq!(request.encoder.as_deref(), Some("hevc_nvenc"));
        assert_eq!(request.seconds, 5);
        assert!(request.capture);
        assert!(!Request::parse(&arguments("--no-capture")).unwrap().capture);
        assert!(Request::parse(&arguments("--codec vp9")).is_err());
        assert!(Request::parse(&arguments("--seconds 0")).is_err());
        assert!(Request::parse(&arguments("--encoder")).is_err());
        assert!(Request::parse(&arguments("--loud")).is_err());
    }
}
