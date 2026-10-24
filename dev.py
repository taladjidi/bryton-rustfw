#!/usr/bin/env python3
"""The computer side of the USB dev link (firmware side: src/dev.rs). Needs the firmware running with the cable
plugged in: the dot at the top right turns green when the link is ready.

  dev.py info                     version and uptime
  dev.py bat                      battery history, one reading a minute (the last 4 hours)
  dev.py shot [-o shot.png]       one screenshot
  dev.py watch [-i 1] [-n 0]      continuous screenshots into target/shots/NNNNN.png (Ctrl-C to stop)
  dev.py run [out/IMG.bin]        load a build into SDRAM and run it (nothing is written to the eMMC;
                                  a reset or a power cycle brings back the installed firmware)
  dev.py peek ADDR [N]            read N registers (peripheral space, aligned; ADDR decimal or 0x hex)
  dev.py poke ADDR VALUE          write one register
  dev.py key page|lap|start|back [--long]   press (or hold) a key: scripted tests on the unit
  dev.py emmc                     identify the eMMC: SEC_COUNT, B (end of the FAT volume), OS area start, reliable write
  dev.py sector N [COUNT] [-o F]  read eMMC sectors N.. (read only): a hex dump, or the raw bytes into F
  dev.py wtest LBA                write test on one sector (in free space): changed, read back, restored
  dev.py reset                    restart the unit (bootloader, then the installed firmware)
  dev.py selftest                 offline checks of this script (check.sh runs it)
"""
import argparse, os, struct, sys, time, zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent
VID, PID = 0x1209, 0x0001          # src/board/usb.rs
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


def shot(s, tries=3):
    """A screenshot as PNG. Retried when cut short: right after another connection (a `key` command), a few
    packets sometimes never reach the computer although the firmware sent them all (seen 2026-10-23, 2 in 30;
    cause not found), and a screenshot can simply be asked again."""
    for _ in range(tries):
        head = command(s, 'shot')
        if head != f'shot {W} {H}':
            sys.exit(f'dev.py: unexpected answer {head!r}')
        data = s.read(2 * W * H)
        if len(data) == 2 * W * H:
            return png(data)
    sys.exit(f'dev.py: screenshot cut short ({len(data)} of {2 * W * H} bytes), {tries} times')


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


def read_sector(s, n, tries=5):
    """Sector n, asked again when an answer comes back cut (the link can drop bytes: AGENTS.md)."""
    for _ in range(tries - 1):
        answer = command(s, f'sector {n}')
        if len(answer) == 1024 and not answer.startswith('err'):
            break
        time.sleep(0.05)
    else:
        answer = command(s, f'sector {n}')
    return sector_bytes(answer)


def sector_bytes(answer):
    """The answer to `sector N`: 1024 hex digits -> 512 bytes (or exit with the firmware's error)."""
    if answer.startswith('err') or len(answer) != 1024:
        sys.exit(f'dev.py: {answer[:80]}')
    return bytes.fromhex(answer)


def hexdump(data, base):
    for i in range(0, len(data), 16):
        row = data[i:i + 16]
        text = ''.join(chr(b) if 32 <= b < 127 else '.' for b in row)
        print(f'{base + i:08x}  {row.hex(" ")}  {text}')


def selftest():
    p = png(bytes(range(256)) * 600)                    # 2 * 240 * 320 bytes
    assert p.startswith(b'\x89PNG') and p[12:16] == b'IHDR'
    raw = zlib.decompress(p[p.index(b'IDAT') + 4:p.index(b'IEND') - 8])
    assert len(raw) == H * (1 + 3 * W) and raw[1:4] == bytes((0, 0, 8))   # pixel 0x0001: blue 1 << 3
    packed = ROOT / 'out' / 'IMG.bin'
    if packed.exists():
        data, total = image_payload(packed)
        assert struct.unpack_from('<I', data, 0x98)[0] == len(data) and total == sum(data) & 0xffffffff
    assert sector_bytes('00ff' * 256) == b'\x00\xff' * 256
    print('dev.py selftest ok')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    sub.add_parser('info')
    sub.add_parser('bat', help='battery history: one reading a minute, the last 4 hours')
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
    sub.add_parser('emmc', help='identify the eMMC and print its layout (BOARD.md 15)')
    p = sub.add_parser('ls', help='list a folder of the eMMC FAT volume (read only): ls [PATH]')
    p.add_argument('path', nargs='?', default='/')
    p = sub.add_parser('get', help='copy a file off the eMMC FAT volume (read only): get PATH -o FILE')
    p.add_argument('path')
    p.add_argument('-o', required=True, help='where to write it')
    p = sub.add_parser('find', help='find sectors with many u32 in [LO, HI] (read only): find LBA COUNT LO HI')
    for k in ('lba', 'count', 'lo', 'hi'):
        p.add_argument(k, type=lambda x: int(x, 0))
    p = sub.add_parser('bench', help='time eMMC reads (read only): bench [LBA] [SECTORS a read] [COUNT], or bench scan KB')
    p.add_argument('lba', nargs='?', default='0x10000')
    p.add_argument('n', nargs='?', type=int, default=16)
    p.add_argument('count', nargs='?', type=int, default=128)
    p = sub.add_parser('sector', help='read eMMC sectors (read only): sector N [COUNT] [-o file]')
    p.add_argument('n', type=lambda x: int(x, 0))
    p.add_argument('count', nargs='?', type=lambda x: int(x, 0), default=1)
    p.add_argument('-o', help='write the raw bytes here instead of a hex dump')
    p = sub.add_parser('wtest', help='write test on one eMMC sector (pick one in free space; needs EMMC_WRITE=1)')
    p.add_argument('lba', type=lambda x: int(x, 0))
    p = sub.add_parser('run')
    p.add_argument('image', nargs='?', default=str(ROOT / 'out' / 'IMG.bin'))
    a = ap.parse_args()

    if a.cmd == 'selftest':
        selftest()
    elif a.cmd == 'run':
        run(a.image)
    else:
        with connect() as s:
            if a.cmd == 'bat':
                rows = [command(s, 'bat')]
                while rows[-1] != 'end':
                    rows.append(line(s))
                rows = rows[:-1]
                for i, r in enumerate(rows):
                    mv, *charging = r.split()
                    print(f'{i - len(rows) + 1:5d} min  {mv} mV{"  charging" if charging else ""}')
            elif a.cmd == 'key':
                print(command(s, f'key {a.name}' + (' long' if a.long else '')))
            elif a.cmd == 'wtest':
                print(command(s, f'wtest {a.lba}'))
            elif a.cmd in ('info', 'reset'):
                print(command(s, a.cmd))
            elif a.cmd == 'peek':
                first = command(s, f'peek {a.addr} {a.n}')
                if first.startswith('err'):
                    sys.exit(first)
                base = int(a.addr, 0)
                for i, v in enumerate([first] + [line(s) for _ in range(a.n - 1)]):
                    print(f'{base + 4 * i:08x}: {v}')
            elif a.cmd == 'find':
                at, end = a.lba, a.lba + a.count
                while at < end:
                    n = min(8192, end - at)  # about 4 MB: an answer within the 5 s wait
                    rows = [command(s, f'find {at} {n} {a.lo} {a.hi}')]
                    while rows[-1] != 'end' and not rows[-1].startswith('err'):
                        rows.append(line(s))
                    if rows[-1].startswith('err'):
                        sys.exit(f'dev.py: {rows[-1]}')
                    for r in rows[:-1]:
                        lba, hits = (int(x, 16) for x in r.split())
                        print(f'{lba:#x} {hits}', flush=True)
                    at += n
            elif a.cmd == 'ls':
                first = command(s, f'ls {a.path}')
                rows = [first]
                while rows[-1] != 'end' and not rows[-1].startswith('err'):
                    rows.append(line(s))
                if rows[-1].startswith('err'):
                    sys.exit(f'dev.py: {rows[-1]}')
                for r in rows[:-1]:
                    size, kind, name = r.split(' ', 2)
                    print(f'{int(size, 16):>12}  {"<dir>" if kind == "d" else "     "}  {name}')
            elif a.cmd == 'get':
                data = bytearray()
                while True:
                    answer = command(s, f'get {a.path} {len(data)} 4096')
                    if answer.startswith('err'):
                        sys.exit(f'dev.py: {answer}')
                    chunk = bytes.fromhex(answer)
                    data += chunk
                    if len(chunk) < 4096:
                        break
                    if len(data) % (256 * 1024) == 0:
                        print(f'{len(data) // 1024} KB...', file=sys.stderr)
                pathlib.Path(a.o).write_bytes(data)
                print(f'{len(data)} bytes -> {a.o}')
            elif a.cmd == 'bench' and a.lba == 'scan':
                answer = command(s, f'bench scan {a.n}')
                us = int(answer.split()[1], 16)
                print(f'scan of {a.n} KB in {us / 1000:.1f} ms: {a.n / 1024 / (us / 1e6):.2f} MB/s')
            elif a.cmd == 'bench':
                a.lba = int(a.lba, 0)
                answer = command(s, f'bench {a.lba} {a.n} {a.count}')
                if not answer.startswith('bench '):
                    sys.exit(f'dev.py: {answer}')
                us = int(answer.split()[1], 16)
                kb = a.n * a.count / 2
                print(f'{a.count} reads of {a.n} sectors ({kb:.0f} KB) in {us / 1000:.1f} ms: {kb / 1024 / (us / 1e6):.2f} MB/s')
            elif a.cmd == 'emmc':
                answer = command(s, 'emmc')
                if not answer.startswith('emmc '):
                    sys.exit(f'dev.py: {answer}')
                n, b, os_start, rel = (int(x, 16) for x in answer.split()[1:])
                print(f'SEC_COUNT {n:#x} ({n * 512 / 1e9:.1f} GB); FAT volume 0..{b - 1:#x} (B = {b:#x}); '
                      f'OS area {os_start:#x}..{n - 1:#x}; WR_REL_PARAM {rel >> 8:#04x} REL_WR_SEC_C {rel & 0xff} '
                      f'(reliable write: {"enhanced" if rel >> 8 & 4 else "legacy" if rel & 0xff else "none"})')
            elif a.cmd == 'sector':
                data = b''.join(read_sector(s, a.n + i) for i in range(a.count))
                if a.o:
                    Path(a.o).write_bytes(data)
                    print(f'{len(data)} B -> {a.o}')
                else:
                    hexdump(data, a.n * 512)
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
