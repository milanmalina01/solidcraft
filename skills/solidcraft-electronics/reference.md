# Electronic components: reference

The worked example builds two parts an agent often needs: an 0603 chip resistor with its 3D body, and a SOT-23 transistor. Then it checks each against numbers worked out by hand. CI runs this exact example (`electronics_skill_example_builds_as_documented`), so it stays true.

## Worked example

One `batch`. Each step names what it works on, because a step can't read the replies before it.

```json
{"commands":[
 {"command":"ecad.component.create","params":{"name":"R0603","prefix":"R","value":"10k","description":"Chip resistor, 0603"}},
 {"command":"ecad.footprint.generate","params":{"component":"R0603","package":"chip","size":"0603"}},
 {"command":"ecad.symbol.generate","params":{"component":"R0603","style":"resistor"}},
 {"command":"component.create","params":{"name":"R0603 body"}},
 {"command":"solid.box","params":{"length":1.6,"width":0.8,"height":0.45,"corner":[-0.8,-0.4,0]}},
 {"command":"component.activate","params":{"component":0}},
 {"command":"ecad.package.set","params":{"component":"R0603","model":"R0603 body"}},
 {"command":"ecad.component.create","params":{"name":"MMBT3904","prefix":"Q","value":"MMBT3904","description":"NPN transistor, SOT-23"}},
 {"command":"ecad.footprint.generate","params":{"component":"MMBT3904","package":"sot23"}},
 {"command":"ecad.symbol.generate","params":{"component":"MMBT3904","style":"box","pins":[
   {"number":1,"name":"B","electrical":"input","side":"left"},
   {"number":2,"name":"E","electrical":"passive","side":"bottom"},
   {"number":3,"name":"C","electrical":"passive","side":"top"}]}}
]}
```

## What to check

Read each part back with `ecad.component.inspect`:

R0603 (EIA 0603: body 1.6 × 0.8 mm, terminals 0.3 mm; pads reach 0.3 mm past the tips and 0.05 mm past the sides):
- `pads`: 1 at [-0.8, 0] and 2 at [0.8, 0], each 0.6 × 0.9 mm.
- `measured.min_pad_gap.gap`: 1.0 mm (the pads' inner edges at ±0.5).
- `measured.courtyard`: [-1.35, -0.7] to [1.35, 0.7], area 3.78 mm² (copper ±1.1 × ±0.45, plus 0.25).
- `package.bbox`: [-0.8, -0.4, 0] to [0.8, 0.4, 0.45]; `package.height` 0.45.
- `ready`: true, `problems`: [].

MMBT3904 (JEDEC TO-236: leads 0.95 mm either side of the middle, span 2.4 mm, feet 0.45 mm, 0.4 mm wide):
- `pads`: 1 at [-0.95, -0.975], 2 at [0.95, -0.975], 3 at [0, 0.975], each 0.5 × 1.05 mm.
- `measured.min_pad_gap.gap`: 1.006 mm, between pads 1 and 3 (0.45 across and 0.9 up from corner to corner).
- `connections`: pin 1 to pad 1, 2 to 2, 3 to 3; `ready`: true.

If a check fails, the reply says why: fix the part with `ecad.pad.edit`, `ecad.pin.edit` or another generator run, and inspect again.

## Making a part by hand

The generators cover common packages. For anything else, place the items yourself. For example, a 2-pin connector with a mounting hole:

```json
{"commands":[
 {"command":"ecad.component.create","params":{"name":"J2","prefix":"J"}},
 {"command":"ecad.pad.add","params":{"component":"J2","pads":[
   {"number":1,"at":[-1.27,0],"size":[1.7,1.7],"drill":1.0,"row":{"count":2,"pitch":[2.54,0]}},
   {"number":"MH","at":[0,-3],"size":[2.2,2.2],"shape":"round","kind":"hole","drill":2.2}]}},
 {"command":"ecad.pin.add","params":{"component":"J2","pins":[
   {"number":1,"at":[-5.08,1.27],"direction":"right","row":{"count":2,"pitch":[0,-2.54]}}]}},
 {"command":"ecad.footprint.courtyard","params":{"component":"J2"}}
]}
```

The hole has no copper, so it needs no pin. A pad that should share a pin, such as a tab, is mapped with `ecad.component.map_pins {"component":"J2","map":{"1":[1],"2":[2]}}`.
