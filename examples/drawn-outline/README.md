# drawn-outline

Everything about the wall is drawn, nothing is a setting.

A 40 x 25 mm board, with the enclosure wall drawn on the **Enclosure** layer as
a rounded rectangle:

* the **path** is the centre line of the wall — 50 x 35 mm;
* the **stroke width** is the wall thickness — 2.5 mm;
* the **arcs** are the corner radii — 5 mm.

So the exterior comes out at 52.5 x 37.5 mm and the cavity at 47.5 x 32.5 mm,
and what KiCad draws in 2D is the wall at true size. The `wall` and
`corner_radius` settings in `enclosure.toml` are ignored here; they only fill in
when nothing is drawn on the Enclosure layer.

```sh
kicase init    --board examples/drawn-outline/drawn-outline.kicad_pcb
kicase rebuild --board examples/drawn-outline/drawn-outline.kicad_pcb
```
