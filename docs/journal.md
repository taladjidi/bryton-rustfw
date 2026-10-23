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

The first frame ever, with the earlier seven-segment digits: [img/first-frame-2026-10-23.png](img/first-frame-2026-10-23.png).
