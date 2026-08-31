# What the KiCad 10 IPC API actually does

Notes from testing KiCase against a running **KiCad 10.0.3** (Ubuntu 24.04
build). Everything here was verified against a live instance, not inferred from
documentation. `cargo run -p kicase-kicad --example probe_kicad` is the probe
used to check these.

## Verified working

| Command | Notes |
| --- | --- |
| `GetVersion`, `Ping` | Fine. |
| `GetOpenDocuments` / `GetCurrentProjectPath` | Returns the project *directory*. |
| `SaveDocumentToString` | Returns the full board s-expression. This is KiCase's geometry source. |
| `GetItems` (footprints) | Fine, but see the read-model gap below. |
| `GetSelection` / `AddToSelection` / `ClearSelection` | Fine; used to highlight objects a diagnostic refers to. |
| `SaveSelectionToString` | Returns a bare `(footprint ...)` with `version` / `generator` / `generator_version` fields. |
| `GetBoardEnabledLayers` / `SetBoardEnabledLayers` | Fine. Layer ids here are the **API** ids, not board-file ids. |
| `BeginCommit` / `EndCommit` | Fine. |

## Verified broken or missing

### `ParseAndCreateItemsFromString` creates nothing

Sending any item text returns a `CreateItemsResponse` of **zero bytes** — the
status field is left unset, which the Rust client reports as `IRS_UNKNOWN` — and
no item appears on the board after committing.

This was tested with:

* a single `gr_line`;
* a footprint with and without `version` / `generator` fields;
* a footprint wrapped in a `kicad_pcb` document;
* **KiCad's own output** from `SaveSelectionToString` for an existing footprint.

All four behaved identically. The command is not usable in 10.0.3.

**Consequence.** KiCase cannot create the 3D preview footprint over IPC. Instead
it writes the footprint into a project-local library
(`.enclosure/kicase.pretty/ENCLOSURE_PREVIEW.kicad_mod`), registers that library
in the project's `fp-lib-table`, and asks the user to place it once. KiCase then
reads the placed footprint's position back from the board and writes
`enclosure.step` relative to it, so it does not matter where the user drops it.

KiCase still *tries* the API call first, so it will start working by itself if a
future KiCad implements the command.

### `RefreshEditor` is not handled by the PCB editor

Returns `AS_UNHANDLED`: *no handler available for request of type
`kiapi.common.commands.RefreshEditor`*. KiCase treats a refresh failure as
normal and never reports it as an error.

**Consequence.** After a rebuild, an already-open 3D viewer may show the old
model. Close and reopen it, or press **Alt+3** again.

### There is no command for renaming a user layer

The 59 commands in 10.0.1/10.0.3 include no layer-rename operation. The board
stackup carries a `user_name` per layer and round-trips it, but user layers are
not generally part of the stackup, so this only sometimes helps.

**Consequence.** KiCase enables and records the layers it claims, attempts the
stackup route, and otherwise prints the exact names to set by hand in
**Board Setup → Board Editor Layers**.

## Client-library gaps (`kicad-ipc-rs` 0.5.1)

The typed read model loses information KiCase needs:

* `PcbGraphicShapeGeometry::Polygon` reports only a vertex **count**, not the
  vertices.
* Pad positions are not resolved against their parent footprint's placement.
* The `proto` module is `pub(crate)`, so there is no escape hatch to the raw
  protobuf.

**Consequence.** KiCase reads geometry from the board document
(`SaveDocumentToString`) rather than from typed items. That is still the IPC
API, it is lossless, and it has the useful side effect that all geometry work
runs headlessly on a saved `.kicad_pcb`.

## Layer ids differ between the API and the board file

| Layer | Board file id | API id |
| --- | --- | --- |
| `Edge.Cuts` | 25 | 47 |
| `User.1` | 39 | 53 |
| `User.9` | 55 | 61 |
| `User.10` | 57 | 63 |

The API's user-layer ids are also **not contiguous**: `User.9` is 61 and
`User.10` is 63, because 62 is KiCad's internal `Rescue` layer. KiCase matches
layers by canonical name everywhere and converts to an id only at the point of
the API call.

## Building on Windows: OpenCascade against CMake 4

The vendored OpenCascade 7.8.1 declares `cmake_minimum_required` below 3.5,
which CMake 4.0 refuses outright. Linux distributions are still on CMake 3.x so
it goes unnoticed there; a current Windows CMake stops the build dead. Setting
`CMAKE_POLICY_VERSION_MINIMUM=3.5` in the environment is CMake's documented
escape hatch and is enough — verified on CMake 4.2.0-rc1 with Visual Studio
Build Tools 2022.

## `${KIPRJMOD}` needs a project file

A 3D model path using `${KIPRJMOD}` silently resolves to nothing if the board
has no sibling `.kicad_pro`. The model simply does not appear, with no error.
Worth knowing when testing a board copied somewhere on its own.

## Per-model visibility works; per-model *viewer* checkboxes do not exist

`(hide yes)` inside a footprint's `(model ...)` is honoured — verified by
rendering the same board with and without it. That is the mechanism behind
hiding the lid.

The 3D viewer's own model-visibility checkboxes are per footprint *category*
(`show_footprints_normal`, `_insert`, `_virtual`, `_dnp`, `_not_in_posfile` in
`3d_viewer.json`), not per model. The per-layer checkboxes (`show_user1` and
friends) control board layer graphics, not 3D models.

## Board files must use canonical layer names

A graphic written as `(layer "Enclosure")` — the *display* name of `User.1` —
makes KiCad refuse to load the whole board ("Failed to load board"), even though
the layers table defines that display name. Graphics must reference layers by
their canonical name, `(layer "User.1")`.

KiCase's reader accepts both, since reading is where tolerance is cheap, but
anything KiCase writes uses canonical names.

## 3D model placement

A front-layer footprint's 3D model has its origin at the **top surface** of the
board, with the model's Y axis pointing opposite to board Y. KiCase geometry
uses Z = 0 at the *bottom* surface of the board and Y up, so the preview
footprint carries `(offset (xyz 0 0 -<pcb thickness>))` and the assembly is
written with Y already flipped.
