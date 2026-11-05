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

Build it again (15 to 25 minutes, most of it routing):

```bash
cargo build --profile ci -p solidcraft-cli
```

```bash
python3 examples/lora-mini/build.py
```

## The board

46 x 30 mm, 2 layers, 1.6 mm, the "JLCPCB 2-layer standard" fab profile (its figures read from
jlcpcb.com's PCB capabilities page on 2026-11-04), 0.15 mm signal tracks and gaps and 0.4 mm power
tracks, four 2.2 mm holes for M2 screws in the corners, ground poured on both sides and stitched
over the board, under the modules, the USB-C, the buttons and the battery plug. The bottom is the
main ground plane: tracks stay on the top where they can (the router's bottom cost, and nothing
crosses under SCK), and the bottom plane is in one piece but for a small pocket by the USB-C tied by
R2's ground via. The E73's ceramic antenna sits at the left edge over a keep-out with no copper
on either layer, the module 0.4 mm inside the edge (Ebyte asks only for the antenna to be clear, not
to overhang). USB-C and buttons on the top edge; slide switch and battery plug out of the right
edge; GPS header J4 along the bottom edge. The board's identity is on its back, behind U1 (text and
QR code; no serial box or date, which fit nowhere clear of the tracks). The E22 carries its own IPEX antenna socket (Ebyte's manual: "IPEX and stamp hole are
optional"), so the antenna cable plugs into the module: the board has no RF line and no U.FL, and
the module's ANT stamp pad is left unconnected. Design intent: the EU868 band (863-870 MHz) and
2.4 GHz BLE.

## Where it stands

The last build (`out/report.json`): the design rule check has 0 errors and 0 warnings, the
quality checks no error, warning or critical finding, the electrical check 0 errors, the release
gate passes, and `ecad.board.verify_outputs` rebuilds the board's nets from the written Gerber and
drill files. Every connection is routed (`route.py`: an A* search per connection, each route pulled
tight to the fewest 0/45/90 degree segments, USB D- run beside D+, a rip-up rescue that routes the
nets in the way again whole; `build.py`'s `PRE_ROUTES` draws 3V3 round U1, RST, BUSY and DIO1 by
hand first and its `HAND_ROUTES` finishes SWCLK, NRESET, the battery sense and C6's ground); three
ground pads whose own via did not fit (U1.5, U1.21, J1.B1) join through the pours. No via is
without a net.

The ground map (`ecad.board.ground.inspect`, 10 mA on every mm2 into the largest plane, target
0.5 mV): top copper 0.49 mV at worst; bottom 0 over the plane and 0.54 mV in that pocket.

Kept on purpose, each approved in the design with its reason (the build's `APPROVE` and
`APPROVE_QUALITY`): JLCPCB's standard 0.3/0.6 mm via at the profile's least drill and ring; the
nRF52840's VBUS sense pin joined by a 0.15 mm track (it detects USB, microamps); narrow same-net
angles (38 to 67 degrees) and thin tracks leaving a pad near its corner, from the router and the
hand routes; the 0.54 mV pocket and the 11 mm via-free circle behind the QR code (against 5.8 mm at
2.4 GHz; the board has no RF line); and solid ground joins: 46 approvals, none of a critical
finding. Tented vias next to pads stay as information. Ten short 45-degree chamfers (0.07-0.28 mm)
remain at corners where a square corner would break clearance.

## Parts

Each LCSC number below was checked against JLCPCB's parts library on 2026-11-04 (its parts search;
class, stock and the price for one): every one is in stock. The cost is for 5 assembled boards: the
quantity JLCPCB buys (5 per board plus its attrition) at its price break.

| Ref | Part | LCSC | Class | Stock | 5 boards |
|---|---|---|---|---|---|
| U1 | Ebyte E73-2G4M08S1C (nRF52840) | C356849 | extended | 491 | $38.56 |
| U2 | Ebyte E22-900M22S (SX1262, 850-930 MHz) | C411293 | extended | 591 | $36.83 |
| J2 | JST S2B-PH-SM4-TB battery connector | C295747 | extended | 27,567 | $1.13 |
| J1 | TYPE-C-31-M-12 USB-C receptacle | C165948 | extended | 160,065 | $0.94 |
| U3 | ME6217C33M5G, 3.3 V LDO, 800 mA | C427602 | extended | 66,389 | $0.88 |
| Q1 | AO3401A P-MOSFET, GPS power | C15127 | basic | 1,638,203 | $0.87 |
| SW1 | MSK12C02 slide switch (power) | C431540 | extended | 182,833 | $0.60 |
| C4, C6, C8 | 10 uF 0603 | C19702 | basic | 10,924,804 | $0.51 |
| C1, C2, C3 | 4.7 uF 0603 | C19666 | basic | 4,240,572 | $0.38 |
| SW2, SW3 | TS-1187A tact switches (user, reset) | C318884 | basic | 1,000,870 | $0.37 |
| U4 | TP4054, 1S Li-ion charger, about 300 mA (RPROG 3.3 kOhm) | C668215 | extended | 22,096 | $0.35 |
| C10 | 1 uF 0603 | C15849 | basic | 12,109,134 | $0.29 |
| D2 | KT-0603G green LED (status) | C12624 | extended | 365,366 | $0.25 |
| C5, C7, C9 | 100 nF 0603 | C14663 | basic | 37,529,957 | $0.23 |
| D1 | KT-0603R red LED (charging) | C2286 | basic | 6,729,148 | $0.15 |
| R1-R8 | 0603 resistors (5.1k, 3.3k, 1k, 1M, 100k) | C23186, C22978, C21190, C22935, C25803 | basic | millions | $0.20 |
| J4 | 1x6 2.54 mm GPS header (3V3 switched, GND, TX, RX, PPS, EN) | none | not assembled | | |
| J5 | SWD test pads (3V3, SWCLK, SWDIO, GND) | none | not assembled | | |

Parts: about $82.50 for 5 boards, $75.40 of it the two modules. JLCPCB's assembly fees come on top
(its price page, updated 2026-10-22: Economic $8.18 setup, $1.53 stencil, $3.07 per extended part;
Standard $25.56 setup, $8.21 stencil, $1.53 per part; $0.0016 per joint).

The green LED changed: C72043 (19-217/GHC) is out of stock and no longer offered. JLCPCB's basic
library has no green 0603 LED (its basic LEDs: red and white 0603, green 0805 C2297), so D2 is the
extended KT-0603G, the red LED's own family.

Cheaper parts that keep the function, not applied: the basic green 0805 LED C2297 or the basic white
0603 C2290 save D2's extended-part fee (a new footprint for C2297); the SX1262 module Ra-01SH
(C2764087, $4.58, 2,573 in stock) would save about $2.80 a board but needs its own footprint and
pinout, and its TCXO and RF switch are not checked here. No basic part keeps the function of the
modules, the LDO (the basic XC6206 gives 200 mA, the basic AMS1117 drops 1.1 V), the charger, the
USB-C, the JST or the slide switch.

## Assembly placement

The placement list gives each part at the middle of its pads, which is what the assembler places
by: the E73's own origin is its body's middle, and its antenna end has no pads, so the module came
out about 2 mm over the board's edge in JLCPCB's preview while it was placed by its origin. The
footprints drawn turned from JLCPCB's reference for the part carry a rotation offset, kept with the
library footprint (`ecad.board.cpl_rotation`, in `parts.py`): USB-C, JST and slide switch 180
degrees, SOT-23 and SOT-23-5 -90 degrees, taken from JLCPCB's preview of the first order (pin-1
marks and bodies against the pads). The LEDs, the tact switches and the modules matched. Check the
preview again after each upload.

J4 and J5 are marked do-not-populate: they are in neither list.

## Checked against datasheets, and what is not

Read from the makers' drawings (LCSC's datasheets): the USB-C land pattern (the old one had the
receptacle turned round, its body into the board), the slide switch (its pins are not evenly
spaced, and it has two peg holes), the tact switch (pins 1-2 and 3-4 joined), the E22 and E73 outer
pads and outlines, the LEDs' polarity (the KT-0603 datasheet calls the anode pin 1; our footprint calls the
cathode pad 1, on the side of its silk mark, and JLCPCB's preview put the part's anode on our anode
pad), and the pinouts of the ME6217, TP4054 and
AO3401A.

Not verified:
- J2's land pattern (JST S2B-PH-SM4-TB): the catalogue page with JST's PCB layout could not be
  fetched where this was built; the outline page gives 7.9 x 6 mm only. The lands are JST's
  pattern as remembered: compare them with JST's drawing before ordering.
- The E73's 15 inner pads (Ebyte gives no dimensions; none is connected here).
- The tented ground vias under the E22 and E73 (and the USB-C, buttons and battery plug): the design
  rules allow them and they keep clear of every pad, but Ebyte's manuals were not re-read for them
  here: check before ordering.
- The rotation offsets: from JLCPCB's preview, not from its footprint data.
- The USB-C shell slots have the maker's 0.15 mm ring; JLCPCB lists 0.18 mm as the least ring for
  a plated hole (0.25 mm advised). JLCPCB stocks and places this part; check their DFM report.
- The fab profile's plating allowance (not on JLCPCB's capabilities page).
- Firmware: P0.00/P0.01 are used as GPIO (no 32.768 kHz crystal: the internal RC is the
  low-frequency clock), and P0.10 (NFC2) as GPIO, so NFCPINS in UICR must be set.

## Ordering

Upload `lora-mini-jlcpcb.zip` as the board, choose assembly on the top side, and give the BOM and
CPL files. Check the placement preview: pin 1 of each part, the USB-C opening at the top edge, the
battery opening and the slide switch's lever out of the right edge. This example never places an order.
