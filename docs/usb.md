# USB at high speed: the drops

High speed USB drops the link under sustained traffic on this unit, with this firmware and with Bryton's. Full speed holds.
This page has what is known, what was tried, and what to ask or try next. Last updated 2026-10-24.

## The hardware

- **MCU:** STM32H747 (Cortex-M7 at 480 MHz). USB1 **OTG_HS** (0x40040000) with an **external ULPI PHY**. The PHY's
  part number is not known yet (ghidra/HARDWARE.md, open questions).
- **ULPI pins, all AF10, very high speed, push-pull, no pull** (BOARD.md 4 and 7; the same as stock's HAL_PCD_MspInit
  0xd0298758):

  | Signal | Pin | Signal | Pin |
  |---|---|---|---|
  | CK (60 MHz from the PHY) | PA5 | STP | PC0 |
  | D0 | PA3 | NXT | PH4 |
  | D1 D2 D7 | PB0 PB1 PB5 | DIR | PI11 |
  | D3..D6 | PB10..PB13 | PHY enable / reset (high = running, inferred) | PA7 |

- **Bring-up (src/board/usb.rs `power_up`, as stock):**
  - VOS1;
  - HSI48 as the 48 MHz kernel clock (stock: PLL3Q; only the internal FS PHY needs it);
  - the ULPI pins;
  - OTG_HS ULPI and OTG_HS clocks;
  - PA7 high, then 200 ms;
  - an RCC reset of the core, then the core soft reset with timeouts.
- **Running it:**
  - polled from the main loop, no DMA;
  - the OTG_HS interrupt only wakes WFI, it is never taken;
  - CPU caches: I-cache on, D-cache off;
  - the code runs from SDRAM.
- **Stack:** usb-device 0.3, synopsys-usb-otg 0.5. A composite device: CDC ACM (the dev link) and Bulk-Only mass
  storage (src/msc.rs), 512-byte bulk packets at high speed.
- **FIFOs: the crate sizes them to the minimum.**
  - **This firmware:** the receive FIFO is the endpoints' total plus 30 words (about 300 words), and each transmit
    FIFO is one packet (128 words for a 512-byte bulk endpoint).
  - **Stock:** RX 0x200, TX0 0x80, TX1 0x200 words.
  - **ST's advice** for mass storage at high speed (community thread below): RX 0x200 and at least 0x100 for the bulk
    endpoint, double buffered.

## What was seen

| When | Firmware | Speed | Traffic | Result |
|---|---|---|---|---|
| 2026-10-22 | stock, its USB drive | high | copying files | the drive vanished every 80 to 90 s; macOS saw the device leave first |
| 2026-10-22 | Engineer Mode (bootloader) | high | 4 MB read | `bulk in: LIBUSB_ERROR_IO`, the device gone; a retry after a replug worked |
| 2026-10-23..24 | this firmware, dev link only | high | hours, 30 MB of sector dumps at about 0.25 MB/s | one answer line cut once (dev.py now asks again), no drop |
| 2026-10-24 | this firmware, USB drive | high | reading a 172 MB file at 6.4 to 7.2 MB/s | dropped after 15 MB, then after 40 MB; once right at the mount |
| 2026-10-24 | this firmware, USB drive | **full** | the same 172 MB file at 1.0 MB/s | **no drop** |

- **Cable and port:** an Apple cable about a year old, straight into the Mac, and another port of the Mac too. Not a
  hub.
- **After a high-speed drop:**
  - **The unit keeps running:** the clock ticks.
  - **The USB dot:**
    - first case: stayed green, so the core saw no bus reset;
    - later cases: orange, so a bus reset came (usb-device left Configured) and the Mac never configured the device
      again.
  - **A soft reconnect does not bring it back:** DCTL.SDIS set for 100 ms, then cleared (src/board/usb.rs
    `Link::watch`; counted in RTC backup register 31, which read 1 after one drop).
  - **Only an unplug brings it back:** this firmware resets the whole unit on unplug, PHY included (PA7 is driven
    again from reset).
- **What this suggests (inferred):**
  - The fault is on the unit's side: stock does it too, on a sound cable.
  - Traffic matters: low throughput at high speed held; 7 MB/s fails within seconds.
  - After the fault, the PHY or the core is in a state only a full reset clears.

## Ruled out or fixed

- **macOS refusing the mount** ("mount approval dissented, 0xF8DA0008") was its lock screen: it does not mount a newly
  inserted disk while locked. Not the unit.
- **The main loop stalling** after a drive command (fixed): the eMMC access slept, which takes over SysTick. This was
  not the drop: the USB link kept working through it.
- **The ULPI pins' slew rate:** already very high speed, as stock sets them.
- **A CBW not valid leaving the drive deaf after a bus reset:** fixed; a bus reset clears it now (msc.rs
  `Drive::bus_reset`).

## Next tries, cheapest first

1. **Read what the drive saw last.** A drop now stores the last command, its status and the resets in backup register
   0 (tag 0xd1; src/app/device.rs `usb_dropped`). After a replug: `./dev.py peek 0x58004050`. `./dev.py drivelog`
   gives the last 32 commands whenever the link is up.
2. **FIFOs as stock and ST size them:** RX 0x200 words, the bulk IN FIFOs 0x200 (double buffered). The crate
   recomputes them at every bus reset (`configure_all`), so this needs a crate option or a small patch.
3. **Reset the PHY on a drop:** PA7 low, then high, 200 ms, then a core soft reset and a new enumeration. This is what
   an unplug does, so it would recover without the cable. The crate keeps its state across, so the USB stack has to
   be rebuilt.
4. **The D-cache on**, or the USB code in ITCM: in slave mode the CPU fills the transmit FIFO word by word from
   uncached SDRAM code. A slow fill should only cost NAKs, but worth ruling out.
5. **The PHY's own state over ULPI** (its registers through the OTG_HS core's PHY access) right after a drop:
   line state, the function control register, any error or suspend bit.
6. **DMA mode** (OTG_HS internal DMA), as ST's examples for high speed do.
7. **An eye or a logic capture** on D+/D- and on the ULPI clock and data under traffic, if the hardware can be
   reached.

## Questions for the board's designer

- **Which ULPI PHY is it**, and what clocks it (its own crystal, the frequency)? Is its 60 MHz clock an output to
  PA5 (input clock mode)?
- **ULPI routing:** trace lengths and matching to the clock, series resistors, the reference plane. 60 MHz
  single data rate needs the data valid around the clock edge the STM32 samples on (RM0399, OTG_HS ULPI timing).
- **D+/D- routing:** 90 ohm differential, any common-mode choke or ESD part, and the connector (micro-USB?) with its
  shield and ground.
- **The PHY's supplies:** decoupling and the 1.8 V regulator, if any. A high-speed transmitter draws more current
  than full speed: a supply sagging under sustained traffic would fit what was seen.
- **Known errata** for this STM32 (ES0445 for the H745/H747/H755/H757) on OTG_HS with a ULPI PHY in device mode. The
  closest found, ES0392 2.25.2 for the H743 (transmit FIFO corrupted when a FIFO write is interleaved with accesses to
  certain OTG_HS registers), should not apply here: this firmware polls from one loop and never interleaves them.
  Stock uses interrupts, so it could.
- **PA7:** is it the PHY's reset or its chip select / enable, and how long does the PHY need after it?

## Meanwhile

- **The installed build runs USB at full speed** (built with `USB_FS=1`): about 1 MB/s for the drive. The dev link
  and `dev.py run` work the same.
- **A high-speed build** is the default `cargo build` (no `USB_FS`), kept for these tests.

Sources:
- RM0399 (STM32H745/747/755/757 reference manual), OTG_HS chapter.
- ES0392 (STM32H742/743/750/753 errata), section 2.25:
  https://www.st.com.cn/resource/en/errata_sheet/es0392-stm32h742xig-stm32h743xig-stm32h750xb-stm32h753xi-device-errata-stmicroelectronics.pdf
- ST community, mass storage on an H723 with a USB3300 PHY (FIFO sizes, the 48 MHz clock):
  https://community.st.com/stm32-mcus-products-25/stm32h723-usb-mass-storage-131452
- The patch repo's recovery notes (bryton-S510-mods docs/recovery.md, "If the USB link drops").
