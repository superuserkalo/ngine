# ngine

[![Check](https://github.com/superuserkalo/ngine/actions/workflows/check.yml/badge.svg)](https://github.com/superuserkalo/ngine/actions/workflows/check.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A lightweight, free and open-source screen recorder and clipper for Windows,
macOS and Linux, written in Rust.

Record a game or anything else on your screen, then cut the parts worth keeping
into clips. One small native app, the same on every operating system.

## Status

ngine is at the start. The repository holds the Rust workspace, the checks and
the contribution rules. It does not record anything yet, and there is nothing
to download.

## Goals

- **Lightweight.** A small download that leaves the CPU and GPU to the game
  being recorded.
- **Cross-platform.** Windows, macOS and Linux from one codebase.
- **Native.** The interface is drawn on the GPU by Rust code, with no web view.
- **Free.** MIT licensed, with the source in this repository.

## Build from source

You need [rustup](https://rustup.rs). The Rust version is pinned in
`rust-toolchain.toml` and rustup installs it on first use.

```sh
git clone https://github.com/superuserkalo/ngine.git
cd ngine
cargo run
```

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md). Coding agents read
[AGENTS.md](AGENTS.md), which holds the rules for working in this repository.

Report security problems privately as described in [SECURITY.md](SECURITY.md).

## Licence

[MIT](LICENSE)
