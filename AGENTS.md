# AGENTS.md: how to work on bryton-rustfw

Rules for anyone (person or AI agent) changing this firmware. Keep it short and update it as we learn.

## Golden rule: never lose a ride

It outranks everything below, performance included: on 2026-10-22 one wrong pointer in a stock-firmware patch
(P9) restarted the unit during a save and a 3-hour ride was lost. The stock firmware's own design is the minimum
bar (public repo docs/how-it-works.md, "How a ride is saved"):
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
- **The save is the riskiest moment** (2026-10-22, docs/journal.md): stock's crash at the save deleted the ride in
  progress and zeroed its directory entry, so nothing could be recovered. Keep the ride in progress until the
  finished file is written and read back whole; finish by rename; keep two recovery records; never zero a deleted
  entry.
- **Tested before any ride build:** host tests that cut the write at every point and check the resume, and on
  the unit a start, a restart mid-ride, then a save.

## Share the storage with stock

The owner switches between this firmware and Bryton's (stock or patched), so both must live on one eMMC volume
(the FAT32 "BRYTON" drive; layout in the bryton repo's ghidra/STORAGE_AND_MAPS.md, section 3):
- **Same places, same formats:** rides are FIT files in `\Activities\` with stock's names (`YYMMDDhhmmss.fit`), and
  with stock's summaries (`\System\History\Summary\<name>.sum / .csum`) once their format is known, so either
  firmware, the Bryton app and anything reading the drive sees every ride.
- **Read what stock left:** its rides (power curve, totals, CP and W'), its settings (`\System\*.ini`: profile,
  paired sensors, pages), so a switch loses nothing.
- **Never break it for stock:** never touch stock's ride in progress or its recovery files (`\Activities\tmp.fit`,
  `\System\tmp*.fit`), never format, keep both FAT copies, FSInfo and the volume label consistent; this firmware's
  own state (its ride in progress, its settings file) goes in its own folder, so neither firmware resumes or reads
  the other's half-written data.

## Performance first

This is a battery device: every cycle the CPU is awake costs ride time.

- **Never spin:**
  - waits sleep (`delay_ms` is SysTick + WFI);
  - a busy loop needs a reason, and a timeout.
- **Do less:**
  - **redraw only what changes, in the smallest box that holds it**: a value in its own fixed box, a digit in its
    own cell; never a whole line, a whole screen or a wide clearing box for one changed character. Every pixel is a
    SPI transfer and CPU time, uncached from SDRAM: a full-width redraw of the seconds kept the CPU awake 17.5 %
    of the time, one 30 x 50 digit cell 2.4 % (measured, `tools/dev.py info`). Full redraws only when a screen opens;
  - send the panel the smallest window;
  - no work in the loop that could be done once.
- **Light code:**
  - no heap;
  - no dependencies without a measured need;
  - tables over code (the panel init is data);
  - watch the image size `tools/pack.py` prints: about <!-- n:image -->638 KB<!-- /n --> (stock's is over 3 MB). Size costs little here
    (the image runs from 64 MB of SDRAM): a jump that nothing explains is what to look for (a font costs nothing
    until something draws with it: the linker drops it).
- **Memory:** the stack is DTCM (about 123 KB); the app itself lives in AXI SRAM (a static cell, link.x `.axibss`),
  since as a local it overflowed the stack when a workout opened (2026-10-24). `tools/stack_check.py` (check.sh) fails
  when a function's frame passes its budget: keep big buffers in their own non-inlined function, never in main's. AXI
  SRAM (512 KB) holds the app, the courses, the scan's cache and the road ahead (about <!-- n:axi_free -->15 KB<!-- /n --> left; link.x
  says what is where); a static there must start as all zeros (an `Option` of a reference or a struct with a bool niche
  does not) or it lands in `.data`, in DTCM. The map's SDRAM (link.x MAP) holds the basemap, then the 96 KB the road
  ahead inflates tiles into.
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
- The binary has `#![deny(unsafe_code)]`. The exception is `src/board/hw.rs`, which holds the unavoidable unsafe
  primitives:
  - the vector table and the reset entry (section copies);
  - the fault and panic handler (`default_handler`: the fault marker, then a reset);
  - `rd` / `wr` / `wr8` (volatile register access; the dev link's `peek` / `poke` in usb.rs check the computer's
    addresses before they get there: poke the peripheral space only);
  - `barrier`, `wfi`, `cpsid` (single CPU instructions);
  - `stage_write` and `run_staged` (the USB RAM-run);
  - `PanelDma` (DMA1 stream 0 into SPI1, the panel's pixels): the only code that sets a DMA's memory address, to its
    own two buffers in AXI SRAM (link.x PANELDMA), handed out (`buf`) only while the stream does not read them;
  - `map_memory` (`from_raw_parts_mut`: link.x MAP in SDRAM handed out once as a slice);
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
  marks the fault and resets the unit (hw.rs `default_handler`), so a ride resumes from its checkpoint. The unit
  parks after 3 resets in a row without a minute of running.

## Layout: logic in the library, transport in the board

Folders sort the files; the modules keep their own names (`crate::ambient`, re-exported from lib.rs):
- `src/`: `lib.rs` (the app), `main.rs` (the board and the loop), and nothing else: every other file is in a folder;
- `src/app/`: the app's parts, one `impl App` per concern;
- `src/drivers/`: each chip's or radio's protocol against `Board`;
- `src/ui/`: what is drawn, the menus and the ride pages' layout (`page.rs`);
- `src/training/`: power metrics and curve, pacing, ramp test, HRV, workouts, reminders, the home trainer (pure logic);
- `src/nav/`: the course and its cues, the climbs ahead, the barometric altitude (pure logic);
- `src/link/`: the USB dev link, USB mass storage and the phone link's formats;
- `src/storage/`: FAT, FIT, and what stock left on the volume;
- `src/board/`: the binary's own modules, hw.rs (all the `unsafe`) and usb.rs.
- Each folder's module root is its `mod.rs` (`src/drivers/mod.rs`, ...); lib.rs re-exports the modules of `drivers`,
  `storage`, `ui`, `training`, `nav` and `link` at the crate root, so paths stay `crate::ride`. `src/sim/` is the tests'
  simulator (`Sim`).
- `src/lib.rs`: the `Board` trait and `App`: its state, `App::start`, `App::step` (the main loop's pass). Keep it
  to that: a new concern goes in its own file in `src/app/`, one `impl App` block each; modules are re-exported whole
  from lib.rs, so paths stay `crate::Settings`; a method another file calls is `pub(crate)`.
  - `Board` is the only way to reach hardware. Each method is a primitive: send a panel command, read RTC_TR,
    read the keys, sleep.
  - **If a method needs an `if` about the device's behaviour, that logic belongs in the library.**
  - `App`'s fields are parts, not flags: a concern with several fields is a sub-struct with its own `new` and its
    small pure methods (`app/groups.rs`). A new concern with three or more fields gets its own. `App` must stay
    under the size budget (`tests::app_size_budget`, 64 KB; it lives in AXI SRAM).
- `Rect` (`contains`, `grown`) is the one hit-test: a touch is tested against a `Rect`, never with its own range checks.
- Menus are data (`src/ui/menu.rs`): a new setting is a `Setting` variant, its row in a table, and its arms in
  `Settings::text` / `cycle` / `step`: never a row number. Back pops a screen stack (`App::open` / `close`).
- `src/ui/fonts.rs` is **generated** by `tools/font.py`; never edit it by hand (edit `tools/pixel_font.py`, run
  `python3 tools/font.py`). No layout may depend on the FONT choice (`smooth_fonts_share_the_pixel_cells`).
- Screens act on inputs, never on keys. Every new input path gets its fuzz test (see Tests).
- The ANT+ network key (`tools/ant_key.py`, gitignored `ant_key.bin`, build.rs `ANT_KEY`) and Shimano's (`di2_key.bin`,
  `DI2_KEY`) are licensed: **never** in this repository, a commit, a test or a doc, never printed. Tests use stand-in
  keys.
- What each file holds: docs/layout.md; read the entries for the files you touch.


## AI agents: the right model for the task

Typed agents in `~/.claude/agents/` do the work; the main session orchestrates. Opus (`critical`) only for code where a
mistake costs a ride: storage, the recorder, power, boot, hw.rs and security. One review per feature, and every merge
passes `./check.sh`. Agents never switch the main checkout: worktrees fork from `origin/main`, merges go through
`runner`. A second review only for recorder, storage or boot changes (reviewer on Opus there); a worktree agent merges
main first.

## Before every commit

    ./check.sh

It runs, in order:
- the layout guard: `src/lib.rs` keeps `#![forbid(unsafe_code)]`, and `unsafe` appears only in hw.rs (plus usb.rs's
  marker line), so the library cannot reach a register;
- the docs guard: every file of `src` and `tools` is named in docs/layout.md, and no doc names a retired flat file
  (the list is in check.sh);
- `cargo fmt --check` (120 columns, `rustfmt.toml`);
- `cargo clippy --release -- -D warnings`, and on the tests and the simulator;
- `cargo test-host` (unit tests, simulator tests and doc tests on the Mac; writes `target/sim/preview.bmp`);
- `RUSTDOCFLAGS="-D warnings" cargo doc` (no broken or private links);
- `cargo deny check` (when installed);
- the firmware build;
- `tools/stack_check.py` (the stack frames against their budgets);
- `tools/pack.py`;
- `tools/numbers.py --write` (the markers of the docs, each changed one printed) and `tools/numbers.py selftest`;
- `tools/dev.py selftest`, `tools/sync.py selftest`, `tools/s810_probe.py selftest`, `tools/route_pack.py selftest`;
- `tools/fit_check.py` on the test rides when fitdecode and garmin-fit-sdk are installed (`FIT_PYTHON` may name a venv's
  Python);
- - if Renode is installed: `renode/run.py`, `renode/run.py --vbus` and `renode/run.py --off` (docs/devloop.md,
  Emulation).

Look at `target/sim/preview.bmp` after any drawing change, and at `target/sim/fonts.bmp` (every font, written by
the `font_sheet` test) when choosing a text size.

## Tests

- **Unit tests:** in `#[cfg(test)] mod tests` at the bottom of each file. `src/ui/` tests draw into a framebuffer
  that panics on any off-screen fill.
- **Simulator tests (`src/sim/`):** the whole app runs against `Sim`, a fake `Board`.
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
- **Dev protocol:** `src/sim/dev_tests.rs` drive `Session` with a fake computer (`Pc`).
  - A screenshot must equal the simulated panel.
  - A good image loads.
  - Bad ones are refused (checksum, tag, sizes).
- - **Hardware:** cannot be tested on the host (Renode covers part of it, see docs/devloop.md). Every register access
  names its source: a [BOARD.md](BOARD.md) section or RM0399. A new hardware fact goes into BOARD.md first, as a fact
  (no copied vendor code).
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

- **Every new input path gets its fuzz test** (`src/sim/fuzz.rs`) in the same change: a parser, a file read,
  a radio or USB message, a sample taken somewhere new. Generate the input's structure (sections and keys, ids,
  frames), not only random bytes, and assert the fuzz reaches the new code (a count over the seeds), or it can
  pass without testing anything: random text never made a whole INI section (2026-10-24).
- **A new boot step that touches the core (MPU, caches, clocks, VTOR)** gets a `DIAG_STOP` stage on the unit
  before it is trusted: Renode does not model it.
- **Findings go to both repositories:** a hardware, bootloader, updater or recovery fact learned here also goes
  into bryton-S510-mods (`docs/how-it-works.md`, `docs/recovery.md`, its local FLASHLOG), so the work is not
  redone there; and the other way.

## Docs

- `//!` at the top of each file: what it is for.
- A live number in the docs (a size, a count, a line total) is a `tools/numbers.py` marker, `<!-- n:NAME -->TEXT<!-- /n -->`,
  rewritten by `numbers.py --write`; never typed by hand. Dated numbers (journal, measurements) stay as they were.
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
  - **gzip is our own** (`src/storage/inflate.rs`): `miniz_oxide` was not taken (its Zlib licence is not in
    `deny.toml`, it needs `adler2`, its decoder wants the whole compressed tile in RAM). The need is one small reader,
    tested against Python's gzip and fuzzed, that pulls bytes from the card as `ign::lz4` does.
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

The commands, the profiler, Renode, the testing layers, the run-from-RAM loop, Engineer Mode and the no-watchdog
decision are in docs/devloop.md. One rule stays here, since that doc does not state it:
- **Renode passing proves little:** it does not model the MPU, caches, attributes, a real WFI wake, SPI or ULPI
  timing (it passed an image whose SPI1 lost master mode on the unit, 2026-10-22). The simulator is the main tool.

## Later (when the firmware does more than one thing)

- Embassy for async tasks and timers, instead of the polling loop.
- Logging:
  - there is no SWD probe, so no defmt / RTT;
  - a UART or a log file on the eMMC is the likely channel.
- Integration tests in `tests/` for multi-module behaviour, still on the host.
- CI (GitHub Actions running check.sh).
