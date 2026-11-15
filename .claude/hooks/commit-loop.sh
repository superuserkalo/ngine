#!/bin/sh
# Stop hook: do not end a turn with uncommitted or unpushed work on main.
# Blocks once per stop; stop_hook_active lets the second attempt through.
input=$(cat)
case "$input" in
  *'"stop_hook_active":true'* | *'"stop_hook_active": true'*) exit 0 ;;
esac

cd "${CLAUDE_PROJECT_DIR:-.}" || exit 0
dirty=$(git status --porcelain 2>/dev/null)
ahead=$(git log --oneline '@{u}..' 2>/dev/null)
[ -z "$dirty" ] && [ -z "$ahead" ] && exit 0

{
  echo "The loop in AGENTS.md is not finished."
  if [ -n "$dirty" ]; then
    echo "Uncommitted changes:"
    echo "$dirty"
  fi
  if [ -n "$ahead" ]; then
    echo "Unpushed commits:"
    echo "$ahead"
  fi
  echo "Run the checks, commit with explicit paths, and push to main."
  echo "If the work cannot be finished, revert it or tell the owner why it stays."
} >&2
exit 2
