#!/usr/bin/env python3
"""Package the Rust firmware as an S510 update: out/IMG.bin (a RAW image) + out/update.ini.

  cargo build --release && python3 tools/pack.py

The stock updater (BOARD.md 10) takes IMG.bin as a raw image, skipping gunzip, when the
4 bytes at 0x94 are "R" + 3 digits; it needs the file to be at least 0x100001 bytes, below 10 MB, and its last
4 bytes to be the additive sum (mod 2^32, little endian) of all the other bytes. It writes the file to the eMMC OS
area and reads it back to check that sum; the bootloader then copies 6 MB of that area to SDRAM 0xd0000000 and
jumps to the reset vector, without any check. Every rule is re-checked here before writing.

A ride build (EMMC_WRITE=1) is refused with a diagnostic build flag also set (DIAG_STOP stops the boot and waits for
a cable; USB_HS and USB_PACE_US are USB experiments, docs/install.md "never install"), unless PACK_VARIANT=1 says it
is meant; a POWER_STEPS other than the ride build's default (a planned variant, docs/status.md) is printed, never
silent. The flags are the image's own, read from the ELF (main.rs `BUILD_FLAGS`, found by name in its symbol table);
the environment is the fallback for an ELF without that symbol.

  python3 tools/pack.py selftest
"""
import os, struct, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ELF = os.path.join(ROOT, 'target', 'thumbv7em-none-eabihf', 'release', 'bryton-rustfw')
OUT = os.path.join(ROOT, 'out')
BASE, SPAN = 0xd0000000, 0x00100000           # load address, and the 1 MB image (link.x SDRAM region)
MIN_SIZE = 0x100001
FLAGS = ('EMMC_WRITE', 'DIAG_STOP', 'USB_HS', 'USB_PACE_US', 'POWER_STEPS', 'FAST_LINK')   # main.rs BUILD_FLAGS
MARK = int.from_bytes(b'BFLG', 'little')
NEVER_RIDE = ('DIAG_STOP', 'USB_HS', 'USB_PACE_US')
RIDE_POWER_STEPS = 0x3f   # main.rs POWER_STEPS with EMMC_WRITE and no POWER_STEPS


def elf_flags(e):
    """The image's own build flags (main.rs BUILD_FLAGS, by name in .symtab) as {name: value}; None without them."""
    shoff, = struct.unpack_from('<I', e, 0x20)
    shentsize, shnum = struct.unpack_from('<HH', e, 0x2e)
    # name, type, flags, addr, offset, size, link, info, addralign, entsize
    secs = [struct.unpack_from('<10I', e, shoff + i * shentsize) for i in range(shnum)]
    for sec in secs:
        if sec[1] != 2 or sec[6] >= shnum:   # SHT_SYMTAB, and its string table
            continue
        names = secs[sec[6]][4]
        for off in range(sec[4], sec[4] + sec[5] - 15, 16):
            name, value, size, _info, _other, shndx = struct.unpack_from('<IIIBBH', e, off)
            start = names + name
            if size != 4 * (1 + len(FLAGS)) or shndx >= shnum or b'11BUILD_FLAGS' not in e[start:e.find(b'\0', start)]:
                continue
            words = struct.unpack_from(f'<{1 + len(FLAGS)}I', e, secs[shndx][4] + value - secs[shndx][3])
            if words[0] == MARK:
                return dict(zip(FLAGS, words[1:]))
    return None


def env_flags(env):
    """The fallback: the flags as the environment would set them. EMMC_WRITE parsed as main.rs `parse_u32` (decimal);
    a diagnostic flag set at all (even empty or 0) counts; POWER_STEPS None when unset or not a number."""
    num = lambda v: int(env[v]) if env.get(v, '').isdigit() else None
    return {'EMMC_WRITE': int(num('EMMC_WRITE') == 1), **{v: int(v in env) for v in NEVER_RIDE},
            'POWER_STEPS': num('POWER_STEPS') if 'POWER_STEPS' in env else RIDE_POWER_STEPS}


def ride_flags(flags):
    """(refused, noted): the flags a ride build (EMMC_WRITE=1) should not carry; both empty for any other build."""
    if flags.get('EMMC_WRITE') != 1:
        return [], []
    return [v for v in NEVER_RIDE if flags.get(v)], [v for v in ('POWER_STEPS',) if flags.get(v) != RIDE_POWER_STEPS]


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
    flags, src = elf_flags(open(ELF, 'rb').read()), 'the ELF'
    if flags is None:
        flags, src = env_flags(os.environ), 'the environment (no BUILD_FLAGS in the ELF)'
    print(f'build flags from {src}: ' + ' '.join(f'{k}={v}' for k, v in flags.items()))
    refused, noted = ride_flags(flags)
    if refused and os.environ.get('PACK_VARIANT') != '1':
        sys.exit(f'pack.py: a ride build (EMMC_WRITE=1) with {" ".join(refused)} set: it must not be installed. '
                 'Build it without them, or PACK_VARIANT=1 to pack this variant on purpose')
    for v in refused + noted:
        print(f'pack.py: ride build variant: {v}={flags[v]}')
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


def fake_elf(words, name=b'_ZN13bryton_rustfw11BUILD_FLAGS17h0123456789abcdefE', addr=0xd0001000):
    """An ELF32 with a .rodata holding `words` and a .symtab naming them (the selftest's)."""
    data = struct.pack(f'<{len(words)}I', *words)
    strtab = b'\0' + name + b'\0'
    symtab = bytes(16) + struct.pack('<IIIBBH', 1, addr, len(data), 1, 0, 1)
    body, offs = b'', []
    for part in (data, symtab, strtab):
        offs.append(52 + len(body))
        body += part
    shoff = 52 + len(body)
    head = b'\x7fELF\x01\x01' + bytes(26) + struct.pack('<I', shoff) + bytes(10) + struct.pack('<HHH', 40, 4, 0)
    secs = bytes(40) + b''.join(struct.pack('<10I', 0, t, 0, a, o, n, link, 0, 4, es) for t, a, o, n, link, es in (
        (1, addr, offs[0], len(data), 0, 0), (2, 0, offs[1], len(symtab), 3, 16), (3, 0, offs[2], len(strtab), 0, 0)))
    assert len(head) == 52
    return head + body + secs


def selftest():
    ride = [MARK, 1, 0, 0, 0, RIDE_POWER_STEPS, 0]
    assert elf_flags(fake_elf(ride)) == dict(zip(FLAGS, ride[1:]))
    assert ride_flags(elf_flags(fake_elf(ride))) == ([], [])
    stop = [MARK, 1, 3, 0, 0, RIDE_POWER_STEPS, 0]   # EMMC_WRITE=1 DIAG_STOP=3 cargo build: whatever pack.py's env
    assert ride_flags(elf_flags(fake_elf(stop))) == (['DIAG_STOP'], [])
    assert ride_flags(elf_flags(fake_elf([MARK, 1, 0, 1, 100, 127, 1]))) == (['USB_HS', 'USB_PACE_US'], ['POWER_STEPS'])
    assert ride_flags(elf_flags(fake_elf([MARK, 0, 3, 1, 0, 0x7f, 1]))) == ([], [])   # not a ride build
    assert elf_flags(fake_elf(ride, name=b'_ZN13bryton_rustfw5OTHER17h0123456789abcdefE')) is None
    assert elf_flags(fake_elf([0] + ride[1:])) is None   # no marker
    assert elf_flags(fake_elf(ride[:-1])) is None        # the wrong size
    # the fallback: the environment
    assert ride_flags(env_flags({})) == ([], [])
    assert ride_flags(env_flags({'DIAG_STOP': '3', 'USB_HS': '1', 'POWER_STEPS': '0'})) == ([], [])
    assert ride_flags(env_flags({'EMMC_WRITE': '0', 'DIAG_STOP': '3'})) == ([], [])
    assert ride_flags(env_flags({'EMMC_WRITE': 'x', 'DIAG_STOP': '3'})) == ([], [])
    assert ride_flags(env_flags({'EMMC_WRITE': '0x1', 'DIAG_STOP': '3'})) == ([], [])   # parse_u32: decimal only
    assert ride_flags(env_flags({'EMMC_WRITE': '01', 'DIAG_STOP': '3'})) == (['DIAG_STOP'], [])
    assert ride_flags(env_flags({'EMMC_WRITE': '1'})) == ([], [])
    assert ride_flags(env_flags({'EMMC_WRITE': '1', 'POWER_STEPS': '63'})) == ([], [])
    assert ride_flags(env_flags({'EMMC_WRITE': '1', 'DIAG_STOP': '', 'USB_PACE_US': '0'})) == (
        ['DIAG_STOP', 'USB_PACE_US'], [])
    assert ride_flags(env_flags({'EMMC_WRITE': '1', 'USB_HS': '1', 'POWER_STEPS': '127'})) == (
        ['USB_HS'], ['POWER_STEPS'])
    print('pack.py selftest: ok')


if __name__ == '__main__':
    selftest() if sys.argv[1:] == ['selftest'] else main()
