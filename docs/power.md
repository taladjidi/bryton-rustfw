# Power: what the peripherals draw, what was changed, how to measure it

With the CPU awake about 1.3 % of the time on a ride page (docs/status.md, `tools/profile.py`), the rest of the unit
dominates the battery. This page estimates each part's current from its data sheet, lists the options, says what
this firmware does, and how the owner can measure the difference. **Every mA figure here is estimated** (from data
sheets, at their test conditions), none was measured on the unit.

## The battery

- **Capacity: unknown.** Not in BOARD.md, the patch repository or the Ghidra notes, and retailers give only the
  claim: "up to 30 hours" (bike24.com, sigmasports.com listings for the S510). The cell is read through a 1/1.5
  divider on ADC3 (BOARD.md 12), the charge estimated from its voltage.
- **So the effect of a saving is given as a share of the total draw.** With `I` the unit's average current and 30 h
  the claim at that draw, a saving `dI` gives `30 x I / (I - dI)` hours. Example: if the unit draws 40 mA (then the
  cell is about 1200 mAh), 4.5 mA saved is 33.8 h instead of 30, 10 mA saved is 40 h.
- **The total is unknown** because the largest part is probably the STM32H747 itself: in WFI sleep its core clock
  and the SDRAM keep running (16.5 to 47 mA for the MCU, DS12930 table 44, plus the SDRAM's). Stop mode between
  passes is the plan below ("Stop mode: plan").

## Today, per part (a ride page outdoors in daylight, a phone connected, heart rate and power paired)

| Part | Today | Options | Saving | Done here |
|------|-------|---------|--------|-----------|
| GNSS (u-blox M10) | 11.8 mA: continuous, 9.5 mA at VCC + 2.3 at V_IO, GPS + Galileo + BeiDou B1I | cyclic tracking (PSMCT) | about 4.8 mA (4.5 at VCC, 0.3 at V_IO) while in it | GPS POWER AUTO (default): while riding fast and straight |
| | | GPS + Galileo only | 1.6 mA | owner's decision |
| | | GPS only | 2.2 mA | no (fewer satellites under trees) |
| | | off between rides once idle | 11.8 mA while idle | yes: 5 min without a fix, 10 min with one and no key |
| Nordic, ANT+ search for an absent sensor | about 2.2 mA each (40 s in 100 at 5.4 mA) | longer rests | about 1.4 mA each | rests of 1, 2, then 4 min |
| Nordic, ANT+ channel tracking a sensor | 30 to 60 uA each at 4 Hz | 2 Hz for heart rate | about 20 uA | no (beats lost for HRV) |
| Nordic, BLE to the phone | 20 to 40 uA connected, 10 to 15 uA advertising each second | longer slave latency | about 10 uA | no (already low) |
| Nordic, its own idle (UART listening) | 0.3 to 1 mA (inferred) | none from the STM32 side | | |
| Backlight | 0.5 mA at 1 % (AUTO in daylight); about 60 mA at 100 % | lower level, OFF AFTER | up to 60 mA | owner's settings |
| Panel (ST7789) | 4 to 7 mA | lower frame rate (FRCTRL2) | 1 to 2 mA (inferred) | no (flicker: needs the unit) |
| Light sensor, barometer | about 0.1 mA, 0.01 mA | | | |

### GNSS receiver

- **What it is:** a u-blox M10, ROM SPG 5.10 (BOARD.md 13). The figures are the MAX-M10S module's (the board's part
  is not known: module or chip, inferred to be close). Which constellations it uses here is not read back: the
  default below, unless stock's configuration survives in it (BOARD.md 13); `tools/gnss.py` with a CFG-VALGET of
  CFG-SIGNAL-* would say.
- **Currents** (MAX-M10S data sheet UBX-20035208 R08, 4.3, table 15: 3.0 V, 1 Hz, 25 C, SBAS and QZSS on, mA):

      constellations            acquisition   tracking   power save (cyclic)   V_IO
      GPS                       8             7.5        4.5                   2.1 (2 in power save)
      GPS + GAL                 10            8          5                     2.2
      GPS + GAL + GLO           12            9          5                     2.3
      GPS + GAL + BDS B1I *     11.5          9.5        5                     2.3
      GPS + GAL + BDS B1C       11            8.5        (not supported)       2.2
      GPS + GAL + BDS B1C + GLO 13            10         (not supported)       2.3
      * the default ("concurrent reception of GPS, Galileo, and BeiDou B1I with QZSS and SBAS enabled", 1.3)

  Backup: 28 uA (hardware backup, V_BCKP only), 46 uA at V_IO in software standby (table 17).
- **Power modes** (u-blox M10 SPG 5.10 interface description UBX-21035062 R02, 4.9.15): CFG-PM-OPERATEMODE
  (0x20d00001) FULL 0, PSMOO 1 (on / off, a fix every CFG-PM-POSUPDATEPERIOD, at least 5 s), PSMCT 2 (cyclic
  tracking: the receiver tracks, then sleeps within each update period); CFG-PM-UPDATEEPH (default on) wakes it to
  refresh ephemeris. The MAX-M10S integration manual (UBX-20053088, 2.3) describes continuous mode and the backup
  modes; the data sheet's "power save mode" figures are cyclic tracking (its note 28).
- **What it costs** (u-blox SAM-M10Q integration manual UBX-22020019 R01, 2.6.2, the M10 platform's power save mode):
  PSMCT keeps the navigation output at 1 Hz ("In the POT state the receiver continues to output position fixes
  according to the CFG-RATE-*"; 1 and 2 Hz supported), but in its power optimized tracking (POT) state "no new
  signals are acquired and no data is downloaded" (it goes back to continuous for the ephemeris it schedules), SBAS
  (EGNOS) is neither downloaded nor used in power save mode, and a weak or lost signal sends it back to tracking. u-blox
  gives no accuracy figure for it; speed comes from Doppler at each fix either way, so a steady speed should change
  little (inferred), while a satellite lost under trees or in a turn is not replaced until the receiver leaves POT,
  and the position loses SBAS's corrections. Hence AUTO takes it only where that matters least, and conservatively.
- **What stock does** (Ghidra, ghidra/notes/agent_d01e8000.md, agent_b1.md): a GPS setting `gpsPW` 0..5
  (`Gps_PowerIndexToModeCmd` 0xd01f34a8) mapped to modes off, FULL ON, "trickle power" (power save), GLONASS and
  BeiDou (`Gps_HandleCommand` 0xd0252e38, `Gps_SwitchConstellationMode` 0xd01f0d40), each a group of UBX
  configuration commands (`Gps_SendConfigGroup` 0xd01eea40: "Config_Rate", "Enable_GAL", "Set_BDS_Signal"). In its
  power save mode, `Gps_AutoSelectConstellationGroup` 0xd0254ab0 switches to another group ("Group 3 bike others")
  after about 10 minutes riding with a 3D fix, an accuracy under 5 m and 8 satellites or more, five checks in a row.
  The commands in each group are not traced.
- **This firmware: GPS POWER AUTO (the default, owner 2026-10-27), FULL or SAVER** (BIKE > GPS POWER). FULL is
  continuous tracking, SAVER cyclic tracking always: CFG-PM-OPERATEMODE in the RAM layer with the rest of the
  configuration, at each power-up and again at each change (`gnss::config`; CFG-VALSET to RAM applies at once).
  **AUTO** (`gnss::Adaptive`, one decision per fix) uses cyclic tracking only while all of these held for a minute in a
  row (`AUTO_STEADY_S`), and goes back to continuous at the first fix where one fails:
  - a ride running (between rides, paused or stopped: continuous);
  - 10 minutes of good 3D fixes since the last loss (`AUTO_WARM_S`), 8 satellites or more, horizontal accuracy under
    5 m: stock's own conditions for its power save mode's switch (`Gps_AutoSelectConstellationGroup` 0xd0254ab0), and
    the receiver has gathered its satellites and their data by then (the manual: more signals tracked first help it
    stay in POT);
  - faster than 20 km/h to enter, and it stays down to 15 km/h (`AUTO_FAST_MM_S`, `AUTO_SLOW_MM_S`): MTB, technical
    ground and steep climbs are under that, where the owner wants the resolution; the gap is the hysteresis;
  - no turn: the heading within 30 degrees of each of the last 5 s (`AUTO_TURN`): a road bend at 20 km/h that turns
    30 degrees in 5 s has a 53 m radius; hairpins, roundabouts and switchbacks turn more.
  So a switch to cyclic tracking comes at most once a minute (no flapping), and a switch back within a second.
  **Expected saving:** 4.8 mA while in cyclic tracking (the table above), so about 2.5 to 3.5 mA averaged on a road
  ride that spends 50 to 70 % of its time fast and straight past the first 10 minutes (an estimate, to measure), next
  to nothing on an MTB ride, which stays continuous. SAVER saves the 4.8 mA all the time, at the cost above.
- **Off when idle (owner, 2026-10-27; app/gps.rs `gnss_idle`):** with no ride open, the receiver's supply is cut
  (as for an indoor ride, BOARD.md 13) once it searched 5 minutes since it was switched on without a fix
  (`GNSS_SEARCH_MS`: a start outdoors takes about half a minute, a few minutes at worst by a window or under trees;
  the search costs about 1 mAh indoors), or, with a fix, after 10 minutes without a key, a touch or a screen that
  needs a position (`GNSS_IDLE_MS`). Keys alone do not wake it (indoors they would restart the search); it is on
  again at Start (any ride; the climbs ahead start with it), for a page with a map, the GPS menu, or a course followed,
  with the same sequencing as at boot (powered 1 s later, the configuration, the time and the last position sent).
  **Fix after switching on again:** the supply cut loses the ephemeris (no V_BCKP known), so it is a warm start helped
  by the time and position: the ephemeris takes 18 to 36 s to download under a strong signal (UBX-22020019, 2.4),
  so expect about 30 s outdoors, not the 1 to 2 s of a hot start; a hot start would need the backup domain kept
  (UBX-RXM-PMREQ software standby, 46 uA, its wake on the unit not tried). **Saving:** 11.8 mA for every hour the
  unit sits on without a ride past those minutes, about 12 mAh an hour left on indoors.
- **Left to the owner:** GPS + Galileo only (CFG-SIGNAL-BDS_ENA 0x10310022 off: 1.6 mA in continuous mode, nothing
  in cyclic tracking, which has the same 5 mA either way).

### Nordic chip (BLE and ANT+)

- **Figures** (nRF52832 product specification v1.4, radio current with the DC/DC converter at 3 V: receive 5.4 mA,
  transmit 0 dBm 5.3 mA; recalled, inferred: the chip's exact model is not known, BOARD.md 11). A listening window
  costs its length at about 5.4 mA, plus the radio's start-up.
- **ANT+ search:** while a channel searches the radio listens all the time (ANT Message Protocol and Usage, rev 5.1,
  search and search time-outs: the high-priority search, 10 s by default, then the low-priority one, which this
  firmware sets to 30 s, `nrf::SEARCH_TIMEOUT`; recalled). Stock searches for ever (0xff, BOARD.md 11).
  - Before: 40 s of search, then 60 s closed: about 40 % of 5.4 mA, 2.2 mA, per paired sensor that is absent (a
    power meter on the other bike, a strap in the drawer).
  - Now (app/radio.rs `search_rest_ms`): the rest doubles after each search in a row that finds nothing, 1, 2, then
    4 minutes: 40 s in 280, about 0.8 mA. A ride's Start, the sensor heard or the rider's action in SENSORS start
    again from a minute, so a strap put on before Start is found at once; one put on mid-ride after a long absence
    waits up to 4 minutes.
- **ANT+ channels tracking:** one receive window per message period (4 Hz for heart rate, 8 Hz for power: stock's
  periods), about 30 to 60 uA each (estimated). Heart rate at 2 Hz would save about 20 uA but loses beats the HRV
  needs when a message is missed.
- **BLE:** advertising only while pairing (152.5 ms for 60 s) or bonded and not connected (1022.5 ms), connected at
  15 to 30 ms with slave latency 6 (app/phone.rs): 10 to 40 uA (Nordic's online power profiler figures quoted
  there, inferred). A slave latency of 30 would save about 10 uA and slow the phone's notifications by up to a
  second: not worth it. The trainer's link (central, 30 to 60 ms, no latency) runs only while one is paired.
- **The chip's own idle:** its UART (1 Mbaud, RTS / CTS) keeps listening for the STM32's commands (the nRF5 SDK
  serialization's transport), so the high-frequency clock may stay on: 0.3 to 1 mA, inferred. Nothing to change
  from this side without the Nordic image's source.

### Backlight and panel

- **Backlight:** PG13, LPTIM1 PWM on the LSE (32768 Hz / 100: 328 Hz), 1 to 99 % (BOARD.md 6). The LED current is
  not known (no panel data sheet); a 2.8-inch TFT's backlight is typically 3 or 4 white LEDs at 15 to 20 mA, so
  about 60 mA at 100 % (inferred), and the average follows the duty: 1 % 0.6 mA, 8 % 5 mA, 40 % 24 mA, 60 % 36 mA.
  - **Decided (owner, 2026-10-27):** a fresh unit (no saved settings) starts with AUTO and OFF AFTER 30 S
    (`Settings::from_word`'s defaults), about 60 mA less than the 100 % never off it had before; saved settings and
    stock's imported ones keep theirs.
  - **AUTO is stock's curve exactly (owner, 2026-10-27):** its setting 1 (BOARD.md 16, `Sensor_LightSensorUpdateBacklight`
    0xd023e304): 8 % up to 5 lux, 20 % at dusk (5 to 20 lux, was 40 %: about 12 mA less), 30 % to 50 lux (was 50 %),
    the 1 % minimum above 50 lux (was 70 % from 50 to 120 lux: about 40 mA less in a lit room), and stock's timing: a
    band chosen once a second, no hysteresis.
  - **Left to the owner:** a ride's OFF AFTER (10, 30, 60 s: at night the backlight is the largest draw by far).
- **Panel (ST7789):** 4 to 7 mA in normal mode (secondary sources on the ST7789V; Sitronix's own table not read).
  Its idle mode (8 colours) and partial mode would change what is shown. A lower frame rate (FRCTRL2, 0xC6: stock's
  0x0F is 60 Hz, 0x1F is 39 Hz) saves part of the driver's current but may flicker: to try on the unit, not done.
  The panel stays on with the backlight off: the transflective panel is read by daylight.
- **Light sensor** (LTR-308, measuring every 100 ms while AUTO is on) and **barometer** (LPS22HH, one reading a second
  while riding): about 0.1 mA and 0.01 mA (recalled from their data sheets): nothing to gain.

- **Measured 2026-10-28: 12.2 h idle on the overview cost about 41 % of the battery.** So SYSTEM > AUTO OFF (OFF, 5,
  10, 30, 60 min; default 5) switches the unit off after that long with no ride open, no cable, no key or touch, the
  drive not lent and no write waiting; a banner and beep warn 10 s and 5 s before, any input cancels (`step_auto_off`,
  kept in register 18 bits 22..24; an older build rewriting that register resets it to 5 min).

## The main loop while idle (2026-10-28)

- **Measured overnight before** (owner, on battery, the overview, backlight off): 12.2 h cost about 41 % of the
  charge, about 3.4 % an hour; the CPU awake 0.8 % of the time, 20 passes a second of 0.4 ms each. So the CPU's work
  is not what drains the cell: the floor is the sleep itself (WFI with the clocks and the SDRAM running, the panel,
  the Nordic chip).
- **Done (lib `App::pass_ms`, main.rs's sleep):** with no ride or its card, no key for 30 s (`IDLE_AFTER_MS`, the
  default OFF AFTER), no finger, no tone or banner timed by the passes and the card not lent to the computer, a pass
  every 250 ms (`IDLE_POLL_MS`) instead of 50. Nothing waits for it: the touch INT (as before), a key's falling edge
  (EXTI 1 to 4, BOARD.md 5) and a byte from the Nordic chip (BOARD.md 11) end the sleep at once. A pending IRQ ends
  WFI with PRIMASK set (ARMv7-M ARM B1.5.19: WFI completes on any exception that would be taken were PRIMASK clear),
  so no handler is needed, as for the touch line. The timers count each pass's real length (`App::dt_ms`) already.
  It applies with the cable in too: USB's own interrupt ends the sleep for its traffic (no SOF interrupt is unmasked),
  which is also how it can be measured.
- **Measured on the unit** (`dev.py run` from RAM, the overview, a phone connected, the GPS off after its search,
  `dev.py prof` over 3 minutes): before, 21.2 passes a second of 0.44 ms, awake 0.99 %; after, 6.7 passes a second
  of 0.73 ms, awake 0.52 %. A pass costs more because the work is per second more than per pass (the second's tick,
  the radio's events); the Nordic chip's bytes wake the loop about 2.7 times a second, and the radio is now the
  largest part (0.23 %). Half the CPU's awake time, which is a fraction of a mA (0.5 % of the difference between
  Run and Sleep currents): the loop Stop mode needs (a long sleep that wakes on what matters) more than a saving in
  itself.

## Stop mode: plan (design, not built)

**Why.** In WFI (CSleep) the STM32H747 draws 16.5 mA at VOS3, 200 MHz with every peripheral clock off and 47.1 mA
with all on (DS12930 Rev 1, table 44, LDO, typical at 25 C); this firmware keeps FMC, SDMMC1, the RAMs, USART2,
LPTIM1, TIM1, SYSCFG and the RTC clocked in sleep (BOARD.md 1), so somewhere between. With D1, D2 and D3 in Stop
it draws 2.78 mA at SVOS3, 1.96 at SVOS4, 1.27 at SVOS5 (flash off, no IWDG; table 42). The SDRAM is the other
part: in sleep the FMC clocks it at 50 MHz with CKE high (its standby current, tens of mA for an SDR SDRAM, inferred:
the part is not known), in self-refresh it keeps its data on its own at about 1 mA or less (mobile SDR, inferred).
**Expected saving: 15 to 45 mA of the MCU and some of the SDRAM's, between passes**, so the idle drain could fall
to a half or a third (an estimate; the cell's capacity and the unit's total current are not known: the battery
history before and after is the measure).

**Can it run with the image in SDRAM?** Not as is: in Stop the FMC has no clock, and the SDRAM must be put in
self-refresh first (FMC_SDCMR MODE = 101, bank 2; RM0399 FMC, "Self-refresh mode"), and nothing may fetch from it
until it is back in normal mode with the clocks restored. So the entry, the WFI and the wake path run from ITCM
(as the RAM-run trampoline does, link.x `.itcm`, 4 KB, copied at reset), with the stack in DTCM (retained in Stop)
and interrupts masked (a fault there would fetch its vector from the SDRAM: none may happen, so no indexing, no
calls out of ITCM). The I-cache keeps its lines in Stop (D1 logic retained); the D-cache is off.

**The sequence** (one hw.rs primitive, `stop(wake)`, in `.itcm`):
1. Before: no DMA running (`PanelDma` idle), no tone (TIM1), no USB (VBUS low: USB needs VOS1 and its clocks, so no
   Stop while the cable is in), the eMMC idle. RTS to the Nordic chip held high as a GPIO (PD4), so it keeps its bytes
   rather than send into a stopped USART (what RTS does when USART2 is unclocked: inferred).
2. FMC: wait until not busy, self-refresh command, wait FMC_SDSR MODES2 = self-refresh.
3. PWR_CPUCR PDDS_D1 / D2 / D3 = 0 (Stop, not Standby), PWR_CR1 SVOS (SVOS3 keeps the D3 peripherals that can wake
   the system running; SVOS5 is lower and still wakes on EXTI and the RTC: DS12930 3.5.3), RCC_CFGR STOPWUCK = HSI
   (the wake clock), SCB_SCR.SLEEPDEEP = 1, DSB, WFI. On the dual-core H747, D3 is shared: it stops only when
   neither CPU keeps it running, so CPU2's own register, PWR_CPU2CR (PDDS_D3 = 0, RUN_D3 = 0), must allow it too
   (RM0399 PWR, "Power management", the domain and system low-power modes and PWR_CPU2CR; the M4 never started here
   leaves it at reset, inferred). Read back before the first WFI.
4. Wake: HSI at once (tWUSTOP 4.4 us typical at VOS3, 39 us at VOS5, table 49); the regulator back at the Run voltage
   before any higher clock: wait PWR_D3CR.VOSRDY and PWR_CSR1.ACTVOSRDY (RM0399 PWR, "Voltage scaling" and the exit
   from Stop: from SVOS4 / SVOS5 the Run VOS is restored by hardware, and 200 MHz needs it there); HSE on and ready
   (tSU(HSE) 2 ms, table 52: the longest step, so PLL1 from HSI instead is worth trying), PLL1 on and locked (tLOCK 50
   us typical, 150 max), SYSCLK back to PLL1P (200 MHz, the bus dividers are kept), SLEEPDEEP = 0.
5. FMC: normal mode command, wait MODES2 = normal; RTS back to its alternate function; return to the SDRAM code.
6. PWR_CPUCR.STOPF says whether the system really was in Stop (it is also what a DIAG stage reads).

**Wake sources.**
- The pass's timer: the RTC wakeup timer (RTC_WUTR, from RTCCLK / 16: 0.49 ms steps), its EXTI line (19 on this
  family, recalled: to check in RM0399's EXTI event input table) unmasked for the CPU; or LPTIM2 on the LSE.
- Keys (EXTI 1 to 4), the touch INT (EXTI 6), VBUS (PC13, EXTI 13, rising: today it is read each pass, which a
  250 ms pass still does), the power key (WKUP1, EXTI 55, as standby): configurable EXTI lines wake from Stop at any
  SVOS (DS12930 3.5.3, "wakeup ... through GPIO or asynchronous interrupt").
- The Nordic chip: USART2 can wake from Stop (DS12930 3.32: start bit, received frame, FIFO status) with UESM and its
  kernel clock HSI, but only at SVOS3, and at 1 Mbaud the first bytes arrive within 10 us of the start bit, so
  whether the wake loses one is to see on the unit. Stock uses PH8, the chip's request line, as an EXTI wake-up
  (BOARD.md 11): with RTS held high, PH8 (what it signals is to trace in Ghidra) or the next pass reads them.
- The GNSS receiver (SPI4, polled) and the I2C sensors keep their data until read: nothing to wake for.

**What breaks.**
- SysTick stops with the core clock: the sleep's length must come from the RTC (RTC_SSR sub-seconds, 1/256 s with
  PREDIV_S 255) or LPTIM2's counter, for `Board::last_sleep`, the uptime and the profiler's `slept_ms`. `delay_ms`
  stays SysTick and WFI (CSleep): Stop is only for the sleep between passes.
- DWT CYCCNT stops (it already stops in WFI), so awake time still counts work only; the wake (about 2 ms with HSE)
  becomes a profiler section of its own, `wake`, or it looks like the step's.
- The backlight's PWM (LPTIM1 on the LSE) runs on in Stop (DS12930 3.29.5: an LPTIM clocked by the LSE runs in Stop);
  TIM1 (buzzer) does not, hence no Stop while a tone plays (`App::pass_ms` is fast then already).
- The dev link: none (no Stop with the cable in), so Stop is measured by its breadcrumbs and the battery history.
- The M4 core is never started (BOARD.md 1): D2 enters DStop when no running CPU holds its peripherals (RM0399 PWR,
  domain states); whether the bootloader leaves the M4 gated so it does is what STOPF answers.

**On the unit, in order** (each a step a review can stop at; AGENTS.md: a new step that touches the core gets a
DIAG stage before it is trusted):
1. **Self-refresh alone, no Stop:** a dev link command runs the ITCM routine with only steps 2 and 5 around a
   CSleep WFI of 100 ms, then the image's CRC is checked against the one at boot. With the cable in, from RAM
   (`dev.py run`): the first step, and nothing written to the eMMC.
2. **One Stop of a second, from RAM:** the same command disconnects USB, enters Stop with the RTC wakeup timer,
   restores the clocks and the SDRAM, writes STOPF, PWR_CPU2CR, the cycles waited for VOSRDY / ACTVOSRDY, the image's
   CRC and the wake time to backup registers, then a system reset (the RAM image is gone, the installed firmware
   starts with the USB link up): `dev.py peek` of the register says whether it worked. Nothing installed, and a hang
   ends at the four-button restart.
3. **Stop between passes, on battery:** a `DIAG_STOP` build installed with the owner present, the breadcrumbs counting
   Stop entries and returns (as `STANDBY_BKP`), and the battery history before and after (this page's plan).

**hw.rs would gain** one primitive: `stop(wake_ms) -> StopReport`, in `.itcm`, with its own volatile accesses
inlined (no call into SDRAM code), its `// SAFETY:` saying why no SDRAM access can happen between the self-refresh
command and its exit (interrupts masked, no fault path, everything it touches in ITCM, DTCM or peripherals). The RTC
wakeup timer, the EXTI lines and RTS as a GPIO are ordinary register writes in main.rs.

## Measurement plan (for the owner)

The unit must run on its battery: USB power charges it (BOARD.md 12) and unplugging resets it, so a build run from
RAM (`dev.py run`) is gone once unplugged. The A / B switches are therefore settings in the installed build: GPS
POWER (FULL / SAVER), BRIGHTNESS and OFF AFTER, a sensor paired but absent (A) or forgotten (B).

- **What is read:** the battery history, one smoothed reading a minute, the last 4 hours, in RAM (`dev.py bat`). It
  survives plugging the cable back in (plugging does not reset the unit; the readings taken on the cable are flagged
  charging), not a power off.
- **One run:**
  1. Charge to full, unplug (the unit restarts), wait 20 minutes (the voltage relaxes after the charge).
  2. Set the case (for example GPS POWER FULL), open the same ride page, outdoors or at a window so the GPS has a
     fix, backlight AUTO. A ride may run (Start) so the receiver, the sensors and the page behave as on a ride.
  3. Leave it 4 hours, then plug in and run `dev.py bat > runA.txt` at once.
  4. Charge to full again and do the same with case B.
- **How long:** the voltage-based reading moves about 10 to 20 mV an hour at 40 mA (inferred from a typical Li-ion
  curve); its noise after the smoothing is a few mV. A 4-hour run from the same start resolves a difference of about
  10 % of the total current: the backlight (100 % against AUTO) shows within an hour; GPS POWER (4.8 mA) and an absent
  sensor (1.4 mA) need the full 4 hours each, twice (A, B, A, B on four charges) to trust the sign.
- **Whole-battery runs** give hours directly: set the case, leave the unit on a ride page until it switches itself
  off at 3.3 V (BOARD.md 12), and note when it is found off (a look every hour or so; the battery history is lost
  with the power off). Two runs (FULL, SAVER) of a day
  each are the clearest answer to "how many hours".
- **Better, if the unit is ever open:** a current meter in series with the cell (or a bench supply on its contacts)
  measures each case in minutes.
