#!/usr/bin/env python3
"""Copies the ANT network keys from your stock firmware image into ant_key.bin and di2_key.bin (8 bytes each,
gitignored), where the build picks them up (build.rs). Neither is ours to publish: the ANT+ key is licensed under the
ANT+ adopter terms (thisisant.com), the second is the key of Shimano's private network, which stock uses for its Di2
channel. They are never in this repository, never printed, and an image built with them is for your own unit: do not
publish out/IMG.bin.

    tools/ant_key.py [IMG.raw]        default firmware/R039/IMG.raw (tools/fetch_firmware.py gets it)

The ANT+ key is found by its SHA-256 below, not by an address, so any stock release that carries it works. Shimano's
is the 8 bytes after it: stock's network key table, network 0 then network 1 (`Ant_SetNetworkKeys` 0xd027d6d4 sets
key 1 from the same table, 8 bytes on; BOARD.md 11). No hash of it is kept here: it is only checked to be neither
blank nor the ANT+ key. Without ant_key.bin the firmware builds without ANT+ (no heart rate; BLE still works); without
di2_key.bin, without the Di2 channel.
"""
import hashlib, os, sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
KEY_SHA256 = 'ba53300909ed532d17a25cad3c433266e2864ee5db97d6424b9021fc3334a774'   # R039: at 0xd03d68f4


def find(img):
    for i in range(len(img) - 7):
        if hashlib.sha256(img[i:i + 8]).hexdigest() == KEY_SHA256:
            return i
    return None


def main(args):
    path = args[0] if args else os.path.join(ROOT, 'firmware', 'R039', 'IMG.raw')
    img = open(path, 'rb').read()
    i = find(img)
    if i is None:
        sys.exit(f'{path}: no ANT+ key in it (not a stock S510 image?)')
    open(os.path.join(ROOT, 'ant_key.bin'), 'wb').write(img[i:i + 8])
    print(f'ANT+ key found at image offset {i:#x}, written to ant_key.bin (private: never commit or publish it)')
    di2 = img[i + 8:i + 16]
    if len(di2) != 8 or di2 in (bytes(8), b'\xff' * 8, img[i:i + 8]):
        print('no Shimano network key after it: di2_key.bin not written (no Di2 channel)')
        return
    open(os.path.join(ROOT, 'di2_key.bin'), 'wb').write(di2)
    print(f'Shimano network key at image offset {i + 8:#x}, written to di2_key.bin (private: never commit or publish it)')


if __name__ == '__main__':
    main(sys.argv[1:])
