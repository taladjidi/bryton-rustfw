# Status

## Seen on the unit

Everything in [features.md](features.md) runs on the unit (2026-10-23), installed or from RAM, unless listed below.
The dated findings are in [journal.md](journal.md).

## Seen on the unit: eMMC writes and the recorder (2026-10-24, from RAM, owner present)

- **Reliable write:** `dev.py emmc` reads WR_REL_PARAM 0x15, REL_WR_SEC_C 1: enhanced reliable write, so CMD23 with
  bit 31 before each CMD25.
- **`dev.py wtest`** on one sector of a cluster free in both FATs: written changed, read back, restored, read back.
- **A 2-minute ride:** `\Rustfw`, `\Rustfw\Rides`, `ride.rec` made; the save moved the ride to
  `\Activities\261024083403.fit` (1227 B, 153 records), which Garmin's FIT SDK and fitdecode read, CRC good.
  Against the backup taken just before: 4 clusters taken in FAT1, root and `\Activities` changed only by appended
  entries, FSInfo recounted (stock's free count was stale: 43700 for 43767). FAT2 is now equal to FAT1: stock had
  left 1559 entries of FAT2 out of date, and the writer copies FAT1's sectors over.
- **A reset mid-ride:** 90 s in, `dev.py reset`; the build again from RAM: RIDE RESUMED, the ride carried on into
  one file (261024083931.fit): a timer stop at the last checkpoint (60 s), a start at the resume, 30 s lost.

## Seen on the unit: two copies of a ride (2026-10-24, from RAM)

- A ride, `dev.py reset` 100 s in, the build again, RIDE RESUMED, the save: `\Activities\261024203259.fit` (FIT 1.0,
  no developer data) and `\Rustfw\Activities\261024203259.fit` (FIT 2.0, Alpha1 and RMSSD described; invalid without
  a strap), 144 records each and the same (tools/fit_check.py --pair, fitdecode and Garmin's FIT SDK). Before the
  review's fix the clean copy had resumed 22 s behind the full one.
- Not yet: a ride with the strap (Alpha1 values), tools/sync.py against the real Intervals.icu and Strava.

## Seen on the unit: the phone (2026-10-24, an iPhone, from RAM with EMMC_WRITE=1)

- **Pairing:** PAIR, then S510 tapped on the iPhone: the normal iOS prompt, Just Works; LINK SECURE, the bond in
  `\Rustfw\ble.bin` (512 B), the clock set from the phone (CTS, right to the minute).
- **Reconnect:** after a restart the bond read back and the iPhone reconnected by itself (when in range).
- **Media (AMS):** title, artist, album, duration and elapsed time from Deezer; play/pause, next, the volume from
  the keys and the new player screen.
- **Calls (ANCS):** the overlay over the overview with the caller's name; answered from the green button. iOS
  removes the call's notification at the answer, so the call's time and a hang-up cannot follow over ANCS.
- Not yet: messages' banners, skip in a podcast, the camera shutter, a re-pair after forgetting on the iPhone only,
  a stranger's phone, range loss during a ride.

## Seen on the unit: fonts, rows and registers (2026-10-26, owner present)

- **FONT (SETTINGS > DISPLAY > FONT, SMOOTH):** run from RAM (e762241): switching to SMOOTH redraws at once; the
  overview, the indoor ride page and the DISPLAY menu draw in Barlow Semi Condensed Bold with no box moving. Register
  16 then read 0xa15a0f30 (bit 24 set). After the flash of e762241 (FLASHLOG #80) it read 0xa05a0f30 (PIXEL): the owner
  had switched back to PIXEL by hand (the choice kept as set). Big "--" values look wide in SMOOTH (the
  glyph keeps the pixel font's cell).
- **BIKE and DISPLAY rows:** BIKE shows WHEEL, WEIGHT, ALTITUDE, FROM GPS, PROFILE, CLIMBS AHEAD; DISPLAY shows
  BRIGHTNESS, OFF AFTER, TOUCH, FONT, GAUGE POWER. WEIGHT edited from 9.0 to 8.5 kg (five Page presses, 0.1 kg each)
  and W' (ME) from 15.0 to 21.0 kJ by the dev link's keys.
- **Settings registers read back** (`dev.py peek` of RTC_BKP16R / BKP28R): register 16 = 0xa0551530 (tag 0xa, rider
  78 kg, W' 21.0 kJ, bike 8.5 kg); register 28 = 0x9280c0e0 (tag 0x9); register 11 unchanged (0xa7345b21). The
  rider's weight and W' from the earlier layout carried over.
- **A ride from RAM (1ba795d, FLASHLOG #81):** started (INDOOR), past its first checkpoint, the image restarted
  mid-ride: RIDE RESUMED; END RIDE > DISCARD put both copies in `\Rustfw\Discarded` (valid FIT: indoor_cycling, 45
  records, the full copy's three developer fields).

## Seen on the unit: palette, altitude and the data of a real ride (2026-10-26, owner present)

- **The new palette and the zone bars on the panel** (from RAM): Intervals.icu's teal, green, olive, brown, wine,
  purple and slate, brightened x 1.5, and the bars following their numbers at once.
- **ALTITUDE** calibrated with the dev link's keys in 1 m steps.
- **The RAM ride check of fe7776bbc83c:** a start, a restart mid-ride, a discard.
- **A real ride's data:** 52,480 beats recorded as `hrv` messages in the full copy and read back. Our DFA a1 against
  an independent implementation of the published method on the same beats: medians 1.05 and 1.05, r = 0.57 window
  by window, a typical difference of +-0.07.

## Not yet seen on the unit

- **The ascent filter on a ride (altitude.rs, ride.rs)**: the median of 5, the 1/6 average, the slewed reference and the
  3 m hysteresis are tuned on the records of one real ride (341 m became 123 m on a real 60 km ride, offline); on the
  unit, a ride's ascent against a known climb and against stock's, with a calibration made at the start.
- **The sliced tile load (climbs_ahead.rs, storage/inflate.rs, storage/route.rs, app/ahead.rs)**: before the change one
  tile took up to 238 ms in one pass on a ride; the longest slice is estimated at about 3 ms (host instruction counts,
  scaled). On a ride, `dev.py info` ends with "climb tile us last N max M slice max S": S is to be seen under a few ms.

- **The ramp test (ramp.rs, app/ramp.rs, WORKOUTS > RAMP TEST)**: only on the simulator. On the unit with the D500 or
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
- **The trainer's release (app/trainer.rs, nrf.rs `ant_tx_completed`)** on the Van Rysel D500: the release at stop, end,
  pause and boot (once), and the **switch-off release**: one free resistance sent before the chip goes off, the D500 free
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
  buffer, is to measure with `dev.py info`; inferred 20 to 50 ms), the banner and the beep 50 m before a climb the
  road ahead has, the trace made again when the road bends away from the one traced; the climbs against what stock
  shows for the same ride; `dev.py info` ends with "climb tile us last N max M": the time of the last and the longest tile read, inflated and
  decoded (estimated 15 ms at the median, about 80 ms for the largest tile that fits the 96 KB buffer, 2080 nodes);
  the AXI SRAM cell stays in `.bss` (`nm`: `make_ahead`, about 105 KB) and the unit
  boots (a new static in `.axibss` is a core-level change: Renode, then `DIAG_STOP`).
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
- **Spin-down calibration of the ANT+ FE-C trainer (trainer.rs `Cal`, drivers/fec.rs pages 1 and 2, app/trainer.rs,
  ui/trainer.rs)**: only against the simulator's trainer. On the Van Rysel D500 showing CALIBRATE: CALIBRATE beside
  MODE (or Start held) sends page 1 with the spin-down bit (0x80), and the trainer answers with page 2 (the layout is
  GoldenCheetah's, from the profile rev 4.1, fec.rs doc; the temperature and speed condition bits and the target speed
  in 0.001 m/s are to be seen on a real trainer); the screen says PEDAL TO nn KM/H, then STOP PEDALLING, then
  CALIBRATED with the spin-down time from its page 1; the status bit clears afterwards; the request's reserved bytes
  and a request with both bits (when the trainer asks for the zero offset as well) are inferred; that the trainer
  leaves the calibration by itself after CANCEL (nothing is sent) is inferred; the ERG target comes back after.
- **The two links' configuration and the Bluetooth trainer, FTMS (nrf.rs `start`, drivers/ftms.rs, app/ftms.rs;
  phase 2)**: only against the simulator's chip and trainer. On the unit, in this order:
  1. `sd_ble_cfg_set` (`00 69 20 00 00 00 01 01 02 03 00`, then `00 69 40 00 00 00 01 01 01 00`) answered 0 and
     `sd_ble_enable` after it answered 0 (else the chip is reset and started as before: `dev.py info` would show
     two resets at the start, and SENSORS' BT TRAINER says NO LINK);
  2. the phone still pairs and comes back, advertising on tag 1 (the PHONE menu, media keys, a notification);
  3. SENSORS > BT TRAINER lists a real FTMS trainer (its service data or its UUID list), Select connects to it
     (`sd_ble_gap_connect` tag 1, stock's layout) **while the phone is linked**; its data on the pages; ERG under a
     workout, SIM on a course (the Control Point's responses: the trainer screen does not say NOT ACCEPTED);
  4. the trainer switched off and on: the link made again; the phone unaffected throughout.
  Inferred and to check there: the cadence bit of Indoor Bike Data read as present when set (pycycling's reading,
  the specification's table says the opposite), a resistance level spread over the trainer's own range, Cw 0.51.
- **The Bluetooth trainer without FTMS, FE-C over BLE (drivers/fecble.rs, app/ftms.rs; phase 2)**: only against the
  simulator's D500 (`FtmsSim::new_fec`). On the unit with the owner's Van Rysel D500 (its older model has no FTMS;
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
  goes on Back or Start), `dev.py ls Activities` and `Rustfw/Activities` have it, the history lists it. Another ride,
  DISCARD: "DISCARD RIDE?", KEEP selected; DISCARD: "RIDE DISCARDED", `dev.py ls Rustfw/Discarded` and
  `Rustfw/Discarded/Full` have it, `\Activities` not, the history and the totals as before; `tools/sync.py` (dry
  run) does not list it. A restart (`dev.py reset`) with the screen up: "RIDE RESUMED", paused. The first ride after
  this build makes `\Rustfw\Discarded\Full` at its start (the FATs compared first, a few seconds).

- **The Nordic link watchdog (app/link.rs)**: `dev.py info` ends with "radio resets 0" over a normal ride and a
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
- **DELETE in HISTORY (storage/trash.rs)**, with an EMMC_WRITE=1 build: a ride's card, Start, Start: "RIDE
  DELETED", the list without it, the totals lower at once; `dev.py ls Rustfw/Deleted` (and `Rustfw/Deleted/Full`
  for one of this firmware's rides) has it, `\Activities` not; the next start's totals the same (the cache written
  without it). Then, on stock: the ride gone from its history and nothing else changed (its `.sum` / `.csum` left in
  `\System\History\Summary`: whether stock lists a summary without its FIT is inferred from Ghidra, not seen);
  moved back from the computer, the ride back in both histories.

- **The page editor and `\Rustfw\pages.bin` (app/editor.rs, page.rs)**, with an EMMC_WRITE=1 build: a hold on a
  ride page's box opens the editor (the frame, the hint in the strip's place, live values); SPLIT, MERGE, TALLER,
  UNDO, a field; Back asks KEEP CHANGES?; ADD PAGE up to six, the Page key round them all; DELETE PAGE. Then
  `dev.py ls Rustfw`: `pages.bin`, 1024 bytes. The first write compares both FAT copies first (as the scan's cache:
  some 10 s of passes, inferred; the loop stays responsive). Restart: the pages as made (the file read 2 s after the
  start: a page shown before that is the registers' copy). Not while a ride records: no editor by a hold, no EDIT
  THIS PAGE in the RIDE menu until paused. The installed stock-switch case: stock and an earlier build of this
  firmware still read pages 1 to 3 from registers 20 to 25 (layouts of up to six fields).
- **The scan's cache (storage/stock.rs `Cache`, `\Rustfw\scan.bin`)**, with an EMMC_WRITE=1 build: the first start
  reads every ride as before (`dev.py info` awake permille during it: now at most about 15 ms of work a pass, so
  lower than the 717 seen; Back held 3 s switches off during it), then `\Rustfw\scan.bin` is there (64 + 64 bytes a
  ride; `dev.py ls Rustfw`). The next start: the overview's totals at the first screen, no 30 s of `--`, and the
  scan 30 s later reads nothing (`dev.py info` stays low). A ride saved: the next scan reads only it. A ride deleted
  from the computer: gone from the totals. The first write of the cache (also after a cache that did not check
  out) first compares both FATs (sectors 0 to 29,807 of each, about 15 MB in all: 3 to 4 s in one go at 10.5 MB/s),
  16 sectors of each a step, within the pass's 15 ms (`Cache::prepare`): some 10 s over 160 passes, inferred; check
  the loop stays responsive (`dev.py info` uptime, a key) and the time it takes. A ride saved during the scan is
  read (its FAT sector read again each pass); a read error leaves a ride out of the totals until the next start,
  not for good. An emptied `scan.bin` (from the computer) is written again with its first cluster (fsck clean).
  A key held while the computer copies to the drive counts its real time (Back held 3 s switches off, not 2 s).
  `dev.py bench scan` now reads only rides not in the cache: measure it with the cache deleted.

- **Electronic shifting (sensors.rs, app/radio.rs)**, all simulated only, the owner's Di2 at hand: with
  `di2_key.bin` built in (`tools/ant_key.py`), SENSORS > DI2 reads OFF (not NO KEY: the chip took network 1's key
  and channel 6's assignment); Start finds the Di2 within seconds (turn a crank or shift to wake it), its number
  shown green. GEAR shows front-rear and follows the shifts (check front and rear are not swapped: inferred from
  stock's gear ratio code); SHIFT BAT shows the percent stock's Sensor Details showed (60 % on the patch project's
  picture; the byte is inferred to be a percent). DI2 KEYS: set R PRESS to NEXT PAGE, press the right hood button
  once: one page; a long press: its action; the left button too (the sides inferred from stock: byte 1 right,
  byte 2 left); START set on a button, restart the unit after a press: the ride must not start by itself (the
  Di2's first button page is taken as the baseline, inferred: it repeats the last press byte). A ride with shifts: SHIFTS counts them, and the saved file has gear change events (fitdecode, or
  Intervals.icu's gear chart). A Di2 switched off (or out of range): GEAR goes to "--". An ANT+ Shifting system
  (SRAM AXS) if one can be borrowed: SHIFTING row, gear, battery status.

- **The phone link (BLE, app/phone.rs)**, in this order, the dev link up (`dev.py run`, then the screenshots), an
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
  8. **Reconnection** (an EMMC_WRITE=1 build, the bond written: `dev.py ls Rustfw` lists ble.bin, 512 B): switch off
     and on: 3 s after the start LINK WAITING, then SECURE without any prompt on the iPhone; media keys work at once
     (the CCCDs given back). Walk away until the link drops: LINK WAITING, and back in range it reconnects.
  9. **FORGET:** LINK OFF, nothing advertised; on the iPhone, forget S510 too.
  9a. **Re-pair and strangers** (2026-10-24 review): bonded, forget S510 on the iPhone only, then PAIR on the unit:
     the iPhone pairs again without FORGET on the unit. A second phone (not paired) taps S510 while the unit only
     waits (no PAIR): pairing refused, and within 30 s the unit drops it (LINK back to WAITING).
  10. **Cost:** `dev.py info` awake time with LINK OFF, WAITING and SECURE (the Nordic chip's own current is not
      measurable here; estimated 10 to 15 uA advertising once a second).
  11. **The RTC set** (`Board::rtc_set`, RM0399 init mode): the time right after the phone set it, and kept through
      a power off; stock afterwards still shows the right time.

- **The full copy of a ride and the sync** (docs/sync.md): `\Rustfw\Activities` written beside `\Activities`
  on the card, with Alpha1 from a real strap; `tools/sync.py` against the real Intervals.icu and Strava (DFA a1
  charted from the upload), and the workouts it writes listed in WORKOUTS (and by stock, which may want the `.json`
  beside them).
- **IGN maps** (`\Rustfw\Maps`): the packs found, a view drawn from them, its time (simulated: one index sector and
  about 45 sectors of tile data per tile, at most 6 tiles, so about 150 KB a view change, as stock's basemap; the
  decoding is 87 us a tile on the computer, about 3 to 6 ms on the unit with the D-cache off, inferred), the
  "(c) IGN" corner, the 2.1 MB map memory zeroed at boot.
- **Sensors other than the heart-rate strap:** speed, cadence, power (the decoders follow the ANT+ profiles,
  checked against openant).
- **The automatic backlight by eye:** stock's curve (8 % in the dark, the 1 % minimum above 120 lux): is the panel
  readable in full sun at 1 % (compare BRIGHTNESS 100 and 10 outdoors)?
- **Backlight levels** (60 / 30 / 10 %) and the time-out, each by eye.
- **The map on a real ride:** its awake time, and the last-known-position dot after a power cycle.
- **The map's zoom buttons** ("+" and "-" on the full-page map, now any level, coarser than the fit too, and the
  crosshair back to the fit): hit with a gloved finger indoors (the fit at 2 m: "-" must zoom out, the drag pan),
  the time of a re-centre over IGN at 1.5 and 2 m a pixel (`dev.py mapbench`), and whether one every 26 to 35 s
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
- **The USB drive (mass storage, msc.rs): to do, high speed** (all of it in docs/usb.md). Seen on the unit (2026-10-24): the composite
  device keeps the dev link; SETTINGS > SYSTEM > USB DRIVE (or `dev.py drive`) mounts BRYTON (macOS refuses the mount while its
  screen is locked: "mount approval dissented, 0xF8DA0008"); 40 files equal to the backup; a write (a 48-byte file)
  read back through the firmware's own FAT reader after the eject and restart. **High speed drops** every long read
  after 15 to 40 MB at 6.4 to 7.2 MB/s: the Mac resets the device, which then stays unconfigured (orange dot) until a
  replug; a soft reconnect (`usb::Link::watch`, counted in backup register 31) did not bring it back. Stock's drive
  dropped the same way on 2026-10-22, on a sound Apple cable straight into the Mac, so the cause is on the unit's
  side (board, PHY, or a setup both firmwares share; the ULPI pins are already very high speed). **Full speed
  (`USB_FS=1`) read all 172 MB of world.dat at 1.0 MB/s with no drop.** To try: `dev.py drivelog` right after a
  drop (the last 32 commands, to rule out a protocol fault), the PHY's own registers over ULPI, the OTG_HS FIFO sizes
  against ST's MSC example, the D-cache on (FIFO writes from uncached SDRAM code are slow), the 60 MHz ULPI clock.
  Not yet: Windows, Linux, unplugging while mounted.
- **The OTG_HS IRQ waking WFI** (the link may run on the 50 ms SysTick wake-ups alone).
- **The touch panel with a finger** (the chip itself answered: BOARD.md 17): INT low while touched, the axes
  (x 0 left, y 0 top expected), the gestures (swipe left / right between pages, hold for the menu, a tap on a menu
  row), the backlight waking on a touch, and the swipe thresholds by feel (stock's: 37 px across, 48 px up).
- **A power cut mid-ride** (the battery connector or a hard power off): the reset mid-ride and the resume are seen
  (2026-10-24, below); a cut while a sector is written is covered by the host tests and the card's reliable write.
- **Stock reading this firmware's rides** (\Activities\261024083403.fit, 261024083931.fit) and the Bryton app's sync.
- **The power button starts the unit after a switch-off:** fixed in ea359a0 (IRQ 149 enabled before standby), seen
  on the unit 2026-10-25, installed and unplugged.

- **New since the installed build, seen only from RAM or in the simulator (2026-10-24):** touch gestures by feel and
  their axes; the overview and its notices; banners (ride start, pause, lap, end); eleven page layouts; graphs (five
  plots by tap, the whole ride by hold); derived power fields and W' balance on a ride with a power meter; DFA a1 and
  RMSSD from the owner's strap (needs a few minutes of real beats); the AUTO backlight on stock's curve, outdoors.
- **Pages moved off backup register 19** (stock reads it at boot): the first start of a new build moves them; then
  stock, reinstalled, should keep its last battery voltage as before.

## Next steps

1. **Stop mode** between seconds (woken by the RTC, the keys, the radio chip's UART, USB): the largest battery
   gain left. A core change, tried with the owner present.
2. **USB high speed** for the drive: the drops after 15 to 40 MB (above); full speed works.
3. **A settings file** to free the backup registers (docs/roadmap.md).
4. The new features above, on the unit, as the owner rides them.
