#!/usr/bin/env python3
"""Live segment files (`\\Rustfw\\Segments\\<id>.seg`, docs/segments.md) from the rider's own Intervals.icu rides.

  segments.py fetch [--since YYYY-MM-DD]   rides with segment matches, their segment lists and streams -> the cache
  segments.py build [--min-km 0.5] [--min-gain-m 10] [--min-rides 2] [--max 128] [--kom ID=M:SS ...] [--out DIR]
  segments.py write --volume PATH          the built files -> PATH/Rustfw/Segments/, each read back and compared
  segments.py fixture [OUT]                the synthetic .seg the Rust tests parse (test-data/segment/synthetic.seg)
  segments.py selftest                     synthetic checks, no network (check.sh)

Source: Intervals.icu (Strava's segment matches are in its activities; no Strava API: it needs a subscription).
Config as sync.py: ~/.config/bryton-rustfw/sync.json, block `intervals`. Cache: ~/.cache/bryton-rustfw/segments/
(BRYTON_SEGMENTS_CACHE overrides); nothing personal enters the repository.

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
import importlib.util
import json
import math
import os
import struct
import sys
import tempfile
import time
from pathlib import Path

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
    return {'dropped': 0, 'rare': 0, 'small': 0, 'wide': 0}


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


def fetch(since, log=print):
    cfg = sync.load('sync.json', None)
    if not cfg or 'intervals' not in cfg:
        sys.exit(f'segments.py: no `intervals` block in {sync.home() / "sync.json"} (docs/sync.md)')
    icu = sync.Intervals(cfg['intervals'])
    cache = cache_dir()
    cache.mkdir(parents=True, exist_ok=True)
    oldest = since or '2000-01-01'
    newest = datetime.date.today().isoformat()
    try:
        acts = get(icu, f'athlete/{icu.athlete}/activities?oldest={oldest}&newest={newest}')
        acts = [a for a in acts if a.get('has_segments')]
        have = sum(1 for a in acts if (cache / f'{a["id"]}.json').exists())
        log(f'{len(acts)} activities with segments, {have} cached')
        for n, a in enumerate(acts, 1):
            f = cache / f'{a["id"]}.json'
            if f.exists():
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
            f.with_suffix('.tmp').write_text(json.dumps(out))
            os.replace(f.with_suffix('.tmp'), f)
            log(f'  {n}/{len(acts)} fetched')
    except Stop as e:
        log(f'segments.py: {e}')
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
        a = json.loads(f.read_text())
        n = len(a['time'])
        if not len(a['distance']) == len(a['lat']) == len(a['lon']) == n:
            continue
        for s in a['segments']:
            i, j, sid = s.get('start_index'), s.get('end_index'), s.get('segment_id')
            if sid is None or i is None or j is None or not 0 <= i < j < n:
                continue
            alt = a['alt'][i:j + 1] if len(a['alt']) == n else []
            out.setdefault(int(sid), []).append({
                'name': s.get('name') or '', 'date': a['date'], 't': a['time'][i:j + 1], 'd': a['distance'][i:j + 1],
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


def fit_course(pts):
    """pts: [(lat, lon, dist_m, alt_m or None)] as a FIT course: file_id, then one record per point, CRCs set."""
    # file_id (0): type (0) = 6 course, manufacturer (1) = 255, time_created (4) = 0
    body = bytearray([0x40, 0, 0, 0, 0, 3, 0, 1, 0, 1, 2, 0x84, 4, 4, 0x86])
    body += bytes([0]) + struct.pack('<BHI', 6, 255, 0)
    # record (20): timestamp (253), position_lat (0), position_long (1), distance (5, cm), altitude (2, (m + 500) * 5)
    body += bytes([0x41, 0, 0, 20, 0, 5, 253, 4, 0x86, 0, 4, 0x85, 1, 4, 0x85, 5, 4, 0x86, 2, 2, 0x84])
    for k, (lat, lon, dist, alt) in enumerate(pts):
        body += bytes([1]) + struct.pack('<IiiIH', k, round(lat * SEMI), round(lon * SEMI), round(dist * 100),
                                         round(((alt or 0) + 500) * 5))
    f = struct.pack('<BBHI4s', 12, 0x10, 2100, len(body), b'.FIT') + bytes(body)
    return f + struct.pack('<H', sync.fit_crc(f))


def seg_file(sid, name, length, gain_m, pts, targets):
    """pts as in fit_course; targets [(kind, total_s, [elapsed s per point])] in file order."""
    n, np_ = len(targets), len(pts)
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


def build_segment(sid, efforts, a, stats):
    """The .seg bytes of one segment, or None when it is not kept. stats counts the dropped efforts."""
    def whole(e):  # a GPS or sensor gap (None) inside the effort: dropped
        return None not in e['t'] + e['d'] + e['lat'] + e['lon'] and e['t'][-1] > e['t'][0] and e['d'][-1] > e['d'][0]

    stats['dropped'] += sum(1 for e in efforts if not whole(e))
    efforts = [prep(dict(e)) for e in efforts if whole(e)]
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
    if len(ok) < a.min_rides:
        stats['rare'] += 1
        return None
    g = gain(best['alt'])
    if length < a.min_km * 1000 or g < a.min_gain_m:
        stats['small'] += 1
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
        out[int(k)] = int(m) * 60 + int(sec)
    return out


def build(a, log=print):
    efforts = load_efforts(cache_dir())
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
        f'{stats["small"]} too short or flat, {stats["wide"]} with an id over 32 bits, {len(keep)} files in {out}')
    return 0


# ---------------------------------------------------------------------------------------------------------------
# write
# ---------------------------------------------------------------------------------------------------------------

def write(out, volume, log=print):
    """Copy the built files to <volume>/Rustfw/Segments, read each back; delete only files an earlier write made."""
    dest = Path(volume) / 'Rustfw' / 'Segments'
    dest.mkdir(parents=True, exist_ok=True)
    mine = cache_dir() / 'written.json'
    old = set(json.loads(mine.read_text())) if mine.exists() else set()
    want = {p.name: p.read_bytes() for p in Path(out).glob('*.seg')}
    for name, data in want.items():
        tmp = dest / (name + '.tmp')
        with open(tmp, 'wb') as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, dest / name)
        if (dest / name).read_bytes() != data:
            raise OSError(f'{name}: the copy read back differs')
    gone = [n for n in old - set(want) if (dest / n).exists()]
    for n in gone:
        (dest / n).unlink()
    mine.parent.mkdir(parents=True, exist_ok=True)
    mine.write_text(json.dumps(sorted(want)))
    log(f'{len(want)} files written and verified in {dest}, {len(gone)} old ones removed')
    return 0


# ---------------------------------------------------------------------------------------------------------------
# selftest
# ---------------------------------------------------------------------------------------------------------------

def synth_effort(length, secs, date, off_m=0.0, n=None):
    """A straight east-going effort at 0.0N 10.0E, `length` m in `secs` s, one point a second."""
    n = n or secs + 1
    deg = 1 / 111195.0
    return {'name': 'Synthetic climb', 'date': date, 't': [secs * i / (n - 1) for i in range(n)],
            'd': [length * i / (n - 1) for i in range(n)], 'lat': [0.0] * n,
            'lon': [10.0 + (off_m + length * i / (n - 1)) * deg for i in range(n)],
            'alt': [100 + 30 * i / (n - 1) for i in range(n)]}


SID = 4_000_000_001


def synth_cache():
    return [synth_effort(2000, 400, '2026-01-01'), synth_effort(2000, 380, '2026-03-01'),
            synth_effort(2000, 420, '2026-05-01'), synth_effort(2000, 410, '2026-06-01'),
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
    assert h['dist'] == 2000 and h['gain'] in (29, 30) and h['np'] == 82, h  # 2000 m at 25 m spacing
    for k in range(4):
        col = [t[k] for t in times]
        assert col[0] == 0 and col == sorted(col) and col[-1] > col[1], (k, col[:5])
    assert times[-1] == (380, 330, 415, 380), times[-1]  # PR, KOM, RECENT, BEST5 of the last 5
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
        (out / '4000000001.seg').write_bytes(f)
        write(out, vol, log=lambda *_: None)
        dest = vol / 'Rustfw' / 'Segments'
        (dest / 'other.seg').write_bytes(b'x')
        assert (dest / '4000000001.seg').read_bytes() == f
        (out / '4000000001.seg').rename(out / '4000000002.seg')
        write(out, vol, log=lambda *_: None)
        assert sorted(p.name for p in dest.iterdir()) == ['4000000002.seg', 'other.seg']
    print('segments.py selftest: ok')


def fixture(path):
    f = build_segment(SID, synth_cache(), A, new_stats())
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_bytes(f)
    print(f'{path}: {len(f)} bytes')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('cmd', choices=['fetch', 'build', 'write', 'fixture', 'selftest'])
    ap.add_argument('path', nargs='?', help='fixture: the output file')
    ap.add_argument('--since')
    ap.add_argument('--min-km', type=float, default=0.5)
    ap.add_argument('--min-gain-m', type=float, default=10)
    ap.add_argument('--min-rides', type=int, default=2)
    ap.add_argument('--max', type=int, default=128)
    ap.add_argument('--kom', action='append', metavar='ID=M:SS')
    ap.add_argument('--out', default=str(cache_dir() / 'out'))
    ap.add_argument('--volume')
    a = ap.parse_args()
    if a.cmd == 'selftest':
        return selftest()
    if a.cmd == 'fixture':
        return fixture(a.path or FIXTURE)
    if a.cmd == 'fetch':
        sys.exit(fetch(a.since))
    if a.cmd == 'write':
        if not a.volume:
            sys.exit('segments.py: write needs --volume PATH')
        sys.exit(write(a.out, a.volume))
    a.kom = parse_kom(a.kom)
    sys.exit(build(a))


if __name__ == '__main__':
    main()
