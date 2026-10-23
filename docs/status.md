# Status

## Seen on the unit

Everything in [features.md](features.md) runs on the unit (2026-10-23), installed or from RAM, unless listed below.
The dated findings are in [journal.md](journal.md).

## Not yet seen on the unit

- **Sensors other than the heart-rate strap:** speed, cadence, power (the decoders follow the ANT+ profiles,
  checked against openant).
- **The automatic backlight by eye** with its new curve: dark, indoors, daylight (the light sensor reads; the
  stock curve dimmed in bright light, so the curve was changed).
- **Backlight levels** (60 / 30 / 10 %) and the time-out, each by eye.
- **The map on a real ride:** its awake time, and the last-known-position dot after a power cycle.
- **The I-cache from a cold boot** (seen from RAM).
- **The Nordic chip after a power off and on** (`sd_power_system_off`, then the reset pulse).
- **Charging control:** the charger is on whenever USB is in (as the bootloader leaves it), with no temperature
  window yet (BOARD.md 12; the barometer's temperature can now feed it). The 3.3 V switch-off not reached.
- **The OTG_HS IRQ waking WFI** (the link may run on the 50 ms SysTick wake-ups alone).
- **"Rebooting with just the power button does not work"** (the owner, after the GPS test): to clarify and fix.

## Next steps

1. **Stop mode** between seconds (woken by the RTC, the keys, the radio chip's UART, USB): the largest battery
   gain left. A core change, tried with the owner present.
2. **Writes to the eMMC** (FAT32 files), then **saving rides** as FIT files with checkpoints and resume (the
   golden rule, AGENTS.md).
3. **A basemap** under the map, from stock's tile cache, rendered only when the view changes (in progress).
4. BLE sensors (stock's connect / discovery / notification encodings are traced, BOARD.md 11).
5. USB mass storage, so files and updates work from inside the firmware.
