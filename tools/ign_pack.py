#!/usr/bin/env python3
"""IGN Plan v2 maps for the unit: fetch the tiles once, pack them in the format the firmware reads
(src/storage/ign.rs, `\\Rustfw\\Maps\\`).

    tools/ign_pack.py fetch --bbox LON0,LAT0,LON1,LAT1 --zooms 11-16 --cache DIR
    tools/ign_pack.py pack --cache DIR --out OUTDIR [--colours 256]
    tools/ign_pack.py selftest
    tools/ign_pack.py fixture OUTDIR        # the synthetic test packs in test-data/ign (no real map)

Licence: Plan IGN v2 is under the Licence Ouverte / Etalab 2.0; the unit shows "(c) IGN" on the map, and `pack`
writes ATTRIB.TXT next to the packs. SCAN 25 is not open to private individuals: never fetch it.

**Cache** (`fetch` writes it, `pack` reads it): `DIR/{z}/{x}/{y}.png`, Web Mercator XYZ tiles of 256 pixels.
A tile the server does not have (outside France) leaves `DIR/{z}/{x}/{y}.none`, so a rerun does not ask again.
A file is written to `.part` then renamed: an interrupted fetch never leaves half a tile, and a rerun resumes.
The endpoint (no key): data.geopf.fr WMTS, layer GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2, TILEMATRIXSET PM, whose tile
matrix z, TILEROW y, TILECOL x are the XYZ tile (z, x, y).

**Pack format** (version 1, little-endian), one file per zoom and aligned block of 32 x 32 tiles, named
`IGN<z>_<x / 32>_<y / 32>.PAK` (a FAT file stays far below 4 GB, and reaching a tile walks a short cluster chain):
- 0: magic `IGNP`; 4: version u16 = 1; 6: zoom u8; 7: block u8 = 32 (columns = rows);
  8: x0 u32, 12: y0 u32, the block's first tile (x0 = 32 (x / 32)).
- 16: the index, 32 x 32 entries of (offset u32, length u32), row-major (entry (y - y0) 32 + (x - x0)), at
  16 + 8 i: an entry never crosses a sector, so a tile is found with one sector read. Offset 0: no tile.
- then the tiles: n - 1 (u8), n palette colours (RGB565 u16, 1 <= n <= 256), then the 256 x 256 colour indices
  (one byte a pixel, rows top down) as one LZ4 block (github.com/lz4/lz4 doc/lz4_Block_format.md: no frame, no
  size prefix; the size is always 65536).

Colours: PIL's median cut to `--colours` (256 by default, no dithering: dithering breaks LZ4's runs and makes text
grainy), each palette colour then rounded to RGB565, which is all the panel shows. Measured on 268 real tiles
(a town and a rural area, zooms 11 to 16; bytes a tile, PSNR against the RGB565 original): 64 colours 21.9 KB,
34 dB, and the rare colours go: the purple cycle route and blue icons turn grey; 128: 24.2 KB, 38 dB, the cycle
route kept, green text lost; 256: 26.2 KB, 42 dB, nothing visible lost. The index image is one byte a pixel
whatever the count, so 256 costs only bytes on the card (+20 % over 64), nothing in decoding.
"""

import argparse
import concurrent.futures
import math
import os
import struct
import sys
import time
import urllib.error
import urllib.request

URL = ('https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0'
       '&LAYER=GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2&STYLE=normal&TILEMATRIXSET=PM'
       '&TILEMATRIX={z}&TILEROW={y}&TILECOL={x}&FORMAT=image/png')
AGENT = 'bryton-rustfw-ign-pack/1 (offline maps for one bike computer; github.com/taladjidi/bryton-rustfw)'
MAGIC, VERSION, BLOCK, SIDE = b'IGNP', 1, 32, 256
HEAD = 16
ATTRIB = ('Cartes : Plan IGN v2, (c) IGN (Institut national de l\'information geographique et forestiere), '
          'Licence Ouverte / Etalab 2.0, https://www.etalab.gouv.fr/licence-ouverte-open-licence\n')


def tile(lon, lat, z):
    """The XYZ tile (x, y) under a position (wiki.openstreetmap.org/wiki/Slippy_map_tilenames)."""
    n = 1 << z
    r = math.radians(lat)
    return int((lon + 180) / 360 * n), int((1 - math.asinh(math.tan(r)) / math.pi) / 2 * n)


def zooms(s):
    a, _, b = s.partition('-')
    return range(int(a), int(b or a) + 1)


def fetch(args):
    lon0, lat0, lon1, lat1 = (float(v) for v in args.bbox.split(','))
    gap = 1 / args.rate
    todo = []
    for z in zooms(args.zooms):
        (xa, ya), (xb, yb) = tile(min(lon0, lon1), max(lat0, lat1), z), tile(max(lon0, lon1), min(lat0, lat1), z)
        todo += [(z, x, y) for x in range(xa, xb + 1) for y in range(ya, yb + 1)]
    print(f'{len(todo)} tiles', file=sys.stderr)
    got = skipped = missing = 0
    last = 0.0
    for i, (z, x, y) in enumerate(todo):
        base = os.path.join(args.cache, str(z), str(x), str(y))
        if os.path.exists(base + '.png') or os.path.exists(base + '.none'):
            skipped += 1
            continue
        os.makedirs(os.path.dirname(base), exist_ok=True)
        for attempt in range(5):
            time.sleep(max(0.0, last + gap - time.monotonic()))
            last = time.monotonic()
            try:
                req = urllib.request.Request(URL.format(z=z, x=x, y=y), headers={'User-Agent': AGENT})
                with urllib.request.urlopen(req, timeout=30) as r:
                    data = r.read()
                if not data.startswith(b'\x89PNG'):
                    raise ValueError('not a PNG')
                with open(base + '.part', 'wb') as f:
                    f.write(data)
                os.replace(base + '.part', base + '.png')
                got += 1
                break
            except urllib.error.HTTPError as e:
                if e.code in (400, 404):  # outside the layer's extent
                    open(base + '.none', 'wb').close()
                    missing += 1
                    break
                wait = 2 ** attempt
            except (urllib.error.URLError, OSError, ValueError):
                wait = 2 ** attempt
            print(f'{z}/{x}/{y}: retry in {wait} s', file=sys.stderr)
            time.sleep(wait)
        if i % 500 == 0:
            print(f'{i}/{len(todo)}: {got} fetched, {skipped} already there, {missing} none', file=sys.stderr)
    print(f'done: {got} fetched, {skipped} already there, {missing} none', file=sys.stderr)


def rgb565(r, g, b):
    return (r >> 3) << 11 | (g >> 2) << 5 | b >> 3


def encode(img, colours):
    """A tile's blob: palette count - 1, the RGB565 palette, the LZ4 block of the indices."""
    import lz4.block
    from PIL import Image
    if img.mode != 'P':
        img = img.convert('RGB').quantize(colours, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
    idx = img.tobytes()
    assert img.size == (SIDE, SIDE) and len(idx) == SIDE * SIDE
    n = max(idx) + 1
    pal = img.getpalette()[:3 * n]
    pal += [0] * (3 * n - len(pal))
    out = bytes([n - 1]) + b''.join(struct.pack('<H', rgb565(*pal[3 * i:3 * i + 3])) for i in range(n))
    block = lz4.block.compress(idx, mode='high_compression', compression=12, store_size=False)
    assert lz4.block.decompress(block, uncompressed_size=len(idx)) == idx
    return out + block


def encode_file(job):
    from PIL import Image
    path, colours = job
    with Image.open(path) as img:
        return encode(img, colours)


def write_pack(path, z, bx, by, blobs):
    """`blobs`: {(x, y): bytes} inside block (bx, by)."""
    index = bytearray(8 * BLOCK * BLOCK)
    body = bytearray()
    at = HEAD + len(index)
    for (x, y), b in sorted(blobs.items(), key=lambda t: (t[0][1], t[0][0])):
        i = (y - by * BLOCK) * BLOCK + (x - bx * BLOCK)
        struct.pack_into('<II', index, 8 * i, at + len(body), len(b))
        body += b
    head = MAGIC + struct.pack('<HBBII', VERSION, z, BLOCK, bx * BLOCK, by * BLOCK)
    with open(path + '.part', 'wb') as f:
        f.write(head + index + body)
    os.replace(path + '.part', path)
    return HEAD + len(index) + len(body)


def pack(args):
    os.makedirs(args.out, exist_ok=True)
    blocks = {}
    for z in sorted(int(d) for d in os.listdir(args.cache) if d.isdigit()):
        for xd in os.listdir(os.path.join(args.cache, str(z))):
            if not xd.isdigit():
                continue
            for f in os.listdir(os.path.join(args.cache, str(z), xd)):
                if f.endswith('.png') and f[:-4].isdigit():
                    x, y = int(xd), int(f[:-4])
                    blocks.setdefault((z, x // BLOCK, y // BLOCK), []).append((x, y))
    total = tiles = 0
    with concurrent.futures.ProcessPoolExecutor() as pool:
        for (z, bx, by), xys in sorted(blocks.items()):
            jobs = [(os.path.join(args.cache, str(z), str(x), f'{y}.png'), args.colours) for x, y in xys]
            blobs = dict(zip(xys, pool.map(encode_file, jobs, chunksize=8)))
            size = write_pack(os.path.join(args.out, f'IGN{z}_{bx}_{by}.PAK'), z, bx, by, blobs)
            total, tiles = total + size, tiles + len(xys)
            print(f'IGN{z}_{bx}_{by}.PAK: {len(xys)} tiles, {size} bytes, {size // len(xys)} a tile', file=sys.stderr)
    with open(os.path.join(args.out, 'ATTRIB.TXT'), 'w') as f:
        f.write(ATTRIB)
    print(f'{len(blocks)} packs, {tiles} tiles, {total} bytes ({total / 2**20:.1f} MB, {total // max(tiles, 1)} a '
          'tile)', file=sys.stderr)


def lz4_decode(src, size):
    """LZ4 block decoder written from the spec (doc/lz4_Block_format.md), to check python-lz4's output is a plain
    block: token (literals << 4 | match - 4), 255-run length bytes, a 2-byte little-endian offset (0 invalid),
    bytes copied one at a time (overlap), the last sequence literals only."""
    out, i = bytearray(), 0

    def length(n):
        nonlocal i
        if n == 15:
            while True:
                b = src[i]
                i += 1
                n += b
                if b != 255:
                    break
        return n
    while True:
        t = src[i]
        i += 1
        lit = length(t >> 4)
        out += src[i:i + lit]
        i += lit
        if i == len(src):
            break
        off = src[i] | src[i + 1] << 8
        i += 2
        assert 0 < off <= len(out)
        for _ in range(length(t & 15) + 4):
            out.append(out[-off])
    assert len(out) == size
    return bytes(out)


def synthetic(z, x, y):
    """A made-up tile from a formula (no map): global grid lines, a band, a water disc, 1-pixel lines; tiles join.
    src/storage/ign.rs's tests compute the same formula."""
    from PIL import Image
    px = bytearray(SIDE * SIDE)
    for j in range(SIDE):
        for i in range(SIDE):
            gx, gy = x * SIDE + i, y * SIDE + j
            c = 0
            if gx % 64 == 0 or gy % 64 == 0:
                c = 1
            if (gx + gy) % 97 < 6:
                c = 2
            if (i - 128) ** 2 + (j - 128) ** 2 < (20 + (x + y + z) % 3 * 10) ** 2:
                c = 3
            if i == j:
                c = 4
            px[j * SIDE + i] = c
    img = Image.frombytes('P', (SIDE, SIDE), bytes(px))
    img.putpalette(list(sum(SYNTH_RGB, ())))
    return img


SYNTH_RGB = [(250, 248, 240), (160, 160, 160), (230, 120, 40), (90, 170, 220), (20, 20, 20)]


def fixture(args):
    """Synthetic packs for the tests: zooms 13 to 16, the 4 x 4 tiles around 0 N 0 E (the sim's origin), so they
    straddle 4 blocks at each zoom; one tile left out (the missing tile test)."""
    os.makedirs(args.out, exist_ok=True)
    for z in range(13, 17):
        h = 1 << (z - 1)
        blocks = {}
        for x in range(h - 2, h + 2):
            for y in range(h - 2, h + 2):
                if (z, x, y) != (16, h, h + 1):
                    blocks.setdefault((x // BLOCK, y // BLOCK), {})[(x, y)] = encode(synthetic(z, x, y), 64)
        for (bx, by), blobs in blocks.items():
            write_pack(os.path.join(args.out, f'IGN{z}_{bx}_{by}.PAK'), z, bx, by, blobs)


def selftest(_):
    import lz4.block
    import random
    rnd = random.Random(1)
    for n in (0, 1, 5, 13, 100, 70000):
        data = bytes(rnd.choice(b'aab\x00') for _ in range(n)) + bytes(n % 300)
        assert lz4_decode(lz4.block.compress(data, mode='high_compression', store_size=False), len(data)) == data
    blob = encode(synthetic(16, 32768, 32768), 64)
    n = blob[0] + 1
    assert n == len(SYNTH_RGB)
    assert lz4_decode(blob[1 + 2 * n:], SIDE * SIDE) == synthetic(16, 32768, 32768).tobytes()
    assert tile(0.0001, -0.0001, 16) == (32768, 32768)
    print('ok')


def main():
    p = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    sub = p.add_subparsers(dest='cmd', required=True)
    f = sub.add_parser('fetch')
    f.add_argument('--bbox', required=True, help='lon0,lat0,lon1,lat1 (degrees)')
    f.add_argument('--zooms', default='11-16')
    f.add_argument('--cache', required=True)
    f.add_argument('--rate', type=float, default=4.0, help='requests a second at most')
    k = sub.add_parser('pack')
    k.add_argument('--cache', required=True)
    k.add_argument('--out', required=True)
    k.add_argument('--colours', type=int, default=256)
    x = sub.add_parser('fixture')
    x.add_argument('out')
    sub.add_parser('selftest')
    a = p.parse_args()
    {'fetch': fetch, 'pack': pack, 'fixture': fixture, 'selftest': selftest}[a.cmd](a)


if __name__ == '__main__':
    main()
