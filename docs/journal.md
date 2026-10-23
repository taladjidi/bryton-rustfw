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

The first frame ever, with the earlier seven-segment digits: [img/first-frame-2026-10-23.png](img/first-frame-2026-10-23.png).
