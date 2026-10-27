# bryton-rustfw 🦀

An open, from-scratch firmware in safe Rust for the **Bryton Rider S510** bike computer. It replaces Bryton's OS
entirely. Not affiliated with Bryton.

## Climbs ahead and the climb coach

<img src="docs/img/climb-page.png" width="240" align="right" alt="The CLIMB page">

On the road ahead, even without a course, the bike computer finds the climbs (Garmin ClimbPro's definition, on
stock's own routing map), says the next one, and when it starts opens the **CLIMB page** once:
- the climb's profile, coloured by grade, the part done dark and the rider's place in white, as on the workout page;
- its gain, length and average grade written on the graph;
- below, the grade now and the **climb coach**: the power to hold for this section, from CP and W' (the time to the
  top, the target and a gauge), so the climb is ridden at the pace that gets to the top soonest.

With a course followed, the same page uses the course's climbs. (Picture: the simulator on a made-up road.)

<br clear="right">

| | | | |
|:-:|:-:|:-:|:-:|
| ![Overview](docs/img/overview.png) | ![Zone bar](docs/img/zone-bar.png) | ![IGN map](docs/img/ign-map.png) | ![Workout graph](docs/img/workout-graph.png) |
| Overview: the date, today's planned workout with its profile, four fields, notices (sim) | A zone as an 8 px bar on the box's top edge (sim) | The map over IGN pack tiles, "(c) IGN" in the corner (sim, synthetic tiles, not a map) | The workout graph: steps by target and zone, the rider's place (sim) |

More screens, grouped by menu: [the gallery](docs/gallery.md).

*"unit": a screenshot of the bike computer over USB (`./dev.py shot`, rendered by the firmware from what it drew);
"sim": the same firmware code drawing in the host simulator.*

**Status (2026-10-27):** runs on the unit (installed, and newer builds from RAM). An overview and ride pages in eleven
layouts, a page editor, touch and keys, menus, ANT+ sensors, a power meter's calibration and a smart trainer (ERG, SIM,
an FTP ramp test), derived power metrics (NP, TSS, W' balance...), heart rate variability (DFA a1, thresholds from
recent rides) and the beats recorded raw, graphs, GPS with fast aided starts, a track map with a basemap (IGN Plan v2
packs, else stock's cached tiles), the workouts stock's app synced, courses with climbs, a climb pacer and the climbs
on the road ahead without a course, with a CLIMB page that opens at a climb's start, the phone as a Bluetooth accessory, a PIXEL or SMOOTH font, and a USB dev link.
Rides are saved to `\Activities` as stock does (seen on the unit), in two copies, and survive a reset or a power cut.
The newest features are seen in the simulator only so far (docs/status.md). It shares the storage with Bryton's firmware, so the two can be swapped.

## Key figures

Measured on the unit (2026-10-27, `tools/profile.py` from RAM, 30 s windows; the CPU sleeps between 50 ms passes):

| | |
|---|---|
| CPU awake, overview idle | 1.55 % |
| CPU awake, a ride page | 1.32 % |
| CPU awake, menus (a key every 0.5 s) | 1.82 % |
| A menu move | 4 ms mean (two rows redrawn; the panel's SPI link, about 1 us a pixel, sets it) |
| Panel traffic, a ride page | about 740 pixels a second (only what changed, in the smallest box) |
| Climb pacer, one plan | about 75 us (estimated from operation counts) |
| A map tile for climbs ahead | at most about 3 ms a slice (estimated; one tile took 238 ms in one go before) |
| Image | 605 KB (stock's: over 3 MB), running from the unit's 64 MB SDRAM |
| Code | about 79,000 lines of Rust, `unsafe` only in `src/board/hw.rs` (and one marker line in usb.rs) |
| Tests | 748 host and simulator tests, 143 doc tests, fuzzing of every input path, the ride recorder cut after every sector written |


| Read | For |
|---|---|
| [docs/features.md](docs/features.md) | what it does, screen by screen |
| [docs/checklist.md](docs/checklist.md) | every feature of stock, the patches and other bike computers, ticked as it lands |
| [docs/install.md](docs/install.md) | build, the ANT+ key, install, and the way back to stock |
| [docs/status.md](docs/status.md) | what is not yet seen on the unit, next steps |
| [docs/journal.md](docs/journal.md) | what was found on the unit, dated |
| [docs/roadmap.md](docs/roadmap.md) | what to build next, in dependency order |
| [docs/boot.md](docs/boot.md) | how it starts, from the bootloader's hand-off |
| [docs/power.md](docs/power.md) | what each peripheral draws (estimated), what was done, how to measure the battery |
| [BOARD.md](BOARD.md) | the hardware: pin map, timing diagrams, every chip, with evidence |
| [AGENTS.md](AGENTS.md) | how to work on it: rules, tests, the dev loop |

It replaces the OS. Rides are saved to `\\Activities` as stock does (seen on the unit); no updates come from this firmware. The way back is the
bootloader's recovery mode, which this firmware cannot touch ([docs/install.md](docs/install.md)).
