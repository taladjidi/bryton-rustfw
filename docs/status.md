# Status

What this file holds: what the unit has not shown yet, the known problems, and the next steps.
What does not belong: what is built (features.md) and the roadmap (roadmap.md).
Where to look: features.md for what the firmware does, journal.md for the dated findings, docs/usb.md for the USB drops.

## What the simulator does not model

The simulator (src/sim/) is the host tests' fake board. Nothing below is checked by them; each one is for the unit:
- `beep` records the frequency only: not the duration, nor the timing of the tone.
- `cycles` follows the simulated awake time at 1 GHz (plus an optional step per call), not the CPU's cycle counter (DWT): no measured time awake there.
- The sleep between passes is whatever a test hands `prof::Clock::slept`: its real length and wake reason are not tested.
- The panel's DMA fallback: the simulated panel takes its pixels by plain writes, not by the DMA stream (hw.rs `PanelDma`).
- The USB core (OTG, PHY, VBUS timing): the dev link runs against a fake computer (`Pc`), not the cable.
- DBP (the backup domain write enable, PWR_CR1): the simulated backup registers are always writable.
- The MPU, the caches and the memory attributes: no protection and no cache lines.
- The eMMC: the card is a model (`EmmcCard`, src/sim/emmc_model.rs) with inferred timing and stalls; the responses of the commands in `expected_resp` are checked and an unlisted command fails the test; the data commands (8, 17, 18, 24, 25) go through `emmc_read` / `emmc_write`, which do not check a response type.

## Not yet seen on the unit

How to read this section: the items are grouped by area, the phone link first (it is being tested now). Each block
gives where it was seen (simulator, emulator, unit), then the steps on the unit, then the details (why, numbers, file
names). A feature not listed here was seen on the unit. What was fully seen is in the last list, "Seen on the unit".

### Phone and phone app

**Intervals.icu upload and planned workouts (docs/app.md "Rides up to Intervals.icu", "Planned workouts from Intervals.icu to the unit")**

Seen: core tests, and the Simulator in Demo mode (canned calendar, workouts sent to the fake unit, ride checks). Not seen against the real account: a real key must upload a new ride once (201), answer 200 for the same file again (it must count as uploaded, and the link must open the activity), and a planned workout of the next 7 days must come down as `.zwo` through `/events/{id}/download.zwo` (the route and the plain `/events` list are inferred). Not seen on the unit: a planned workout arriving in its workout list through an EMMC_WRITE build, a move on Intervals.icu deleting the old file, and a rider's action in the middle of a calendar write (the write is dropped; the unit's reaction to the next WRITE_BEGIN is unverified).

**Profile from Intervals.icu (docs/app.md "Profile from Intervals.icu")**

Seen: app core tests (parse, merge, fuzz, a fake unit round trip) and the Simulator build with the Demo answer. Seen on the unit? No, and not against the real Intervals.icu API: with an EMMC_WRITE build, a real key and the unit connected, "Sync from Intervals.icu" must change the FTP in the unit's menu, and it must still be there after a restart; during a ride the line must say it will try again, and on a build without card writes it must say READ ONLY.

**Training profile over the phone (PROFILE_GET / PROFILE_SET, docs/app.md "Training profile")**

Seen: simulator (get, set, read back, kept after a restart, each refusal, a ride, fuzz), app core tests. Seen on the unit? No: a RAM run with an EMMC_WRITE build, the phone bonded, sets the FTP from the phone; check that the unit's menu shows it and that it survives a restart.

**Workouts read by the phone (LIST_IN / READ_IN of `\Workout\calendar`, docs/app.md "Workouts")**

Seen: simulator (a folder changed between the two commands, fuzz), app core tests, the iOS Simulator demo. Reading only.

On the unit: a RAM run (`tools/dev.py run`, no ride) and the app bonded: LIST_IN of the real calendar lists the owner's workouts, each reads whole and decodes, and a file changed on the card between the list and a read gives NOT FOUND.

**Workouts sent and deleted by the phone (WRITE_BEGIN / WRITE_DATA / WRITE_END / DELETE, docs/app.md "Workouts")**

Seen: simulator only (both MTUs, every refusal, a power cut after every sector written, the fuzz), and the app's "Send to
the unit" and workout Delete against the Demo unit in the iOS Simulator (core tests: round trip, each status, a resend
writes once, both MTUs). Not seen with the real app on the real unit.

On the unit, with the owner present (it writes the card):
1. Build the card-writing ride image (`EMMC_WRITE=1`, which is also what makes the commands answer anything but READ
   ONLY: the default RAM-run build answers 5) and install it as the owner installs a ride build (docs/install.md);
   `tools/dev.py info` says `card writes yes`. No ride running.
2. Bond the phone, enable Data, write WRITE_BEGIN for today with the size and CRC-32 of a small workout file (one that
   `tools/fit_check.py` or the app's core decodes), then its bytes in WRITE_DATA frames of 14 bytes (and once of 238
   on the fast link: Control must accept 244 bytes, which only the unit's radio chip can say), then WRITE_END: END ok
   with the size. Check the unit's WORKOUTS list shows it today, LIST_IN lists it and READ_IN gives the same bytes.
3. The same with a wrong CRC (MISMATCH, nothing new in WORKOUTS), during a ride (BUSY), then DELETE of the listed key:
   END ok, gone from WORKOUTS.
4. The same from the phone app (an `EMMC_WRITE` RAM build, owner present): editor, Send to the unit for today, the app's
   list shows it, the unit's WORKOUTS and stock's calendar both show it; send it again (still one); swipe Delete in the
   app, gone from both. A card without `\Workout\calendar` should say "no workout calendar yet".
5. Over USB (`dev.py drive`) the volume passes the computer's fsck, and `\Rustfw\Inbox\WORKOUT.TMP` is there (kept on
   purpose). Switch to stock: it lists the workout in its calendar for that day (stock reads `<name>.fit` of the day
   folder; that it needs no `.json` beside it is inferred).

**Courses sent and deleted by the phone (folder 2, `\Courses\`, docs/app.md "Courses from the phone")**

Seen: simulator only (both MTUs, the followed course refused, every refusal, a power cut after every sector of a write
and of a delete, the fuzz) and the app core against its fake unit. Not seen on the unit, and the app has no route
screen yet.

A cut between the temporary file's remove and its link, or inside a delete, leaves the course's clusters lost (up to
64 for 1 MB): accepted, never reclaimed by the unit, freed by a computer's fsck (check.sh runs fsck_msdos / fsck.vfat
on both cuts, `target/fat/*_lost.img`, and allows orphan clusters there only).

The first course makes `\Courses` (a workout its day folder): its entry torn on a card without reliable write left it
live on cluster 0 in 2 of 64 tears (2026-11-08), every later send answering I/O until a computer's chkdsk. Since then
`Fat::mkdir` writes a folder's entry as a move's link (deleted, then made live by its first bytes): no tear leaves it
on cluster 0 (host tests: every write of a mkdir cut and torn, fsck_msdos on the cut between, `mkdir_cut_lost1.img`).

On the unit, with the owner present (it writes the card; an `EMMC_WRITE` build, no ride):
1. Send a FIT course (one the unit already follows when copied over USB) as WRITE_BEGIN `06 seq 02 size crc len name`,
   WRITE_DATA, WRITE_END: END ok with the size. The route shows in the unit's COURSES menu under its name and is
   followed (COURSE ON, the map draws it); LIST_IN 2 lists it and READ_IN gives the same bytes.
2. While it is followed, DELETE of its key: BUSY, still in COURSES. Stop following, DELETE again: END ok, gone from
   COURSES (an open COURSES menu lists again by itself).
3. A 1 MB course: time WRITE_END's answer (up to three passes of 1 MB in one loop pass; about 0.3 s expected) and check that the
   phone link does not drop meanwhile.
4. Over USB (`dev.py drive`) the volume passes the computer's fsck; switch to stock: nothing of it minds `\Courses`.
5. On a card without `\Courses`, the first course sent makes it; COURSES with 64 courses answers FULL at WRITE_BEGIN.

**The fast phone link (2026-11-07, roadmap 3c; `nrf::CFG_FAST`, app/filesvc.rs)**

Seen: simulator; Bryton's real radio app in the emulator (`src/sim/nrf_session_tests.rs`); unit 2026-11-07 (RAM run, iPhone 16 Pro), steps 1 to 3 done, speed measured twice. The chip-bytes wake build (main 7bd424c5) is not measured on the unit yet.

On the unit:
1. RAM run (`tools/dev.py run`; the default build has the fast link, the ride build does not, `FAST_LINK=1` forces it), cable in: `tools/dev.py info` shows the radio started and the PHONE menu shows a FAST LINK row (it appears only when the chip took the configuration; no row means the fallback). Seen 2026-11-07: the row is there, the chip took the MTU 247 configuration.
2. Phone app or nRF Connect bonded, no ride: connect, request MTU 247 (iOS asks by itself; nRF Connect has a button), read the MTU back. Seen 2026-11-07: iOS negotiated MTU 247.
3. Enable Data, READ a ride (`02 seq 00 00 00 00 <name>`), time it and compute KB/s (target 40 to 80; the simulator says 45), compare the bytes with the file read through USB. Seen 2026-11-07: a 468,042-byte ride came out byte-identical to the USB copy (`dev.py drive` + `cmp`); speed below.
4. The phone's connection interval during the READ (nRF Connect shows 15 to 30 ms) and back to what the phone had set after the END, a Cancel, a cut link, with the 1M PHY (the unit remembers the connect event's and the last update's parameters; none known: it sends nothing).
5. Media keys, a notification banner and a call still work during and after a transfer.
6. A trainer connected at the same time keeps its readings and its control through a READ.
7. An hour with the link up and no ride: the radio chip makes no silence (the suspect of 2026-11-01 was never shown).
8. Only then one ride with the owner on a build with `FAST_LINK=1 EMMC_WRITE=1`.
9. Read `full <n> turns <n>` in `dev.py info` after a READ on the build of main 7bd424c5 or later: `full` high means raise `nrf::FAST_QUEUE` to 8 (the FAST LINK row vanishing means the chip refused it) or the phone is the limit (its interval); `full` near 0 and still slow means the loop or the UART (about 3.1 ms a notification caps it near 78 KB/s).

Details:
- Speed on the unit, 2026-11-07: about 3 KB/s at event length 3 (the simulator said 34 KB/s: it was optimistic, it let a whole notification through an event whatever the data length). After `nrf::FAST_EVENT_LEN` 6 (7.5 ms) and the connection event length extension (`sd_ble_opt_set` 0x67, id 2; RAM run of 20989f2a): 16.6 KB/s. `info`: mtu 247 int 12 (15 ms) phy 2 dle 251 ext 1 boost 0 0 0; last read 383,218 B in 23,022 ms; hvn 1152 for 1871 notifications sent. The radio link is no longer the limit (2M, 251, 15 ms all granted).
- `dev.py prof` (38.6 s, about 13 s of it a READ): asleep 78.8 %, step 16.9/s mean 8.9 ms, radio 36.7/s mean 2.6 ms, phone 16.9/s mean 3.0 ms. Each notification is one blocking UART round trip of about 3.1 ms. The fix in main 7bd424c5: the chip's bytes wake the READ's 10 ms sleep (`LoopPolicy::begin_sleep`, `wake_on_io(keys off, bytes on)`), so an HVN_TX_COMPLETE refills the freed slots at once; simulator 44 KB/s before and 54 KB/s after (`a_byte_from_the_chip_ends_the_sleep...`). `FAST_QUEUE` stays 4.
- Read it in `dev.py info`: `phone link mtu 247 int <1.25 ms units> phy <1|2> dle <octets> ext <0|1> boost <p d y codes> last read <B> B <ms> ms hvn <n> sent <n>`; `int` 24 is 30 ms, 12 is 15 ms; `phy` 1 means the 2M request was not granted; `dle` 27 means the default data length (the cause of 3 KB/s); `ext` 0 means the extension was refused; `boost` is the result of the three calls (0 accepted).
- Still needs the unit: the real SoftDevice accepting the configuration (the app passes ram_base 0x20006800 to `sd_ble_enable`; `sd_ble_enable` answering NO_MEM falls back to the two links, then to the defaults; the need for MTU 247 and a queue of 4 is inferred, no primary source found), and its own refusals (hvx NO_MEM / RESOURCES). ANCS data can come in pieces up to 244 bytes now: events over `nrf::KEPT` (264, with MAX 264) are dropped while a command waits; check a long notification still shows whole.
- The chip app's layer, the throughput model and the simulator's chip: docs/app.md, "Fast link: the chip app's layer and the speed model".

**File service on the phone bond (2026-11-07, roadmap 9 A1, reads only; app/filesvc.rs)**

Seen: simulator, and on the unit the fast-link READ above.

The GATT table is frozen (`the_phone_gatt_table_is_frozen`, 28 rows): a change costs every bonded iPhone one forget and re-pair, and there is no Service Changed (Bryton's app hangs on 0x54). The rule and the app's message: docs/app.md. Still owed on the unit: the message seen on the phone after a stale bond.

On the unit (phone bonded, no ride running, in nRF Connect or any BLE tool that is the bonded phone: the attributes need encryption):
1. The service `aa3c0001-98d8-4772-99ed-dfb117c6746f` is listed.
2. Enable notifications on Data (`...0003`), write `01 01` to Control (`...0002`): one `81 01 <12 digits> <size>` per ride, then `83 01 00 <count>`.
3. Write `02 02 00 00 00 00 <12 digits of a ride>`: `82 02 <18 bytes>` frames until `83 02 00 <size>`; compare the bytes with the file read through USB.
4. Cut the link mid-read and resume from the bytes kept.
5. During a ride the same commands answer `83 .. 02` (BUSY).
6. Time a whole ride (expected about 360 B/s at MTU 23, 50 ms passes) and check that the stack really queues one notification (a second in a pass answers NRF_ERROR_RESOURCES; the simulator models it).
7. Check the 10th `uuid_vs_add` answers OK, and read `sys_attr_get`'s length after enabling Data (ATTRS is 64; inferred need about 32).

Details:
- The bond's saved CCCDs (`ATTRS`, 64 bytes) must hold Data's as well as the HID report's and the battery's.
- The vendor UUID bases are now 10 of the stack's 10 (8 for the phone, the FE-C trainer, this one): an eleventh needs `sd_ble_cfg_set`. FE-C's base is registered at the phone setup, before the file service's, so a chip holding fewer costs the file service (no service in the table), never the trainer.
- The service answers only to the stored bond (Just Works pairing in the 60 s PAIR window is the protection). The recorder's eMMC urgent path (lib.rs `emmc.urgent(!ride_settled())`) is set on a ride start and cuts a running back-off, so a read error before it does not delay the first checkpoint.

**The iPhone app against the unit (2026-11-07, docs/app.md)**

Seen: unit 2026-11-07 (iPhone 16 Pro, iOS 27): the app shows S510, rustfw and the battery.

On the unit:
1. After the unit switches firmware (stock to this one, or a new image), forget S510 on the iPhone and pair again: iOS keeps a stale GATT table because there is no Service Changed characteristic yet.
2. After a RAM run, the new bond lives only in RAM: forget S510 on the iPhone before pairing again.

**The phone link (BLE, app/phone.rs)**

Seen: simulator.

On the unit, in this order, with the dev link up (`tools/dev.py run`, then the screenshots) and an iPhone at hand. A build run from RAM without EMMC_WRITE=1 keeps the bond only until a reset: after one, forget "S510" on the iPhone before pairing again (the iPhone keeps a key the unit no longer has).
1. Set-up: MENU > PHONE: LINK reads OFF (the chip took every GATT set-up command: services, characteristics, the advertising data, the 8 vendor UUIDs). ERR xx/yyyy instead names the command refused and its error (for example A2 characteristic add with 0004 NRF_ERROR_NO_MEM: the attribute table is too small without `sd_ble_cfg_set`, inferred). The heart-rate strap still reads (ANT+ unchanged).
2. Advertising: PAIR: LINK PAIRING; the iPhone's Settings > Bluetooth lists "S510" under Other Devices within a few seconds (the advertising data: HID and battery services, ANCS solicitation; the name in the scan response). The BLE NEARBY scan of a second unit, or nRF Connect, sees the same.
3. Pairing: tap S510 on the iPhone: "Bluetooth Pairing Request ... S510 would like to pair with your iPhone", Pair (Just Works, no code); then iOS asks whether S510 may receive the iPhone's notifications: Allow. The unit: banner PHONE PAIRED, LINK SECURE, the clock set to the iPhone's time (and the date), Settings > Bluetooth shows S510 connected. Note the order the events came in if pairing fails (the security request at connection: is the iPhone's prompt shown without opening a HID characteristic?).
4. Media: start music on the iPhone; SENSORS > PHONE > REMOTE shows its title, artist and PLAYING with the time running; Start pauses (AMS), Page next, Lap previous, held Page / Lap the volume (the iPhone's volume HUD). Hold Lap on a ride page: play / pause. The MUSIC field.
5. Camera: open the iPhone's Camera, hold a finger on the REMOTE screen: a photo (HID volume up). If the banner says KEYS NOT ENABLED, the iPhone did not subscribe to the HID report: note whether it lists S510 as a keyboard.
6. Notifications: with the iPhone locked, a message from another phone: the banner shows "sender: text" within a second or two; the overview's notices keep it. NOTIFY OFF, then a message: nothing.
7. Calls: call the iPhone: the CALL screen with the caller's name; Start answers it (on the iPhone too); a second call, Back declines; a third, answered on the iPhone: the screen closes.
8. Reconnection (an EMMC_WRITE=1 build, the bond written: `tools/dev.py ls Rustfw` lists ble.bin, 512 B): switch off and on: 3 s after the start LINK WAITING, then SECURE without any prompt on the iPhone; media keys work at once (the CCCDs given back). Walk away until the link drops: LINK WAITING, and back in range it reconnects.
9. FORGET: LINK OFF, nothing advertised; on the iPhone, forget S510 too.
10. Re-pair and strangers (2026-10-24 review): bonded, forget S510 on the iPhone only, then PAIR on the unit: the iPhone pairs again without FORGET on the unit. A second phone (not paired) taps S510 while the unit only waits (no PAIR): pairing refused, and within 30 s the unit drops it (LINK back to WAITING).
11. Cost: `tools/dev.py info` awake time with LINK OFF, WAITING and SECURE (the Nordic chip's own current is not measurable here; estimated 10 to 15 uA advertising once a second).
12. The RTC set (`Board::rtc_set`, RM0399 init mode): the time right after the phone set it, and kept through a power off; stock afterwards still shows the right time.

**Phone link: the notification's app (sim only)**

Seen: simulator.

On the unit (iPhone): a message from WhatsApp, Messages or Mail shows "WHATSAPP: sender: text" in the banner and the list (Get App Attributes on the Control Point, answered in notifications of 20 bytes).

Details: the ATT MTU stays 23 (the raise to 103 was removed on 2026-11-01, suspected of the radio chip's silences on that day's ride; not proven). Closed, not planned (owner, 2026-11-02: "let's not break something that works"; the gain is fewer packets for long ANCS texts).

**Long app identifiers (2026-11-01, found by the chip's command table, src/sim/chip_table.rs)**

Seen: simulator.

On the unit: a notification from a long-named app, the link watchdog's `radio resets` unchanged.

Details:
- The Get App Attributes request for an app of 18 bytes or more ("com.apple.MobileSMS" is 19) is a GATT write of 21 to 27 bytes, and the chip's write buffer holds 20: its decoder refuses it and the chip hangs (Bryton's nRF app, reversed).
- The app never sends one: its guard skips those ids, so those apps show no display name. `ble::write` also refuses a value over `WRITE_MAX` (20), as defence in depth, and nothing is sent. A longer name could come from a prepared write (not done).

### Radio and sensors

**The Nordic link watchdog (app/nrf_watch.rs)**

Seen: simulator.

On the unit:
1. `tools/dev.py info` ends with "radio resets 0" over a normal ride and a quiet hour (no sensor, no phone).
2. With a strap and the phone linked, reset the chip from the dev link (`poke` of its reset line, not a DFU command): both come back within a minute and "RADIO RESET" shows once; the ride keeps recording through it.

**The radio chip's reset no longer blocks the main loop (2026-11-01, app/nrf_watch.rs `restart_step`, `Board::nrf_hold`)**

Seen: simulator.

On the unit: `tools/dev.py info` `radio resets` after forcing a reset, the screen and keys alive meanwhile, the sensors back about 1.2 s after the reset began.

Details:
- A reset used to hold the pass for about 1 s (PA6 low 150 ms, 800 ms of boot, then the start's commands): the GNSS went unread for 2 to 3 s, the screen froze and keys were late. It is a state machine over the passes now; the simulator's worst pass during a live chip's reset is under 1 ms of its time (was 950 ms and more), a dead chip's 520 ms (one command time-out; was 1470 ms).
- A truncated frame resets at once (first break of a calm period); the power-off resets a mis-framed chip first (about 1 s more before standby).

**Review 3 of the chip's reset (2026-11-01)**

Seen: simulator.

On the unit: force a reset mid-command (main.rs `nrf_hold`): no `resyncs` after it; a switch-off during a reset ends with the chip in system off (current in standby, as after a normal switch-off; see "The radio chip's switch-off made certain").

Details: nothing is sent while PA6 is low or the chip boots (the TX FIFO is flushed at the hold: USART_RQR TXFRQ); the rider's sensor choice and the PAIR window during a reset are applied after it; a put is bounded to 1 s whatever the chip floods.

**The Nordic send path: a frame is never cut short (2026-11-01, `nrf::send`, main.rs `nrf_send`)**

Seen: simulator (`chip_link.rs`, BOARD.md 11, byte by byte; its tests reproduce the deadlock: two commands back to back behind a stalled event frame).

On the unit: `tools/dev.py info` at a ride's start and after 10 min, `radio resets` and `resyncs` unchanged, commands answered while sensors stream (a phone's pairing with three sensors on).

Details:
- `nrf_send` used to give up after 10 ms on one byte (the chip holding CTS) and return mid-frame, which hangs the chip for ever. It now returns how many bytes the TX FIFO took, and `nrf::send` reads the chip's bytes while CTS is held, bounded by 500 ms without a byte either way, each wait a 1 ms sleep; a frame that went out partly marks the link broken so the watchdog resets the chip at once.
- ANT events that arrive during a command are kept for the next pass (`Rx::keep_last`), not dropped.

**The radio receive's wait (main.rs `nrf_recv`): finding 8 taken out (2026-11-01)**

Seen: simulator (`ChipLimits`, `src/sim/ride_radio_tests.rs`).

On the unit: a ride with the owner's sensors, `tools/dev.py info` at the start and after 60 s: each kind's count after `ant` grown by about 4 a second per sensor, `resyncs` and `radio resets` unchanged; the awake cost back to the pre-finding-8 figure (est. 2.4 to 3.2 ms a second more).

Details: the short wait of finding 8 (2026-10-31) is back to 200 us after every byte. The chip's app needs more than 20 us (inferred) to pull and encode its next event, so the 20 us wait ended each read after one frame; the drain fell behind a ride's events and the chip's 96-slot queue overflowed, which hangs it for ever (the ride's silence about 30 s after every reset).

**The 100 ms ride pass taken out (2026-10-31 review finding 9; out on 2026-11-01)**

Seen: simulator.

On the unit: a ride with the owner's sensors, the `ant` counts per kind over 60 s, `resyncs` and `radio resets` unchanged.

Details: the second suspect of the ride's radio dropouts (the Nordic chip went silent as a whole about 30 s after every restart on the 2026-11-01 road ride; the first suspect, ATT MTU 103, is out as well, each in its own commit to be tested apart). A running ride is back on the short pass with the chip's wake as before 18e270e; the awake cost is back to about the pre-18e270e figure (1.62 % riding, against 1.05 % with the 100 ms pass, as measured then). Finding 10's gating stays.

**The ANT search time-out stays at 30 s (2026-11-01)**

Seen: simulator (`the_search_timeouts_of_all_channels_together_stay_under_the_hang_limit`).

Details: after a chip reset every channel times out together; the simulator's burst (8 channels, SEARCH_TIMEOUT and CHANNEL_CLOSED each, on a queue already at 22) peaks at 37 of the 48 the chip's pull rule allows, a margin of 11.

**The ride's hang, reproduced in the model (2026-11-01, `ride_radio_tests.rs` `the_bursts_of_a_ride_...`)**

Seen: simulator.

Details:
- After the differential test fixed the response order, the old configuration (100 ms pass, 20 us wait) keeps the queue near 70 to 88 of 96, and a burst on top hangs it: the phone's 12 notifications as 48 events in one pass (3 hangs in 5 minutes) and 32 ANT events after a suspend (1 hang); the 8 channels' search time-outs alone (16 events) do not.
- The production configuration never hangs under the same bursts: worst high-water 48 of 96. Which burst the ride really had is still open (the model shows it could be either).

**`ant events lost` in `dev.py info` (2026-11-02, EVENT_QUE_OVERFLOW 0x35)**

Seen: simulator.

On the unit: that the real SoftDevice sends it, in a 44-byte ANT event with the code at byte 42 (the layout is inferred from the other events), and that the count stays 0 in a ride with the owner's sensors.

**The radio chip's switch-off made certain (2026-11-02, BOARD.md 11 "Power off", drivers/nrf.rs `system_off`)**

Seen: unit 2026-11-02 (owner: fine): the unit starts again with the power button and the radio comes back, the time from the 3 s press to the screen off is unchanged. The chip's RTS releasing in System OFF is seen (`tools/dev.py nrfoff`, BOARD.md 11).

On the unit: the standby current with a meter is not planned (owner, 2026-11-02: needs the unit opened); System OFF confirmed by the chip's lines instead (BOARD.md 11).

Details:
- The command is read through (the chip's frame in flight drained), the chip is seen off (USART2 TC set, CTS released for 3 ms, then PD3 following a 1 ms pull-down: released, not driven by a hung chip), else reset and asked again, else held in reset (lines left as they were); seen off, PD3..PD6 analog, no pull. The beep and the screen go off first.
- For reference, stock's System OFF is 0.3 uA; a chip left running is 0.3 mA and more.
- PA2 driven low (2026-11-02) dropped the USB dev link after a few seconds, nothing else seen (inferred: the USB PHY's enable or supply); in standby every STM32 pin is high-impedance anyway, so PA2 cannot change the standby current.

**The Nordic chip after a power off and on** (`sd_power_system_off`, then the reset pulse)

Seen: not yet. On the unit: switch off, power-key start, the radio and sensors come back.

**The ANT+ search rests growing (app/radio.rs `search_rest_ms`)**

Seen: simulator.

On the unit: leave a paired sensor switched off: searches at 0:00, 1:40, 4:10, 8:40, then every 4 min 40 s (the high-priority search may add 10 s each); found at once after a ride's Start; the battery with one absent sensor against none (docs/power.md).

**Sensors other than the heart-rate strap**

Seen: simulator. On the unit: speed, cadence, power (the decoders follow the ANT+ profiles, checked against openant).

**Torque effectiveness and pedal smoothness (drivers/sensors.rs page 0x13, the TE L/R and PS L/R fields)**

Seen: simulator.

On the unit: pair the owner's Sigeyi spider PM and pedal for a few minutes with the TE L/R box on screen. A value means it sends 0x13; "--" all ride means it does not (or its bytes are 0xff, one-sided). Then note which, here.

**Heart rate intervals with a power meter paired (sensors.rs `each_interval_once`, radio.rs)**

Seen: simulator.

On the unit: DFA a1 and the hrv messages on a ride with the strap and a power meter after the fix, each interval once (the count of hrv intervals against the beats).

**Sensor names and the strap's battery (drivers/sensors.rs pages 7 and 80, app/menus.rs SENSORS row, app/fields.rs HR BATT)**

Seen: simulator.

On the unit:
1. The strap's percent on HR BATT (its page 7 on the owner's strap, ANT+ profile layout as openant reads it).
2. A SENSORS row reading "POLAR HR" after the strap sends page 80.
3. The row back to "HEART RATE" after a forget.

**Power meter calibration (src/drivers/powercal.rs, app/powercal.rs, SENSORS > CALIBRATE POWER)**

Seen: simulator.

On the unit with the Sigeyi AXO spider: does it answer 0xAA with 0xAC (and which offset), does it report auto zero (byte 2), does a message to a sleeping meter fail cleanly.

Details: the page 0x01 layouts (ids 0xAA, 0xAB, 0xAC, 0xAF, 0x12; offset in bytes 6 and 7) are from the Nordic SDK and GoldenCheetah; the request bytes and the acknowledged send on the power channel (the same op as the trainer's, itself not yet confirmed) are inferred; the 10 s time-out is ours. Sigeyi's own documentation of this was not found (retailers list auto zero and a manual zero offset only).

**Electronic shifting (sensors.rs, app/radio.rs)**

Seen: simulator; the owner's Di2 at hand, `di2_key.bin` built in (`tools/ant_key.py`).

On the unit:
1. SENSORS > DI2 reads OFF (not NO KEY: the chip took network 1's key and channel 6's assignment).
2. Start finds the Di2 within seconds (turn a crank or shift to wake it), its number shown green.
3. GEAR shows front-rear and follows the shifts. With CHAINRINGS and CASSETTE set in DI2 KEYS it shows teeth, "50x17": check front 1 is the smallest ring and rear 1 the largest cog, and that front and rear are not swapped (both inferred; the second from stock's gear ratio code).
4. SHIFT BAT shows the percent stock's Sensor Details showed (60 % on the patch project's picture; the byte is inferred to be a percent).
5. DI2 KEYS: set R PRESS to NEXT PAGE, press the right hood button once: one page; a long press: its action; the left button too (the sides inferred from stock: byte 1 right, byte 2 left).
6. START set on a button, restart the unit after a press: the ride must not start by itself (the Di2's first button page is taken as the baseline, inferred: it repeats the last press byte).
7. A ride with shifts: SHIFTS counts them, and the saved file has gear change events (fitdecode, or Intervals.icu's gear chart).
8. A Di2 switched off (or out of range): GEAR goes to "--".
9. An ANT+ Shifting system (SRAM AXS) if one can be borrowed: SHIFTING row, gear, battery status.

Details: stock reads the teeth from the Di2 info record (`Sensor_Di2StoreGearConfig` 0xd02889f2, layout not decoded: open fact).

### Trainer

**Power match (training/power_match.rs, BIKE > POWER MATCH)**

Seen: simulator.

On the unit: an ERG ride with the Sigeyi paired:
1. The power meter reads the workout's target within a few % after a minute, with the "PM+x%" on the trainer screen.
2. The correction is held, then eased back when the meter is unplugged.
3. OFF sends the target as set.

**The home trainer, ANT+ FE-C (src/training/trainer.rs, drivers/fec.rs, app/trainer.rs; phase 1)**

Seen: simulator's model of a trainer only.

On the unit with a smart trainer:
1. SENSORS > TRAINER finds it (channel 7, type 17, 8192, which is not stock's channel 8); its power, cadence and speed on the pages.
2. The acknowledged send (`sd_ant_acknowledge_message_tx`, op 0xc8, `[6 c8 ch 8 data]`, inferred from stock's encoder, Ghidra notes agent_b2.md) is accepted by the chip.
3. The capabilities page (54) comes after the request (page 70).
4. The trainer takes pages 49 (ERG under a workout), 48 and 51 (SIM on a course; the trainer's speed advances the ride), 55; its page 71 answers pass.
5. Not seen yet: the chip's EVENT_TRANSFER_TX_COMPLETED (code 5; the layout `[0x40, ch, 0x01, 5]` is from the ANT Message Protocol document), which stops the release being sent again (five times at most without it).

Details: the pairing is kept in `\Rustfw\sensors.bin` (with the Bluetooth trainer's); backup register 14 is the shifting systems', and a trainer an earlier build kept there is only moved to the file.

**Level mode and the slope scale (training/trainer.rs `Mode::Level`, app/trainer.rs `trainer_drive`), Van Rysel D900**

Seen: simulator.

On the unit:
1. LVL on the trainer screen: level 1 then 20 feel lightest and heaviest, +/- by LEVEL STEP, page 48 (and FTMS op 0x04 over BLE) applied.
2. A course followed in SIM with SLOPE SCALE 50 %: the trainer's resistance on the hill is about half of 100 %'s, and the grade stays inside -10 to +20 %.

**The trainer's release (app/trainer.rs, nrf.rs `ant_tx_completed`), Van Rysel D900**

Seen: simulator.

On the unit:
1. The release at stop, end, pause and boot (once).
2. The switch-off release: one free resistance sent before the chip goes off, the D900 free in the next seconds with the ride saved (ANT+ and over Bluetooth); the 330 ms wait enough.

Details: `TX_COMPLETED`'s layout (`[0x40, ch, 0x01, 5]`, ANT Message Protocol 9.5.6.1) is read from the document, not seen.

**Spin-down calibration of the ANT+ FE-C trainer (trainer.rs `Cal`, drivers/fec.rs pages 1 and 2, app/trainer.rs, ui/trainer.rs)**

Seen: simulator's trainer only.

On the Van Rysel D900 showing CALIBRATE:
1. CALIBRATE beside MODE (or Start held) sends page 1 with the spin-down bit (0x80); the trainer answers with page 2.
2. The screen says PEDAL TO nn KM/H, then STOP PEDALLING, then CALIBRATED with the spin-down time from its page 1.
3. The status bit clears afterwards; the ERG target comes back.

Details: the layout is GoldenCheetah's, from the profile rev 4.1 (fec.rs doc); the temperature and speed condition bits and the target speed in 0.001 m/s are to be seen on a real trainer; the request's reserved bytes and a request with both bits (when the trainer asks for the zero offset as well) are inferred; that the trainer leaves the calibration by itself after CANCEL (nothing is sent) is inferred.

**The trainer in the full copy (storage/fit.rs "The trainer", app/riding.rs `trainer_sample`)**

Seen: simulator's trainer only.

On the unit: one ERG ride on the Van Rysel D900 with the power meter, then read `\Rustfw\Activities\<name>.fit` (`tools/fit_check.py`'s decoders, or Intervals.icu's fields):
1. `target_w`, `trainer_w`, `meter_w`, `fe_state`, `trn_stat`, `link`, `cmd_sent`, `ack_fail` every second the trainer is heard and none after it stops.
2. The `device_info` with the D900's pages 80 and 81 (the software version's scale from openant is inferred).
3. The clean copy in `\Activities` unchanged and still synced by Bryton's app.

**The two links' configuration and the Bluetooth trainer, FTMS (nrf.rs `start`, drivers/ftms.rs, app/ftms.rs; phase 2)**

Seen: simulator's chip and trainer only.

On the unit, in this order:
1. `sd_ble_cfg_set` (`00 69 20 00 00 00 01 01 02 03 00`, then `00 69 40 00 00 00 01 01 01 00`) answered 0 and `sd_ble_enable` after it answered 0 (else the chip is reset and started as before: `tools/dev.py info` would show two resets at the start, and SENSORS' BT TRAINER says NO LINK).
2. The phone still pairs and comes back, advertising on tag 1 (the PHONE menu, media keys, a notification).
3. SENSORS > BT TRAINER lists a real FTMS trainer (its service data or its UUID list); Select connects to it (`sd_ble_gap_connect` tag 1, stock's layout) while the phone is linked; its data on the pages; ERG under a workout, SIM on a course (the Control Point's responses: the trainer screen does not say NOT ACCEPTED).
4. The trainer switched off and on: the link made again; the phone unaffected throughout.

Details: inferred and to check there: the cadence bit of Indoor Bike Data read as present when set (pycycling's reading, the specification's table says the opposite), a resistance level spread over the trainer's own range, Cw 0.51.

**The Bluetooth trainer without FTMS, FE-C over BLE (drivers/fecble.rs, app/ftms.rs; phase 2)**

Seen: simulator's D500 only (`FtmsSim::new_fec`); from a computer on 2026-10-26 (bleak, no bike on it) the owner's Van Rysel D900 (it has no FTMS): name `VANRYSEL-HT-xxxx`, 0x1818 and 0x1816 and the 128-bit 6e40fec1-b5a3-f393-e0a9-e50e24dcca9e in its advertising or scan response, characteristics 6e40fec2 notify and 6e40fec3 write; its notifications are `A4 09 4F 00 <page> <xor>` (message 0x4F, channel 0, ANT's XOR checksum), pages 16 and 25 at about 4 Hz and 81 now and then; page 25 says calibration needed (byte 6 bits 4 and 5); a page 70 request for 54 or 71 got no answer on channel 5 or 0, so the capabilities may never come over BLE; whether a target (page 49) is applied needs the bike on it.

On the unit with the D900:
1. SENSORS > BT TRAINER lists it as `FE-C` (the 128-bit UUID may be in the scan response only: the search waits for it); Select connects; `sd_ble_uuid_vs_add` for the base answers 0 (the phone link took 8 of the 10 vendor UUIDs; a duplicate answering the existing type is inferred, not seen) and the service is found.
2. The notifications on 6e40fec2 give pages 16 and 25 (power, cadence, speed on the pages); the capabilities request (page 70) is answered by page 54.
3. Which checksum the trainer takes: the frame is `A4 09 4F 05 <page> <checksum>`; the XOR of all the bytes (ANT's) is sent until a notification shows the sum of the bytes after the sync (pycycling's), which is then used. If the trainer ignores the writes, look here first (a write with response, one per 250 ms; its ATT answer would say refused).
4. ERG under a workout (page 49), SIM on a course (page 51), the trainer's page 71 answers pass; the link made again after the trainer is switched off and on.

Details: pairing keeps the protocol in `\Rustfw\sensors.bin` (`F`).

**The ramp test (ramp.rs, app/ramp.rs, WORKOUTS > RAMP TEST)**

Seen: simulator.

On the unit with the D900 or another trainer:
1. The 5 min at 50 %, then the one-minute steps in ERG.
2. The stop on cadence under 60 rpm, on power under half the target, on Lap and on STOP WORKOUT (the cool-down target at once).
3. The result (FTP at 75 % of the best minute, HRVT1 with a strap), APPLY and KEEP by touch, and the trainer released afterwards.

**RIDE A COURSE (WORKOUTS)**

Seen: simulator.

On the D900: pick a course, the ride starts indoors, the trainer follows the grades, the profile's rider moves with the trainer's distance, DONE at the length.

Details: known limit: a reset mid course resumes the ride, not the course.

**An indoor ride saved (src/storage/fit.rs sub_sport, recorder.rs)**

Seen: simulator and host tests (every sector cut).

On the unit:
1. An INDOOR ride (a trainer heard at Start, or the ride menu's INDOOR row).
2. A restart mid-ride that comes back INDOOR, then a save.
3. The file's laps and session with sport cycling and sub_sport indoor_cycling (6).
4. Synced: a Ride marked trainer on Strava, the indoor flag on Intervals.icu (docs/sync.md, "Indoor rides").

### Recording and storage

**A resumed ride goes on by itself once ridden (2026-11-03, owner: "Resume automatically")**

Seen: simulator (`a_reset_mid_ride_goes_on_once_riding`, `a_ride_paused_by_hand_stays_paused_after_a_reset_and_auto_pause_off_waits_for_riding`, `a_resumed_ride_goes_on_when_riding`, `fit_check.py --timer` on `autoresume.fit`).

On the unit:
1. A restart mid-ride (`tools/dev.py reset`), then riding on without a key: the ride comes back paused (RIDE RESUMED) and goes on at the first second at speed, cadence or power, the time off an auto stop in the file; the saved timer against the screen's.
2. A restart while paused by hand: it stays paused until Start.

**The ride file's timer stops at an auto-pause (2026-11-02, as Garmin's)**

Seen: simulator, read by fitdecode and Garmin's SDK (`auto_pauses_stop_the_files_timer`, `a_reset_during_an_auto_pause_keeps_the_timer`, `fit_check.py --timer`).

On the unit: a ride with stops, its saved timer against the screen's, and Intervals.icu's / Strava's moving time.

Details: timer stop_all and start events with timer_trigger auto at each stop and when moving again; each lap's and the session's total_timer_time leave the stops out, so they are the screen's timer (the unit had saved 241 s for a 4.5 min ride at 0 km/h showing TIMER 0:00); a reset during a stop comes back paused with the stop's timer.

**A power cut with the backup domain lost, then the boot (2026-11-02, app/riding.rs `left_by_a_power_loss`)**

Seen: simulator only, with the RTC back at 2000-01-01 and stopped until the boot starts it (lib.rs `begin`, main.rs `rtc_start`, BOARD.md 8).

On the unit, with the owner:
1. Before and after a full drain, a `tools/dev.py peek` of RCC_BDCR (0x58024470: LSEON, LSERDY, RTCSEL, RTCEN) and RTC_PRER (0x58004010): whether the drain resets the backup domain at all (no coin cell seen), and the values stock leaves (its LSE drive, LSEDRV bits 3..4).
2. A ride recorded a few minutes, the battery run flat (or VBAT cut), the boot: no CLOCK NOT RUNNING, the clock counting from 2000-01-01, RIDE RESUMED, paused at the last checkpoint, then the save; both copies (`\Activities`, `\Rustfw\Activities`) with records after the resume, and the next ride's full copy with records too.
3. A first start of this firmware on a unit that never ran it: nothing resumed, stock's settings imported.

Details: a boot with none of this firmware's backup registers set reads the ride's record once (read only) and resumes a ride left open as the marker's path does (a card that does not answer then: the resume pending, stock's settings imported once it answers with no ride).

**A card that answers late at boot (2026-10-31, app/riding.rs `step_recorder`)**

Seen: simulator.

On the unit: a ride recording over a checkpoint, the unit restarted (four buttons) with the card failing at the boot, and without Start the ride coming back paused with RIDE RESUMED once the card answers, the marker kept (`tools/dev.py peek` of RTC backup register 30) until then.

Details: no way to force that on the unit yet: a RAM run whose first eMMC commands fail, `tools/dev.py poke` on the card's clock, or a dev build that refuses the first tries.

**A write the card answers `Busy` after landing it (2026-11-02, fat/write.rs module doc)**

Seen: simulator only (`a_write_that_lands_but_fails_loses_a_cluster_at_most`): at most one cluster lost a failed write, 4 over a 75 s start with 1.2 s stalls; a computer's fsck reclaims them.

Details: the policy is "on a write that fails after landing, at worst a cluster is lost (fsck reports it), never freed while it may be named": no cluster is given back (an undo that itself lands and fails would leave a chain or handle naming a free cluster: a cross-link, a lost ride) nor taken back; a link that failed (landed or not) is not taken by the handle, the next grow takes another cluster and its link overwrites the failed one in both copies.

**The card model's rules (2026-11-02, src/sim/emmc_model.rs E1 to E6)**

Seen: simulator only.

On the unit, with the write bench: the longest CMD13 busy over a few thousand sector writes, and R1 bit 6 in `tools/dev.py emmc`'s status.

Details:
- The stall is now the card's busy (CMD13 answers programming until it ends; over the driver's 1 s it gives `Busy`, the ride goes on); R1's error bits are sourced from Linux's mmc.h. The stall's length and period, and whether the unit's card raises URGENT_BKOPS (EXCEPTION_EVENT, R1 bit 6) at all, are *inferred*.
- Datasheet check (2026-11-02): Linux allows a write 10 s of busy (`MMC_BLK_TIMEOUT_MS`), the driver 1 s (`emmc::TRIES`; SK hynix lists 1 s for a power-on GC, 600 ms for an erase); a card may abort a write on a voltage drop and set ERROR + CC_ERROR only in the first CMD13 after the busy (inferred: the vendor sheet is not public). The model has both (`Cost::stall_ms`, `EmmcCard::prog_error`); the driver is unchanged, an ignored test shows its 1 s today.
- Other open facts: the cache (EXT_CSD [33], on in iNAND would lose acknowledged writes in a power cut) and CMD6 busy (GENERIC_CMD6_TIME [248]) are not read from the unit yet.

**Transfer times behind the simulator's cost model (2026-11-01, `Sim::cost`, src/sim/mod.rs)**

Seen: simulator; inferred: the card's write time (1 ms a write command, 0.2 or 2 ms a sector: the gate runs at both) and its stall tail (300 ms after every 1024 sectors). The panel, I2C and GNSS times are computed from the clocks (main.rs); the read is 430 ns a byte (a scan's measured end-to-end rate).
To take on the unit, silently (`tools/dev.py` only: no key presses, no eMMC writes from the owner's rides):
1. The DWT cycles of one 512-byte `emmc_write` and of a 64-sector run on a spare scratch file in `\Rustfw` (needs the write build and the owner present, so later).
2. The cycles of one `gnss_transfer` of 32 bytes and of one `i2c_read` of 6 bytes on I2C4 and I2C1 (`dev.py info`'s DWT counter around a poke of the firmware's own read, or `prof`).

Details: the gate's named exemptions (`EXEMPT` in src/sim/budget_tests.rs, the owner's performance worklist) are the passes that break a limit with these costs: ride start (about 1.1 s of keys unread at 0.2 ms a sector, 1.8 s at 2 ms), the checkpoint under a card stall, the save's read-back of a 3 h file, the screenshot's write.

**First ride on a fresh card (2026-11-01, src/sim/budget_tests.rs)**

Seen: simulator.

Details:
- The recorder creates its 20 folders and files on the first ride (setup 202 sectors, make_kept 154) and syncs the whole FAT three times (1.69 MB read, about 727 ms): the Start pass is 1051 to 1730 ms, the gate's one named exemption for it. On a card that already had a ride the Start pass is 53 to 97 ms, and the prepared-card rows (keys 108 to 136 ms, GNSS under 1.1 s) hold the general limits.
- Open owner decision: prepare the card's folders at boot or when idle, or buffer the first seconds in RAM (a branch's approach, not merged: the golden rule wants ride data on the eMMC as it goes).

**The recovery's chain check and the read-only drive (storage/recorder/mod.rs `still_ours`, the record's checked mark, app/drive.rs `left_open`; review S1 to S6)**

Seen: host tests only (cut at every sector, chains freed or reused by another file).

On the unit:
1. A ride left open, the unit restarted, the card lent to a computer: USB DRIVE READ ONLY with "RIDE LEFT OPEN: START A RIDE".
2. The ride deleted from the computer after a Start, and the next boot letting it go without touching the file that took its clusters.

Details: GIVE UP SAVE's new order (the link, then the record "done") needs a card that really fails, so it may never be seen.

**The app's state files made at a ride's start (storage/recorder/mod.rs `make_kept`; review S7)**

Seen: host tests only (a card without them, cut at every sector, plain and torn).

On the unit:
1. On the owner's card the first ride start after this build writes `\Rustfw`'s directory sector once (the eight files, bikes.bin and places.bin among them, `Deleted\Full` and `Log` created zeroed: review 2026-10-31 finding 1; the files already there are left as they are).
2. `dev.py ls \Rustfw` shows all of them, and the pages, the bond and the sensors are still read and saved afterwards.

**The settings' file (`\Rustfw\settings.bin`, app/settings_file.rs)**

Seen: unit 2026-10-29 (RAM run, EMMC_WRITE=1): the first start created the file (seq 1); a FONT change wrote seq 2 into the other copy; after `dev.py reset` the older installed build b0861d8 booted, and re-running f5d7f3e kept the change; changing it back wrote seq 3.

On the unit: switch to stock and back; stock's settings and rides as they were, `\Activities\` and `\System\` untouched, and this firmware's settings back.

**The save fixes (fat/write.rs `make_room`, recorder.rs)**

Seen: host tests with a cut at every sector and a card with failing sectors.

On the unit: a save on a card with a full `\Activities` folder, and a Start while the last ride is saved ("NOT RECORDING: CARD", then the ride starting once the save ends).

**GIVE UP SAVE (recorder.rs `give_up_save`, app/riding.rs)**

Seen: simulator and host tests (a card whose directory sectors fail to write or read).

On the unit: it needs a card that really fails, so it may never be seen: the row appearing in SYSTEM while stuck, the four banners, and `\Rustfw\Rescue` or the lost chain checked from a computer (fsck's `FILE0000.CHK`).

**The end-ride screen (app/riding.rs `end_input`, recorder.rs "A discarded ride")**

Seen: simulator. Needs an EMMC_WRITE=1 build.

On the unit:
1. A short ride, Start, Start held: END RIDE, the time and distance, RESUME selected; Page and Lap move, Back and RESUME leave it paused on its page; taps with gloves on each button.
2. SAVE: the card (it stays on Page and Lap, goes on Back or Start); `tools/dev.py ls Activities` and `Rustfw/Activities` have it, the history lists it.
3. Another ride, DISCARD: "DISCARD RIDE?", KEEP selected; DISCARD: "RIDE DISCARDED"; `tools/dev.py ls Rustfw/Discarded` and `Rustfw/Discarded/Full` have it, `\Activities` not, the history and the totals as before; `tools/sync.py` (dry run) does not list it.
4. A restart (`tools/dev.py reset`) with the screen up: "RIDE RESUMED", paused.

Details: the first ride after this build makes `\Rustfw\Discarded\Full` at its start (the FATs compared first, a few seconds).

**DELETE in HISTORY (storage/trash.rs)**

Seen: simulator. Needs an EMMC_WRITE=1 build.

On the unit:
1. A ride's card, Start held (or a tap on its red DELETE), then Start: "RIDE DELETED", the list without it, the totals lower at once.
2. `tools/dev.py ls Rustfw/Deleted` (and `Rustfw/Deleted/Full` for one of this firmware's rides) has it, `\Activities` not; the next start's totals the same (the cache written without it).
3. On stock: the ride gone from its history and nothing else changed (its `.sum` / `.csum` left in `\System\History\Summary`).
4. Moved back from the computer, the ride back in both histories.

Details: whether stock lists a summary without its FIT is inferred from Ghidra, not seen.

**The page editor and `\Rustfw\pages.bin` (app/editor.rs, page.rs)**

Seen: simulator. Needs an EMMC_WRITE=1 build.

On the unit:
1. A hold on a ride page's box opens the editor (the frame, the hint in the strip's place, live values); SPLIT, MERGE, TALLER, UNDO, a field; Back keeps, DROP CHANGES asks; ADD PAGE up to six, the Page key round them all; DELETE PAGE.
2. `tools/dev.py ls Rustfw`: `pages.bin`, 1024 bytes.
3. Restart: the pages as made (the file is read 2 s after the start: a page shown before that is the registers' copy).
4. While a ride records: no editor by a hold, no EDIT THIS PAGE in the RIDE menu until paused.

Details: the first write compares both FAT copies first (as the scan's cache: some 10 s of passes, inferred; the loop stays responsive). The installed stock-switch case: stock and an earlier build of this firmware still read pages 1 to 3 from registers 20 to 25 (layouts of up to six fields).

**The scan's cache (storage/stock.rs `Cache`, `\Rustfw\scan.bin`)**

Seen: simulator. Needs an EMMC_WRITE=1 build.

On the unit:
1. The first start reads every ride as before (`tools/dev.py info` awake permille during it: now at most about 15 ms of work a pass, so lower than the 717 seen; Back held 3 s switches off during it), then `\Rustfw\scan.bin` is there (64 + 64 bytes a ride; `tools/dev.py ls Rustfw`).
2. The next start: the overview's totals at the first screen, no 30 s of `--`, and the scan 30 s later reads nothing (`tools/dev.py info` stays low).
3. A ride saved: the next scan reads only it. A ride deleted from the computer: gone from the totals.
4. The first write of the cache (also after a cache that did not check out) first compares both FATs: check the loop stays responsive (`tools/dev.py info` uptime, a key) and the time it takes.
5. An emptied `scan.bin` (from the computer) is written again with its first cluster (fsck clean).
6. A key held while the computer copies to the drive counts its real time (Back held 3 s switches off, not 2 s).

Details:
- The FAT comparison reads sectors 0 to 29,807 of each, about 15 MB in all: 3 to 4 s in one go at 10.5 MB/s, done 16 sectors of each a step within the pass's 15 ms (`Cache::prepare`): some 10 s over 160 passes, inferred.
- A ride saved during the scan is read (its FAT sector read again each pass); a read error leaves a ride out of the totals until the next start, not for good.
- `tools/dev.py bench scan` now reads only rides not in the cache: measure it with the cache deleted.

**Screenshots and the fault log (app/screenshot.rs, app/faultlog.rs, storage/screenshot.rs, storage/faultlog.rs; hw.rs's marker now keeps the kind and detail)**

Seen: simulator only; both write the card (an `EMMC_WRITE=1` build).

On the unit:
1. SYSTEM > SCREENSHOT on the overview and on a ride page, then the drive lent and `\Screenshot\` opened on the computer (the picture as `tools/dev.py shot` shows it, the time it took).
2. A fault on purpose (a `tools/dev.py poke` to an unmapped address: a CPU fault with PRECISERR or IBUSERR) and a panic, then after the restart `\Rustfw\Log\faults.log` read on the computer (one line each, the CFSR bits as named, the marker's register 29 read with `dev.py peek` showing bit 10 set).
3. A fault mid-ride: the line written only after the resumed ride is saved.

**Three bikes and part wear (src/storage/bikes.rs, src/app/bikes.rs)**

Seen: simulator.

On the unit:
1. The first start making BIKE 1 from the settings' wheel and ODO (`\Rustfw\bikes.bin` written once, after the next ride if the file was not there: a ride's start makes it, review 2026-10-31 finding 1; settings.bin untouched).
2. ACTIVE BIKE switching the wheel and weight (the speed from a wheel sensor follows).
3. A ride saved, then within a minute its distance on the active bike and its parts.
4. A power cut right after "RIDE SAVED" counting the ride once at the next start.
5. A part's SERVICE passed and its notice at the start.
6. The scan's time to count on the unit's card (70 rides).

**Raw RR intervals in a real ride's full copy (storage/fit.rs `Writer::hrv`, recorder.rs `Recorder::beat`)**

Seen: simulator and host tests.

On the unit: a ride with the owner's strap: the `hrv` messages in `\Rustfw\Activities` (about 20 KB an hour), `ArtPct` on the records, `tools/fit_check.py` decoding them, and the beats offline in Domestique, Runalyze or Kubios (roadmap: the artifact filter comparison).

**GRAVEL, CX and COMMUTE profiles**

Seen: simulator.

On the unit:
1. BIKE > PROFILE cycling to them and back to AUTO.
2. A GRAVEL ride (file as gravel_cycling, auto-pause at 3 km/h, continuous tracking), a CX one (cyclocross, never pauses, laps by hand), a COMMUTE one (commuting, no climb warnings, the sunset alert at 30 min with a fix).
3. The unit restarted mid-ride coming back with the same profile and sub_sport; the profile kept in settings.bin across a restart.

**MTB mode (2026-10-27)**

Seen: simulator.

On the unit: an MTB ride started, the unit restarted mid-ride, then saved; the file reads as mountain biking in Intervals.icu; the receiver switching from cyclic to continuous tracking at an MTB ride's start and back after it; climbs ahead on a real path network.

**The full copy of a ride and the sync (docs/sync.md)**

Seen: simulator.

On the unit: `\Rustfw\Activities` written beside `\Activities` on the card, with Alpha1 from a real strap; `tools/sync.py` against the real Intervals.icu and Strava (DFA a1 charted from the upload), and the workouts it writes listed in WORKOUTS (and by stock, which may want the `.json` beside them).

**A power cut mid-ride (the battery connector or a hard power off)**

Seen: the reset mid-ride and the resume (2026-10-24, see "Seen on the unit"); a cut while a sector is written is covered by the host tests and the card's reliable write.

**Stock reading this firmware's rides** (\Activities\261024083403.fit, 261024083931.fit) and the Bryton app's sync

Seen: not yet. On the unit: stock lists these rides, and the Bryton app syncs them.

### GPS, navigation and maps

**The receiver's answers and its SPI limits (2026-11-01, BOARD.md 13 "The receiver's rules")**

Seen: simulator. Until the unit answers, G1 to G4 and G6 are inferred.

On the unit, with `tools/gnss.py`:
1. Read back (VALGET) the four CFG-SIGNAL keys after the start-up and CFG-SPI-MAXFF / CFG-SPI-EXTENDEDTIMEOUT.
2. MON-COMMS (txErrors, tx buffer usage) after a ride.
3. A deliberate 2 s gap between reads to see the stream stop.
4. The MGA-ACK info codes for ANO with CFG-NAVSPG-ACKAIDING on.

Details: in the simulator the longest unread gap is 2.2 to 3.3 s in the link, recorder and segment tests and 6.1 s in the 20 us hang test: the Nordic reset's 950 ms, the start-up after it, against the receiver's 1.5 s time-out, so one to three NAV-PVT epochs go missing.

**The GNSS receiver after a stall, indoors (2026-11-02)**

Seen on the unit 2026-11-02 (see the list at the end).

**GNSS SYSTEMS and AssistNow Offline (`gnss::systems`, `mga::Ano`, `tools/assist.py`)**

Seen: host only.

On the unit:
1. CFG-VALGET of the CFG-SIGNAL-GPS/GAL/BDS/GLO_ENA keys (0x1031001f, 21, 22, 25) reads what the setting says after power-up and after a change (and the receiver keeps tracking after the restart of its engine).
2. With `mga.ubx` on the drive, `tools/gnss.py` shows UBX-MGA-ACK answers if CFG-NAVSPG-ACKAIDING is on, and the receiver takes the messages at 840 B/s without loss.
3. Time to first fix: with the unit off for over 4 hours (no ephemeris left), outdoors in the open, power it on and read the time from power-up to the first NAV-PVT with fixType 3 (`tools/dev.py` `info`, or the strip going green) five times with the file and five times without it (rename it).
4. Also whether the receiver accepts ANO of a system it is not set to track (GLONASS with GPS+GAL).

Details: the same with the position and time aiding only is the baseline (BOARD.md 13: not measured yet).

**GPS POWER AUTO (`gnss::Adaptive`)**

Seen: host only.

On the unit, a road ride:
1. `tools/gnss.py` CFG-VALGET of 0x20d00001 reads 2 after 11 minutes fast and straight, 0 within a second of a stop or a hairpin.
2. The track and the speed against FULL's (and a speed sensor's) on the same road.
3. The share of the ride in cyclic tracking.

**GPS POWER SAVER (u-blox cyclic tracking, `gnss::config`)**

Seen: host only.

On the unit, outdoors:
1. The configuration answered ACK-ACK (`tools/gnss.py`, CFG-VALGET of 0x20d00001 reads 2).
2. NAV-PVT still read every second over SPI between the receiver's sleeps (no gaps: the strip's GPS stays green, no fix dropped after 3 s).
3. The track beside FULL's on the same road, and the battery against FULL (docs/power.md, measurement plan).
4. Which constellations the receiver uses (CFG-VALGET of CFG-SIGNAL-*_ENA: the default or what stock left).

**The GNSS receiver off when idle (app/gps.rs `gnss_idle`)**

Seen: simulator.

On the unit:
1. Indoors, the strip's GPS goes blank 5 minutes after power-on; outdoors with a fix, 10 minutes after the last key.
2. The GPS menu, a map page or Start bring it back, and the time to a fix then (expected about 30 s, the ephemeris not kept).

**The GNSS receiver off for an indoor ride (app/gps.rs `gnss_switch`, BOARD.md 13)**

Seen: simulator.

On the unit:
1. With a trainer heard, Start switches the receiver off (strip GPS blank).
2. After SAVE or DISCARD, or once the profile goes to ROAD, it is powered again 1 s later and gives a fix outdoors (how long, with the time and position aiding); the current saved to measure.

**Barometric altitude (src/nav/altitude.rs)**

Seen: simulator.

On the unit: the sign and size of the pressure swings from wind at speed (the 3 m ascent hysteresis is inferred, BARO_HYSTERESIS_MM), ALTITUDE and FROM GPS in SETTINGS > BIKE against a known height, the ride start's own calibration, `\Rustfw\altcal.bin` kept over a restart, the chip's current at a reading a second (datasheet, not measured), `Alt_P0` of a real DeviceData.ini.

**The ascent filter on a ride (altitude.rs, ride.rs)**

Seen: offline on the records of one real ride.

On the unit: a ride's ascent against a known climb and against stock's, with a calibration made at the start.

Details: the median of 5, the 1/6 average, the slewed reference and the 3 m hysteresis are tuned on the records of one real ride (341 m became 123 m on a real 60 km ride, offline).

**Favourite places (src/nav/places.rs, src/app/places.rs)**

Seen: simulator.

On the unit:
1. SAVE PLACE at home with a real fix; PLACES' distance and bearing against a known place a kilometre away; its altitude typed in its menu.
2. At the next ride's start there "ALTITUDE SET AT PLACE 1" and the ride's altitude at that height (no ascent counted for the correction).
3. `\Rustfw\places.bin` kept over a restart; a place saved during a ride written only after the save.

**Climbs ahead (src/nav/climbs_ahead.rs, storage/route.rs, storage/inflate.rs, app/ahead.rs)**

Seen: simulator, host tests, fuzz, and a real region file read on the computer (1009 of 1014 tiles decode, 5 are over the graph's 2560 nodes; traces from 199 random places: 194 made, median 4 km, 111 climbs).

On the unit, with a map file of the region copied to `\Map\MapData\<Region>` (stock's own has them):
1. A ride shows CLIMB and CLIMB M after the first seconds (the tiles read one a pass; a 445-node tile is inflated 16 KB into the SDRAM buffer: measure how long a pass takes with `tools/dev.py info`; inferred 20 to 50 ms).
2. The banner and the beep 50 m before a climb the road ahead has; the trace made again when the road bends away from the one traced; the climbs against what stock shows for the same ride.
3. `tools/dev.py info` ends with "climb tile us last N max M": the time of the last and the longest tile read, inflated and decoded (estimated 15 ms at the median, about 80 ms for the largest tile that fits the 96 KB buffer, 2080 nodes).
4. The AXI SRAM cell stays in `.bss` (`make_ahead`, about <!-- n:ahead -->126 KB<!-- /n -->) and the unit boots (a new static in `.axibss` is a core-level change: Renode, then `DIAG_STOP`).

**The sliced tile load (climbs_ahead.rs, storage/inflate.rs, storage/route.rs, app/ahead.rs)**

Seen: host instruction counts, scaled.

On the unit, on a ride: `tools/dev.py info` ends with "climb tile us last N max M slice max S": S is to be seen under a few ms.

Details: before the change one tile took up to 238 ms in one pass on a ride; the longest slice is estimated at about 3 ms.

**Descents and STEEP DESCENT (nav/course, app/descents.rs)**

Seen: simulator.

On the unit: the banner and beep 300 m before a steep drop on a followed course and on the road ahead (the max grade over 50 m from the map's elevation, smoothed, may read differently from the road); the DESCENT AHEAD field.

**Peaks and cols along a course (storage/named.rs, NEXT POI, PROFILE marks)**

Seen: simulator, on synthetic tiles.

On the unit:
1. A followed course through a region stock's cache holds (zoom 12, 11 or 9 under `\Map\Data\Temp`): its cols and summits found within 100 m, the marks on the profile, NEXT POI counting down.
2. The scan's cost on the main loop (not measured: one H read of 500 bytes and at most 8 P reads a pass by construction; `tools/dev.py info`).

Details:
- The tile format is inferred: the name, then `ele` as a u64 of metres after the point, was read off a real cache on the computer (the stock document says only "8 bytes"), not on the unit.
- Missing for more: stock caches only the tiles its screens showed, so a course through a region it never drew has no names; filling the cache, or reading the names from the map files' vector tiles (`\Map\MapData`, gzip MVT, `route.rs` reads the same files), would be a new reader.
- Water points and shops: stock's `poi` classes (4.5) have no drinking water, and `tools/map_pack.py` does not write them yet (it draws roads, paths, rail and water only): it would need to write `natural=peak|saddle`, `mountain_pass` and `amenity=drinking_water` with names and elevation into a pack the unit reads.

**CLIMBS DONE (app/climbs_done.rs)**

Seen: simulator.

On the unit: a real climb of a followed course or the road ahead listed at its top with a plausible time and VAM, the banner, the average power and heart rate, the list empty after a reset-resume.

**The climb pacer on a real climb (pacing.rs, app/pace.rs)**

Seen: simulator.

On the unit: a course followed with a climb of 5 minutes or more: the TOP field within 5 km of it, the plan's power against what the rider can hold, the time to the top counting down, the W' balance staying above 0 at the top, the bike weight added; the replan's cost at 200 MHz (about 75 us for 32 sections, computed on the host).

**Course reverse, the way to the start and BACK TO START (app/courses.rs, nav/course)**

Seen: simulator.

On a ride:
1. Follow a course, REVERSE COURSE and ride it the other way (turns the other way round).
2. Start away from a course and see NEXT TURN give the compass point and distance until joined.
3. Ride out, BACK TO START in the ride menu, and ride back on the breadcrumb with its cues.

Details: a reversed or way-back course is not kept across a reset: after one mid-ride nothing is followed (the ride itself resumes).

**Courses on the unit**

Seen: host, on one unit's backup (the 14 routes of `\PlanTrip`: every one decodes, lengths within 0.25 % of the files' own, climbs close to the ones Bryton's app marked).

On the unit:
1. COURSES over the card's own `\PlanTrip`: the time the lengths take to come (each file read whole, 96 KB a pass: about 550 KB for those 14, some 6 passes).
2. A course ridden: the window keeping up at speed, the off-course alert in a town's parallel streets, the climb banners, a lap course rejoined after a detour.
3. Turn by turn: the cue banners and their beeps at speed (200 m, or 20 s if farther, then 32 m or 4 s, on 1 s fixes).
4. The turns found on a real route's shape: no cue on a winding road or a switchback climb, every junction where the road turns 45 degrees or more at a corner, a real roundabout's exit right, counted in either direction, and no EXIT at a junction's jog, kinks or a swerve; how many junctions taken in a wide sweep go unsaid.
5. A Garmin Connect, Komoot, RideWithGPS or BRouter course's cues (BRouter's roundabouts, its Locus style's cues on the track points).
6. The arrows and the ring readable at 2x in the banner and 3x in the field, END at the field past the last cue.
7. The time a course's cues take at its read (host: about 1 ms for 4000 points and 256 cues; a waypoint off a lap course bounded to 8 chunks).

**Live segments, detection (app/segments.rs, step S3; no effort written)**

Seen: simulator.

On the unit: the index built from `\Rustfw\Segments\` on the card 5 s after the start (no pass slower), the load at a start, the events (`dev.py` reads them), no lost ride on a segment's start or finish.

Details: AXI SRAM's free space is the live `n:axi_free` marker in docs/layout.md.

**Live segments, efforts (app/segments.rs, step S5)**

Seen: simulator. Needs an `EMMC_WRITE=1` build.

On the unit:
1. A segment ridden faster than its PR, the ride saved, then `\Rustfw\Segments\<id>.pr` and a line of `efforts.csv` written after the save only (never during the ride, nothing for a ride discarded), read back on the computer.
2. The next pass over the segment with the `.pr` as its PR target (a delta about 0 at the same pace).
3. Stock still reads the card.

**Live segments, screens (app/segment_ui.rs, ui/segment.rs, step S4)**

Seen: simulator.

On the unit, a ride over a known segment: the approach banner with the name, the CLIMB page opening by itself (and found again by paging), the delta ahead and behind, the segment's profile with the target's mark, the coach's gauge, the finish banner and the card, the round going on without the page; the page's redraw cost on the unit.

**Map HEADING UP and PINCH (DISPLAY > MAP, touch.rs `feed2`)**

Seen: simulator.

On the unit:
1. Heading up: on a ride, the top follows the course in steps, stays put at a stop, the red north arrow points the right way; the time awake of one step (`tools/dev.py info`, DWT cycle counter).
2. Two fingers (pinch): whether the chip reports both (slot 2 at 0x16, inferred from the protocol doc and 0x3f = 5) and a pinch steps the zoom one level; the second finger must show in 2 samples in a row.

Details: the sim says one map redraw of 79 k panel pixels about every 18 s on a winding ride. On an IGN view add `raster_turned`: about 25 cycles a pixel over the tiles' boxes, so about 2.5 M cycles (6 ms at 400 MHz), estimated by counting operations (two divisions a row, then adds), not measured.

**IGN maps (`\Rustfw\Maps`)**

Seen: simulator.

On the unit: the packs found, a view drawn from them, its time, the "(c) IGN" corner, the 2.1 MB map memory zeroed at boot.

Details: simulated: one index sector and about 45 sectors of tile data per tile, at most 6 tiles, so about 150 KB a view change, as stock's basemap; the decoding is 87 us a tile on the computer, about 3 to 6 ms on the unit with the D-cache off, inferred.

**OpenStreetMap packs (`\Rustfw\Maps\osm`, `tools/map_pack.py`)**

Seen: simulator, on a synthetic area.

On the unit:
1. A pack of the owner's own area made from an Overpass extract and copied over USB (docs/install.md), drawn at zoom 14 with its tracks and paths and at 13 and 11 to 12 with the roads.
2. The time of a view change (in the sim it reads one index sector and the tile's blobs, a few hundred bytes to a few KB a tile, against stock's cache files).
3. A lake assembled from several ways (not drawn: the tool takes closed ways only); a pack larger than a block (several files).

**The basemap (roads and water from stock's cached tiles, `\Map\Data\Temp`)**

Seen: simulator, with real tiles.

On the unit: the FAT volume mounted by our reader, the tiles at the last known position, the time a view change takes (simulated: about 140 KB read, 84 K buffer pixels, 73 K panel pixels), the 1.5 MB SDRAM zeroing at boot (`hw::map_memory`).

**The map's zoom buttons ("+" and "-" on the full-page map, now any level, coarser than the fit too, and the crosshair back to the fit)**

Seen: simulator.

On the unit:
1. Hit them with a gloved finger indoors (the fit at 2 m: "-" must zoom out, the drag pan).
2. The time of a re-centre over IGN at 1.5 and 2 m a pixel (`tools/dev.py mapbench`), and whether one every 26 to 35 s is acceptable awake time.
3. IGN zoom 16 drawn at 0.85x at 2 m (before: zoom 15 at 1.7x): sharper, or one-pixel lines broken by the shrink?

**Panning the zoomed map by a drag**

Seen: simulator.

On the unit: the drag read from the panel's points (no swipe taken for a page change while zoomed in), the time of the redraw at the release (a basemap render plus the whole map, about 79 K pixels in the simulator), the crosshair button.

**The map on a real ride**

On the unit: its awake time, and the last-known-position dot after a power cycle.

### Power, sleep and board

**Stop mode, phase 2b (2026-11-03, docs/power.md "Stop mode": Stop between a ride's passes)**

Seen: simulator. **Off in the card-writing build** (`EMMC_WRITE=1`, the installed ride image) until this procedure passes; on in the plain build.

On the unit, with the owner: the ride build with it on (`EMMC_WRITE=1 POWER_STEPS=127`) run from RAM (`tools/dev.py run`; `info` ends `ride stop built yes`), the heart-rate strap and the power meter paired, the phone linked:
1. Start a ride, wait past its first minute, then `tools/dev.py idlestop 600` (the bench allows Stop with the cable in) and ride or spin the sensors for the 10 minutes.
2. During it, by eye: the backlight at 10, 30 and 60 % and on AUTO, steady, no flicker (LPTIM1 on the LSE runs in Stop by the data sheet, not yet seen with the backlight lit).
3. After: `tools/dev.py info`: ride Stops on most passes (the simulator: 77 % of about 19.5 a second, so near 9,000 in 600 s; a pass meeting a byte of the chip's or our bytes going out sleeps in WFI), ride 0 failed, radio resets 0, resyncs 0, ant events lost 0, each sensor's broadcasts at its rate (HR 4 Hz).
4. `tools/dev.py prof`: `wake` about 15 calls a second (its us up to 3 times short, on the HSI; the SDRAM marks' few us in it), the awake share up by the wakes' share.
5. A lap, an auto-pause (stand still), a pause and a resume during the bench, then the save: the FIT through `tools/fit_check.py` (a record a second, no time twice, the laps and the session's timer and distance), its full copy's `hrv` messages present with RR near the strap's, and the HRV field on the ride page alive.
6. A key, a tap and the power key during the ride's bench: each acts at once.
7. Then, and only then, on by default in the card-writing build (main.rs `POWER_STEPS`); on battery, an installed build (with the owner): a ride, the battery history against one without (`POWER_STEPS=63`).

**Stop mode, phase 2a (2026-11-03, docs/power.md "Stop mode": Stop between idle passes, no ride open)**

Seen on the unit 2026-11-03 (RAM run of 361c5fc, cable in, `idlestop 150`, no touch): 377 Stops, 0 failed, mean 250 ms, 94.2 s in Stop; radio resets 0, resyncs 0; the USB link answered after; `appup` 178.29 s against the RTC's 179 s (1 s resolution, consistent).

Owed (the review's fixes since: a broken wake resets, `App::keep_on` blockers, the bench's drop not logged):
1. The same `idlestop` run on the fixed build: `info` as above, and no USB RECONNECT notice or new USB drop (`tools/dev.py usbdrop`) from the bench's end.
2. A key, a tap and the power key during an idle Stop: each acts at once (no 250 ms lag); the cable plugged in (from battery) starts USB at the next pass.
3. The same right after a switch-off and a power-key wake (the 931d594 case: WKUPF1 and PDDS left set).
4. On battery, an installed build (with the owner): a night idle, the battery history against one without (`POWER_STEPS=95`).

**Stop mode, phase 1 (2026-11-03, docs/power.md "Stop mode")**

Seen on the unit 2026-11-03 (931d594, cable in; from RAM, `tools/dev.py run`, no ride open, nothing written to the eMMC): `srefresh 100` and `1000` OK (flags 0xff: self-refresh in and out, the clocks put back, woken by the timer, the image's CRC unchanged; slept 99 and 1000 ms); `stop 1000` came back and reset, but **no STOPF**, and the wake's waits were a few cycles: the clocks never stopped, so the system stayed in Run (D1 at most in DStop).

Owed, in this order (each answers one suspect):
1. `tools/dev.py stop 1000 --keep`: Stop with USB as it is, no reset; the line's `pre` / `post` registers (decoded below it: PWR_CPUCR, PWR_CPU2CR, PWR_D3CR, RCC_D3AMR, RCC_CR, RCC_CFGR, EXTI_CPUPR1..3) say what stayed on and what was pending. If the link dies, `tools/dev.py stopreport` after a replug reads the verdict in register 0.
2. `tools/dev.py stop 1000 --usboff`, then `tools/dev.py stopreport`: USB fully off (core and ULPI clocks, the PHY's enable PA7, PA2, HSI48, VOS3) as on battery. `Stop OK` (STOPF, clocks stopped) means USB is what held the system in Run: phase 2 only stops without the cable anyway. Still no STOPF: the `--keep` registers (the M4's PWR_CPU2CR, D3AMR, a pending line) are the next lead.

**The sleep power steps (main.rs `sleep_power_init`)**

Seen on the unit 2026-10-25 from RAM, masks 0, 8, 4, 1, 3 and 15 (boots, dev link, BLE, keys, a full redraw at 16 MHz clean, PLL3 off, a ride recorded and discarded by keys; with 15 the owner checked touch, backlight, the USB drive and the power button).

Still to see: a GPS fix outdoors, the start / pause beeps, and the gain (battery draw idle on the overview, before and after).

**BATT SAVER (SYSTEM, `App::saver_on`)**

Seen: simulator.

On the unit: the same ride (same route, brightness, pages) once with it OFF and once ON:
1. The battery's percent and `tools/dev.py info` awake share read at the start and after an hour: the drain per hour ON against OFF (estimated gain in docs/power.md "Battery saver").
2. The "S" in the strip; the map redrawing every 5 s on a map page.
3. AUTO turning on at 20 % with its notice and off on the charger.
4. The receiver in cyclic tracking (CFG-PM-OPERATEMODE 2) while ON; the recording unchanged (checkpoints as usual).

**BATT HRS (drivers/battery.rs)**

Seen: simulator. The time left on the battery, from the drain of the last 20 minutes.

On the unit:
1. On a real ride from full, its value against the strip's percent over the first hour (the drain it reports should match the percent the strip loses, 1 h of ride to about 5 to 10 % at a riding draw, as `tools/dev.py bat` logs it).
2. The value shown at 10 minutes (the first "--" ending) and while the unit is on the charger ("--").
3. "--" on the idle overview, where the drain is under 3.4 % an hour until the cell has dropped.

**Idle loop woken by a key (BOARD.md 5, main.rs `wake_on_io`)**

Seen on the unit 2026-10-29: 7 passes a second idle on the overview (`prof`, 1 s window, 0.91 % awake).

Still to see: a real press of each key, after 30 s idle, acts at once (not up to 250 ms later).

**The main loop's policy in the library (app/loop_policy.rs, review A8)**

Seen on the unit 2026-10-29 from a RAM run: 21 passes a second with the cable in, 0.98 % awake idle, a key acted on at once, the awake and CPU per mille of `info` sane after 40 s.

Still to see (nothing in those rules changed, the code moved):
1. A real key press after 30 s idle waking the loop at once.
2. A touch read during a USB drive copy.
3. Unplugging resetting the unit outside a ride and not during one.

**The stack canary (review S8; link.x `_stack_canary`, lib app/main_loop.rs `stack_ok`)**

Seen on the unit 2026-10-29 from a RAM run: `info` and `prof` as before (0.98 % awake idle on the overview), `dev.py stackfault` reset the unit and `peek 0x580040c4` read 0xfa000001.

Still to see: a DIAG_STOP-free cold boot of an installed build with it (nothing changed in the boot, so low risk).

**The smaller repaints (review P1 to P5; app/screens.rs, app/menus.rs, ui/mod.rs)**

Seen: measured from RAM 2026-10-29: `draw` 125 to 52 us a pass on the overview, awake 1.10 to 0.90 % with the cable in. The changes: the page's value boxes once a second, a banner's dismissal skipping the boxes under it, a menu value in its own box, the BLE list by row.

Still to see: the SENSORS and BLE NEARBY menus updating live with no flicker; a banner dismissed over a graph page leaving the graph whole; the key figures of README.md measured again with tools/profile.py from RAM (they hold the 2026-10-29 morning numbers).

**The I-cache from a cold boot**

Seen: from RAM. On the unit: a cold boot.

**A USB suspend (lib link/dev.rs `watch_usb`, board/usb.rs `Link::watch`)**

Seen: decided on the host only; macOS would not suspend the idle unit while awake.

On the unit, with the owner: plug the unit in, let the Mac sleep for a minute, wake it. Expected: the dot grey during the sleep, no "USB RECONNECT", `tools/dev.py info` answers at once after the wake (same uptime, no new enumeration), backup register 31 (`tools/dev.py peek 0x580040cc`) unchanged.

**The OTG_HS IRQ waking WFI**

Seen: not yet. The link may run on the 50 ms SysTick wake-ups alone.

**The USB drive (mass storage, msc.rs): to do**
High speed dropped, not planned (owner, 2026-11-02): full speed is the setting. The drops, what was seen and what to try are in docs/usb.md. Not yet: Windows, Linux, unplugging while mounted.

**Charging control**
The charger is on whenever USB is in (as the bootloader leaves it), with no temperature window yet (BOARD.md 12; the barometer's temperature can now feed it). The 3.3 V switch-off not reached.

**AUTO OFF (SYSTEM > AUTO OFF)**

Seen on the unit: switching off idle on battery confirmed by the owner 2026-10-28 (b0861d8; before it, a powered trainer heard held it on); 2026-11-02: the banner, its beeps at 10 s and 5 s, a key cancelling, the power button starting it again.

Still to see: it never switches off with a ride or recorder busy, a cable in, the drive lent, a write waiting, or movement; the setting survives a restart (register 18 bits 22..24).

**The sensor bus clear (2026-11-01, main.rs `i2c_clear`, BOARD.md 16 "Bus clear")**

Seen: never run on the unit. Nine SCL pulses and a STOP on PD12 / PD13, run at start when no chip answers and after 3 failed transfers in a row.

On the unit: check it with a scope or by holding SDA low from a test fixture, and that a healthy bus is left as `i2c_init` leaves it.

Details: the sim's sensor rules S1 to S3 (BDU lock, one-shot clear, new-data bit) and the touch chip's Idle (T2, the lost first report, 2 s) are inferred from the datasheets' wording, not seen.

**The panel at 50 MHz SPI**
An owner's look for noise or shifted pixels; if any, MBR /4 (25 MHz) in `spi_init`.

**Backlight levels (60 / 30 / 10 %) and the time-out**
Each by eye.

**The automatic backlight by eye**
Stock's curve exactly (its setting 1: 8 % in the dark, 20 % at dusk, 30 % to 50 lux, the 1 % minimum above 50 lux; a band chosen once a second, no hysteresis, 2026-10-27).

On the unit: is the panel readable in full sun and in a lit room at 1 % (a desk lamp read 480 lux on 2026-10-23), and does a light on a band's edge make the level step visibly?

**The touch panel with a finger (the chip itself answered: BOARD.md 17)**

On the unit: INT low while touched, the axes (x 0 left, y 0 top expected), the gestures (swipe left / right between pages, hold for the menu, a tap on a menu row), the backlight waking on a touch, and the swipe thresholds by feel (stock's: 37 px across, 48 px up).

**A tap on the panel after the stuck-IRQ fixes** (touch IRQ 23, panel DMA IRQ 11, main.rs `sleep`)

On the unit: a tap works.

**Pages moved off backup register 19 (stock reads it at boot)**

On the unit: the first start of a new build moves them; then stock, reinstalled, should keep its last battery voltage as before.

**The night review's fixes (2026-11-01, docs/review-2026-11-01.md)**

Seen on the unit (RAM runs only, cable in, no ride, no keys): main 21ea124 (the board fixes: delay_ms masking the touch and USB lines, the park, POIS in AXI) up 72 s, fault 0; main be83a5f (the main loop in the library, the radio-only idle passes) up 94 s, no reset, fault 0, no radio reset or resync, `card writes no`, 0.70 % awake idle over 94 s (`dev.py prof`; 0.91 to 0.93 % on 2026-10-31).

Still to see, before a ride build:
1. A ride with the owner's sensors (caps at 4 000 W meter / 2 000 W trainer, the stale trainer fields, `dev.py info`'s `climb tile us ... slice max` under the new 10 ms AHEAD_US with quarter slices).
2. A restart mid-ride with power (RIDE RESUMED: kJ, TSS, NP, max and a drained W'bal carry on).
3. A busy BLE scan while idle (the 16-radio-pass cap, the clock and auto-off not stalling).
4. `dev.py reset` / `stackfault` and a drive eject's restart through the new loop.
5. A killed `shot` client (output dropped, DTR / bus reset recovering).
6. An EMMC_WRITE build's first HISTORY delete on a full card ("CARD FULL", nothing written).
7. The route graph's edge count on the owner's own route file (`cargo test-host real_file -- --ignored --nocapture` with BRYTON_ROUTE_DAT: set EDGES to the measured maximum plus 10 %, today 6 144 scaled, not measured).
8. Engineer Mode's USB id and `iaprecover.py --boot-sum` before the next recovery write.

### Screens, fields and settings

**Gauge fields (ui/dial.rs: POWER RING, HR RING, CAD BAR)**

Seen: simulator.

On the unit: the time awake per loop with a power ring on a 1-field page against the plain value; the ring's look in the sun (LIGHT, OUTDOOR); the digits inside it at the 2 x 2 size.

Details: the sim sends 2 800 px/s against 1 200 with 0 to 60 W of noise a second; the box whole is 50 000 px.

**The power curve screen (app/curve.rs, ui/curve.rs, HISTORY > FITNESS > POWER CURVE and the ride menu)**

Seen: simulator.

On the unit: the 90-day curve after the scan, the green curve growing during a ride with the meter, the cursor by Page and by tap, both ends of the axis legible, and the redraw cost of opening it (`tools/dev.py info`).

**Training load: CTL, ATL, TSB, the chart and MONTHS (training/load.rs, app/pmc.rs, ui/load.rs, HISTORY > FITNESS)**

Seen: simulator, on synthetic rides.

On the unit:
1. The numbers against Intervals.icu or TrainingPeaks for the same rides (TSS of the power rides should match within a point or two; the hrTSS of a ride without power will be lower).
2. The chart after the scan; the three fields on a ride page moving as the ride goes; the month rows against the scan's totals.

**HRVT1 / HRVT2 and EST FTP from real full copies (hrvt.rs, storage/thresholds.rs, app/fitness.rs)**

Seen: simulator and fuzz.

On the unit: FITNESS after a few rides with the strap: the thresholds against the rider's own, the time the 96 KB a pass reading takes, and the 3 KB in AXI SRAM.

**HISTORY > FITNESS on the unit**

On the unit: how long READ RIDES takes over stock's rides (about 175 KB a ride on one unit's backups, so about 35 MB for 200 rides; the eMMC read speed and the parsing time per byte from SDRAM code are not measured: estimated 10 to 20 s at 256 KB a pass), the menu's response meanwhile, and IMPORT against the unit's own `\System` files (checked on the host against two backups); the first start over stock's volume (its import by itself, the eMMC identified at boot then).

**Workouts on the unit**

On the unit: WORKOUTS over the card's own `\Workout\calendar` (read on the host from one unit's backup: Bryton's files decode, names cut at 15 characters); a workout ridden through (the beeps' loudness and pitch, 2 kHz and 2.5 kHz, by ear; the banners); the time a list takes to read (16 workout files of 8 KB at most, about 50 ms, inferred from the scan's 2.5 MB/s).

**Next-step strip and two targets per step (app/workouts.rs `ahead`, ui/strip.rs `draw_next_strip`, workout_file.rs fields 19 to 22)**

Seen: simulator.

On the unit: the strip over the status strip for the last 9 s of a step and its countdown redrawn once a second without flicker, the status strip back at the step change, a banner over it, and a calendar workout from Bryton's app with a secondary target (whether its files carry fields 19 to 22 at all is not known).

**Zones (training/ride.rs `Zones`, app/settings.rs `zones`, training/hrtss.rs, features.md "Zones")**

Seen: simulator.

On the unit: ME > ZONES on the panel (11 rows, a bound edited and kept in settings.bin data 123..150); ZONES BY LTHR on a ride (bars, HR ZONE field, time in zone move with it); the TSS field and the end card showing hrTSS on a ride with a strap and no power meter.

**The heart rate and power fields, POWER SMOOTH, LTHR and the balance average (training/ride.rs ZoneTime, app/fields.rs, app/settings_file.rs)**

Seen: simulator.

On the unit: the zone numbers and times against the strap's zones on a real ride, the average balance against the meter's own figure, and LTHR and POWER SMOOTH kept through a switch-off and a switch to stock.

**The sun, heading, arrival and battery fields (nav/sun.rs, app/fields.rs)**

Seen: simulator.

On the unit: sunrise and sunset against the clock on a real fix (the zone learnt from the phone), the time to sunset, the heading on a ride, the ETA on a followed course, and the power meter's and the strap's battery status.

**The trip, lap and slope fields (training/ride.rs, app/fields.rs)**

Seen: simulator. The page has trip time, laps, last lap, lap power and NP, top cadence and altitude, descent, lap ascent and descent, uphill and downhill.

On the unit, on a real ride: the trip time through an auto-pause and a Start pause, each lap's power and NP against the power meter's own figures, and the uphill and downhill split on a real descent (the 30 s grade lags about 15 s at a change).

**QUICK STATUS (sim only)**

Seen: simulator. A tap on a plain field of a ride page opens six chosen fields.

On the unit: the tap on a wet or gloved finger (a stray tap only opens a read-only overlay, 10 s), the six values updating each second, the picker in PAGES > QUICK FIELDS, and the choice kept after a restart (settings.bin bytes 158 to 164).

**Start reminder and ready check (app/start_cues.rs, features.md "Start reminder and ready check")**

Seen: simulator.

On the unit: START THE RIDE? and its beep a few seconds into a real ride's first pedalling (and not again for 5 minutes); the ready card's two lines fitting the banner with real sensor names; the "+N" cut of a long list of missing names; Shifting or Di2 listed only when heard; the sensors it calls missing being the ones not yet heard at the press.

**Climbs and dates (app/settings.rs, ui/overview.rs, features.md "Climbs and dates")**

Seen: simulator.

On the unit: CLIMBS MEDIUM+ on a real road ahead (the climbs kept), CLIMB PAGE OFF with a climb due, and the history and workout rows in D/M/Y.

**Units, sound, auto pause, trips (app/fields.rs, app/settings.rs, features.md "Units, sound, trips")**

Seen: simulator.

On the unit: IMPERIAL on a real ride (the values and labels fit their boxes), SOUND OFF with an alert due, an AUTO PAUSE of 5 km/h at a stop, and a trip and the odometer growing after a save and surviving a restart.

**Page switches, names and AUTO SCROLL (app/pagemeta.rs, `pages.bin` after the MTB part)**

Seen: simulator.

On the unit: a page switched OFF skipped by Page and the swipes, MOVE UP / DOWN, a four character name in the list, all of them back after a restart (the new bytes at 489 to 511 of the sector, an older file read as all ON); AUTO SCROLL at 5 s on a ride; a swipe up from a ride page (after MORE) going to the overview.

**AUTO LAP and the lap banner (app/riding.rs `auto_lap`, `lap_summary`)**

Seen: simulator.

On the unit: a lap at each 5 km or 10 min on a real ride, its banner for 8 s, and the lap messages in the saved file on Intervals.icu.

**The lap list of a past ride (HISTORY > a ride > LAPS, tap the label or Page)**

Seen: simulator.

On the unit:
1. On a ride recorded here with Lap presses, the times and distances of its laps.
2. On a stock ride, whether its laps carry average power, heart rate and speed (fitread.rs reads `lap` fields 8, 9, 13 or 110, 15, 19 as the SDK profile has them; stock's layout is not checked on a real file).
3. The recorder's laps now carry their own avg / max heart rate (15, 16), cadence (17, 18) and power (19, 20) (fit.rs "Laps"): a ride here with a meter, a strap and Lap presses, its LAPS showing W and HR, the same numbers in Intervals.icu's laps, and a restart mid-lap (the lap's averages across it, from the recovery record's bytes 192..228).

**STORAGE and ABOUT (SYSTEM; storage/volume.rs, app/survey.rs)**

Seen: simulator.

On the unit: TOTAL, FREE and RIDES against the owner's card (FSInfo's count against the FAT's count there), the survey's passes and their time awake on the unit, and the oldest ride's card opened from STORAGE and its DELETE.

**ZONE and SUMMER TIME (DISPLAY, app/clock.rs)**

Seen: simulator.

On the unit: with ZONE +1 and SUMMER TIME EU:
1. The strip at UTC + 1 after the last Sunday of March's change (2027-03-28 01:00 UTC) and the next October one (2027-10-31), without the phone setting the RTC; the RTC itself unchanged (stock shows it as is).
2. A ride across the change keeping its name, its FIT times read in the Bryton app or Intervals.icu without a jump; the sun's times on the new hour.

**The sunset alert (ALERTS > SUNSET)**

Seen: simulator. On the unit: "SUNSET SOON" and its three beeps on a ride about 30 minutes before the set, once, and not again after a pause.

**The limit alerts and the distance, kcal and kJ reminders (ALERTS menu)**

Seen: simulator. On the unit: the buzzer and the red banner for a limit crossed on a ride.

**FONT = BARLOW (T3)**

Seen: simulator. DISPLAY > FONT cycles PIXEL and BARLOW, and the smooth text draws in the pixel cells.

On the unit: the labels and values on the ride page and the menus with BARLOW (no box moves, the strip and the fields redraw whole once), and a restart with BARLOW kept (the file read, then the register mirror).

**Themes (ui/theme.rs, app/themes.rs; T1)**

Seen: simulator.

On the unit:
1. OUTDOOR and LIGHT (T2) read in the sun: is either better than DARK on the transflective panel?
2. A theme file loaded from `\Rustfw\Themes` on the card (listed, picked, kept in settings.bin, applied after a restart).
3. The redraw at the first pass when the settings file names a file theme (one flash of DARK).

**THEME AUTO (ui/theme.rs `DayNight`, app/themes.rs; theming.md 4b)**

Seen: simulator.

On the unit:
1. The switch at dusk (night under 20 lux for 30 s, day back over 50 lux for 30 s, at most one switch a minute), the screen redrawn once in each theme.
2. The cost of the light sensor measuring all the time while AUTO is on (its time awake, measured on the unit with the light on AUTO and with THEME off).
3. The sensor with the screen white at 30 % backlight in the dark: the panel's own light reaching the sensor against the 20 and 50 lux thresholds (assumed: the sensor sees the room, not the panel).

**UI review 2, step A (sim only so far)**

On the unit:
1. The touch lock (Back and Lap held together 1 s: the padlock in the strip, a tap on the gear ignored, again to unlock); try it with a glove: the two keys are on opposite sides.
2. Back on the overview doing nothing.
3. The zone bars (the 8 px bar and its value margin with a real power meter and strap: legible in sunlight? the teal, green, olive, brown, wine, purple and slate zones by eye).
4. The readings once a second, the 48 px map buttons, the targets as "210" and "+-10", the confirmations "START: YES BACK: NO".

**The overview cog, the target gauge over all zones and the 8 px zone bars**

Seen: simulator. On the unit: the 44 px touch box with a glove, the gauge's zones by eye in sunlight, and the bars' height on the bike.

**New since the installed build (2026-10-24), seen only from RAM or in the simulator, one line each**
- touch gestures by feel and their axes;
- the overview and its notices;
- banners (ride start, pause, lap, end);
- the page layouts;
- graphs (five plots by tap, the whole ride by hold);
- derived power fields and W' balance on a ride with a power meter;
- DFA a1 and RMSSD from the owner's strap (needs a few minutes of real beats);
- the AUTO backlight on stock's curve, outdoors.

**Added 2026-10-27, one line each**
- the power-average tap during a ride;
- the CLIMB page appearing at a climb (course and climbs ahead);
- the map's 1 km default view following the rider, its redraw cost on the unit (`tools/dev.py prof`);
- the crosshair toggling the whole route;
- the cog opening settings during a ride.

**The flattened menus and the quick panel (f935c0f), and refactors B1 to B3**
Only booted and profiled. Seen 2026-11-01 (main 51ad015, `tools/walk.py --build out/IMG.bin --rides`, RAM run): 132 paths, 53 walked (the menus, the sub-struct screens and the four ride pages), 79 refused by the walk's own rules (62 imperial, 5 rows not on the unit, 4 KEYS, 4 rows that act, 2 INDOOR, 2 NEARBY), 0 title drift, 0 failed, no reset or fault, the ride idle at the end, 1.04 % awake idle on the overview, 1 489 s.

Still to see: the imperial pages (a human check).

### Seen on the unit

Fully seen; kept here for the numbers. The partly seen ones stay in their area above.
- **Every second of a stall is on the timer (2026-11-02, no cap; 10 s before):** the ride's seconds are counted from the RTC (`catch_up_seconds`); the last `REPEAT_S` (5) of a stall are ticked with the pass's readings, as before, the older ones go on the timer at once (as the ride was moving or auto-paused before it) with no records, the file's times jumping over them (Garmin's SDK and fitdecode read it; the session's elapsed and timer times include the stall), and a checkpoint follows at once. A step of the RTC forward by over an hour (`CLOCK_STEP_S`) or back counts one second; the phone's set (the only RTC setter, refused during a ride) moves the count with it; the GPS never sets the RTC. Seen in the simulator (`a_stall_of_10_min_is_all_on_the_timer`, the budget row "catch-up of a 10 min stall") and on the unit 2026-11-02 (`dev.py stall`: 60 s, 6 s, 4 s; the list below).
- **Seen on the unit 2026-11-02 (indoors, receiver searching, no fix):**
  - a long stall on a ride (`bench stall`): 60 s gave a jump of 56 s in the records, then 5 ticked; 6 s a jump of 2 s; 4 s none; the session's elapsed and timer totals were exact;
  - the power button wake with the radio back; the AUTO OFF banner, its beeps at 10 s and 5 s, and a key cancelling it;
  - the app clock's drift without the cable: 0.003 % over 20 min (0.033 % with the cable);
  - PA2 driven low (BOARD.md 4): the USB dev link dropped after a few seconds, nothing else seen (inferred: the USB HS PHY's (ULPI) enable or supply); the standby current: not planned (owner: needs the unit opened), System OFF confirmed by the chip's lines instead (BOARD.md 11);
  - the GNSS receiver after a stall: answering again at once after 15 s and 60 s stalls (label sampled every 0.5 s, three runs); a first look (four single screenshots, all in the blink's off half) had suggested a 2 min silence: not reproduced.
- **The app's clock against the RTC (2026-11-01, main.rs `WAITED_US`, BOARD.md 6):** the WFI waits of the work (panel DMA, `delay_ms`) now count in `up_ms` and the uptime (time-outs, cadences counted in ms). The silent check: read `tools/dev.py info` (uptime) and the RTC time, wait 10 min on a screen that redraws (the main page), read both again; the two differences must agree within a second. Seen 2026-11-02: drift 0.033 % with the cable, 0.003 % without, over 20 min.
- **A reset mid-ride and the resume (2026-10-24):** seen on the unit; a cut while a sector is written is covered by the host tests and the card's reliable write.
- **Switch-off reaches system Standby (2026-11-07, main.rs `standby`: PDDS_D3 set too, as stock; BOARD.md 9):** the
  crumb after a switch-off and a power-key start reads SBF set (RAM run of ec28cd4, then installed fbadb9b, FLASHLOG
  #92); the owner reported on 2026-11-09 that there was no more battery drain while switched off. Before the fix, the
  cell lost about 58 % in 88 h (docs/power.md "Switched off"). No measured figure for the drain yet.
- **The power button starts the unit after a switch-off:** fixed in ea359a0 (IRQ 149 enabled before standby), seen on the unit 2026-10-25, installed and unplugged.
- **The profiler (link/prof.rs, `tools/dev.py prof`, tools/profile.py):** seen on the unit 2026-10-29: section overhead 15 cycles; the rows sum to 1.508 % against 1.51 % awake. Idle overview 21.2 passes/s, 1.51 % awake; ride page 1 1.46 %; ride page 5 1.37 %; menu 2.69 %; ride discarded 4.98 %. Measured again in the evening on build 13fec4c after the review's repaint changes: idle overview 0.98 %, ride page 0.93 %, menu 1.37 % (21 passes/s with the cable in); README.md holds these.

## Known problems

- **A card that never answers keeps the unit on** (auto off held) until the battery runs out; the ride stays on the
  card. Owner to decide whether to allow auto off past `GIVE_UP` (storage/recorder/mod.rs).
- **The only lap of a ride without laps averages the auto-paused seconds too** (found 2026-10-31, not a fault of
  the dropped seconds as first thought): with no LAP pressed, the end's lap message takes the session's values
  (storage/fit.rs `Writer::end`: "the only lap is the ride"), whose heart rate, cadence and power average every
  record, while a lap of a ride with laps averages the moving records only (`this_lap`). A 62 s test ride showed
  138 bpm in its lap against 139 over its moving records. Owner to decide whether the single lap should be
  `this_lap` too (its sums survive a resume since `lap_words`).
- **Kept rides share `\Rustfw\Rides` with the ride in progress** (the review of 2026-10-31, storage 7): each
  checkpoint rewrites the ride's entry sector there (its size), which also names up to four rides kept unsaved from
  earlier days (`Outcome::Kept`). On a card without atomic sector writes, a cut tearing that write would lose their
  names (their chains then lost clusters). This unit's card has enhanced reliable write (docs/journal.md,
  2026-10-24), so a sector write is all or nothing; a card without it needs the kept rides in their own folder.
- **A damaged radio stream starves the GNSS reads past the 1.2 s budget** (simulator, src/sim/link_tests.rs
  `DAMAGED_GAP_MS`): up to 1.46 s over 5 seeds, 1.32 s in the GNSS walk of radio failures after 2026-11-08's timing
  shift (830 ms before, at the seed's whim), under the receiver's time-out and no epoch lost. A slow frame's put may
  run 2x its bound to finish the frame: cap it in the pass so the GNSS read is not starved.
- **COURSE page while riding:** the screenshot differs from the panel at row 137, x 4 (found 2026-10-27, the consistency check is skipped on that page in `climb_page_in_the_round`). The map test's course pixel count varied (83 to 281) between runs of the simulator, which should be deterministic: to look into.

## Next steps

1. **Stop mode** between passes: plan and wake sources in docs/power.md (Stop mode: plan); the 250 ms idle loop is
   merged and its clock drift seen on the unit (2026-11-02); the standby current is not planned (owner: needs the unit opened; System OFF confirmed by the chip's lines, BOARD.md 11).
2. ~~**USB high speed** for the drive: the drops after 15 to 40 MB (above); full speed works.~~ Dropped, not planned (owner, 2026-11-02).
3. **A settings file** to free the backup registers (docs/roadmap.md).
4. Everything under 'Not yet seen on the unit', on the unit, as the owner rides it.
5. D-cache on for the SDRAM (hw.rs PanelDma cache contract), the next drawing gain.
