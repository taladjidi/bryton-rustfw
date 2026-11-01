#!/usr/bin/env python3
"""Download the stock Rider S510 firmware from Bryton's own update server and check it is R039.

  tools/fetch_firmware.py [--dest DIR]        default DIR = firmware/R039
  tools/fetch_firmware.py --from-readback FILE
      no download: use your own unit's firmware, read back in Engineer Mode with tools/iaprecover.py (for when
      Bryton no longer serves R039). Only IMG.raw is produced; that is all build.py needs.
  tools/fetch_firmware.py selftest            a fake download, no network (check.sh)

Gets img.bin (what the Bryton Active app downloads), writes IMG.raw (= img.bin decompressed, what the patches
apply to) and checks both against firmware/SHA256SUMS. If Bryton has published a newer release, the hashes
will not match: the patches here are for R039 only and must not be applied to anything else. Nothing is written
to DIR until both hashes match (iaprecover.py --write-vanilla trusts what is there), and then by rename.
"""
import hashlib, os, sys, urllib.request, zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))  # progress.py, also under python3 -I
from progress import tqdm

URL = 'https://corp.brytonsport.com/device/riders510/'
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')


def get(name):
    with urllib.request.urlopen(URL + name, timeout=60) as r:
        total = int(r.headers.get('Content-Length') or 0)
        out = bytearray()
        with tqdm(total=total, unit='B', unit_scale=True, desc=name) as bar:
            while chunk := r.read(65536):
                out += chunk
                bar.update(len(chunk))
    return bytes(out)


def save(dest, name, data):
    """Write dest/name whole or not at all: a temporary file, then a rename."""
    os.makedirs(dest, exist_ok=True)
    tmp = os.path.join(dest, name + '.part')
    with open(tmp, 'wb') as f:
        f.write(data)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, os.path.join(dest, name))


def main(args, fetch=get):
    dest = args[args.index('--dest') + 1] if '--dest' in args else os.path.join(ROOT, 'firmware', 'R039')
    want = dict(reversed(line.split()) for line in open(os.path.join(ROOT, 'firmware', 'SHA256SUMS')) if line.strip())
    if '--from-readback' in args:
        raw = open(args[args.index('--from-readback') + 1], 'rb').read()[:4026368]   # readback may be padded
        if hashlib.sha256(raw).hexdigest() != want['IMG.raw']:
            sys.exit('This readback is not stock R039; cannot be used as the base for the patches.')
        save(dest, 'IMG.raw', raw)
        return print(f'IMG.raw  OK (stock R039, from your unit) -> {dest}')
    print(fetch('release.ini').decode(errors='replace').split('[BootLoader]')[0].strip())
    img = fetch('img.bin')
    raw = zlib.decompressobj(16 + 15).decompress(img)       # img.bin = gzip + zero padding + 4-byte sum
    files = (('img.bin', img), ('IMG.raw', raw))
    ok = True
    for name, data in files:
        good = hashlib.sha256(data).hexdigest() == want[name]
        ok &= good
        print(f'{name:8} {len(data):>8} B  {"OK (stock R039)" if good else "DOES NOT MATCH R039"}')
    if not ok:
        sys.exit('Not the R039 these patches were made for; do not use them on this firmware. Nothing saved.')
    for name, data in files:
        save(dest, name, data)
    print(f'saved to {dest}')


def selftest():
    """A fake download against a fake SHA256SUMS: a wrong one leaves DIR empty, a right one lands whole."""
    import gzip, tempfile
    global ROOT
    raw = b'stock image'
    img = gzip.compress(raw) + bytes(8)
    served = {'release.ini': b'[Firmware]\nR039\n[BootLoader]\n', 'img.bin': img}
    old = ROOT
    try:
        with tempfile.TemporaryDirectory() as d:
            ROOT = d
            os.makedirs(os.path.join(d, 'firmware'))
            dest = os.path.join(d, 'out')
            sums = f'{hashlib.sha256(img).hexdigest()}  img.bin\n\n{hashlib.sha256(raw + b"!").hexdigest()}  IMG.raw\n'
            open(os.path.join(d, 'firmware', 'SHA256SUMS'), 'w').write(sums)
            try:
                main(['--dest', dest], served.__getitem__)
                raise AssertionError('a wrong IMG.raw was accepted')
            except SystemExit:
                pass
            assert not os.path.exists(dest), os.listdir(dest)
            sums = f'{hashlib.sha256(img).hexdigest()}  img.bin\n\n{hashlib.sha256(raw).hexdigest()}  IMG.raw\n'
            open(os.path.join(d, 'firmware', 'SHA256SUMS'), 'w').write(sums)
            main(['--dest', dest], served.__getitem__)
            assert sorted(os.listdir(dest)) == ['IMG.raw', 'img.bin']
            assert open(os.path.join(dest, 'IMG.raw'), 'rb').read() == raw
    finally:
        ROOT = old
    print('fetch_firmware selftest OK')


if __name__ == '__main__':
    selftest() if sys.argv[1:] == ['selftest'] else main(sys.argv[1:])
