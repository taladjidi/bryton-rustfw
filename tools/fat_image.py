#!/usr/bin/env python3
"""Makes test-data/fat32.sparse: a real FAT32 volume made by macOS (newfs_msdos, 16 KB clusters and 32 reserved
sectors as on the unit, BOARD.md 15) holding synthetic map tiles in stock's cache format and files that exercise
the FAT reader (long names, a directory over several clusters with deleted entries, a fragmented file).

The tiles are made up (AGENTS.md, "Public repository"): around 0 N 0 E, the four Zoom14 tiles and the four Zoom12
tiles that meet there, each with a grid of roads of every class, a motorway across, a tunnel, a cycleway, an
unpaved track, a rail line, a river, a lake with an island, and points and names the renderer must skip.
Format (ghidra/STORAGE_AND_MAPS.md 4.5): H = u32 count, then 10-byte records (u8 layer, u16 points, u32 offset
into P, u8 class, u8 sub, u8 oneway); P = i16 x, y points (extent 4096), then a name for layer 6 and for points.
An empty tile is a 0 count and one 0 byte (stock's Map_WriteEmptyTileCache).

The 1.1 GB image is mostly zeros, so it is stored sparse: records of u32 LE first sector, u32 LE sector count,
then the sectors (fat.rs tests expand it).

usage (on a Mac): tools/fat_image.py
"""
import math, os, shutil, struct, subprocess, tempfile

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'test-data', 'fat32.sparse')
# layers
ROAD, WATER, WATERWAY, PLACE, POI, ROAD_NAME = 1, 2, 3, 4, 5, 6


def tile(z):
    """One synthetic tile: (H bytes, P bytes). The same in every tile, so the roads meet across tile edges."""
    feats = []  # (layer, class, sub, points, name)
    # a grid: 12 horizontal and 12 vertical roads, every class 1..12 (mixed, so any part of a tile shows major and
    # minor roads), edge to edge
    for i, c in enumerate([1, 6, 3, 9, 2, 7, 4, 8, 5, 12, 11, 10]):
        at = 170 + i * 330
        feats.append((ROAD, c, 0, [(-64, at), (4160, at)], None))
        feats.append((ROAD, c, 0, [(at, -64), (at, 4160)], None))
        feats.append((ROAD_NAME, c, 0, [(-64, at), (4160, at)], 'Road %d' % c))
    # a motorway across, in pieces
    feats.append((ROAD, 1, 0, [(-64, 4160), (1000, 3000), (2600, 1500), (4160, -64)], None))
    # near the lake: a tunnel (minor road, sub 2: skipped), a rail line, a cycleway (path, sub 1), an unpaved track
    # (track, sub 7)
    feats.append((ROAD, 6, 2, [(500, 1300), (1500, 1300)], None))
    feats.append((ROAD, 11, 0, [(-64, 1400), (4160, 1420)], None))
    feats.append((ROAD, 9, 1, [(700, 1550), (1100, 1650)], None))
    feats.append((ROAD, 8, 7, [(700, 1650), (1100, 1550)], None))
    # a river: a wave across the tile
    feats.append((WATERWAY, 2, 0, [(x, 3300 + int(200 * math.sin(x / 400))) for x in range(-64, 4161, 128)], None))
    # a lake (outer ring clockwise in tile coordinates, y down: positive area) and its island (a hole)
    lake = [(600 + int(450 * math.cos(a / 20 * math.tau)), 900 + int(350 * math.sin(a / 20 * math.tau)))
            for a in range(20)]
    feats.append((WATER, 2, 0, lake + [lake[0]], None))
    island = [(600 + int(120 * math.cos(a / 8 * math.tau)), 900 - int(100 * math.sin(a / 8 * math.tau)))
              for a in range(8)]
    feats.append((WATER, 2, 0, island + [island[0]], None))
    # points to skip
    feats.append((POI, 6, 0, [(2000, 2000)], 'Cafe'))
    feats.append((PLACE, 7, 0, [(3000, 1000)], 'Village'))
    h, p = bytearray(struct.pack('<I', len(feats))), bytearray()
    for layer, cls, sub, pts, name in feats:
        h += struct.pack('<BHIBBB', layer, len(pts), len(p), cls, sub, 0)
        for x, y in pts:
            p += struct.pack('<hh', x, y)
        if name is not None:
            b = name.encode()
            p += bytes([len(b)]) + b + b'\0'
    return bytes(h), bytes(p)


def fill(m):
    t = os.path.join(m, 'Map', 'Data', 'Temp')
    with_tiles = {14: [(8191, 8191), (8191, 8192), (8192, 8191), (8192, 8192)],
                  12: [(2047, 2047), (2047, 2048), (2048, 2047), (2048, 2048)]}
    for z, xys in with_tiles.items():
        h, p = tile(z)
        for hp, data in (('H', h), ('P', p)):
            d = os.path.join(t, 'Zoom%d' % z, hp)
            os.makedirs(d, exist_ok=True)
            for x, y in xys:
                with open(os.path.join(d, '%d_%d_0.bin' % (x, y)), 'wb') as f:
                    f.write(data)
            with open(os.path.join(d, '%d_%d_0.bin' % (with_tiles[z][0][0] - 1, with_tiles[z][0][1])), 'wb') as f:
                f.write(b'\0' * 5 if hp == 'H' else b'')  # an empty tile, west of the others
    with open(os.path.join(t, 'version.ini'), 'wb') as f:
        f.write(b'1.2.1\0')
    os.makedirs(os.path.join(m, 'Progress Planner'))
    with open(os.path.join(m, 'Progress Planner', 'A Long File Name.json'), 'wb') as f:
        f.write(bytes((i * 7 + 3) & 0xff for i in range(40000)))  # 3 clusters
    with open(os.path.join(m, 'SHORT.TXT'), 'wb') as f:
        f.write(b'hello')
    d = os.path.join(m, 'many')  # over several clusters (512 entries each)
    os.makedirs(d)
    for i in range(600):
        open(os.path.join(d, 'file number %03d.txt' % i), 'wb').close()
    with open(os.path.join(d, 'last.bin'), 'wb') as f:
        f.write(bytes(range(256)) * 4)
    # macOS left a ._ file (one cluster) beside each file: deleting them leaves holes, which the next file fills
    subprocess.run(['find', m, '-name', '._*', '-delete'], check=True)
    with open(os.path.join(m, 'frag.bin'), 'wb') as f:
        f.write(b''.join(struct.pack('<I', i) for i in range(50000)))  # 200 KB over 13 clusters
    subprocess.run(['find', m, '-name', '._*', '-delete'], check=True)
    shutil.rmtree(os.path.join(m, '.fseventsd'), ignore_errors=True)


def run(*cmd):
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


def main():
    tmp = tempfile.mkdtemp()
    img, mnt = os.path.join(tmp, 'fat.img'), os.path.join(tmp, 'mnt')
    run('mkfile', '-n', '1100m', img)  # 16 KB clusters need over 65525 clusters for FAT32: about 1.1 GB
    dev = run('hdiutil', 'attach', '-imagekey', 'diskimage-class=CRawDiskImage', '-nomount', img).split()[0]
    try:
        run('newfs_msdos', '-F', '32', '-c', '32', '-v', 'BRYTON', dev.replace('/dev/disk', '/dev/rdisk'))
    finally:
        run('hdiutil', 'detach', dev)
    os.makedirs(mnt)
    dev = run('hdiutil', 'attach', '-imagekey', 'diskimage-class=CRawDiskImage', '-mountpoint', mnt, img).split()[0]
    try:
        fill(mnt)
    finally:
        run('hdiutil', 'detach', dev)
    data, zero, out = open(img, 'rb').read(), bytes(512), bytearray()
    s, n = 0, len(data) // 512
    while s < n:
        if data[s * 512:(s + 1) * 512] == zero:
            s += 1
            continue
        e = s
        while e < n and data[e * 512:(e + 1) * 512] != zero:
            e += 1
        out += struct.pack('<II', s, e - s) + data[s * 512:e * 512]
        s = e
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    open(OUT, 'wb').write(out)
    print(len(out), 'bytes ->', OUT)
    shutil.rmtree(tmp)


main()
