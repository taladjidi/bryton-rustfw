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

## Not yet seen on the unit

- **The phone link (BLE, app/phone.rs)**, in this order, the dev link up (`dev.py run`, then the screenshots), an
  iPhone at hand. A build run from RAM without EMMC_WRITE=1 keeps the bond only until a reset: after one, forget
  "S510" on the iPhone before pairing again (the iPhone keeps a key the unit no longer has).
  1. **Set-up:** MENU > PHONE: LINK reads OFF (the chip took every GATT set-up command: services, characteristics,
     the advertising data, the 8 vendor UUIDs). ERR xx/yyyy instead names the command refused and its error (for
     example A2 characteristic add with 0004 NRF_ERROR_NO_MEM: the attribute table is too small without
     `sd_ble_cfg_set`, inferred). The heart-rate strap still reads (ANT+ unchanged).
  2. **Advertising:** PAIR: LINK PAIRING; the iPhone's Settings > Bluetooth lists "S510" under Other Devices within a
     few seconds (the advertising data: HID and battery services, ANCS solicitation; the name in the scan
     response). The NEARBY BLE scan of a second unit, or nRF Connect, sees the same.
  3. **Pairing:** tap S510 on the iPhone: "Bluetooth Pairing Request ... S510 would like to pair with your iPhone",
     Pair (Just Works, no code); then iOS asks whether S510 may receive the iPhone's notifications: Allow. The unit:
     banner PHONE PAIRED, LINK SECURE, the clock set to the iPhone's time (and the date), Settings > Bluetooth shows
     S510 connected. Note the order the events came in if pairing fails (the security request at connection: is
     the iPhone's prompt shown without opening a HID characteristic?).
  4. **Media:** start music on the iPhone; PHONE > REMOTE shows its title, artist and PLAYING with the time running;
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
  10. **Cost:** `dev.py info` awake time with LINK OFF, WAITING and SECURE (the Nordic chip's own current is not
      measurable here; estimated 10 to 15 uA advertising once a second).
  11. **The RTC set** (`Board::rtc_set`, RM0399 init mode): the time right after the phone set it, and kept through
      a power off; stock afterwards still shows the right time.

- **IGN maps** (`\Rustfw\Maps`): the packs found, a view drawn from them, its time (simulated: one index sector and
  about 45 sectors of tile data per tile, at most 6 tiles, so about 150 KB a view change, as stock's basemap; the
  decoding is 87 us a tile on the computer, about 3 to 6 ms on the unit with the D-cache off, inferred), the
  "(c) IGN" corner, the 2.1 MB map memory zeroed at boot.
- **Sensors other than the heart-rate strap:** speed, cadence, power (the decoders follow the ANT+ profiles,
  checked against openant).
- **The automatic backlight by eye:** stock's curve (8 % in the dark, the 1 % minimum above 120 lux): is the panel
  readable in full sun at 1 % (compare LIGHT % 100 and 10 outdoors)?
- **Backlight levels** (60 / 30 / 10 %) and the time-out, each by eye.
- **The map on a real ride:** its awake time, and the last-known-position dot after a power cycle.
- **The basemap** (roads and water from stock's cached tiles, `\Map\Data\Temp`): the FAT volume mounted by our
  reader, the tiles at the last known position, the time a view change takes (simulated with real tiles: about
  140 KB read, 84 K buffer pixels, 73 K panel pixels), the 1.5 MB SDRAM zeroing at boot (`hw::map_memory`).
- **STOCK DATA on the unit:** how long READ RIDES takes over stock's rides (about 175 KB a ride on one unit's
  backups, so about 35 MB for 200 rides; the eMMC read speed and the parsing time per byte from SDRAM code are
  not measured: estimated 10 to 20 s at 256 KB a pass), the menu's response meanwhile, and IMPORT against the
  unit's own `\System` files (checked on the host against two backups).
- **Workouts on the unit:** WORKOUTS over the card's own `\Workout\calendar` (read on the host from one unit's
  backup: Bryton's files decode, names cut at 15 characters), a workout ridden through (the beeps' loudness and
  pitch, 2 kHz and 2.5 kHz, by ear; the banners), and the time a list takes to read (16 workout files of 8 KB at
  most, about 50 ms, inferred from the scan's 2.5 MB/s).
- **Courses on the unit:** COURSES over the card's own `\PlanTrip` (the 14 routes of one unit's backup read on
  the host: every one decodes, lengths within 0.25 % of the files' own, climbs close to the ones Bryton's app marked),
  the time the lengths take to come (each file read whole, 96 KB a pass: about 550 KB for those 14, some 6
  passes), a course ridden (the window keeping up at speed, the off-course alert in a town's parallel streets, the
  climb banners, a lap course rejoined after a detour).
- **The I-cache from a cold boot** (seen from RAM).
- **The Nordic chip after a power off and on** (`sd_power_system_off`, then the reset pulse).
- **Charging control:** the charger is on whenever USB is in (as the bootloader leaves it), with no temperature
  window yet (BOARD.md 12; the barometer's temperature can now feed it). The 3.3 V switch-off not reached.
- **The USB drive (mass storage, msc.rs)**, in this order, reading only first (a build without EMMC_WRITE=1, so
  the drive is write protected): `dev.py info` still answers with the composite device (the dev link beside the
  drive); with nothing lent, macOS shows no disk; DEVICE > USB DRIVE (or `dev.py drive`): the BRYTON volume mounts
  read only, its files equal `dev.py get`'s, a large file copies off at several MB/s (time it: the target is the
  eMMC's 10 MB/s, less the USB packet loop); Eject in Finder: USB DRIVE goes OFF with no restart. Then, with the
  owner and an EMMC_WRITE=1 build run from RAM: copy a small file on, eject, the unit restarts, `dev.py ls` sees it,
  stock still boots and lists the volume. Also: Windows and Linux (TEST UNIT READY polling, which the medium
  change relies on, is inferred for every host).
- **The OTG_HS IRQ waking WFI** (the link may run on the 50 ms SysTick wake-ups alone).
- **The touch panel with a finger** (the chip itself answered: BOARD.md 17): INT low while touched, the axes
  (x 0 left, y 0 top expected), the gestures (swipe left / right between pages, hold for the menu, a tap on a menu
  row), the backlight waking on a touch, and the swipe thresholds by feel (stock's: 37 px across, 48 px up).
- **A power cut mid-ride** (the battery connector or a hard power off): the reset mid-ride and the resume are seen
  (2026-10-24, below); a cut while a sector is written is covered by the host tests and the card's reliable write.
- **Stock reading this firmware's rides** (\Activities\261024083403.fit, 261024083931.fit) and the Bryton app's sync.
- **"Rebooting with just the power button does not work"** (the owner, after the GPS test): to clarify and fix.

- **New since the installed build, seen only from RAM or in the simulator (2026-10-24):** touch gestures by feel and
  their axes; the overview and its notices; banners (ride start, pause, lap, end); eleven page layouts; graphs (five
  plots by tap, the whole ride by hold); derived power fields and W' balance on a ride with a power meter; DFA a1 and
  RMSSD from the owner's strap (needs a few minutes of real beats); the AUTO backlight on stock's curve, outdoors.
- **Pages moved off backup register 19** (stock reads it at boot): the first start of a new build moves them; then
  stock, reinstalled, should keep its last battery voltage as before.

## Next steps

1. **Stop mode** between seconds (woken by the RTC, the keys, the radio chip's UART, USB): the largest battery
   gain left. A core change, tried with the owner present.
2. **Writes to the eMMC and saving rides** on the unit, with the owner (written and tested in the simulator:
   docs/journal.md 2026-10-24).
3. **A basemap** under the map, from stock's tile cache, rendered only when the view changes (in progress).
4. BLE sensors (stock's connect / discovery / notification encodings are traced, BOARD.md 11).
5. USB mass storage, so files and updates work from inside the firmware.
