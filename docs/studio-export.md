# Exporting from Roblox Studio

Meshvault does not read place files. Instead, a script run in Studio collects
what Meshvault needs: each mesh and map's asset id, folder and name, and each
animated model's parts, joints, bones and clips. It does not export mesh
geometry or image files; Meshvault downloads those afterwards. The result is
an [export file](export-schema.md).

Meshvault serves the exporter itself, so there is no script to install or keep
up to date.

## The Command Bar line

Open *Export from Roblox Studio…* (in the ⋮ menu, on the welcome screen, or by
clicking *Studio link* in the status bar), copy the code, and paste it into
Studio's Command Bar in **Edit mode** (not during a play test). The dialog
shows it as one line or formatted over several. The one line is:

```lua
local h=game:GetService("HttpService") local e=h.HttpEnabled h.HttpEnabled=true local ok,s=pcall(h.GetAsync,h,"http://localhost:34875/export.luau") h.HttpEnabled=e assert(ok,"Could not reach Meshvault (is it open, and is Studio in Edit mode, not Play?): "..tostring(s)) loadstring(s)()()
```

Meshvault saves what it receives to `Documents/Meshvault/Exports/<place>-<time>.json`
and opens it for import, making a workspace first if none is open.

## What is exported

- **With something selected**, the models it belongs to: a model, or anything
  inside one (a part, its Animator), exports the whole model, even one without
  Animation objects.
- **With nothing selected**, the whole game except Workspace.

When the exporter scans the whole game or a folder, it collects every file
mesh and exports every Model holding parts or Animations, static or animated.
Each is exported once, as its outermost Model below what is scanned: a Model
inside another is part of it. To export an inner Model on its own, select it,
or pass it in `roots`. Parts outside any Model are exported as meshes only.
- When nothing exportable is found, the exporter says so in the Output window
  instead of sending an empty export.

Besides Motor6Ds, the exporter records the AnimationConstraints that join
current avatars' limbs (with their attachments' CFrames as C0 and C1), so
those avatars animate too, and the Welds, WeldConstraints and RigidConstraints
that hold accessories and tools on. Each joint's class is in its `Kind`.

Animations must be loadable in the current Studio session. A clip still
loading after 20 seconds is given up on and reported: a locally registered
clip from another Studio session never loads, and an unpublished place can't
load a game's own animations.

## Options

The script returns a function, and the line calls it with no options. Call it
yourself to pass them:

```lua
loadstring(s)()({ roots = { workspace.MyRig }, exclude = {} })
```

| Option | Meaning | Default |
| --- | --- | --- |
| `roots` | The instances to export | The selection, or `{ game }` when nothing is selected |
| `exclude` | Instances to leave out, with everything beneath them | `{ Workspace }` when `roots` is not given and nothing is selected, otherwise none (so `roots = { game }` includes Workspace unless you also pass `exclude = { workspace }`) |
| `send` | `false` keeps the export in Studio instead of sending it | sends |

The exporter writes the newest schema (2). Add `?schema=1` to the URL for the
schema 1 exporter.

## What it returns

The function returns three values:

1. **`issues`**: the problems it met, as a list that is empty when all went
   well.
2. **`export`**: the export as a Lua table (`nil` when there was nothing to
   export).
3. **`send`**: a function that sends an export to Meshvault.

Each issue is `{ Kind, Message, Path?, AnimationId? }`:

| `Kind` | Meaning |
| --- | --- |
| `PlayMode` | Studio is in a play test, so joints are exported in their current animated pose |
| `AnimationUnavailable` | A clip could not be loaded (carries `AnimationId`) |
| `CurveAnimationFailed` | A CurveAnimation could not be read (carries `AnimationId`) |
| `MeshSkipped`, `ModelSkipped` | An instance had to be skipped (carries `Path`) |
| `NothingToExport` | Nothing exportable was found |
| `SendFailed` | Meshvault could not be reached, or refused the export |
| `HttpEnabledNotRestored` | `HttpService.HttpEnabled` could not be put back |
| `Error` | An unexpected error, with its stack trace in `Message` |

```lua
local issues = loadstring(s)()()
for _, issue in issues do
	print(issue.Kind, issue.Message, issue.Path or issue.AnimationId)
end
```

### Changing an export before sending it

Pass `send = false` and the export is not sent. Change the table, then call
`send` with it. The dialog's *Edit first* option gives this as a script to
paste:

```lua
local issues, export, send = loadstring(s)()({ send = false })

-- change the export here, e.g. keep only the first model:
export.Models = { export.Models[1] }

send(export) -- or save h:JSONEncode(export) your own way
```

`send` takes an export table or its JSON text, sends it the same way the line
does (in pieces, with its headers), and returns `true`, or `false` and why.
`true` means Meshvault received it, not that it imported: Meshvault saves what
it receives and checks it as it imports, so a changed export that no longer
fits the schema is refused there, with the reason, in Meshvault. When `send`
fails it only warns; keep the table or its JSON and try again, or save it
yourself. Without `send = false` the export is still returned, after it has
been sent.

## The Output window

Every line the exporter writes starts with `[Meshvault]`. It is quiet when
all goes well: how many models and meshes it exported, and that the export
was sent.

Problems are warnings that say what to do: clips that could not be loaded
(a clip still loading after 20 seconds is given up on), models or meshes it
had to skip, Meshvault not running, a play test in progress. A mesh that
fails is skipped on its own; an error while a model is put together skips
that model, and the others still export. An unexpected error prints its
stack trace, then the exporter's schema and Meshvault's version, for bug
reports.

If sending fails when the line sends the export itself, the exporter prints the export itself between
`=== JSON CHUNK i/n ===` and `=== END JSON EXPORT ===` lines, without the
prefix. Copy them into a `.json` file and import it: Meshvault joins the
chunks back together.

## The Studio link

Everything travels over `http://localhost:34875`:

- `GET /export.luau` serves the exporter (`?schema=1` for the older one).
- `POST /export` takes the export in pieces. The exporter sends 2 MB pieces;
  Meshvault accepts pieces of up to 64 MB and exports of up to 2 GB, holds at
  most 2 GB at once across exports still arriving and those waiting to be
  imported, and drops an unfinished export when no piece has come for 120
  seconds.

The line turns `HttpService.HttpEnabled` on only while it talks to Meshvault
and restores it afterwards. Meshvault accepts connections from this machine
only, and requires custom headers on exports, which a web page cannot send.
There is no other authentication: any program on this machine can send
Meshvault an export.

Only one program can use the port. If it is taken (usually by another
Meshvault), Meshvault warns at startup, the status bar reads
*Studio link unavailable*, and exporting from Studio is off until the port is
free and Meshvault is restarted.

The exporters' source is in [`crates/meshvault/studio/`](../crates/meshvault/studio)
and is built into the app. [`tools/studio/stand_in_link.py`](../tools/README.md)
serves them on another port, for trying changes without a running Meshvault.
