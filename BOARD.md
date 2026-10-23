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
    a readout, which later proved not to work (section on the backlight below). Unproven.
  - **Seen on the unit (2026-10-22, diag v9):** an image that turns the I-cache on, reads PC13 and requests a
    system reset (AIRCR) runs as expected: the reset brings the bootloader up again (README "First run on the
    unit").
  - Switching the MPU off is not an option: the default memory map makes 0xc0000000..0xdfffffff execute-never.
  - **Unaligned reads from the SDRAM return wrong bytes** (seen on the unit, 2026-10-23, this firmware's region:
    normal, not cached). A 7-byte constant at 0xd000a0c3 copied with unaligned loads came out as `03 01 03 00`
    for `02 01 03 04`: bytes from the neighbouring lanes. Aligned word reads of the same memory (`dev.py peek`)
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
- PB4 leaves reset as NJTRST (alternate function, pull-up): it must be made an input (seen on the unit: MODER
  field 2, PUPDR pull-up, IDR high at rest). PA1, PB2 and PE3 read high at rest without a pull-up (external
  ones, inferred).
- Stock reads them by EXTI falling edge and debounces 15 ms later. This firmware polls them every 50 ms.

## 6. Display

- 240 x 320 panel, **ST7789-class controller** (inferred from the command set), RGB565.
- Commands over **SPI1** (0x40013000): 8 bit, transmit only, CPOL 1, CPHA 1 (mode 3), MSB first, D/C on PG9.
- SPI1 with software NSS (SSM): CR1.SSI must be 1 **before** CFG2.MASTER is set. Otherwise the internal NSS is
  low, the mode fault clears MASTER, and the SPI takes 16 bytes into its FIFO and never clocks (seen on the unit,
  2026-10-23; RM0399, SPI mode fault). Renode does not model it.
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
- Stock uses it for mass storage only, and a plug-in restarts the unit into that mode (RTC backup register 4).

## 8. RTC

- RTC (0x58004000) on the LSE, 24 h format, prescalers 31 / 1023 (1 Hz). It keeps running in standby.
- The stock firmware sets it from GPS. It holds **local time** (seen on the unit, 2026-10-23).
- Backup registers used by stock: 1, 2, 3, 19 (clock), 4 (USB mode), 9, 12, 13 (last battery voltage, mV: 4178
  on the unit). Registers 20 to 31 also hold values after stock (0x22, then 0x35a..0x3c5: not traced), so they
  are not free either. The bootloader has no RTC address in it.
- This firmware uses 5, 6, 7, 8, 10 (paired ANT sensor per kind: heart rate, speed, cadence, speed and cadence,
  power), 11 (wheel size) and 14, 15, 16 (ride page layouts), each behind a tag so an empty register reads as
  unset. This firmware keeps the paired heart-rate strap in register 5 (0x58004064). Writes
  need PWR_CR1.DBP, which reads set after the bootloader (seen on the unit); a write kept its value through a
  system reset (seen), and should through standby (backup domain, RM0399; to see).

## 9. Standby (power off)

- Wake-up pins **WKUP1 (PA0, power button)** and **WKUP4 (PC13, USB)**, rising, no pull.
- The power button drives both PA0 (high when pressed) and PA1 (low when pressed): seen on the unit (2026-10-23).
- PA0 must be a GPIO input before standby (stock does it at boot): left in its reset analog mode, the press does
  not wake the unit (seen on the unit, 2026-10-23); only the four-button restart brings it back.
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
- **Framing:** a 2-byte little-endian length, then the payload. No delimiter, no checksum, no handshake and no
  resync: after a bad frame, reset the chip. (SLIP is only the DFU bootloader's, at 115200 baud.)
- **Payload:** `[type][op][arguments, little endian]`. Types: 0 command, 1 response, 2 BLE event, 5 reset
  command, 6 ANT command, 7 ANT response, 8 ANT event. A response is `[1][op][result u32][outputs]`.
- **Seen on the unit:**
  - `sd_ble_version_get` (`00 65 01`) before enabling: result 0x3001 (BLE_ERROR_NOT_ENABLED);
  - `sd_ble_enable` (`00 60`): result 0;
  - `sd_ble_version_get` again: result 0, then `04 | 01 00 | 32 00`. Bryton fills these fields with its own
    versions, logged by stock as "Image 4, Bootloader 1, SD 50"; stock keeps the image number in RTC backup
    register 12, which reads 4 on this unit.
- **This firmware (src/nrf.rs, seen on the unit 2026-10-23):** the same reset sequence, with USART2 on the HSI
  kernel clock (64 MHz, BRR 64) rather than the bootloader's PLL3; then `sd_ble_enable` and `sd_ble_version_get`
  without any `sd_ble_cfg_set`, both answered (result 0, version 4 / 1 / 50).
- **Power off:** `sd_power_system_off` (`00 32`), no response; stock sends nothing else first and leaves PA6 /
  PI4 as they are. The next start's reset pulse wakes the chip (nRF52 pin reset; inferred, to see on the unit).
- **Scanning (API v5 layout, seen on the unit 2026-10-23):** `sd_ble_gap_scan_start` `00 8a 01 [flags]
  [interval u16] [window u16] [timeout u16]`, flags bit 0 = active, times in 0.625 ms units, timeout in s (0 =
  until `sd_ble_gap_scan_stop`, `00 8b`). Stock: active, interval 0x60, window 0x60 (or 0x30 to save power), no
  timeout. Reports: event 0x1d, `[2][1d 00][conn u16][peer addr: type byte, 6 bytes LSB first][direct addr 7]
  [rssi i8][flags][dlen][present][data]`; the name is AD type 0x09 / 0x08, often only in the scan response. Scan
  timeout: event 0x1b `[2][1b 00][conn u16][source]`. No `sd_ble_cfg_set` is needed before scanning.
- **ANT+ heart rate (seen on the unit 2026-10-23, `tools/nrf.py ant` and src/nrf.rs):** ANT commands are type 6,
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
    `01 d4 00 00 00 00 26 62 78 f1`, device 25126). Opening with that ID (`06 d3 00 26 62 78 f1`) takes only that
    strap (seen). Other ANT ops in stock's encoders: 0xc5 close, 0xc7 / 0xc8 broadcast / acknowledged data,
    0xd5, 0xd7, 0xdd, 0xdf, 0xe7 (not traced).
  - A BLE scan with the channel open (searching) still works: fewer reports, as the radio is shared.
- **ANT+ sensor channels, as stock opens them** (`Ant_OpenSensorChannel` 0xd025e7f0): channel, device type, period
  (1/32768 s): 0 heart rate 0x78 8070; 1 speed 0x7b 8118; 2 cadence 0x7a 16204; 3 speed and cadence 0x79 8086;
  4 power 0x0b 4091; then 5 0x22, 6 0x01, 7 0x28, 8 0x11, 9 0x02, 10 0x14, 11 0x23 (other ANT devices, not used
  here). All channels are assigned once at start (`06 c2 [ch] 00 00 00`); a channel is reconfigured only while
  closed. Close: `06 c5 [ch]`, answered `01 c5 [result]`, then an ANT event with event code 0x07 (channel closed)
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
- **Charge estimate:** a typical lithium-ion curve, 3.3 V = 0 % to 4.2 V = 100 % (inferred; stock's table is
  not traced). Below 3.3 V three readings in a row (10 s apart) on battery, the firmware switches off, as stock
  does at 0 %.
- **Charging:** PG14 is the charge enable, active low. The bootloader leaves it an output, low: **charging is on
  whenever USB power is present** (seen: MODER output, ODR bit 14 = 0). Stock switches it with a temperature
  window from the barometer: charge only if 0 < T < 50 C (5 < T after "too cold", T < 45 after "too hot"). This
  firmware does not drive PG14 yet: no temperature check, so charging below 0 C is possible. PF6: charger status
  input (reads high; meaning not traced).

