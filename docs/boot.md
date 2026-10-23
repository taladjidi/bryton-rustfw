# How it starts

From the bootloader's hand-off to the first screen (details and evidence: BOARD.md 2 and 3).

1. **The bootloader** (internal flash, never touched by this project) copies 6 MB of the eMMC's OS area to SDRAM
   at 0xd0000000, sets MSP from word 0 of the image and jumps to word 1. It never checks the image. It sets the
   MPU up but turns both caches off just before the jump. No watchdog runs.
2. **The reset handler** (`src/hw.rs`) copies the data and zeroes the bss in DTCM, where data and stack live; code
   and constants run from SDRAM, as the stock OS does at first.
3. **MPU and I-cache:** one MPU region over the SDRAM (normal, write-through cacheable, executable, highest
   number so it wins), then the I-cache on. The D-cache stays off. The MPU is never switched off: under the
   default map 0xc0000000..0xdfffffff is execute-never.
4. **Clocks are the bootloader's** (CPU 200 MHz); each peripheral takes its kernel clock from the HSI, so nothing
   depends on which PLL outputs were left on. Peripherals are reset through RCC before use, since a RAM run
   (`dev.py run`) inherits their state.
5. **The app** (`App::start`): the panel's init sequence, the power-on beep, the Nordic chip (reset, BLE, ANT and
   the paired sensors), the settings from the RTC backup registers, the main screen. The GPS receiver is powered
   one second later, then given its configuration, the time and the last position.
6. **The loop:** every 50 ms it reads the RTC, the keys, the radio chip and the GPS, redraws what changed, and
   sleeps (SysTick + WFI) until the next pass; with a cable in, the USB dev link is served while it sleeps.
