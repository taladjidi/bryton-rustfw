#!/usr/bin/env python3
"""Rider S510 recovery through the BOOTLOADER USB mode: read back the OS area, or rewrite it with vanilla.

Needs sudo (libusb must take the device from the OS disk driver): sudo python3 tools/iaprecover.py …
  sudo python3 iaprecover.py [--out FILE] [--bytes N]   read the OS area, compare with vanilla IMG.raw
  sudo python3 iaprecover.py --write-vanilla            write vanilla IMG.raw to the OS area, verify every chunk
  sudo python3 iaprecover.py --write out/IMG.bin        write a bryton-rustfw image (tools/pack.py output) the same way
  sudo python3 iaprecover.py --boot-sum                 compare the unit's bootloader with iap.bin (its byte sum, read-only)

Put the unit in bootloader USB mode first: USB plugged in, press all four buttons to reboot, keep holding Page
(lower right) until the screen shows "Engineer Mode" (docs/how-it-works.md).
Verified 2026-10-19: full 4 026 368 B readback == vanilla IMG.raw (sha256 c7f5e714…).

Protocol (recovered from the bootloader, see docs/how-it-works.md): plain USB mass storage (Bulk-Only Transport, EP OUT 0x01 /
IN 0x81). Opcode 0x28 READ(10) with CDB byte 6 = 0x0d ("read data from iNAND") returns 2 KB of the OS area:
LBA field k -> OS-area byte offset k*2048; transfer length must be 4 blocks (2048 B).
Sub 0x04 returns the 32-bit byte sum of the bootloader's 128 KB of flash (0x08000000..0x08020000) in the first 4
bytes, little-endian. Sub 0x0b ("read internal flash") exists in the handler, but the READ path never calls the
handler for it: the reply is a stale buffer (seen on the unit 2026-10-22). So the bootloader cannot be dumped over USB.
Opcode 0x2A WRITE(10) with byte 6 = 0x0c writes 2 KB the same way (bootloader FUN_0800932c case 0xc).
A failed sector write is only logged on the bootloader's debug UART (the CSW still says OK), so --write-vanilla
reads every chunk back and stops at the first mismatch. Only the SHA256SUMS-verified vanilla image can be
written, or with --write an IMG.bin that passes the checks tools/pack.py and the stock updater make (bryton-rustfw).
Uses libusb through ctypes (no pyusb). sudo is needed so libusb can detach macOS's mass-storage driver.
"""
import ctypes, ctypes.util, hashlib, os, struct, sys

from progress import tqdm

VID = 0x0483                     # STMicroelectronics (as seen in normal USB mode); PID printed when found
EP_OUT, EP_IN = 0x01, 0x81
CHUNK = 2048
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
VANILLA = os.path.join(ROOT, 'firmware', 'R039', 'IMG.raw')

lib = ctypes.CDLL(ctypes.util.find_library('usb-1.0') or '/opt/homebrew/lib/libusb-1.0.dylib')
lib.libusb_get_device_list.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.POINTER(ctypes.c_void_p))]
lib.libusb_get_device_descriptor.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
lib.libusb_open.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p)]
lib.libusb_free_device_list.argtypes = [ctypes.c_void_p, ctypes.c_int]
for f in ('libusb_detach_kernel_driver', 'libusb_claim_interface', 'libusb_release_interface',
          'libusb_attach_kernel_driver', 'libusb_kernel_driver_active'):
    getattr(lib, f).argtypes = [ctypes.c_void_p, ctypes.c_int]
lib.libusb_bulk_transfer.argtypes = [ctypes.c_void_p, ctypes.c_ubyte, ctypes.c_char_p, ctypes.c_int,
                                     ctypes.POINTER(ctypes.c_int), ctypes.c_uint]
lib.libusb_close.argtypes = [ctypes.c_void_p]
lib.libusb_error_name.restype = ctypes.c_char_p


def check(rc, what):
    if rc < 0:
        sys.exit(f'{what}: {lib.libusb_error_name(rc).decode()}')
    return rc


def open_unit():
    check(lib.libusb_init(None), 'libusb_init')
    devs = ctypes.POINTER(ctypes.c_void_p)()
    n = check(lib.libusb_get_device_list(None, ctypes.byref(devs)), 'device list')
    found = []
    for i in range(n):
        desc = ctypes.create_string_buffer(18)
        lib.libusb_get_device_descriptor(devs[i], desc)
        vid, pid = struct.unpack_from('<HH', desc.raw, 8)
        if vid == VID:
            found.append((devs[i], pid))
    if len(found) != 1:
        sys.exit(f'expected exactly one USB device with VID {VID:04x}, found {[hex(p) for _, p in found]}')
    h = ctypes.c_void_p()
    check(lib.libusb_open(found[0][0], ctypes.byref(h)), 'open (run with sudo?)')
    lib.libusb_free_device_list(devs, 1)
    print(f'opened {VID:04x}:{found[0][1]:04x}')
    if lib.libusb_kernel_driver_active(h, 0) == 1:
        check(lib.libusb_detach_kernel_driver(h, 0), 'detach macOS driver (sudo? drive ejected?)')
    check(lib.libusb_claim_interface(h, 0), 'claim interface 0')
    return h


def bulk(h, ep, data_or_len, timeout=5000):
    n = ctypes.c_int()
    if ep & 0x80:
        buf = ctypes.create_string_buffer(data_or_len)
        check(lib.libusb_bulk_transfer(h, ep, buf, data_or_len, ctypes.byref(n), timeout), 'bulk in')
        return buf.raw[:n.value]
    check(lib.libusb_bulk_transfer(h, ep, data_or_len, len(data_or_len), ctypes.byref(n), timeout), 'bulk out')


def command(h, k, tag, write=None, sub=0x0d):
    """One 2 KB vendor transfer: READ(10)/sub (0x0d: 2 KB of the OS area, 0x04: the bootloader sum), or
    WRITE(10)/0x0c when write is the 2 KB to send."""
    op, sub, flags = (0x2a, 0x0c, 0x00) if write else (0x28, sub, 0x80)
    cdb = struct.pack('>BBIBH', op, 0, k, sub, CHUNK // 512).ljust(16, b'\0')
    bulk(h, EP_OUT, struct.pack('<4sIIBBB', b'USBC', tag, CHUNK, flags, 0, 10) + cdb)
    data = bulk(h, EP_OUT, write) if write else bulk(h, EP_IN, CHUNK)
    sig, rtag, residue, status = struct.unpack('<4sIIB', bulk(h, EP_IN, 13))
    if sig != b'USBS' or rtag != tag or status != 0 or (not write and len(data) != CHUNK):
        sys.exit(f'chunk {k}: bad reply (sig {sig}, tag {rtag}/{tag}, status {status})')
    return data


def read_chunk(h, k, tag):
    return command(h, k, tag)


def write_vanilla(h, van):
    """Write every 2 KB chunk of vanilla, read it back, stop at the first mismatch."""
    for line in open(os.path.join(ROOT, 'firmware', 'SHA256SUMS')):
        hsh, name = line.split()
        if name == 'IMG.raw' and hashlib.sha256(van).hexdigest() != hsh:
            sys.exit('vanilla IMG.raw fails SHA256SUMS; refusing to write')
    if len(van) % CHUNK:
        sys.exit('image size is not a multiple of 2 KB')
    write_chunks(h, van, 'vanilla R039')


def check_image(img):
    """What the stock updater checks for a raw IMG.bin (BOARD.md 10), plus a sane vector table (as tools/pack.py)."""
    sp, reset = struct.unpack_from('<2I', img, 0)
    tag = img[0x94:0x98]
    if not 0x100001 <= len(img) < 0xa00001:
        sys.exit(f'size {len(img)} B: the updater wants > 1 MB and < 10 MB')
    if not (tag[:1] == b'R' and tag[1:].isdigit()):
        sys.exit(f'0x94 holds {tag!r}, not "R" + 3 digits')
    if sum(img[:-4]) & 0xffffffff != int.from_bytes(img[-4:], 'little'):
        sys.exit('trailer is not the additive sum of the image')
    if not (0x20000000 < sp <= 0x20020000 and 0xd0000000 <= reset < 0xd0000000 + len(img) and reset & 1):
        sys.exit(f'vector table not sane: SP {sp:#x}, reset {reset:#x}')


def write_chunks(h, img, label):
    """Write img from offset 0 of the OS area in 2 KB chunks (the last one zero-padded, as nothing reads past the
    image), reading each back; stop at the first mismatch. Re-running is always safe: the bootloader is not here."""
    img = img + bytes(-len(img) % CHUNK)
    n = len(img) // CHUNK
    if input(f'Write {label} ({len(img)} B, {n} chunks) to the OS area? type "write": ') != 'write':
        sys.exit('aborted, nothing written')
    for k in tqdm(range(n), desc='write+verify', unit='chunk'):
        want = img[k * CHUNK:(k + 1) * CHUNK]
        command(h, k, 2 * k + 1, write=want)
        if command(h, k, 2 * k + 2) != want:
            sys.exit(f'chunk {k} (offset {k * CHUNK:#x}) did not read back as written; STOP, re-run to retry')


def main():
    args = sys.argv[1:]
    out = args[args.index('--out') + 1] if '--out' in args else 'os_area_readback.bin'
    van = open(VANILLA, 'rb').read() if os.path.isfile(VANILLA) else None   # absent: compare by hash only
    size = int(args[args.index('--bytes') + 1], 0) if '--bytes' in args else 4026368
    if '--write-vanilla' in args and van is None:
        sys.exit(f'{VANILLA} missing; run tools/fetch_firmware.py first')
    img = open(args[args.index('--write') + 1], 'rb').read() if '--write' in args else None
    if img is not None:
        check_image(img)
    h = open_unit()
    if '--boot-sum' in args:
        try:
            got = int.from_bytes(command(h, 0, 1, sub=0x04)[:4], 'little')
        finally:
            lib.libusb_release_interface(h, 0)
            lib.libusb_attach_kernel_driver(h, 0)
            lib.libusb_close(h)
        ref = os.path.join(ROOT, 'firmware', 'R039', 'iap.bin')
        want = sum(open(ref, 'rb').read()) & 0xffffffff if os.path.isfile(ref) else None
        return print(f'bootloader byte sum on the unit {got:#010x}, firmware/R039/iap.bin {want and hex(want)}: '
                     + ('MATCH' if got == want else 'DIFFER'))
    if '--write-vanilla' in args or img is not None:
        try:
            if img is not None:
                write_chunks(h, img, f'{args[args.index("--write") + 1]} sha256 {hashlib.sha256(img).hexdigest()[:12]}')
            else:
                write_vanilla(h, van)
        finally:
            lib.libusb_release_interface(h, 0)
            lib.libusb_attach_kernel_driver(h, 0)
            lib.libusb_close(h)
        print('done; now run without options for a full independent readback')
        return
    got = bytearray()
    try:
        for k in tqdm(range((size + CHUNK - 1) // CHUNK), desc='read', unit='chunk'):
            got += read_chunk(h, k, k + 1)
    finally:
        lib.libusb_release_interface(h, 0)
        lib.libusb_attach_kernel_driver(h, 0)
        lib.libusb_close(h)
    got = bytes(got[:size])
    open(out, 'wb').write(got)
    print(f'read {len(got)} B -> {out}  sha256 {hashlib.sha256(got).hexdigest()}')
    if van is None:
        stock = any(line.split() == [hashlib.sha256(got).hexdigest(), 'IMG.raw']
                    for line in open(os.path.join(ROOT, 'firmware', 'SHA256SUMS')))
        return print('MATCHES stock R039 (by hash)' if stock else 'not stock R039 (patched, or another version)')
    ref = van[:size]
    if got == ref:
        print('MATCHES vanilla IMG.raw' + ('' if size == len(van) else f' (first {size} B)'))
    else:
        i = next(i for i in range(len(ref)) if got[i] != ref[i])
        print(f'DIFFERS from vanilla at offset {i:#x}: got {got[i:i+16].hex()} want {ref[i:i+16].hex()}')


if __name__ == '__main__':
    main()
