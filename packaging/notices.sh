#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: packaging/notices.sh <folder>

Writes into <folder> the licence texts that every ngine package carries:
ngine's own licence, FFmpeg's licence and source, the licences of the
libraries built into FFmpeg, and THIRD-PARTY-RUST.txt, a notice of every Rust
crate compiled into the ngine binary for this target with its licence texts.
Needs cargo and jq.
EOF
}

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") usage >&2; exit 2 ;;
esac

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

destination=$1

crate_table() {
  cargo metadata --format-version 1 --locked --filter-platform "$triple" --manifest-path "$repo_root/Cargo.toml" | jq -r '
    (.resolve.nodes | map({key: .id, value: [.deps[] | select(any(.dep_kinds[]; .kind == null)) | .pkg]}) | from_entries) as $graph
    | ([.packages[] | select(.name == "ngine" and .source == null) | .id]) as $roots
    | ({seen: {}, todo: $roots}
       | until(.todo | length == 0;
           .todo[0] as $id
           | .todo |= .[1:]
           | if .seen[$id] then . else .seen[$id] = true | .todo += $graph[$id] end)
       | .seen) as $seen
    | [.packages[] | select($seen[.id] and .source != null)]
    | sort_by(.name, .version)[]
    | [.name, .version, (.license // .license_file // "unknown"), (.repository // (.source | sub("^(git|registry)\\+"; "") | sub("[?#].*"; ""))), .source, .manifest_path]
    | @tsv
  '
}

licence_files_in() {
  find "$1" -maxdepth 1 -type f \( -iname 'LICEN[CS]E*' -o -iname 'COPYING*' -o -iname 'NOTICE*' -o -iname 'UNLICENSE*' -o -iname 'COPYRIGHT*' \) | sort
}

licence_files_of() {
  local source=$1 folder=$2 found
  found=$(licence_files_in "$folder")
  if [ -z "$found" ] && [ "${source#git+}" != "$source" ]; then
    local parent=$folder
    while [ -z "$found" ] && [ "$parent" != "/" ]; do
      parent=$(dirname "$parent")
      case $parent in */checkouts/*/*) ;; *) break ;; esac
      found=$(licence_files_in "$parent")
    done
  fi
  printf '%s' "$found"
}

write_rust_notice() {
  local notice=$1 work
  work=$(mktemp -d)
  trap 'rm -rf "$work"' RETURN
  mkdir -p "$work/texts" "$work/users"
  local table
  table=$(crate_table)
  [ -n "$table" ] || die "cargo metadata listed no crates for ngine on $triple"

  local name crate_version licence repository source manifest folder file hash missing=""
  while IFS=$'\t' read -r name crate_version licence repository source manifest; do
    folder=$(dirname "$(unix_path "$manifest")")
    local files
    files=$(licence_files_of "$source" "$folder")
    if [ -z "$files" ]; then
      missing+="$name $crate_version ($licence)"$'\n'
      continue
    fi
    while IFS= read -r file; do
      hash=$(sha256_of "$file")
      [ -f "$work/texts/$hash" ] || cp "$file" "$work/texts/$hash"
      echo "$name $crate_version" >> "$work/users/$hash"
    done <<<"$files"
  done <<<"$table"

  {
    echo "Rust crates compiled into ngine $version for $triple"
    echo
    echo "ngine itself is MIT licensed; see ngine-LICENSE. It is built from these"
    echo "crates, listed with the licence each one declares. Their licence texts,"
    echo "as shipped in each crate, follow the list. Identical texts appear once."
    echo
    while IFS=$'\t' read -r name crate_version licence repository source manifest; do
      printf '%s %s\t%s\t%s\n' "$name" "$crate_version" "$licence" "$repository"
    done <<<"$table"
    if [ -n "$missing" ]; then
      echo
      echo "These crates ship no licence file; the licence named above applies:"
      printf '%s' "$missing"
    fi
    local text
    for text in "$work/texts"/*; do
      hash=$(basename "$text")
      echo
      echo "================================================================"
      echo "Used by: $(sort -u "$work/users/$hash" | paste -sd, - | sed 's/,/, /g')"
      echo "================================================================"
      echo
      cat "$text"
    done
  } > "$notice"
}

json_with_valid_escapes() {
  sed 's/\\\([$]\)/\\\\\1/g' "$1"
}

write_ffmpeg_source() {
  jq -r '
    "FFmpeg \(.ffmpeg.version) is bundled with ngine as separate shared libraries under",
    "the GNU Lesser General Public License, version 2.1 or later (FFmpeg-COPYING.LGPLv2.1).",
    "You may replace those libraries with your own build of the same FFmpeg version.",
    "",
    "Source code: \(.ffmpeg.url)",
    "SHA-256:     \(.ffmpeg.sha256)",
    "Build script and configuration: https://github.com/superuserkalo/ngine/tree/v'"$version"'/ffmpeg",
    "",
    "Configure flags:",
    (.configure[] | "  \(.)"),
    "",
    "Built into or shipped with these libraries:",
    (.external[] | "  \(.name) \(.version) (\(.licence), \(.use)): \(.url)")
  ' < <(json_with_valid_escapes "$ffmpeg_dir/manifest.json")
}

rm -rf "$destination"
mkdir -p "$destination"
cp "$repo_root/LICENSE" "$destination/ngine-LICENSE"
cp "$ffmpeg_dir"/licenses/* "$destination/"
write_ffmpeg_source > "$destination/FFmpeg-SOURCE.txt"
write_rust_notice "$destination/THIRD-PARTY-RUST.txt"
