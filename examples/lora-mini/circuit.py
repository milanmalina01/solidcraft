"""The LoRa Mesh Node Mini's circuit: its components (with their LCSC numbers), the symbols' pins, the
nets and the schematic's sections.

Part choices (LCSC numbers as search results gave them on 2026-11-02; lcsc.com itself could not be
read from where this was built, so check stock and class before ordering):

- U1 E73-2G4M08S1C (nRF52840 module, ceramic antenna), C356849.
- U2 E22-900M22S (SX1262, 850-930 MHz, TCXO, stamp holes and IPEX), C411293.
- U3 ME6217C33M5G (3.3 V LDO, 800 mA, 100 mV typ. dropout at 300 mA), C427602.
- U4 TP4054 (single-cell Li-ion linear charger, SOT-23-5), C668215 (UMW).
- J1 TYPE-C-31-M-12 (USB-C, 16 pins), C165948. J2 S2B-PH-SM4-TB (JST PH, 2 pins), C295747.
- J3 U.FL-R-SMT-1(10), C88373. Q1 AO3401A (P-MOSFET), C15127.
- Passives in 0603, LEDs in 0603; their numbers are JLCPCB basic parts as commonly listed.

Circuit notes, from the datasheets:
- TP4054: PROG to ground through RPROG; 10 kOhm gives 100 mA, 2 kOhm 500 mA (its C/10 termination
  is given at 50 mA for 2 kOhm). 3.3 kOhm gives about 300 mA, under 1C for a small LiPo.
  CHRG is open drain, low while charging: the charge LED hangs from VBUS.
- ME6217: pin 1 VIN, 2 VSS, 3 CE (on when high), 4 NC, 5 VOUT.
- nRF52840 in the module: the normal voltage mode, VDD and VDDH both on 3V3, DCCH left open; USB D+
  and D- go straight to the receptacle, VBUS to the module's VBUS for USB detection.
  P0.00 and P0.01 (XL1, XL2) are used as GPIO (the internal RC oscillator is the low-frequency
  clock), and P0.10 (NFC2) as GPIO: the firmware sets NFCPINS in UICR.
- E22: DIO2 drives TXEN (the module's RF switch), RXEN from a GPIO, DIO3 feeds its TCXO inside.
- Battery sense: 1 MOhm / 1 MOhm to AIN4 with 100 nF (500 kOhm source: the SAADC's 40 us
  acquisition time is for up to 800 kOhm), about 2 uA from the cell.
- GPS power: AO3401A high-side switch, its gate pulled up to 3V3 (off until P0.10 pulls it low).
"""

# (name, prefix, value, symbol, footprint, lcsc, mpn, manufacturer)
COMPONENTS = [
    ("E73-2G4M08S1C", "U", "E73-2G4M08S1C", "E73", "E73-2G4M08S1C", "C356849", "E73-2G4M08S1C", "Ebyte"),
    ("E22-900M22S", "U", "E22-900M22S", "E22", "E22-900M22S", "C411293", "E22-900M22S", "Ebyte"),
    ("ME6217C33M5G", "U", "ME6217C33", "LDO", "SOT-23-5", "C427602", "ME6217C33M5G", "Microne"),
    ("TP4054", "U", "TP4054", "CHARGER", "SOT-23-5", "C668215", "TP4054", "UMW"),
    ("USB-C", "J", "USB-C", "USBC", "USB-C-16P", "C165948", "TYPE-C-31-M-12", "Korean Hroparts"),
    ("JST-PH-2", "J", "Battery", "JST2", "JST-PH-2-SMD", "C295747", "S2B-PH-SM4-TB(LF)(SN)", "JST"),
    ("U.FL", "J", "U.FL", "UFL", "U.FL", "C88373", "U.FL-R-SMT-1(10)", "Hirose"),
    ("GPS-HDR", "J", "GPS", "GPSHDR", "HDR-1x6", "", "", ""),
    ("SWD-PADS", "J", "SWD", "SWD", "SWD-PADS-4", "", "", ""),
    ("SLIDE", "SW", "ON/OFF", "SLIDE", "SLIDE-SPDT", "C431540", "MSK12C02", "SHOU HAN"),
    ("TACT", "SW", "TACT", "TACT", "TACT-5.1", "C318884", "TS-1187A-B-A-B", "XKB"),
    ("AO3401A", "Q", "AO3401A", "PMOS", "SOT-23", "C15127", "AO3401A", "AOS"),
    ("LED-RED", "D", "RED", "LED", "LED0603", "C2286", "KT-0603R", "Hubei KENTO"),
    ("LED-GREEN", "D", "GREEN", "LED", "LED0603", "C72043", "19-217/GHC-YR1S2/3T", "Everlight"),
    ("R-5k1", "R", "5.1k", "RES", "0603", "C23186", "0603WAF5101T5E", "UNI-ROYAL"),
    ("R-3k3", "R", "3.3k", "RES", "0603", "C22978", "0603WAF3301T5E", "UNI-ROYAL"),
    ("R-1k", "R", "1k", "RES", "0603", "C21190", "0603WAF1001T5E", "UNI-ROYAL"),
    ("R-100k", "R", "100k", "RES", "0603", "C25803", "0603WAF1003T5E", "UNI-ROYAL"),
    ("R-1M", "R", "1M", "RES", "0603", "C22935", "0603WAF1004T5E", "UNI-ROYAL"),
    ("C-100n", "C", "100nF", "CAP", "0603", "C14663", "CC0603KRX7R9BB104", "YAGEO"),
    ("C-1u", "C", "1uF", "CAP", "0603", "C15849", "CL10A105KB8NNNC", "Samsung"),
    ("C-4u7", "C", "4.7uF", "CAP", "0603", "C19666", "CL10A475KO8NNNC", "Samsung"),
    ("C-10u", "C", "10uF", "CAP", "0603", "C19702", "CL10A106KP8NNNC", "Samsung"),
]

E73_PINS = {
    1: "P1.11", 2: "P1.10", 3: "P0.03", 4: "P0.28/AIN4", 5: "GND", 6: "P1.13", 7: "P0.02", 8: "P0.29", 9: "P0.31", 10: "P0.30",
    11: "P0.00", 12: "P0.26", 13: "P0.01", 14: "P0.06", 15: "P0.05", 16: "P0.08", 17: "P1.09", 18: "P0.04", 19: "VDD", 20: "P0.12",
    21: "GND", 22: "P0.07", 23: "VDDH", 24: "GND", 25: "DCCH", 26: "P0.18/RESET", 27: "VBUS", 28: "P0.15", 29: "D-", 30: "P0.17",
    31: "D+", 32: "P0.20", 33: "P0.13", 34: "P0.22", 35: "P0.24", 36: "P1.00", 37: "SWDIO", 38: "P1.02", 39: "SWDCLK", 40: "P1.04",
    41: "P0.09", 42: "P1.06", 43: "P0.10",
}
E22_PINS = {
    1: "GND", 2: "GND", 3: "GND", 4: "GND", 5: "GND", 6: "RXEN", 7: "TXEN", 8: "DIO2", 9: "VCC", 10: "GND", 11: "GND",
    12: "GND", 13: "DIO1", 14: "BUSY", 15: "NRST", 16: "MISO", 17: "MOSI", 18: "SCK", 19: "NSS", 20: "GND", 21: "ANT", 22: "GND",
}


def _box(pins):
    """A box symbol's pins: power on the left, the rest on the right."""
    out = []
    for n, name in pins:
        power = name in ("GND", "VDD", "VDDH", "VBUS", "VCC", "VIN", "VOUT", "BAT", "DCCH") or name.startswith("SHIELD")
        kind = "power_input" if name in ("GND", "VDD", "VDDH", "VCC", "VIN", "BAT") else ("power_output" if name in ("VOUT",) else "passive")
        out.append({"number": str(n), "name": name, "electrical": kind, "side": "left" if power else "right"})
    return out


def symbol_commands():
    out = [
        ("ecad.symbol.generate", {"symbol": "RES", "style": "resistor"}),
        ("ecad.symbol.generate", {"symbol": "CAP", "style": "capacitor"}),
        ("ecad.symbol.generate", {"symbol": "LED", "style": "led", "pins": [{"number": "1", "name": "K"}, {"number": "2", "name": "A"}]}),
        ("ecad.symbol.generate", {"symbol": "PMOS", "style": "pmos", "pins": [{"number": "1", "name": "G"}, {"number": "2", "name": "S"}, {"number": "3", "name": "D"}]}),
        ("ecad.symbol.generate", {"symbol": "TACT", "style": "switch", "count": 4}),
        ("ecad.symbol.generate", {"symbol": "E73", "style": "box", "pins": _box(sorted(E73_PINS.items()))}),
        ("ecad.symbol.generate", {"symbol": "E22", "style": "box", "pins": _box(sorted(E22_PINS.items()))}),
        ("ecad.symbol.generate", {"symbol": "LDO", "style": "box", "pins": _box([(1, "VIN"), (2, "GND"), (3, "CE"), (4, "NC"), (5, "VOUT")])}),
        ("ecad.symbol.generate", {"symbol": "CHARGER", "style": "box", "pins": _box([(1, "CHRG"), (2, "GND"), (3, "BAT_OUT"), (4, "VCC"), (5, "PROG")])}),
        ("ecad.symbol.generate", {"symbol": "USBC", "style": "box", "pins": _box([
            ("A1", "GND"), ("A4", "VBUS"), ("B8", "SBU2"), ("A5", "CC1"), ("B7", "DN2"), ("A6", "DP1"), ("A7", "DN1"), ("B6", "DP2"),
            ("A8", "SBU1"), ("B5", "CC2"), ("B4", "VBUS"), ("B1", "GND"), ("S1", "SHIELD1"), ("S2", "SHIELD2"), ("S3", "SHIELD3"), ("S4", "SHIELD4")])}),
        ("ecad.symbol.generate", {"symbol": "JST2", "style": "box", "pins": _box([(1, "+"), (2, "-"), ("MP1", "MP1"), ("MP2", "MP2")])}),
        ("ecad.symbol.generate", {"symbol": "UFL", "style": "box", "pins": _box([(1, "SIG"), (2, "GND"), (3, "GND")])}),
        ("ecad.symbol.generate", {"symbol": "GPSHDR", "style": "box", "pins": _box([(1, "3V3"), (2, "GND"), (3, "TX"), (4, "RX"), (5, "PPS"), (6, "EN")])}),
        ("ecad.symbol.generate", {"symbol": "SWD", "style": "box", "pins": _box([(1, "3V3"), (2, "SWCLK"), (3, "SWDIO"), (4, "GND")])}),
        ("ecad.symbol.generate", {"symbol": "SLIDE", "style": "box", "pins": _box([(1, "A"), (2, "COM"), (3, "B"), ("MP1", "MP1"), ("MP2", "MP2"), ("MP3", "MP3"), ("MP4", "MP4")])}),
    ]
    return out


def component_commands():
    out = []
    for name, prefix, value, symbol, footprint, lcsc, mpn, maker in COMPONENTS:
        p = {"name": name, "prefix": prefix, "value": value, "symbol": symbol, "footprint": footprint}
        if lcsc:
            p["lcsc"] = lcsc
        if mpn:
            p["mpn"] = mpn
        if maker:
            p["manufacturer"] = maker
        out.append(("ecad.component.create", p))
    return out


# Parts: designator -> (component, section, schematic at [x, y]). Sections are laid out 3 x 2.
SECTIONS = {
    "Power": [[0, 0], [150, 100]],
    "USB": [[160, 0], [260, 100]],
    "MCU": [[0, 110], [150, 230]],
    "LoRa": [[160, 110], [300, 230]],
    "GPS": [[310, 110], [420, 230]],
    "UI": [[270, 0], [420, 100]],
}

PARTS = {
    # Power: charger, battery, switch, regulator.
    "U4": ("TP4054", "Power", [40, 60]),
    "R3": ("R-3k3", "Power", [55, 40]),
    "C1": ("C-4u7", "Power", [15, 60]),
    "D1": ("LED-RED", "Power", [30, 85]),
    "R4": ("R-1k", "Power", [15, 85]),
    "J2": ("JST-PH-2", "Power", [80, 40]),
    "C2": ("C-4u7", "Power", [70, 70]),
    "SW1": ("SLIDE", "Power", [95, 70]),
    "U3": ("ME6217C33M5G", "Power", [125, 60]),
    "C3": ("C-4u7", "Power", [110, 30]),
    "C4": ("C-10u", "Power", [140, 30]),
    "R6": ("R-1M", "Power", [60, 15]),
    "R7": ("R-1M", "Power", [80, 15]),
    "C9": ("C-100n", "Power", [95, 15]),
    # USB.
    "J1": ("USB-C", "USB", [190, 50]),
    "R1": ("R-5k1", "USB", [230, 70]),
    "R2": ("R-5k1", "USB", [245, 70]),
    # MCU.
    "U1": ("E73-2G4M08S1C", "MCU", [60, 170]),
    "C5": ("C-100n", "MCU", [20, 200]),
    "C6": ("C-10u", "MCU", [30, 200]),
    "J5": ("SWD-PADS", "MCU", [125, 140]),
    # LoRa.
    "U2": ("E22-900M22S", "LoRa", [220, 170]),
    "C7": ("C-100n", "LoRa", [180, 200]),
    "C8": ("C-10u", "LoRa", [190, 200]),
    "J3": ("U.FL", "LoRa", [270, 150]),
    # GPS.
    "J4": ("GPS-HDR", "GPS", [390, 170]),
    "Q1": ("AO3401A", "GPS", [340, 200]),
    "R8": ("R-100k", "GPS", [330, 175]),
    "C10": ("C-1u", "GPS", [365, 205]),
    # UI.
    "SW2": ("TACT", "UI", [300, 50]),
    "SW3": ("TACT", "UI", [340, 50]),
    "D2": ("LED-GREEN", "UI", [380, 30]),
    "R5": ("R-1k", "UI", [380, 60]),
}

# Nets: name -> pins. Power rails get power ports on the schematic; the rest are wired within a
# section and labelled where they leave it.
RAILS = {
    "GND": ["U1.5", "U1.21", "U2.1", "U2.2", "U2.3", "U2.4", "U2.5", "U2.10", "U2.11", "U2.12", "U2.20", "U2.22",
            "J1.A1", "J1.B1", "J2.2", "U3.2", "U4.2", "C1.2", "C2.2", "C3.2", "C4.2", "C5.2",
            "C6.2", "C7.2", "C8.2", "C9.2", "C10.2", "R1.2", "R2.2", "R3.2", "R7.2", "SW2.3", "SW3.3", "J3.2", "J3.3", "J4.2",
            "J5.4", "D2.1"],
    "VBUS": ["J1.A4", "J1.B4", "U4.4", "C1.1", "U1.27", "R4.1"],
    "VBAT": ["U4.3", "J2.1", "C2.1", "SW1.1", "R6.1"],
    "VSYS": ["SW1.2", "U3.1", "U3.3", "C3.1"],
    "3V3": ["U3.5", "C4.1", "U1.19", "U1.23", "C5.1", "C6.1", "U2.9", "C7.1", "C8.1", "Q1.2", "R8.1", "J5.1"],
    "3V3_GPS": ["Q1.3", "J4.1", "C10.1"],
}
SIGNALS = {
    "CC1": ["J1.A5", "R1.1"],
    "CC2": ["J1.B5", "R2.1"],
    "USB_DP": ["J1.A6", "J1.B6", "U1.31"],
    "USB_DN": ["J1.A7", "J1.B7", "U1.29"],
    "CHG_LED": ["U4.1", "D1.1"],
    "CHG_LED_A": ["R4.2", "D1.2"],
    "PROG": ["U4.5", "R3.1"],
    "GPS_PWR_N": ["Q1.1", "R8.2", "U1.43"],
    "LORA_NSS": ["U2.19", "U1.11"],
    "LORA_SCK": ["U2.18", "U1.13"],
    "LORA_MOSI": ["U2.17", "U1.15"],
    "LORA_MISO": ["U2.16", "U1.17"],
    "LORA_RST": ["U2.15", "U1.33"],
    "LORA_BUSY": ["U2.14", "U1.35"],
    "LORA_DIO1": ["U2.13", "U1.10"],
    "LORA_RXEN": ["U2.6", "U1.9"],
    "LORA_TXEN": ["U2.7", "U2.8"],
    "RF_ANT": ["U2.21", "J3.1"],
    "SWDIO": ["U1.37", "J5.3"],
    "SWCLK": ["U1.39", "J5.2"],
    "NRESET": ["U1.26", "SW3.1"],
    "BTN_USER": ["U1.7", "SW2.1"],
    "LED_STAT": ["U1.8", "R5.1"],
    "LED_STAT_A": ["R5.2", "D2.2"],
    "VBAT_SENSE": ["R6.2", "R7.1", "C9.1", "U1.4"],
    "GPS_TX": ["J4.3", "U1.2"],
    "GPS_RX": ["J4.4", "U1.6"],
    "GPS_PPS": ["J4.5", "U1.1"],
    "GPS_EN": ["J4.6", "U1.3"],
}
NO_CONNECT = ["U1.24", "J1.S1", "J1.S2", "J1.S3", "J1.S4", "U1.12", "U1.14", "U1.16", "U1.18", "U1.20", "U1.22", "U1.25", "U1.28", "U1.30", "U1.32", "U1.34", "U1.36",
              "U1.38", "U1.40", "U1.41", "U1.42", "J1.A8", "J1.B8", "J2.MP1", "J2.MP2", "SW1.3", "SW1.MP1", "SW1.MP2",
              "SW1.MP3", "SW1.MP4", "SW2.2", "SW2.4", "SW3.2", "SW3.4", "U3.4"]

# Ground pads of small parts get a short track to a via of their own, so each reaches the bottom
# pour whatever the top pour does between the tracks.
GND_ESCAPES = ["U1.5", "U1.21", "J5.4", "U2.1", "U2.11", "U2.12", "U2.20", "U2.22", "J3.2", "J3.3", "D2.1", "C1.2", "C2.2", "C3.2", "C4.2", "C5.2", "C6.2", "C7.2", "C8.2", "C9.2", "C10.2",
               "R1.2", "R2.2", "R3.2", "R7.2", "SW2.3", "SW3.3", "U3.2", "U4.2", "J2.2"]

# The USB-C's ground pads have no room for a via between its shell's slots and pegs: each is tied by
# a short track to the ground pad of the CC resistor beside it, which has one.
GND_LINKS = [("J1.A1", "R1.2"), ("J1.B1", "R2.2")]

POWER_WIDTH = 0.4
SIGNAL_WIDTH = 0.2


def section_of(pin):
    return PARTS[pin.split(".")[0]][1]
