# Third-party notices

Meshvault is MIT licensed ([LICENSE](LICENSE)). It includes the following work
by others.

## rbx_mesh

`crates/rbx_mesh` is [rbx_mesh](https://github.com/krakow10/rbx_mesh) by Rhys
Lloyd (krakow10) and contributors, vendored at upstream commit `1ea4cc4`
(0.8.2), with mesh v6/v7 and Draco decoding added for Meshvault. It is
licensed under either of:

- the MIT licence: [crates/rbx_mesh/LICENSE-MIT](crates/rbx_mesh/LICENSE-MIT)
- the Apache License, Version 2.0: [crates/rbx_mesh/LICENSE-APACHE](crates/rbx_mesh/LICENSE-APACHE)

at your option. Its changes from upstream are listed in
[crates/rbx_mesh/CHANGELOG.md](crates/rbx_mesh/CHANGELOG.md).

## Cal Sans

The interface typeface. Copyright 2021 The Cal Sans Project Authors
(https://github.com/calcom/font), licensed under the SIL Open Font License,
Version 1.1. The licence is in
[crates/meshvault/assets/fonts/cal-sans/OFL.txt](crates/meshvault/assets/fonts/cal-sans/OFL.txt).

## Phosphor Icons

The interface icons, from [Phosphor](https://phosphoricons.com) as packaged by
[egui-phosphor](https://github.com/amPerl/egui-phosphor). Copyright (c) 2023
Phosphor Icons, MIT licence. The font is subset to its icon code points; the
licence and how it was subset are in
[crates/meshvault/assets/fonts/phosphor/LICENSE.txt](crates/meshvault/assets/fonts/phosphor/LICENSE.txt).

## Blender logo

`crates/meshvault/assets/icons/blender.svg` is the Blender logo, used only to
label the Export for Blender action. The Blender logo is a trademark of the
Blender Foundation; Meshvault is not affiliated with or endorsed by the
Blender Foundation.

## Rust crates

Meshvault is built from many open-source Rust crates (egui, wgpu, draco-core,
serde, ureq and others), each under its own licence. All are on the allow
list in [`deny.toml`](deny.toml): MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0,
Unicode, OFL and similar. `Cargo.lock` lists every crate and version. Release
packages are to include the full licence text of each crate compiled into the
app; until then, each crate's licence is in its own source.

## Trademarks

Roblox and Roblox Studio are trademarks of Roblox Corporation. Meshvault is
not affiliated with, endorsed by or sponsored by Roblox Corporation.
