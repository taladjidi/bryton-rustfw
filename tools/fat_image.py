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

It also holds a made-up `\Activities` (FIT rides with known power, a stop, compressed timestamps, a bad CRC, a
cut file, stock's `tmp.fit`, a GNSS log) and `\System\*.ini` in stock's layout, for stock.rs (the values the
tests expect are worked out next to each ride), a made-up `\Workout\calendar` of planned workouts in stock's
layout, for storage/workout_file.rs, and made-up courses (`\PlanTrip` in Bryton's route layout, `\Courses` as a Garmin
FIT course and as GPX) near 0 N 0 E, for storage/course_file.rs.

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
    rides(m)
    workouts(m)
    courses(m)
    shutil.rmtree(os.path.join(m, '.fseventsd'), ignore_errors=True)


def fit_crc(data, crc=0):
    """FIT's CRC-16 (fit.rs `crc`)."""
    t = [0x0000, 0xcc01, 0xd801, 0x1400, 0xf001, 0x3c00, 0x2800, 0xe401,
         0xa001, 0x6c00, 0x7800, 0xb401, 0x5000, 0x9c01, 0x8801, 0x4400]
    for b in data:
        for nib in (b & 0xf, b >> 4):
            crc = (crc >> 4) ^ t[crc & 0xf] ^ t[nib]
    return crc


def fit_ride(start, secs, power, dist_m, compressed=False, gap=None, track=None, enhanced=False, ascent=None):
    """A synthetic FIT ride: records (timestamp, heart rate, distance, power) every second from FIT time `start`
    for `secs` seconds, none during `gap` (first, last second), then a session with `dist_m`, the timer time and
    `ascent` (m). With `compressed`, records after the first of a run carry a compressed timestamp header instead.
    With `track(t)` giving (latitude, longitude in degrees, altitude in m), the records also carry position_lat and
    position_long (semicircles) and altitude ((m + 500) x 5; enhanced_altitude with `enhanced`), FIT profile."""
    U8, U16, S32, U32 = 0x02, 0x84, 0x85, 0x86
    pos = [0, 4, S32, 1, 4, S32] + ([78, 4, U32] if enhanced else [2, 2, U16]) if track else []
    n = 4 + len(pos) // 3
    d = bytearray()
    d += bytes([0x40, 0, 0, 20, 0, n, 253, 4, U32, 3, 1, U8, 5, 4, U32, 7, 2, U16] + pos)  # record, local 0
    d += bytes([0x41, 0, 0, 20, 0, n - 1, 3, 1, U8, 5, 4, U32, 7, 2, U16] + pos)  # without timestamp, local 1
    prev, timer = None, 0
    for t in range(secs):
        if gap and gap[0] <= t < gap[1]:
            continue
        timer += 1
        ts, w = start + t, power(t)
        fields = struct.pack('<BIH', 140, t * 800, 0xffff if w is None else w)
        if track:
            lat, lon, alt = track(t)
            semi = lambda deg: round(deg * 2**31 / 180)
            fields += struct.pack('<ii', semi(lat), semi(lon))
            fields += struct.pack('<I' if enhanced else '<H', round((alt + 500) * 5))
        if compressed and prev is not None and ts - prev < 32:
            d += bytes([0x80 | (1 << 5) | (ts & 0x1f)]) + fields
        else:
            d += bytes([0x00]) + struct.pack('<I', ts) + fields
        prev = ts
    d += bytes([0x42, 0, 0, 18, 0, 6, 253, 4, U32, 2, 4, U32, 8, 4, U32, 9, 4, U32, 16, 1, U8, 22, 2, U16])  # session
    d += bytes([0x02]) + struct.pack('<IIIIBH', start + secs, start, timer * 1000, dist_m * 100, 140,
                                     0xffff if ascent is None else ascent)
    h = bytearray(struct.pack('<BBHI4s', 14, 0x10, 2140, len(d), b'.FIT'))
    h += struct.pack('<H', fit_crc(h))
    f = bytes(h + d)
    return f + struct.pack('<H', fit_crc(f))


def rides(m):
    """\\Activities as stock leaves it, with made-up rides (stock.rs tests: today is 2026-10-23, a Friday)."""
    import calendar
    a = os.path.join(m, 'Activities')
    os.makedirs(a)
    def fit_time(y, mo, d, h):
        return calendar.timegm((y, mo, d, h, 0, 0)) - 631065600
    # tracks (the ride history's cards) in the sea at 0 N 0 E, nowhere real (AGENTS.md, "Public repository")
    def loop(t):  # 14.4 km round a circle in 30 min, over two 40 m hills: 80 m of ascent
        a = 2 * math.pi * t / 1800
        r = 14400 / (2 * math.pi) / 111320
        return -0.03 + r * math.sin(a), -0.03 + r * math.cos(a) / math.cos(math.radians(0.03)), \
            100 + 20 * (1 - math.cos(2 * a))
    def out_back(t):  # east and back, climbing 30 m and down
        f = t / 1500
        return -0.01, -0.06 + 0.05 * (1 - abs(1 - 2 * f)), 50 + 30 * (1 - abs(1 - 2 * f))
    files = {
        # this week: 2 min at 400 W then 250 W, 30 min
        '261020080000.fit': fit_ride(fit_time(2026, 10, 20, 8), 1800, lambda t: 400 if t < 120 else 250, 14400,
                                     track=loop, ascent=80),
        # this year, in the window: 300 W, stopped 5 min after 10 min (compressed timestamps)
        '261001070000.fit': fit_ride(fit_time(2026, 10, 1, 7), 1500, lambda t: 300, 12000, True, (600, 900),
                                     track=out_back, enhanced=True, ascent=30),
        # last year, out of the window: 600 W, north on the flat
        '250315090000.fit': fit_ride(fit_time(2025, 3, 15, 9), 600, lambda t: 600, 6000,
                                     track=lambda t: (-0.05 + t / 600 * 0.054, -0.07, 20), ascent=0),
        # this week, no power
        '261022100000.fit': fit_ride(fit_time(2026, 10, 22, 10), 600, lambda t: None, 5000),
        # stock's ride in progress: never read (900 W would show)
        'tmp.fit': fit_ride(fit_time(2026, 10, 23, 7), 600, lambda t: 900, 5000),
    }
    bad = bytearray(fit_ride(fit_time(2026, 10, 21, 10), 600, lambda t: 800, 5000))
    bad[-1] ^= 1
    files['261021100000.fit'] = bytes(bad)  # a bad CRC: skipped
    files['261019100000.fit'] = files['261020080000.fit'][:20000]  # cut: skipped
    files['20261022100000.ubx'] = bytes(range(256)) * 4  # a GNSS log: not a ride
    for name, data in files.items():
        with open(os.path.join(a, name), 'wb') as f:
            f.write(data)
    os.makedirs(os.path.join(a, 'UploadTemp'))
    # stock's settings, made up, in its layout (sections and keys as on a unit; LF, a NUL at the end)
    s = os.path.join(m, 'System')
    os.makedirs(s)
    ini = {
        'DeviceData.ini': '[Info]\nVer=1000\n\n[DeviceSys]\nbrightness=50\nActBike=1\n\0',
        'UserProfile.ini': '[Info]\nVer=1000\n\n[UserProfile]\nWbalance=21000\nCP=255\nWeight_kg=72.50000\n\n'
                           '[FTP]\nZone1L=0\nBaseVale=263\n\n[MHR]\nZone7H=65535\nBaseVale=188\n\0',
        'Sensor.ini': '[Sensor]\nHR_Num=1\nPWR_Num=1\n\n[HRSensor1]\nID=1234\nHWType=0\nSentype=4\nactive=1\n\n'
                      '[PWRSensor1]\nID=4321\nHWType=0\nSentype=3\nactive=1\n\0',
        # ActBike=1: the second bike's pages
        'Bike1Profile.ini': '[Info]\nVer=6001\n[Page1]\nGrid1=2\ntype=1\nisEnabled=1\n\0',
        'Bike2Profile.ini': '[Info]\nVer=6001\n[Page1]\nGrid4=29,21,24,2\ntype=4\nisEnabled=2\n\n'
                            '[Page2]\nGrid2=5,8\ntype=2\nisEnabled=1\n\0',
    }
    for name, text in ini.items():
        with open(os.path.join(s, name), 'wb') as f:
            f.write(text.encode())
    subprocess.run(['find', m, '-name', '._*', '-delete'], check=True)


def fit_workout(name, steps):
    """A FIT workout file laid out as Bryton's app writes them (storage/workout_file.rs): file_id (type 5), workout
    (16-byte name, step count, sport cycling), then a workout_step per step: (duration type, value, target type,
    target value, custom low, custom high, intensity)."""
    U8, U16, U32, STR = 0x00, 0x84, 0x86, 0x07
    d = bytearray([0x40, 0, 0, 0, 0, 3, 0, 1, U8, 1, 2, U16, 4, 4, U32])  # file_id
    d += bytes([0x00, 5]) + struct.pack('<HI', 255, 1_100_000_000)
    d += bytes([0x40, 0, 0, 26, 0, 3, 8, 16, STR, 6, 2, U16, 4, 1, U8])  # workout
    d += bytes([0x00]) + name.encode()[:15].ljust(16, b'\0') + struct.pack('<HB', len(steps), 2)
    d += bytes([0x42, 0, 0, 27, 0, 9, 0, 10, STR, 2, 4, U32, 4, 4, U32, 5, 4, U32, 6, 4, U32, 254, 2, U16,
                1, 1, U8, 3, 1, U8, 7, 1, U8])  # workout_step, local 2
    for i, (dt, dv, tt, tv, lo, hi, intensity) in enumerate(steps):
        d += bytes([0x02]) + b'Step Name\0' + struct.pack('<IIIIHBBB', dv, tv, lo, hi, i, dt, tt, intensity)
    h = bytearray(struct.pack('<BBHI4s', 14, 0x10, 2140, len(d), b'.FIT'))
    h += struct.pack('<H', fit_crc(h))
    f = bytes(h + d)
    return f + struct.pack('<H', fit_crc(f))


def workouts(m):
    """\\Workout as stock leaves it, with made-up workouts (sim.rs tests: today is 2026-10-23): a folder per
    planned day, a .fit and a .json per workout, stock's temporary copies beside."""
    S = 1000  # ms

    def bryton(lo, hi):  # Bryton's power target: 100000 + % FTP
        return (245, 0, 100000 + lo, 100000 + hi)
    # warm-up 20 s at 50-60 % (Bryton's), 2 x (30 s at 100-110 %, 15 s at 150 W) as a repeat, an open cool-down
    today = [
        (0, 20 * S, *bryton(50, 60), 2),
        (0, 30 * S, 4, 0, 100, 110, 0),
        (0, 15 * S, 4, 0, 1150, 1150, 1),
        (6, 1, 2, 2, 0, 0, 0),
        (5, 0, 2, 0, 0, 0, 3),
    ]
    long_name = 'Long Ride \u2014 Easy'  # a non-ASCII long name, as the app's
    days = {
        '20261020': {'Old Ride': fit_workout('Old Ride', [(0, 3600 * S, *bryton(60, 70), 0)])},  # past: not listed
        '20261023': {'Short Intervals': fit_workout('Short Intervals', today)},
        '20261024': {'Broken': fit_workout('Broken', today)[:-1] + b'\0'},  # a bad CRC: left out
        '20261025': {long_name: fit_workout(long_name, [(0, 5400 * S, *bryton(65, 75), 0)])},
    }
    w = os.path.join(m, 'Workout')
    for day, files in days.items():
        d = os.path.join(w, 'calendar', day)
        os.makedirs(d)
        for name, data in files.items():
            with open(os.path.join(d, name + '.fit'), 'wb') as f:
                f.write(data)
            with open(os.path.join(d, name + '.json'), 'wb') as f:
                f.write(b'{"name":"made up","plan":["%s"],"provider":"bryton","interval":[],"ver":5}' % day.encode())
    for sub in ('calendar_temp', 'saved_temp', 'Workout_temp'):
        os.makedirs(os.path.join(w, sub))
    with open(os.path.join(w, 'calendar_temp', 'temp0.fit'), 'wb') as f:
        f.write(days['20261023']['Short Intervals'])
    subprocess.run(['find', m, '-name', '._*', '-delete'], check=True)


def hill(step=25.0):
    """The synthetic course (sim.rs `course_ride` rides it): from 2 km west and 1.5 km south of 0 N 0 E, 2 km east
    on the flat at 100 m, 2 km north climbing to 220 m (6 %), 1 km east on the top. Points every `step` m: (latitude,
    longitude in degrees, altitude in m, distance in m)."""
    legs = [((-2000.0, -1500.0, 100.0), (0.0, -1500.0, 100.0)), ((0.0, -1500.0, 100.0), (0.0, 500.0, 220.0)),
            ((0.0, 500.0, 220.0), (1000.0, 500.0, 220.0))]
    pts, dist = [], 0.0
    for i, (a, b) in enumerate(legs):
        length = math.hypot(b[0] - a[0], b[1] - a[1])
        n = int(length / step)
        for k in range(0 if i == 0 else 1, n + 1):
            t = k / n
            x, y, z = (a[j] + (b[j] - a[j]) * t for j in range(3))
            pts.append((y / 111_320, x / 111_320, z, dist + length * t))
        dist += length
    return pts


def bryton_route(pts):
    """A route as Bryton's app writes \\PlanTrip\\<name>.fit (storage/course_file.rs): messages 248, 254, 251 per point,
    253, 250 (a climb's start and end, types 190 and 191), 252, then 249 per point; degrees x 1e6, altitudes as FIT
    ((m + 500) x 5)."""
    U8, U16, S16, S32, U32, STR = 0x00, 0x84, 0x83, 0x85, 0x86, 0x07
    e6 = lambda v: int(round(v * 1e6))  # noqa: E731
    alt = lambda m: int(round((m + 500) * 5))  # noqa: E731
    lats, lons = [e6(p[0]) for p in pts], [e6(p[1]) for p in pts]
    d = bytearray([0x47, 0, 0, 248, 0, 2, 1, 2, U16, 2, 2, U16, 0x07]) + struct.pack('<HH', 0, 1)
    d += bytes([0x41, 0, 0, 254, 0, 10, 1, 2, U16, 2, 4, S32, 3, 4, S32, 4, 4, S32, 5, 4, S32, 6, 4, U32,
                7, 2, U16, 8, 2, U16, 9, 2, U16, 10, 2, U16])
    d += bytes([0x01]) + struct.pack('<HiiiiIHHHH', len(pts), max(lats), min(lats), max(lons), min(lons),
                                     int(pts[-1][3]), alt(220), alt(100), 120, 0)
    d += bytes([0x42, 0, 0, 251, 0, 1, 1, 2, U16])
    for i in range(len(pts)):
        d += bytes([0x02]) + struct.pack('<H', i)
    d += bytes([0x43, 0, 0, 253, 0, 1, 1, 2, U16, 0x03]) + struct.pack('<H', 2)
    d += bytes([0x44, 0, 0, 250, 0, 5, 1, 2, U16, 2, 1, U8, 3, 4, U32, 4, 4, U32, 5, 32, STR])
    for i, kind in ((80, 190), (160, 191)):
        d += bytes([0x04]) + struct.pack('<HBII', i, kind, 0, 0) + bytes(32)
    d += bytes([0x45, 0, 0, 252, 0, 1, 1, 2, U16, 0x05]) + struct.pack('<H', len(pts))
    d += bytes([0x46, 0, 0, 249, 0, 4, 1, 4, S32, 2, 4, S32, 3, 2, U16, 4, 2, S16])
    for i, p in enumerate(pts):
        grade = 600 if 80 <= i < 160 else 0
        d += bytes([0x06]) + struct.pack('<iiHh', lats[i], lons[i], alt(p[2]), grade)
    h = bytearray(struct.pack('<BBHI4s', 14, 0x10, 2167, len(d), b'.FIT'))
    h += struct.pack('<H', fit_crc(h))
    f = bytes(h + d)
    return f + struct.pack('<H', fit_crc(f))


def garmin_course(pts, cues=((80, 1),)):
    """A FIT course as Garmin's SDK cookbook encodes one (CourseEncode): file_id (type 6), course, lap, a timer
    start event, a record per point (time, semicircles, altitude, distance), course points (after the record of
    their point: `cues`, (point index, type)) with their position, distance and type, the stop event."""
    U8, U16, S32, U32, STR = 0x00, 0x84, 0x85, 0x86, 0x07
    semi = lambda v: int(round(v * 2**31 / 180))  # noqa: E731
    t0 = 1_100_000_000
    d = bytearray([0x40, 0, 0, 0, 0, 3, 0, 1, U8, 1, 2, U16, 4, 4, U32])
    d += bytes([0x00, 6]) + struct.pack('<HI', 255, t0)
    d += bytes([0x41, 0, 0, 31, 0, 2, 5, 16, STR, 4, 1, U8, 0x01]) + b'Hill'.ljust(16, b'\0') + bytes([2])
    d += bytes([0x42, 0, 0, 19, 0, 2, 253, 4, U32, 9, 4, U32, 0x02]) + struct.pack('<II', t0, int(pts[-1][3] * 100))
    d += bytes([0x43, 0, 0, 21, 0, 3, 253, 4, U32, 0, 1, U8, 1, 1, U8])
    d += bytes([0x03]) + struct.pack('<IBB', t0, 0, 0)
    d += bytes([0x44, 0, 0, 20, 0, 5, 253, 4, U32, 0, 4, S32, 1, 4, S32, 2, 2, U16, 5, 4, U32])
    for i, p in enumerate(pts):
        d += bytes([0x04]) + struct.pack('<IiiHI', t0 + 3 * i, semi(p[0]), semi(p[1]), int(round((p[2] + 500) * 5)),
                                         int(p[3] * 100))
        for k, kind in cues:  # the hill's: a summit point at the climb's foot, as the cookbook puts one
            if k == i:
                d += bytes([0x45, 0, 0, 32, 0, 5, 1, 4, U32, 2, 4, S32, 3, 4, S32, 4, 4, U32, 5, 1, U8])
                d += bytes([0x05]) + struct.pack('<IiiIB', t0 + 3 * i, semi(p[0]), semi(p[1]), int(p[3] * 100), kind)
    d += bytes([0x03]) + struct.pack('<IBB', t0 + 3 * len(pts), 0, 9)
    h = bytearray(struct.pack('<BBHI4s', 14, 0x10, 2140, len(d), b'.FIT'))
    h += struct.pack('<H', fit_crc(h))
    f = bytes(h + d)
    return f + struct.pack('<H', fit_crc(f))


def gpx(pts):
    """The same hill as GPX 1.1 track points with elevation."""
    rows = ''.join('<trkpt lat="%.7f" lon="%.7f"><ele>%.1f</ele></trkpt>\n' % (p[0], p[1], p[2]) for p in pts)
    return ('<?xml version="1.0" encoding="UTF-8"?>\n<gpx version="1.1" creator="fat_image.py" '
            'xmlns="http://www.topografix.com/GPX/1/1">\n<trk><name>Hill</name><trkseg>\n%s</trkseg></trk></gpx>\n'
            % rows).encode()


def lines(corners, step):
    """Points every `step` m (at most) along straight lines between `corners` (x east, y north, m), as course.rs's
    tests' `lines`."""
    pts = [corners[0]]
    for a, b in zip(corners, corners[1:]):
        n = max(1, math.ceil(math.hypot(b[0] - a[0], b[1] - a[1]) / step))
        pts += [(a[0] + (b[0] - a[0]) * k / n, a[1] + (b[1] - a[1]) * k / n) for k in range(1, n + 1)]
    return pts


# The zigzag's corners after the winding stretch, and its hairpin (course/cues.rs `zigzag`, which sim.rs rides).
ZIGZAG_END = [(3300.0, 600.0), (2800.0, 100.0), (2800.0, -500.0), (2350.0, -700.0)]
HAIRPIN = [(2350.0, -700.0), (1900.0, -900.0), (1900.0, -906.0), (2350.0, -706.0)]
# Its turns as a FIT course states them: (corner, course_point type: 6 left, 7 right, 22 sharp right, 19 slight left,
# 23 U-turn; FIT profile), and a water point on the winding stretch (3).
ZIGZAG_CUES = [((1000.0, 0.0), 6), ((1000.0, 600.0), 7), ((2550.0, 600.0), 3), ((3300.0, 600.0), 22),
               ((2800.0, 100.0), 19), ((2800.0, -500.0), 7), ((1900.0, -903.0), 23)]


def zigzag():
    """A made-up course of turns south-east of 0 N 0 E (course/cues.rs `zigzag`): a left and a right corner, 1.5 km
    of winding road (15 m either side, 300 m waves: no turn), a sharp right, a slight left, a right, a hairpin.
    Points as `hill`'s: (latitude, longitude, altitude, distance)."""
    xy = lines([(0.0, 0.0), (1000.0, 0.0), (1000.0, 600.0), (1800.0, 600.0)], 10.0)
    xy += [(1800.0 + 10.0 * k, 600.0 + 15.0 * math.sin(2 * math.pi * 10.0 * k / 300.0)) for k in range(1, 151)]
    xy += lines(ZIGZAG_END, 10.0)[1:] + lines(HAIRPIN, 3.0)[1:]
    pts, dist = [], 0.0
    for i, (x, y) in enumerate(xy):
        if i:
            dist += math.hypot(x - xy[i - 1][0], y - xy[i - 1][1])
        pts.append((y / 111_320, x / 111_320, 50.0, dist))
    return pts


def courses(m):
    """Made-up courses (storage/course_file.rs, sim.rs): stock's \\PlanTrip with a route in Bryton's layout and a file that
    is no FIT, and \\Courses (copied over USB) with the same hill as a Garmin FIT course and as GPX; and the zigzag
    as a FIT course with its turns as course points and as a GPX track (its turns found on its shape)."""
    pts = hill()
    zig = zigzag()
    def near(c):  # the zigzag's point nearest corner c
        return min(range(len(zig)), key=lambda i: math.hypot(zig[i][1] * 111_320 - c[0], zig[i][0] * 111_320 - c[1]))
    turns = [(near(c), kind) for c, kind in ZIGZAG_CUES]
    os.makedirs(os.path.join(m, 'PlanTrip', 'LiveSegment'))
    for name, data in (('PlanTrip/Hill.fit', bryton_route(pts)), ('PlanTrip/Broken.fit', b'not a FIT file\n'),
                       ('Courses/Hill_Garmin.fit', garmin_course(pts)), ('Courses/Hill.gpx', gpx(hill(10.0))),
                       ('Courses/Turns.fit', garmin_course(zig, turns)), ('Courses/Zigzag.gpx', gpx(zig))):
        os.makedirs(os.path.dirname(os.path.join(m, name)), exist_ok=True)
        with open(os.path.join(m, name), 'wb') as f:
            f.write(data)
        subprocess.run(['find', m, '-name', '._*', '-delete'], check=True)


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
