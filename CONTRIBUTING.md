# Contributing to Meshvault

Thanks for helping. Bug reports, ideas, documentation fixes and code are all
welcome. Everyone taking part is expected to follow the
[code of conduct](CODE_OF_CONDUCT.md).

## Reporting bugs and asking for features

Open an [issue](https://github.com/studio-hills/meshvault/issues/new/choose)
with the form for a bug or an idea. For a bug, the most useful things are:

- what you did, what you expected, and what happened instead;
- Meshvault's version (the release you downloaded, or the commit you built)
  and your Windows version;
- for a Studio export problem, the `[Meshvault]` lines from Studio's Output
  window, including any stack trace;
- for a file that won't open or looks wrong, the asset id or the file itself,
  if you are allowed to share it.

Please don't post credentials, cookies, or other people's assets you don't
have the right to share. Report security problems privately, as
[SECURITY.md](SECURITY.md) describes.

## Building

Install Rust with [rustup](https://rustup.rs). `rust-toolchain.toml` pins the
toolchain, so the first build fetches it.

```sh
cargo run -p meshvault                 # a debug build
cargo run --release -p meshvault       # what users get
```

Meshvault targets Windows 10 and 11. Other platforms may build but are not
tested or supported yet.

## The repository

| Path | What it is |
| --- | --- |
| `crates/meshvault` | The app (egui and wgpu). Its modules are grouped by area: `scene`, `render`, `anim`, `workspace`, `studio`, `export`, `ui` |
| `crates/meshvault/studio` | The Roblox Studio exporters (Luau), built into the app and served to Studio |
| `crates/rbx_mesh` | The mesh parsing library, vendored from [krakow10/rbx_mesh](https://github.com/krakow10/rbx_mesh); its README says how it differs from upstream |
| `docs` | User documentation |
| `tools` | Development scripts: test workspaces and a stand-in Studio link ([README](tools/README.md)) |

## Before sending a pull request

Run these checks locally:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check            # needs cargo-deny: cargo install cargo-deny
```

- **Tests.** Fixes come with a test that fails without them. Tests live next
  to the code in `#[cfg(test)]` modules, and shared helpers are in
  `crates/meshvault/src/test_support.rs`. A few tests need real exports or
  workspaces, which are not committed; they are `#[ignore]`d, and
  [tools/README.md](tools/README.md) says how to run them.
- **The Studio exporters.** `studio/exporter_v1.luau` and `exporter_v2.luau`
  are the source. Their tests (`studio::link::exporter_tests`) run them in Luau
  against a stand-in Studio and the real link server, so most changes can be
  checked without Studio. If you change what an export contains, change the
  [schema docs](docs/export-schema.md) and the parser in
  `crates/meshvault/src/anim/schema.rs` and `studio/export_file.rs` with it.
  A change that older Meshvault builds could not read needs a new
  `SchemaVersion`.
- **Formatting.** `rustfmt.toml` sets hard tabs; `cargo fmt` does the rest.
  Text files use LF line endings, except PowerShell scripts (`*.ps1`), which use CRLF (`.gitattributes`).
- **Dependencies.** New crates must pass `cargo deny check` (licences,
  advisories, sources), set in `deny.toml`.
- **Docs and changelog.** Update the docs when behaviour users see changes,
  and add a line to `CHANGELOG.md` under *Unreleased*.
- **Commits.** Keep each commit to one change, with a short imperative subject
  ("Fix the grid's folder order"), and a body saying why when it isn't obvious.

## Licence

Meshvault is MIT licensed. By contributing you agree that your contribution is
licensed under the MIT licence; changes to `crates/rbx_mesh` are under its
licence, MIT OR Apache-2.0.
