# AGENTS.md: how to work on bryton-rustfw

Rules for anyone (person or AI agent) changing this firmware. Keep it short and update it as we learn.

## Golden rule: never lose a ride

It outranks everything below, performance included. Learned the hard way on 2026-10-22: one wrong pointer in a
stock-firmware patch (P9) restarted the unit at the start and the save of a ride, and the whole 3 hours were
lost. The stock firmware's own design is the minimum bar (public repo docs/how-it-works.md, "How a ride is
saved"):
- **On the eMMC as it goes:** ride data is appended to the file in chunks; it never exists only in RAM.
- **A checkpoint at least every 60 s:** flush the data, then write a recovery record (file length, CRC, ride
  totals). Keep two copies (old / new) so a power cut while writing one leaves the other whole.
- **Resume on boot:** a recording in progress at boot (fault, panic, power cut) reopens at the last checkpoint
  and carries on. At most the last chunk is lost.
- **The file is valid at every checkpoint:** a reader can use it even if it was never finalized, and a new
  definition goes with the data that needs it.
- **Nothing optional can stop the recording:** no `unwrap`, indexing that can panic, or unbounded loops on the
  recording path. Extras (live fields, HRV, analysis) feed the recorder through checked data, and a failing extra
  is dropped instead of taking the recorder down.
- **Tested before any ride build:** host tests that cut the write at every point and check the resume, and on
  the unit a start, a restart mid-ride, then a save.

## Performance first

This is a battery device: every cycle the CPU is awake costs ride time.

- **Never spin:**
  - waits sleep (`delay_ms` is SysTick + WFI);
  - a busy loop needs a reason, and a timeout.
- **Do less:**
  - **redraw only what changes, in the smallest box that holds it**: a value in its own fixed box, a digit in its
    own cell; never a whole line, a whole screen or a wide clearing box for one changed character. Every pixel is a
    SPI transfer and CPU time, uncached from SDRAM: redrawing the main screen's two seconds digits over the full
    width each second kept the CPU awake 17.5 % of the time; one 30 x 50 digit cell, 2.4 % (measured, `dev.py
    info`). Full redraws only when a screen opens;
  - send the panel the smallest window;
  - no work in the loop that could be done once.
- **Light code:**
  - no heap;
  - no dependencies without a measured need;
  - tables over code (the panel init is data);
  - watch the image size `pack.py` prints: about 48 KB, of which about 20 KB is the USB stack and about 2 KB the
    pixel font (a font costs nothing until something draws with it: the linker drops it).
- **USB costs power** (VOS1, the PHY, the core), so it runs only while a cable is in: started when PC13 goes
  high, and unplugging resets the unit, which brings it up without USB.
- **Measure before and after** a change that could cost: image size, and on the unit, time awake per loop
  (DWT cycle counter). Write the number in the commit message.
- **Next steps down in power, in order of gain:**
  - stop mode between seconds, woken by the RTC or LPTIM instead of WFI with the core clock running;
  - a lower CPU clock when idle;
  - SPI by DMA so the CPU sleeps during a redraw;
  - the panel's partial or idle modes.

## Safe Rust only

The point of this project is a firmware in safe Rust.

- `src/lib.rs` has `#![forbid(unsafe_code)]`: no `unsafe` there, ever.
- The binary has `#![deny(unsafe_code)]`. The exception is `src/hw.rs`, which holds the unavoidable unsafe
  primitives:
  - the vector table and the reset entry (section copies);
  - `rd` / `wr` / `wr8` (volatile register access);
  - `barrier`, `wfi`, `cpsid` (single CPU instructions);
  - `stage_write` and `run_staged` (the USB RAM-run);
  - the empty critical-section implementation the USB crates need. It is sound only because interrupts are
    never unmasked: keep it that way, or replace it.
- One other `unsafe` line: `unsafe impl UsbPeripheral` in `usb.rs`. It is the crate's marker trait, which
  states that the register address is right.
- Bare metal cannot avoid these: crates such as cortex-m and the PACs only hide the same `unsafe` inside
  their own code.
- Every `unsafe` block carries a `// SAFETY:` comment saying why it is sound (clippy `undocumented_unsafe_blocks`).
- **Adding to hw.rs** must be a deliberate, reviewed step. The new primitive must be small, with one
  purpose, and must expose a safe interface. Never add `unsafe` elsewhere to get around this.
- **Rules that follow from it:**
  - No `static mut`. State lives in values owned by `main` (see `App`).
  - The two `'static` USB buffers are `StaticCell`s, which are initialised once and checked.
  - Raw pointers appear only in hw.rs.
- **Panics:** no `unwrap` / indexing that can fail on the firmware path, unless it is provably in range. A panic
  parks the CPU (the panic handler), so the screen freezes.

## Layout: logic in the library, transport in the board

- `src/lib.rs`: the `Board` trait, the app (`App::start`, `App::step`), RTC decoding, keys.
  - `Board` is the only way to reach hardware. Each method is a primitive: send a panel command, read RTC_TR,
    read the keys, sleep.
  - **If a method needs an `if` about the device's behaviour, that logic belongs in the library.**
- `src/ui.rs`: what is on screen, through the `Canvas` trait (`fill` a rectangle, `blit` one from a pixel
  function). It knows nothing about the panel. Text styles (`LABEL`, `VALUE`) are defined there once.
- `src/text.rs`: anti-aliased proportional text (`text::draw`, a `Style`, `Align`), one panel window per glyph.
  For a value that changes, `text::draw_in` (a fixed box, the rest painted with the background) and
  `text::Line` (`write!` into a stack buffer, no heap). Float formatting (`{:.1}`) costs a few KB of image.
- `src/fonts.rs`: glyph data, **generated** by `tools/font.py` (4 bits per pixel); never edit it by hand.
  - `PIXEL`, the UI font: this project's own 5 x 7 dot matrix (`tools/pixel_font.py`, one text grid per glyph),
    drawn at 2x or 4x (`Style::scale`) so it matches Ferris.
  - `SANS_16`, `SANS_BOLD_32`: DejaVu Sans, anti-aliased, if smooth text is ever wanted.
  - To change a glyph or add a font, edit those files and run `python3 tools/font.py` (needs PIL).
- `src/panel.rs`: the ST7789 protocol as data and command bytes (init table, CASET / RASET / RAMWR, off).
- `src/gnss.rs`: the u-blox receiver's UBX framing, NAV-PVT and the configuration sent (RAM layer only).
  `tools/gnss.py` talks to it from the computer through `peek` / `poke`.
- `src/ride.rs`: the ride (Start / pause / lap / end, auto-pause, totals), the heart-rate and power zones and their
  colours, the history behind the graph field. Pure logic, one tick a second; each change comes out as an `Event`
  for the recorder (`App::recorder`, the one place the FIT writer will plug in).
- `src/map.rs`: the map field: the track (fixed array, thinned when full), the local projection, the fitted view,
  and drawing in runs per row clipped to `Canvas::clip`, so a dirty box and a screenshot row use the same code.
- `src/ambient.rs`: the sensor bus chips (I2C4) against the `Board` primitives `i2c_read` / `i2c_write`: the
  ambient light sensor, the barometer, the accelerometer (switched off), and the automatic backlight (smoothing,
  hysteresis). `sim.rs` has a model of the bus (`Bus`); `tools/i2c.py` reaches it through `peek` / `poke`.
- `src/battery.rs`: battery millivolts from the ADC readings, the charge estimate, when to switch off.
- `src/sensors.rs`: ANT+ sensor kinds (stock's channels), decoding their broadcasts, the live values the ride
  pages show. Pure logic.
- `src/nrf.rs`: the Nordic BLE / ANT chip's serialization protocol (framing, commands, start-up), against the
  `Board` byte primitives `nrf_reset` / `nrf_send` / `nrf_recv`; `sim.rs` has a model of the chip (`Nordic`).
- `src/fit.rs`: the ride file (FIT activity), written as it goes against a `Sink` (write at an offset, set the
  length, flush); a checkpoint leaves a valid file and returns the recovery record, which resumes it after a reset.
  `tools/fit_check.py` decodes the test ride with independent decoders (fitdecode, Garmin's FIT SDK).
- `src/emmc.rs`: the eMMC (SDMMC1) protocol, read only: identification, EXT_CSD SEC_COUNT, block reads, stock's
  layout; against the `Board` primitives `emmc_power_on` / `emmc_bus` / `emmc_cmd` / `emmc_read`; `sim.rs` has a
  model of the card (`EmmcCard`). Used only by the dev link's `emmc` and `sector` commands so far.
- `src/dev.rs`: the USB dev link protocol (commands, screenshot rows, image load and checks), against the
  `Host` trait.
- `src/sim.rs` (tests only): a simulated board (`Sim`) and computer (`Pc`), see Tests.
- `src/main.rs`: `impl Board for S510` with registers, and the main loop. Clocks, GPIO, SPI byte transport,
  RTC read, standby, SysTick sleep. No app logic.
- `src/usb.rs`: OTG_HS and the ULPI PHY brought up as stock does it, and the CDC serial port. It implements `Host`.
- `src/hw.rs`: the unsafe primitives (above).
- `dev.py`: the computer side of the dev link. `renode/`: the emulator platform and checks.
- `tools/nrf.py`: talks to the Nordic chip from the computer through `peek` / `poke` (BOARD.md 11), to explore the
  protocol before the firmware has a driver. Only with a build that leaves USART2 alone (the installed one, or
  after `dev.py reset`): a firmware that reads the link takes the bytes first.
- `build.rs`, `tools/ant_key.py`: the ANT+ network key. It is licensed, so it is **never** in this repository, in a
  commit, a test or a doc: `tools/ant_key.py` copies it from the owner's stock image into the gitignored
  `ant_key.bin`, and build.rs passes it to main.rs as `ANT_KEY` (None without the file). Tests use a stand-in key. `tools/font.py`, `tools/pixel_font.py`: the fonts.
- `link.x`, `pack.py`: memory map and the update file. `pack.py` re-checks what the stock updater checks.

## Before every commit

    ./check.sh

It runs, in order:
- `cargo fmt --check` (120 columns, `rustfmt.toml`);
- `cargo clippy --release -- -D warnings`;
- `cargo test-host` (unit tests, simulator tests and doc tests on the Mac; writes `target/sim/preview.bmp`);
- the firmware build;
- `pack.py`;
- `dev.py selftest`;
- if Renode is installed: `renode/run.py` and `renode/run.py --vbus` (Emulation).

Look at `target/sim/preview.bmp` after any drawing change, and at `target/sim/fonts.bmp` (every font, written by
the `font_sheet` test) when choosing a text size.

## Tests

- **Unit tests:** in `#[cfg(test)] mod tests` at the bottom of each file. `ui.rs` tests draw into a framebuffer
  that panics on any off-screen fill.
- **Simulator tests (`sim.rs`):** the whole app runs against `Sim`, a fake `Board`.
  - **The panel model** checks the command bytes as the real ST7789 would see them: powered, pixel format,
    RAMCTRL routing, window bounds, pixel count. It paints its framebuffer from RAMWR.
  - **Time:** the RTC advances with the delays.
  - **Keys** are a function of time.
  - **Example:** `ticks_through_midnight` runs 3 simulated seconds in a millisecond.
  - **Screenshots:** `Sim::save(name)` writes one. `Sim::record(name)` writes a frame after every step that
    changed the screen (`target/sim/<name>/00000.bmp`...). Use that to debug drawing over time.
  - Test behaviour, not implementation: what is on screen, what a key sequence does.
  - When a bug is found, write the test that catches it first.
- **Doc tests:** small examples in `///` comments of pure functions; they double as documentation.
- **Dev protocol:** `sim.rs` `dev_tests` drive `Session` with a fake computer (`Pc`).
  - A screenshot must equal the simulated panel.
  - A good image loads.
  - Bad ones are refused (checksum, tag, sizes).
- **Hardware:** cannot be tested on the host (Renode covers part of it, see Emulation). Every register access names its source: a
  [BOARD.md](BOARD.md) section or RM0399. A new hardware fact goes into BOARD.md first, as a fact (no copied
  vendor code).
  - Guesses are marked "inferred".
  - With a diagram where it helps: the pin map (BOARD.md 4) gets every new pin, and a sequence where order or
    timing matters (power-up, reset, a bus transfer, a key) gets a timing diagram in plain text, signal names on
    the left, `~` high, `_` low, times between `|<-` and `->|`. Plain text renders anywhere and diffs well.
  - What still needs the unit is listed in docs/status.md, "Not yet seen on the unit"; update that list after
    each run on the unit.
- **When a driver is added** (GNSS, Nordic, eMMC), do it the same way:
  - its protocol goes in the library, against a `Board` primitive;
  - the simulator gets a model of the chip.

  This is what embedded-hal plus embedded-hal-mock do in the wider ecosystem.

## Testing strategy

Modelled on the patch repo's layers (bryton-S510-mods `docs/validation.md`: build checks, Ghidra instruction
review, host tests, Unicorn fuzzing of every patch entry point, a CI gate, then the unit: short test, ride path,
ride). Each layer catches what the others miss; none proves the firmware right on its own.

| Layer | Tool | Catches | Misses |
|-------|------|---------|--------|
| 1. Build checks | fmt, clippy `-D warnings`, `forbid(unsafe_code)`, `pack.py` | undocumented `unsafe`, an image the updater or the bootloader would refuse | behaviour |
| 2. Host tests | unit, simulator (`Sim`, panel model), doc tests, `dev_tests` | logic, the panel protocol bytes, the dev protocol | everything below `Board` |
| 3. Emulation | Renode (`renode/run.py`) | start-up order, faults Renode models, pixels equal to the simulator | the MPU, caches and attributes the bootloader leaves on, a real WFI wake, SPI and ULPI timing. On 2026-10-22 it passed an image whose SPI1 lost master mode to a mode fault on the unit on the unit |
| 4. Fuzzing | `sim.rs` `fuzz`: pseudo-random bytes (xorshift, seeded: failures reproduce) into the Nordic packet and UBX parsers with every decoder, and into `dev::Session` with command words mixed in; a host test, no new dependency | panics and out-of-range reads on input from the radio chip, the GNSS receiver and the cable (it would have caught `ant_closed` indexing a short packet) | logic errors that do not panic |
| 5. Gate | `check.sh` before every commit | a commit that skips layers 1 to 3 | |
| 6. On the unit, boot | first the dev link (`dev.py peek` / `poke`, `run`) when USB comes up. Before that, a `DIAG_STOP=N` build: the checkpoint at boot stage N waits for USB power and resets; power on, hold Page, plug in: Engineer Mode means stage N was reached (control: an image without it gives none). Written from Engineer Mode with `tools/iaprecover.py --write`. The backlight is no readout (BOARD.md) | where a boot stops when nothing else is visible | anything after the panel is up |
| 7. On the unit, features | docs/status.md "Not yet seen on the unit", the dev link once it runs | the hardware facts the layers above stub | |

- **A new boot step that touches the core (MPU, caches, clocks, VTOR)** gets a `DIAG_STOP` stage on the unit
  before it is trusted: Renode does not model it.
- **Findings go to both repositories:** a hardware, bootloader, updater or recovery fact learned here also goes
  into bryton-S510-mods (`docs/how-it-works.md`, `docs/recovery.md`, its local FLASHLOG), so the work is not
  redone there; and the other way.

## Docs

- `//!` at the top of each file: what it is for.
- `///` on every public item (`#![warn(missing_docs)]` in lib.rs).
- Comments say why, and where a value comes from; the code says what.
- `cargo doc --target aarch64-apple-darwin --open` renders the library docs.

## Public repository: no personal data

This repository is public. Never commit what locates or identifies the owner: GPS coordinates, the saved last
position, screenshots of the GPS screen or map taken on the unit, map tiles or tile numbers of their area, sensor
serial numbers paired with a place. Test data is synthetic; real data stays in ignored tests that read it from
outside the repository. The ANT+ key never enters it either (docs/install.md).

## Look it up, don't recall it

Before writing a protocol, a format, an algorithm or a driver from memory, check primary sources online
(datasheets, reference manuals, the official spec, the reference implementation) and well-known crates or
libraries that already do it; use or adapt them when they fit (no_std, no heap, small, safe). Cite what was used
(URL and what was taken) in the module doc. Something that could not be checked is marked "inferred".

## Toolchain and dependencies

- Rust is pinned in `rust-toolchain.toml` (1.92, edition 2024, thumbv7em-none-eabihf). Raise it on purpose, in
  its own commit.
- `Cargo.lock` is committed (this is a binary).
- **Dependencies:** only the USB stack (usb-device, usbd-serial's `CdcAcmClass`, synopsys-usb-otg, static_cell,
  critical-section).
  - Each new one needs a reason and must be a well-known embedded crate (embassy, heapless, embedded-hal).
  - Its own `unsafe` is part of what we trust.
  - `cargo deny check` in check.sh (when installed) checks advisories, licences and duplicates.
- **No `usbd-serial::SerialPort`:** it is hardcoded to 64-byte packets, which is full speed only. On this
  high-speed link, `usb.rs` uses `CdcAcmClass` with 512-byte packets.
- Release profile: `opt-level = "s"`, LTO, `panic = "abort"`, debug info kept in the ELF only.
- `+strict-align` (.cargo/config.toml): no unaligned accesses, since unaligned reads from the SDRAM return wrong
  bytes on the unit (BOARD.md 2). Keep it until the D-cache is on.

## Project rules

- **Unit safety:**
  - Never flash without the owner present.
  - Never touch IAP.bin or the bootloader.
  - The way back is Engineer Mode (cable in **before** the four-button restart) + `tools/iaprecover.py
    --write-vanilla` (docs/install.md). `--write out/IMG.bin` writes one of our builds the same way, checked first.
  - Record every write to the unit in bryton-S510-mods' FLASHLOG.
  - **The USB link to the unit can drop** (2026-10-22, see that repo's `docs/recovery.md`): copy, then verify
    by reading back; never trust a copy that was not compared.
- **Evidence:** hardware facts come from the reference manual or from reverse engineering the stock firmware
  (Ghidra) for interoperability. Claims are checked against the binary or marked "inferred". Nothing from
  the stock firmware is copied into this repository except hardware facts (BOARD.md).
- **Wording:**
  - "bike computer", never "watch";
  - no em or en dashes in any text;
  - commit messages end with the session trailers.

## Development loop on the unit

Install the firmware once (docs/install.md). From then on, with the cable in and the green dot at the top right:

    ./dev.py info                 # version, uptime
    ./dev.py shot -o shot.png     # screenshot (what the app drew, rendered row by row: no framebuffer)
    ./dev.py watch -i 1           # continuous screenshots into target/shots/
    ./check.sh && ./dev.py run    # run the new build from RAM: nothing written to the eMMC
    ./dev.py peek 0x40013014 2    # read registers, or DTCM / SDRAM (read only); poke ADDR VALUE writes a register
    ./dev.py key page [--long]    # press or hold a key (page, lap, start, back): scripted tests
    ./dev.py emmc                 # eMMC layout; sector N [COUNT] [-o F] reads sectors (read only)
    ./dev.py reset                # back to the installed firmware (so does unplugging, or a power cycle)

- **`run`** stages the image in SDRAM (0xd0800000), checks it as `pack.py` does, and starts it. The start goes
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
  - an eMMC `reflash` command (the driver is read only so far).
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

## Later (when the firmware does more than one thing)

- Embassy for async tasks and timers, instead of the polling loop.
- Logging:
  - there is no SWD probe, so no defmt / RTT;
  - a UART or a log file on the eMMC is the likely channel.
- Integration tests in `tests/` for multi-module behaviour, still on the host.
- CI (GitHub Actions running check.sh) if the project goes public.
