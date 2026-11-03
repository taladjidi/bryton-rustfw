# Power: what the peripherals draw, what was changed, how to measure it

With the CPU awake about 0.5 % of the time idle on the overview and about 1.3 % on a ride page (docs/status.md, `tools/profile.py`), the rest of the unit
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
| | | GPS + Galileo only | 1.6 mA | BIKE > GNSS GPS+GAL (the owner's choice) |
| | | GPS only | 2.2 mA | BIKE > GNSS GPS (not the default: fewer satellites under trees) |
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
- **Switched off:** System OFF, 0.3 uA (nRF52832 PS v1.4); held in reset it would draw about 0.2 mA and more, so the
  switch-off makes sure of System OFF and leaves PA6 released (BOARD.md 11 "Power off"). No pin is known to cut the
  chip's supply. A chip that will not go off after a reset and a second ask is held in reset: about 0.2 mA for as long
  as the unit is off (a month: about 150 mAh), against 0.3 mA to several mA for a hung chip left running. Only a failing
  chip costs this; seen on the unit, a healthy one goes off at the first ask.

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
- **Light sensor** (LTR-308, measuring while AUTO is on, read every 500 ms; every 5 s for THEME AUTO alone) and **barometer** (LPS22HH, one reading a second
  while riding): about 0.1 mA and 0.01 mA (recalled from their data sheets): nothing to gain.


## Battery saver (SYSTEM > BATT SAVER)

OFF, ON, or AUTO (on once at 20 % or less, off at the charger). It reads existing knobs and changes no setting (app/device.rs
`App::saver_on`), so OFF restores everything. **Savings are estimates from the tables above, none measured.**

| Knob | While ON | Estimated saving |
|------|----------|------------------|
| GPS | cyclic tracking as GPS POWER SAVER (app/gps.rs `gnss_saver`) while no ride is running; a running ride and an MTB ride keep the rider's GPS POWER (the distance comes from the GPS speed) | about 4.8 mA over FULL, when idle or paused only |
| Backlight level | at most 30 % | 60 to 18 mA from 100 %, 36 to 18 mA from 60 %, nothing in AUTO daylight (1 %) |
| Backlight time-out | at most 10 s (never off, 30 s and 60 s become 10 s), except under a running ride, which keeps the rider's | up to the lit level (18 to 60 mA) for each second the unit sits unwatched |
| Redraw | graphs of a ride page every 5 s (app/screens.rs `update_page`); fields, the strip, the map and the profile as before; an input at once | well under 1 mA (inferred: the CPU is awake 1.3 % on a ride page) |
| GNSS systems | not used: cyclic tracking draws 5 mA with GPS alone or with three systems (table above) | 0 |
| Phone link | not used: no scan; advertising is already 1 s | under 0.01 mA |

The recording is outside it: 1 s samples and the checkpoints are unchanged (a sim test compares the card's commands with it on and off).
To measure: the same ride ON and OFF, the battery's percent after an hour (docs/status.md). Per profile (ride profiles' settings) is open.

The walk's idle bound is 1.7 % awake (tools/walk.py ends with a 20 s window on the overview and exits 1 above it; set 2026-10-31 beside the 1.52 % measured that day).

## The main loop while idle (2026-10-28)

- **Measured overnight before** (owner, on battery, the overview, backlight off): 12.2 h cost about 41 % of the
  charge, about 3.4 % an hour; the CPU awake 0.8 % of the time, 20 passes a second of 0.4 ms each. So the CPU's work
  is not what drains the cell: the floor is the sleep itself (WFI with the clocks and the SDRAM running, the panel,
  the Nordic chip).
- **Measured 2026-10-28: 12.2 h idle on the overview cost about 41 % of the battery.** So SYSTEM > AUTO OFF (OFF, 5,
  10, 30, 60 min; default 5) switches the unit off after that long with no ride open, no cable, no key or touch, the
  drive not lent and no write waiting; a banner and beep warn 10 s and 5 s before, any input cancels (`step_auto_off`,
  kept in register 18 bits 22..24; an older build rewriting that register resets it to 5 min). Movement (speed over
  2 km/h, cadence, power, from a sensor, the GPS or a trainer; a trainer only heard does not count, 2026-10-28)
  keeps it on for 60 s after the last; never while the recorder holds anything; not
  on the end-ride screens, with a call ringing, or with a settings write or the stock scan pending.
- **Done (lib `App::pass_ms`, main.rs's sleep):** with no ride or its card, no key for 30 s (`IDLE_AFTER_MS`, the
  default OFF AFTER), no finger, no tone or banner timed by the passes and the card not lent to the computer, a pass
  every 250 ms (`IDLE_POLL_MS`) instead of 50. Nothing waits for it: the touch INT (as before), a key's falling edge
  (EXTI 1 to 4, BOARD.md 5) end the sleep at once; a byte from the Nordic chip (BOARD.md 11) wakes it for a radio pass
  (`App::radio_pass`: the chip read, its watch, the sensors' silence; the step at the sleep's end; review 2026-11-01,
  finding 57: a byte ran the whole step). A pending IRQ ends
  WFI with PRIMASK set (ARMv7-M ARM B1.5.19: WFI completes on any exception that would be taken were PRIMASK clear),
  so no handler is needed, as for the touch line. The timers count each pass's real length (`App::dt_ms`) already.
  It applies with the cable in too: USB's own interrupt ends the sleep for its traffic (no SOF interrupt is unmasked),
  which is also how it can be measured.
- **Measured on the unit** (`tools/dev.py run` from RAM, the overview, a phone connected, the GPS off after its search,
  `tools/dev.py prof` over 3 minutes): before, 21.2 passes a second of 0.44 ms, awake 0.99 %; after, 6.7 passes a second
  of 0.73 ms, awake 0.52 %. A pass costs more because the work is per second more than per pass (the second's tick,
  the radio's events); the Nordic chip's bytes wake the loop about 2.7 times a second, and the radio is now the
  largest part (0.23 %). Half the CPU's awake time, which is a fraction of a mA (0.5 % of the difference between
  Run and Sleep currents): the loop Stop mode needs (a long sleep that wakes on what matters) more than a saving in
  itself.
- **Tried 2026-10-31 (18e270e, review finding 9) and taken out 2026-11-01 as a suspect of the ride's radio dropouts:**
  a running ride used a 100 ms pass, and the Nordic chip's bytes did not end it. Measured with the 100 ms pass on the
  unit: riding 1.62 % to 1.05 % awake, 21.2 to 13.1 passes a second.
- **Finding 10 (2026-10-31), in place:** with a fix, the GNSS read waits 800 ms after a whole NAV-PVT (8.8 to 5.0 SPI4
  transfers a second in the simulation), the light read comes every 500 ms for the backlight and every 5 s for THEME
  AUTO alone, and the per-pass date conversions and the page's widget list run only when used. Measured on the unit
  (`tools/profile.py`, RAM run, 10 s windows): awake on the overview 1.54 to 1.36 %, on the first ride page 1.42 to 1.10 %.
- **The status strip and the radio's receive (2026-10-31, review findings 7 and 8):** the strip's section split in
  two (`strip`: its inputs, the next-step lookup and the compare; `strip_draw`: the draws alone). The compare is
  about 10 us a pass (0.02 %); the cost was the draw, once a second: the grey "GPS" blinking while the receiver
  searches (indoors, at the start of a ride), 1.4 ms a draw. Its off half is now one fill instead of "GPS" in black,
  and the text's glyph and font are copied to the stack before a blit (ui/text.rs `pick`: they are in uncached
  SDRAM, read for every pixel), which halves the longest draw of any text. Measured (`tools/profile.py`, RAM run,
  10 s windows): `strip` 105 us mean, 2.0 ms max, 0.22 % of wall on the overview and the first ride page, before;
  after, `strip` 10 us plus `strip_draw` 0.5 to 0.8 ms once a second, 1.5 ms max: 0.07 to 0.10 %. The rest is the
  grey "GPS" itself, about 2 us a pixel with the D-cache off; a steady "GPS" while searching would remove it. The
  Tried 2026-10-31 (finding 8: 20 us after a byte, 200 us only after a full FIFO) and taken out 2026-11-01: the chip
  needs longer than 20 us between frames (inferred), so a pass took about one frame and a ride's events outran the
  drain (docs/status.md).

## Stop mode: plan (phase 1 built: the primitive and its benches; phase 2a: Stop between idle passes; 2b: a ride's)

**Phase 2a, built 2026-11-03: Stop between idle passes, no ride open** (lib app/main_loop.rs `MainLoop::pass`,
app/device.rs `App::stop_ok`, app/loop_policy.rs `Stops`; main.rs `idle_stop`; src/sim/stop_tests.rs). Not yet seen
on the unit (docs/status.md).
- **When:** an idle pass (`App::pass_ms` is 250 ms: no ride open nor its card work, no tone, banner or finger, the card
  not lent, no key for 30 s) a minute after the start (`STOP_AFTER_BOOT_MS`), no cable (VBUS low, no USB link), the
  build's kill switch on (main.rs `POWER_STEPS` bit 5, default on: `POWER_STEPS=95` builds without it alone), fewer than
  `STOP_FAILS_MAX` (4) failed Stops in a row (then WFI only until the next start), and none of AUTO OFF's reasons to
  stay on but the cable and movement (`App::keep_on`, one list for both: the recorder holding anything, a resume or the
  full copy pending, the ride marked open on the card, a file waiting to be written, stock's scan, a call). The board
  does not try (`PENDING`, not counted) while the panel's DMA runs, a key's or the touch's EXTI flag is up, or a byte
  from the Nordic chip is coming in.
- **Outcomes** (stopbench.rs `outcome`): OK; interrupted (no STOPF, but a key, touch or cable EXTI flag or a WKUPF
  flag up after the wake, `WAKE_PENDING`: a wake source that came between the check and the WFI; not a failure);
  failed (no timer, no self-refresh, or the system stayed in Run: harmless, counted toward the 4 in a row); broken (the
  SDRAM's normal mode, the regulator, an oscillator, a PLL or SYSCLK's source not back after a WFI: the unit could be
  left on the HSI with the FMC on hclk3 and SPI1 / SDMMC1 on a dead PLL1Q), which writes its report to backup
  registers 0 and 31 as `bench stop` does (`tools/dev.py stopreport` reads it) and resets: no ride is open.
- **How:** one Stop a pass, for the pass's whole sleep, through phase 1's `hw::stop` at SVOS3. Wake sources: the RTC
  wake-up timer, the keys, the touch INT, VBUS rising (PC13 on EXTI 13: the next pass starts USB as before) and the
  power key (WKUP1); every NVIC line but theirs off meanwhile and all put back after (PRIMASK stays set, no handler).
  The Nordic chip is held off with RTS (PD4) high as an output (stock's way); after it goes high, 20 us for a byte
  on the wire to land, and a byte there cancels the Stop. Its request line PH8 is not a wake source (what it
  signals is not known): its bytes wait for the pass's end, 250 ms at most, as stock's 100 ms alarm makes it wait.
- **Time:** the Stop is timed by the RTC (TR and SSR, 1/1024 s) from the call to its return, in us with the sub-ms
  rest carried (`LoopPolicy::slept_stop_us`), so the app clock, the uptime and the profiler's sleep count it; the
  cycles of the call (entry and wake) are sleep-loop cycles for the time (the RTC counts them already), but work for
  the awake share (`LoopPolicy::woke`). `stop_tests.rs` checks the app clock against the RTC over 600 s of Stops
  (0.02 %), the simulated Stop slept on the wake-up timer's 2048 Hz grid and timed by two readings of the RTC at its
  1/1024 s, as on the unit: a bias of one tick a Stop would be 0.4 %. Seen on the unit (RAM run of 361c5fc, cable in,
  `idlestop 150`): 377 Stops, none failed, mean 250 ms, 94.2 s in Stop; `appup` 178.29 s against the RTC's 179 s
  (1 s resolution).
- **Seen from the computer:** `info`'s `stops N F MS` (Stops, failed ones, ms in Stop; `tools/dev.py info` adds the
  mean) and `prof`'s `wake` section (calls/s: Stops a second; its us are up to 3 times short, the wake running on the
  HSI's 64 MHz). With the cable in, `tools/dev.py idlestop S` (`bench idle-stop`) allows Stop for S seconds with USB
  on, for that measurement only: the link is not served during a Stop, so it may drop and come back, and the bench
  ends by time-out only. A drop the watch decides within 5 s of such a Stop is not logged (no backup register, no
  USB RECONNECT notice).
- **Why 931d594's `bench stop` stayed in Run** (the first unit run, docs/status.md): that run followed a switch-off
  and a power-key wake. Its `srefresh` read PWR_CPUCR 0x1a3 (PDDS_D1, PDDS_D2, STOPF, SBF_D1, SBF_D2: `standby`'s D1 /
  D2 DStandby, kept since PWR sits in D3, which only stopped) and PWR_CPU2CR 0x1e7 (the same SBF bits). Two things
  were left from it: PDDS_D1 / D2 set (but `stop_itcm` clears them before its WFI, so not the cause there; a WFI with
  SLEEPDEEP and them set would be a D1 standby, a reset), and the wake-up flag WKUPF1 with WKUPEN1 still on (cleared
  only by PWR_WKUPCR or NRST, RM0399 PWR_WKUPFR; stock clears it at boot, App_HardwareInit, this firmware did not): a
  wake-up source pending, which keeps the system out of Stop (inferred: RM0399 not re-read on the point). cc2cd49
  cleared WKUPF1 before the WFI and reached Stop, but after a reset, not a switch-off, so it does not prove which.
  Either way each idle Stop now clears all six WKUPF flags, writes PDDS_D1..D3 = 0 with CSSF, and `stop_itcm` reads
  PDDS back before its WFI (none if one is still set). To see on the unit: a Stop right after a switch-off and a
  power-key wake.

**Phase 2b, built 2026-11-03: Stop between a ride's passes** (lib app/device.rs `App::ride_stop_ok`, app/main_loop.rs;
main.rs `idle_stop` with `ride`; src/sim/stop_tests.rs). Not yet seen on the unit (docs/status.md). Stock never stops
while recording (ghidra HARDWARE.md 13): this goes beyond it, so it is bounded so that a missed Stop costs power only.
- **When:** a ride open (running, auto-paused or paused) a minute after the start (a resumed ride's first minute clears
  the resets in a row first), the board's rules of 2a (no cable but for the bench, the panel's DMA idle, no key or
  touch EXTI flag) and the build's own kill switch (main.rs `POWER_STEPS` bit 6: **off by default in the card-writing
  build**, `EMMC_WRITE=1`, the installed ride image, until the unit's ride procedure passes, docs/status.md; on by
  default in the plain build that RAM runs use; `POWER_STEPS=127` turns it on, `63` off; `info` ends with `ride stop
  built yes|no`); then the app's: neither recorder starting or saving, nor with card work due (`Recorder::due`: a
  chunk, a checkpoint, the full copy, the save), failing, waiting to retry or holding a start; no tone, no finger, the
  GNSS read between two UBX frames, none of AUTO OFF's other reasons (a key in the last second, the end-ride and
  discard screens, the card lent, a call, stock's scan, a resume or the full copy pending). The files whose writes wait
  for the ride's end by design (pages.bin, settings.bin, sensors.bin, the altitude calibration) do not keep it out:
  one setting changed would keep the whole ride out of Stop, and a Stop keeps every RAM. The Stop comes after the
  pass's step, so the recorder's card work of the pass is done first (every eMMC command ends inside the step); the
  simulated card asserts that no Stop begins with a transfer open or the card programming.
- **New on the board side, both phases:** no Stop while our bytes to the Nordic chip are still going out (USART2
  enabled and USART_ISR TC clear, RM0399): Stop stops its clocks, and a byte cut mid-frame would mis-frame the chip
  (inferred). A short pass never reads the keys' EXTI flags, so the loop clears them before a ride's Stop (a press
  left one up, which would keep every later Stop out; `a_key_on_a_ride_does_not_keep_its_stops_out`); the step polls
  the keys themselves, as before. After the wake, 32 words of the image spread over the SDRAM's banks (main.rs
  `image_marks`, folded at boot) must read as at boot, or the wake counts as broken (REFRESH_OUT cleared, the reset):
  32 uncached reads, a few us (estimated: about 140 ns each at the FMC's 50 MHz on a 16-bit bus; the unit's `wake`
  section in `prof` includes them).
- **Length: the ride's pass stays 50 ms (`POLL_MS`), and a Stop is at most the rest of it.** Why not a longer pass:
  the 100 ms ride pass of 2026-10-31 was taken out as a suspect of the radio dropouts (above), and a Stop pays already
  at 50 ms. Per pass, the sleep today is about 48.5 ms of CSleep at 16.5 to 47 mA (DS12930 table 44) plus the SDRAM's
  standby; in Stop 2.78 mA at SVOS3 (table 42) plus the self-refresh's 1 mA or less (inferred), and the wake of about
  2 ms measured on the unit (HSE start-up dominates) at Run current. Even counting the wake at 100 mA (taken high on
  purpose; inferred: most of it runs on the HSI at 64 MHz), it costs 2 / 50 x 100 = 4 mA on average, against
  46 / 50 x (16.5 - 2.78) = 12.6 mA saved at the low end and 46 / 50 x (47 - 2.78) = 40.7 mA at the high: **about 9
  to 37 mA saved on a ride, plus the SDRAM's standby** (estimates; the battery history of a ride is the measure). A
  wake on PLL1 from the HSI instead of waiting for the HSE (step 4 below) would save at most those 4 mA, and changes
  the reference every peripheral clock comes from (SPI1 50 MHz, SDMMC1, the GNSS SPI4; USART2 is on the HSI already):
  not done, to measure first.
- **The radio, why 50 ms holds no ANT event too long:** (1) in a ride pass today the chip is already held: USART2's
  RTS rises once the 16-byte RX FIFO is full, so a 46-byte frame arriving during the 48 ms WFI waits after 16 bytes
  for the next step (BOARD.md 11, "Receiving"); a Stop holds it from its first byte, for no longer. (2) The emulated
  Bryton nRF app (`tools/nrf_emu.py selftest`): RTS held 200 ms with 30 ANT events, nothing sent, no hang, the 30
  frames in order on the release; its queue holds 96 events (97 in one SoftDevice pass hang it). At a ride's load
  (three channels at 4 Hz, a BLE notification at 10 Hz, about 22 events a second) 50 ms queues one or two events,
  and a phone's burst of 12 notifications 14. (3) The simulator's strict chip model (96 slots, one frame at a time while
  the host reads): 10 minutes of ride traffic leave a high-water of 12 at 50 ms passes and 12 at 100 ms
  (sim/ride_radio_tests.rs); an hour through the unit's own pass with the ride's Stops
  (`an_hour_ride_with_stops_records_as_one_without`: heart rate with beats, power, Di2, the phone's notifications),
  the simulated Stop holding RTS as main.rs does (the chip sends nothing for its span while its events queue; no
  Stop with a byte in our FIFO or landing within the 20 us look, or ours still going out): 54,294 Stops in 70,382
  passes (77 %; 14,848 passes met a byte of the chip's and slept in WFI), the chip's queue high-water 14 of 96 (12
  without), 14,400 broadcasts of each sensor (4 Hz), no ANT event lost, no hang, no reset, as without. (4) HRV takes the RR intervals from the strap's own beat time (1/1024 s, in the
  broadcast: drivers/sensors.rs `Beats`), not the arrival, so a held message keeps its beats' timing; only a lost
  broadcast loses beats (the full copy's `hrv` messages: 865 in the hour with Stops and without). PH8 is not a wake:
  the Ghidra notes call it the chip's request line, inferred, only cleared by stock (HARDWARE.md 10).
- **GNSS and the records:** the receiver keeps its output until read (SPI4, polled in the step), and its reads are
  timed by the pass's real length (`GNSS_FILL_MS`, `GNSS_PVT_WAIT_MS`), so the pass length unchanged reads each fix as
  early: in the hour, the longest unread gap 840 ms with Stops (820 without), no epoch lost. The recorder records once
  a second from the RTC, and the app clock is the RTC's across Stops (2a): 3,481 records in the hour (all but the
  pause's 2 minutes, an auto-pause records on), one gap (the pause), none twice; the same laps, auto-pause, timer
  (3,360 s) and distance as without.
- **A wake that broke** resets as in 2a: the ride resumes from its checkpoint (at most its last chunk lost, as any
  fault), and its report stays in backup register 0 (`stopbench::broke`): while it is there no ride stops again, so a
  wake that keeps breaking cannot reset the unit once a minute through a ride. While a ride is open the USB
  diagnostics (a drop's word, a snapshot) leave it there; once the ride is over a bench or a USB drop's word clears it
  (`tools/dev.py stopreport` reads it first). A ride resumed after any fault comes back paused and stays so until
  Start, moving or not (`Ride::resumed`; `a_ride_wake_that_breaks_resets_once_and_stops_no_more`).
- **Seen from the computer:** `info`'s `stops N F MS RN RF RMS` (all Stops, failed, ms; then the ride's Stops, failed
  and ms; `tools/dev.py info` says idle and ride apart) and `ride stop built yes|no`. On a ride from RAM the cable is in, so `tools/dev.py idlestop S` (the bench)
  allows the ride's Stops too, for S seconds.

**Phase 1, built 2026-11-03; first unit run the same day** (docs/status.md): the self-refresh around a CSleep WFI
works (the image's CRC unchanged over 1 s); `bench stop` with the cable in did not reach system Stop (no STOPF, the
clocks never stopped). The benches now record the registers around the WFI (`stop --keep`) and can switch USB fully
off first (`stop --usboff`, as on battery), to find what holds the system in Run.
- **hw.rs `stop(deep, svos, report)`:** `stop_itcm`, in link.x `.itcm` beside the RAM-run trampoline (about 600 B of
  the 4 KB), is register accesses, counted waits (at most 1,000,000 reads each) and stores into the caller's report
  (DTCM): no call, no table, no literal into the SDRAM, which `tools/stack_check.py` checks in the release ELF on every
  gate (`itcm_calls`). Steps 2 to 5 below, with two changes: the clocks are put back as RCC_CR and RCC_CFGR SW read just
  before (the bootloader sets PLL1, main.rs never did, so there are no boot values to reuse; BOARD.md 1), and there is
  no FMC busy flag to wait on (the H7's FMC_SDSR has none: BOARD.md 3). If self-refresh is not confirmed there is no
  WFI; whatever fails on the wake, the normal-mode command is sent before it returns. `deep` false is unit step 1
  (CSleep, the clocks kept), true is step 2 (SVOS3, main.rs `STOP_SVOS`).
- **The wake (main.rs `low_power`, `wakeup_timer`):** the RTC wake-up timer on EXTI line 19 (checked: STM32CubeH7's
  `RTC_EXTI_LINE_WAKEUPTIMER_EVENT`, BOARD.md 8) and IRQ 3, every other NVIC line and the CPU's EXTI masks saved,
  cleared, then put back; SysTick off; the image's CRC before and after; the sleep timed by the RTC (SSR, RSF
  resynchronised after the wake).
- **The benches (src/link/dev.rs, src/link/stopbench.rs, `tools/dev.py srefresh / stop / stopreport`):** refused
  while a ride is open. `bench stop` leaves its report in backup registers 0 (tag 0x57) and 31 (tag 0x58), the shared
  diagnostics words, then resets; `--usboff` turns USB fully off first, `--keep` stays on the bus and answers the
  registers before and after the WFI. The power key (WKUP1, EXTI 55) ends the sleep too. Not done in phase 1: RTS held high for the Nordic chip (the bench resets after), the
  wake on keys, touch, VBUS or the Nordic chip, the sleep's time for the profiler and the uptime.

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
- The pass's timer: the RTC wakeup timer (RTC_WUTR, from RTCCLK / 16: 0.49 ms steps), its EXTI line (19, IRQ 3:
  BOARD.md 8) unmasked for the CPU; or LPTIM2 on the LSE. (Phase 2a: the RTC wake-up timer.)
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
  PREDIV_S 255) or LPTIM2's counter, for `prof::Clock::slept`, the uptime and the profiler's `slept_ms`. `delay_ms`
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
   (`tools/dev.py run`): the first step, and nothing written to the eMMC.
2. **One Stop of a second, from RAM:** the same command disconnects USB, enters Stop with the RTC wakeup timer,
   restores the clocks and the SDRAM, writes STOPF, PWR_CPU2CR, the cycles waited for VOSRDY / ACTVOSRDY, the image's
   CRC and the wake time to backup registers, then a system reset (the RAM image is gone, the installed firmware
   starts with the USB link up): `tools/dev.py peek` of the register says whether it worked. Nothing installed, and a hang
   ends at the four-button restart.
3. **Stop between passes, on battery:** a `DIAG_STOP` build installed with the owner present, the breadcrumbs counting
   Stop entries and returns (as `STANDBY_BKP`), and the battery history before and after (this page's plan).

**hw.rs would gain** one primitive: `stop(wake_ms) -> StopReport`, in `.itcm`, with its own volatile accesses
inlined (no call into SDRAM code), its `// SAFETY:` saying why no SDRAM access can happen between the self-refresh
command and its exit (interrupts masked, no fault path, everything it touches in ITCM, DTCM or peripherals). The RTC
wakeup timer, the EXTI lines and RTS as a GPIO are ordinary register writes in main.rs.

## Measurement plan (for the owner)

The unit must run on its battery: USB power charges it (BOARD.md 12) and unplugging resets it, so a build run from
RAM (`tools/dev.py run`) is gone once unplugged. The A / B switches are therefore settings in the installed build: GPS
POWER (FULL / SAVER), BRIGHTNESS and OFF AFTER, a sensor paired but absent (A) or forgotten (B).

- **What is read:** the battery history, one smoothed reading a minute, the last 4 hours, in RAM (`tools/dev.py bat`). It
  survives plugging the cable back in (plugging does not reset the unit; the readings taken on the cable are flagged
  charging), not a power off.
- **One run:**
  1. Charge to full, unplug (the unit restarts), wait 20 minutes (the voltage relaxes after the charge).
  2. Set the case (for example GPS POWER FULL), open the same ride page, outdoors or at a window so the GPS has a
     fix, backlight AUTO. A ride may run (Start) so the receiver, the sensors and the page behave as on a ride.
  3. Leave it 4 hours, then plug in and run `tools/dev.py bat > runA.txt` at once.
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
