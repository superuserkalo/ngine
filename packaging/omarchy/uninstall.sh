#!/usr/bin/env bash
# Takes out everything install.sh added, so Omarchy's own screen recording
# is back. Set NGINE_OMARCHY_NO_RELOAD=1 to only write files.
set -euo pipefail

# shellcheck source=packaging/omarchy/blocks.sh
. "$(dirname "${BASH_SOURCE[0]}")/blocks.sh"

version=$(omarchy_version)

strip_block "$hypr/bindings.lua" "--"
strip_block "$hypr/autostart.lua" "--"
strip_block "$hypr/hyprland.lua" "--"
strip_block "$omarchy/extensions/omarchy-menu.jsonc" "//"
strip_block "$hypr/bindings.conf" "#"
strip_block "$hypr/autostart.conf" "#"
strip_block "$hypr/hyprland.conf" "#"
strip_block "$omarchy/extensions/menu.sh" "#"
strip_block "$waybar/config.jsonc" "//"
remove_from_waybar_center "$waybar/config.jsonc"
strip_block "$waybar/style.css" "/*" " */"

if reloading && [ -d "$plugin_dir" ] && have omarchy; then
  omarchy plugin disable "$plugin_id" --yes >/dev/null || true
fi
rm -rf "$plugin_dir"

if reloading; then
  if [ "$version" = 3 ] && have omarchy-restart-waybar && pgrep -x waybar >/dev/null; then
    omarchy-restart-waybar
  fi
  if have hyprctl; then hyprctl reload >/dev/null || true; fi
fi

echo "ngine is no longer bound into Omarchy $version. ngine itself stays installed."
