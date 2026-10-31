# PCB layout: RF, EMC and ESD

Contents: [RF lines](#rf-lines) · [Fences](#fences) · [Matching](#matching) · [Isolation](#isolation) · [RF ICs](#rf-ics) · [Antennas](#antennas) · [Shields](#shields) · [Coexistence](#coexistence) · [ESD](#esd) · [Cables](#cables) · [Emissions](#emissions) · [Myths](#myths) · [Checks that point here](#checks-that-point-here)

Wavelengths in FR-4 (εr 4.4) use λg = c/(f·√εr):

| Frequency | λg | λg/20 |
|---|---|---|
| 868 MHz | 164 mm | 8.2 mm |
| 2.45 GHz | 58 mm | 2.9 mm |

The free-space quarter wave is 86 mm at 868 MHz and 31 mm at 2.4 GHz.

For an integrated radio, copy the vendor's reference layout exactly: part order, spacing, orientation, and the height to ground. At these frequencies the pads and tracks are part of the matching network. If your stack-up differs, recompute the line widths rather than copying copper blindly.

## RF lines

- **Treat any RF track longer than about λg/16 at its highest frequency as a 50 Ω line** (Silicon Labs Z-Wave 800 guide): about 3.5–4 mm at 2.45 GHz.
  - Compute its width with `ecad.board.impedance.inspect`.
  - Put the stack-up and an impedance-control note in the fab notes. Swapping two layers turned a 50 Ω line into 73 Ω (TI AN098).
  - Keep widths of at least 0.2 mm on FR-4, for tolerance (u-blox).
  - The check reads each RF net's `z_target` (50 Ω when left out) and `f_mhz` (else the top of the board's highest band, and its findings say so). It warns 5 % off target and errors 10 % off, or at the net's `z_tol`.
- **Decide microstrip or grounded coplanar on purpose.**
  - Same-layer ground closer than max(2W, 3h) loads the line, so either keep it away or make the gap a designed dimension in the coplanar model (u-blox).
  - Never leave the gap at a generic pour clearance.
  - Another net as the nearest copper beside the line is an error: it couples straight in, even through a series part's two pads.
- **Reference:** keep an unbroken ground directly under every RF line, RF IC and matching part. Route nothing of another net in that layer under the RF path.
  - The check errors on another net's copper there, and on a power plane as the reference. It walks the line point by point to the first plane under it.
  - A void is no error: a void under an oversized connector pad keeps the pad near 50 Ω (u-blox NINA-B3 3.3; TI SPRAAR7J 3.10). A void a track crosses is a return-path finding (`power-ground.md#return-paths`).
- **Bends:** use a constant-width arc with radius at least 3W, or the optimal mitre: cut fraction M = 0.52 + 0.65·e^(−1.35·W/h) of the corner diagonal (Douville and James 1978).
  - Rounding only the outside corner fixes little.
  - The corner capacitance is about 1.7 fF per mil of width (Bogatin SIJ 2021): invisible on a 20 mil line below 25 GHz, noticeable on a 120 mil line at 5 GHz.
- **Width changes:** taper over at least 3× the width step.
- **Short and direct:** keep the RF path short and straight, with series parts in line and shunt parts on the shortest connection. Leave no branch or dangling copper on an RF net: an open branch is a stub. Use coax for runs of more than a few centimetres (u-blox).
- **Vias:** route RF on one outer layer without vias. If a via is unavoidable, put at least 2 ground vias right beside it, within 1 mm or λg/20: `ecad.board.stitch {"net":"GND","pattern":"layer_change","nets":["ANT_FEED"]}` places them.

## Fences

- **Tie coplanar grounds, and any ground pour bordering an RF line, to the plane with via rows along each side.**
  - Keep the rows close to the gap edge.
  - Keep the pitch below λg/20 at the highest frequency of concern (default: twice the carrier), about 2.9 mm at 2.45 GHz.
  - Add vias at every bend, tee, pad and line end.
  - `ecad.board.stitch {"net":"GND","pattern":"fence","nets":["ANT_FEED"]}` places both rows. Check `rf.fence` afterwards: at 45° bends the outer row can still be too sparse, and one more via at each outer corner (`ecad.board.via` with `net`) fixes it.
- **Silicon Labs reference boards are said to use 1.0–1.3 mm.** That figure is unverified here.
- **Isolation fences beside stripline are a different case:** they sit at least about 4h away so they do not change the impedance.

## Matching

- **Place matching, filter and balun parts:**
  - on the RF IC's layer, as close to the RF pin as they fit, in the reference order and orientation;
  - in 0402 or smaller above 1 GHz (TI AN098: 0603 not recommended);
  - with inductors not side by side, so they do not couple.
- **Route a balun's two halves symmetrically.** Long or unsymmetrical balun tracks are among the most common RF layout faults (TI AN098).
- **Reserve a pi network (shunt-series-shunt) at the antenna feed.** Fit 0 Ω in the series position and leave the shunts open until the antenna has been measured in its final enclosure. Datasheet match values come from the vendor's test board (TI AN058; Johanson).

## Isolation

- **Keep digital lines, clocks, switch nodes, USB and their test points away from RF lines, the match, the crystal and the antenna.** Never route them under the RF path, even on inner layers. If something must cross, cross at a right angle with solid ground between.
- **The RF zone** is the RF copper plus the matching courtyards and the crystal, grown by 1 mm.
- **Keep at least:**
  - max(3h, 1 mm) from RF copper to non-ground copper on the same layer;
  - 10 mm from switch-node or USB copper to the antenna keepout.
- No check covers isolation yet: review it.

## RF ICs

- **Connect the exposed ground pad with many short vias**, following the datasheet pattern: Espressif asks for at least 9. Tent or fill the vias.
- **Never feed an RF ground pin through relief spokes or a long track.**
- **Decoupling for RF ICs** follows `power-ground.md#decoupling`: pin → capacitor → via, within 2 mm, and the reference's capacitor-to-pin assignment.

## Antennas

- **Keepouts:** reproduce each antenna's keepout exactly as its maker draws it, as a keepout object, not as a silkscreen note. Footprints carry no keepouts yet, so draw it with `ecad.board.keepout`. The check reports other parts' pads inside it, and lists every antenna (and every module with no RF pin on a radio board) as a review item to compare with the maker's drawing. Typically that means no copper, parts or vias on any layer, and no battery or metal under or around it.
  - Some modules need ground under the antenna: a u-blox PIFA variant needs full ground under the whole module, while its trace-antenna sibling needs a cut-out.
  - Follow the part's own drawing and never generalise from one module to another.
- **Placement:** put the antenna at a board edge or corner, facing out, with its feed near the edge, never in the middle surrounded by ground. u-blox measured the corner as best for its PIFA. If a module's antenna cannot overhang the board, cut the board away under and beside it (Espressif).
- **Ground plane:** a monopole-type antenna uses the ground plane as its other half, so give it about λ0/4 along its axis at the lowest frequency: 31 mm at 2.4 GHz, 86 mm at 868 MHz. Expect to retune when the ground differs from the reference board.
- **Chip antennas:** reproduce the evaluation board's clearance on every layer, the edge position, the feed and the ground vias along the clearance edge. Johanson's example asks for more than 2 mm from ground at the ends and more than 4 mm along the long side.
- **Metal and enclosure:**
  - Keep tall and metal parts at least 10 mm from the antenna, and keep the module's gap to the enclosure wall (u-blox: 5–10 mm).
  - Plastic lowers the resonance, and a hand lowers it further (TI AN058).
- **Several antennas:** separate them by at least λ0/4 at the lowest shared frequency, and use orthogonal polarisation where possible. Different bands are a coexistence question.

## Shields

- **Give a shield can a continuous ground land**, stitched at λg/20 at the highest frequency inside, and keep it unbroken. Leakage grows quickly as a slot approaches half a wavelength. A gap of λ/20 gives only about 20 dB (Ott).
- **Bring signals in on an inner layer beneath the land**, never through a gap, and filter supply and control lines where they enter.
- **Put the PA and the receiver, VCO and crystal in separate compartments.**
- **Check the lowest cavity mode**, f ≈ 150·√(1/a² + 1/b²) GHz with a and b in mm (Pozar). A 50 × 40 mm can resonates near 4.8 GHz, close to 5 GHz Wi-Fi.

## Coexistence

- **List the harmonics of every clock, bus clock and switcher that land in a receive band of the board's radios.** Declare the bands with `ecad.intent.set {"board":{"bands":[…]}}`: the RF and stitching checks use them. No check lists the harmonics yet: work them out by hand.
  - Example: on a LoRa and BLE node, a 32 MHz crystal's 27th harmonic is 864 MHz, inside EU868, and its 75th–77th fall inside the 2.4 GHz band.
  - Keep those sources compact, terminated and away from the RF zone.
  - Choose the switcher frequency, synchronise it or use spread spectrum where the part allows.
  - Place a radio SoC's own DC-DC inductor exactly as the vendor shows.
- **The acceptance test is receiver desense:** sensitivity with everything running against a quiet board. Plan it as a lab check.

## ESD

- **Put each TVS right at the connector pin it protects**, within 5 mm along the copper. Route the line connector pad → TVS pad → protected IC, with the IC clearly farther from the TVS than the TVS is from the connector: the check warns below 2×. A protected pad the connector reaches without passing over the TVS pad is an error: a TVS on a branch longer than 0.5 mm clamps too late.
  - An 8 kV contact discharge reaches about 30 A in under 1 ns, so each 0.25 nH in the clamp path adds about 10 V at the pin (TI SLVA680A).
- **The ESD checks read the protection parts' records:** a `tvs` or `esd_array` part whose `of` names the connector pins it protects (`ecad.intent.inspect` proposes them).
- **No via and no stub between the connector and the TVS.** If a layer change is needed, make it after the TVS.
- **Give the TVS ground at least 2 vias within 1 mm**, straight into the plane, and put a chassis or shell ground close by.
- **Keep the connector-to-TVS segment short and straight**, with no unprotected track alongside it on any layer (0.5 mm over 2 mm). The fast discharge couples into neighbours before the clamp acts. Its corner style matters far less than its length.
- **The USB data lines go connector → ESD array → common-mode choke (if any) → PHY**, with flow-through packages (TI SPRAAR7J 3.13).

## Cables

- **Put all cable connectors on one edge or in one corner** (LearnEMC). Return current crossing the plane between two cable connectors puts millivolts between them, which drives the cables like a dipole. A few µA of common-mode current on a 1 m cable can fail a Class B limit.
  - If the connectors must sit on different edges, keep clocked and switching circuits out of the area between them, or plan a metal enclosure with chassis bonding.
  - Treat this as critical with two or more cable connectors, no shielded enclosure and noisy parts between. No check covers it yet.
- **Filter at the connector.**
  - Reference filter capacitors to a ground or chassis via right at the pad.
  - Join chassis and board ground with several low-inductance connections near the I/O, not one narrow bridge, which works the wrong way at radiated-emission frequencies (LearnEMC).
  - Use I/O capacitors below about 1000 pF (TI SZZA009).

## Emissions

- **Look first for what can drive common-mode current onto cables and large metal**, not at trace radiation.
  - Common-mode field ≈ 1.257e-6·f·L·I/r. Differential-mode field ≈ 1.316e-14·f²·A·I/r (Paul).
  - About 8 µA of common mode on 1 m at 30 MHz reaches the Class B limit at 3 m, while a 1 cm² loop needs milliamps.
- **Slow every fast driver to what timing allows**, with a series resistor at the source or the driver's slew setting. LearnEMC suggests about 20 % of the bit period.
- **Never put a ferrite or resistor in a return path.**
- **Rank loops by f²·A·I and shrink the worst first**: switcher hot loops, clocks, gate drives.

## Myths

- "90° corners radiate": they measure the same as two 45° bends (Wang, Hubing et al. 2000).
- "Ring the board with ground vias every 2 cm": usually useless on multi-plane boards (LearnEMC).
- "Follow the 20-H rule": no general benefit was measured.
- "Guard traces always help": they rarely do.
- "A chip antenna sold as 50 Ω needs no match": the match depends on your board.
- "Never put ground under an antenna": some modules need it.
- "Meet as many guidelines as possible": rank by mechanism. Boards that tried to meet them all came out worse (LearnEMC).

## Checks that point here

`rf.line`, `rf.coplanar_gap`, `rf.reference`, `rf.fence`, `rf.antenna_keepout`, `esd.path`, `esd.ground`, and the DRC code `keepout`.

No check covers bends, transitions, matching, isolation, RF IC grounds, antenna placement, shields, coexistence, cables, I/O filters or emissions yet: review those against this file.
