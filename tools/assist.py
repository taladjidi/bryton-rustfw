#!/usr/bin/env python3
"""AssistNow Offline for the GNSS receiver: download u-blox's orbit predictions with the rider's own token and put
them on the mounted unit as \\Rustfw\\mga.ubx, which the firmware streams to the receiver at its start (the messages
of that day only; src/drivers/mga.rs).

    python3 tools/assist.py [--volume /Volumes/BRYTON] [--gnss gps,gal,bds] [--weeks 1] [--days 1]
    python3 tools/assist.py selftest        # no network (check.sh)

The token is the rider's: u-blox's Thingstream portal (free plan) gives one for AssistNow. It is read from the
environment (UBLOX_ASSISTNOW_TOKEN) or from ~/.config/bryton-rustfw/assist.json {"token": "..."} (outside the
repository, made private: chmod 600), never from the command line, never printed, never written to the unit. See
docs/sync.md.

The service: https://offline-live1.services.u-blox.com/GetOfflineData.ashx?token=...;gnss=gps,gal,glo;
format=mga;period=<weeks>;resolution=<days>. The query is inferred from u-blox's AssistNow Offline service notes
(the portal's documentation is the authority; a changed parameter shows as an HTTP error here, not on the unit). The
answer is a concatenation of UBX-MGA-ANO messages (class 0x13, id 0x20, 76-byte payload: type, version, svId,
gnssId, year since 2000, month, day, reserved, 64 data bytes, 4 reserved; u-blox M10 SPG 5.10 interface
description UBX-21035062). This tool keeps the whole frames with a good checksum from the newest date up to today on, in date order (the
firmware relies on it: it stops reading at a date past the one it sends), and nothing else, so the file on the unit stays small (the firmware reads at most 512 KB of it). It copies the file with a
temporary name, reads it back and compares it, and only then renames it (the USB link can drop).
"""
import argparse
import datetime
import fcntl
import json
import os
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request

HOST = 'https://offline-live1.services.u-blox.com/GetOfflineData.ashx'
CONFIG = os.path.expanduser('~/.config/bryton-rustfw/assist.json')
GNSS_OK = {'gps', 'gal', 'glo', 'bds', 'qzss'}
FRAME = 84  # 6 header, 76 payload, 2 checksum


def token():
    """The rider's token: the environment first, then the private config file; None if there is none."""
    t = os.environ.get('UBLOX_ASSISTNOW_TOKEN')
    if t:
        return t.strip()
    try:
        if os.stat(CONFIG).st_mode & 0o077:
            print(f'{CONFIG} is readable by others: chmod 600 it', file=sys.stderr)
            return None
        with open(CONFIG) as f:
            return str(json.load(f).get('token', '')).strip() or None
    except (OSError, ValueError):
        return None


def url(tok, gnss, weeks, days):
    """The request; the token only appears in the string this returns (never printed)."""
    if not set(gnss.split(',')) <= GNSS_OK or not 1 <= weeks <= 5 or not 1 <= days <= 3:
        raise ValueError('gnss is gps,gal,glo,bds,qzss; weeks 1 to 5; days 1 to 3')
    q = ';'.join([f'token={urllib.parse.quote(tok)}', f'gnss={gnss}', 'format=mga', f'period={weeks}', f'resolution={days}'])
    return f'{HOST}?{q}'


def checksum(body):
    a = b = 0
    for x in body:
        a = (a + x) & 255
        b = (b + a) & 255
    return bytes([a, b])


def frames(data):
    """The whole MGA-ANO frames in `data` with a good checksum, as (date, bytes); noise and damage skipped."""
    i, out = 0, []
    while i + FRAME <= len(data):
        if data[i:i + 6] == b'\xb5\x62\x13\x20\x4c\x00' and checksum(data[i + 2:i + 82]) == data[i + 82:i + 84] \
                and data[i + 6] == 0:
            y, mo, d = data[i + 10], data[i + 11], data[i + 12]
            out.append(((2000 + y, mo, d), data[i:i + FRAME]))
            i += FRAME
        else:
            i += 1
    return out


def trim(data, today):
    """The frames from the newest date up to `today` (a datetime.date) on, sorted by date (stable: the order within a
    date is the service's). A 2 or 3 day resolution has no message dated today itself."""
    fs = sorted(frames(data), key=lambda x: x[0])
    t = (today.year, today.month, today.day)
    best = max((d for d, _ in fs if d <= t), default=None)
    if best is None or (today - datetime.date(*best)).days > 3:  # as the firmware: older than a 3 day resolution is stale
        return b''
    return b''.join(f for d, f in fs if d >= best)


def put(volume, data):
    """Writes Rustfw/mga.ubx under `volume` through a temporary name, flushes the OS cache, compares it read back, then
    renames. The temporary file is removed on any failure, and a stale one from an earlier run is cleaned first."""
    folder = os.path.join(volume, 'Rustfw')
    os.makedirs(folder, exist_ok=True)
    tmp, final = os.path.join(folder, 'mga.tmp'), os.path.join(folder, 'mga.ubx')
    if os.path.exists(tmp):
        os.remove(tmp)
    try:
        with open(tmp, 'wb') as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        if hasattr(os, 'sync'):
            os.sync()
        with open(tmp, 'rb') as f:
            if hasattr(fcntl, 'F_NOCACHE'):
                fcntl.fcntl(f.fileno(), fcntl.F_NOCACHE, 1)  # macOS: read it from the drive, not the cache
            if f.read() != data:
                raise OSError('the copy read back differently: not used')
        os.replace(tmp, final)
    except BaseException:
        if os.path.exists(tmp):
            os.remove(tmp)
        raise
    return final


def download(tok, gnss, weeks, days):
    try:
        with urllib.request.urlopen(url(tok, gnss, weeks, days), timeout=60) as r:
            return r.read()
    except urllib.error.HTTPError as e:
        raise SystemExit(f'the service answered HTTP {e.code} (token wrong or expired?)')
    except urllib.error.URLError as e:
        raise SystemExit(f'no answer from the service: {e.reason}')


def selftest():
    import datetime as dt

    def ano(sv, gnss, date):
        body = bytes([0x13, 0x20, 76, 0, 0, 0, sv, gnss, date[0] - 2000, date[1], date[2], 0]) + bytes([sv]) * 64 + bytes(4)
        return b'\xb5\x62' + body + checksum(body)

    today = dt.date(2026, 10, 26)
    data = b''.join(ano(sv, 0, (2026, 10, d)) for d in (25, 26, 27) for sv in range(1, 5))
    bad = bytearray(ano(9, 0, (2026, 10, 26)))
    bad[20] ^= 1
    noisy = b'\xff' * 5 + bytes(bad) + data[:100] + b'\xb5\x62\x13\x20' + data
    assert len(frames(noisy)) >= 12, len(frames(noisy))
    kept = trim(noisy, today)
    assert all(d >= (2026, 10, 26) for d, _ in frames(kept)) and len(kept) % FRAME == 0
    assert len(kept) == 8 * FRAME, len(kept)
    assert trim(data, dt.date(2026, 11, 1)) == b'', 'stale: nothing'
    with tempfile.TemporaryDirectory() as v:
        p = put(v, kept)
        assert open(p, 'rb').read() == kept and os.listdir(os.path.join(v, 'Rustfw')) == ['mga.ubx']
    u = url('SECRET', 'gps,gal', 1, 1)
    assert u.startswith(HOST) and 'token=SECRET' in u and 'gnss=gps,gal' in u
    for args in (('x', 'gps,foo', 1, 1), ('x', 'gps', 0, 1), ('x', 'gps', 1, 9)):
        try:
            url(*args)
            raise AssertionError(args)
        except ValueError:
            pass
    # 3-day resolution: dates 20, 23, 26; on the 25th the 23rd on, in date order
    shuffled = b''.join(ano(1, 26 - 3 * k, (2026, 10, 26 - 3 * k)) for k in range(3))
    kept3 = trim(shuffled, dt.date(2026, 10, 25))
    assert [d for d, _ in frames(kept3)] == [(2026, 10, 23), (2026, 10, 26)], frames(kept3)
    # the token: environment first; the private file only if mode 0600; never printed
    import io
    import contextlib
    global CONFIG
    os.environ.pop('UBLOX_ASSISTNOW_TOKEN', None)
    out = io.StringIO()
    with tempfile.TemporaryDirectory() as td, contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
        CONFIG = os.path.join(td, 'assist.json')
        assert token() is None, 'no file'
        with open(CONFIG, 'w') as f:
            json.dump({'token': 't'}, f)
        os.chmod(CONFIG, 0o644)
        assert token() is None, 'readable by others'
        os.chmod(CONFIG, 0o600)
        assert token() == 't'
        os.environ['UBLOX_ASSISTNOW_TOKEN'] = ' abc '
        assert token() == 'abc'
        # a stale temporary file is cleaned and a failed copy leaves none
        os.makedirs(os.path.join(td, 'v', 'Rustfw'))
        stale = os.path.join(td, 'v', 'Rustfw', 'mga.tmp')
        open(stale, 'wb').write(b'old')
        put(os.path.join(td, 'v'), kept)
        assert not os.path.exists(stale)
    assert 'abc' not in out.getvalue() and '"t"' not in out.getvalue(), 'the token is never printed'
    print('assist.py selftest ok')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('cmd', nargs='?', default='fetch', choices=['fetch', 'selftest'])
    ap.add_argument('--volume', help='the mounted BRYTON volume (default /Volumes/BRYTON if there; else out/mga.ubx)')
    ap.add_argument('--gnss', default='gps,gal,bds', help='systems asked for: the receiver\'s default set (add glo for GPS+GAL+GLO)')
    ap.add_argument('--weeks', type=int, default=1, help='weeks of predictions, 1 to 5 (default 1)')
    ap.add_argument('--days', type=int, default=1, help='days between predictions, 1 to 3 (default 1)')
    a = ap.parse_args()
    if a.cmd == 'selftest':
        return selftest()
    tok = token()
    if not tok:
        raise SystemExit(f'no token: set UBLOX_ASSISTNOW_TOKEN or write {CONFIG} (docs/sync.md)')
    data = trim(download(tok, a.gnss, a.weeks, a.days), datetime.datetime.now(datetime.timezone.utc).date())
    if not data:
        raise SystemExit('the service sent no usable AssistNow Offline message')
    vol = a.volume or ('/Volumes/BRYTON' if os.path.isdir('/Volumes/BRYTON') else None)
    if vol:
        print(f'{len(data) // FRAME} messages, {len(data)} bytes -> {put(vol, data)}')
    else:
        os.makedirs('out', exist_ok=True)
        with open('out/mga.ubx', 'wb') as f:
            f.write(data)
        print(f'{len(data) // FRAME} messages -> out/mga.ubx (copy it to \\Rustfw\\mga.ubx on the unit)')


if __name__ == '__main__':
    main()
