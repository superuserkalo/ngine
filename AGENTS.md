# Working on ngine

ngine is a free, open-source screen recorder and clipper for Windows, macOS and
Linux. It records games and other video, and cuts clips from what it recorded.
It exists because the alternatives are heavy, paid, or tied to one operating
system. Being lightweight is the point of the product.

**Status: scaffold.** The workspace builds and the binary prints its name and
version. Nothing captures, encodes, or opens a window yet. Do not describe
planned work as finished in code, documentation, or commit messages.

## Decided

- Rust, in one codebase for all three operating systems. A feature is done when
  it works on Windows, macOS and Linux, or when the gap is stated in the README.
- The UI is drawn natively on the GPU by a Rust crate. No web view: no Electron,
  no Tauri, no embedded browser.
- MIT licence, free to download. A dependency whose licence would restrict
  that, such as a GPL build of FFmpeg, needs the owner's explicit OK.

## Not decided

Do not settle one of these as a side effect of another task. Raise it with the
owner, and record the outcome as an ADR in `docs/adr/` once it is made.

- **UI crate.** Candidates: [gpui](https://crates.io/crates/gpui) (from Zed,
  Apache-2.0, on crates.io) and [Rune](https://github.com/anomalyco/rune) (MIT,
  pre-release).
- **Capture.** Each operating system has its own API: ScreenCaptureKit on
  macOS, Windows.Graphics.Capture on Windows, and PipeWire through the
  xdg-desktop-portal on Linux. Whether to call them directly or through a crate
  is open.
- **Encoding and container.** The operating system's hardware encoders against
  a bundled FFmpeg. This choice drives download size and licence.
- **Audio.** Which sources are recorded, and how they stay in sync with video.
- **Instant replay.** Whether ngine keeps the last minutes in memory so a clip
  can be saved after the moment happened.

## Layout

```text
Cargo.toml            workspace: shared version, lints, release profile
crates/ngine/         the application binary
docs/adr/             decisions, one file each, created when a decision is made
docs/agents/          how agent skills use the issue tracker, labels and domain docs
.scratch/             ignored working files
```

Add a crate when a concrete responsibility needs its own boundary, for example
keeping capture logic testable without the UI. Do not add crates for work that
is not started.

## Code rules

- **Lightweight is a requirement.** A new dependency needs a reason you can
  state in the commit message. Run `cargo tree` before and after adding one and
  look at what it pulls in. Prefer the standard library and the operating
  system's own APIs over a large crate.
- **Pin direct dependencies to exact versions** (`= "=1.2.3"`) and commit
  `Cargo.lock`.
- **Keep operating-system code behind a boundary.** Code that calls an OS API
  lives in a module gated with `#[cfg(target_os = "...")]`. Shared code calls a
  common interface and never names an OS API.
- **Every `unsafe` block has a `// SAFETY:` comment** that states why it is
  sound. Clippy enforces this.
- **No `unwrap()` outside tests.** A recorder that panics loses the user's
  recording. Return an error, or use `expect` with a message that states the
  invariant. Clippy enforces this.
- **Never lose a recording.** A failure during capture or encoding must leave
  the bytes already written playable.
- **Do not weaken a lint or a test to make it pass.** Fix the code, or say why
  the rule is wrong and change it deliberately.

## Verification

Run all three before you say a change is done, and report the real result:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI runs the same commands on Linux, macOS and Windows; see
[.github/workflows/check.yml](.github/workflows/check.yml).

- **Tests run without a display.** No screen-recording permission, no GPU, no
  network. A test that needs a real display, GPU or audio device is marked
  `#[ignore]` with the reason, and is run by hand.
- **Tests write to the system temp directory** and remove what they create. A
  test run must leave `git status` clean.
- **A bug fix comes with a regression test** that fails without the fix.
- **Say which operating systems you ran on.** CI proves that code compiles and
  that tests pass. It does not prove that capture works. Report behaviour on an
  operating system as unverified until someone has run it there.
- For a UI change, launch the app and look at the result before reporting it.

## Git

- Work on `main`. No branches or worktrees unless asked.
- Inspect `git status` first and preserve unrelated changes.
- Commit each completed, verified change before starting the next. One logical
  change per commit, staging explicit paths (`git add <path>...`, never
  `git add -A` or `git add .`).
- Write the subject as one plain sentence that says what changed, without a
  type prefix. Put the reason in the body when it is not obvious.
- Push only when the owner asks.
- Never commit recordings, clips, screenshots with personal content,
  credentials, or build output.

## Agent skills

### Issue tracker

Issues live in GitHub Issues for `superuserkalo/ngine`, managed with the `gh`
CLI. See [docs/agents/issue-tracker.md](docs/agents/issue-tracker.md).

### Triage labels

Default labels: `needs-triage`, `needs-info`, `ready-for-agent`,
`ready-for-human`, `wontfix`. See
[docs/agents/triage-labels.md](docs/agents/triage-labels.md).

### Domain docs

Single-context: root `CONTEXT.md` plus `docs/adr/`, both created when a term or
decision is settled. See [docs/agents/domain.md](docs/agents/domain.md).
