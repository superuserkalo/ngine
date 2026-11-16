#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ffmpeg/build.sh

Builds FFmpeg as trimmed LGPL shared libraries for this machine into
target/ffmpeg/<rust-target-triple>/, or into $NGINE_FFMPEG_DIR when it is set,
then checks them with ffmpeg/check.sh.

macOS (Apple silicon): Xcode command line tools, cmake, nasm, pkg-config.
Linux (x86_64):        gcc, g++, make, cmake, meson, ninja, nasm, pkg-config,
                       libdrm-dev.
Windows (x86_64):      an MSYS2 shell opened from an x64 Visual Studio developer
                       prompt with MSYS2_PATH_TYPE=inherit, the MSYS2 packages
                       make, nasm, pkgconf and diffutils, and CMake from Visual
                       Studio on the PATH.
EOF
}

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage >&2; exit 2 ;;
esac

. "$(dirname "$0")/platform.sh"

ffmpeg_version=9.0.2
ffmpeg_url=https://ffmpeg.org/releases/ffmpeg-$ffmpeg_version.tar.xz
ffmpeg_sha256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e

opus_version=1.5.2
opus_url=https://downloads.xiph.org/releases/opus/opus-$opus_version.tar.gz
opus_sha256=65c1d2f78b9f2fb20082c38cbe47c951ad5839345876e46941612ee87f9a7ce1

openh264_version=2.6.0
openh264_url=https://github.com/cisco/openh264/archive/refs/tags/v$openh264_version.tar.gz
openh264_sha256=558544ad358283a7ab2930d69a9ceddf913f4a51ee9bf1bfb9e377322af81a69

nvcodec_version=12.1.14.1
nvcodec_url=https://github.com/FFmpeg/nv-codec-headers/releases/download/n$nvcodec_version/nv-codec-headers-$nvcodec_version.tar.gz
nvcodec_sha256=043e46265517855f13bcb98b3a225b2761b2a83983b1645fa1d05876f1c3a781

amf_version=1.5.3
amf_url=https://github.com/GPUOpen-LibrariesAndSDKs/AMF/releases/download/v$amf_version/AMF-headers-v$amf_version.tar.gz
amf_sha256=0f521ad98041f8001147cded4a660127f18b1c27a801a5cd691bda1fa48381be

libvpl_version=2.17.0
libvpl_url=https://github.com/intel/libvpl/archive/refs/tags/v$libvpl_version.tar.gz
libvpl_sha256=4de3e2faf1e8307fb282e4a43f443191810f6a6b0a484fffa7995ba1c814c6ec

vulkan_version=1.4.335.0
vulkan_url=https://github.com/KhronosGroup/Vulkan-Headers/archive/refs/tags/vulkan-sdk-$vulkan_version.tar.gz
vulkan_sha256=269e95cc5138ea0a0d52fcb0ee19102add2560fedf5a43b1b5c17780c2775764

libva_version=2.24.1
libva_url=https://github.com/intel/libva/releases/download/$libva_version/libva-$libva_version.tar.bz2
libva_sha256=eec6050b52876f229bd35e9df17cd31a06785e18e6f7990c445b584628483d67
libva_driver_dirs=/usr/lib/x86_64-linux-gnu/dri:/usr/lib64/dri:/usr/lib/dri:/usr/local/lib/x86_64-linux-gnu/dri:/usr/local/lib64/dri:/usr/local/lib/dri

downloads=$ffmpeg_root/target/ffmpeg/downloads
sources=$work_dir/src
builds=$work_dir/build
prefix=$work_dir/prefix
jobs=$(parallel_jobs)

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

fetch() {
  local name=$1 url=$2 sha256=$3
  local archive=$downloads/$name.tar.${url##*.tar.}
  mkdir -p "$downloads"
  if [ ! -f "$archive" ] || [ "$(sha256_of "$archive")" != "$sha256" ]; then
    echo "Downloading $url"
    curl -fsSL --retry 3 -o "$archive.part" "$url"
    mv "$archive.part" "$archive"
  fi
  local actual
  actual=$(sha256_of "$archive")
  [ "$actual" = "$sha256" ] || die "$archive has sha256 $actual, expected $sha256"
  rm -rf "${sources:?}/$name"
  mkdir -p "$sources/$name"
  tar -xf "$archive" -C "$sources/$name" --strip-components=1
}

cmake_generator() {
  if [ "$os" = windows ]; then
    echo "NMake Makefiles"
  else
    echo "Unix Makefiles"
  fi
}

cmake_build() {
  local name=$1
  shift
  local platform_flags=()
  case $os in
    macos) platform_flags=(-DCMAKE_OSX_DEPLOYMENT_TARGET="$macos_deployment_target" -DCMAKE_OSX_ARCHITECTURES=arm64) ;;
    windows) platform_flags=(-DCMAKE_POLICY_DEFAULT_CMP0091=NEW -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded) ;;
  esac
  cmake -S "$(native_path "$sources/$name")" -B "$(native_path "$builds/$name")" \
    -G "$(cmake_generator)" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$(native_path "$prefix")" \
    -DCMAKE_INSTALL_LIBDIR=lib \
    -DBUILD_SHARED_LIBS=OFF \
    -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    "${platform_flags[@]}" "$@"
  cmake --build "$(native_path "$builds/$name")" --config Release --parallel "$jobs"
  cmake --install "$(native_path "$builds/$name")" --config Release
}

build_opus() {
  fetch opus "$opus_url" "$opus_sha256"
  local flags=(-DOPUS_BUILD_PROGRAMS=OFF -DOPUS_BUILD_TESTING=OFF -DOPUS_INSTALL_CMAKE_CONFIG_MODULE=OFF)
  [ "$os" != windows ] || flags+=(-DOPUS_STATIC_RUNTIME=ON)
  cmake_build opus "${flags[@]}"
}

build_openh264() {
  fetch openh264 "$openh264_url" "$openh264_sha256"
  case $os in
    macos)
      MACOSX_DEPLOYMENT_TARGET=$macos_deployment_target \
        make -C "$sources/openh264" -j"$jobs" OS=darwin ARCH=arm64 \
        PREFIX="$prefix" STATIC_LDFLAGS=-lc++ install-static
      ;;
    linux)
      make -C "$sources/openh264" -j"$jobs" OS=linux ARCH=x86_64 \
        PREFIX="$prefix" install-static
      ;;
    windows)
      make -C "$sources/openh264" -j"$jobs" OS=msvc ARCH=x86_64 \
        PREFIX="$(native_path "$prefix")" \
        CFLAGS_OPT="-O2 -Ob1 -Oy- -GF -GS -Gy -DNDEBUG -MT" install-static
      ;;
  esac
}

install_nvcodec_headers() {
  fetch nv-codec-headers "$nvcodec_url" "$nvcodec_sha256"
  make -C "$sources/nv-codec-headers" PREFIX="$prefix" install
}

install_amf_headers() {
  fetch amf-headers "$amf_url" "$amf_sha256"
  rm -rf "$prefix/include/AMF"
  mkdir -p "$prefix/include"
  cp -R "$sources/amf-headers/AMF" "$prefix/include/AMF"
}

install_vulkan_headers() {
  fetch vulkan-headers "$vulkan_url" "$vulkan_sha256"
  mkdir -p "$prefix/include"
  rm -rf "$prefix/include/vulkan" "$prefix/include/vk_video"
  cp -R "$sources/vulkan-headers/include/vulkan" "$sources/vulkan-headers/include/vk_video" "$prefix/include/"
}

build_libva() {
  fetch libva "$libva_url" "$libva_sha256"
  meson setup "$builds/libva" "$sources/libva" \
    --prefix="$prefix" --libdir=lib --buildtype=release \
    -Ddriverdir="$libva_driver_dirs" \
    -Dwith_x11=no -Dwith_glx=no -Dwith_wayland=no -Denable_docs=false \
    '-Dc_link_args=-Wl,-rpath,$ORIGIN'
  meson compile -C "$builds/libva"
  meson install -C "$builds/libva"
}

build_libvpl() {
  fetch libvpl "$libvpl_url" "$libvpl_sha256"
  cmake_build libvpl -DUSE_MSVC_STATIC_RUNTIME=ON -DBUILD_TESTS=OFF -DBUILD_EXAMPLES=OFF \
    -DBUILD_TOOLS=OFF -DINSTALL_EXAMPLES=OFF -DBUILD_EXPERIMENTAL=OFF
  local pc
  for pc in "$prefix"/lib/pkgconfig/vpl.pc; do
    sed -i -e "s|^prefix=.*|prefix=$(native_path "$prefix")|" \
      -e "s|^libdir=.*|libdir=\${prefix}/lib|" \
      -e "s|^includedir=.*|includedir=\${prefix}/include|" "$pc"
  done
}

common_flags=(
  --enable-shared
  --disable-static
  --disable-programs
  --disable-doc
  --disable-debug
  --enable-small
  --disable-autodetect
  --disable-everything
  --disable-avdevice
  --disable-network
  --enable-libopus
  --enable-libopenh264
  --pkg-config-flags=--static
  --enable-decoder=h264,hevc,av1,aac,opus
  --enable-muxer=mp4,mov,matroska
  --enable-demuxer=mov,matroska
  --enable-parser=h264,hevc,av1,aac,opus
  --enable-bsf=h264_mp4toannexb,hevc_mp4toannexb,aac_adtstoasc,extract_extradata,h264_metadata,hevc_metadata,av1_metadata
  --enable-protocol=file
)
base_filters=scale,format,aresample,amix,hwupload,hwdownload,null,anull
base_encoders=aac,libopus,libopenh264

platform_flags() {
  case $os in
    macos)
      platform=(
        --install-name-dir=@rpath
        --extra-cflags="-mmacosx-version-min=$macos_deployment_target -I$prefix/include"
        --extra-ldflags="-mmacosx-version-min=$macos_deployment_target -L$prefix/lib"
        --enable-videotoolbox
        --enable-audiotoolbox
        --enable-encoder=$base_encoders,h264_videotoolbox,hevc_videotoolbox,aac_at
        --enable-hwaccel=h264_videotoolbox,hevc_videotoolbox,av1_videotoolbox
        --enable-filter=$base_filters,scale_vt
      )
      ;;
    linux)
      platform=(
        --extra-cflags="-I$prefix/include"
        --extra-ldflags="-L$prefix/lib"
        "--extra-ldsoflags=-Wl,-rpath,'\\\$\\\$\\\$\\\$ORIGIN'"
        --enable-vaapi
        --enable-vulkan
        --enable-ffnvcodec
        --enable-nvenc
        --enable-nvdec
        --enable-encoder=$base_encoders,h264_nvenc,hevc_nvenc,av1_nvenc,h264_vaapi,hevc_vaapi,av1_vaapi,h264_vulkan,hevc_vulkan,av1_vulkan
        --enable-hwaccel=h264_vaapi,hevc_vaapi,av1_vaapi,h264_nvdec,hevc_nvdec,av1_nvdec,h264_vulkan,hevc_vulkan,av1_vulkan
        --enable-filter=$base_filters,scale_vaapi,hwmap,hwupload_cuda
      )
      ;;
    windows)
      platform=(
        --toolchain=msvc
        --target-os=win64
        --arch=x86_64
        --extra-cflags="-I$(native_path "$prefix/include")"
        --extra-ldflags="-libpath:$(native_path "$prefix/lib")"
        --enable-w32threads
        --enable-d3d11va
        --enable-d3d12va
        --enable-mediafoundation
        --enable-amf
        --enable-ffnvcodec
        --enable-nvenc
        --enable-nvdec
        --enable-libvpl
        --enable-encoder=$base_encoders,h264_nvenc,hevc_nvenc,av1_nvenc,h264_amf,hevc_amf,av1_amf,h264_qsv,hevc_qsv,av1_qsv,h264_mf,hevc_mf,av1_mf,h264_d3d12va,hevc_d3d12va,av1_d3d12va
        --enable-hwaccel=h264_d3d11va2,hevc_d3d11va2,av1_d3d11va2,h264_d3d12va,hevc_d3d12va,av1_d3d12va,h264_nvdec,hevc_nvdec,av1_nvdec
        --enable-filter=$base_filters,scale_d3d11,hwmap,hwupload_cuda
      )
      ;;
  esac
}

build_dependencies() {
  rm -rf "${prefix:?}" "${builds:?}"
  mkdir -p "$prefix" "$builds"
  build_opus
  build_openh264
  case $os in
    linux)
      build_libva
      install_nvcodec_headers
      install_vulkan_headers
      ;;
    windows)
      install_nvcodec_headers
      install_amf_headers
      build_libvpl
      ;;
  esac
}

configure_ffmpeg() {
  fetch ffmpeg "$ffmpeg_url" "$ffmpeg_sha256"
  platform_flags
  configure_flags=("${common_flags[@]}" "${platform[@]}")
  local flag
  for flag in "${configure_flags[@]}"; do
    case $flag in
      --enable-gpl|--enable-nonfree|--enable-version3) die "$flag is not allowed; ngine ships FFmpeg as LGPL 2.1" ;;
    esac
  done
  mkdir -p "$builds/ffmpeg"
  if ! (
    cd "$builds/ffmpeg"
    PKG_CONFIG_PATH="$prefix/lib/pkgconfig" "$sources/ffmpeg/configure" \
      --prefix="$ffmpeg_dir" "${configure_flags[@]}"
  ) | tee "$work_dir/configure.log"; then
    tail -n 80 "$builds/ffmpeg/ffbuild/config.log" >&2
    die "FFmpeg's configure failed"
  fi
  local licence
  licence=$(sed -n 's/^License: //p' "$work_dir/configure.log")
  [ "$licence" = "LGPL version 2.1 or later" ] ||
    die "configure reports the licence '$licence'; only 'LGPL version 2.1 or later' is allowed"
}

build_ffmpeg() {
  make -C "$builds/ffmpeg" -j"$jobs"
  rm -rf "${ffmpeg_dir:?}"
  make -C "$builds/ffmpeg" install
  rm -rf "$ffmpeg_dir/share"
  if [ "$os" = linux ]; then
    cp -P "$prefix"/lib/libva.so.2* "$prefix"/lib/libva-drm.so.2* "$ffmpeg_dir/lib/"
  fi
  local library
  while IFS= read -r library; do
    case $os in
      macos) strip -x "$library" ;;
      linux) strip --strip-unneeded "$library" ;;
    esac
  done < <(shared_libraries)
}

header_notice() {
  awk 'NR == 1 || !/^ \*\// { print } /^ \*\// { print; exit }' "$1"
}

copy_licences() {
  local licences=$ffmpeg_dir/licenses
  mkdir -p "$licences"
  cp "$sources/ffmpeg/COPYING.LGPLv2.1" "$licences/FFmpeg-COPYING.LGPLv2.1"
  cp "$sources/ffmpeg/LICENSE.md" "$licences/FFmpeg-LICENSE.md"
  cp "$sources/opus/COPYING" "$licences/opus-COPYING"
  cp "$sources/openh264/LICENSE" "$licences/openh264-LICENSE"
  case $os in
    linux)
      header_notice "$sources/nv-codec-headers/include/ffnvcodec/nvEncodeAPI.h" > "$licences/nv-codec-headers-NOTICE"
      cp "$sources/libva/COPYING" "$licences/libva-COPYING"
      cp "$sources/vulkan-headers/LICENSE.md" "$licences/Vulkan-Headers-LICENSE.md"
      cp "$sources/vulkan-headers/LICENSES/MIT.txt" "$licences/Vulkan-Headers-MIT.txt"
      cp "$sources/vulkan-headers/LICENSES/Apache-2.0.txt" "$licences/Vulkan-Headers-Apache-2.0.txt"
      ;;
    windows)
      header_notice "$sources/nv-codec-headers/include/ffnvcodec/nvEncodeAPI.h" > "$licences/nv-codec-headers-NOTICE"
      sed -n '/^\/\//p;/^[^\/]/q' "$sources/amf-headers/AMF/core/Platform.h" > "$licences/AMF-headers-NOTICE"
      cp "$sources/libvpl/LICENSE" "$licences/libvpl-LICENSE"
      ;;
  esac
}

manifest_entry() {
  printf '    {"name": "%s", "version": "%s", "url": "%s", "sha256": "%s", "licence": "%s", "use": "%s"}' "$@"
}

write_manifest() {
  local entries=()
  entries+=("$(manifest_entry opus "$opus_version" "$opus_url" "$opus_sha256" BSD-3-Clause "static, inside libavcodec")")
  entries+=("$(manifest_entry openh264 "$openh264_version" "$openh264_url" "$openh264_sha256" BSD-2-Clause "static, inside libavcodec")")
  case $os in
    linux)
      entries+=("$(manifest_entry nv-codec-headers "$nvcodec_version" "$nvcodec_url" "$nvcodec_sha256" MIT "headers only")")
      entries+=("$(manifest_entry Vulkan-Headers "$vulkan_version" "$vulkan_url" "$vulkan_sha256" "Apache-2.0 OR MIT" "headers only")")
      entries+=("$(manifest_entry libva "$libva_version" "$libva_url" "$libva_sha256" MIT "shared, bundled in lib/")")
      ;;
    windows)
      entries+=("$(manifest_entry nv-codec-headers "$nvcodec_version" "$nvcodec_url" "$nvcodec_sha256" MIT "headers only")")
      entries+=("$(manifest_entry AMF-headers "$amf_version" "$amf_url" "$amf_sha256" MIT "headers only")")
      entries+=("$(manifest_entry libvpl "$libvpl_version" "$libvpl_url" "$libvpl_sha256" MIT "static, inside libavcodec and libavutil")")
      ;;
  esac
  local configure_json flag
  configure_json=""
  for flag in "${configure_flags[@]}"; do
    configure_json+="${configure_json:+, }\"${flag//\"/\\\"}\""
  done
  local libraries_json library
  libraries_json=""
  while IFS= read -r library; do
    libraries_json+="${libraries_json:+, }\"$(basename "$library")\""
  done < <(shared_libraries)
  {
    echo "{"
    echo "  \"target\": \"$triple\","
    echo "  \"ffmpeg\": {\"version\": \"$ffmpeg_version\", \"url\": \"$ffmpeg_url\", \"sha256\": \"$ffmpeg_sha256\", \"licence\": \"LGPL-2.1-or-later\"},"
    echo "  \"shared_libraries\": [$libraries_json],"
    echo "  \"configure\": [$configure_json],"
    echo "  \"external\": ["
    local i
    for i in "${!entries[@]}"; do
      if [ "$i" -lt $((${#entries[@]} - 1)) ]; then
        echo "${entries[$i]},"
      else
        echo "${entries[$i]}"
      fi
    done
    echo "  ]"
    echo "}"
  } > "$ffmpeg_dir/manifest.json"
}

build_dependencies
configure_ffmpeg
build_ffmpeg
copy_licences
write_manifest
"$ffmpeg_root/ffmpeg/check.sh"
