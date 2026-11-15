ffmpeg_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

die() {
  echo "ffmpeg: $*" >&2
  exit 1
}

case "$(uname -s)" in
  Darwin)
    [ "$(uname -m)" = arm64 ] || die "macOS builds target Apple silicon only"
    os=macos
    triple=aarch64-apple-darwin
    ;;
  Linux)
    [ "$(uname -m)" = x86_64 ] || die "Linux builds target x86_64 only"
    os=linux
    triple=x86_64-unknown-linux-gnu
    ;;
  MINGW*|MSYS*)
    os=windows
    triple=x86_64-pc-windows-msvc
    ;;
  *)
    die "unsupported system $(uname -s)"
    ;;
esac

ffmpeg_dir=${NGINE_FFMPEG_DIR:-$ffmpeg_root/target/ffmpeg/$triple}
[ "$os" != windows ] || ffmpeg_dir=$(cygpath -u "$ffmpeg_dir")
work_dir=$ffmpeg_root/target/ffmpeg/work/$triple
size_limit_bytes=10000000
macos_deployment_target=15.0

case $os in
  macos)
    required_encoders="h264_videotoolbox hevc_videotoolbox libopenh264 aac aac_at libopus"
    ;;
  linux)
    required_encoders="h264_nvenc hevc_nvenc av1_nvenc h264_vaapi hevc_vaapi av1_vaapi h264_vulkan hevc_vulkan av1_vulkan libopenh264 aac libopus"
    ;;
  windows)
    required_encoders="h264_nvenc hevc_nvenc av1_nvenc h264_amf hevc_amf av1_amf h264_qsv hevc_qsv av1_qsv h264_mf hevc_mf av1_mf h264_d3d12va hevc_d3d12va av1_d3d12va libopenh264 aac libopus"
    ;;
esac

native_path() {
  if [ "$os" = windows ]; then
    cygpath -m "$1"
  else
    printf '%s\n' "$1"
  fi
}

shared_libraries() {
  case $os in
    macos) find "$ffmpeg_dir/lib" -maxdepth 1 -type f -name '*.dylib' | sort ;;
    linux) find "$ffmpeg_dir/lib" -maxdepth 1 -type f -name '*.so*' | sort ;;
    windows) find "$ffmpeg_dir/bin" -maxdepth 1 -type f -name '*.dll' | sort ;;
  esac
}

file_size() {
  wc -c < "$1" | tr -d ' '
}

parallel_jobs() {
  if [ -n "${NUMBER_OF_PROCESSORS:-}" ]; then
    echo "$NUMBER_OF_PROCESSORS"
  else
    getconf _NPROCESSORS_ONLN
  fi
}
