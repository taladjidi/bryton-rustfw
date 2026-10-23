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

The golden rule (AGENTS.md) depends on this whole phase.

1. eMMC writes (SDMMC single and multiple block writes, bounded busy waits).
2. FAT32 writes: create, append, truncate, rename, both FAT copies, in an order a power cut can stop at any point.
   Host tests cut the write at every step.
3. Ride recording: `fit.rs` (done) into a file, a checkpoint every minute, resume on boot.
4. Settings file (phase 1's store moves here).
5. USB mass storage: rides off and courses, workouts and maps on, with no phone app. The recorder and the FAT
   writer stop while the computer owns the volume.

## 4. Ride data

- More fields: NP, IF, TSS, kJ, kcal, W/kg, 3, 10 and 30 s power (patch P6), grade, VAM, temperature; ascent from
  the barometer instead of GPS altitude.
- Alerts on the buzzer and a banner: HR and power limits, eat and drink reminders by time, distance or kJ (stock
  has six kinds).
- Auto lap; a ride summary at the end; ride history and totals for the overview.
- From the patch project: HRV recording (P8), live DFA a1 (P9), W' balance (planned P10), power smoothing (P5).

## 5. Maps

- **Now:** stock's cached tiles (`\Map\Data\Temp`), read only. They exist only where stock has drawn. The full
  region maps stock downloads (`\MAP\MapData\<region>`) were empty on the unit.
- **Next:** our own packs, made on the computer from **OpenStreetMap** with planetiler's OpenMapTiles profile, written
  in the same record format `src/ui/tiles.rs` already draws (stock's maps use the same schema). One file per zoom 10
  tile holding zooms 10 to 14 (341 tiles: a 32-byte header, an index of offset and length, 4-byte aligned records),
  copied over USB mass storage. A tool, `tools/map_pack.py`, with no new dependency (sqlite3 is in Python's
  standard library).
- **IGN:** the Plan IGN vector tiles (data.geopf.fr, Etalab 2.0, no key) can add contour lines and paths for France,
  merged by the same tool. The SCAN 25 and SCAN 100 rasters are not open data, so they are out.
- **Notices** on an About page: "© OpenStreetMap contributors (ODbL), © OpenMapTiles", and IGN's when used.
- Vector over raster: about ten times smaller, any zoom, and text that stays upright if the map turns.

## 6. Navigation and climbs

- Courses from FIT and GPX files: the line on the map, off-course alert, distance to go, back to start.
- The course's elevation profile; **climb detection** on a course (as Garmin's ClimbPro: each climb's length,
  grade, what is left), and without a course from the barometer and the map's contours; pacing on a climb from
  FTP and W' balance.
- Turn-by-turn cues; rerouting on the unit later.

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
