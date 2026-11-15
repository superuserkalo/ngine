# Contributing to ngine

Thanks for helping. ngine is meant to stay small and light, and most of the
work of contributing is keeping it that way.

[AGENTS.md](AGENTS.md) holds the rules for code, tests and commits. They apply
to people and to coding agents alike. This file covers how to get started.

## Before you start

- For a bug, open an issue with your operating system, GPU, the version or
  commit, and the smallest steps that reproduce it.
- For a feature, open an issue first. Several large choices are still open and
  are listed under "Not decided" in [AGENTS.md](AGENTS.md).
- For a typo or a small fix, just send the pull request.

## Setting up

Install [rustup](https://rustup.rs). The Rust version is pinned in
`rust-toolchain.toml` and rustup installs it on first use. On Linux, also
install `libxkbcommon-dev` and `libwayland-dev`, or your distribution's
equivalents.

```sh
git clone https://github.com/superuserkalo/ngine.git
cd ngine
cargo run
```

## Running the checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI runs the same three commands on Linux, macOS and Windows.

## Pull requests

1. Branch from `main`.
2. Keep the change focused. Unrelated clean-ups belong in their own pull request.
3. Run the checks.
4. In the description, say what changed, why, and what you ran to verify it.
   Name the operating systems you ran it on. If something is untested on one of
   them, say so.

A new dependency needs a stated reason. A bug fix comes with a regression test
that fails without the fix.

## Licence

By contributing you agree that your contribution is licensed under the
[MIT licence](LICENSE).
