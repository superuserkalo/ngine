---
name: no-comments
description: Remove banned comments from changed Rust files with the cheap no-comments subagent, then fix the code it flags. Use before committing any change to a .rs file, or when asked to clean comments.
argument-hint: "[files | all]"
---

# No comments

The `no-comments` subagent runs on Haiku and does the reading. You act on its report. Do not read the files for comments yourself and do not restate its rules to it.

## Steps

1. **Scope.** Use the files in the arguments. `all` means every `.rs` file under `crates/`. With no arguments, use the `.rs` files changed since the last commit, new files included.
2. **Skip when there is nothing to judge.** Run `git diff HEAD -U0 -- '*.rs' | grep -E '^\+.*(//|/\*|#\[(allow|expect))'` and check `git status --short` for new `.rs` files. If no line and no new file turns up, stop and say `clean`. Skip this step for `all` or named files.
3. **Spawn** the `no-comments` subagent with the scope, one line.
4. **Check its edits** with `git diff`. It may delete comment text and nothing else. Restore any code it changed.
5. **Act on the report.**
   - `FIX`: make the smallest change in scope that removes the need for the comment. Do not restore the comment. If the real fix is out of scope, open an issue.
   - `ISSUE`: `gh issue create` with the line it gave and the label `needs-triage`.
   - `SOURCE`: add the link to the vendor documentation or issue. If you cannot find one, delete the comment.
   - `SUPPRESS`: fix the code so the lint passes, or give the `#[expect]` a reason that names the false positive.
6. **Run the checks**, then continue the loop in `AGENTS.md`.

Report in one line: comments deleted, fixes made, issues opened.
