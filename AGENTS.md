# Working on ngine

ngine is a free, open-source screen recorder and clipper for Windows, macOS and
Linux, written in Rust. Being lightweight is the point of the product.

**Status: scaffold.** The binary prints its name and version. Nothing captures,
encodes, or opens a window yet. Never describe planned work as finished.

## Decided

- One Rust codebase for all three operating systems. A feature is done when it
  works on each, or the README states the gap.
- The UI is drawn natively on the GPU by a Rust crate. No web view.
- MIT licence. A dependency whose licence restricts that, such as a GPL build
  of FFmpeg, needs the owner's OK.

## Not decided

Do not settle these as a side effect of another task. Ask the owner, then
record the outcome in `docs/adr/`.

- **UI crate:** [gpui](https://crates.io/crates/gpui) or
  [Rune](https://github.com/anomalyco/rune).
- **Capture:** ScreenCaptureKit, Windows.Graphics.Capture and PipeWire through
  xdg-desktop-portal, called directly or through a crate.
- **Encoding and container:** the operating system's hardware encoders, or a
  bundled FFmpeg.
- **Audio:** which sources, and how they stay in sync with video.
- **Instant replay:** whether ngine keeps the last minutes in memory.

## Code rules

- **Avoid `unsafe`.** The workspace denies it. Use a safe crate or a safe API
  first. When an operating-system API leaves no safe option, put the `unsafe`
  in the smallest module that wraps it, mark that module
  `#![expect(unsafe_code, reason = "...")]`, give every block a `// SAFETY:`
  comment, and expose only a safe interface.
- **No comments.** A test fails on any. Say it in code: rename, extract a
  function, or encode the rule in a type or a test. Only three kinds stay: a
  doc comment stating a public item's contract, `// SAFETY:`, and a constraint
  forced by an OS, vendor or protocol that links its source with `https://`.
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
cargo deny check    # when dependencies change; needs cargo-deny installed
```

CI runs the same on Linux, macOS and Windows. Lints live in `Cargo.toml`,
banned APIs in `clippy.toml`, allowed licences and sources in `deny.toml`.

- Tests need no display, GPU, audio device, network or screen-recording
  permission. A test that does is marked `#[ignore]` with the reason.
- Tests write to the system temp directory and leave `git status` clean.
- A bug fix comes with a regression test that fails without the fix.
- Say which operating systems you ran on. CI proves that code compiles, not
  that capture works. Behaviour on an OS nobody ran is unverified.
- For a UI change, launch the app and look at it before reporting.

## The loop

Trunk-based: every change lands on `main`. No branches, worktrees or pull
requests for the owner's and agents' own work.

A milestone is the smallest change that works on its own. For each one:

1. Make the change.
2. Run the three checks.
3. Fix any line of this file that the change made stale.
4. Commit with explicit paths (`git add <path>...`, never `-A` or `.`). The
   subject is one plain sentence without a type prefix. Put the reason in the
   body when it is not obvious, and `Closes #n` when it finishes an issue.
5. `git push origin main`.
6. Check CI with `gh run list --limit 1` before the next push. If it is red,
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
- Update it in step 3 of the loop, in the same commit as the change. Never run
  a separate documentation clean-up.
- `docs/` holds only `docs/adr/` (one short file per decision) and
  `docs/agents/`. CI rejects anything else. Everything else goes in an issue.
