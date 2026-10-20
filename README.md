Roblox Mesh Parser
==================

[![Latest version](https://img.shields.io/crates/v/rbx_mesh.svg)](https://crates.io/crates/rbx_mesh)
![License](https://img.shields.io/crates/l/rbx_mesh.svg)

`rbx_mesh` is a collection of deserializers for the different mesh versions in Roblox, and a function to detect the version and invoke the correct deserializer.  The meshes are decoded into a structure that reflects the on-disk format of the particular mesh version, rather than being translated into a catch-all structure.

## Mesh Example

Print the mesh vertices for any mesh version / vertex size

```rust
use rbx_mesh::read_mesh_versioned;
use rbx_mesh::mesh::{Chunks6,CoreMeshChunk,Mesh,Vertices2};

let file=std::fs::read("meshes/torso.mesh")?;
let versioned_mesh=read_mesh_versioned(std::io::Cursor::new(file))?;
match versioned_mesh{
	Mesh::V1(mesh)=>println!("{:?}",mesh.vertices),
	Mesh::V2(mesh)=>{
		match mesh.vertices{
			Vertices2::Full(vertices)=>println!("{:?}",vertices),
			Vertices2::Truncated(vertices_truncated)=>println!("{:?}",vertices_truncated),
		}
	},
	Mesh::V3(mesh)=>{
		match mesh.vertices{
			Vertices2::Full(vertices)=>println!("{:?}",vertices),
			Vertices2::Truncated(vertices_truncated)=>println!("{:?}",vertices_truncated),
		}
	},
	Mesh::V4(mesh)=>println!("{:?}",mesh.vertices),
	Mesh::V5(mesh)=>println!("{:?}",mesh.vertices),
	// v6 and v7 are made of chunks; the vertices live in the COREMESH chunk
	Mesh::V6(mesh)=>{
		if let Some(CoreMeshChunk::V1(core))=mesh.core_mesh(){
			println!("{:?}",core.vertices);
		}
	},
	// v7 usually Draco compresses COREMESH (decode needs the default "mesh-v7-draco" feature)
	Mesh::V7(mesh)=>{
		match mesh.core_mesh(){
			Some(CoreMeshChunk::V1(core))=>println!("{:?}",core.vertices),
			Some(CoreMeshChunk::V2(draco))=>println!("{:?}",draco.decode().unwrap().vertices),
			_=>(),
		}
	},
}
# binrw::BinResult::Ok(())
```

## Union Graphics Example
```rust
use rbx_mesh::read_union_graphics_versioned;
use rbx_mesh::union_graphics::UnionGraphics;

// this data is extracted from the "MeshData" property of UnionOperation
// the data is not usually contained in the roblox file itself
// but is sourced from the associated "AssetId" of the UnionOperation
let mesh_file=std::fs::read("meshes/4500696697_4.meshdata")?;
let mesh=read_union_graphics_versioned(std::io::Cursor::new(mesh_file))?;

// print mesh vertices or vertex positions
match mesh{
	UnionGraphics::CSGK(_)=>(),
	UnionGraphics::V2(mesh2)=>println!("{:?}",mesh2.mesh.vertices),
	UnionGraphics::V4(mesh4)=>println!("{:?}",mesh4.mesh.vertices),
	UnionGraphics::V5(mesh5)=>println!("{:?}",mesh5.positions),
}
# binrw::BinResult::Ok(())
```

## Union Physics Example
```rust
use rbx_mesh::read_union_physics_versioned;
use rbx_mesh::union_physics::UnionPhysics;

// this data is extracted from the "PhysicsData" property of UnionOperation
let phys_file=std::fs::read("meshes/CSGPHS_3.data")?;
let mesh=read_union_physics_versioned(std::io::Cursor::new(phys_file))?;

// Meshes contain multiple convex hulls.
// print vertex positions of the first mesh
match mesh{
	// Who writes a uuid as ascii hex in a binary format!?
	UnionPhysics::CSGK(_csgk)=>println!("CSGK"),
	UnionPhysics::Block(_block)=>println!("CSGPHS Block"),
	UnionPhysics::V3(mesh3)=>println!("{:?}",mesh3.meshes[0].positions),
	UnionPhysics::V5(mesh5)=>println!("{:?}",mesh5.meshes[0].positions),
	UnionPhysics::V6(mesh6)=>println!("{:?}",mesh6.mesh.positions),
	UnionPhysics::V7(mesh7)=>println!("{:?}",mesh7.meshes[0].positions),
	UnionPhysics::V8(mesh8)=>println!("{:?}",mesh8.mesh.hulls.iter_hulls().next().unwrap().positions),
}
# binrw::BinResult::Ok(())
```

## Meshvault (GUI)

The `gui` crate is Meshvault, a desktop viewer built on `rbx_mesh` and [egui](https://github.com/emilk/egui).  It auto-detects the file type, shows header details (version, vertex/face counts, LODs, bones, FACS, hulls, physics info) and renders the mesh in an orbitable 3D view.

```sh
cargo run --release -p meshvault -- meshes/sphere.mesh
```

- Three-pane layout: a library of workspace models, the 3D viewport with a floating toolbar, and an inspector for the model's stats, material and properties; both side panels can be hidden
- Dark and light themes, notifications for long-running work, and keyboard shortcuts (press `?` in the app for the list)
- Open files with the Open button (Ctrl+O), drag and drop, or pass a path on the command line
- Switch between LODs for mesh v3+ files (including v6/v7); toggle individual convex hulls for union physics files
- Solid, wireframe or combined shading, a ground grid, and optional back-face culling
- GPU rendering (wgpu): depth-tested, multisampled, mipmapped, with physically based shading; falls back to a CPU renderer when no GPU backend is available
- Materials: base color, normal, roughness and metalness maps (Roblox `SurfaceAppearance`).  Maps next to the mesh load automatically by name (`Name.png` or `Name.color.png`, `Name.normal.png`, `Name.roughness.png`, `Name.metalness.png`); otherwise use the inspector's Material section or drop PNG/JPEG/BMP/TGA/WebP files onto the window.  Each map can be toggled to compare with and without it
- Export the visible geometry as Wavefront OBJ
- Controls: left-drag to orbit, right/middle-drag to pan, scroll to zoom, double-click or F to frame the model

Meshvault bundles the [Cal Sans](https://github.com/calcom/sans) typeface (SIL Open Font License 1.1) and [Phosphor](https://phosphoricons.com) icons (MIT); see `gui/assets/` for their licenses.

### Workspaces and bulk download

A workspace is a folder of models downloaded from Roblox, organized to mirror the folders of an export file.  Two export layouts are read:

- Schema 0, an array: `[{"Name", "MeshId", "Folder", "TextureId"?, "ColorMap"?, "NormalMap"?, "RoughnessMap"?, "MetalnessMap"?}, ...]`
- Schema 1, an object: `{"SchemaVersion": 1, "Meshes": [{"Id", ...the fields above}], "Models": [...]}`, where each model carries its parts, Motor6D joints, bones and animation clips (KeyframeSequence or CurveAnimation) and refers to meshes by `Id`
- Schema 2, the same with each model's `Scale`, each part's `ClassName`, `Shape`, `Color`, `BrickColor`, `Material`, `Reflectance` and `SpecialMesh`, and each joint's captured `Transform`

Other `SchemaVersion` values are rejected.  Empty strings count as missing.  Content ids can be `rbxassetid://N`, `http(s)://www.roblox.com/asset/?id=N` or `assetdelivery.roblox.com` URLs.  Studio output split into `=== JSON CHUNK i/n ===` blocks can be imported as is.

1. Open the workspace menu in the top bar and choose New workspace… (created under `Documents/Meshvault` by default)
2. Click the key icon (Roblox access, Ctrl+,) and add a credential (see below), then press Test
3. Click Import (Ctrl+I), or drop the `.json` onto the window, then pick the folders, meshes and maps to import
4. Browse the workspace tree; click a downloaded model to view it with its texture

```text
<workspace>/workspace.json            items and their download state
<workspace>/Rigs/<Path>.rig.json       an animated model, as a schema 1 export of its own
<workspace>/Models/<Folder>/<Name>.mesh
<workspace>/Models/<Folder>/<Name>.png  TextureID, next to its mesh
<workspace>/Models/<Folder>/<Name>.color.png, .normal.png, .roughness.png, .metalness.png
<workspace>/.cache/<asset id>          each asset is downloaded once, even when reused
```

Roblox requires authentication to download assets.  Use an [Open Cloud API key](https://create.roblox.com/dashboard/credentials) with the `legacy-asset:manage` permission (recommended), or a `.ROBLOSECURITY` cookie.  Credentials are kept in the system credential store (Windows Credential Manager, macOS Keychain or Secret Service), never in the workspace or config files.  Downloads run in parallel, retry rate limits and server errors, resolve Decal ids to their image, and resume after the app is closed.  Each asset id is downloaded once even when it is used as several maps or by many models.  The import dialog can include or skip each map type.  Re-importing an export only adds what is new.

The app also accepts a workspace folder and an export on the command line: `meshvault <workspace> <export.json>`.

### Exporting from Roblox Studio

Meshvault serves the exporter itself, so there is no script to keep up to date. Open Export from Roblox Studio… (in the ⋮ menu, on the welcome screen, or by clicking "Studio link" in the status bar), copy the line, and paste it into Studio's Command Bar in Edit mode:

```lua
local h=game:GetService("HttpService") local e=h.HttpEnabled h.HttpEnabled=true local ok,s=pcall(h.GetAsync,h,"http://localhost:34875/export.luau") h.HttpEnabled=e assert(ok,"Could not reach Meshvault (is it open, and is Studio in Edit mode, not Play?): "..tostring(s)) loadstring(s)()()
```

It exports what is selected in Studio: a model, or anything inside one (a part, its Animator), exports the whole model, even one without Animation objects. With nothing selected it exports the whole game except Workspace, and when nothing exportable is found it says so in the Output window instead of sending an empty export. The script returns a function taking options, so `loadstring(s)()({roots = {workspace.MyRig}, exclude = {}})` picks the roots by hand. It writes the newest schema; add `?schema=1` to the URL for the schema 1 exporter. Meshvault saves the export to `Documents/Meshvault/Exports/<place>-<time>.json` and opens it for import (making a workspace first if none is open). If sending fails, the exporter prints `=== JSON CHUNK ===` blocks instead, which can be pasted into a file and imported as before.

The exporter reports each step in the Output window, every line starting with `[Meshvault]`: what it exports (and which model a selection stands for), each model with its parts, joints, bones and clips, the totals, and the sending. Problems are warnings that say what to do: clips that could not be loaded, models or meshes it had to skip, Meshvault not running, a play test in progress. One bad instance skips only itself. An unexpected error prints its stack trace with the exporter's schema and Meshvault's version for reporting, and the run ends with a count of warnings.

The function also returns those problems, as a list that is empty when all went well, so a script can act on them:

```lua
local issues = loadstring(s)()()
for _, issue in issues do
	print(issue.Kind, issue.Message, issue.Path or issue.AnimationId)
end
```

`Kind` is one of `PlayMode`, `AnimationUnavailable`, `CurveAnimationFailed`, `MeshSkipped`, `ModelSkipped` (with the instance's `Path`), `NothingToExport`, `SendFailed`, `HttpEnabledNotRestored` or `Error` (an unexpected error, with its stack trace in `Message`). The animation kinds carry the `AnimationId`.

Everything travels over `http://localhost:34875`: `GET /export.luau` serves the exporter, and `POST /export` takes the export in 2 MB pieces. The line turns `HttpService.HttpEnabled` on only while it talks to Meshvault and restores it afterwards. Meshvault accepts connections from this machine only, and requires custom headers on exports, which a web page cannot send. Only one program can use the port: if it is taken (usually by another Meshvault), Meshvault warns at startup, the status bar reads "Studio link unavailable", and exporting from Studio is off until the port is free and Meshvault restarts. The exporters live in `gui/studio/` and are built into the app.

### Animated models

Schema 1 exports list animated models in the import dialog; selecting one also selects the meshes its parts use.  In the library they appear under Animated models; click one (or open its `.rig.json`) to play it:

- Parts are placed by their Motor6D joints (`Part1 = Part0 * C0 * Transform * C1⁻¹`) and bones (`Parent * CFrame * Transform`); MeshPart meshes are centred and scaled to each part's size, SpecialMesh file meshes keep their own units times `Scale`, plus `Offset`, and fully transparent parts are hidden
- Parts without a file mesh are drawn as their Roblox shape (block, ball, cylinder, wedge, corner wedge, or a SpecialMesh brick, sphere or cylinder) in their part colour; anything Meshvault has no shape for (trusses, unions, SpecialMesh heads) is drawn approximately and listed in the warnings
- Schema 2 models show the pose they were exported in (each joint's captured `Transform`), and clip translations are scaled by the model's `Scale`, as `Model:ScaleTo` does in Roblox. Schema 1 has neither, so joints start at rest and bone translations are scaled by the skinned mesh's own scale
- Skinned meshes (mesh v4 and later) bend with their bones.  Each bone the mesh was bound to follows a Bone in the model, else a part of the same name (as R15 bodies do), else its bind pose from the mesh file.  Meshes Draco-compressed with edgebreaker stay rigid, since their vertices no longer line up with their weights
- The transport bar plays, pauses, scrubs, loops and changes speed (Space plays or pauses), with clip markers on the track.  Clips the exporter could not read are listed with their status and cannot be played
- The inspector lists clips, tracks that match nothing in the model, parts with visibility toggles, and warnings such as missing meshes or a rig captured mid-animation

Facial animation (FACS) is not part of the export, so face bones hold their bind pose unless a clip animates them by name. Roblox materials other than their colour, reflectance and partial transparency are not rendered yet.

#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>
