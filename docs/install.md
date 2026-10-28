# Build, install, and go back

## Build

    ./check.sh          # fmt, clippy, host tests, build, pack -> out/IMG.bin (1 MB + trailer), out/update.ini;
                        # plus the Renode checks when RENODE points at Renode (docs/devloop.md, Emulation)

### Build variants

- `EMMC_WRITE=1 ./check.sh` (src/main.rs `EMMC_WRITE`): a build that may write the card (rides, settings, the
  drive's write mode). Without it the unit records nothing and the USB drive is read only. Install only a build you
  mean to ride with.
- `USB_FS=1 ./check.sh` (src/board/usb.rs `FULL_SPEED`): USB at full speed (12 Mbit/s, 64-byte packets) instead of
  the default high speed. An experiment for the drops (docs/usb.md).
- `DIAG_STOP=N cargo build --release` (src/main.rs `STOP`): stops at boot stage N, to see where a boot stops
  (docs/devloop.md, layer 6). Diagnostic only.

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

This **replaces the OS**: no updates come from this firmware. Rides are saved to `\Activities` and the BRYTON volume can be lent as a USB drive (below). The way back is the
bootloader's recovery mode, which this firmware cannot touch (it lives in the internal flash).

- **From stock:** plug in, copy `out/IMG.bin` and `out/update.ini` to the BRYTON drive, eject, unplug. The stock
  OS installs it while starting up, then restarts into it.
- **From Engineer Mode** (any firmware, or a stuck unit): USB cable plugged **first**, then the four-button
  restart while holding **Page** until "Engineer Mode" shows (without the cable the bootloader ignores Page).
  Then `sudo python3 tools/iaprecover.py --write out/IMG.bin`: it checks the image as `pack.py` and the updater
  do, writes it in 2 KB chunks and reads each one back. Restart.
  The computer will offer to initialise the raw disk: always Ignore or Cancel. Leave the mode by unplugging the cable.
  If the link drops mid-write, redo the write from the start and compare.
- Expected: the overview (the date, today's card), "BLE" green, then "GPS" blinking.

Record every write to the unit in bryton-S510-mods' FLASHLOG (AGENTS.md).

## Back to Bryton's firmware

First, once: `tools/fetch_firmware.py` (the stock image, needed by `--write-vanilla`). Then Engineer Mode as above,
then `sudo python3 tools/iaprecover.py --write-vanilla` (in this repo; details in
[bryton-S510-mods](https://github.com/taladjidi/bryton-S510-mods) `docs/recovery.md`). Restart, then install any
firmware as usual.

## Copying files without stock

With the cable in and the USB symbol green, SETTINGS > SYSTEM > USB DRIVE (or `./dev.py drive`) lends the BRYTON volume to the
computer as a USB drive: copy rides off, or routes and workouts on (a build with EMMC_WRITE=1; others offer it read
only). Writes need an EMMC_WRITE=1 build (Build, above). No ride may run or wait to be saved. Eject it on the computer to give it back; after a write the unit
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
3. Copy them to the card, into a folder `Rustfw/Maps` at the drive's top (make it if missing):
   - free the space first: remove the map folders you do not need from `\MAP\MapData\` with `rm -rf` on the mounted
     drive, not in the Finder (its trash stays on the card);
   - copy with `cp -X` (no `._` files) through this firmware's USB drive (below, "Copying files without stock"), or
     Bryton's firmware's. This firmware needs an EMMC_WRITE=1 build (Build, above). At full speed, about 1 MB/s, so
     3.9 GB takes over an hour: copy a smaller area first, then compare;
   - eject the drive (the unit restarts), mount it again and compare: `diff -r ~/ign-packs /Volumes/BRYTON/Rustfw/Maps`.
     A compare right after the copy reads the Mac's cache, not the card. The USB link can drop, so the compare is the
     test. This firmware reads the folder, never writes it; stock ignores it.
4. On the unit, check the map: the draw time per view, the zoom chosen, and "(c) IGN" in the corner.

## Day to day

Once installed, later builds don't need a flash: with the cable in and the USB symbol green, `./dev.py run` runs
`out/IMG.bin` from RAM, `./dev.py shot` takes a screenshot, and `./dev.py reset` (or unplugging) returns to the
installed build. docs/devloop.md has the rest.
