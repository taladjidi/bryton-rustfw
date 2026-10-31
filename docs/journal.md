# Journal: what the unit taught us

What this file holds: dated findings from the real unit, one heading per date, newest last; append only.
What does not belong: what is built (features.md) and what is still to do (roadmap.md, status.md).
Where to look: each finding's evidence is in BOARD.md; every write to the unit is a row in bryton-S510-mods' FLASHLOG.

## 2026-10-22: the first install stops before the first frame

- Installed through the stock updater; the OS area read back equal to `out/IMG.bin`. Dark screen, no USB.
- **The backlight is no readout:** v1 to v8 drove PG13 as a plain output; nothing they showed can be trusted.
- **The signal that works (diag v9):** the image waits for USB power, then resets; holding Page while plugging in
  then gives Engineer Mode. Control: without the reset, no Engineer Mode. So our code runs.
- Checkpoints at chosen stages (v10 to v16) narrowed it down: a long SPI write after the panel power-on never
  completes; SPI1 took 16 bytes (its FIFO) and never clocked. Renode ran the same images to the end.

## 2026-10-23

- **SPI1 mode fault:** CR1.SSI must be set before CFG2.MASTER, else a mode fault clears MASTER (found with
  `tools/dev.py peek` on v17, which skipped the panel and started USB). Fixed: the clock shows, the dev link works.
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
- **Redrawing:** redrawing only what changes cut the main screen from 17.5 % to 0.4 % awake (`tools/dev.py info`).
- **Pixels two per 32-bit SPI write:** the panel shows them right (by eye).
- **eMMC (read only):** 31.3 GB; the OS area equals the installed image; sector 0 is a FAT32 boot sector, no MBR.
- **Sensor bus (I2C4):** LTR-308ALS light sensor, LPS22HH barometer, LIS2DH12 accelerometer; stock's automatic
  backlight dims to 1 % above 120 lux (by design), so ours rises with the light instead.
- **Beeps** at power-on and power-off: heard.
- **The cycle counter stops in sleep:** DWT CYCCNT does not count during WFI (the core clock stops; DBGMCU_CR is 0),
  while SysTick does. A main loop that timed its sleep with CYCCNT never woke by itself: passes went on only when a
  key or a touch woke it (found by `tools/dev.py info`: uptime stuck at 0). The sleep is now timed by SysTick; the awake
  figure, which counts CYCCNT, was right all along.

The first frame ever, with the earlier seven-segment digits: [img/first-frame-2026-10-23.png](img/first-frame-2026-10-23.png).

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

## 2026-10-24: the first eMMC writes, on the unit (from RAM, owner present)

- **Backup first,** over the dev link (no mass storage in this firmware): the whole FAT region (sectors 0 to 29807,
  read twice, equal), every directory outside `\Map` (62), and the files of `\Activities`, `\System`, `\Workout`,
  `\PlanTrip` (99 FIT files, CRCs good). The volume is 31 GB and nearly full of map data (0.7 GB free), so a whole
  image would take hours at the link's rate; what a write can damage is the FATs and the directories, which this
  covers. A long `sector` read stopped once on a cut answer: tools/dev.py now asks again.
- **Steps 1 to 4 of the plan passed** (this file, 2026-10-24): the write test, a ride saved, a reset
  mid-ride and its resume. Not done: the power cut (step 5) and stock booted afterwards (step 6).
- **Found:** stock leaves FAT2 out of date (1559 entries) and FSInfo's free count stale; this writer leaves both
  right. The overview's notice box cut "08:41 RIDE RESUMED" to 16 characters: notices and banners now drop to the
  small font when too wide.

## 2026-10-24: the USB drive on the unit; high speed drops, full speed holds

- **Mounted** from this firmware (no stock needed), read only first: 40 files equal to the backup. macOS refuses to
  mount a new disk while its screen is locked ("dissented, 0xF8DA0008"), which looked like a fault at first.
- **The main loop stalled** after a drive command: its card access sleeps, which takes over SysTick, and the loop
  waited for a sleep end that never came while USB was still served (the uptime froze, the dev link answered).
  Fixed: `Link::worked` tells the loop, as the dev link's own card commands did.
- **High speed drops** every long transfer after 15 to 40 MB, as stock's drive did on 2026-10-22: the Mac resets the
  device and it stays unconfigured until a replug. **Full speed** read 172 MB with no drop, at 1.0 MB/s. Cause not
  found yet (docs/usb.md lists what to try); the installed build runs at full speed meanwhile.
- **The IGN copy is put aside** until there is an hour with the unit (docs/roadmap.md, section 5).

## 2026-10-24: what the unit saw at a drop

Backup register 0 after a high-speed drop at the mount, read after the replug: `0xd1280200`.
- **Last command:** 0x28, READ(10), with status 2: a **phase error**. The data phase could not hand the next 512-byte
  packet to the IN endpoint for a whole second (`Bulk::send` gave up), so the computer had stopped taking data in
  the middle of a transfer from the unit.
- **Resets:** none of either kind (0xff bus reset, 0xfe Bulk-Only reset) among the last 32 entries. The computer did
  not reset the device first.
- **Then:** the core left Configured (the dot orange, most likely USB suspend: no more starts of frame), and the
  watch reconnected (register 31: 2 drops in all).
- **Reading (inferred):** the computer disabled the port without a reset. That is what a host controller does after
  transaction errors (babble, a bad CRC) on an IN transfer. Only fast IN traffic fails: a damaged high-speed packet
  from the device's side is the likeliest. Candidates: the transmit FIFO (sized to one packet, filled word by word
  by uncached code), the ULPI data timing at 60 MHz, the PHY's transmitter or supply. Next tries 2, 4, 5 and 7 below
  target exactly that.

## 2026-10-24: eMMC writes and the recorder, on the unit (from RAM, owner present)

- **Reliable write:** `tools/dev.py emmc` reads WR_REL_PARAM 0x15, REL_WR_SEC_C 1: enhanced reliable write, so CMD23 with
  bit 31 before each CMD25.
- **`tools/dev.py wtest`** on one sector of a cluster free in both FATs: written changed, read back, restored, read back.
- **A 2-minute ride:** `\Rustfw`, `\Rustfw\Rides`, `ride.rec` made; the save moved the ride to
  `\Activities\261024083403.fit` (1227 B, 153 records), which Garmin's FIT SDK and fitdecode read, CRC good.
  Against the backup taken just before: 4 clusters taken in FAT1, root and `\Activities` changed only by appended
  entries, FSInfo recounted (stock's free count was stale: 43700 for 43767). FAT2 is now equal to FAT1: stock had
  left 1559 entries of FAT2 out of date, and the writer copies FAT1's sectors over.
- **A reset mid-ride:** 90 s in, `tools/dev.py reset`; the build again from RAM: RIDE RESUMED, the ride carried on into
  one file (261024083931.fit): a timer stop at the last checkpoint (60 s), a start at the resume, 30 s lost.


## 2026-10-24: two copies of a ride, on the unit (from RAM)

- A ride, `tools/dev.py reset` 100 s in, the build again, RIDE RESUMED, the save: `\Activities\261024203259.fit` (FIT 1.0,
  no developer data) and `\Rustfw\Activities\261024203259.fit` (FIT 2.0, Alpha1 and RMSSD described; invalid without
  a strap), 144 records each and the same (tools/fit_check.py --pair, fitdecode and Garmin's FIT SDK). Before the
  review's fix the clean copy had resumed 22 s behind the full one.
- Not yet: a ride with the strap (Alpha1 values), tools/sync.py against the real Intervals.icu and Strava.


## 2026-10-24: the phone, on the unit (an iPhone, from RAM with EMMC_WRITE=1)

- **Pairing:** PAIR, then S510 tapped on the iPhone: the normal iOS prompt, Just Works; LINK SECURE, the bond in
  `\Rustfw\ble.bin` (512 B), the clock set from the phone (CTS, right to the minute).
- **Reconnect:** after a restart the bond read back and the iPhone reconnected by itself (when in range).
- **Media (AMS):** title, artist, album, duration and elapsed time from Deezer; play/pause, next, the volume from
  the keys and the new player screen.
- **Calls (ANCS):** the overlay over the overview with the caller's name; answered from the green button. iOS
  removes the call's notification at the answer, so the call's time and a hang-up cannot follow over ANCS.
- Not yet: messages' banners, skip in a podcast, the camera shutter, a re-pair after forgetting on the iPhone only,
  a stranger's phone, range loss during a ride.

## 2026-10-26: fonts, rows and registers (owner present)

- **FONT (SETTINGS > DISPLAY > FONT, SMOOTH):** run from RAM (e762241): switching to SMOOTH redraws at once; the
  overview, the indoor ride page and the DISPLAY menu draw in Barlow Semi Condensed Bold with no box moving. Register
  16 then read 0xa15a0f30 (bit 24 set). After the flash of e762241 (FLASHLOG #80) it read 0xa05a0f30 (PIXEL): the owner
  had switched back to PIXEL by hand (the choice kept as set). Big "--" values look wide in SMOOTH (the
  glyph keeps the pixel font's cell).
- **BIKE and DISPLAY rows:** BIKE shows WHEEL, WEIGHT, ALTITUDE, FROM GPS, PROFILE, CLIMBS AHEAD; DISPLAY shows
  BRIGHTNESS, OFF AFTER, TOUCH, FONT, GAUGE POWER. WEIGHT edited from 9.0 to 8.5 kg (five Page presses, 0.1 kg each)
  and W' (ME) from 15.0 to 21.0 kJ by the dev link's keys.
- **Settings registers read back** (`tools/dev.py peek` of RTC_BKP16R / BKP28R): register 16 = 0xa0551530 (tag 0xa, rider
  78 kg, W' 21.0 kJ, bike 8.5 kg); register 28 = 0x9280c0e0 (tag 0x9); register 11 unchanged (0xa7345b21). The
  rider's weight and W' from the earlier layout carried over.
- **A ride from RAM (1ba795d, FLASHLOG #81):** started (INDOOR), past its first checkpoint, the image restarted
  mid-ride: RIDE RESUMED; END RIDE > DISCARD put both copies in `\Rustfw\Discarded` (valid FIT: indoor_cycling, 45
  records, the full copy's three developer fields).

## 2026-10-26: palette, altitude and the data of a real ride (owner present)

- **The new palette and the zone bars on the panel** (from RAM): Intervals.icu's teal, green, olive, brown, wine,
  purple and slate, brightened x 1.5, and the bars following their numbers at once.
- **ALTITUDE** calibrated with the dev link's keys in 1 m steps.
- **The RAM ride check of fe7776bbc83c:** a start, a restart mid-ride, a discard.
- **A real ride's data:** 52,480 beats recorded as `hrv` messages in the full copy and read back. Our DFA a1 against
  an independent implementation of the published method on the same beats: medians 1.05 and 1.05, r = 0.57 window
  by window, a typical difference of +-0.07.

## 2026-10-27: a main loop that never slept (from RAM)

- **The spin:** with some builds, every WFI returned at once (the profiler's `usb` row at 255,000 calls a second, 82 %
  of wall time awake): the touch line's IRQ 23 stuck pending in the NVIC (below, "2026-10-24 to 10-27: USB causes ruled out or fixed").
- **After the fix** (`touch_edge` clears the pending bit on every call): `usb` at 43 to 44 calls a second, 0.05 % of wall
  time, in 12 runs of two layouts; the dev link and `info` work. A tap on the panel after the fix is still to see.
- **Again with the panel's DMA:** IRQ 11 stuck the same way (ISPR0 0x00000c00, DMA1_LISR 0). Now one place, main.rs
  `sleep`, clears every wake source's pending bit just before each WFI: 0 spins in 20 runs (cpu 9 to 10 permille), and
  0 in 10 of a variant that spun 5 times in 6 before. A tap after this fix is still to see.

## 2026-10-27: the profiler and the performance pass (from RAM, owner away)

- **`tools/profile.py`** from RAM (30 s windows; no card writes: the ride scenario left out): idle overview 1.72 %
  awake before the performance pass (54d3f0a), 1.55 % after; a ride page 1.51 % then 1.32 %; menus with a key every
  0.5 s 2.75 % then 1.82 %, a menu move 7.6 then 4.0 ms on average (the panel's SPI, about 1 us a pixel, sets it),
  the worst 161 then 95 ms. GNSS 113 then 45 us a pass, the light sensor 45 then 28 us a call.
- **`info`'s new `cpu` figure** (99e6f5e): 12 per mille on the overview after the touch fix, beside `awake` 11.
- **Not seen on the unit yet:** the flattened menus and the quick panel (f935c0f), the refactors B1 to B3 (behaviour
  kept by the tests; the unit only booted and ran the profile scenarios on them).

## 2026-10-27: SPI1 at 50 MHz and the panel's DMA (from RAM, owner away)

`tools/profile.py -s 8 idle page menu`, cpu about 10 permille on each build (no stuck interrupt). The profiler counts
CPU cycles, which stop in WFI: with the DMA the figures are the CPU's work, not the wall time; the 50 MHz CPU-only
build (POWER_STEPS=15) gives the wall time.

| | 16 MHz, CPU (main 72193a1) | 50 MHz, CPU | 50 MHz + DMA (CPU time) |
|---|---:|---:|---:|
| menu move (input mean) | 16.0 ms | 10.2 ms | 9.6 ms |
| a whole menu screen (input max) | 98.6 ms | 61.8 ms | 58.2 ms |
| overview full draw (draw max) | 62.6 ms | 44.2 ms | 32.5 ms |
| awake, idle overview | 1.78 % | 1.56 % | 1.41 % |
| awake, menu | 3.89 % | 2.81 % | 2.67 % |

- Drawing is now CPU-bound: a menu screen (about 70k pixels) takes about 22 ms on the wire at 50 MHz but 58 ms of
  CPU, about 165 cycles a pixel to make (the per-pixel callback, glyph and code fetches from uncached SDRAM). Fills go
  by DMA while the CPU sleeps; a blit's DMA only overlaps the wire with that work. The next gain is the D-cache
  (hw.rs `PanelDma`'s cache contract) or cheaper pixel making in ui/text.rs.
- Still to see: the panel itself at 50 MHz (screenshots are the app's rendering, not the panel's): an owner's look for
  noise or shifted pixels; if any, MBR /4 (25 MHz) in `spi_init`.

## 2026-10-27: suspend, a normal state, not a drop

- **What a suspend is:** the computer stops sending starts of frame; after 3 ms of an idle bus the core sets
  OTG_GINTSTS.USBSUSP and OTG_DSTS.SUSPSTS (USB 2.0 7.1.7.6; RM0399, OTG_HS, OTG_GINTSTS and OTG_DSTS). The computer
  ends it with resume signalling (OTG_GINTSTS.WKUPINT) or a bus reset. synopsys-usb-otg 0.5 reports USBSUSP and
  WKUPINT as `PollResult::Suspend` / `Resume` and does nothing else (its `suspend` / `resume` are empty); usb-device
  0.3 goes to `UsbDeviceState::Suspend` and back to the state before at the next resume, reset or traffic.
- **Before:** `Suspend` is not `Configured`, so after 2 s the watch called it a drop: a soft disconnect (DCTL.SDIS,
  100 ms of busy-waiting), "USB RECONNECT" on the screen, register 31 counted one more drop, and register 0's drive
  record was overwritten. Once per suspend: the watch re-armed only once configured again.
- **Now** (lib link/dev.rs `watch_usb`, board/usb.rs `Link::watch`, tests in dev.rs): a suspend from configured is
  left alone however long it lasts, the dot turns grey, and the link carries on as configured at the resume. Nothing
  is polled in between: no USB interrupt comes, so WFI sleeps whole slices. Its timers run on the main loop's clock
  (they ran on the cycle counter, which stops in WFI and wraps every 9 s).
- **What is still a drop:**
  - configured and the frame number (DSTS.FNSOF) still for 200 ms with no suspend (the green-dot drop);
  - configured once, then not configured again for 2 s (a reset the computer did not follow up);
  - a suspend after the drive's last command ended in a phase error: a computer suspends a device only when it is
    idle, never in the middle of a transfer, so that is the port disabled (the 2026-10-24 drop above), and the old
    recovery runs.
- **On the unit:** a suspend could not be provoked without sleeping the Mac (not allowed). With the serial port closed
  and the drive not lent, macOS kept the device powered (ioreg: IOUSBHostDevice DevicePowerState 2, over 2
  minutes): it does not suspend this device while awake. The build ran from RAM with the link up, a green dot, cpu 8
  permille. What a real suspend does is in docs/status.md, "Not yet seen on the unit".

**Do suspends explain the past issues?** Mostly not.
- **Do not fit:** the drops came during sustained high-speed transfers, and a computer never suspends a device with
  a transfer in flight. They depend on speed (full speed held), which fits signal integrity, not power management.
  Stock's drive (every 80 to 90 s while copying) and Engineer Mode's `LIBUSB_ERROR_IO` are other firmwares, without
  this watch. The green-dot drop had no suspend: a suspend would have set USBSUSP and left Configured. Only an unplug
  brought the link back, while a suspended device comes back with the next traffic.
- **Fits, as an effect:** register 0 at the 2026-10-24 drop (`0xd1280200`: a READ(10) phase error, no reset, then the
  core left Configured) is what the core reports as a suspend: the port went idle, but after the transfer failed.
- **Fits, separately:** while the Mac slept with the unit plugged in (a system sleep suspends every port), the old
  watch disconnected and reconnected the unit 2 s in; on waking, macOS found a device that had left and come back,
  so it enumerated it afresh (the serial port gone and back, a lent drive "not ejected properly"), and the unit
  showed "USB RECONNECT". Register 31 counted those as
  drops and register 0 could hold a sleep's record instead of a real drop's (status 0 in it means no failed command).

## 2026-10-24 to 10-27: USB causes ruled out or fixed

- **A main loop that never slept with the cable in** (2026-10-27, fixed; not USB): the profiler showed the `usb`
  section 255,000 times a second (3.2 us each, 82 % of wall time) instead of about 42. It looked like a suspended
  bus, but DSTS.SUSPSTS read 0 and the spin went on with GINTMSK = 0 and with GAHBCFG.GINT = 0. The NVIC (peeked
  with a debug build) held IRQ 23, the touch line's EXTI9_5, pending (ISPR0 0x00800400, ICSR.VECTPENDING 39) with
  EXTI_CPUPR1 line 6 clear: the NVIC clear had landed before the EXTI write, the line pended it again, and nothing
  cleared it any more. The usb row only showed it because the link is polled once per WFI; without the cable the
  loop spins just the same, unmeasured. Timing decides it: about three runs in four of some builds, none of
  others (adding a store after WFI hid it; `dsb` / `isb` around WFI did not). Fix, first step: main.rs `touch_edge` reads CPUPR1 back after
  clearing it and clears the NVIC pending bit on every call: 0 spins in 12 runs of two layouts that spun before
  (superseded: `sleep` now clears the NVIC pending bits, next paragraph).
  **The same again with the panel's DMA** (2026-10-27, after c5465b0): the cpu figure at 1006 permille, ISPR0 0x00000c00,
  ICSR.VECTPENDING 27 = IRQ 11 (DMA1 stream 0) with DMA1_LISR 0: `dma_wait` cleared the NVIC right after LIFCR, the
  line re-pended it, and nothing cleared it until the next transfer. 0 in 10 runs of the build at e923b23, but 5 in 6
  with LISR's read back removed. Fix, for every wake source: main.rs `sleep` clears the pending bits of all three
  (IRQ 11, 23, 77) just before each WFI, after the sources were read; the lines are level, so an unread event pends
  again at once. The sources no longer clear their own NVIC bit. With the read back removed: 0 spins in 10 runs; the
  build as committed: 0 in 20. `tools/dev.py peek` now reads the NVIC (0xe000e100 to 0xe000e33c) and ICSR (0xe000ed04).
- **macOS refusing the mount** ("mount approval dissented, 0xF8DA0008") was its lock screen: it does not mount a newly
  inserted disk while locked. Not the unit.
- **The main loop stalling** after a drive command (fixed): the eMMC access slept, which takes over SysTick. This was
  not the drop: the USB link kept working through it.
- **The ULPI pins' slew rate:** already very high speed, as stock sets them.
- **A CBW not valid leaving the drive deaf after a bus reset:** fixed; a bus reset clears it now (msc.rs
  `Drive::bus_reset`).

## 2026-10-30: the heart rate intervals counted again on every power broadcast (main, fixed in this branch)

Found on the host (not on the unit): after each beat of the strap, the radio path pushed the beat's RR interval into
the HRV and the full copy's hrv messages again after every message of any kind. With a power meter paired, each
interval went in once per power-meter broadcast: 38 instead of 19 in the test `each_interval_once`. So rides recorded
with a power meter and the strap may carry duplicate hrv messages, and their DFA a1 and RMSSD are skewed. Fixed in the
sensor names branch: the intervals are taken once per message, and only from the strap. Rides before the fix are not
re-read; the next ride checks the count.

## 2026-10-31: a card that failed at boot dropped the ride's resume after one retry (main, fixed)

Found on the host (not on the unit): a ride open, the unit reset, the eMMC not answering at the boot. The boot tried
to reopen the ride, failed and waited its 5 s (`RETRY` passes) to try again; the pass that ended that wait tried
nothing and returned `Nothing`, and `step_recorder` took that for "no ride to resume": `resume_pending` went false and
the `RIDE_BKP` marker was cleared, whether the card answered again or not. The ride was never reopened, the screen
said only NOT SAVING, and the next boot resumed nothing. Nothing was lost on the card: the ride's file in
`\Rustfw\Rides` kept every record to its last checkpoint and its recovery record still said "recording", so Start
within an hour resumed it (an older one was saved as it was); without Start the ride stayed out of `\Activities`, the
unit could switch off as idle, and the drive was not lent writable. Fix (app/riding.rs `step_recorder`, `step_full`):
a pass that was waiting keeps the resume pending; only a try that finds no ride clears it. Test:
`the_card_answers_late_at_boot_without_start` (the card back after 2 s, 5 s and 60 s, a further reset, the save).
