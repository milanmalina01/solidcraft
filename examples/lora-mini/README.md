# LoRa Mesh Node Mini

A small LoRa mesh node, designed headlessly with SolidCraft's commands and laid out for JLCPCB's
2-layer board with full assembly: an nRF52840 module for Bluetooth and USB, an SX1262 module for
EU868 LoRa, USB-C charging of a single-cell LiPo, and a header for a GPS module.

- `build.py` builds everything: the library, the schematic in six sections (Power, USB, MCU, LoRa,
  GPS, UI), the board, its routes, pours, stitching, names, marking, the checks and the outputs.
  `parts.py` has the footprints (with where each comes from), `circuit.py` the components, the
  nets and the circuit notes, `route.py` the small router that turns connections into
  `ecad.board.track` commands (SolidCraft has no autorouter command).
- `lora-mini-library.sc` the electronics library, `lora-mini.sc` the design.
- `out/lora-mini-jlcpcb.zip` the Gerber X2 and Excellon files for the board order;
  `out/lora-mini-jlcpcb-bom.csv` and `out/lora-mini-jlcpcb-cpl.csv` the parts and placement lists
  for the assembly order; `out/lora-mini.pdf` every printed page; `out/report.json` the checks'
  results.

Build it again (about a quarter of an hour, most of it routing):

```bash
cargo build --profile ci -p solidcraft-cli
```

```bash
python3 examples/lora-mini/build.py
```

## The board

52 x 34 mm, 2 layers, 1.6 mm, the "JLCPCB 2-layer standard" fab profile, four 2.2 mm holes for
M2 screws, ground poured on both sides and stitched where the ground map says it is weak, plus a
fence along the RF line. The E73's ceramic antenna sits at the left edge over a keep-out with no
copper on either layer. The E22's antenna pin feeds a U.FL through a short coplanar line of the
width `ecad.board.impedance.inspect` gives for 50 ohm (Wadell's conductor-backed coplanar
waveguide, 0.25 mm gaps). Design intent: the EU868 band (863-870 MHz) and 2.4 GHz BLE.

| Ref | Part | LCSC |
|---|---|---|
| U1 | Ebyte E73-2G4M08S1C (nRF52840) | C356849 |
| U2 | Ebyte E22-900M22S (SX1262, 850-930 MHz) | C411293 |
| U3 | ME6217C33M5G, 3.3 V LDO, 800 mA | C427602 |
| U4 | TP4054, 1S Li-ion charger, about 300 mA (RPROG 3.3 kOhm) | C668215 |
| J1 | TYPE-C-31-M-12 USB-C receptacle | C165948 |
| J2 | JST S2B-PH-SM4-TB battery connector | C295747 |
| J3 | Hirose U.FL-R-SMT-1(10) | C88373 |
| J4 | 1x6 2.54 mm GPS header (3V3 switched, GND, TX, RX, PPS, EN) | none: fit by hand or leave out |
| J5 | SWD test pads (3V3, SWCLK, SWDIO, GND) | none |
| SW1 | MSK12C02 slide switch (power) | C431540 |
| SW2, SW3 | TS-1187A tact switches (user, reset) | C318884 |
| Q1 | AO3401A P-MOSFET, GPS power | C15127 |
| D1, D2 | 0603 LEDs, red (charging) and green (status) | C2286, C72043 |
| R, C | 0603 resistors and capacitors | see the parts list |

The LCSC numbers came from search results on 2026-11-02; stock and the basic/extended class
change, so check each before ordering.

## Check before ordering

- The footprints of J1 (USB-C), J2 (JST PH), J3 (U.FL), SW1 and SW2/SW3 are marked VERIFY in
  `parts.py`: their datasheets could not be read while this was built. Compare each land pattern
  with the manufacturer's drawing (or the assembler's footprint check) and correct `parts.py`.
- The E73's 15 inner pads are scaled from Ebyte's drawing (it gives no dimensions for them):
  about +-0.1 mm. Only its outer pads are used here.
- The fab profile's figures are search summaries of JLCPCB's capability pages, each marked to
  verify.
- Firmware: P0.00/P0.01 are used as GPIO (no 32.768 kHz crystal: the internal RC is the
  low-frequency clock), and P0.10 (NFC2) as GPIO, so NFCPINS in UICR must be set.

## Ordering

Upload `lora-mini-jlcpcb.zip` as the board, choose assembly on the top side, and give the BOM and
CPL files. Check the placement preview: the rotation offsets of footprints drawn turned from the
assembler's reference are set with `ecad.board.cpl_rotation`.
