#!/usr/bin/env bash
packaging_root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$packaging_root/.." && pwd)

die() {
  echo "packaging: $*" >&2
  exit 1
}

require() {
  local tool
  for tool in "$@"; do
    command -v "$tool" >/dev/null || die "$tool is needed and was not found on the PATH"
  done
}

unix_path() {
  if command -v cygpath >/dev/null; then
    cygpath -u "$1"
  else
    printf '%s\n' "$1"
  fi
}

windows_path() {
  cygpath -w "$1"
}

sha256_of() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

require cargo jq

workspace_metadata=$(cargo metadata --format-version 1 --no-deps --locked --manifest-path "$repo_root/Cargo.toml")
version=$(jq -r '.packages[] | select(.name == "ngine") | .version' <<<"$workspace_metadata")
[ -n "$version" ] || die "cargo metadata names no ngine package"

triple=${NGINE_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}
ffmpeg_dir=$(unix_path "${NGINE_FFMPEG_DIR:-$repo_root/target/ffmpeg/$triple}")
target_dir=$(unix_path "${CARGO_TARGET_DIR:-$repo_root/target}")
dist_dir=$(unix_path "${NGINE_DIST:-$repo_root/dist}")

case $triple in
  *-windows-*) default_binary=$target_dir/release/ngine.exe ;;
  *) default_binary=$target_dir/release/ngine ;;
esac
binary=$(unix_path "${NGINE_BINARY:-$default_binary}")

[ -f "$binary" ] || die "no release binary at $binary; run cargo build --release --locked -p ngine first, or set NGINE_BINARY"
[ -f "$ffmpeg_dir/manifest.json" ] || die "no FFmpeg build in $ffmpeg_dir; run ffmpeg/build.sh first, or set NGINE_FFMPEG_DIR"
mkdir -p "$dist_dir"
