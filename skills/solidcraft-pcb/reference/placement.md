# PCB layout: placement

Contents: [Floorplan](#floorplan) · [Grouping](#grouping) · [Zones](#zones) · [Connectors](#connectors) · [Edges](#edges) · [Flex](#flex) · [Holes](#holes) · [Spacing](#spacing) · [Sides](#sides) · [Thermal](#thermal) · [Rework and height](#rework-and-height) · [Checks that point here](#checks-that-point-here)

Placement decides most of what routing can achieve: a critical loop that placement makes long cannot be repaired by routing. Decoupling, crystal, switcher and ESD placement rules are owned by `power-ground.md`, `signal-integrity.md` and `rf-emc.md`. This file covers how to order the work and the mechanical and assembly limits.

## Floorplan

Place in constraint order, because each step fixes what the next can do:
1. Parts the enclosure positions: the outline, mounting holes, connectors, switches, LEDs, display, antenna, and height zones. Take coordinates from the mechanical data and keep each part there.
2. The power entry, with its protection and filter chain at its connector.
3. The anchor IC of each critical block: processor, radio, regulators.
4. Each anchor's satellites, at the pins they serve: decoupling, crystal, termination, feedback, TVS.
5. General passives.

A board with no enclosure has few fixed parts. Start with the connectors anyway. Parts cannot be locked yet: write down the fixed ones and leave them where they are.

## Grouping

- Keep each functional block (one schematic sheet or circuit) in one contiguous area, and arrange the blocks so signals flow one way, from input connector to output connector. Compact blocks keep each block's return currents in its own patch of plane (TI SCAA082A; LearnEMC).
- Do not interleave blocks. A part more than 3 × the group's median nearest-neighbour spacing from its group's hull has strayed. Ratsnest crossings and HPWL are hints only: confirm a placement by routing its critical connections.
- Move a block as one with `ecad.board.move {"items":["U3","L1","C24","C25"],"by":[dx,dy]}`. Track ends on their pads follow; put the tracks' ids in `items` too to move the routing whole.

## Zones

- Put the fastest and noisiest circuits (processor, clocks, switchers, motor drivers) near the power entry and away from I/O connectors and analog. Put sensitive analog farthest away (TI SZZA009). Distance is the cheapest isolation.
- LearnEMC suggests about 2 cm between non-I/O circuitry and I/O, and says the right distance "depends on the situation". Scale it to the board: min(20 mm, a quarter of the shorter side), and treat it as a hint. An MCU whose own pins are the interface, such as an integrated USB PHY, is the interface, not a noise source.
- Mixed-signal boards: one continuous ground, with analog and digital areas partitioned by placement. Converters sit on the boundary with their analog pins facing the analog side (`power-ground.md#mixed-signal`).

## Connectors

- Group cable connectors on one edge or one corner. Plane voltage between two cable attachment points drives the cables as an antenna. The rule is owned by `rf-emc.md#cables`.
- Place each interface IC (transceiver, PHY, level shifter) close to its connector, with no unrelated tracks or parts in the corridor between them. Bring all of one interface out through one connector (LearnEMC).
- Put line filters (common-mode chokes, ferrites, filter capacitors) where the line crosses the board edge, laid out as a straight chain with the unfiltered and filtered sides apart. Copper of the two sides running side by side bypasses the filter (Hubing 2019).
- Connectors that see mating force (USB-C, barrel jacks) need their anchor or retention pads soldered, and no vias in those pads. Pads lifting off SMD connectors is a common field failure.

## Edges

- Keep courtyards and pads back from the outline by the assembler's handling margin: 2.5 mm without rails (JLCPCB), about 3.5 mm for conveyor handlers (UNCC DFT notes), or 0.5 mm when the board is assembled in a panel with rails. The profile holds the value.
- Copper-to-edge is a fab rule: 0.2–0.3 mm on routed edges and 0.4–0.45 mm on V-scored edges (`routing-dfm.md#copper`).
- Edge-mount parts (edge connectors, right-angle connectors) overhang on purpose: check their overhang against the datasheet, and approve their `footprint_outside` finding with that reason.

## Flex

- Keep MLCCs and other brittle ceramic bodies at least about 5 mm from V-score lines, breakout tabs, screw holes, standoffs, board corners and high-insertion-force connectors (Gormally et al. 2007; Knowles).
- Bending cracks the ceramic at its terminations. The crack often passes test and later fails short.
- Near a score line, turn the part's long axis parallel to the line and prefer a small case. A routed relief slot along a break line cuts the stress by about half (Knowles).
- 1206 and larger parts, and capacitors on battery or mains nets, are errors in the flex zone, because a short there is hazardous.

## Holes

- Size each mounting hole's keepout from the real hardware: head or washer plus about 0.5 mm, and room for the tool. ISO 7089 washer outside diameters are 5 mm (M2), 6 mm (M2.5) and 7 mm (M3).
- Inside the keepout: no courtyards, and no exposed copper of other nets. Tightening flexes the board, so the flex zone above applies around holes too.
- A hole used as a chassis or ESD bond belongs next to the connectors and their TVS ground (`rf-emc.md#esd`).

## Spacing

Never overlap courtyards on one side (`courtyard_overlap`). Keep the assembler's part-to-part minimum by package class:

| Package class | Minimum spacing (Eurocircuits) |
|---|---|
| Default | 0.25 mm |
| Smaller than 0603 | 0.15 mm |
| Connectors, cans, crystals | 0.5 mm, plus mating space |
| BGAs | 1.0 mm |

IPC-7351 courtyard excess by density level is reported as 0.5, 0.25 and about 0.1 mm for levels A, B and C. Those figures come from secondary summaries and need verifying. Opposite-side overlaps are allowed unless the process forbids them, for example under a bottom-side BGA.

## Sides

- Put heavy parts, tall parts, BGAs, fine-pitch parts, through-hole parts and heat-sensitive parts on the primary side.
- Parts on the second side must hold by surface tension through the second reflow: mass divided by pad area at most 30 g/in², about 0.047 g/mm² (Peo 1998; Liu, Geiger and Shangguan 2006 found no lead-free difference). Heavier parts need glue.
- Every second-side part sees one more reflow. MSL-rated parts are qualified for three (NXP AN10365).
- On the LoRa node example the bottom side holds the charger, buck-boost, flash, IMU and the pogo-pin debug pads. All are light SMD and within the limit.

## Thermal

- Spread heat sources rather than clustering them, and give each hot part copper for its rise. Rule of thumb: a two-sided board with solid copper needs about 15.3 cm² per watt for a 40 °C rise in still air. Airflow halves that (TI SNVA419).
- Keep temperature-sensitive parts out of hot parts' copper and plumes. That covers electrolytics (life doubles per 10 °C cooler), crystals, references, sensors and batteries. Keep them 10 mm × √P[W] away.
- A temperature sensor measures the copper it sits on. For ambient temperature, put it at an edge on a slotted island. For a part's temperature, put it opposite that part with vias (TI SNOA967A).
- Exposed-pad vias and heat spreading are owned by `power-ground.md#thermal-pads`.

## Rework and height

- Leave room around BGAs and large QFNs for rework. Keep plastic connectors, electrolytics and crystals out of the first 3 mm. Neighbouring BGAs within about 10 mm see secondary reflow (IPC APEX 2023 abstract).
- A tall part hides small neighbours from the AOI camera. Keep the gap at least the height difference for parts of 0603 and smaller, unless the assembler uses 3D AOI.
- Wave or selective soldering only: about 2 mm clear on three sides of a miniwave pin and 5 mm on the side the nozzle leaves (Ersa). No leaded SMD finer than about 0.6 mm pitch on the wave side (NXP).

## Checks that point here

The DRC codes `courtyard_overlap`, `footprint_outside`, `hole_outside`, and the board check's completeness gate (every footprint on the board).

No check covers the floorplan, grouping, zones, connectors, edge margins, flex zones, hole keepouts, sides, thermal placement or rework yet: review those against this file.
