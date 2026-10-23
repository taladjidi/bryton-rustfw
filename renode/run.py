#!/usr/bin/env python3
"""Runs the firmware ELF on Renode's STM32H747 (s510.repl) and checks the hardware layer the host tests cannot:
start-up, clocks and GPIO writes, the SPI1 byte stream to the panel, no fault, no hang.

  renode/run.py [--seconds 8] [--vbus] [--renode PATH]

It logs every SPI1 data write together with the D/C pin (PG9), decodes the panel protocol into a screen
(target/renode/screen.png) and compares its fixed parts (the status strip's line, Ferris) with the simulator's preview
(target/sim/preview.bmp, written by cargo test-host). --vbus raises PC13 first, so the USB start path runs: Renode
models the OTG core but no host, so the dot must turn orange (or red if the core reset fails) and the clock keep
running.
"""
import argparse, os, re, struct, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from dev import png, W, H  # noqa: E402

ELF = ROOT / 'target/thumbv7em-none-eabihf/release/bryton-rustfw'
OUT = ROOT / 'target/renode'
RENODE = os.environ.get('RENODE', '/Applications/Renode.app/Contents/MacOS/renode')
SPI = re.compile(r'spi1: .*WriteByte to 0x20 .*value 0x([0-9A-Fa-f]+)')
BSRR_G = re.compile(r'gpioPortG: .*Write\w* to 0x18 .*value 0x([0-9A-Fa-f]+)')
OTG = re.compile(r'usb1: ')
DC = 1 << 9                                             # PG9: 0 command, 1 parameter / pixel data


def symbols(path):
    """{address: name} of the ELF's function symbols (ELF32 little endian)."""
    e = path.read_bytes()
    shoff, = struct.unpack_from('<I', e, 0x20)
    shentsize, shnum = struct.unpack_from('<HH', e, 0x2e)
    secs = [struct.unpack_from('<10I', e, shoff + i * shentsize) for i in range(shnum)]
    out = {}
    for s in secs:
        if s[1] == 2:                                   # SHT_SYMTAB
            strtab = secs[s[6]]
            for i in range(s[5] // 16):
                name, value, size, info = struct.unpack_from('<IIIB', e, s[4] + 16 * i)
                if info & 0xf == 2:                     # STT_FUNC
                    n = e[strtab[4] + name:e.index(b'\0', strtab[4] + name)].decode()
                    out[value & ~1] = (n, size)
    return out


def where(pc, syms):
    for a, (n, size) in syms.items():
        if a <= pc < a + max(size, 2):
            return n
    return '?'


def decode(stream):
    """(dc, byte) pairs -> (commands in order, screen as RGB565 big-endian bytes)."""
    screen = bytearray(2 * W * H)
    cmds, cmd, params = [], None, []
    win = [0, W - 1, 0, H - 1]
    pos = 0
    for dc, b in stream:
        if not dc:
            cmd, params = b, []
            cmds.append(b)
            pos = 0
            continue
        if cmd in (0x2a, 0x2b):
            params.append(b)
            if len(params) == 4:
                lo, hi = params[0] << 8 | params[1], params[2] << 8 | params[3]
                if cmd == 0x2a:
                    win[0], win[1] = lo, hi
                else:
                    win[2], win[3] = lo, hi
        elif cmd == 0x2c:
            w = win[1] - win[0] + 1
            px = pos // 2
            x, y = win[0] + px % w, win[2] + px // w
            if x < W and y < H:
                screen[2 * (y * W + x) + pos % 2] = b
            pos += 1
    return cmds, bytes(screen)


def bmp_rgb(path):
    """24-bit bottom-up BMP (sim.rs) -> list of rows of (r, g, b)."""
    b = path.read_bytes()
    off, = struct.unpack_from('<I', b, 10)
    rows = []
    for y in range(H - 1, -1, -1):
        r = b[off + y * W * 3:off + (y + 1) * W * 3]
        rows.append([(r[3 * x + 2], r[3 * x + 1], r[3 * x]) for x in range(W)])
    return rows


def rgb(screen, x, y):
    p = screen[2 * (y * W + x)] << 8 | screen[2 * (y * W + x) + 1]
    return ((p >> 11) << 3, ((p >> 5) & 0x3f) << 2, (p & 0x1f) << 3)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    # 8 s: the start waits about 1 s for the Nordic chip, which Renode lacks, so its commands time out first.
    ap.add_argument('--seconds', type=float, default=8)
    ap.add_argument('--vbus', action='store_true')
    ap.add_argument('--renode', default=RENODE)
    a = ap.parse_args()

    OUT.mkdir(parents=True, exist_ok=True)
    log = OUT / ('vbus.log' if a.vbus else 'boot.log')
    script = [f'$elf=@{ELF}', f'include @{ROOT}/renode/s510.resc', 'logLevel 2',
              'logLevel 0 spi1', 'sysbus LogPeripheralAccess spi1', 'logLevel 0 gpioPortG',
              'sysbus LogPeripheralAccess gpioPortG', 'logLevel 0 usb1', 'sysbus LogPeripheralAccess usb1',
              'gpioPortE OnGPIO 3 true', 'gpioPortA OnGPIO 1 true']   # Page and Back released (low = pressed)
    if a.vbus:
        script.append('gpioPortC OnGPIO 13 true')
    script += [f'emulation RunFor "{a.seconds}"', 'cpu PC', 'quit']
    with open(log, 'w') as f:
        subprocess.run([a.renode, '--disable-gui', '--console', '-e', '; '.join(script)], stdout=f,
                       stderr=subprocess.STDOUT, check=True, cwd=ROOT)

    stream, dc, usb, after_usb, pc = [], 0, 0, 0, None
    for line in open(log, errors='replace'):
        if m := BSRR_G.search(line):
            v = int(m[1], 16)
            dc = 1 if v & DC else 0 if v & (DC << 16) else dc
        elif m := SPI.search(line):
            stream.append((dc, int(m[1], 16)))
            after_usb += usb > 0
        elif OTG.search(line):
            usb += 1
        elif m := re.fullmatch(r'(?:\x1b\[[0-9;]*m)*(0x[0-9a-fA-F]+)\s*', line):
            pc = int(m[1], 16)
    cmds, screen = decode(stream)
    (OUT / 'screen.png').write_bytes(png(screen))

    fn = where(pc, symbols(ELF)) if pc is not None else '?'
    print(f'{len(stream)} SPI bytes, {len(cmds)} panel commands, {usb} OTG_HS accesses; '
          f'PC 0x{pc or 0:08x} in {fn}; screen -> {OUT / "screen.png"}')
    fails = []
    order = [c for c in cmds if c in (0x01, 0x3a, 0xb0, 0x11, 0x29)]
    if order[:5] != [0x01, 0x3a, 0xb0, 0x11, 0x29]:
        fails.append(f'panel start-up order {[hex(c) for c in order[:5]]}')
    if fn in ('default_handler', 'panic') or (fn == '?' and pc is not None):
        fails.append(f'stopped in {fn} (fault or panic)')
    elif pc is None:
        # With PC13 high the emulated USB core keeps flagging interrupts and the polling floods the log; the answer
        # to `cpu PC` can then be lost at `quit`. The panel traffic checks below still catch a hang.
        print('note: PC not read (log flooded)')
    preview = ROOT / 'target/sim/preview.bmp'
    if preview.exists():
        sim = bmp_rgb(preview)
        for y in list(range(22, 24)) + list(range(272, 316)):          # the strip's line, Ferris (ui.rs)
            bad = [x for x in range(W) if rgb(screen, x, y) != sim[y][x]]
            if bad:
                fails.append(f'row {y} differs from the simulator at x {bad[0]}..{bad[-1]}')
                break
    if a.vbus:
        if usb == 0:
            fails.append('PC13 high but the USB core was never touched')
        if after_usb == 0:
            fails.append('no panel traffic after the USB start: hung')
        dot = rgb(screen, 167 + 6, 6 + 6)                                   # the USB symbol's stem (ui.rs)
        if dot not in ((248, 164, 0), (248, 0, 0)):                         # powered (no host) or failed
            fails.append(f'USB symbol is {dot}, neither orange nor red')
    for f in fails:
        print('FAIL', f)
    print('renode check ok' if not fails else f'{len(fails)} failures (log: {log})')
    sys.exit(1 if fails else 0)


if __name__ == '__main__':
    main()
