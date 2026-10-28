# Roadmap

What is not built yet, in the order the pieces depend on each other (2026-10-23, updated 2026-10-28).
Not here: what is built (features.md), what the unit has not shown (status.md), dated findings (journal.md).
Where to look: checklist.md for the comparison with stock, Garmin and other units.

Every phase keeps the split of AGENTS.md (Layout, docs/layout.md): protocols, decisions and drawing in the library, which cannot
reach a register (`check.sh` checks it keeps `forbid(unsafe_code)`); `main.rs` only moves bytes and bits through
`Board` primitives. A new chip gets primitives in the shape of the existing ones (the touch controller reuses the
I2C ones with a bus number) and a model in the simulator. Each phase ends with a read of `main.rs` for any decision
that crept in.

## 1. Pages and menus that the rest can build on

Phase 1 (done): input events, screen stack, menus as data, Settings table, banners, touch, page editor (features.md).
Left: the overview through the same editor, pages switched off and reordered, a MORE variant per page, and widgets
with a fingerprint.

- **Widgets with a fingerprint.** A page is a list of boxes, each with a widget (a value, the clock, a graph, the
  map, a climb profile, a banner) and the fingerprint of what it shows. Each pass redraws only the boxes whose
  fingerprint changed, and inside a box only the cells that changed. One mechanism instead of four.
- **Pages from data.** The overview, the map, the climb and the workout pages are the same kind of thing as the ride
  pages: a layout and its widgets, chosen in the menu.

## 2. Power

- **USB high speed** for the drive: drops during large copies (docs/usb.md); full speed works.
- **Stop mode** between seconds, woken by the RTC, the keys, the touch interrupt, the radio chip's UART and USB:
  the largest gain left. A core change: with the owner present. The plan and its wake sources: docs/power.md,
  "Stop mode: plan".
- **Idle loop at 250 ms and AUTO OFF:** done 2026-10-28 (docs/power.md).
- **SPI by DMA** for the panel: done (2026-10-27, seen from RAM, SPI1 at 50 MHz); the next drawing gain is the D-cache.
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
   wheel, ANT+ sensors, pages); the STOCK DATA menu (now HISTORY > FITNESS and SYSTEM > IMPORT FROM STOCK). Next: weight, CP and W' applied once those settings exist;
   the scan's result kept (a cache file in our own folder, once there is a write path) so it is not read again
   at each start; stock's summaries (`.sum` / `.csum`) instead of whole rides for the totals, once their format
   is known.
1. eMMC writes (done; seen on the unit 2026-10-24, docs/journal.md): CMD24 / CMD25 + CMD12, CMD13 busy bounded,
   refused at or past sector B.
2. FAT32 writes (done in the simulator): create with a long name, append, cut back, move, delete by 0xe5 only,
   both FAT copies, FSInfo, in an order a power cut can stop at any point (`fat/write.rs` module doc). Host tests
   cut the power after every sector.
3. Ride recording (done; seen on the unit 2026-10-24, docs/journal.md): `recorder.rs`, the ride in `\Rustfw`, a
   checkpoint at least every minute, two recovery records, resume at boot, the save to `\Activities`. Next: the
   track, graphs and power totals of a resumed ride (only the ride's totals come back).
   Two copies of each ride (done in the simulator): the clean one for stock, Bryton and Strava, and a full one
   with DFA a1 (`Alpha1`) and RMSSD as developer fields for Intervals.icu (docs/sync.md, "Why two files").
   `tools/sync.py` uploads them from the computer and brings the planned workouts back. Next: a run against the
   real services; the iOS app (phase 8).
4. Settings file (phase 1's store moves here). Open, and now wanted: the 32 backup registers are full (docs/status.md,
   features.md "Settings registers"), so a setting added needs a file to free them, two copies with a CRC.
5. USB mass storage: rides off and courses, workouts and maps on, with no phone app. The recorder and the FAT
   writer stop while the computer owns the volume.

## 4. Ride data

Done (features.md, Power, HRV and the power curve): the ride metrics, W' balance, CP and W' fitted from stock's
rides with EST FTP / APPLY, the ride's CP / W' latched at its start, the power meter's calibration, the bike weight,
the live DFA a1 and RMSSD fields, the HRV messages in the full copy, HRVT1 and HRVT2.

- Next: torque pages (0x11, 0x12) and torque effectiveness / pedal smoothness (page 0x13). One FTP only: a CP setting
  apart from it, and indoor-only copies of the FTP, are dropped.
- Alerts on the buzzer and a banner: HR and power limits, eat and drink reminders by time, distance or kJ (stock
  has six kinds).
- Auto lap; ride history and totals for the overview.
- Power smoothing on the big power value, kept across restarts (patch P5).
- **Open: the DFA a1 artifact filter.** The live filter drops a beat more than 5 % from the previous kept one (hrv.rs). Compare it
  with Domestique's method (median of the neighbours, a 20 % rule, an R^2 >= 0.95 gate on the window) on a ride
  with raw beats recorded: pending a real ride with the strap (docs/status.md).

## 4a. An MTB mode (the owner's ask, 2026-10-27)

A third ride profile beside ROAD and INDOOR, chosen by hand (BIKE > PROFILE) and kept in the ride file (sub_sport
mountain), with what off-road riding needs:
- GNSS at full rate whatever the speed (the adaptive GPS POWER gives full tracking when slow, but MTB should not
  depend on it), and the track kept at a higher resolution for the map and the FIT records.
- Its own page set (as INDOOR has): bigger fields, fewer per page, readable on rough ground; climbing and descending
  first (altitude, grade, ascent and descent, VAM, a profile of the last minutes); time over distance; heart rate.
- Auto-pause for slow climbing and stops at trail junctions (a lower threshold, or time based), and auto lap by
  position if useful.
- Climbs ahead and the climb pacer on tracks and paths (the road classes stock's routing ignores are where MTB rides).
- Touch: larger targets and the touch lock by default while moving (gloves, mud); keys first.
- Later: suspension or dropper data if the sensors exist (ANT+ / BLE), and a descent flow field.
Done 2026-10-27 (owner's choices): the profile by hand in BIKE > PROFILE (INDOOR still wins from the trainer heard), sub_sport mountain kept in the recovery record, continuous GNSS during an MTB ride, auto-pause under 2 km/h for 5 s, a set of 4 pages (PAGES > MTB PAGES), climbs ahead following paths (stock's speed class 1). Still to do: touch for gloves and mud, a higher-resolution map track, descent and moving-time fields, suspension and dropper data.

## 4b. History and training load

From every ride on the shared volume (stock's and ours, AGENTS.md "Share the storage with stock"), as Domestique
(github.com/platypus45/domestique) and intervals.icu do on a computer, on the unit.
Done (features.md, History): the ride scan and its cache, the 90-day power curve for CP and W'.

- **A ride index:** per ride its TSS (from power: NP, IF and FTP; from heart rate when there is no power: hrTSS from
  the threshold heart rate), time in zones and best efforts, updated by each saved ride.
- **Power curve screen:** the mean-maximal power from 1 s to 60 min over the season and all time, with the date of
  each best.
- **Training load:** daily TSS, fitness (CTL: 42-day average), fatigue (ATL: 7-day), form (TSB: CTL - ATL), the ramp
  rate, weekly TSS against a target. Not built.
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
  France, with contour lines and hill shading built in: `tools/ign_pack.py` fetches an area (resumable) and packs it,
  `src/storage/ign.rs` reads it, `src/ui/tiles.rs` draws it, stock's map under it where it stops. The SCAN 25 and
  SCAN 100 rasters are not open to private individuals, so they are out.
- **IGN copy to the unit:** procedure in docs/install.md (IGN part). Then: a pack per region with a lower zoom limit
  so a whole country fits.
- **Notices** on an About page: "© OpenStreetMap contributors (ODbL), © OpenMapTiles", and IGN's when used (the map
  already shows "(c) IGN" in its corner whenever IGN tiles are drawn).
- Vector over raster: about ten times smaller, any zoom, and text that stays upright if the map turns.

## 6. Navigation and climbs

- **Done (sim):** courses from stock's `\PlanTrip` (Bryton's own route layout), Garmin FIT courses and GPX copied to
  `\Courses`: the line on the map, where the rider is on it (a moving window, a bounded search when lost),
  off-course alert, distance to go, ascent left; the course's elevation profile coloured by grade; **climb
  detection** (Garmin ClimbPro's definition: each climb's length, grade, what is left, a banner at its start).
- **Done (sim):** climbs without a course, from stock's routing maps (climbs_ahead.rs, BIKE > CLIMBS AHEAD); the
  climb pacer on a followed course (pacing.rs, the TOP field: constant-speed closed form from CP, W' and the weights);
  the CLIMB page, opened once at a climb's start (profile by grade, gain, length, grade).
- **Next:** the way back to the course drawn, navigate to the start, reverse a course.
- **Open: live segments without Strava**, from the rider's own rides: a segment found in a past ride (a climb, a
  stretch ridden often), the time against the best, on a field, with no service in between.
- **Open: stock's Climb Challenge**, compared with our climbs ahead on the same ride (the climbs found, their
  starts, the grades), to tune the rule or to say where they differ.
- Turn-by-turn cues (stock's `voiceTrip.fit` holds them: message 250 with street names); rerouting on the unit
  later.

## 7. Training

- Structured workouts from FIT workout files, with step targets, the 3-2-1 beeps and the next-step strip (patches
  P1, P3). ZWO, ERG and MRC files are converted to FIT on the computer.
- Smart trainer control (ANT+ FE-C, and over Bluetooth: FTMS, FE-C over BLE) for workouts indoors. Done (sim): ERG,
  SIM, release rules, spin-down calibration, the FTP ramp test.

## 8. Radios and the phone

- BLE sensors (heart rate, speed and cadence, power), then ANT+ radar (Varia) and lights.
- **Shifting and hood buttons** (small, ANT only): Shimano Di2 is not ANT+ but a private ANT channel on its own
  network key (stock: channel 6, device type 1, period 8198, 2457 MHz; the key sits next to the ANT+ key in the stock
  image and is copied the same way, never committed): gears and battery on page 0, the hood buttons (single, long)
  on page 4, sent as acknowledged messages, which `nrf::ant_broadcast` must accept too. SRAM AXS and Campagnolo use
  the ANT+ Shifting profile (device type 0x22: gears, up to 8 buttons, batteries). The buttons become `Input`s (patch
  P12 mapped ten actions), the gears a field.
- **Phone, no app needed** (done in the simulator, 2026-10-24; docs/status.md has the order to try it on the unit):
  one bonded BLE link, Just Works, one bond in `\Rustfw\ble.bin`:
  - media keys as a BLE HID consumer-control device (patch P7: play / pause, next, previous, volume), plus the camera
    shutter (volume up) and mute;
  - iOS notifications through ANCS on the banner, with accept / decline for a call;
  - iOS now playing (title, artist, elapsed) and its remote commands through AMS;
  - the clock and the time zone from the phone's Current Time Service.
  - Next: LE Secure Connections (pairing is legacy Just Works now), a whitelist of the bonded phone (its IRK is
    distributed but not kept), the ATT MTU raised (`sd_ble_cfg_set`) so ANCS answers come in fewer notifications,
    the notification's app shown (its bundle id is fetched and dropped).
- With a companion app only: Android notifications and track info, find my phone, live tracking, automatic upload; a
  computer tool over USB can upload rides.
- **The iOS app, next:** `tools/sync.py`'s logic (docs/sync.md) over the BLE bond instead of USB. It would list the
  new rides, fetch both copies with their FIT CRCs checked, upload the full copy to Intervals.icu and the clean one
  to Strava, remember what went where, and send the planned workouts back to `\Workout\calendar`. It needs a file
  transfer service on the bond (the Nordic chip's serialization, nrf.rs) and the FAT writer for the workouts.

## Plans, not built

- docs/s810.md: the Rider S810.
- docs/theming.md: themes from a file.
