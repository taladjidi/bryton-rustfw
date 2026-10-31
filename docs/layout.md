# Layout: what each file holds

What each file holds. The rules (what goes where, the `App` budget, the `Board` primitives) are in AGENTS.md,
"Layout". Read the entries for the files you touch.

- `src/lib.rs`: the `Board` trait and `App` (its state, `App::start`, `App::step`). The app's cell is <!-- n:app -->64 KB<!-- /n --> in AXI SRAM, with <!-- n:axi_free -->52 KB<!-- /n --> free after the cells (sizes: `tools/numbers.py`).
- `src/board/mod.rs`: the binary's side of the board: the unsafe primitives (hw.rs, the only `unsafe` in the firmware) and the USB link (usb.rs); `main.rs` implements `Board` with them.
- `src/drivers/mod.rs`: the chips and radios, each one's protocol against the `Board` primitives (panel, touch, sensor bus, battery, GNSS, the Nordic chip, the ANT+ sensors, the eMMC).
- `src/link/mod.rs`: the links to the outside, as bytes: the USB dev link, USB mass storage, the phone link's formats, and the profiler.
- `src/nav/mod.rs`: where the rider is and where the route goes: the course and its cues, the climbs ahead, the barometric altitude (pure logic).
- `src/storage/mod.rs`: files: the FAT32 volume on the eMMC, the FIT ride file, the recorder, the IGN map packs, and what stock left on the volume.
- `src/training/mod.rs`: training logic, all pure: power metrics and the curve, pacing, the ramp test, HRV, workouts, reminders, the home trainer.
- `src/app/mod.rs` and `src/app/`: the rest of the app, each file an `impl App` block (several are allowed in a crate)
  plus its own types and constants. The modules are private and re-exported whole from lib.rs, so paths stay
  `crate::Settings`, `bryton_rustfw::POLL_MS`; App's fields are private to the crate root, which every module can
  read; a method another file calls is `pub(crate)`.
  - `settings.rs`: `Settings`, `PageCfg`, every backup register of the app (`*_BKP`), saving them; which registers
    stay beside the settings' file, and why.
  - `themes.rs`: the colour theme in use (`App::look`, `apply_theme`, `next_theme`): THEME's name resolved to a
    built-in or a file of `\Rustfw\Themes` (read only, listed 14 at most), the whole screen drawn again.
  - `settings_file.rs`: `\Rustfw\settings.bin`, the settings' file (`SettingsCopy`: two copies, magic, version,
    length, sequence number, CRC-32; read with the pages' file, written between rides; which wins over the
    registers; the migration from them).
  - `screenshot.rs`: SYSTEM > SCREENSHOT (the menus left, the screen under them written at the next pass, never with a
    ride open). `faultlog.rs`: the fault marker found at the start, its line appended once the card is quiet.
  - `input.rs`: `Key`, `Press`, `Keys`, `Screen`, the screen stack (`open` / `close`), `press`, `input` (global arms, then one `input_<screen>` per screen), `touch`.
  - `editor.rs`: the page editor on the page (`Screen::Edit`, its actions, keep or drop), pages added and deleted,
    and `\Rustfw\pages.bin` (read after the start, written between rides, the backup registers after it).
  - `pagemeta.rs`: the page switches (SHOW, MOVE UP / DOWN, RENAME, the road set's names and OFF mask in `pages.bin`) and AUTO SCROLL.
  - `screens.rs`: `show` (a screen whole), `update_page` (only the boxes that changed), the page widgets, the banner,
    the ride card, the basemap, `render_row` (screenshots). `fields.rs`: a ride field's text and colour, number
    formatting. `graphs.rs`: the graph fields and the course profile.
  - `more.rs`: MORE, a denser variant of each ride page (swipe up shows it, swipe down or Back returns): the rider's own
    (the editor edits it, kept in `pages.bin` after the first CRC) or `Page::more_of`; "MORE" in the strip.
  - `menus.rs`: `Menu`, each menu's rows and the text that depends on state (the fixed text is `Row::label` in ui/menu.rs), `activate` (what a
    row does, one helper per kind of row).
  - `nrf_watch.rs`: the Nordic link's watchdog: signs of a framing out of step (garbage frames, commands without a response,
    a quiet link that does not answer a probe), drain and resync, else reset the chip and start ANT and the phone
    link again, with backoff; the count is in the dev link's `info`. Tests: `src/sim/link_tests.rs`.
  - `radio.rs`: the ANT+ channels (`Sensor`), `poll_nrf`, pairing. `gps.rs`: `poll_gnss`, the aiding.
  - `phone.rs`: the phone link over BLE: the GATT server set up (HID, battery, device information), advertising,
    bonding and the bond in `\Rustfw\ble.bin`, the BLE events, the GATT client of the phone (ANCS, AMS, CTS
    discovered one procedure at a time, then its operations queued), media keys, the call and media screens.
  - `notifs.rs`: the phone's last notifications kept (`phone::Inbox`), the NOTIFICATIONS list (a tap on the overview's
    notices, or PHONE in the menu) and a notification's detail.
  - `status.rs`: QUICK STATUS, the six chosen fields over a ride page by a tap on a plain field (`Screen::Status`).
  - `start_cues.rs`: the REMINDER (the bike moves with no ride) and the READY CHECK card at a new ride's start.
  - `riding.rs`: Start / pause, the end-ride screen (RESUME, SAVE, DISCARD), the second tick, the recorder glue.
    `stock_data.rs`: the scan of stock's rides and the import of its settings. `workouts.rs`, `courses.rs`: the workout played and the course followed.
  - `ahead.rs`: climbs ahead, with no course followed: the memory (main.rs), the second's step (the rider on the trace,
    the banner and beep at a climb's start through `course_event`), the work spread over the passes (`step_ahead`: a
    tile, the snap, the trace, at most 12 ms or 6 units a pass, and a pass ends after a unit that read a tile; paused: no work), and the CLIMB fields fed from the trace when no course
    is on (`ahead_text`, `ahead_bar`, called from `course_text` and `course_bar`). BIKE > CLIMBS AHEAD turns it off
    (`Settings::ahead`, register 11's tag 0xaa to 0xac).
  - `groups.rs`: the sub-structs of `App` (`Phone`, `Ftms`, `Notices` (`notes`), `Light`, `Gps`, `Radio`,
    `TrainerLink` (`tlink`), `Plan`, `RampState` (`ramp`), `Recording` (`rec`), `AutoOff`, `ClimbPage` (`climb`), `AheadUs`, `RideStart`), each with its `new` and its
    small pure methods. `pass.rs`: the parts of `App::step` (seconds caught up, the alert said,
    sensors found, the banner's time, the screen's redraw).
  - `device.rs`: backlight, buzzer, status strip, switching off, AUTO OFF (`step_auto_off`) and the idle pass length (`pass_ms`). `clock.rs`: RTC decoding, dates, the shown clock (ZONE and SUMMER TIME: `local_time`, the EU and US rules).
  - `loop_policy.rs`: `LoopPolicy`, the main loop's rules, pure and tested on the host: the cable (start USB once,
    reset on an unplug outside a ride), the idle sleep's wake sources, the sleep slices (`TOUCH_MS` while touching),
    what ends the pass's sleep (`Woke`), and the awake and CPU shares of `info`; main.rs keeps the primitives.
  - `profile.rs`: the ride profile (ROAD, INDOOR or MTB), chosen from the trainer heard or set by hand, given up when the trainer goes silent.
  - `quick.rs`: the quick panel (`Screen::Quick`, a swipe down): what the rider wants first, top to bottom.
  - `drive.rs`: the USB drive: the card lent to the computer (`emmc::Emmc::lend`: one owner at a time, every other
    use refused), when it may be (no ride running or unsaved), taken back on eject, a restart after the computer
    wrote.
- `src/ui/mod.rs`: what is on screen, through the `Canvas` trait (`fill` a rectangle, `blit` one from a pixel
  function). It knows nothing about the panel. Text styles (`LABEL`, `VALUE`) are defined there once.
  `Rect` (`contains`, `grown`) is the one hit-test: a touch is tested against a `Rect`, never with its own range checks.
  Its parts are re-exported whole, so paths stay `ui::X`: `src/ui/strip.rs` (the status strip, whole and as changes),
  `ui/fields.rs` (a ride field's box, value, layouts, frames, zone bars, the graphs' axes and columns), `src/ui/gauge.rs`
  (the workout gauge field), `src/ui/dial.rs` (the ring and bar gauge fields: drawn with the box, then only the arc or columns that moved) and `src/ui/overview.rs` (the overview: date, TODAY card, cog, notices, workout list rows).
- `src/ui/menu.rs`: menus as data. `Input` (what a key or a gesture asks for: screens act on inputs, never on keys),
  `Row` (what a menu row is: a submenu, a setting, a sensor, a reading, a page field) and each menu's table,
  `Setting` (label, choice or number with range and step). A new setting is a `Setting` variant, its row in a table,
  and its arms in `Settings::text` / `cycle` / `step`: never a row number. Back pops a screen stack (`App::open` /
  `close`), so no screen knows where Back goes.
- `src/ui/theme.rs`: colour themes (`Theme`: what each built-in colour is drawn as, `Canvas::ink`), the built-in
  DARK (identity), OUTDOOR and LIGHT, the INI parser and the whole-theme checks (contrast, red only for alarms).
- `src/ui/text.rs`: text in the pixel font at a scale (`text::draw`, a `Style`, `Align`), one panel window per glyph.
  For a value that changes, `text::draw_in` (a fixed box, the rest painted with the background) and
  `text::Line` (`write!` into a stack buffer, no heap). Float formatting (`{:.1}`) costs a few KB of image.
- `src/ui/fonts.rs`: glyph data, **generated** by `tools/font.py` (4 bits per pixel); never edit it by hand.
  - `PIXEL`, the UI font: this project's own 5 x 7 dot matrix (`tools/pixel_font.py`, one text grid per glyph),
    drawn at 2x or 4x (`Style::scale`) so it matches Ferris.
  - `SMOOTH2` to `SMOOTH8`, the FONT setting (SETTINGS > DISPLAY, PIXEL by default, `Settings::smooth`, register 16
    bit 24): Barlow Semi Condensed Bold (SIL OFL 1.1, `tools/fonts/`, licence beside it) rasterized into the pixel
    font's own cell at each scale (advance 6 x scale, height 10 x scale, ink within 5 x scale columns), drawn 1:1.
    Scale 1 stays PIXEL, and from scale 5 only the characters a value shows are smooth (the rest, and the cue arrows,
    fall back to PIXEL at that scale: `fonts::smooth`, `text::pick`), so no layout depends on the choice: keep it so
    (`smooth_fonts_share_the_pixel_cells`). `Canvas::smooth` carries the choice to every draw (`Panel`'s third field, filled from
    the settings; screenshot rows carry it in `Row`), so the screenshot equals the panel.
  - To change a glyph, edit `tools/pixel_font.py` and run `python3 tools/font.py` (needs PIL; `-o PATH` writes elsewhere; check.sh compares its output with this file).
- `src/drivers/panel.rs`: the ST7789 protocol as data and command bytes (init table, CASET / RASET / RAMWR, off).
- `src/drivers/gnss.rs`: the u-blox receiver's UBX framing, NAV-PVT and the configuration sent (RAM layer only), the GNSS SYSTEMS keys.
- `src/drivers/mga.rs`: AssistNow Offline: the reader of `\Rustfw\mga.ubx` that picks today's UBX-MGA-ANO messages a sector at a time.
  `tools/gnss.py` talks to it from the computer through `peek` / `poke`.
- `src/training/ride.rs`: the ride (Start / pause / lap / end, auto-pause, totals), the heart-rate and power zones and their
  colours, the history behind the graph field. Pure logic, one tick a second; each change comes out as an `Event`
  for the recorder (`App::recorder`, the one place the FIT writer will plug in).
- `src/ui/page.rs`: a ride page as rows of cells (up to 5 rows of 1 or 2, 8 fields), the layouts as presets, the edits
  (split across or down, merge, taller, shorter, a field) that never leave a field too small to read, and the
  format of `pages.bin` (two copies, seq and CRC). Pure logic.
- `src/training/power.rs`: power metrics once a second (3 / 10 / 30 s averages, NP, IF, TSS, kJ, kcal, W' balance), running
  sums over a ring, integers only; `training/ride.rs` keeps max heart rate and the climbing (VAM, grade). Pure logic.
- `src/ui/card.rs`: a ride's card once it has ended, and for each past ride: its map, elevation profile and ten
  numbers. Drawn once, when shown.
- `src/ui/curve.rs`: the POWER CURVE screen's drawing: the log time axis, the plot in one blit, the cursor's strip and
  readout. `src/app/curve.rs`: its screen, inputs and this ride's curve ([`power_curve::Rolling`] fed a second at a time).
- `src/ui/load.rs`: the TRAINING LOAD chart's drawing (two lines and the form's bars in one blit, the cursor's strip and
  readout). `src/app/pmc.rs`: its screen and inputs, the build of the load from the scan's index (again when it, the day
  or the settings change), the CTL / ATL / TSB fields' values and the MONTHS rows.
- `src/ui/media.rs`: the media player screen: what the phone says is playing (AMS), a progress bar, touch buttons,
  each part in its own fixed box.
- `src/ui/quick.rs`: the quick panel's geometry and drawing (app/quick.rs decides what each box says).
- `src/ui/map.rs`: the map field: the track (fixed array, thinned when full), the local projection, the fitted view,
  and drawing in runs per row clipped to `Canvas::clip`, so a dirty box and a screenshot row use the same code.
- `src/drivers/ambient.rs`: the sensor bus chips (I2C4) against the `Board` primitives `i2c_read` / `i2c_write`: the
  ambient light sensor, the barometer, the accelerometer (switched off), and the automatic backlight (stock's curve
  and timing). `src/sim/mod.rs` has a model of the bus (`Bus`); `tools/i2c.py` reaches it through `peek` / `poke`.
- `src/drivers/touch.rs`: the touch controller (Sitronix, I2C1) against the `Board` primitives `touch_read` / `touch_reset` /
  `touch_int`: its registers, the start, and the gestures (tap, hold, swipe) recognised from the points; `src/sim/mod.rs`
  has a model of the chip (`TouchChip`) with a scripted finger. `tools/i2c.py --touch` reads it from the computer.
- `src/nav/altitude.rs`: the barometric altitude: the international barometric formula as an integer table, the
  smoothing (a median of 5 readings, then an average of 1/6), the calibration (typed, from the receiver, at a ride's
  start; `\Rustfw\altcal.bin`'s sector), and which source the ride takes (barometer or GPS, with a reference number);
  a new reference never steps the ride's altitude: shifted, then let go at 0.5 m/s, that part (`corr`) taken out of
  the ascent (ride.rs, 3 m hysteresis on the barometer). Pure logic. `src/app/barometer.rs`: its readings (a second while riding), the menu rows and the file.
- `src/nav/places.rs`: favourite places: the list (up to 16, PLACE n, an altitude or none), `\Rustfw\places.bin`'s two copies
  (magic, version, length, sequence, CRC-32), the distance and bearing, the nearest with an altitude. Pure logic.
  `src/app/places.rs`: SAVE PLACE, the PLACES menu rows, the barometer calibrated at a place, the file read and written
  between rides.
- `src/storage/bikes.rs`: the three bikes: each one's wheel, weight, gears, odometer and five parts (km since RESET,
  SERVICE), `\Rustfw\bikes.bin`'s two copies (magic, version, length, sequence, CRC-32), the rides counted once
  by a mark of the newest counted. Pure logic. `src/app/bikes.rs`: BIKES' menu rows, ACTIVE BIKE (its gear into the
  settings), the migration from the settings' single bike, the count at a ride scan's end, the part notice, the file
  read and written between rides (before settings.bin; the store in AXI SRAM, main.rs `make_bikes`).
- `src/storage/bytes.rs`: the bounds-checked little-endian fields (`le16`, `le32`) the unit's own readers share.
- `src/training/pacing.rs`: the climb pacer: the power for each section of a climb that gets to the top soonest with the W' the
  rider has left (Martin's power equation for the speed; with the budget spent above CP every section not at a bound
  rides at one speed, so a plan is one bisection on it). Pure logic;
  `src/app/pace.rs` cuts the segment being ridden (its altitudes), else the followed course's or the road ahead's climb, into sections, plans again at each one, and draws the TOP gauge
  (`Field::ClimbPace`, the workout gauge's widget).
- `src/training/alerts.rs`: reminders during a ride (eat and drink every so many minutes, heart rate over a limit), fed a second
  at a time, and `Queue`, which keeps several due at once to be said one after the other. Pure logic.
- `src/drivers/powercal.rs`: the power meter's calibration over ANT+ (Bicycle Power page 0x01: the zero offset, the auto zero
  switch, the meter's answers). Pure logic. `src/app/powercal.rs`: SENSORS > CALIBRATE POWER, its screen and the
  request sent as an acknowledged message on the power channel.
- `src/training/ramp.rs`: the FTP ramp test: its workout, the stop rules, the best minute and FTP, HRVT1 from DFA a1. Pure logic.
  `src/app/ramp.rs`: it played through the workout player, and the result screen (APPLY).
- `src/training/load.rs`: training load: a ride's TSS (power, else hrTSS of its average heart rate), CTL / ATL / TSB
  from the daily TSS (fixed point, Coggan's PMC), the last twelve months' totals. Pure logic.
- `src/training/hrtss.rs`: hrTSS, a training stress score from heart rate for a ride without power (Banister TRIMP against an
  hour at LTHR; sex picks the constant). Pure logic.
- `src/training/hrv.rs`: heart rate variability from the strap's RR intervals (`sensors::Beats`): RMSSD and DFA a1 (P9's
  algorithm), recomputed every 5 beats. Pure logic.
- `src/drivers/battery.rs`: battery millivolts from the ADC readings, the charge estimate, when to switch off.
- `src/training/power_match.rs`: ERG power match: the trainer / meter ratio from a 30 s average, the corrected target (Q16, pure).
- `src/training/trainer.rs`: the home trainer whatever the link (what is read, what is commanded: ERG, resistance, SIM; when to
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
  `src/app/survey.rs`: STORAGE's glue: the survey's passes while its screen is open and no ride runs, and the oldest
  ride's card (HISTORY's, DELETE there).
  `src/app/trainer.rs`: the glue (readings into `Live` where no meter or sensor is heard, the commands sent as
  acknowledged messages, the workout's power step and the course's grade as drives) and the trainer screen and
  fields; `src/ui/trainer.rs` draws the screen. One acknowledged message per 250 ms on its channel; its pairing is in
  `\Rustfw\sensors.bin` (backup register 14 is the shifting systems').
- `src/drivers/sensors.rs`: ANT+ sensor kinds (stock's channels), decoding their broadcasts, the live values the ride
  pages show. Pure logic.
- `src/drivers/nrf.rs`: the Nordic BLE / ANT chip's serialization protocol (framing, commands, start-up), against the
  `Board` byte primitives `nrf_reset` / `nrf_send` / `nrf_recv`; `src/sim/mod.rs` has a model of the chip (`Nordic`). BLE
  events that arrive while a command waits are kept (`Rx::pop`); the main loop acts on events only once the bytes
  read are framed (a command sent mid-read would take the rest of the stream out of order).
- `src/drivers/ble.rs`: the SoftDevice API v5 calls of the phone link (GAP peripheral, GATT server and client), the
  central's (connect, cancel, scan stop) and their events, each with its connection, serialized as the nRF5 SDK's
  codecs do (sources in the module doc); `nrf::start` configures two links (`sd_ble_cfg_set`) and falls back to the
  defaults when refused; `sim/ble_sim.rs` models the chip's side, a scripted phone and a trainer (FTMS or FE-C over BLE).
- `src/link/phone.rs`: the phone link's formats, pure logic: the HID report map, ANCS, AMS, CTS, the bond's sector.
- `src/storage/fit.rs`: the ride file (FIT activity), written as it goes against a `Sink` (write at an offset, set the
  length, flush); a checkpoint leaves a valid file and returns the recovery record, which resumes it after a reset.
  `tools/fit_check.py` decodes the test ride with independent decoders (fitdecode, Garmin's FIT SDK), the full copy's
  developer fields included.
- `src/drivers/emmc.rs`: the eMMC (SDMMC1) protocol: identification, EXT_CSD SEC_COUNT, block reads and writes
  (writes refused at or past sector B, stock's reserved area), stock's layout; against the `Board` primitives
  `emmc_power_on` / `emmc_bus` / `emmc_cmd` / `emmc_read` / `emmc_write`; `src/sim/mod.rs` has a model of the card
  (`EmmcCard`, which serves a FAT image when given one and can lose power after any sector written). Used by the
  basemap, the recorder and the dev link's `emmc` and `sector` commands; identified at first use (at the first main loop pass, for the scan's cache of
  stock's rides; at boot at the first start, for stock's settings, or when a ride is open).
- `src/storage/fat/mod.rs`: a FAT32 reader: boot sector, chains, 8.3 and long names, files at an offset, directory
  listings; every loop bounded. Tested on `test-data/fat32.sparse`, a volume macOS made (`tools/fat_image.py`,
  which also puts made-up stock rides and settings files on it). `src/storage/fat/write.rs`: the write path, only
  what the recorder needs (create with a long name, write sectors, set the size, move by delete + link, mark
  deleted with 0xe5 only, sync the FATs and FSInfo), each write ordered so a power cut leaves a volume stock and a
  computer accept; `fsck` in its tests checks that.
- `src/storage/recorder/mod.rs`: the ride recorder (the golden rule): the FIT file in `\Rustfw\Rides` from RAM to the card at
  each checkpoint (a minute at most), two recovery records in `\Rustfw\ride.rec` (3 KB), resume at boot, the save to
  `\Activities\YYMMDDhhmmss.fit`. Its tests cut the power after every sector of a whole ride and of the reboot.
  Two recorders run per ride (`Kind`): the clean copy above, and the full one (`Alpha1` / `RMSSD` / `ArtPct`
  developer fields and the strap's beats in `hrv` messages, `Recorder::beat`, `\Rustfw\Full`, `\Rustfw\full.rec`,
  saved to `\Rustfw\Activities`), which never holds the clean one up
  (`App::step_full`, docs/sync.md). A ride discarded at its end goes through the same save into
  `\Rustfw\Discarded` (`Discarded\Full`), its records saying so, so no cut puts it in `\Activities`.
  `src/storage/recorder/rec.rs`: the recovery record (`Rec`, one sector of `ride.rec`) and the ride names; `recorder/tests.rs`: its tests.
- `src/storage/fitread.rs`: a FIT reader, one byte at a time (no buffer): records (time, power, HR, distance; never
  the position) and the session, the CRCs checked. `src/storage/ini.rs`: a tiny INI reader.
- `src/storage/stock.rs`: what stock left on the volume: the scan of its rides (a bounded amount per call, spread
  over the main loop's passes, each pass bounded by `Board::awake_us`) and the import of its settings. Its cache
  (`Cache`, `\Rustfw\scan.bin`, <!-- n:scan_cache -->198 KB<!-- /n --> in AXI SRAM from main.rs `make_scan_cache`): an entry of 64 bytes per ride
  (totals, history, best powers), at most 3072 rides (rides past that are read and counted again at every start,
  in a second pass), so each ride is decoded once ever and a start reads only new or changed rides. `src/training/power_curve.rs`: best powers per duration in
  one pass, merged across rides, and the 2-parameter CP / W' fit (an envelope, the reference tools in its module
  doc). Pure logic. `src/training/hrvt.rs`: HRVT1 / HRVT2 (DFA alpha1 0.75 / 0.5) from a ride's alpha1 and power, the median
  over recent rides, pure logic; `src/storage/thresholds.rs` reads the recent full copies for it (memory from
  main.rs `make_thresholds`); `src/app/fitness.rs`: FITNESS's estimates, EST FTP, APPLY, and the ride's CP / W'
  (`model_now`, latched at the ride's start for W' balance and the pacer).
- `src/storage/trash.rs`: HISTORY's DELETE: a ride moved (never erased) into `\Rustfw\Deleted` (its full copy
  into `\Rustfw\Deleted\Full`), `\Rustfw\delete.rec` naming the move so the next start finishes one a cut left;
  stock's `.sum` / `.csum` left as they are (module doc: why).
- `src/storage/screenshot.rs`: the screenshot file, `\Screenshot\YYYYMMDDhhmmss.bmp` (stock's): a 16-bit BI_BITFIELDS
  BMP filled from the rows, the room checked first, the chain written before its entry. `src/storage/faultlog.rs`: the
  fault marker's layout (register 29, written by hw.rs's handler: kind, panic line or CFSR bits, logged, the count),
  the log's line, and the append to `\Rustfw\Log\faults.log` (capped at 64 KB).
- `src/storage/volume.rs`: STORAGE's survey of the card, read a pass at a time: the size and free space (FSInfo's count as an estimate, then the FAT counted), the rides in `\Activities` and the oldest five by name; the size and day texts.
- `src/storage/history.rs`: the ride history: the index of stock's rides (filled by that scan, newest first) and a
  past ride's card, its file read once with the position (`fitread::Reader::with_position`).
- `src/storage/workout_file.rs`: stock's planned workouts (`\Workout\calendar\YYYYMMDD\*.fit`): a FIT workout file read
  whole (name, steps, targets, repeats written out), the calendar from today on. `src/training/workout.rs`: the player, pure
  logic fed one second at a time (the step running, time left, 3-2-1 beeps, Lap to the next step).
- `src/storage/course_file.rs`: course files, read only: stock's `\PlanTrip\*.fit` (Bryton's own route layout, in the
  module doc), FIT courses and GPX (a small streaming reader) in `\Courses`; the list, and `Courses`, the course
  memory (about <!-- n:courses -->75 KB<!-- /n -->, a static cell in AXI SRAM from main.rs `make_courses`). `src/nav/course/mod.rs`: the course model,
  pure logic: points thinned to 4096, where the rider is (a moving window, a bounded search when lost),
  off course, ascent left, the climbs (Garmin ClimbPro's definition), the profile's columns. `src/nav/course/cues.rs`:
  turn-by-turn cues (FIT course points, GPX turn words, else the turns of the course's shape), the next one said.
- `src/ui/tiles.rs`: the basemap: stock's cached map tiles (`\Map\Data\Temp\Zoom<z>`) read from the FAT volume,
  drawn once per view into a pixel buffer in SDRAM (`hw::map_memory`), which map.rs copies to the panel and
  restores from under the arrow and the track; IGN's raster tiles first where the owner's packs have them.
- `src/storage/named.rs`: the named points along a followed course (peaks, volcanoes, saddles with elevation) from stock's cached
  tiles' layer 13, a bounded scan a step a pass; the `Pois` list lives in `Courses`. `src/sim/poi_tests.rs`: the synthetic tiles, the
  scan and its fuzz.
- `src/storage/ign.rs`: the IGN map packs (`\Rustfw\Maps\IGN<z>_<bx>_<by>.PAK`, the format in its module doc),
  made on the computer by `tools/ign_pack.py` (fetch, pack, the synthetic `fixture` in `test-data/ign`); the LZ4
  block decoder. Real IGN tiles never enter the repository.
- `src/storage/osm.rs`: the OpenStreetMap map packs (`\Rustfw\Maps\osm\OSM<z>_<bx>_<by>.PAK`, 16 x 16 tiles a file,
  stock's decoded H and P tile records inside; the format in its module doc), made on the computer by
  `tools/map_pack.py` (an Overpass JSON and a bounding box; its selftest builds a synthetic area and reads it back
  with an independent reader, and the committed synthetic pack `test-data/osm` equals a fresh `fixture`, which the Rust tests read through `include_bytes!`). `ui/tiles.rs` draws them through the same `load` as stock's tiles (`InPack`), stock's files for a tile the packs lack.
- `src/storage/route.rs`: stock's routing graph from its map files (`\Map\Update` then `\Map\MapData\<Region>\9-<x>-<y>.dat`,
  section 1, zoom 14): the container, a tile's gzip stream, its protobuf decoded into a `Graph` (a few tiles at once, nodes
  sorted by position, 100 KB), `Maps` (the files found once, one tile a call, into the inflate buffer main.rs takes from
  the map's SDRAM). The node keys are sign and magnitude, not two's complement (module doc: how it was found).
  `src/storage/inflate.rs`: gzip and deflate (RFC 1951, puff.c's method) over an iterator, into a `Scratch` of 16-bit
  words, CRC checked; its own code rather than `miniz_oxide` (module doc: licence, memory, size). `tools/route_pack.py`
  makes the synthetic map file in `test-data/route` (a made-up road in the open South Pacific) and reads real files to check.
- `src/nav/climbs_ahead.rs`: climbs on the road ahead without a course (pure logic against a `Graph`; stock's Climb Challenge
  2.0, ghidra/recovered_src/ride/route_climb.c): the snap to the nearest road (heading-aware), the trace (same way, else
  same class, else the smallest turn under 110 degrees, tiles asked for as it goes), stock's 9-tap smoothing, and the
  climbs by the course's own Garmin definition (`Course::climbs_of`), the rider placed on the trace each second, the
  climb said once. About <!-- n:ahead -->106 KB<!-- /n --> in AXI SRAM from main.rs `make_ahead` (all-zero start: no `Option`, so it stays in `.bss`).
  `src/app/ahead.rs`: its glue, the work spread over the main loop's passes (a tile, the snap, the trace), the banner and
  beep at a climb's start and the CLIMB fields when no course is followed.
- `src/nav/segment.rs`: a live segment file, `\Rustfw\Segments\<id>.seg` (stock's live segment package unzipped; docs/segments.md,
  "The file"; the layout checked against stock's live_segments.c): the header (the index reads its 0x200 bytes), the points and the
  times of the four kept efforts, every check of the file (`Error`), parsed into a caller's storage (about 10 KB with each point's altitude, a static
  cell), read in 1 KB pieces (`parse_with`); the rider's PR file `<id>.pr` (`PrCopy`, `pr_copy`: two CRC'd copies, step S5).
  Pure logic; step S1.
- `src/nav/segment_match.rs`: the live segment matcher (docs/segments.md, "Detection", "Ghost"): `Matcher` (near, approach,
  start, progress by `course::nearest_by`, lost, finish), the compact index `Entry` and the ghost's `target_s` / `delta_s`. Pure
  logic; step S2, fed by `app/segments.rs`.
- `src/app/segments.rs`: the live segments on the unit (docs/segments.md, "On the unit"; step S3): the index of `\Rustfw\Segments\`
  read 8 headers a pass, the matcher fed once a second during a ride, the active segment loaded from its file in 1 KB reads, what
  the screens (S4) read (`segment_now`, `events`); the efforts (S5): each finish's times kept in RAM, written after the
  ride's save (`<id>.pr`, `efforts.csv`), the `.pr` the PR target at the next load. Nothing drawn.
- `src/app/descents.rs`: the DESCENT AHEAD field and the STEEP DESCENT alert (the next descent of the course followed, else of the road ahead).
- `src/app/climbs_done.rs`: CLIMBS DONE, the climbs ridden to the top this ride (bounded list, bookkeeping, the ride menu rows).
- `src/app/segment_ui.rs`: the live segment screens' glue (step S4): the banners for the matcher's events, the head box of the CLIMB page
  (`head_text`), the result card, the SEGMENTS menu's list rows (headers read from the card).
- `src/ui/segment.rs`: the CLIMB page's head box (`SegHead`: the name, the delta in big digits, the target; characters
  redrawn when they change) and the result card.
  start, progress by `course::nearest_by`, lost, finish) and the ghost's `target_s` / `delta_s`. Pure logic; step S2,
  nothing on the unit calls it yet.
- `src/nav/sun.rs`: the sun's rise and set at a place and date (NOAA's solar calculator, Meeus's equations; cited in the
  module): local minutes, None in polar day and night. Pure logic; `App::refresh_sun` works it out once a minute.
- `src/link/dev.rs`: the USB dev link protocol (commands, screenshot rows, image load and checks), against the
  `Host` trait; `App::serve` there answers the effects that need the app and the card (main.rs keeps the keys, the reset
  and the image run). `src/link/prof.rs`: the profiler the dev link's `prof` reads (sections of the pass, DWT cycles from
  `Board::cycles`); `tools/profile.py`: scripted scenarios on the unit, a `prof` window each; `tools/walk.py`: the simulator's screen walk
  (`src/sim/walk_tests.rs`) replayed on the unit, unit and sim shots side by side in contact sheets under `target/walk/`.
- `src/link/usbsnap.rs`: the USB core and ULPI PHY registers taken at a link's first failure (`Keeper`), packed into
  backup-register words (`pack`) that `tools/dev.py usbdrop` decodes.
- `src/link/msc.rs`: USB mass storage, SCSI over Bulk-Only Transport (the commands a computer needs to mount the FAT
  volume, the 13 cases by padding IN and taking all of OUT, sense data), against the `Pipe` (bulk endpoints) and
  `Media` (sectors: `emmc::Lent`) traits. Its own code, not `usbd-storage` (module doc: why).
- `src/sim/mod.rs` (tests only): a simulated board (`Sim`) and computer (`Pc`), see Tests. Its tests are in `src/sim/`:
  `tests.rs` (the app on the Sim), `page_tests.rs` (the page editor and `pages.bin`, cut at every sector),
  `status_tests.rs` (QUICK STATUS), `switch_tests.rs` (page switches, names, moves, the auto scroll, swipe up),
  `settings_tests.rs` (`settings.bin`: a menu change back after a restart, the migration, cut at every sector),
  `places_tests.rs` (`places.bin`: a place listed after a restart, a ride's start calibrated at a place, cut at every
  sector, damaged copies), `bikes_tests.rs` (`bikes.bin`: the migration, a saved ride counted once with a cut between the
  save and the write and at every sector of it, a rescan ended within a pass, a migration during a ride, WHEEL
  changed mid-ride over a reset, ACTIVE BIKE refused while a resume waits, ACTIVE BIKE's wheel and weight, a part's notice and RESET),
  `theme_tests.rs` (the scramble theme, DARK as it was, OUTDOOR and LIGHT shots, a theme file picked from the card and kept),
  `diag_tests.rs` (SCREENSHOT and the fault log: the file equals the panel, a full card, cut at every sector),
  `dev_tests.rs`, `drive_tests.rs`, `emmc_tests.rs`, `fuzz.rs`, `recorder_tests.rs`, `altitude_tests.rs` (the barometer's
  climb, calibrations and file),
  `phone_tests.rs` (the phone link against `ble_sim.rs`: the chip's BLE side and a scripted phone).
  `interaction_tests.rs` (how many keys and taps each task takes, docs/features.md "Interactions": a menu that gets deeper fails it).
- `src/sim/tests.rs` keeps the shared helpers (`run`, `assert_consistent`, `back`, `KEY`, `STRAP`...) and the boot, key, touch,
  banner, light and misc tests; the rest is by concern: `stock_tests.rs` (the overview's totals, STOCK DATA, fitness, ride
  history and delete), `walk_tests.rs` (the screen walk: every menu screen by keys, metric and imperial, a ride started; text cut by its box fails, `text::clips`; writes target/sim/walk/ and its paths.json), `workout_tests.rs` (workouts from the calendar, the graph and gauge fields), `storage_tests.rs` (STORAGE and ABOUT: the card's figures, its oldest ride), `menu_tests.rs` (the menu
  tree, pairing, pages from the menu, HRV fields, the RIDE menu; `to_row`, `row_in`), `start_cue_tests.rs` (the start reminder and the ready card), `ride_tests.rs` (zone colours and bar,
  live readings, laps, power, climb and graph fields), `map_tests.rs` (basemap, IGN tiles, zoom, pan; `drag`, `tiles_route`),
  `assist_tests.rs` (GNSS SYSTEMS and the AssistNow Offline messages to the receiver), `segment_tests.rs` (live segments: the index, a ride along a synthetic segment, the efforts written after the save and cut at every sector), `course_tests.rs` (the followed course, its cues; `hill`), `laps_tests.rs` (a past ride's lap list) and `perf_tests.rs` (time budgets of a pass, a menu, idle).
- `src/sim/overview_tests.rs` (the overview's TODAY card and the WORKOUTS thumbnails), `powercal_tests.rs` (power meter
  zero offset), `curve_tests.rs` (the power curve screen), `pmc_tests.rs` (training load, its chart and the month rows), `ramp_tests.rs` (the FTP ramp test), `trainer_tests.rs` (the home trainer: FE-C pairing, ERG, SIM).
- `src/sim/saver_tests.rs`: BATT SAVER: each knob applied and given back, AUTO at 20 % once and off on the charger, the recording unchanged, the strip tag.
- `src/sim/power_tests.rs`: AUTO OFF: the unit switches itself off after its time-out of quiet, and only then.
- `src/sim/shift_tests.rs`: electronic shifting on the Sim: a Di2 and an ANT+ shifting system paired, the shifts
  counted while riding, a system that goes silent.
- `src/main.rs`: `impl Board for S510` with registers, and the main loop. Clocks, GPIO, SPI byte transport,
  RTC read and set, standby, SysTick sleep. No app logic.
- `src/board/usb.rs`: OTG_HS and the ULPI PHY brought up as stock does it, and a composite device: the CDC serial
  port (it implements `Host`) and the mass storage interface (its class requests, and `msc::Pipe` over its bulk
  endpoints, each wait 1 s at most).
- `src/board/hw.rs`: the unsafe primitives (above).
- `tools/dev.py`: the computer side of the dev link. `renode/`: the emulator platform and checks.
- `tools/assist.py`: downloads u-blox's AssistNow Offline data with the rider's own token (environment or
  `~/.config/bryton-rustfw/assist.json`, never in the repository), keeps today's and later messages, and copies them to
  `\Rustfw\mga.ubx` on the mounted unit, compared after the copy (docs/sync.md). `assist.py selftest` (check.sh): no network.
- `tools/sync.py`: the computer's sync (docs/sync.md): rides from the mounted drive or the dev link (tools/dev.py `ls` /
  `get`) to Intervals.icu (full copy) and Strava (clean copy), never twice; planned workouts from Intervals.icu to
  `\Workout\calendar`. Dry run unless `--go`; keys and tokens only in `~/.config/bryton-rustfw/` (0600), never in
  the repository. `sync.py selftest` (check.sh) runs against a fake server on 127.0.0.1.
- `tools/segments.py`: the live segment files (docs/segments.md): fetches the rider's segment efforts from Intervals.icu, builds
  `<id>.seg` files (also from the rider's own FIT rides, `own`), writes them to the drive verified. `segments.py selftest` (check.sh) is synthetic; its fixture is
  `test-data/segment/synthetic.seg`.
- `tools/theme.py`: the built-in colour themes as example files (`examples OUT_DIR`, never a mounted drive by default),
  a theme file drawn through the simulator (`preview FILE`, the ignored test `theme_preview`), and its selftest; the
  fixture is `test-data/themes/` (docs/theming.md, section 4).
- `tools/nrf.py`: talks to the Nordic chip from the computer through `peek` / `poke` (BOARD.md 11), to explore the
  protocol before the firmware has a driver. Only with a build that leaves USART2 alone (the installed one, or
  after `tools/dev.py reset`): a firmware that reads the link takes the bytes first.
- `tools/s810_probe.py`: what an S810 owner sends back (docs/s810.md, section 3): read only, the stock drive's
  versions, file shapes and the debug logs' hardware lines in one zip, nothing personal; `selftest` (check.sh).
- `tools/fonts/`: Barlow Semi Condensed Bold (`BarlowSemiCondensed-Bold.ttf`) and its OFL.txt licence, for the SMOOTH fonts (`tools/font.py`).
- `tools/progress.py`: a progress bar when tqdm is installed, nothing otherwise (cosmetic).
- `tools/fetch_firmware.py`: downloads the stock R039 firmware from Bryton's update server and checks it against
  SHA256SUMS.
- `tools/stack_check.py`: the firmware's stack frames against their budgets, the stack's size (from the canary
  to the top; check.sh, after the build).
- `tools/panics.py`: every panic site of the release ELF (bounds, unwrap, overflow, explicit) with its source line, per file and
  folder, and the bounds sites in files that deny `clippy::indexing_slicing` (a report, exit 0; `selftest`).
- `tools/numbers.py`: the live numbers of the docs (image, AXI SRAM free, the cells' sizes, the tests, lines of Rust): `--write`
  rewrites the markers `<!-- n:NAME -->TEXT<!-- /n -->` (check.sh, after tools/pack.py), `--check` lists the stale ones, `selftest`.
- `tools/lint.py`: the checks reviewers did by hand (pub items only tests use, dashes, licensed keys in tracked files, dead
  tool functions, ruff F and E9; check.sh, before cargo), `selftest`.
- `tools/review.py`: the per-branch review report (cargo-mutants on the diff, new jscpd clones, lint; `--base REF`, `--quick`
  (mutants only under src/storage/), `selftest`, which check.sh runs; the report itself is not in check.sh; config `.cargo/mutants.toml`).
- `tools/iaprecover.py`: recovery through the bootloader's USB mode: read back the OS area, or rewrite it (sudo).
- `build.rs`, `tools/ant_key.py`: the ANT+ network key. It is licensed, so it is **never** in this repository, in a
  commit, a test or a doc: `tools/ant_key.py` copies it from the owner's stock image into the gitignored
  `ant_key.bin`, and build.rs passes it to main.rs as `ANT_KEY` (None without the file). Shimano's private network
  key (the Di2 channel) is handled the same way: `di2_key.bin`, `DI2_KEY`, never printed, no hash of it kept.
  Tests use stand-in keys. `tools/font.py`, `tools/pixel_font.py`: the fonts.
- `deny.toml`: cargo deny's policy (advisories, licences, duplicate crates), run by check.sh.
- `link.x`, `tools/pack.py`: memory map and the update file. `tools/pack.py` re-checks what the stock updater checks.
