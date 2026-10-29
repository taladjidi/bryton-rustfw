# Development loop, emulation and testing strategy

Moved out of AGENTS.md (which keeps the rules).

## Development loop on the unit

Install the firmware once (docs/install.md). From then on, with the cable in and the green dot at the top right:

    ./dev.py info                 # version, uptime
    ./dev.py shot -o shot.png     # screenshot (what the app drew, rendered row by row: no framebuffer)
    ./dev.py watch -i 1           # continuous screenshots into target/shots/
    ./check.sh && ./dev.py run    # run the new build from RAM: nothing written to the eMMC
    ./dev.py peek 0x40013014 2    # read registers, or DTCM / SDRAM (read only); poke ADDR VALUE writes a register
    ./dev.py key page [--long]    # press or hold a key (page, lap, start, back): scripted tests
    ./dev.py emmc                 # eMMC layout; sector N [COUNT] [-o F] reads sectors (read only)
    ./dev.py drive                # lend the eMMC FAT volume to the computer as a USB drive (eject gives it back)
    ./dev.py ls PATH              # list a folder of the volume; get PATH -o F copies a file off it (read only)
    ./dev.py bat                  # battery history; drivelog: the USB drive's last 32 commands
                                  # find / bench / mapbench / wtest: see ./dev.py -h
    ./dev.py prof [--reset] [--watch 5]   # the profiler: per section of the pass, calls/s, mean / max us, % of wall time
    ./dev.py reset                # back to the installed firmware (so does unplugging, or a power cycle)
    tools/profile.py --run        # run a build from RAM, then scripted scenarios (idle, pages, menu, a ride
                                  # discarded), a markdown table each

- **The profiler** (`src/link/prof.rs`, always on): `App::step`'s sections (`App::timed`, one DWT read each) and the
  main loop's own (touch, input, USB), each its calls, cycles and longest run; the panel's pixels and the eMMC's
  sectors; `tools/dev.py prof --reset` starts a window. A section costs a few tens of cycles (inferred; `prof` reports the overhead
  measured at start-up), well under 0.1 % of the CPU. A build run from RAM is the same code and memory as an installed
  one (the image runs from SDRAM either way): only the USB link differs, and it is its own row. A new costly part of
  the pass gets its section (`Sec`, its name, `App::timed` around it).

- **`run`** stages the image in SDRAM (0xd0800000), checks it as `tools/pack.py` does, and starts it. The start goes
  through a trampoline in ITCM, which copies the image over 0xd0000000 and jumps as the bootloader does.
- **Status: works on the unit (2026-10-23):** `info`, `shot`, `peek` / `poke` and `run` (the build runs from RAM
  and the link comes back). `peek` / `poke` found both first-run bugs (docs/journal.md).
- **The dot:**
  - orange: USB powered, not configured yet;
  - green: the dev link is ready;
  - red: the PHY or the core did not come up (the clock keeps running).
- **Software can't enter Engineer Mode.** The bootloader configures PE3 itself and reads the
  real key, so firmware cannot fake a held Page. Updating the eMMC still goes through the stock updater or
  Engineer Mode (`tools/iaprecover.py --write out/IMG.bin`). `run` covers day-to-day iteration.
- **Still to build:**
  - `log` lines from the firmware over the link;
  - an eMMC `reflash` command (writes exist for the recorder, the drive and `wtest`, behind EMMC_WRITE=1).
- **No watchdog, on purpose.** The IWDG keeps counting in standby unless an option byte (FZ_IWDG_SDBY) says
  otherwise, so it would wake the unit from power-off. Option bytes are flash configuration, and we never
  touch them.

## Emulation

- **The simulator above is the main tool:** it tests the logic at full speed with exact panel checks.
- **Renode, for the hardware layer:**
  - `renode/run.py` boots the unmodified ELF on Renode's STM32H747, plus SPI1 (`renode/s510.repl`, `s510.resc`).
  - It logs the SPI1 bytes with the D/C pin (PG9) and decodes the panel protocol into
    `target/renode/screen.png`.
  - **Checks:**
    - the panel start-up order;
    - no fault or panic;
    - the bar and Ferris are pixel-equal to the simulator's preview;
    - with `--vbus` (PC13 high), the USB start path runs and the clock keeps going.
  - **Install:** `/Applications/Renode.app`, from the `osx-arm64-portable.dmg` of github.com/renode/renode
    release 1.17.0 (sha256 checked against the release; not Apple-signed).
    - Docs: https://renode.readthedocs.io.
    - Another location: set `RENODE`.
  - **Limits:**
    - Renode's RTC model is not running, so the emulated clock shows 00:00:00;
    - there is no ULPI PHY or USB host;
    - SysTick is modelled at 400 MHz, so emulated delays are half as long.
- QEMU has no STM32H7 board.

## Testing strategy

Modelled on the patch repo's layers (bryton-S510-mods `docs/validation.md`: build checks, Ghidra instruction
review, host tests, Unicorn fuzzing of every patch entry point, a CI gate, then the unit: short test, ride path,
ride). Each layer catches what the others miss; none proves the firmware right on its own.

| Layer | Tool | Catches | Misses |
|-------|------|---------|--------|
| 1. Build checks | fmt, clippy `-D warnings`, `forbid(unsafe_code)`, `tools/pack.py` | undocumented `unsafe`, an image the updater or the bootloader would refuse | behaviour |
| 2. Host tests | unit, simulator (`Sim`, panel model), doc tests, `dev_tests` | logic, the panel protocol bytes, the dev protocol | everything below `Board` |
| 3. Emulation | Renode (`renode/run.py`) | start-up order, faults Renode models, pixels equal to the simulator, a switched-off core that still runs (`--off`: a pending interrupt waking the standby's WFI, 2026-10-25) | the MPU, caches and attributes the bootloader leaves on, a real WFI wake, SPI and ULPI timing. On 2026-10-22 it passed an image whose SPI1 lost master mode to a mode fault on the unit |
| 4. Fuzzing | `src/sim/fuzz.rs`: pseudo-random bytes (xorshift, seeded: failures reproduce) into the Nordic packet and UBX parsers with every decoder, into `dev::Session` with command words mixed in, into the FIT reader and the INI import, CBWs and command blocks into the USB drive (`msc.rs`), BLE events into the phone link (`ble::event`, `App::phone_event` on the Sim, and the ANCS / AMS / CTS formats) and into the trainer's (FTMS data, status and responses, FE-C over BLE frames, connections mixed), and a damaged volume under the ride scan, random page edits, random and damaged `pages.bin` and `settings.bin` copies and keys and taps in the page editor and on the end-ride screen; a host test, no new dependency | panics and out-of-range reads on input from the radio chip, the GNSS receiver, the cable and the files on the card (it would have caught `ant_closed` indexing a short packet) | logic errors that do not panic |
| 5. Gate | `check.sh` before every commit | a commit that skips layers 1 to 3 | |
| 6. On the unit, boot | first the dev link (`tools/dev.py peek` / `poke`, `run`) when USB comes up. Before that, a `DIAG_STOP=N` build: the checkpoint at boot stage N waits for USB power and resets; power on, hold Page, plug in: Engineer Mode means stage N was reached (control: an image without it gives none). Written from Engineer Mode with `tools/iaprecover.py --write`. The backlight is no readout (BOARD.md) | where a boot stops when nothing else is visible | anything after the panel is up |
| 7. On the unit, features | docs/status.md "Not yet seen on the unit", the dev link once it runs | the hardware facts the layers above stub | |
