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
    tools/ant_key.py               # finds the key in it (by its SHA-256), writes ant_key.bin (gitignored)
    ./check.sh                     # build.rs builds the key in when ant_key.bin exists

Without `ant_key.bin` the firmware builds without ANT+ (BLE still works). A build with the key is for your own
unit: don't publish its `out/IMG.bin`. `touch ant_key.bin` if you put it back with an old date.

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

## Day to day

Once installed, later builds don't need a flash: with the cable in and the USB symbol green, `./dev.py run` runs
`out/IMG.bin` from RAM, `./dev.py shot` takes a screenshot, and `./dev.py reset` (or unplugging) returns to the
installed build. AGENTS.md, "Development loop on the unit", has the rest.
