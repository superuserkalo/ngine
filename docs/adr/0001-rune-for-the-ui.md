# The UI is built with Rune

Status: accepted on 2026-11-15

ngine draws its interface with [Rune](https://github.com/anomalyco/rune), pinned to one git revision. gpui was the other candidate.

## Why Rune

Rune has the pieces a recorder needs. It draws streamed pixel frames, shows click-through overlays above other apps on all three operating systems, runs windowless behind a tray, registers global shortcuts, and exposes a window's macOS window number so a capture can leave an overlay out. Every app also runs headless: it renders to PNG and dumps its element tree, so an agent can check a UI change without a screen.

gpui is proven in Zed on all three operating systems, which Rune is not. Its crates.io release is a year old, so it would also be a git dependency.

## What was tested

A spike drew a 1920x1080 frame through `Pixels`, showed an edge overlay and pushed frames at 60 Hz. It built, rendered headless and opened a real window on Linux, macOS and Windows in CI, with the same picture on each. On an Apple silicon Mac, streaming 1080p used about 9% of one core and an idle window used none.

Not tested: real GPU drivers on Windows and Linux (the CI runners have none), Wayland, and whether the overlay is visible on screen.

## What it costs

- Rune was 24 days old when chosen. It has no releases and takes no pull requests, and its own CI runs macOS only. Windows and Linux bugs are ours to find and report.
- It is a git dependency. `rune` and the five crates under `[patch.crates-io]` must share one `rev`. `deny.toml` allows that one git source.
- Its `ogg-audio` feature is off, because it pulls in MPL-2.0 crates. The other default features are listed by hand in `crates/ngine/Cargo.toml`, so a new Rune default is not picked up until it is added there.
- Three unmaintained-crate advisories from its Linux D-Bus stack are ignored by ID in `deny.toml`.
- Video playback and zero-copy surfaces are macOS and Windows only. On Linux, frames go through `Pixels`, which copies on the CPU.
- A stripped release build is about 18 MB on macOS. Rust is pinned to 1.99.1 to match Rune.

## When to revisit

If a Windows or Linux defect blocks a release and Rune does not fix it, or if an update breaks the app more often than it can be re-checked. gpui is the fallback.
