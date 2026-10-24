# Roadmap

What to build, in the order the pieces depend on each other (2026-10-23). The sources: the features built on the
stock firmware in bryton-S510-mods, what riders ask of Garmin, Wahoo, Hammerhead, Coros and Bryton units (forums,
reviews), and the hardware facts in BOARD.md. Phase 1 comes first because everything after it adds pages, fields,
settings and menu rows.

Every phase keeps the split of AGENTS.md (Layout): protocols, decisions and drawing in the library, which cannot
reach a register (`check.sh` checks it keeps `forbid(unsafe_code)`); `main.rs` only moves bytes and bits through
`Board` primitives. A new chip gets primitives in the shape of the existing ones (the touch controller reuses the
I2C ones with a bus number) and a model in the simulator. Each phase ends with a read of `main.rs` for any decision
that crept in.

## 1. Pages and menus that the rest can build on

Today `App` (src/lib.rs) holds menus as hard-coded row numbers (`activate` matching `(Menu::Main, 7 | 8)`), each
Back has its own destination, and four mechanisms track what is on screen (`shown`, `main_shown`, `menu_shown`,
`graph_shown`). Each new feature would add another case to all of them.

- **Input events, not keys.** Keys and touch both become one `Input`: `Next`, `Prev`, `Select`, `Back`, `Menu`,
  `Up` / `Down` (a value editor, with repeat), `Tap(x, y)`, `Swipe(dir)`, `Hold`. Screens handle `Input` only, so a
  gesture and a key reach the same code, and a Di2 hood button or the dev link can too.
- **A screen stack.** Opening a menu or a sub-page pushes, Back pops (a fixed array, a few deep). No screen knows
  where Back goes.
- **Menus as data.** A static tree of items: submenu, choice (a list), number (range, step, unit), action, reading.
  One list view draws any of them, scrolls, edits in place (as MAX HR does now) and handles touch (tap a row,
  slide to scroll). Lists that change at run time (sensors, nearby BLE, pages) are a function returning rows.
- **Settings in one place.** A `Settings` struct with each setting described once (name, kind, range, default) so
  the menu, the store and the tests read the same table. Stored in the RTC backup registers now, in a settings file
  (two copies, a CRC, a version) once the eMMC is written.
- **Widgets with a fingerprint.** A page is a list of boxes, each with a widget (a value, the clock, a graph, the
  map, a climb profile, a banner) and the fingerprint of what it shows. Each pass recomputes the fingerprints and
  redraws only the boxes that changed, and inside a box only the cells that changed (as the digits and field
  values do now). One mechanism instead of four.
- **Pages from data.** The ride pages, the main screen and new ones (overview, map, climb, workout) are the same
  kind of thing: a layout and its widgets, chosen in the menu. The **overview** becomes the home screen: time,
  battery, sensors, notifications, totals (week and year distance, odometer, once rides are saved).
- **Banners** for alerts and notifications: one box drawn over a page for a few seconds; the boxes under it are
  redrawn when it goes.
- **Touch** (BOARD.md 17): a driver against new `Board` primitives on I2C1 (the same shape as the I2C4 ones), the
  point parsing and a gesture recogniser in the library (stock's thresholds: a swipe is over 37 px across or 48 px
  up), a model of the chip in the simulator. Read only while the interrupt line is low, so no touch costs nothing.

Done when: every existing screen runs on it, with the same pixels (the simulator's screenshots) and the same or
lower awake time (`dev.py info`).

## 2. Power

- **Stop mode** between seconds, woken by the RTC, the keys, the touch interrupt, the radio chip's UART and USB:
  the largest gain left. A core change: with the owner present.
- **SPI by DMA** for the panel: the CPU sleeps while a map (about 130 ms at 8 MHz) goes out. The format of what is
  drawn matters less than this.
- **D-cache on for the SDRAM** (write-through), after which `+strict-align` can go. Until then, data the firmware
  scans (tiles, tracks, tables) is laid out word-aligned so it is read with 32-bit loads (`LDR`, `LDM`), never
  byte by byte.

## 3. Storage and ride saving

The volume is shared with Bryton's firmware, so the owner can switch back and forth (AGENTS.md, "Share the storage
with stock"): read stock's rides and settings first (read only, possible now), then write ours in the same places
and formats.

The golden rule (AGENTS.md) depends on this whole phase.

0. Read what stock left (done, 2026-10-24): `fitread.rs` streams a FIT file, `stock.rs` scans `\Activities`
   (power curve of 90 days, CP and W', week / year / all-time totals) and imports `\System\*.ini` (FTP, max HR,
   wheel, ANT+ sensors, pages); the STOCK DATA menu. Next: weight, CP and W' applied once those settings exist;
   the scan's result kept (a cache file in our own folder, once there is a write path) so it is not read again
   at each start; stock's summaries (`.sum` / `.csum`) instead of whole rides for the totals, once their format
   is known.
1. eMMC writes (done in the simulator, 2026-10-24): CMD24 / CMD25 + CMD12, CMD13 busy bounded, refused at or
   past sector B. Not yet run on the unit.
2. FAT32 writes (done in the simulator): create with a long name, append, cut back, move, delete by 0xe5 only,
   both FAT copies, FSInfo, in an order a power cut can stop at any point (`fat/write.rs` module doc). Host tests
   cut the power after every sector.
3. Ride recording (done in the simulator): `recorder.rs`, the ride in `\Rustfw`, a checkpoint at least every
   minute, two recovery records, resume at boot, the save to `\Activities`. Next: on the unit with the owner
   (docs/journal.md 2026-10-24); a reliable write (CMD23 bit 31) for the sectors rewritten in place; the track,
   graphs and power totals of a resumed ride (only the ride's totals come back); HRV messages.
4. Settings file (phase 1's store moves here).
5. USB mass storage: rides off and courses, workouts and maps on, with no phone app. The recorder and the FAT
   writer stop while the computer owns the volume.

## 4. Ride data

- Done (src/power.rs, ride.rs): NP, IF, TSS, kJ, kcal, W/kg, 3, 10 and 30 s power (patch P6), W' balance (the
  planned P10, differential model, CP = FTP), max power and heart rate, average cadence, pedal balance, grade, VAM,
  temperature. Weight and W' in the menu (RTC backup register 16).
- Next: grade, VAM and ascent from the barometer instead of GPS altitude; a CP setting apart from FTP if riders
  want one; torque pages (0x11, 0x12) and torque effectiveness / pedal smoothness (page 0x13).
- Alerts on the buzzer and a banner: HR and power limits, eat and drink reminders by time, distance or kJ (stock
  has six kinds).
- Auto lap; a ride summary at the end; ride history and totals for the overview.
- From the patch project: HRV recording (P8: the RR intervals as FIT hrv messages, and DFA a1 as the `Alpha1`
  developer field, as P9 does), W' balance (planned P10), power smoothing (P5). Done: the live DFA a1 and RMSSD
  fields (P9's algorithm, hrv.rs).

## 4b. History and training load

From every ride on the shared volume (stock's and ours, AGENTS.md "Share the storage with stock"), as Domestique
(github.com/platypus45/domestique) and intervals.icu do on a computer, on the unit:

- **A ride index:** per ride its date, time, distance, TSS (from power: NP, IF and FTP; from heart rate when there is
  no power: hrTSS from the threshold heart rate), time in zones and best efforts. Built by scanning the FIT files on
  demand (about 64 bytes a ride, all of them in SDRAM), kept in this firmware's own folder once the eMMC is written
  so it is not scanned at every start, and updated by each saved ride.
- **Power curve and best efforts:** the mean-maximal power from 1 s to 60 min over the last 90 days, the season and
  all time, with the date of each best; CP and W' from it (section 4).
- **Training load:** daily TSS, fitness (CTL: 42-day average), fatigue (ATL: 7-day), form (TSB: CTL - ATL), the ramp
  rate, weekly TSS against a target.
- **Fields for the overview and the pages:** this week's load as one bar a day (coloured by intensity), a progress
  bar of the week (TSS or distance against the target), form as a number with its state (fresh, neutral, tired),
  fitness as a sparkline of the last weeks.
- **Screens:** the power curve (log time axis, two curves), best efforts, the fitness / fatigue / form chart, the
  weeks.
- **The plan:** today's planned workout and the week's planned TSS from the workout files on the volume (stock's
  \Workout\ and \Progress Planner\, or FIT workouts from intervals.icu or Domestique copied over USB), each ride
  matched to the planned session by TSS, duration and intensity, so the overview shows done, missed and what is
  left this week.

## 5. Maps

- **Now:** stock's cached tiles (`\Map\Data\Temp`), read only. They exist only where stock has drawn. The full
  region maps stock downloads (`\MAP\MapData\<region>`) were empty on the unit.
- **Next:** our own packs, made on the computer from **OpenStreetMap** with planetiler's OpenMapTiles profile, written
  in the same record format `src/ui/tiles.rs` already draws (stock's maps use the same schema). One file per zoom 10
  tile holding zooms 10 to 14 (341 tiles: a 32-byte header, an index of offset and length, 4-byte aligned records),
  copied over USB mass storage. A tool, `tools/map_pack.py`, with no new dependency (sqlite3 is in Python's
  standard library).
- **IGN (done, not yet seen on the unit):** Plan IGN v2 raster tiles (data.geopf.fr WMTS, Etalab 2.0, no key) for
  France, with contour lines and hill shading built in: `tools/ign_pack.py` fetches an area (resumable, 4 requests a
  second) and packs it (per-tile palette of up to 256 colours, LZ4; about 24 KB a tile, about 3.9 GB for 150 x 150 km
  at zooms 11 to 16), `src/storage/ign.rs` reads it, `src/ui/tiles.rs` draws it, stock's map under it where it
  stops. The SCAN 25 and SCAN 100 rasters are not open to private individuals, so they are out.
- **IGN, to do when there is an hour with the unit (2026-10-24, put aside by the owner):** the owner's region is
  fetched and packed on the computer (zooms 10 to 16, 163,441 tiles, 222 packs, 3.34 GB, every tile decoded by the
  firmware's own decoder, outside the repository). Left: (1) free space: the card has 0.7 GB free; the owner keeps
  Europe, so `\MAP\MapData\{Australia,Japan,New_Zealand,South_Korea,Taiwan,israel}` go (4.2 GB, removed from the
  computer with `rm -rf` on the mounted drive, not the Finder, whose trash stays on the card); (2) copy the packs to
  `\Rustfw\Maps` with `cp -X` (no `._` files) through this firmware's USB drive at full speed (about 1 MB/s, so
  about an hour); (3) eject (the unit restarts), mount again and compare with `diff -r` (a compare right after the
  copy reads the Mac's cache, not the card); (4) the map on the unit: draw time per view, the zoom chosen, "(c) IGN".
  Then: a pack per region with a lower zoom limit so a whole country fits.
- **Notices** on an About page: "© OpenStreetMap contributors (ODbL), © OpenMapTiles", and IGN's when used (the map
  already shows "(c) IGN" in its corner whenever IGN tiles are drawn).
- Vector over raster: about ten times smaller, any zoom, and text that stays upright if the map turns.

## 6. Navigation and climbs

- **Done (sim):** courses from stock's `\PlanTrip` (Bryton's own route layout), Garmin FIT courses and GPX copied to
  `\Courses`: the line on the map, where the rider is on it (a moving window, a bounded search when lost),
  off-course alert, distance to go, ascent left; the course's elevation profile coloured by grade; **climb
  detection** (Garmin ClimbPro's definition: each climb's length, grade, what is left, a banner at its start).
- **Next:** the way back to the course drawn, navigate to the start, reverse a course; a climb page that opens by
  itself; climbs without a course from the barometer and the map's contours; pacing on a climb from FTP and W'
  balance.
- Turn-by-turn cues (stock's `voiceTrip.fit` holds them: message 250 with street names); rerouting on the unit
  later.

## 7. Training

- Structured workouts from FIT workout files, with step targets, the 3-2-1 beeps and the next-step strip (patches
  P1, P3). ZWO, ERG and MRC files are converted to FIT on the computer.
- Smart trainer control (ANT+ FE-C) for workouts indoors.

## 8. Radios and the phone

- BLE sensors (heart rate, speed and cadence, power), then ANT+ radar (Varia) and lights.
- **Shifting and hood buttons** (small, ANT only): Shimano Di2 is not ANT+ but a private ANT channel on its own
  network key (stock: channel 6, device type 1, period 8198, 2457 MHz; the key sits next to the ANT+ key in the stock
  image and is copied the same way, never committed): gears and battery on page 0, the hood buttons (single, long)
  on page 4, sent as acknowledged messages, which `nrf::ant_broadcast` must accept too. SRAM AXS and Campagnolo use
  the ANT+ Shifting profile (device type 0x22: gears, up to 8 buttons, batteries). The buttons become `Input`s (patch
  P12 mapped ten actions), the gears a field.
- **Phone, no app needed** (one bonded BLE link carries all of these; Just Works bonding, one bond kept in backup
  registers until the eMMC is written):
  - media keys as a BLE HID consumer-control device (patch P7: play / pause, next, previous, volume), plus the camera
    shutter (volume up) and mute for free;
  - iOS notifications through ANCS on the banner, with accept / decline for a call;
  - iOS now playing (title, artist, elapsed) and its remote commands through AMS;
  - the time zone from the phone's Current Time Service (inferred).
- With a companion app only: Android notifications and track info, find my phone, live tracking, automatic upload; a
  computer tool over USB can upload rides.
