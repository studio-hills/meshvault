## [Unreleased]

### Added

- Meshvault reads versioned (schema 1) exports and plays their animated models, with Motor6D and bone rigs, KeyframeSequence and CurveAnimation clips, and skinned meshes
- Meshvault reads schema 2 exports: model scale, primitive part shapes and colours, SpecialMesh scale and offset, and captured joint poses
- Meshvault serves its Studio exporter (`GET /export.luau`, schema 2 by default, `?schema=1` for the older one) and receives the exports it sends, so exporting is one Command Bar line and nothing is copied out of the Output window. Its "Export from Roblox Studio" dialog has the line, and Meshvault warns when the link's port is taken
- The served exporter exports AnimationConstraint joints (current avatars), so those rigs animate, and Welds, WeldConstraints and RigidConstraints with each joint's class (`Kind`), so accessories and tools follow the body when it animates
- The served exporter gives up on an animation that has not loaded after 20 seconds (a locally registered clip from another Studio session never loads) and reports it, instead of stopping the export without a word

### Changed

- Meshvault binds clip poses as Roblox does, by name and parent name, so poses under another parent or at the top of a keyframe no longer move joints Roblox leaves still (the root bones of many pets and fish)
- Meshvault's window is frameless, with its own title bar and caption buttons, and keeps rounded corners on Windows 11
- Exports record keyframe clips' face control poses, and Meshvault plays facial animation on FACS heads from them and from curve clips' face control curves (bone movements; rotations not yet)
- Exports record the lights on each part and its attachments, and Meshvault lights the model with them
- The library has a grid view of thumbnails (drawn off screen once and cached in the workspace), filters for animated, skinned and not-downloaded models and for each map kind, and multi-select (Ctrl/Shift-click, Ctrl+A) to retry, export or remove many models at once
- Export for Blender: animated models and meshes export as binary glTF (`.glb`) with one armature, skin weights, every clip baked at 30 fps, and their maps embedded; checked by importing into Blender 5.2
- OBJ exports carry texture coordinates, normals and an MTL file with the maps beside it, from the viewer (an animated model as posed now) and the library
- Save the view as a transparent PNG, or as a turntable GIF that plays the selected clip
- The Studio exporter can keep the export instead of sending it (`send = false`): it returns the export as a Lua table, with a `send` function for sending it, changed or not, to Meshvault. The Export from Roblox Studio dialog offers this as an Edit first script
- The Export from Roblox Studio dialog is simpler: a choice of Send to Meshvault or Edit first, the code with Luau syntax highlighting as one line or formatted, and one Copy button
- Lighting presets (Studio, Daylight, Overcast, Sunset, Night, Flat), a screenshot button (F12) and copying the view to the clipboard
- Skeleton and joint overlays for animated models, and highlighting a part hovered in the parts list
- See-through parts blend their far side under their near side; cutout maps get smooth edges (alpha to coverage); meshes opened on their own can blend or cut out by their colour map's alpha
- Exports record a model's classic clothing (Shirt, Pants, ShirtGraphic and their colours), and Meshvault dresses the body in it as Studio does
- Exports record each part's MaterialVariant (its maps and studs per tile), and Meshvault tiles it over the part as Studio does
- Exports record each part's Decals and Textures, and Meshvault downloads their images and draws them on their faces as Studio does (no longer standing a mesh part's decal in for its texture)
- Exports record each part's SurfaceAppearance tint and alpha mode, and Meshvault draws all four alpha modes as Studio does
- Meshvault renders parts closer to Studio: partial transparency, Neon glow and reflectance; trusses as their lattice; SpecialMesh heads at Studio's size and rounded shape
- Pose easing matches Studio: Elastic and Bounce are no longer reversed, Elastic and Bounce InOut use Roblox's curves, and Constant jumps where Roblox does
