# rectangular-board

50 x 30 mm board with four M3 mounting holes.

With `pcb_clearance_xy = 1.0` and `wall = 2.0`, the enclosure exterior is
56 x 36 mm.

```
kicase init     --board examples/rectangular-board/rectangular-board.kicad_pcb
kicase rebuild  --board examples/rectangular-board/rectangular-board.kicad_pcb --stl
```
