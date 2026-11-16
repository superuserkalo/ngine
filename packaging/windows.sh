#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: packaging/windows.sh

Packages the release binary with the FFmpeg DLLs next to it and the licence
texts in licenses\, as dist/ngine-<version>-windows-x64.zip (portable) and
dist/ngine-<version>-windows-x64-setup.exe (an Inno Setup installer with a
Start menu entry and an uninstaller).

Signs ngine.exe and the installer only when NGINE_WINDOWS_CERTIFICATE names a
.pfx file; NGINE_WINDOWS_CERTIFICATE_PASSWORD holds its password.

Reads CARGO_TARGET_DIR, NGINE_BINARY, NGINE_FFMPEG_DIR and NGINE_DIST.
Runs in Git Bash or MSYS2. Needs cargo, jq, 7z and Inno Setup 6 (iscc).
EOF
}

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage >&2; exit 2 ;;
esac

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

require 7z cygpath
name=ngine-$version-windows-x64
stage=$dist_dir/$name
zip=$dist_dir/$name.zip
installer=$dist_dir/$name-setup.exe

find_iscc() {
  local candidate
  for candidate in "$(command -v iscc 2>/dev/null || true)" \
    "/c/Program Files (x86)/Inno Setup 6/ISCC.exe" \
    "/c/Program Files/Inno Setup 6/ISCC.exe"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return
    fi
  done
  die "Inno Setup 6 was not found; install it, for example with choco install innosetup"
}

find_signtool() {
  find "/c/Program Files (x86)/Windows Kits/10/bin" -path '*/x64/signtool.exe' 2>/dev/null | sort | tail -n 1
}

sign() {
  [ -n "${NGINE_WINDOWS_CERTIFICATE:-}" ] || return 0
  local signtool
  signtool=$(find_signtool)
  [ -n "$signtool" ] || die "signtool.exe was not found in the Windows SDK"
  "$signtool" sign -fd sha256 -tr http://timestamp.digicert.com -td sha256 \
    -f "$(windows_path "$(unix_path "$NGINE_WINDOWS_CERTIFICATE")")" \
    -p "${NGINE_WINDOWS_CERTIFICATE_PASSWORD:-}" "$(windows_path "$1")"
}

stage_files() {
  rm -rf "$stage"
  mkdir -p "$stage"
  cp "$binary" "$stage/ngine.exe"
  sign "$stage/ngine.exe"
  local dlls
  dlls=$(find "$ffmpeg_dir/bin" -maxdepth 1 -type f -name '*.dll' | sort)
  [ -n "$dlls" ] || die "no FFmpeg DLLs in $ffmpeg_dir/bin"
  while IFS= read -r dll; do
    cp "$dll" "$stage/"
  done <<<"$dlls"
  "$packaging_root/notices.sh" "$stage/licenses"
}

build_zip() {
  rm -f "$zip"
  (cd "$dist_dir" && 7z a -tzip -mx=9 -bd "$(windows_path "$zip")" "$name" >/dev/null)
}

build_installer() {
  rm -f "$installer"
  "$(find_iscc)" -Q \
    "-DVersion=$version" \
    "-DSourceDir=$(windows_path "$stage")" \
    "-DOutputDir=$(windows_path "$dist_dir")" \
    "$(windows_path "$packaging_root/windows/ngine.iss")"
  [ -f "$installer" ] || die "Inno Setup did not write $installer"
  sign "$installer"
}

stage_files
"$stage/ngine.exe" --version
build_zip
build_installer
rm -rf "$stage"
echo "Built $zip ($(du -h "$zip" | cut -f1)) and $installer ($(du -h "$installer" | cut -f1))"
