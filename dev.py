#!/usr/bin/env python3
"""The computer side of the USB dev link (firmware side: src/dev.rs). Needs the firmware running with the cable
plugged in: the dot at the top right turns green when the link is ready.

  dev.py info                     version and uptime
  dev.py shot [-o shot.png]       one screenshot
  dev.py watch [-i 1] [-n 0]      continuous screenshots into target/shots/NNNNN.png (Ctrl-C to stop)
  dev.py run [out/IMG.bin]        load a build into SDRAM and run it (nothing is written to the eMMC;
                                  a reset or a power cycle brings back the installed firmware)
  dev.py peek ADDR [N]            read N registers (peripheral space, aligned; ADDR decimal or 0x hex)
  dev.py poke ADDR VALUE          write one register
  dev.py key page|lap|start|back [--long]   press (or hold) a key: scripted tests on the unit
  dev.py reset                    restart the unit (bootloader, then the installed firmware)
  dev.py selftest                 offline checks of this script (check.sh runs it)
"""
import argparse, os, struct, sys, time, zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent
VID, PID = 0x1209, 0x0001          # src/usb.rs
W, H = 240, 320


def find_port(timeout=10.0):
    from serial.tools import list_ports
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        for p in list_ports.comports():
            if p.vid == VID and p.pid == PID:
                return p.device
        time.sleep(0.2)
    sys.exit('dev.py: no S510 dev link found (cable in? green dot?)')


def connect(timeout=10.0):
    import serial
    return serial.Serial(find_port(timeout), timeout=3)


def line(s):
    got = s.readline()
    if not got.endswith(b'\n'):
        sys.exit('dev.py: no answer')
    return got.decode(errors='replace').strip()


def command(s, cmd):
    s.reset_input_buffer()
    s.write(cmd.encode() + b'\n')
    return line(s)


def png(pixels565, w=W, h=H):
    """RGB565 big-endian rows -> PNG bytes (stdlib only)."""
    raw = bytearray()
    for y in range(h):
        raw.append(0)                                   # filter: none
        row = pixels565[2 * w * y:2 * w * (y + 1)]
        for i in range(0, len(row), 2):
            p = row[i] << 8 | row[i + 1]
            raw += bytes(((p >> 11) << 3, ((p >> 5) & 0x3f) << 2, (p & 0x1f) << 3))

    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(bytes(raw), 6)) + chunk(b'IEND', b''))


def shot(s):
    head = command(s, 'shot')
    if head != f'shot {W} {H}':
        sys.exit(f'dev.py: unexpected answer {head!r}')
    data = s.read(2 * W * H)
    if len(data) != 2 * W * H:
        sys.exit(f'dev.py: screenshot cut short ({len(data)} of {2 * W * H} bytes)')
    return png(data)


def image_payload(path):
    """The bytes the firmware runs: the packed image up to its size word (pack.py), and their additive sum."""
    img = Path(path).read_bytes()
    size, = struct.unpack_from('<I', img, 0x98)
    if img[0x94:0x95] != b'R' or not 0x9c <= size <= len(img):
        sys.exit(f'dev.py: {path} is not a pack.py image')
    body = img[:size]
    return body, sum(body) & 0xffffffff


def run(path):
    body, total = image_payload(path)
    with connect() as s:
        answer = command(s, f'load {len(body)} {total}')
        if answer != 'ready':
            sys.exit(f'dev.py: {answer}')
        s.write(body)
        answer = line(s)
        if answer != 'ok':
            sys.exit(f'dev.py: {answer}')
    print(f'running {len(body)} B; waiting for it to come back...')
    time.sleep(1.0)                                     # the old link disappears, the new one enumerates
    with connect(timeout=15) as s:
        print(command(s, 'info'))


def selftest():
    p = png(bytes(range(256)) * 600)                    # 2 * 240 * 320 bytes
    assert p.startswith(b'\x89PNG') and p[12:16] == b'IHDR'
    raw = zlib.decompress(p[p.index(b'IDAT') + 4:p.index(b'IEND') - 8])
    assert len(raw) == H * (1 + 3 * W) and raw[1:4] == bytes((0, 0, 8))   # pixel 0x0001: blue 1 << 3
    packed = ROOT / 'out' / 'IMG.bin'
    if packed.exists():
        data, total = image_payload(packed)
        assert struct.unpack_from('<I', data, 0x98)[0] == len(data) and total == sum(data) & 0xffffffff
    print('dev.py selftest ok')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    sub.add_parser('info')
    sub.add_parser('reset')
    p = sub.add_parser('key', help='press a key (the app acts as if it was pressed): key page [--long]')
    p.add_argument('name', choices=['page', 'lap', 'start', 'back'])
    p.add_argument('--long', action='store_true', help='hold it (long press)')
    sub.add_parser('selftest')
    p = sub.add_parser('shot')
    p.add_argument('-o', default='shot.png')
    p = sub.add_parser('watch')
    p.add_argument('-i', type=float, default=1.0, help='seconds between screenshots')
    p.add_argument('-n', type=int, default=0, help='how many (0 = until Ctrl-C)')
    p.add_argument('-d', default=str(ROOT / 'target' / 'shots'))
    p = sub.add_parser('peek', help='read registers: peek 0x40013014 [n]')
    p.add_argument('addr')
    p.add_argument('n', nargs='?', type=int, default=1)
    p = sub.add_parser('poke', help='write a register: poke 0x40013000 0x1001')
    p.add_argument('addr')
    p.add_argument('value')
    p = sub.add_parser('run')
    p.add_argument('image', nargs='?', default=str(ROOT / 'out' / 'IMG.bin'))
    a = ap.parse_args()

    if a.cmd == 'selftest':
        selftest()
    elif a.cmd == 'run':
        run(a.image)
    else:
        with connect() as s:
            if a.cmd == 'key':
                print(command(s, f'key {a.name}' + (' long' if a.long else '')))
            elif a.cmd in ('info', 'reset'):
                print(command(s, a.cmd))
            elif a.cmd == 'peek':
                first = command(s, f'peek {a.addr} {a.n}')
                if first.startswith('err'):
                    sys.exit(first)
                base = int(a.addr, 0)
                for i, v in enumerate([first] + [line(s) for _ in range(a.n - 1)]):
                    print(f'{base + 4 * i:08x}: {v}')
            elif a.cmd == 'poke':
                print(command(s, f'poke {a.addr} {a.value}'))
            elif a.cmd == 'shot':
                Path(a.o).write_bytes(shot(s))
                print(a.o)
            else:
                os.makedirs(a.d, exist_ok=True)
                n = 0
                try:
                    while a.n == 0 or n < a.n:
                        t0 = time.monotonic()
                        Path(a.d, f'{n:05}.png').write_bytes(shot(s))
                        n += 1
                        time.sleep(max(0.0, a.i - (time.monotonic() - t0)))
                except KeyboardInterrupt:
                    pass
                print(f'{n} screenshots in {a.d}')


if __name__ == '__main__':
    main()
