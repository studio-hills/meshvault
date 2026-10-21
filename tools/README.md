# Development tools

Small scripts for developing Meshvault. None of them is part of the build or
the normal test run; they make the inputs that the ignored tests need, or let
you try the Studio exporter without touching a running Meshvault.

They need Python 3.10 or newer; the Studio tools also need Roblox Studio.
Run them from the repository root.

| Tool | What it does |
| --- | --- |
| `make_test_workspace.py OUT [--schema 1\|2] [--force]` | Builds a small workspace: one animated model, TestRig, with the skinned sample head `crates/rbx_mesh/meshes/13674780763`, plus (schema 2) coloured primitive parts. Open it in Meshvault, or point `MESHVAULT_WS` at it. |
| `studio/stand_in_link.py [--port 34876] [--out DIR]` | A stand-in for Meshvault's Studio link on another port. It serves `crates/meshvault/studio/exporter_v{1,2}.luau` with its own address in them, saves what they send as `export.json`, saves posted Studio samples as `<name>.json`, and refuses everything sent to `/refuse` (413). Output goes to `target/stand-in` unless `--out` says otherwise. |
| `studio/sample_animator.luau` | Run in Studio against a test place: plays every clip of the models under `ReplicatedStorage.MeshvaultTest` in Studio's own Animator and posts the joint, bone and part poses to the stand-in link. The header says how the test place is laid out. |

## The ignored tests

`cargo test` skips six tests that need real data, which is not committed.
Run one with its environment variables set, using **absolute paths** (tests
run from their crate's folder):

```sh
VARIABLE=/absolute/path cargo test -p meshvault -- --ignored <test name> --nocapture
```

The `anim` and `scene` tests read an export from `MESHVAULT_EXPORT`; the
older names `RBX_V1_EXPORT` and `RBX_EXPORT` still work in its place for
one release. They fail when a variable they need is not set.
`studio::export_file::tests::parses_real_export` still reads only
`RBX_EXPORT`, and returns early and reports `ok` without it, so check its
output says what it tested.

| Test | Needs | Where the input comes from |
| --- | --- | --- |
| `anim::real_export_tests::rigs_rest_pose_and_binding` | `MESHVAULT_EXPORT` (or `RBX_V1_EXPORT`/`RBX_EXPORT`): an export with models (schema 1 or 2) | an export from Studio, saved by Meshvault or the stand-in link |
| `scene::model_view::tests::plays_real_export` | `MESHVAULT_EXPORT`, as above | as above |
| `studio::export_file::tests::parses_real_export` | `RBX_EXPORT`: an export without warnings or skipped entries | as above |
| `export::gltf::tests::exports_workspace_models` | `MESHVAULT_WS`: a workspace; optional `MESHVAULT_GLB_OUT` (default: the temp folder) | any workspace, e.g. from `make_test_workspace.py` |
| `export::obj::tests::exports_workspace_models` | `MESHVAULT_WS`, as above | as above |
| `anim::real_export_tests::matches_studio_samples` | `MESHVAULT_EXPORT` and `MESHVAULT_STUDIO_SAMPLES`; optional `MESHVAULT_DETAIL` (a joint, bone or part name, printed at every sample) | a developer diagnostic: export the test place's models through the stand-in link, then run `sample_animator.luau` against the same models. It prints differences rather than failing. |

For example, with a generated workspace:

```sh
python tools/make_test_workspace.py target/test-ws
MESHVAULT_WS="$PWD/target/test-ws" cargo test -p meshvault -- --ignored exports_workspace_models
```
