# PCB layout: power and ground

Contents: [Stack-up](#stack-up) · [Return paths](#return-paths) · [Splits](#splits) · [Stitching](#stitching) · [Pours](#pours) · [Decoupling](#decoupling) · [Current](#current) · [Power vias](#power-vias) · [Switchers](#switchers) · [Sense](#sense) · [Thermal pads](#thermal-pads) · [Mixed signal](#mixed-signal) · [Two-layer boards](#two-layer-boards) · [Power entry](#power-entry) · [Checks that point here](#checks-that-point-here)

Ground is the return conductor of specific current loops, not a sink. For each significant current, know where it returns, and keep the return short, wide and right beside the outgoing path (TI SCAA082A; Ott 2001).

## Stack-up

- **Put every layer that carries fast edges or switching current next to a solid plane, on a thin dielectric.** Below about 0.25 mm is the usual target (Olney AN2011_2). Nothing done later in routing recovers a bad stack-up.
- **The plane-pair regime sets the decoupling rules.**
  - The pair's capacitance is ε0·εr/d per area, and its spreading inductance is about μ0·d per square:

    | Spacing d | Capacitance | Spreading inductance |
    |---|---|---|
    | 1.0 mm | 3.8 pF/cm² | 1.26 nH/sq |
    | 0.1 mm | 38 pF/cm² | 0.13 nH/sq |

  - Close: below about 0.3 mm. Wide: above about 0.5 mm. That covers the usual 1.6 mm 4-layer board with a 1 mm core (Clemson CVEL FAQ; LearnEMC). Treat 0.3–0.5 mm as wide.
  - The decoupling checks pick their rules by the pair's regime and say which one in each finding.
- **4-layer boards:** use signal / GND / power / signal with thin outer dielectrics. Layer 2 is ground. The bottom layer then references the power plane, so decouple that plane where bottom-layer fast signals change layers.
- **6-layer boards:** every signal layer touches a plane, the fastest signals reference ground, adjacent inner signal layers are routed at right angles, and at least one power–ground pair is close (TI SPRAAR7J 3.12).
- **The LoRa node's default 6-layer stack-up** uses 0.292 mm dielectrics, so its +3V3 (inner3) and GND (inner4) pair is close. The pair still gives only about 13 pF/cm², roughly 135 pF over the whole board, so the local capacitors carry the high-frequency decoupling.

## Return paths

- **Above a few hundred kHz, the return current flows in the plane right under the track.** The share within x of the centreline is (2/π)·atan(x/h): 50 % within 1h, 80 % within 3h and 94 % within 10h (Ott; Johnson and Graham).
- **Never route a T1-or-faster, RF or switching track across a gap, slot, split, plane edge or merged antipad in its reference plane.** The detour adds inductance and crosstalk, and the slot radiates. Cutting the ground under a buck converter's switching path cost about 6 dB of emissions margin (TI SNVA638A); a crossing slowed a 300 ps edge to 400 ps (Smith 2008).
- **The check errors only when the centreline crosses a gap**, once per track and plane, with the number of crossings. It reports corridor coverage within ±3h as info, for tracks of 3 mm or more.
  - A hole that holds one via or pad of another net, no larger than its antipad, is no crossing.
  - A track end 1 mm or more off the plane has left it at the plane's edge.
- **Exemptions go by edge-rate tier and role, never by toggle rate.** A 1 kHz GPIO with 2 ns edges is a T1 net (`signal-integrity.md#edge-rates`).
- **Space vias so a row of antipads leaves copper webs in every plane they pierce.** Merged antipads form a slot that cuts return paths like a split (TI SCAA082A).
- **When a fast signal changes reference planes, give its return a path right beside the signal via:**
  - a ground via joining both ground planes, within 5 mm (TI SPRAAR7J: 200 mil), and within 2 mm above about 1 GHz;
  - if the two planes are different nets, a stitching capacitor of 1 µF or less, within the same distance.
  - Vias between layers that share one reference plane need nothing extra.
  - `ecad.board.stitch {"net":"GND","pattern":"layer_change"}` adds the return vias.
- **Reference the fastest signals to ground rather than to a power plane.** If a power plane must serve, decouple it to ground near the driver, the receiver and each layer change.

## Splits

- **Default to one continuous ground plane.** Separate noisy and sensitive circuits by placement, and route each section's signals only over its own section, on every layer (Ott 2001; TI SLYT512).
- **A split helps only when low-frequency current (below about 100 kHz) must stay off a sensitive return**, or for galvanic isolation.
- **If a split is required:**
  - Join the parts at one bridge, and route every crossing signal over that bridge.
  - Keep each power plane over its own ground.
  - Never place a decoupling capacitor between a power plane and an unrelated ground.
  - A crossing needs a stitching capacitor within about 2.5 mm. Capacitors farther than about 12.7 mm do not help (Chen et al. 2002, via Riazi 2007). The return-path check accepts a split crossing with a capacitor between the two planes' nets within that distance.
- **Power-plane islands for different rails are fine**, as long as no fast signal on the next layer crosses the gap between two islands.

## Stitching

- **Tie ground layers and pours together with distributed vias, not a perimeter ring.** Perimeter rows every 2 cm on multi-plane boards are usually unnecessary and can form a resonant cavity (LearnEMC).
- **Stitch densely where it counts:** near switchers, connectors, shield lands, RF lines and layer changes.
- **Pitch:** below λg/10 at the declared frequency of concern for pour grids, and λg/20 for fences along RF lines (`rf-emc.md#fences`). With εr 4.4, λg/10 is about 14 mm at 1 GHz and 5.8 mm at 2.45 GHz.
- **The check measures how weakly the planes are tied.** `ecad.board.ground.inspect` lays each copper layer on a 0.25 mm grid, takes the ground net's copper as a resistive sheet and its vias as plated barrels, draws 10 mA from every mm² of it (1 A/cm²) into the main ground (the largest plane, or the power-entry connector's ground pads when the net has no pour), and gives each spot's rise in mV, the worst spots, each layer's tied, isolated and weak shares and a coarse map. The board view shows the same as the Ground connectivity layer. `pi.stitching` warns when a plane's worst spot is past `tie_mv` (0.5 mV, a judgment default) or plane copper is tied by nothing. Only when that solve can't finish does it fall back to the older test: a via-free circle wider than λg/10 at the frequency of concern where the planes overlap (the frequency from intent: `board.f_concern_mhz`, else the top of the highest radio band; never an assumed 1 GHz). It also reports an outer ground island with no via to an inner plane, and stitching along the edge only (info).
- **The number is a comparison, not a voltage the board will see:** use it to find weak spots and to show a change helped. It is a DC measure; λg/10 spacing for RF cavities still matters on radio boards.
- **`ecad.board.stitch {"net":"GND","pattern":"grid"}`** places a staggered grid at that pitch and skips spots that would break a rule; `dry_run` counts them first.
- **`ecad.board.stitch {"net":"GND","pattern":"needed"}`** places one via at a time at the most weakly tied legal spot and solves again, until the worst spot is within `target` (0.5 mV) or `budget` vias (20) are placed. Spots near RF lines, switching regulators, layer changes and connectors weigh more. Its reply gives the worst spot before and after; `dry_run` shows them without placing.

## Pours

- **Multilayer boards with a solid inner ground:** outer ground pour is optional. If you pour:
  - Delete floating islands and long unstitched fingers.
  - Stitch the pour to the plane at short intervals.
  - Keep pour edges at least 3× the dielectric height from controlled-impedance tracks, or include the coplanar ground in the impedance model. A pour 5 mil beside a 58 Ω microstrip pulled it to 50 Ω on one side and 43 Ω on both (Olney AN2010_4).
- **2-layer boards:** a well-stitched ground pour on the far side is the plane. See [Two-layer boards](#two-layer-boards).
- **`ecad.board.pour`** takes `priority`: higher fills first where pours overlap. `remove_islands` is on by default, and `min_width` drops copper thinner than its value.

## Decoupling

**Place each high-frequency capacitor by the plane-pair regime:**

| Regime | Distance to the pin it serves (warn above) | Topology |
|---|---|---|
| No planes (2-layer) | 3 mm for fast ICs, 5 mm for others | Route the supply into the capacitor pad, then to the pin; the same for ground. The capacitor is in the current path |
| Wide planes (> 0.3–0.5 mm) | 3 mm for fast ICs, 5 mm for others, beside the pin that connects to the farther plane | The capacitor's vias sit next to the IC's same-side via, or share it. Same-side sharing is fine here (LearnEMC) |
| Close planes (< 0.3 mm) | 10 mm, or any split between | The IC pin gets its own short via. Spend effort on low mounting inductance and on spreading capacitors, not on millimetres |
| RF IC, any stack-up | 2 mm, with no via between pin and capacitor | Follow the vendor reference: pin → capacitor → via, with at least 2 ground vias at the capacitor (Silicon Labs) |

**Mounting inductance often outweighs the part.**
- Measured connection inductance ranged from 0.61 to 15.7 nH across six pad and via layouts (Zeeff, UMR TR01-1-030). A 100 nF part with 1.5 nH resonates near 13 MHz.
- Put vias in or right beside the pads, with no track longer than 0.5 mm. Keep the two vias as close as the pads allow.
- Give each capacitor its own vias, and mount it on the side nearer its plane pair.
- Four vias instead of two nearly halve the connection inductance (LearnEMC).
- `ecad.board.loop.inspect {"kind":"decap"}` ranks the loops by estimated nH.

**Values.**
- Use one value (0.1–0.2 µF) in the smallest practical package, several times over, instead of a ladder of 100 n / 10 n / 1 n. At high frequency the same package mounted the same way has the same impedance, and mixed values can form anti-resonances (TI SPRABV2; LearnEMC).
- Add bulk of about 1–10× the total high-frequency capacitance where the rail is made or enters, with at least 2 vias per pad. TI SPRABV2 asks for at least 15 µF per about 10 power pins on BGA processors.

**Spread.** Several capacitors on one IC go on different sides. A second capacitor on the far side of the pin gives about 70 % of one capacitor's inductance, while one placed right beside the first barely helps (Archambeault et al. 2010).

**Ferrite-isolated rails.** For PLL, analog or RF supplies, put the load-side capacitor at the pin and the bead between it and the main rail. Do not bead-isolate high-current digital core supplies fed from planes: it cuts them off from the plane capacitance (TI SPRABV2).

**Assignment.** `ecad.intent.inspect` proposes the capacitor each IC power pin gets. Confirm one per pin, or the datasheet's pin group. Pins of one net side by side (within 0.5 mm) count as one group, and a capacitor placed but left unfitted counts as a site to review, not as decoupling.

## Current

- **Size current-carrying copper from an allowed temperature rise using IPC-2152 data, not the old IPC-2221 charts.**
  - IPC-2221 halves internal traces without test data. IPC-2152 shows internal traces cool about as well as external ones.
  - For wide external traces, IPC-2221 is optimistic: 100 mil of 1 oz copper at 10 °C gives 4.68 A by IPC-2221 and about 3.56 A by the IPC-2152 fit (Brooks and Adam 2015).
  - The fit is ΔT = 215.3·I²·W^-1.15·Th^-1, with W and Th in mil (external, conservative for internal layers too).
  - A plane 10 mil below a 200 mil trace cut its rise from 95 °C to 34 °C at 15 A (Brooks and Adam).
  - `ecad.board.current.inspect` uses IPC-2221 unless asked for the fit: `ecad.board.current.inspect {"current":2,"formula":"ipc2152"}`. The check `pi.current` uses the fit, with the net's `current_a` (or its class's `current`) and the class's `temp_rise`.
  - Where copper runs in parallel, the check splits the current by conductance and judges each piece by the least share it carries.
- **Judge a path by its narrowest copper:** necks at pads, between vias, through relief spokes, across antipad webs and through connector pins.
- **DC drop:** sheet resistance is about 0.49 mΩ per square for 1 oz copper (0.98 for 0.5 oz), plus 0.39 %/°C. 2 A through 50 mm of 0.5 mm-wide 1 oz copper (100 squares) drops about 98 mV before the return. The 2 % per-rail budget is a judgment default: use the load's datasheet tolerance.

## Power vias

- **Size power vias by resistance, drop and inductance.**
  - A 0.3 mm via through 1.6 mm with 25 µm plating has about 1.2 mΩ and 1.3 nH.
  - Use at least 2 per transition above about 0.5 A, placed where current enters and leaves the pads.
  - A via's temperature follows the tracks it joins (Brooks and Adam).
- **Do not use vias-per-amp rules.** Vendor heuristics differ by 5× (one per 200 mA in TI AN-1149, one per amp in TI SLYT614 and SLVA773).

## Switchers

1. **Mark the hot loop from the schematic first.** It is the loop whose current switches in nanoseconds. Buck: input capacitor, high-side switch, low-side switch or diode. Boost: switch, rectifier, output capacitor. Buck-boost: both. Inductor current is nearly DC (Hegarty, TI SLYT682).
2. **Place the smallest input capacitor of the loop first:**
   - on the same layer, right at the IC's VIN and PGND pins;
   - close the loop with short, wide copper and no vias;
   - put the system-plane vias outside the loop;
   - keep a solid ground on the next layer, on the thinnest dielectric.

   The numbers: 2 nH at 5 A/ns gives a 10 V overshoot (Hegarty). Loop inductance of 0.4 nH instead of 2.9 nH gained more than 4 % efficiency at 1 MHz (EPC WP010).
3. **Keep the switch node small, on one layer, and on an intact ground.**
   - Keep other nets' copper out from under the inductor on the mounting layer.
   - Keep feedback, compensation, analog and clock copper away: the check warns below 1 mm on the same layer and errors on overlap across a layer with no ground between.
   - Removing the plane under the switch node made emissions worse (TI SNVA638A).
4. **Feedback.** Put the divider at the FB pin. Take the sense point from the output capacitor (or the load, for remote sensing) through its own track, never from partway along a power track.
5. **Quiet ground.** Gather the controller's quiet returns (FB divider, soft-start, compensation, AGND) and join them to PGND at one point at the IC, usually the exposed pad. This is a local single point, not a split of the board's ground.
6. **Snubbers and input filters.** A snubber needs the tightest loop between SW and PGND. Put an input filter where the supply enters the converter area: on a battery-fed buck it cut radiated EMI by up to 20 dB (TI SNVA638A).
7. **Start from the vendor's EVM or datasheet layout**, and write down every deviation. The check ranks loops by estimated nH (`ecad.board.loop.inspect {"kind":"hot"}`, whose `ground_under` is the share of ground under the loop the check judges) and compares them with the vendor's loop when the loop record gives its `reference_ratio`. It errors only on a via inside the loop, or an input capacitor on the far side; a far-side capacitor the datasheet asks for is approved with that reason.

**Chargers** are switchers with extra high-current paths. Size the input, system and battery paths for the maximum charge plus the system current. A linear charger dissipates most at the lowest battery voltage, about 3.4 V on Li-ion, reached within minutes (TI BQ24075).

## Sense

- **Route current- and voltage-sense lines as Kelvin connections.** Take them from inside the sense resistor's pads, or from the battery terminal pad, and run them as a pair to the IC with their filter capacitors at the IC.
- **Share no high-current copper with the sense path.** A 1 mΩ shunt at 20 A gives 20 mV (TI SLVA959B), so 0.1 mΩ of shared copper is a 10 % error.

## Thermal pads

- **Solder exposed pads to a matching land with a via array into inner and bottom copper.**
  - Vias of about 0.3 mm drill (at most 0.33 mm) at 1.0–1.5 mm pitch, connected solid, never through reliefs.
  - Tent or plug them from the component side if they are not filled.
  - 5–9 vias usually suffice for a small die (TI SLMA002H).
  - One 12 mil via is about 261 °C/W, so only an array helps (TI SNVA419C).
  - Keep the area opposite the pad for copper or a heatsink, not other parts.
- **Treat this as critical** when the datasheet's θJA assumes vias or the part dissipates more than about 0.5 W. No check covers exposed pads yet: review them.
- **Keep spreading copper continuous around hot parts.** A slot across the heat flow added 5.5 °C, against 1.5 °C for one along it (TI SNVA419C).

## Mixed signal

- **Partition one ground** (see [Splits](#splits)). Put data converters on the analog/digital boundary.
- **Tie a converter's AGND and DGND pins together at the package, to the quiet ground**, and decouple its digital supply straight to its DGND pin: an ADC needed its capacitor within 2.5 mm to avoid missing codes (Bryant, ADI). The pin names describe currents inside the chip. Large DSP or codec parts with over 100 mA of digital transients are the exception.
- **Analog front ends:**
  - Keep high-impedance nodes small.
  - Put the feedback resistor at the inverting pin, and the ADC input RC and reference capacitor at their pins (2 mm).
  - Make differential analog routing symmetric.
  - Use a driven guard or guard ring for pA inputs, and keep flux and humidity off them.
  - Watch thermal-EMF symmetry at precision junctions.

## Two-layer boards

- **Make one side a near-solid ground.** Route the other side, with only short jumpers on the ground side, crossing at right angles. Stitch the ground pour.
- **50 Ω on a 1.6 mm 2-layer board needs a track of about 2.8–3 mm** (TI SLLA414 tables), so use grounded coplanar waveguide for RF. For other nets, report the estimated impedance instead of a target.
- **Decoupling here is local:** capacitors sit in the current path at the pins (table above).

## Power entry

These are review items, in order from the connector:
1. reverse-polarity protection
2. fuse or PTC
3. input TVS
4. inrush limiting
5. bulk capacitance

Hot-plugged inputs need the TVS and the input filter at the connector. Li-ion protection goes at the cell. Gate drivers need short gate loops, a Kelvin source, and the bootstrap capacitor at its pins.

## Checks that point here

`pi.return_path`, `pi.layer_change`, `pi.stitching`, `pi.decap_assigned`, `pi.decap_distance`, `pi.decap_mounting`, `pi.decap_topology`, `pi.current`, `pi.bottleneck`, `pi.smps_hot_loop`, `pi.smps_switch_node`, `pi.smps_feedback`, and the DRC codes `via_stub`, `pour_unknown_net`, `pour_empty`.

No check covers splits and domains, bulk and ferrite-fed rails, IR drop, power vias, exposed pads, sense lines, mixed-signal grounds or gate loops yet: review those against this file.
