# USB at high speed: the drops

High speed USB drops the link under sustained traffic on this unit, with this firmware and with Bryton's. Full speed holds.
This page has what is known, what was tried, and what to ask or try next. Last updated 2026-10-27.

## The hardware

- **MCU:** STM32H747 (Cortex-M7 at 200 MHz as the bootloader leaves it; the part allows 480). USB1 **OTG_HS** (0x40040000) with an **external ULPI PHY**. The PHY's
  part number is not known yet (bryton-S510-mods, ghidra/HARDWARE.md, open questions).
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
  storage (src/link/msc.rs), 512-byte bulk packets at high speed.
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

## What the unit saw at a drop (2026-10-24)

Backup register 0 after a high-speed drop at the mount, read after the replug: `0xd1280200`.
- **Last command:** 0x28, READ(10), with status 2: a **phase error**. The data phase could not hand the next 512-byte
  packet to the IN endpoint for a whole second (`Bulk::send` gave up), so the computer had stopped taking data in
  the middle of a transfer from the unit.
- **Resets:** none of either kind (0xff bus reset, 0xfe Bulk-Only reset) among the last 32 entries. The computer did
  not reset the device first.
- **Then:** the core left Configured (the dot orange, most likely USB suspend: no more starts of frame), and the
  watch reconnected (register 31: 2 drops in all).
- **Reading (inferred):** the computer disabled the port without a reset. That is what a host controller does after
  transaction errors (babble, a bad CRC) on an IN transfer. Only fast IN traffic fails: a damaged high-speed packet
  from the device's side is the likeliest. Candidates: the transmit FIFO (sized to one packet, filled word by word
  by uncached code), the ULPI data timing at 60 MHz, the PHY's transmitter or supply. Next tries 2, 4, 5 and 7 below
  target exactly that.

## Suspend: a normal state, not a drop (2026-10-27)

- **What a suspend is:** the computer stops sending starts of frame; after 3 ms of an idle bus the core sets
  OTG_GINTSTS.USBSUSP and OTG_DSTS.SUSPSTS (USB 2.0 7.1.7.6; RM0399, OTG_HS, OTG_GINTSTS and OTG_DSTS). The computer
  ends it with resume signalling (OTG_GINTSTS.WKUPINT) or a bus reset. synopsys-usb-otg 0.5 reports USBSUSP and
  WKUPINT as `PollResult::Suspend` / `Resume` and does nothing else (its `suspend` / `resume` are empty); usb-device
  0.3 goes to `UsbDeviceState::Suspend` and back to the state before at the next resume, reset or traffic.
- **Before:** `Suspend` is not `Configured`, so after 2 s the watch called it a drop: a soft disconnect (DCTL.SDIS,
  100 ms of busy-waiting), "USB RECONNECT" on the screen, register 31 counted one more drop, and register 0's drive
  record was overwritten. Once per suspend: the watch re-armed only once configured again.
- **Now** (lib link/dev.rs `watch_usb`, board/usb.rs `Link::watch`, tests in dev.rs): a suspend from configured is
  left alone however long it lasts, the dot turns grey, and the link carries on as configured at the resume. Nothing
  is polled in between: no USB interrupt comes, so WFI sleeps whole slices. Its timers run on the main loop's clock
  (they ran on the cycle counter, which stops in WFI and wraps every 9 s).
- **What is still a drop:**
  - configured and the frame number (DSTS.FNSOF) still for 200 ms with no suspend (the green-dot drop);
  - configured once, then not configured again for 2 s (a reset the computer did not follow up);
  - a suspend after the drive's last command ended in a phase error: a computer suspends a device only when it is
    idle, never in the middle of a transfer, so that is the port disabled (the 2026-10-24 drop above), and the old
    recovery runs.
- **On the unit:** a suspend could not be provoked without sleeping the Mac (not allowed). With the serial port closed
  and the drive not lent, macOS kept the device powered (ioreg: IOUSBHostDevice DevicePowerState 2, over 2
  minutes): it does not suspend this device while awake. The build ran from RAM with the link up, a green dot, cpu 8
  permille. What a real suspend does is in docs/status.md, "Not yet seen on the unit".

**Do suspends explain the past issues?** Mostly not.
- **Do not fit:** the drops came during sustained high-speed transfers, and a computer never suspends a device with
  a transfer in flight. They depend on speed (full speed held), which fits signal integrity, not power management.
  Stock's drive (every 80 to 90 s while copying) and Engineer Mode's `LIBUSB_ERROR_IO` are other firmwares, without
  this watch. The green-dot drop had no suspend: a suspend would have set USBSUSP and left Configured. Only an unplug
  brought the link back, while a suspended device comes back with the next traffic.
- **Fits, as an effect:** register 0 at the 2026-10-24 drop (`0xd1280200`: a READ(10) phase error, no reset, then the
  core left Configured) is what the core reports as a suspend: the port went idle, but after the transfer failed.
- **Fits, separately:** while the Mac slept with the unit plugged in (a system sleep suspends every port), the old
  watch disconnected and reconnected the unit 2 s in; on waking, macOS found a device that had left and come back,
  so it enumerated it afresh (the serial port gone and back, a lent drive "not ejected properly"), and the unit
  showed "USB RECONNECT". Register 31 counted those as
  drops and register 0 could hold a sleep's record instead of a real drop's (status 0 in it means no failed command).

## Ruled out or fixed

- **A main loop that never slept with the cable in** (2026-10-27, fixed; not USB): the profiler showed the `usb`
  section 255,000 times a second (3.2 us each, 82 % of wall time) instead of about 42. It looked like a suspended
  bus, but DSTS.SUSPSTS read 0 and the spin went on with GINTMSK = 0 and with GAHBCFG.GINT = 0. The NVIC (peeked
  with a debug build) held IRQ 23, the touch line's EXTI9_5, pending (ISPR0 0x00800400, ICSR.VECTPENDING 39) with
  EXTI_CPUPR1 line 6 clear: the NVIC clear had landed before the EXTI write, the line pended it again, and nothing
  cleared it any more. The usb row only showed it because the link is polled once per WFI; without the cable the
  loop spins just the same, unmeasured. Timing decides it: about three runs in four of some builds, none of
  others (adding a store after WFI hid it; `dsb` / `isb` around WFI did not). Fix, first step: main.rs `touch_edge` reads CPUPR1 back after
  clearing it and clears the NVIC pending bit on every call: 0 spins in 12 runs of two layouts that spun before
  (superseded: `sleep` now clears the NVIC pending bits, next paragraph).
  **The same again with the panel's DMA** (2026-10-27, after c5465b0): the cpu figure at 1006 permille, ISPR0 0x00000c00,
  ICSR.VECTPENDING 27 = IRQ 11 (DMA1 stream 0) with DMA1_LISR 0: `dma_wait` cleared the NVIC right after LIFCR, the
  line re-pended it, and nothing cleared it until the next transfer. 0 in 10 runs of the build at e923b23, but 5 in 6
  with LISR's read back removed. Fix, for every wake source: main.rs `sleep` clears the pending bits of all three
  (IRQ 11, 23, 77) just before each WFI, after the sources were read; the lines are level, so an unread event pends
  again at once. The sources no longer clear their own NVIC bit. With the read back removed: 0 spins in 10 runs; the
  build as committed: 0 in 20. `dev.py peek` now reads the NVIC (0xe000e100 to 0xe000e33c) and ICSR (0xe000ed04).
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
