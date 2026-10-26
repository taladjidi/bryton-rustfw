#!/usr/bin/env python3
"""Profile the firmware on the unit through the dev link: scripted scenarios (keys and waits), a profiler window each
(`prof`, src/link/prof.rs), printed as markdown tables. Only `ride` writes to the eMMC (its discarded ride).

  tools/profile.py [--run [IMG]] [-s SECONDS] [-o FILE] [SCENARIO...]

Scenarios (all by default, in this order):
  idle    the overview, untouched
  page    the first ride page (with whatever sensors are paired and heard)
  pages   each ride page after it in turn, one window each (`pages 1`, `pages 2`...: the map page is one of them)
  menu    the menu, a Page press every half second
  ride    a ride started with Start, running; then paused and ended with DISCARD (End, DISCARD, DISCARD): it goes to
          \\Rustfw\\Discarded (a build without EMMC_WRITE records nothing), never to \\Activities

Each scenario starts from the overview (Back pressed a few times). `--run` first runs a build from RAM (dev.py run,
out/IMG.bin by default): the same code and memory as an installed one (the image runs from SDRAM either way), so
the figures should match a flashed build, except for the USB link itself, which only runs with the cable in: its cost
is its own row (`usb`), and the rows above it are what a ride without the cable costs.
"""
import argparse, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
import dev  # noqa: E402  (the dev link's computer side)

SCENARIOS = ('idle', 'page', 'pages', 'menu', 'ride')


def key(s, name, long=False, wait=0.4):
    answer = dev.command(s, f'key {name}' + (' long' if long else ''))
    if answer != 'ok':
        sys.exit(f'profile.py: key {name}: {answer}')
    time.sleep(wait)  # the app acts at the end of the pass, then draws


def home(s):
    for _ in range(4):
        key(s, 'back')


def window(s, seconds, title, during=None):
    """A fresh profiler window of `seconds`, `during(s)` called once at its start (keys pressed inside it)."""
    dev.command(s, 'prof reset')
    t0 = time.monotonic()
    if during:
        during(s)
    time.sleep(max(0.0, seconds - (time.monotonic() - t0)))
    return dev.prof_table(dev.prof_read(s), title)


def menu_presses(seconds):
    def go(s):
        key(s, 'page', long=True, wait=0.5)
        for _ in range(int(seconds * 2) - 2):
            key(s, 'page', wait=0.5)
    return go


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('scenarios', nargs='*', help=f'any of {", ".join(SCENARIOS)} (default: all)')
    ap.add_argument('-s', '--seconds', type=float, default=10.0, help='each window (default 10)')
    ap.add_argument('--pages', type=int, default=4, help='ride pages after the first for `pages` (default 4)')
    ap.add_argument('--run', nargs='?', const=str(ROOT / 'out' / 'IMG.bin'), help='run this build from RAM first')
    ap.add_argument('-o', help='also write the tables here')
    a = ap.parse_args()
    if set(a.scenarios) - set(SCENARIOS):
        ap.error(f'scenarios: {", ".join(SCENARIOS)}')
    if a.run:
        dev.run(a.run)
        time.sleep(3.0)  # the start: the scan of stock's rides and the first fix's work settle
    out = []
    with dev.connect() as s:
        out.append(f'# Profile: {dev.command(s, "info")}\n')
        for sc in a.scenarios or SCENARIOS:
            home(s)
            if sc == 'idle':
                out.append(window(s, a.seconds, 'idle overview'))
            elif sc == 'page':
                key(s, 'page')
                out.append(window(s, a.seconds, 'ride page 1'))
            elif sc == 'pages':
                key(s, 'page')
                for k in range(a.pages):
                    key(s, 'page')
                    out.append(window(s, a.seconds, f'ride page {k + 2}'))
            elif sc == 'menu':
                out.append(window(s, a.seconds, 'menu, Page every 0.5 s', menu_presses(a.seconds)))
            elif sc == 'ride':
                key(s, 'start', wait=1.0)  # the ride starts, on the first ride page
                out.append(window(s, a.seconds, 'ride running'))

                def end(s):
                    key(s, 'start')  # pause
                    key(s, 'start', long=True)  # END RIDE: RESUME selected
                    key(s, 'page')  # SAVE
                    key(s, 'page')  # DISCARD
                    key(s, 'start')  # DISCARD RIDE?, KEEP selected
                    key(s, 'lap')  # DISCARD
                    key(s, 'start', wait=1.0)  # discarded: its save into \Rustfw\Discarded
                out.append(window(s, max(a.seconds, 5.0), 'ride paused, ended, discarded', end))
            print(out[-1] + '\n', flush=True)
        home(s)
    if a.o:
        Path(a.o).write_text('\n\n'.join(out) + '\n')
        print(a.o)


if __name__ == '__main__':
    main()
