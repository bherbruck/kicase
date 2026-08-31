# KiCase - KiCad Enclosure Designer

## Implementation Specification

**Working codename:** `kicase`
**Language:** Rust
**Initial target:** KiCad 10.0.x
**Platforms:** Windows, Linux, macOS
**License:** MIT or Apache-2.0
**Primary goal:** Build a native-feeling KiCad PCB Editor plugin that lets users design simple parametric electronics enclosures directly from the PCB editor using KiCad drawing layers, then generates real B-rep STEP geometry that appears with the PCB in KiCad's 3D Viewer.

Do not turn this into a general-purpose CAD system. The product is specifically an **electronics enclosure generator built around KiCad PCB geometry**.

---

# 1. Core user experience

The intended workflow is:

```text
KiCad PCB Editor
      │
      ├── Edge.Cuts
      │      PCB shape
      │
      ├── Enclosure
      │      case outline / shell intent
      │
      ├── Enclosure.Cuts
      │      holes / connector openings / vents
      │
      ├── Enclosure.Datums
      │      side-wall reference planes
      │
      └── Enclosure.Solids
             ribs / bosses / extra material

              ↓

       Rust enclosure plugin

              ↓

         B-rep geometry

          ┌────┴─────┐
          │          │
        STEP        STL
          │
          ▼
   KiCad 3D Viewer
```

The user should not normally need FreeCAD, Fusion 360, SolidWorks, or OpenSCAD.

The PCB Editor itself is the 2D sketching interface.

The plugin interprets ordinary KiCad graphics as enclosure features.

---

# 2. Design principles

## 2.1 KiCad owns 2D geometry

Do not implement a separate sketching engine.

Users draw using normal KiCad tools:

* lines
* arcs
* circles
* rectangles
* polygons

The plugin reads those objects through the KiCad IPC API.

KiCad 10 supports executable IPC plugins and exposes the socket/token to launched plugins using environment variables. Use the executable plugin mechanism, not deprecated SWIG Python bindings.

Use:

```toml
kicad-ipc-rs = "0.5.1"
```

or the current compatible release if newer when implementation begins.

`kicad-ipc-rs 0.5.1` targets KiCad 10.0.1 and exposes the required item retrieval/create/update operations.

---

## 2.2 Semantic data is stored separately

Never encode important parameters in:

* line widths
* colors
* object names
* footprint reference designators
* arbitrary magic coordinates

KiCad graphics have persistent UUIDs. Use those UUIDs to associate graphics with enclosure semantics stored in:

```text
.enclosure/enclosure.toml
```

Example:

```toml
version = 1

[layers]
outline = "User.1"
datums = "User.2"
cuts = "User.3"
solids = "User.4"

[shell]
wall = 2.0
bottom = 2.0
pcb_clearance = 0.75
pcb_z = 4.0
top_clearance = 3.0
corner_radius = 3.0

[[datum]]
id = "front"
graphic_uuid = "8b27..."
z_origin = "pcb_top"
z_offset = 0.0
normal = "right"

[[feature]]
id = "usb"
graphic_uuid = "abc3..."
kind = "cutout"
datum = "front"
depth = "through"
clearance = 0.3
```

KiCad supports additional `User.*` layers and custom display names, so the plugin should create or assign user layers rather than abusing fabrication layers.

---

# 3. Project structure

Create a Cargo workspace:

```text
kicase/
├── Cargo.toml
├── README.md
├── LICENSE
├── crates/
│
│   ├── kicase-app/
│   │   executable KiCad plugin
│   │
│   ├── kicase-kicad/
│   │   KiCad IPC adapter
│   │
│   ├── kicase-model/
│   │   semantic enclosure model
│   │
│   ├── kicase-geometry/
│   │   geometry abstraction
│   │
│   ├── kicase-occ/
│   │   OpenCascade backend
│   │
│   ├── kicase-export/
│   │   STEP/STL/OpenSCAD output
│   │
│   └── kicase-ui/
│       egui application UI
│
├── plugin/
│   ├── plugin.json
│   └── icons/
│
├── examples/
│   ├── rectangular-board/
│   ├── rounded-board/
│   └── usb-cutout/
│
└── tests/
```

Do not allow OpenCascade types outside `kicase-occ`.

---

# 4. Geometry abstraction

The enclosure model must not depend directly on OpenCascade.

Create a kernel trait approximately like:

```rust
pub trait CadKernel {
    type Solid;
    type Face;
    type Edge;

    fn extrude(
        &self,
        profile: &Profile2d,
        distance: Length,
    ) -> Result<Self::Solid>;

    fn union(
        &self,
        a: &Self::Solid,
        b: &Self::Solid,
    ) -> Result<Self::Solid>;

    fn subtract(
        &self,
        a: &Self::Solid,
        b: &Self::Solid,
    ) -> Result<Self::Solid>;

    fn fillet(
        &self,
        solid: &Self::Solid,
        edges: &[Self::Edge],
        radius: Length,
    ) -> Result<Self::Solid>;

    fn translate(
        &self,
        solid: &Self::Solid,
        transform: Transform3d,
    ) -> Result<Self::Solid>;

    fn export_step(
        &self,
        solid: &Self::Solid,
        path: &Path,
    ) -> Result<()>;

    fn export_stl(
        &self,
        solid: &Self::Solid,
        path: &Path,
        tolerance: Length,
    ) -> Result<()>;
}
```

The exact API may evolve, but this separation is mandatory.

---

# 5. Initial CAD kernel

Use OpenCascade initially.

Preferred Rust interface:

```text
opencascade-rs
```

The crate provides a Rust-facing API over OpenCascade and supports the sort of B-rep modeling and STEP/STL interchange required here. Its bindings are still developing, so keep the backend isolated behind `CadKernel`.

OpenCascade is acceptable even though it introduces C++ internally.

Do not route canonical geometry through OpenSCAD.

Canonical pipeline:

```text
semantic model
      ↓
Rust
      ↓
B-rep
      ↓
 ┌────┴────┐
STEP      STL
```

---

# 6. Units

All public geometry APIs must use strongly typed lengths.

Use millimeters externally.

Do not pass naked `f64` values representing physical distances between modules.

Implement or use:

```rust
struct Length(f64);
```

or a suitable units crate.

Coordinates imported from KiCad must be converted once at the `kicase-kicad` boundary.

---

# 7. Enclosure coordinate system

Define:

```text
X/Y = KiCad PCB plane
Z   = perpendicular to PCB
Z=0 = PCB bottom surface by default
```

Shell configuration defines:

```rust
pub struct ShellParameters {
    pub wall: Length,
    pub floor: Length,
    pub pcb_clearance_xy: Length,
    pub pcb_standoff_height: Length,
    pub component_clearance_z: Length,
    pub corner_radius: Length,
}
```

Do not assume the PCB is rectangular.

Support arbitrary closed `Edge.Cuts` contours containing:

* lines
* arcs

Polygonize curves only when necessary.

Prefer retaining analytic curves in the B-rep backend.

---

# 8. Shell generation

Given PCB `Edge.Cuts`:

1. Construct closed board profile.
2. Offset outward by `pcb_clearance_xy`.
3. Offset again by `wall` to obtain exterior profile.
4. Extrude exterior profile.
5. Extrude interior cavity.
6. Boolean subtract cavity.
7. Retain configured floor thickness.
8. Apply external corner fillets where possible.

Result:

```text
        exterior
     ┌────────────┐
     │            │
     │   cavity   │
     │            │
     │ ┌────────┐ │
     │ │  PCB   │ │
─────┴─┴────────┴─┴─────
        floor
```

If filleting fails for particular topology, emit a warning and generate usable unfilleted geometry instead of failing the entire build.

---

# 9. Lid

v0.1 needs one lid style:

**screw-on inset lid**

Parameters:

```rust
pub struct LidParameters {
    pub thickness: Length,
    pub fit_clearance: Length,
    pub lip_depth: Length,
    pub lip_thickness: Length,
}
```

Output separate solids:

```text
bottom.step
lid.step
```

and a combined preview assembly:

```text
enclosure.step
```

The preview should place the lid in its assembled location.

Later lid styles can include:

```text
snap
slide
hinged
```

Do not implement those in v0.1.

---

# 10. User layers

On initialization, assign four unused KiCad user layers.

Preferred display names:

```text
Enclosure
Enclosure.Datums
Enclosure.Cuts
Enclosure.Solids
```

Store the canonical layer mapping in `enclosure.toml`.

Never assume `User.1` through `User.4` are unused.

If they are occupied, find available user layers.

KiCad user layers may contain arbitrary graphics and can have custom display names.

---

# 11. Enclosure outline

Default behavior:

If the `Enclosure` layer contains no valid closed outline:

```text
case outline = offset(Edge.Cuts, pcb_clearance)
```

If a closed outline exists on `Enclosure`:

```text
case cavity outline = user's enclosure outline
```

This lets users intentionally make the enclosure larger or differently shaped than the PCB.

---

# 12. Datums

Datums are first-class enclosure entities.

A datum is a line drawn on `Enclosure.Datums`.

The XY position and orientation of that line define a **vertical plane**.

Conceptually:

```text
top view

       enclosure

   ┌──────────────────────┐
   │                      │
   │                      │
   └──────────────────────┘
          │
          │ datum D1
          │
```

That line produces:

```text
vertical plane through D1
```

Datum data:

```rust
pub struct SideDatum {
    pub id: DatumId,
    pub graphic_uuid: KiCadUuid,
    pub z_origin: ZOrigin,
    pub z_offset: Length,
    pub normal: DatumNormal,
}
```

Supported Z origins:

```rust
CaseBottom
PcbBottom
PcbTop
CaseTop
Absolute
```

The datum direction determines its local X axis.

The perpendicular horizontal direction determines the wall-normal axis.

Z remains vertical.

Thus a datum provides a local 2D sketch plane:

```text
U = along datum line
V = Z
N = wall normal
```

This is fundamental to the design.

---

# 13. Side cutouts

A graphic on `Enclosure.Cuts` may be associated with a datum.

Supported v0.1 shapes:

* rectangle
* circle
* rounded rectangle
* arbitrary closed polygon

The 2D graphic is interpreted in datum-local coordinates.

Example:

```text
Enclosure.Cuts:

      ┌───────────────┐
      │ USB-C opening │
      └───────────────┘

──────────────────────── D1
```

The shape becomes a cutting solid projected along datum normal through the enclosure wall.

Cut parameters:

```rust
pub struct Cutout {
    pub graphic_uuid: KiCadUuid,
    pub datum: DatumId,
    pub clearance: Length,
    pub depth: CutDepth,
}
```

v0.1 only needs:

```rust
CutDepth::Through
```

---

# 14. Top and bottom cutouts

Closed graphics on `Enclosure.Cuts` that are not attached to a side datum may be configured as:

```text
Top
Bottom
```

These are extruded along Z and boolean-subtracted.

Typical uses:

* button openings
* LED windows
* vents
* access holes

---

# 15. Solids

Closed graphics on `Enclosure.Solids` become additive geometry.

v0.1 solid types:

```text
Boss
Rib
Extrusion
```

A generic extrusion is enough initially.

Parameters:

```rust
pub struct AddedSolid {
    pub graphic_uuid: KiCadUuid,
    pub z_start: Length,
    pub height: Length,
}
```

---

# 16. PCB mounting holes and standoffs

Automatically detect likely PCB mounting holes.

Initial algorithm:

Find footprints or pads containing non-plated through holes with circular drills.

Do not blindly create standoffs for every NPTH.

The UI should show detected holes and allow:

```text
[x] Hole 1
[x] Hole 2
[x] Hole 3
[x] Hole 4
```

For each selected mounting hole create:

```text
floor
  +
cylindrical boss
  -
center hole
```

Parameters:

```rust
pub struct StandoffParameters {
    pub outer_diameter: Length,
    pub height: Length,
    pub hole_diameter: Length,
    pub insert_diameter: Option<Length>,
    pub insert_depth: Option<Length>,
}
```

Heat-set inserts are optional.

---

# 17. KiCad 3D preview

Generate:

```text
${KIPRJMOD}/.enclosure/generated/enclosure.step
```

Create a special KiCad footprint on the board representing the enclosure preview.

Suggested reference:

```text
ENCLOSURE_PREVIEW
```

The footprint must:

* be excluded from BOM
* be excluded from position files
* be locked
* contain no electrical pads
* attach `generated/enclosure.step`
* stay at a known project-relative origin

Do not create duplicate preview footprints.

On every rebuild:

1. regenerate the STEP file
2. preserve the preview footprint
3. attempt appropriate KiCad editor refresh
4. do not fail if the already-open 3D Viewer cannot automatically reload

KiCad's API exposes `RefreshEditor`, but do not assume that this guarantees a live reload of an already-open 3D viewer. Treat automatic 3D refresh as optional until verified experimentally.

The user must always be able to reopen or manually refresh the 3D Viewer and see the new model.

---

# 18. Generated files

Project layout:

```text
project/
├── board.kicad_pcb
├── board.kicad_sch
│
└── .enclosure/
    ├── enclosure.toml
    │
    ├── generated/
    │   ├── enclosure.step
    │   ├── bottom.step
    │   ├── lid.step
    │   ├── bottom.stl
    │   └── lid.stl
    │
    └── openscad/
        ├── generated.scad
        └── custom.scad
```

Generated files may be safely overwritten except:

```text
custom.scad
```

Never overwrite `custom.scad` after creating it.

---

# 19. OpenSCAD export

OpenSCAD is an optional secondary representation.

It is **not** the geometry kernel.

Generate:

```text
openscad/generated.scad
```

with modules approximately like:

```scad
module enclosure_bottom() {
    ...
}

module enclosure_lid() {
    ...
}

module enclosure() {
    enclosure_bottom();
    enclosure_lid();
}
```

Generate `custom.scad` only if it does not exist:

```scad
include <generated.scad>

// This file is never overwritten by KiCase.

enclosure();
```

Expose useful parameters where practical.

Example:

```scad
wall = 2.0;
corner_radius = 3.0;
```

The purpose is to give users a hackable derivative for custom print-specific modifications.

Changes made in `custom.scad` do **not** feed back into the canonical STEP model.

Document that clearly.

---

# 20. UI

Use:

```text
egui / eframe
```

The main action:

```text
Tools
  → Enclosure Designer
```

Minimum UI:

```text
┌───────────────────────────────────┐
│ KiCad Enclosure Designer          │
│                                   │
│ Shell                             │
│ PCB clearance       0.75 mm       │
│ Wall                2.00 mm       │
│ Floor               2.00 mm       │
│ Standoff height     4.00 mm       │
│ Top clearance       3.00 mm       │
│ Corner radius       3.00 mm       │
│                                   │
│ Lid                               │
│ Thickness           2.00 mm       │
│ Fit clearance       0.20 mm       │
│ Lip depth            3.00 mm      │
│                                   │
│ Mounting holes                     │
│ ☑ H1                              │
│ ☑ H2                              │
│ ☑ H3                              │
│ ☑ H4                              │
│                                   │
│ Features                          │
│ ▾ Datums                          │
│   Front                           │
│   Rear                            │
│ ▾ Cutouts                         │
│   USB-C                           │
│   Switch                          │
│                                   │
│ [ Rebuild ]                       │
│ [ Export STEP ]                   │
│ [ Export STL ]                    │
│ [ Generate OpenSCAD project ]     │
└───────────────────────────────────┘
```

Do not build a custom 3D viewport in v0.1.

KiCad's 3D Viewer is the viewport.

---

# 21. KiCad actions

Expose plugin actions:

```text
Enclosure Designer
Rebuild Enclosure
```

Optional:

```text
Export Enclosure
```

KiCad IPC executable plugins use `plugin.json`; KiCad's add-on system also supports IPC-runtime package metadata.

Use a single executable with command-line subcommands if practical:

```text
kicase designer
kicase rebuild
kicase export
```

---

# 22. Undo and board modification

Any modifications to KiCad board objects must use KiCad commit semantics where available:

```text
BeginCommit
...
CreateItems / UpdateItems
...
EndCommit
```

The Rust IPC library exposes these operations.

Generating external `.step`, `.stl`, `.toml`, or `.scad` files does not require a KiCad board commit.

---

# 23. Error handling

Never panic for malformed user geometry.

Return actionable errors such as:

```text
Enclosure outline is not closed.
Datum "Front" references a deleted KiCad graphic.
Cutout "USB-C" has no associated datum.
PCB Edge.Cuts contains two disconnected outer contours.
Wall thickness must be greater than zero.
Boolean cut failed near cutout "Switch".
```

Where possible highlight/select the offending KiCad object using its UUID.

Geometry generation should proceed with unaffected features when safely possible.

---

# 24. Deleted objects

If `enclosure.toml` references a KiCad UUID that no longer exists:

* mark the corresponding feature as orphaned
* show it in the UI
* allow deletion/reassignment
* do not silently bind it to another graphic

Never identify objects by geometry alone when a persistent UUID exists.

---

# 25. Determinism

Identical:

```text
board geometry
+
enclosure.toml
```

must generate equivalent enclosure geometry.

Do not store hidden state in:

* global config
* temporary files
* registry
* user home directory

Global UI preferences may be stored separately, but project geometry must be project-local and reproducible.

---

# 26. CLI

The binary should also work outside the normal GUI flow where practical.

Provide:

```text
kicase --help
kicase rebuild
kicase export --step
kicase export --stl
kicase validate
```

For KiCad 10, IPC operations require a running KiCad GUI instance, so do not pretend the plugin can use the KiCad 10 IPC API headlessly. KiCad's developer docs explicitly note this limitation for versions 9 and 10.

Pure geometry tests and operations on already serialized intermediate data may run headlessly.

---

# 27. Serialization

Use `serde`.

`enclosure.toml` must contain:

```toml
version = 1
```

Version the schema from day one.

Unknown fields should preferably be ignored for forward compatibility.

Unknown enum variants should produce a meaningful compatibility error.

---

# 28. Internal semantic model

Use an explicit model approximately like:

```rust
pub struct Enclosure {
    pub shell: Shell,
    pub lid: Lid,
    pub datums: Vec<Datum>,
    pub cutouts: Vec<Cutout>,
    pub solids: Vec<AddedSolid>,
    pub standoffs: Vec<Standoff>,
}

pub struct Shell {
    pub profile: Profile2d,
    pub wall: Length,
    pub floor: Length,
    pub height: Length,
    pub corner_radius: Length,
}

pub enum Datum {
    Side(SideDatum),
}

pub enum Feature {
    Cutout(Cutout),
    AddedSolid(AddedSolid),
    Standoff(Standoff),
}
```

Keep semantic objects independent from KiCad protobuf types.

---

# 29. Intermediate geometry representation

Create internal geometry primitives:

```rust
Point2
Point3
Vector2
Vector3
LineSegment2
Arc2
Circle2
Polygon2
Profile2d
Plane3
Transform3d
```

The KiCad adapter translates KiCad objects into these.

The CAD backend translates these into kernel-native topology.

This gives clean boundaries:

```text
KiCad protobuf
      ↓
kicase-kicad
      ↓
neutral geometry
      ↓
semantic enclosure
      ↓
CadKernel
      ↓
OpenCascade
```

---

# 30. v0.1 scope

v0.1 must support:

1. Connect to running KiCad 10.
2. Read `Edge.Cuts`.
3. Detect closed PCB profile.
4. Create/configure enclosure user layers.
5. Generate enclosure shell around arbitrary line/arc PCB profiles.
6. Wall thickness.
7. Floor thickness.
8. PCB XY clearance.
9. PCB standoff height.
10. Top clearance.
11. Corner radius.
12. Screw-on inset lid.
13. Automatic candidate mounting-hole detection.
14. Cylindrical standoffs.
15. Side datum lines.
16. Rectangular side cutouts.
17. Circular side cutouts.
18. Arbitrary polygon side cutouts.
19. Top/bottom cutouts.
20. Additive extrusions.
21. STEP export.
22. STL export.
23. Combined STEP preview.
24. KiCad preview footprint.
25. egui settings interface.
26. TOML persistence.
27. Optional generated OpenSCAD project.

Nothing else is required for v0.1.

---

# 31. Explicitly out of scope

Do NOT implement yet:

* general CAD sketch constraints
* arbitrary 3D freeform surfaces
* loft editor
* injection molding draft analysis
* shell thickness analysis
* hinges
* snap-fit generator
* DIN rail clips
* gasket channels
* IP-rating validation
* threaded geometry
* text embossing
* OpenSCAD-to-STEP conversion
* STEP import/editing
* FreeCAD integration
* cloud functionality
* collaborative editing
* schematic integration
* auto-connector recognition
* custom realtime 3D renderer

These can come later.

---

# 32. Tests

Unit-test:

```text
profile closure
line/arc conversion
polygon orientation
offset behavior
datum transformations
local datum coordinates
serialization
deleted UUID handling
shell dimensions
standoff placement
cutout transformations
```

Geometry golden tests should generate known STEP/STL artifacts but avoid binary byte-for-byte comparison if the kernel produces unstable metadata.

Instead verify:

* bounding box
* volume
* number of solids
* expected voids/intersections
* expected major dimensions

---

# 33. Required example boards

Create fixtures for:

### Rectangular PCB

```text
50 × 30 mm
4 mounting holes
```

Expected enclosure:

```text
clearance 1 mm
wall 2 mm
```

Exterior dimensions should be predictable.

### Rounded PCB

Board with rounded corners using Edge.Cuts arcs.

Verify shell follows curve geometry.

### USB cutout

Board with USB connector near enclosure edge.

Create front datum and rectangular cutout.

Verify opening intersects wall but not floor.

### Nonrectangular PCB

Simple trapezoidal or L-shaped board.

Verify no rectangular-board assumptions exist.

---

# 34. Definition of done for first usable release

A user can:

1. Install the plugin.
2. Open an ordinary KiCad 10 PCB.
3. Click **Enclosure Designer**.
4. Click **Initialize Enclosure**.
5. Configure wall/floor/clearance.
6. Draw a datum line.
7. Draw a rectangle representing a USB opening.
8. Associate that rectangle with the datum.
9. Click **Rebuild**.
10. Open KiCad's 3D Viewer.
11. See the PCB sitting inside a generated enclosure with the USB opening correctly cut into the side.
12. Export separate printable bottom and lid STL files.
13. Export bottom and lid STEP files.
14. Close/reopen the project and retain all enclosure settings.

If this workflow works reliably, v0.1 is successful.

---

# 35. Implementation order

Implement in this order:

```text
Phase 1
KiCad IPC connection
↓
Read board geometry
↓
Neutral geometry types
↓
Edge.Cuts parser

Phase 2
OpenCascade backend
↓
Extrude simple profile
↓
Shell via boolean subtraction
↓
STEP export

Phase 3
Generate preview STEP
↓
Create KiCad preview footprint
↓
Verify in KiCad 3D Viewer

Phase 4
TOML project model
↓
Custom user layers
↓
Graphic UUID mapping

Phase 5
Datums
↓
Datum coordinate transform
↓
Side cutouts

Phase 6
Standoffs
↓
Lid
↓
STL export

Phase 7
egui interface

Phase 8
OpenSCAD derivative export
↓
Packaging
↓
Documentation
```

Do not begin advanced UI work before a generated STEP enclosure can be displayed around a real PCB in KiCad.

That is the first major technical milestone.

---

# 36. First milestone

The first concrete demonstration should be intentionally crude:

```text
PCB Edge.Cuts
      ↓
Rust plugin
      ↓
rectangular/rounded shell
      ↓
enclosure.step
      ↓
special KiCad footprint
      ↓
Alt+3
      ↓
PCB visibly inside enclosure
```

No datums.

No lid.

No pretty UI.

Prove this path first.

Commit it as:

```text
milestone: generated B-rep enclosure visible in KiCad 3D viewer
```

Only then build the feature system.

---

# 37. Engineering priorities

Prioritize, in order:

1. Correct geometry.
2. Stable project format.
3. Reproducible regeneration.
4. Native-feeling KiCad workflow.
5. Useful diagnostics.
6. Cross-platform packaging.
7. Performance.

Performance is unlikely to be a meaningful problem for enclosure-sized geometry.

Do not prematurely optimize.

---

# 38. Coding requirements

Use:

```text
cargo fmt
cargo clippy
cargo test
```

CI should build/test on:

```text
windows-latest
ubuntu-latest
macos-latest
```

Keep unsafe Rust isolated to dependency/FFI boundaries.

No unwraps in normal user-facing paths.

Use `thiserror` for library errors and a suitable application-level error/reporting crate where useful.

Use structured logging through `tracing`.

---

# 39. Documentation

README must include:

* what the project does
* screenshot/GIF eventually
* supported KiCad versions
* installation
* basic enclosure workflow
* layer semantics
* datum explanation
* generated file layout
* OpenSCAD customization explanation
* known limitations

Include a diagram showing the datum coordinate system.

---

# 40. Architecture invariant

The most important architectural rule is:

```text
KiCad is the sketch editor.
enclosure.toml stores meaning.
Rust owns the enclosure model.
The CAD kernel only creates geometry.
STEP is the canonical generated CAD artifact.
OpenSCAD is an optional editable derivative.
```

Do not collapse these responsibilities together.

The eventual project should be able to replace:

```text
OpenCascade
```

with another B-rep kernel without changing:

* KiCad integration
* enclosure.toml
* datum behavior
* UI behavior
* project files

That separation is mandatory.

---

# 41. Codex execution instruction

Begin implementation immediately.

Do not attempt to implement the entire specification in one giant change.

Work milestone by milestone and leave the repository buildable after every milestone.

For uncertain KiCad IPC details:

1. inspect current KiCad 10 IPC documentation
2. inspect current `kicad-ipc-rs` APIs
3. build a minimal live test against KiCad
4. document any API limitation discovered
5. choose the smallest reliable workaround

Do not invent undocumented KiCad behavior.

The first target is:

**Generate a B-rep enclosure from `Edge.Cuts`, export it as STEP, attach that STEP to a generated preview footprint, and successfully view the PCB and enclosure together in KiCad's 3D Viewer.**

Everything else follows from that.
