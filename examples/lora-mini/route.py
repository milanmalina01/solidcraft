"""A small two-layer grid router for the example, so the board's tracks come from SolidCraft's
ecad.board.track commands (SolidCraft has no autorouter command of its own).

The board is a 0.05 mm grid on each layer. Every piece of copper marks the cells it covers with its
net, and the cells within the clearance plus the track's half width round it as taken by that net;
the board's edge band, the keep-outs, the holes and the modules' bodies (on the top) are taken by
none. A net may go through free cells and its own. Each connection is an A* search over both layers
(45 degree steps, a turn costs a little, a via a cost of its own, both layers free round it), from a
pad to the copper its net already has. The path is then pulled tight: each run of it is replaced by
the fewest straight 0, 45 and 90 degree segments that keep to the same free cells, so the tracks
have no stair steps or short jogs. Each route becomes one ecad.board.track with legs and vias. USB
D- is drawn to run beside D+ as a pair. When connections fail, the nets that failed are ripped up
and routed first on the next pass, in a few orders, and the best pass is kept.
"""

import heapq
import math

G = 0.05  # grid step, mm
CLEARANCE = 0.15
WIDEST = 0.4
MARGIN = CLEARANCE + WIDEST / 2 + G / 2
# What a signal track (0.15 mm) needs round other copper: the clearance, its half width, a hair.
SIGNAL_MARGIN = CLEARANCE + 0.075 + G / 2
EDGE = 0.3 + WIDEST / 2 + G
# A signal track's centre from an unplated hole: the edge clearance, half the width, half a cell.
HOLE_SIG = 0.3 + 0.075 + G / 2
VIA_COST = 50.0
# A 45 degree turn and a sharper one, in cells: enough to prefer long straight runs.
TURN_45 = 3.0
TURN_90 = 8.0
# A cell beside the pair partner's keep-out costs this share of a step (D- hugs D+).
PAIR_SHARE = 0.55
# The second line of each pair, and the first it runs beside.
PAIRS = {"USB_DN": "USB_DP"}
# The cells round a via that must be free: its 0.3 mm radius and a 0.15 mm hair past a pad.
VIA_REACH = range(-int(0.45 / G), int(0.45 / G) + 1)
MULTI = 65535


class Board:
    def __init__(self, w, h):
        self.x0, self.y0 = -w / 2, -h / 2
        self.nx, self.ny = int(round(w / G)) + 1, int(round(h / G)) + 1
        self.block = [[0] * (self.nx * self.ny) for _ in range(2)]
        # The same for signal tracks, which need less room.
        self.block_sig = [[0] * (self.nx * self.ny) for _ in range(2)]
        self.own = [[0] * (self.nx * self.ny) for _ in range(2)]
        # Pad cells, either layer: no via goes in or next to a pad.
        self.pad = bytearray(self.nx * self.ny)
        # Vias placed so far, as cells: holes keep their distance.
        self.vias = []
        self.nets = {}
        self.signal = False

    def net(self, name):
        if name not in self.nets:
            self.nets[name] = len(self.nets) + 1
        return self.nets[name]

    def cell(self, x, y):
        return int(round((x - self.x0) / G)), int(round((y - self.y0) / G))

    def xy(self, i, j):
        return round(self.x0 + i * G, 4), round(self.y0 + j * G, 4)

    def idx(self, i, j):
        return j * self.nx + i

    def inside(self, i, j):
        return 0 <= i < self.nx and 0 <= j < self.ny

    def mark(self, layer, test, box, net, margin, sig=None):
        """Cells whose middle `test(x, y, margin)` says is in: `own` for margin 0, else `block` (and
        `block_sig` within the signal margin). Copper keeps its own net, whatever comes near it."""
        (x0, y0), (x1, y1) = box
        i0, j0 = self.cell(x0 - margin - G, y0 - margin - G)
        i1, j1 = self.cell(x1 + margin + G, y1 + margin + G)
        sig = min(margin, SIGNAL_MARGIN) if sig is None else min(margin, sig)
        for j in range(max(j0, 0), min(j1, self.ny - 1) + 1):
            for i in range(max(i0, 0), min(i1, self.nx - 1) + 1):
                x, y = self.xy(i, j)
                if not test(x, y, margin):
                    continue
                k = self.idx(i, j)
                if margin == 0:
                    if net != MULTI:
                        self.own[layer][k] = net
                    for g in (self.block, self.block_sig):
                        g[layer][k] = net
                    continue
                if self.own[layer][k] and self.own[layer][k] != MULTI:
                    continue
                b = self.block[layer][k]
                self.block[layer][k] = net if b in (0, net) else MULTI
                if test(x, y, sig):
                    b = self.block_sig[layer][k]
                    self.block_sig[layer][k] = net if b in (0, net) else MULTI

    def passable(self, layer, k, net):
        g = self.block_sig if self.signal else self.block
        return g[layer][k] in (0, net)


def rect_test(c, half, angle):
    ca, sa = math.cos(math.radians(angle)), math.sin(math.radians(angle))

    def test(x, y, m):
        dx, dy = x - c[0], y - c[1]
        u, v = dx * ca + dy * sa, -dx * sa + dy * ca
        ex, ey = max(abs(u) - half[0], 0.0), max(abs(v) - half[1], 0.0)
        return math.hypot(ex, ey) <= m + 1e-9

    return test


def pad_test(c, half, angle, shape):
    """A pad's outline: a circle, an oval (a stadium) or a rectangle, turned."""
    if shape == "round":
        r = min(half)
        return lambda x, y, m: math.hypot(x - c[0], y - c[1]) <= r + m + 1e-9
    if shape == "oval":
        ca, sa = math.cos(math.radians(angle)), math.sin(math.radians(angle))
        r = min(half)
        hx, hy = half[0] - r, half[1] - r
        a = [c[0] - hx * ca + hy * sa, c[1] - hx * sa - hy * ca]
        b = [c[0] + hx * ca - hy * sa, c[1] + hx * sa + hy * ca]
        return seg_test(a, b, r)
    return rect_test(c, half, angle)


def seg_test(a, b, r):
    def test(x, y, m):
        ax, ay, bx, by = a[0], a[1], b[0], b[1]
        dx, dy = bx - ax, by - ay
        L = dx * dx + dy * dy
        t = 0.0 if L == 0 else max(0.0, min(1.0, ((x - ax) * dx + (y - ay) * dy) / L))
        return math.hypot(x - ax - t * dx, y - ay - t * dy) <= r + m + 1e-9

    return test


def box_of(points, r):
    xs, ys = [p[0] for p in points], [p[1] for p in points]
    return (min(xs) - r, min(ys) - r), (max(xs) + r, max(ys) + r)


def claim(b, layer, test, box, net):
    """The copper's cells, taken by its net in both grids (not its own: see add_copper)."""
    (x0, y0), (x1, y1) = box
    i0, j0 = b.cell(x0 - G, y0 - G)
    i1, j1 = b.cell(x1 + G, y1 + G)
    for j in range(max(j0, 0), min(j1, b.ny - 1) + 1):
        for i in range(max(i0, 0), min(i1, b.nx - 1) + 1):
            if test(*b.xy(i, j), 0.0):
                k = b.idx(i, j)
                for g in (b.block, b.block_sig):
                    g[layer][k] = net


def add_copper(b, layer, test, box, net, width=None):
    """Copper of `net`: taken round it, and its own a hair inside, so an end there joins it."""
    b.mark(layer, test, box, net, MARGIN)
    claim(b, layer, test, box, net)
    if width is not None:
        return
    b.mark(layer, test, box, net, 0.0)


def add_track(b, layer, a, c, width, net):
    box = box_of([a, c], width / 2)
    b.mark(layer, seg_test(a, c, width / 2), box, net, MARGIN)
    claim(b, layer, seg_test(a, c, width / 2), box, net)
    b.mark(layer, seg_test(a, c, max(width / 2 - G, G / 2)), box, net, 0.0)


def pad_shapes(board, footprints):
    """Each pad on the board: (designator.number, net, centre, half size, angle, layers)."""
    out = []
    for f in board["footprints"]:
        lib = {p["number"]: p for p in footprints.get(f["footprint"], [])}
        for p in f["pads"]:
            q = lib.get(p["number"], {})
            size = q.get("size", [0.6, 0.6])
            angle = (f["rotation"] + q.get("rotation", 0)) % 360
            th = q.get("kind") in ("through_hole",)
            hole = q.get("kind") == "hole"
            layers = [0, 1] if th else ([] if hole else [0])
            out.append((f"{f['designator']}.{p['number']}", p.get("net"), p["at"], [size[0] / 2, size[1] / 2], angle, layers, q))
    return out


def route_board(board, footprints, circuit, passes=10):
    w, h = board["size"] if isinstance(board.get("size"), list) else (52.0, 34.0)
    b = Board(w, h)
    pads = pad_shapes(board, footprints)
    # The modules' bodies: no top-layer copper under them but their own pads.
    for f in board["footprints"]:
        if f["footprint"] in ("E73-2G4M08S1C", "E22-900M22S"):
            (x0, y0), (x1, y1) = f["bbox"][0], f["bbox"][1]
            core = rect_test([(x0 + x1) / 2, (y0 + y1) / 2], [(x1 - x0) / 2 - 1.3, (y1 - y0) / 2 - 1.3], 0)
            b.mark(0, core, ((x0, y0), (x1, y1)), MULTI, 0.0)
    for ref, net, at, half, angle, layers, q in pads:
        n = b.net(net) if net else b.net("__" + ref)
        r = math.hypot(*half)
        shape = q.get("shape", "rect")
        for l in layers:
            box = ((at[0] - r, at[1] - r), (at[0] + r, at[1] + r))
            # The copper itself, and a hair inside it as the net's own: a track's end there is in
            # the pad as the board measures it.
            b.mark(l, pad_test(at, half, angle, shape), box, n, MARGIN)
            inner = [max(half[0] - G, G / 2), max(half[1] - G, G / 2)]
            b.mark(l, pad_test(at, inner, angle, shape), box, n, 0.0)
            claim(b, l, pad_test(at, half, angle, shape), box, n)
        t = pad_test(at, half, angle, shape)
        i0, j0 = b.cell(at[0] - r, at[1] - r)
        i1, j1 = b.cell(at[0] + r, at[1] + r)
        for j in range(max(j0, 0), min(j1, b.ny - 1) + 1):
            for i in range(max(i0, 0), min(i1, b.nx - 1) + 1):
                if t(*b.xy(i, j), 0.0):
                    b.pad[b.idx(i, j)] = 1
        if q.get("kind") in ("through_hole", "hole"):
            d = q.get("drill", 0.6)
            d = max(d) if isinstance(d, list) else d
            for l in (0, 1):
                b.mark(l, seg_test(at, at, d / 2), ((at[0] - d, at[1] - d), (at[0] + d, at[1] + d)), MULTI, 0.6, HOLE_SIG if q.get("kind") == "hole" else None)
    for hole in board.get("holes", []):
        at, d = hole["at"], hole["drill"]
        for l in (0, 1):
            b.mark(l, seg_test(at, at, d / 2), ((at[0] - d, at[1] - d), (at[0] + d, at[1] + d)), MULTI, 0.6, HOLE_SIG)
    for k in board.get("keepouts", []):
        pts = k["outline"]
        (x0, y0), (x1, y1) = box_of(pts, 0)
        names = k.get("layers") or []
        on = [l for l, n in ((0, "top"), (1, "bottom")) if not names or k.get("all_layers") or n in names]
        for l in on:
            b.mark(l, rect_test([(x0 + x1) / 2, (y0 + y1) / 2], [(x1 - x0) / 2, (y1 - y0) / 2], 0), ((x0, y0), (x1, y1)), MULTI, MARGIN)
    # The edge band.
    for l in (0, 1):
        for j in range(b.ny):
            for i in range(b.nx):
                x, y = b.xy(i, j)
                if abs(x) > w / 2 - EDGE or abs(y) > h / 2 - EDGE:
                    b.block[l][b.idx(i, j)] = MULTI
                    b.block_sig[l][b.idx(i, j)] = MULTI
    commands = []
    pad_at = {ref: (net, at, half, angle, layers, q.get("shape", "rect")) for ref, net, at, half, angle, layers, q in pads}

    nets = dict(circuit.SIGNALS)
    nets.update({k: v for k, v in circuit.RAILS.items() if k != "GND"})
    base = (b, list(commands))
    best = None
    order = None
    for attempt in range(passes):
        b = copy_board(base[0])
        commands = list(base[1])
        records, fails = [], []
        failed, order = route_nets(b, nets, pad_at, circuit, commands, order, records, fails, fixed=base[0])
        print(f"pass {attempt + 1}: {len(failed)} connections unrouted")
        # A pass near the best gets the rip-up rescue before it is weighed.
        if fails and (best is None or len(failed) <= len(best[0]) + 4):
            b, records, fails = rescue(base[0], b, records, fails, pad_at)
            commands = list(base[1]) + [r["cmd"] for r in records]
            failed = [f"{f['name']}: {f['src']}" for f in fails] + [f for f in failed if f.startswith("GND")]
            print(f"  after the rescue: {len(failed)}")
        if best is None or len(failed) < len(best[0]):
            best = (failed, commands, b, records, fails)
        if not failed:
            break
        # Rip up and route again, what failed first; when a pass did no better than the best, the
        # rest go the other way round (longest first), so the next pass is not the same again.
        bad = []
        for f in failed:
            n = f.split(":")[0]
            if n in nets and n not in bad:
                bad.append(n)
        rest = [n for n in order if n not in bad]
        if len(failed) >= len(best[0]) and attempt % 2 == 1:
            rest = rest[::-1]
        order = bad + rest
    failed, commands, b, records, fails = best
    if failed:
        print("unrouted:", ", ".join(failed))
    return commands


def copy_board(b0):
    b = Board.__new__(Board)
    b.__dict__.update(b0.__dict__)
    b.block = [l[:] for l in b0.block]
    b.block_sig = [l[:] for l in b0.block_sig]
    b.own = [l[:] for l in b0.own]
    b.pad = bytearray(b0.pad)
    b.vias = list(b0.vias)
    b.nets = dict(b0.nets)
    return b


def route_nets(b, nets, pad_at, circuit, commands, order, records=None, fails=None, fixed=None):
    """Route every net in `order`; each route made goes to `records` (to replay or rip up), each
    connection that failed to `fails`."""
    records = [] if records is None else records
    fails = [] if fails is None else fails
    def span(pins):
        pts = [pad_at[p][1] for p in pins if p in pad_at]
        return max((math.dist(a, c) for a in pts for c in pts), default=0)


    failed = []
    # The 3V3 trunk first (it reaches both modules), then the other rails, then the signals,
    # shortest first; on a later pass, what failed before goes first.
    if order is None:
        first = ["USB_DP", "USB_DN", "VBUS", "CC1", "CC2", "3V3", "VBAT", "VSYS", "3V3_GPS"]
        order = [n for n in first if n in nets] + sorted((n for n in nets if n not in first), key=lambda n: span(nets[n]))
    # The ground links (see circuit.GND_LINKS) before anything else.
    g = b.net("GND")
    for a_ref, z_ref in getattr(circuit, "GND_LINKS", []):
        b.signal = False
        path = search(b, g, pad_at[a_ref], 0.3, [pad_at[z_ref][1]], only=pad_at[z_ref])
        if path is None:
            failed.append(f"GND link: {a_ref}")
            continue
        legs = tidy(b, g, path)
        commands.append(track_command(b, legs, 0.3))
        commit(b, legs, 0.3, g)
        records.append({"kind": "link", "name": "GND", "src": a_ref, "goals": [z_ref], "width": 0.3, "legs": legs, "cmd": commands[-1]})
    escaped = False
    for name in order:
        # The ground escapes once the USB-C's own lines are in, before the rest crowd them.
        if not escaped and name not in ("USB_DP", "USB_DN", "CC1", "CC2"):
            escaped = True
            ground_escapes(b, pad_at, circuit, commands, failed, records)
        pins = [p for p in nets[name] if p in pad_at]
        width = circuit.POWER_WIDTH if name in circuit.RAILS else circuit.SIGNAL_WIDTH
        b.signal = width <= circuit.SIGNAL_WIDTH
        if fixed is not None:
            fixed.signal = b.signal
        n = b.net(name)
        # USB D- runs beside D+ (routed just before it) as a pair.
        attract = b.nets.get(PAIRS[name]) if name in PAIRS else None
        # Each pin joins the copper the net already has, nearest first.
        done = [pins[0]]
        rest = pins[1:]
        while rest:
            src = min(rest, key=lambda p: min(math.dist(pad_at[p][1], pad_at[d][1]) for d in done))
            rest.remove(src)
            path = search(b, n, pad_at[src], width, [pad_at[d][1] for d in done], attract=attract)
            if path is None and fixed is not None:
                path = free_path(b, fixed, n, search(b, n, pad_at[src], width, [pad_at[d][1] for d in done], attract=attract, fixed=fixed))
            if path is None:
                failed.append(f"{name}: {src}")
                fails.append({"name": name, "src": src, "goals": list(done), "width": width})
                done.append(src)
                continue
            legs = tidy(b, n, path)
            commands.append(track_command(b, legs, width))
            commit(b, legs, width, n)
            records.append({"kind": "conn", "name": name, "src": src, "goals": list(done), "width": width, "legs": legs, "cmd": commands[-1]})
            done.append(src)
    if not escaped:
        ground_escapes(b, pad_at, circuit, commands, failed, records)
    return failed, order


def ground_escapes(b, pad_at, circuit, commands, failed, records=None):
    """Ground pads the pours may not reach between tracks: a short track to a via each."""
    g = b.net("GND")
    signal = b.signal
    b.signal = False
    for ref in circuit.GND_ESCAPES:
        if ref not in pad_at:
            continue
        path = escape(b, g, pad_at[ref], 0.3)
        if path is None:
            failed.append(f"GND escape: {ref}")
            continue
        legs = tidy(b, g, path)
        cmd = track_command(b, legs, 0.3)
        cmd[1]["via"] = {}
        commands.append(cmd)
        commit(b, legs, 0.3, g)
        end_via(b, (path[-1][1], path[-1][2]), g)
        if records is not None:
            records.append({"kind": "escape", "name": "GND", "src": ref, "goals": [], "width": 0.3, "legs": legs, "cmd": cmd})
    b.signal = signal


def end_via(b, cell, net):
    """A via at the cell, ending an escape."""
    end = b.xy(*cell)
    b.vias.append(tuple(cell))
    for ll in (0, 1):
        add_track(b, ll, end, end, 0.6, net)


def search(b, net, src, width, goals, only=None, attract=None, fixed=None):
    """A path from the pad `src` to copper of `net`, as [(layer, i, j)]; None when there is none.
    With `attract`, cells hugging that net's keep-out cost less (a pair's second line)."""
    _, at, half, angle, layers, shape = src
    # The board whose cells are open: the fixed one in a rip-up search, where cells routes hold
    # on `b` cost RIP_COST more.
    P = fixed if fixed is not None else b
    starts = []
    test = pad_test(at, [max(half[0] - G, G / 2), max(half[1] - G, G / 2)], angle, shape)
    i0, j0 = b.cell(at[0], at[1])
    r = int(math.hypot(*half) / G) + 1
    for l in layers:
        for j in range(j0 - r, j0 + r + 1):
            for i in range(i0 - r, i0 + r + 1):
                if b.inside(i, j) and test(*b.xy(i, j), 0.0) and P.passable(l, b.idx(i, j), net):
                    starts.append((l, i, j))
    if not starts:
        return None
    goal_cells = [b.cell(*g) for g in goals]
    # Search a window round the pad and the goals, 8 mm wider each way.
    pad = int(8.0 / G)
    xs = [i0] + [g[0] for g in goal_cells]
    ys = [j0] + [g[1] for g in goal_cells]
    lo_i, hi_i, lo_j, hi_j = min(xs) - pad, max(xs) + pad, min(ys) - pad, max(ys) + pad

    def heur(i, j):
        return min(max(abs(i - gi), abs(j - gj)) + 0.414 * min(abs(i - gi), abs(j - gj)) for gi, gj in goal_cells)

    own_start = set((l, b.idx(i, j)) for l, i, j in starts)
    openq = []
    cost = {}
    prev = {}
    heading = {}
    grid = b.block_sig if b.signal else b.block

    def hugs(l, i, j):
        g = grid[l]
        for a, c2 in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            if b.inside(i + a, j + c2) and g[b.idx(i + a, j + c2)] == attract:
                return True
        return False
    for s in starts:
        cost[s] = 0.0
        heapq.heappush(openq, (heur(s[1], s[2]), 0.0, s))
    steps = [(1, 0, 1.0), (-1, 0, 1.0), (0, 1, 1.0), (0, -1, 1.0), (1, 1, 1.414), (1, -1, 1.414), (-1, 1, 1.414), (-1, -1, 1.414)]
    # Heuristic and costs in cells.
    expanded = 0
    while openq:
        _, g0, s = heapq.heappop(openq)
        if g0 > cost.get(s, 1e18):
            # Reached more cheaply since it was queued: not a step of the search.
            continue
        l, i, j = s
        k = b.idx(i, j)
        if b.own[l][k] == net and (l, k) not in own_start and (only is None or in_pad(b, only, i, j, l)):
            path = [s]
            while path[-1] in prev:
                path.append(prev[path[-1]])
            return path[::-1]
        expanded += 1
        if expanded > 3000000:
            return None
        c = cost[s]
        for di, dj, dc in steps:
            ni, nj = i + di, j + dj
            if not (lo_i <= ni <= hi_i and lo_j <= nj <= hi_j and b.inside(ni, nj)):
                continue
            nk = b.idx(ni, nj)
            if not P.passable(l, nk, net):
                continue
            # A diagonal step may not cut a blocked corner.
            if di and dj and not (P.passable(l, b.idx(i + di, j), net) and P.passable(l, b.idx(i, j + dj), net)):
                continue
            if fixed is not None and not b.passable(l, nk, net):
                dc += RIP_COST
            t = (l, ni, nj)
            h0 = heading.get(s)
            if h0 is not None and h0 != (di, dj):
                # A turn: 45 degrees when the headings' dot product is positive, else sharper.
                dc += TURN_45 if h0[0] * di + h0[1] * dj > 0 else TURN_90
            if attract is not None and hugs(l, ni, nj):
                dc *= PAIR_SHARE
            nc = c + dc
            if nc < cost.get(t, 1e18):
                cost[t] = nc
                prev[t] = s
                heading[t] = (di, dj)
                heapq.heappush(openq, (nc + heur(ni, nj), nc, t))
        # A via: both layers free round it, off the pads.
        o = 1 - l
        if b.own[0][k] == 0 and b.own[1][k] == 0:
            # Clear of every pad (its own too: no via in a pad), of other nets on both layers and
            # of other vias' holes.
            # In a cost search the via keeps its distance from the vias routes made (a via is
            # never worth ripping up a route for); the copper round it may cost.
            ok = via_ok(P, i, j, net) and (fixed is None or not near_via(b, i, j))
            vc = VIA_COST + (RIP_COST * 4 if fixed is not None and ok and not via_ok(b, i, j, net) else 0.0)
            t = (o, i, j)
            if ok and c + vc < cost.get(t, 1e18):
                cost[t] = c + vc
                prev[t] = s
                heapq.heappush(openq, (c + vc + heur(i, j), c + vc, t))
    return None


VIA_GAP = int(0.9 / G)


def near_via(b, i, j):
    """Another via's hole within 0.9 mm of the cell."""
    return any(abs(i - vi) <= VIA_GAP and abs(j - vj) <= VIA_GAP and math.hypot(i - vi, j - vj) <= VIA_GAP for vi, vj in b.vias)


def via_ok(b, i, j, net):
    """A via fits at the cell: both layers free round it for `net`, no pad near, no other via's
    hole within 0.9 mm."""
    if near_via(b, i, j):
        return False
    for a in VIA_REACH:
        for c2 in VIA_REACH:
            if not b.inside(i + a, j + c2):
                return False
            k = b.idx(i + a, j + c2)
            if not (b.passable(0, k, net) and b.passable(1, k, net)) or b.pad[k]:
                return False
    return True


def escape(b, net, src, width):
    """A short track from the pad `src` to a via, for a ground pad its pour may not reach:
    [(layer, i, j)] ending where the via goes; None when there is no room."""
    _, at, half, angle, layers, shape = src
    test = pad_test(at, [max(half[0] - G, G / 2), max(half[1] - G, G / 2)], angle, shape)
    i0, j0 = b.cell(*at)
    r = int(math.hypot(*half) / G) + 1
    q = []
    seen = {}
    for j in range(j0 - r, j0 + r + 1):
        for i in range(i0 - r, i0 + r + 1):
            if b.inside(i, j) and test(*b.xy(i, j), 0.0) and b.passable(0, b.idx(i, j), net):
                q.append((0.0, (i, j)))
                seen[(i, j)] = None
    heapq.heapify(q)
    reach = int(3.0 / G)
    while q:
        c, (i, j) = heapq.heappop(q)
        if math.dist((i, j), (i0, j0)) * G > math.hypot(*half) + 0.5 and via_ok(b, i, j, net):
            path = [(0, i, j)]
            p = seen[(i, j)]
            while p is not None:
                path.append((0, p[0], p[1]))
                p = seen[p]
            return path[::-1]
        for di, dj, dc in ((1, 0, 1), (-1, 0, 1), (0, 1, 1), (0, -1, 1), (1, 1, 1.414), (1, -1, 1.414), (-1, 1, 1.414), (-1, -1, 1.414)):
            t = (i + di, j + dj)
            if t in seen or not b.inside(*t) or abs(t[0] - i0) > reach or abs(t[1] - j0) > reach:
                continue
            if not b.passable(0, b.idx(*t), net):
                continue
            seen[t] = (i, j)
            heapq.heappush(q, (c + dc, t))
    return None


def in_pad(b, pad, i, j, l):
    _, at, half, angle, layers, shape = pad
    return l in layers and pad_test(at, half, angle, shape)(*b.xy(i, j), 0.0)


def _legs(path):
    """A path's runs on one layer: [(layer, [(i, j), ...])]; a via sits where one run ends and the
    next starts, at the same cell."""
    legs = []
    cur_l, cur = path[0][0], [(path[0][1], path[0][2])]
    for l, i, j in path[1:]:
        if l != cur_l:
            legs.append((cur_l, cur))
            cur_l, cur = l, [(i, j)]
        else:
            cur.append((i, j))
    legs.append((cur_l, cur))
    return legs


def _sign(v):
    return (v > 0) - (v < 0)


def clear_line(b, net, l, a, c):
    """The straight 0/45/90 degree run from cell `a` to `c` keeps to cells `net` may use."""
    dx, dy = c[0] - a[0], c[1] - a[1]
    if dx and dy and abs(dx) != abs(dy):
        return False
    si, sj = _sign(dx), _sign(dy)
    i, j = a
    for _ in range(max(abs(dx), abs(dy))):
        ni, nj = i + si, j + sj
        if not b.inside(ni, nj) or not b.passable(l, b.idx(ni, nj), net):
            return False
        if si and sj and not (b.passable(l, b.idx(ni, j), net) and b.passable(l, b.idx(i, nj), net)):
            return False
        i, j = ni, nj
    return True


def connect(b, net, l, a, c):
    """The fewest 0/45/90 degree segments from `a` to `c` on free cells: the corner between them
    ([] for a straight run, [corner] for two), or None."""
    dx, dy = c[0] - a[0], c[1] - a[1]
    if dx == 0 or dy == 0 or abs(dx) == abs(dy):
        return [] if clear_line(b, net, l, a, c) else None
    m = min(abs(dx), abs(dy))
    diag = (_sign(dx) * m, _sign(dy) * m)
    for corner in ((a[0] + diag[0], a[1] + diag[1]), (c[0] - diag[0], c[1] - diag[1])):
        if clear_line(b, net, l, a, corner) and clear_line(b, net, l, corner, c):
            return [corner]
    return None


def pull(b, net, l, cells):
    """A run of cells as the fewest straight segments: from each kept point, the farthest later
    corner it reaches in one or two segments on free cells."""
    if len(cells) < 2:
        return list(cells)
    corners = [cells[0]]
    for k in range(1, len(cells) - 1):
        a, m, c = cells[k - 1], cells[k], cells[k + 1]
        if (m[0] - a[0], m[1] - a[1]) != (c[0] - m[0], c[1] - m[1]):
            corners.append(m)
    corners.append(cells[-1])
    out = [corners[0]]
    i = 0
    while i < len(corners) - 1:
        for k in range(len(corners) - 1, i, -1):
            mid = connect(b, net, l, corners[i], corners[k])
            if mid is not None:
                out += mid + [corners[k]]
                i = k
                break
        else:
            out.append(corners[i + 1])
            i += 1
    return out


def tidy(b, net, path):
    """A searched path as legs of the fewest straight segments each."""
    return [(l, pull(b, net, l, cells)) for l, cells in _legs(path)]


def commit(b, legs, width, net):
    """A route's copper on the board: its segments, and a via where a leg starts after another."""
    for k, (l, pts) in enumerate(legs):
        for a, c in zip(pts, pts[1:]):
            add_track(b, l, b.xy(*a), b.xy(*c), width, net)
        if k:
            at = b.xy(*pts[0])
            b.vias.append(pts[0])
            for ll in (0, 1):
                add_track(b, ll, at, at, 0.6, net)


def track_command(b, legs, width):
    """One ecad.board.track for a tidied route: a leg per layer, a via between."""
    name = lambda l: "top" if l == 0 else "bottom"  # noqa: E731
    pts = lambda cells: [list(b.xy(*p)) for p in cells]  # noqa: E731
    l0, first = legs[0]
    cmd = {"layer": name(l0), "width": width, "points": pts(first)}
    if len(legs) > 1:
        cmd["via"] = {}
        cmd["legs"] = []
        for k, (l, cells) in enumerate(legs[1:]):
            entry = {"layer": name(l), "points": pts(cells)}
            if k < len(legs) - 2:
                entry["via"] = {}
            cmd["legs"].append(entry)
    return ("ecad.board.track", cmd)


# A cell another route holds costs this many steps in the rip-up search (pads and the rest of the
# fixed board stay closed).
RIP_COST = 40.0


def replay(b, r):
    """A recorded route's copper on the board `b` (made from the fixed board)."""
    n = b.net(r["name"])
    b.signal = r["width"] <= 0.15 + 1e-9
    commit(b, r["legs"], r["width"], n)
    if r["kind"] == "escape":
        l, pts = r["legs"][-1]
        end_via(b, pts[-1], n)


def rebuild(fixed, records):
    b = copy_board(fixed)
    for r in records:
        replay(b, r)
    return b


def free_path(b, fixed, net, path):
    """`path` (from a search on the fixed board) when every cell and via of it is free on `b`
    too, else None: the cost search often finds a free path the walled one ran out of steps on."""
    if path is None:
        return None
    if fixed is not None:
        fixed.signal = b.signal
    for k, (l, i, j) in enumerate(path):
        if not b.passable(l, b.idx(i, j), net):
            return None
        if k and path[k - 1][0] != l and not via_ok(b, i, j, net):
            return None
    return path


def route_valid(b, pad_at, f, fixed):
    """One connection on `b`: the cost search's path when it is free on `b`, else the walled
    search's; None when neither finds one."""
    path = free_path(b, fixed, b.net(f["name"]), route_one(b, pad_at, f, fixed=fixed))
    return path if path is not None else route_one(b, pad_at, f)


def route_one(b, pad_at, f, fixed=None):
    """One connection on `b` as a searched path, or None. With `fixed` (the board before any
    route), cells other routes hold are open at a cost: the rip-up search."""
    n = b.net(f["name"])
    b.signal = f["width"] <= 0.15 + 1e-9
    if fixed is not None:
        fixed.signal = b.signal
    attract = b.nets.get(PAIRS[f["name"]]) if f["name"] in PAIRS else None
    return search(b, n, pad_at[f["src"]], f["width"], [pad_at[d][1] for d in f["goals"]], attract=attract, fixed=fixed)


def hits(b, path, net, records):
    """The recorded routes whose copper holds a cell of `path` that `net` may not use on `b`."""
    out = set()
    cells = [(l, b.xy(i, j)) for l, i, j in path if not b.passable(l, b.idx(i, j), net)]
    # Round each via of the path: the room it needs on both layers.
    for k in range(1, len(path)):
        if path[k][0] != path[k - 1][0]:
            _, i, j = path[k]
            for a in VIA_REACH:
                for c2 in VIA_REACH:
                    if b.inside(i + a, j + c2):
                        kk = b.idx(i + a, j + c2)
                        for l in (0, 1):
                            if not b.passable(l, kk, net):
                                cells.append((l, b.xy(i + a, j + c2)))
    for k, r in enumerate(records):
        if r["name"] == "GND" and r["kind"] == "link":
            continue
        reach = r["width"] / 2 + MARGIN + G
        for l, pts in r["legs"]:
            seg = [b.xy(*p) for p in pts]
            for lc, (x, y) in cells:
                if lc != l and not (len(seg) == 1):
                    continue
                for a, c in zip(seg, seg[1:] or seg):
                    if seg_test(a, c, reach)(x, y, 0.0):
                        out.add(k)
                        break
                if k in out:
                    break
            if k in out:
                break
        # Its vias, on both layers.
        if k not in out:
            vs = [b.xy(*pts[0]) for l, pts in r["legs"][1:]] + ([b.xy(*r["legs"][-1][1][-1])] if r["kind"] == "escape" else [])
            for v in vs:
                if any(math.dist(v, (x, y)) <= 0.3 + MARGIN + G for _, (x, y) in cells):
                    out.add(k)
                    break
    return out


def rescue(fixed, b, records, fails, pad_at, rounds=4):
    for _ in range(rounds):
        better = False
        for f in list(fails):
            n = b.net(f["name"])
            path = route_one(b, pad_at, f, fixed=fixed)
            if path is None:
                continue
            victims = hits(b, path, n, records)
            if not victims:
                free = free_path(b, fixed, n, path)
                if free is None:
                    continue
                legs = tidy(b, n, free)
                commit(b, legs, f["width"], n)
                records.append({"kind": "conn", "name": f["name"], "src": f["src"], "goals": f["goals"], "width": f["width"],
                                "legs": legs, "cmd": track_command(b, legs, f["width"])})
                fails = [x for x in fails if x is not f]
                better = True
                print(f"routed {f['name']}: {f['src']} on a second search")
                continue
            kept = [r for k, r in enumerate(records) if k not in victims]
            ripped = [r for k, r in enumerate(records) if k in victims]
            b2 = rebuild(fixed, kept)
            new_records = list(kept)
            path2 = route_valid(b2, pad_at, f, fixed)
            if path2 is None:
                continue
            legs = tidy(b2, n, path2)
            new_records.append({"kind": "conn", "name": f["name"], "src": f["src"], "goals": f["goals"], "width": f["width"],
                                "legs": legs, "cmd": track_command(b2, legs, f["width"])})
            commit(b2, legs, f["width"], n)
            lost = []
            for r in ripped:
                if r["kind"] == "escape":
                    g = b2.net("GND")
                    b2.signal = False
                    p = escape(b2, g, pad_at[r["src"]], 0.3)
                    if p is None:
                        lost.append(r)
                        continue
                    lg = tidy(b2, g, p)
                    cmd = track_command(b2, lg, 0.3)
                    cmd[1]["via"] = {}
                    commit(b2, lg, 0.3, g)
                    end_via(b2, (p[-1][1], p[-1][2]), g)
                    new_records.append({**r, "legs": lg, "cmd": cmd})
                    continue
                p = route_valid(b2, pad_at, r, fixed)
                if p is None:
                    lost.append(r)
                    continue
                rn = b2.net(r["name"])
                lg = tidy(b2, rn, p)
                commit(b2, lg, r["width"], rn)
                new_records.append({**r, "legs": lg, "cmd": track_command(b2, lg, r["width"])})
            # Escapes lost count less than connections: a pad still has the pour.
            lost_conns = [r for r in lost if r["kind"] != "escape"]
            if not lost_conns and len(lost) <= 1:
                fails = [x for x in fails if x is not f] + [{k: r[k] for k in ("name", "src", "goals", "width")} for r in lost_conns]
                b, records = b2, new_records
                better = True
                print(f"rescued {f['name']}: {f['src']} (ripped {len(ripped)} routes)")
        if not better or not fails:
            break
    return b, records, fails
