#!/usr/bin/env bash
# Shared by install.sh and uninstall.sh. Every change ngine makes to a file
# sits between two marker lines, so installing again replaces it and
# uninstalling removes exactly it.

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
config=${XDG_CONFIG_HOME:-$HOME/.config}
hypr=$config/hypr
omarchy=$config/omarchy
waybar=$config/waybar
plugin_id=ngine.status
plugin_dir=$omarchy/plugins/$plugin_id

die() {
  echo "ngine: $*" >&2
  exit 1
}

omarchy_version() {
  if [ -f "$hypr/hyprland.lua" ]; then
    echo 4
  elif [ -f "$hypr/hyprland.conf" ]; then
    echo 3
  else
    die "$hypr holds neither hyprland.lua nor hyprland.conf; is this Omarchy?"
  fi
}

begin_marker() { echo "$1 ngine:begin${2:-}"; }
end_marker() { echo "$1 ngine:end${2:-}"; }

rewrite() {
  local file=$1 new=$2
  cat "$new" >"$file"
  rm -f "$new"
}

strip_block() {
  local file=$1 open=$2 close=${3:-}
  [ -f "$file" ] || return 0
  local begin end new
  begin=$(begin_marker "$open" "$close")
  end=$(end_marker "$open" "$close")
  new=$(mktemp)
  awk -v b="$begin" -v e="$end" '$0 == b { skip = 1; next } $0 == e { skip = 0; next } !skip' "$file" >"$new"
  rewrite "$file" "$new"
}

append_block() {
  local file=$1 open=$2 close=$3 snippet=$4
  strip_block "$file" "$open" "$close"
  mkdir -p "$(dirname "$file")"
  touch "$file"
  {
    begin_marker "$open" "$close"
    cat "$snippet"
    end_marker "$open" "$close"
  } >>"$file"
}

insert_block_after_brace() {
  local file=$1 snippet=$2 new
  strip_block "$file" "//"
  [ -f "$file" ] || printf '{\n}\n' >"$file"
  new=$(mktemp)
  awk -v b="$(begin_marker //)" -v e="$(end_marker //)" -v snippet="$snippet" '
    !done && /^[[:space:]]*\{[[:space:]]*$/ {
      print
      print b
      while ((getline line < snippet) > 0) print line
      print e
      done = 1
      next
    }
    { print }
  ' "$file" >"$new"
  rewrite "$file" "$new"
}

add_to_waybar_center() {
  local file=$1 new
  grep -q '"custom/ngine", ' "$file" && return 0
  new=$(mktemp)
  awk '!done && sub(/"modules-center": \[/, "\"modules-center\": [\"custom/ngine\", ") { done = 1 } { print }' "$file" >"$new"
  rewrite "$file" "$new"
}

remove_from_waybar_center() {
  local file=$1 new
  [ -f "$file" ] || return 0
  new=$(mktemp)
  awk '{ gsub(/"custom\/ngine", /, "") } { print }' "$file" >"$new"
  rewrite "$file" "$new"
}

reloading() {
  [ "${NGINE_OMARCHY_NO_RELOAD:-}" != 1 ]
}

have() {
  command -v "$1" >/dev/null 2>&1
}
