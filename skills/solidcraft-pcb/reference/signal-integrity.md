# PCB layout: signal integrity

Contents: [Edge rates](#edge-rates) · [Length](#length) · [Termination](#termination) · [Impedance](#impedance) · [Reference](#reference) · [Crosstalk](#crosstalk) · [Pairs](#pairs) · [Buses](#buses) · [Stubs](#stubs) · [Clocks](#clocks) · [Crystals](#crystals) · [Slow buses](#slow-buses) · [Myths](#myths) · [Checks that point here](#checks-that-point-here)

Delays use the stack-up's propagation speed: about 6.0 ps/mm on outer layers and 6.9 ps/mm on inner layers of FR-4 (εr 4.3).

## Edge rates

- **How "high-speed" a net is depends on its driver's rise time (RT), never its clock frequency.** Bandwidth is about 0.35/RT (Bogatin RoT #1). A 1 MHz SPI clock from a 2 ns driver reflects and couples like a 100 MHz one.
- **Declare RT (`edge_ns`), or a driver class (`driver`), for every fast-capable net** with `ecad.intent.set`:

  | Driver class | RT the checks use | Range |
  |---|---|---|
  | MCU, standard drive (`standard`) | 3 ns | 3–10 ns |
  | MCU, high drive (`high`) | 1 ns | 1–3 ns |
  | LVDS, SerDes (`lvds`, `serdes`) | per spec | |

  - The checks take the fast end of each range. Datasheet RT is often quoted at a large load capacitance, so it overstates RT at the source (SiTime AN10002).
  - Only a clock whose edges scale with its frequency may use RT ≈ 0.07/f (`driver`: `clock`, with its `f_mhz`).
- **A net an IC output drives, with nothing declared, is assumed standard drive** (3 ns, T1), and its findings say "assumed". They are warnings at most until you declare the real driver or `edge_ns`.
- **A kept `slow` role makes the net T0.** It says the net's edges don't matter (a LED, a button, a reset, a status line), so the return-path and layer-change checks leave it out. A declared `driver` or `edge_ns` wins over the role: give one when a "slow" net has fast edges that matter.
- **A crystal's own nets are T0**, even with a `clock` role: they carry a near-sine at the crystal's frequency. The crystal checks (`si.xtal_loop`, `si.xtal_keepout`) judge them.
- **Otherwise an unknown RT makes the net "unverified":** its tier-gated checks report `not_applicable`. They do not fall back to the strict rules.
- **Tiers set which rules apply:**

  | Tier | RT | What matters |
  |---|---|---|
  | T0 | ≥ 5 ns, or static, DC, slow analog | Board-scale lumped; return path only for high di/dt |
  | T1 | 0.5–5 ns (MCU SPI/QSPI/SDIO, SDRAM) | Return path, terminated long clocks, stubs, crosstalk |
  | T2 | 0.1–0.5 ns (USB HS, DDR, RGMII, LVDS) | Plus controlled impedance, skew, return vias |
  | T3 | < 0.1 ns or ≥ 2 Gb/s | Plus loss, via stubs, antipads, fibre weave, pad voids |

  Two interfaces set the tier: `usb2_fs` makes a net T0 (its drivers limit their slew) and `usb2_hs` makes it T2. Give the interface on the pair record (`pairs: [{p, n, interface}]`) or on each net's record; a net's own record wins. Other interface names don't set the tier: 32 MHz QSPI is T1 from its driver.

## Length

- **Electrical length is r = TD/RT**, the one-way delay over the rise time.
  - Below 1/10 the line acts as a lumped capacitor (LaMeres).
  - Above about 1/6 it acts as a transmission line and needs a decision: terminate it, or show it is fine (SiTime AN10002).
  - At 1 ns RT, 1/6 is about 167 ps, or 25–28 mm.
- **The decision is required only for clocks, strobes, edge-triggered inputs (resets, interrupts) and spec-defined interfaces.** Data lines sampled after they settle are reported as info.
- **The ratio is triage, not proof.** A strong driver (about 10 Ω) on 1 inch of line rings into tens of pF even at r = 1/6 (Johnson, newsletter 6.06). Damp it with a series resistor or simulate it.
- **No check covers electrical length yet.** `ecad.board.net.inspect` gives each net's `delay_ps` to compare with RT.

## Termination

- **Series (source) termination.**
  - Put a resistor of Z0 − Rout right at the driver: a 15 Ω driver on a 60 Ω line takes about 43 Ω (SiTime AN10002).
  - Track between the driver and the resistor acts as an inductive stub. Johnson's estimate of the reflection is ½·V·L/(Z0·RT).
  - Keep the driver-to-resistor delay under RT/6 for 20 dB of suppression: about 12–14 mm at 0.5 ns and 25–28 mm at 1 ns. Above RT/3 is an error ("How Close is Close Enough?", EDN 1998).
- **Parallel (end) termination.**
  - Put it at the receiver, after the last load.
  - The stub beyond it, plus the receiver capacitance, reflects about ½·Z0·(C_stub + C_gate)/RT. That breaks the system above 0.3 and is worth fixing above 0.1 (Johnson, newsletter 2.07).
  - Differential clock terminations go at the load (SiTime AN10006).
- **Drivers with matched output impedance or on-die termination need no external part.**

## Impedance

- **Specify a target impedance only where a spec asks for one (USB 90 Ω, PCIe, HDMI and DP 100 Ω) or at T2 and above.** On a 1.6 mm 2-layer board 50 Ω needs a track of about 2.8–3 mm (TI SLLA414 tables), so report the estimated Z0 there instead of a target.
- **Compute widths from the real stack-up** with `ecad.board.impedance.inspect {"layer":…,"kind":…,"target":…}`.
  - It uses closed forms (IPC-2141; Wadell for coplanar) and leaves out the solder mask, which lowers microstrip impedance by a few percent.
  - Fabs hold about ±10 % on controlled layers (Eurocircuits). Design to the spec tolerance minus the fab tolerance.
  - Sanity checks: 50 Ω microstrip has W ≈ 2h; 50 Ω stripline has W ≈ 0.8 × the height of each side (RoT #27, #28).
- **Keep width and gap constant.** Neck only inside the breakout zone (within 6.35 mm of a part of 0.8 mm pitch or finer, TI SPRAAR7J), and keep each neck under a third of the edge's length on the board.
- **Keep same-layer copper at least 1 track width away, or use the coplanar model.** Beyond 3W the effect is not measurable. At 1W on both sides a 50 Ω line drops about 4 % (Bogatin RoT #25).

## Reference

- **Return paths, splits and layer changes are owned by `power-ground.md#return-paths`.**
- **Keep fast tracks inside their reference copper:** at least 3h from its edge, board edge included, and 5h where there is room.
  - A track 25 mil from the edge, 45 mil over its plane, drove about 18 dB more cable common-mode current than a centred one (Berg et al. 2000).
  - Multi-Gb/s pairs keep at least 2.3 mm (TI SPRAAR7J).
  - A 20h rule applies only where an interface guide names it together with its dielectric (USB.org's 90 mil assumes h = 4.5 mil).

## Crosstalk

- **Measure spacing edge to edge, and think in units of the height over the plane.** Coupling goes roughly as 1/(1 + (s/h)²).
  - At 50 Ω, an edge gap of 2W gives about 2.5 % NEXT per neighbour (Bogatin RoT #20).
  - 3h keeps the worst case near −20 dB (LearnEMC).
  - "3W" is ambiguous: record a vendor's NW rule as edge gap or centre spacing.
- **Coupled length matters up to saturation**, L_sat = RT·v/2: about 75 mm at 1 ns.
  - Far-end crosstalk on microstrip grows as about 0.5 % × length[in] / RT[ns] at s = W (RoT #21).
  - Stripline FEXT is near zero, so move long fast buses inside.
- **Budget about 5 % of swing for all crosstalk on a single-ended victim** (RoT #19). Keep s ≥ 3h wherever two fast nets run together for more than min(12.7 mm, L_sat/4). No check covers crosstalk yet: `ecad.board.net.inspect {"nets":["QSPI_SCK"],"metrics":["coupling"]}` lists each neighbour's coupled length and least gap.
- **Clocks get extra room:** 3h from everything. Pairs keep 1.27 mm from clocks and 0.5 mm from other signals (TI SPRAAR7J; USB.org).
- **Prefer spacing to guard tracks.**
  - On microstrip a guard can do more harm than good.
  - On stripline, short the guard to the plane at both ends of the coupled run (Bogatin and Simonovich 2013).
  - Never leave a guard or an island floating.
- **Adjacent signal layers with no plane between run at right angles** (`routing-dfm.md#neat-routing`).

## Pairs

- **Route the two members together, mirror-symmetric, with a constant gap.**
  - Break out within 6.35 mm of the package.
  - Pass obstacles on the same side.
  - Give both members the same via count, with the vias side by side.
  - Place in-line parts (AC caps, ESD, chokes) symmetrically, in 0402 or smaller (TI SPRAAR7J).
- **Bend with interior angles above 135° or with arcs, and alternate left and right turns** so the inner and outer lengths cancel.
- **Match the delay inside each pair to the interface's budget, and compensate near the mismatch:**
  - USB 2.0: warn at 2.5 mm (Renesas). Never treat it as an error: needless serpentines add more harm than 8 ps of skew.
  - 5 Gb/s and up: about 5 mil (TI).
  - 0.37 in of mismatch gave 50 ps of skew (SiTime AN10006).
- **Choose the gap for density and loss, then hold it.** Tight coupling barely helps noise: 6.6 % to 6.1 % (Bogatin 2009). At equal impedance, tight pairs over a plane showed more than twice the pair-to-pair crosstalk.
- **USB 2.0 High Speed:**
  - 90 Ω ±15 % over solid ground.
  - Board delay within 1 ns for a device and 3 ns for a host. That is about 150 mm and 450 mm at 6.7 ps/mm; the 100 and 300 mm figures often quoted are conservative.
  - At most 4 vias per track (TI), no stubs or test pads, no series capacitor.
- **USB 2.0 Full Speed:**
  - Edges are 4–20 ns, so a few inches of track is lumped.
  - Keep D+/D− short, together and over ground, with ESD at the connector (`rf-emc.md#esd`).
  - Impedance and skew are notes unless the run exceeds 100 mm.
  - A PHY that can do High Speed takes the HS rules.
- **The pair checks read a pair record:** `ecad.intent.set {"pairs":[{"name":"USB","p":"USB_DP","n":"USB_DN","interface":"usb2_fs"}]}`. The gap comes from the pair's net class (`diff_pair`). Skew compares the members' whole delay, stubs included, and `skew_mm` on the record overrides the budget. A connector's escape is exempt from the uncoupled-end rule.
- **Fibre weave** matters only for multi-Gb/s runs longer than about 25 mm: route at 10–35° to the weave, or ask for a spread glass.

## Buses

- **Derive length-match tolerances from the timing budget.**
  - 10 mil is about 1.7 ps, which means nothing on a 32 MHz QSPI bus whose margins are nanoseconds.
  - The default is 5 % of the bit period for single-data-rate buses.
  - Match each group to its own clock or strobe, and compare delays, not lengths, across layers.
- **Serpentines:**
  - Keep the loops at least 3W apart.
  - Keep each coupled section short: about 1 inch at 1 ns RT, 0.1 inch at 100 ps.
  - Put the serpentine near the end where the mismatch arises.
  - Tight switchbacks let the signal cut the corner through crosstalk and arrive early (Johnson, "Serpentine Delays").

## Stubs

- **Route fast nets point to point, or as a daisy chain.**
  - Keep every branch under RT/6 of delay: 0.5 inch branches were fine at 1 ns and 3 inch branches failed (SiTime AN10002).
  - At multi-Gb/s rates, keep stubs under 0.3/BR[Gb/s] inch (Bogatin RoT #18).
  - A test pad sitting on the route is not a stub; one on a branch is.
- **Via stubs resonate at about 1.5/length[in] GHz:** around 25 GHz for a full 1.6 mm board. They matter only at multi-Gb/s rates, where you back-drill above 15 mil (TI SPRAAR7J). USB 2.0 and QSPI never need it.

## Clocks

- **Route each clock first**, point to point from its own driver, short, over solid ground, with few vias. Keep it away from board edges, connectors and switchers.
- **Never drive three or more lines from one clock output.** Two equal, equally loaded branches, each with its own series resistor, can work (SiTime AN10002).
- **Keep fast tracks out of the areas around crystals, oscillators and switching inductors on every layer:** courtyard + 1 mm, 3 mm around switch inductors. A solid ground plane between makes it info.

## Crystals

The pins are high-impedance, low-level nodes: a 32 kHz amplifier has a few µA/V of gain and swings tens to hundreds of mV (ST AN2867). Stray capacitance pulls the frequency: at 45 ppm/pF, 0.175 pF shifts about 7.8 ppm, roughly 0.7 s a day.

| Rule | Value | Class |
|---|---|---|
| Crystal to oscillator pins, routed | Warn above 5 mm (MHz) or 3 mm (32 kHz); error above 12 mm (Microchip) | Critical |
| Vias on crystal nets | None | Critical |
| Load capacitors to crystal pads | Within 3 mm, same side as the IC, ground ends close together | Critical |
| Foreign copper in the crystal zone (+0.5 mm), same layer and the next (every layer on 2-layer boards) | None. Critical for 32 kHz crystals near fast or high-current copper | Practical |
| Fast or high-current copper near the zone | Warn within 2 mm, error within 1 mm | Practical |
| Test points on crystal nets | None (ST lists them as a fault) | Practical |

- **Guard with a ground pour under and around the oscillator**, stitched to the MCU's oscillator ground, with the continuous plane beneath on multilayer boards. Do not use a long thin ground track looped around the crystal, which becomes a pickup loop of its own (TI SWRA640).
- **Use equal C0G load capacitors** unless the crystal vendor says otherwise.
- **For micropower 32 kHz circuits in humid places, plan cleaning or coating:** leakage stops them.
- **Oscillator modules and TCXOs are clock sources**, and follow the clock rules.

## Slow buses

- **I2C:**
  - Keep the bus capacitance under 400 pF in standard and fast mode. Track capacitance comes from the stack-up, plus about 10 pF per pin.
  - Size the pull-ups for the rise time, and fit one set per bus.
- **RS-485 and long UART lines:** terminate both ends of the line, add failsafe bias, and keep stubs to each transceiver short (RT/10 of round trip, TI THVD datasheets).

## Myths

- "A low clock frequency means the net is slow": bandwidth follows the edge.
- "Return current takes the least-resistance path": true only below about 10 kHz (Bogatin SIJ 2023).
- "Rounding a corner removes its discontinuity": it removes only about 21 % of the excess copper (Johnson).
- "Match every bus to 10 mil": derive the tolerance from timing.
- "Copper near a controlled line ruins it": below 2 % per side beyond 1W.
- "Via stubs ruin USB 2.0": they resonate near 25 GHz.

## Checks that point here

`si.xtal_loop`, `si.xtal_keepout`, `si.pair`, `si.skew` (with the `usb2_hs` and `usb2_fs` rules).

No check covers electrical length, termination, impedance outside RF lines, plane edges, crosstalk, buses, stubs, clocks or slow buses yet: review those against this file.
