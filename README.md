# bryton-rustfw

An open, from-scratch firmware in safe Rust for the **Bryton Rider S510** bike computer. It replaces Bryton's OS
entirely. Not affiliated with Bryton.

| | | | |
|:-:|:-:|:-:|:-:|
| ![Overview](docs/img/overview.png) | ![Ride started](docs/img/ride.png) | ![Power fields](docs/img/power.png) | ![W' balance graph](docs/img/wbal.png) |
| Overview: time, date, four fields, notices (sim) | Start: the ride, said in a banner (sim) | Power: 3 / 10 / 30 s, NP, IF, TSS (sim) | Graph of W' balance; a tap goes to the next plot (sim) |
| ![Power and HR graphs](docs/img/graphs.png) | ![HRV](docs/img/hrv.png) | ![Layout](docs/img/layout.png) | ![Settings](docs/img/settings.png) |
| Power graph with its axes, over the HR graph (sim) | DFA a1 and RMSSD from the strap's beats (sim) | An uneven layout: a big box over 2 x 2 (sim) | Menu: settings edited in place (sim) |
| ![Ride card](docs/img/card.png) | ![Map](docs/img/map.png) | ![Sensors](docs/img/sensors.png) | ![GPS](docs/img/gps.png) |
| The ride's card when it ends: map, profile, numbers (sim) | Map with the track (sim) | Sensors and built-in readings (unit) | GPS (sim) |
| ![History](docs/img/history_list.png) | ![A past ride](docs/img/history.png) | ![Workout steps](docs/img/workout-steps.png) | ![Workout](docs/img/workout.png) |
| History: stock's rides, newest first (sim) | A past ride's card, read from its file (sim) | A planned workout's steps, from stock's calendar (sim) | A workout running: target, step left, next (sim) |
| ![Course](docs/img/course.png) | ![Course profile](docs/img/course-profile.png) | ![Course fields](docs/img/course-fields.png) | ![Courses](docs/img/courses.png) |
| A course followed: the line under the track (sim) | The course profile: climbs by grade, the rider's place (sim) | Distance to go, off course, the climb ahead (sim) | Courses on the card: stock's routes, FIT, GPX (sim) |
| ![Workout graph](docs/img/workout-graph.png) | | | |
| The workout graph: steps by target and zone, the rider's place (sim) | | | |

*"unit": a screenshot of the bike computer over USB (`./dev.py shot`, rendered by the firmware from what it drew);
"sim": the same firmware code drawing in the host simulator.*

**Status (2026-10-24):** runs on the unit (installed, and newer builds from RAM). An overview and ride pages in eleven
layouts, touch and keys, menus, ANT+ sensors (heart rate tried), derived power metrics (NP, TSS, W' balance...) and
heart rate variability (DFA a1), graphs, GPS with fast aided starts, a track map with a basemap, battery, backlight
with an automatic mode, beeps, the workouts stock's app synced (steps, targets, 3-2-1 beeps), courses with climbs (stock's routes, FIT, GPX), and a USB dev link. Rides are not saved yet: the FIT writer is ready, the eMMC driver
reads but does not write yet. It shares the storage with Bryton's firmware, so the two can be swapped.

| Read | For |
|---|---|
| [docs/features.md](docs/features.md) | what it does, screen by screen |
| [docs/checklist.md](docs/checklist.md) | every feature of stock, the patches and other bike computers, ticked as it lands |
| [docs/install.md](docs/install.md) | build, the ANT+ key, install, and the way back to stock |
| [docs/status.md](docs/status.md) | what is not yet seen on the unit, next steps |
| [docs/journal.md](docs/journal.md) | what was found on the unit, dated |
| [docs/roadmap.md](docs/roadmap.md) | what to build next, in dependency order |
| [docs/boot.md](docs/boot.md) | how it starts, from the bootloader's hand-off |
| [BOARD.md](BOARD.md) | the hardware: pin map, timing diagrams, every chip, with evidence |
| [AGENTS.md](AGENTS.md) | how to work on it: rules, tests, the dev loop |

It replaces the OS: **no rides are saved yet, and no updates come from this firmware.** The way back is the
bootloader's recovery mode, which this firmware cannot touch ([docs/install.md](docs/install.md)).
