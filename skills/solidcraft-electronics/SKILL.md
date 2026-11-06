---
name: solidcraft-electronics
description: Use when making electronic components in SolidCraft through its MCP server - library symbols, footprints and 3D packages, the components that use them, pin-to-pad maps and graphics - or when a component is not ready to place. Read reference.md for the commands and the worked example.
---

# SolidCraft electronics: the library and electronic components

The library holds separate, shared items: symbols (pins, body), footprints (pads; silkscreen, fab, courtyard graphics) and 3D packages (a SolidCraft component placed on a footprint). An electronic component names one symbol and one or more variants, each a footprint with its own pin-to-pad map and optional 3D package; the first is the default. One footprint (SOT-23) serves many components. Every change is an `ecad.*` command (`execute` or `batch`); read results as data with `ecad.library.inspect`, `ecad.symbol.inspect`, `ecad.footprint.inspect` and `ecad.component.inspect`, never a screenshot.

## Conventions
- Millimetres; x right, y up, seen from the board's top; degrees counter-clockwise.
- Items are named by id or name. Ids share one space; names are unique within a kind.
- A pin's `at` is where a wire connects, `direction` points toward the body; keep pin ends on the 2.54 mm grid.
- A pad's `at` is its centre, `size` `[w, h]` before `rotation`. A pin connects to the pad with its number unless the variant's map says otherwise.
- A 3D package's origin goes on the footprint's origin; the board's top is z = 0.

## The fast way
1. `ecad.footprint.generate {"footprint":"R0603","package":"chip","size":"0603"}`: a new name makes the footprint (chip, sot23, soic, qfn, dip, header), with pads, fab outline, pin-1 mark and courtyard.
2. `ecad.symbol.generate {"symbol":"Resistor","style":"resistor"}`: a standard shape (passives, diodes, LEDs, crystals, switches, transistors; pins in reference.md) or `box` with a pin list.
3. `ecad.component.create {"name":"R0603-10k","prefix":"R","value":"10k","symbol":"Resistor","footprint":"R0603"}`: the footprint becomes the first variant.
4. `ecad.component.inspect {"component":"R0603-10k"}`: check `ready`, `problems`, and each variant's `problems` and `connections`.

Batch steps can't read earlier replies: name items yourself.

## Reading problems
A variant is `ready` with the symbol's pins, the footprint's pads and no errors; a component when every variant is. Fix errors first:
- `pads_overlap`: move pads apart; `ecad.footprint.inspect` gives `min_pad_gap`.
- `pin_unconnected`: add a pad with the pin's number, or map it (`ecad.component.map_pins`).
- `pad_outside_courtyard`: run `ecad.footprint.courtyard` again.
- `package_missing`: give the package another `model` (`ecad.package.edit`), or take it off the variant.
Warnings (`courtyard_missing`, `pad_unconnected`, `no_pins`, `no_pads`, `no_symbol`, `no_footprint`) don't block placing.

## Traps
- `component` is the electronic component in `ecad.*` commands; `model` is the SolidCraft one.
- Editing a shared symbol or footprint changes every component using it (pin maps and schematic wires follow): copy it to change one.
- A shape numbers pins as the usual part does (a diode's cathode 1, a transistor's base 1); for another pinout give `pins` by role: `{"number":2,"name":"K"}`.
- Deleting an item in use is refused unless `force: true` clears the references.
- Package numbers come from public drawings (JEDEC, EIA), never another tool's library; from a datasheet: the solidcraft-datasheet skill.
- A refused call changes nothing and names the item: `pads[2]: …`.
