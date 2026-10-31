#!/usr/bin/env python3
"""The scripted unit walk: replay the key paths of the simulator's screen walk (src/sim/walk_tests.rs writes
target/sim/walk/paths.json and NNN-<name>.bmp) on the unit through the dev link, shoot each screen, and set it beside
the simulator's own for a human to compare in a few contact sheets. The standard unit check after a batch.

  tools/walk.py [--build out/IMG.bin] [--only REGEX] [-o DIR]
  tools/walk.py selftest

--build first runs the image from RAM (dev.py run: nothing is written to the eMMC). Then `info`, every path, `info`
again (uptime must have grown, else the unit reset), `peek 0x580040c4` (the fault marker, must be 0) and a 20 s `prof`
window on the overview. Exit 1 on a reset or a fault. Needs Pillow (plain python3, not -I).

Output goes under DIR only (default target/walk/, gitignored): NNN-<name>.png (unit | sim side by side) and
sheet-N.png (six pairs a sheet, labelled). The shots can show the owner's position or map: never copy them into the
repository.

Safety, enforced here (refuse): a path is walked only if its keys are page / lap / start / back / "page --long" and
no word of its name is on DENY (anything that saves, formats, deletes, forgets, updates or writes the card). The
paths hold no setting change (the simulator's walk takes no shot while it cycles a setting); imperial paths (IMP) and
date-format overviews are skipped since showing them needs a setting change. A path that starts a ride (the ride
pages and ride menu) runs on a ride that is ended with DISCARD at the end of its group; a build without EMMC_WRITE
records nothing anyway, but the guard stays.
"""
import argparse, json, re, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools'))
import dev  # noqa: E402

SIM = ROOT / 'target' / 'sim' / 'walk'
KEYS = ('page', 'lap', 'start', 'back', 'page --long')
# whole words of a screen name that act on the card or the unit: never walked
DENY = ('SAVE', 'SCREENSHOT', 'FORMAT', 'RESET', 'DELETE', 'FORGET', 'UPDATE', 'FIRMWARE', 'IMPORT', 'ERASE')
SETTLE = 0.3
FAULT = '0x580040c4'
PER_SHEET, COLS = 6, 3


def refuse(entry):
    """Why a path must not run on the unit, or None."""
    name = entry['name']
    words = re.split(r'[^A-Z0-9]+', name.upper())
    for d in DENY:
        if d in words:
            return f'name has {d}'
    bad = [k for k in entry['keys'] if k not in KEYS]
    if bad:
        return f'unknown key {bad[0]!r}'
    if name.startswith('IMP '):
        return 'imperial needs a setting change'
    if re.search(r'OVERVIEW \w+$', name):
        return 'date format needs a setting change'
    return None


def is_ride(entry):
    return 'RIDE' in entry['name'] and entry['keys'][:1] == ['start']


def press(s, k):
    answer = dev.command(s, 'key ' + k.replace(' --long', ' long'))  # the wire form (src/link/dev.rs)
    if answer != 'ok':
        sys.exit(f'walk.py: key {k}: {answer}')
    time.sleep(SETTLE)


def home(s):
    for _ in range(8):
        press(s, 'back')


def discard(s):
    """Ends the ride: pause, END RIDE, DISCARD, KEEP, DISCARD (as tools/profile.py)."""
    for k in ('start', 'start --long', 'page', 'page', 'start', 'lap', 'start'):
        press(s, k)
    time.sleep(1.0)


def sim_bmp(n):
    got = sorted(SIM.glob(f'{n:03}-*.bmp'))
    return got[0] if got else None


def pair(unit_png, sim_path, label):
    """Unit | sim side by side, the label on top."""
    import io
    from PIL import Image, ImageDraw
    u = Image.open(io.BytesIO(unit_png) if isinstance(unit_png, bytes) else unit_png).convert('RGB')
    sim = Image.open(sim_path).convert('RGB') if sim_path else Image.new('RGB', u.size, (60, 0, 0))
    out = Image.new('RGB', (u.width + sim.width + 6, u.height + 14), (30, 30, 30))
    out.paste(u, (0, 14))
    out.paste(sim, (u.width + 6, 14))
    ImageDraw.Draw(out).text((2, 1), label[:110], fill=(255, 255, 0))
    return out


def sheets(pairs, outdir):
    """Contact sheets of PER_SHEET pairs; returns their paths."""
    from PIL import Image
    paths = []
    for i in range(0, len(pairs), PER_SHEET):
        chunk = pairs[i:i + PER_SHEET]
        w, h = chunk[0].size
        rows = -(-len(chunk) // COLS)
        sheet = Image.new('RGB', (min(COLS, len(chunk)) * (w + 4), rows * (h + 4)), (0, 0, 0))
        for j, p in enumerate(chunk):
            sheet.paste(p, (j % COLS * (w + 4), j // COLS * (h + 4)))
        paths.append(outdir / f'sheet-{i // PER_SHEET + 1}.png')
        sheet.save(paths[-1])
    return paths


def uptime_ms(info):
    m = re.search(r' up (\d+) ms', info)
    return int(m.group(1)) if m else -1


def fresh_paths():
    src, test = SIM / 'paths.json', ROOT / 'src' / 'sim' / 'walk_tests.rs'
    if not src.exists() or src.stat().st_mtime < test.stat().st_mtime:
        subprocess.run(['cargo', 'test-host', 'walk'], cwd=ROOT, check=True)
    return json.loads(src.read_text())


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--build', help='run this image from RAM first')
    ap.add_argument('--only', help='regex on the path names')
    ap.add_argument('-o', default=str(ROOT / 'target' / 'walk'))
    a = ap.parse_args()
    out = Path(a.o)
    out.mkdir(parents=True, exist_ok=True)
    entries = fresh_paths()
    t0 = time.monotonic()
    if a.build:
        dev.run(a.build)
        time.sleep(3.0)
    walked, skipped, pairs = 0, [], []
    with dev.connect() as s:
        before = dev.command(s, 'info')
        print(before)
        riding = False
        for e in entries:
            if a.only and not re.search(a.only, e['name']):
                continue
            why = refuse(e)
            if why:
                skipped.append((e['name'], why))
                continue
            if riding:  # the keys of a ride path start from the overview: end the ride before the next path
                discard(s)
                riding = False
            home(s)
            riding = is_ride(e)
            for k in e['keys']:
                press(s, k)
            time.sleep(0.5)
            png = dev.shot(s)
            fname = out / ('%03d-%s.png' % (e['n'], re.sub(r'[^a-z0-9]+', '_', e['name'].lower())))
            p = pair(png, sim_bmp(e['n']), f'{e["n"]:03} {e["name"]}')
            p.save(fname)
            pairs.append(p)
            walked += 1
        if riding:
            discard(s)
        home(s)
        after = dev.command(s, 'info')
        print(after)
        fault = dev.command(s, f'peek {FAULT} 1')
        dev.command(s, 'prof reset')
        time.sleep(20)
        prof = dev.prof_read(s)
    paths = sheets(pairs, out) if pairs else []
    wall = prof['wall_ms'] / 1000 or 1
    st = prof['sections']
    awake = sum(t for n, c, t, m in st if n == 'step' or n in dev.OUTSIDE)
    awake_pct = 100 * awake / dev.CPU_HZ / wall
    reset = not 0 <= uptime_ms(before) < uptime_ms(after)
    faulted = int(fault.split()[-1], 16) != 0
    print(f'walked {walked}, skipped {len(skipped)}')
    for n, w in skipped:
        print(f'  skipped {n}: {w}')
    print(f'reset: {"YES" if reset else "no"}; fault marker: {fault} ({"FAULT" if faulted else "clear"}); '
          f'awake {awake_pct:.2f} % idle on the overview; {time.monotonic() - t0:.0f} s')
    for p in paths:
        print(p)
    sys.exit(1 if reset or faulted else 0)


def selftest():
    import tempfile
    from PIL import Image
    ok = lambda name, keys=('start',): {'n': 1, 'name': name, 'keys': list(keys)}
    assert refuse(ok('MET MAIN/HISTORY')) is None
    assert refuse(ok('MET MAIN/SETTINGS/SYSTEM/FORMAT')) == 'name has FORMAT'
    assert refuse(ok('MET MAIN/PLACES/SAVE PLACE')) == 'name has SAVE'
    assert refuse(ok('MET MAIN/RESET')) and refuse(ok('MET RIDE MENU/SAVE'))
    assert refuse(ok('MET X', ('start', 'long_page'))) == "unknown key 'long_page'"
    assert refuse(ok('MET X', ('page --long', 'back'))) is None
    assert refuse(ok('IMP MAIN')) and refuse(ok('MET OVERVIEW Dmy'))
    assert refuse(ok('MET OVERVIEW')) is None
    assert is_ride(ok('MET RIDE PAGE 1')) and not is_ride(ok('MET MAIN'))
    assert uptime_ms('bryton-rustfw 1 x up 1234 ms, awake 3') == 1234
    d = Path(tempfile.mkdtemp())
    Image.new('RGB', (240, 320), (0, 255, 0)).save(d / 'u.png')
    Image.new('RGB', (240, 320), (0, 0, 255)).save(d / 's.bmp')
    ps = [pair(d / 'u.png', d / 's.bmp', f'{i:03} a') for i in range(7)]
    assert ps[0].size == (486, 334)
    sh = sheets(ps, d)
    assert [p.name for p in sh] == ['sheet-1.png', 'sheet-2.png']
    assert Image.open(sh[0]).size == (3 * 490, 2 * 338) and Image.open(sh[1]).size == (490, 338)
    print('walk selftest ok')


if __name__ == '__main__':
    selftest() if sys.argv[1:] == ['selftest'] else main()
