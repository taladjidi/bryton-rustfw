# bryton-rustfw

An open, from-scratch firmware in safe Rust for the **Bryton Rider S510** bike computer. It replaces Bryton's OS
entirely. Not affiliated with Bryton.

| | | | |
|:-:|:-:|:-:|:-:|
| ![Overview](docs/img/overview.png) | ![Ride started](docs/img/ride.png) | ![Power fields](docs/img/power.png) | ![W' balance graph](docs/img/wbal.png) |
| Overview: the date, today's planned workout with its profile, four fields, notices (sim) | Start: the ride, said in a banner at the top (sim) | Power: 3 / 10 / 30 s, NP, IF, TSS (sim) | Graph of W' balance; a tap goes to the next plot, never while recording (sim) |
| ![Power and HR graphs](docs/img/graphs.png) | ![HRV](docs/img/hrv.png) | ![Layout](docs/img/layout.png) | ![Settings](docs/img/settings.png) |
| Power graph with its axes, over the HR graph (sim) | DFA a1 and RMSSD from the strap's beats (sim) | An uneven layout: a big box over 2 x 2 (sim) | SETTINGS > ME: values with their units, edited in place (sim) |
| ![Ride card](docs/img/card.png) | ![Map](docs/img/map.png) | ![Sensors](docs/img/sensors.png) | ![GPS](docs/img/gps.png) |
| The ride's card when it ends: map, profile, numbers (sim) | Map with the track (sim) | Sensors and built-in readings (sim) | GPS (sim) |
| ![History](docs/img/history_list.png) | ![A past ride](docs/img/history.png) | ![Workout steps](docs/img/workout-steps.png) | ![Workout](docs/img/workout.png) |
| History: stock's rides, newest first (sim) | A past ride's card, read from its file (sim) | A planned workout's steps, from stock's calendar (sim) | A workout running: target, step left, next (sim) |
| ![Course](docs/img/course.png) | ![Course profile](docs/img/course-profile.png) | ![Course fields](docs/img/course-fields.png) | ![Courses](docs/img/courses.png) |
| A course followed: the line under the track (sim) | The course profile: climbs by grade, the rider's place (sim) | Distance to go, off course, the climb ahead, the climb pacer's TOP field (sim) | Courses on the card: stock's routes, FIT, GPX (sim) |
| ![Workout graph](docs/img/workout-graph.png) | ![Workout gauge](docs/img/workout-gauge.png) | ![Menu](docs/img/menu.png) | ![Field groups](docs/img/field-groups.png) |
| The workout graph: steps by target and zone, the rider's place (sim) | The workout gauge: 3 s power on the step's band (sim) | The menu: what to ride, what was ridden, then the set-up; 44 px rows, a tap selects then acts (sim) | A page's field: its group, then the field (sim) |
| ![IGN map](docs/img/ign-map.png) | ![Media remote](docs/img/phone-media.png) | ![A call](docs/img/phone-call.png) | ![Map zoom](docs/img/map-zoom.png) |
| The map over IGN pack tiles, "(c) IGN" in the corner (sim, synthetic tiles, not a map) | The phone's media remote: what plays (AMS), the keys (sim) | An incoming call from the phone (ANCS): the banner at the top, the buttons name their keys (sim) | The map zoomed in with "+" and "-", panned by a drag, the crosshair to follow again (sim, synthetic tiles) |
| ![Ride menu](docs/img/ride-menu.png) | ![Settings](docs/img/menu-settings.png) | ![Page editor](docs/img/page-editor.png) | ![Editor actions](docs/img/page-editor-actions.png) |
| Page held during a ride: the ride menu, only what a rider does on the road (sim) | SETTINGS: a menu per subject, any setting three levels down at most (sim) | The page editor: a box held, framed in orange, live values, the hint in the strip's place (sim) | Start in the editor: the box's actions, SPLIT across or down by their icons (sim) |
| ![A page made in the editor](docs/img/page-made.png) | ![Eight fields](docs/img/page-eight.png) | ![End of a ride](docs/img/end-ride.png) | ![Discard asked](docs/img/end-ride-discard.png) |
| A page made in the editor: a graph over four values (sim) | Eight fields, four rows of two (sim) | Start held while paused: RESUME, SAVE, DISCARD, big enough for gloves (sim) | DISCARD asks once more; the ride is moved to `\Rustfw\Discarded`, never erased (sim) |
| ![Touch lock](docs/img/touch-lock.png) | ![Zone bar](docs/img/zone-bar.png) | ![No notifications](docs/img/notifs-empty.png) | ![Overview cog](docs/img/overview-cog.png) |
| The touch lock: a padlock in the strip (sim) | A zone as an 8 px bar on the box's top edge (sim) | An empty list says so in a row: no notifications (sim) | The overview's cog opens the menu (sim) |
| ![Overview, SMOOTH font](docs/img/overview-cog-smooth.png) | ![Target gauge](docs/img/gauge-zones.png) | ![Target gauge, half box](docs/img/gauge-zones-half.png) | ![Ramp test result](docs/img/ramp-result.png) |
| The same with FONT = SMOOTH (Barlow Semi Condensed Bold) (sim) | The target gauge spans every zone, power from 40 to 165 % of the FTP (sim) | The same in a half box: the climb pacer's TOP field (sim) | FTP ramp test result: FTP, best minute, APPLY / KEEP (sim) |
| ![Rest day](docs/img/overview-rest.png) | ![WORKOUTS list](docs/img/workouts-list.png) | | |
| The overview's TODAY card with nothing planned (sim) | WORKOUTS: each day's profile beside its date and length; tap the overview's card to get here (sim) | | |
| ![FITNESS](docs/img/fitness.png) | ![Power meter calibration](docs/img/power-zero.png) | ![BIKE menu](docs/img/menu-bike.png) | |
| HISTORY > FITNESS: CP, W', EST FTP, HRVT1 / HRVT2 from past rides (sim, no rides read) | SENSORS > CALIBRATE POWER: zero offset and auto zero (sim meter) | SETTINGS > BIKE: wheel, weight, altitude, profile, climbs ahead (sim) | |

*"unit": a screenshot of the bike computer over USB (`./dev.py shot`, rendered by the firmware from what it drew);
"sim": the same firmware code drawing in the host simulator.*

**Status (2026-10-26):** runs on the unit (installed, and newer builds from RAM). An overview and ride pages in eleven
layouts, a page editor, touch and keys, menus, ANT+ sensors, a power meter's calibration and a smart trainer (ERG, SIM,
an FTP ramp test), derived power metrics (NP, TSS, W' balance...), heart rate variability (DFA a1, thresholds from
recent rides) and the beats recorded raw, graphs, GPS with fast aided starts, a track map with a basemap (IGN Plan v2
packs, else stock's cached tiles), the workouts stock's app synced, courses with climbs, a climb pacer and the climbs
on the road ahead without a course, the phone as a Bluetooth accessory, a PIXEL or SMOOTH font, and a USB dev link.
Rides are saved to `\Activities` as stock does (seen on the unit), in two copies, and survive a reset or a power cut.
The newest features are seen in the simulator only so far (docs/status.md). It shares the storage with Bryton's firmware, so the two can be swapped.

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

It replaces the OS. Rides are saved to `\\Activities` as stock does (seen on the unit); no updates come from this firmware. The way back is the
bootloader's recovery mode, which this firmware cannot touch ([docs/install.md](docs/install.md)).
