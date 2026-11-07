# PCB layout: the process for a board that matters

Several blind starting placements are scored by code and the weak ones dropped, one layout is finished, specialists review it, and an auditor attacks the evidence. Start after the schematic stage (`solidcraft-schematic`) passes. A simple board may use one start, the scorer, one review and the audit; say so in the evidence.

## Inputs

- `requirements.json` and the merged circuit
- each part's `board_rules` from `solidcraft-datasheet`
- `constraints.json`: what code can decide about a placement, in the format below, each rule with its `source`. Every `board_rules` line and every layout requirement becomes a constraint or is named for one specialist. A number no page prints is marked as a choice.
- a base design: schematic drawn, board synced, nothing placed

## Steps

1. **Starts.** 3 or 4 agents, each blind to the others, each with one priority, in the brief below. Each writes `place.json` and may run the scorer as often as it needs.
2. **Score.** `place_tool.py compare 2 */score.json`. A hard failure is out. Keep two. Starts within 5% of the second also stay. Weights are not calibrated, so report the raw numbers and don't trust a small gap.
3. **Route** every kept start, using `reference/routing-dfm.md`. Compare what the scorer only guesses: unrouted connections, via count, power width kept, RF line length. Pick one and write why. An idea from a dropped start can be applied to it only if the result is scored again.
   The scorer can't see routability, so let the router answer. When connections stay open: first check the router itself (does it use every layer it may, are its via and clearance margins the real rules, do ground vias go in first?), then give one placer the open connections and let it move parts and route again, then add a layer, and only then a larger board. Draw by hand, before routing, the links a fine-pitch connector's pads can't untangle (a USB-C's A and B rows). When all connections route, keep those tracks and fix the rest on them: another routing run will not give the same board.
4. **Finish** the chosen one through stages 5 to 7 of `workflow.md`, until `ecad.board.check {"stage":"release"}` has no block. Any approval carries its reason. Critical approvals wait for the user.
5. **Specialists.** One fresh agent per row. They read and report; they change nothing.
6. **Settle.** Check each finding on its page or with a data command, because findings can be wrong. Fix the real ones, rerun the gate, then send the changed area back to that specialist.
7. **Planted faults.** Copy the board, plant 5 or 6 faults, and give a fresh reviewer that copy. Leave no note that contradicts a fault, and nothing beside the copy that lists them: the scripts stay in another folder. Report how many were found. A miss becomes a rubric line or a check.
8. **Audit.** The brief below. The auditor can't fix anything.
9. **Record** in `verification/board.json`: starts and their scores, what was dropped and why, the route comparison, reviews, faults found, audits, approvals, and one row per requirement with its layout evidence. `status` is `not_checked` until the user signs off.
10. **Every mistake that escaped** becomes a constraint, a check or a rubric line.

## Specialists

| Specialist | Reads | Answers with |
|---|---|---|
| EMC and RF | `rf-emc.md`, `signal-integrity.md` | return paths and ground under RF, noisy parts against GPS, ESD path or the stated choice, antenna leads |
| Power | `power-ground.md` | each rail's current against width and vias (`ecad.board.current.inspect`), pulse loops, part heat, decoupling (`ecad.board.loop.inspect`) |
| Manufacturing | `manufacturing.md`, `routing-dfm.md`, `silkscreen-assembly.md` | fab limits, paste and stencil as the maker prints, part rotations in the placement file, reflow limits, read-back of the outputs |
| Mechanical fit | outline, the makers' drawings | every plug and tool's room, connector edges, overhangs, tallest part, what sits on the back, the product's fit |
| Visual | `review.md` sections 3 to 5, pictures only | flow, tidiness, labels |

Each answers `yes`, `no` or `unknown` per item, with objects and the command or page that shows it. Limit findings to what could change the board or a decision.

Things that have slipped past checks, for the specialists to look at by name: a heat pad with no vias (a stitch command can report success and place none); paste on parts not fitted; capacitor voltage ratings against what the table assumes, and their value under bias; a checker's current figure against each branch's real load; connector names on the silk, above all two plugs of the same size; pin marks clipped by the board's edge; socket positions of a module's own antennas, and which is which; the fab's stack-up, and whether the design is sensitive to it.

## Briefs

**Placer.** "Place every part of this board. Your priority: PRIORITY. You see the requirements, the circuit, the constraints and the footprint sizes; other placers are working blind. Read `reference/placement.md`. Keep to whole-part rotations. Write `place.json`, run `place_tool.py run`, and fix hard failures first. Stop when your last few changes no longer help. Return the score, the file, and in a few lines what you gave up for your priority."

Priorities, one each:
- RF and sensitive parts first, the rest around them
- power and pulse currents first: shortest high-current paths
- connectors and fit first: smallest outline, cables that leave cleanly
- shortest wiring and fewest crossings

**Auditor.** "Try to show this layout's evidence is wrong or incomplete. For each requirement: does the cited evidence show it, on the board as it is now? Rerun the data commands rather than reading the reports. Check that no finding was lowered by removing copper, that every approval's reason holds, that the board's netlist is the merged circuit's, and that each constraint matches its source. You can't fix anything. Limit findings to what could change the board or a decision. Return `breaks`, `risks`, and what you didn't check."

## constraints.json

- `outline_max`: `[w, h]`
- `back_allowed`: parts allowed on the back
- `plane_nets`: nets left out of wiring length
- `net_weights`: net to multiplier on its length
- `opens`: part to `{dir, origin_to_edge | origin_to_edge_max, clear, source}`. `dir` is the way it opens at rotation 0 (`-y`). It may overhang that edge only. `clear` is a zone in front of it that no part may enter.
- `near`: `{a, b, limit, hard, kind, weight, source}`, pads (`U2.11`) or parts; `kind` is `electrical` or `em`
- `far`: `{a, b, min, hard, weight, source}`, parts, gap between their boxes
- `rf_lines`, `rf_net`: lines no other connection should cross

`place.json`: `{"outline":[w,h],"parts":{"U1":[x,y,rotation]},"why":"..."}`. A part left out stays off the board and fails.

## place_tool.py

Scores: **hard** is overlap, outside, back side, edge and clear zones, hard limits. **Soft** is `electrical` (wiring length, crossings, nearness), `em` (RF and pulse nearness, separation, RF crossings), `fit` (area). It doesn't know part heights, the fill, or routes: those belong to the route step and the specialists. `ecad.board.check {"stage":"place"}` is saved beside it as `check.json`.

```python
#!/usr/bin/env python3
"""Score starting placements. Same code for every start, no judgement in it.

run CLI BASE.sc PLACE.json CONSTRAINTS.json OUTDIR   apply a placement headlessly, score it
score BOARD.json CONSTRAINTS.json                    score a board.inspect reply
compare KEEP SCORE.json...                           rank scores; a hard failure is out
"""
import json
import math
import os
import subprocess
import sys

VEC = {"-y": (0, -1), "+y": (0, 1), "-x": (-1, 0), "+x": (1, 0)}


def turned(d, rot):
    x, y = VEC[d]
    a = math.radians(rot)
    return (round(x * math.cos(a) - y * math.sin(a)), round(x * math.sin(a) + y * math.cos(a)))


def boxes_hit(a, b):
    return a[0][0] < b[1][0] and b[0][0] < a[1][0] and a[0][1] < b[1][1] and b[0][1] < a[1][1]


def box_gap(a, b):
    dx = max(a[0][0] - b[1][0], b[0][0] - a[1][0], 0)
    dy = max(a[0][1] - b[1][1], b[0][1] - a[1][1], 0)
    return math.hypot(dx, dy)


def crosses(p, q, r, s):
    def side(a, b, c):
        return (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    return side(p, q, r) * side(p, q, s) < 0 and side(r, s, p) * side(r, s, q) < 0


def score(board, con):
    parts = {f["designator"]: f for f in board["footprints"]}
    pads = {f"{f['designator']}.{p['number']}": p["at"] for f in board["footprints"] for p in f["pads"]}
    xs = [p[0] for p in board["outline"]]
    ys = [p[1] for p in board["outline"]]
    x0, x1, y0, y1 = min(xs), max(xs), min(ys), max(ys)
    hard, soft = [], {"electrical": 0.0, "em": 0.0, "fit": 0.0}
    detail = {}
    opens = con.get("opens", {})

    w, h = x1 - x0, y1 - y0
    lim = con.get("outline_max")
    if lim and (w > lim[0] + 1e-6 or h > lim[1] + 1e-6):
        hard.append(f"outline {w:g} x {h:g} is over {lim[0]:g} x {lim[1]:g}")
    detail["outline"] = [round(w, 2), round(h, 2)]
    soft["fit"] = w * h

    for ref, f in sorted(parts.items()):
        (a, b), (c, d) = f["bbox"]
        if f["side"] != "top" and ref not in con.get("back_allowed", []):
            hard.append(f"{ref} is on the back")
        over = {"-x": a < x0, "+x": c > x1, "-y": b < y0, "+y": d > y1}
        for side, out in over.items():
            if not out:
                continue
            if ref in opens and turned(opens[ref]["dir"], f["rotation"]) == VEC[side]:
                continue
            hard.append(f"{ref} is outside the outline at {side}")

    names = sorted(parts)
    for i, m in enumerate(names):
        for n in names[i + 1:]:
            if parts[m]["side"] == parts[n]["side"] and boxes_hit(parts[m]["bbox"], parts[n]["bbox"]):
                hard.append(f"{m} overlaps {n}")

    for ref, o in sorted(opens.items()):
        f = parts.get(ref)
        if not f:
            hard.append(f"{ref} is missing")
            continue
        dx, dy = turned(o["dir"], f["rotation"])
        (a, b), (c, d) = f["bbox"]
        to_edge = {(-1, 0): f["at"][0] - x0, (1, 0): x1 - f["at"][0], (0, -1): f["at"][1] - y0, (0, 1): y1 - f["at"][1]}[(dx, dy)]
        if "origin_to_edge" in o and abs(to_edge - o["origin_to_edge"]) > 0.05:
            hard.append(f"{ref} origin is {to_edge:.2f} from the edge it opens to; {o['origin_to_edge']:g} wanted ({o.get('source', '')})")
        if "origin_to_edge_max" in o and to_edge > o["origin_to_edge_max"]:
            hard.append(f"{ref} origin is {to_edge:.2f} from the edge it opens to; {o['origin_to_edge_max']:g} at most ({o.get('source', '')})")
        reach = o.get("clear", 0)
        if reach:
            front = {(-1, 0): [[a - reach, b], [a, d]], (1, 0): [[c, b], [c + reach, d]], (0, -1): [[a, b - reach], [c, b]], (0, 1): [[a, d], [c, d + reach]]}[(dx, dy)]
            for n, g in sorted(parts.items()):
                if n != ref and g["side"] == f["side"] and boxes_hit(front, g["bbox"]):
                    hard.append(f"{n} is in {ref}'s {reach:g} mm clear zone ({o.get('source', '')})")

    def where(name):
        if name in pads:
            return pads[name]
        if name in parts:
            return parts[name]["at"]
        hard.append(f"{name} is missing")
        return None

    near = []
    for r in con.get("near", []):
        p, q = where(r["a"]), where(r["b"])
        if not p or not q:
            continue
        dist = math.dist(p, q)
        near.append({"a": r["a"], "b": r["b"], "mm": round(dist, 2), "limit": r.get("limit")})
        if r.get("hard") and dist > r["limit"]:
            hard.append(f"{r['a']} to {r['b']} is {dist:.2f}; {r['limit']:g} at most ({r.get('source', '')})")
        soft[r.get("kind", "electrical")] += r.get("weight", 1) * dist
    detail["near"] = near

    far = []
    for r in con.get("far", []):
        f, g = parts.get(r["a"]), parts.get(r["b"])
        if not f or not g:
            hard.append(f"{r['a']} or {r['b']} is missing")
            continue
        gap = box_gap(f["bbox"], g["bbox"])
        short = max(0.0, r["min"] - gap)
        far.append({"a": r["a"], "b": r["b"], "gap": round(gap, 2), "min": r["min"]})
        if r.get("hard") and short > 0:
            hard.append(f"{r['a']} to {r['b']} gap is {gap:.2f}; {r['min']:g} at least ({r.get('source', '')})")
        soft[r.get("kind", "em")] += r.get("weight", 1) * short
    detail["far"] = far

    skip = set(con.get("plane_nets", ["GND"]))
    weight = con.get("net_weights", {})
    wires = [a for a in board["airwires"] if a["net"] not in skip]
    length = sum(weight.get(a["net"], 1) * math.dist(a["from"], a["to"]) for a in wires)
    cross = sum(1 for i, a in enumerate(wires) for b in wires[i + 1:] if a["net"] != b["net"] and crosses(a["from"], a["to"], b["from"], b["to"]))
    detail["airwire_mm"], detail["crossings"] = round(length, 1), cross
    soft["electrical"] += length + con.get("crossing_cost", 5) * cross

    rf_cross = 0
    for a, b in con.get("rf_lines", []):
        p, q = where(a), where(b)
        if p and q:
            rf_cross += sum(1 for c in wires if c["net"] != con.get("rf_net") and crosses(p, q, c["from"], c["to"]))
    detail["rf_crossings"] = rf_cross
    soft["em"] += con.get("rf_crossing_cost", 10) * rf_cross

    return {"ok": not hard, "hard": hard, "soft": {k: round(v, 1) for k, v in soft.items()}, "detail": detail}


def run(cli, base, place_path, con_path, out):
    os.makedirs(out, exist_ok=True)
    place = json.load(open(place_path))
    cmds = [{"command": "ecad.board.outline", "params": {"rect": place["outline"]}}]
    for ref, (x, y, rot) in sorted(place["parts"].items()):
        cmds.append({"command": "ecad.board.move", "params": {"items": [ref], "to": [x, y], "rotation": rot}})
    cmds += [{"command": "ecad.board.check", "params": {"stage": "place", "limit": 60}},
             {"command": "ecad.board.inspect", "params": {}}]
    script = os.path.join(out, "script.json")
    json.dump({"commands": cmds}, open(script, "w"))
    res = os.path.join(out, "results.json")
    r = subprocess.run([cli, "run", script, "--in", base, "--results", "--json-out", res, "--quiet", "--save", os.path.join(out, "placed.sc")],
                       stdin=subprocess.DEVNULL, capture_output=True, text=True)
    if r.returncode:
        sys.exit(r.stdout[-2000:] + r.stderr[-2000:])
    got = json.load(open(res))
    got = got.get("results", got.get("steps", got)) if isinstance(got, dict) else got
    pick = lambda x: x.get("result", x) if isinstance(x, dict) else x
    check, board = pick(got[-2]), pick(got[-1])
    json.dump(board, open(os.path.join(out, "board.json"), "w"))
    json.dump(check, open(os.path.join(out, "check.json"), "w"), indent=1)
    s = score(board, json.load(open(con_path)))
    json.dump(s, open(os.path.join(out, "score.json"), "w"), indent=1)
    return s


def compare(keep, paths):
    rows = [(p, json.load(open(p))) for p in paths]
    good = [r for r in rows if r[1]["ok"]]
    for p, s in rows:
        if not s["ok"]:
            print(f"OUT  {p}: {len(s['hard'])} hard: {s['hard'][0]}")
    if not good:
        sys.exit("no placement passes")
    aspects = list(good[0][1]["soft"])
    best = {a: max(min(s["soft"][a] for _, s in good), 1e-9) for a in aspects}
    ranked = sorted(good, key=lambda r: sum(r[1]["soft"][a] / best[a] for a in aspects))
    for i, (p, s) in enumerate(ranked):
        total = sum(s["soft"][a] / best[a] for a in aspects)
        print(f"{'KEEP' if i < keep else 'DROP'} {p}: {total:.2f}  " + "  ".join(f"{a} {s['soft'][a]:g}" for a in aspects))


if __name__ == "__main__":
    a = sys.argv
    if a[1:2] == ["run"]:
        print(json.dumps(run(*a[2:7]), indent=1))
    elif a[1:2] == ["score"]:
        print(json.dumps(score(json.load(open(a[2])), json.load(open(a[3]))), indent=1))
    elif a[1:2] == ["compare"]:
        compare(int(a[2]), a[3:])
    else:
        sys.exit(__doc__)
```
