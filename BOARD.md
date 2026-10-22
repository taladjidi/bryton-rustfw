# The Rider S510 board, as this firmware uses it

Hardware facts this firmware relies on, learned by reverse engineering the stock firmware (OS R039, bootloader
R002) for interoperability. Items marked **inferred** were deduced, not traced. Register names are from the
STM32H747 reference manual (RM0399). Sections are referred to from the code as "BOARD.md N".

## 1. MCU and clocks

- STM32H747, Cortex-M7 only (the M4 is never started).
- HSE 25 MHz (inferred from the PLL ranges). LSE 32.768 kHz, for the RTC.
- As the bootloader leaves it (and as the stock OS sets it): PLL1 from HSE, M 5, N 160, P 4, Q 8, R 8.
  - SYSCLK = PLL1P = **CPU 200 MHz**.
  - AHB /2 = **HCLK 100 MHz**. APB1 to APB4 /8 = 12.5 MHz each.
  - Regulator VOS3 in normal use; VOS1 is needed for USB.
- The bootloader uses the same PLL1 and bus setup as the OS, so code started by it runs at 200 MHz.

## 2. Boot hand-off

- The bootloader lives in internal flash (0x08000000). It is never modified by this project.
- It sets up the FMC SDRAM, the eMMC, the clocks, then copies **6 MB of the eMMC OS area to SDRAM 0xd0000000**,
  sets MSP from word 0 of the image and jumps to word 1. It does not check the image.
- Before the jump it **enables the MPU and both caches**:
  - SDRAM 64 MB at 0xd0000000 no-execute, except the first 4 MB (the image), which is executable and made
    read-only before the jump;
  - ITCM executable.
  - A firmware that wants other attributes adds its own region with a higher number (higher regions win).
  - Regions as the bootloader sets them (HAL_MPU_Disable, 8 regions, enable with PRIVDEFENA; Ghidra, IAP
    FUN_08007ddc): 3 = SDRAM 0xd0000000 64 MB execute-never, 4 = 0xd0000000 4 MB executable (read-only from
    the second call, just before the jump), 6 = SRAM4, 7 = ITCM 0x0 64 KB executable. D-cache and I-cache on.
  - **Seen on the unit (2026-10-22):** our firmware stops between reading MPU_TYPE and writing MPU_RNR /
    MPU_RBAR for region DREGION - 1 (diag v2, README "First run on the unit"). Why is not known yet; v3 (no
    MPU write) is pending.
  - Switching the MPU off is not an option: the default memory map makes 0xc0000000..0xdfffffff execute-never.
- No watchdog is started, by the bootloader or the stock OS.
- **Recovery mode ("Engineer Mode")**: USB plugged, restart with the four buttons while holding Page. The
  bootloader configures the Page pin itself and reads the real key, so no firmware can enter it by software.
  Recovery tools: github.com/taladjidi/bryton-S510-mods (`tools/iaprecover.py`, `docs/recovery.md`).

## 3. Memory

| region | address | size | notes |
|---|---|---|---|
| ITCM | 0x00000000 | 64 KB | executable; this firmware puts its RAM-run trampoline at 0x1000 |
| DTCM | 0x20000000 | 128 KB | data, bss and stack here |
| AXI SRAM | 0x24000000 | 512 KB | |
| SRAM1 to 3 | 0x30000000 | 288 KB | |
| SRAM4 | 0x38000000 | 64 KB | survives a reset, not standby |
| SDRAM (FMC bank 2) | 0xd0000000 | 64 MB | 16-bit mobile SDR SDRAM, 50 MHz; no remap |

- The image runs in place from SDRAM 0xd0000000, code and constants included.
- This firmware's image is 1 MB at most (`link.x`). SDRAM from 0xd0800000 is its staging area for RAM-run.

## 4. Pins

| pin | use |
|---|---|
| PA0 | power button, standby wake-up WKUP1 (rising) |
| PA1 | key 0x01 back / power, active low |
| PA3 PA5 | USB ULPI D0, CK (AF10) |
| PA7 | USB ULPI PHY enable / reset, high = running (inferred) |
| PB0 PB1 PB5 PB10..PB13 | USB ULPI D1 D2 D7 D3..D6 (AF10) |
| PB2 | key 0x02 lap, active low |
| PB4 | key 0x04 start / stop, active low |
| PC0 | USB ULPI STP (AF10) |
| PC13 | USB / charger present, 1 = plugged (also standby wake-up WKUP4) |
| PD7 | SPI1 MOSI (panel), AF5 |
| PE3 | key 0x08 page, active low |
| PG3 | display and touch power enable (inferred) |
| PG9 | panel D/C: 0 command, 1 parameter or pixel data |
| PG10 | SPI1 NSS (panel chip select) |
| PG11 | SPI1 SCK (panel), AF5 |
| PG13 | backlight (LPTIM1_OUT, AF1, in stock) |
| PH4 | USB ULPI NXT (AF10) |
| PI3 | panel reset release (inferred) |
| PI11 | USB ULPI DIR (AF10) |

The full board has more (GNSS on SPI4, the Nordic radio on USART2, I2C sensors, touch, eMMC on SDMMC1, buzzer,
battery on ADC3): not used yet.

## 5. Keys

- Four keys, active low: PA1 back / power, PB2 lap, PB4 start / stop, PE3 page.
- Stock reads them by EXTI falling edge and debounces 15 ms later. This firmware polls them every 50 ms.

## 6. Display

- 240 x 320 panel, **ST7789-class controller** (inferred from the command set), RGB565.
- Commands over **SPI1** (0x40013000): 8 bit, transmit only, CPOL 1, CPHA 1 (mode 3), MSB first, D/C on PG9.
- Stock pushes pixels over the LTDC RGB interface (framebuffer in AXI SRAM). This firmware does not use the LTDC.
- Power-up: PG3 high, PI3 high, 10 ms, then the stock init sequence (commands, parameters, waits in ms):

      01 ; 10 ms ; 13 ; 21
      36 00 ; 3A 05 ; 10 ms ; B7 05 ; 10 ms ; BB 29 ; 10 ms ; C0 2C ; 10 ms
      C2 01 ; C3 1D ; C4 20 ; C6 0F ; D0 A4 A1
      E0 F0 1F 2B 16 17 0F 4C 44 5B 39 16 15 27 28
      E1 F0 13 1E 09 0A 15 4C 43 5B 0F 1D 1D 28 29
      B0 00 F0 ; 11 ; 120 ms
      2A 00 00 00 EF ; 2B 00 00 01 3F ; B1 41 04 0A ; B2 0C 0C 00 33 33 ; B0 10 F0

  Display on (29) comes later. The final RAMCTRL `B0 10 F0` makes the panel take pixels from the RGB interface.
- This firmware (`src/panel.rs`) sends the same sequence without that final RAMCTRL, so `B0 00 F0` stays in force
  and pixels are written over SPI (RAMWR). It waits 120 ms after the software reset, sets the window before
  each write, and switches the display on after clearing the screen.
- Backlight: stock drives PG13 with an LPTIM1 PWM on the LSE, output polarity inverted, 1..100 levels. This
  firmware drives PG13 as a plain output, high = on (derived from the inverted PWM; to confirm on the unit).

## 7. USB

- **USB1 OTG_HS (0x40040000) with an external ULPI PHY**, high speed. Pins in section 4, all AF10.
- PHY enable PA7: high, then 200 ms before the core is used.
- Clocks: OTG HS and OTG HS ULPI in RCC_AHB1ENR. Kernel clock 48 MHz (stock: PLL3Q; this firmware: HSI48).
- Needs VOS1 (PWR_D3CR VOS = 3).
- VBUS sensing is off in the core: VBUS is read on **PC13**, not on the OTG VBUS pin.
- Interrupt: OTG_HS is IRQ 77.
- Stock uses it for mass storage only, and a plug-in restarts the unit into that mode (RTC backup register 4).

## 8. RTC

- RTC (0x58004000) on the LSE, 24 h format, prescalers 31 / 1023 (1 Hz). It keeps running in standby.
- The stock firmware sets it from GPS. Whether it holds UTC or local time is not traced.
- Backup registers used by stock: 1, 2, 3, 19 (clock), 4 (USB mode), 9, 12, 13. Others are free.

## 9. Standby (power off)

- Wake-up pins **WKUP1 (PA0, power button)** and **WKUP4 (PC13, USB)**, rising, no pull.
- Entry as stock: PWR_CPUCR PDDS_D1 and PDDS_D2 set, SLEEPDEEP, WFI. Waking resets the unit.
- Stock waits for the back key to be released first (otherwise it wakes at once).

## 10. The update file

The stock OS installs `IMG.bin` from the root of its USB drive at start-up, with `update.ini` beside it.
- An `IMG.bin` with `R` and 3 digits at offset 0x94 is taken as a **raw image** (not gzip).
- It must be larger than 1 MB, smaller than 10 MB, and end with a 4-byte trailer: the additive sum of all bytes before it, little
  endian. The OS writes it to the eMMC OS area and reads it back to check that sum.
- The bootloader then copies it to 0xd0000000 at the next start (section 2).
- `pack.py` builds this file and re-checks all of the above.
- **Seen on the unit (2026-10-22):** a raw image is written from offset 0 of the OS area, trailer included, and
  the rest of the area keeps the previous image (Engineer Mode readback after installing this firmware).
- In Engineer Mode the bootloader writes the OS area 2 KB at a time (`tools/iaprecover.py`); its write status is
  not trustworthy, so every chunk is read back.
