# User guide

Meshvault has three panes: the **library** of the open workspace's models on
the left, the **viewport** with a floating toolbar in the middle, and the
**inspector** with the model's stats, material and properties on the right.
Both side panels can be hidden (Ctrl+B and Ctrl+Shift+B). In a narrow
window (or at high display scaling) the inspector steps aside so the 3D view
keeps room, and comes back when the window is wider or the library is hidden;
the top bar's buttons show only their icons, and the viewport's toolbar takes
two rows.

The window is frameless, with Meshvault's own title bar: drag its empty space
to move the window, double-click it to maximize, and use the buttons at its
right end. The edges resize it, and Windows 11 keeps its corners rounded.
The sun or moon button in the top bar switches between the light and dark
themes.

## Updates

At startup Meshvault asks GitHub whether a newer version has been released.
When there is one, a notice says so and *Update* appears at the right of the
status bar; click it to open the release page. Nothing is downloaded or
installed by itself, and the request carries nothing about you or your files.
Turn the check off with *Check for updates at startup* in the ⋮ menu, which
also shows the version you are running.

## Opening files

Open a file with the Open button (Ctrl+O), by dropping it onto the window, or
by passing its path on the command line. Meshvault detects the file type and
shows its header details: version, vertex and face counts, LODs, bones, FACS,
hulls and physics information.

- Mesh v3 and later (including v6 and v7) can switch between their LODs.
- Union physics files can toggle each convex hull.
- Shading is solid, wireframe or both (1, 2, 3), with a ground grid (G) and
  optional back-face culling.

### Materials

Meshvault draws base colour, normal, roughness and metalness maps (Roblox's
`SurfaceAppearance`) with physically based shading. Maps next to a mesh load
by name: `Name.png` or `Name.color.png`, then `Name.normal.png`,
`Name.roughness.png` and `Name.metalness.png`. Otherwise use the inspector's
Material section, or drop PNG, JPEG, BMP, TGA or WebP files onto the window.
Each map can be turned off to compare (M turns all of them off and on).

When no GPU backend is available, Meshvault draws with a simpler CPU renderer:
flat shading, and a base map only when the scene is a single part. Normal,
roughness and metalness maps, decals, clothing and model lights need the GPU.

A mesh opened on its own can show its colour map's alpha over the colour (as
Roblox draws a TextureID), as a cutout, or blended (hair cards, leaves).

### Lighting

The bulb over the viewport picks a lighting preset, and L cycles through
them: Studio, Daylight, Overcast, Sunset, Night (so a model's own lights stand out) and Flat
(unlit, every surface in its own colour). The choice is remembered.

## The library

The library shows the workspace's models, grouped by the game they were
exported from, either as the export's folder tree or as a grid of thumbnails
grouped by folder (the button beside the title
switches). Each thumbnail is drawn once, when it first scrolls into view, and
kept in the workspace's `.cache/thumbnails`.

- **Search** by name, folder or asset id, and narrow by download status.
- **Filter** (the funnel beside the search) to models with animations, skinned
  meshes, missing meshes, or a given map. Whether a mesh is skinned is read
  once in the background and kept in `.cache/facts.json`.
- **Select** several models with Ctrl-click and Shift-click. With the
  pointer over the library, Ctrl+A selects everything shown and Esc clears the
  selection. Then export them, remove them, or retry their failed mesh and
  map downloads at once, from the bar over the library or the right-click
  menu.
  [Workspaces and downloads](workspaces.md#removing-models) says what removing
  deletes.

[Workspaces and downloads](workspaces.md) covers making a workspace, importing
an export and downloading.

## Animated models

Schema 1 and 2 exports list animated models in the import dialog; selecting
one also selects the meshes its parts use. In the library they appear under
Animated models (with a heading per game when they come from more than one).
Click one to play it, or open its `.rig.json` while its
workspace is open. A `.rig.json` from elsewhere is imported into a workspace
like any export.

The transport bar plays, pauses, scrubs, loops and changes speed (Space plays
or pauses), with clip markers on the track. Clips the exporter could not read
are listed with their status and cannot be played. The inspector lists the
clips, tracks that match nothing in the model, the parts with visibility
toggles, and warnings such as missing meshes or a rig captured mid-animation.

The skeleton (K) and joints (J) can be drawn over the model: lines from joint
to joint and along its Bones, and a mark at each joint, accent coloured for
joints clips drive and grey for welds. Hovering a mark names it. Hovering a
part in the inspector's parts list highlights it in the view.

### How models are put together

- Parts are placed by their joints (`Part1 = Part0 * C0 * Transform * C1⁻¹`)
  and bones (`Parent * CFrame * Transform`), solved outward from the model's
  root. Welded accessories and tools follow the part they are welded to, even
  when the weld points from the accessory to the body, as Roblox's do.
- MeshPart meshes are centred and scaled to each part's size. SpecialMesh file
  meshes keep their own units times `Scale`, plus `Offset`. Fully transparent
  parts are hidden.
- Parts without a file mesh are drawn as their Roblox shape (block, ball,
  cylinder, wedge, corner wedge, truss, or a SpecialMesh brick, torso, sphere,
  cylinder or head, each sized as Studio sizes it) in their part colour.
  Anything Meshvault has no shape for, such as unions, whose geometry Roblox
  does not let scripts read, is drawn approximately and listed in the warnings.
- Schema 2 models show the pose they were exported in (each joint's captured
  `Transform`), and clip translations are scaled by the model's `Scale`, as
  `Model:ScaleTo` does in Roblox. Schema 1 has neither, so joints start at rest
  and bone translations are scaled by the skinned mesh's own scale.

### Surfaces

- **Decals and Textures** are drawn on their face as Studio projects them
  (upright seen from outside; the top and bottom faces read with their top
  towards the back), tinted by `Color3` and faded by `Transparency`. Textures
  tile from the face's top-left corner by their studs per tile and offset. On
  mesh parts a decal only reaches the triangles facing its way (within about
  57°). Their images are downloaded with the rest, once each, into the
  workspace's `Images` folder.
- **SurfaceAppearance** `Color` tints the colour map, and its `AlphaMode` is
  followed as Studio draws it: Overlay lays the map over the part colour,
  Transparency cuts the surface away where alpha is below one half, TintMask
  tints only where the map is opaque, and Opaque ignores alpha.
- **MaterialVariants** (a part's own, or MaterialService's override for its
  material) tile their maps over each face of the part's box by
  `StudsPerTile`, from the face's top-left corner, tinted by the part's colour,
  as Studio does on parts. On mesh parts Studio lays them out differently,
  which the warnings note.
- **Classic clothing** (a model's Shirt, Pants and ShirtGraphic) is worn as
  Studio composites it: each body section (torso, each arm, each leg, whether
  one R6 part or three R15 pieces) is a box in the joints' rest pose, and each
  face of the box shows its region of the 585 by 559 template, tinted by the
  clothing's `Color3`. Pants go under the shirt, and a T-shirt covers the
  torso's front.
- Parts are see-through by their `Transparency`, mirror the sky by their
  `Reflectance` (tinted by the part on metal materials), and Neon parts glow in
  their colour. See-through parts draw their far side before their near side,
  so a glass tube shows its back wall.
- **Lights** (PointLights, SpotLights and SurfaceLights on parts and their
  attachments) light the model and move with their parts. Spot and surface
  lights shine out of their face in a cone of their `Angle`, fading out to
  their `Range`. The 8 nearest lights are used; their brightness and reach
  approximate Roblox's lighting and have not been compared with Studio yet.

### Skinning and clips

- Skinned meshes (mesh v4 and later) bend with their bones. Each bone the mesh
  was bound to follows a Bone in the model, else a part of the same name (as
  R15 bodies do), else its bind pose from the mesh file. Meshes
  Draco-compressed with edgebreaker stay rigid, since their vertices no longer
  line up with their weights.
- Clips play as Roblox's Animator plays them, checked against Studio. A pose
  drives a joint only when its parent pose is named after the joint's Part0
  (or the bone's parent), as in Roblox, so poses authored for another rig, or
  left at the top of a keyframe, move nothing and are listed as unmatched.
- Pose easing follows Roblox's own curves, including its quirks: legacy Cubic
  swaps In and Out, and Constant In jumps to the next pose at once.
- Facial animation plays on FACS heads: clips' face controls (NumberPoses in a
  keyframe clip's `FaceControls`, FloatCurves in a curve clip's `FaceControls`
  folder) weight the head's FACS poses and correctives, which move its face
  bones. The poses' bone movements are applied, but not yet their bone
  rotations, since applying them as read curls lips far more than Studio does.

Not drawn yet: material textures (wood grain, metal plates, glass refraction)
and shadows.

## Exporting

### To Blender (glTF)

Ctrl+E, the export button over the viewport, or a library model's menu exports
binary glTF (`.glb`). An animated model becomes one armature, which Blender's
glTF importer reads as such:

- Every part and Bone is a bone of the armature, and every mesh is skinned to
  it: a skinned mesh with its own weights, a rigid part's mesh fully to its
  part.
- Every playable clip is baked at 30 frames a second into its own action, so
  Blender plays it as Meshvault does, with easing, bones only meshes have, and
  facial animation's bone movements.
- Colour, normal, roughness and metalness maps are embedded.
- Lengths are in studs, which Blender reads as metres.

A mesh opened on its own exports its visible geometry and maps, without a
skeleton, skin weights or clips. Decals, clothing and MaterialVariants are not
exported yet.

### To OBJ

Ctrl+Shift+E exports Wavefront OBJ, with an MTL file and the colour and normal
maps copied beside it: a mesh's visible LODs or hulls, or an animated model as
one OBJ of its parts as posed now.

Many library models can be exported at once, as glTF or OBJ, from the
library's selection bar or right-click menu.

### Images and turntables

The camera button over the viewport (or F12) saves the view as a PNG, 1080
pixels tall with a transparent background. The export button beside it
(*Export and save images*) also has *Copy image*, which copies the view to
paste elsewhere, and *Save turntable (GIF)…*: the camera turns once around the
model while the selected clip plays.

Thumbnails, copied images, PNGs and GIFs need the GPU renderer.
