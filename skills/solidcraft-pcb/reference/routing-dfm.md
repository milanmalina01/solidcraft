# PCB layout: routing and manufacturability

Contents: [Connectivity](#connectivity) · [Widths and clearances](#widths-and-clearances) · [Voltage spacing](#voltage-spacing) · [Corners](#corners) · [Pad entry](#pad-entry) · [Pad to pour](#pad-to-pour) · [Vias](#vias) · [Copper](#copper) · [Mask](#mask) · [Neat routing](#neat-routing) · [Checks that point here](#checks-that-point-here)

The fab profile holds the minimums the DRC uses (`manufacturing.md#profiles`); the rest here are the fabs' published figures. Design one step looser than the profile where space allows: the smallest feature on the board decides its price class, and a design right at the limit leaves no room for process spread (Eurocircuits).

## Connectivity

- A `short` is two nets' copper touching or joined. Fix it before anything else, because every other finding near it is unreliable.
- A dangling track end is either an unfinished connection or a useless stub. Remove it or finish it. A same-net pour counts as an end. The DRC warns on each one (`dangling_track`).
- Join two nets on purpose only through a part (a net-tie footprint, a shunt, a 0 Ω link): copper that joins them is a `short`.

## Widths and clearances

- **One width per net class.** Change width only at a deliberate neck or taper. Make power tracks as wide as space allows, and size them from their current (`power-ground.md#current`).
- **Necks:** neck a track only where pads force it (fine-pitch and BGA escape). Keep a neck inside the pad field plus 1 mm and shorter than 2 mm, or v·RT/10 when the edge time is known. Return to the class width straight after.
- **Escape widths for BGAs (NXP AN10778):** one 0.15–0.18 mm track between 1.0 mm-pitch lands, 0.10–0.15 mm at 0.8 mm pitch, and 0.125/0.125 mm with staggered vias at 0.65 mm. At 0.5 mm no through via fits between lands.
- **Clearance:** the DRC holds clearance to the larger of the two nets' classes. It flags gaps within 10 % of the limit as "at limit". Crosstalk spacing is a separate, larger rule (`signal-integrity.md#crosstalk`), always measured edge to edge.

## Voltage spacing

The fab minimum means nothing at mains voltage. Size each gap from the peak voltage between the two nets. IPC-2221 Table 6-1 gives functional spacing; the values below come from public summaries and need verifying against the standard:

| Peak voltage | External, uncoated (B2) | Internal (B1) |
|---|---|---|
| 0–30 V | 0.1 mm | 0.05 mm |
| 31–100 V | 0.6 mm | 0.1 mm |
| 101–150 V | 0.6 mm | 0.2 mm |
| 151–300 V | 1.25 mm | 0.2 mm |
| 301–500 V | 2.5 mm | 0.25 mm |
| Above 500 V | + 0.005 mm/V | + 0.0025 mm/V |

- Isolation barriers follow the product safety standard (IEC 62368-1, IEC 60664-1), not IPC-2221. Measure creepage along the surface: a slot lengthens it. Under an isolator no layer may carry copper across the barrier.
- From about 700 V peak (the IEC 60664-1 partial-discharge threshold), round copper corners that face another net with the largest radius that fits (Mukherjee et al.). Below about 330 V no gas discharge can start, so corner shape does not matter there. Footprint pad corners are excluded from the check.

## Corners

- **Use 45° bends or arcs by habit.** They shorten routes, free channels and make a board quicker to review.
- **A 90° corner on an ordinary track is not a defect.**
  - It adds about 2 fF per mil of width.
  - It matters only when the width in mils exceeds 5 × the rise time in ps (Bogatin RoT #24). An 8 mil corner at 100 ps reflects about 0.3 % (Johnson).
  - Measured radiation from a 90° bend equals that of two 45° bends (Wang, Hubing et al. 2000).
  - Rounding the outside corner removes only about 21 % of the excess copper, so a rounded corner is fine only because the corner was negligible anyway.
- **When corners do matter:** RF lines (use the mitre or arc in `rf-emc.md#rf-lines`), multi-Gb/s pairs (keep bends symmetric, interior angle above 135°, `signal-integrity.md#pairs`), and high voltage (above).
- **Never turn more than 90° in one bend.** Never let a track meet another track, a pad or a pour at an acute angle. The narrow etched pocket can leave copper or a ragged tip. Modern etching has reduced the problem but not removed it, and fab DFM still flags it. The fix costs one segment. No check covers acute pockets yet: look for them.

## Pad entry

- **Enter an SMD pad from its outer end, along its long axis, centred, and end the track inside the pad near its centre.** A track that only touches the pad edge is an ambiguous connection.
- **Keep the entering copper no wider than the pad.** At fine pitch, keep it narrower, so the mask web between pads survives.
- **Run straight for about one pad width before the first bend.**
- **Side entry is fine where it shortens a decoupling loop:** vias beside the pads give a smaller loop than vias off the ends (`power-ground.md#decoupling`).
- **BGA dog-bones at 45° are normal.**

## Pad to pour

One table governs every pad connected to a pour or plane. Other rules defer to it.

| Pad | Connection | Why |
|---|---|---|
| Through-hole pin in a plane | Thermal relief with ≥ 2 connected spokes | The plane sinks the soldering heat. A solid pin on two or more planes or heavy pours is hard to solder |
| Exposed pad, thermal vias | Solid | Spokes throttle the heat path (TI SLMA002H) |
| Chip ≤ 0603 on a same-layer pour | Symmetric reliefs on both pads | Unequal heat sinking lifts one end (tombstoning, Nordson EFD) |
| Decoupling capacitor | A via at the pad; a relief on the surface is fine then | Low inductance comes from the via, not from a solid surface pour |
| High-current pin | Solid, or spokes whose total width meets the IPC-2152 width | A relief is a current neck (`pi.current` checks the spoke set) |

- **What a relief costs:** a relief's DC resistance is small. One 0.25 mm wide, 0.3 mm long spoke of 1 oz copper is about 1.2 squares × 0.49 mΩ/sq ≈ 0.6 mΩ, and four in parallel are about 0.15 mΩ. Its real cost is the current-density neck and the thermal throttling.
- **Starved reliefs:** check that reliefs keep their spokes after clearances clip them. A pad with no connected spoke is open (`relief_starved`).
- **Tombstoning on 0402 and smaller:** the check flags only asymmetric copper on the same layer, joined to the pad within 1.5 mm of it, at a ratio above 2:1 (0603 is an info, 0201 and smaller an error). A via at the pad is not heavy sinking.
- **Set how a pour meets a footprint's pads** with `ecad.board.pour.connect {"footprint":"J1","connect":"thermal"}`, or a pour's own default with `connect` on `ecad.board.pour`.

## Vias

- **Ring convention.** The fab profile says whether the ring is measured from the finished hole or from the drill tool (finished + 0.10 mm at Eurocircuits). The same 0.35/0.15 mm via has a 0.10 mm ring on the first rule and a 0.05 mm ring on the second.
- **Aspect ratio.**
  - Through holes: board thickness divided by the hole at most the profile's limit (8 on tool size at Eurocircuits). Deep, narrow holes plate thinly in the middle and crack.
  - Microvias: at most 1:1, and no deeper than 0.25 mm (IPC-6012 via NCAB).
  - Do not use micro-via sizes for through vias.
- **Drill to copper:** on inner layers, keep unconnected copper back from each hole: max(inner ring + 0.075, 0.20) mm from the tool (Eurocircuits), or 0.2 mm for vias and 0.3 mm for PTH at JLCPCB.
- **Via in pad** must be filled and capped (IPC-4761 type VII). An open via in a pad wicks the solder away. Thermal vias under an exposed pad are the exception: drill ≤ 0.3 mm, plugged or tented from the component side (TI SLUA271).
- **Via beside a pad:** keep a mask web between the via and the pad, joined by a short track (dog-bone), or tent or plug the via. A via half over a pad is the worst case.
- **Protection:** give each via an explicit `protection` (open, tented, plugged, filled, filled_capped) that the fab offers, on `ecad.board.via` or `ecad.board.via.edit`. A mask tent alone is not a seal: Eurocircuits tents only with resin fill, and Leiton no longer offers tenting.
- **Teardrops** (no tool or check yet): add them where the ring is near the minimum (within 0.05 mm), on inner layers of Class 2/3 builds and on flex. IPC-A-600 accepts Class 2 inner breakout only with such lands (ZVEI). They are optional elsewhere.
- **Economy:** change layers only when needed. Give both members of a pair the same via count. Keep the number of distinct drill sizes small: 6 or fewer on production boards.

## Copper

- **Slivers:** no copper narrower than the minimum track width, including pour necks, fingers between pads and clearance leftovers. Set the pour's `min_width` to at least the minimum track width.
- **Islands:** remove copper islands that connect to nothing, or tie them down with at least two vias. On multilayer boards with solid planes, signal-layer ground fills add little and shift nearby impedances (Johnson, Ground Fills). On 2-layer boards a well-stitched ground fill helps (`power-ground.md#pours`).
- **Board edge:** keep copper back from the edge: 0.2–0.3 mm on routed edges, 0.4–0.45 mm on V-scored edges, and more for planes. Castellations and edge fingers are declared, not flagged.

## Mask

The board has no solder mask model yet, so no check covers these: review them.

- **Mask web:** keep the web between adjacent pad openings at or above the fab's minimum for the mask colour:
  - Eurocircuits: LDI colours 0.075 mm, white and transparent 0.13 mm.
  - JLCPCB: green 0.10 mm, black and white 0.13 mm.
  - A narrower web is stripped, and the joints bridge.
  - Worked example: 0.5 mm pitch, 0.28 mm pads and 0.05 mm expansion leave a 0.12 mm web. That passes green LDI and fails white.
- **Openings:** a mask opening must not uncover another net's copper. Keep 0.06–0.09 mm to it.

## Neat routing

Neat routing is how a board stays reviewable. Each rule here gives way to an electrical rule.

- **Give each signal layer a preferred direction.** On 2-layer boards and adjacent signal layers without a plane between them, route the layers at right angles: broadside overlap couples more than side-by-side (Olney).
- **Route buses as parallel groups.** Make the bends together, stay on one layer, and keep the pitch at or above the crosstalk spacing of the bus's tier. Crosstalk wins over tight neatness.
- **Avoid shapes that hide errors:**
  - stair-steps and any-angle runs
  - jogs shorter than the track width
  - overlapping segments and zero-length segments
  - detours more than 1.5 × the shortest path
  - unintended loops in one net
- **Chamfer T-junctions on tracks below 0.6 mm** (Jones).
- **Use `ecad.board.track.miter`** to round or chamfer corners after routing. It leaves corners where other copper joins.

## Checks that point here

DRC: `short`, `clearance`, `track_width`, `track_width_class`, `annular_ring`, `copper_to_edge`, `dangling_track`, `via_aspect_ratio`, `drill_to_copper`, `via_in_pad`, `relief_starved`.

Quality: `dfm.pad_to_pour`, `dfm.tombstone`.

No check covers voltage spacing, acute pockets, pad entry, teardrops, slivers, the mask or neat routing yet: review those against this file.
