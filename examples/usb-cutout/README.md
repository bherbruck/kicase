# usb-cutout

The rectangular board plus:

* a datum line on `Enclosure.Datums` running along the front edge;
* a 9.2 x 3.6 mm rectangle on `Enclosure.Cuts` drawn 1 mm beside that datum,
  which becomes the USB-C opening in the front wall;
* a 5 mm circle on `Enclosure.Cuts` with no datum, which becomes an LED window
  through the top.

`enclosure.toml` is what ties the rectangle to the datum:

```toml
[[datum]]
id = "front"
graphic_uuid = "<uuid of the datum line>"
z_origin = "pcb_top"

[[feature]]
id = "usb"
graphic_uuid = "<uuid of the rectangle>"
kind = "cutout"
datum = "front"
clearance = 0.3
```
