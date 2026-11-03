"""The LoRa Mesh Node Mini's parts: footprints, symbols and components, as SolidCraft commands.

Footprint sources, so each can be checked:

- E22-900M22S: Ebyte "E22-900M22S User Manual" v1.3, section 3 (size and pin definition): 14 x 20 mm,
  22 stamp holes at 1.27 mm, bottom pad 0.9 x 0.8 mm, pin 1 1.00 mm and pin 11 2.00 mm from the ends.
  The land runs 0.6 mm past the module's edge for the solder fillet.
- E73-2G4M08S1C: Ebyte "E73-2G4M08S1C User Manual" v1.7, section 3: 13 x 18 mm, 28 stamp holes at
  1.27 mm (outer pads 1.0 x 0.8 mm, the first top pad 2.06 mm from each side, the first side pad
  2.60 mm from the top, 3.97 mm from the last side pad to the antenna end) and 15 inner pads
  0.8 x 0.8 mm. The inner pads' places are not dimensioned in the drawing: they are scaled from it
  (about +-0.1 mm) and marked VERIFY.
- SOT-23-5, SOT-23, 0603: generated from the JEDEC/EIA packages by ecad.footprint.generate.
- TYPE-C-31-M-12 (USB-C), S2B-PH-SM4-TB (JST PH), MSK12C02 (slide switch), TS-1187A (tact switch),
  U.FL-R-SMT-1(10): VERIFY. Their datasheets could not be read while this example was built (the
  part pages were out of reach); the lands below are from the manufacturers' published patterns
  as remembered, and must be checked against each datasheet before ordering.
"""

# Pads: (number, [x, y], [w, h], options). Positions in mm, the footprint's top view, y up.


def _e22():
    """E22-900M22S, top view, pin 1 at the bottom right (the IPEX corner is the bottom left)."""
    ys = {1: -9.0, 2: -7.73, 3: -6.46, 4: -0.89, 5: 0.38, 6: 1.65, 7: 2.92, 8: 4.19, 9: 5.46, 10: 6.73, 11: 8.0}
    pads = []
    for n, y in ys.items():
        pads.append({"number": str(n), "at": [6.85, y], "size": [1.5, 0.8]})
    left = {12: 8.0, 13: 6.73, 14: 5.46, 15: 4.19, 16: 2.92, 17: 1.65, 18: 0.38, 19: -0.89, 20: -6.46, 21: -7.73, 22: -9.0}
    for n, y in left.items():
        pads.append({"number": str(n), "at": [-6.85, y], "size": [1.5, 0.8]})
    body = [[-7, -10], [7, 10]]
    return pads, body


def _e73():
    """E73-2G4M08S1C, top view, the ceramic antenna at the bottom (y -9 .. about -5.5)."""
    pads = []
    side = [6.4 - 1.27 * k for k in range(10)]  # pin 10 (right) / 26 (left) at the top
    for k, y in enumerate(side):
        pads.append({"number": str(10 - k), "at": [6.3, round(y, 3)], "size": [1.6, 0.8]})
    left = [26, 27, 29, 31, 33, 35, 37, 39, 41, 43]
    for n, y in zip(left, side):
        pads.append({"number": str(n), "at": [-6.3, round(y, 3)], "size": [1.6, 0.8]})
    top = [11, 13, 15, 17, 19, 21, 23, 25]
    for k, n in enumerate(top):
        pads.append({"number": str(n), "at": [round(4.445 - 1.27 * k, 3), 8.8], "size": [0.8, 1.6]})
    # VERIFY: the inner pads' places are scaled from the drawing.
    for k, n in enumerate([12, 14, 16, 18, 20, 22, 24]):
        pads.append({"number": str(n), "at": [round(3.81 - 1.27 * k, 3), 7.07], "size": [0.8, 0.8]})
    for k, n in enumerate([28, 30, 32, 34, 36, 38, 40, 42]):
        pads.append({"number": str(n), "at": [-4.49, round(4.495 - 1.27 * k, 3)], "size": [0.8, 0.8]})
    body = [[-6.5, -9], [6.5, 9]]
    return pads, body


def _usb_c():
    """TYPE-C-31-M-12 (16-pin USB-C receptacle), opening to +y at the board's edge. VERIFY."""
    pads = []
    row = [("A1", -3.2, 0.6), ("A4", -2.4, 0.6), ("B8", -1.75, 0.3), ("A5", -1.25, 0.3), ("B7", -0.75, 0.3), ("A6", -0.25, 0.3),
           ("A7", 0.25, 0.3), ("B6", 0.75, 0.3), ("A8", 1.25, 0.3), ("B5", 1.75, 0.3), ("B4", 2.4, 0.6), ("B1", 3.2, 0.6)]
    for n, x, w in row:
        pads.append({"number": n, "at": [x, -0.05], "size": [w, 1.15]})
    # Shell: four plated slots, tied to nothing (left unconnected: no shield net on this board).
    for k, (x, y) in enumerate([(-4.32, 0.53), (4.32, 0.53), (-4.32, -3.67), (4.32, -3.67)]):
        h = 2.0 if y > 0 else 1.6
        pads.append({"number": f"S{k + 1}", "at": [x, y], "size": [1.0, h + 0.6], "shape": "oval", "kind": "through_hole", "drill": [0.6, h]})
    # Two locating pegs, unplated.
    for k, x in enumerate([-2.89, 2.89]):
        pads.append({"number": f"H{k + 1}", "at": [x, -1.7], "size": [0.65, 0.65], "shape": "round", "kind": "hole", "drill": 0.65})
    body = [[-4.47, -6.1], [4.47, 1.25]]
    return pads, body


def _jst_ph2():
    """S2B-PH-SM4-TB (JST PH, 2 pins, side entry), the opening to +y. VERIFY."""
    pads = [
        {"number": "1", "at": [-1.0, -2.0], "size": [1.0, 3.5]},
        {"number": "2", "at": [1.0, -2.0], "size": [1.0, 3.5]},
        {"number": "MP1", "at": [-3.35, 1.4], "size": [1.6, 3.0]},
        {"number": "MP2", "at": [3.35, 1.4], "size": [1.6, 3.0]},
    ]
    body = [[-3.95, -0.5], [3.95, 3.1]]
    return pads, body


def _slide():
    """MSK12C02 (SPDT slide switch), the lever to +y. VERIFY."""
    pads = [{"number": str(k + 1), "at": [x, -1.6], "size": [0.7, 1.5]} for k, x in enumerate([-1.5, 0.0, 1.5])]
    for k, (x, y) in enumerate([(-3.4, -1.1), (3.4, -1.1), (-3.4, 1.1), (3.4, 1.1)]):
        pads.append({"number": f"MP{k + 1}", "at": [x, y], "size": [1.0, 0.8]})
    body = [[-3.35, -0.75], [3.35, 0.75]]
    return pads, body


def _tact():
    """TS-1187A (5.1 x 5.1 mm tact switch): pins 1 and 2 joined, 3 and 4 joined. VERIFY."""
    pads = [{"number": n, "at": [x, y], "size": [1.0, 0.75]} for n, x, y in [("1", -3.0, 1.85), ("2", 3.0, 1.85), ("3", -3.0, -1.85), ("4", 3.0, -1.85)]]
    body = [[-2.55, -2.55], [2.55, 2.55]]
    return pads, body


def _ufl():
    """U.FL-R-SMT-1(10), the signal pin at the bottom. VERIFY."""
    pads = [
        {"number": "1", "at": [0.0, -1.5], "size": [1.0, 1.05]},
        {"number": "2", "at": [-1.475, 0.0], "size": [1.05, 2.2]},
        {"number": "3", "at": [1.475, 0.0], "size": [1.05, 2.2]},
    ]
    body = [[-1.3, -1.3], [1.3, 1.3]]
    return pads, body


def _sot23_5():
    """SOT-23-5 (JEDEC MO-178 AA: 2.9 x 1.6 mm body, 0.95 mm pitch, 2.8 mm lead span): pins 1-3 along
    the bottom from the left, 4 and 5 along the top from the right."""
    pads = [{"number": str(n), "at": [x, y], "size": [0.6, 1.1]} for n, x, y in
            [(1, -0.95, -1.2), (2, 0.0, -1.2), (3, 0.95, -1.2), (4, 0.95, 1.2), (5, -0.95, 1.2)]]
    body = [[-1.45, -0.8], [1.45, 0.8]]
    return pads, body


def _swd():
    """Four 1.5 mm test pads at 2.54 mm: 3V3, SWDIO, SWCLK, GND (pogo pins, no part)."""
    pads = [{"number": str(k + 1), "at": [round(-3.81 + 2.54 * k, 2), 0.0], "size": [1.5, 1.5], "shape": "round"} for k in range(4)]
    body = [[-4.7, -0.9], [4.7, 0.9]]
    return pads, body


CUSTOM = {
    "E22-900M22S": _e22,
    "E73-2G4M08S1C": _e73,
    "USB-C-16P": _usb_c,
    "JST-PH-2-SMD": _jst_ph2,
    "SLIDE-SPDT": _slide,
    "TACT-5.1": _tact,
    "U.FL": _ufl,
    "SWD-PADS-4": _swd,
    "SOT-23-5": _sot23_5,
}


def _line(a, b):
    return {"type": "line", "points": [a, b], "width": 0.15}


# Silkscreen polarity and pin-1 marks: a short line beside pin 1 (or the cathode), outside the
# pads by the silk-to-pad gap.
SILK = {
    "SWD-PADS-4": [_line([-4.4, -1.2], [-3.2, -1.2])],
    "TACT-5.1": [_line([-3.5, 2.7], [-2.5, 2.7]), _line([-2.0, 2.7], [2.0, 2.7]), _line([-2.0, -2.7], [2.0, -2.7])],
    "SLIDE-SPDT": [_line([-1.85, -2.7], [-1.15, -2.7])],
    "SOT-23-5": [_line([-1.6, -1.9], [-1.6, -0.5])],
    "JST-PH-2-SMD": [_line([-1.5, -4.2], [-0.5, -4.2])],
    "U.FL": [_line([-0.5, -2.4], [0.5, -2.4])],
    "USB-C-16P": [_line([-4.6, -5.8], [4.6, -5.8])],
}


def footprint_commands():
    """Commands making the footprints: generated packages, then the custom ones with a fab outline,
    a silkscreen outline and a courtyard."""
    out = [
        ("ecad.footprint.generate", {"footprint": "0603", "package": "chip", "size": "0603"}),
        ("ecad.footprint.generate", {"footprint": "LED0603", "package": "chip", "size": "0603"}),
        ("ecad.footprint.set_graphics", {"footprint": "LED0603", "layer": "silkscreen", "graphics": [_line([-1.35, -0.5], [-1.35, 0.5])]}),
        ("ecad.footprint.generate", {"footprint": "SOT-23", "package": "sot23", "pins": 3}),
        ("ecad.footprint.generate", {"footprint": "HDR-1x6", "package": "header", "pins": 6}),
        ("ecad.footprint.set_graphics", {"footprint": "HDR-1x6", "layer": "silkscreen", "graphics": [
            {"type": "rect", "from": [-1.32, -7.62], "to": [1.32, 7.62], "width": 0.15}, _line([-1.65, 5.6], [-1.65, 7.1])]}),
    ]
    for name, make in CUSTOM.items():
        pads, (lo, hi) = make()
        out.append(("ecad.footprint.create", {"name": name}))
        out.append(("ecad.pad.add", {"footprint": name, "pads": pads}))
        out.append(("ecad.footprint.set_graphics", {"footprint": name, "layer": "fab", "graphics": [{"type": "rect", "from": lo, "to": hi, "width": 0.1}]}))
        if name in SILK:
            out.append(("ecad.footprint.set_graphics", {"footprint": name, "layer": "silkscreen", "graphics": SILK[name]}))
        out.append(("ecad.footprint.courtyard", {"footprint": name, "margin": 0.25}))
    return out


def pads_of(name):
    """A custom footprint's pads, for the router (the generated ones come from the design)."""
    return CUSTOM[name]()[0]
