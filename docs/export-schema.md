# Export file format

An export is a JSON file listing the meshes and maps to download and, from
schema 1 on, the animated models that use them. The Studio exporter writes it
(see [Exporting from Roblox Studio](studio-export.md)), but any tool can.
Meshvault reads three layouts:

| Schema | Shape | Adds |
| --- | --- | --- |
| 0 | An array of mesh entries | The original layout: meshes and their maps only |
| 1 | `{ "SchemaVersion": 1, "Meshes": [...], "Models": [...] }` | Animated models: parts, joints, bones and clips |
| 2 | The same, with `"SchemaVersion": 2` | The place it came from, model scale, part appearance, SpecialMesh, captured joint poses, clothing, decals, MaterialVariants, lights |

Other `SchemaVersion` values are refused, with a message naming the versions
this build reads. The Studio exporter writes schema 2.

General rules:

- Field names are PascalCase, as in Roblox.
- For the content-id fields of a mesh entry (`TextureId` and the maps), a
  missing field and an empty string both mean "none". `Name` and `MeshId` are
  required strings. Most model fields that are left out take defaults (zeros
  for numbers and arrays such as `CFrame` and `Size`, empty lists, and absent
  optional fields), but each curve needs its `Curve` object with a `Type`. An
  empty string can't stand in for a number, list or object.
- Content ids can be `rbxassetid://N`, `http(s)://www.roblox.com/asset/?id=N`
  or `assetdelivery.roblox.com` URLs.
- A **CFrame** is 12 numbers: the position `x, y, z`, then the rotation
  matrix by rows (`R00, R01, R02, R10, ... R22`), as `CFrame:GetComponents()`
  returns them.
- Colours are `[r, g, b]` with each channel from 0 to 1.
- Paths are an instance's full name with `/` between the names, for example
  `ReplicatedStorage/Pets/Wolf/Head`.
- Studio output split into `=== JSON CHUNK i/n ===` blocks (ending with
  `=== END JSON EXPORT ===`) can be imported as it is.

## The place (schema 2)

| Field | Type | Meaning |
| --- | --- | --- |
| `GameName` | string | The game's published name (`MarketplaceService:GetProductInfo(game.PlaceId).Name`), else `game.Name`, else `game_<PlaceId>`. Everything imported from the export goes under a folder of this name, so the library groups imports by game |
| `PlaceId` | integer | `game.PlaceId`, sent as `Roblox-Place-Id` when downloading the export's assets. 0 (an unpublished place) means none |

Both are optional: an export without them imports as before, with no game
folder and no place id.

## Mesh entries

Schema 0 is an array of these. In schemas 1 and 2 they are the `Meshes` array,
each with an `Id` that parts refer to.

| Field | Type | Meaning |
| --- | --- | --- |
| `Id` | integer | Schemas 1 and 2: unique within the export, referred to by `Parts[].MeshRef` |
| `Name` | string | The model's name in the library |
| `MeshId` | content id | The mesh to download. Required; an entry whose `MeshId` is empty or not a content id is skipped |
| `Folder` | string | Where the model goes in the library, with `/` between folders |
| `TextureId` | content id | The base texture: the Studio exporter writes the SurfaceAppearance's `ColorMap` when there is one, else the MeshPart's `TextureID` or the SpecialMesh's `TextureId` |
| `ColorMap`, `NormalMap`, `RoughnessMap`, `MetalnessMap` | content id | The SurfaceAppearance's maps |

```json
[
  { "Name": "Body", "Folder": "Pets/Wolf", "MeshId": "rbxassetid://1234",
    "TextureId": "rbxassetid://5678" }
]
```

An entry with a duplicate `Id` or a `MeshId` Meshvault can't read is skipped
and listed in the import dialog; a map it can't read is ignored with a warning.

The Studio exporter lists a mesh once for each look of the parts that show it,
and describes that look with fields Meshvault doesn't read:
- `LegacyTextureId` and `TextureContent`;
- `SurfaceAppearance`, which includes its Content-backed maps;
- `Material`, `MaterialVariant`, `Color`, `Transparency` and `Reflectance`;
- `MaterialSource`.

Parts carry their own look, so Meshvault reads entries that have an `Id` and
the same `MeshId` and maps as one entry, and points their parts' `MeshRef` at
the first of them. The export's top-level `MaterialDiagnostics` counts these
looks (for example meshes with a SurfaceAppearance, or an unresolved
MaterialVariant) to help find meshes whose images were missed.

## Models (schemas 1 and 2)

```json
{
  "SchemaVersion": 2,
  "Meshes": [{ "Id": 1, "Name": "Body", "MeshId": "rbxassetid://10", "Folder": "RS/Wolf" }],
  "Models": [{
    "Name": "Wolf", "Path": "RS/Wolf", "Scale": 1,
    "Parts": [{ "Path": "RS/Wolf/Body", "Name": "Body", "MeshRef": 1, "ClassName": "MeshPart",
                "CFrame": [0,0,0, 1,0,0, 0,1,0, 0,0,1], "Size": [2,2,1], "Transparency": 0 }],
    "Joints": [], "Bones": [], "Animations": []
  }]
}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `Name`, `Path` | string | The model |
| `Scale` | number | Schema 2: `Model:GetScale()`. Part sizes and CFrames already include it; clip translations do not, and are scaled by it when played |
| `Parts` | array | [Parts](#parts) |
| `Joints` | array | [Joints](#joints) |
| `Bones` | array | [Bones](#bones) |
| `Animations` | array | [Clips](#clips) |
| `Clothing` | object | Schema 2: [classic clothing](#clothing) |
| `Pivot` | CFrame | Schema 2: `Model:GetPivot()` when exported |
| `PrimaryPartPath` | string | Schema 2: the PrimaryPart's path; absent without one |
| `Attachments` | array | Schema 2: [Attachments](#attachments), Bones aside |
| `Effects` | array | Schema 2: [effects](#effects): ParticleEmitters, Beams, Trails, Fire, Smoke and Sparkles |

### Parts

| Field | Type | Meaning |
| --- | --- | --- |
| `Path`, `Name` | string | The part |
| `MeshRef` | integer | `Meshes[].Id` of its mesh, or 0 when it has no file mesh |
| `CFrame` | CFrame | Its world transform when exported |
| `Size` | `[x, y, z]` | Its size in studs |
| `Pivot`, `PivotOffset` | CFrame | Schema 2: `BasePart:GetPivot()` in the world, and `PivotOffset` in the part's own space |
| `Transparency` | number | 0 to 1 |
| `ClassName` | string | Schema 2: `Part`, `MeshPart`, `WedgePart`, ... |
| `Shape` | string | Schema 2: `Block`, `Ball`, `Cylinder`, `Wedge`, `CornerWedge`, `Truss`, or `""` |
| `Color` | colour | Schema 2: `BasePart.Color` |
| `BrickColor` | string | Schema 2: the BrickColor's name |
| `Material` | string | Schema 2: the `Enum.Material` name |
| `Reflectance` | number | Schema 2 |
| `SpecialMesh` | object | Schema 2, absent without one: `MeshType` (`Enum.MeshType` name), `Scale` and `Offset` (`[x, y, z]`) |
| `SurfaceAppearance` | object | Schema 2, absent without one: `Color` (colour) and `AlphaMode` (`Overlay`, `Transparency`, `TintMask` or `Opaque`) |
| `Decals` | array | Schema 2: [Decals and Textures](#decals-and-textures) |
| `MaterialVariant` | object | Schema 2: the variant the part shows, its own or MaterialService's override: `Name`, `StudsPerTile`, `ColorMap`, `NormalMap`, `RoughnessMap`, `MetalnessMap` |
| `Lights` | array | Schema 2: [lights](#lights) on the part or its attachments |

### Joints

| Field | Type | Meaning |
| --- | --- | --- |
| `Name`, `Path` | string | The joint |
| `Part0`, `Part1` | string | The paths of the parts it joins, or `""` |
| `C0`, `C1` | CFrame | As on the joint; for an AnimationConstraint, its attachments' CFrames |
| `Transform` | CFrame | Schema 2: the Motor6D's or AnimationConstraint's `Transform` when exported, the pose the model was in; welds have the identity |
| `Kind` | string | The joint's class: `Motor6D`, `AnimationConstraint`, or a weld (`Weld`, `WeldConstraint`, `RigidConstraint`, ...) that only holds parts together. Absent means `Motor6D` |

### Bones

| Field | Type | Meaning |
| --- | --- | --- |
| `Name`, `Path` | string | The Bone |
| `ParentPath` | string | The part or bone it hangs from |
| `CFrame` | CFrame | Its transform relative to its parent |

### Attachments

| Field | Type | Meaning |
| --- | --- | --- |
| `Name`, `Path` | string | The Attachment |
| `ParentPath` | string | The part it is on |
| `CFrame` | CFrame | Where it sits on its part |
| `WorldCFrame` | CFrame | Where it was in the world when exported |
| `Visible` | boolean | `Attachment.Visible` |

### Effects

Each has `ClassName` (`ParticleEmitter`, `Beam`, `Trail`, `Fire`, `Smoke` or
`Sparkles`), `Name`, `Path` and `ParentPath` (the part, Attachment or Bone it
is in), then its class's properties under their Roblox names, as far as the
exporter reads them:

- **ParticleEmitter:** `Enabled`, `Texture`, `Color`, `Transparency`,
  `Size`, `Rate`, `Lifetime`, `Speed`, `Rotation`, `RotSpeed`,
  `SpreadAngle`, `EmissionDirection`, `Acceleration`, `Drag`,
  `VelocityInheritance`, `LockedToPart`, `Orientation`, `ZOffset`,
  `LightEmission`, `LightInfluence`, `Brightness`, `Squash`, `Shape`,
  `ShapeStyle`, `ShapeInOut`, `FlipbookLayout`, `FlipbookMode`,
  `FlipbookFramerate`, `FlipbookStartRandom`, `TimeScale`;
- **Beam:** `Enabled`, `Texture`, `Color`, `Transparency`, `Width0`,
  `Width1`, `CurveSize0`, `CurveSize1`, `TextureSpeed`, `TextureLength`,
  `TextureMode`, `Segments`, `FaceCamera`, `LightEmission`,
  `LightInfluence`, `Brightness`, `ZOffset`, with `Attachment0Path` and
  `Attachment1Path`;
- **Trail:** `Enabled`, `Texture`, `Color`, `Transparency`, `Lifetime`,
  `MinLength`, `MaxLength`, `WidthScale`, `TextureMode`, `TextureLength`,
  `FaceCamera`, `LightEmission`, `LightInfluence`, `Brightness`, with
  `Attachment0Path` and `Attachment1Path`;
- **Fire:** `Enabled`, `Color`, `SecondaryColor`, `Heat`, `Size`,
  `TimeScale`; **Smoke:** `Enabled`, `Color`, `Opacity`, `RiseVelocity`,
  `Size`, `TimeScale`; **Sparkles:** `Enabled`, `SparkleColor`, `TimeScale`.

Values are written as in the rest of the file: enums by name, colours as
`[r, g, b]`, a Vector2 or Vector3 as a list, a NumberRange as `[min, max]`, a
NumberSequence as `[{ Time, Value, Envelope }, ...]` and a ColorSequence as
`[{ Time, Value }, ...]` with a colour `Value`. The same property can take a
different shape in different classes (a ParticleEmitter's `Size` is a
NumberSequence, Fire's a number; a Trail's `Lifetime` is a number).
`Texture` is a content id, a built-in `rbxasset://` texture, or empty.

### Clips

| Field | Type | Meaning |
| --- | --- | --- |
| `Name` | string | The Animation's name |
| `AnimationId` | content id | The animation asset |
| `Status` | string | `KeyframeSequence` or `CurveAnimation` when it was read; `Unavailable`, `CurveAnimationExportFailed` or `UnsupportedClipType` when not (such clips are listed but don't play) |
| `Duration` | number | Seconds: the last key's time |
| `Loop` | boolean | Whether the clip loops |
| `Priority` | string | The `Enum.AnimationPriority` name |
| `Keyframes` | array | A KeyframeSequence's keyframes |
| `Curves` | array | A CurveAnimation's curves |

A **keyframe** is `{ Time, Poses, Markers }`. `Markers` are `{ Name, Value }`.
A **pose** is:

| Field | Type | Meaning |
| --- | --- | --- |
| `Name` | string | The part or bone it drives |
| `Transform` | CFrame | Its offset from rest |
| `Weight` | number | 0 marks a placeholder pose that only carries children |
| `EasingStyle`, `EasingDirection` | string | `Enum.PoseEasingStyle` and `Enum.PoseEasingDirection` names |
| `Children` | array | Poses of the parts and bones joined to this one |
| `FaceControls` | array | Face control values set at this pose: `{ Name, Value, EasingStyle, EasingDirection }` |

A pose moves a joint only when its parent pose is named after the joint's
`Part0` (or the bone's parent), as in Roblox.

A **curve** is `{ Path, Name, Curve }`. `Path` is the folder path inside the
clip, whose last name is the target; `Name` is usually `Position` or
`Rotation`. `Curve` has a `Type`:

| `Type` | Contents |
| --- | --- |
| `FloatCurve` | `Keys`: scalar keys |
| `RotationCurve` | `Keys`: keys whose `Value` is a CFrame |
| `Vector3Curve` | `Channels`: `X`, `Y` and `Z`, each a list of scalar keys |
| `EulerRotationCurve` | `RotationOrder` (`Enum.RotationOrder` name) and `Channels` as above |

Each key is `{ Time, Value, Interpolation, LeftTangent, RightTangent }`, with
`Interpolation` the `Enum.KeyInterpolationMode` name.

Vector3Curves move their target and RotationCurves and EulerRotationCurves
turn it. FloatCurves drive facial animation only: their `Path` ends in
`FaceControls` and their `Name` is the face control, such as `JawDrop`.
Other FloatCurves, and curve types Meshvault doesn't know, are skipped and
counted in the inspector; a clip left with nothing that moves can't play.

### Clothing

`Clothing` is `{ ShirtTemplate, ShirtColor, PantsTemplate, PantsColor, Graphic, GraphicColor }`:
the Shirt's and Pants' templates and the ShirtGraphic's image, as content ids,
each tinted by its colour.

### Decals and Textures

| Field | Type | Meaning |
| --- | --- | --- |
| `ClassName` | string | `Decal` or `Texture` |
| `Texture` | content id | The image, or a Decal asset (resolved to its image on download) |
| `Face` | string | `Front`, `Back`, `Left`, `Right`, `Top` or `Bottom` |
| `Color` | colour | Tints the image |
| `Transparency` | number | 0 to 1 |
| `ZIndex` | integer | Draw order on the face |
| `StudsPerTileU`, `StudsPerTileV`, `OffsetStudsU`, `OffsetStudsV` | number | Textures only: the tile size and offset |

### Lights

| Field | Type | Meaning |
| --- | --- | --- |
| `ClassName` | string | `PointLight`, `SpotLight` or `SurfaceLight` |
| `Enabled`, `Shadows` | boolean | As on the light |
| `Brightness`, `Range` | number | As on the light |
| `Color` | colour | As on the light |
| `Angle`, `Face` | number, string | Spot and surface lights: the cone's angle in degrees and the face it shines from |
| `CFrame` | CFrame | Where the light sits on the part, when it is on an attachment |

## Compatibility

Older exports keep working: fields a schema doesn't have are simply absent.
A newer Meshvault reads every older schema, and an export with a higher
`SchemaVersion` than a build knows is refused rather than half read. A clip
`Status` a build doesn't know is shown as unsupported and the clip can't play;
an unknown curve `Type` is skipped, and the rest of its clip still plays.

## Limits

Meshvault refuses an export that would take far too long, or far too much
memory, to show or import:

- an export file over 2 GB (the same limit as the Studio link);
- more than 1,000,000 meshes or 100,000 models;
- a model with more than 100,000 parts, 100,000 joints, 10,000 bones or
  10,000 clips, 100,000 attachments or 100,000 effects; a part with more than
  1,000 decals or 1,000 lights; a sequence with more than 1,000 keypoints;
- a clip with more than 1,000,000 keyframes or 1,000,000 curves, or more than
  5,000,000 keys (keyframes, poses and curve keys together); a keyframe with
  more than 100,000 poses or 1,000 markers, a pose with more than 100,000
  child poses or 1,000 face controls, a curve (or one channel of it) with more
  than 5,000,000 keys;
- a document whose lists would take more than 64 MiB plus 8 bytes for each
  byte of the file once read. A real record takes at most a few times
  its length in the file, so only a document of mostly empty records (`{}`) comes near this.

Real exports stay far below these: a few thousand meshes, some hundred models
of at most a few thousand parts, a few hundred bones, about 50 face controls.
Each list is counted as it is read, and reading stops at the first record over
a limit, so an export over one is refused before it is built in memory.

Some entries are skipped instead, and listed as ignored in the import dialog:
a mesh entry or model in a folder more than 64 levels deep (counting the
game's folder), with a folder name over 200 characters, or a folder path over
2,000 characters.

A clip whose `Duration` or last key is more than an hour (or not a number)
doesn't load, and is shown as unplayable. Exporting a model to Blender bakes
each clip at 30 frames a second for every bone; a model whose clips come to
more than 10,000,000 keys that way (counting a model without bones as one key
a frame) is not exported. The frames are counted before any is baked.

The workspace keeps each animated model as `Rigs/<Path>.rig.json`, which is
itself a schema 2 export holding that one model.
