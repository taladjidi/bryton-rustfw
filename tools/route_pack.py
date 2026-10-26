#!/usr/bin/env python3
"""Stock's routing graph (section 1 of a Bryton map file) for the firmware's tests, and a reader to check files.

    tools/route_pack.py fixture OUTDIR      # the synthetic test file in test-data/route (a made-up road, no real map)
    tools/route_pack.py info FILE.dat       # decode a real file with this independent reader: counts, checks
    tools/route_pack.py selftest

The format is stock's (ghidra/STORAGE_AND_MAPS.md 4.4 and 4.7 in the bryton repo; the firmware's reader is
src/storage/route.rs). In short, little endian: a 0x12-byte header whose u32 at 0x0e is the routing section; there,
20 u32 zoom offsets, and for zoom 14 (the only one) a 100-byte header ("BRYTON", the tile ranges, index and length
sizes, where the data starts), an index of one entry per tile (column-major), and the tiles, each a length then a
gzip stream of one protobuf message: base latitude and longitude (sint32, 1e-7 degrees: the centre of the z14 tile),
deltaBits, numOfTransport (2), the nodes (key, elevation, way ids, edges) and the ways.

One finding the firmware's reader depends on, from 451,537 nodes of a real file (2026-10-26): a node key packs two
halves of deltaBits bits, latitude high and longitude low, and each half is SIGN AND MAGNITUDE, not two's
complement: bit deltaBits-1 is the sign, the low deltaBits-1 bits the distance from the base, in 1e-7 degrees. Read
so, every node lies in its own z14 tile, every edge is listed at both ends, and edges to nodes of the next tile
resolve there; read as two's complement, three nodes in four land in other tiles and 1.5 % of the edges are
kilometres long.

The fixture is a MADE-UP network in the open South Pacific, far from any road: a main road with a 2 km climb at
6 %, a junction where the road bends and a straight branch (a different way) dead-ends, and a short side road. It
crosses several z14 tiles and leaves one inside the file's range empty.
"""

import gzip
import math
import os
import struct
import sys

Z = 14
BITS = 20
LAT0, LON0 = -52.0, -145.0  # the origin of the made-up network (open ocean)
M_PER_E7 = 0.011132         # metres per 1e-7 degree of latitude, as src/ui/map.rs
MAIN, BRANCH, SIDE = 100001, 100002, 100003  # way ids of the made-up roads
ATTR = 0xee                 # an edge's attribute byte (class); the side road has another


def cos_q16(lat_e7):
    """src/ui/map.rs cos_q16, integer for integer (truncating division toward zero, as Rust's)."""
    t = int(lat_e7 / 10000)
    return ((32_400_000_000 - 4 * t * t) << 16) // (32_400_000_000 + t * t)


def tile_f(lat, lon):
    n = 1 << Z
    return ((lon + 180) / 360 * n, (1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * n)


def tile_of(lat_e7, lon_e7):
    x, y = tile_f(lat_e7 / 1e7, lon_e7 / 1e7)
    return int(x), int(y)


def centre(x, y):
    """The centre of z14 tile (x, y) in 1e-7 degrees: the base of its routing tile."""
    n = 1 << Z
    lon = (x + 0.5) / n * 360 - 180
    lat = math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * (y + 0.5) / n))))
    return round(lat * 1e7), round(lon * 1e7)


def varint(v):
    out = bytearray()
    while True:
        b = v & 0x7f
        v >>= 7
        out.append(b | (0x80 if v else 0))
        if not v:
            return bytes(out)


def field(f, wire, payload):
    return varint(f << 3 | wire) + payload


def vfield(f, v):
    return field(f, 0, varint(v))


def lfield(f, b):
    return field(f, 2, varint(len(b)) + b)


def zigzag(v):
    return (v << 1) ^ (v >> 31) if v < 0 else v << 1


def unzigzag(v):
    return (v >> 1) ^ -(v & 1)


def half(delta):
    """A delta as BITS bits, sign and magnitude."""
    assert abs(delta) < 1 << (BITS - 1), delta
    return (1 << (BITS - 1) if delta < 0 else 0) | abs(delta)


def unhalf(h):
    m = h & ((1 << (BITS - 1)) - 1)
    return -m if h >> (BITS - 1) & 1 else m


def key(base, lat, lon):
    return half(lat - base[0]) << BITS | half(lon - base[1])


class Net:
    """Nodes (lat, lon in 1e-7 degrees, elevation in metres, way ids), and edges (both ways, with an attribute)."""

    def __init__(self):
        self.nodes = {}  # (lat, lon) -> [elev, set(ways)]
        self.edges = {}  # (lat, lon) -> {(lat, lon): attr}

    def node(self, lat, lon, elev, way):
        n = self.nodes.setdefault((lat, lon), [elev, set()])
        n[1].add(way)
        return (lat, lon)

    def link(self, a, b, attr=ATTR):
        self.edges.setdefault(a, {})[b] = attr
        self.edges.setdefault(b, {})[a] = attr


def metres(x, y):
    """A place x metres east and y north of the origin, in 1e-7 degrees (src/ui/map.rs's plane)."""
    lat0, lon0 = round(LAT0 * 1e7), round(LON0 * 1e7)
    lat = lat0 + round(y * 1e6 / 11132)
    cos = cos_q16(lat0) / 65536
    return lat, lon0 + round(x * 1e6 / (11132 * cos))


def main_elev(s):
    """The main road's elevation at s metres along it: flat, 2 km at 6 % from 1 km, flat, down, flat."""
    if s < 1000:
        return 100.0
    if s < 3000:
        return 100 + 0.06 * (s - 1000)
    if s < 4500:
        return 220.0
    if s < 6500:
        return 220 - 0.04 * (s - 4500)
    return 140.0


def fixture_net():
    net = Net()
    step, bend, total = 25, 3200, 7000
    prev = None
    for s in range(0, bend + 1, step):  # east along y = 0
        p = net.node(*metres(s, 0), round(main_elev(s)), MAIN)
        if prev:
            net.link(prev, p)
        prev = p
    junction = prev
    c, sn = math.cos(math.radians(40)), math.sin(math.radians(40))
    for t in range(step, total - bend + 1, step):  # then 40 degrees to the left
        p = net.node(*metres(bend + t * c, t * sn), round(main_elev(bend + t)), MAIN)
        net.link(prev, p)
        prev = p
    prev = junction
    for t in range(step, 801, step):  # straight on: another way, a dead end after 800 m
        p = net.node(*metres(bend + t, 0), 220, BRANCH)
        net.link(prev, p)
        prev = p
    net.nodes[junction][1].add(BRANCH)
    base = net.node(*metres(1500, 0), round(main_elev(1500)), SIDE)
    prev = base
    for t in range(step, 301, step):  # a side road north from 1.5 km, another class
        p = net.node(*metres(1500, t), round(main_elev(1500)), SIDE)
        net.link(prev, p, 0x99)
        prev = p
    return net


def tile_blob(net, x, y, nodes):
    base = centre(x, y)
    out = bytearray()
    out += vfield(1, zigzag(base[0])) + vfield(2, zigzag(base[1])) + vfield(3, BITS) + vfield(4, 2)
    ways = set()
    for p in sorted(nodes):
        elev, ws = net.nodes[p]
        n = vfield(1, key(base, *p)) + vfield(2, (elev + 500) * 5)
        n += lfield(3, b''.join(varint(w) for w in sorted(ws)))
        ways |= ws
        for q, attr in sorted(net.edges.get(p, {}).items()):
            n += lfield(4, vfield(1, key(base, *q)) + vfield(2, attr))
        out += lfield(5, n)
    for w in sorted(ways):
        out += lfield(6, vfield(1, w) + lfield(2, b'made-up road %d' % w))
    out += vfield(15, 1)
    return gzip.compress(bytes(out), compresslevel=9, mtime=0)


def build(net):
    """The .dat file's bytes for the network, and the name stock would give it (9-<x>-<y>.dat)."""
    cells = {}
    for p in net.nodes:
        cells.setdefault(tile_of(*p), []).append(p)
    xs, ys = [c[0] for c in cells], [c[1] for c in cells]
    (x0, x1), (y0, y1) = (min(xs), max(xs)), (min(ys), max(ys) + 1)  # one empty row under the road
    z9 = {(x >> 5, y >> 5) for x, y in cells} | {(x0 >> 5, y1 >> 5), (x1 >> 5, y1 >> 5)}
    assert len(z9) == 1, 'the network crosses a z9 tile: move the origin'
    h = y1 - y0 + 1
    index = [0xffffffff] * ((x1 - x0 + 1) * h)
    data = bytearray()
    for i, (cell, nodes) in enumerate(sorted(cells.items())):
        index[(cell[0] - x0) * h + cell[1] - y0] = len(data)
        blob = tile_blob(net, *cell, nodes)
        data += struct.pack('<I', len(blob)) + blob
        if i % 3 == 1:
            data += b'\0\0'  # stock's files have a couple of bytes between some tiles
    zb = 0x12 + 80
    zh = bytearray(100)
    zh[0:6] = b'BRYTON'
    struct.pack_into('<H', zh, 6, 1)
    zh[8] = Z
    struct.pack_into('<4I', zh, 0x10, x0, x1, y0, y1)
    zh[0x20], zh[0x21] = 4, 4
    struct.pack_into('<I', zh, 0x22, 100 + 4 * len(index))
    ztab = [0] * 20
    ztab[Z] = 80
    head = bytearray(0x12)
    struct.pack_into('<II', head, 0x0a, 0, 0x12)
    body = bytes(head) + struct.pack('<20I', *ztab) + bytes(zh) + struct.pack('<%dI' % len(index), *index) + bytes(data)
    assert len(head) + 80 == zb
    z9x, z9y = next(iter(z9))
    return body, '9-%d-%d.dat' % (z9x, z9y)


def read_varint(b, i):
    r = s = 0
    while True:
        c = b[i]
        i += 1
        r |= (c & 0x7f) << s
        s += 7
        if c < 0x80:
            return r, i


def varints(b):
    i, out = 0, []
    while i < len(b):
        v, i = read_varint(b, i)
        out.append(v)
    return out


def fields(b):
    i = 0
    while i < len(b):
        k, i = read_varint(b, i)
        f, w = k >> 3, k & 7
        if w == 0:
            v, i = read_varint(b, i)
        elif w == 2:
            n, i = read_varint(b, i)
            v, i = b[i:i + n], i + n
        elif w == 1:
            v, i = b[i:i + 8], i + 8
        elif w == 5:
            v, i = b[i:i + 4], i + 4
        else:
            raise ValueError(w)
        yield f, v


def decode(path):
    """A file's routing tiles: {(x, y): {(lat, lon): (elev_m, ways, [(lat, lon, attr)])}} and the index range."""
    d = open(path, 'rb').read()
    sec = struct.unpack_from('<I', d, 0x0e)[0]
    ztab = struct.unpack_from('<20I', d, sec)
    zb = sec + ztab[Z]
    assert d[zb:zb + 6] == b'BRYTON'
    x0, x1, y0, y1 = struct.unpack_from('<4I', d, zb + 0x10)
    isz, lsz = d[zb + 0x20], d[zb + 0x21]
    doff = struct.unpack_from('<I', d, zb + 0x22)[0]
    h = y1 - y0 + 1
    tiles = {}
    for x in range(x0, x1 + 1):
        for y in range(y0, y1 + 1):
            e = (x - x0) * h + y - y0
            off = int.from_bytes(d[zb + 100 + e * isz:zb + 100 + (e + 1) * isz], 'little')
            if off == (1 << 8 * isz) - 1:
                continue
            p = zb + doff + off
            n = int.from_bytes(d[p:p + lsz], 'little')
            raw = gzip.decompress(d[p + lsz:p + lsz + n])
            top, nodes = {}, []
            for f, v in fields(raw):
                if f == 5:
                    nodes.append(v)
                elif f != 6:
                    top[f] = v
            bits = top[3]
            assert bits == BITS and top[4] == 2
            base = (unzigzag(top[1]), unzigzag(top[2]))
            mask = (1 << bits) - 1

            def pos(k):
                return base[0] + unhalf(k >> bits & mask), base[1] + unhalf(k & mask)
            t = {}
            for nb in nodes:
                k = e5 = None
                ways, edges = [], []
                for f, v in fields(nb):
                    if f == 1:
                        k = v
                    elif f == 2:
                        e5 = v
                    elif f == 3:
                        ways = varints(v)
                    elif f == 4:
                        ed = dict(fields(v))
                        edges.append((*pos(ed[1]), ed.get(2, 0)))
                t[pos(k)] = (e5 // 5 - 500, ways, edges)
            tiles[(x, y)] = t
    return tiles, (x0, x1, y0, y1)


def check(tiles):
    """Every node in its own tile, every edge at both ends (when both tiles are there)."""
    every = {}
    stray = 0
    for c, t in tiles.items():
        for p in t:
            stray += tile_of(*p) != c  # a node on a tile's edge, rounded the other way (1 in 451,537 in a real file)
            every[p] = t[p]
    assert stray * 1000 <= len(every), '%d nodes of %d outside their own tile' % (stray, len(every))
    missing = asym = 0
    for p, (e, ways, edges) in every.items():
        for lat, lon, attr in edges:
            q = every.get((lat, lon))
            if q is None:
                missing += 1
            elif not any((p[0], p[1]) == (a, b) for a, b, _ in q[2]):
                asym += 1
    return len(every), missing, asym


def info(args):
    tiles, rng = decode(args.file)
    n, missing, asym = check(tiles)
    print('tiles', len(tiles), 'range', rng, 'nodes', n, 'edges to nodes not in the file', missing, 'one-way edges', asym)


def fixture(args):
    body, name = build(fixture_net())
    os.makedirs(args.out, exist_ok=True)
    path = os.path.join(args.out, name)
    open(path, 'wb').write(body)
    print(path, len(body), 'bytes')


def selftest(args=None):
    net = fixture_net()
    body, name = build(net)
    tmp = os.path.join(os.environ.get('TMPDIR', '/tmp'), name + '.selftest')
    open(tmp, 'wb').write(body)
    tiles, rng = decode(tmp)
    os.remove(tmp)
    n, missing, asym = check(tiles)
    assert (n, missing, asym) == (len(net.nodes), 0, 0), (n, missing, asym)
    assert len(tiles) >= 4, len(tiles)
    print('route_pack selftest ok:', len(net.nodes), 'nodes in', len(tiles), 'tiles, range', rng, name)


if __name__ == '__main__':
    import argparse
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest='cmd', required=True)
    sub.add_parser('selftest')
    f = sub.add_parser('fixture')
    f.add_argument('out')
    i = sub.add_parser('info')
    i.add_argument('file')
    a = ap.parse_args()
    {'fixture': fixture, 'info': info, 'selftest': selftest}[a.cmd](a)
