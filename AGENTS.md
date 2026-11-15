# Working on ngine

ngine is a free, open-source screen recorder and clipper for Windows, macOS and
Linux, written in Rust. Being lightweight is the point of the product.

**Status: an empty window.** The app opens a window that shows its name and
version. Nothing captures or encodes yet. Never describe planned work as
finished.

## Decided

- One Rust codebase for all three operating systems. A feature is done when it
  works on each, or the README states the gap.
- The UI is [Rune](https://github.com/anomalyco/rune) at one pinned git
  revision ([ADR 0001](docs/adr/0001-rune-for-the-ui.md)). No web view. To
  update it, give `rune` and every `[patch.crates-io]` entry the same `rev`.
- MIT licence. A dependency whose licence restricts that needs the owner's OK.
- Encoding and MP4 use a trimmed LGPL FFmpeg, bundled as shared libraries with
  nothing GPL in it ([ADR 0003](docs/adr/0003-ffmpeg.md)). Recording is SDR.
- Audio is system sound and microphone in three tracks: the mix, then each
  source ([ADR 0004](docs/adr/0004-audio.md)).
- Capture is a crate of our own over one safe crate per operating system
  ([ADR 0002](docs/adr/0002-capture.md)). It never touches another process.
- Instant replay keeps encoded video in memory up to a budget, then on disk
  ([ADR 0005](docs/adr/0005-instant-replay.md)).

A choice that changes what the product does or ships is the owner's. Do not
settle one as a side effect of another task. Ask, then record it in `docs/adr/`.

## Code rules

- **Avoid `unsafe`.** The workspace denies it. Use a safe crate or a safe API
  first. When an operating-system API leaves no safe option, put the `unsafe`
  in the smallest module that wraps it, mark that module
  `#![expect(unsafe_code, reason = "...")]`, give every block a `// SAFETY:`
  comment, and expose only a safe interface.
- **No comments.** Say it in code: rename, extract a function, or encode the
  rule in a type or a test. The `no-comments` skill removes them and its
  subagent lists the few kinds that stay.
- **Stay lightweight.** A new dependency needs a reason stated in the commit
  message. Compare `cargo tree` before and after. Pin it to an exact version
  (`"=1.2.3"`) and commit `Cargo.lock`.
- **Keep operating-system code behind a boundary.** It lives in modules gated
  with `#[cfg(target_os = "...")]`. Shared code never names an OS API.
- **No `unwrap()` outside tests.** A panic loses the user's recording. Return
  an error, or use `expect` with the invariant as its message.
- **Never lose a recording.** A failure during capture or encoding must leave
  the bytes already written playable.
- **Add a crate only when a concrete responsibility needs its own boundary.**
- **Do not weaken a lint or a test to make it pass.** Suppress one only with
  `#[expect(..., reason = "...")]`.

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check advisories bans sources    # when dependencies change
```

CI runs the same on Linux, macOS and Windows. Lints live in `Cargo.toml`,
banned APIs in `clippy.toml`, allowed sources in `deny.toml`. Any licence is
fine except a GPL-type one, which CI rejects.

- Tests need no display, GPU, audio device, network or screen-recording
  permission. A test that does is marked `#[ignore]` with the reason.
- Tests write to the system temp directory and leave `git status` clean.
- A bug fix comes with a regression test that fails without the fix.
- Say which operating systems you ran on. CI proves that code compiles, not
  that capture works. Behaviour on an OS nobody ran is unverified.
- For a UI change, render it headless into `.scratch/` and look at it:
  `cargo run -- --snap out.png`, or `--tree out.json` for the element tree.

## The loop

Trunk-based: every change lands on `main`. No branches, worktrees or pull
requests for the owner's and agents' own work.

A milestone is the smallest change that works on its own. For each one:

1. Make the change.
2. If a `.rs` file changed, run the `no-comments` skill.
3. Run the three checks.
4. Fix any line of this file that the change made stale.
5. Commit with explicit paths (`git add <path>...`, never `-A` or `.`). The
   subject is one plain sentence without a type prefix. Put the reason in the
   body when it is not obvious, and `Closes #n` when it finishes an issue.
6. `git push origin main`.
7. Check CI with `gh run list --limit 1` before the next push. If it is red,
   fix it in the next commit.

Then start the next milestone. Do not wait to be asked to commit or push.

- Never end a turn with uncommitted or unpushed changes. If work cannot be
  finished, revert it or tell the owner why it stays.
- Never push failing checks. Never force-push or rewrite `main`.
- Preserve changes you did not make. Never commit recordings, credentials or
  build output.

## Issues

Work is tracked in GitHub Issues for `superuserkalo/ngine` with the `gh` CLI:
bugs, features, research, plans and open questions. Never keep them in files.

Triage labels: `needs-triage`, `needs-info`, `ready-for-agent`,
`ready-for-human`, `wontfix`. Skill details are in `docs/agents/`.

## This file

- Budget: 120 lines. CI fails above it. To add a line, cut or tighten another.
- Keep only what code, `git log` and issues cannot tell an agent: decisions,
  rules and the loop. No history, status, dated notes or plans.
- A rule belongs in a check before it belongs here. When a mistake happens
  twice, block it with a lint, a type or a test whose message names the fix.
- Update it in step 4 of the loop, in the same commit as the change. Never run
  a separate documentation clean-up.
- `docs/` holds only `docs/adr/` (one short file per decision) and
  `docs/agents/`. CI rejects anything else. Everything else goes in an issue.
