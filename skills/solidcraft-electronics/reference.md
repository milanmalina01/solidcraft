# Electronic components: reference

The worked example builds two parts an agent often needs: an 0603 chip resistor with its 3D body, and a SOT-23 transistor. The library items come first, then the components that use them. Then it checks each against numbers worked out by hand. CI runs this exact example (`electronics_skill_example_builds_as_documented`), so it stays true.

## Commands

- Library items: `ecad.symbol.create|edit|copy|delete|inspect {"symbol":…}`, `ecad.footprint.create|edit|copy|delete|inspect {"footprint":…}`, `ecad.package.create|edit|delete {"package":…}`; `ecad.library.inspect` lists all with who uses them. `create` takes `component` (and `variant` for a footprint or package) to give the new item to a component in the same call; so do both generators.
- Pins and pads: `ecad.pin.add|edit|delete {"symbol":…}`, `ecad.pad.add|edit|delete {"footprint":…}`. Lists; a `row` `{count, pitch}` repeats one item, numbering on from its number; `replace: true` rebuilds the list. Pads: `shape` rect, round, oval or rounded_rect; `kind` smd, through_hole (give a `drill`) or hole.
- Graphics: `ecad.symbol.set_graphics {"symbol":…}` and `ecad.footprint.set_graphics {"footprint":…,"layer":…}` replace one set; `ecad.draw.line|rect|circle|text` add one shape to a `symbol`, or to a `footprint` with its `layer`; `ecad.footprint.courtyard {"footprint":…}` draws the courtyard round the pads and fab outline.
- Components: `ecad.component.create {name, prefix?, …, symbol?, footprint?, package?}`, `ecad.component.edit` (`symbol: null` clears it), `ecad.component.variant.add|edit|delete {"component":…}`, `ecad.component.map_pins {"component":…,"variant"?:…,"map"|"by_number"}`.
- A 3D package's `model` is a SolidCraft component built with solid commands, or the one `file.insert_step` makes.
- On the schematic a part is built with its component's first variant unless `ecad.schematic.edit` gives it a `variant`.

## Symbols

`ecad.symbol.generate {symbol, style}` draws a whole symbol, replacing its pins and body, with every pin end on the 2.54 mm grid. `box` takes `pins` `[{number, name?, electrical?, side?}]` for an IC (`side` left, right, top or bottom; left out, the first half go left and the rest right). The other styles are standard shapes. Each pin has a role, the name the shape gives it, and the number it usually has:

| `style` | Pins: role (number) |
|---|---|
| `resistor`, `capacitor`, `inductor`, `ferrite`, `fuse` | 1 left, 2 right |
| `polarized_capacitor`, `battery` | + (1) left, - (2) right |
| `diode`, `zener`, `schottky`, `tvs` | A (2) left, K (1) right; a `tvs` with `bidirectional: true`: 1 left, 2 right |
| `led` | A (2) left, K (1) right; `count` 2 to 4 LEDs share a `common` anode (A 1, cathodes K1… from 2) or cathode (K 1, anodes A1… from 2) |
| `crystal` | 1 left, 2 right; with `count: 4`, 1 left, 3 right and the case pins 2 and 4 below |
| `resonator` | 1 left, 3 right, ground 2 below |
| `switch` | 1 left, 2 right; with `count: 4`, 1 and 2 joined on the left, 3 and 4 on the right |
| `antenna`, `test_point` | 1 below |
| `npn`, `pnp` | B (1) left, C (3) up, E (2) down |
| `nmos`, `pmos` | G (1) left, D (3) up, S (2) down |

A shape's `pins` gives one entry for each of its pins: an entry whose `name` is a role (`A`, `K`, `+`, `B`, `G`…) takes that pin, the others the rest in `pin_list` order. A diode whose footprint has the anode on pad 1 is `{"symbol":"D","style":"diode","pins":[{"number":1,"name":"A"},{"number":2,"name":"K"}]}`. The reply's `pin_list` gives each pin's number, name, role and `at`. A shape keeps the names but doesn't draw them on its pins, except several LEDs in one symbol, which print a name that is not a role (`K_RED`) beside its LED.

Generating again over a symbol that parts use keeps the schematic's wires: a wire on a pin that moved follows it with a short jog, and a pin whose number goes follows its name, or `renames` `{old: new}`. `ecad.component.edit` with another `symbol` (a shape in place of a box) keeps them the same way. Both replies list in `netlist_changed` the nets that gained or lost a pin; expect none.

## Worked example

One `batch`. Each step names what it works on, because a step can't read the replies before it.

```json
{"commands":[
 {"command":"ecad.footprint.generate","params":{"footprint":"R0603","package":"chip","size":"0603"}},
 {"command":"ecad.symbol.generate","params":{"symbol":"Resistor","style":"resistor"}},
 {"command":"component.create","params":{"name":"R0603 body"}},
 {"command":"solid.box","params":{"length":1.6,"width":0.8,"height":0.45,"corner":[-0.8,-0.4,0]}},
 {"command":"component.activate","params":{"component":0}},
 {"command":"ecad.package.create","params":{"name":"R0603 body","model":"R0603 body"}},
 {"command":"ecad.component.create","params":{"name":"R0603-10k","prefix":"R","value":"10k","description":"Chip resistor, 0603","symbol":"Resistor","footprint":"R0603","package":"R0603 body"}},
 {"command":"ecad.footprint.generate","params":{"footprint":"SOT-23","package":"sot23"}},
 {"command":"ecad.symbol.generate","params":{"symbol":"NPN","style":"npn"}},
 {"command":"ecad.component.create","params":{"name":"MMBT3904","prefix":"Q","value":"MMBT3904","description":"NPN transistor, SOT-23","symbol":"NPN","footprint":"SOT-23"}}
]}
```

`ecad.library.inspect` then lists two symbols, two footprints and one 3D package, each with the component that uses it.

## What to check

R0603 footprint, with `ecad.footprint.inspect {"footprint":"R0603"}` (EIA 0603: body 1.6 × 0.8 mm, terminals 0.3 mm; pads reach 0.3 mm past the tips and 0.05 mm past the sides):
- `pads`: 1 at [-0.8, 0] and 2 at [0.8, 0], each 0.6 × 0.9 mm.
- `min_pad_gap.gap`: 1.0 mm (the pads' inner edges at ±0.5).
- `courtyard`: [-1.35, -0.7] to [1.35, 0.7], area 3.78 mm² (copper ±1.1 × ±0.45, plus 0.25).

R0603-10k, with `ecad.component.inspect {"component":"R0603-10k"}`:
- `variants[0]`: named `R0603`; `package.bbox` [-0.8, -0.4, 0] to [0.8, 0.4, 0.45], `package.height` 0.45.
- `ready`: true, `problems`: [] and `variants[0].problems`: [].

SOT-23 footprint (JEDEC TO-236: leads 0.95 mm either side of the middle, span 2.4 mm, feet 0.45 mm, 0.4 mm wide):
- `pads`: 1 at [-0.95, -0.975], 2 at [0.95, -0.975], 3 at [0, 0.975], each 0.5 × 1.05 mm.
- `min_pad_gap.gap`: 1.006 mm, between pads 1 and 3 (0.45 across and 0.9 up from corner to corner).

NPN symbol, with `ecad.symbol.inspect {"symbol":"NPN"}` (base two grid steps left of the origin, collector and emitter one step right and three up or down):
- `pins`: 1 B at [-5.08, 0], 2 E at [2.54, -7.62], 3 C at [2.54, 7.62].

MMBT3904: `variants[0].connections` pin 1 to pad 1, 2 to 2, 3 to 3 (SOT-23 numbers base, emitter, collector as the shape does); `ready`: true. Another NPN in SOT-23 reuses both items: `ecad.component.create {"name":"BC847","prefix":"Q","symbol":"NPN","footprint":"SOT-23"}`.

If a check fails, the reply says why: fix the item with `ecad.pad.edit`, `ecad.pin.edit` or another generator run, and inspect again.

## Making a part by hand

The generators cover common packages. For anything else, place the items yourself. For example, a 2-pin connector with a mounting hole:

```json
{"commands":[
 {"command":"ecad.component.create","params":{"name":"J2","prefix":"J"}},
 {"command":"ecad.footprint.create","params":{"name":"Header 1x2 MH","component":"J2"}},
 {"command":"ecad.pad.add","params":{"footprint":"Header 1x2 MH","pads":[
   {"number":1,"at":[-1.27,0],"size":[1.7,1.7],"drill":1.0,"row":{"count":2,"pitch":[2.54,0]}},
   {"number":"MH","at":[0,-3],"size":[2.2,2.2],"shape":"round","kind":"hole","drill":2.2}]}},
 {"command":"ecad.symbol.create","params":{"name":"Conn 2","component":"J2"}},
 {"command":"ecad.pin.add","params":{"symbol":"Conn 2","pins":[
   {"number":1,"at":[-5.08,1.27],"direction":"right","row":{"count":2,"pitch":[0,-2.54]}}]}},
 {"command":"ecad.footprint.courtyard","params":{"footprint":"Header 1x2 MH"}}
]}
```

`ecad.footprint.create` with `component` makes the component's first variant, named after the footprint. The hole has no copper, so it needs no pin. A pad that should share a pin, such as a tab, is mapped with `ecad.component.map_pins {"component":"J2","map":{"1":[1],"2":[2]}}`. A second footprint for the same part is another variant: `ecad.component.variant.add {"component":"J2","name":"Right angle","footprint":"…"}`.
