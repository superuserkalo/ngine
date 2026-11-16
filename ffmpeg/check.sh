#!/usr/bin/env bash
set -euo pipefail

. "$(dirname "$0")/platform.sh"

[ -d "$ffmpeg_dir" ] || die "no libraries in $ffmpeg_dir; run ffmpeg/build.sh"

check_size() {
  local total=0 size library
  echo "Stripped shared libraries for $triple:"
  while IFS= read -r library; do
    size=$(file_size "$library")
    total=$((total + size))
    printf '  %-32s %9d bytes\n' "$(basename "$library")" "$size"
  done < <(shared_libraries)
  [ "$total" -gt 0 ] || die "found no shared libraries in $ffmpeg_dir"
  awk -v total="$total" -v triple="$triple" \
    'BEGIN { printf "Total stripped size for %s: %d bytes (%.2f MB)\n", triple, total, total / 1000000 }'
  [ "$total" -le "$size_limit_bytes" ] ||
    die "the libraries take $total bytes; the limit is $size_limit_bytes. Trim the build or revisit ADR 0003."
}

show_runtime_dependencies() {
  local library
  echo "Run-time dependencies:"
  while IFS= read -r library; do
    echo "  $(basename "$library"):"
    case $os in
      macos) otool -L "$library" | tail -n +2 | sed 's/^[[:space:]]*/    /' ;;
      linux) readelf -d "$library" | awk '/NEEDED|RUNPATH|RPATH/ { print "    " $0 }' ;;
      windows) dumpbin -nologo -dependents "$library" | awk '/\.dll$/ && !/Dump of file/ { print "    " $1 }' ;;
    esac
  done < <(shared_libraries)
}

check_relocatable() {
  local library
  case $os in
    macos)
      while IFS= read -r library; do
        [[ "$(otool -D "$library" | tail -n +2)" == @rpath/* ]] ||
          die "$(basename "$library") does not use an @rpath install name"
      done < <(shared_libraries)
      ;;
    linux)
      while IFS= read -r library; do
        [[ "$(readelf -d "$library")" == *'path: [$ORIGIN]'* ]] ||
          die "$(basename "$library") does not search \$ORIGIN for its siblings"
      done < <(shared_libraries)
      ;;
  esac
}

run_probe() {
  local probe_dir=$work_dir/probe
  mkdir -p "$probe_dir"
  local source=$ffmpeg_root/ffmpeg/probe.c
  local include=$ffmpeg_dir/include
  local lib=$ffmpeg_dir/lib
  case $os in
    macos)
      cc -mmacosx-version-min="$macos_deployment_target" -I"$include" "$source" \
        -L"$lib" -lavformat -lavcodec -lavutil -Wl,-rpath,"$lib" -o "$probe_dir/probe"
      "$probe_dir/probe" $required_encoders
      ;;
    linux)
      cc -I"$include" "$source" -L"$lib" -lavformat -lavcodec -lavutil \
        -Wl,-rpath,"$lib" -o "$probe_dir/probe"
      "$probe_dir/probe" $required_encoders
      ;;
    windows)
      local linker
      linker="$(dirname "$(command -v cl)")/link.exe"
      cl -nologo -c -I"$(native_path "$include")" "$(native_path "$source")" \
        -Fo"$(native_path "$probe_dir/probe.obj")"
      "$linker" -nologo -out:"$(native_path "$probe_dir/probe.exe")" \
        "$(native_path "$probe_dir/probe.obj")" -libpath:"$(native_path "$lib")" \
        avformat.lib avcodec.lib avutil.lib
      PATH="$ffmpeg_dir/bin:$PATH" "$probe_dir/probe.exe" $required_encoders
      ;;
  esac
}

check_size
check_relocatable
show_runtime_dependencies
run_probe
echo "FFmpeg for $triple is in $ffmpeg_dir"
