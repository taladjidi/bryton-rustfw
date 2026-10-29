#!/usr/bin/env python3
"""Live segment files (`\\Rustfw\\Segments\\<id>.seg`, docs/segments.md) from the rider's own Intervals.icu rides.

  segments.py fetch [--since YYYY-MM-DD] [--refresh]   rides with segment matches, their segment lists and streams -> the cache
  segments.py build [--min-km 0.5] [--min-gain-m 10] [--min-rides 2] [--max 128] [--kom ID=M:SS ...] [--out DIR]
  segments.py write --volume PATH [--init] [--prune]   the built files -> PATH/Rustfw/Segments/, each read back and compared
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


def fit_course(pts):
    """pts: [(lat, lon, dist_m, alt_m or None)] as a FIT course: file_id, then one record per point, CRCs set."""
    # file_id (0): type (0) = 6 course, manufacturer (1) = 255, time_created (4) = 0
    body = bytearray([0x40, 0, 0, 0, 0, 3, 0, 1, 0, 1, 2, 0x84, 4, 4, 0x86])
    body += bytes([0]) + struct.pack('<BHI', 6, 255, 0)
    # record (20): timestamp (253), position_lat (0), position_long (1), distance (5, cm), altitude (2, (m + 500) * 5)
    body += bytes([0x41, 0, 0, 20, 0, 5, 253, 4, 0x86, 0, 4, 0x85, 1, 4, 0x85, 5, 4, 0x86, 2, 2, 0x84])
    for k, (lat, lon, dist, alt) in enumerate(pts):
        body += bytes([1]) + struct.pack('<IiiIH', k, round(lat * SEMI), round(lon * SEMI), round(dist * 100),
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
    """Copy the built files to <volume>/Rustfw/Segments, each checked before its rename; then delete the `<id>.seg`
    files this tool put there earlier (the manifest, per destination) that are no longer wanted. Raises SystemExit
    for a volume that is not a BRYTON drive (no Rustfw/ folder, unless `init`) and for nothing to write (unless `prune`)."""
    root = Path(volume)
    if not root.is_dir() or not ((root / 'Rustfw').is_dir() or init):
        sys.exit(f'segments.py: {root} is not a directory holding Rustfw/ (a mistyped path? --init creates it)')
    want = {p.name: p.read_bytes() for p in Path(out).glob('*.seg') if NAME.match(p.name)}
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
    assert gain([None, 100, 105, None, 110]) == 10
    assert whole_ok(synth_effort(100, 10, 'a')) and not whole_ok({**synth_effort(100, 10, 'a'), 'd': [0] * 11})
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
        sys.exit(write(a.out, a.volume, prune=a.prune, init=a.init))
    try:
        a.kom = parse_kom(a.kom)
    except ValueError as e:
        ap.error(f'--kom ID=M:SS (a time of 1 s to 18 h): {e}')
    sys.exit(build(a))


if __name__ == '__main__':
    main()
