# Build, install, and go back

## Build

    ./check.sh          # fmt, clippy, host tests, build, pack -> out/IMG.bin (1 MB + trailer), out/update.ini;
                        # plus the Renode checks when RENODE points at Renode (AGENTS.md, Emulation)

rustup installs the pinned toolchain and target on first use. `cargo test-host` alone runs the tests and draws
the screens through a simulated panel into `target/sim/*.bmp`. `pack.py` re-checks what the stock updater checks
(raw tag "R039" at 0x94, size over 1 MB, additive-sum trailer) and that the vector table is sane.

## The ANT+ key

ANT+ sensors use a network key licensed under the ANT+ adopter terms, so it is not in this repository. The stock
firmware carries it; copy it from your own stock image:

    tools/fetch_firmware.py        # once: the stock image into firmware/R039/IMG.raw (gitignored)
    tools/ant_key.py               # finds the key in it (by its SHA-256), writes ant_key.bin (gitignored),
                                   # and Shimano's network key (the 8 bytes after it) into di2_key.bin
    ./check.sh                     # build.rs builds the keys in when the files exist

Without `ant_key.bin` the firmware builds without ANT+ (BLE still works); without `di2_key.bin`, without the Di2
channel (SENSORS > DI2 reads NO KEY). A build with the keys is for your own unit: don't publish its `out/IMG.bin`.
`touch ant_key.bin` if you put it back with an old date.

## Install

This **replaces the OS**: no USB drive, no rides saved yet, no updates from this firmware. The way back is the
bootloader's recovery mode, which this firmware cannot touch (it lives in the internal flash).

- **From stock:** plug in, copy `out/IMG.bin` and `out/update.ini` to the BRYTON drive, eject, unplug. The stock
  OS installs it while starting up, then restarts into it.
- **From Engineer Mode** (any firmware, or a stuck unit): USB cable plugged **first**, then the four-button
  restart while holding **Page** until "Engineer Mode" shows (without the cable the bootloader ignores Page).
  Then `sudo python3 tools/iaprecover.py --write out/IMG.bin`: it checks the image as `pack.py` and the updater
  do, writes it in 2 KB chunks and reads each one back. Restart.
- Expected: the main screen (the time, Ferris), "BLE" green, then "GPS" blinking.

Record every write to the unit in bryton-S510-mods' FLASHLOG (AGENTS.md).

## Back to Bryton's firmware

Engineer Mode as above, then `sudo python3 tools/iaprecover.py --write-vanilla` (from
[bryton-S510-mods](https://github.com/taladjidi/bryton-S510-mods), see its `docs/recovery.md`; run
`tools/fetch_firmware.py` once to get the stock image). Restart, then install any firmware as usual.

## Copying files without stock

With the cable in and the USB symbol green, DEVICE > USB DRIVE (or `./dev.py drive`) lends the BRYTON volume to the
computer as a USB drive: copy rides off, or routes and workouts on (a build with EMMC_WRITE=1; others offer it read
only). No ride may run or wait to be saved. Eject it on the computer to give it back; after a write the unit
restarts. Unplugging also gives it back (the unit resets). Not yet seen on the unit (docs/status.md).

## IGN maps

Plan IGN v2 (IGN, Licence Ouverte / Etalab 2.0: the map shows "(c) IGN") for an area, made on the computer and copied
to the card. Needs Python 3 with Pillow and lz4 (`pip3 install pillow lz4`). SCAN 25 is not open to private
individuals: the tool only fetches Plan IGN v2.

1. Fetch the tiles into a cache outside the repository (`DIR/{z}/{x}/{y}.png`; resumable: run it again after a cut,
   it skips what it has; 4 requests a second, so 150 x 150 km at zooms 11 to 16, about 165,000 tiles, takes about
   12 hours):

       tools/ign_pack.py fetch --bbox LON0,LAT0,LON1,LAT1 --zooms 11-16 --cache ~/ign-cache

   (the bounding box in degrees, west, south, east, north; 150 km is about 1.35 degrees of latitude and, at 45 N,
   1.9 degrees of longitude).
2. Pack them (about 20 ms a tile on one core, spread over all of them; about 3.9 GB for that area):

       tools/ign_pack.py pack --cache ~/ign-cache --out ~/ign-packs

   It writes `IGN<z>_<x / 32>_<y / 32>.PAK` (one per zoom and block of 32 x 32 tiles) and `ATTRIB.TXT`.
   To check them on the computer: `IGN_PACKS=~/ign-packs cargo test-host every_packed_tile -- --ignored --nocapture`.
3. Copy them to the card through this firmware's USB drive (below, "Copying files without stock"), or Bryton's
   firmware's, into a folder `Rustfw/Maps` at the drive's top (make it if missing).
   Compare the copy (`diff -r ~/ign-packs /Volumes/BRYTON/Rustfw/Maps`): the USB link can drop. This firmware reads the folder, never writes it; stock ignores it.

## Day to day

Once installed, later builds don't need a flash: with the cable in and the USB symbol green, `./dev.py run` runs
`out/IMG.bin` from RAM, `./dev.py shot` takes a screenshot, and `./dev.py reset` (or unplugging) returns to the
installed build. AGENTS.md, "Development loop on the unit", has the rest.
