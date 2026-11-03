#!/usr/bin/env python3
"""Build the LoRa Mesh Node Mini with SolidCraft's commands, headless, through solidcraft-cli.

    python3 examples/lora-mini/build.py            # uses target/ci/solidcraft-cli, else target/release
    SOLIDCRAFT_CLI=/path/to/solidcraft-cli python3 examples/lora-mini/build.py

Phase 1 makes the library, draws the schematic in its sections, sets up the board (JLCPCB 2-layer
profile, outline, holes, the E73's antenna keep-out, design intent) and places the parts; it saves
out/phase1.sc and reads the pads back. Phase 2 routes the board with the small grid router in
route.py (SolidCraft has no autorouter command: each route becomes an ecad.board.track), then
pours ground on both sides, stitches it (where needed, and a fence along the RF line), tidies the
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

BOARD_W, BOARD_H = 52.0, 34.0

# Board placement: designator -> (at, rotation). Board origin in its middle, y up.
PLACE = {
    "U1": ([-16.6, -1.0], 270),  # the antenna end 0.4 mm in from the left edge
    "U2": ([5.0, 1.0], 0),
    "J3": ([-4.4, -11.6], 180),  # below the E22, the signal pin up
    "C5": ([-6.0, -0.4], 90),  # beside U1.19, upright to leave the channel free
    "C6": ([-3.6, 5.0], 90),
    "C7": ([14.6, 5.6], 270),
    "C8": ([14.6, 2.6], 90),
    "J1": ([-13.0, 14.7], 0),  # the USB-C opening at the top edge
    "R1": ([-17.2, 7.5], 180),
    "R2": ([-9.2, 7.5], 0),
    "J5": ([-19.8, 11.3], 90),
    "U4": ([15.0, 14.2], 0),
    "C1": ([15.6, 11.4], 0),
    "R3": ([15.6, 9.6], 0),
    "R4": ([15.6, 7.8], 0),
    "D1": ([18.6, 7.8], 0),
    "J2": ([21.5, 13.0], 0),  # the battery plug from the top edge
    "C2": ([21.8, 8.15], 0),
    "SW1": ([24.0, 3.0], 270),  # the lever out of the right edge
    "U3": ([19.0, 3.0], 0),
    "C3": ([19.0, 6.0], 0),
    "C4": ([19.0, 0.0], 0),
    "R6": ([15.5, -2.1], 90),
    "R7": ([15.5, -5.1], 90),
    "C9": ([17.1, -3.6], 90),
    "J4": ([-14.75, -14.5], 270),  # 6 pins along the bottom edge, under U1's GPS pins
    "Q1": ([-9.0, -10.3], 0),
    "R8": ([-11.6, -10.4], 90),
    "C10": ([-6.1, -14.5], 90),
    "SW2": ([10.0, -13.4], 0),
    "SW3": ([17.5, -13.4], 0),
    "R5": ([0.5, -13.5], 0),
    "D2": ([3.3, -13.5], 180),
}
HOLES = [[-23.6, 14.6], [23.6, -14.6], [-23.6, -14.6], [8.5, 14.6]]


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
    return out


def board_setup_commands():
    hw, hh = BOARD_W / 2, BOARD_H / 2
    out = [
        ("ecad.board.profile", {"fab": "jlcpcb_2layer"}),
        ("ecad.board.outline", {"rect": [BOARD_W, BOARD_H]}),
        ("ecad.board.stack", {"copper_layers": 2, "thickness": 1.6}),
        ("ecad.board.netclass", {"name": "Power", "nets": list(circuit.RAILS.keys() - {"GND"}), "track_width": circuit.POWER_WIDTH}),
        ("ecad.board.sync", {}),
    ]
    for d, (at, rot) in sorted(PLACE.items()):
        out.append(("ecad.board.move", {"items": [d], "to": at, "rotation": rot}))
    for at in HOLES:
        out.append(("ecad.board.hole", {"at": at, "drill": 2.2}))
    # The E73's antenna: no copper under it or beside it, on either side.
    x0 = -hw
    out.append(("ecad.board.keepout", {"points": [[x0, -8.6], [x0 + 3.9, -8.6], [x0 + 3.9, 6.6], [x0, 6.6]],
                                       "forbid": {"tracks": True, "vias": True, "pours": True, "components": False}}))
    # Under the RF line the bottom stays ground: no other net's track or via (the pour stays).
    out.append(("ecad.board.keepout", {"layers": ["bottom"], "points": [[-5.9, -11.0], [-0.9, -11.0], [-0.9, -5.8], [-5.9, -5.8]],
                                       "forbid": {"tracks": True, "vias": True, "pours": False, "components": False}}))
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
            {"net": "RF_ANT", "role": "rf", "f_mhz": 868, "z_target": 50},
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
    script += parts.footprint_commands()
    script += circuit.symbol_commands()
    script += circuit.component_commands()
    script += schematic_commands(force_labels)
    script += board_setup_commands()
    script.append(("ecad.intent.inspect", {"blocking": True, "limit": 200}))
    script.append(("ecad.board.impedance.inspect", {"layer": "top", "kind": "coplanar", "target": 50, "gap": 0.25}))
    for fp in ["0603", "LED0603", "SOT-23-5", "SOT-23", "HDR-1x6"] + list(parts.CUSTOM):
        script.append(("ecad.footprint.inspect", {"footprint": fp}))
    script.append(("ecad.board.inspect", {}))
    return script


def library():
    """The parts as an electronics library of their own (lora-mini-library.sc)."""
    script = [("file.new", {"kind": "electronics_library", "name": "lora-mini-library"})]
    script += parts.footprint_commands() + circuit.symbol_commands() + circuit.component_commands()
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
    imp = None
    for r in res:
        v = r.get("result", r) if isinstance(r, dict) else r
        if isinstance(v, dict) and isinstance(v.get("footprint"), dict) and isinstance(v["footprint"].get("pads"), list):
            footprints[v["footprint"]["name"]] = v["footprint"]["pads"]
        if isinstance(v, dict) and "z0" in v and "gap" in v:
            imp = v
    rf_width = round(imp["width"], 2) if imp and imp.get("valid_range", False) else 1.0
    # The routes, worked out again only when the board or the router changed.
    import hashlib
    shape = {k: board.get(k) for k in ("footprints", "holes", "keepouts", "outline")}
    key = hashlib.sha256((json.dumps(shape, sort_keys=True) + open(os.path.join(HERE, "route.py")).read()
                          + open(os.path.join(HERE, "circuit.py")).read() + str(rf_width)).encode()).hexdigest()[:16]
    cache = os.path.join(OUT, f"routes-{key}.json")
    if os.path.exists(cache):
        with open(cache) as f:
            tracks = [tuple(t) for t in json.load(f)]
    else:
        tracks = route.route_board(board, footprints, rf_width, circuit)
        with open(cache, "w") as f:
            json.dump(tracks, f)
    # The intent's proposals that only want a look, confirmed; decoupling assigned by hand.
    proposals = next((r for r in res if isinstance(r, dict) and "proposals" in r and "coverage" in r), {}).get("proposals", [])
    mine = {"C1", "C5", "C6", "C7", "C8"}
    accept = [p["key"] for p in proposals if set(p.get("needs", [])) <= {"confirm"} and p["key"].split("|")[-1] not in mine]
    script = [("ecad.intent.set", {"accept": accept, "parts": [
        {"part": "C5", "role": "decoupling", "of": ["U1.19"]}, {"part": "C6", "role": "decoupling", "of": ["U1.23"]},
        {"part": "C7", "role": "decoupling", "of": ["U2.9"]}, {"part": "C8", "role": "bulk", "of": ["U2.9"]},
        {"part": "C1", "role": "decoupling", "of": ["U4.4", "U1.27"]}], "note": "The decoupling as placed; proposals confirmed after a look."})]
    script += list(tracks)
    hw, hh = BOARD_W / 2, BOARD_H / 2
    for layer in ("top", "bottom"):
        script.append(("ecad.board.pour", {"layer": layer, "net": "GND", "rect": [BOARD_W - 0.2, BOARD_H - 0.2], "connect": "solid"}))
    script += [
        ("ecad.board.stitch", {"net": "GND", "pattern": "fence", "nets": ["RF_ANT"]}),
        ("ecad.board.stitch", {"net": "GND", "pattern": "needed"}),
        ("ecad.board.labels", {"mode": "fit"}),
        ("ecad.board.marking", {"side": "bottom", "at": [-12.0, 8.0], "number": "01",
                                "template": {"part_number": "LMN-MINI-{number}-{revision}", "serial_box": True, "qr": True}}),
    ]
    # A first rule check; the clearance errors it can fix by narrowing a track (the router's
    # corners inside a pad) are fixed with the check's own suggestion, then everything is checked.
    mid = os.path.join(OUT, "phase2.sc")
    first = results_of(run(script + [("ecad.drc.inspect", {})], "phase2a", start=phase1_sc, save=mid))[-1]
    first = first.get("result", first) if isinstance(first, dict) else first
    fixes = []
    for f in first.get("findings", []):
        if f.get("severity") == "error" and f.get("code") == "clearance":
            for fx in f.get("fix", []):
                if fx.get("command") == "ecad.board.track.edit" and fx.get("params"):
                    fixes.append(("ecad.board.track.edit", fx["params"]))
                    break
    if fixes:
        print("fixing", len(fixes), "clearance errors with the check's own suggestion")
    script = fixes + [
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
