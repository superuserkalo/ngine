#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: packaging/macos.sh

Packages the release binary as dist/ngine.app, with the FFmpeg libraries in
Contents/Frameworks and the licence texts in Contents/Resources/licenses, signs
it, and puts it in dist/ngine-<version>-macos-arm64.dmg.

Signs ad hoc unless NGINE_MACOS_SIGN_IDENTITY names a Developer ID identity in
the keychain, which also turns on the hardened runtime that notarisation needs.

Reads CARGO_TARGET_DIR, NGINE_BINARY, NGINE_FFMPEG_DIR and NGINE_DIST.
Needs Xcode command line tools, cargo and jq.
EOF
}

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage >&2; exit 2 ;;
esac

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

require codesign install_name_tool otool strip hdiutil iconutil sips plutil ditto
bundle_id=dev.ngine.app
minimum_macos=15.0
frameworks_rpath=@executable_path/../Frameworks
identity=${NGINE_MACOS_SIGN_IDENTITY:--}
app=$dist_dir/ngine.app
dmg=$dist_dir/ngine-$version-macos-arm64.dmg

write_info_plist() {
  cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>ngine</string>
  <key>CFBundleExecutable</key>
  <string>ngine</string>
  <key>CFBundleIconFile</key>
  <string>ngine</string>
  <key>CFBundleIdentifier</key>
  <string>$bundle_id</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>ngine</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$version</string>
  <key>CFBundleVersion</key>
  <string>$version</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.video</string>
  <key>LSMinimumSystemVersion</key>
  <string>$minimum_macos</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>Copyright ngine contributors. MIT licence.</string>
  <key>NSMicrophoneUsageDescription</key>
  <string>ngine records your microphone into its own track when you turn the microphone on.</string>
  <key>NSScreenCaptureUsageDescription</key>
  <string>ngine records your screen, a window or an app, and the sound it plays.</string>
  <key>NSAudioCaptureUsageDescription</key>
  <string>ngine records the sound your Mac or an app plays.</string>
</dict>
</plist>
EOF
}

make_icon() {
  local iconset=$dist_dir/ngine.iconset size
  rm -rf "$iconset"
  mkdir -p "$iconset"
  for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$packaging_root/icon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) "$packaging_root/icon.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$iconset" -o "$1"
  rm -rf "$iconset"
}

install_name_of() {
  otool -D "$1" | sed -n 2p
}

copy_ffmpeg_libraries() {
  local frameworks=$1 library name
  mkdir -p "$frameworks"
  while IFS= read -r library; do
    name=$(install_name_of "$library")
    case $name in
      @rpath/*) cp "$library" "$frameworks/${name#@rpath/}" ;;
      *) die "$library has the install name $name; ffmpeg/build.sh must build with --install-name-dir=@rpath" ;;
    esac
  done < <(find "$ffmpeg_dir/lib" -maxdepth 1 -type f -name '*.dylib' | sort)
}

rpaths_of() {
  otool -l "$1" | awk '/cmd LC_RPATH/ { getline; getline; print $2 }'
}

point_rpaths_into_the_bundle() {
  local executable=$1 rpath
  while IFS= read -r rpath; do
    case $rpath in
      @*|/usr/*|/System/*) ;;
      *) install_name_tool -delete_rpath "$rpath" "$executable" ;;
    esac
  done < <(rpaths_of "$executable")
  rpaths_of "$executable" | grep -qxF "$frameworks_rpath" ||
    install_name_tool -add_rpath "$frameworks_rpath" "$executable"
}

dependencies_of() {
  otool -L "$1" | tail -n +2 | awk '{ print $1 }'
}

check_every_library_resolves() {
  local frameworks=$1 file dependency unresolved=""
  for file in "$app/Contents/MacOS/ngine" "$frameworks"/*.dylib; do
    [ -e "$file" ] || continue
    while IFS= read -r dependency; do
      case $dependency in
        /usr/lib/*|/System/Library/*) ;;
        @rpath/libswift*) ;;
        @rpath/*) [ -f "$frameworks/${dependency#@rpath/}" ] || unresolved+="$file needs $dependency"$'\n' ;;
        *) unresolved+="$file needs $dependency, which is not inside the bundle or the system"$'\n' ;;
      esac
    done < <(dependencies_of "$file" | grep -vxF "$(install_name_of "$file" 2>/dev/null || true)")
  done
  [ -z "$unresolved" ] || die "libraries that would not load on another Mac:"$'\n'"$unresolved"
}

sign() {
  local flags=(--force --sign "$identity")
  [ "$identity" = - ] || flags+=(--options runtime --timestamp)
  local library
  for library in "$app/Contents/Frameworks"/*.dylib; do
    codesign "${flags[@]}" "$library"
  done
  codesign "${flags[@]}" --deep "$app"
  codesign --verify --deep --strict "$app"
}

build_app() {
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  write_info_plist > "$app/Contents/Info.plist"
  plutil -lint "$app/Contents/Info.plist" >/dev/null
  printf 'APPL????' > "$app/Contents/PkgInfo"
  make_icon "$app/Contents/Resources/ngine.icns"
  cp "$binary" "$app/Contents/MacOS/ngine"
  chmod 755 "$app/Contents/MacOS/ngine"
  strip -x "$app/Contents/MacOS/ngine"
  copy_ffmpeg_libraries "$app/Contents/Frameworks"
  point_rpaths_into_the_bundle "$app/Contents/MacOS/ngine"
  check_every_library_resolves "$app/Contents/Frameworks"
  "$packaging_root/notices.sh" "$app/Contents/Resources/licenses"
  sign
}

build_dmg() {
  local staging=$dist_dir/dmg
  rm -rf "$staging" "$dmg"
  mkdir -p "$staging"
  ditto "$app" "$staging/ngine.app"
  ln -s /Applications "$staging/Applications"
  hdiutil create -quiet -volname "ngine $version" -srcfolder "$staging" -fs HFS+ -format ULMO -ov "$dmg"
  rm -rf "$staging"
  [ "$identity" = - ] || codesign --force --sign "$identity" --timestamp "$dmg"
}

build_app
build_dmg
"$app/Contents/MacOS/ngine" --version
echo "Built $app ($(du -sh "$app" | cut -f1)) and $dmg ($(du -h "$dmg" | cut -f1))"
