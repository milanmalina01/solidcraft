---
name: solidcraft-electronics
description: Use when making electronic components in SolidCraft through its MCP server - a part's symbol pins, footprint pads, pin-to-pad map, silkscreen, fab and courtyard graphics and 3D package - or when a component is not ready to place. Read reference.md for the worked example.
---

# SolidCraft electronics: electronic components

An electronic component (a resistor, a chip, a connector) lives in the design and owns its symbol, its footprint and a link to its 3D package. Every change is an `ecad.*` command: run them with `execute` or `batch`, and read them back as data with `ecad.component.inspect`; you never need a screenshot.

## Conventions
- Lengths in millimetres; x right, y up, seen from the top of the board; angles in degrees, counter-clockwise.
- A pin's `at` is where a wire connects and `direction` points toward the body. Keep pin ends on the 2.54 mm grid.
- A pad's `at` is its centre, `size` is `[w, h]` before `rotation`. A pin connects to the pad with its number unless `ecad.component.map_pins` says otherwise.
- The 3D package is an ordinary SolidCraft component; its origin goes on the footprint's origin, and the board's top is z = 0.

## The fast way
1. `ecad.component.create {"name":"R0603","prefix":"R","value":"10k"}`
2. `ecad.footprint.generate {"component":"R0603","package":"chip","size":"0603"}`: chip, sot23, soic, qfn, dip or header. It makes the pads, the fab outline, a pin-1 mark and the courtyard.
3. `ecad.symbol.generate {"component":"R0603","style":"resistor"}`: resistor, capacitor, or box with a pin list.
4. `ecad.component.inspect {"component":"R0603"}`: check `ready`, `problems`, `measured`.

Batch steps can't read earlier replies, so name components and pins yourself and refer to them by name and number.

## By hand
- `ecad.pin.add` and `ecad.pad.add` take lists; a `row` `{count, pitch}` repeats one item, numbering on from its number. `replace: true` rebuilds the list.
- `ecad.pin.edit`, `ecad.pad.edit`, `ecad.pin.delete`, `ecad.pad.delete` change one item or several by number.
- Pads: `shape` rect, round, oval or rounded_rect; `kind` smd, through_hole (give a `drill`) or hole.
- `ecad.symbol.set_graphics` and `ecad.footprint.set_graphics` (per `layer`: silkscreen, fab, courtyard) replace one set; `ecad.footprint.courtyard` draws the courtyard round the pads and the fab outline.
- `ecad.package.set {"component":…,"model":…}` links a SolidCraft component (built with solid commands, or the one `file.insert_step` makes).

## Reading problems
`ready` is true with pins, pads and no errors. Fix errors first:
- `pads_overlap`: move pads apart; `measured.min_pad_gap` shows the closest pair.
- `pin_unconnected`: add a pad with the pin's number, or map it.
- `pad_outside_courtyard`: run `ecad.footprint.courtyard` again.
- `package_missing`: set another package, or clear it.
Warnings (`courtyard_missing`, `pad_unconnected`, `no_pins`, `no_pads`) don't block placing.

## Traps
- `component` means the electronic component in `ecad.*` commands; `model` is the SolidCraft one.
- Copied numbers must come from public package drawings (JEDEC, EIA), never from another tool's library.
- A refused call changes nothing and names the item: `pads[2]: …`.
