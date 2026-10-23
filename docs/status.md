# Status

## Seen on the unit

Everything in [features.md](features.md) runs on the unit (2026-10-23), installed or from RAM, unless listed below.
The dated findings are in [journal.md](journal.md).

## Not yet seen on the unit

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
  pitch, 2 kHz and 2.5 kHz, by ear; the banners), and the time a list takes to read (one 8 KB read a workout).
- **Courses on the unit:** COURSES over the card's own `\PlanTrip` (the 14 routes of one unit's backup read on
  the host: every one decodes, lengths within 0.25 % of the files' own, climbs close to the ones Bryton's app marked),
  the time the list takes (each file read whole for its length: about 550 KB for those 14), a course ridden (the
  window keeping up at speed, the off-course alert in a town's parallel streets, the climb banners).
- **The I-cache from a cold boot** (seen from RAM).
- **The Nordic chip after a power off and on** (`sd_power_system_off`, then the reset pulse).
- **Charging control:** the charger is on whenever USB is in (as the bootloader leaves it), with no temperature
  window yet (BOARD.md 12; the barometer's temperature can now feed it). The 3.3 V switch-off not reached.
- **The OTG_HS IRQ waking WFI** (the link may run on the 50 ms SysTick wake-ups alone).
- **The touch panel with a finger** (the chip itself answered: BOARD.md 17): INT low while touched, the axes
  (x 0 left, y 0 top expected), the gestures (swipe left / right between pages, hold for the menu, a tap on a menu
  row), the backlight waking on a touch, and the swipe thresholds by feel (stock's: 37 px across, 48 px up).
- **eMMC writes and the ride recorder** (simulator only, 2026-10-24: `src/drivers/emmc.rs` writes, the FAT writer
  `src/storage/fat/write.rs`, `src/storage/recorder.rs`): CMD24 / CMD25 through the SDMMC1 FIFO (main.rs
  `emmc_write`, never run), the folders `\Rustfw` and the ride in `\Activities` as stock and a computer see them, a
  reset mid-ride then the resume, a power cut mid-ride, the save. The plan with the owner is in docs/journal.md
  (2026-10-24, "The recorder: what to check on the unit").
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
