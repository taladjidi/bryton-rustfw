#!/usr/bin/env python3
"""Package the Rust firmware as an S510 update: out/IMG.bin (a RAW image) + out/update.ini.

  cargo build --release && python3 tools/pack.py

The stock updater (BOARD.md 10) takes IMG.bin as a raw image, skipping gunzip, when the
4 bytes at 0x94 are "R" + 3 digits; it needs the file to be at least 0x100001 bytes, below 10 MB, and its last
4 bytes to be the additive sum (mod 2^32, little endian) of all the other bytes. It writes the file to the eMMC OS
area and reads it back to check that sum; the bootloader then copies 6 MB of that area to SDRAM 0xd0000000 and
jumps to the reset vector, without any check. Every rule is re-checked here before writing.

A ride build (EMMC_WRITE=1 in the environment) is refused with a diagnostic build flag also set (DIAG_STOP stops the
boot and waits for a cable; USB_HS and USB_PACE_US are USB experiments, docs/install.md "never install"), unless
PACK_VARIANT=1 says it is meant; POWER_STEPS (a planned variant, docs/status.md) is printed, never silent.

  python3 tools/pack.py selftest
"""
import os, struct, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ELF = os.path.join(ROOT, 'target', 'thumbv7em-none-eabihf', 'release', 'bryton-rustfw')
OUT = os.path.join(ROOT, 'out')
BASE, SPAN = 0xd0000000, 0x00100000           # load address, and the 1 MB image (link.x SDRAM region)
MIN_SIZE = 0x100001
NEVER_RIDE = ('DIAG_STOP', 'USB_HS', 'USB_PACE_US')   # set at all (even empty or 0): the build is a variant


def ride_flags(env):
    """(refused, noted): the diagnostic flags a ride build (EMMC_WRITE=1) carries; both empty for any other build."""
    try:
        ride = int(env.get('EMMC_WRITE', '0'), 0) == 1
    except ValueError:
        ride = False
    if not ride:
        return [], []
    return [v for v in NEVER_RIDE if v in env], [v for v in ('POWER_STEPS',) if v in env]


def load_segments(path):
    """(physical address, bytes) of every PT_LOAD segment with file contents (ELF32 little endian)."""
    e = open(path, 'rb').read()
    if e[:4] != b'\x7fELF' or e[4] != 1 or e[5] != 1:
        sys.exit('pack.py: not a 32-bit little-endian ELF')
    phoff, = struct.unpack_from('<I', e, 0x1c)
    phentsize, phnum = struct.unpack_from('<HH', e, 0x2a)
    segs = []
    for i in range(phnum):
        p_type, off, _vaddr, paddr, filesz, _memsz, _flags, _align = struct.unpack_from('<8I', e, phoff + i * phentsize)
        if p_type == 1 and filesz and off != 0:   # off 0: only the ELF headers, which lld loads when the
            segs.append((paddr, e[off:off + filesz]))  # lowest address (ITCM 0x1000) leaves room below it
    return segs


def main():
    refused, noted = ride_flags(os.environ)
    if refused and os.environ.get('PACK_VARIANT') != '1':
        sys.exit(f'pack.py: a ride build (EMMC_WRITE=1) with {" ".join(refused)} set: it must not be installed. '
                 'Unset them, or PACK_VARIANT=1 to pack this variant on purpose')
    for v in refused + noted:
        print(f'pack.py: ride build variant: {v}={os.environ[v]}')
    raw = bytearray(SPAN)
    end = 0
    for paddr, data in load_segments(ELF):
        if not (BASE <= paddr and paddr + len(data) <= BASE + SPAN):
            sys.exit(f'pack.py: segment at 0x{paddr:08x} is outside 0x{BASE:08x}..+1 MB')
        raw[paddr - BASE:paddr - BASE + len(data)] = data
        end = max(end, paddr - BASE + len(data))
    sp, reset = struct.unpack_from('<2I', raw, 0)
    tag = bytes(raw[0x94:0x98])
    size, = struct.unpack_from('<I', raw, 0x98)
    if not (0x20000000 < sp <= 0x20020000):
        sys.exit(f'pack.py: initial SP 0x{sp:08x} not in DTCM')
    if not (BASE <= reset < BASE + end and reset & 1):
        sys.exit(f'pack.py: reset vector 0x{reset:08x} not Thumb code in the image')
    if not (tag[:1] == b'R' and tag[1:].isdigit()):
        sys.exit(f'pack.py: 0x94 holds {tag!r}, the updater needs "R" + 3 digits')
    if size != end:
        sys.exit(f'pack.py: image size word 0x{size:x} != loaded end 0x{end:x}')
    img = bytes(raw) + (sum(raw) & 0xffffffff).to_bytes(4, 'little')   # 1 MB + trailer: the updater wants > 1 MB
    if not (MIN_SIZE <= len(img) < 0xa00001):
        sys.exit(f'pack.py: image of {len(img)} B outside {MIN_SIZE}..0xa00000')
    if (sum(img[:-4]) & 0xffffffff) != int.from_bytes(img[-4:], 'little'):
        sys.exit('pack.py: image sum does not match its last 4 bytes')
    os.makedirs(OUT, exist_ok=True)
    open(os.path.join(OUT, 'IMG.bin.tmp'), 'wb').write(img)   # atomic: lane P's dev.py selftest may read IMG.bin
    os.replace(os.path.join(OUT, 'IMG.bin.tmp'), os.path.join(OUT, 'IMG.bin'))
    open(os.path.join(OUT, 'update.ini'), 'w', newline='\r\n').write(
        f'[OS]\nName=IMG.bin\nVersion={tag.decode()}\nSize={len(img)}\n')
    print(f'image {end} B used, reset 0x{reset:08x}, SP 0x{sp:08x}, tag {tag.decode()}; '
          f'IMG.bin {len(img)} B, sum {img[-4:].hex()} -> {OUT}')


def selftest():
    assert ride_flags({}) == ([], [])
    assert ride_flags({'DIAG_STOP': '3', 'USB_HS': '1', 'POWER_STEPS': '0'}) == ([], [])   # not a ride build
    assert ride_flags({'EMMC_WRITE': '0', 'DIAG_STOP': '3'}) == ([], [])
    assert ride_flags({'EMMC_WRITE': 'x', 'DIAG_STOP': '3'}) == ([], [])
    assert ride_flags({'EMMC_WRITE': '1'}) == ([], [])
    assert ride_flags({'EMMC_WRITE': '1', 'DIAG_STOP': '', 'USB_PACE_US': '0'}) == (['DIAG_STOP', 'USB_PACE_US'], [])
    assert ride_flags({'EMMC_WRITE': '0x1', 'USB_HS': '1', 'POWER_STEPS': '127'}) == (['USB_HS'], ['POWER_STEPS'])
    print('pack.py selftest: ok')


if __name__ == '__main__':
    selftest() if sys.argv[1:] == ['selftest'] else main()
