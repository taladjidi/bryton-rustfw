# Journal: what the unit taught us

Findings on the real hardware, newest last. Each one is in BOARD.md with its evidence; every write to the unit is a
row in bryton-S510-mods' FLASHLOG.

## 2026-10-22: the first install stops before the first frame

- Installed through the stock updater; the OS area read back equal to `out/IMG.bin`. Dark screen, no USB.
- **The backlight is no readout:** v1 to v8 drove PG13 as a plain output; nothing they showed can be trusted.
- **The signal that works (diag v9):** the image waits for USB power, then resets; holding Page while plugging in
  then gives Engineer Mode. Control: without the reset, no Engineer Mode. So our code runs.
- Checkpoints at chosen stages (v10 to v16) narrowed it down: a long SPI write after the panel power-on never
  completes; SPI1 took 16 bytes (its FIFO) and never clocked. Renode ran the same images to the end.

## 2026-10-23

- **SPI1 mode fault:** CR1.SSI must be set before CFG2.MASTER, else a mode fault clears MASTER (found with
  `dev.py peek` on v17, which skipped the panel and started USB). Fixed: the clock shows, the dev link works.
- **Power button:** PA0 (WKUP1) must be a GPIO input, or the press does not wake the unit from standby.
- **Caches off at hand-off:** the bootloader turns both caches off before jumping; the firmware runs uncached
  until it turns the I-cache on (now done, seen from RAM: ride page 0.5 % to 0.2 % awake).
- **Nordic chip:** BLE and ANT over USART2 (nRF5 serialization) answer; a BLE scan lists the devices nearby.
- **A link command that sleeps** inside the main loop's 50 ms sleep (a key press, then an eMMC read) took over
  SysTick and froze the loop while USB still answered: such commands now end the sleep.
- **Unaligned reads from the SDRAM return wrong bytes** (a 7-byte constant copied wrong; found by reading the
  app's state in DTCM with `peek`): built with `+strict-align`.
- **Heart rate and pairing:** "HR 58" to "HR 70" from the owner's strap (once wetted); the strap's ANT ID kept in
  an RTC backup register through a reset.
- **Battery:** ADC3 against VREFINT as stock; four bring-up quirks (calibration 82 ms, channels before the
  calibration, ADEN retried, a RAM run needs ADC3 reset first).
- **GPS:** a u-blox M10 on SPI4 that already sends NAV-PVT after stock. After a reset its power line must be held
  low over a second before power-on, else it never starts. Outside: a fix, and with the time and last position
  given at power-up, a much faster fix after a power cycle.
- **Redrawing:** redrawing only what changes cut the main screen from 17.5 % to 0.4 % awake (`dev.py info`).
- **Pixels two per 32-bit SPI write:** the panel shows them right (by eye).
- **eMMC (read only):** 31.3 GB; the OS area equals the installed image; sector 0 is a FAT32 boot sector, no MBR.
- **Sensor bus (I2C4):** LTR-308ALS light sensor, LPS22HH barometer, LIS2DH12 accelerometer; stock's automatic
  backlight dims to 1 % above 120 lux (by design), so ours rises with the light instead.
- **Beeps** at power-on and power-off: heard.
- **The cycle counter stops in sleep:** DWT CYCCNT does not count during WFI (the core clock stops; DBGMCU_CR is 0),
  while SysTick does. A main loop that timed its sleep with CYCCNT never woke by itself: passes went on only when a
  key or a touch woke it (found by `dev.py info`: uptime stuck at 0). The sleep is now timed by SysTick; the awake
  figure, which counts CYCCNT, was right all along.

## 2026-10-24: what a lost ride left on the card

The 2 h 40 ride of 2026-10-22 (lost to the P9 patch's crash at the start and at the save, AGENTS.md) was looked for
on the eMMC, read only, with new dev link commands (`ls`, `get`, `find`):
- **Nothing to undelete:** stock's saved file for that day is 3.6 KB; its ride in progress (`\Activities\tmp.fit`)
  and recovery files (`\System\tmp*.fit`) are gone, and **stock zeroes the directory entries of deleted files**
  (the four deleted entries in `\Activities` are all zeros), so neither the start cluster nor the size survives.
- **The GNSS logs did not carry the ride:** the `.ubx` logs of that day are 9 KB and 1.1 KB (the restarts); logging
  did not go on after the second one.
- **Carving the free clusters** for the ride's FIT timestamps (the method checked on Tuesday's ride: 10 to 17 hits a
  sector) found nothing near the clusters stock was allocating then; the full scan was stopped (the owner: move on).
- **For this firmware's recorder (the golden rule):** keep the ride in progress until the finished file is written
  and read back whole; finish by rename, never by copy and delete; keep the recovery record and the previous one; do
  not zero deleted entries, so a file can still be found by hand; write the GNSS position into the ride file itself
  as it goes (a side log that stops is no backup); and the host tests cut power at every write of the save itself,
  not only during the ride.

## 2026-10-24: the recorder, in the simulator

The golden rule's code, written and tested on the host only (nothing ran on the unit; eMMC writes wait for the
owner):
- **eMMC writes** (`emmc.rs` `Card::write`, main.rs `emmc_write`): CMD24 / CMD25 + CMD12, CMD13 until the card has
  programmed, a refusal of anything at or past B before a command goes out.
- **FAT32 writes** (`fat/write.rs`): its module doc gives the write order and why a cut at any point leaves a volume
  stock and a computer accept. A move is delete (0xe5 only) then link, so a cut between leaves a lost chain that the
  recovery record names, never two entries on one chain.
- **The recorder** (`recorder.rs`): the ride in `\Rustfw\YYMMDDhhmmss.fit`, about 30 bytes a second into a 2 KB RAM
  buffer, written at a checkpoint (60 s, a pause, a low buffer, the end); recovery records alternating between the
  two sectors of `\Rustfw\ride.rec` with a sequence number, the phase and the file's name and first cluster; the
  save reads the file back, then moves it to `\Activities`. Backup register 30 marks a ride open, so a boot without
  one does not touch the card.
- **Tests:** a 15-minute ride with a pause and a lap, the power cut after each of its 191 sector writes (the folders
  made, the checkpoints, the save), every reboot also cut at each of its own writes (737 more); each time the volume
  stays readable, the reboot gives the ride back with at most a minute lost (or finishes the save), the ride is
  saved whole, and an fsck finds the FATs equal, FSInfo exact, no chain into a free cluster, no cross-link, one lost
  cluster at most. The saved file passes Garmin's FIT SDK and fitdecode. Volume writes are fuzzed.
- **Cost:** about 4 write commands a minute (data, header sector, entry, record; a FAT and FSInfo write per new
  16 KB cluster, every 8 minutes or so); making the folders the first time, about 20 more. Image +23 KB (150.9 KB to
  174.0 KB without the ANT+ key), App +1.5 KB (39.4 KB), make_app's frame 107 KB of its 110 KB budget.

### The recorder: what to check on the unit (with the owner, in this order)

1. A backup of the drive first (stock's USB drive, read only on the computer: `dd` of the whole `BRYTON` volume).
2. The write primitive alone, from RAM (`dev.py run`): a test command (to add to the
   dev link) that writes one sector inside a file made for it on the computer (`\Rustfw\scratch.bin`, found by the
   FAT reader), reads it back, and restores it; `dev.py sector` before and after.
3. A short ride from RAM (`dev.py run`): Start, two minutes, End. Then stock (or the computer) must show
   `\Activities\YYMMDDhhmmss.fit` and `\Rustfw\ride.rec`; the file opens in the Bryton app / Garmin's FIT tools;
   `fsck_msdos -n` on the Mac finds nothing.
4. A restart mid-ride: Start, three minutes, `dev.py reset` (or the four-button restart); the build again from RAM:
   "RIDE RESUMED", the timer back to the last minute; Start, a minute, End: one file with both parts.
5. A power cut mid-ride: the same with the battery connector or a hard power off, if the owner agrees.
6. Stock booted after each of 3 to 5: it must not complain, format, or lose any file; its own ride recording still
   works.
7. Only then an install (`iaprecover.py --write`), recorded in FLASHLOG.

## 2026-10-24: review 2, courses, workouts, alerts and the buzzer: fixed

From the second adversarial review (findings numbered as there), each with a test written first that failed:
- **6, lap courses** (`course.rs` `search`): the lost-rider search kept the first 4 chunk boxes at distance 0, so on
  a lap course every lap's box held the rider and laps 1 to 4 always won: after a gap or a detour on lap 5 the
  rider was placed a lap back. Fixed: of the boxes about as near as the nearest, the 4 nearest along the course to
  where the rider was. Test `lap_course_found_on_the_right_lap` (lap 4 before).
- **7 and 15, long single passes:** opening COURSES read every file to the end for its length (up to 32 of 4 MB),
  following a course read it whole, a past ride's card read up to 4 MB, and WORKOUTS read 8 KB of every future
  file. Now COURSES lists from the folders and measures a file at a time, the course to follow and the card are
  read 96 KB a pass (`App::SCAN_IDLE`, about 40 ms), and the calendar looks into the earliest 8 day folders, 16
  files at most. Test `long_reads_spread_over_passes`: the most a pass reads went from 2.3 MB to 108 KB (300 ms at
  2.5 MB/s is the limit it checks).
- **11, climbs:** `find_climbs` made a bin per 50 m of whatever length the file claimed (86 million for a crafted
  30 KB GPX): at most 32768 bins now. Test `climbs_bounded_on_a_crafted_course`.
- **12, GPX numbers:** `decimal` overflowed an i64 on 12 digits with 7 places (a panic in debug, a wrapped point in
  release): checked arithmetic, the point refused. Test `decimal_overflow`; the GPX fuzz gets long digit runs.
- **13, alerts:** a heart-rate alert returned before the timer was noted, so an eat or drink minute in the same
  second was lost: the reminders due are kept pending first. Test `heart_rate_alert_keeps_the_reminders_due`.
- **14, the buzzer:** the workout's tones and the alerts had a player each, both driving the buzzer in the same
  pass and cutting each other off. One player (`App::tones`) with a queue of 3. Test `tones_do_not_cut_each_other`.
- **Also:** a course thinned past a point every 50 m (over about 330 km) widens its off-course threshold to half
  the spacing plus 25 m, since its chords cut bends by up to half the spacing (test
  `thinned_course_no_false_off_course`); make_courses's comment said it was built in place, it is not (35.6 KB,
  budgeted).
- **Cost:** image 190.7 KB to 192.2 KB; App 42.9 KB on the host; make_app 43.3 to 44.3 KB, make_courses 34.0 to
  35.6 KB.
- **Not done:** GPX gaps (a point far from the previous one is still joined to it), a course's own stated length
  (FIT total_distance, Bryton's 254) instead of reading it whole.

The first frame ever, with the earlier seven-segment digits: [img/first-frame-2026-10-23.png](img/first-frame-2026-10-23.png).

## 2026-10-24: the second review's recorder findings, fixed (simulator only)

The second adversarial review (read only, on ce3ead8) found these in the recorder; each fix came with a test that
fails without it (the review's probes turned into tests where they fit):
- **R1, one bad byte blocked every later ride:** the save's read-back failed its CRC and was tried again forever.
  Fixed: a file that reads but is not whole is left as it is in `\Rustfw\Rides`, the record "done", said once
  (SAVE FAILED); only a card error is tried again, and a save or a ride left unfinished only `GIVE_UP` (12, a minute)
  times in a row. Tests `a_damaged_ride_is_kept_and_the_next_one_recorded`, `a_save_that_keeps_failing_is_given_up`.
  A failed save also stopped being retried at all (its checkpoint cleared `want`): a save is now due until done.
- **R2, Start while the boot's recovery was pending took over the new ride's writer:** `recover` does nothing once a
  ride has started; the new ride's first card work finishes the old one first (resumed in its place when the marker
  said it was open, else saved). Tests `start_while_the_boot_recovery_is_pending` (the probe's case: both rides
  saved whole), `start_while_the_card_failed_at_boot` (the app's glue).
- **R3 (C1), the read-only build's loop:** fixed on main before (1ef21ec).
- **R3 (torn writes):** reliable write when the card offers it (emmc.rs, BOARD.md 15), the EXT_CSD bytes in
  `dev.py emmc`. For a card that does not: the ride in progress moved to `\Rustfw\Rides`, so its entry (rewritten every
  minute) and `ride.rec`'s never share a sector; each record slot carries copies of the two file sectors the next
  checkpoint rewrites in place (sector 0, the last), which the resume writes back; `sync_fats` takes the second FAT's
  sector when the first's holds what no FAT holds (torn), and makes FSInfo again when its signatures are gone; a ride
  whose entry was torn is saved from its record. The card models (sim.rs `EmmcCard`, recorder.rs tests' `Card`) tear
  the sector a cut falls in. Tests `power_cut_tearing_the_sector_at_every_write` (274 cuts: 155 resumed, 25 saved at
  boot, the next ride always saved, at most 2 lost clusters; one cut tears the root directory's sector, which loses
  stock's entries there: the reliable write's job), `every_write_is_reliable_when_the_card_offers_it` (CMD23 bit 31
  before every CMD25, none of CMD24, and a cut at every sector of a ride's first 75 s on the Sim: clean volume each
  time; with plain writes the same sweep loses the root's entries), `reliable_writes_when_offered` (the driver).
- **R4, Off lost up to a minute:** `power_off` checkpoints first (one try). Test `off_mid_ride_keeps_every_second`.
- **R5, caught-up seconds shared one time:** each has its own (`lag_s`). Test `seconds_caught_up_keep_their_own_time`.
- **R6, a lost marker resumed a ride from days ago:** a ride left open is resumed at a Start only if the marker said
  so or it stopped less than an hour (`RESUME_S`) before; else ended at its last record and saved. Test
  `an_old_ride_left_open_is_saved_and_a_new_one_started`.
- **R7, the move gap:** narrowed: `\Activities` and a free name are found before the entry is marked deleted, so only
  the link is left in the gap. The fsck cross-link check before linking is not done.
- **R8, "done" without a link:** ten names taken is now an error (the ride kept), never "done".
- **R10, the record's layout:** tag RFR2, the ride words' count in the record (checked), a `const` assert that they fit.
- **R11, a failing recorder looked like a working one:** the strip's dot is grey while not saving (a failure, a
  refused start, a build that may not write); NOT SAVING at the first failure in a row, then every 5 minutes of them
  (the old RECORDING ERROR did not fit its 20-character line and was never shown); the count resets on success. Test
  `a_failing_recorder_shows`.
- **Low:** `h.clusters * cluster_bytes` checked.
- **Not done:** `find_free` one sector at a time on a nearly full volume (review P3), R9 (power totals, workout and
  course after a resume), R12 (stock reading this firmware's FIT), the fsck check of R7.
- **Cost:** image 189,796 B to 191,204 B (+1.4 KB); App +8 B; make_app's frame 43.0 KB to 44.5 KB (budget 50 KB);
  `ride.rec` 1 KB to 3 KB; a checkpoint's record write 1 sector to 3 (one command), and CMD23 before each write on
  a card with reliable write.

## 2026-10-24: the first eMMC writes, on the unit (from RAM, owner present)

- **Backup first,** over the dev link (no mass storage in this firmware): the whole FAT region (sectors 0 to 29807,
  read twice, equal), every directory outside `\Map` (62), and the files of `\Activities`, `\System`, `\Workout`,
  `\PlanTrip` (99 FIT files, CRCs good). The volume is 31 GB and nearly full of map data (0.7 GB free), so a whole
  image would take hours at the link's rate; what a write can damage is the FATs and the directories, which this
  covers. A long `sector` read stopped once on a cut answer: dev.py now asks again.
- **Steps 1 to 4 of the plan passed** (docs/status.md, "Seen on the unit"): the write test, a ride saved, a reset
  mid-ride and its resume. Not done: the power cut (step 5) and stock booted afterwards (step 6).
- **Found:** stock leaves FAT2 out of date (1559 entries) and FSInfo's free count stale; this writer leaves both
  right. The overview's notice box cut "08:41 RIDE RESUMED" to 16 characters: notices and banners now drop to the
  small font when too wide.
