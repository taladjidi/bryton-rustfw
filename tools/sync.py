#!/usr/bin/env python3
"""Rides off the bike computer to Intervals.icu and Strava, planned workouts from Intervals.icu onto it (docs/sync.md).

  sync.py [--go] [--volume PATH | --link] [--days N]   new rides up, the next N days of workouts down
  sync.py strava-login                                   one time: Strava's OAuth, the refresh token kept
  sync.py status                                         what is configured and what was uploaded
  sync.py selftest                                       offline checks (a fake server and volume; check.sh)

Without --go nothing is uploaded or written: it says what it would do.

Rides: `\\Activities\\<name>.fit` (this firmware's clean copy, or stock's ride) goes to Strava; to Intervals.icu goes
the full copy `\\Rustfw\\Activities\\<name>.fit` (Alpha1 and RMSSD in the records, recorder.rs "Two copies") when there
is one, else the clean one. Bryton's own server refuses the full one, so it never goes there (the Bryton app syncs
from the unit itself). Every file is checked whole (its FIT CRCs) before it goes anywhere: a copy the USB link cut
short is never uploaded. The rides found either on a mounted volume (this firmware's USB drive, or stock's) or
through the dev link (dev.py `ls` / `get`, read only).

Workouts: Intervals.icu's planned workouts (events of category WORKOUT) of today and the next N days, each as FIT
(`/events/{id}/download.fit`), written to `\\Workout\\calendar\\YYYYMMDD\\icu<id>.fit`, where stock's app puts them and
src/storage/workout.rs reads them. Only on a mounted volume: the dev link does not write.

Never twice: a state file remembers each upload by ride name, with the file's name, size and CRC-32; a ride already
uploaded to a service is not sent to it again. Intervals.icu also refuses a file it has (a hash of its bytes), and
Strava reports a duplicate, which is then remembered as done.

Config (never in the repository): ~/.config/bryton-rustfw/ (BRYTON_SYNC_HOME to move it), files 0600:
  sync.json     {"intervals": {"athlete_id": "i12345", "api_key": "..."},
                 "strava": {"client_id": "12345", "client_secret": "..."},
                 "volume": "/Volumes/BRYTON", "since": "2026-10-01", "days": 7}
                either service may be left out; INTERVALS_API_KEY in the environment overrides api_key
  strava.json   the Strava tokens (strava-login writes it)
  state.json    what was uploaded and written

APIs, read 2026-10-24:
- Intervals.icu (OpenAPI at https://intervals.icu/api/v1/docs; https://forum.intervals.icu/t/api-access-to-intervals-icu/609):
  basic auth user "API_KEY", the key as password; POST /api/v1/athlete/{id}/activities, multipart field `file`
  (201 created, 200 when nothing new: the file's hash is checked); GET /api/v1/athlete/{id}/events?oldest=&newest=
  &category=WORKOUT; GET /api/v1/athlete/{id}/events/{eventId}/download.fit. DFA a1 is read from a developer field
  named Alpha1 (https://forum.intervals.icu/t/average-dfa-a1-and-wbal-added-to-interval-fields/7528).
- Strava (https://developers.strava.com/docs/authentication/, https://developers.strava.com/docs/uploads/):
  OAuth2 code flow, scope activity:write, tokens from POST /oauth/token (authorization_code, then refresh_token);
  POST /api/v3/uploads (file, data_type=fit, external_id), then GET /api/v3/uploads/{id} once a second at most until
  activity_id or error ("... duplicate of activity N").

Python's standard library only.
"""
import argparse
import base64
import datetime
import http.server
import json
import os
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ICU = os.environ.get('SYNC_INTERVALS_URL', 'https://intervals.icu')
STRAVA = os.environ.get('SYNC_STRAVA_URL', 'https://www.strava.com')
REDIRECT_PORT = 8721
FULL_DIR = 'Rustfw/Activities'
WORKOUT_MAX = 8192  # src/storage/workout.rs FILE_MAX: a longer file is not shown on the unit


def home():
    return Path(os.environ.get('BRYTON_SYNC_HOME', Path.home() / '.config' / 'bryton-rustfw'))


def load(name, default):
    p = home() / name
    return json.loads(p.read_text()) if p.exists() else default


def save(name, data):
    """Written whole then renamed, 0600 (tokens and keys)."""
    home().mkdir(parents=True, exist_ok=True)
    os.chmod(home(), 0o700)
    tmp = home() / (name + '.tmp')
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    os.fchmod(fd, 0o600)  # a .tmp left by an older run keeps its mode through O_CREAT
    with os.fdopen(fd, 'w') as f:
        json.dump(data, f, indent=1, sort_keys=True)
    os.replace(tmp, home() / name)


# ---------------------------------------------------------------------------------------------------------------
# FIT: whole or not (the same CRC as src/storage/fit.rs)
# ---------------------------------------------------------------------------------------------------------------

CRC_T = [0x0000, 0xcc01, 0xd801, 0x1400, 0xf001, 0x3c00, 0x2800, 0xe401,
         0xa001, 0x6c00, 0x7800, 0xb401, 0x5000, 0x9c01, 0x8801, 0x4400]


def fit_crc(data, crc=0):
    for b in data:
        for nib in (b & 0xf, b >> 4):
            crc = (crc >> 4) ^ CRC_T[crc & 0xf] ^ CRC_T[nib]
    return crc


def fit_whole(data):
    """A whole FIT file: its header, its size and its CRCs (the FIT protocol, developer.garmin.com/fit/protocol)."""
    if len(data) < 14 or data[0] not in (12, 14) or data[8:12] != b'.FIT':
        return False
    h = data[0]
    size = int.from_bytes(data[4:8], 'little')
    if len(data) != h + size + 2:
        return False
    if h == 14 and data[12:14] != b'\0\0' and fit_crc(data[:12]) != int.from_bytes(data[12:14], 'little'):
        return False
    return fit_crc(data) == 0


def same_ride(clean, full):
    """Both copies of one ride begin with the same file_id (definition and message: 28 bytes, src/storage/fit.rs),
    which holds the ride's start: a full copy of another ride under the same name is never sent in its place."""
    return clean[clean[0]:clean[0] + 28] == full[full[0]:full[0] + 28]


def key(name, data):
    return f'{name}:{len(data)}:{zlib.crc32(data):08x}'


# ---------------------------------------------------------------------------------------------------------------
# Where the rides are: a mounted volume, or the dev link
# ---------------------------------------------------------------------------------------------------------------

class Volume:
    """The FAT volume mounted on the computer (this firmware's USB drive, or stock's)."""
    writable = True

    def __init__(self, root):
        self.root = Path(root)

    def __str__(self):
        return str(self.root)

    def files(self, folder):
        d = self.root / folder
        return sorted(p.name for p in d.iterdir() if p.is_file()) if d.is_dir() else []

    def read(self, path):
        return (self.root / path).read_bytes()

    def write(self, path, data):
        """Whole or not at all: a .tmp beside it (no reader takes it: workout.rs lists *.fit), flushed to the
        drive itself (F_FULLFSYNC on macOS: fsync alone leaves it in the drive's cache), then renamed."""
        p = self.root / path
        p.parent.mkdir(parents=True, exist_ok=True)
        tmp = p.with_name(p.name + '.tmp')
        try:
            with open(tmp, 'wb') as f:
                f.write(data)
                f.flush()
                flush(f.fileno())
            os.replace(tmp, p)
        finally:
            tmp.unlink(missing_ok=True)
        fd = os.open(p.parent, os.O_RDONLY)
        try:
            flush(fd)  # the directory entry too
        finally:
            os.close(fd)
        if p.read_bytes() != data:  # the drive can drop (AGENTS.md): read back, compare
            raise OSError(f'{p}: read back differs')

    def exists(self, path):
        return (self.root / path).is_file()


def flush(fd):
    import fcntl
    try:
        fcntl.fcntl(fd, fcntl.F_FULLFSYNC)
    except (AttributeError, OSError):
        os.fsync(fd)


class Link:
    """The dev link (dev.py), read only."""
    writable = False

    def __init__(self):
        sys.path.insert(0, str(ROOT))
        import dev
        self.dev = dev
        self.s = dev.connect()

    def __str__(self):
        return 'the dev link'

    def files(self, folder):
        try:
            return sorted(name for _, is_dir, name in self.dev.ls(self.s, folder) if not is_dir)
        except OSError:
            return []

    def read(self, path):
        return self.dev.get(self.s, path)


def find_volume(cfg):
    for v in (cfg.get('volume'), '/Volumes/BRYTON'):
        if v and Path(v, 'Activities').is_dir():
            return Volume(v)
    return None


# ---------------------------------------------------------------------------------------------------------------
# HTTP (urllib): JSON, multipart
# ---------------------------------------------------------------------------------------------------------------

def request(method, url, headers=None, data=None, form=None, files=None):
    """(status, body bytes). `form` urlencoded, or `files` multipart with `form` as its other fields."""
    headers = dict(headers or {})
    if files:
        boundary = uuid.uuid4().hex
        parts = []
        for k, v in (form or {}).items():
            parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode())
        for k, (fname, content) in files.items():
            parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"; filename="{fname}"\r\n'
                         f'Content-Type: application/octet-stream\r\n\r\n'.encode() + content + b'\r\n')
        data = b''.join(parts) + f'--{boundary}--\r\n'.encode()
        headers['Content-Type'] = f'multipart/form-data; boundary={boundary}'
    elif form is not None:
        data = urllib.parse.urlencode(form).encode()
        headers['Content-Type'] = 'application/x-www-form-urlencoded'
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


class Intervals:
    def __init__(self, cfg):
        self.athlete = cfg['athlete_id']
        k = os.environ.get('INTERVALS_API_KEY') or cfg['api_key']
        self.auth = {'Authorization': 'Basic ' + base64.b64encode(f'API_KEY:{k}'.encode()).decode()}

    def url(self, path):
        return f'{ICU}/api/v1/athlete/{self.athlete}/{path}'

    def upload(self, name, data):
        """The activity id (an upload Intervals.icu already had is no error: it says so)."""
        q = urllib.parse.urlencode({'external_id': name})
        st, body = request('POST', self.url(f'activities?{q}'), self.auth, files={'file': (name, data)})
        if st not in (200, 201):
            raise OSError(f'Intervals.icu upload {name}: HTTP {st} {body[:200]!r}')
        r = json.loads(body or b'{}')
        ids = [a.get('id') for a in r.get('activities') or []]
        return ','.join(str(i) for i in ids) or r.get('id') or ('already there' if st == 200 else 'created')

    def workouts(self, first, last):
        q = urllib.parse.urlencode({'oldest': first.isoformat(), 'newest': last.isoformat(), 'category': 'WORKOUT'})
        st, body = request('GET', self.url(f'events?{q}'), self.auth)
        if st != 200:
            raise OSError(f'Intervals.icu events: HTTP {st} {body[:200]!r}')
        return [e for e in json.loads(body) if e.get('category') == 'WORKOUT']

    def workout_fit(self, event_id):
        st, body = request('GET', self.url(f'events/{event_id}/download.fit'), self.auth)
        if st != 200:
            raise OSError(f'Intervals.icu workout {event_id}: HTTP {st}')
        return body


class Strava:
    def __init__(self, cfg):
        self.cfg = cfg

    def token(self):
        t = load('strava.json', None)
        if not t:
            raise OSError('Strava: no token, run sync.py strava-login first')
        if t['expires_at'] < time.time() + 120:
            t = self.exchange({'grant_type': 'refresh_token', 'refresh_token': t['refresh_token']})
        return t['access_token']

    def exchange(self, form):
        form = dict(form, client_id=self.cfg['client_id'], client_secret=self.cfg['client_secret'])
        st, body = request('POST', f'{STRAVA}/oauth/token', form=form)
        if st != 200:
            raise OSError(f'Strava token: HTTP {st} {body[:200]!r}')
        r = json.loads(body)
        t = {k: r[k] for k in ('access_token', 'refresh_token', 'expires_at')}
        save('strava.json', t)
        return t

    def upload(self, name, data, poll=1.0, tries=60):
        """The activity id; a duplicate is done too ('duplicate of N')."""
        auth = {'Authorization': f'Bearer {self.token()}'}
        st, body = request('POST', f'{STRAVA}/api/v3/uploads', auth, form={'data_type': 'fit', 'external_id': name},
                        files={'file': (name, data)})
        if st not in (200, 201):
            raise OSError(f'Strava upload {name}: HTTP {st} {body[:200]!r}')
        up = json.loads(body)
        for _ in range(tries):
            if up.get('activity_id'):
                return str(up['activity_id'])
            if up.get('error'):
                if 'duplicate' in up['error']:
                    return 'duplicate: ' + up['error']
                raise OSError(f'Strava upload {name}: {up["error"]}')
            time.sleep(poll)  # Strava: once a second at most
            st, body = request('GET', f'{STRAVA}/api/v3/uploads/{up["id_str"]}', auth)
            if st != 200:
                raise OSError(f'Strava upload {name}: HTTP {st}')
            up = json.loads(body)
        raise OSError(f'Strava upload {name}: still processing, tried {tries} times (run again later)')

    def login(self, port=REDIRECT_PORT, browser=None):
        """Strava's authorization page, its code caught on a local redirect, exchanged for tokens. Only the answer
        that carries this login's random `state` is taken (RFC 6749 10.12: another page or program on the computer
        cannot slip its own code in); any other request is ignored, for 5 minutes at most."""
        import secrets
        state = secrets.token_urlsafe(16)
        got = {}

        class Catch(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                q = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
                ours = q.get('state') == [state]
                if ours:
                    got.update(q)
                self.send_response(200 if ours else 404)
                self.end_headers()
                self.wfile.write(b'bryton-rustfw: done, this tab can be closed.' if ours else b'')

            def log_message(self, *a):
                pass

        srv = http.server.HTTPServer(('127.0.0.1', port), Catch)
        # 127.0.0.1, not localhost (which may be ::1 first); Strava allows both as callback domains
        redirect = f'http://127.0.0.1:{srv.server_port}/'
        q = urllib.parse.urlencode({'client_id': self.cfg['client_id'], 'redirect_uri': redirect, 'state': state,
                                    'response_type': 'code', 'approval_prompt': 'auto', 'scope': 'activity:write'})
        url = f'{STRAVA}/oauth/authorize?{q}'
        print(f'Open this page and allow access:\n  {url}')
        if browser is None:
            import webbrowser
            browser = webbrowser.open
        browser(url)
        deadline = time.time() + 300
        while not got and time.time() < deadline:
            srv.timeout = max(deadline - time.time(), 0.1)
            srv.handle_request()
        srv.server_close()
        if 'code' not in got:
            raise OSError(f'Strava login: no code ({got.get("error", ["no answer"])[0]})')
        if 'activity:write' not in got.get('scope', [''])[0]:
            raise OSError('Strava login: upload permission (activity:write) not granted')
        self.exchange({'grant_type': 'authorization_code', 'code': got['code'][0]})
        print(f'Strava: token saved in {home() / "strava.json"}')


# ---------------------------------------------------------------------------------------------------------------
# The sync
# ---------------------------------------------------------------------------------------------------------------

def ride_date(name):
    """YYMMDDhhmmss.fit: its date, None for another name."""
    try:
        return datetime.date(2000 + int(name[0:2]), int(name[2:4]), int(name[4:6]))
    except ValueError:
        return None


def sync(src, cfg, state, go, days, today=None, log=print):
    today = today or datetime.date.today()
    since = datetime.date.fromisoformat(cfg['since']) if cfg.get('since') else today - datetime.timedelta(days=14)
    services = {}
    if cfg.get('intervals'):
        services['intervals'] = Intervals(cfg['intervals'])
    if cfg.get('strava'):
        services['strava'] = Strava(cfg['strava'])
    done = state.setdefault('uploaded', {})
    errors = 0
    full = set(src.files(FULL_DIR))
    for name in src.files('Activities'):
        d = ride_date(name)
        if not name.lower().endswith('.fit') or d is None or d < since:
            continue  # tmp.fit (stock's ride in progress), another file, or older than `since`
        todo = [s for s in services if name not in done.setdefault(s, {})]
        if not todo:
            continue
        files = {}
        try:
            files['clean'] = src.read(f'Activities/{name}')
            if 'intervals' in todo and name in full:
                files['full'] = src.read(f'{FULL_DIR}/{name}')
        except OSError as e:
            log(f'{name}: not read ({e})')
            errors += 1
            continue
        if not fit_whole(files['clean']):
            log(f'{name}: not a whole FIT file, skipped')
            errors += 1
            continue
        if 'full' in files and not fit_whole(files['full']):
            log(f'{name}: its full copy is not whole: the clean one goes to Intervals.icu')
            del files['full']
        if 'full' in files and not same_ride(files['clean'], files['full']):
            log(f'{name}: its full copy is of another ride: the clean one goes to Intervals.icu')
            del files['full']
        for s in todo:
            which = 'full' if s == 'intervals' and 'full' in files else 'clean'
            data = files[which]
            if not go:
                log(f'{name}: would upload the {which} copy ({len(data)} B) to {s}')
                continue
            try:
                rid = services[s].upload(name, data)
            except (OSError, ValueError, KeyError) as e:  # ValueError, KeyError: an answer that is not what we know
                log(f'{name}: {e}')
                errors += 1
                continue
            done[s][name] = {'file': key(name, data), 'copy': which, 'id': rid, 'at': time.strftime('%Y-%m-%d %H:%M')}
            save('state.json', state)
            log(f'{name}: {which} copy uploaded to {s} ({rid})')
    if 'intervals' in services and days > 0:
        errors += workouts(src, services['intervals'], state, go, today, days, log)
    return errors


def workouts(src, icu, state, go, today, days, log):
    try:
        events = icu.workouts(today, today + datetime.timedelta(days=days))
    except OSError as e:
        log(str(e))
        return 1
    if not src.writable:
        if events:
            log(f'{len(events)} planned workouts: not written, the dev link is read only (mount the volume)')
        return 0
    written = state.setdefault('workouts', {})
    errors = 0
    for e in events:
        day = str(e.get('start_date_local', ''))[:10].replace('-', '')
        if len(day) != 8 or not day.isdigit():
            continue
        try:
            path = f'Workout/calendar/{day}/icu{int(e["id"])}.fit'
            data = icu.workout_fit(int(e['id']))
        except (OSError, ValueError, KeyError, TypeError) as err:
            log(str(err))
            errors += 1
            continue
        if not fit_whole(data) or len(data) > WORKOUT_MAX:
            log(f'{path}: not a whole FIT file of at most {WORKOUT_MAX} B, skipped')
            errors += 1
            continue
        if written.get(path) == key(path, data) and src.exists(path):
            continue
        if not go:
            log(f'{path}: would write "{e.get("name", "")}" ({len(data)} B)')
            continue
        src.write(path, data)
        written[path] = key(path, data)
        save('state.json', state)
        log(f'{path}: "{e.get("name", "")}" written')
    return errors


def status(cfg, state):
    print(f'config: {home() / "sync.json"}')
    print(f'  intervals.icu: {"athlete " + cfg["intervals"]["athlete_id"] if cfg.get("intervals") else "off"}')
    st = 'token saved' if load('strava.json', None) else 'run strava-login'
    print(f'  strava: {st if cfg.get("strava") else "off"}')
    for s, rides in state.get('uploaded', {}).items():
        print(f'{s}: {len(rides)} rides uploaded' + (f', the last {max(rides)}' if rides else ''))
    print(f'workouts written: {len(state.get("workouts", {}))}')


# ---------------------------------------------------------------------------------------------------------------
# Self-test: a fake Intervals.icu + Strava, a fake volume, synthetic FIT files; no network
# ---------------------------------------------------------------------------------------------------------------

def fake_fit(body=b'', dev=False):
    """A synthetic, whole FIT file: header, `body` as its data, the CRC."""
    h = bytearray([14, 0x20 if dev else 0x10, 0x5c, 0x08]) + len(body).to_bytes(4, 'little') + b'.FIT'
    h += fit_crc(h).to_bytes(2, 'little')
    f = bytes(h) + body
    return f + fit_crc(f).to_bytes(2, 'little')


def selftest():
    import tempfile
    got = []  # (service, name, body)
    seen_hashes = set()

    class Fake(http.server.BaseHTTPRequestHandler):
        def reply(self, st, obj):
            b = obj if isinstance(obj, bytes) else json.dumps(obj).encode()
            self.send_response(st)
            self.send_header('Content-Length', str(len(b)))
            self.end_headers()
            self.wfile.write(b)

        def body(self):
            return self.rfile.read(int(self.headers.get('Content-Length', 0)))

        def do_GET(self):
            u = urllib.parse.urlparse(self.path)
            if u.path == '/api/v1/athlete/i1/events':
                assert self.headers['Authorization'] == 'Basic ' + base64.b64encode(b'API_KEY:k').decode()
                q = urllib.parse.parse_qs(u.query)
                assert q['category'] == ['WORKOUT'] and q['oldest'] == ['2026-10-24'], q
                return self.reply(200, [{'id': 7, 'category': 'WORKOUT', 'name': 'Sweet spot',
                                         'start_date_local': '2026-10-25T00:00:00'},
                                        {'id': 8, 'category': 'NOTE', 'start_date_local': '2026-10-25T00:00:00'}])
            if u.path == '/api/v1/athlete/i1/events/7/download.fit':
                return self.reply(200, fake_fit(b'workout'))
            if u.path == '/api/v3/uploads/5':
                assert self.headers['Authorization'] == 'Bearer at2'
                return self.reply(200, {'id_str': '5', 'activity_id': 99, 'error': None})
            self.reply(404, {})

        def do_POST(self):
            u = urllib.parse.urlparse(self.path)
            body = self.body()
            if u.path == '/oauth/token':
                f = urllib.parse.parse_qs(body.decode())
                if f['grant_type'] == ['authorization_code']:
                    assert f['code'] == ['good'], f
                    return self.reply(200, {'access_token': 'at3', 'refresh_token': 'rt3', 'expires_at': time.time() + 9e3})
                assert f['grant_type'] == ['refresh_token'] and f['refresh_token'] == ['rt1'] and f['client_secret']
                return self.reply(200, {'access_token': 'at2', 'refresh_token': 'rt2', 'expires_at': time.time() + 9e3})
            if u.path == '/api/v1/athlete/i1/activities':
                got.append(('intervals', urllib.parse.parse_qs(u.query)['external_id'][0], body))
                h = zlib.crc32(body.split(b'\r\n\r\n', 1)[1])
                new = h not in seen_hashes
                seen_hashes.add(h)
                return self.reply(201 if new else 200, {'id': 'x', 'activities': [{'id': 'i42'}] if new else []})
            if u.path == '/api/v3/uploads':
                name = 'dup' if b'261020' in body else 'ok'
                got.append(('strava', name, body))
                if name == 'dup':
                    return self.reply(201, {'id_str': '6', 'error': '261020080000.fit duplicate of activity 3',
                                            'activity_id': None})
                return self.reply(201, {'id_str': '5', 'error': None, 'activity_id': None})
            self.reply(404, {})

        def log_message(self, *a):
            pass

    global ICU, STRAVA
    srv = http.server.HTTPServer(('127.0.0.1', 0), Fake)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    ICU = STRAVA = f'http://127.0.0.1:{srv.server_port}'
    with tempfile.TemporaryDirectory() as tmp:
        os.environ['BRYTON_SYNC_HOME'] = str(Path(tmp, 'cfg'))
        os.environ.pop('INTERVALS_API_KEY', None)
        vol = Path(tmp, 'vol')
        fid = lambda t: bytes([0x40, 0, 0, 0, 0, 4]) + bytes(12) + bytes([0, 4, 255, 0, 0, 0]) + t.to_bytes(4, 'little')
        clean, full = fake_fit(fid(1) + b'clean ride'), fake_fit(fid(1) + b'full ride', dev=True)
        other, not_its = fake_fit(fid(2) + b'clean'), fake_fit(fid(3) + b'full copy of another ride', dev=True)
        for p, data in [('Activities/261023100000.fit', clean), (f'{FULL_DIR}/261023100000.fit', full),
                        ('Activities/261022070000.fit', other), (f'{FULL_DIR}/261022070000.fit', not_its),
                        ('Activities/261024090000.fit', fake_fit(b'stock ride')),
                        ('Activities/261020080000.fit', fake_fit(b'on strava already')),
                        ('Activities/261024100000.fit', clean[:-1]),  # cut short: never uploaded
                        ('Activities/tmp.fit', fake_fit(b'stock in progress')),
                        ('Activities/250101080000.fit', fake_fit(b'too old'))]:
            Path(vol, p).parent.mkdir(parents=True, exist_ok=True)
            Path(vol, p).write_bytes(data)
        cfg = {'intervals': {'athlete_id': 'i1', 'api_key': 'k'},
               'strava': {'client_id': '1', 'client_secret': 's'}, 'since': '2026-10-01'}
        save('strava.json', {'access_token': 'at1', 'refresh_token': 'rt1', 'expires_at': 0})
        assert oct(os.stat(home() / 'strava.json').st_mode & 0o777) == '0o600'
        today = datetime.date(2026, 10, 24)
        lines = []
        state = {}
        # dry run: nothing sent, nothing written
        sync(Volume(vol), cfg, state, False, 7, today, lines.append)
        assert got == [] and not Path(vol, 'Workout').exists(), got
        assert any('would upload the full copy' in x for x in lines), lines
        # for real
        lines.clear()
        errors = sync(Volume(vol), cfg, state, True, 7, today, lines.append)
        assert errors == 1, lines  # the file cut short
        icu = {n: b for s, n, b in got if s == 'intervals'}
        assert set(icu) == {'261023100000.fit', '261024090000.fit', '261020080000.fit', '261022070000.fit'}, set(icu)
        assert other in icu['261022070000.fit'] and not_its not in icu['261022070000.fit'], 'a full copy of another ride'
        assert full in icu['261023100000.fit'] and clean not in icu['261023100000.fit'], 'Intervals: the full copy'
        st = [b for s, n, b in got if s == 'strava']
        assert len(st) == 4 and any(clean in b for b in st) and not any(full in b for b in st), 'Strava: clean only'
        assert state['uploaded']['strava']['261020080000.fit']['id'].startswith('duplicate')
        assert load('strava.json', {})['refresh_token'] == 'rt2', 'refreshed and kept'
        w = Path(vol, 'Workout/calendar/20261025/icu7.fit')
        assert w.read_bytes() == fake_fit(b'workout') and len(list(Path(vol, 'Workout/calendar').iterdir())) == 1
        assert load('state.json', {}) == state
        # again: nothing new
        n = len(got)
        sync(Volume(vol), cfg, load('state.json', {}), True, 7, today, lines.append)
        assert len(got) == n, got[n:]
        # a workout deleted on the unit (or a copy the drive dropped) is written again
        w.unlink()
        sync(Volume(vol), cfg, load('state.json', {}), True, 7, today, lines.append)
        assert w.read_bytes() == fake_fit(b'workout') and len(got) == n
        assert not list(Path(vol, 'Workout/calendar/20261025').glob('*.tmp'))
        # a FIT CRC that is wrong anywhere is refused
        bad = bytearray(clean)
        bad[16] ^= 1
        assert fit_whole(clean) and fit_whole(full) and not fit_whole(bytes(bad)) and not fit_whole(clean[:-1])
        assert ride_date('261023100000.fit') == datetime.date(2026, 10, 23) and ride_date('tmp.fit') is None
        # Strava's login: a stray request and a forged answer (another state) are ignored, the real one taken
        def browser(url):
            state = urllib.parse.parse_qs(urllib.parse.urlparse(url).query)['state'][0]
            redirect = urllib.parse.parse_qs(urllib.parse.urlparse(url).query)['redirect_uri'][0]

            def answer():
                for q in ('', '?code=forged&scope=read,activity:write&state=nope',
                          f'?code=good&scope=read,activity:write&state={state}'):
                    for _ in range(50):
                        try:
                            urllib.request.urlopen(redirect + q, timeout=5).read()
                            break
                        except urllib.error.HTTPError:
                            break
                        except OSError:
                            time.sleep(0.05)  # the server not up yet
            threading.Thread(target=answer, daemon=True).start()
        Strava(cfg['strava']).login(port=0, browser=browser)
        assert load('strava.json', {})['access_token'] == 'at3'
        assert oct(os.stat(home() / 'strava.json').st_mode & 0o777) == '0o600'
    srv.shutdown()
    print('sync.py selftest ok')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('cmd', nargs='?', default='sync', choices=['sync', 'strava-login', 'status', 'selftest'])
    ap.add_argument('--go', action='store_true', help='upload and write (without: say what would be done)')
    ap.add_argument('--volume', help='the mounted BRYTON volume (default: config "volume", /Volumes/BRYTON)')
    ap.add_argument('--link', action='store_true', help='read the rides through the dev link (read only)')
    ap.add_argument('--days', type=int, help='days of planned workouts to fetch (default: config "days", 7)')
    a = ap.parse_args()
    if a.cmd == 'selftest':
        return selftest()
    cfg = load('sync.json', None)
    if cfg is None:
        sys.exit(f'sync.py: no {home() / "sync.json"} (docs/sync.md)')
    if os.stat(home() / 'sync.json').st_mode & 0o077:
        print(f'sync.py: {home() / "sync.json"} holds keys and others can read it: chmod 600 it', file=sys.stderr)
    if a.cmd == 'strava-login':
        return Strava(cfg['strava']).login()
    state = load('state.json', {})
    if a.cmd == 'status':
        return status(cfg, state)
    src = Link() if a.link else (Volume(a.volume) if a.volume else find_volume(cfg))
    if src is None:
        sys.exit('sync.py: no BRYTON volume mounted (--volume PATH, or --link for the dev link)')
    print(f'rides from {src}{"" if a.go else " (dry run: --go to upload and write)"}')
    days = a.days if a.days is not None else int(cfg.get('days', 7))
    sys.exit(1 if sync(src, cfg, state, a.go, days) else 0)


if __name__ == '__main__':
    main()
