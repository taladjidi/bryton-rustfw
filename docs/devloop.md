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
    ./dev.py nrfoff               # the radio chip's switch-off on a running unit (BOARD.md 11 "Power off"): path, time,
                                  # its RTS 10 ms / 100 ms / 1 s after; the radio then restarts (resets +1)
    ./dev.py where                # the screen and the cursor row's label (read-only)
    ./dev.py stall S              # the loop sleeps S s (1..600) in one go, a ride may be open: the timer must take the gap
    ./dev.py srefresh [MS]        # Stop step 1 (docs/power.md): the SDRAM in self-refresh around one CSleep WFI of MS ms
                                  # (default 100), woken by the RTC wake-up timer; pass = `srefresh ok flags 000000c3 ...`
    ./dev.py stop [MS]            # Stop step 2, from RAM: USB off, Stop for MS ms (default 1000), clocks and SDRAM back,
                                  # the report in backup registers 0 / 31, a reset; then ./dev.py stopreport: pass =
                                  # `Stop OK` (a hang: the four-button restart; nothing is written to the eMMC)
                                  # --usboff: USB fully off first (as on battery); --keep: USB left on, no reset, the
                                  # registers before / after the WFI printed (docs/status.md, Stop mode)
    ./dev.py idlestop S           # phase 2a: the idle Stop allowed with the cable in for S s (0..3600); it ends by
                                  # time-out only (the link is not served during a Stop, so `idlestop 0` may never get
                                  # there); the drop at its end is not logged; then ./dev.py info: `idle Stops ...`
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
    - SysTick (CLKSOURCE = core) counts at the NVIC's `Frequency`, which is 64 MHz unless set (the platform file's
      `systickFrequency: 400_000_000` is not that clock): left alone, every firmware delay is 200 / 64 = 3.125 times
      longer in virtual time (measured 2026-11-02: read gaps of 625 / 782 / 2502 ms for the firmware's 200 / 250 /
      800 ms). `renode/s510.resc` sets `nvic Frequency 200000000`, the firmware's CPU_HZ (src/main.rs), so 1 s of
      SysTick is 1 s of virtual time: the GNSS reads then come every 200 ms, then every 801 to 812 ms (within 5 %).
- QEMU has no STM32H7 board.

### The radio chip's real app (`tools/nrf_emu.py`)

Bryton's nRF52 application (the radio chip's firmware) runs in a Unicorn emulator, so the library's radio code can talk
to the real app byte for byte instead of to the simulator's model. `python3 -I tools/nrf_emu.py selftest` (a fraction of
a second, in check.sh lane P) boots it, gets the version, checks that an unknown opcode hangs it with no answer, and
that injected ANT events come out as type 08 frames of 44 bytes; it prints `skip:` when the binary
(`NRF_BIN`, default `~/Documents/bryton/firmware/R039/nordic.bin`) or `unicorn` is missing. `python3 -I` ignores the user
  site-packages, so install unicorn where `-I` sees it (`python3 -m pip install unicorn` into the interpreter's own
  site-packages or a venv) and name that interpreter with `NRF_PYTHON` (default `python3`; check.sh lane P and the Rust
  test use it, as `FIT_PYTHON` works for the FIT readers): then the gate runs the selftest instead of skipping it. Nothing of the binary is
in this repository: the header gives the app's offset, the load address is the S332 application origin, and anything
specific to it stays in the private notes (`NRF_PRIVATE`, environment only, may name a JSON with its error handler's address).

- **Why Unicorn, not Renode:** the harness must pace the UART byte by byte against a host that holds RTS, count
  instructions between two frames, inject SoftDevice events through SWI2 and answer SVCs in Python; Unicorn gives that
  directly. Unicorn's M-class core has no NVIC, so the harness does exception entry and return itself (PRIMASK and the
  condition flags are honoured).
- **Models:** UARTE0 with EasyDMA (1 Mbaud byte time, ENDRX / ENDTX / RXTO events, the ENDRX_STARTRX short, a TX that
  waits while the host holds RTS), NVIC enable / pending, the SoftDevice SVCs the app makes at start and in its event
  poll (enable, cfg_set, ble_enable, version get, ble and ANT event get, SoC event get, app event wait). Any other SVC
  stops the run and is named. Events are injected with `inject_ant` / `inject_ble` (SWI2 pended).
- **Does not model:** the real SoftDevice (its calls cost nothing here; they cost microseconds on the chip), the ANT
  queue of 32, radio timing, RTC, UART errors, a MAXCNT of 0. One instruction is one cycle at 64 MHz; the Cortex-M4
  average is a little higher. The ANT SVCs the app is seen to call (8 of them, in start, `ant_start`, `ant_open`) are
  accepted with success and no output; any other ANT SVC stops the run (its number and pc are printed, never a
  register: the ANT licence key could be in one).
- **UARTE and the NVIC (spec-faithful parts):** RXD / TXD pointer and count are latched at STARTRX / STARTTX, AMOUNT is
  written at ENDRX / ENDTX / STOP only, the ENDRX_STARTRX short restarts from the new pointer and leaves the finished
  AMOUNT for the ISR; RXSTARTED, TXSTARTED, TXSTOPPED, RXTO, CTS and NCTS (our RTS changes) are raised; INTEN, INTENSET
  and INTENCLR share one value. Exception entry and return follow ARMv7-M (8-byte aligned frame with the xPSR bit 9
  padding flag, EXC_RETURN in LR, return detected on the branch to it, IPSR set); priorities come from IPR (lower
  number first, preemption only by a strictly higher one, nesting), BASEPRI and PRIMASK are honoured.
- **Hang detection (by address and instruction count, never by slice size):** the harness reports a hang when the core
  reaches a `B .` exception handler of the vector table (or the error handler address from `NRF_PRIVATE`, a JSON
  `{"error_handler": N}` kept outside this repository), or runs 2720 instructions in a row with the UART interrupt
  unable to be taken (NVIC disabled, PRIMASK, or BASEPRI at or above its priority), or sleeps in that state. Sleeping
  executes nothing, so waiting for an interrupt never counts. Measured in this emulator: the longest such stretch over
  a healthy start plus 10 s of traffic (version requests, single events, bursts of 8) and over the Rust test's
  `nrf::start`, `ant_start`, `ant_open` and broadcasts is 272 instructions; the limit is 10 times that. `selftest` runs
  one script at slice sizes 97, 2000 and 5000 and asserts the same frames and TX times, and the unknown opcode's hang
  at the same instruction.
- **ANT event codes:** an event code outside `ant_parameters.h`'s list hangs the app (Bryton's nRF app, reversed). The
  harness fills the message like the SoftDevice: `[9, 0x4E, ch, 8 bytes]` for 0x80, `[3, 0x40, ch, 1, code]` for the
  channel events (inferred from the ANT message protocol), then the event code and the channel, which is what
  `nrf::ant_broadcast`, `ant_closed`, `ant_tx_failed` and `ant_tx_completed` parse; `selftest` checks each code.
- **Flow control and the event queue (selftest):** with the host's RTS held for 200 ms while 30 events arrive nothing
  is sent, the app does not hang and the 30 frames come out in order when released; 96 events in one SoftDevice pass
  are queued, 97 hang it (Bryton's nRF app, reversed: 96 slots).
- **`--serve` and the Rust test:** `python3 -I tools/nrf_emu.py --serve` is the chip's UART as a byte pipe on
  stdin / stdout (requests `W`rite, `R`ead, `T`ime, `H`old RTS, `A`NT event, `B`roadcasts at a period, BLE `V` HVX,
  re`X`et; every answer starts with a status and the flags CTS held / RTS held / dead; see `serve`).
  `cargo test-host -- --ignored nrf_emu` runs `src/sim/nrf_emu_tests.rs`: the library's real `nrf::start`, `ant_start`
  and `ant_open` against the real app, then injected broadcasts read through `ant_broadcast`. It prints `skip:` and
  passes without python `unicorn` or the binary. `Sim::nrf_ext` ([`NrfExt`]) routes `nrf_send`, `nrf_recv`, `nrf_reset`
  and `delay_ms` to the pipe instead of the `Nordic` model.
- **Measured in this emulator (burst of 8 queued ANT events):** 965 instructions of app work between ENDTX of a frame
  and the app starting the next frame's header DMA (STARTTX), 15 us at CPI 1 and 64 MHz; 1167 instructions to the
  first byte on the wire, 18 us. The simulator's `frame_gap_us: 80` is pessimistic by about four times, before the
  real SoftDevice's own call time.

- **Differential tests and the soak (`src/sim/nrf_diff_tests.rs`, `#[ignore]`d, not in check.sh):**
  - `cargo test-host -- --ignored nrf_diff` plays one script through the library's real radio code to the emulated app
    and to the fast model (`chip_link.rs`, `chip_table.rs`) and compares what the host sees: the frames out (type, op,
    length, order), whether the chip holds CTS at each checkpoint, the bytes the chip took behind a stalled event frame,
    and the hang verdict. About 10 s. Scenarios: start and a version request; ANT at 4 Hz on 3 channels drained every
    pass; RTS held 200 ms and 2 s with 30 events; 96 events in one pass (fine) and 97 (hang in both); an unknown BLE and
    ANT opcode, a wrong length, a type-only packet, a bad type and an oversize packet (each hangs both); two commands
    behind a stalled event frame. Times are not compared. A disagreement prints both traces: fix the fast model unless
    the emulator is shown wrong against nordic-app.md.
  - Found on 2026-11-01: the chip's UARTE takes 4 more bytes after the header it holds CTS on (11 bytes in all behind a
    5-byte command, not 7); the fast model took none (fixed, `UART_FIFO`, tested in `chip_link_tests.rs`). Two
    emulator limits, not model errors: a payload length of 0 is not hung by the emulator (it does not model a MAXCNT of
    0; nordic-app.md says the real app hangs, the model follows that), and a lone-byte HVX comes out with a zero length
    (an artefact of the inferred input struct, so the tests use 3 bytes). The CTS flag is not compared after a hang (the
    emulator stops stepping there).
    The hang is compared as hang or not by the next checkpoint: both sides log the scripted step it showed in, but the
    emulator finds it one step later (when time runs, not when the bytes are queued), so the steps are printed on a
    disagreement and not compared. The harness's `F` request makes its RTS follow a 16-byte host RX FIFO like the
    model's, which the backlog scenarios need. Found with it: an event queued before a response goes out before it
    (`resp_ahead`), and the UART's 4 bytes outlive the command's frame (stall by payload length: 3, 4 and 5 bytes
    agree). The 4 is inferred (the emulator's UARTE model), cross-checked only by `--ignored nrf_diff`.
  - `NRF_SOAK_S=3600 cargo test-host -- --ignored nrf_soak --nocapture`: a ride's radio traffic for that many emulated
    seconds (default 3600) against the real app: three ANT channels at 246 to 252 ms, a BLE trainer at 10 Hz, a phone
    burst of 12 notifications every 30 s, a version request every 10 s, the host's pass every 50 ms and a 200 ms stall
    every 20 s. It asserts no hang and that every broadcast and response came out, and prints the deepest backlog.
    Speed: about 6 emulated s per wall s (120 s in 19 s; an hour in 593 s, measured 2026-11-01: 81119 events and requests, all 81119 frames out, deepest backlog 16).
  - `python3 -I tools/nrf_emu.py load [s]`: the gap between frames under the same kind of load (below).
- **The gap under load (2026-11-01, `nrf_emu.py load 20`: 3 ANT channels at 4 Hz, trainer at 10 Hz, bursts of 12, the
  host draining at once, 487 gaps):** ENDTX to the next STARTTX is 965 instructions at the least (15 us at 64 MHz and
  CPI 1, the model's `frame_gap_us`), 1632 in the median (26 us), 2056 at p99 (32 us), 6760 at the worst (106 us). The
  15 us holds for frames back to back from the queue; a frame that follows an idle spell costs the wake-up work too.
  The longest stretch with the UART interrupt masked stays 272 instructions.

### Renode with the radio chip's real app (`renode/nrf_run.py`, a spike, 2026-11-01)

The firmware's own board code (`src/main.rs`: the USART2 setup, `nrf_reset` with PA6, `nrf_send`, `nrf_recv`) runs on
Renode against Bryton's real nRF52 app, not the simulator's `Nordic` model.
- **How:** `renode/NrfUsart.cs` (compiled by Renode at load time) replaces Renode's `usart2`. It starts
  `tools/nrf_emu.py --serve` as a child and talks its pipe protocol, in lockstep: every pump sends the bytes that left
  the TX FIFO (`W`), advances the chip by the virtual microseconds since the last pump (`T`) and reads what the chip
  sent (`R`). A pump is due every 10 us while there is traffic and every 10 ms idle. Modelled: the registers the
  firmware uses (CR1, CR3, ISR RXFNE / TC / TXFNF, RDR, TDR, RQR), 16-byte FIFOs, one byte per 10 us out, the RXFNE
  interrupt (NVIC 38), CTS (the chip's FIFO full holds the TX), RTS (asserted at 12 bytes in the RX FIFO: the chip's
  transmitter waits), PA6 (low holds the chip, the rising edge boots the app again: harness `X`).
  Rules from RM0399 (USART): CR1.UE / TE / RE and FIFOEN gate TDR, RDR, TXFNF and RXFNE (the FIFO is one deep without
  FIFOEN); TC is set one byte time (10 us) after the last byte left; the chip's bytes arrive at most one per 10 us of
  virtual time, whatever the FIFO holds, and a byte that finds the FIFO full sets ISR.ORE (bit 3, cleared by ICR.ORECF)
  and is dropped, logged as `nrf overrun`: only RTS (CR3.RTSE, asserted below 12 bytes) keeps the chip from sending.
  BRR is checked when UE is set: 1 Mbaud within 2 % of the kernel clock, else `nrf baud error`. Renode's RCC does not
  keep D2CCIP2R (it reads 0, also the reset value, pclk1), so a 0 is taken as unknown and BRR is checked against 64 MHz,
  the value BOARD.md 11 and the comment in `src/main.rs` give for hsi_ker_ck: a firmware that forgot to select HSI
  would pass. Not modelled: parity and framing errors.
- **Harness failure:** a closed pipe or no answer within 5 s of wall time logs `nrf harness gone` once and ends the
  bridge (the run fails on that line); `Dispose` sends `Q` and kills the child; `nrf_run.py` runs Renode in its own
  process group and kills the group on a time-out.
- **Run:** `python3 renode/nrf_run.py` (35 s virtual, about 2 minutes of wall), `python3 renode/nrf_run.py --hang 3`
  (70 s virtual, about 3.5 minutes: after the last chip reset a status 0 answer to cfg_set or version get is required)
  and `--no-drain` (a test of the overrun check: a 4-byte FIFO the firmware's reads never empty, RTS ignored; it passes only
  when `nrf overrun` is logged, the other runs fail on one). Not in check.sh. It prints `skip:` and exits 0 without Renode, python `unicorn`, the app
  binary (`NRF_BIN`) or the release ELF (`cargo build --release` first). Log: `target/renode/nrf.log`; every command
  and every chip frame is one `nrf tx` / `nrf rx` line.
- **State 2026-11-02 (after the SysTick fix):** both runs end with `FAIL an overrun` at the phone-link setup's frame 9
  (op 0xA2), at 1.78 s of virtual time (5.50 s before the fix: the same event, 3.1 times later, so not the time base).
  It follows the eMMC probe (no card in Renode) and predates the fix; not yet chased (what the firmware does between the
  poll of the FIFO and the reply there). The watchdog's timing could not be re-measured for that reason.
- **What it showed (2026-11-01):**
  - The radio start is answered by the real app, byte for byte: the two cfg_set, ble enable and version get, each with
    status 0.
  - Then the firmware's phone-link setup (device name, appearance, PPCP, the HID, battery and device information
    services, the advertising data, 8 vendor UUIDs) runs against the stubbed SoftDevice calls (`nrf_emu.py`: sources in
    its module doc) and every command is answered with status 0, 30 s of virtual time with no unknown SVC. Advertising
    and a scan (0x73, 0x8a) are stubbed but not reached by this run (a bond and a menu start them).
  - `--hang 3`: one byte too many between a command's header and its payload (injected by the bridge, once) stops the
    real app (a UART interrupt unable to be taken). The start fails, the firmware's watchdog (`nrf_watch.rs`) resets
    the chip through PA6 about 49 s of emulated time later (a 15 s wait counted in the firmware's own passes), and the
    start succeeds again.
  - No ANT key is in this build, so `ant_start` is not exercised.
- **Limits:** those of Renode above (no MPU, caches, real WFI wake, SPI or ULPI timing), and its CPU runs about one
  instruction per cycle with no flash or SDRAM wait states, so the firmware looks faster than on the unit.

### Renode with the GNSS model (`renode/gnss_run.py`, 2026-11-02)

The firmware's SPI4 code (`src/main.rs` `gnss_init`, `gnss_power`, `gnss_transfer`) and the driver (`src/drivers/gnss.rs`,
`src/app/gps.rs`) run on Renode against the host simulator's own receiver, `src/sim/gnss_model.rs`: one source of the
rules (G1 to G10, their sources in that file), not a second model.
- **How:** `renode/GnssSpi.cs` replaces Renode's `spi4`. It starts the ignored test `src/sim/gnss_serve.rs` as a child
  (the test binary, built by `cargo test-host --no-run`) and talks a small binary protocol over its pipe (power, chip
  select low, four bytes exchanged, chip select high; each request carries the virtual time in ms, which is the model's
  clock). PE4 (chip select, by hand in the firmware) is the bridge's GPIO 0, PI6 (supply) its GPIO 1. Registers
  (RM0399 SPI): CR1, SR (RXP, TXP, TXC), TXDR (a 32-bit write is four 8-bit frames, low byte first), RXDR. The bytes go
  to the model at once, so a word never takes the 8 us it takes at 4 MHz. The track is synthetic (a walk north from
  `FIX_LAT`, 4.5 m/s), and so is the UTC.
- **Run:** `python3 renode/gnss_run.py` (30 s virtual, about 100 s of wall; the model in its `record` mode, so a rule
  broken or a lost epoch fails the run and prints the counts and the longest gap), `--fault nak` (the receiver
  refuses the first CFG-VALSET only), `--fault stall` (nothing from 15 s to 17 s: the lost epochs must be inside
  that window, none before or after). `--selftest` drives the served model without Renode, with the verdicts, and
  fails on an EOF or after 120 s. `--modf` runs the SPI registers alone (below).
  Log of the model: `target/renode/gnss_events.log` (a line per change of acks, naks, valsets, pvt, lost epochs and
  violations), Renode's: `target/renode/gnss.log`. Not in check.sh; `skip:` without Renode or the release ELF.
- **Proves:** the register code reaches the receiver (the right bytes, chip select framing, 0xff idle); normal: the
  configuration is ACKed, NAV-PVT arrives about once a second, no rule of the model broken (a violation panics the
  model, and the run fails on `gnss harness gone`); `nak`: the NAK is read and the VALSET sent again, then ACKed;
  `stall`: the model records the lost epochs (G2) and the firmware reads NAV-PVT again afterwards.
- **SPI registers (RM0399 SPI, `GnssSpi.cs`):** CFG1, CFG2 and CR2 take writes only while SPE = 0; TXP, TXC and the
  data path need SPE, CSTART and MASTER; TXDR written otherwise is dropped, while RXP is still set it is OVR; RXDR
  read without RXP gives 0xffffffff (logged); EOT and CTSIZE follow TSIZE. **MODF:** SSM with SSI = 0 and MASTER set
  sets MODF and clears MASTER and SPE. `gnss_run.py --modf` reproduces the SPI1 incident of 2026-10-23 (BOARD.md 6)
  with no firmware: pokes in the right order (SSI, then MASTER) give TXP and an answered TXDR, the reversed order
  gives SR = 0x200 (MODF only), MASTER and SPE cleared, no TXP and no RXP. The kernel clock select (RCC D2CCIP1R
  SPI45SEL) and the pins' alternate functions are not modelled by Renode's RCC and GPIO (no readback), so a wrong
  one is not caught here.
- **Cannot prove:** SPI timing (a word is exchanged when TXDR is written), that the firmware decoded the fixes (only the wire is seen: reading `gps.fixed` or the
  answers counters needs the address of the app's fields in AXI SRAM, not done), the receiver itself (a model on both
  sides, as the Nordic `Hang` rules were), and the limits of Renode above.
- **What it showed (2026-11-02):** the receiver is powered about 3.3 s into the boot, the CFG-VALSET and the second
  configuration message are ACKed (2 acks, 2 VALSET), NAV-PVT arrives, the firmware reads every 200 ms at first, then
  every 801 to 812 ms (the cadence `an_idle_minute_is_read_at_least_every_800_ms` pins on the host; a pass that took
  longer showed 901 ms once, inside the model's 1.5 s). 30 s: 27 NAV-PVT, no lost epoch, no violation. `--fault nak`:
  the refused VALSET is sent again and ACKed. `--fault stall`: the lost epochs are inside the window, NAV-PVT again
  after 17 s. **The first runs showed 0.6 s reads and 2.5 s from 17 s: that was the time base** (above: SysTick at 64
  MHz in a 200 MHz firmware, 3.1 times too slow), not the firmware.

#### Assessment: next steps, and whether Renode's USART timing can test the drain rate

- **Drain rate (the 2026-11-01 failure class: the firmware not emptying the chip's bytes fast enough):** Renode proves
  the wiring and the register protocol here (FIFO full, CTS, RTS release, the interrupt) and catches a gross stall (a
  loop that never reads). It cannot size the margin: instruction timing is ideal, and the
  UART model is ours. The margin (bytes per frame gap, a 16-byte FIFO against the firmware's worst pass) stays the host
  simulator's job (`frame_gap_us`, the cost model), or the unit's.
- **GNSS on SPI4:** Renode has the SPI4 controller; the slave is a C# `ISPIPeripheral` bridging to the GNSS model. The
  model is Rust (src/sim), so either a small Rust binary with the same pipe protocol or a Python port: 1 to 2 days. No
  real receiver firmware exists, so it is a model on both sides.
- **I2C slaves (touch, LTR-308, LPS22HH, LIS2DH12):** C# `II2CPeripheral`s from tables (register map, reset values,
  the auto-increment and ID rules): about half a day for the shape, then a few hours per chip. Only as right as the
  tables.
- **eMMC:** the most valuable (the recorder and the save) and the most work: Renode's SDMMC model targets SD cards, not
  eMMC commands (CMD1, EXT_CSD, switch), so 3 to 5 days for a card model and its timing, with the host simulator's
  `Emmc` as the oracle.
- **Cost of running it:** about 3 s of wall per virtual second with the bridge (a pipe round trip per pump): fine for a
  nightly or branch run, not for check.sh.

## Testing strategy

Modelled on the patch repo's layers (bryton-S510-mods `docs/validation.md`: build checks, Ghidra instruction
review, host tests, Unicorn fuzzing of every patch entry point, a CI gate, then the unit: short test, ride path,
ride). Each layer catches what the others miss; none proves the firmware right on its own.

| Layer | Tool | Catches | Misses |
|-------|------|---------|--------|
| 1. Build checks | fmt, clippy `-D warnings`, `forbid(unsafe_code)`, `tools/pack.py` | undocumented `unsafe`, an image the updater or the bootloader would refuse | behaviour |
| 2. Host tests | unit, simulator (`Sim`, panel model), doc tests, `dev_tests` | logic, the panel protocol bytes, the dev protocol | everything below `Board` |
| 3. Emulation | Renode (`renode/run.py`; `renode/nrf_run.py` with the radio chip's real app) | start-up order, faults Renode models, pixels equal to the simulator, a switched-off core that still runs (`--off`: a pending interrupt waking the standby's WFI, 2026-10-25) | the MPU, caches and attributes the bootloader leaves on, a real WFI wake, SPI and ULPI timing. On 2026-10-22 it passed an image whose SPI1 lost master mode to a mode fault on the unit |
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
