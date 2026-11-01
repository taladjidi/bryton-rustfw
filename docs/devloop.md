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
    ./dev.py stackfault           # run the stack past its canary (no ride open): pass = the unit resets and
                                  # ./dev.py peek 0x580040c4 reads 0xfa0000nn within a minute
                                  # find / bench / mapbench / wtest: see ./dev.py -h
    ./dev.py prof [--reset] [--watch 5]   # the profiler: per section of the pass, calls/s, mean / max us, % of wall time
    ./dev.py stepbench [N]        # App::step N times back to back (2..16): per-step us and the first / rest ratio; run it
                                  # on a quiet unit with nothing dirty
    ./dev.py where                # the screen and the cursor row's label (read-only)
    ./dev.py reset                # back to the installed firmware (so does unplugging, or a power cycle)
                                  # reset / load / find / bench / sector / poke answer `err ride open` while a ride or its save is
                                  # open; a wedged recorder needs the four-button restart (Engineer Mode gesture, AGENTS.md)
    tools/profile.py --run        # run a build from RAM, then scripted scenarios (idle, pages, menu, a ride
                                  # discarded), a markdown table each
    tools/walk.py --build out/IMG.bin   # the unit walk: the simulator's screen walk replayed by key, unit vs sim
                                  # contact sheets in target/walk/ (gitignored: shots may show the owner's position)
                                  # Only for a build WITHOUT EMMC_WRITE (what check.sh builds): dev.py run loads into
                                  # RAM, but the image's own writes still happen in an EMMC_WRITE build. Build it from
                                  # the main checkout (a worktree lacks ant_key.bin / di2_key.bin: sensor screens show
                                  # NO KEY). Before each Start it asks the unit `where` its cursor is and moves it to the row by name;
                                  # --rides adds the ride pages.

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

## The gate: tiers, lanes and skips

`./check.sh` (AGENTS.md "Before every commit" lists what it checks):
- **Tiers:** `./check.sh --quick` (guards, lint, fmt, firmware clippy, host clippy, host tests; prints that it is not the gate) for
  iterating: it runs a quarter of the fuzz seeds (`FUZZ_SCALE=0.25`, set by the script) and, on a branch, only the tests
  the diff can reach (`tools/test_select.py`: `src/nav`, `training`, `link`, `ui`, `drivers` and `app` give their own unit
  tests plus `sim::` and `app::`, a `src/sim/x_tests.rs` or `fuzz.rs` gives itself, Markdown in docs/, tools/ and renode/
  gives none; anything else is ALL: `src/storage` (the FAT test image is used everywhere), `src/lib.rs`, `src/sim/mod.rs`
  and the simulator's non-test files, `test-data/`, `docs/features.md`, `Cargo.*`, `check.sh`, a new folder; its selftest
  fails if a file a test reads maps to none. The filters are printed, a filter that selects 0 tests fails the tier, doc
  tests are skipped when filtered). `FUZZ_SCALE=0.5 cargo test-host` by hand runs the fuzz tests that read it (fit
  structure, history, page editor; each prints the scale) on that fraction of their seeds, thresholds scaled (never
  below 1); the menu tree's 36 seeds are never cut. check.sh sets it only for its quick test step, unsets it in the
  gate, and tools/review.py drops it. `./check.sh` is the gate; `--full` runs everything whatever the diff; `-v` prints every lane's log.
- **Lanes:** after the guards, lint and fmt, three lanes run in parallel, each in a subshell with its own log in
  `target/check/` (F: firmware clippy, build, stack_check, panics, pack, then the same three checks on the ride build
  (`EMMC_WRITE=1`, built first in the same target dir, the default build last); H: host clippy, tests, doc, deny, fsck of the volumes the tests wrote
  (`target/fat/*.img`: `fsck_msdos -n` or `fsck.vfat -n`), fit_check, walk selftest; P: the Python self-tests and the font regeneration). F and H take different cargo locks
  (`target/thumbv7em-none-eabihf/release` and `target/aarch64-apple-darwin/debug`), so they do not block each other.
  Any failed lane fails the gate and its log is printed. Every step's wall seconds and a total end the output.
- **Renode is bounded:** `renode/run.py` kills Renode after 300 s scaled by the machine's load (the 1-minute load average
  over the cores, never below 1; `RENODE_TIMEOUT` fixes it), retries once at twice that limit on a timeout only (it prints
  "renode: timed out; retrying"), and fails with "timed out twice"; Renode hangs now and then on a loaded machine.
- **Test speed:** `cargo test-host` is as slow as its slowest test while the cores are free, so the long ones are split:
  the recorder's cut tests run in 8 parts (`_part_N`, cut `c` in part `c % 8`; the cut tests assert which part owns the
  few rare cuts, computed from the uncut run), `menu_tree_by_chance` in 4 chunks of 9 seeds (each part asserts its own
  share: `MENU_TREE_OWNED` names the part that reaches each rare menu; re-measure from the printed counts when the walk
  changes). `cargo-nextest` (process per test, per-test times) was measured
  and not adopted: see the commit message of the change that added this; per-test times: `cargo nextest run --target
  aarch64-apple-darwin` with a `[profile.default.junit]` config if you have it installed.
- **Skips by diff, on a branch only:** the changed files are the diff against the merge base with
  `origin/main` plus the working tree's `status --porcelain`. The tool self-tests run only when `tools/`, `test-data/`,
  `renode/`, `docs/theming.md`, `src/ui/tiles.rs` or `check.sh` changed (the walk selftest: `src/sim/walk_tests.rs`
  instead of `src/ui/tiles.rs`); the font check only for `tools/font.py`, `tools/pixel_font.py`, `tools/fonts/`,
  `src/ui/fonts.rs`; Renode only for `src/main.rs`, `src/lib.rs`, `src/app/pass.rs`, `src/board/`, `src/drivers/panel.rs`,
  `src/ui/strip.rs`, `src/ui/overview.rs`, `link.x`, `build.rs`, `Cargo.toml`, `Cargo.lock`, `renode/`. Each skip is
  printed with its reason and repeated in the `SKIPPED:` line. On main, on a detached HEAD and with `--full` nothing is
  skipped, and a missing fitdecode / garmin-fit-sdk, fsck tool, or a deny database that cannot be fetched fails the gate
  instead of skipping.
  (Renode on a branch also runs for `src/drivers/`, `.cargo/config.toml` and `rust-toolchain.toml`, not for `src/storage/`.)
- **Stale files:** `target/fit`, `target/sim` and `target/fat` are removed at the start, so a test that stops writing a
  file cannot leave an old one to pass fit_check, fsck or Renode.
- **No network:** when `cargo deny` cannot fetch the advisory database, the gate retries `cargo deny --offline check -s`
  and prints a skip line for the fetch.
- **Real data, before a ride build:** the `#[ignore]` tests that read the owner's rides, drive, tiles, routes and
  segments run with `tools/real.sh` (`cargo test-host -- --ignored`), the variables (`BRYTON_RIDES`, `BRYTON_DRIVE`,
  `BRYTON_TILES`, `BRYTON_ROUTE_DAT`, `BRYTON_PLANTRIP`, `IGN_PACKS`, `HRVT_FILES`, `SEGMENTS_DIR`) exported by the
  gitignored `tools/real.env`; a test panics with its variable's name when it is unset, so the run cannot pass
  empty. Paths and data never enter the repository.
- **Renode runs last,** after the three lanes and only when they passed: it compares the boot screen with the
  `target/sim/preview.bmp` that lane H writes (it fails when that file is missing) and needs lane F's image.
- **Live numbers:** `tools/numbers.py --write` runs on main (or `--full`) only. On a branch `--check` reports the stale
  markers without failing; the merge step on main writes them.

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
    - after those checks the same boot raises PC13 (the cable in): the USB start path runs and the clock keeps going.
  - **Install:** `/Applications/Renode.app`, from the `osx-arm64-portable.dmg` of github.com/renode/renode
    release 1.17.0 (sha256 checked against the release; not Apple-signed).
    - Docs: https://renode.readthedocs.io.
    - Another location: set `RENODE`.
  - **Limits:**
    - Renode's RTC model is not running, so the emulated clock shows 00:00:00;
    - there is no ULPI PHY or USB host;
    - SysTick is modelled at 400 MHz, so emulated delays are half as long.
- QEMU has no STM32H7 board.

### The radio chip's real app (`tools/nrf_emu.py`)

Bryton's nRF52 application (the radio chip's firmware) runs in a Unicorn emulator, so the library's radio code can talk
to the real app byte for byte instead of to the simulator's model. `python3 -I tools/nrf_emu.py selftest` (a fraction of
a second, in check.sh lane P) boots it, gets the version, checks that an unknown opcode hangs it with no answer, and
that injected ANT events come out as type 08 frames of 44 bytes; it prints `skip:` when the binary
(`NRF_BIN`, default `~/Documents/bryton/firmware/R039/nordic.bin`) or `unicorn` is missing. Nothing of the binary is
in this repository: the header gives the app's offset, the load address is the S332 application origin, and anything
specific to it stays in the private notes (`NRF_PRIVATE` may name a JSON with its error handler's address).

- **Why Unicorn, not Renode:** the harness must pace the UART byte by byte against a host that holds RTS, count
  instructions between two frames, inject SoftDevice events through SWI2 and answer SVCs in Python; Unicorn gives that
  directly. Unicorn's M-class core has no NVIC, so the harness does exception entry and return itself (PRIMASK and the
  condition flags are honoured).
- **Models:** UARTE0 with EasyDMA (1 Mbaud byte time, ENDRX / ENDTX / RXTO events, the ENDRX_STARTRX short, a TX that
  waits while the host holds RTS), NVIC enable / pending, the SoftDevice SVCs the app makes at start and in its event
  poll (enable, cfg_set, ble_enable, version get, ble and ANT event get, SoC event get, app event wait). Any other SVC
  stops the run and is named. Events are injected with `inject_ant` / `inject_ble` (SWI2 pended).
- **Does not model:** the real SoftDevice (its calls cost nothing here; they cost microseconds on the chip), the ANT
  queue of 32, radio timing, RTC, the 4-byte UART FIFO beyond a count, UART errors, priorities (one interrupt at a
  time). One instruction is one cycle at 64 MHz; the Cortex-M4 average is a little higher.
- **Hang detection (by address and instruction count, never by slice size):** the harness reports a hang when the core
  reaches a `B .` exception handler of the vector table, or when the UART interrupt stays masked for 20 000
  instructions (the error handler enters the SoftDevice critical region and spins; a healthy app masks it for a few
  hundred), or sleeps with it masked. `selftest` runs one script at slice sizes 97, 2000 and 5000 and asserts the same
  frames and TX times, and the unknown opcode's hang at the same instruction.
- **ANT event codes:** the app's encoder (SDK `ant_event.c`, `ant_event_enc`) accepts only the codes in its switch
  (0x00 to 0x07, 0x80 and the rest of `ant_parameters.h`'s events). A value that is a message id (0x4F) or an
  unlisted one returns NOT_SUPPORTED and the app hangs: that was the earlier "events beyond 0x80 hang", not a harness
  bug. The harness fills the message like the SoftDevice: `[9, 0x4E, ch, 8 bytes]` for 0x80, `[3, 0x40, ch, 1, code]`
  for the channel events (inferred from the ANT message protocol), then the event code and the channel, which is what
  `nrf::ant_broadcast`, `ant_closed`, `ant_tx_failed` and `ant_tx_completed` parse; `selftest` checks each code.
- **Flow control and the scheduler queue (selftest):** with the host's RTS held for 200 ms while 30 events arrive the
  app waits without hanging and the 30 frames come out in order when released; 96 events in one SoftDevice pass are
  queued, 97 hang it (the 96-slot scheduler queue).
- **`--serve` and the Rust test:** `python3 -I tools/nrf_emu.py --serve` is the chip's UART as a byte pipe on
  stdin / stdout (requests `W`rite, `R`ead, `T`ime, `H`old RTS, `A`NT event, `B`roadcasts at a period, BLE `V` HVX,
  re`X`et; every answer starts with a status and the flags CTS held / RTS held / dead; see `serve`).
  `cargo test-host -- --ignored nrf_emu` runs `src/sim/nrf_emu_tests.rs`: the library's real `nrf::start`, `ant_start`
  and `ant_open` against the real app, then injected broadcasts read through `ant_broadcast`. It prints `skip:` and
  passes without python `unicorn` or the binary. `Sim::nrf_ext` ([`NrfExt`]) routes `nrf_send`, `nrf_recv`, `nrf_reset`
  and `delay_ms` to the pipe instead of the `Nordic` model.
- **Measured (burst of 8 queued ANT events, deterministic detector):** 966 instructions of app work between ENDTX of a
  frame and the app starting the next frame's header DMA (STARTTX), 15 us at CPI 1 and 64 MHz; 1168 instructions to
  the first byte on the wire. The simulator's `frame_gap_us: 80` is pessimistic by about five times, before the real
  SoftDevice's own call time.

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

**Golden screens.** `walk_every_screen` (`src/sim/walk_tests.rs`) hashes every screen it visits (FNV-1a of the
framebuffer) into `test-data/walk.hashes`, one `NNN-path hash` line each, and fails on any difference, naming the
first ten. A screen change you meant: run `WALK_HASHES=write cargo test-host -- walk_every_screen`, read the diff of
that file (the run panics on purpose: a write never counts as a pass; check.sh unsets the variable) together with the screenshots in `target/sim/walk/`, and commit it with the change. A change you did not
mean is the bug the check exists for. The walk is deterministic (the simulator's clock and the synthetic state), so
no frame is left out.
