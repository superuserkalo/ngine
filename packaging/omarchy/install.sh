#!/usr/bin/env bash
# Makes ngine Omarchy's screen recorder: ALT + PRINT and the capture menu
# drive ngine, ngine starts with the session, and the bar shows its state.
# Running it again changes nothing; uninstall.sh takes it all out.
# Set NGINE_OMARCHY_NO_RELOAD=1 to only write files.
set -euo pipefail

# shellcheck source=packaging/omarchy/blocks.sh
. "$(dirname "${BASH_SOURCE[0]}")/blocks.sh"

version=$(omarchy_version)

if [ "$version" = 4 ]; then
  append_block "$hypr/bindings.lua" "--" "" "$here/hypr/bindings.lua"
  append_block "$hypr/autostart.lua" "--" "" "$here/hypr/autostart.lua"
  append_block "$hypr/hyprland.lua" "--" "" "$here/hypr/windowrules.lua"
  insert_block_after_brace "$omarchy/extensions/omarchy-menu.jsonc" "$here/menu/omarchy-menu.jsonc"
  rm -rf "$plugin_dir"
  mkdir -p "$plugin_dir"
  cp "$here/shell-plugin/manifest.json" "$here/shell-plugin/NgineStatus.qml" "$plugin_dir/"
else
  append_block "$hypr/bindings.conf" "#" "" "$here/hypr/bindings.conf"
  append_block "$hypr/autostart.conf" "#" "" "$here/hypr/autostart.conf"
  append_block "$hypr/hyprland.conf" "#" "" "$here/hypr/windowrules.conf"
  append_block "$omarchy/extensions/menu.sh" "#" "" "$here/menu/menu.sh"
  if [ -f "$waybar/config.jsonc" ]; then
    insert_block_after_brace "$waybar/config.jsonc" "$here/waybar/module.jsonc"
    add_to_waybar_center "$waybar/config.jsonc"
    append_block "$waybar/style.css" "/*" " */" "$here/waybar/style.css"
  fi
fi

if reloading; then
  if [ "$version" = 4 ]; then
    if have omarchy-shell; then omarchy-shell shell rescanPlugins >/dev/null || true; fi
    if have omarchy; then omarchy plugin enable "$plugin_id" --yes >/dev/null || true; fi
  elif have omarchy-restart-waybar && pgrep -x waybar >/dev/null; then
    omarchy-restart-waybar
  fi
  if have hyprctl; then hyprctl reload >/dev/null || true; fi
  if have ngine && ! ngine status >/dev/null 2>&1; then
    if have uwsm-app; then
      setsid uwsm-app -- ngine --background >/dev/null 2>&1 &
    else
      setsid ngine --background >/dev/null 2>&1 &
    fi
  fi
fi

echo "ngine is now Omarchy $version's screen recorder: ALT + PRINT records, SHIFT + ALT + PRINT saves the last 30 seconds."
