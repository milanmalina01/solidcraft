#!/usr/bin/env python3
"""Build the LoRa Mesh Node Mini with SolidCraft's commands, headless, through solidcraft-cli.

    python3 examples/lora-mini/build.py            # uses target/ci/solidcraft-cli, else target/release
    SOLIDCRAFT_CLI=/path/to/solidcraft-cli python3 examples/lora-mini/build.py

Phase 1 makes the library, draws the schematic in its sections, sets up the board (JLCPCB 2-layer
profile, outline, holes, the E73's antenna keep-out, design intent) and places the parts; it saves
out/phase1.sc and reads the pads back. Phase 2 routes the board with the small grid router in
route.py (SolidCraft has no autorouter command: each route becomes an ecad.board.track), then
pours ground on both sides, stitches it where the ground map says it is weak, tidies the
names, stamps the board marking, runs the checks and writes the PDF and the JLCPCB files into
out/. The checks' results go to out/report.json.
"""

import json
import math
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
OUT = os.path.join(HERE, "out")
sys.path.insert(0, HERE)

import circuit  # noqa: E402
import parts  # noqa: E402
import route  # noqa: E402

BOARD_W, BOARD_H = 46.0, 30.0

# Board placement: designator -> (at, rotation). Board origin in its middle, y up.
PLACE = {
    "U1": ([-13.6, -2.85], 270),  # the antenna end 0.4 mm in from the left edge
    "U2": ([7.5, -4.7], 0),  # low, so its SPI pins are nearly level with U1's across the channel
    "C5": ([-2.6, -0.4], 270),  # its 3V3 pad level with U1.23, its ground beside U1.21
    "C6": ([-2.6, 2.5], 90),  # above U1.23
    "C7": ([16.45, 1.0], 270),  # beside U2.9
    "C8": ([16.45, 3.75], 90),
    "J1": ([-10.0, 7.85], 0),  # the USB-C opening at the top edge
    "R1": ([-14.8, 5.85], 180),
    "R2": ([-7.45, 5.85], 0),  # R2.1 right under J1.B5: CC2 straight down, VBUS from B4 goes right
    "J5": ([-17.5, 9.8], 90),
    "Q1": ([-20.8, 6.6], 90),  # the GPS switch by U1.43 and J5.1, its drain straight down the left side to J4.1
    "R8": ([-20.8, 9.1], 0),
    "C10": ([-20.8, 10.55], 0),
    "SW2": ([-1.1, 11.9], 180),  # the buttons along the top edge, above U2
    "SW3": ([6.55, 11.9], 180),
    "R4": ([-3.65, 7.4], 270),  # the charger's row, under the buttons and over U2
    "D1": ([-2.25, 7.4], 270),
    "C1": ([-0.85, 7.4], 270),
    "R3": ([0.55, 7.4], 90),  # beside U4.5 (PROG)
    "U4": ([3.3, 7.4], 90),  # VBUS in from the left, VBAT out to the right
    "U3": ([17.2, 12.9], 0),  # above the slide switch, VSYS up from SW1.2
    "C4": ([14.6, 12.6], 90),
    "C3": ([17.55, 8.7], 90),
    "SW1": ([21.25, 6.4], 270),  # the lever out of the right edge
    "C2": ([18.6, 0.75], 0),
    "J2": ([19.63, -4.8], 270),  # the battery plug out of the right edge, below the slide switch
    "R6": ([18.85, -11.0], 270),
    "R7": ([17.45, -11.0], 90),
    "C9": ([16.05, -11.0], 90),
    "J4": ([-11.85, -13.45], 90),  # 6 pins along the bottom edge, under U1's GPS pins, pin 1 on the left
    "R5": ([6.2, 7.4], 90),  # the status LED beside the buttons
    "D2": ([7.7, 7.4], 90),
}
HOLES = [[-20.7, 12.7], [20.7, 12.7], [-20.7, -12.7], [20.7, -12.7]]


# Routes drawn by hand before the router starts, so the channel between the modules stays open and
# the bottom plane in one piece. 3V3 round U1: U1.23 and U1.19 joined past U1.21, with C5 and C6
# beside them, and a bottom run under U1's top pads to J5 (the router joins C6.1 to the rest). RST
# and BUSY on the bottom beside that run, up into the channel. DIO1 to the far pin of the three
# (U2.13): along the bottom under U2's pads, past the end of the SCK line, so it walls off neither
# U2.14 and U2.15 nor the plane under U1.
PRE_ROUTES = {
    "3V3": {
        "anchor": "C6.1",
        "pins": ["U1.23", "U1.19", "C5.1", "J5.1"],
        "tracks": [
            [("top", [[-4.8, 0.35], [-2.6, 0.35]])],
            [("top", [[-3.65, 0.35], [-3.65, -2.2], [-4.8, -2.2]])],
            [("top", [[-3.65, 1.75], [-2.6, 1.75]])],
            [("top", [[-3.65, 0.35], [-3.65, 3.25]]), ("bottom", [[-3.65, 3.25], [-16.0, 3.25], [-17.5, 4.75]]), ("top", [[-17.5, 4.75], [-17.5, 6.0]])],
        ],
    },
    "LORA_BUSY": {
        "pins": ["U2.14"],
        "tracks": [[("top", [[-13.55, 3.45], [-13.55, 4.75]]), ("bottom", [[-13.55, 4.75], [-12.6, 3.8], [-1.1, 3.8]]),
                    ("top", [[-1.1, 3.8], [-1.1, 1.2], [-0.65, 0.75], [0.65, 0.75]])]],
    },
    "LORA_RST": {
        "pins": ["U2.15"],
        "tracks": [[("top", [[-12.3, 3.45], [-12.3, 4.55], [-12.1, 4.75]]), ("bottom", [[-12.1, 4.75], [-11.5, 4.15], [-3.8, 4.15], [-3.55, 4.4]]),
                    ("top", [[-3.55, 4.4], [-3.05, 4.9], [-2.05, 4.9], [-1.65, 4.5], [-1.65, -0.1], [-1.25, -0.5], [0.65, -0.5]])]],
    },
    "LORA_DIO1": {
        "pins": ["U2.13"],
        "tracks": [[("top", [[-7.2, -9.15], [-7.2, -8.6], [-3.9, -8.6]]), ("bottom", [[-3.9, -8.6], [1.1, -8.6], [1.1, 1.6], [0.05, 2.65], [-0.55, 2.65]]),
                    ("top", [[-0.55, 2.65], [0.05, 2.05], [0.65, 2.05]])]],
    },
}


# What the router leaves, drawn by hand on its result: SWCLK and NRESET on the bottom, BAT sense
# along the bottom edge under U2, and C6's ground up to a via of its own.
TENTED = {"protection": "tented"}
HAND_ROUTES = [
    {"layer": "top", "width": 0.15, "points": [[-17.5, 8.55], [-18.45, 7.6]], "via": TENTED, "legs": [
        {"layer": "bottom", "points": [[-18.45, 7.6], [-16.55, 5.7], [-16.55, 4.75]], "via": TENTED},
        {"layer": "top", "points": [[-16.55, 4.75], [-16.1, 4.3], [-16.1, 3.45]]}]},
    {"layer": "top", "width": 0.15, "points": [[-7.2, 3.45], [-7.2, 4.45], [-7.0, 4.7]], "via": TENTED, "legs": [
        {"layer": "bottom", "points": [[-7.0, 4.7], [-6.9, 4.6], [-5.1, 4.6], [-4.9, 4.8], [-4.9, 6.5], [1.2, 12.6], [8.7, 12.6], [8.7, 10.9]], "via": TENTED},
        {"layer": "top", "points": [[8.7, 10.9], [9.55, 10.05]]}]},
    {"layer": "top", "width": 0.15, "points": [[-14.85, -9.15], [-14.55, -9.45], [-14.55, -10.35]], "via": TENTED, "legs": [
        {"layer": "bottom", "points": [[-14.55, -10.35], [-8.55, -10.35], [-6.6, -12.3], [15.55, -12.3], [15.85, -12.6]], "via": TENTED},
        {"layer": "top", "points": [[15.85, -12.6], [15.85, -12.05]]}]},
    {"layer": "top", "width": 0.3, "points": [[-2.6, 3.25], [-2.6, 4.35]], "via": TENTED},
]


# The header's pin-1 bar on the side that stays on the board with pin 1 at the bottom-left corner.
PIN1_BAR = ("ecad.footprint.set_graphics", {"footprint": "HDR-1x6", "layer": "silkscreen", "graphics": [
    {"type": "rect", "from": [-1.32, -7.62], "to": [1.32, 7.62], "width": 0.15},
    {"type": "line", "points": [[1.65, 5.6], [1.65, 7.1]], "width": 0.15}]})
NAME_EDIT = "ecad.board.edit"


def stitch_areas():
    """Where the ground is stitched under parts: (rectangle, pitch)."""
    (ux, uy), (vx, vy) = PLACE["U1"][0], PLACE["U2"][0]
    jx, jy = PLACE["J1"][0]
    bx, by = PLACE["J2"][0]
    return [
        ([[ux - 5.3, uy - 5.2], [ux - 3.8, uy + 1.9]], 3.5),  # U1's two sides, clear of the marking on the back
        ([[ux + 5.15, uy - 5.2], [ux + 6.25, uy + 2.6]], 3.5),
        ([[ux + 6.2, uy + 4.75], [ux + 6.8, uy + 5.35]], 1.0),  # U1's corner by U1.26
        ([[jx - 1.5, jy - 3.4], [jx + 3.6, jy - 1.5]], 1.3),  # the copper between VBUS and RST under the USB-C
        ([[-17.0, -12.5], [-4.5, -10.2]], 2.4),  # the row above J4
        ([[vx - 5.6, vy - 9.6], [vx + 5.6, vy + 9.6]], 5.0),  # U2
        ([[jx - 4.4, jy + 0.6], [jx + 4.4, jy + 6.6]], 4.5),  # the USB-C's body
        ([[PLACE["SW2"][0][0] - 2.4, PLACE["SW2"][0][1] - 2.0], [PLACE["SW3"][0][0] + 2.4, PLACE["SW3"][0][1] + 2.0]], 4.5),  # the buttons
        ([[bx - 0.2, by - 4.2], [bx + 3.2, by + 4.2]], 4.5),  # the battery plug
    ]


def cli():
    exe = os.environ.get("SOLIDCRAFT_CLI")
    if exe:
        return exe
    for profile in ("ci", "release", "debug"):
        p = os.path.join(ROOT, "target", profile, "solidcraft-cli")
        if os.path.exists(p):
            return p
    sys.exit("no solidcraft-cli: build it (cargo build --profile ci -p solidcraft-cli) or set SOLIDCRAFT_CLI")


def run(script, name, start=None, save=None):
    """Run a command script; the results of every command, in order."""
    path = os.path.join(OUT, f"{name}.json")
    with open(path, "w") as f:
        json.dump({"commands": [{"command": c, "params": p} for c, p in script]}, f, indent=1)
    out = os.path.join(OUT, f"{name}-results.json")
    args = [cli(), "run", path, "--results", "--json-out", out, "--quiet"]
    if start:
        args += ["--in", start]
    if save:
        args += ["--save", save]
    r = subprocess.run(args, capture_output=True, text=True)
    if r.returncode != 0:
        sys.stderr.write(r.stdout[-4000:] + r.stderr[-4000:])
        sys.exit(f"{name} failed")
    with open(out) as f:
        return json.load(f)


def results_of(reply):
    """The list of command results whatever shape --results wraps them in."""
    if isinstance(reply, dict):
        for k in ("results", "steps"):
            if isinstance(reply.get(k), list):
                return reply[k]
    return reply


# Practical quality findings this prototype keeps, with why.
APPROVE_QUALITY = {
    "pi.stitching": "A small bottom pocket by the USB-C (between VBUS and RST, tied by R2's ground via) rises 0.54 mV against the 0.5 mV target; the QR code on U1's back leaves an 11 mm via-free circle against 5.8 mm (silk breaks on vias); the board has no RF line.",
    "dfm.pad_to_pour": "Solid joins kept for the ground's sake: the board is reflowed by the assembler, and J4 (hand-fitted, not assembled) takes a hotter iron.",
}

# Practical design-rule findings this prototype keeps, with why; the board's other findings are fixed.
APPROVE = {
    "fab_class": "JLCPCB's standard via, 0.3 mm hole in a 0.6 mm pad, at the profile's least drill and ring: the via their standard price is made for.",
    "track_width_class": "The nRF52840's VBUS pin only detects USB (microamps): its branch of VBUS is a 0.15 mm signal track by design.",
    "acute_angle": "Narrow angles (38 to 67 degrees) where a track meets a pad or track of its own net, from the example's grid router and hand routes; kept for this prototype, to tidy by hand for a production run.",
    "pad_entry": "Thin tracks leaving a pad near its corner, from the example's grid router; kept for this prototype, to tidy by hand for a production run.",
    "via_near_pad": "A tented via next to a pad: the mask covers the via, so no solder wicks into it.",
}


def schematic_commands(force_labels=()):
    out = [("ecad.schematic.place", {"parts": [
        {"component": comp, "at": at, "designator": d} for d, (comp, _, at) in sorted(circuit.PARTS.items())
    ]})]
    for title, rect in circuit.SECTIONS.items():
        out.append(("ecad.schematic.frame", {"title": title, "rect": rect}))
    ports = []
    for rail, pins in circuit.RAILS.items():
        for pin in pins:
            ports.append({"name": rail, "at": pin})
    out.append(("ecad.schematic.power", {"ports": ports}))
    # Wires between small parts in one section; labels at the modules' and connectors' dense pin
    # rows (a wire run along such a row would join the pins it passes).
    small = {"RES", "CAP", "LED", "PMOS", "TACT", "LDO", "CHARGER"}
    symbol = {c[0]: c[3] for c in circuit.COMPONENTS}
    nets = []
    for name, pins in circuit.SIGNALS.items():
        sections = {circuit.section_of(p) for p in pins}
        local = len(sections) == 1 and name not in force_labels and all(symbol[circuit.PARTS[p.split(".")[0]][0]] in small for p in pins)
        nets.append({"pins": pins, "name": name, "by": "wires" if local else "labels"})
    out.append(("ecad.schematic.connect", {"nets": nets}))
    out.append(("ecad.schematic.marker", {"markers": [{"kind": "no_connect", "at": p} for p in circuit.NO_CONNECT]}))
    out.append(("ecad.schematic.edit", {"parts": [{"part": d, "dnp": True} for d in circuit.NOT_ASSEMBLED]}))
    return out


def board_setup_commands():
    hw, hh = BOARD_W / 2, BOARD_H / 2
    out = [
        ("ecad.board.profile", {"fab": "jlcpcb_2layer"}),
        ("ecad.board.outline", {"rect": [BOARD_W, BOARD_H]}),
        ("ecad.board.stack", {"copper_layers": 2, "thickness": 1.6}),
        # 0.15 mm signal tracks and gaps (6/6 mil, inside JLCPCB's 2-layer standard of 0.10/0.10 at
        # no extra cost): the room the two modules' fanouts need on two layers.
        ("ecad.board.rules", {"track_width": circuit.SIGNAL_WIDTH, "clearance": circuit.CLEARANCE}),
        ("ecad.board.netclass", {"name": "Power", "nets": list(circuit.RAILS.keys() - {"GND"}), "track_width": circuit.POWER_WIDTH}),
        ("ecad.board.sync", {}),
    ]
    for d, (at, rot) in sorted(PLACE.items()):
        out.append(("ecad.board.move", {"items": [d], "to": at, "rotation": rot}))
    for at in HOLES:
        out.append(("ecad.board.hole", {"at": at, "drill": 2.2}))
    # The E73's antenna: no copper under it or beside it, on either side.
    x0 = -hw
    y0 = PLACE["U1"][0][1]  # beside U1's antenna end, wherever U1 sits
    out.append(("ecad.board.keepout", {"points": [[x0, y0 - 7.6], [x0 + 3.9, y0 - 7.6], [x0 + 3.9, y0 + 7.6], [x0, y0 + 7.6]],
                                       "forbid": {"tracks": True, "vias": True, "pours": True, "components": False}}))
    out.append(("ecad.design.info", {"title": "LoRa Mesh Node Mini", "revision": "A", "date": "2026-11-02", "author": "SolidCraft example",
                                     "notes": "nRF52840 (E73) + SX1262 (E22, EU868), USB-C charging, 1S LiPo, GPS header"}))
    out.append(("ecad.intent.set", {
        "nets": [
            {"net": "GND", "role": "ground"},
            {"net": "3V3", "role": "power", "voltage": {"nominal": 3.3}, "current_a": 0.3},
            {"net": "VBUS", "role": "power", "voltage": {"nominal": 5.0}, "current_a": 0.5},
            {"net": "VBAT", "role": "power", "voltage": {"nominal": 3.7, "peak": 4.2}, "current_a": 0.5},
            {"net": "VSYS", "role": "power", "voltage": {"nominal": 3.7, "peak": 4.2}, "current_a": 0.3},
            {"net": "3V3_GPS", "role": "power", "voltage": {"nominal": 3.3}, "current_a": 0.05},
            {"net": "VBAT_SENSE", "role": "analog", "note": "the battery's voltage through 1M/1M to AIN4"},
        ],
        "board": {"product": "prototype", "bands": [
            {"name": "EU868", "lo_mhz": 863, "hi_mhz": 870},
            {"name": "BLE 2.4 GHz", "lo_mhz": 2400, "hi_mhz": 2483.5},
        ]},
        "note": "From the product's brief: an EU868 LoRa radio and a 2.4 GHz BLE radio.",
        "accept_low_risk": True,
    }))
    return out


def phase1(force_labels=()):
    script = [("file.new", {"kind": "electronics_design", "name": "lora-mini"})]
    script += parts.footprint_commands() + [PIN1_BAR]
    script += circuit.symbol_commands()
    script += circuit.component_commands()
    script += schematic_commands(force_labels)
    script += board_setup_commands()
    script.append(("ecad.intent.inspect", {"blocking": True, "limit": 200}))
    for fp in ["0603", "LED0603", "SOT-23-5", "SOT-23", "HDR-1x6"] + list(parts.CUSTOM):
        script.append(("ecad.footprint.inspect", {"footprint": fp}))
    script.append(("ecad.board.inspect", {}))
    return script


def library():
    """The parts as an electronics library of their own (lora-mini-library.sc)."""
    script = [("file.new", {"kind": "electronics_library", "name": "lora-mini-library"})]
    script += parts.footprint_commands() + [PIN1_BAR] + circuit.symbol_commands() + circuit.component_commands()
    run(script, "library", save=os.path.join(HERE, "lora-mini-library.sc"))


def main():
    os.makedirs(OUT, exist_ok=True)
    library()
    phase1_sc = os.path.join(OUT, "phase1.sc")
    want = {}
    for net, pins in list(circuit.RAILS.items()) + list(circuit.SIGNALS.items()):
        for pin in pins:
            want[pin] = net
    force = set()
    for _ in range(6):
        res = results_of(run(phase1(sorted(force)), "phase1", save=phase1_sc))
        board = res[-1]
        # The board's nets must be the circuit's, pad for pad: a schematic wire that ran over
        # another pin joined it, so that net is drawn with labels instead, and again.
        wrong = []
        for f in board["footprints"]:
            for p in f["pads"]:
                ref = f"{f['designator']}.{p['number']}"
                if ref in want and p.get("net") != want[ref]:
                    wrong.append((ref, p.get("net"), want[ref]))
        if not wrong:
            break
        force |= {w for _, got, w in wrong} | {got for _, got, _ in wrong if got}
        print("relabelling", sorted(force))
    else:
        sys.exit("the schematic's nets differ from the circuit's: " + ", ".join(f"{r}: {g} (want {w})" for r, g, w in wrong))
    footprints = {}
    for r in res:
        v = r.get("result", r) if isinstance(r, dict) else r
        if isinstance(v, dict) and isinstance(v.get("footprint"), dict) and isinstance(v["footprint"].get("pads"), list):
            footprints[v["footprint"]["name"]] = v["footprint"]["pads"]
    # The routes, worked out again only when the board or the router changed.
    import hashlib
    shape = {k: board.get(k) for k in ("footprints", "holes", "keepouts", "outline")}
    key = hashlib.sha256((json.dumps(shape, sort_keys=True) + json.dumps(PRE_ROUTES, sort_keys=True) + open(os.path.join(HERE, "route.py")).read()
                          + open(os.path.join(HERE, "circuit.py")).read()).encode()).hexdigest()[:16]
    cache = os.path.join(OUT, f"routes-{key}.json")
    if os.path.exists(cache):
        with open(cache) as f:
            tracks = [tuple(t) for t in json.load(f)]
    else:
        tracks = route.route_board(board, footprints, circuit, passes=1, pre=PRE_ROUTES)
        with open(cache, "w") as f:
            json.dump(tracks, f)
    # The intent's proposals that only want a look, confirmed; decoupling assigned by hand.
    proposals = next((r for r in res if isinstance(r, dict) and "proposals" in r and "coverage" in r), {}).get("proposals", [])
    mine = {"C1", "C5", "C6", "C7", "C8"}
    accept = [p["key"] for p in proposals if set(p.get("needs", [])) <= {"confirm"} and p["key"].split("|")[-1] not in mine]
    script = [("ecad.intent.set", {"accept": accept, "parts": [
        {"part": "C5", "role": "decoupling", "of": ["U1.19"]}, {"part": "C6", "role": "decoupling", "of": ["U1.23"]},
        {"part": "C7", "role": "decoupling", "of": ["U2.9"]}, {"part": "C8", "role": "bulk", "of": ["U2.9"]},
        {"part": "C1", "role": "decoupling", "of": ["U4.4", "U1.27"]},
        {"part": "J4", "role": "connector", "cable": True}], "note": "The decoupling as placed; proposals confirmed after a look. J4 is the GPS header of the product's brief: a GPS module's cable plugs into it."})]
    script += list(tracks) + [("ecad.board.track", t) for t in HAND_ROUTES]
    hw, hh = BOARD_W / 2, BOARD_H / 2
    for layer in ("top", "bottom"):
        script.append(("ecad.board.pour", {"layer": layer, "net": "GND", "rect": [BOARD_W - 0.2, BOARD_H - 0.2], "connect": "solid"}))
    # Under the modules first, tented (the top copper there has no other tie but the modules' edges
    # and is the weakest on the board), and under the other large parts, whose bodies would
    # otherwise leave holes wider than lambda/10 in the grid, then over the rest.
    for area, pitch in stitch_areas():
        script.append(("ecad.board.stitch", {"net": "GND", "pattern": "grid", "region": {"rect": area}, "under_parts": True, "pitch": pitch}))
    script += [
        ("ecad.board.stitch", {"net": "GND", "pattern": "grid"}),
        ("ecad.board.stitch", {"net": "GND", "pattern": "needed", "budget": 40}),
        ("ecad.board.labels", {"mode": "fit"}),
        # Where "fit" put names on tracks or nearer another part: to the assembly drawing with the rest.
        (NAME_EDIT, {"footprints": [{"footprint": "U2", "show_name": False}, {"footprint": "C3", "show_name": False}, {"footprint": "C4", "show_name": False}]}),
        # On the back of U1, the one clear place: the text over the QR code (no serial box or date:
        # they fit nowhere between the tracks; the drawings carry the date).
        ("ecad.board.marking", {"side": "bottom", "at": [-19.15, 3.65], "number": "01",
                                "template": {"title": False, "date": False, "part_number": "LMN-MINI-{number}-{revision}", "serial_box": False, "qr": False}}),
        ("ecad.board.marking", {"side": "bottom", "at": [-7.5, -0.25], "number": "01",
                                "template": {"title": False, "revision": False, "date": False, "part_number": "", "serial_box": False, "qr": True, "qr_text": "LMN-MINI-01-A"}}),
    ]
    # A first rule check; the clearance errors it can fix by narrowing a track (the router's
    # corners inside a pad) are fixed with the check's own suggestion, then everything is checked.
    mid = os.path.join(OUT, "phase2.sc")
    both = results_of(run(script + [("ecad.drc.inspect", {"limit": 2000}), ("ecad.quality.inspect", {"limit": 2000})], "phase2a", start=phase1_sc, save=mid))
    first, quality = [x.get("result", x) if isinstance(x, dict) else x for x in both[-2:]]
    fixes = []
    for f in first.get("findings", []):
        if f.get("severity") == "error" and f.get("code") == "clearance":
            for fx in f.get("fix", []):
                if fx.get("command") == "ecad.board.track.edit" and fx.get("params"):
                    fixes.append(("ecad.board.track.edit", fx["params"]))
                    break
    if fixes:
        print("fixing", len(fixes), "clearance errors with the check's own suggestion")
    # The practical findings the example keeps, each kind approved with why (see the README).
    approvals = []
    for code, note in APPROVE.items():
        keys = [f["key"] for f in first.get("findings", []) if f.get("code") == code and f.get("severity") == "warning"
                and (code != "via_near_pad" or "tented" in f.get("message", ""))]
        if keys:
            approvals.append(("ecad.drc.approve", {"keys": keys, "note": note}))
    for code, note in APPROVE_QUALITY.items():
        keys = [f["key"] for f in quality.get("findings", []) if f.get("code") == code and f.get("severity") == "warning"]
        if keys:
            approvals.append(("ecad.quality.approve", {"keys": keys, "note": note}))
    script = fixes + approvals + [
        ("ecad.drc.inspect", {}),
        ("ecad.quality.inspect", {}),
        ("ecad.erc.inspect", {}),
        ("ecad.board.check", {"stage": "release", "limit": 40}),
        ("ecad.board.export.jlc", {"path": os.path.join(OUT, "lora-mini-jlcpcb.zip"), "stem": "lora-mini"}),
        ("ecad.board.verify_outputs", {"path": os.path.join(OUT, "lora-mini-jlcpcb.zip")}),
        ("ecad.export.pages", {"format": "pdf", "path": os.path.join(OUT, "lora-mini.pdf")}),
    ]
    final = os.path.join(HERE, "lora-mini.sc")
    res2 = results_of(run(script, "phase2", start=mid, save=final))
    tail = [r.get("result", r) if isinstance(r, dict) else r for r in res2[-8:]]
    names = ["drc", "quality", "erc", "check", "jlc", "verify", "pdf"]
    report = dict(zip(names, tail[-7:]))
    with open(os.path.join(OUT, "report.json"), "w") as f:
        json.dump(report, f, indent=1)
    drc = report["drc"].get("counts", {})
    erc = report["erc"].get("counts", {})
    print(f"DRC {drc}  ERC {erc}  verified {report['verify'].get('ok')}")


if __name__ == "__main__":
    main()
