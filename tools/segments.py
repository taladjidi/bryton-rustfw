#!/usr/bin/env python3
"""Live segment files (`\\Rustfw\\Segments\\<id>.seg`, docs/segments.md) from the rider's own Intervals.icu rides.

  segments.py fetch [--since YYYY-MM-DD] [--refresh]   rides with segment matches, their segment lists and streams -> the cache
  segments.py build [--min-km 0.5] [--min-gain-m 10] [--min-rides 2] [--max 128] [--kom ID=M:SS ...] [--out DIR]
  segments.py write --volume PATH [--init] [--prune] [--out DIR]   (no --out: build's and own's folders) the built files -> PATH/Rustfw/Segments/, each read back and compared
  segments.py own --rides DIR | --volume PATH [--dry-run] [--min-km 1] [--min-gain-m 0] [--max 128] [--max-per-cell 8] [--out DIR]
                                           stretches the rider rode twice, from the FIT rides (no Strava) -> .seg files
  segments.py fixture [OUT]                the synthetic .seg the Rust tests parse (test-data/segment/synthetic.seg)
  segments.py selftest                     synthetic checks, no network (check.sh)

Source: Intervals.icu, API https://intervals.icu/api/v1/docs (Strava's segment matches are in its activities; no Strava API: it needs a subscription).
Config as sync.py: ~/.config/bryton-rustfw/sync.json, block `intervals`. Cache: ~/.cache/bryton-rustfw/segments/
(BRYTON_SEGMENTS_CACHE overrides; activities/ holds the rides, state/ the manifest of what `write` put on a drive);
nothing personal enters the repository.

The ghost is the rider's own: PR (fastest effort), RECENT (latest), BEST5 (fastest of the last 5), and a KOM at
constant pace when `--kom ID=M:SS` gives its time (read by hand on Strava).

Selection: efforts grouped by segment id; kept when ridden at least --min-rides times (after the sanity drop), length
>= --min-km, gain >= --min-gain-m; longest first, at most --max. Geometry: the fastest effort among those whose length
is within 10 % of the median, its track between the segment's indices, thinned to at most 400 points spaced evenly
along the distance (about every 25 m; first and last always kept). Sanity: an effort is dropped when its length
differs from the geometry's by more than 10 % or its first or last point is over 50 m from the geometry's. Each
effort's time at a geometry point: its time stream at the same distance from its own start, scaled by its length over
the segment's.

Layout (docs/segments.md "The file"; stock's live_segments.c): header 0x200, data section, FIT course (FIT SDK
protocol: 12-byte header, definition + data messages, CRC-16 per Garmin's table). Python 3 standard library only.
"""
import argparse
import bisect
import datetime
import re
import importlib.util
import json
import math
import os
import struct
import sys
import tempfile
import time
import urllib.error
from pathlib import Path

try:
    import fcntl
except ImportError:  # not POSIX: the read-back below then goes through the page cache
    fcntl = None

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('sync', HERE / 'sync.py')
sync = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sync)  # sync.py guards its main(), so this only loads the helpers

FIXTURE = HERE.parent / 'test-data' / 'segment' / 'synthetic.seg'
MAX_POINTS = 400
HEADER = 0x200
SEMI = 2147483648.0 / 180.0
CODES = {'PR': b'07', 'KOM': b'05', 'RECENT': b'12', 'BEST5': b'13'}
ORDER = ['PR', 'KOM', 'RECENT', 'BEST5']
PAUSE = 0.3  # s between calls: polite to Intervals.icu


def cache_dir():
    return Path(os.environ.get('BRYTON_SEGMENTS_CACHE', Path.home() / '.cache' / 'bryton-rustfw' / 'segments'))


def new_stats():
    return {'dropped': 0, 'rare': 0, 'small': 0, 'wide': 0, 'long': 0}


def acts_dir():
    return cache_dir() / 'activities'


def state_dir():
    return cache_dir() / 'state'


# ---------------------------------------------------------------------------------------------------------------
# fetch
# ---------------------------------------------------------------------------------------------------------------

class Stop(Exception):
    pass


def get(icu, path):
    time.sleep(PAUSE)
    st, body = sync.request('GET', f'{sync.ICU}/api/v1/{path}', icu.auth)
    if st == 429:
        raise Stop('HTTP 429 (rate limited): stopped, run fetch again later; the cache keeps what was fetched')
    if st != 200:
        raise OSError(f'Intervals.icu {path.split("?")[0]}: HTTP {st}')
    return json.loads(body)


def recent(date):
    try:
        return (datetime.date.today() - datetime.date.fromisoformat(date[:10])).days < 2
    except ValueError:
        return True


def fetch(since, refresh=False, log=print):
    cfg = sync.load('sync.json', None)
    if not cfg or 'intervals' not in cfg:
        sys.exit(f'segments.py: no `intervals` block in {sync.home() / "sync.json"} (docs/sync.md)')
    icu = sync.Intervals(cfg['intervals'])
    cache = acts_dir()
    cache.mkdir(parents=True, exist_ok=True)
    oldest = since or '2000-01-01'
    newest = datetime.date.today().isoformat()
    try:
        acts = get(icu, f'athlete/{icu.athlete}/activities?oldest={oldest}&newest={newest}')
        acts = [a for a in acts if a.get('has_segments')]
        have = 0 if refresh else sum(1 for a in acts if (cache / f'{a["id"]}.json').exists())
        log(f'{len(acts)} activities with segments, {have} cached')
        for n, a in enumerate(acts, 1):
            f = cache / f'{a["id"]}.json'
            if f.exists() and not refresh:
                continue
            segs = get(icu, f'activity/{a["id"]}/segments')
            st = get(icu, f'activity/{a["id"]}/streams?types=time,latlng,distance,altitude')
            streams = {s['type']: s for s in st}
            if not all(k in streams for k in ('time', 'distance', 'latlng')):  # no GPS: kept empty, not asked again
                streams = {'time': {'data': []}, 'distance': {'data': []}, 'latlng': {'data': [], 'data2': []}}
            out = {'date': a.get('start_date_local') or a.get('start_date') or '', 'segments': segs,
                   'time': streams['time']['data'], 'distance': streams['distance']['data'],
                   'lat': streams['latlng']['data'], 'lon': streams['latlng']['data2'],
                   'alt': (streams.get('altitude') or {}).get('data') or []}
            if not segs and recent(out['date']):  # Intervals.icu may still be matching it: ask again next time
                continue
            f.with_suffix('.tmp').write_text(json.dumps(out))
            os.replace(f.with_suffix('.tmp'), f)
            log(f'  {n}/{len(acts)} fetched')
    except Stop as e:
        log(f'segments.py: {e}')
        return 1
    except (OSError, ValueError, KeyError) as e:  # network, timeout, HTTP error, a malformed answer
        log(f'segments.py: fetch stopped: {e!r} (run again: the cache keeps what was fetched)')
        return 1
    return 0


# ---------------------------------------------------------------------------------------------------------------
# build
# ---------------------------------------------------------------------------------------------------------------

def metres(lat1, lon1, lat2, lon2):
    """Equirectangular distance, m (the unit's matcher does the same; fine at segment scale)."""
    k = 111195.0
    return math.hypot((lat2 - lat1) * k, (lon2 - lon1) * k * math.cos(math.radians((lat1 + lat2) / 2)))


def load_efforts(cache):
    """{segment_id: [effort]} from the cached activities. An effort is the slice of its activity's streams."""
    out = {}
    for f in sorted(cache.glob('*.json')):
        seen = set()
        a = json.loads(f.read_text())
        n = len(a['time'])
        if not len(a['distance']) == len(a['lat']) == len(a['lon']) == n:
            continue
        for s in a['segments']:
            i, j, sid = s.get('start_index'), s.get('end_index'), s.get('segment_id')
            if sid is None or i is None or j is None or not 0 <= i < j < n or (sid, i) in seen:
                continue
            seen.add((sid, i))
            alt = a['alt'][i:j + 1] if len(a['alt']) == n else []
            out.setdefault(int(sid), []).append({
                'act': f.stem, 'name': s.get('name') or '', 'date': a['date'], 't': a['time'][i:j + 1], 'd': a['distance'][i:j + 1],
                'lat': a['lat'][i:j + 1], 'lon': a['lon'][i:j + 1], 'alt': alt})
    return out


def prep(e):
    """Add the effort's own time, length and distance from its start."""
    d0, t0 = e['d'][0], e['t'][0]
    e['dd'] = [x - d0 for x in e['d']]
    e['tt'] = [x - t0 for x in e['t']]
    e['len'] = e['dd'][-1]
    e['time'] = e['tt'][-1]
    return e


def gain(alt):
    """Ascent in m with 2 m hysteresis (barometer noise does not add up)."""
    alt = [a for a in alt if a is not None]  # gaps are skipped
    if not alt:
        return 0
    total, ref = 0.0, alt[0]
    for a in alt[1:]:
        if a - ref >= 2:
            total += a - ref
            ref = a
        elif ref - a >= 2:
            ref = a
    return round(total)


def thin(e, limit=MAX_POINTS, spacing=25.0):
    """Indices into the effort's points: evenly spaced along the distance, first and last kept, at most `limit`."""
    dd, n = e['dd'], len(e['dd'])
    k = max(2, min(limit, n, int(e['len'] / spacing) + 2))
    pick = {0, n - 1}
    for m in range(1, k - 1):
        pick.add(min(n - 2, bisect.bisect_left(dd, e['len'] * m / (k - 1))))
    return sorted(pick)


def time_at(e, scale, g):
    """The effort's elapsed s at geometry distance g (its own distance is g * scale), linear in between."""
    x = g * scale
    dd, tt = e['dd'], e['tt']
    j = bisect.bisect_left(dd, x)
    if j <= 0:
        return 0.0
    if j >= len(dd):
        return float(tt[-1])
    span = dd[j] - dd[j - 1]
    return tt[j - 1] + (tt[j] - tt[j - 1]) * ((x - dd[j - 1]) / span if span else 1.0)


def fit_course(pts, times=None):
    """pts: [(lat, lon, dist_m, alt_m or None)] as a FIT course: file_id, then one record per point, CRCs set.
    `times` (timestamps per point, the selftest's rides) replaces the point numbers."""
    # file_id (0): type (0) = 6 course, manufacturer (1) = 255, time_created (4) = 0
    body = bytearray([0x40, 0, 0, 0, 0, 3, 0, 1, 0, 1, 2, 0x84, 4, 4, 0x86])
    body += bytes([0]) + struct.pack('<BHI', 6, 255, 0)
    # record (20): timestamp (253), position_lat (0), position_long (1), distance (5, cm), altitude (2, (m + 500) * 5)
    body += bytes([0x41, 0, 0, 20, 0, 5, 253, 4, 0x86, 0, 4, 0x85, 1, 4, 0x85, 5, 4, 0x86, 2, 2, 0x84])
    for k, (lat, lon, dist, alt) in enumerate(pts):
        body += bytes([1]) + struct.pack('<IiiIH', k if times is None else times[k], round(lat * SEMI), round(lon * SEMI), round(dist * 100),
                                         0xFFFF if alt is None else round((alt + 500) * 5))
    f = struct.pack('<BBHI4s', 12, 0x10, 2100, len(body), b'.FIT') + bytes(body)
    return f + struct.pack('<H', sync.fit_crc(f))


def seg_file(sid, name, length, gain_m, pts, targets):
    """pts as in fit_course; targets [(kind, total_s, [elapsed s per point])] in file order."""
    n, np_ = len(targets), len(pts)
    assert all(0 <= x <= 65535 for t in targets for x in t[2]) and all(t[1] <= 65535 for t in targets)
    h = bytearray(HEADER)
    struct.pack_into('<III', h, 1, sid, 1, HEADER + 8 + 13 * n + 4 * np_ * n)
    h[0x0d:0x2d] = name.encode('ascii', 'replace')[:31].ljust(32, b'\0')
    struct.pack_into('<I', h, 0x2d, round(length))
    struct.pack_into('<I', h, 0x35, gain_m)
    for i, (lat, lon, _, _) in enumerate(pts[:40]):
        struct.pack_into('<ff', h, 0x39 + 8 * i, lat, lon)
    h[0x179] = n
    for k, (kind, total, _) in enumerate(targets):
        at = 0x17a + 0x11 * k
        h[at:at + 2] = CODES[kind]
        h[at + 3:at + 3 + len(kind)] = kind.encode()
        struct.pack_into('<I', h, at + 13, total)
    h[0x1fd] = 1
    data = struct.pack('<II', np_, n) + bytes(13 * n)
    data += b''.join(struct.pack('<I', t[2][i]) for i in range(np_) for t in targets)
    return bytes(h) + data + fit_course(pts)


def whole_ok(e):  # a GPS or sensor gap (None) inside the effort, or a pause: dropped
    if None in e['t'] + e['d'] + e['lat'] + e['lon'] or e['t'][-1] <= e['t'][0] or e['d'][-1] <= e['d'][0]:
        return False
    steps = list(zip(zip(e['t'], e['t'][1:]), zip(e['d'], e['d'][1:])))
    # time and distance never fall; a stop of over a minute inside the effort is no clean effort to race
    return all(t1 >= t0 and d1 >= d0 and not (t1 - t0 > 60 and d1 - d0 < 1) for (t0, t1), (d0, d1) in steps)


def build_segment(sid, efforts, a, stats):
    """The .seg bytes of one segment, or None when it is not kept. stats counts the dropped efforts."""
    stats['dropped'] += sum(1 for e in efforts if not whole_ok(e))
    efforts = [prep(dict(e)) for e in efforts if whole_ok(e)]
    if not efforts:
        return None
    med = sorted(e['len'] for e in efforts)[len(efforts) // 2]
    best = min((e for e in efforts if abs(e['len'] - med) <= 0.1 * med), key=lambda e: e['time'])
    idx = thin(best)
    geo = [(best['lat'][i], best['lon'][i], best['dd'][i], best['alt'][i] if best['alt'] else None) for i in idx]
    length = best['len']
    ok = []
    for e in efforts:
        if (abs(e['len'] - length) <= 0.1 * length
                and metres(e['lat'][0], e['lon'][0], geo[0][0], geo[0][1]) <= 50
                and metres(e['lat'][-1], e['lon'][-1], geo[-1][0], geo[-1][1]) <= 50):
            ok.append(e)
        else:
            stats['dropped'] += 1
    if len({e['act'] for e in ok}) < a.min_rides:
        stats['rare'] += 1
        return None
    g = gain(best['alt'])
    if length < a.min_km * 1000 or g < a.min_gain_m:
        stats['small'] += 1
        return None

    if any(e['time'] > 65535 for e in ok):  # the unit keeps u16 seconds
        stats['long'] += 1
        return None

    def col(e):
        t = [round(time_at(e, e['len'] / length, p[2])) for p in geo]
        t[0] = 0
        for i in range(1, len(t)):
            t[i] = max(t[i], t[i - 1])
        t[-1] = max(t[-1], round(e['time']))
        return t

    by_date = sorted(ok, key=lambda e: e['date'])
    cand = {'PR': min(ok, key=lambda e: e['time']), 'RECENT': by_date[-1],
            'BEST5': min(by_date[-5:], key=lambda e: e['time'])}
    targets = []
    for kind in ORDER:
        if kind == 'KOM':
            if sid in a.kom:
                kt = a.kom[sid]
                targets.append((kind, kt, [round(kt * p[2] / geo[-1][2]) for p in geo]))
        else:
            c = col(cand[kind])
            targets.append((kind, c[-1], c))
    name = next((e['name'] for e in ok if e['name']), str(sid))
    return seg_file(sid, name, length, g, geo, targets)


def parse_kom(items):
    out = {}
    for s in items or []:
        k, v = s.split('=')
        m, sec = v.split(':')
        if not 0 <= int(sec) < 60 or not 0 < int(m) * 60 + int(sec) <= 65535:
            raise ValueError(s)
        out[int(k)] = int(m) * 60 + int(sec)
    return out


def build(a, log=print):
    efforts = load_efforts(acts_dir())
    stats = new_stats()
    built = []
    for sid, es in efforts.items():
        if sid >= 2 ** 32:
            stats['wide'] += 1  # the header's id is a u32
            continue
        f = build_segment(sid, es, a, stats)
        if f:
            built.append((struct.unpack_from('<I', f, 0x2d)[0], sid, f))
    built.sort(key=lambda x: -x[0])
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    keep = {f'{sid}.seg' for _, sid, _ in built[:a.max]}
    for old in out.glob('*.seg'):
        if old.name not in keep:
            old.unlink()
    for _, sid, f in built[:a.max]:
        (out / f'{sid}.seg').write_bytes(f)
    log(f'{len(efforts)} segments seen, {len(keep)} kept ({len(built) - len(keep)} over --max), '
        f'{stats["dropped"]} efforts dropped by sanity, {stats["rare"]} segments too rare, '
        f'{stats["small"]} too short or flat, {stats["wide"]} with an id over 32 bits, {stats["long"]} over 18 h, {len(keep)} files in {out}')
    return 0


# ---------------------------------------------------------------------------------------------------------------
# own: stretches the rider rode at least twice, from the FIT rides themselves (S7, no Strava)
# ---------------------------------------------------------------------------------------------------------------

# Two tracks are the same road where they are within OWN_TOL_M and go the same way (within 45 degrees). 25 m is the
# unit's own START_M (src/nav/segment_match.rs): a ride the matcher would start on this segment passes within 25 m
# of its start, so the files built here are matched the way they were found. GNSS error is 3 to 5 m per pass, so two
# passes on one road differ by about 10 m, and 25 m still keeps apart roads 40 m or more from each other.
OWN_TOL_M = 25.0
OWN_STEP_M = 30.0  # tracks are thinned to this spacing before matching: 1 point in 30 m, not one a second
OWN_MIN_KM = 1.0  # the overlap of two rides over 1 km (docs/segments.md S7)
OWN_ID_M = 50.0  # a stretch keeps its id while its ends move less than this and its length less than 10 %
OWN_FIRST_ID = 4_000_000_000  # own segments count down from here so they never meet Strava ids (docs/segments.md)
FIT_EPOCH = 631065600  # 1989-12-31 UTC, the FIT timestamps' zero


def fit_records(data):
    """[(timestamp s, lat, lon, distance m or None, altitude m or None)] of a FIT file's `record` messages (global 20:
    fields 253 timestamp, 0 / 1 position in semicircles, 5 distance cm, 2 altitude (m + 500) * 5, 78 the same finer).
    A record without a position is skipped; a file that cannot be walked gives []. Walked as sync.sub_sport."""
    out, defs, ts = [], {}, 0
    try:
        i, end = data[0], data[0] + int.from_bytes(data[4:8], 'little')
        if end > len(data):  # cut short
            return []
        while i < end:
            b = data[i]
            i += 1
            if b & 0x80:  # compressed timestamp header: the low 5 bits of the time
                local = b >> 5 & 3
                ts += ((b & 0x1f) - ts) & 0x1f
            elif b & 0x40:  # a definition
                bo = 'big' if data[i + 1] else 'little'
                n = data[i + 4]
                fields = [(data[i + 5 + 3 * k], data[i + 6 + 3 * k]) for k in range(n)]
                glob, size, i = int.from_bytes(data[i + 2:i + 4], bo), sum(s for _, s in fields), i + 5 + 3 * n
                if b & 0x20:
                    size += sum(data[i + 2 + 3 * k] for k in range(data[i]))
                    i += 1 + 3 * data[i]
                defs[b & 0xf] = (glob, bo, fields, size)
                continue
            else:
                local = b & 0xf
            glob, bo, fields, size = defs[local]
            if glob == 20:
                f, at = {}, i
                for num, s in fields:
                    f[num] = int.from_bytes(data[at:at + s], bo, signed=num in (0, 1))
                    at += s
                ts = f[253] if f.get(253, 0) >= 0x10000000 else ts  # below that it is a time since power-on, not a date
                if 0 in f and 1 in f and abs(f[0]) <= 1 << 30 and abs(f[1]) < 0x7FFFFFFF and (f[0], f[1]) != (0, 0):
                    alt = f[78] / 5 - 500 if f.get(78, 0xFFFFFFFF) != 0xFFFFFFFF else (
                        f[2] / 5 - 500 if f.get(2, 0xFFFF) != 0xFFFF else None)
                    dist = f.get(5, 0xFFFFFFFF)
                    out.append((ts, f[0] / SEMI, f[1] / SEMI, None if dist == 0xFFFFFFFF else dist / 100, alt))
            i += size
    except (IndexError, KeyError):
        return []
    return out


def load_rides(folder, log=print):
    """The rides of a folder of FIT files as efforts' streams: whole files only, outdoors, each ride once."""
    rides, seen, skipped = [], set(), 0
    for p in sorted(Path(folder).iterdir()):
        if p.suffix.lower() != '.fit' or p.name.lower().startswith('tmp'):
            continue
        data = p.read_bytes()
        recs = fit_records(data) if sync.fit_whole(data) and not sync.indoor(data) else []
        if len(recs) < 60 or recs[0][0] in seen:
            skipped += 1
            continue
        seen.add(recs[0][0])
        t0, lat, lon = recs[0][0], [r[1] for r in recs], [r[2] for r in recs]
        d = [r[3] for r in recs]
        if None in d:  # no distance in the file: along the track
            d = [0.0]
            for k in range(1, len(lat)):
                d.append(d[-1] + metres(lat[k - 1], lon[k - 1], lat[k], lon[k]))
        alt = [r[4] for r in recs]
        last = next((x for x in alt if x is not None), None)
        for k, x in enumerate(alt):  # a gap keeps the last altitude; none at all: no profile
            last = alt[k] = last if x is None else x
        date = datetime.datetime.fromtimestamp(t0 + FIT_EPOCH, datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%S')
        rides.append({'act': p.stem, 'date': date, 't': [r[0] - t0 for r in recs], 'd': d, 'lat': lat, 'lon': lon,
                      'alt': [] if last is None else alt})
    log(f'{len(rides)} rides read from {folder} ({skipped} files skipped: not whole, indoor, short or repeated)')
    return rides


def own_spans(ps):
    """[(first, last)] of the contiguous pieces of the sorted ordinals `ps` (a gap of one missing point is bridged)."""
    spans = []
    for p in ps:
        if spans and p - spans[-1][1] <= 2:
            spans[-1] = (spans[-1][0], p)
        else:
            spans.append((p, p))
    return spans


def own_find(rides, max_per_cell=8, stats=None):
    """Stretches ridden in the same direction by two or more rides: [(length m, {ride: (first, last) index}, ends)],
    `ends` the stretch's (lat, lon) at its start and end. Each ride is thinned to a point every OWN_STEP_M and
    indexed in a grid of cells 3 x OWN_TOL_M wide: the 9 cells around a point hold every candidate, so there is no
    all-pairs scan. A cell indexes the points of at most `max_per_cell` rides (the first, oldest ones); a later ride
    is still matched against those, so the work stays linear in the rides, not quadratic (a road ridden 100 times
    is compared with 8 rides, not 99), and it joins a stretch through them. A point matches another ride's point
    within OWN_TOL_M going the same way (dot product of the headings over 0.7); its own ride's points never count
    (a loop or a repeat inside one ride is not a second ride). A run of matches along two rides over OWN_MIN_KM is
    an overlap, taken on the earlier ride. The longest overlap claims its stretch with every ride that covers 90 % of
    it; the stretch is the longest piece of it, still over OWN_MIN_KM, where every member is within the tolerance
    (a detour of one ride is not in the stretch). An overlap more than half claimed already is the same stretch
    again and is dropped (ten rides, one segment). `stats['cmp']` counts the candidates looked at."""
    cell, k = 3 * OWN_TOL_M, 111195.0
    # shortcut: one cos for all rides (the first ride's): cells and heading stay right within a factor 2 of it (tens of
    # degrees of latitude); upgrade: a cos per ride or a projection
    c = math.cos(math.radians(rides[0]['lat'][0])) if rides else 1.0
    lim = (2 * OWN_TOL_M / k) ** 2  # degrees squared: the cheap prefilter before metres(), loose by 2x
    keep, grid, pairs, runs = [], {}, {}, []
    for r, ride in enumerate(rides):
        lat, lon, ks = ride['lat'], ride['lon'], [0]
        for i in range(1, len(lat)):
            if metres(lat[ks[-1]], lon[ks[-1]], lat[i], lon[i]) >= OWN_STEP_M:
                ks.append(i)
        keep.append(ks)
        for p, i in enumerate(ks):
            a, b = (i, ks[p + 1]) if p + 1 < len(ks) else (ks[p - 1] if p else i, i)
            ux, uy = (lon[b] - lon[a]) * c, lat[b] - lat[a]
            norm = math.hypot(ux, uy) or 1.0
            ux, uy = ux / norm, uy / norm
            key = (int(lon[i] * k * c // cell), int(lat[i] * k // cell))
            best = {}
            for dx in (-1, 0, 1):
                for dy in (-1, 0, 1):
                    for q, qp, qlat, qlon, qx, qy in grid.get((key[0] + dx, key[1] + dy), ([],))[0]:
                        if stats is not None:
                            stats['cmp'] = stats.get('cmp', 0) + 1
                        if q == r or (lat[i] - qlat) ** 2 + ((lon[i] - qlon) * c) ** 2 > lim:
                            continue
                        m = metres(lat[i], lon[i], qlat, qlon)
                        if m <= OWN_TOL_M and ux * qx + uy * qy >= 0.7 and m < best.get(q, (1e9,))[0]:
                            best[q] = (m, qp)
            for q, (_, qp) in best.items():
                pairs.setdefault((r, q), {})[p] = qp
                pairs.setdefault((q, r), {})[qp] = p
            pts, who = grid.setdefault(key, ([], set()))
            if r in who or len(who) < max_per_cell:
                who.add(r)
                pts.append((r, p, lat[i], lon[i], ux, uy))
    for (r, q), m in pairs.items():
        if q < r:
            ps = sorted(m)
            s = 0
            for n in range(1, len(ps) + 1):  # a run: p advances by at most 2, q forward by at most 4 steps
                if n == len(ps) or not (ps[n] - ps[n - 1] <= 2 and 0 <= m[ps[n]] - m[ps[n - 1]] <= 4):
                    js = [m[p] for p in ps[s:n]]
                    q0, q1 = min(js), max(js)  # on the earlier ride q, which is indexed in more cells than r
                    ln = rides[q]['d'][keep[q][q1]] - rides[q]['d'][keep[q][q0]]
                    if ln >= OWN_MIN_KM * 1000:
                        runs.append((ln, q, q0, q1))
                    s = n
    claimed, found = {}, []
    for ln, r, p0, p1 in sorted(runs, reverse=True):
        if sum(max(0, min(p1, y) - max(p0, w)) for w, y in claimed.get(r, ())) > (p1 - p0) / 2:
            continue
        # the overlap with the earlier ride may run past the road the others share (lead-ins that run side by side for
        # a while): the stretch is first the longest piece of it that most of the rides covering 1 km of it cover
        cand = {}
        for o in range(len(rides)):
            m = pairs.get((r, o), {})
            cov = [p for p in range(p0, p1 + 1) if p in m] if o != r else []
            if len(cov) * OWN_STEP_M >= OWN_MIN_KM * 1000:
                cand[o] = cov
        seen = {}
        for cov in cand.values():
            for p in cov:
                seen[p] = seen.get(p, 0) + 1
        need = (len(cand) + 2) // 2 - 1  # a majority of the rides, the pivot among them
        core = [p for p in range(p0, p1 + 1) if seen.get(p, 0) >= need]
        spans = own_spans(core)
        span = max(spans, key=lambda x: x[1] - x[0], default=None)
        if span is None:
            continue
        p0, p1 = span
        members = {r}
        for o in cand:
            m = pairs[(r, o)]
            js = [m[p] for p in range(p0, p1 + 1) if p in m]
            if len(js) >= 0.9 * (p1 - p0 + 1) and 0.7 * (p1 - p0) <= max(js) - min(js) <= 1.4 * (p1 - p0):
                members.add(o)
        # the stretch is where every member is within the tolerance, its longest contiguous piece
        both = [p for p in range(p0, p1 + 1) if all(p in pairs[(r, o)] for o in members if o != r)]
        spans = own_spans(both)
        dist = rides[r]['d']
        span = max(spans, key=lambda x: dist[keep[r][x[1]]] - dist[keep[r][x[0]]], default=None)
        if span is None or dist[keep[r][span[1]]] - dist[keep[r][span[0]]] < OWN_MIN_KM * 1000:
            continue
        p0, p1 = span
        rng = {o: (p0, p1) if o == r else tuple(sorted((pairs[(r, o)][p0], pairs[(r, o)][p1]))) for o in members}
        for o, (x, y) in rng.items():
            claimed.setdefault(o, []).append((x, y))
        a, b = keep[r][p0], keep[r][p1]
        found.append((dist[b] - dist[a], {o: (keep[o][x], keep[o][y]) for o, (x, y) in rng.items()},
                      (rides[r]['lat'][a], rides[r]['lon'][a], rides[r]['lat'][b], rides[r]['lon'][b])))
    return found


def own_ids(book, stretches):
    """Stable ids: `book` is ids.json's list [{id, start, end, length_m}], `stretches` [(ends, length)] longest first.
    A stretch takes the id of an entry whose start and end are each within OWN_ID_M and whose length is within 10 %
    (so a ride added to a stretch does not renumber it); else the next id below every id ever given, dropped
    entries' included (an id is never reused for another road). The book is updated in place."""
    used, ids = set(), []
    for (la0, lo0, la1, lo1), ln in stretches:
        hit = next((e for e in book if e['id'] not in used and metres(la0, lo0, *e['start']) <= OWN_ID_M
                    and metres(la1, lo1, *e['end']) <= OWN_ID_M and abs(ln - e['length_m']) <= 0.1 * e['length_m']), None)
        if hit is None:
            hit = {'id': min([e['id'] for e in book] + [OWN_FIRST_ID + 1]) - 1}
            book.append(hit)
        hit.update(start=[la0, lo0], end=[la1, lo1], length_m=round(ln))
        used.add(hit['id'])
        ids.append(hit['id'])
    return ids


def own(a, log=print):
    """Find the stretches, build each with build_segment (the same writer and targets as `build`), write to a.out
    with ids.json beside them. a.out must not be build's folder: each wipes the .seg files it does not make."""
    out = Path(a.out)
    if out.resolve() == (cache_dir() / 'out').resolve():
        sys.exit("segments.py: own's --out must not be the folder `build` writes (each removes the other's files)")
    rides = load_rides(a.rides, log)
    stats = new_stats()
    found = own_find(rides, a.max_per_cell)

    def make(ln, members, sid, name, st):
        return build_segment(sid, [{'act': rides[o]['act'], 'name': name, 'date': rides[o]['date'],
                                    **{key: rides[o][key][i:j + 1] for key in ('t', 'd', 'lat', 'lon', 'alt')}}
                                   for o, (i, j) in members.items()], a, st)

    kept = [s for s in found if make(s[0], s[1], OWN_FIRST_ID, 'OWN', stats)]
    book_path = out / 'ids.json'
    try:
        book = json.loads(book_path.read_text()) if book_path.exists() else []
        assert all(len(e['start']) == len(e['end']) == 2 and e['id'] == int(e['id']) and e['length_m'] > 0 for e in book)
    except (ValueError, KeyError, TypeError, AssertionError):
        log(f'{book_path} is not readable: ids start again')
        book = []
    ids = own_ids(book, [(s[2], s[0]) for s in kept])
    built = []
    for (ln, members, _), sid in zip(kept, ids):
        name = f'OWN {ln / 1000:.1f} KM {OWN_FIRST_ID - sid + 1}'
        f = make(ln, members, sid, name, new_stats())
        built.append((ln, sid, f))
        best = struct.unpack_from('<I', f, 0x17a + 13)[0]  # the first target is the PR
        log(f'  {name}: {len(members)} rides, best {best // 60}:{best % 60:02d}')
    built.sort(key=lambda x: -x[0])
    cut = max(0, len(built) - a.max)
    built = built[:a.max]
    if not a.dry_run:
        out.mkdir(parents=True, exist_ok=True)
        for old in out.glob('*.seg'):
            old.unlink()
        for _, sid, f in built:
            (out / f'{sid}.seg').write_bytes(f)
        tmp = book_path.with_suffix('.tmp')
        tmp.write_text(json.dumps(book))
        os.replace(tmp, book_path)
    log(f'{len(found)} stretches found, {len(built)} kept ({cut} cut by --max), {stats["dropped"]} efforts dropped by '
        f'sanity, {stats["rare"]} too rare, {stats["small"]} too short or flat'
        + (' (dry run: nothing written)' if a.dry_run else f', files in {a.out}'))
    return 0


# ---------------------------------------------------------------------------------------------------------------
# write
# ---------------------------------------------------------------------------------------------------------------

NAME = re.compile(r'^\d+\.seg$')


def read_back(path):
    """The file's bytes as the drive has them: not from the page cache (macOS F_NOCACHE, Linux fadvise DONTNEED).
    shortcut: elsewhere (no such call) the read may come from the cache, so a cut USB link can go unseen; use a POSIX OS."""
    fd = os.open(path, os.O_RDONLY)
    with os.fdopen(fd, 'rb') as f:
        if fcntl is not None and hasattr(fcntl, 'F_NOCACHE'):
            fcntl.fcntl(fd, fcntl.F_NOCACHE, 1)
        elif hasattr(os, 'posix_fadvise'):
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        return f.read()


def write(out, volume, log=print, prune=False, init=False):
    """`out` is a folder or a list of them (build's and own's: both coexist on the drive). Copy the built files to <volume>/Rustfw/Segments, each checked before its rename; then delete the `<id>.seg`
    files this tool put there earlier (the manifest, per destination) that are no longer wanted. Raises SystemExit
    for a volume that is not a BRYTON drive (no Rustfw/ folder, unless `init`) and for nothing to write (unless `prune`)."""
    root = Path(volume)
    if not root.is_dir() or not ((root / 'Rustfw').is_dir() or init):
        sys.exit(f'segments.py: {root} is not a directory holding Rustfw/ (a mistyped path? --init creates it)')
    outs = [out] if isinstance(out, (str, Path)) else out
    want = {p.name: p.read_bytes() for o in outs for p in Path(o).glob('*.seg') if NAME.match(p.name)}
    if not want and not prune:
        sys.exit('segments.py: nothing built to write (--prune to remove the files of earlier writes)')
    dest = root / 'Rustfw' / 'Segments'
    dest.mkdir(parents=True, exist_ok=True)
    state = state_dir() / 'written.json'
    book = json.loads(state.read_text()) if state.exists() else {}
    key = str(dest.resolve())
    mine = set(book.get(key, []))

    def record(names):
        book[key] = sorted(names)
        state.parent.mkdir(parents=True, exist_ok=True)
        state.write_text(json.dumps(book))

    record(mine | set(want))  # before any write: a run cut half way still knows what it may have made
    for name, data in want.items():
        tmp, final = dest / (name + '.tmp'), dest / name
        try:
            with open(tmp, 'wb') as f:
                f.write(data)
                f.flush()
                os.fsync(f.fileno())
            if read_back(tmp) != data:
                raise OSError(f'{name}: the copy read back differs')
            os.replace(tmp, final)
        finally:
            tmp.unlink(missing_ok=True)
    gone = [n for n in mine - set(want) if NAME.match(n) and (dest / n).is_file()]
    for n in gone:
        (dest / n).unlink()
    record(want)
    log(f'{len(want)} files written and verified in {dest}, {len(gone)} old ones removed')
    return 0


# ---------------------------------------------------------------------------------------------------------------
# selftest
# ---------------------------------------------------------------------------------------------------------------

def synth_effort(length, secs, date, off_m=0.0, n=None):
    """A straight east-going effort at 0.0N 10.0E, `length` m in `secs` s, one point a second."""
    n = n or secs + 1
    deg = 1 / 111195.0
    return {'act': date, 'name': 'Synthetic climb', 'date': date, 't': [secs * i / (n - 1) for i in range(n)],
            'd': [length * i / (n - 1) for i in range(n)], 'lat': [0.0] * n,
            'lon': [10.0 + (off_m + length * i / (n - 1)) * deg for i in range(n)],
            'alt': [100 + 30 * i / (n - 1) for i in range(n)]}


SID = 4_000_000_001


def synth_cache():
    gap = synth_effort(2000, 420, '2026-05-01')
    gap['alt'][5] = None  # a barometer gap: no crash
    return [synth_effort(2000, 400, '2026-01-01'), synth_effort(2000, 380, '2026-03-01'), gap,
            synth_effort(2000, 370, '2025-01-01'),  # the best of all, older than the last 5: PR differs from BEST5
            synth_effort(2000, 410, '2026-06-01'),
            synth_effort(2000, 405, '2026-07-01'), synth_effort(2000, 415, '2026-08-01'),
            synth_effort(2400, 400, '2026-02-01'),  # 20 % too long: dropped
            synth_effort(2000, 400, '2026-02-02', off_m=80)]  # starts 80 m away: dropped


class A:
    min_km, min_gain_m, min_rides, max, kom = 0.5, 10, 2, 128, {SID: 330}


def read_seg(b):
    """An independent reader of the layout: (header dict, times[point][target])."""
    sid, _, fit_at = struct.unpack_from('<III', b, 1)
    n = b[0x179]
    np_, n2 = struct.unpack_from('<II', b, HEADER)
    assert n2 == n and 2 <= np_ <= MAX_POINTS and fit_at == HEADER + 8 + 13 * n + 4 * np_ * n
    times = [struct.unpack_from(f'<{n}I', b, HEADER + 8 + 13 * n + 4 * n * i) for i in range(np_)]
    fit = b[fit_at:]
    assert fit[8:12] == b'.FIT' and sync.fit_crc(fit) == 0 and struct.unpack_from('<I', fit, 4)[0] == len(fit) - 14
    return {'id': sid, 'n': n, 'np': np_, 'dist': struct.unpack_from('<I', b, 0x2d)[0],
            'gain': struct.unpack_from('<I', b, 0x35)[0],
            'codes': [bytes(b[0x17a + 0x11 * k:0x17c + 0x11 * k]) for k in range(n)]}, times


def selftest():
    f = build_segment(SID, synth_cache(), A, st := new_stats())
    assert st['dropped'] == 2, st
    h, times = read_seg(f)
    assert h['id'] == SID and h['n'] == 4 and h['codes'] == [b'07', b'05', b'12', b'13'], h
    assert h['dist'] == 2000 and 27 <= h['gain'] <= 30 and h['np'] == 82, h  # 2000 m at 25 m spacing
    for k in range(4):
        col = [t[k] for t in times]
        assert col[0] == 0 and col == sorted(col) and col[-1] > col[1], (k, col[:5])
    assert times[-1] == (370, 330, 415, 380), times[-1]  # PR, KOM, RECENT, BEST5 of the last 5
    assert abs(times[h['np'] // 2][1] - 165) <= 4  # the KOM at constant pace: halfway after half the time
    # thresholds and the decimation of a long track
    st = new_stats()
    assert build_segment(7, synth_cache()[:1], A, st) is None and st['rare'] == 1
    assert build_segment(7, [synth_effort(300, 60, 'a'), synth_effort(300, 61, 'b')], A, st) is None and st['small'] == 1
    long = prep(synth_effort(20000, 2000, 'a', n=5000))
    assert len(thin(long)) == 400 and thin(long)[0] == 0 and thin(long)[-1] == 4999
    # the committed fixture is what the tool makes now
    assert FIXTURE.read_bytes() == f, f'{FIXTURE} differs from a fresh one: run segments.py fixture'
    # write + verify into a temp dir; a file an earlier write made and is no longer wanted goes, others stay
    with tempfile.TemporaryDirectory() as d:
        os.environ['BRYTON_SEGMENTS_CACHE'] = d + '/cache'
        out, vol = Path(d, 'out'), Path(d, 'vol')
        out.mkdir()
        (vol / 'Rustfw').mkdir(parents=True)
        (out / '4000000001.seg').write_bytes(f)
        write(out, vol, log=lambda *_: None)
        dest = vol / 'Rustfw' / 'Segments'
        (dest / 'other.seg').write_bytes(b'x')
        assert (dest / '4000000001.seg').read_bytes() == f
        (out / '4000000001.seg').rename(out / '4000000002.seg')
        write(out, vol, log=lambda *_: None)
        assert sorted(p.name for p in dest.iterdir()) == ['4000000002.seg', 'other.seg']
        # a mistyped volume, an empty build and a failed copy leave things as they were
        for bad in (lambda: write(out, Path(d, 'typo')), lambda: write(Path(d, 'none'), vol)):
            try:
                bad()
                raise AssertionError('should refuse')
            except SystemExit:
                pass
        assert not Path(d, 'typo').exists()
        # the cache next to the manifest: a build after a write works (it once globbed written.json as a ride)
        acts = acts_dir()
        acts.mkdir(parents=True)
        for k, e in enumerate(synth_cache()):
            n = len(e['t'])
            seg = {'segment_id': SID, 'start_index': 0, 'end_index': n - 1, 'name': e['name']}
            a = {'date': e['date'], 'segments': [seg, dict(seg)], 'time': e['t'], 'distance': e['d'], 'lat': e['lat'],
                 'lon': [x for x in e['lon']], 'alt': e['alt']}
            (acts / f'i{k}.json').write_text(json.dumps(a))
        args = argparse.Namespace(min_km=0.5, min_gain_m=10, min_rides=2, max=128, kom={SID: 330}, out=str(Path(d, 'o2')))
        build(args, log=lambda *_: None)
        built = (Path(d, 'o2') / f'{SID}.seg').read_bytes()
        assert read_seg(built)[0]['n'] == 4  # the duplicate (same activity, segment and start) counted once
        write(Path(d, 'o2'), vol, log=lambda *_: None)
        assert (dest / f'{SID}.seg').read_bytes() == built
    own_selftest()
    assert gain([None, 100, 105, None, 110]) == 10
    assert whole_ok(synth_effort(100, 10, 'a')) and not whole_ok({**synth_effort(100, 10, 'a'), 'd': [0] * 11})
    print('segments.py selftest: ok')


def synth_ride(path, day, lead, shared, speed, back=False, off=0, detour=None):
    """A FIT ride at 0 N 10 E (made up): a lead-in of its own, `shared` metres east along y = 0 (reversed when `back`),
    a lead-out of its own, one point a second at `speed` m/s. `lead` picks the lead-in's bearing so rides share only the stretch."""
    deg = 1 / 111195.0
    xs = [(-1500 + 1500 * i / 300, lead * (1500 - 1500 * i / 300)) for i in range(300)]  # lead-in to (0, 0)
    d0, d1 = detour or (0, 0)  # a detour: the ride leaves the road by up to 100 m between x = d0 and d1
    xs += [(shared * i / 400, off + (100 * math.sin(math.pi * (shared * i / 400 - d0) / (d1 - d0)) if d0 < shared * i / 400 < d1 else 0))
           for i in range(401)]  # the stretch
    xs += [(shared + 1500 * i / 300, lead * 1500 * i / 300) for i in range(1, 301)]  # lead-out
    if back:
        xs = xs[::-1]  # the same road, the other way
    pts, d, t = [], 0.0, []
    for k, (x, y) in enumerate(xs):
        if k:
            d += math.hypot(x - xs[k - 1][0], y - xs[k - 1][1])
        pts.append((y * deg, 10.0 + x * deg, d, 100 + 30 * min(max(x, 0), shared) / shared))
    # one point every 5 m is `speed` m/s when the time is distance / speed
    times = [round(p[2] / speed) + 1_000_000_000 + 86400 * day for p in pts]
    path.write_bytes(fit_course(pts, times))


def fit_file(body):
    f = struct.pack('<BBHI4s', 12, 0x10, 2100, len(body), b'.FIT') + bytes(body)
    return f + struct.pack('<H', sync.fit_crc(f))


def own_selftest():
    import random
    quiet = lambda *_: None

    def args(d, **kw):
        return argparse.Namespace(min_km=1.0, min_gain_m=0, min_rides=2, max=128, kom={}, dry_run=False,
                                  out=str(Path(d, 'out')), rides=d, max_per_cell=8, **kw)

    def put(d, specs, first=0):  # spec: (lead, shared, speed[, back[, off[, detour]]])
        for k, spec in enumerate(specs, first):
            synth_ride(Path(d, f'r{k:02d}.fit'), k + 1, *spec)

    def segs(d):
        out = Path(d, 'out')
        return [read_seg(f.read_bytes()) + (f.name,) for f in sorted(out.glob('*.seg'))]

    def run(specs):
        with tempfile.TemporaryDirectory() as d:
            put(d, specs)
            own(args(d), quiet)
            return segs(d)

    def find(specs, **kw):
        with tempfile.TemporaryDirectory() as d:
            put(d, specs)
            return own_find(load_rides(d, quiet), **kw)

    two = run([(1, 2000, 6), (-1, 2000, 7)])
    assert len(two) == 1 and 1900 <= two[0][0]['dist'] <= 2100 and two[0][0]['n'] == 3, two  # PR, RECENT, BEST5
    assert two[0][0]['id'] == OWN_FIRST_ID and two[0][2] == f'{OWN_FIRST_ID}.seg'
    assert run([(1, 500, 6), (-1, 500, 7)]) == []  # under 1 km
    assert run([(1, 2000, 6), (-1, 2000, 7, True)]) == []  # the other way
    three = run([(1, 2000, 6), (-1, 2000, 7), (0.5, 2000, 5)])
    assert len(three) == 1 and 1900 <= three[0][0]['dist'] <= 2100, three
    assert three[0][1][-1][0] <= 2000 / 7 + 8, three[0][1][-1]  # the PR: the 7 m/s ride's time over the stretch
    # the 90 % claim: a ride on 95 % of the stretch is a member, one on 80 % is not; its overlap, mostly claimed, is dropped
    assert len(f := find([(1, 2000, 6), (-1, 2000, 7), (0.5, 1900, 5)])) == 1 and set(f[0][1]) == {0, 1, 2}, f
    assert len(f := find([(1, 2000, 6), (-1, 2000, 7), (0.5, 1600, 5)])) == 1 and set(f[0][1]) == {0, 1}, f
    # a ride that skips 200 m of the road: the detour is not in its effort, the stretch is the longer piece
    assert len(f := find([(1, 2000, 6), (-1, 2000, 7), (0.5, 2000, 5, False, 0, (500, 700))])) == 1, f
    assert set(f[0][1]) == {0, 1, 2} and 1200 <= f[0][0] <= 1400, f  # not the 2000 m with the detour in it
    # stable ids: a ride added to a stretch keeps its id; a new road gets the next id down; none is reused
    with tempfile.TemporaryDirectory() as d:
        put(d, [(1, 2000, 6), (-1, 2000, 7)])
        own(args(d), quiet)
        first = segs(d)
        put(d, [(0.5, 2000, 5)], 2)  # a third ride moves the stretch's ends a little: same id
        own(args(d), quiet)
        again = segs(d)
        assert [x[0]['id'] for x in first] == [x[0]['id'] for x in again] == [OWN_FIRST_ID]
        for f in Path(d).glob('*.fit'):
            f.unlink()
        put(d, [(1, 2000, 6, False, 400), (-1, 2000, 7, False, 400)])  # another road, 400 m north
        own(args(d), quiet)
        assert [x[0]['id'] for x in segs(d)] == [OWN_FIRST_ID - 1]
        assert sorted(e['id'] for e in json.loads(Path(d, 'out', 'ids.json').read_text())) == [OWN_FIRST_ID - 1, OWN_FIRST_ID]
        own(args(d, ), quiet)  # again: nothing changes
        assert [x[0]['id'] for x in segs(d)] == [OWN_FIRST_ID - 1]
        # --max cuts the shortest, and the dry run writes nothing
        before = sorted(Path(d, 'out').iterdir())
        a = args(d)
        a.dry_run = True
        own(a, quiet)
        assert sorted(Path(d, 'out').iterdir()) == before
        a.dry_run, a.out = False, str(cache_dir() / 'out')
        try:
            own(a, quiet)
            raise AssertionError('should refuse build\'s folder')
        except SystemExit:
            pass
    # 30 rides on one road: one stretch with all of them, and the matching is bounded by max_per_cell
    leads = [math.tan(math.radians(-80 + 160 * k / 29)) for k in range(30)]  # lead-ins 5.5 degrees apart: they share no km
    specs = [(lead, 2000, 5 + k % 4) for k, lead in enumerate(leads)]
    with tempfile.TemporaryDirectory() as d:
        put(d, specs)
        t0 = time.time()
        rides = load_rides(d, quiet)
        capped, full = {}, {}
        f = own_find(rides, stats=capped)
        secs = time.time() - t0
        own_find(rides, max_per_cell=100, stats=full)
        assert len(f) == 1 and len(f[0][1]) == 30 and 1900 <= f[0][0] <= 2300, (len(f), [len(x[1]) for x in f])
        assert capped['cmp'] < 0.7 * full['cmp'], (capped, full)  # 0.55 measured: the lead-ins hold few rides per cell
        assert secs < 20, secs  # about a second; a quadratic version is several times that
        print(f'segments.py own: 30 same-road rides in {secs:.1f} s, {capped["cmp"]} candidates ({full["cmp"]} unbounded)')
        own(args(d), quiet)
        assert len(segs(d)) == 1
    # both sources on one drive: write takes both folders, and prunes only what neither lists
    with tempfile.TemporaryDirectory() as d:
        os.environ['BRYTON_SEGMENTS_CACHE'] = d + '/cache'
        o1, o2, vol = Path(d, 'o1'), Path(d, 'o2'), Path(d, 'vol')
        o1.mkdir(), o2.mkdir(), Path(vol, 'Rustfw').mkdir(parents=True)
        (o1 / '1.seg').write_bytes(b'a'), (o2 / '2.seg').write_bytes(b'b')
        write([o1, o2], vol, log=quiet)
        dest = vol / 'Rustfw' / 'Segments'
        assert sorted(p.name for p in dest.iterdir()) == ['1.seg', '2.seg']
        (o2 / '2.seg').unlink()
        write([o1, o2], vol, log=quiet)
        assert sorted(p.name for p in dest.iterdir()) == ['1.seg']
    # fit_records: a compressed timestamp, a developer field, junk positions, a truncated file
    T, deg = 1_000_000_000, lambda x: round(x * SEMI)
    body = bytes([0x60, 0, 0]) + struct.pack('<H', 20) + bytes([3, 253, 4, 0x86, 0, 4, 0x85, 1, 4, 0x85, 1, 0, 2, 0])
    body += bytes([0]) + struct.pack('<Iii', T, deg(10), deg(20)) + b'\xaa\xbb'  # + 2 bytes of developer data
    body += bytes([0]) + struct.pack('<Iii', T + 1, 0, 0) + b'\xaa\xbb'  # no position
    body += bytes([0]) + struct.pack('<Iii', T + 2, deg(91), deg(20)) + b'\xaa\xbb'  # not a latitude
    body += bytes([0x41, 0, 0]) + struct.pack('<H', 20) + bytes([2, 0, 4, 0x85, 1, 4, 0x85])
    body += bytes([0x80 | 1 << 5 | (T + 3) & 31]) + struct.pack('<ii', deg(11), deg(21))
    got = fit_records(fit_file(body))
    assert [r[0] for r in got] == [T, T + 3] and abs(got[1][1] - 11) < 1e-6 and abs(got[0][2] - 20) < 1e-6, got
    assert fit_records(fit_file(body)[:-30]) == [] and fit_records(b'junk') == [] and fit_records(b'') == []
    # mutation fuzz: flipped and truncated rides never raise; the unmodified one gives all its records
    with tempfile.TemporaryDirectory() as d:
        synth_ride(Path(d, 'x.fit'), 1, 1, 500, 5)
        base = Path(d, 'x.fit').read_bytes()
    assert len(fit_records(base)) == 300 + 401 + 300
    alive = 0
    for seed in range(200):
        rng = random.Random(seed)
        b = bytearray(base)
        if seed % 4 == 0:
            b = b[:rng.randrange(len(b))]
        else:
            for _ in range(rng.randrange(1, 9)):
                b[rng.randrange(len(b))] ^= 1 << rng.randrange(8)
        alive += bool(fit_records(bytes(b)))
    assert alive >= 80, alive  # most flips leave a readable file: the fuzz reaches the record code
    print('segments.py own selftest: ok')


def fixture(path):
    f = build_segment(SID, synth_cache(), A, new_stats())
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_bytes(f)
    print(f'{path}: {len(f)} bytes')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('cmd', choices=['fetch', 'build', 'write', 'own', 'fixture', 'selftest'])
    ap.add_argument('path', nargs='?', help='fixture: the output file')
    ap.add_argument('--since')
    ap.add_argument('--min-km', type=float, help='build 0.5, own 1')
    ap.add_argument('--min-gain-m', type=float, help='build 10, own 0')
    ap.add_argument('--min-rides', type=int, default=2)
    ap.add_argument('--max', type=int, default=128)
    ap.add_argument('--kom', action='append', metavar='ID=M:SS')
    ap.add_argument('--out', help='build: DIR (default cache/out); own: DIR (cache/own); write: DIR (default both)')
    ap.add_argument('--max-per-cell', type=int, default=8, help='own: rides indexed per 75 m cell (bounds the matching)')
    ap.add_argument('--volume', help='write: the drive; own: reads its Activities/')
    ap.add_argument('--rides', help='own: a folder of FIT rides')
    ap.add_argument('--dry-run', '--list', action='store_true', help='own: list the stretches, write nothing')
    ap.add_argument('--init', action='store_true', help='write: create Rustfw/ on the volume')
    ap.add_argument('--prune', action='store_true', help='write: allowed with nothing built (removes earlier files)')
    ap.add_argument('--refresh', action='store_true', help='fetch: ask again for cached activities')
    a = ap.parse_args()
    if a.cmd == 'selftest':
        return selftest()
    if a.cmd == 'fixture':
        return fixture(a.path or FIXTURE)
    if a.cmd == 'fetch':
        sys.exit(fetch(a.since, a.refresh))
    if a.cmd == 'write':
        if not a.volume:
            sys.exit('segments.py: write needs --volume PATH')
        sys.exit(write(a.out or [cache_dir() / 'out', cache_dir() / 'own'], a.volume, prune=a.prune, init=a.init))
    own_mode = a.cmd == 'own'
    a.min_km = a.min_km if a.min_km is not None else (OWN_MIN_KM if own_mode else 0.5)
    a.min_gain_m = a.min_gain_m if a.min_gain_m is not None else (0 if own_mode else 10)
    if own_mode:
        a.rides = a.rides or (a.volume and str(Path(a.volume) / 'Activities'))
        if not a.rides or not Path(a.rides).is_dir():
            sys.exit('segments.py: own needs --rides DIR or --volume PATH (holding Activities/)')
        a.kom, a.out = {}, a.out or str(cache_dir() / 'own')
        sys.exit(own(a))
    a.out = a.out or str(cache_dir() / 'out')
    try:
        a.kom = parse_kom(a.kom)
    except ValueError as e:
        ap.error(f'--kom ID=M:SS (a time of 1 s to 18 h): {e}')
    sys.exit(build(a))


if __name__ == '__main__':
    main()
