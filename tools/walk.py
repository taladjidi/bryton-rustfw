#!/usr/bin/env python3
"""The scripted unit walk: replay the key paths of the simulator's screen walk (src/sim/walk_tests.rs writes
target/sim/walk/paths.json and NNN-<name>.bmp) on the unit through the dev link, shoot each screen, and set it beside
the simulator's own for a human to compare in a few contact sheets. The standard unit check after a batch.

  tools/walk.py [--build out/IMG.bin] [--only REGEX] [--rides] [-o DIR]
  tools/walk.py selftest

--build first runs the image from RAM (dev.py run: the image's own writes still happen if it is an EMMC_WRITE build, so
walk only a build WITHOUT EMMC_WRITE: what check.sh builds). Build it from the main checkout: a worktree lacks the
gitignored ant_key.bin / di2_key.bin and the sensor screens then show NO KEY. Then `info` (must say `ride idle`), every
path, `info` again (idle again; uptime must have grown after every path, else the unit reset), `peek 0x580040c4` (the
fault marker, must be 0, also checked every 10 paths) and a 20 s `prof` window on the overview. Exit 1 on a reset, a
fault, a ride left running or an unexpected answer. Needs Pillow (plain python3, not -I).

Output goes under DIR only (default target/walk/, gitignored): NNN-<name>.png (unit | sim side by side) and
sheet-N.png (six pairs a sheet, labelled). The shots can show the owner's position or map: never copy them into the
repository.

Safety, enforced here (refuse). Replay is positional and the unit's data-dependent menus (sensors, places, bikes,
history, the ride menu...) have other rows than the simulator's, so a Start is only ever pressed inside a static menu
(MAIN, SETTINGS, ME, ZONES, DISPLAY, BIKE, ALERTS, PAGES, DI2 KEYS: the const tables, src/sim/walk_tests.rs
`static_menu`). A path is walked only if:
- its keys are page / lap / start / back / "page --long" and no word of its name is on DENY;
- the simulator did not mark it "dyn" (a Start pressed inside a data-dependent menu) and no ancestor of its name
  (`a/b/c` -> a, b) is a non-static menu: entering a dynamic menu from a static parent is fine, going into it is not;
- it is not a ride path (RIDE PAGE n, RIDE MENU: Start on the overview), unless --rides (only with a build without
  EMMC_WRITE). After a ride path the ride is discarded (home, the ride page, END, DISCARD, >= 0.5 s between keys) and
  `info` must say `ride idle`, else the walk stops;
- after each Start that opens a menu with more Starts to come, the title band of the unit's shot must match the
  simulator's (thresholded mask): on a mismatch the path is aborted and the unit sent home (drift check).
The paths hold no setting change (the simulator's walk takes no shot while it cycles a setting); imperial paths (IMP) and
date-format overviews are skipped since showing them needs a setting change. -o must be under target/ or outside
the repository.
"""
import argparse, json, re, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools'))
import dev  # noqa: E402

SIM = ROOT / 'target' / 'sim' / 'walk'
KEYS = ('page', 'lap', 'start', 'back', 'page --long')
# whole words of a screen name that act on the card or the unit: never walked
DENY = ('SAVE', 'SCREENSHOT', 'FORMAT', 'RESET', 'DELETE', 'FORGET', 'UPDATE', 'FIRMWARE', 'IMPORT', 'ERASE',
        'DRIVE', 'GIVE', 'APPLY', 'PAIR', 'TRAINER', 'COURSE', 'RAMP', 'START', 'ADD PAGE', 'ACTIVE BIKE', 'SERVICE',
        'INDOOR', 'LIGHT', 'CAL', 'MOVE', 'SHOW', 'RENAME', 'END', 'DISCARD', 'FROM GPS')
# the menus a Start may be pressed in (titles, as menu_title names them; the root "MET MAIN" reads as MAIN)
STATIC = ('MAIN', 'SETTINGS', 'ME', 'ZONES', 'DISPLAY', 'BIKE', 'ALERTS', 'PAGES', 'DI2 KEYS')
SETTLE = 0.5
BAND = (20, 24, 220, 50)       # the title band under the status strip (x0, y0, x1, y1), static text
DRIFT = 0.25                   # fraction of the lit pixels that may differ (antialias, colours)
FAULT = '0x580040c4'
PER_SHEET, COLS = 6, 3


def refuse(entry):
    """Why a path must not run on the unit, or None."""
    name = entry['name']
    up = name.upper()
    words = ' ' + ' '.join(w for w in re.split(r'[^A-Z0-9]+', up) if w) + ' '
    for d in DENY:
        if f' {d} ' in words:
            return f'name has {d}'
    if entry.get('dyn', True):
        return 'a Start inside a data-dependent menu'
    for a in name.split('/')[:-1]:
        if re.sub(r'^(MET|IMP) ', '', a) not in STATIC:
            return f'ancestor {a} is not a static menu'
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


def ride_state(info):
    m = re.search(r', ride (\w+)', info)
    return m.group(1) if m else '?'


def mask(img):
    """The title band as lit/unlit pixels: lit = far from the band's commonest colour (so a theme change or antialias
    does not matter)."""
    from collections import Counter
    band = img.convert('RGB').crop(BAND)
    px = list(band.getdata())
    bg = Counter(px).most_common(1)[0][0]
    return [sum(abs(a - b) for a, b in zip(p, bg)) > 200 for p in px]


def same_title(a, b):
    """Do two screens show the same title text (thresholded masks, DRIFT tolerance)."""
    ma, mb = mask(a), mask(b)
    union = sum(x or y for x, y in zip(ma, mb))
    diff = sum(x != y for x, y in zip(ma, mb))
    return union == 0 or diff / union <= DRIFT


def png_image(png):
    import io
    from PIL import Image
    return Image.open(io.BytesIO(png)) if isinstance(png, bytes) else Image.open(png)


def press(s, k):
    answer = dev.command(s, 'key ' + k.replace(' --long', ' long'))  # the wire form (src/link/dev.rs)
    if answer != 'ok':
        sys.exit(f'walk.py: key {k}: {answer}')
    time.sleep(SETTLE)


def home(s):
    for _ in range(8):
        press(s, 'back')


def discard(s):
    """Ends the ride: home, one press to the ride page, then pause, END RIDE, DISCARD, KEEP, DISCARD (as
    tools/profile.py), SETTLE between keys. True when `info` then says the ride is idle."""
    home(s)
    press(s, 'page')
    for k in ('start', 'start --long', 'page', 'page', 'start', 'lap', 'start'):
        press(s, k)
    time.sleep(1.5)
    home(s)
    return ride_state(dev.command(s, 'info')) == 'idle'


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


def fault_marker(s):
    """The fault marker word, or None when the unit did not answer a value."""
    try:
        return int(dev.command(s, f'peek {FAULT} 1').split()[-1], 16)
    except (ValueError, IndexError):
        return None


def drift_checks(e):
    """(key index, prefix path name) after which the unit must show that menu's title: every Start or long Page that
    opens a menu with more Starts to come (the last Start is only ever pressed in a menu already checked)."""
    keys, parts = e['keys'], e['name'].split('/')
    out, lvl = [], 0
    for i, k in enumerate(keys):
        if k == 'page --long' and re.match(r'(MET|IMP) MAIN$', parts[0]) and lvl == 0:
            lvl = 1
        elif k == 'start':
            lvl += 1
        else:
            continue
        if 'start' in keys[i + 1:] and 0 < lvl <= len(parts):
            out.append((i, '/'.join(parts[:lvl])))
    return out


def check_out(path):
    """-o inside the repository is allowed only under target/."""
    p = Path(path).resolve()
    if ROOT in p.parents or p == ROOT:
        if (ROOT / 'target') not in p.parents and p != ROOT / 'target':
            sys.exit(f'walk.py: -o {path} is inside the repository but not under target/')
    return p


def fresh_paths():
    src, test = SIM / 'paths.json', ROOT / 'src' / 'sim' / 'walk_tests.rs'
    if not src.exists() or src.stat().st_mtime < test.stat().st_mtime:
        subprocess.run(['cargo', 'test-host', 'walk'], cwd=ROOT, check=True)
    return json.loads(src.read_text())


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--build', help='run this image from RAM first')
    ap.add_argument('--only', help='regex on the path names')
    ap.add_argument('--rides', action='store_true', help='also walk the ride paths (a build without EMMC_WRITE only)')
    ap.add_argument('-o', default=str(ROOT / 'target' / 'walk'))
    a = ap.parse_args()
    out = check_out(a.o)
    out.mkdir(parents=True, exist_ok=True)
    entries = fresh_paths()
    t0 = time.monotonic()
    if a.build:
        dev.run(a.build)
        time.sleep(3.0)
    walked, skipped, drifts, pairs = 0, [], [], []
    by_name = {}
    for e in entries:
        by_name.setdefault(e['name'], e)
    problem = None
    with dev.connect() as s:
        before = dev.command(s, 'info')
        print(before)
        if ride_state(before) != 'idle':
            sys.exit(f'walk.py: the unit is not idle at the start: {before}')
        last_up, nth = uptime_ms(before), 0
        for e in entries:
            if a.only and not re.search(a.only, e['name']):
                continue
            why = refuse(e)
            ride = is_ride(e)
            if not why and ride and not a.rides:
                why = 'ride path (needs --rides)'
            if why:
                skipped.append((e['name'], why))
                continue
            home(s)
            aborted = None
            checks = dict(drift_checks(e))
            for i, k in enumerate(e['keys']):
                press(s, k)
                if i in checks:
                    sim = sim_bmp(by_name[checks[i]]['n']) if checks[i] in by_name else None
                    if sim is None or not same_title(png_image(dev.shot(s)), sim):
                        aborted = f'after key {i} ({k}) the screen is not {checks[i]}'
                        break
            if aborted:
                drifts.append((e['name'], aborted))
                home(s)
            else:
                time.sleep(0.5)
                png = dev.shot(s)
                fname = out / ('%03d-%s.png' % (e['n'], re.sub(r'[^a-z0-9]+', '_', e['name'].lower())))
                p = pair(png, sim_bmp(e['n']), f'{e["n"]:03} {e["name"]}')
                p.save(fname)
                pairs.append(p)
                walked += 1
            if ride and not discard(s):
                problem = f'ride not idle after {e["name"]}: stopped'
            info = dev.command(s, 'info')
            up = uptime_ms(info)
            if up <= last_up:
                problem = problem or f'uptime went {last_up} -> {up} after {e["name"]}: the unit reset'
            last_up, nth = up, nth + 1
            if nth % 10 == 0:
                m = fault_marker(s)
                if m != 0:
                    problem = problem or f'fault marker {m} after {e["name"]}'
            if problem:
                break
        home(s)
        after = dev.command(s, 'info')
        print(after)
        if ride_state(after) != 'idle':
            problem = problem or f'the unit is not idle at the end: {after}'
        m = fault_marker(s)
        if m != 0:
            problem = problem or f'fault marker {m} at the end'
        dev.command(s, 'prof reset')
        time.sleep(20)
        prof = dev.prof_read(s)
    paths = sheets(pairs, out) if pairs else []
    wall = prof['wall_ms'] / 1000 or 1
    st = prof['sections']
    awake = sum(t for n, c, t, m in st if n == 'step' or n in dev.OUTSIDE)
    awake_pct = 100 * awake / dev.CPU_HZ / wall
    print(f'walked {walked}, skipped {len(skipped)}, drift aborts {len(drifts)}')
    for n, w in skipped:
        print(f'  skipped {n}: {w}')
    for n, w in drifts:
        print(f'  drift {n}: {w}')
    print(f'problem: {problem or "none"}; awake {awake_pct:.2f} % idle on the overview; '
          f'{time.monotonic() - t0:.0f} s')
    for p in paths:
        print(p)
    sys.exit(1 if problem else 0)


def selftest():
    import tempfile
    from PIL import Image
    ok = lambda name, keys=('start',), dyn=False: {'n': 1, 'name': name, 'keys': list(keys), 'dyn': dyn}
    assert refuse(ok('MET MAIN/SETTINGS/ME')) is None
    assert refuse(ok('MET MAIN/SENSORS/PHONE')) == 'ancestor SENSORS is not a static menu'
    assert refuse(ok('MET MAIN/SENSORS')) is None  # entering a dynamic menu from a static parent is fine
    assert refuse(ok('MET MAIN/SETTINGS/ME', dyn=True)) == 'a Start inside a data-dependent menu'
    assert refuse(ok('MET MAIN/PLACES/ADD PAGE')) and refuse(ok('MET MAIN/PAGES/ACTIVE BIKE'))
    assert refuse(ok('MET RIDE MENU/LIGHT')) and refuse(ok('MET MAIN/SETTINGS/DISPLAY/CAL'))
    assert ride_state('x, auto off held 1, ride idle') == 'idle' and ride_state('x') == '?'
    k = ('page --long', 'page', 'start', 'page', 'start', 'start')
    assert drift_checks(ok('MET MAIN/SETTINGS/ME/LEAF', k)) == [(0, 'MET MAIN'), (2, 'MET MAIN/SETTINGS'),
                                                                (4, 'MET MAIN/SETTINGS/ME')]
    assert drift_checks(ok('MET MAIN/SETTINGS', ('page --long', 'start'))) == [(0, 'MET MAIN')]
    assert refuse(ok('MET MAIN/WORKOUTS')) is None
    assert refuse(ok('MET MAIN/SETTINGS/SYSTEM/FORMAT')) == 'name has FORMAT'
    assert refuse(ok('MET MAIN/PLACES/SAVE PLACE')) == 'name has SAVE'
    assert refuse(ok('MET MAIN/RESET')) and refuse(ok('MET RIDE MENU/SAVE'))
    assert refuse(ok('MET MAIN/SETTINGS/DRIVE')) == 'name has DRIVE'
    assert refuse(ok('MET X', ('start', 'long_page'))) == "unknown key 'long_page'"
    assert refuse(ok('MET X', ('page --long', 'back'))) is None
    assert refuse(ok('IMP MAIN')) and refuse(ok('MET OVERVIEW Dmy'))
    assert refuse(ok('MET OVERVIEW')) is None
    assert is_ride(ok('MET RIDE PAGE 1')) and not is_ride(ok('MET MAIN'))
    # title bands: equal text in other colours matches, other text does not
    def band(text_x, fg, bg):
        im = Image.new('RGB', (240, 320), bg)
        for x in range(text_x, text_x + 60):
            for y in range(30, 44):
                im.putpixel((x, y), fg)
        return im
    assert same_title(band(80, (255, 255, 255), (0, 0, 0)), band(80, (0, 0, 0), (255, 255, 255)))
    assert not same_title(band(80, (255, 255, 255), (0, 0, 0)), band(150, (255, 255, 255), (0, 0, 0)))
    assert not same_title(band(80, (255, 255, 255), (0, 0, 0)), Image.new('RGB', (240, 320)))
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
