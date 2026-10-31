#!/usr/bin/env python3
"""OpenStreetMap vector packs for the unit (src/storage/osm.rs, `\\Rustfw\\Maps\\osm\\`), built from an Overpass
API result: no dependency (pyosmium is not installed here, and a bounding box is what the rider has).

    tools/map_pack.py query --bbox LON0,LAT0,LON1,LAT1 > q.ql
    curl -o area.json --data-urlencode data@q.ql https://overpass-api.de/api/interpreter
    tools/map_pack.py pack --osm area.json --bbox LON0,LAT0,LON1,LAT1 --out OUTDIR [--zooms 11-14]
    tools/map_pack.py selftest
    tools/map_pack.py fixture --out test-data/osm   # the synthetic area the Rust tests draw

Copy OUTDIR's files to `\\Rustfw\\Maps\\osm\\` on the unit's drive (docs/install.md, OSM part). The data is
(c) OpenStreetMap contributors, ODbL (openstreetmap.org/copyright): `pack` writes ATTRIB.TXT next to the packs, and the
unit's About page carries the notice. Keep the bounding box to what the rider needs: one pack file of 16 x 16 tiles at
zoom 14 (about 39 km square) is a few hundred KB to a few MB in a town, and Overpass refuses very large queries.

**Input**: the query's JSON (`out geom`: each way carries its tags and its points, so no node lookup is needed;
relations carry their members' points). Used: `highway` (motorway to service, track, path, footway, bridleway,
steps, cycleway), `railway=rail`, `waterway` (river, canal, stream, drain), closed ways and relation members of
`natural=water`, `landuse=reservoir|basin`, `waterway=riverbank`. Left out: place names, land use, buildings,
contours; a lake made of several ways (an open ring) is not drawn, only closed ways are.

**Pack format** (version 1, little-endian; the Rust reader's module doc is the same text): one file per zoom and
aligned block of 16 x 16 XYZ tiles, `OSM<z>_<x / 16>_<y / 16>.PAK`.
- 0: magic `OSMP`; 4: version u16 = 1; 6: zoom u8; 7: block u8 = 16; 8: x0 u32, 12: y0 u32 (x0 = 16 (x / 16)).
- 16: the index, 256 entries of (H offset u32, H length u32, P offset u32, P length u32), row-major, entry i at
  16 + 16 i. H offset 0: no tile (outside the area). An empty tile inside the area: H = a count of 0, P empty.
- 4112: the blobs, each 4-byte aligned. A tile's H and P are stock's decoded tile cache files (ghidra/
  STORAGE_AND_MAPS.md 4.5 in the bryton repo), which the unit already draws: H = count u32, then records of 10 bytes
  (layer u8: 1 road, 2 water area, 3 waterway; point count u16; offset u32 into P; class u8; sub u8; 0); P = points,
  (x u16, y u16) each, 4096 units a tile side, y down, 0..=4096 (clipped to the tile).
- Road class (stock's table, shown by zoom from zoom 10: 1-4, 12: 1-6, 13: 1-7, 14: 1-12): 1 motorway and trunk,
  2 primary, 3 secondary, 4 tertiary, 6 residential, living street, unclassified, 7 service, 8 track and path
  (sub 7: brown) or cycleway (sub 1: purple), 10 railway; sub 2: a tunnel (hidden on minor roads). Tracks and
  paths exist only at zoom 14: the pack must have it for an MTB map.
- Water rings: outer positive by the surveyor's formula in y-down units, holes negative; water is drawn from zoom 13.
- Caps (the unit's slot): 8000 features and 30000 points a tile; over that the minor roads go first (counted).
Geometry is clipped to each tile (Liang-Barsky for lines, Sutherland-Hodgman for rings) and rounded to the grid;
no further simplification (shortcut: a dense river at zoom 11 could be thinned, when a pack is too big).
"""

import argparse
import json
import math
import os
import struct
import sys
import tempfile

BLOCK = 16
DATA = 16 + 16 * BLOCK * BLOCK
SIDE = 4096
MAX_FEATS, MAX_PTS = 8000, 30000
# stock's road classes shown by zoom (bit = class), src/ui/tiles.rs ROADS_SHOWN
ROADS_SHOWN = [0, 2, 2, 2, 2, 2, 6, 6, 0xe, 0xe, 0x1e, 0x1e, 0x7e, 0xfe, 0x1ffe, 0x1ffe, 0x1ffe]
HIGHWAY = {
    'motorway': (1, 0), 'motorway_link': (1, 0), 'trunk': (1, 0), 'trunk_link': (1, 0),
    'primary': (2, 0), 'primary_link': (2, 0), 'secondary': (3, 0), 'secondary_link': (3, 0),
    'tertiary': (4, 0), 'tertiary_link': (4, 0),
    'unclassified': (6, 0), 'residential': (6, 0), 'living_street': (6, 0), 'pedestrian': (6, 0),
    'service': (7, 0), 'track': (8, 7), 'path': (8, 7), 'footway': (8, 7), 'bridleway': (8, 7),
    'steps': (8, 7), 'cycleway': (8, 1),
}
WATERWAY = {'river': 0, 'canal': 0, 'stream': 13, 'drain': 13, 'ditch': 14}  # the first zoom drawn
ROAD, WATER_AREA, WATERWAY_LAYER = 1, 2, 3


def query(bbox):
    w, s, e, n = bbox
    box = f'({s},{w},{n},{e})'
    return ('[out:json][timeout:300];\n(\n'
            + ''.join(f'  way["{k}"]{box};\n' for k in ('highway', 'waterway'))
            + f'  way["railway"="rail"]{box};\n  way["natural"="water"]{box};\n'
            + f'  way["landuse"~"^(reservoir|basin)$"]{box};\n  relation["natural"="water"]{box};\n);\nout geom;\n')


def lines_of(el):
    return [(p['lon'], p['lat']) for p in el.get('geometry', []) if 'lon' in p]


def features(data):
    """Overpass JSON -> [(layer, class, sub, min zoom, [(lon, lat)], hole)]; rings closed."""
    out = []
    for el in data.get('elements', []):
        t = el.get('tags', {})
        if el['type'] == 'way':
            pts = lines_of(el)
            if len(pts) < 2:
                continue
            closed = pts[0] == pts[-1] and len(pts) >= 4
            if t.get('natural') == 'water' or t.get('landuse') in ('reservoir', 'basin') or t.get('waterway') == 'riverbank':
                if closed:
                    out.append((WATER_AREA, 1, 0, 13, pts, False))
            elif t.get('highway') in HIGHWAY:
                c, sub = HIGHWAY[t['highway']]
                if t.get('tunnel') in ('yes', 'building_passage', 'culvert'):
                    sub = 2
                out.append((ROAD, c, sub, 0, pts, False))
            elif t.get('railway') == 'rail':
                out.append((ROAD, 10, 0, 0, pts, False))
            elif t.get('waterway') in WATERWAY:
                out.append((WATERWAY_LAYER, 1, 0, WATERWAY[t['waterway']], pts, False))
        elif el['type'] == 'relation' and t.get('natural') == 'water':
            for m in el.get('members', []):
                pts = [(p['lon'], p['lat']) for p in m.get('geometry', [])]
                if m.get('type') == 'way' and len(pts) >= 4 and pts[0] == pts[-1]:
                    out.append((WATER_AREA, 1, 0, 13, pts, m.get('role') == 'inner'))
    return out


def tile_xy(lon, lat, z):
    n = 1 << z
    x = (lon + 180) / 360 * n
    y = (1 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2 * n
    return x, y


def clip_line(a, b):
    """Liang-Barsky: the part of segment a-b inside 0..SIDE, or None."""
    t0, t1 = 0.0, 1.0
    dx, dy = b[0] - a[0], b[1] - a[1]
    for p, q in ((-dx, a[0]), (dx, SIDE - a[0]), (-dy, a[1]), (dy, SIDE - a[1])):
        if p == 0:
            if q < 0:
                return None
        else:
            r = q / p
            if p < 0:
                t0 = max(t0, r)
            else:
                t1 = min(t1, r)
            if t0 > t1:
                return None
    return (a[0] + t0 * dx, a[1] + t0 * dy), (a[0] + t1 * dx, a[1] + t1 * dy)


def clip_polyline(pts):
    runs, cur = [], []
    for a, b in zip(pts, pts[1:]):
        c = clip_line(a, b)
        if c is None:
            if len(cur) > 1:
                runs.append(cur)
            cur = []
            continue
        if cur and cur[-1] != c[0]:
            runs.append(cur)
            cur = []
        if not cur:
            cur = [c[0]]
        cur.append(c[1])
    if len(cur) > 1:
        runs.append(cur)
    return runs


def clip_ring(pts):
    """Sutherland-Hodgman against the tile's square."""
    ring = pts[:-1]
    for inside, cut in (
            (lambda p: p[0] >= 0, lambda a, b: (0, a[1] + (b[1] - a[1]) * (0 - a[0]) / (b[0] - a[0]))),
            (lambda p: p[0] <= SIDE, lambda a, b: (SIDE, a[1] + (b[1] - a[1]) * (SIDE - a[0]) / (b[0] - a[0]))),
            (lambda p: p[1] >= 0, lambda a, b: (a[0] + (b[0] - a[0]) * (0 - a[1]) / (b[1] - a[1]), 0)),
            (lambda p: p[1] <= SIDE, lambda a, b: (a[0] + (b[0] - a[0]) * (SIDE - a[1]) / (b[1] - a[1]), SIDE))):
        out = []
        for i, b in enumerate(ring):
            a = ring[i - 1]
            if inside(b):
                if not inside(a):
                    out.append(cut(a, b))
                out.append(b)
            elif inside(a):
                out.append(cut(a, b))
        ring = out
        if not ring:
            return []
    return ring + ring[:1]


def area2(pts):
    return sum(a[0] * b[1] - b[0] * a[1] for a, b in zip(pts, pts[1:] + pts[:1]))


def quantise(pts):
    out = []
    for x, y in pts:
        p = (min(max(round(x), 0), SIDE), min(max(round(y), 0), SIDE))
        if not out or out[-1] != p:
            out.append(p)
    return out


def build(feats, bbox, zooms):
    """-> ({filename: bytes}, {zoom: dropped features}). Tiles of the box's rectangle are all written."""
    w, s, e, n = bbox
    files, dropped = {}, {}
    for z in zooms:
        x0, y1 = tile_xy(w, s, z)
        x1, y0 = tile_xy(e, n, z)
        x0, x1, y0, y1 = int(x0), int(x1), int(y0), int(y1)
        shown = ROADS_SHOWN[min(z, 16)]
        tiles = {(x, y): [] for x in range(x0, x1 + 1) for y in range(y0, y1 + 1)}
        for layer, c, sub, minz, pts, hole in feats:
            if z < minz or (layer == ROAD and not shown >> c & 1):
                continue
            g = [tile_xy(lo, la, z) for lo, la in pts]
            tx0, tx1 = int(min(p[0] for p in g)), int(max(p[0] for p in g))
            ty0, ty1 = int(min(p[1] for p in g)), int(max(p[1] for p in g))
            for tx in range(max(tx0, x0), min(tx1, x1) + 1):
                for ty in range(max(ty0, y0), min(ty1, y1) + 1):
                    local = [((gx - tx) * SIDE, (gy - ty) * SIDE) for gx, gy in g]
                    if layer == WATER_AREA:
                        ring = quantise(clip_ring(local))
                        if len(ring) >= 4 and ring[0] == ring[-1] or len(ring) >= 3:
                            if ring[0] != ring[-1]:
                                ring.append(ring[0])
                            if len(ring) < 4 or area2(ring[:-1]) == 0:
                                continue
                            if (area2(ring[:-1]) > 0) == hole:
                                ring.reverse()
                            tiles[(tx, ty)].append((layer, c, sub, ring))
                    else:
                        for run in clip_polyline(local):
                            run = quantise(run)
                            if len(run) >= 2:
                                tiles[(tx, ty)].append((layer, c, sub, run))
        blocks = {}
        for (x, y), fs in tiles.items():
            fs.sort(key=lambda f: (f[0] != WATER_AREA, f[0] != WATERWAY_LAYER, f[1]))  # the keepers first
            keep, pts = [], 0
            for f in fs:
                if len(keep) < MAX_FEATS and pts + len(f[3]) <= MAX_PTS:
                    keep.append(f)
                    pts += len(f[3])
                else:
                    dropped[z] = dropped.get(z, 0) + 1
            blocks.setdefault((x // BLOCK, y // BLOCK), {})[(x, y)] = keep
        for (bx, by), ts in blocks.items():
            files[f'OSM{z}_{bx}_{by}.PAK'] = pack_block(z, bx, by, ts)
    return files, dropped


def pack_block(z, bx, by, tiles):
    out = bytearray(struct.pack('<4sHBBII', b'OSMP', 1, z, BLOCK, bx * BLOCK, by * BLOCK))
    out += bytes(DATA - len(out))
    for (x, y), fs in tiles.items():
        h, p = bytearray(struct.pack('<I', len(fs))), bytearray()
        for layer, c, sub, pts in fs:
            h += struct.pack('<BHIBBB', layer, len(pts), len(p), c, sub, 0)
            for u, v in pts:
                p += struct.pack('<HH', u, v)
        ho = len(out)
        out += h
        out += bytes(-len(out) % 4)
        po = len(out)
        out += p
        out += bytes(-len(out) % 4)
        struct.pack_into('<IIII', out, 16 + 16 * ((y % BLOCK) * BLOCK + x % BLOCK), ho, len(h), po, len(p))
    return bytes(out)


def read_pack(b):
    """An independent reader: {(x, y): [(layer, class, sub, points)]}."""
    magic, ver, z, blk, x0, y0 = struct.unpack_from('<4sHBBII', b)
    assert magic == b'OSMP' and ver == 1 and blk == BLOCK
    tiles = {}
    for i in range(BLOCK * BLOCK):
        ho, hl, po, pl = struct.unpack_from('<IIII', b, 16 + 16 * i)
        if ho == 0:
            continue
        assert ho >= DATA and po >= DATA and ho % 4 == 0 and po % 4 == 0
        assert ho + hl <= len(b) and po + pl <= len(b)
        (n,) = struct.unpack_from('<I', b, ho)
        assert hl == 4 + 10 * n
        fs = []
        for k in range(n):
            layer, np_, off, c, sub, _ = struct.unpack_from('<BHIBBB', b, ho + 4 + 10 * k)
            assert off + 4 * np_ <= pl
            fs.append((layer, c, sub, [struct.unpack_from('<HH', b, po + off + 4 * j) for j in range(np_)]))
        tiles[(x0 + i % BLOCK, y0 + i // BLOCK)] = fs
    return z, tiles


def parse_zooms(s):
    a, _, b = s.partition('-')
    return list(range(int(a), int(b or a) + 1))


BBOX = (0.001, -0.060, 0.050, -0.001)


def synthetic():
    """A made-up Overpass result round 0 N 0 E, 5 km wide: three tiles of zoom 14 a side (2.4 km each), the
    south one empty. The Rust tests (src/ui/tiles.rs, test-data/osm) draw the committed `fixture` of it."""

    def way(tags, pts):
        return {'type': 'way', 'tags': tags, 'geometry': [{'lon': x, 'lat': y} for x, y in pts]}

    lake = [(0.010, -0.010), (0.020, -0.010), (0.020, -0.020), (0.010, -0.020), (0.010, -0.010)]
    isle = [(0.013, -0.013), (0.013, -0.017), (0.017, -0.017), (0.017, -0.013), (0.013, -0.013)]
    return {'elements': [
        way({'highway': 'motorway'}, [(0.002, -0.030), (0.048, -0.032)]),  # crosses two tiles
        way({'highway': 'track'}, [(0.002, -0.005), (0.030, -0.005)]),
        way({'highway': 'path', 'tunnel': 'yes'}, [(0.005, -0.002), (0.005, -0.038)]),
        way({'highway': 'cycleway'}, [(0.045, -0.002), (0.045, -0.020)]),
        way({'highway': 'residential'}, [(0.002, -0.025), (0.040, -0.025)]),
        way({'highway': 'corridor'}, [(0.002, -0.026), (0.040, -0.026)]),  # not a road: left out
        way({'railway': 'rail'}, [(0.002, -0.035), (0.048, -0.035)]),
        way({'waterway': 'river'}, [(0.030, -0.002), (0.031, -0.038)]),
        way({'waterway': 'stream'}, [(0.040, -0.002), (0.041, -0.038)]),  # zoom 13 and over only
        {'type': 'relation', 'tags': {'natural': 'water'}, 'members': [
            {'type': 'way', 'role': 'outer', 'geometry': [{'lon': x, 'lat': y} for x, y in lake]},
            {'type': 'way', 'role': 'inner', 'geometry': [{'lon': x, 'lat': y} for x, y in isle]}]},
    ]}


def tiles_rust_roads():
    """ROADS_SHOWN as src/ui/tiles.rs has it."""
    import re
    src = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'src', 'ui', 'tiles.rs')).read()
    m = re.search(r'const ROADS_SHOWN: \[u16; \d+\] = \[([^\]]*)\];', src)
    return [int(v, 0) for v in m.group(1).replace('\n', ' ').split(',') if v.strip()]


def selftest():
    bbox, data = BBOX, synthetic()
    assert tiles_rust_roads() == ROADS_SHOWN, 'ROADS_SHOWN differs from src/ui/tiles.rs'
    feats = features(data)
    assert len(feats) == 9 + 1, len(feats)  # eight ways kept, the lake and its island
    files, dropped = build(feats, bbox, [11, 12, 13, 14])
    assert not dropped
    assert all(n.startswith('OSM') and n.endswith('.PAK') for n in files)
    seen = {}
    for name, b in files.items():
        z, tiles = read_pack(b)
        assert name.startswith(f'OSM{z}_')
        for t, fs in tiles.items():
            assert all(0 <= v <= SIDE for f in fs for p in f[3] for v in p)
            for layer, c, sub, pts in fs:
                seen.setdefault((z, layer, c, sub), []).append((t, pts))
    # zoom 14: tracks, paths, the tunnel and the cycleway exist; at zoom 12 only classes 1-6 (the residential road
    # too, class 6); stream from 13; water from 13
    assert (14, ROAD, 8, 7) in seen and (14, ROAD, 8, 1) in seen and (14, ROAD, 10, 0) in seen
    assert (12, ROAD, 8, 7) not in seen and (12, ROAD, 1, 0) in seen and (12, ROAD, 6, 0) in seen
    assert (13, ROAD, 7, 0) not in seen and (13, ROAD, 10, 0) not in seen
    assert (12, WATERWAY_LAYER, 1, 0) in seen and len(seen[(12, WATERWAY_LAYER, 1, 0)]) >= 1
    assert (13, WATERWAY_LAYER, 1, 0) in seen and (12, WATER_AREA, 1, 0) not in seen and (14, WATER_AREA, 1, 0) in seen
    tunnel = [k for k in seen if k[:3] == (14, ROAD, 8) and k[3] == 2]
    assert tunnel, 'the tunnel keeps sub 2 (the unit hides it for minor roads)'
    # the motorway crosses two tile borders: its three parts meet on them
    m = sorted(seen[(14, ROAD, 1, 0)])
    assert len(m) == 3, m
    for (ta, pa), (tb, pb) in zip(m, m[1:]):
        assert ta[0] + 1 == tb[0] and pa[-1][0] == SIDE and pb[0][0] == 0 and abs(pa[-1][1] - pb[0][1]) <= 1, (ta, tb)
    # water: the lake's ring positive, the island's negative, the stream absent at zoom 11
    rings = [pts for _, pts in seen[(14, WATER_AREA, 1, 0)]]
    signs = sorted(area2(r[:-1]) > 0 for r in rings)
    assert signs[0] is False and signs[-1] is True, signs
    assert (11, WATERWAY_LAYER, 1, 0) in seen and len([k for k in seen if k[0] == 11 and k[1] == WATERWAY_LAYER]) == 1
    # a tile of the box with nothing in it is written as an empty tile (the unit then knows it is in the area)
    assert any(not fs for b in files.values() for fs in read_pack(b)[1].values())
    # the query names the box and the data used
    q = query(bbox)
    assert '["highway"](-0.06,0.001,-0.001,0.05)' in q and 'out geom' in q
    # the files go to disk and read back as written
    with tempfile.TemporaryDirectory() as d:
        write(files, d)
        for name, b in files.items():
            assert open(os.path.join(d, name), 'rb').read() == b
        assert 'OpenStreetMap contributors' in open(os.path.join(d, 'ATTRIB.TXT')).read()
    # the committed fixture (test-data/osm, which the Rust tests read) equals a fresh one
    here = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'test-data', 'osm')
    with tempfile.TemporaryDirectory() as d:
        fixture(d)
        fresh = sorted(os.listdir(d))
        assert fresh == sorted(os.listdir(here)), (fresh, sorted(os.listdir(here)))
        for n in fresh:
            assert open(os.path.join(d, n), 'rb').read() == open(os.path.join(here, n), 'rb').read(), n
    print('map_pack selftest ok:', len(files), 'packs,', sum(len(b) for b in files.values()), 'bytes')


def fixture(out):
    """The synthetic area at zoom 14 only (test-data/osm)."""
    files, _ = build(features(synthetic()), BBOX, [14])
    write(files, out)


def write(files, out):
    os.makedirs(out, exist_ok=True)
    for name, b in files.items():
        with open(os.path.join(out, name + '.part'), 'wb') as f:
            f.write(b)
        os.replace(os.path.join(out, name + '.part'), os.path.join(out, name))
    with open(os.path.join(out, 'ATTRIB.TXT'), 'w') as f:
        f.write('Map data (c) OpenStreetMap contributors, ODbL: https://www.openstreetmap.org/copyright\n')


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    sub = ap.add_subparsers(dest='cmd', required=True)
    for name in ('query', 'pack'):
        p = sub.add_parser(name)
        p.add_argument('--bbox', required=True, help='LON0,LAT0,LON1,LAT1 (west, south, east, north)')
        if name == 'pack':
            p.add_argument('--osm', required=True, help="the Overpass JSON")
            p.add_argument('--out', required=True)
            p.add_argument('--zooms', default='11-14')
    sub.add_parser('selftest')
    sub.add_parser('fixture').add_argument('--out', required=True)
    a = ap.parse_args()
    if a.cmd == 'selftest':
        return selftest()
    if a.cmd == 'fixture':
        return fixture(a.out)
    bbox = tuple(float(v) for v in a.bbox.split(','))
    if len(bbox) != 4 or not (bbox[0] < bbox[2] and bbox[1] < bbox[3]):
        sys.exit('--bbox wants LON0,LAT0,LON1,LAT1 with west < east and south < north')
    if a.cmd == 'query':
        return print(query(bbox), end='')
    with open(a.osm) as f:
        feats = features(json.load(f))
    files, dropped = build(feats, bbox, parse_zooms(a.zooms))
    write(files, a.out)
    print(f'{len(feats)} features, {len(files)} packs, {sum(len(b) for b in files.values()) / 1e6:.2f} MB in {a.out}')
    for z, n in sorted(dropped.items()):
        print(f'zoom {z}: {n} minor features dropped to fit the tile caps')


if __name__ == '__main__':
    main()
