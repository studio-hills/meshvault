# Workspaces and downloads

A workspace is a folder of models downloaded from Roblox, laid out to mirror
the folders of the [export](export-schema.md) they came from. Meshvault opens
the most recent workspace when it starts; the workspace menu in the top bar
switches between them, and the welcome screen lists the recent ones.

## Making one and importing

1. In the workspace menu choose *New workspace…* (Ctrl+N). It goes under
   `Documents/Meshvault` unless you choose another folder.
2. Add Roblox access (below).
3. Import an export: send one from Studio, click Import (Ctrl+I), or drop a
   `.json` onto the window. Pick the folders, models and maps to import; the
   dialog can include or skip each kind of map.
4. Downloads start in the background when the import is done.

Importing an export again merges it with what is there:

- A mesh entry with the same folder, name and mesh id as one already in the
  workspace is the same model; newly picked maps are added to it. If one of
  its maps has a different id, the entry becomes a separate model.
- An animated model with the same `Path` replaces the saved one: its rig,
  appearance and clips are those of the new export.

The app also takes a workspace folder and an export on the command line:
`meshvault <workspace> <export.json>`.

## Roblox access

Roblox serves assets only to signed-in requests. Click the key icon in the top
bar (Ctrl+,), choose the kind of credential, paste it, and click Save:

- **An Open Cloud API key** (recommended): make one at
  [create.roblox.com/dashboard/credentials](https://create.roblox.com/dashboard/credentials)
  with the `legacy-asset:manage` permission. Keys made for a group can't have
  this permission; make one for your own account.
- **A `.ROBLOSECURITY` cookie**: this gives full access to your account, so
  prefer an API key, use the cookie only on a computer you trust, and never
  share it. Meshvault sends it only to roblox.com.

Credentials are kept in Windows Credential Manager, never in the workspace or
the settings file.

To try a credential before saving it, enter an asset id it should be able to
download under *Test with asset id* and click Test. Testing does not save it.
Forget deletes the saved credential.

The same dialog has two optional settings:

- **Place ID**: a place you can edit. It is sent as `Roblox-Place-Id`, which
  can grant access to private assets used in that place.
- **Parallel downloads**: from 1 to 8 (4 by default).

## Downloading

- Rate limits and server errors are retried.
- Each asset id is downloaded once, even when several models or maps use it.
- Decal ids are resolved to their image.
- Progress is kept when the app is closed. Downloads don't start again by
  themselves: after reopening a workspace, click Download in the top bar
  (Ctrl+D) to carry on. Files deleted outside the app are fetched again then,
  from the cache when it still has them.
- The top bar's Retry button retries every failed download. A mesh model's
  right-click menu, or the bar over a multi-selection, retries the failed
  meshes and maps of just those models. An animated model's menu has no Retry,
  and decal, clothing and MaterialVariant images are only retried by the top
  bar's Retry.

## Removing models

Remove (a model's right-click menu, or the bar over a selection) takes models
out of the workspace:

- A mesh model's downloaded mesh and maps are deleted from `Models/`. The
  download cache keeps them, so importing them again is quick.
- An animated model's rig and clips are deleted. The meshes it uses stay in
  the library.

## Layout

```text
<workspace>/
  workspace.json                      the manifest: items and their download state
  Models/<Folder>/<Name>.mesh         each mesh, in the export's folders
  Models/<Folder>/<Name>.png          its TextureID, beside it
  Models/<Folder>/<Name>.color.png    its SurfaceAppearance maps
                     .normal.png, .roughness.png, .metalness.png
  Rigs/<Path>.rig.json                each animated model, as an export holding just that model
  Images/<asset id>.<ext>             decal, clothing and MaterialVariant images
  .cache/<asset id>                   one raw copy of each asset
  .cache/thumbnails/                  the library's pictures of models
  .cache/facts.json                   what the library filters on, per mesh
```

`Models/` is meant for browsing in Explorer or taking files into other tools.
Each file's extension comes from what was downloaded, so a map may be `.jpg`,
`.bmp` or `.webp` rather than `.png`, and data Meshvault doesn't recognise is
saved as `.bin`.
When two models' files would have the same name, the later ones are numbered.

The manifest has a format version (3 at present). A newer Meshvault upgrades an
older workspace when it opens it.

## Settings

Settings (theme, lighting, recent workspaces, credential type, place id,
parallel downloads, the update check) are in `%APPDATA%\meshvault\config.json`. Meshvault writes
them when they change and when a workspace is opened. Deleting the file resets
them; it holds no credentials.
