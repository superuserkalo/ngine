use ngine_media::{
    GpuFramePool, PixelFormat, PixelView, PlaneView, VideoDecoder, VideoDecoderOptions,
    VideoEncoder, VideoInput, VideoSource, try_video_encoder,
};

use crate::common::{HEIGHT, WIDTH, frame_pts, gray_level, mean_gray, video_settings};

const ENCODERS: [&str; 2] = ["h264_videotoolbox", "hevc_videotoolbox"];
const FRAMES: i64 = 45;

fn encodes_pool_surfaces(encoder_name: &str) {
    if try_video_encoder(encoder_name).is_err() {
        eprintln!("skipped: {encoder_name} does not work on this machine");
        return;
    }
    let pool = GpuFramePool::new(WIDTH, HEIGHT, PixelFormat::Nv12).unwrap();
    let mut encoder =
        VideoEncoder::open(&video_settings(encoder_name, 30), VideoSource::Gpu(&pool)).unwrap();

    let chroma = vec![128_u8; (WIDTH * HEIGHT / 2) as usize];
    let mut packets = Vec::new();
    for frame in 0..FRAMES {
        let luma = vec![gray_level(frame); (WIDTH * HEIGHT) as usize];
        let stride = WIDTH as usize;
        let pixels = PixelView::nv12(
            WIDTH,
            HEIGHT,
            PlaneView {
                data: &luma,
                stride,
            },
            PlaneView {
                data: &chroma,
                stride,
            },
        )
        .unwrap();
        let surface = pool.upload(&pixels).unwrap();
        packets.extend(
            encoder
                .encode(VideoInput::Gpu(&surface), frame_pts(frame))
                .unwrap(),
        );
    }
    packets.extend(encoder.finish().unwrap());

    assert_eq!(packets.len(), usize::try_from(FRAMES).unwrap());
    let keyframes = packets
        .iter()
        .enumerate()
        .filter(|(_, packet)| packet.keyframe)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(keyframes, [0, 30]);

    let options = VideoDecoderOptions {
        output: PixelFormat::Nv12,
        hardware: false,
    };
    let mut decoder = VideoDecoder::open(encoder.stream_info(), options).unwrap();
    let mut pictures = Vec::new();
    for packet in &packets {
        pictures.extend(decoder.decode(packet).unwrap());
    }
    pictures.extend(decoder.finish().unwrap());
    assert_eq!(pictures.len(), packets.len());
    for (frame, picture) in pictures.iter().enumerate() {
        let expected = f64::from(gray_level(i64::try_from(frame).unwrap()));
        let level = mean_gray(picture);
        assert!(
            (level - expected).abs() < 6.0,
            "frame {frame} is {level}, not {expected}"
        );
    }
}

#[test]
fn videotoolbox_encodes_gpu_surfaces_that_decode_back_to_the_same_pictures() {
    for encoder in ENCODERS {
        encodes_pool_surfaces(encoder);
    }
}

#[test]
fn an_encoder_opened_for_gpu_surfaces_refuses_cpu_pixels_and_the_wrong_size() {
    if try_video_encoder(ENCODERS[0]).is_err() {
        eprintln!("skipped: {} does not work on this machine", ENCODERS[0]);
        return;
    }
    let pool = GpuFramePool::new(WIDTH, HEIGHT, PixelFormat::Nv12).unwrap();
    let small = GpuFramePool::new(WIDTH / 2, HEIGHT / 2, PixelFormat::Nv12).unwrap();
    let settings = video_settings(ENCODERS[0], 30);
    assert!(VideoEncoder::open(&settings, VideoSource::Gpu(&small)).is_err());

    let mut encoder = VideoEncoder::open(&settings, VideoSource::Gpu(&pool)).unwrap();
    let pixels = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];
    let view = PixelView::bgra(
        WIDTH,
        HEIGHT,
        PlaneView {
            data: &pixels,
            stride: (WIDTH * 4) as usize,
        },
    )
    .unwrap();
    assert!(encoder.encode(VideoInput::Cpu(&view), 0).is_err());
    assert!(pool.upload(&view).is_err());
}
