# Contributing to KiCase

## Getting set up

```sh
cargo build
cargo test --workspace
```

The CAD kernel is pure Rust. You still need `cmake` and a C compiler for
`nng`, the transport behind KiCad's IPC API, and on Linux the usual windowing
development packages for the designer window (`libx11-dev`, `libwayland-dev`,
`libxkbcommon-dev`, `libgl1-mesa-dev`).

The geometry suite runs against both CAD backends. `cargo test --workspace`
uses the reference OpenCascade backend, which does need `cmake` and a C++
compiler; `--features kicase-tests/truck` runs the identical expectations
against the pure-Rust one that ships.

## The rules that keep this project honest

1. **KiCad is the sketch editor.** Do not add a sketching engine, constraints,
   or a 3D viewport. KiCad already has all three.
2. **`enclosure.toml` stores meaning.** Never encode a parameter in a line
   width, a colour, an object name, a reference designator or a magic
   coordinate. Bind by KiCad UUID, always.
3. **No CAD-kernel type may appear outside its backend crate.** Everything
   else talks to the `CadKernel` trait. Swapping the kernel must not touch
   KiCad integration, the project format, datum behaviour or the UI — and a
   workaround for a kernel's quirks belongs inside that kernel's crate, never
   in code the other backend also runs.
4. **Lengths are typed.** `Length`, not `f64`, across module boundaries. KiCad
   coordinates are converted exactly once, at the `kicase-kicad` boundary.
5. **Never panic on user geometry.** Malformed drawings produce actionable
   errors that name the object, ideally with its UUID so it can be selected in
   KiCad.
6. **Degrade, do not fail.** If one feature cannot be generated, warn and build
   the rest.
7. **Do not invent KiCad behaviour.** If the API does not document something,
   test it against a real KiCad, and write down what you found.

## Testing geometry

Geometry tests assert on measurable properties — bounding box, volume, body
count, expected voids — not on file bytes: STEP output carries unstable
metadata. See `tests/tests/enclosure_geometry.rs`.

Tests must not require a running KiCad. The board reader works on saved
`.kicad_pcb` files precisely so that the geometry half stays testable.

## Before opening a pull request

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
