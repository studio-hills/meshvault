# Measuring tree scrolling

`ui::import::replay::tree_scroll_replay` is an ignored, headless debug test.
It parses the real export, opens `ImportDialog::show`, and fills an isolated
workspace's in-memory library with the export's meshes and models before
calling `Browser::show`. It never starts eframe, reads user settings or
credentials, downloads assets, or writes rig files. Its temporary workspace
is deleted when the test ends. The export stays outside git.

From this worktree in PowerShell:

```powershell
$env:MESHVAULT_EXPORT='C:\Users\jaden\.t3\scratch\2026-10-19-can-you-clone-this-app-9ed3e37c\tools\large_import\game_107778070777162.json'
$env:MESHVAULT_SCROLL_BUDGET='1'
$env:MESHVAULT_SCROLL_SELECTION='1'
cargo test -p meshvault --target-dir C:\Users\jaden\.t3\scratch\2026-10-19-can-you-clone-this-app-9ed3e37c\target-tree tree_scroll_replay -- --ignored --nocapture
```

The budget flag fails a run whose p95 exceeds 16.667 ms. The selection flag
adds a library Ctrl+A stress scenario. Neither is needed to print the four
primary measurements. The test also asserts that wheel input actually moves
the viewport and that fewer than 100 rows are laid out in a frame.

Each scenario uses the app's fonts and dark theme at 1280 by 800 logical
pixels. It sends a 240-point wheel event each frame, warms up for six frames,
then records 30 frames. Fully expanded means every export folder is open;
the partial state has the first three folder levels open and deeper folders
closed. Library model sections stay open, and import models' own mesh
disclosures stay closed, matching the existing expand-all command. All
import checkboxes are selected so the footer is included in the measurement.
Library assets are pending and no renderer is supplied.

These are CPU egui frame times, including layout and paint-shape generation.
They exclude parsing, initial construction/cache building, GPU rendering and
the rest of the desktop app. Absolute times vary with the machine and load;
this is not an end-to-end FPS claim.

## Results on the supplied export

The input is 485,404,296 bytes, with 6,347 parsed meshes and 1,385 models.
The import tree has 1,417 selectable rows (models plus unused meshes), 196
folders and a maximum folder depth of 10. The library has 6,347 mesh rows and
1,385 model rows, with 1,625 mesh folders and a maximum folder depth of 12.

Baseline source: `bb77b9806e46b1fb96ff6d882d5db90ce5519fd2`, with the same
headless replay added before the implementation changed.

| View and state | Before median / p95 (ms) | After median / p95 (ms) |
| --- | ---: | ---: |
| Import, expanded | 74.134 / 77.503 | 5.754 / 7.751 |
| Import, partial | 22.587 / 24.500 | 5.554 / 7.880 |
| Library, expanded | 189.868 / 200.402 | 2.371 / 4.700 |
| Library, partial | 55.534 / 58.008 | 2.352 / 4.976 |

Only 18 import rows and 24 library rows are laid out per scrolling frame,
including a row before the viewport for reverse keyboard navigation. The
replay scrolls about 8,000 points through each primary scenario.

A separate Ctrl+A probe after virtualization exposed a remaining selected
retry-list scan: 6.851 ms median / 8.209 ms p95. Caching that list reduced the
same stress scenario to 1.138 / 2.200 ms. It continues scrolling from the
expanded scenario, so its absolute timing is not directly comparable to
the primary scenarios above.

## What changed

Previously each frame recursively laid out every expanded row, sorted
folders (and import entries), allocated folder paths/IDs, walked descendants
for every folder, and repeated filtering and counts. Import model headers
also scanned their parts, even with the model's own disclosure closed.
The import footer rebuilt the selected entry union and a map collection per
selected mesh. The library rebuilt model groups, statuses and selection
order even while simply scrolling.

- `ui/tree.rs` builds a sorted, flat topology once, using an explicit stack.
  Each folder stores a descendant row boundary and a range in the ordered
  entry list. Prefix totals give constant-time folder counts. Full paths
  are reconstructed only for menus/hover UI. Stored state still uses
  `folder_id`; state changes bump a revision for visibility caches.
- `ui/import.rs` caches matching rows, visible rows, selection totals, kind
  and map counts, model details, and the import/download summary. Model mesh
  children are virtual rows too. Map collections are built only on import.
- `ui/library/list.rs` and `mod.rs` cache filtering, model sections, status
  totals and visible selection order. Reveal uses cached row positions so
  it can scroll directly to a row that was never laid out.
- `ui/widgets.rs` wraps `ScrollArea::show_rows` with the existing margin
  gutter. One preceding row and immediate focus scrolling allow Tab and
  Shift+Tab to cross viewport boundaries. Explicit UI scopes keep widget
  IDs stable as other rows enter and leave the viewport in egui 0.37.
- `ui/library/selection.rs`, `menus.rs` and `toolbar.rs` cache selected
  failures and status chips, invalidating on selection/download/data changes.
  `workspace/facts.rs` publishes fact changes to filtered lists and avoids
  rescanning an unchanged workspace each frame with the skinned filter on.

No dependencies were added. Filter/selection/open-state/data changes can
still rebuild caches; steady scrolling works on viewport rows.

## Verification and limits

`cargo fmt`, `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, and `cargo test -p meshvault`
passed using the separate target directory above. The normal suite reports
463 passed and 9 ignored; the real-export scrolling test was run separately
with its frame budget enabled.

Focused tests cover natural order and subtree ranges; closed ancestors,
search and empty matches; revision invalidation; filtered selection totals;
model children and missing meshes; data and download-state changes;
off-screen Shift-click ranges; reveal scrolling; selection retry caching;
Space on disclosure buttons; and Tab/Shift+Tab across viewport boundaries.
The 10,000-level flatten/visibility/drop test passes on a 256 KiB stack.

Native GUI/GPU rendering and mouse menus were not manually exercised. Menu
commands and their targets are retained in code. As part of using fixed
rows, long import labels truncate (full text is on hover), and folder bodies
appear immediately; disclosure arrows retain their animation.
