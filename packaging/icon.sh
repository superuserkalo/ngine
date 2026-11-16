#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: packaging/icon.sh

Renders packaging/icon.svg into packaging/icon.png (1024 px, for macOS and
Linux) and packaging/icon.ico (for Windows). Needs ImageMagick 7 (magick).
The rendered files are committed, so releases do not need ImageMagick.
EOF
}

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage >&2; exit 2 ;;
esac

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
magick -background none -density 96 "$here/icon.svg" -resize 1024x1024 \
  -strip -define png:exclude-chunks=date,time "$here/icon.png"
magick -background none -density 96 "$here/icon.svg" \
  -define icon:auto-resize=256,128,64,48,32,24,16 "$here/icon.ico"
