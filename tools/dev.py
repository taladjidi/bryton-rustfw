#!/usr/bin/env python3
"""The computer side of the USB dev link (firmware side: src/link/dev.rs). Needs the firmware running with the cable
plugged in: the dot at the top right turns green when the link is ready.

  dev.py info                     version and uptime
  dev.py where                    the screen, cursor row and label: where TITLE|row/rows|LABEL (read-only)
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
  dev.py drivelog                 the USB drive's last 32 commands, oldest first
  dev.py wtest LBA                write test on one sector (in free space): changed, read back, restored
  dev.py prof [--reset] [--watch N]   the profiler (src/link/prof.rs): per section of the main loop, calls/s,
                                  mean and max us, % of wall time; panel pixels and eMMC sectors a second.
                                  --reset starts a new window; --watch N prints a fresh N-second window, again
                                  and again (tools/profile.py runs scripted scenarios)
  dev.py reset                    restart the unit (bootloader, then the installed firmware)
  dev.py stackfault               run the stack past its canary on purpose (no ride open): the unit must reset
                                  with the fault marker (peek 0x580040c4 reads 0xfa0000nn within a minute)
  dev.py selftest                 offline checks of this script (check.sh runs it)
"""
import argparse, os, struct, sys, time, zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VID, PID = 0x1209, 0x0001          # src/board/usb.rs
W, H = 240, 320
CPU_HZ = 200_000_000               # src/main.rs CPU_HZ: the profiler counts these cycles
# sections that hold others (src/link/prof.rs): left out of the sums
INCLUSIVE = ('step', 'tiles')
# the main loop's own sections, outside App::step
OUTSIDE = ('touch', 'input', 'usb')


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


def ls(s, path):
    """A folder of the eMMC FAT volume: [(size, is_dir, name)] (tools/sync.py uses it too)."""
    rows = [command(s, f'ls {path}')]
    while rows[-1] != 'end' and not rows[-1].startswith('err'):
        rows.append(line(s))
    if rows[-1].startswith('err'):
        raise OSError(f'ls {path}: {rows[-1]}')
    out = []
    for r in rows[:-1]:
        size, kind, name = r.split(' ', 2)
        out.append((int(size, 16), kind == 'd', name))
    return out


def get(s, path, progress=False):
    """A file of the eMMC FAT volume, 4 KB an answer (tools/sync.py uses it too). Its size is the one `ls` gives its
    folder: an answer cut by a dropped link is raised here, never copied short (AGENTS.md, Project rules)."""
    folder, _, name = path.replace('/', '\\').rpartition('\\')
    sizes = {n.upper(): size for size, is_dir, n in ls(s, folder or '\\') if not is_dir}
    if name.upper() not in sizes:
        raise OSError(f'get {path}: not in its folder')
    size = sizes[name.upper()]
    data = bytearray()
    while len(data) < size:
        answer = command(s, f'get {path} {len(data)} 4096')
        if answer.startswith('err'):
            raise OSError(f'get {path}: {answer}')
        try:
            chunk = bytes.fromhex(answer)
        except ValueError:
            raise OSError(f'get {path}: malformed answer at {len(data)}') from None
        want = min(4096, size - len(data))
        if len(chunk) != want:
            raise OSError(f'get {path}: answer of {len(chunk)} bytes at {len(data)}, want {want}')
        data += chunk
        if progress and len(data) % (256 * 1024) == 0:
            print(f'{len(data) // 1024} KB...', file=sys.stderr)
    return bytes(data)


def prof_read(s):
    """The profiler's window: {'wall_ms', 'slept_ms', 'overhead', 'pixels', 'read', 'written', 'sections':
    [(name, calls, cycles, max cycles)]}."""
    rows = [command(s, 'prof')]
    if not rows[0].startswith('prof '):
        sys.exit(f'dev.py: {rows[0]} (a build without the profiler?)')
    while rows[-1] != 'end':
        rows.append(line(s))
    return prof_parse(rows)


def prof_parse(rows):
    head = rows[0].split()
    if head[0] != 'prof' or len(head) != 7 or rows[-1] != 'end':
        sys.exit(f'dev.py: unexpected answer {rows[0]!r}')
    keys = ('wall_ms', 'slept_ms', 'overhead', 'pixels', 'read', 'written')
    p = dict(zip(keys, map(int, head[1:])))
    p['sections'] = [(n, int(c), int(t), int(m)) for n, c, t, m in (r.split() for r in rows[1:-1])]
    return p


def prof_table(p, title=None):
    """The window as a markdown table: per section calls/s, mean and max us, % of wall time."""
    wall = max(p['wall_ms'], 1) / 1000
    us = lambda cycles: cycles / (CPU_HZ / 1e6)
    out = [f'### {title}', ''] if title else []
    out += [f'window {wall:.1f} s, asleep {100 * p["slept_ms"] / 1000 / wall:.1f} %, '
            f'{p["pixels"] / wall:.0f} px/s to the panel, eMMC {p["read"] / wall:.1f} sectors/s read, '
            f'{p["written"] / wall:.1f} written; a section costs {p["overhead"]} cycles', '',
            '| section | calls/s | mean us | max us | % wall |', '|---|---:|---:|---:|---:|']
    sections = {n: (c, t, m) for n, c, t, m in p['sections']}
    for n, (c, t, m) in sections.items():
        if c:
            note = ' (holds the others)' if n == 'step' else ' (inside draw or input)' if n == 'tiles' else ''
            out.append(f'| {n}{note} | {c / wall:.1f} | {us(t) / c:.1f} | {us(m):.0f} | {100 * us(t) / 1e6 / wall:.3f} |')
    inner = sum(t for n, (c, t, m) in sections.items() if n not in INCLUSIVE + OUTSIDE)
    other = sections.get('step', (0, 0, 0))[1] - inner
    awake = sections.get('step', (0, 0, 0))[1] + sum(sections.get(n, (0, 0, 0))[1] for n in OUTSIDE)
    out.append(f'| step, not in a section | | | | {100 * us(other) / 1e6 / wall:.3f} |')
    out.append(f'| **awake** | | | | **{100 * us(awake) / 1e6 / wall:.2f}** |')
    return '\n'.join(out)


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
    # a window of 2 s: step 4 ms of work, draw 1 ms of it, usb 2 ms outside it
    p = prof_parse(['prof 2000 1990 24 4800 3 0', 'step 40 800000 30000', 'draw 40 200000 9000', 'tiles 0 0 0',
                    'usb 100 400000 8000', 'end'])
    t = prof_table(p)
    assert '| draw | 20.0 | 25.0 | 45 | 0.050 |' in t, t
    assert '| step, not in a section | | | | 0.150 |' in t and '**0.30**' in t and 'tiles' not in t, t
    assert '2400 px/s' in t and '1.5 sectors/s' in t, t
    class Link:
        """The dev link's computer side: `ls` lists one file of `size` bytes, each `get` takes the next of `answers`."""
        def __init__(self, size, answers=()):
            self.size, self.answers, self.out = size, list(answers), []

        def reset_input_buffer(self):
            self.out = []

        def write(self, cmd):
            if cmd.decode().split()[0] == 'ls':
                self.out = [f'{self.size:x} f FILE.FIT', 'end']
            else:
                self.out = [self.answers.pop(0)]

        def readline(self):
            return (self.out.pop(0) + '\n').encode()

    class Ask:
        """A link whose `where` answer is canned."""
        def __init__(self, answer):
            self.answer = answer
        def reset_input_buffer(self):
            pass
        def write(self, cmd):
            assert cmd == b'where\n', cmd
        def readline(self):
            return (self.answer + '\n').encode()
    assert command(Ask('where SENSORS|3/7|HEART RATE'), 'where') == 'where SENSORS|3/7|HEART RATE'
    assert command(Ask('where OVERVIEW'), 'where') == 'where OVERVIEW'
    assert any(l.strip().startswith("sub.add_parser('where'") for l in open(__file__)), 'where is a subcommand'
    blob = bytes(range(256)) * 40                       # 10240 bytes: answers of 4096, 4096 and 2048
    path = 'Activities\\FILE.FIT'
    chunks = [blob[i:i + 4096].hex() for i in range(0, len(blob), 4096)]
    assert get(Link(len(blob), chunks), path) == blob
    cases = [
        ('err busy', ['err busy'], 'err busy'),
        ('odd hex', ['abc'], 'malformed answer at 0'),
        ('cut in the middle', [chunks[0], blob[4096:4196].hex()], 'answer of 100 bytes at 4096, want 4096'),
    ]
    for what, answers, want in cases:
        try:
            get(Link(len(blob), answers), path)
        except OSError as e:
            assert want in str(e), (what, str(e))
        else:
            raise AssertionError(f'{what}: a bad answer was copied')
    try:
        get(Link(len(blob)), 'Activities\\OTHER.FIT')
    except OSError as e:
        assert 'not in its folder' in str(e), e
    else:
        raise AssertionError('a file ls does not list was copied')
    print('dev.py selftest ok')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    sub.add_parser('info')
    sub.add_parser('where', help='the screen, the cursor row and its label: where TITLE|row/rows|LABEL (read-only)')
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
    sub.add_parser('stackfault', help='run the stack past its canary: the unit resets with the fault marker')
    sub.add_parser('drive', help='offer the eMMC FAT volume to the computer as a USB drive (eject gives it back)')
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
    p = sub.add_parser('mapbench', help='time the basemap of the full-page map at the unit\'s position: mapbench LEVEL '
                       '[STEPS]: STEPS views, each moved a screen east (cold re-centres)')
    p.add_argument('level', type=int)
    p.add_argument('steps', nargs='?', type=int, default=4)
    p = sub.add_parser('sector', help='read eMMC sectors (read only): sector N [COUNT] [-o file]')
    p.add_argument('n', type=lambda x: int(x, 0))
    p.add_argument('count', nargs='?', type=lambda x: int(x, 0), default=1)
    p.add_argument('-o', help='write the raw bytes here instead of a hex dump')
    p = sub.add_parser('prof', help='the profiler: per section of the main loop, calls/s, mean / max us, %% of wall')
    p.add_argument('--reset', action='store_true', help='start a new window')
    p.add_argument('--watch', type=float, metavar='N', help='a fresh N-second window, again and again (Ctrl-C)')
    sub.add_parser('drivelog', help="the USB drive's last 32 commands (op, lba, count, status, residue)")
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
            elif a.cmd == 'prof' and a.watch:
                try:
                    while True:
                        command(s, 'prof reset')
                        time.sleep(a.watch)
                        print(prof_table(prof_read(s)) + '\n', flush=True)
                except KeyboardInterrupt:
                    pass
            elif a.cmd == 'prof':
                print(command(s, 'prof reset') if a.reset else prof_table(prof_read(s)))
            elif a.cmd == 'key':
                print(command(s, f'key {a.name}' + (' long' if a.long else '')))
            elif a.cmd == 'drivelog':
                print('op  lba      count    st residue')
                r = command(s, 'drivelog')
                while r != 'end' and not r.startswith('err'):
                    op, lba, n, st, res = (int(v, 16) for v in r.split())
                    name = {0xff: 'BUS RESET', 0xfe: 'BOT RESET', 0xfd: 'BAD CBW'}.get(op, f'{op:02x}')
                    print(f'{name:9} {lba:8x} {n:8x} {st} {res:x}')
                    r = line(s)
            elif a.cmd == 'wtest':
                print(command(s, f'wtest {a.lba}'))
            elif a.cmd in ('info', 'where', 'reset', 'drive', 'stackfault'):
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
                try:
                    rows = ls(s, a.path)
                except OSError as e:
                    sys.exit(f'dev.py: {e}')
                for size, is_dir, name in rows:
                    print(f'{size:>12}  {"<dir>" if is_dir else "     "}  {name}')
            elif a.cmd == 'get':
                try:
                    data = get(s, a.path, progress=True)
                except OSError as e:
                    sys.exit(f'dev.py: {e}')
                Path(a.o).write_bytes(data)
                print(f'{len(data)} bytes -> {a.o}')
            elif a.cmd == 'mapbench':
                # a screen east each time (240 px at the level's metres a pixel): a view never drawn before
                mpp = [1, 1.5][a.level % 2] * 2 ** (a.level // 2)  # map.rs mpp16(level) / 16
                for k in range(a.steps):
                    r = command(s, f'bench map {a.level} {int(240 * mpp * (k + 1))} 0').split()
                    if r[0] != 'map':
                        sys.exit(' '.join(r))
                    us, tiles, nbytes, ign, px, zoom = (int(v, 16) for v in r[1:])
                    print(f'level {a.level} (zoom {zoom}): {us / 1000:.1f} ms, {tiles} stock tiles, {ign} IGN tiles '
                          f'decoded, {nbytes / 1024:.0f} KB read, {px} px')
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
