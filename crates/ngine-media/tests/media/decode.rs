use ngine_media::{
    AudioCodec, AudioDecoder, Demuxer, PixelFormat, Rational, VideoDecoder, VideoDecoderOptions,
};

use crate::common::{
    HEIGHT, SAMPLE_RATE, SYSTEM_TONE, TempFile, WIDTH, decode_file, encode_source, gray_level,
    mean_gray, tone, write_recording,
};

const SECONDS: i64 = 3;

fn recording(name: &str) -> TempFile {
    let file = TempFile::new(name);
    let frames = usize::try_from(SECONDS).unwrap() * SAMPLE_RATE;
    let source = encode_source(
        SECONDS,
        30,
        AudioCodec::Aac,
        &[tone(SYSTEM_TONE, 0, frames)],
    );
    write_recording(file.path(), &source);
    file
}

#[test]
fn seeking_lands_on_the_keyframe_before_the_target_and_decoding_resumes_there() {
    let file = recording("seek.mp4");
    let mut demuxer = Demuxer::open(file.path()).unwrap();
    let stream = demuxer.streams()[0].clone();
    let options = VideoDecoderOptions {
        output: PixelFormat::Bgra,
        hardware: false,
    };
    let mut decoder = VideoDecoder::open(&stream, options).unwrap();

    let first = demuxer.read().unwrap().unwrap();
    assert!(decoder.decode(&first).is_ok());

    demuxer.seek(1_700_000).unwrap();
    decoder.reset();
    let mut pictures = Vec::new();
    for packet in demuxer.packets() {
        let packet = packet.unwrap();
        if packet.stream != 0 {
            continue;
        }
        if pictures.is_empty() {
            assert!(packet.keyframe);
            let micros = stream
                .time_base
                .rescale(packet.pts.unwrap(), Rational::MICROS);
            assert_eq!(micros, Some(1_000_000));
        }
        pictures.extend(decoder.decode(&packet).unwrap());
    }
    pictures.extend(decoder.finish().unwrap());

    assert_eq!(pictures.len(), 60);
    let level = mean_gray(&pictures[0]);
    assert!((level - f64::from(gray_level(30))).abs() < 6.0, "{level}");
}

#[test]
fn hardware_decoding_gives_the_same_pictures_as_software_decoding() {
    let file = recording("hardware-decode.mp4");
    let software = decode_file(file.path(), PixelFormat::Nv12, false);
    let hardware = decode_file(file.path(), PixelFormat::Nv12, true);

    assert_eq!(software.video.len(), 90);
    assert_eq!(hardware.video.len(), 90);
    assert_eq!(software.video_micros(), hardware.video_micros());
    for (index, (software, hardware)) in software.video.iter().zip(&hardware.video).enumerate() {
        assert_eq!((hardware.width, hardware.height), (WIDTH, HEIGHT));
        assert_eq!(hardware.planes.len(), 2);
        assert_eq!(hardware.planes[0].data.len(), (WIDTH * HEIGHT) as usize);
        assert_eq!(hardware.planes[1].data.len(), (WIDTH * HEIGHT / 2) as usize);
        assert!(hardware.view().is_ok());
        let difference = (mean_gray(software) - mean_gray(hardware)).abs();
        assert!(difference < 3.0, "frame {index} differs by {difference}");
    }
}

#[test]
fn a_decoder_refuses_a_stream_of_the_other_kind() {
    let file = recording("wrong-kind.mp4");
    let demuxer = Demuxer::open(file.path()).unwrap();
    let options = VideoDecoderOptions {
        output: PixelFormat::Bgra,
        hardware: false,
    };
    assert!(VideoDecoder::open(&demuxer.streams()[1], options).is_err());
    assert!(AudioDecoder::open(&demuxer.streams()[0]).is_err());
    assert!(Demuxer::open(TempFile::new("missing.mp4").path()).is_err());
}

#[test]
#[ignore = "needs a real recording: set NGINE_REAL_RECORDING to its path and NGINE_REAL_SIZE to WxH"]
fn a_real_recording_decodes_at_the_size_it_was_recorded_at() {
    let path = std::path::PathBuf::from(std::env::var_os("NGINE_REAL_RECORDING").unwrap());
    let wanted = std::env::var("NGINE_REAL_SIZE").unwrap();
    let decoded = decode_file(&path, PixelFormat::Bgra, false);
    assert_ne!(decoded.video.len(), 0);
    for picture in &decoded.video {
        assert_eq!(format!("{}x{}", picture.width, picture.height), wanted);
    }
    println!("{} pictures of {wanted}", decoded.video.len());
}
