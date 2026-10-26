#!/usr/bin/env python3
"""What an S810 owner can send to help port this firmware (docs/s810.md, phase 0), read only, no personal data.

    tools/s810_probe.py drive [/Volumes/BRYTON] [-o s810_probe.zip]   # the stock USB drive, as mounted
    tools/s810_probe.py selftest                                       # offline check on a made-up volume

Part 1 of the probe (docs/s810.md, "The diagnostics tool"): it only opens files for reading and never writes to
the volume. It collects:
- `\\System\\device.txt` with the UUID line removed (firmware, bootloader, Nordic, language versions, model);
- the names and sizes of the files directly in `\\System` (stock's fixed names) and, for its `.ini` files, the
  section and key names only (every value replaced by `<v>`);
- per top-level folder: the number of files, their total size and the extensions, never a file name (ride files
  are named by date and time);
- from each `\\Debug\\*.log`, only the lines that match a list of hardware patterns (panel type, touch resolution,
  barometer model, versions, charger registers), after dropping any line that could hold a position, a name or an
  address;
- the volume's size and free space.
Everything goes into one zip with `manifest.json` (what was taken, the rules, a SHA-256 per member). Open the zip
and read it before sending it. Part 2 (the bootloader's read commands) is designed in docs/s810.md, not built.
"""

import hashlib
import io
import json
import os
import re
import sys
import zipfile

VERSION = 1
# Lines of stock's debug log worth having (the S510's own log shows these at boot: BOARD.md, docs/s810.md).
KEEP = re.compile(r'LCD|touch ic|Barometer|model|Light sensor|Nordic|IAP|[Vv]ersion|SC89620|MMC|\bALS\b|Battery|'
                  r'u-?blox|MON|PROTVER|HW|SDRAM|flash|reset|RESET|fault|Fault')
# Dropped even when kept above: anything that could locate or name someone.
DROP = re.compile(r'lat|lon|pos|name|mail|@|phone|addr|http|\d+\.\d{3,}|([0-9A-Fa-f]{2}:){3,}|UUID|serial|ID=',
                  re.IGNORECASE)


def ini_keys(data):
    """An INI file with every value replaced by <v>: its shape without its content."""
    out = []
    for line in data.decode('latin-1').splitlines():
        s = line.strip()
        if s.startswith('[') or '=' not in s:
            out.append(s if s.startswith('[') else '')
        else:
            out.append(s.split('=', 1)[0].strip() + '=<v>')
    return '\n'.join(x for x in out if x) + '\n'


def debug_lines(data):
    """The hardware lines of one debug log, de-duplicated, in order."""
    seen, out = set(), []
    for line in data.decode('latin-1').splitlines():
        s = line.strip()
        if s and KEEP.search(s) and not DROP.search(s) and s not in seen:
            seen.add(s)
            out.append(s)
    return out


def find(root, name):
    """A top-level entry of the volume by name, any case (FAT is case-insensitive), or None."""
    for e in os.listdir(root):
        if e.lower() == name.lower():
            return os.path.join(root, e)
    return None


def read(path):
    with open(path, 'rb') as f:  # read only, always
        return f.read()


def collect(root):
    """{member name: bytes} for the zip, manifest included."""
    m = {}
    system = find(root, 'System')
    if system:
        dev = os.path.join(system, 'device.txt')
        if os.path.isfile(dev):
            m['device.txt'] = b''.join(l for l in read(dev).splitlines(True) if b'UUID=' not in l)
        listing = []
        for e in sorted(os.listdir(system)):
            p = os.path.join(system, e)
            listing.append(f'{e}\t{"dir" if os.path.isdir(p) else os.path.getsize(p)}')
            if e.lower().endswith('.ini') and os.path.isfile(p):
                m[f'System_ini/{e}'] = ini_keys(read(p)).encode()
        m['system_listing.tsv'] = ('\n'.join(listing) + '\n').encode()
    tree = {}
    for top in sorted(os.listdir(root)):
        p = os.path.join(root, top)
        if not os.path.isdir(p) or top.startswith('.'):
            continue
        n = size = 0
        ext = {}
        for d, _, files in os.walk(p):
            for f in files:
                n += 1
                size += os.path.getsize(os.path.join(d, f))
                x = os.path.splitext(f)[1].lower()
                ext[x] = ext.get(x, 0) + 1
        tree[top] = {'files': n, 'bytes': size, 'extensions': ext}
    m['tree.json'] = json.dumps(tree, indent=1, sort_keys=True).encode()
    debug = find(root, 'Debug')
    if debug:
        logs = sorted(f for f in os.listdir(debug) if f.lower().endswith('.log'))
        for i, f in enumerate(logs):  # numbered: the file name is a date and time
            m[f'debug/{i:03}.txt'] = ('\n'.join(debug_lines(read(os.path.join(debug, f)))) + '\n').encode()
    st = os.statvfs(root)
    m['volume.json'] = json.dumps({'bytes': st.f_blocks * st.f_frsize, 'free': st.f_bavail * st.f_frsize}).encode()
    m['manifest.json'] = json.dumps({
        'tool': 's810_probe.py', 'version': VERSION,
        'collected': 'device.txt without UUID; System file names, sizes, INI keys without values; per-folder counts, '
                     'sizes, extensions; hardware lines of Debug logs; volume size',
        'never': 'rides, positions, names, file names outside System, INI values, the UUID',
        'keep_pattern': KEEP.pattern, 'drop_pattern': DROP.pattern,
        'sha256': {k: hashlib.sha256(v).hexdigest() for k, v in sorted(m.items())},
    }, indent=1).encode()
    return m


def write_zip(members, out):
    with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as z:
        for k, v in sorted(members.items()):
            z.writestr(k, v)


def selftest():
    import tempfile
    with tempfile.TemporaryDirectory() as t:
        os.makedirs(os.path.join(t, 'System'))
        os.makedirs(os.path.join(t, 'Debug'))
        os.makedirs(os.path.join(t, 'Activities'))
        open(os.path.join(t, 'System', 'device.txt'), 'w').write('[OS]\nVersion=R049\n[UUID]\nUUID=1234ABCD\n'
                                                                  '[MODEL]\nModel=RiderS810\n')
        open(os.path.join(t, 'System', 'UserProfile.ini'), 'w').write('[User]\nName=Alice\nWeight=61\n')
        open(os.path.join(t, 'Activities', '261021090000.fit'), 'wb').write(b'\0' * 100)
        open(os.path.join(t, 'Debug', '20261021090000.log'), 'w').write(
            'LCD Type VA, 0x02, 0x00, 0x00\n[Barometer] model lps22hh\ntouch ic resolution ( 282, 470 )\n'
            '[GPS] lat 45.123456 lon 5.654321\nBattery 80 user name Alice\nIAP version (R002)\nnoise\n'
            '[Barometer] model lps22hh\n')
        before = {p: os.stat(os.path.join(d, p)).st_mtime_ns for d, _, fs in os.walk(t) for p in fs}
        m = collect(t)
        out = io.BytesIO()
        write_zip(m, out)
        blob = b''.join(m.values())
        for secret in (b'1234ABCD', b'Alice', b'45.123', b'261021090000', b'20261021090000', b'Weight=61'):
            assert secret not in blob, secret
        log = m['debug/000.txt'].decode().splitlines()
        assert log == ['LCD Type VA, 0x02, 0x00, 0x00', '[Barometer] model lps22hh',
                       'touch ic resolution ( 282, 470 )', 'IAP version (R002)'], log
        assert b'Model=RiderS810' in m['device.txt'] and b'Weight=<v>' in m['System_ini/UserProfile.ini']
        assert json.loads(m['tree.json'])['Activities'] == {'files': 1, 'bytes': 100, 'extensions': {'.fit': 1}}
        man = json.loads(m['manifest.json'])
        assert all(hashlib.sha256(m[k]).hexdigest() == h for k, h in man['sha256'].items())
        assert zipfile.ZipFile(out).namelist() == sorted(m)
        after = {p: os.stat(os.path.join(d, p)).st_mtime_ns for d, _, fs in os.walk(t) for p in fs}
        assert before == after, 'the volume changed'
    print('s810_probe.py selftest ok')


def main():
    a = sys.argv[1:]
    if a[:1] == ['selftest']:
        return selftest()
    if a[:1] != ['drive']:
        sys.exit(__doc__)
    out = 's810_probe.zip'
    if '-o' in a:
        i = a.index('-o')
        out = a[i + 1]
        del a[i:i + 2]
    root = a[1] if len(a) > 1 else '/Volumes/BRYTON'
    if not os.path.isdir(root):
        sys.exit(f'{root}: not mounted (plug the bike computer in with its stock firmware)')
    m = collect(root)
    write_zip(m, out)
    print(f'{out}: {len(m)} files. Open it and read them before sending; nothing else was taken.')


if __name__ == '__main__':
    main()
