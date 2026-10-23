#!/usr/bin/env python3
"""Copies the ANT+ network key from your stock firmware image into ant_key.bin (8 bytes, gitignored), where the
build picks it up (build.rs). The key is licensed under the ANT+ adopter terms (thisisant.com), so it is never in
this repository, and an image built with it is for your own unit: do not publish out/IMG.bin.

    tools/ant_key.py [IMG.raw]        default firmware/R039/IMG.raw (tools/fetch_firmware.py gets it)

The key is found by its SHA-256 below, not by an address, so any stock release that carries it works. Without
ant_key.bin the firmware builds without ANT+ (no heart rate; BLE still works).
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


if __name__ == '__main__':
    main(sys.argv[1:])
