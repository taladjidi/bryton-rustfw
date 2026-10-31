# Status

What this file holds: what the unit has not shown yet, the known problems, and the next steps.
What does not belong: what is built (features.md) and the roadmap (roadmap.md).
Where to look: features.md for what the firmware does, journal.md for the dated findings, docs/usb.md for the USB drops.

## Not yet seen on the unit

- **CLIMBS DONE (app/climbs_done.rs)**: sim only. Still to see: a real climb of a followed course or the road ahead
  listed at its top with a plausible time and VAM, the banner, the average power and heart rate, the list empty after
  a reset-resume.

A feature not listed here was seen on the unit.

- **The power curve screen (app/curve.rs, ui/curve.rs, HISTORY > FITNESS > POWER CURVE and the ride menu)**: sim only.
  To check on the unit: the 90-day curve after the scan, the green curve growing during a ride with the meter, the
  cursor by Page and by tap, both ends of the axis legible, and the redraw cost of opening it (`tools/dev.py info`).
- **Training load: CTL, ATL, TSB, the chart and MONTHS (training/load.rs, app/pmc.rs, ui/load.rs, HISTORY > FITNESS)**:
  sim only, on synthetic rides. To check on the unit: the numbers against Intervals.icu or TrainingPeaks for the same
  rides (TSS of the power rides should match within a point or two; the hrTSS of a ride without power will be lower),
  the chart after the scan, the three fields on a ride page moving as the ride goes, the month rows against the
  scan's totals.
- **Course reverse, the way to the start and BACK TO START (app/courses.rs, nav/course)**: sim only. To check on a ride:
  follow a course, REVERSE COURSE and ride it the other way (turns the other way round); start away from a course and
  see NEXT TURN give the compass point and distance until joined; ride out, BACK TO START in the ride menu, and ride
  back on the breadcrumb with its cues. A reversed or way-back course is not kept across a reset: after one mid-ride
  nothing is followed (the ride itself resumes).

- **Torque effectiveness and pedal smoothness (drivers/sensors.rs page 0x13, the TE L/R and PS L/R fields)**: sim only.
  The owner's Sigeyi spider PM may not send page 0x13 at all, so its fields would stay "--" for a whole ride. To check:
  pair it, pedal for a few minutes with the TE L/R box on screen; a value means it sends 0x13, "--" all ride means it
  does not (or its bytes are 0xff, one-sided). Then note which, here.
- **BATT HRS (sim only)**: the time left on the battery, from the drain of the last 20 minutes (drivers/battery.rs). Still to see: on a real ride from full, its value against the strip's percent over the first hour (the drain it reports should match the percent the strip loses, 1 h of ride to about 5 to 10 % at a riding draw, as `tools/dev.py bat` logs it), the value shown at 10 minutes (the first "--" ending) and while the unit is on the charger ("--"), and "--" on the idle overview, where the drain is under 3.4 % an hour until the cell has dropped.
- **QUICK STATUS (sim only)**: a tap on a plain field of a ride page opens six chosen fields. Still to see: the tap on a wet or gloved finger (a stray tap only opens a read-only overlay, 10 s), the six values updating each second, the picker in PAGES > QUICK FIELDS, and the choice kept after a restart (settings.bin bytes 158 to 164).

- **Screenshots and the fault log (app/screenshot.rs, app/faultlog.rs, storage/screenshot.rs, storage/faultlog.rs;
  hw.rs's marker now keeps the kind and detail)**: sim only, and both write the card (an `EMMC_WRITE=1` build). Still to
  see: SYSTEM > SCREENSHOT on the overview and on a ride page, then the drive lent and `\Screenshot\` opened on the
  computer (the picture as `tools/dev.py shot` shows it, the time it took); a fault on purpose (a `tools/dev.py poke`
  to an unmapped address: a CPU fault with PRECISERR or IBUSERR) and a panic, then after the restart
  `\Rustfw\Log\faults.log` read on the computer (one line each, the CFSR bits as named, the marker's register 29 read
  with `dev.py peek` showing bit 10 set); and a fault mid-ride: the line written only after the resumed ride is saved.
- **The lap list of a past ride (sim only)**: HISTORY > a ride > LAPS (tap the label or Page). Still to see: on a ride recorded here with Lap presses, the times and distances of its laps; on a stock ride, whether its laps carry average power, heart rate and speed (fitread.rs reads `lap` fields 8, 9, 13 or 110, 15, 19 as the SDK profile has them; stock's layout is not checked on a real file). The recorder's laps now carry their own avg / max heart rate (15, 16), cadence (17, 18) and power (19, 20) (fit.rs "Laps", sim only): still to see, a ride here with a meter, a strap and Lap presses, its LAPS showing W and HR, the same numbers in Intervals.icu's laps, and a restart mid-lap (the lap's averages across it, from the recovery record's bytes 192..228).

- **FONT = BARLOW (T3, sim only)**: DISPLAY > FONT cycles PIXEL and BARLOW, and the smooth text draws in the pixel cells. Still to see: the labels and values on the ride page and the menus with BARLOW (no box moves, the strip and the fields redraw whole once), and a restart with BARLOW kept (the file read, then the register mirror).
- **Heart rate intervals with a power meter paired (sensors.rs `each_interval_once`, radio.rs)**: sim only. Still to see: DFA a1 and the hrv messages on a ride with the strap and a power meter after the fix, each interval once (the count of hrv intervals against the beats).
- **Sensor names and the strap's battery (drivers/sensors.rs pages 7 and 80, app/menus.rs SENSORS row, app/fields.rs HR BATT)**: sim only. Still to see: the strap's percent on HR BATT (its page 7 on the owner's strap, ANT+ profile layout as openant reads it), a SENSORS row reading "POLAR HR" after the strap sends page 80, and the row back to "HEART RATE" after a forget.
- **Zones (training/ride.rs `Zones`, app/settings.rs `zones`, training/hrtss.rs, features.md "Zones")**: sim only. Still to see: ME > ZONES
  on the panel (11 rows, a bound edited and kept in settings.bin data 123..150), ZONES BY LTHR on a ride (bars, HR ZONE field,
  time in zone move with it), and the TSS field and the end card showing hrTSS on a ride with a strap and no power meter.

- **Start reminder and ready check (app/start_cues.rs, features.md "Start reminder and ready check")**: sim only. Still to see: START THE RIDE? and its beep a few seconds into a real ride's first pedalling (and not again for 5 minutes), the ready card's two lines fitting the banner with real sensor names, the "+N" cut of a long list of missing names, Shifting or Di2 listed only when heard, and the sensors it calls missing being the ones not yet heard at the press.
- **Climbs and dates (app/settings.rs, ui/overview.rs, features.md "Climbs and dates")**: sim only. Still to see: CLIMBS MEDIUM+ on a real road ahead (the climbs kept), CLIMB PAGE OFF with a climb due, and the history and workout rows in D/M/Y.
- **Units, sound, auto pause, trips (app/fields.rs, app/settings.rs, features.md "Units, sound, trips")**: sim only. Still to see: IMPERIAL on a real ride (the values and labels fit their boxes), SOUND OFF with an alert due, an AUTO PAUSE of 5 km/h at a stop, and a trip and the odometer growing after a save and surviving a restart.
- **Next-step strip and two targets per step (app/workouts.rs `ahead`, ui/strip.rs `draw_next_strip`, workout_file.rs
  fields 19 to 22)**: sim only. Still to see: the strip over the status strip for the last 9 s of a step and its
  countdown redrawn once a second without flicker, the status strip back at the step change, a banner over it, and
  a calendar workout from Bryton's app with a secondary target (whether its files carry fields 19 to 22 at all is
  not known).

- **Page switches, names and AUTO SCROLL (app/pagemeta.rs, `pages.bin` after the MTB part)**: sim only. Still to see: a page switched OFF skipped by Page and the swipes, MOVE UP / DOWN, a four character name in the list, all of them back after a restart (the new bytes at 489 to 511 of the sector, an older file read as all ON); AUTO SCROLL at 5 s on a ride; a swipe up from a ride page (after MORE) going to the overview.
- **AUTO LAP and the lap banner (app/riding.rs `auto_lap`, `lap_summary`)**: sim only. Still to see: a lap at each 5 km or 10 min on a real ride, its banner for 8 s, and the lap messages in the saved file on Intervals.icu.
- **Live segments, screens (app/segment_ui.rs, ui/segment.rs, step S4)**: sim only. Still to see: a ride over a known segment: the approach
  banner with the name, the CLIMB page opening by itself (and found again by paging), the delta ahead and behind, the segment's profile
  with the target's mark, the coach's gauge, the finish banner and the card, the round going on without the page; the page's redraw
  cost on the unit.
- **STORAGE and ABOUT (SYSTEM; storage/volume.rs, app/survey.rs)**: sim only. Still to see: TOTAL, FREE and RIDES against
  the owner's card (FSInfo's count against the FAT's count there), the survey's passes and their time awake on the unit,
  and the oldest ride's card opened from STORAGE and its DELETE.

- **Live segments, detection (app/segments.rs, step S3; no effort written)**: sim only. Still to see: the index built
  from `\Rustfw\Segments\` on the card 5 s after the start (no pass slower), the load at a start, the events (`dev.py` reads
  them), no lost ride on a segment's start or finish; AXI SRAM has 1.6 KB left.
- **Live segments, efforts (app/segments.rs, step S5)**: sim only. Still to see, with an `EMMC_WRITE=1` build: a segment
  ridden faster than its PR, the ride saved, then `\Rustfw\Segments\<id>.pr` and a line of `efforts.csv` written after
  the save only (never during the ride, nothing for a ride discarded), read back on the computer; the next pass over
  the segment with the `.pr` as its PR target (a delta about 0 at the same pace); stock still reads the card.
- **THEME AUTO (ui/theme.rs `DayNight`, app/themes.rs; theming.md 4b)**: sim only. Still to see: the switch at dusk
  (night under 20 lux for 30 s, day back over 50 lux for 30 s, at most one switch a minute), the screen redrawn once
  in each theme, and the cost of the light sensor measuring all the time while AUTO is on (its time awake, measured
  on the unit with the light on AUTO and with THEME off). Also the sensor with the screen white at 30 % backlight in
  the dark: the panel's own light reaching the sensor against the 20 and 50 lux thresholds (assumed: the sensor sees
  the room, not the panel).

- **The heart rate and power fields, POWER SMOOTH, LTHR and the balance average (training/ride.rs ZoneTime, app/fields.rs,
  app/settings_file.rs)**: sim only. Still to see: the zone numbers and times against the strap's zones on a real ride,
  the average balance against the meter's own figure, and LTHR and POWER SMOOTH kept through a switch-off and a switch to stock.

- **The sun, heading, arrival and battery fields (nav/sun.rs, app/fields.rs)**: sim only. Still to see: sunrise and sunset
  against the clock on a real fix (the zone learnt from the phone), the time to sunset, the heading on a ride, the ETA on
  a followed course, and the power meter's and the strap's battery status on the unit.

- **ZONE and SUMMER TIME (DISPLAY, app/clock.rs)**: sim only. Still to see: with ZONE +1 and SUMMER TIME EU, the
  strip at UTC + 1 after the last Sunday of March's change (2027-03-28 01:00 UTC) and the next October one
  (2027-10-31), without the phone setting the RTC; the RTC itself unchanged (stock shows it as is); a ride across the
  change keeping its name, its FIT times read in the Bryton app or Intervals.icu without a jump; the sun's times on
  the new hour.
- **The sunset alert (ALERTS > SUNSET ALERT)**: sim only. Still to see: "SUNSET SOON" and its three beeps on a ride about 30 minutes before the set, once, and not again after a pause.
- **The limit alerts and the distance, kcal and kJ reminders (ALERTS menu)**: sim only. Still to see: the buzzer and
  the red banner on the unit for a limit crossed on a ride.
- **The trip, lap and slope fields (training/ride.rs, app/fields.rs; the page with trip time, laps, last lap, lap power
  and NP, top cadence and altitude, descent, lap ascent and descent, uphill and downhill)**: sim only. Still to see:
  on a real ride, the trip time through an auto-pause and a Start pause, each lap's power and NP against the power
  meter's own figures, and the uphill and downhill split on a real descent (the 30 s grade lags about 15 s at a change).

- **The stack canary (review S8; link.x `_stack_canary`, main.rs `stack_check`)**: seen on the unit 2026-10-29 from a
  RAM run: `info` and `prof` as before (0.98 % awake idle on the overview), `dev.py stackfault` reset the unit and
  `peek 0x580040c4` read 0xfa000001. Still to see: a DIAG_STOP-free cold boot of an installed build with it (nothing
  changed in the boot, so low risk).

- **The main loop's policy in the library (app/loop_policy.rs, review A8)**: seen on the unit 2026-10-29 from a RAM
  run: 21 passes a second with the cable in, 0.98 % awake idle, a key acted on at once, the awake and CPU per mille of
  `info` sane after 40 s. Still to see: a real key press after 30 s idle waking the loop at once, a touch read during
  a USB drive copy, unplugging resetting the unit outside a ride and not during one (nothing in those rules changed,
  the code moved).

- **The recovery's chain check and the read-only drive (storage/recorder/mod.rs `still_ours`, the record's checked
  mark, app/drive.rs `left_open`; review S1 to S6)**: host tests only (cut at every sector, chains freed or reused by
  another file). On the unit: a ride left open, the unit restarted, the card lent to a computer (expected: USB DRIVE
  READ ONLY with "RIDE LEFT OPEN: START A RIDE"), then the ride deleted from the computer after a Start, and the next
  boot letting it go without touching the file that took its clusters; GIVE UP SAVE's new order (the link, then the
  record "done") needs a card that really fails, so it may never be seen.

- **The smaller repaints (review P1 to P5: the page's value boxes once a second, a banner's dismissal skipping the
  boxes under it, a menu value in its own box, the BLE list by row; app/screens.rs, app/menus.rs, ui/mod.rs)**:
  measured from RAM 2026-10-29: `draw` 125 to 52 us a pass on the overview, awake 1.10 to 0.90 % with the cable in.
  Still to see: the SENSORS and BLE NEARBY menus updating live with no flicker, a banner dismissed over a graph page
  leaving the graph whole, and the key figures of README.md measured again with tools/profile.py from RAM (they hold
  the 2026-10-29 morning numbers).

- **The app's state files made at a ride's start (storage/recorder/mod.rs `make_kept`; review S7)**: host tests only
  (a card without them, cut at every sector, plain and torn). On the owner's card the first ride start after this
  build writes `\Rustfw`'s directory sector once (the six files and `Deleted\Full` created zeroed; the files already
  there are left as they are). Still to see: that start, then `dev.py ls \Rustfw` showing all of them, and the pages,
  the bond and the sensors still read and saved afterwards.

- **Power match (training/power_match.rs, BIKE > POWER MATCH)**: an ERG ride with the Sigeyi paired: the power
  meter reads the workout's target within a few % after a minute, the "PM+x%" on the trainer screen, the correction
  held then eased back when the meter is unplugged, OFF sending the target as set.

- **Themes (ui/theme.rs, app/themes.rs; T1)**: OUTDOOR and LIGHT (T2) read in the sun (is either better than DARK on the transflective
  panel?); a theme file loaded from `\Rustfw\Themes` on the card (listed, picked, kept in settings.bin, applied
  after a restart); the redraw at the first pass when the settings file names a file theme (one flash of DARK).

- **The settings' file (`\Rustfw\settings.bin`, app/settings_file.rs)**: seen on the unit 2026-10-29 (RAM run,
  EMMC_WRITE=1). The first start created the file (seq 1). A FONT change wrote seq 2 into the other copy. After
  `dev.py reset` the older installed build b0861d8 booted, and re-running f5d7f3e kept the change. Changing it back wrote
  seq 3. Still to see: switch to stock and back, stock's settings and rides as they were, `\Activities\` and `\System\`
  untouched, and this firmware's settings back.

- **Idle loop woken by a key (BOARD.md 5, main.rs `wake_on_io`)**: seen on the unit 2026-10-29: 7 passes a second idle
  on the overview (`prof`, 1 s window, 0.91 % awake). Still to see: a real press of each key, after 30 s idle, acts at
  once (not up to 250 ms later).
- **GPS POWER AUTO (`gnss::Adaptive`)**: on the host only. On the unit, a road ride: `tools/gnss.py` CFG-VALGET of
  0x20d00001 reads 2 after 11 minutes fast and straight, 0 within a second of a stop or a hairpin; the track and the
  speed against FULL's (and a speed sensor's) on the same road; the share of the ride in cyclic tracking.
- **GPS POWER SAVER (u-blox cyclic tracking, `gnss::config`)**: on the host only. On the unit, outdoors: the
  configuration answered ACK-ACK (`tools/gnss.py`, CFG-VALGET of 0x20d00001 reads 2), NAV-PVT still read every second
  over SPI between the receiver's sleeps (no gaps: the strip's GPS stays green, no fix dropped after 3 s), the track
  beside FULL's on the same road, and the battery against FULL (docs/power.md, measurement plan). Also which
  constellations the receiver uses (CFG-VALGET of CFG-SIGNAL-*_ENA: the default or what stock left).
- **The ANT+ search rests growing (app/radio.rs `search_rest_ms`)**: a paired sensor left switched off: searches at
  0:00, 1:40, 4:10, 8:40, then every 4 min 40 s (the high-priority search may add 10 s each); found at once after a
  ride's Start; the battery with one absent sensor against none (docs/power.md).
- **MTB mode (2026-10-27)**: an MTB ride started, the unit restarted mid-ride, then saved; the file reads as mountain
  biking in Intervals.icu; the receiver switching from cyclic to continuous tracking at an MTB ride's start and back
  after it; climbs ahead on a real path network.

- **A USB suspend (lib link/dev.rs `watch_usb`, board/usb.rs `Link::watch`)**: decided on the host only; macOS
  would not suspend the idle unit while awake. With the owner: plug the unit in, let the Mac sleep for a minute, wake
  it. Expected: the dot grey during the sleep, no "USB RECONNECT", `tools/dev.py info` answers at once after the wake (same
  uptime, no new enumeration), backup register 31 (`tools/dev.py peek 0x580040cc`) unchanged.

- **The profiler (link/prof.rs, `tools/dev.py prof`, tools/profile.py)**: seen on the unit 2026-10-29: section overhead
  15 cycles; the rows sum to 1.508 % against 1.51 % awake. Idle overview 21.2 passes/s, 1.51 % awake; ride page 1 1.46 %;
  ride page 5 1.37 %; menu 2.69 %; ride discarded 4.98 %. Measured again in the evening on build 13fec4c after the review's repaint changes: idle overview 0.98 %, ride page
  0.93 %, menu 1.37 % (21 passes/s with the cable in); README.md holds these.

- **The ascent filter on a ride (altitude.rs, ride.rs)**: the median of 5, the 1/6 average, the slewed reference and the
  3 m hysteresis are tuned on the records of one real ride (341 m became 123 m on a real 60 km ride, offline); on the
  unit, a ride's ascent against a known climb and against stock's, with a calibration made at the start.
- **The sliced tile load (climbs_ahead.rs, storage/inflate.rs, storage/route.rs, app/ahead.rs)**: before the change one
  tile took up to 238 ms in one pass on a ride; the longest slice is estimated at about 3 ms (host instruction counts,
  scaled). On a ride, `tools/dev.py info` ends with "climb tile us last N max M slice max S": S is to be seen under a few ms.

- **The ramp test (ramp.rs, app/ramp.rs, WORKOUTS > RAMP TEST)**: only on the simulator. On the unit with the D900 or
  another trainer: the 5 min at 50 %, the one-minute steps in ERG, the stop on cadence under 60 rpm, on power under half
  the target, on Lap and on STOP WORKOUT (the cool-down target at once), the result (FTP at 75 % of the best minute,
  HRVT1 with a strap), APPLY and KEEP by touch, and the trainer released afterwards.
- **The climb pacer on a real climb (pacing.rs, app/pace.rs)**: only on the simulator. A course followed with a climb
  of 5 minutes or more: the TOP field within 5 km of it, the plan's power against what the rider can hold, the time to
  the top counting down, the W' balance staying above 0 at the top, the bike weight added; the replan's cost at
  200 MHz (about 75 us for 32 sections, computed on the host).
- **Raw RR intervals in a real ride's full copy (storage/fit.rs `Writer::hrv`, recorder.rs `Recorder::beat`)**: only on
  the simulator and the host tests. A ride with the owner's strap: the `hrv` messages in `\Rustfw\Activities` (about
  20 KB an hour), `ArtPct` on the records, `tools/fit_check.py` decoding them, and the beats offline in Domestique,
  Runalyze or Kubios (roadmap: the artifact filter comparison).
- **HRVT1 / HRVT2 and EST FTP from real full copies (hrvt.rs, storage/thresholds.rs, app/fitness.rs)**: on the
  simulator and fuzz only; FITNESS on the unit after a few rides with the strap: the thresholds against the rider's
  own, the time the 96 KB a pass reading takes, and the 3 KB in AXI SRAM.
- **The overview cog, the target gauge over all zones and the 8 px zone bars:** drawn in the simulator only; the 44 px touch box
  with a glove, the gauge's zones by eye in sunlight, and the bars' height on the bike.
- **The save fixes (fat/write.rs `make_room`, recorder.rs)**: host tests with a cut at every sector and a card with
  failing sectors; on the unit a save on a card with a full `\Activities` folder, and a Start while the last ride is
  saved ("NOT RECORDING: CARD", then the ride starting once the save ends).
- **GIVE UP SAVE (recorder.rs `give_up_save`, app/riding.rs)**: only on the simulator and the host tests (a card whose
  directory sectors fail to write or read). On the unit it needs a card that really fails, so it may never be seen:
  the row appearing in SYSTEM while stuck, the four banners, and `\Rustfw\Rescue` or the lost chain checked from a
  computer (fsck's `FILE0000.CHK`).
- **Level mode and the slope scale (training/trainer.rs `Mode::Level`, app/trainer.rs `trainer_drive`)** on the Van Rysel D900:
  LVL on the trainer screen: level 1 then 20 feel lightest and heaviest, +/- by LEVEL STEP, page 48 (and FTMS op 0x04
  over BLE) applied; a course followed in SIM with SLOPE SCALE 50 %: the trainer's resistance on the hill is about
  half of 100 %'s, and the grade stays inside -10 to +20 %. Sim only so far.
- **The trainer's release (app/trainer.rs, nrf.rs `ant_tx_completed`)** on the Van Rysel D900: the release at stop, end,
  pause and boot (once), and the **switch-off release**: one free resistance sent before the chip goes off, the D900 free
  in the next seconds with the ride saved (ANT+ and over Bluetooth), the 330 ms wait enough. `TX_COMPLETED`'s layout
  (`[0x40, ch, 0x01, 5]`, ANT Message Protocol 9.5.6.1) is read from the document, not seen.
- **Power meter calibration (src/drivers/powercal.rs, app/powercal.rs, SENSORS > CALIBRATE POWER)**: only on the simulator. The page 0x01 layouts (ids
  0xAA, 0xAB, 0xAC, 0xAF, 0x12; offset in bytes 6 and 7) are from the Nordic SDK and GoldenCheetah, the request
  bytes and the acknowledged send on the power channel (the same op as the trainer's, itself not yet confirmed) are
  inferred, and the 10 s time-out is ours. On the unit with the Sigeyi AXO spider: does it answer 0xAA with 0xAC (and
  which offset), does it report auto zero (byte 2), does a message to a sleeping meter fail cleanly; Sigeyi's own
  documentation of this was not found (retailers list auto zero and a manual zero offset only).
- **Climbs ahead (src/nav/climbs_ahead.rs, storage/route.rs, storage/inflate.rs, app/ahead.rs)**: only on the simulator,
  the host tests, the fuzz, and a real region file read on the computer (1009 of 1014 tiles decode, 5 are over the
  graph's 2560 nodes; traces from 199 random places: 194 made, median 4 km, 111 climbs). On the unit, with a map file
  of the region copied to `\Map\MapData\<Region>` (stock's own has them): a ride shows CLIMB and CLIMB M after the first
  seconds (the tiles read one a pass: how long a pass takes with a 445-node tile, inflated 16 KB into the SDRAM
  buffer, is to measure with `tools/dev.py info`; inferred 20 to 50 ms), the banner and the beep 50 m before a climb the
  road ahead has, the trace made again when the road bends away from the one traced; the climbs against what stock
  shows for the same ride; `tools/dev.py info` ends with "climb tile us last N max M": the time of the last and the longest tile read, inflated and
  decoded (estimated 15 ms at the median, about 80 ms for the largest tile that fits the 96 KB buffer, 2080 nodes);
  the AXI SRAM cell stays in `.bss` (`make_ahead`, about <!-- n:ahead -->106 KB<!-- /n -->) and the unit
  boots (a new static in `.axibss` is a core-level change: Renode, then `DIAG_STOP`).
- **The GNSS receiver off when idle (app/gps.rs `gnss_idle`)**: only against the simulator. On the unit: indoors,
  the strip's GPS goes blank 5 minutes after power-on; outdoors with a fix, 10 minutes after the last key; the GPS
  menu, a map page or Start bring it back, and the time to a fix then (expected about 30 s, the ephemeris not kept).
- **The GNSS receiver off for an indoor ride (app/gps.rs `gnss_switch`, BOARD.md 13)**: only against the simulator.
  On the unit: with a trainer heard, Start switches the receiver off (strip GPS blank), and after SAVE or DISCARD,
  or once the profile goes to ROAD, it is powered again 1 s later and gives a fix outdoors (how long, with the time
  and position aiding); the current saved to measure.

- **An indoor ride saved (src/storage/fit.rs sub_sport, recorder.rs)**: only on the simulator and the host tests
  (every sector cut). On the unit: an INDOOR ride (a trainer heard at Start, or the ride menu's INDOOR row), a
  restart mid-ride that comes back INDOOR, then a save; the file's laps and session with sport cycling and sub_sport
  indoor_cycling (6). Then synced: a Ride marked trainer on Strava, the indoor flag on Intervals.icu (docs/sync.md,
  "Indoor rides").
- **The home trainer, ANT+ FE-C (src/training/trainer.rs, drivers/fec.rs, app/trainer.rs; phase 1)**: only against the
  simulator's model of a trainer. On the unit with a smart trainer: SENSORS > TRAINER finds it (channel 7, type 17,
  8192, which is not stock's channel 8), its power, cadence and speed on the pages; the acknowledged send
  (`sd_ant_acknowledge_message_tx`, op 0xc8, `[6 c8 ch 8 data]`, inferred from stock's encoder, Ghidra notes
  agent_b2.md) is accepted by the chip; the capabilities page (54) comes after the request (page 70); the trainer
  takes pages 49 (ERG under a workout), 48 and 51 (SIM on a course; the trainer's speed advances the ride), 55; and
  its page 71 answers pass. The pairing is kept in `\Rustfw\sensors.bin` (with the Bluetooth trainer's); backup
  register 14 is the shifting systems', and a trainer an earlier build kept there is only moved to the file. Not seen
  yet: the chip's EVENT_TRANSFER_TX_COMPLETED (code 5; the layout `[0x40, ch, 0x01, 5]` is from the ANT Message
  Protocol document), which stops the release being sent again (five times at most without it).
- **The trainer in the full copy (storage/fit.rs "The trainer", app/riding.rs `trainer_sample`)**: only against the
  simulator's trainer. On the unit: one ERG ride on the Van Rysel D900 with the power meter, then read
  `\Rustfw\Activities\<name>.fit` (`tools/fit_check.py`'s decoders, or Intervals.icu's fields): `target_w`,
  `trainer_w`, `meter_w`, `fe_state`, `trn_stat`, `link`, `cmd_sent`, `ack_fail` every second the trainer is heard
  and none after it stops; the `device_info` with the D900's pages 80 and 81 (the software version's scale from
  openant is inferred); the clean copy in `\Activities` unchanged and still synced by Bryton's app.
- **Spin-down calibration of the ANT+ FE-C trainer (trainer.rs `Cal`, drivers/fec.rs pages 1 and 2, app/trainer.rs,
  ui/trainer.rs)**: only against the simulator's trainer. On the Van Rysel D900 showing CALIBRATE: CALIBRATE beside
  MODE (or Start held) sends page 1 with the spin-down bit (0x80), and the trainer answers with page 2 (the layout is
  GoldenCheetah's, from the profile rev 4.1, fec.rs doc; the temperature and speed condition bits and the target speed
  in 0.001 m/s are to be seen on a real trainer); the screen says PEDAL TO nn KM/H, then STOP PEDALLING, then
  CALIBRATED with the spin-down time from its page 1; the status bit clears afterwards; the request's reserved bytes
  and a request with both bits (when the trainer asks for the zero offset as well) are inferred; that the trainer
  leaves the calibration by itself after CANCEL (nothing is sent) is inferred; the ERG target comes back after.
- **The two links' configuration and the Bluetooth trainer, FTMS (nrf.rs `start`, drivers/ftms.rs, app/ftms.rs;
  phase 2)**: only against the simulator's chip and trainer. On the unit, in this order:
  1. `sd_ble_cfg_set` (`00 69 20 00 00 00 01 01 02 03 00`, then `00 69 40 00 00 00 01 01 01 00`) answered 0 and
     `sd_ble_enable` after it answered 0 (else the chip is reset and started as before: `tools/dev.py info` would show
     two resets at the start, and SENSORS' BT TRAINER says NO LINK);
  2. the phone still pairs and comes back, advertising on tag 1 (the PHONE menu, media keys, a notification);
  3. SENSORS > BT TRAINER lists a real FTMS trainer (its service data or its UUID list), Select connects to it
     (`sd_ble_gap_connect` tag 1, stock's layout) **while the phone is linked**; its data on the pages; ERG under a
     workout, SIM on a course (the Control Point's responses: the trainer screen does not say NOT ACCEPTED);
  4. the trainer switched off and on: the link made again; the phone unaffected throughout.
  Inferred and to check there: the cadence bit of Indoor Bike Data read as present when set (pycycling's reading,
  the specification's table says the opposite), a resistance level spread over the trainer's own range, Cw 0.51.
- **The Bluetooth trainer without FTMS, FE-C over BLE (drivers/fecble.rs, app/ftms.rs; phase 2)**: only against the
  simulator's D500 (`FtmsSim::new_fec`). On the unit with the owner's Van Rysel D900 (it has no FTMS;
  2026-10-26 from a computer: name `VANRYSEL-HT-xxxx`, 0x1818 and 0x1816 and the 128-bit 6e40fec1-b5a3-f393-e0a9-
  e50e24dcca9e in its advertising or scan response, characteristics 6e40fec2 notify and 6e40fec3 write):
  Seen from the computer the same night (bleak, no bike on it): its notifications are `A4 09 4F 00 <page> <xor>`
  (message 0x4F, channel 0, ANT's XOR checksum), pages 16 and 25 at about 4 Hz and 81 now and then; page 25 says
  calibration needed (byte 6 bits 4 and 5); a page 70 request for 54 or 71 got no answer on channel 5 or 0, so the
  capabilities may never come over BLE; whether a target (page 49) is applied needs the bike on it.
  1. SENSORS > BT TRAINER lists it as `FE-C` (the 128-bit UUID may be in the scan response only: the search waits
     for it); Select connects; `sd_ble_uuid_vs_add` for the base answers 0 (the phone link took 8 of the 10
     vendor UUIDs; a duplicate answering the existing type is inferred, not seen) and the service is found;
  2. the notifications on 6e40fec2 give pages 16 and 25 (power, cadence, speed on the pages); the capabilities
     request (page 70) is answered by page 54;
  3. which checksum the trainer takes: the frame is `A4 09 4F 05 <page> <checksum>`; the XOR of all the bytes
     (ANT's) is sent until a notification shows the sum of the bytes after the sync (pycycling's), which is then
     used. If the trainer ignores the writes, this is the first thing to look at (a write with response, one per
     250 ms; its ATT answer would say refused);
  4. ERG under a workout (page 49), SIM on a course (page 51), the trainer's page 71 answers pass; the link made
     again after the trainer is switched off and on. Pairing keeps the protocol in `\Rustfw\sensors.bin` (`F`).
- **The sleep power steps** (main.rs `sleep_power_init`): seen on the unit 2026-10-25 from RAM, masks 0, 8, 4, 1, 3
  and 15 (boots, dev link, BLE, keys, a full redraw at 16 MHz clean, PLL3 off, a ride recorded and discarded by keys;
  with 15 the owner checked touch, backlight, the USB drive and the power button). Still to see: a GPS fix outdoors,
  the start / pause beeps, and the gain (battery draw idle on the overview, before and after).
- **The end-ride screen (app/riding.rs `end_input`, recorder.rs "A discarded ride")**, with an EMMC_WRITE=1 build:
  a short ride, Start, Start held: END RIDE, the time and distance, RESUME selected; Page and Lap move, Back and
  RESUME leave it paused on its page; taps with gloves on each button. SAVE: the card (it stays on Page and Lap,
  goes on Back or Start), `tools/dev.py ls Activities` and `Rustfw/Activities` have it, the history lists it. Another ride,
  DISCARD: "DISCARD RIDE?", KEEP selected; DISCARD: "RIDE DISCARDED", `tools/dev.py ls Rustfw/Discarded` and
  `Rustfw/Discarded/Full` have it, `\Activities` not, the history and the totals as before; `tools/sync.py` (dry
  run) does not list it. A restart (`tools/dev.py reset`) with the screen up: "RIDE RESUMED", paused. The first ride after
  this build makes `\Rustfw\Discarded\Full` at its start (the FATs compared first, a few seconds).

- **The Nordic link watchdog (app/nrf_watch.rs)**: `tools/dev.py info` ends with "radio resets 0" over a normal ride and a
  quiet hour (no sensor, no phone); with a strap and the phone linked, a reset of the chip from the dev link (`poke`
  of its reset line, not a DFU command) brings both back within a minute and "RADIO RESET" once; the ride keeps
  recording through it.
- **Barometric altitude (src/nav/altitude.rs)**: the sign and size of the pressure swings from wind at speed (the 3 m
  ascent hysteresis is inferred, BARO_HYSTERESIS_MM), ALTITUDE and FROM GPS in SETTINGS > BIKE against a known
  height, the ride start's own calibration, `\Rustfw\altcal.bin` kept over a restart, the chip's current at a reading
  a second (datasheet, not measured), `Alt_P0` of a real DeviceData.ini.

- **UI review 2, step A (sim only so far):** the touch lock (Back and Lap held together 1 s: the padlock in the strip,
  a tap on the gear ignored, again to unlock; try it with a glove: the two keys are on opposite sides), Back on the
  overview doing nothing, the zone bars (the 8 px bar and its value margin with a real power meter and strap: legible in
  sunlight? the teal, green, olive, brown, wine, purple and slate zones by eye), the readings once a second, the 48 px map
  buttons, the targets as "210" and "+-10", the confirmations "START: YES BACK: NO".
- **DELETE in HISTORY (storage/trash.rs)**, with an EMMC_WRITE=1 build: a ride's card, Start held (or a tap on
  its red DELETE), then Start: "RIDE
  DELETED", the list without it, the totals lower at once; `tools/dev.py ls Rustfw/Deleted` (and `Rustfw/Deleted/Full`
  for one of this firmware's rides) has it, `\Activities` not; the next start's totals the same (the cache written
  without it). Then, on stock: the ride gone from its history and nothing else changed (its `.sum` / `.csum` left in
  `\System\History\Summary`: whether stock lists a summary without its FIT is inferred from Ghidra, not seen);
  moved back from the computer, the ride back in both histories.

- **The page editor and `\Rustfw\pages.bin` (app/editor.rs, page.rs)**, with an EMMC_WRITE=1 build: a hold on a
  ride page's box opens the editor (the frame, the hint in the strip's place, live values); SPLIT, MERGE, TALLER,
  UNDO, a field; Back keeps, DROP CHANGES asks; ADD PAGE up to six, the Page key round them all; DELETE PAGE. Then
  `tools/dev.py ls Rustfw`: `pages.bin`, 1024 bytes. The first write compares both FAT copies first (as the scan's cache:
  some 10 s of passes, inferred; the loop stays responsive). Restart: the pages as made (the file read 2 s after the
  start: a page shown before that is the registers' copy). Not while a ride records: no editor by a hold, no EDIT
  THIS PAGE in the RIDE menu until paused. The installed stock-switch case: stock and an earlier build of this
  firmware still read pages 1 to 3 from registers 20 to 25 (layouts of up to six fields).
- **The scan's cache (storage/stock.rs `Cache`, `\Rustfw\scan.bin`)**, with an EMMC_WRITE=1 build: the first start
  reads every ride as before (`tools/dev.py info` awake permille during it: now at most about 15 ms of work a pass, so
  lower than the 717 seen; Back held 3 s switches off during it), then `\Rustfw\scan.bin` is there (64 + 64 bytes a
  ride; `tools/dev.py ls Rustfw`). The next start: the overview's totals at the first screen, no 30 s of `--`, and the
  scan 30 s later reads nothing (`tools/dev.py info` stays low). A ride saved: the next scan reads only it. A ride deleted
  from the computer: gone from the totals. The first write of the cache (also after a cache that did not check
  out) first compares both FATs (sectors 0 to 29,807 of each, about 15 MB in all: 3 to 4 s in one go at 10.5 MB/s),
  16 sectors of each a step, within the pass's 15 ms (`Cache::prepare`): some 10 s over 160 passes, inferred; check
  the loop stays responsive (`tools/dev.py info` uptime, a key) and the time it takes. A ride saved during the scan is
  read (its FAT sector read again each pass); a read error leaves a ride out of the totals until the next start,
  not for good. An emptied `scan.bin` (from the computer) is written again with its first cluster (fsck clean).
  A key held while the computer copies to the drive counts its real time (Back held 3 s switches off, not 2 s).
  `tools/dev.py bench scan` now reads only rides not in the cache: measure it with the cache deleted.

- **Electronic shifting (sensors.rs, app/radio.rs)**, all simulated only, the owner's Di2 at hand: with
  `di2_key.bin` built in (`tools/ant_key.py`), SENSORS > DI2 reads OFF (not NO KEY: the chip took network 1's key
  and channel 6's assignment); Start finds the Di2 within seconds (turn a crank or shift to wake it), its number
  shown green. GEAR shows front-rear and follows the shifts (with CHAINRINGS and CASSETTE set in DI2 KEYS it shows teeth, "50x17": check front 1 is the smallest ring and rear 1 the largest cog, both inferred; stock reads the teeth from the Di2 info record, `Sensor_Di2StoreGearConfig` 0xd02889f2, layout not decoded: open fact) (check front and rear are not swapped: inferred from
  stock's gear ratio code); SHIFT BAT shows the percent stock's Sensor Details showed (60 % on the patch project's
  picture; the byte is inferred to be a percent). DI2 KEYS: set R PRESS to NEXT PAGE, press the right hood button
  once: one page; a long press: its action; the left button too (the sides inferred from stock: byte 1 right,
  byte 2 left); START set on a button, restart the unit after a press: the ride must not start by itself (the
  Di2's first button page is taken as the baseline, inferred: it repeats the last press byte). A ride with shifts: SHIFTS counts them, and the saved file has gear change events (fitdecode, or
  Intervals.icu's gear chart). A Di2 switched off (or out of range): GEAR goes to "--". An ANT+ Shifting system
  (SRAM AXS) if one can be borrowed: SHIFTING row, gear, battery status.

- **The phone link (BLE, app/phone.rs)**, in this order, the dev link up (`tools/dev.py run`, then the screenshots), an
  iPhone at hand. A build run from RAM without EMMC_WRITE=1 keeps the bond only until a reset: after one, forget
  "S510" on the iPhone before pairing again (the iPhone keeps a key the unit no longer has).
  1. **Set-up:** MENU > PHONE: LINK reads OFF (the chip took every GATT set-up command: services, characteristics,
     the advertising data, the 8 vendor UUIDs). ERR xx/yyyy instead names the command refused and its error (for
     example A2 characteristic add with 0004 NRF_ERROR_NO_MEM: the attribute table is too small without
     `sd_ble_cfg_set`, inferred). The heart-rate strap still reads (ANT+ unchanged).
  2. **Advertising:** PAIR: LINK PAIRING; the iPhone's Settings > Bluetooth lists "S510" under Other Devices within a
     few seconds (the advertising data: HID and battery services, ANCS solicitation; the name in the scan
     response). The BLE NEARBY scan of a second unit, or nRF Connect, sees the same.
  3. **Pairing:** tap S510 on the iPhone: "Bluetooth Pairing Request ... S510 would like to pair with your iPhone",
     Pair (Just Works, no code); then iOS asks whether S510 may receive the iPhone's notifications: Allow. The unit:
     banner PHONE PAIRED, LINK SECURE, the clock set to the iPhone's time (and the date), Settings > Bluetooth shows
     S510 connected. Note the order the events came in if pairing fails (the security request at connection: is
     the iPhone's prompt shown without opening a HID characteristic?).
  4. **Media:** start music on the iPhone; SENSORS > PHONE > REMOTE shows its title, artist and PLAYING with the time running;
     Start pauses (AMS), Page next, Lap previous, held Page / Lap the volume (the iPhone's volume HUD). Hold Lap on
     a ride page: play / pause. The MUSIC field.
  5. **Camera:** open the iPhone's Camera, hold a finger on the REMOTE screen: a photo (HID volume up). If the banner
     says KEYS NOT ENABLED, the iPhone did not subscribe to the HID report: note whether it lists S510 as a
     keyboard.
  6. **Notifications:** with the iPhone locked, a message from another phone: the banner shows "sender: text" within
     a second or two; the overview's notices keep it. NOTIFY OFF, then a message: nothing.
  7. **Calls:** call the iPhone: the CALL screen with the caller's name; Start answers it (on the iPhone too); a
     second call, Back declines; a third, answered on the iPhone: the screen closes.
  8. **Reconnection** (an EMMC_WRITE=1 build, the bond written: `tools/dev.py ls Rustfw` lists ble.bin, 512 B): switch off
     and on: 3 s after the start LINK WAITING, then SECURE without any prompt on the iPhone; media keys work at once
     (the CCCDs given back). Walk away until the link drops: LINK WAITING, and back in range it reconnects.
  9. **FORGET:** LINK OFF, nothing advertised; on the iPhone, forget S510 too.
  9a. **Re-pair and strangers** (2026-10-24 review): bonded, forget S510 on the iPhone only, then PAIR on the unit:
     the iPhone pairs again without FORGET on the unit. A second phone (not paired) taps S510 while the unit only
     waits (no PAIR): pairing refused, and within 30 s the unit drops it (LINK back to WAITING).
  10. **Cost:** `tools/dev.py info` awake time with LINK OFF, WAITING and SECURE (the Nordic chip's own current is not
      measurable here; estimated 10 to 15 uA advertising once a second).
  11. **The RTC set** (`Board::rtc_set`, RM0399 init mode): the time right after the phone set it, and kept through
      a power off; stock afterwards still shows the right time.

- **The full copy of a ride and the sync** (docs/sync.md): `\Rustfw\Activities` written beside `\Activities`
  on the card, with Alpha1 from a real strap; `tools/sync.py` against the real Intervals.icu and Strava (DFA a1
  charted from the upload), and the workouts it writes listed in WORKOUTS (and by stock, which may want the `.json`
  beside them).
- **Map HEADING UP and PINCH (DISPLAY > MAP, touch.rs `feed2`)**: sim only. Still to see on the unit:
  - heading up: on a ride, the top follows the course in steps, stays put at a stop, the red north arrow points the
    right way; the time awake of one step (`tools/dev.py info`, DWT cycle counter). The sim says one map redraw of
    79 k panel pixels about every 18 s on a winding ride. On an IGN view add `raster_turned`: about 25 cycles a pixel
    over the tiles' boxes, so about 2.5 M cycles (6 ms at 400 MHz), estimated by counting operations (two divisions
    a row, then adds), not measured;
  - two fingers (pinch): whether the chip reports both (slot 2 at 0x16, inferred from the protocol doc and 0x3f = 5)
    and a pinch steps the zoom one level; the second finger must show in 2 samples in a row.
- **IGN maps** (`\Rustfw\Maps`): the packs found, a view drawn from them, its time (simulated: one index sector and
  about 45 sectors of tile data per tile, at most 6 tiles, so about 150 KB a view change, as stock's basemap; the
  decoding is 87 us a tile on the computer, about 3 to 6 ms on the unit with the D-cache off, inferred), the
  "(c) IGN" corner, the 2.1 MB map memory zeroed at boot.
- **Sensors other than the heart-rate strap:** speed, cadence, power (the decoders follow the ANT+ profiles,
  checked against openant).
- **The automatic backlight by eye:** stock's curve exactly (its setting 1: 8 % in the dark, 20 % at dusk, 30 % to
  50 lux, the 1 % minimum above 50 lux; a band chosen once a second, no hysteresis, 2026-10-27): is the panel readable
  in full sun and in a lit room at 1 % (a desk lamp read 480 lux on 2026-10-23), and does a light on a band's edge
  make the level step visibly?
- **Backlight levels** (60 / 30 / 10 %) and the time-out, each by eye.
- **The map on a real ride:** its awake time, and the last-known-position dot after a power cycle.
- **The map's zoom buttons** ("+" and "-" on the full-page map, now any level, coarser than the fit too, and the
  crosshair back to the fit): hit with a gloved finger indoors (the fit at 2 m: "-" must zoom out, the drag pan),
  the time of a re-centre over IGN at 1.5 and 2 m a pixel (`tools/dev.py mapbench`), and whether one every 26 to 35 s
  is acceptable awake time. IGN zoom 16 drawn at 0.85x at 2 m (before: zoom 15 at 1.7x): sharper, or one-pixel
  lines broken by the shrink?
- **Panning the zoomed map** by a drag: the drag read from the panel's points (no swipe taken for a page change while
  zoomed in), the time of the redraw at the release (a basemap render plus the whole map, about 79 K pixels in the
  simulator), the crosshair button.
- **The basemap** (roads and water from stock's cached tiles, `\Map\Data\Temp`): the FAT volume mounted by our
  reader, the tiles at the last known position, the time a view change takes (simulated with real tiles: about
  140 KB read, 84 K buffer pixels, 73 K panel pixels), the 1.5 MB SDRAM zeroing at boot (`hw::map_memory`).
- **HISTORY > FITNESS on the unit:** how long READ RIDES takes over stock's rides (about 175 KB a ride on one unit's
  backups, so about 35 MB for 200 rides; the eMMC read speed and the parsing time per byte from SDRAM code are
  not measured: estimated 10 to 20 s at 256 KB a pass), the menu's response meanwhile, and IMPORT against the
  unit's own `\System` files (checked on the host against two backups); the first start over stock's volume
  (its import by itself, the eMMC identified at boot then).
- **Workouts on the unit:** WORKOUTS over the card's own `\Workout\calendar` (read on the host from one unit's
  backup: Bryton's files decode, names cut at 15 characters), a workout ridden through (the beeps' loudness and
  pitch, 2 kHz and 2.5 kHz, by ear; the banners), and the time a list takes to read (16 workout files of 8 KB at
  most, about 50 ms, inferred from the scan's 2.5 MB/s).
- **Courses on the unit:** COURSES over the card's own `\PlanTrip` (the 14 routes of one unit's backup read on
  the host: every one decodes, lengths within 0.25 % of the files' own, climbs close to the ones Bryton's app marked),
  the time the lengths take to come (each file read whole, 96 KB a pass: about 550 KB for those 14, some 6
  passes), a course ridden (the window keeping up at speed, the off-course alert in a town's parallel streets, the
  climb banners, a lap course rejoined after a detour). Turn by turn: the cue banners and their beeps at speed
  (200 m, or 20 s if farther, then 32 m or 4 s, on 1 s fixes), the turns found on a real route's shape (no cue on a
  winding road or a switchback climb, every junction where the road turns 45 degrees or more at a corner, a real
  roundabout's exit right, counted in either direction, and no EXIT at a junction's jog, kinks or a swerve; how
  many junctions taken in a wide sweep go unsaid), a
  Garmin Connect, Komoot, RideWithGPS or BRouter course's cues (BRouter's roundabouts, its Locus style's cues on
  the track points), the arrows and the ring readable at 2x in the banner and 3x in the field, END at the field
  past the last cue, the time a course's cues take at its read (host: about 1 ms for 4000 points and 256 cues; a
  waypoint off a lap course bounded to 8 chunks).
- **The I-cache from a cold boot** (seen from RAM).
- **The Nordic chip after a power off and on** (`sd_power_system_off`, then the reset pulse).
- **Charging control:** the charger is on whenever USB is in (as the bootloader leaves it), with no temperature
  window yet (BOARD.md 12; the barometer's temperature can now feed it). The 3.3 V switch-off not reached.
- **The USB drive (mass storage, msc.rs): to do, high speed.** The drops, what was seen and what to try are in
  docs/usb.md. Not yet: Windows, Linux, unplugging while mounted.
- **The OTG_HS IRQ waking WFI** (the link may run on the 50 ms SysTick wake-ups alone).
- **The touch panel with a finger** (the chip itself answered: BOARD.md 17): INT low while touched, the axes
  (x 0 left, y 0 top expected), the gestures (swipe left / right between pages, hold for the menu, a tap on a menu
  row), the backlight waking on a touch, and the swipe thresholds by feel (stock's: 37 px across, 48 px up).
- **A power cut mid-ride** (the battery connector or a hard power off): the reset mid-ride and the resume are seen
  (2026-10-24, below); a cut while a sector is written is covered by the host tests and the card's reliable write.
- **Stock reading this firmware's rides** (\Activities\261024083403.fit, 261024083931.fit) and the Bryton app's sync.
- **The power button starts the unit after a switch-off:** fixed in ea359a0 (IRQ 149 enabled before standby), seen
  on the unit 2026-10-25, installed and unplugged.
- **AUTO OFF** (SYSTEM > AUTO OFF): switching off idle on battery confirmed by the owner 2026-10-28 (b0861d8; before
  it, a powered trainer heard held it on). Still to see on the unit: 5 min idle on battery gives the banner and beeps at 10 s and
  5 s, a key or touch cancels, the unit powers off; never with a ride or recorder busy, a cable in, the drive lent, a
  write waiting, or movement; the power button starts it again; the setting survives a restart (register 18 bits
  22..24).

- **New since the installed build (2026-10-24), seen only from RAM or in the simulator, one line each:**
  - touch gestures by feel and their axes;
  - the overview and its notices;
  - banners (ride start, pause, lap, end);
  - the page layouts;
  - graphs (five plots by tap, the whole ride by hold);
  - derived power fields and W' balance on a ride with a power meter;
  - DFA a1 and RMSSD from the owner's strap (needs a few minutes of real beats);
  - the AUTO backlight on stock's curve, outdoors.
- **Pages moved off backup register 19** (stock reads it at boot): the first start of a new build moves them; then
  stock, reinstalled, should keep its last battery voltage as before.
- **Added 2026-10-27, one line each:**
  - the power-average tap during a ride;
  - the CLIMB page appearing at a climb (course and climbs ahead);
  - the map's 1 km default view following the rider, its redraw cost on the unit (`tools/dev.py prof`);
  - the crosshair toggling the whole route;
  - the cog opening settings during a ride.
- A tap on the panel after the stuck-IRQ fixes (touch IRQ 23, panel DMA IRQ 11, main.rs `sleep`).
- The flattened menus and the quick panel (f935c0f), and refactors B1 to B3: only booted and profiled.
- The panel at 50 MHz SPI: an owner's look for noise or shifted pixels; if any, MBR /4 (25 MHz) in `spi_init`.

## Known problems

- **COURSE page while riding:** the screenshot differs from the panel at row 137, x 4 (found 2026-10-27, the consistency check is skipped on that page in `climb_page_in_the_round`). The map test's course pixel count varied (83 to 281) between runs of the simulator, which should be deterministic: to look into.

## Next steps

1. **Stop mode** between passes: plan and wake sources in docs/power.md (Stop mode: plan); the 250 ms idle loop is
   merged, still to see on the unit.
2. **USB high speed** for the drive: the drops after 15 to 40 MB (above); full speed works.
3. **A settings file** to free the backup registers (docs/roadmap.md).
4. Everything under 'Not yet seen on the unit', on the unit, as the owner rides it.
5. D-cache on for the SDRAM (hw.rs PanelDma cache contract), the next drawing gain.
