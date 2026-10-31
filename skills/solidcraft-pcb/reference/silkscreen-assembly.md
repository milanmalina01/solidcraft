# PCB layout: silkscreen and assembly

Contents: [Modes](#modes) · [Sizes](#sizes) · [Clipping](#clipping) · [Designators](#designators) · [Polarity](#polarity) · [Assembly drawing](#assembly-drawing) · [Fiducials](#fiducials) · [Orientation](#orientation) · [Test and debug](#test-and-debug) · [Checks that point here](#checks-that-point-here)

Designators are for people: hand assembly, inspection, rework and debugging. Machines place parts from the placement file. A designator that is hidden under its part after assembly, sits closer to a neighbour, or is clipped by the fab is worse than none (Benson, PCD&F 2016). Polarity marks matter more than designators: reversed parts are among the most common assembly faults.

## Modes

Every footprint's designator prints on the silkscreen. Set one height for the board with `ecad.board.names {"height":1.0}` (left null, each is sized to its courtyard, 0.5–1 mm, which is below most fabs' minimum), and hide or move single names with `ecad.board.edit` (`show_name`, `name_at`). A per-board silkscreen mode that places every designator against the rules below comes later; until then, work the ladder by hand.

Choose what the silkscreen carries:

| Choice | Silkscreen carries |
|---|---|
| Full | Every designator. For boards whose assembler or customer asks for it |
| Fit | Designators that fit legally at the fab minimum. Arrays get one range label (R10–R17) and a bracket. The rest are hidden and live on the assembly drawing |
| Minimal | The must-keep set only |
| None | Nothing. Polarity marks go in copper or on the assembly drawing |

The must-keep set stays on silk whatever you choose:
- polarity and pin-1 marks
- connector pin-1 and user-facing labels
- test points
- switches and LEDs
- board name, revision and date code

When designators do not fit, work down this ladder (Benson; Hausherr):
1. Shrink to the fab minimum.
2. Move or rotate within the rules below.
3. Group arrays.
4. Drop small passives' designators.
5. Rely on the assembly drawing.

Never shrink below the fab minimum. `ecad.quality.inspect {"families":["silk"],"summary":true}` counts the names that are too small, clipped or ambiguous; when fewer than about 80 % fit at the minimum, choose Fit.

## Sizes

| Item | Minimum | Source |
|---|---|---|
| Text height | 1.0 mm (IPC-7351 nominal 1.5, maximum 2.5) | Eurocircuits; JLCPCB; Hausherr |
| Stroke | 0.10 mm at Eurocircuits, 0.15 mm at JLCPCB and PCBWay. Thinner strokes are removed | Fab rules |
| Stroke-to-height ratio | 1/10 to 1/4. 10 % of 1.0 mm is 0.10 mm, below a 0.15 mm fab minimum | IPC-7351 via Hausherr |
| Polarity mark | 0.5 mm on silk, 1 mm on the assembly drawing | Hausherr |

The check `silk.size` takes 1.0 mm as the least height, and 0.10 mm as the least stroke on the fine and HDI profiles, 0.15 mm otherwise. The board keeps no stroke width for text yet, so the check takes a tenth of the height. On passives of 0402 and smaller, keep the body outline off the silk and on the documentation layer: it cannot clear the pads and still say anything.

## Clipping

Fabs remove legend that lands on exposed copper, holes and the board edge, so the board you receive can lose half a designator or the polarity bar (Eurocircuits). Keep ink off these, by the profile's distance:

| Keep ink off | Distance |
|---|---|
| Mask openings and exposed copper | 0.10–0.15 mm |
| Holes, and open vias over about 0.4 mm | 0.10 mm |
| Routed edges | 0.25 mm |
| Scored edges | 0.45 mm |
| SMD bodies (Eurocircuits assembly) | 0.2 mm |
| Pads of parts at 0.4 mm pitch or finer (Eurocircuits assembly) | 2 mm radius |

Silk may cross tented or plugged small vias. `silk.clip` simulates the fab and lists every designator or mark that would lose a stroke.

## Designators

- **Same side as its part, next to it, outside the body once placement is final.** The owner must be the nearest body, and the next nearest at least 1.4× farther.
- **Read from the bottom or the right edge:** 0° or 90°. 180° and 270° read upside down.
- **One height for the whole board.** Bottom-side text is mirrored.
- **No values on silk:** they go stale and add clutter.
- **Edit spots with `ecad.board.edit`** (`name_at`, `show_name`) and heights with `ecad.board.names`.

## Polarity

- **Every part that can be placed wrongly gets a visible, unambiguous mark:** diodes, LEDs, polarised capacitors, ICs, keyed connectors, and symmetric packages above all.
- **Place the mark within 1 mm of pin 1 or the cathode, outside the body**, so it can still be checked after placement. Use no identical mark at the other terminal.
- **Prefer a diode symbol or "K" at the cathode to +/− alone.** The same mark means anode on one LED and cathode on another, so say which in the mark itself (Benson; Bittele).
- **Keep footprints at IPC-7351 zero orientation** (pin 1 upper left; cathode or positive pin on the left for two-pin parts), so placement-file rotations stay consistent.
- **Classes:** polarity is practical. It becomes critical on hand-assembled boards (`ecad.intent.set {"board":{"hand_assembly":true}}`) and for pad patterns that fit turned round.

## Assembly drawing

- **The assembly drawing shows every designator centred in a true-size body outline, sized by package:**

  | Package | Height |
  |---|---|
  | 0402, 0201 | 0.5 mm |
  | 0603 | 0.7 mm |
  | 0805 | 1.0 mm |
  | 1206 | 1.2 mm |
  | Others | 1.5 mm |

  These heights come from a repost of Hausherr's table and need verifying.
- **Polarity marks are at least 1 mm**, drawn for the top and the bottom (the bottom as seen from below).
- **Ship it as a PDF with the fabrication files:** `ecad.export.pages {"pages":["assembly_top","assembly_bottom"],"format":"pdf","path":"assembly.pdf"}` prints each side's part outlines and designators, with pad 1 and cathodes marked and parts not fitted struck through. It is how dense boards stay legible for rework and inspection (Benson).

## Fiducials

- **Give each assembled side at least 3 global fiducials** in an asymmetric L, as far apart as possible, so the machine can detect a board loaded rotated.
- **Add 2 local fiducials diagonally at parts of 0.5 mm pitch or finer.**
- **Shape:** a 1.0 mm bare copper dot with a mask opening and a clear zone of at least 2 mm, at least 3.35 mm from the edge (JLCPCB). Keep silk off it.

## Orientation

Orient like parts the same way within a row or group, and polarised parts in one direction per area, so a reversed part stands out. Rotate only in multiples of 90°. This is cosmetic: a part rotated to shorten a critical loop keeps its rotation.

## Test and debug

- **Bed-of-nails test:**
  - pads 0.9–1.0 mm on 2.54 mm pitch (1.27 mm minimum)
  - at least 3.2 mm from the edge
  - 1–2.5 mm from bodies on the probed side
  - spread evenly, with several ground points
  - two diagonal unplated tooling holes (JJS; UNCC)
  - never probe component leads
- **Bring-up boards** need pads on each rail, the clocks, the reset and the programming lines.
- **Crystal and RF nets** get no test pads: they add capacitance and stubs (ST AN2867).
- **Debug:**
  - Bring SWD or JTAG (data, clock, reset, VTref, GND) to a reachable footprint.
  - Keep its own keepouts clear, for example the latch holes of a pogo-pin footprint.
  - Mark pin 1.

## Checks that point here

`silk.size`, `silk.clip`, `silk.designator`, `silk.polarity`, and the DRC code `silk_over_pad`.

No check covers the must-keep set, the assembly drawing, fiducials, orientation or test and debug access yet: review those against this file.
