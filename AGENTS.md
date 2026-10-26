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
    SPI transfer and CPU time, uncached from SDRAM: redrawing the main screen's two seconds digits over the full
    width each second kept the CPU awake 17.5 % of the time; one 30 x 50 digit cell, 2.4 % (measured, `dev.py
    info`). Full redraws only when a screen opens;
  - send the panel the smallest window;
  - no work in the loop that could be done once.
- **Light code:**
  - no heap;
  - no dependencies without a measured need;
  - tables over code (the panel init is data);
  - watch the image size `pack.py` prints: about 310 KB (2026-10-25; stock's is over 3 MB). Size costs little here
    (the image runs from 64 MB of SDRAM): a jump that nothing explains is what to look for (a font costs nothing
    until something draws with it: the linker drops it).
- **Memory:** the stack is DTCM (about 123 KB); the app itself lives in AXI SRAM (a static cell, link.x `.axibss`),
  since as a local it was held in main's frame several times over (111 KB) and opening a workout overflowed the
  stack (2026-10-24). `tools/stack_check.py` (check.sh) fails when a function's frame passes its budget: keep big
  buffers in their own non-inlined function, never in main's.
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

Folders sort the files; the modules keep their own names (`crate::ambient`, re-exported from lib.rs):
- `src/`: `lib.rs` (the app), `main.rs` (the board and the loop), `ride.rs`, `power.rs`, `dev.rs`, `sim.rs` (tests);
- `src/app/`: the app's parts, one `impl App` per concern;
- `src/drivers/`: each chip's or radio's protocol against `Board`;
- `src/ui/`: what is drawn and the menus;
- `src/storage/`: FAT, FIT, and what stock left on the volume;
- `src/board/`: the binary's own modules, hw.rs (all the `unsafe`) and usb.rs.
- `src/drivers.rs`: the module root of `src/drivers/`.
- `src/storage.rs`: the module root of `src/storage/`.
- `src/board.rs`: the module root of `src/board/`.

- `src/lib.rs`: the `Board` trait and `App`: its state, `App::start`, `App::step` (the main loop's pass). Keep it
  to that: a new concern goes in its own file in `src/app/`.
  - `Board` is the only way to reach hardware. Each method is a primitive: send a panel command, read RTC_TR,
    read the keys, sleep.
  - **If a method needs an `if` about the device's behaviour, that logic belongs in the library.**
- `src/app.rs` and `src/app/`: the rest of the app, each file an `impl App` block (several are allowed in a crate)
  plus its own types and constants. The modules are private and re-exported whole from lib.rs, so paths stay
  `crate::Settings`, `bryton_rustfw::POLL_MS`; App's fields are private to the crate root, which every module can
  read; a method another file calls is `pub(crate)`.
  - `settings.rs`: `Settings`, `PageCfg`, every backup register of the app (`*_BKP`), saving them.
  - `input.rs`: `Key`, `Press`, `Keys`, `Screen`, the screen stack (`open` / `close`), `press`, `input`, `touch`.
  - `editor.rs`: the page editor on the page (`Screen::Edit`, its actions, keep or drop), pages added and deleted,
    and `\Rustfw\pages.bin` (read after the start, written between rides, the backup registers after it).
  - `screens.rs`: `show` (a screen whole), `update_page` (only the boxes that changed), the page widgets, the banner,
    the ride card, the basemap, `render_row` (screenshots). `fields.rs`: a ride field's text and colour, number
    formatting. `graphs.rs`: the graph fields and the course profile.
  - `more.rs`: MORE, a denser variant of each ride page (swipe up shows it, swipe down or Back returns): the rider's own
    (the editor edits it, kept in `pages.bin` after the first CRC) or `Page::more_of`; "MORE" in the strip.
  - `menus.rs`: `Menu`, each menu's rows and their text, `activate` (what a row does).
  - `link.rs`: the Nordic link's watchdog: signs of a framing out of step (garbage frames, commands without a response,
    a quiet link that does not answer a probe), drain and resync, else reset the chip and start ANT and the phone
    link again, with backoff; the count is in the dev link's `info`. Tests: `src/sim/link_tests.rs`.
  - `radio.rs`: the ANT+ channels (`Sensor`), `poll_nrf`, pairing. `gps.rs`: `poll_gnss`, the aiding.
  - `phone.rs`: the phone link over BLE: the GATT server set up (HID, battery, device information), advertising,
    bonding and the bond in `\Rustfw\ble.bin`, the BLE events, the GATT client of the phone (ANCS, AMS, CTS
    discovered one procedure at a time, then its operations queued), media keys, the call and media screens.
  - `notifs.rs`: the phone's last notifications kept (`phone::Inbox`), the NOTIFICATIONS list (a tap on the overview's
    notices, or PHONE in the menu) and a notification's detail.
  - `riding.rs`: Start / pause, the end-ride screen (RESUME, SAVE, DISCARD), the second tick, the recorder glue.
    `stock_data.rs`: the scan of stock's rides and the import of its settings. `workouts.rs`, `courses.rs`: the workout played and the course followed.
  - `device.rs`: backlight, buzzer, status strip, switching off. `clock.rs`: RTC decoding, dates.
  - `drive.rs`: the USB drive: the card lent to the computer (`emmc::Emmc::lend`: one owner at a time, every other
    use refused), when it may be (no ride running or unsaved), taken back on eject, a restart after the computer
    wrote.
- `src/ui.rs`: what is on screen, through the `Canvas` trait (`fill` a rectangle, `blit` one from a pixel
  function). It knows nothing about the panel. Text styles (`LABEL`, `VALUE`) are defined there once.
- `src/ui/menu.rs`: menus as data. `Input` (what a key or a gesture asks for: screens act on inputs, never on keys),
  `Row` (what a menu row is: a submenu, a setting, a sensor, a reading, a page field) and each menu's table,
  `Setting` (label, choice or number with range and step). A new setting is a `Setting` variant, its row in a table,
  and its arms in `Settings::text` / `cycle` / `step`: never a row number. Back pops a screen stack (`App::open` /
  `close`), so no screen knows where Back goes.
- `src/ui/text.rs`: text in the pixel font at a scale (`text::draw`, a `Style`, `Align`), one panel window per glyph.
  For a value that changes, `text::draw_in` (a fixed box, the rest painted with the background) and
  `text::Line` (`write!` into a stack buffer, no heap). Float formatting (`{:.1}`) costs a few KB of image.
- `src/ui/fonts.rs`: glyph data, **generated** by `tools/font.py` (4 bits per pixel); never edit it by hand.
  - `PIXEL`, the UI font: this project's own 5 x 7 dot matrix (`tools/pixel_font.py`, one text grid per glyph),
    drawn at 2x or 4x (`Style::scale`) so it matches Ferris.
  - To change a glyph, edit `tools/pixel_font.py` and run `python3 tools/font.py` (needs PIL).
- `src/drivers/panel.rs`: the ST7789 protocol as data and command bytes (init table, CASET / RASET / RAMWR, off).
- `src/drivers/gnss.rs`: the u-blox receiver's UBX framing, NAV-PVT and the configuration sent (RAM layer only).
  `tools/gnss.py` talks to it from the computer through `peek` / `poke`.
- `src/ride.rs`: the ride (Start / pause / lap / end, auto-pause, totals), the heart-rate and power zones and their
  colours, the history behind the graph field. Pure logic, one tick a second; each change comes out as an `Event`
  for the recorder (`App::recorder`, the one place the FIT writer will plug in).
- `src/page.rs`: a ride page as rows of cells (up to 5 rows of 1 or 2, 8 fields), the layouts as presets, the edits
  (split across or down, merge, taller, shorter, a field) that never leave a field too small to read, and the
  format of `pages.bin` (two copies, seq and CRC). Pure logic.
- `src/power.rs`: power metrics once a second (3 / 10 / 30 s averages, NP, IF, TSS, kJ, kcal, W' balance), running
  sums over a ring, integers only; ride.rs keeps max heart rate and the climbing (VAM, grade). Pure logic.
- `src/ui/card.rs`: a ride's card once it has ended, and for each past ride: its map, elevation profile and ten
  numbers. Drawn once, when shown.
- `src/ui/media.rs`: the media player screen: what the phone says is playing (AMS), a progress bar, touch buttons,
  each part in its own fixed box.
- `src/ui/map.rs`: the map field: the track (fixed array, thinned when full), the local projection, the fitted view,
  and drawing in runs per row clipped to `Canvas::clip`, so a dirty box and a screenshot row use the same code.
- `src/drivers/ambient.rs`: the sensor bus chips (I2C4) against the `Board` primitives `i2c_read` / `i2c_write`: the
  ambient light sensor, the barometer, the accelerometer (switched off), and the automatic backlight (smoothing,
  hysteresis). `sim.rs` has a model of the bus (`Bus`); `tools/i2c.py` reaches it through `peek` / `poke`.
- `src/drivers/touch.rs`: the touch controller (Sitronix, I2C1) against the `Board` primitives `touch_read` / `touch_reset` /
  `touch_int`: its registers, the start, and the gestures (tap, hold, swipe) recognised from the points; `sim.rs`
  has a model of the chip (`TouchChip`) with a scripted finger. `tools/i2c.py --touch` reads it from the computer.
- `src/altitude.rs`: the barometric altitude: the international barometric formula as an integer table, the
  smoothing, the calibration (typed, from the receiver, at a ride's start; `\Rustfw\altcal.bin`'s sector), and which
  source the ride takes (barometer or GPS, with a reference number that makes the ride forget its low point when it
  changes). Pure logic. `src/app/barometer.rs`: its readings (a second while riding), the menu rows and the file.
- `src/pacing.rs`: the climb pacer: the power for each section of a climb that gets to the top soonest with the W' the
  rider has left (Martin's power equation for the speed, a Lagrange multiplier on W' spent above CP). Pure logic;
  `src/app/pace.rs` cuts the followed course's climb into sections, plans again at each one, and draws the PACE gauge
  (`Field::ClimbPace`, the workout gauge's widget).
- `src/alerts.rs`: reminders during a ride (eat and drink every so many minutes, heart rate over a limit), fed a second
  at a time. Pure logic.
- `src/hrv.rs`: heart rate variability from the strap's RR intervals (`sensors::Beats`): RMSSD and DFA a1 (P9's
  algorithm), recomputed every 5 beats. Pure logic.
- `src/drivers/battery.rs`: battery millivolts from the ADC readings, the charge estimate, when to switch off.
- `src/trainer.rs`: the home trainer whatever the link (what is read, what is commanded: ERG, resistance, SIM; when to
  send again; nothing is sent until the rider or a workout or course takes it over). Pure logic.
  `src/drivers/fec.rs`: ANT+ FE-C pages as bytes, sources in its module doc; `src/drivers/ftms.rs`: its Bluetooth
  sibling, FTMS (advertising, Indoor Bike Data, Control Point, status) as bytes.
  `src/drivers/fecble.rs`: the other Bluetooth protocol, FE-C over BLE (trainers without FTMS, the D500): the FE-C
  pages in ANT message frames over the vendor service 6e40fec1, the framing and checksum, and what the advertising
  says (`Proto`: FTMS wins when both).
  `src/app/ftms.rs`: the Bluetooth trainer's link, the unit as a central beside the phone's link: the search from
  SENSORS (BT TRAINER, each trainer with its protocol), connect with backoff, discovery, subscriptions, Request
  Control, the commands on the Control Point (one procedure at a time), its readings into the trainer's; for FE-C
  over BLE the vendor UUID base is registered once per chip start, no Request Control, and app/trainer.rs writes
  the ANT+ path's pages as frames, one write in flight; ANT+ first when both are paired, both kept in
  `\Rustfw\sensors.bin`. Tests: `src/sim/ftms_tests.rs` and `src/sim/fecble_tests.rs` against `ble_sim.rs`'s `FtmsSim` (`new` and `new_fec`).
  `src/app/trainer.rs`: the glue (readings into `Live` where no meter or sensor is heard, the commands sent as
  acknowledged messages, the workout's power step and the course's grade as drives) and the trainer screen and
  fields; `src/ui/trainer.rs` draws the screen. One acknowledged message per 250 ms on its channel; its pairing is in
  `\Rustfw\sensors.bin` (backup register 14 is the shifting systems').
- `src/drivers/sensors.rs`: ANT+ sensor kinds (stock's channels), decoding their broadcasts, the live values the ride
  pages show. Pure logic.
- `src/drivers/nrf.rs`: the Nordic BLE / ANT chip's serialization protocol (framing, commands, start-up), against the
  `Board` byte primitives `nrf_reset` / `nrf_send` / `nrf_recv`; `sim.rs` has a model of the chip (`Nordic`). BLE
  events that arrive while a command waits are kept (`Rx::pop`); the main loop acts on events only once the bytes
  read are framed (a command sent mid-read would take the rest of the stream out of order).
- `src/drivers/ble.rs`: the SoftDevice API v5 calls of the phone link (GAP peripheral, GATT server and client), the
  central's (connect, cancel, scan stop) and their events, each with its connection, serialized as the nRF5 SDK's
  codecs do (sources in the module doc); `nrf::start` configures two links (`sd_ble_cfg_set`) and falls back to the
  defaults when refused; `sim/ble_sim.rs` models the chip's side, a scripted phone and a trainer (FTMS or FE-C over BLE).
- `src/phone.rs`: the phone link's formats, pure logic: the HID report map, ANCS, AMS, CTS, the bond's sector.
- `src/storage/fit.rs`: the ride file (FIT activity), written as it goes against a `Sink` (write at an offset, set the
  length, flush); a checkpoint leaves a valid file and returns the recovery record, which resumes it after a reset.
  `tools/fit_check.py` decodes the test ride with independent decoders (fitdecode, Garmin's FIT SDK), the full copy's
  developer fields included.
- `src/drivers/emmc.rs`: the eMMC (SDMMC1) protocol: identification, EXT_CSD SEC_COUNT, block reads and writes
  (writes refused at or past sector B, stock's reserved area), stock's layout; against the `Board` primitives
  `emmc_power_on` / `emmc_bus` / `emmc_cmd` / `emmc_read` / `emmc_write`; `sim.rs` has a model of the card
  (`EmmcCard`, which serves a FAT image when given one and can lose power after any sector written). Used by the
  basemap, the recorder and the dev link's `emmc` and `sector` commands; identified at first use (at the first main loop pass, for the scan's cache of
  stock's rides; at boot at the first start, for stock's settings, or when a ride is open).
- `src/storage/fat.rs`: a FAT32 reader: boot sector, chains, 8.3 and long names, files at an offset, directory
  listings; every loop bounded. Tested on `test-data/fat32.sparse`, a volume macOS made (`tools/fat_image.py`,
  which also puts made-up stock rides and settings files on it). `src/storage/fat/write.rs`: the write path, only
  what the recorder needs (create with a long name, write sectors, set the size, move by delete + link, mark
  deleted with 0xe5 only, sync the FATs and FSInfo), each write ordered so a power cut leaves a volume stock and a
  computer accept; `fsck` in its tests checks that.
- `src/storage/recorder.rs`: the ride recorder (the golden rule): the FIT file in `\Rustfw\Rides` from RAM to the card at
  each checkpoint (a minute at most), two recovery records in `\Rustfw\ride.rec` (3 KB), resume at boot, the save to
  `\Activities\YYMMDDhhmmss.fit`. Its tests cut the power after every sector of a whole ride and of the reboot.
  Two recorders run per ride (`Kind`): the clean copy above, and the full one (`Alpha1` / `RMSSD` developer fields,
  `\Rustfw\Full`, `\Rustfw\full.rec`, saved to `\Rustfw\Activities`), which never holds the clean one up
  (`App::step_full`, docs/sync.md). A ride discarded at its end goes through the same save into
  `\Rustfw\Discarded` (`Discarded\Full`), its records saying so, so no cut puts it in `\Activities`.
- `src/storage/fitread.rs`: a FIT reader, one byte at a time (no buffer): records (time, power, HR, distance; never
  the position) and the session, the CRCs checked. `src/storage/ini.rs`: a tiny INI reader.
- `src/storage/stock.rs`: what stock left on the volume: the scan of its rides (a bounded amount per call, spread
  over the main loop's passes, each pass bounded by `Board::awake_us`) and the import of its settings. Its cache
  (`Cache`, `\Rustfw\scan.bin`, 256 KB in AXI SRAM from main.rs `make_scan_cache`): an entry of 64 bytes per ride
  (totals, history, best powers), so each ride is decoded once ever and a start reads only new or changed rides. `src/power_curve.rs`: best powers per duration in
  one pass, merged across rides, and the 2-parameter CP / W' fit. Pure logic.
- `src/storage/trash.rs`: HISTORY's DELETE: a ride moved (never erased) into `\Rustfw\Deleted` (its full copy
  into `\Rustfw\Deleted\Full`), `\Rustfw\delete.rec` naming the move so the next start finishes one a cut left;
  stock's `.sum` / `.csum` left as they are (module doc: why).
- `src/storage/history.rs`: the ride history: the index of stock's rides (filled by that scan, newest first) and a
  past ride's card, its file read once with the position (`fitread::Reader::with_position`).
- `src/storage/workout.rs`: stock's planned workouts (`\Workout\calendar\YYYYMMDD\*.fit`): a FIT workout file read
  whole (name, steps, targets, repeats written out), the calendar from today on. `src/workout.rs`: the player, pure
  logic fed one second at a time (the step running, time left, 3-2-1 beeps, Lap to the next step).
- `src/storage/course.rs`: course files, read only: stock's `\PlanTrip\*.fit` (Bryton's own route layout, in the
  module doc), FIT courses and GPX (a small streaming reader) in `\Courses`; the list, and `Courses`, the course
  memory (about 70 KB, a static cell in AXI SRAM from main.rs `make_courses`). `src/course.rs`: the course model,
  pure logic: points thinned to 4096, where the rider is (a moving window, a bounded search when lost),
  off course, ascent left, the climbs (Garmin ClimbPro's definition), the profile's columns. `src/course/cues.rs`:
  turn-by-turn cues (FIT course points, GPX turn words, else the turns of the course's shape), the next one said.
- `src/ui/tiles.rs`: the basemap: stock's cached map tiles (`\Map\Data\Temp\Zoom<z>`) read from the FAT volume,
  drawn once per view into a pixel buffer in SDRAM (`hw::map_memory`), which map.rs copies to the panel and
  restores from under the arrow and the track; IGN's raster tiles first where the owner's packs have them.
- `src/storage/ign.rs`: the IGN map packs (`\Rustfw\Maps\IGN<z>_<bx>_<by>.PAK`, the format in its module doc),
  made on the computer by `tools/ign_pack.py` (fetch, pack, the synthetic `fixture` in `test-data/ign`); the LZ4
  block decoder. Real IGN tiles never enter the repository.
- `src/dev.rs`: the USB dev link protocol (commands, screenshot rows, image load and checks), against the
  `Host` trait.
- `src/msc.rs`: USB mass storage, SCSI over Bulk-Only Transport (the commands a computer needs to mount the FAT
  volume, the 13 cases by padding IN and taking all of OUT, sense data), against the `Pipe` (bulk endpoints) and
  `Media` (sectors: `emmc::Lent`) traits. Its own code, not `usbd-storage` (module doc: why).
- `src/sim.rs` (tests only): a simulated board (`Sim`) and computer (`Pc`), see Tests. Its tests are in `src/sim/`:
  `tests.rs` (the app on the Sim), `page_tests.rs` (the page editor and `pages.bin`, cut at every sector),
  `dev_tests.rs`, `drive_tests.rs`, `emmc_tests.rs`, `fuzz.rs`, `recorder_tests.rs`, `altitude_tests.rs` (the barometer's
  climb, calibrations and file),
  `phone_tests.rs` (the phone link against `ble_sim.rs`: the chip's BLE side and a scripted phone).
- `src/sim/shift_tests.rs`: electronic shifting on the Sim: a Di2 and an ANT+ shifting system paired, the shifts
  counted while riding, a system that goes silent.
- `src/main.rs`: `impl Board for S510` with registers, and the main loop. Clocks, GPIO, SPI byte transport,
  RTC read and set, standby, SysTick sleep. No app logic.
- `src/board/usb.rs`: OTG_HS and the ULPI PHY brought up as stock does it, and a composite device: the CDC serial
  port (it implements `Host`) and the mass storage interface (its class requests, and `msc::Pipe` over its bulk
  endpoints, each wait 1 s at most).
- `src/board/hw.rs`: the unsafe primitives (above).
- `dev.py`: the computer side of the dev link. `renode/`: the emulator platform and checks.
- `tools/sync.py`: the computer's sync (docs/sync.md): rides from the mounted drive or the dev link (dev.py `ls` /
  `get`) to Intervals.icu (full copy) and Strava (clean copy), never twice; planned workouts from Intervals.icu to
  `\Workout\calendar`. Dry run unless `--go`; keys and tokens only in `~/.config/bryton-rustfw/` (0600), never in
  the repository. `sync.py selftest` (check.sh) runs against a fake server on 127.0.0.1.
- `tools/nrf.py`: talks to the Nordic chip from the computer through `peek` / `poke` (BOARD.md 11), to explore the
  protocol before the firmware has a driver. Only with a build that leaves USART2 alone (the installed one, or
  after `dev.py reset`): a firmware that reads the link takes the bytes first.
- `tools/progress.py`: a progress bar when tqdm is installed, nothing otherwise (cosmetic).
- `tools/fetch_firmware.py`: downloads the stock R039 firmware from Bryton's update server and checks it against
  SHA256SUMS.
- `tools/stack_check.py`: the firmware's stack frames against their budgets (check.sh, after the build).
- `tools/iaprecover.py`: recovery through the bootloader's USB mode: read back the OS area, or rewrite it (sudo).
- `build.rs`, `tools/ant_key.py`: the ANT+ network key. It is licensed, so it is **never** in this repository, in a
  commit, a test or a doc: `tools/ant_key.py` copies it from the owner's stock image into the gitignored
  `ant_key.bin`, and build.rs passes it to main.rs as `ANT_KEY` (None without the file). Shimano's private network
  key (the Di2 channel) is handled the same way: `di2_key.bin`, `DI2_KEY`, never printed, no hash of it kept.
  Tests use stand-in keys. `tools/font.py`, `tools/pixel_font.py`: the fonts.
- `link.x`, `pack.py`: memory map and the update file. `pack.py` re-checks what the stock updater checks.

## Before every commit

    ./check.sh

It runs, in order:
- the layout guard: `src/lib.rs` keeps `#![forbid(unsafe_code)]`, and `unsafe` appears only in hw.rs (plus usb.rs's
  marker line), so the library cannot reach a register;
- `cargo fmt --check` (120 columns, `rustfmt.toml`);
- `cargo clippy --release -- -D warnings`, and on the tests and the simulator;
- `cargo test-host` (unit tests, simulator tests and doc tests on the Mac; writes `target/sim/preview.bmp`);
- the firmware build;
- `pack.py`;
- `dev.py selftest`, `tools/sync.py selftest`;
- `tools/fit_check.py` on the test rides when fitdecode and garmin-fit-sdk are installed (`FIT_PYTHON` may name a venv's
  Python);
- if Renode is installed: `renode/run.py`, `renode/run.py --vbus` and `renode/run.py --off` (Emulation).

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
| 3. Emulation | Renode (`renode/run.py`) | start-up order, faults Renode models, pixels equal to the simulator, a switched-off core that still runs (`--off`: a pending interrupt waking the standby's WFI, 2026-10-25) | the MPU, caches and attributes the bootloader leaves on, a real WFI wake, SPI and ULPI timing. On 2026-10-22 it passed an image whose SPI1 lost master mode to a mode fault on the unit on the unit |
| 4. Fuzzing | `sim.rs` `fuzz`: pseudo-random bytes (xorshift, seeded: failures reproduce) into the Nordic packet and UBX parsers with every decoder, into `dev::Session` with command words mixed in, into the FIT reader and the INI import, CBWs and command blocks into the USB drive (`msc.rs`), BLE events into the phone link (`ble::event`, `App::phone_event` on the Sim, and the ANCS / AMS / CTS formats) and into the trainer's (FTMS data, status and responses, FE-C over BLE frames, connections mixed), and a damaged volume under the ride scan, random page edits, random and damaged `pages.bin` copies and keys and taps in the page editor and on the end-ride screen; a host test, no new dependency | panics and out-of-range reads on input from the radio chip, the GNSS receiver, the cable and the files on the card (it would have caught `ant_closed` indexing a short packet) | logic errors that do not panic |
| 5. Gate | `check.sh` before every commit | a commit that skips layers 1 to 3 | |
| 6. On the unit, boot | first the dev link (`dev.py peek` / `poke`, `run`) when USB comes up. Before that, a `DIAG_STOP=N` build: the checkpoint at boot stage N waits for USB power and resets; power on, hold Page, plug in: Engineer Mode means stage N was reached (control: an image without it gives none). Written from Engineer Mode with `tools/iaprecover.py --write`. The backlight is no readout (BOARD.md) | where a boot stops when nothing else is visible | anything after the panel is up |
| 7. On the unit, features | docs/status.md "Not yet seen on the unit", the dev link once it runs | the hardware facts the layers above stub | |

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
