"""A small two-layer grid router for the example, so the board's tracks come from SolidCraft's
ecad.board.track commands (SolidCraft has no autorouter command of its own).

The board is a 0.1 mm grid on each layer. Every piece of copper marks the cells it covers with its
net, and the cells within the clearance plus the widest track's half width round it as taken by
that net; the board's edge band, the keep-outs, the holes and the modules' bodies (on the top) are
taken by none. A net may go through free cells and its own. Each connection is an A* search over
both layers (45 degree steps, a via a cost of its own, both layers free round it), from a pad to
the copper its net already has; its path, its straight runs merged, becomes one ecad.board.track
with legs and vias. The RF line is drawn by hand, as a coplanar line of the width the impedance
tool gave.
"""

import heapq
import math

G = 0.05  # grid step, mm
CLEARANCE = 0.2
WIDEST = 0.4
MARGIN = CLEARANCE + WIDEST / 2 + G / 2
# What a signal track (0.2 mm) needs round other copper: the clearance, its half width, a hair.
SIGNAL_MARGIN = CLEARANCE + 0.1 + G / 2
EDGE = 0.3 + WIDEST / 2 + G
# A signal track's centre from an unplated hole: the edge clearance, half the width, half a cell.
HOLE_SIG = 0.3 + 0.1 + G / 2
# The ground-only band beside the RF line, wide enough for the pour to fill it (the 0.25 mm
# coplanar gap, a strip of pour, the clearance), so the line's nearest neighbour is ground.
RF_ZONE = 1.5
VIA_COST = 50.0
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


def route_board(board, footprints, rf_width, circuit):
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

    # The RF line by hand: out of the E22's ANT pad to the left, then down to the U.FL.
    # Out of the pad at the pad's own width (its neighbours are 1.27 mm away), then the 50 ohm width.
    ant, ufl = pad_at["U2.21"], pad_at["J3.1"]
    out = [ant[1][0] - ant[2][0] - 1.2, ant[1][1]]
    corner = [ufl[1][0], ant[1][1]]
    nrf = b.net("RF_ANT")
    for pts, wdt in (([ant[1], out], 0.8), ([out, corner, ufl[1]], rf_width)):
        commands.append(("ecad.board.track", {"layer": "top", "width": wdt, "points": [list(p) for p in pts]}))
        for a, c in zip(pts, pts[1:]):
            add_track(b, 0, a, c, wdt, nrf)
    # Beside the RF line on its layer only ground (the quality check wants ground as its
    # neighbour within max(2W, 3h)).
    g = b.net("GND")
    for a, c in zip([ant[1], out, corner], [out, corner, ufl[1]]):
        box = box_of([a, c], RF_ZONE)
        b.mark(0, seg_test(a, c, rf_width / 2), box, g, RF_ZONE, RF_ZONE)

    nets = dict(circuit.SIGNALS)
    nets.update({k: v for k, v in circuit.RAILS.items() if k != "GND"})
    nets.pop("RF_ANT", None)
    base = (b, list(commands))
    best = None
    order = None
    for attempt in range(6):
        b = copy_board(base[0])
        commands = list(base[1])
        failed, order = route_nets(b, nets, pad_at, circuit, commands, order)
        print(f"pass {attempt + 1}: {len(failed)} connections unrouted")
        if best is None or len(failed) < len(best[0]):
            best = (failed, commands)
        if not failed:
            break
        # Rip up and route again, what failed first.
        bad = []
        for f in failed:
            n = f.split(":")[0]
            if n in nets and n not in bad:
                bad.append(n)
        order = bad + [n for n in order if n not in bad]
    failed, commands = best
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


def route_nets(b, nets, pad_at, circuit, commands, order):
    def span(pins):
        pts = [pad_at[p][1] for p in pins if p in pad_at]
        return max((math.dist(a, c) for a in pts for c in pts), default=0)


    failed = []
    # The 3V3 trunk first (it reaches both modules), then the other rails, then the signals,
    # shortest first; on a later pass, what failed before goes first.
    if order is None:
        first = ["USB_DP", "USB_DN", "CC1", "CC2", "3V3", "VBUS", "VBAT", "VSYS", "3V3_GPS"]
        order = [n for n in first if n in nets] + sorted((n for n in nets if n not in first), key=lambda n: span(nets[n]))
    # The ground links (see circuit.GND_LINKS) before anything else.
    g = b.net("GND")
    for a_ref, z_ref in getattr(circuit, "GND_LINKS", []):
        b.signal = False
        path = search(b, g, pad_at[a_ref], 0.3, [pad_at[z_ref][1]], only=pad_at[z_ref])
        if path is None:
            failed.append(f"GND link: {a_ref}")
            continue
        commands.append(track_command(b, path, 0.3))
        for (l, i, j), (l2, i2, j2) in zip(path, path[1:]):
            add_track(b, l, b.xy(i, j), b.xy(i2, j2), 0.3, g)
    escaped = False
    for name in order:
        # The ground escapes once the USB-C's own lines are in, before the rest crowd them.
        if not escaped and name not in ("USB_DP", "USB_DN", "CC1", "CC2"):
            escaped = True
            ground_escapes(b, pad_at, circuit, commands, failed)
        pins = [p for p in nets[name] if p in pad_at]
        width = circuit.POWER_WIDTH if name in circuit.RAILS else circuit.SIGNAL_WIDTH
        b.signal = width <= circuit.SIGNAL_WIDTH
        n = b.net(name)
        # Each pin joins the copper the net already has, nearest first.
        done = [pins[0]]
        rest = pins[1:]
        while rest:
            src = min(rest, key=lambda p: min(math.dist(pad_at[p][1], pad_at[d][1]) for d in done))
            rest.remove(src)
            path = search(b, n, pad_at[src], width, [pad_at[d][1] for d in done])
            if path is None:
                failed.append(f"{name}: {src}")
                done.append(src)
                continue
            commands.append(track_command(b, path, width))
            for (l, i, j), (l2, i2, j2) in zip(path, path[1:]):
                a, c = b.xy(i, j), b.xy(i2, j2)
                if l == l2:
                    add_track(b, l, a, c, width, n)
                else:
                    b.vias.append((i, j))
                    for ll in (0, 1):
                        add_track(b, ll, a, a, 0.6, n)
            done.append(src)
    if not escaped:
        ground_escapes(b, pad_at, circuit, commands, failed)
    return failed, order


def ground_escapes(b, pad_at, circuit, commands, failed):
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
        cmd = track_command(b, path, 0.3)
        cmd[1]["via"] = {}
        commands.append(cmd)
        for (l, i, j), (l2, i2, j2) in zip(path, path[1:]):
            add_track(b, 0, b.xy(i, j), b.xy(i2, j2), 0.3, g)
        end = b.xy(path[-1][1], path[-1][2])
        b.vias.append((path[-1][1], path[-1][2]))
        for ll in (0, 1):
            add_track(b, ll, end, end, 0.6, g)
    b.signal = signal


def search(b, net, src, width, goals, only=None):
    """A path from the pad `src` to copper of `net`, as [(layer, i, j)]; None when there is none."""
    _, at, half, angle, layers, shape = src
    starts = []
    test = pad_test(at, [max(half[0] - G, G / 2), max(half[1] - G, G / 2)], angle, shape)
    i0, j0 = b.cell(at[0], at[1])
    r = int(math.hypot(*half) / G) + 1
    for l in layers:
        for j in range(j0 - r, j0 + r + 1):
            for i in range(i0 - r, i0 + r + 1):
                if b.inside(i, j) and test(*b.xy(i, j), 0.0) and b.passable(l, b.idx(i, j), net):
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
    for s in starts:
        cost[s] = 0.0
        heapq.heappush(openq, (heur(s[1], s[2]), s))
    steps = [(1, 0, 1.0), (-1, 0, 1.0), (0, 1, 1.0), (0, -1, 1.0), (1, 1, 1.414), (1, -1, 1.414), (-1, 1, 1.414), (-1, -1, 1.414)]
    # Heuristic and costs in cells.
    expanded = 0
    while openq:
        _, s = heapq.heappop(openq)
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
            if not b.passable(l, nk, net):
                continue
            # A diagonal step may not cut a blocked corner.
            if di and dj and not (b.passable(l, b.idx(i + di, j), net) and b.passable(l, b.idx(i, j + dj), net)):
                continue
            t = (l, ni, nj)
            nc = c + dc
            if nc < cost.get(t, 1e18):
                cost[t] = nc
                prev[t] = s
                heapq.heappush(openq, (nc + heur(ni, nj), t))
        # A via: both layers free round it, off the pads.
        o = 1 - l
        if b.own[0][k] == 0 and b.own[1][k] == 0:
            # Clear of every pad (its own too: no via in a pad), of other nets on both layers and
            # of other vias' holes.
            ok = via_ok(b, i, j, net)
            t = (o, i, j)
            if ok and c + VIA_COST < cost.get(t, 1e18):
                cost[t] = c + VIA_COST
                prev[t] = s
                heapq.heappush(openq, (c + VIA_COST + heur(i, j), t))
    return None


VIA_GAP = int(0.9 / G)


def via_ok(b, i, j, net):
    """A via fits at the cell: both layers free round it for `net`, no pad near, no other via's
    hole within 0.9 mm."""
    if any(abs(i - vi) <= VIA_GAP and abs(j - vj) <= VIA_GAP and math.hypot(i - vi, j - vj) <= VIA_GAP for vi, vj in b.vias):
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


def track_command(b, path, width):
    """One ecad.board.track for a path: a leg per layer, a via between."""
    legs = []
    cur = [path[0]]
    for p in path[1:]:
        if p[0] != cur[-1][0]:
            legs.append(cur)
            cur = [p]
        else:
            cur.append(p)
    legs.append(cur)

    def points(leg):
        pts = [leg[0]]
        for k in range(1, len(leg) - 1):
            a, m, c = leg[k - 1], leg[k], leg[k + 1]
            if (m[1] - a[1], m[2] - a[2]) != (c[1] - m[1], c[2] - m[2]):
                pts.append(m)
        if len(leg) > 1:
            pts.append(leg[-1])
        return [list(b.xy(p[1], p[2])) for p in pts]

    name = lambda l: "top" if l == 0 else "bottom"  # noqa: E731
    first = points(legs[0])
    cmd = {"layer": name(legs[0][0][0]), "width": width, "points": first}
    if len(legs) > 1:
        cmd["via"] = {}
        cmd["legs"] = []
        for k, leg in enumerate(legs[1:]):
            pts = points(leg)
            entry = {"layer": name(leg[0][0]), "points": pts}
            if k < len(legs) - 2:
                entry["via"] = {}
            cmd["legs"].append(entry)
    return ("ecad.board.track", cmd)
