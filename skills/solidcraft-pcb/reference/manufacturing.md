# PCB layout: manufacturing

Contents: [Profiles](#profiles) · [Choosing one](#choosing-one) · [Class report](#class-report) · [Stencil](#stencil) · [Panels](#panels) · [Finish](#finish) · [Release](#release) · [Checks that point here](#checks-that-point-here)

## Profiles

A fab profile holds a fab class's numbers, each with its source, and sets the DRC minimums in one step: `ecad.board.profile {"fab":"standard"}`. Choose it before routing. The smallest feature on the board decides its price class, and finding out at order time that one neck forces a finer class is expensive. `ecad.board.profile.inspect` lists every value and where it came from; `ecad.board.rules` can still set any rule to another value, and `fab_class` then says what the profile cannot build.

| Value | What it means | `standard` | `fine` | `hdi` |
|---|---|---|---|---|
| `track`, `gap` | Least width and gap | 0.15 / 0.15 mm | 0.10 / 0.10 mm | 0.09 / 0.09 mm |
| `drill` | Least mechanical drill (finished) | 0.3 mm | 0.2 mm | 0.15 mm |
| `ring` | Least annular ring | 0.15 mm on the finished hole | 0.10 mm on the drill tool | 0.10 mm on the drill tool |
| `plating_allowance` | Drill tool = finished hole + this | 0.10 mm | 0.10 mm | 0.10 mm |
| `aspect_ratio` | Board thickness over hole, at most | 8 | 8 | 8 |
| `hole_to_hole` | Least gap between holes | 0.25 mm | 0.25 mm | 0.25 mm |
| `copper_to_edge` | Copper back from a routed edge | 0.3 mm | 0.25 mm | 0.25 mm |
| `drill_to_copper`, `drill_to_copper_pth` | Inner-layer copper back from via holes, plated through holes | 0.2, 0.3 mm | 0.2, 0.2 mm | 0.2, 0.2 mm |
| `silk_to_pad` | Legend back from pads | 0.15 mm | 0.10 mm | 0.10 mm |
| `via_to_pad` | Open via ring back from an SMD pad (`verify`) | 0.25 mm | 0.2 mm | 0.15 mm |
| `pad_entry_angle`, `acute_angle` | Most a track leaves a pad off square; least pocket angle (`verify`) | 30°, 90° | 30°, 90° | 30°, 90° |
| `microvia_drill`, `microvia_aspect`, `microvia_depth` | Laser micro vias between neighbouring layers | none | none | 0.10 mm, 1:1, 0.25 mm deep |

The figures come from the fabs' published pages (Eurocircuits pattern and drill classes, JLCPCB capabilities) and record the month they were read. Fabs change them, so check the fab's own page before ordering. Micro via limits come from public summaries of IPC-6012, and the via-to-pad gap and the two angles from public summaries of fab DFM guides; they are marked `verify`: findings from them are warnings at most.

What the profiles do not hold yet, from the same fabs' pages:

| What | Example values |
|---|---|
| Rings by copper weight | Eurocircuits class 6: 35 µm copper ≥ 0.125 mm, 70 µm ≥ 0.20, 105 µm ≥ 0.25 |
| V-scored edges | Copper 0.4–0.45 mm back |
| Mask web by colour, opening to copper | Eurocircuits LDI 0.075 mm, white 0.13 mm, opening 0.06/0.09 mm. JLCPCB green 0.10 mm, black and white 0.13 mm |
| Legend stroke and height | Eurocircuits 0.10 / 1.0 mm. JLCPCB 0.15 / 1.0 mm. PCBWay 0.15 / 0.8 mm |
| Via protection offered (IPC-4761) | Tenting only with resin fill (Eurocircuits). Plugging up to 0.5 mm, filled and capped 0.15–0.55 mm (JLCPCB) |
| Controlled-impedance tolerance | About ±10 % with coupons |
| Assembly handling | 2.5 mm body to edge without rails; spacing table in `placement.md#spacing`; fiducial rules in `silkscreen-assembly.md#fiducials` |

## Choosing one

- **Order the board where it will be built,** and pick the profile whose numbers that fab builds without a price step.
- **For a first prototype with no fab chosen,** use `standard`: 0.15/0.15 mm track and gap, 0.3 mm drill, 0.15 mm ring on the hole, which most fabs build without a price step.
- **Use `fine` or `hdi` only where a part forces them** (0.5 mm pitch escape, laser micro vias), and keep the count of features at the minimum small.

## Class report

`fab_class` reports what the board needs of the fab: rules finer than the profile builds (a warning), and the features right at its minimums. It names the features that force it. When more than 5 features sit at the absolute minimum, relax them or accept the class knowingly.

## Stencil

- **Paste opening area divided by (opening perimeter × stencil thickness) at least 0.66**, or the paste does not release (IPC-7525).
- **Divide exposed pads into windowpanes covering 50–80 %**, so the part floats level and voids stay small. Reduce openings for fine pitch.
- **A via in a pad needs filling and capping** (`routing-dfm.md#vias`), or the paste drains away.

## Panels

- Assemblers handle small boards in panels with rails.
- **Keep breakaway tabs and mouse bites at least 5 mm from MLCCs** (`placement.md#flex`).
- **V-score only straight outlines.**
- **Put panel fiducials and tooling holes on the rails.** Ask the assembler for its minimum board size before deciding to panelise.

## Finish

- **ENIG or OSP for 0.5 mm pitch and finer, QFNs and BGAs:** they are flat.
- **HASL's uneven surface** causes bridging and opens on fine pitch.
- **Hard gold for edge fingers and sliding contacts.**

## Release

1. **Run the release gate:** `ecad.board.check {"stage":"release"}`, then a fresh reviewer runs `review.md`. At release every warning of the connectivity, fab and critical gates counts as a failure.
2. **Export what exists now:**
   - the drawing set with `ecad.export.pages {"pages":["board_top","board_bottom","inner_layers","assembly_top","assembly_bottom","stackup"],"format":"pdf","path":"board.pdf"}`: copper by layer, the assembly drawing and the stack-up
   - the parts list with `ecad.export.pages {"format":"csv","path":"bom.csv"}`
   - the netlist with `ecad.netlist.export`
3. **Fabrication files come later.** SolidCraft writes no Gerber, drill or placement files yet, and nothing yet checks such files against the board. When they come, the package is:
   - Gerber with X2 attributes: copper, mask, legend and paste for each side, and the outline
   - Excellon drill files, with plated and unplated holes separate
   - one origin and one unit system
   - a stack-up drawing with impedance notes
   - BOM (designator, quantity, description, manufacturer, part number)
   - placement file
   - assembly drawing PDF

   Verify the files themselves, not just the board: rebuild the copper from them and compare the netlist with the schematic. Passing the DRC proves the rules were followed, not that the files are right.
4. **Report what was not verified:** circuit function, safety and regulatory compliance, thermal margins without data, the antenna in its enclosure, receiver desense, and the board check's `not_verified` list.

## Checks that point here

The DRC codes `profile_missing`, `fab_class`, `drill`, `hole_to_hole`.

No check covers drill counts, copper balance, the stencil, panels, the finish or the output files yet: review those against this file.
