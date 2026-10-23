# Meshvault

Meshvault is a desktop app for Roblox meshes. It opens `.mesh` files and their
materials in a 3D view, downloads every mesh, texture and animated model a game
uses into a folder you can browse, plays those models' animations as Roblox
does, and exports them to Blender.

<!-- screenshot: the library, viewport and inspector with an animated model -->

- **View** any Roblox mesh version (v1 to v7, including Draco-compressed v6/v7),
  union and physics files, with their colour, normal, roughness and metalness
  maps, LODs, hulls and bones.
- **Export from Roblox Studio** with one Command Bar line: Meshvault serves the
  exporter itself and receives what it sends.
- **Download** a game's meshes and maps in bulk into a *workspace* that mirrors
  the game's folders, each asset once, carrying on after a restart.
- **Play** animated models: Motor6D and bone rigs, skinned meshes, keyframe and
  curve clips with Roblox's own easing, and facial animation on FACS heads.
- **Export** to Blender as glTF (one armature, every clip baked) or to OBJ, and
  save stills or turntable GIFs.

Meshvault runs on Windows 10 and 11. It draws with the GPU (wgpu) and falls
back to a simpler CPU renderer, without most materials and image saving,
when no GPU backend is available.

## Install

Release builds will be on the [Releases](https://github.com/studio-hills/meshvault/releases)
page. Until then, build from source:

1. Install Rust with [rustup](https://rustup.rs). The repository pins the
   toolchain (`rust-toolchain.toml`), so rustup fetches the right version itself.
2. Build and run:

   ```sh
   git clone https://github.com/studio-hills/meshvault
   cd meshvault
   cargo run --release -p meshvault
   ```

   The app is `target/release/meshvault.exe`. It can also open a file given on
   the command line: `meshvault path/to/model.mesh`.

## Quick start

1. **Make a workspace.** In the workspace menu in the top bar choose
   *New workspace…* (it goes under `Documents/Meshvault` unless you pick
   another folder).
2. **Add Roblox access.** Roblox only serves assets to signed-in requests.
   Click the key icon (Ctrl+,), paste an
   [Open Cloud API key](https://create.roblox.com/dashboard/credentials) with
   the `legacy-asset:manage` permission, and click Save. The key is kept in
   Windows Credential Manager. To try it first, enter an asset id you can
   access under *Test with asset id* and click Test.
3. **Export from Studio.** Open *Export from Roblox Studio…* (the ⋮ menu, the
   welcome screen, or *Studio link* in the status bar), copy the line, and paste
   it into Studio's Command Bar in Edit mode. Meshvault receives the export and
   opens it for import.
4. **Import and download.** Pick the folders, models and maps to import.
   Meshvault downloads them in the background. After a restart, click
   Download in the top bar (Ctrl+D) to carry on.
5. **View.** Click a model in the library. Animated models play from the
   transport bar (Space plays or pauses).
6. **Export to Blender.** Press Ctrl+E for a `.glb`. Blender's glTF importer
   reads an animated model as one armature, with every clip as an action.

Press `?` in the app for the keyboard shortcuts.

## Documentation

- [User guide](docs/guide.md): the viewer, the library, animated models,
  exporting to Blender and OBJ
- [Exporting from Roblox Studio](docs/studio-export.md): the Command Bar line,
  the exporter's options, what it reports, and the Studio link
- [Export file format](docs/export-schema.md): schemas 0, 1 and 2
- [Workspaces and downloads](docs/workspaces.md): the folder layout,
  credentials, and where settings live
- [Keyboard shortcuts and controls](docs/shortcuts.md)
- [Development tools](tools/README.md): test workspaces, the stand-in Studio
  link, and the tests that need real data

## Using it responsibly

Meshvault downloads assets through Roblox's own asset delivery, with your own
credentials, so it can only reach what your account can. Assets on Roblox
belong to their creators: keep to the [Roblox Terms of Use](https://en.help.roblox.com/hc/en-us/articles/115004647846-Roblox-Terms-of-Use)
and the creators' rights when you use what you download.

Meshvault is not affiliated with, endorsed by or sponsored by Roblox
Corporation. Roblox and Roblox Studio are trademarks of Roblox Corporation.

## Contributing

Bug reports, ideas and pull requests are welcome; see
[CONTRIBUTING.md](CONTRIBUTING.md). Please report security problems privately,
as [SECURITY.md](SECURITY.md) describes.

## Credits and licence

Meshvault is MIT licensed ([LICENSE](LICENSE)).

Its mesh parsing is [rbx_mesh](https://github.com/krakow10/rbx_mesh) by Rhys
Lloyd (krakow10) and contributors, vendored in [`crates/rbx_mesh`](crates/rbx_mesh)
with mesh v6/v7 and Draco decoding added (MIT OR Apache-2.0). The interface
uses the [Cal Sans](https://github.com/calcom/sans) typeface (SIL Open Font
License 1.1) and [Phosphor](https://phosphoricons.com) icons (MIT), and is
built with [egui](https://github.com/emilk/egui) and [wgpu](https://wgpu.rs).
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) credits the vendored library,
the bundled fonts and artwork, and explains the Rust dependencies' licences.
