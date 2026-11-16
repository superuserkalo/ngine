use ngine_media::{
    PixelFormat, VideoCodec, VideoEncoder, VideoSource, has_encoder, try_video_encoder,
    working_video_codecs, working_video_encoders,
};

use crate::common::{encode_gray_frame, h264_encoder, video_settings};

#[test]
fn the_probe_reports_only_encoders_that_really_encode() {
    let encoders = working_video_encoders();
    println!("working encoders: {encoders:?}");
    for encoder in &encoders {
        assert!(has_encoder(&encoder.name));
        let mut opened = VideoEncoder::open(
            &video_settings(&encoder.name, 30),
            VideoSource::Cpu(PixelFormat::Bgra),
        )
        .unwrap();
        let mut packets = encode_gray_frame(&mut opened, 0);
        packets.extend(opened.finish().unwrap());
        assert!(packets[0].keyframe, "{} starts on a keyframe", encoder.name);
        assert_eq!(opened.codec(), encoder.codec);
    }

    let codecs = working_video_codecs(&encoders);
    assert_eq!(
        codecs.contains(&VideoCodec::H264),
        encoders
            .iter()
            .any(|encoder| encoder.codec == VideoCodec::H264)
    );
    let hardware_after_software = encoders
        .windows(2)
        .any(|pair| pair[0].codec == pair[1].codec && !pair[0].hardware && pair[1].hardware);
    assert!(!hardware_after_software);
}

#[test]
fn the_encoder_the_tests_use_passes_the_probe() {
    try_video_encoder(h264_encoder()).unwrap();
}

#[test]
fn an_encoder_this_build_lacks_is_reported_as_unavailable() {
    assert!(!has_encoder("no_such_encoder"));
    assert!(matches!(
        try_video_encoder("no_such_encoder"),
        Err(ngine_media::Error::Unavailable(_))
    ));
}
