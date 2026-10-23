# Workspaces and downloads

A workspace is a folder of models downloaded from Roblox, laid out to mirror
the folders of the [export](export-schema.md) they came from. Exports from
Studio name their game, and everything imported from one goes under a folder
named after the game, so a workspace can hold several games. Meshvault opens
the most recent workspace when it starts; the workspace menu in the top bar
switches between them, and the welcome screen lists the recent ones.

## Making one and importing

1. In the workspace menu choose *New workspace…* (Ctrl+N). It goes under
   `Documents/Meshvault` unless you choose another folder.
2. Add Roblox access (below).
3. Import an export: send one from Studio, click Import (Ctrl+I), or drop a
   `.json` onto the window. Pick the folders, models and maps to import; the
   dialog can show only animated models, static models or loose meshes, and
   include or skip each kind of map.
4. Downloads start in the background when the import is done.

Exports are read one at a time. One that arrives (or is opened) while another
is being read or its import dialog is open waits, and opens when that import
is done; up to 8 can wait, and one more is not imported (an export from Studio
is still saved, so it can be imported again later). Waiting exports are
dropped when the workspace is closed, or another is opened or created.

Importing an export again merges it with what is there:

- A mesh entry with the same game, folder, name and mesh id as one already in
  the workspace is the same model; newly picked maps are added to it. If one
  of its maps has a different id, the entry becomes a separate model.
- An animated model from the same game with the same `Path` replaces the saved
  one: its rig, appearance and clips are those of the new export.
- The same mesh or model from another game is another model.
- Models imported before exports named their game (or from exports that name
  none) are taken in by the first game whose export has them: they move under
  its folder in the library, and their files stay where they are.

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
Forget deletes the saved credential. The same dialog sets how many downloads
run at once, from 1 to 8 (4 by default).

## Downloading

- Each asset is requested with the place it was exported from (the export's
  `PlaceId`, sent as `Roblox-Place-Id`). If your account can edit that place,
  this lets it download assets private to the place. An unpublished place has
  no place id, and exports that name none are downloaded without one.
- Rate limits and server errors are retried.
- Each asset id is downloaded once, even when several models or maps use it.
- Decal ids are resolved to their image.
- A download over 256 MB, or one that unpacks to more, fails rather than
  being saved cut short. Meshvault reads no mesh, texture or cached download
  over 256 MB either: such a mesh or texture fails to open or export, and such
  a cached copy is downloaded again.
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
  Models/<Game>/<Folder>/<Name>.mesh  each mesh, under its game, in the export's folders
  Models/<Game>/<Folder>/<Name>.png   its TextureID, beside it
  Models/<Game>/<Folder>/<Name>.color.png   its SurfaceAppearance maps
                     .normal.png, .roughness.png, .metalness.png
  Rigs/<Game>/<Path>.rig.json         each animated model, as an export holding just that model
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
Models from exports that name no game have no `<Game>` folder.

The manifest has a format version (3 at present). A newer Meshvault upgrades an
older workspace when it opens it.

A workspace from someone else is checked when it is opened, before any file in
it is touched: every path in the manifest must be a plain relative path, with
`/` between its parts, under the folder it belongs in (`Models/`, `Rigs/` or
`Images/`). A path with `..`, a drive, a network share, a `\`, a `:`, or a
reserved name such as `CON`, or a model more than 64 folders deep, makes
Meshvault refuse the whole workspace and say which entry is wrong. Folders
deeper than 64 in an export are cut off when it is imported. Meshvault also
refuses to write or delete a file outside those folders.

Links (junctions and symbolic links) inside a workspace are not followed:

- A workspace whose `workspace.json`, `Models`, `Rigs`, `Images` or `.cache`
  (or `.cache/thumbnails`, `.cache/facts.json`, `.cache/format`) is a link is
  refused when it is opened.
- Nothing is written, deleted or read through a link deeper inside it, such as
  a linked folder in `Models/`, and Meshvault doesn't check whether a file is
  there either. Opening the workspace leaves downloads behind a link as they
  are; viewing, exporting or loading something behind one says why it can't;
  maps, images, thumbnails and facts behind one are left out. So a link to a
  network share in a workspace someone sent you is never followed.
- The workspace folder itself may be a link, since you chose it. OneDrive
  placeholders (files not downloaded yet) are not links and are read as usual.
- Every file Meshvault writes in a workspace goes to a new temporary file,
  created only if nothing (no file or link) has that name, then renamed into
  place, so a link or hard link left at either name is replaced, never written
  through.
- The checks look at the folders when Meshvault reads or writes; a link
  swapped in at that exact moment is not caught.

A model in a manifest may have no more mesh references than a model may have
parts (100,000), and no more images than an export may have meshes
(1,000,000). Cached thumbnails are used only if they are PNGs of the size
Meshvault draws them (192 by 192 pixels); any other is drawn again.

## Settings

Settings (theme, lighting, recent workspaces, credential type, parallel
downloads, the update check) are in `%APPDATA%\meshvault\config.json`. Meshvault writes
them when they change and when a workspace is opened. Deleting the file resets
them; it holds no credentials.
