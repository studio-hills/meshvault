# Changelog

Meshvault's notable changes, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). The vendored mesh library has its
own changelog in [`crates/rbx_mesh`](crates/rbx_mesh/CHANGELOG.md).

## [Unreleased]

## [0.1.0] - Unreleased

The first public release.

### App

- A portable Windows build with its own icon, and a check at startup for
  newer releases on GitHub (can be turned off in the ⋮ menu, which also shows
  the version)

### Viewer

- Opens Roblox meshes v1 to v7 (including Draco-compressed v6 and v7), union
  graphics and physics files, with LODs, hulls, bones and FACS details
- Colour, normal, roughness and metalness maps with physically based shading,
  loaded from beside the mesh, picked, or dropped onto the window
- Solid and wireframe shading, a ground grid, lighting presets, and a CPU
  renderer when no GPU backend is available

### Studio export and workspaces

- A Studio exporter served by Meshvault itself, run with one Command Bar line,
  that sends the export straight to Meshvault; options to pick roots, exclude
  instances, or keep and change the export before sending it; a whole-game
  export takes every model with parts, static or animated, and names the
  game by its published name
- Export schema 2: meshes, maps, and animated models with their parts,
  joints, bones, clips, appearance, clothing, decals, MaterialVariants and
  lights (schemas 0 and 1 are still read)
- Workspaces that mirror the export's folders, grouped by the game each
  export came from, with parallel, resumable downloads of each asset once,
  through an Open Cloud API key or a cookie kept in Windows Credential Manager;
  each asset is requested with the place it was exported from
- A library with a folder tree or a thumbnail grid, search, filters and
  multi-select; models are listed as animated or static, above the meshes

### Animated models

- Motor6D, AnimationConstraint and bone rigs, welded accessories, and skinned
  meshes
- KeyframeSequence and CurveAnimation clips played as Roblox's Animator plays
  them, with Roblox's easing curves, and facial animation on FACS heads
- Primitive parts, SpecialMeshes, decals and textures, SurfaceAppearance alpha
  modes, MaterialVariants, classic clothing, transparency, reflectance, Neon
  and part lights drawn as Studio draws them
- Skeleton and joint overlays

### Exporting

- Binary glTF for Blender: one armature, skin weights, every clip baked at
  30 fps, maps embedded
- Wavefront OBJ with an MTL file and maps, of a mesh or of a model as posed
- PNG stills with a transparent background, clipboard copies, and turntable
  GIFs

[Unreleased]: https://github.com/studio-hills/meshvault/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/studio-hills/meshvault/releases/tag/v0.1.0
