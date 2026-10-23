# Making a release

Releases are built by `.github/workflows/release.yml` when a version tag is pushed.

1. Set the version in `crates/meshvault/Cargo.toml` and run `cargo build` so `Cargo.lock`
   follows it.
2. In `CHANGELOG.md`, give the version's section its date (`## [0.1.0] - 2026-11-01`) and
   start a new empty `## [Unreleased]` above it. The release notes are taken from that
   section; the workflow refuses a version still marked Unreleased.
3. Commit, push, and wait for CI to pass.
4. Tag and push the tag: `git tag v0.1.0 && git push origin v0.1.0`.
5. The workflow checks the dependencies (`cargo deny`, including security advisories: a
   release is refused while one is open), tests, builds and packages the app, then opens a
   **draft** release with
   `Meshvault-<version>-windows-x64.zip` and `SHA256SUMS.txt`. Check the draft, then publish it.

Running the workflow by hand (Actions → Release → Run workflow) builds and packages
without making a release; the zip is kept as a workflow artifact.

## What is in the zip

`meshvault.exe` (with its icon and version information, from `crates/meshvault/build.rs`),
the README, LICENSE, CHANGELOG, THIRD_PARTY_NOTICES.md, `THIRD_PARTY_LICENSES.html` (every
crate compiled into the app and its licence text, made by
[cargo-about](https://github.com/EmbarkStudios/cargo-about) from `about.toml` and
`about.hbs`), and the licences of the bundled fonts and of rbx_mesh in `licenses/`.

## Packaging locally

```sh
cargo build --release -p meshvault --locked
cargo install --locked cargo-about --features cli
cargo about generate -c packaging/about.toml --locked --fail -o target/THIRD_PARTY_LICENSES.html packaging/about.hbs
powershell -File packaging/package.ps1            # writes target/dist
```

## The icon

`crates/meshvault/assets/icons/meshvault.ico` and `meshvault.png` are drawn by
`tools/make_icon.py` (needs Pillow) from the Phosphor cube, the title bar's logo:

```sh
python tools/make_icon.py crates/meshvault/assets/fonts/phosphor/Phosphor.ttf crates/meshvault/assets/icons
```

The executable is not code-signed, so SmartScreen warns on first run; the release notes
say how to get past it.
