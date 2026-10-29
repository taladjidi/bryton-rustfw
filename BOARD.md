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
- **While the core sleeps** (WFI = CSleep: the CPU clock stops, a peripheral keeps its clocks while its LPEN bit
  is set; RM0399 RCC). Sleep at VOS3, 200 MHz, LDO: 16.5 mA with every peripheral off, 47.1 mA with all on
  (DS12930 table 44). This firmware (main.rs `sleep_power_init`, `POWER_STEPS`) clears every RCC_xxxLPENR bit but
  FMC, SDMMC1, the TCMs, AXI SRAM, D2 SRAMs, SRAM4, BKPRAM, USB1 OTG HS + ULPI, LPTIM1, USART2, TIM1, SYSCFG, RTC
  (the flash interface and the GPIO ports included), and stops PLL2 (unless FMCSEL or SDMMCSEL is on it) and PLL3:
  nothing it uses runs from them. **Inferred, not yet seen on the unit.** Which PLLs and peripherals the
  bootloader leaves on: to read (`tools/dev.py peek 0x58024400` RCC_CR, `0x580244d4 9` RCC_AHB3ENR..APB4ENR,
  `0x580244fc 9` the LPENR registers).

## 2. Boot hand-off

- The bootloader lives in internal flash (0x08000000). It is never modified by this project.
- It sets up the FMC SDRAM, the eMMC, the clocks, then copies **6 MB of the eMMC OS area to SDRAM 0xd0000000**,
  sets MSP from word 0 of the image and jumps to word 1. It does not check the image.
- At start it enables the MPU and both caches, but **just before the jump it turns both caches off again**
  (I-cache off and invalidated, D-cache cleaned, invalidated and off; IAP 0x0800828a) and stops SysTick. So the
  firmware starts with the caches off and every instruction is fetched uncached from SDRAM: much slower than the
  cycle counts suggest, until the firmware turns the I-cache on itself. MPU regions:
  - SDRAM 64 MB at 0xd0000000 no-execute, except the first 4 MB (the image), which is executable and made
    read-only before the jump;
  - ITCM executable.
  - A firmware that wants other attributes adds its own region with a higher number (higher regions win).
  - Regions as the bootloader sets them (HAL_MPU_Disable, 8 regions, enable with PRIVDEFENA; Ghidra, IAP
    FUN_08007ddc): 3 = SDRAM 0xd0000000 64 MB execute-never, 4 = 0xd0000000 4 MB executable (read-only from
    the second call, just before the jump), 5 = 0xd0400000 2 MB (same attributes as 4), 6 = SRAM4, 7 = ITCM 0x0
    64 KB executable. Regions 3 to 5 are write-through cacheable (C = 1, B = 0, TEX 0), not shareable.
  - An earlier reading, "our firmware stops in its first MPU writes" (diag v1 / v2), relied on the backlight as
    a readout, which later proved not to work (section 6). Unproven.
  - **Seen on the unit (2026-10-22, diag v9):** an image that turns the I-cache on, reads PC13 and requests a
    system reset (AIRCR) runs as expected: the reset brings the bootloader up again (docs/journal.md, 2026-10-22).
  - **This firmware turns the I-cache on** (MPU region write-through cacheable, ICIALLU, CCR.IC; D-cache off):
    seen stable on the unit from RAM (2026-10-23).
  - Switching the MPU off is not an option: the default memory map makes 0xc0000000..0xdfffffff execute-never.
  - **Unaligned reads from the SDRAM return wrong bytes** (seen on the unit, 2026-10-23, this firmware's region:
    normal, not cached). A 7-byte constant at 0xd000a0c3 copied with unaligned loads came out as `03 01 03 00`
    for `02 01 03 04`: bytes from the neighbouring lanes. Aligned word reads of the same memory (`tools/dev.py peek`)
    are right. This firmware builds with `+strict-align` (.cargo/config.toml), so it never emits unaligned
    accesses; precompiled `core` code could still, and the D-cache (line fills are aligned bursts) would be the
    complete fix. Inferred cause: the FMC's handling of unaligned accesses to a 16-bit SDRAM, not traced. Stock
    runs with the caches on, so it would not see this.
- No watchdog is started, by the bootloader or the stock OS.
- **Recovery mode ("Engineer Mode")**: USB plugged, restart with the four buttons while holding Page. The
  bootloader configures the Page pin itself and reads the real key, so no firmware can enter it by software.
  Recovery tools: github.com/taladjidi/bryton-S510-mods (`tools/iaprecover.py`, `docs/recovery.md`).
  - The bootloader can't be read back over USB: the read path only hands sub-commands 0x03, 0x04, 0x05, 0x0d,
    0x10, 0x13, 0x14 and 0x15 to the handler, so its "read internal flash" case (0x0b) is unreachable (a stale
    buffer comes back). Sub 0x04 returns the byte sum of its 128 KB; `iaprecover.py --boot-sum` compares it
    with R039 `iap.bin`. **Seen on the unit (2026-10-22): MATCH (0x00548f3c)**, so the disassembly is of the
    bootloader the unit runs.

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
- **The stacks (this firmware, link.x):** DTCM holds .data, .bss, then a 256 B stack guard, the thread stack (PSP)
  and at the top a 1 KB handler stack (MSP, word 0 of the image). The facts it rests on (ARMv7-M Architecture
  Reference Manual, DDI 0403E; the STM32H7's Cortex-M7 MPU has 16 regions, RM0399 / PM0253):
  - CONTROL (B1.4.4): SPSEL (bit 1) = 1 puts thread mode on PSP; handler mode always uses MSP. Written by MSR in
    privileged thread mode, followed by an ISB. hw.rs `reset` does it before any Rust code runs; the RAM-run
    trampoline writes CONTROL 0 back before starting an image, as the bootloader would.
  - MPU_RBAR / MPU_RASR (B3.5.8, B3.5.9): a region's base is aligned to its size, SIZE n means 2^(n+1) bytes (7:
    256 B), AP 000 is no access for privileged and unprivileged code, XN (bit 28) execute-never. With PRIVDEFENA, a
    higher-numbered region wins where regions overlap (B3.5.3). The guard is region DREGION - 2, under the SDRAM one.
  - SHCSR.MEMFAULTENA (B3.2.13, bit 16) makes an MPU violation a MemManage fault instead of a HardFault. A fault
    while stacking an exception frame (MMFSR.MSTKERR, B3.2.15) is taken all the same, on the handler's stack: MSP.
  - A fault the core cannot take (one in HardFault, or stacking that faults on the stack the handler itself uses)
    is lockup (B1.5.15): no watchdog runs (section 2), so the unit would freeze. That is why faults get their own
    stack.
  - **Not yet seen on the unit** (docs/status.md): Renode does not model the MPU.

## 4. Pins

Every pin this firmware uses, from the MCU's side (arrows: signal direction). Sections 5 to 17 have the details.

                                      +----------------------------+
     power button --+---------------->| PA0  WKUP1 (rising)        |
                    +--(low pressed)->| PA1  key Back              |
     key Lap ---------(low pressed)-->| PB2                        |
     key Start -------(low pressed)-->| PB4  NJTRST after reset    |
     key Page --------(low pressed)-->| PE3                        |
                                      |                            |
     USB VBUS (1 = plugged) --------->| PC13 WKUP4                 |
     battery --[ / 1.5 ]------------->| PC3_C  ADC3 channel 1      |
     charger enable (low = on) <------| PG14                       |
     charger status ----------------->| PF6  (meaning not traced)  |
                                      |                            |
     USB ULPI PHY (AF10)              |                            |
       enable  <----------------------| PA7   high = running       |
       CK      ---------------------->| PA5                        |
       D0..D7  <--------------------->| PA3 PB0 PB1 PB10..PB13 PB5 |
       STP     <----------------------| PC0                        |
       NXT     ---------------------->| PH4                        |
       DIR     ---------------------->| PI11                       |
                                      |                            |
                                      |         STM32H747          |
                                      |                            |
       ST7789 panel 240 x 320         |                            |
         SCK   <----------------------| PG11  SPI1 (AF5)           |
         MOSI  <----------------------| PD7   SPI1 (AF5)           |
         CS    <----------------------| PG10                       |
         D/C   <----------------------| PG9   0 command, 1 data    |
         RESET <----------------------| PI3   low = reset          |
         power <----------------------| PG3   (inferred)           |
         backlight <------------------| PG13  high = on            |
                                      |                            |
       Nordic BLE / ANT chip          |                            |
         UART RX  <-------------------| PD5   USART2 TX (AF7)      |
         UART TX  ------------------->| PD6   USART2 RX (AF7)      |
         UART CTS <-------------------| PD4   USART2 RTS (AF7)     |
         UART RTS ------------------->| PD3   USART2 CTS (AF7)     |
         reset    <-------------------| PA6   low = reset          |
         boot mode <------------------| PI4   high = application   |
         request  ------------------->| PH8   (EXTI only, unused)  |
                                      |                            |
       sensor bus (I2C4, AF4,         |                            |
         open drain, no pull)         |                            |
         SCL   <----------------------| PD12                       |
         SDA   <--------------------->| PD13                       |
         light sensor INT ----------->| PF9   (unused here)        |
         accelerometer INT1 --------->| PD11  (unused here)        |
                                      |                            |
       u-blox M10 GNSS                |                            |
         SCK   <----------------------| PE2   SPI4 (AF5), pull-down|
         MOSI  <----------------------| PE6   SPI4 (AF5)           |
         MISO  ---------------------->| PE5   SPI4 (AF5)           |
         CS    <----------------------| PE4                        |
         power <----------------------| PI6   high = on            |
         line  ---------------------->| PF7   (unused here)        |
                                      |                            |
       buzzer (TIM1, AF1)             |                            |
         PWM     <--------------------| PA10  TIM1 CH3 (AF1)       |
         amp enable <-----------------| PH2   high = on            |
                                      |                            |
       touch (I2C1, AF4,              |                            |
         open drain, no pull)         |                            |
         SCL   <----------------------| PB6                        |
         SDA   <--------------------->| PB7                        |
         touch INT ------------------>| PC6   low = a point        |
         reset  <---------------------| PH3   high = run           |
                                      |                            |
       eMMC (SDMMC1, AF12)            |                            |
         CLK   <----------------------| PC12                       |
         CMD   <--------------------->| PD2                        |
         D0..D3 <-------------------->| PC8 PC9 PC10 PC11          |
         RST_n or power <-------------| PF8   (inferred; untouched)|
                                      +----------------------------+

| pin | use | section |
|---|---|---|
| PA0 | power button, standby wake-up WKUP1 (rising); must be a GPIO input | 9 |
| PA1 | key 0x01 back / power, active low (also low while the power button is pressed) | 5 |
| PA3 PA5 | USB ULPI D0, CK (AF10) | 7 |
| PA6 | Nordic chip reset, low = reset | 11 |
| PA7 | USB ULPI PHY enable / reset, high = running (inferred) | 7 |
| PA10 | buzzer: TIM1 channel 3 PWM (AF1); a plain low output between tones | 14 |
| PB0 PB1 PB5 PB10..PB13 | USB ULPI D1 D2 D7 D3..D6 (AF10) | 7 |
| PB2 | key 0x02 lap, active low | 5 |
| PB4 | key 0x04 start / stop, active low; NJTRST (AF, pull-up) after reset | 5 |
| PB6 PB7 | I2C1 SCL, SDA: the touch controller (AF4, open drain, no pull) | 17 |
| PC0 | USB ULPI STP (AF10) | 7 |
| PC3_C | battery through a divider by 1.5, ADC3 channel 1 (analog switch PC3SO) | 12 |
| PC8..PC11 | eMMC D0..D3, SDMMC1 (AF12) | 15 |
| PC12 | eMMC CLK, SDMMC1 (AF12) | 15 |
| PC6 | touch interrupt, low while the chip has a point (EXTI falling in stock; EXTI6 falling edge wakes the idle sleep, section 17) | 17 |
| PC13 | USB / charger present, 1 = plugged (also standby wake-up WKUP4) | 7 |
| PD2 | eMMC CMD, SDMMC1 (AF12) | 15 |
| PD3 PD4 PD5 PD6 | USART2 CTS, RTS, TX, RX to the Nordic chip (AF7, pull-up) | 11 |
| PD7 | SPI1 MOSI (panel), AF5 | 6 |
| PD11 | accelerometer INT1 (EXTI rising in stock; unused here) | 16 |
| PD12 PD13 | I2C4 SCL, SDA: the sensor bus (AF4, open drain, no pull) | 16 |
| PE2 PE4 PE5 PE6 | SPI4 SCK (pull-down), chip select, MISO, MOSI to the GNSS (AF5; PE4 by hand here) | 13 |
| PE3 | key 0x08 page, active low | 5 |
| PF6 | charger status input (not traced) | 12 |
| PF7 | GNSS line (EXTI in stock; unused here) | 13 |
| PF8 | eMMC reset or power (inferred); left as the bootloader leaves it | 15 |
| PF9 | ambient light sensor interrupt (EXTI falling, pull-up in stock; unused here) | 16 |
| PG3 | display and touch power enable (inferred) | 6, 17 |
| PG9 | panel D/C: 0 command, 1 parameter or pixel data | 6 |
| PG10 | panel chip select (SPI1 NSS, driven by hand) | 6 |
| PG11 | SPI1 SCK (panel), AF5 | 6 |
| PG13 | backlight, high = on (LPTIM1_OUT, AF1, in stock) | 6 |
| PG14 | charge enable, active low (left low by the bootloader) | 12 |
| PH2 | buzzer amplifier enable, high = on | 14 |
| PH3 | touch controller reset, high = run (the chip answers once it is high: seen on the unit) | 17 |
| PH4 | USB ULPI NXT (AF10) | 7 |
| PH8 | Nordic request line (EXTI wake-up in stock; unused here) | 11 |
| PI3 | panel reset, low = reset (inferred) | 6 |
| PI4 | Nordic boot mode: high = application | 11 |
| PI6 | GNSS power, high = on | 13 |
| PI11 | USB ULPI DIR (AF10) | 7 |


## 5. Keys

- Four keys, active low: PA1 back / power, PB2 lap, PB4 start / stop, PE3 page.
- PB4 leaves reset as NJTRST (alternate function, pull-up): it must be made an input (seen on the unit: MODER
  field 2, PUPDR pull-up, IDR high at rest). PA1, PB2 and PE3 read high at rest without a pull-up (external
  ones, inferred).
- Stock reads them by EXTI falling edge and debounces 15 ms later. This firmware polls them every pass (50 ms), and
  while idle (passes of 250 ms, lib `App::pass_ms`) their falling edges end the sleep at once: EXTI lines 1 to 4 (PA1,
  PB2, PE3, PB4; SYSCFG EXTICR1 / EXTICR2), IRQs 7 to 10 enabled in the NVIC only around that sleep (main.rs
  `wake_on_io`), interrupts masked as for the touch line. **Inferred, not yet seen on the unit** (a finger needed:
  `tools/dev.py key` goes through the link); without it a press waits a pass, 250 ms at most.
- Short and long presses (lib.rs `Keys`): a short press acts at the release; a long one acts once, while the key
  is still held, and its release does nothing.

                   press        release
     short   ~~~~~~~~~~\____________/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                                    ^ acts at the release (held under 2 s)

                   press                          2 s           release
     long    ~~~~~~~~~~\__________________________________________/~~~~~~~
                                                  ^ acts once here (Back: at 3 s);
                                                    the release does nothing
     poll    |    |    |    |    |    |    |    |    |    |    |    |    every 50 ms

## 6. Display

- 240 x 320 panel, **ST7789-class controller** (inferred from the command set), RGB565.
- Commands over **SPI1** (0x40013000): 8 bit, transmit only, CPOL 1, CPHA 1 (mode 3), MSB first, D/C on PG9.
- SPI1 with software NSS (SSM): CR1.SSI must be 1 **before** CFG2.MASTER is set. Otherwise the internal NSS is
  low, the mode fault clears MASTER, and the SPI takes 16 bytes into its FIFO and never clocks (seen on the unit,
  2026-10-23; RM0399, SPI mode fault). Renode does not model it.
- Stock pushes pixels over the LTDC RGB interface (framebuffer in AXI SRAM). This firmware does not use the LTDC.
- One command on SPI1 (`lcd_cmd`): chip select low, D/C low for the command byte, high for its parameters (or
  the pixels of a RAMWR), chip select high. Mode 3, 50 MHz, MSB first. Pixels go two per 32-bit FIFO write (FTHLV
  4 frames; the SPI sends the word's bytes from the lowest, so each RGB565 pixel is byte-swapped into it); a fill
  writes the same word over and over. Checked in Renode against the simulator, and **seen on the panel** (2026-10-23).
- **SPI1's clock** (main.rs `spi_init`): kernel clock pll1_q_ck (RCC_D2CCIP1R SPI123SEL 0), 100 MHz (section 1;
  PLL1 always runs, and its Q output feeds the SDMMC too, so nothing is switched on for it), MBR /2: **50 MHz**. The
  ST7789V's serial write cycle is 16 ns at least (tSCYCW: 62.5 MHz; ST7789V datasheet, serial interface
  characteristics); SPI1's master clock goes to 80 MHz (DS12930 table 119). It was 8 MHz (HSI / 8), then 16 (HSI / 4,
  2026-10-25). Without POWER_STEPS bit 3, or if PLL1's Q output is off (RCC_PLLCFGR DIVQ1EN), HSI / 8 as before.
- **Pixel runs by DMA** (hw.rs `PanelDma`, main.rs `lcd_ram`): DMA1 stream 0, DMAMUX1 channel 0 on request 38
  (spi1_tx_dma; RM0399 DMAMUX1 table, stm32h7xx_hal_dma.h `DMA_REQUEST_SPI1_TX`), memory to peripheral, 32-bit both
  sides, direct mode, one word (two pixels) per SPI1 request: with FTHLV 4 frames, TXP (and so the request) means
  room for 4 bytes. Order (RM0399 SPI, "Communication using DMA"): TSIZE with SPE 0, the stream enabled, TXDMAEN,
  SPE, CSTART; TXDMAEN cleared after EOT (CFG1 is writable only with SPE 0). The master holds the clock when the
  FIFO is empty, so the stream can be re-armed mid-transfer: a blit goes in buffers of 128 words, the CPU filling one
  while the stream sends the other; a fill is one word repeated (no memory increment). The buffers are the top 4 KB
  of AXI SRAM (link.x PANELDMA, 0x2407f000): DMA1 reaches AXI SRAM and the SDRAM through the bus matrix, never the
  DTCM (RM0399 "System architecture"). The CPU sleeps (WFI) on the stream's line, NVIC 11, pended by TCIE / TEIE /
  DMEIE with interrupts masked; DMA1LPEN and SPI1LPEN are set for that wait only. Its NVIC pending bit is cleared
  just before each WFI (main.rs `sleep`), never right after LIFCR: there it stuck as IRQ 23's did (seen 2026-10-27:
  NVIC_ISPR0 0x00000c00, ICSR 0x0041b000, VECTPENDING 27 = IRQ 11, DMA1_LISR 0), so every WFI returned at once until
  the next transfer. An abort sets TCIF (RM0399 DMA,
  stream disable). **Seen on the unit 2026-10-27** (from RAM): a first timeout of 10 ms cut full-screen fills
  (0x7ffe pixels take 10.5 ms at 50 MHz); 30 ms since. Renode 1.17's SPI model has no transmit DMA request: there
  the first wait times out and the CPU path takes over (the pixels still match). **Cache contract:** the D-cache is
  off, so no maintenance; PANELDMA is 4 KB-aligned so one MPU region can make it non-cacheable when the D-cache is
  turned on (or clean it by address before each start: hw.rs).

     PG10 CS    ~~~~\______________________________________________/~~~~
     PG9  D/C   ~~~~\________/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
     PD7  MOSI       < cmd  >< param 0 >< param 1 > ... < param n >
     PG11 SCK   ~~~~~~|_|~|_|~ ... (idle high, data sampled on the rising edge: mode 3)
- Power-up (this firmware, `lcd_power_on`, as stock's order): PG3 high, 20 ms, PI3 low, 10 ms, PI3 high,
  120 ms, then the init sequence:

                0      20 ms   30 ms            150 ms   270 ms
     PG3 power  __/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
     PI3 reset  _______________/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~  (low = reset; low
                        |<10>|                                                     from board_init)
     SPI1                                      [01]     [13][21][36].. [11]  ..  clear [29]
                                               SWRESET  init table     SLPOUT      DISPON
                                               120 ms wait            120 ms

- Stock's init sequence (commands, parameters, waits in ms):

      01 ; 10 ms ; 13 ; 21
      36 00 ; 3A 05 ; 10 ms ; B7 05 ; 10 ms ; BB 29 ; 10 ms ; C0 2C ; 10 ms
      C2 01 ; C3 1D ; C4 20 ; C6 0F ; D0 A4 A1
      E0 F0 1F 2B 16 17 0F 4C 44 5B 39 16 15 27 28
      E1 F0 13 1E 09 0A 15 4C 43 5B 0F 1D 1D 28 29
      B0 00 F0 ; 11 ; 120 ms
      2A 00 00 00 EF ; 2B 00 00 01 3F ; B1 41 04 0A ; B2 0C 0C 00 33 33 ; B0 10 F0

  Display on (29) comes later. The final RAMCTRL `B0 10 F0` makes the panel take pixels from the RGB interface.
- This firmware (`src/drivers/panel.rs`) sends the same sequence without that final RAMCTRL, so `B0 00 F0` stays in force
  and pixels are written over SPI (RAMWR). It waits 120 ms after the software reset, sets the window before
  each write, and switches the display on after clearing the screen.
- Backlight: stock drives PG13 with an LPTIM1 PWM on the LSE, output polarity inverted, 1..100 levels. This
  firmware drives PG13 as a plain output, high = on (derived from the inverted PWM). **Seen on the unit
  (2026-10-22): no visible effect** (diag v5 to v8: toggled, held high, held low, code proven to run by v9). The
  backlight needs the panel on as well: **seen on the unit (2026-10-23)**, with the panel running, toggling PG13
  (the Page key) switches the backlight; which level is on is not checked yet.

## 7. USB

- **USB1 OTG_HS (0x40040000) with an external ULPI PHY**, high speed. Pins in section 4, all AF10.
- PHY enable PA7: high, then 200 ms before the core is used.
- Clocks: OTG HS and OTG HS ULPI in RCC_AHB1ENR. Kernel clock 48 MHz (stock: PLL3Q; this firmware: HSI48).
- Needs VOS1 (PWR_D3CR VOS = 3).
- VBUS sensing is off in the core: VBUS is read on **PC13**, not on the OTG VBUS pin.
- Interrupt: OTG_HS is IRQ 77.
- **Core interrupts the crate unmasks** (synopsys-usb-otg 0.5 `enable`): GINTMSK read 0x80043810 on the unit
  (RXFLVL, USBSUSP, USBRST, ENUMDNE, IEPINT, WUIM); SOF and EOPF stay masked, so frames do not wake WFI. GINTSTS
  reads 0x44808438 whenever the link answers a peek (SOF, EOPF, ESUSP and the FIFO-empty bits are set but masked).
- Stock uses it for mass storage only, and a plug-in restarts the unit into that mode (RTC backup register 4).
- This firmware: the dev link starts when PC13 goes high, and unplugging resets the unit (simplest way to power
  the PHY and the core down again):

     PC13 VBUS  ______/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\____
     PA7 PHY    ________/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~  (next 50 ms poll)
     OTG_HS              |<- 200 ms ->| core up, enumerated  ...  |
     strip USB  (none)   orange (powered) ........ green (configured) | system reset at the fall

- **Suspend** (the computer idle or asleep: no starts of frame for 3 ms, OTG_GINTSTS.USBSUSP and OTG_DSTS.SUSPSTS
  set, RM0399 OTG_HS): the dot turns grey and the link waits; resume signalling (OTG_GINTSTS.WKUPINT) or traffic
  brings it back to green. Not a drop (docs/journal.md, "2026-10-27: suspend, a normal state, not a drop").

## 8. RTC

- RTC (0x58004000) on the LSE, 24 h format, prescalers 31 / 1023 (1 Hz). It keeps running in standby.
- The stock firmware sets it from GPS. It holds **local time** (seen on the unit, 2026-10-23).
- Backup registers and stock (R039, from its 27 calls of the HAL read / write, bryton repo, ghidra/HARDWARE.md "RTC and backup
  registers"): 1 (clock set, 0xa5a5), 2, 3 (clock), 4 (USB mode), 9, 12 (Nordic image version), 13 (last battery
  voltage, mV: 4178 on the unit) are its own; 15 and 19 are read at boot (13 is kept only if 19 = 0xa5a5 and 15 !=
  0x4649; stock's first boot writes 19 = 0); 0, 5..8, 10, 11, 14, 16..18 and 20..31 are not referenced by its code
  (20..31 hold values after stock, 0x22 then 0x35a..0x3c5, from something not traced). The bootloader has no RTC
  address in it.
- This firmware uses 5, 6, 7, 8, 10 (paired ANT sensor per kind: heart rate, speed, cadence, speed and cadence,
  power), 14 (the paired shifting system, ANT+ tag 0x61 or Di2 tag 0x62; 14 held a ride page of the first builds,
  tag 0xa, read only), 11 (wheel, backlight, max HR, FTP), 16 (the rider's and the bike's weights and W', tag 0xa;
  0xb and 0xc1 in earlier builds, read only, which also held the Di2 single presses' actions), 17, 18 (GNSS aiding: the RTC's offset
  from UTC and the last position) and 20..27 (the first three ride pages and the overview, two each, tags 0xd /
  0xe; all the ride pages are in `\Rustfw\pages.bin`, the registers their copy for earlier builds; bit 24 of 21 set
  while they hold page changes the file does not have yet, `PAGES_NEWER`), each behind a
  tag so an empty register reads as unset; 14, 15 and 16 held the ride pages of the first builds, and 19..26 those of
  the builds of 2026-10-23 (moved off 19, which stock reads, and 19 set back to 0), 28 (the alerts and the Di2 presses' actions, tag 0x9; 0xc2 in earlier builds) and 29
  (a fault's marker, 0xfa and the faults in a row, written by hw.rs before it resets), 30 (a ride open on the card,
  0x52430001, lib.rs `RIDE_BKP`), 31 (USB drops the link recovered from, tag 0x0b, `USB_DROPS_BKP`) and 0 (what the USB drive saw last at a
  drop, tag 0xd1, `USB_LAST_BKP`; docs/usb.md). None free: the standby breadcrumbs share 0 (tag 0x5b) and 31 (the
  reset flags, tag 0x5c), the last writer winning; the whole table is in src/app/settings.rs.
- **Setting it** (this firmware, from the phone's time; not yet seen on the unit): RM0399's calendar initialisation:
  PWR_CR1 DBP, RTC_WPR 0xca then 0x53, INIT (RTC_ISR bit 7, RTC_ICSR in main.rs), wait for INITF (bit 6, 10 ms at
  most), TR and DR (the weekday in DR bits 13..15, 1 Monday to 7 Sunday; 0 is forbidden), INIT cleared, RSF (bit 5)
  cleared so the next read waits for the shadow registers, RTC_WPR 0xff. The prescalers stay as stock set them.
  Settings move to a file in this firmware's own folder once the eMMC is written (AGENTS.md, "Share the storage with
  stock"). This firmware keeps the paired heart-rate strap in register 5 (0x58004064). Writes
  need PWR_CR1.DBP, which reads set after the bootloader (seen on the unit); a write kept its value through a
  system reset (seen), and should through standby (backup domain, RM0399; to see).

## 9. Standby (power off)

- Wake-up pins **WKUP1 (PA0, power button)** and **WKUP4 (PC13, USB)**, rising, no pull.
- The power button drives both PA0 (high when pressed) and PA1 (low when pressed): seen on the unit (2026-10-23).
- PA0 must be a GPIO input before standby (stock does it at boot): left in its reset analog mode, the press does
  not wake the unit (seen on the unit, 2026-10-23); only the four-button restart brings it back.
- Entry as stock: PWR_CPUCR PDDS_D1 and PDDS_D2 set, SLEEPDEEP, WFI. Waking resets the unit.
- PDDS_D3 stays 0 (stock too): D1 and D2 power down, D3 only stops, so the system is in Stop, not Standby (the
  HAL's HAL_PWREx_EnterSTANDBYMode doc; Standby needs D1, D2 and D3 all in DStandby), and the GPIOs keep their
  levels (Nordic PI4 / PA6, GNSS power). In Stop the WKUP pin reaches the core only through EXTI line 55
  (EXTI_CPUIMR2 bit 23, reset value 0 in ST's SVD, but read 0xfff5ffff on the unit while running: already unmasked
  there): stock unmasks it and enables IRQ 149 (WAKEUP_PIN) before standby. Enabling IRQ 149 in the NVIC (our standby
  had cleared every enable) made the power button start the unit (2026-10-25, rustfw ea359a0, seen on the unit).
  Without it the press woke nothing (2026-10-25: WFI never returned, PWR_CPUCR.SBF clear at the next boot); to see
  with the fix.
- Stock waits for the back key to be released first (otherwise it wakes at once). Power off and on with the
  power button, as this firmware does it:

     button      ____/~~~~~~~~~~~~~~~~~~~~~~~~~\____________________________/~~~~\______
     PA0 WKUP1   ____/~~~~~~~~~~~~~~~~~~~~~~~~~\____________________________/~~~~\______  high pressed
     PA1 Back    ~~~~\_________________________/~~~~~~~~~~~~~~~~~~~~~~~~~~~~\____/~~~~~~  low pressed
                     |<------- 3 s ------->|   |<- 50 ms ->|                |
                                           |   released    standby          rising edge: reset,
                             radio off, GNSS off,           (PDDS_D1, D2,   bootloader, firmware
                             screen off, wait for the       SLEEPDEEP, WFI)
                             release

## 10. The update file

The stock OS installs `IMG.bin` from the root of its USB drive at start-up, with `update.ini` beside it.
- An `IMG.bin` with `R` and 3 digits at offset 0x94 is taken as a **raw image** (not gzip).
- It must be larger than 1 MB, smaller than 10 MB, and end with a 4-byte trailer: the additive sum of all bytes before it, little
  endian. The OS writes it to the eMMC OS area and reads it back to check that sum.
- The bootloader then copies it to 0xd0000000 at the next start (section 2).
- `tools/pack.py` builds this file and re-checks all of the above.
- **Seen on the unit (2026-10-22):** a raw image is written from offset 0 of the OS area, trailer included, and
  the rest of the area keeps the previous image (Engineer Mode readback after installing this firmware).
- In Engineer Mode the bootloader writes the OS area 2 KB at a time (`tools/iaprecover.py`); its write status is
  not trustworthy, so every chunk is read back.

## 11. Nordic BLE / ANT chip

A Nordic chip runs the radios (BLE and ANT+) with its own firmware; the STM32 drives it over USART2 with Nordic's
nRF5 SDK serialization. Traced in the stock firmware (Ghidra: ser_app_hal_hw_init 0xd024edac, Ble_NrfResetPin*
0xd02ba190 / 0xd02ba1a0, ser_phy_tx_pkt_send 0xd02c9328) and **seen on the unit (2026-10-23, `tools/nrf.py`)**.

- **Pins:** USART2 TX PD5, RX PD6, CTS PD3, RTS PD4 (AF7, pull-up). Reset PA6 (low = reset, output, pull-up).
  PI4 high at boot and kept high: the application. PI4 low through a reset starts the chip's DFU bootloader
  (stock does that only to update the Nordic firmware from `\Nordic.bin`; inferred from that path, not tried on
  purpose). PH8: the chip's request line, an EXTI wake-up only.
- **UART:** 1,000,000 baud, 8N1, RTS / CTS flow control. The kernel clock left by the bootloader is PLL3Q 48 MHz
  (RCC_D2CCIP2R USART234578SEL = 2), so BRR = 48.
- **Start-up, as stock:** PI4 high; PA6 low; USART2 up (so our RTS is low); PA6 high; wait 800 ms. The chip sends
  nothing on its own: the STM32 speaks first.

     PI4 mode    __/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~  high = application
     PA6 reset   ~~~\______________/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~  (low through a reset
     PD4 RTS     ~~~~~~~\__________________________________________________   with PI4 low: DFU)
     PD5 TX      ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~\_/ first command
                    |<- 150 ms -->|<----------- 800 ms ----------->|
                    USART2 up     released                 drop one glitch byte, clear errors
- **Framing:** a 2-byte little-endian length, then the payload. No delimiter, no checksum, no handshake and no
  resync: after a bad frame, reset the chip. (SLIP is only the DFU bootloader's, at 115200 baud.)
- **Frame on the wire:**

     | len lo | len hi | type | op | arguments (little endian) ...                |
     |<- payload length ->|<------------------- payload --------------------------->|
     command   00 op args        response  01 op [result u32] [outputs]
     BLE event 02 [id u16] ...   ANT cmd   06 op args (answered as type 01)
     ANT event 08 [41-byte ANT message] [event] [channel]   (44 bytes)

- **Payload:** `[type][op][arguments, little endian]`. Types: 0 command, 1 response, 2 BLE event, 5 reset
  command, 6 ANT command, 7 ANT response, 8 ANT event. A response is `[1][op][result u32][outputs]`.
- **Seen on the unit:**
  - `sd_ble_version_get` (`00 65 01`) before enabling: result 0x3001 (BLE_ERROR_NOT_ENABLED);
  - `sd_ble_enable` (`00 60`): result 0;
  - `sd_ble_version_get` again: result 0, then `04 | 01 00 | 32 00`. Bryton fills these fields with its own
    versions, logged by stock as "Image 4, Bootloader 1, SD 50"; stock keeps the image number in RTC backup
    register 12, which reads 4 on this unit.
- **This firmware (src/drivers/nrf.rs, seen on the unit 2026-10-23):** the same reset sequence, with USART2 on the HSI
  kernel clock (64 MHz, BRR 64) rather than the bootloader's PLL3; then `sd_ble_enable` and `sd_ble_version_get`
  without any `sd_ble_cfg_set`, both answered (result 0, version 4 / 1 / 50).
- **Receiving:** polled (`nrf_recv`), the 16-byte RX FIFO and RTS keep the chip's bytes waiting, so nothing is lost
  on this side however long a pass; how much the chip itself queues meanwhile is not known. So while idle (passes of
  250 ms) a byte ends the sleep at once: CR1.RXFNEIE pends USART2's IRQ 38, enabled in the NVIC only around that
  sleep (main.rs `wake_on_io`), interrupts masked.
- **Power off:** `sd_power_system_off` (`00 32`), no response; stock sends nothing else first and leaves PA6 /
  PI4 as they are. The next start's reset pulse wakes the chip (nRF52 pin reset; inferred, to see on the unit).
- **Scanning (API v5 layout, seen on the unit 2026-10-23):** `sd_ble_gap_scan_start` `00 8a 01 [flags]
  [interval u16] [window u16] [timeout u16]`, flags bit 0 = active, times in 0.625 ms units, timeout in s (0 =
  until `sd_ble_gap_scan_stop`, `00 8b`). Stock: active, interval 0x60, window 0x60 (or 0x30 to save power), no
  timeout. Reports: event 0x1d, `[2][1d 00][conn u16][peer addr: type byte, 6 bytes LSB first][direct addr 7]
  [rssi i8][flags][dlen][present][data]`; the name is AD type 0x09 / 0x08, often only in the scan response. Scan
  timeout: event 0x1b `[2][1b 00][conn u16][source]`. No `sd_ble_cfg_set` is needed before scanning.
- **ANT+ heart rate (seen on the unit 2026-10-23, `tools/nrf.py ant` and src/drivers/nrf.rs):** ANT commands are type 6,
  `[6][op][arguments]`, and **their responses come back as type 1** (`[1][op][result u32]`), like BLE ones. Stock's
  sequence for a heart-rate channel (Ghidra: ANT enable 0xd029dce0, `Ant_SetNetworkKeys` 0xd027d6d4,
  `Ant_ConfigureChannel` 0xd027d3c0, `Ant_OpenChannel` 0xd027d480), each answered 0 on the unit, after
  `sd_ble_enable`:
  - enable `06 fa 01 0f 00 20 00 7c 16 06 24 fc 05` (15 channels, 0 encrypted, event queue 32, stock's buffer
    pointer and size 0x5fc);
  - network key `06 cc [net] [8 bytes]`: net 0 is the ANT+ key. It is licensed and is not written here; stock
    loads it from its initialised data, at 0xd03d68f4 in R039 (`tools/ant_key.py` finds it by SHA-256);
  - assign `06 c2 [ch] 00 00 00` (slave, net 0), channel id `06 d3 [ch] 00 00 78 00` (any device, type 0x78 heart
    rate), low-priority search timeout `06 da [ch] ff`, frequency `06 cd [ch] 39` (2457 MHz), period
    `06 d1 [ch] 86 1f` (8070 / 32768 s), open `06 c4 [ch] 00 00`. Stock uses channel 0 for heart rate.
  - ANT event: `[8][41-byte ANT message][event][channel]`, 44 bytes; the message is `[size][id][channel][8 data
    bytes]...`. A broadcast is id 0x4e and the ANT+ heart rate is data byte 7, payload byte 11 (inferred from the
    ANT+ profile and stock's decoder 0xd029f91c; **seen on the unit** with the owner's strap: 58 to 70 bpm).
  - Channel ID get (stock's encoder 0xd02bf562): `06 d4 [ch]`, answered `01 d4 [result u32] [device u16] [type]
    [transmission type]`. With the wildcard channel ID, it gives the strap the channel locked onto (seen:
    `01 d4 00 00 00 00 xx xx 78 f1`, the owner's strap; xx xx is the device number, 2 bytes LE).
    Opening with that ID (`06 d3 00 xx xx 78 f1`) takes only that strap (seen). Other ANT ops in stock's encoders:
    0xc5 close, 0xc7 / 0xc8 broadcast / acknowledged data, 0xd5, 0xd7, 0xdd, 0xdf, 0xe7 (not traced).
  - A BLE scan with the channel open (searching) still works: fewer reports, as the radio is shared.
- **ANT+ sensor channels, as stock opens them** (`Ant_OpenSensorChannel` 0xd025e7f0): channel, device type, period
  (1/32768 s): 0 heart rate 0x78 8070; 1 speed 0x7b 8118; 2 cadence 0x7a 16204; 3 speed and cadence 0x79 8086;
  4 power 0x0b 4091; 5 ANT+ shifting 0x22 8192; 6 Shimano Di2 0x01 8198 (0x2006); then 7 0x28, 8 0x11, 9 0x02,
  10 0x14, 11 0x23 (other ANT devices, not used here). This firmware puts the FE-C trainer (type 0x11, 8192) on channel 7, free here, not on stock's 8 (a
  channel is only a number; stock's own is untouched). All channels are assigned once at start (`06 c2 [ch] 00
  [net] 00`); a channel is reconfigured only while closed. **Two networks** (stock's assignment table 0xd027d2a0,
  disassembled 2026-10-25): channels 6 (Di2) and 9 (Shimano STEPS) on network 1, the rest on 0 (channels 13 and
  14, stock's scan channels, extended: 14 on 0, 13 on 1). `Ant_SetNetworkKeys` 0xd027d6d4 sets key 0 from the table
  at 0x24010410 and key 1 from 8 bytes on (in R039's image, the 8 bytes after the ANT+ key: `tools/ant_key.py`).
  Every channel at 2457 MHz (0x39), low-priority search time-out 0xff (power: 7 and 0x61, not traced).
  This firmware sets the low-priority search time-out to 12 (30 s) and rests a channel closed after a search that
  found nothing (1, 2, then 4 minutes; app/radio.rs): a searching channel keeps the radio listening (ANT Message
  Protocol and Usage, search time-outs; recalled), about 5 mA on an nRF52 (docs/power.md, estimated).
  Shimano's channel sends its button page as acknowledged data (ANT message 0x4f, stock's `Ant_Di2_OnAntEvent`). Close: `06 c5 [ch]`, answered `01 c5 [result]`, then an ANT event with event code 0x07 (channel closed)
  in byte 42 and the channel in byte 43 (inferred from the ANT message format; the simulator models it).
- **BLE sensors (central), traced in stock, not yet tried on the unit** (Ghidra; ser_*_enc / _dec):
  - `sd_ble_gap_connect` op 0x8c: `00 8c 01 [addr type byte][6 addr] 01 [scan flags][interval][window][timeout]
    01 [min ci][max ci][latency][supervision] [conn cfg tag]`, the address byte as in the advertising report.
    Stock: scan active 0x60 / 0x60, timeout 0; connection 0x0006 / 0x000c, latency 0, supervision 0x0190; tag 1
    (after `sd_ble_cfg_set`; tag 0 without it, inferred). Disconnect op 0x76 `[conn u16][reason 0x13]`.
  - Events: connected 0x10 `[conn][addr 7][role][4 x u16 params]`; disconnected 0x11 `[conn][reason]`;
    connection parameter update request 0x1f (stock accepts: op 0x75 `[conn] 01 [same 4 params]`); PHY update
    request 0x21 (op 0x8f `[conn] 01 00 00`); data length update request 0x23 (op 0x90).
  - GATT discovery (stock uses Nordic's ble_db_discovery): primary services op 0x94 `[conn][start 0x0001] 01
    [uuid u16][type 1]`, characteristics op 0x96 `[conn] 01 [start][end]`, descriptors op 0x97 `[conn] 01
    [start][end]`. Responses 0x30 / 0x32 / 0x33 `[conn][gatt status u16][error handle u16][count u16]` then 7, 9
    or 5 bytes per entry. The CCCD is UUID 0x2902.
  - Enable notifications: op 0x9c `[conn] 01 [write op 1][flags 0][cccd handle][offset 0][len 2] 01 01 00`;
    write response 0x38. Notification 0x39 `[conn][status][error handle][handle][type 1][len u16][data]`.
  - Stock's `sd_ble_cfg_set` (op 0x69) frames: connection config tag 1 (6 links, event length 5), role counts
    (1 peripheral, 5 central), ATT MTU 153, 11 vendor UUIDs, attribute table 0x580, no service changed.
  - Stock's sensor search keeps reports at RSSI >= -75 dBm listing 0x180d (heart rate) or 0x1816 (speed and
    cadence); it has no BLE power client (power is ANT+ only on stock).
- **Stock's order at start:** `sd_ble_cfg_set` (op 0x69) six times, `sd_ble_enable`, ANT enable (`06 fa 01` + its
  configuration), `sd_ble_version_get`. Each command waits for its response, 5 s at most.
- **SoftDevice API version 5** (S132 / S332 v5.x, nRF5 SDK 14): stock's GATTS request encoders use 0xa0 service add,
  0xa2 characteristic add, 0xa6 hvx, 0xa7 service changed, 0xa8 authorize reply, 0xa9 / 0xaa system attributes set /
  get, 0xab, 0xad MTU reply (Ghidra, bryton repo, ghidra/notes/agent_b2.md, 0xd02ac312..0xd02ac966), and GATTC 0x94 / 0x96 / 0x97 /
  0x9c (above): GATTS at 0xa0 and GATTC at 0x94 are API v5's ranges (v6: 0xa8 and 0x9b; Nordic's pc-ble-driver
  `include/sd_api_v5/ble_ranges.h` and `sd_api_v6`), and the scan parameters and advertising report seen on the unit
  have v5's layouts. The Nordic image itself is not in the stock image (`\Nordic.bin`), so the exact version is not
  known. Opcodes then follow the v5 headers: GAP from 0x6c (adv data set 0x72, adv start 0x73, stop 0x74, appearance
  0x78, PPCP 0x7a, name 0x7c, authenticate 0x7e, sec params reply 0x7f, sec info reply 0x86), common 0x60 (UUID vendor
  add 0x62, user memory reply 0x66); events GAP from 0x10 (connected 0x10, disconnected 0x11, sec params request
  0x13, sec info request 0x14, auth status 0x19, conn sec update 0x1a, timeout 0x1b), GATTC from 0x30, GATTS from 0x50
  (write 0x50, system attributes missing 0x52, MTU request 0x55). Layouts: src/drivers/ble.rs, from the nRF5 SDK
  serialization codecs pc-ble-driver carries.
- **What this firmware configures (src/drivers/nrf.rs `start`, simulated only; docs/status.md has the test order):**
  two `sd_ble_cfg_set` before `sd_ble_enable`, each no larger than stock's: `00 69 20 00 00 00 01 01 02 03 00`
  (BLE_CONN_CFG_GAP, present, tag 1, 2 links, event length 3) and `00 69 40 00 00 00 01 01 01 00`
  (BLE_GAP_CFG_ROLE_COUNT, present, 1 peripheral, 1 central, 0 central security), the layout of pc-ble-driver's
  `ble_cfg_set_req_enc` (API v5: the id u32, a presence byte, the tag for a connection configuration, the
  structure). Advertising and `sd_ble_gap_connect` then use tag 1: the phone's link (peripheral) and a trainer's
  (central, FTMS) at once. A chip that refuses either, or answers `sd_ble_enable` NO_MEM, is reset and started
  without them (the defaults: one peripheral link, tag 0), so the phone works either way. The rest stays at the
  defaults: ATT MTU 23, attribute table 1408 B, 10 vendor UUIDs (ble.h, ble_gap.h, ble_gatts.h `*_DEFAULT`).
- **The central side (this firmware, simulated only):** `sd_ble_gap_connect` with stock's layout and parameters
  above (scan 0x60 / 0x60, 7.5 to 15 ms, latency 0, 4 s, tag 1), `sd_ble_gap_connect_cancel` `00 8d` after 10 s,
  `sd_ble_gap_scan_stop` `00 8b`; a peripheral's connection parameter update request (0x1f) answered with its own
  values (`00 75 [conn] 01 [4 x u16]`). The GATT client's events carry their connection handle (bytes 3-4), which
  sends the trainer's to app/ftms.rs and the phone's to app/phone.rs. The keyset given with `sd_ble_gap_sec_params_reply`
  comes back after BLE_GAP_EVT_AUTH_STATUS's own fields (the serialization's extension, ble_gap_evt_app.c). Stock has
  a peripheral too (ble_advertising, peer manager, its own services 0x7766 for Android notifications and 0x5566
  'ldc', Ghidra 0xd029c010 / 0xd029c3e0) but no HID, ANCS client or AMS client.

## 12. Battery and charging

From stock (Ghidra: `Batt_AdcInit` 0xd01fac58, `Batt_ReadVbat` 0xd01fadb8, `Batt_ChargeTempCheck` 0xd018b63c) and
**seen on the unit (2026-10-23)**.

- **Voltage:** ADC3 (0x58026000). Channel 19 is VREFINT (VDDA = 3.3 V x VREFINT_CAL / reading; VREFINT_CAL is the
  16-bit word at 0x1ff1e860), channel 1 is PC3_C, the battery through a divider: battery = pin x 1.5 (stock's
  factor 3/2). PC3_C reaches the ADC through the analog switch SYSCFG_PMCR.PC3SO (bit 27, 0 = connected), which
  reads 0 after stock. Stock samples both with 810.5 cycles.
- **This firmware:** kernel clock per_ck (HSI 64 MHz) / 16 = 4 MHz (BOOST 0), 16-bit, linearity and offset
  calibration. Seen on the unit:
  - the calibration takes 82 ms;
  - PCSEL and SMPR must be written before the calibration (written after it they stayed 0);
  - ADEN does not stick when set at once after ADCAL clears: set it until ADRDY;
  - a RAM run inherits an enabled ADC whose configuration can't be written: the driver resets ADC3 (RCC_AHB4RSTR
    bit 24) first.
  - Reading on USB: 99 %, in line with stock's last stored value (RTC backup register 13: 4178 mV).
- **Bring-up order that works** (`adc_init`):

                 0   ~10 us                      +82 ms        +<1 ms
     RCC reset   _/\_                                                    ADC3 from reset (a RAM run inherits it)
     DEEPPWD     ~~~\_____________________________________________________
     ADVREGEN    ___/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
     LDORDY      _______/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
     PCSEL, SMPR        ^ written here, before the calibration
     ADCAL       ________/~~~~~~~~~~~~~~~~~~~~~~~\_________________________  offset + linearity
     ADEN        ________________________________..../~~~~~~~~~~~~~~~~~~~  set again until ADRDY
     ADRDY       _____________________________________/~~~~~~~~~~~~~~~~~~
     reading     every 10 s: channel 19 (VREFINT), then channel 1 (battery), ~205 us each

- **Charge estimate:** a typical lithium-ion curve, 3.3 V = 0 % to 4.2 V = 100 % (inferred; stock's table is
  not traced). Below 3.3 V three readings in a row (10 s apart) on battery, the firmware switches off, as stock
  does at 0 %.
- **Charging:** PG14 is the charge enable, active low. The bootloader leaves it an output, low: **charging is on
  whenever USB power is present** (seen: MODER output, ODR bit 14 = 0). Stock switches it with a temperature
  window from the barometer: charge only if 0 < T < 50 C (5 < T after "too cold", T < 45 after "too hot"). This
  firmware does not drive PG14 yet: no temperature check, so charging below 0 C is possible. PF6: charger status
  input (reads high; meaning not traced).

## 13. GNSS receiver

From stock (Ghidra: `Gps_SpiInit` 0xd018af34, `Gps_SetPower` 0xd01f10ac) and **seen on the unit (2026-10-23,
`tools/gnss.py`)**.

- **Receiver:** u-blox M10: UBX-MON-VER answers "ROM SPG 5.10 (7b202e)", hardware 000A0000, PROTVER 34.10,
  GPS, GLONASS, Galileo, BeiDou, SBAS, QZSS.
- **Power:** PI6, high = on (stock: then 3 s before reading). PI6 is analog (undriven) after a reset and the
  bootloader. **Powered about 100 ms after being driven low from that state, the receiver does not start** (it
  never answers on SPI until power-cycled; seen on the unit 2026-10-23, 4 resets out of 4, reproduced by setting
  PI6 and the SPI4 pins analog for 3 s). Driven low for over a second first, it starts every time. Inferred: the
  undriven line (or the SPI pins) leave it half powered, and a short low does not reset it. This firmware drives
  PI6 low at boot and powers the receiver 1 s after the app's first step (over 2 s low in all).

    PI6 power  (reset: undriven) ____________________/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                                  |<-- driven low >= 1 s -->|<- 1 s ->| configuration, aiding
                                  boot (gnss_init)          first step + 1 s
- **SPI4** (0x40013400): PE2 SCK (pull-down), PE4 NSS, PE5 MISO, PE6 MOSI, AF5; mode 0. Stock: 1 MHz from the
  CSI, hardware NSS, DMA1 streams 6 / 7. This firmware: **4 MHz** from the HSI (SPI45SEL = 3, /16; the receiver
  answers and acknowledges at that rate, seen), PE4 driven by hand, 32-bit FIFO accesses (four 8-bit frames
  each), polled: 32 bytes a pass, more only while the receiver has data (it stops at 32 bytes of idle fill
  between frames). SPI4 is reset through RCC_APB2RSTR (bit 13) first:
  re-initialised while enabled it latched a mode fault (MODF) and would not start (seen).
- **Protocol:** UBX. The receiver idles with 0xff on MISO and ignores 0xff on MOSI. After stock it already
  sends **UBX-NAV-PVT** (class 0x01, id 0x07, 92 bytes) once a second on SPI, and no NMEA, straight from power-on:
  stock's configuration survives in the receiver. Its time does not: after power-on it reports 2021-03-07 and a
  clock from 00:00:00 until it has a fix. This firmware also sends, once, a CFG-VALSET to the RAM layer only
  (NAV-PVT on SPI, no NMEA on SPI; never flash or battery-backed RAM), in case the receiver ever loses that.
- **SPI4 transfer** (`gnss_transfer`), mode 0, 1 MHz; what goes out while there is nothing to say is 0xff, what
  comes back while the receiver has nothing is 0xff:

     PE4 CS    ~~~~\________________________________________________________/~~~~
     PE2 SCK   ______|~|_|~|_ ... (idle low, sampled on the rising edge: mode 0)
     PE6 MOSI       | b5 | 62 | 06 | 8a | len | ... | ck_a | ck_b | ff | ff |   (a UBX frame, padded
     PE5 MISO       | ff | ff | ff | b5 | 62  | 01  | 07   | 5c   | 00 | .. |    to 4-byte words)
     FIFO           |<- one 32-bit word ->|<- the next ...  (each FIFO write / read moves 4 frames)

- **UBX frame:**

     | b5 | 62 | class | id | length u16 LE | payload ... | ck_a | ck_b |
               |<--------- checksum over these ---------->|

- **Power-up, this firmware:**

     PI6 power   __/~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
     MISO (in)      ff ff ...  [NAV-PVT]          [NAV-PVT]          [NAV-PVT]   1 Hz
     MOSI (out)                      [CFG-VALSET][MGA time][MGA pos]
                    |<-- 1 s -->|    one per 50 ms pass; time and position only when known
     polled         64 bytes per 50 ms pass (2 x 32-byte transfers)

- **Indoors:** no fix, 0 satellites (seen). A fix: to see.
- **Power modes and currents** (u-blox M10 SPG 5.10 interface description UBX-21035062 R02, 4.9.15; MAX-M10S data sheet
  UBX-20035208 R08, table 15, 3 V, 1 Hz; the figures are the module's, the board's part is not known): CFG-PM-OPERATEMODE
  (0x20d00001) 0 FULL (continuous), 1 PSMOO (on / off), 2 PSMCT (cyclic tracking). Tracking at VCC: 9.5 mA continuous
  and 5 mA cyclic for the default GPS + Galileo + BeiDou B1I, 8 mA for GPS + Galileo, 7.5 for GPS; V_IO about 2.2 mA
  more. Stock has a GPS power setting (`gpsPW`, `Gps_PowerIndexToModeCmd` 0xd01f34a8: off, full, "trickle power",
  GLONASS, BeiDou) and in its power save mode switches to another constellation group after about 10 minutes of a
  good fix (`Gps_AutoSelectConstellationGroup` 0xd0254ab0); the UBX commands of each group are not traced. This
  firmware: GPS POWER AUTO (PSMCT only while riding fast and straight, `gnss::Adaptive`), FULL or SAVER (RAM layer,
  `gnss::config`, sent again at each change); not seen on the unit yet. docs/power.md has the estimates.
- **PSMCT's behaviour** (SAM-M10Q integration manual UBX-22020019 R01, 2.6.2, the M10 platform): 1 and 2 Hz navigation
  rates (and periods of 2 to 10 s); fixes still output at CFG-RATE in its power optimized tracking state, where no
  new signals are acquired and no data downloaded (it returns to continuous for scheduled ephemeris downloads, about
  every 30 minutes with CFG-PM-UPDATEEPH); SBAS not used in power save mode; BeiDou B1C not supported (B1I is, the
  default); a weak or lost signal returns it to tracking, then acquisition.
- **Switched off for an indoor ride (app/gps.rs `gnss_switch`):** PI6 low while a ride runs with the INDOOR profile
  (a u-blox M10 draws about 7 to 8 mA tracking), high again at the ride's end or when the profile goes to ROAD, 1 s
  later like the cold start (the line has been low over 1 s), then the configuration, the time and the position are
  sent again. The supply is cut, not UBX-RXM-PMREQ backup mode: its wake sources (EXTINT, SPI CS; u-blox M10
  interface description) and the V_BCKP wiring are not known here, and a receiver that never woke would stay dead.
  Inferred: a warm start after the ride, no ephemeris kept. The same cut between rides once idle (app/gps.rs
  `gnss_idle`: 5 minutes without a fix, or 10 with one and no key), with the same sequencing when it comes back.
- **Aiding at power-up, as stock's minimum** (stock also feeds AssistNow Offline data from `\System\mgaoffline.ubx`,
  which the phone app downloads; libMga, `GPS_AssistNow_Offline_Host`): after the configuration, this firmware
  sends UBX-MGA-INI-TIME_UTC (UTC from the RTC, which holds local time, minus an offset learnt at the first fix
  with a valid time, in 15-minute steps; 2 s accuracy) and UBX-MGA-INI-POS_LLH (the last good fix, kept to 1e-4
  degrees, 10 km accuracy), and turns AssistNow Autonomous on (CFG-ANA-USE_ANA, key 0x10230001, RAM). Seen on the
  unit: CFG-VALSET answered ACK-ACK, ANA reads back 1, and with CFG-NAVSPG-ACKAIDING on both MGA messages are
  answered MGA-ACK "used". **The time aiding works** (seen 2026-10-23, indoors, no fix): after power-up the
  receiver's NAV-PVT carried the right UTC date and time, flagged valid, instead of 2021-03-07. How much faster a
  fix comes: to measure outside.

## 14. Buzzer

From stock (Ghidra: `Buzzer_PwmInit` 0xd018b3f0, `Buzzer_PlayTone` 0xd018b54a, `Buzzer_On` 0xd018b4cc,
`Buzzer_Off` 0xd018b614).

- **TIM1** (0x40010000) channel 3, PWM mode 1, on **PA10** (AF1, no pull); **PH2** = amplifier enable (high = on).
  The tone is the timer's period; stock sets the pulse to half the period (a square wave). Stock's default:
  prescaler 41, period 99 (5.95 kHz if TIM1 runs at 25 MHz, inferred).
- TIM1 is an advanced timer: its outputs stay off until BDTR.MOE is set.
- This firmware computes TIM1's input clock from the prescalers the bootloader left (RCC_D1CFGR.HPRE,
  RCC_D2CFGR.D2PPRE2; doubled when APB2 is divided, RM0399) and picks prescaler and period for each tone. Between
  tones PA10 is a plain low output and PH2 low. Rising tones at power-on, falling at power-off (stock's own
  melodies are not traced). To hear on the unit.

    PH2 amp     ____/~~~~~~~~~~~~~~~~~~~~~~\_____
    PA10 TIM1   ____|_|~|_|~|_ ... _|~|_|~|_|_____   square wave, pulse = period / 2
                    |<- 80 ms 1500 Hz ->|<- 120 ms 2500 Hz ->|   (power-on: rising)

## 15. eMMC

**Seen on the unit (2026-10-23, `tools/dev.py emmc` / `sector`, read only, from RAM):**
- SEC_COUNT 0x3a40000 (31.3 GB); B = 0x3a2dfe6; OS area from 0x3a3d000.
- The first 8 KB of the OS area equal the installed image as the bootloader copied it to SDRAM (`peek`).
- **No MBR:** sector 0 is the FAT32 boot sector itself: OEM "MSWIN4.1", 512 bytes per sector, 32 sectors per
  cluster (16 KB), 32 reserved sectors, 2 FATs, total sectors 0x3a2dfe6 (= B exactly), FAT size 0x3a28 sectors,
  root directory at cluster 2, FSInfo at sector 1, signature 55 aa.
- B + 0x4404 holds the light sensor calibration `55 aa a4 03 55 aa`: 0x3a4 = 932 (as in stock's log).
- Handling a link command that uses the driver inside the main loop's sleep froze the loop (the driver's waits take
  over SysTick); such commands now end that pass's sleep, as a key from the link does.

From stock (Ghidra, OS R039 and bootloader R002; the addresses are those of the report
`ghidra/STORAGE_AND_MAPS.md` in the bryton repo, sections 1 and 2), register meanings from RM0399 (SDMMC) and the card side from
the JEDEC eMMC standard. **Nothing of this section is seen on the unit yet**: this firmware's driver
(`src/drivers/emmc.rs`, read only) runs only on the dev link's `emmc` and `sector` commands.

- **Card:** a soldered eMMC (stock's log calls it "iNAND"), about 31 GB (SEC_COUNT about 0x3a3_e000, inferred
  from the ~31 GB USB drive; to see with `tools/dev.py emmc`).
- **Pins** (stock's Mmc_MspInit 0xd018b790): PC8..PC11 D0..D3, PC12 CLK, PD2 CMD, all AF12, push-pull, no pull,
  very high speed. No card detect, no write protect.
- **PF8:** stock drives it low 10 ms, then high, then waits 600 ms before a new card init after a pin or
  brown-out reset (Pwr_DetectResetCause 0xd018b1c4). RST_n or a power switch: inferred, not settled (RST_n works
  only if EXT_CSD byte 162 RST_n_FUNCTION enables it). This firmware does not touch PF8.
- **Clocks:** SDMMC1 on AHB3 (RCC_AHB3ENR bit 16, reset RCC_AHB3RSTR bit 16), kernel clock D1CCIPR.SDMMCSEL = 0,
  PLL1Q = 100 MHz (section 1; stock 0xd018b790). Bus clock = 100 MHz / (2 x CLKDIV). Stock: CLKDIV 250 (200 kHz)
  at power-on, 126 (397 kHz) for identification, 2 (25 MHz) after (Mmc_SetClockSpeed 0xd018b896). Default speed,
  single data rate, no HS_TIMING switch (no 0x03b9.... literal in the image). Whether the bootloader's PLL1Q
  output (RCC_PLLCFGR DIVQ1EN, bit 17) is on: inferred from the bootloader reading the eMMC with SDMMCSEL at its
  reset value, to see (`tools/dev.py peek 0x5802442c`).
- **Controller settings, stock** (Mmc_Init 0xd018b824): falling clock edge (CLKCR NEGEDGE), power save (PWRSAV),
  4-bit bus, hardware flow control (HWFC_EN), DTIMER 0xffffff. Data path polled through the FIFO (0x80), no IDMA:
  read 8 words on RXFIFOHF until DCRCFAIL, DTIMEOUT, RXOVERR or DATAEND, then ICR = 0x1fe00fff
  (Mmc_ReadBlocksPolling 0xd018b8fc). Blocks of 512 bytes (DCTRL DBLOCKSIZE 9).
- **This firmware:** the same kernel clock, CLKDIV 125 (400 kHz) then 2 (25 MHz), NEGEDGE and HWFC_EN as stock,
  PWRSAV at 25 MHz only. SDMMC1 is reset through RCC first (a RAM run, or an earlier failed init, can leave it
  mid-transfer). A read: DCTRL 0, DTIMER 2,500,000 (100 ms), DLEN, DCTRL = 512-byte blocks, card to host, then the
  command with CMDTRANS (RM0399: the command starts the data path); the FIFO read a word at a time while RXFIFOE is
  clear, until DATAEND with every word in. CMD12 goes out with CMDSTOP. Every wait has a time-out (10 ms a command,
  100 ms + 1 ms a block for data).
- **Bring-up, stock** (uC/FS FSDev_SD_Card_Refresh 0xd0394e96): CMD0; CMD8 0x1a5 and ACMD41 (SD probes, unanswered
  by an eMMC: about 0.5 s lost); CMD1 0x40ff8000 every 2 ms until OCR bit 31 (bit 30: sector addressing); CMD2;
  CMD3 0x10000 (the host sets RCA 1); CMD9; 25 MHz; CMD7; CMD16 512; CMD6 0x03b70101 (EXT_CSD[183] BUS_WIDTH = 4
  bits) and CMD13 until ready; CMD8 SEND_EXT_CSD (FSDev_SD_Card_ParseEXT_CSD 0xd03960a0). Wait-ready (0xd0395830):
  CMD13 until state 4 (tran) and READY_FOR_DATA, CMD12 first if the card is in state 5 or 6. Reads: CMD17, or CMD18
  then CMD12 (FSDev_SD_Card_Rd 0xd0394a8c); the argument is the sector number for a high-capacity card.
- **Writes, this firmware (not yet seen on the unit; simulator only):** `Card::write` in emmc.rs refuses any
  sector at or past B before a command goes out; then CMD13 until tran, CMD24 (one sector) or CMD25 then CMD12
  (more), and CMD13 until the card is back in tran with READY_FOR_DATA (programming done, 1 s at most), its status
  checked. main.rs `emmc_write` mirrors the read: DCTRL 0, DTIMER 2,500,000, DLEN, DCTRL 512-byte blocks host to card
  (DTDIR 0), the command with CMDTRANS, then a word into the FIFO whenever TXFIFOF (STA bit 16) is clear, until every
  word is in and DATAEND; DCRCFAIL (the card's CRC status token), DTIMEOUT and TXUNDERR (bit 4) are errors (RM0399
  SDMMC; ST's HAL_MMC_WriteBlocks feeds the FIFO on TXFIFOHE and checks the same flags). No erase, no CMD6 other
  than the bus width. What is written and in which order: `src/storage/fat/write.rs` and `src/storage/recorder.rs`.
- **Reliable write, this firmware:** the EXT_CSD read at identification
  gives WR_REL_PARAM (byte 166) and REL_WR_SEC_C (byte 222), as Linux's MMC core reads them (include/linux/mmc/mmc.h,
  drivers/mmc/core/block.c, checked 2026-10-24). With EN_REL_WR (166 bit 2) every write is CMD23 SET_BLOCK_COUNT
  with bit 31 and the count, then CMD25, which ends by itself (no CMD12); with REL_WR_SEC_C only (the legacy mode),
  the same one sector at a time; with neither, plain writes as above. A cut during a reliable write leaves each
  sector old or new; during a plain one, JEDEC allows the sector being written to be garbage, and a cut tears what
  the recorder rewrites in place every minute (the ride's first and last sectors, its directory sector, a FAT
  sector). The unit's (2026-10-29, `tools/dev.py emmc`, read-only): WR_REL_PARAM 0x15 (EN_REL_WR set: the enhanced
  reliable write, so every write of this firmware is reliable), REL_WR_SEC_C 1, SEC_COUNT 0x3a40000 (31.3 GB).

      CMD   [CMD13]..[R1 tran] [CMD23 1<<31 | n]..[R1] [CMD25 lba]..[R1]                     [CMD13]..[prg]..[tran]
      D0..3                                                       [start][512 B][CRC16][end] x n ..[busy]

      CMD   [CMD13]..[R1 tran] [CMD25 lba]..[R1]                          [CMD12]..[R1b] [CMD13]..[prg]..[CMD13]..[tran]
      D0..3                                 [start][512 B][CRC16][end]..[CRC status][busy]..[next block]..
      STA                            TXFIFOF clear: write FIFO                DATAEND
- **Bring-up, this firmware** (emmc.rs), the same without the SD probes and CMD9 (emmc.rs says why), and with the
  EXT_CSD read again on the 4-bit bus as a check. CMD0 first whatever the card's state: the bootloader leaves it
  selected, in the transfer state, 4-bit (bootloader 0x08008aa4), and CMD0 clears all of that.

      power on  |2 ms|  400 kHz, 1 bit                         |  25 MHz, 1 bit                        | 4 bit
      CMD       CMD0  CMD1 ... CMD1  CMD2  CMD3   CMD7 CMD13 CMD16 CMD8      CMD6       CMD13  CMD8
      arg       0     40ff8000      0     10000  10000      200   0         03b70101            0
      card      idle  busy ... ready ident stby   tran            EXT_CSD   bus 4 bit           EXT_CSD
      data (D0)                                                   [512 B]                       [512 B]
                      |<- 1 ms apart, 1 s at most ->|              SEC_COUNT                     same SEC_COUNT
- **A read** (CMD17; CMD18 the same with more blocks, then CMD12):

      CMD   [CMD13 rca]..[R1: tran]  [CMD17 lba]..[R1]
      D0..3                                           ..[start][512 B, 4 bits per clock][CRC16 x 4][end]
      STA                                                   RXFIFOE clear: read FIFO      DATAEND
- **Layout** (N = SEC_COUNT, B = N - 0x1201a; stock's device size, FSDev_SD_Card_ParseEXT_CSD 0xd03960a0):

| sectors | size | what | evidence |
|---|---|---|---|
| 0 .. B-1 | ~31 GB | the FAT volume, the USB drive "BRYTON" (MBR or not: to see) | uC/FS "sdcard:0:", FS_AppOpenSdVolume 0xd02443c8 |
| B+2 .. B+0x1001 | 2 MB | factory image, header "BRYTONBRYTON" | Cfg_RestoreFactoryImage 0xd018dcb0 |
| B+0x4000 .. | | TimeZone.bin image | fwupdate.c (recovered source) |
| B+0x4403 | 1 | NAND serial, 16 bytes at 0x1f0 | Cfg_ReadNandSerial16 0xd018cbd4 |
| B+0x4404 | 1 | light sensor calibration, `55 aa` `v` `..` `55 aa` | Pwr_ReadLightSensorCali 0xd018dc10 |
| B+0x501a .. B+0xf019 | 20 MB | DeviceLangs.bin (UI fonts) | Ui_LoadDeviceLangsTtf 0xd0189fc0 |
| B+0xf01a .. N-1 = N-0x3000 .. N-1 | 6 MB | OS area: what the bootloader copies to SDRAM (section 2) | bootloader 0x08007fe2 |

- The USB drive of the stock OS is exactly sectors 0 .. B-1, so a read-only `dd` of it on a computer gives the
  same bytes as `tools/dev.py sector` there. The OS area starts with the installed image (section 10).
- Writes only from the ride recorder, inside the FAT volume; the one CMD6 is the bus width, a volatile byte the
  card clears at CMD0 and power-off. The card is identified at first use (the basemap, the dev link, a ride's
  start), and at boot only when backup register 30 says a ride is open.

## 16. Sensor bus (I2C4): light sensor, barometer, accelerometer

From stock (Ghidra: `I2C2_Configuration` 0xd02437e8, which sets up I2C4 despite its name; `Sensor_I2cWrite`
0xd024390c, `Sensor_I2cRead` 0xd0243968; the chip drivers below).

- **I2C4** (0x58001c00, D3 domain), PD12 SCL, PD13 SDA, AF4, open drain, no internal pull (the pull-ups are on the
  board, inferred). Stock: kernel clock APB4 12.5 MHz, TIMINGR 0x00100414 (about 400 kHz), HAL `Mem_Write` /
  `Mem_Read` with a 1-byte register address, 1 s time-out. This firmware: kernel clock HSI 64 MHz (RCC_D3CCIPR
  I2C4SEL = 2), TIMINGR 0xf0420709 (about 200 kHz), polled, every wait 1 ms at most; I2C4 is reset first
  (RCC_APB4RSTR bit 7), as for the other peripherals a RAM run inherits.
- **Chips** (7-bit addresses; stock writes them shifted, as 8-bit):

| chip | address | ID | stock's use |
|---|---|---|---|
| ambient light, Lite-On **LTR-308ALS** (inferred from the address, the part ID and the register map) | 0x53 | PART_ID 0x06 = 0xb1 | `Sensor_LightSensorCmd` 0xd023e024: MAIN_CTRL 0x00 = 2 on, 0 standby; MEAS_RATE 0x04 = 0x05, GAIN 0x05 = 2; interrupt on thresholds (INT_CFG 0x19, INT_PST 0x1a, thresholds 0x21 / 0x24, 3 bytes each) |
| barometer, ST **LPS22HH** | 0x5c | WHO_AM_I 0x0f = 0xb3 | `BSP_Baro_Init` 0xd023d234: CTRL_REG2 0x11 = 0x92 (reboot), IF_CTRL 0x0e = 0x02, CTRL_REG1 0x10 = 0x3e (25 Hz) |
| barometer, Bosch BMP280 (the alternative stock tries first) | 0x76 | 0xd0 = 0x58 | `Baro_DetectModel` 0xd023d3dc |
| accelerometer, ST LIS2DH12 / LIS3DH class | 0x18 | WHO_AM_I 0x0f = 0x33 | `Sensor_GSensorCmd` 0xd0243b60: off is CTRL_REG1 0x20 = 0x07; on, 400 Hz with an interrupt on INT1 (PD11) |

- **Seen on the unit (2026-10-23, `tools/i2c.py`, with a build that never touched the bus):** exactly three chips
  answer (0x18, 0x53, 0x5c; no BMP280), with the IDs above (0xb1, 0xb3, 0x33). Stock's settings survive a reset
  (the chips stay powered): the light sensor in standby (MAIN_CTRL 0) with MEAS_RATE 0x05 and GAIN 0x02, the
  barometer at CTRL_REG1 0x02 (so stock's power-down value is 0x02: output rate 0, BDU) and CTRL_REG2 0x12, the
  accelerometer off (CTRL_REG1 0x07). Indoors, dim: a light reading of 3 (0.1 lux); the barometer 994.5 hPa,
  25.5 C.
- **Altitude calibration in stock** (ghidra notes agent_a2.md: Cfg_DevDataReadAltitudeCalib 0xd01867c2,
  Cfg_DevDataApplySettingById id 0x1d, Sensor_CalibrateAltitudeFromGps 0xd0193dc0, Baro_CheckAltCalibrationAtLocation
  0xd026aaac): `\System\DeviceData.ini` `[DeviceSys]` keeps `calAltitude_km` (default 200: metres), `calAltitude_mi` (default
  218.7: yards, inferred from 200 m) and `Alt_P0` (float, hPa, default 1020.0, the sea-level pressure); the GPS altitude
  of a 3D fix can be sent to the sensor task as the calibration. This firmware imports `Alt_P0` only.
- **Light reading** (`Sensor_LightSensorReadLux` 0xd023e1e8): ALS_DATA 0x0d..0x0f, 20 bits little endian; lux =
  reading x 0.025 x calibration / 1000 (the calibration word comes from an eMMC raw sector, 1000 by default). 0.025 is
  the datasheet's 0.6 / (gain 6 x 4), the 4 being the 400 ms of a 20-bit conversion (MEAS_RATE 0x05: 20 bits, one
  measurement a second). Stock masks the reading to 18 bits before scaling (`ubfx #0, #18`), so above 6553 lux it
  would wrap; this firmware keeps all 20 (full scale 26214 lux). MAIN_STATUS 0x07 bit 3: a new reading.
- **Stock's automatic backlight** (`Sensor_LightSensorUpdateBacklight` 0xd023e304, on the sensor's threshold
  interrupt on PF9; tables in initialised data at 0x2400ea44, read from the image by running its start-up data
  copy): lux bands 0..5, 5..20, 20..50, 50..120, 120..26214, and the backlight level in each band for each of the
  user's five settings (byte 0x3003e643):

      setting   0..5  5..20  20..50  50..120  >120 lux
      0           5     10      0       0       0
      1           8     20     30       0       0
      2          11     30     40       0       0
      3          14     40     50      70       0
      4          17     50     60      80       0

  Level 0 is the PWM's minimum, 1 % (`Pwr_BacklightSetPwmDuty` clamps to 1..100): in daylight the transflective
  panel is read by reflected light. Changing band reprograms the sensor's thresholds to the new band's edges.
- **Stock's timing and hysteresis** (`Sensor_LightSensorCmd` 0xd023e024, `Sensor_LightSensorUpdateBacklight`):
  MEAS_RATE 0x05 is a 400 ms, 20-bit conversion once a second (rate field 5 is 1000 ms in DFRobot's driver,
  `eRate_1000ms`); INT_PST 0x1a = 0 (an interrupt on the first reading outside the thresholds); at each interrupt the
  band is the first whose edges hold the reading (`lo <= lux <= hi`), and the thresholds are re-armed at that band's
  own edges: no averaging beyond the conversion and no hysteresis margin. Level 0 sets the PWM to its 1 % minimum;
  stock switches the backlight off instead in one state (`Pwr_BacklightApplyOnOff` 0xd0189c42 when 0xd0189c0e returns
  1 and 0xd0189c16 returns 0; what those two read is not traced). Which of the five settings a fresh stock unit uses
  is not traced either (DeviceData.ini `AutoBrnVal`).
- **This firmware** (src/drivers/ambient.rs): MEAS_RATE 0x22 (18 bits in 100 ms, a reading every 100 ms; the fields from
  DFRobot's LTR-308 driver: resolution in bits 6..4, rate in 2..0), so 0.1 lux per count at gain 6. The sensor
  measures continuously while the backlight is AUTO and on (or the sensors menu shows the light), read every 250 ms
  (MAIN_STATUS, ALS_DATA), and is in standby (MAIN_CTRL = 0) otherwise. (Until 2026-10-23: one 400 ms measurement
  every 3 s, which the owner found too slow to follow the light.) The level follows stock's curve exactly since
  2026-10-27 (src/drivers/ambient.rs `AUTO_BANDS`: its setting 1, 8 / 20 / 30 % up to 5 / 20 / 50 lux, the 1 %
  minimum above; the owner's choice, dusk at 20 %) and stock's timing: the band chosen once a second from the mean of
  that second's four readings, no hysteresis. (Before: stock's setting 3 with 8 % in the dark, a running average and a
  quarter of the edge as hysteresis.) Stock's lux are this firmware's x 0.93 on this unit (its calibration, below):
  its edges fall about 7 % lower in this firmware's lux, not corrected. **Seen on the unit (2026-10-23)** with stock's curve: a desk lamp read about
  480 lux (raw 19304) and dimmed the screen to 1 %, which the owner found wrong; the room read 0.3 to 0.6 lux, a hand
  over the sensor 0.05. Stock's calibration on this unit is 932 (its debug log, `Light sensor cali value 932`, from
  the eMMC raw sector at base + 0x4404 with 55 aa markers; `0xd018dc10`), so its lux are this firmware's x 0.93. The barometer powered down (CTRL_REG1 0x10 = 0x02: output rate 0, BDU) and one one-shot conversion
  every 10 s (CTRL_REG2 0x11 = 0x11); STATUS 0x27 bits 0, 1; PRESS_OUT 0x28..0x2a (24 bits, 4096 per hPa) and
  TEMP_OUT 0x2b..0x2c (C x 100). The accelerometer switched off at start (CTRL_REG1 0x20 = 0x07), as stock does at
  boot. Seen on the unit (2026-10-23, from RAM): the menu rows read LUX 0, TEMP C 25.5, HPA 994, and AUTO in the
  dark set the PWM to level 14 (LPTIM1_CMP 13).
- **A register read** (a write of the register address, a repeated start, the read; AUTOEND sends the stop):

     SCL  ~~~~\_/~\_/~ ... ~\_/~~~~~\_/~\_/~ ... ~\_/~\_/~ ... ~\_/~~~~~~
     SDA  ~~\_< addr W >A< reg >A~~~\_< addr R >A< data 0 >A ... < data n >N_/~~
          S                       Sr                                        P
          |<---- CR2: NBYTES 1, START ---->|<-- CR2: RD_WRN, NBYTES n, AUTOEND, START -->|
                        TXIS: write reg, TC                 RXNE per byte, STOPF

  S start, Sr repeated start, P stop, A acknowledge (by the chip, or by the STM32 for data bytes), N the last byte
  not acknowledged. A write is one transfer: address, register, data, with AUTOEND.

## 17. Touchscreen

The S510 has a touch panel over the display (retail spec: 2.8 in colour touchscreen and 4 buttons). From stock
(Ghidra: `Touch_GpioInit` 0xd0243d90, `Touch_Init` 0xd0243e08, the EXTI6 handler 0xd0243eaa, `Touch_GetState`
0xd024400e) and Sitronix's "Touch IC Protocol A" v2.7
(https://www.crystalfontz.com/products/document/4590/Sitronix_Touch_IC_Protocol_A_V2.7.pdf), whose register map
stock's reads match. Read on the unit on 2026-10-23 (below).

- **Controller:** Sitronix ST1xxx class at I2C address **0x55** (stock passes 0xaa, 8-bit). Stock reads two point
  slots (0x12, 0x16), so an ST1232-class part (2 points; Linux `st1232.c`); the exact part is inferred. Registers
  0x0c..0x0f (firmware revision) and 0x3f (max contacts) would tell.
- **Bus and pins:**

| what | value |
|---|---|
| bus | **I2C1** (0x40005400), PB6 SCL, PB7 SDA, AF4, open drain, no internal pull; RCC_APB1LENR bit 21, reset APB1LRSTR bit 21 |
| stock's timing | kernel clock D2PCLK1 12.5 MHz, TIMINGR 0x00100414 (about 400 kHz, inferred); HAL analog filter call with 0 (on, inferred from HAL's encoding), digital filter off |
| interrupt | **PC6**, EXTI6 falling edge, no pull (EXTI9_5 shared with the light sensor's PF9 line). This firmware: EXTI6 from port C (SYSCFG EXTICR2), falling edge, unmasked for the CPU, EXTI9_5 enabled in the NVIC only so the edge wakes WFI (interrupts stay masked); the main loop clears EXTI PR1 bit 6 and reads it back; the NVIC pending bit is cleared just before each WFI with every wake source's (main.rs `sleep`): cleared right after the EXTI write, the still-asserted line pended IRQ 23 again and it stuck (seen 2026-10-27: NVIC_ISPR0 0x00800400, ICSR 0x00427000, VECTPENDING 39 = IRQ 23, with EXTI_CPUPR1 0x00000010, line 6 clear), so every WFI returned at once |
| power | PG3 high, shared with the panel (already high in this firmware) |
| reset | **PH3**, high to run (active-low RESET: the chip answers on I2C1 once PH3 is high, seen on the unit) |

- **Stock's start:** PC6 as falling-edge EXTI; PG3 high, 10 ms; PH3 high, 100 ms (the protocol doc gives about 50 ms
  of init); I2C1 up; read status (0x01: low nibble the device status, high nibble the error code), the firmware
  version (0x00), the resolution (0x04, 3 bytes, decoded as a point). It writes no register: Device Control (0x02)
  and Timeout to Idle (0x03, the chip idles after 8 s without a touch) stay at their defaults.
- **A register read:** a 1-byte write of the register, stop, then a separate read (HAL transmit, then receive).
- **A point:** 3 bytes from 0x12: byte 0 bit 7 valid, X = (b0 & 0x70) << 4 | b1, Y = (b0 & 7) << 8 | b2. Stock
  maps raw to screen 1:1 (0..240 x 0..320, inferred: no swap seen) and treats a point outside as "no finger"
  (its sentinel is 241, 321).
- **Stock's reading loop:** the EXTI6 interrupt only sets a flag and wakes the UI task; the task reads 0x12 while a
  finger is down and goes back to waiting on the interrupt when none is. A touch turns the backlight on and resets
  the auto-off counter. On an I2C error: status and version read again, then I2C1 disabled for 10 ms and enabled.
- **Read on the unit** (2026-10-23, this firmware from RAM: PH3 low 5 ms then high, I2C1 at about 200 kHz from
  HSI; `tools/i2c.py --touch read 0x55 REG N`, no finger on the panel):

| register | bytes read | meaning |
|---|---|---|
| 0x00 firmware version | 01 | 1 |
| 0x01 status | 04 | Idle (device status 4), no error: the chip had idled since its start |
| 0x02, 0x03 | 00 02 | Device Control at its default; Timeout to Idle 2 (seconds per the protocol doc; the doc's default is 8) |
| 0x04..0x06 resolution | 01 f0 40 | **240 x 320**, the panel's size: raw points map 1:1 (orientation not checked yet) |
| 0x0c..0x0f firmware revision | 05 31 03 30 | |
| 0x3f max contacts | 05 | **5 points**: an ST1633-class part (Linux `st1232.c`: ST1232 2, ST1633 5; the part number is inferred), although stock reads only 2 slots |
| 0x12.. point slots | 00 ... | no valid bit: no finger |

  INT (PC6) read high with no finger, and polling it costs nothing measurable: awake 6 to 7 per mille before and
  after the driver (`tools/dev.py info`). Not checked yet (needs a finger): that INT goes low while a finger is down,
  and the axes (expected: x 0 at the left, y 0 at the top, as the panel; to check, hold a finger near the top left
  corner and run `tools/i2c.py --touch read 0x55 0x12 3`: small x and y, valid bit set).
- **Gestures:** none from the chip (register 0x10, gesture type, exists only on the ST1615 per the protocol doc;
  stock never reads it). Stock recognises them in software (Embedded Wizard handlers): tap, hold, slide (scrolling
  lists), wipe. Its page swipe (`Ui_OnTouchReleaseDetectSwipe` 0xd017c894): on release, if the drag was over 10 px
  or the hold at least 600 ms: dy < -48 is up, dx >= 37 right, dx <= -37 left; otherwise a tap at that point.
