#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: packaging/linux.sh [appimage | install <root> <prefix>]

appimage (the default)
  Packages the release binary, the FFmpeg libraries and the bundled libva as
  dist/ngine-<version>-linux-x86_64.AppImage, and writes dist/PKGBUILD with
  this version for Arch-based systems.

install <root> <prefix>
  Installs the same files under <root><prefix>, the program and its libraries
  in <prefix>/lib/ngine, for a distribution package such as the PKGBUILD.

Reads CARGO_TARGET_DIR, NGINE_BINARY, NGINE_FFMPEG_DIR and NGINE_DIST.
Needs cargo, jq, patchelf, strip and ldd; appimage also needs curl.
EOF
}

mode=${1:-appimage}
case $mode in
  -h|--help) usage; exit 0 ;;
  appimage) [ $# -le 1 ] || { usage >&2; exit 2; } ;;
  install) [ $# -eq 3 ] || { usage >&2; exit 2; } ;;
  *) usage >&2; exit 2 ;;
esac

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

require patchelf strip ldd

appimagetool_url=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
appimagetool_sha256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
runtime_url=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
runtime_sha256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

copy_ffmpeg_libraries() {
  mkdir -p "$1"
  find "$ffmpeg_dir/lib" -maxdepth 1 -name '*.so.*' -exec cp -P {} "$1/" \;
}

install_program() {
  local program=$1 run_path=$2
  install -D -m 755 "$binary" "$program"
  strip --strip-unneeded "$program"
  patchelf --set-rpath "$run_path" "$program"
}

check_every_library_resolves() {
  local program=$1 bundled=$2 missing outside
  missing=$(ldd "$program" | grep 'not found' || true)
  [ -z "$missing" ] || die "$program cannot find these libraries:"$'\n'"$missing"
  outside=$(ldd "$program" | grep -E 'lib(av|sw|va)[a-z-]*\.so' | grep -vF "$bundled/" || true)
  [ -z "$outside" ] || die "$program loads FFmpeg or libva from outside the package:"$'\n'"$outside"
}

install_shared_files() {
  local share=$1
  install -D -m 644 "$packaging_root/linux/ngine.desktop" "$share/applications/ngine.desktop"
  install -D -m 644 "$packaging_root/icon.svg" "$share/icons/hicolor/scalable/apps/ngine.svg"
  "$packaging_root/notices.sh" "$share/licenses/ngine"
  mkdir -p "$share/ngine/omarchy"
  cp -R "$packaging_root/omarchy/." "$share/ngine/omarchy/"
}

install_tree() {
  local root=$1 prefix=$2
  local libexec=$root$prefix/lib/ngine
  copy_ffmpeg_libraries "$libexec"
  install_program "$libexec/ngine" "\$ORIGIN"
  check_every_library_resolves "$libexec/ngine" "$libexec"
  mkdir -p "$root$prefix/bin"
  ln -sf "$prefix/lib/ngine/ngine" "$root$prefix/bin/ngine"
  install_shared_files "$root$prefix/share"
}

fetch_tool() {
  local url=$1 sha256=$2 file=$3
  if [ ! -f "$file" ] || [ "$(sha256_of "$file")" != "$sha256" ]; then
    mkdir -p "$(dirname "$file")"
    curl -fsSL --retry 3 -o "$file.part" "$url"
    mv "$file.part" "$file"
  fi
  [ "$(sha256_of "$file")" = "$sha256" ] || die "$file does not have the sha256 $sha256"
  chmod 755 "$file"
}

build_appimage() {
  require curl
  local appdir=$dist_dir/AppDir tools=$dist_dir/tools
  local appimage=$dist_dir/ngine-$version-linux-x86_64.AppImage
  rm -rf "$appdir" "$appimage"
  copy_ffmpeg_libraries "$appdir/usr/lib"
  install_program "$appdir/usr/bin/ngine" "\$ORIGIN/../lib"
  check_every_library_resolves "$appdir/usr/bin/ngine" "$appdir/usr/lib"
  install_shared_files "$appdir/usr/share"
  cp "$packaging_root/linux/ngine.desktop" "$appdir/ngine.desktop"
  cp "$packaging_root/icon.png" "$appdir/ngine.png"
  ln -s usr/bin/ngine "$appdir/AppRun"

  fetch_tool "$appimagetool_url" "$appimagetool_sha256" "$tools/appimagetool"
  fetch_tool "$runtime_url" "$runtime_sha256" "$tools/runtime-x86_64"
  APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$tools/appimagetool" --no-appstream \
    --runtime-file "$tools/runtime-x86_64" "$appdir" "$appimage"
  rm -rf "$appdir"
  "$appimage" --appimage-extract-and-run --version
  echo "Built $appimage ($(du -h "$appimage" | cut -f1))"
}

write_aur_package() {
  local name=$1 pkgbuild=$2 sha256=$3 folder=$dist_dir/aur/$1
  mkdir -p "$folder"
  sed -e "s/^pkgver=.*/pkgver=$version/" -e "s/^sha256sums=('SKIP')$/sha256sums=('$sha256')/" \
    "$pkgbuild" >"$folder/PKGBUILD"
  cp "$packaging_root/arch/ngine.install" "$folder/"
  if command -v makepkg >/dev/null; then
    (cd "$folder" && makepkg --printsrcinfo >.SRCINFO)
  fi
  echo "Wrote $folder for the AUR"
}

write_pkgbuild() {
  local appimage=$dist_dir/ngine-$version-linux-x86_64.AppImage
  sed "s/^pkgver=.*/pkgver=$version/" "$packaging_root/arch/PKGBUILD" > "$dist_dir/PKGBUILD"
  echo "Wrote $dist_dir/PKGBUILD for ngine $version"
  write_aur_package ngine "$packaging_root/arch/PKGBUILD" SKIP
  write_aur_package ngine-bin "$packaging_root/arch/PKGBUILD-bin" "$(sha256_of "$appimage")"
}

case $mode in
  appimage)
    build_appimage
    write_pkgbuild
    ;;
  install)
    install_tree "$2" "$3"
    ;;
esac
