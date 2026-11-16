/// Where a captured frame lives when it arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SurfaceKind {
    /// A `CVPixelBuffer` from `ScreenCaptureKit`.
    CoreVideo,
    /// A Direct3D 11 texture from Windows.Graphics.Capture.
    D3d11,
    /// A DMA-BUF from `PipeWire`.
    DmaBuf,
    /// Pixels in ordinary memory.
    Cpu,
}

/// What is known about a frame before an encoder is opened for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrival {
    pub surface: SurfaceKind,
    /// Four bytes per pixel in blue, green, red, alpha order.
    pub is_bgra: bool,
    /// The encoders take even sizes only, and a surface cannot be cropped
    /// without a copy.
    pub has_even_size: bool,
}

/// How frames reach the encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    /// The encoder reads the GPU surface, converted on the GPU where it has
    /// to be. No pixel passes through ordinary memory.
    GpuSurface,
    /// The frame is copied to ordinary memory and the encoder, or the
    /// upload in front of it, takes it from there.
    CpuPixels,
}

fn family(encoder: &str) -> &str {
    encoder.rsplit_once('_').map_or("", |(_, family)| family)
}

fn reads(encoder: &str, arrival: Arrival) -> bool {
    if !arrival.has_even_size {
        return false;
    }
    match (arrival.surface, family(encoder)) {
        (SurfaceKind::CoreVideo, "videotoolbox") | (SurfaceKind::DmaBuf, "vaapi") => true,
        (SurfaceKind::D3d11, "nvenc" | "amf" | "mf") => arrival.is_bgra,
        _ => false,
    }
}

/// The routes to try for `encoder`, best first. The last one is always
/// [`Route::CpuPixels`], which every encoder takes, so a route that fails
/// has a successor until that one fails too.
pub fn routes(encoder: &str, arrival: Arrival) -> Vec<Route> {
    if reads(encoder, arrival) {
        vec![Route::GpuSurface, Route::CpuPixels]
    } else {
        vec![Route::CpuPixels]
    }
}

/// Whether capture should hand `encoder` DMA-BUFs on Linux. They cannot be
/// copied to ordinary memory, so they are asked for only when the encoder
/// can import them.
pub fn imports_dma_bufs(encoder: &str) -> bool {
    family(encoder) == "vaapi"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrival(surface: SurfaceKind) -> Arrival {
        Arrival {
            surface,
            is_bgra: true,
            has_even_size: true,
        }
    }

    const BOTH: [Route; 2] = [Route::GpuSurface, Route::CpuPixels];
    const COPY: [Route; 1] = [Route::CpuPixels];

    #[test]
    fn each_system_has_encoders_that_read_its_surfaces_and_falls_back_to_pixels() {
        for (encoder, surface) in [
            ("h264_videotoolbox", SurfaceKind::CoreVideo),
            ("hevc_videotoolbox", SurfaceKind::CoreVideo),
            ("h264_vaapi", SurfaceKind::DmaBuf),
            ("av1_vaapi", SurfaceKind::DmaBuf),
            ("h264_nvenc", SurfaceKind::D3d11),
            ("hevc_amf", SurfaceKind::D3d11),
            ("av1_mf", SurfaceKind::D3d11),
        ] {
            assert_eq!(routes(encoder, arrival(surface)), BOTH, "{encoder}");
        }
    }

    #[test]
    fn an_encoder_that_cannot_read_the_surface_gets_pixels_only() {
        for (encoder, surface) in [
            ("h264_qsv", SurfaceKind::D3d11),
            ("h264_nvenc", SurfaceKind::DmaBuf),
            ("h264_vaapi", SurfaceKind::D3d11),
            ("h264_nvenc", SurfaceKind::CoreVideo),
            ("libopenh264", SurfaceKind::D3d11),
            ("libopenh264", SurfaceKind::DmaBuf),
            ("libopenh264", SurfaceKind::CoreVideo),
            ("h264_videotoolbox", SurfaceKind::Cpu),
            ("h264_vaapi", SurfaceKind::Cpu),
            ("h264_amf", SurfaceKind::Cpu),
        ] {
            assert_eq!(routes(encoder, arrival(surface)), COPY, "{encoder}");
        }
    }

    #[test]
    fn an_odd_size_or_a_texture_that_is_not_bgra_is_copied() {
        let odd = Arrival {
            has_even_size: false,
            ..arrival(SurfaceKind::CoreVideo)
        };
        let rgba = Arrival {
            is_bgra: false,
            ..arrival(SurfaceKind::D3d11)
        };
        let nv12_on_macos = Arrival {
            is_bgra: false,
            ..arrival(SurfaceKind::CoreVideo)
        };

        assert_eq!(routes("h264_videotoolbox", odd), COPY);
        assert_eq!(routes("h264_nvenc", rgba), COPY);
        assert_eq!(routes("h264_videotoolbox", nv12_on_macos), BOTH);
    }

    #[test]
    fn every_list_of_routes_ends_with_pixels() {
        for encoder in ["h264_vaapi", "h264_nvenc", "libopenh264", "", "odd_name_"] {
            for surface in [
                SurfaceKind::CoreVideo,
                SurfaceKind::D3d11,
                SurfaceKind::DmaBuf,
                SurfaceKind::Cpu,
            ] {
                assert_eq!(
                    routes(encoder, arrival(surface)).last(),
                    Some(&Route::CpuPixels)
                );
            }
        }
    }

    #[test]
    fn only_the_va_api_encoders_ask_capture_for_dma_bufs() {
        assert!(imports_dma_bufs("h264_vaapi"));
        assert!(imports_dma_bufs("av1_vaapi"));
        assert!(!imports_dma_bufs("h264_nvenc"));
        assert!(!imports_dma_bufs("libopenh264"));
    }
}
