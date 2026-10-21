# Meshvault

Meshvault is a desktop viewer built on `rbx_mesh` and [egui](https://github.com/emilk/egui).  It auto-detects the file type, shows header details (version, vertex/face counts, LODs, bones, FACS, hulls, physics info) and renders the mesh in an orbitable 3D view.

```sh
cargo run --release -p meshvault -- crates/rbx_mesh/meshes/sphere.mesh
```

- Three-pane layout: a library of workspace models, the 3D viewport with a floating toolbar, and an inspector for the model's stats, material and properties; both side panels can be hidden
- The library shows the export's folder tree, or a grid of thumbnails grouped by folder (the button beside the title switches). Each model is drawn once, when it first scrolls into view, and its picture is kept in the workspace's `.cache/thumbnails`
- Search by name, folder or asset id, narrow by download status, and filter (the funnel beside the search) to models with animations, skinned meshes (read once in the background and kept in `.cache/facts.json`), missing meshes, or a given map
- Select several models with Ctrl-click and Shift-click (Ctrl+A selects everything shown, Esc clears), then retry their failed downloads, export them, or remove them at once from the bar over the library or the right-click menu
- Export for Blender (Ctrl+E, the export button over the viewport, or a library model's menu): binary glTF (`.glb`) that Blender's glTF importer reads as one armature. Every part and Bone is a bone of it, every mesh is skinned to it (a skinned mesh with its own weights, a rigid part's mesh fully to its part), and every playable clip is baked at 30 frames a second into its own action, so Blender plays it as Meshvault does: easing, bones only meshes have, and facial animation's bone movements included. Colour, normal, roughness and metalness maps are embedded. Lengths are in studs, which Blender reads as metres. Decals, clothing and MaterialVariants are not exported yet
- Export as Wavefront OBJ (Ctrl+Shift+E), with an MTL file and the colour and normal maps copied beside it: a mesh's visible LODs or hulls, or an animated model as one OBJ of its parts as posed now
- Save the view as a PNG (1080 pixels tall, transparent background; the camera button over the viewport or F12), copy it to the clipboard, or save a turntable GIF: the camera turns once around the model while the selected clip plays
- Lighting presets (the bulb over the viewport, or L to cycle): Studio, Daylight, Overcast, Sunset, Night (so a model's own lights stand out) and Flat (unlit, every surface in its own colour); the choice is remembered
- An animated model's skeleton (K) and joints (J) can be drawn over it: lines from joint to joint and along its Bones, and a mark at each joint, accent for joints clips drive and grey for welds; hovering a mark names it. Hovering a part in the inspector's parts list highlights it in the view
- See-through parts draw their far side before their near side, so a glass tube shows its back wall; Transparency-mode colour maps cut out with smooth, multisampled edges. A mesh opened on its own can show its colour map's alpha over the colour (as Roblox draws a TextureID), as a cutout, or blended (hair cards, leaves)
- Export many library models at once, as glTF or OBJ, from the library's selection bar or right-click menu
- A frameless window with Meshvault's own title bar: drag its empty space to move the window, double-click it to maximize, and use the minimize, maximize and close buttons at its right end; the edges resize it, and Windows 11 keeps its corners rounded
- Dark and light themes, notifications for long-running work, and keyboard shortcuts (press `?` in the app for the list)
- Open files with the Open button (Ctrl+O), drag and drop, or pass a path on the command line
- Switch between LODs for mesh v3+ files (including v6/v7); toggle individual convex hulls for union physics files
- Solid, wireframe or combined shading, a ground grid, and optional back-face culling
- GPU rendering (wgpu): depth-tested, multisampled, mipmapped, with physically based shading; falls back to a CPU renderer when no GPU backend is available
- Materials: base color, normal, roughness and metalness maps (Roblox `SurfaceAppearance`).  Maps next to the mesh load automatically by name (`Name.png` or `Name.color.png`, `Name.normal.png`, `Name.roughness.png`, `Name.metalness.png`); otherwise use the inspector's Material section or drop PNG/JPEG/BMP/TGA/WebP files onto the window.  Each map can be toggled to compare with and without it
- Controls: left-drag to orbit, right/middle-drag to pan, scroll to zoom, double-click or F to frame the model

Meshvault bundles the [Cal Sans](https://github.com/calcom/sans) typeface (SIL Open Font License 1.1) and [Phosphor](https://phosphoricons.com) icons (MIT); see `crates/meshvault/assets/` for their licenses.

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

Meshvault serves the exporter itself, so there is no script to keep up to date. Open Export from Roblox Studio… (in the ⋮ menu, on the welcome screen, or by clicking "Studio link" in the status bar), copy the code, and paste it into Studio's Command Bar in Edit mode. It comes as one line or formatted over several, with Luau syntax highlighting; the one line is:

```lua
local h=game:GetService("HttpService") local e=h.HttpEnabled h.HttpEnabled=true local ok,s=pcall(h.GetAsync,h,"http://localhost:34875/export.luau") h.HttpEnabled=e assert(ok,"Could not reach Meshvault (is it open, and is Studio in Edit mode, not Play?): "..tostring(s)) loadstring(s)()()
```

Besides Motor6Ds it exports the AnimationConstraints that join current avatars' limbs, as joints with their attachments' CFrames for C0 and C1, so those avatars animate too, and the Welds, WeldConstraints and RigidConstraints that hold accessories and tools on. Each joint's class is in its `Kind`. It exports what is selected in Studio: a model, or anything inside one (a part, its Animator), exports the whole model, even one without Animation objects. With nothing selected it exports the whole game except Workspace, and when nothing exportable is found it says so in the Output window instead of sending an empty export. The script returns a function taking options, so `loadstring(s)()({roots = {workspace.MyRig}, exclude = {}})` picks the roots by hand. It writes the newest schema; add `?schema=1` to the URL for the schema 1 exporter. Meshvault saves the export to `Documents/Meshvault/Exports/<place>-<time>.json` and opens it for import (making a workspace first if none is open). If sending fails, the exporter prints `=== JSON CHUNK ===` blocks instead, which can be pasted into a file and imported as before.

The exporter reports each step in the Output window, every line starting with `[Meshvault]`: what it exports (and which model a selection stands for), each model with its parts, joints, bones and clips, the totals, and the sending. Problems are warnings that say what to do: clips that could not be loaded (one still loading after 20 seconds is given up on, since a locally registered clip from another Studio session never loads), models or meshes it had to skip, Meshvault not running, a play test in progress. One bad instance skips only itself. An unexpected error prints its stack trace with the exporter's schema and Meshvault's version for reporting, and the run ends with a count of warnings.

The function also returns those problems, as a list that is empty when all went well, so a script can act on them:

```lua
local issues = loadstring(s)()()
for _, issue in issues do
	print(issue.Kind, issue.Message, issue.Path or issue.AnimationId)
end
```

To change an export before Meshvault gets it, or to keep it yourself, pass `send = false`. The export is then not sent, and the function returns it as a Lua table after the problems, with a `send` function that sends an export, changed or not, the same way the line does (in pieces, with its headers). The dialog's Edit first option gives this as a script to paste:

```lua
local issues, export, send = loadstring(s)()({ send = false })

-- change the export here, e.g. keep only the first model:
export.Models = { export.Models[1] }

send(export) -- or save h:JSONEncode(export) your own way
```

`send` also takes the export's JSON text, and returns `true`, or `false` and why. Without `send = false` the export is still returned, after it has been sent. Meshvault checks every export it imports, so a changed one that no longer fits the schema is refused with the reason.

`Kind` is one of `PlayMode`, `AnimationUnavailable`, `CurveAnimationFailed`, `MeshSkipped`, `ModelSkipped` (with the instance's `Path`), `NothingToExport`, `SendFailed`, `HttpEnabledNotRestored` or `Error` (an unexpected error, with its stack trace in `Message`). The animation kinds carry the `AnimationId`.

Everything travels over `http://localhost:34875`: `GET /export.luau` serves the exporter, and `POST /export` takes the export in 2 MB pieces. The line turns `HttpService.HttpEnabled` on only while it talks to Meshvault and restores it afterwards. Meshvault accepts connections from this machine only, and requires custom headers on exports, which a web page cannot send. Only one program can use the port: if it is taken (usually by another Meshvault), Meshvault warns at startup, the status bar reads "Studio link unavailable", and exporting from Studio is off until the port is free and Meshvault restarts. The exporters live in `crates/meshvault/studio/` and are built into the app.

### Animated models

Schema 1 exports list animated models in the import dialog; selecting one also selects the meshes its parts use.  In the library they appear under Animated models; click one (or open its `.rig.json`) to play it:

- Parts are placed by their Motor6D joints (`Part1 = Part0 * C0 * Transform * C1⁻¹`) and bones (`Parent * CFrame * Transform`), solved outward from the model's root, so welded accessories and tools follow the part they are welded to (even when the weld points from the accessory to the body, as Roblox's do); MeshPart meshes are centred and scaled to each part's size, SpecialMesh file meshes keep their own units times `Scale`, plus `Offset`, and fully transparent parts are hidden
- Parts without a file mesh are drawn as their Roblox shape (block, ball, cylinder, wedge, corner wedge, truss, or a SpecialMesh brick, torso, sphere, cylinder or head, each sized as Studio sizes it) in their part colour; anything Meshvault has no shape for (unions, whose geometry Roblox does not let scripts read) is drawn approximately and listed in the warnings
- Decals and Textures are drawn on their face as Studio projects them (upright seen from outside; the top and bottom faces read with their top towards the back), tinted by `Color3` and faded by `Transparency`. Textures tile from the face's top-left corner by their studs per tile and offset. On mesh parts a decal only reaches the triangles facing its way (within about 57°). Their images are downloaded with the rest, once each, into the workspace's `Images` folder
- PointLights, SpotLights and SurfaceLights on parts and their attachments light the model, moving with their parts: spot and surface lights shine out of their face in a cone of their `Angle`, fading out to their `Range` (the 8 nearest lights; how bright and how far they reach is an approximation of Roblox's lighting, not yet compared with Studio)
- MaterialVariants (a part's own, or MaterialService's override for its material) tile their colour, normal, roughness and metalness maps over each face of the part's box by `StudsPerTile`, from the face's top-left corner, tinted by the part's colour, as Studio does on parts. On mesh parts Studio lays them out differently, which the warnings note
- Classic clothing (a model's Shirt, Pants and ShirtGraphic) is worn as Studio composites it: each body section (torso, each arm, each leg, whether one R6 part or three R15 pieces) is a box in the joints' rest pose, and each face of the box shows its region of the 585 by 559 template, tinted by the clothing's `Color3`; pants go under the shirt, and a T-shirt covers the torso's front
- A SurfaceAppearance's `Color` tints its colour map, and its `AlphaMode` is followed as Studio draws it: Overlay lays the map over the part colour, Transparency cuts the surface away where alpha is below one half, TintMask tints only where the map is opaque, and Opaque ignores alpha
- Parts are see-through by their `Transparency`, mirror the sky by their `Reflectance` (tinted by the part on metal materials), and Neon parts glow in their colour
- Schema 2 models show the pose they were exported in (each joint's captured `Transform`), and clip translations are scaled by the model's `Scale`, as `Model:ScaleTo` does in Roblox. Schema 1 has neither, so joints start at rest and bone translations are scaled by the skinned mesh's own scale
- Skinned meshes (mesh v4 and later) bend with their bones.  Each bone the mesh was bound to follows a Bone in the model, else a part of the same name (as R15 bodies do), else its bind pose from the mesh file.  Meshes Draco-compressed with edgebreaker stay rigid, since their vertices no longer line up with their weights
- Clips play as Roblox's Animator plays them, checked against Studio: a pose drives a joint only when its parent pose is named after the joint's Part0 (or the bone's parent), as in Roblox, so poses authored for another rig, or left at the top of a keyframe, move nothing and are listed as unmatched. Pose easing follows Roblox's own curves, including its quirks (legacy Cubic swaps In and Out; Constant In jumps to the next pose at once)
- The transport bar plays, pauses, scrubs, loops and changes speed (Space plays or pauses), with clip markers on the track.  Clips the exporter could not read are listed with their status and cannot be played
- The inspector lists clips, tracks that match nothing in the model, parts with visibility toggles, and warnings such as missing meshes or a rig captured mid-animation

Facial animation plays on FACS heads: clips' face controls (NumberPoses in a keyframe clip's `FaceControls`, FloatCurves in a curve clip's `FaceControls` folder) weight the head's FACS poses and correctives, which move its face bones. The poses' bone movements are applied; their bone rotations are not yet, since applying them as read curls lips far more than Studio does. Material textures (wood grain, metal plates, glass refraction) and shadows are not rendered yet.
