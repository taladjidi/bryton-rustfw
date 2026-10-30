# Gallery

Every screen of the firmware, grouped by where it is on the bike computer. "sim": drawn by the firmware's own code in the host simulator; "unit": a screenshot over USB.

## Overview

| | | |
|:-:|:-:|:-:|
| ![Overview](img/overview.png) | ![Rest day](img/overview-rest.png) | ![Overview cog](img/overview-cog.png) |
| Overview: the date, today's planned workout with its profile, four fields, notices (sim) | The overview's TODAY card with nothing planned (sim) | The overview's cog opens the menu (sim) |
| ![Overview, SMOOTH font](img/overview-cog-smooth.png) | ![WORKOUTS list](img/workouts-list.png) | ![Quick panel](img/quick-panel.png) |
| The same with FONT = SMOOTH (Barlow Semi Condensed Bold) (sim) | WORKOUTS: each day's profile beside its date and length; a rest day's card gets here, else it opens today's workout (sim) | The quick panel, a swipe down anywhere: the media remote, the power meter with ZERO and its result, each sensor with its battery (LOW in red), GPS, phone, battery and the brightness (sim) |
| ![No notifications](img/notifs-empty.png) | ![Touch lock](img/touch-lock.png) |  |
| An empty list says so in a row: no notifications (sim) | The touch lock: a padlock in the strip (sim) |  |

## Ride pages and fields

| | | |
|:-:|:-:|:-:|
| ![Ride started](img/ride.png) | ![Power fields](img/power.png) | ![W' balance graph](img/wbal.png) |
| Start: the ride, said in a banner at the top (sim) | Power: 3 / 10 / 30 s, NP, IF, TSS (sim) | Graph of W' balance; a tap goes to the next plot, never while recording (sim) |
| ![Power and HR graphs](img/graphs.png) | ![HRV](img/hrv.png) | ![Zone bar](img/zone-bar.png) |
| Power graph with its axes, over the HR graph (sim) | DFA a1 and RMSSD from the strap's beats (sim) | A zone as an 8 px bar on the box's top edge (sim) |
| ![Target gauge](img/gauge-zones.png) | ![Target gauge, half box](img/gauge-zones-half.png) | ![Layout](img/layout.png) |
| The target gauge spans every zone, power from 40 to 165 % of the FTP (sim) | The same in a half box: the climb pacer's TOP field (sim) | An uneven layout: a big box over 2 x 2 (sim) |
| ![Eight fields](img/page-eight.png) |  |  |
| Eight fields, four rows of two (sim) |  |  |
| ![Trip, laps and last lap](img/ride-trip-fields.png) |  |  |
| Trip time, laps, last lap, lap power and lap NP on a ride page (sim) |  |  |
| ![Heart rate and power against the numbers](img/ride-zone-fields.png) |  |  |
| % of max HR (LTHR not set: "--"), heart-rate zone and its time, % of FTP and the power zone on a ride page (sim) |  |  |
| ![Heading, sun and battery fields](img/ride-sun-fields.png) |  |  |
| Heading, sunrise, sunset, time to sunset, ETA ("--" off course) and the unit's battery on a ride page (sim) |  |  |

## Page editor

| | | |
|:-:|:-:|:-:|
| ![Page editor](img/page-editor.png) | ![Editor actions](img/page-editor-actions.png) | ![A page made in the editor](img/page-made.png) |
| The page editor: a box held, framed in orange, live values, the hint in the strip's place (sim) | Start in the editor: the box's actions, SPLIT across or down by their icons (sim) | A page made in the editor: a graph over four values (sim) |
| ![Field groups](img/field-groups.png) |  |  |
| A page's field: its group, then the field (sim) |  |  |

## Map

| | | |
|:-:|:-:|:-:|
| ![Map](img/map.png) | ![IGN map](img/ign-map.png) | ![Map zoom](img/map-zoom.png) |
| Map with the track (sim) | The map over IGN pack tiles, "(c) IGN" in the corner (sim, synthetic tiles, not a map) | The map zoomed in with "+" and "-", panned by a drag, the crosshair to follow again (sim, synthetic tiles) |

## Courses and climbs

| | | |
|:-:|:-:|:-:|
| ![Course](img/course.png) | ![Course profile](img/course-profile.png) | ![Course fields](img/course-fields.png) |
| A course followed: the line under the track (sim) | The course profile: climbs by grade, the rider's place (sim) | Distance to go, off course, the climb ahead, the climb pacer's TOP field (sim) |
| ![Courses](img/courses.png) | ![CLIMB page](img/climb-page.png) |  |
| Courses on the card: stock's routes, FIT, GPX (sim) | The CLIMB page, opened when the climb starts: its profile by grade, the part done dark and the rider white, gain, length and grade, then the grade and the climb coach (sim, made-up road) |  |

## Workouts and training

| | | |
|:-:|:-:|:-:|
| ![Workout steps](img/workout-steps.png) | ![Workout](img/workout.png) | ![Workout graph](img/workout-graph.png) |
| A planned workout's steps, from stock's calendar (sim) | A workout running: target, step left, next (sim) | The workout graph: steps by target and zone, the rider's place (sim) |
| ![Workout gauge](img/workout-gauge.png) | ![Ramp test result](img/ramp-result.png) |  |
| The workout gauge: 3 s power on the step's band (sim) | FTP ramp test result: FTP, best minute, APPLY / KEEP (sim) |  |

## End of a ride and history

| | | |
|:-:|:-:|:-:|
| ![End of a ride](img/end-ride.png) | ![Discard asked](img/end-ride-discard.png) | ![Ride card](img/card.png) |
| Start held while paused: RESUME, SAVE (selected), DISCARD, big enough for gloves (sim) | DISCARD asks once more; the ride is moved to `\Rustfw\Discarded`, never erased (sim) | The ride's card when it ends: map, profile, numbers (sim) |
| ![History](img/history_list.png) | ![A past ride](img/history.png) | ![FITNESS](img/fitness.png) |
| History: stock's rides, newest first (sim) | A past ride's card, read from its file; DELETE in red asks first (sim) | HISTORY > FITNESS: CP, W', EST FTP, HRVT1 / HRVT2 from past rides (sim, no rides read) |

## Menus and settings

| | | |
|:-:|:-:|:-:|
| ![Menu](img/menu.png) | ![Ride menu](img/ride-menu.png) | ![ME settings](img/settings.png) |
| The menu: what to ride, what was ridden, then the set-up; 44 px rows, a tap acts on a row (sim) | Page held during a ride: the ride menu, only what a rider does on the road (sim) | SETTINGS > ME: values with their units, edited in place (sim) |
| ![SETTINGS menu](img/menu-settings.png) | ![BIKE menu](img/menu-bike.png) | ![ME menu](img/me-menu.png) |
| SETTINGS: a menu per subject, any setting three levels down at most (sim) | SETTINGS > BIKE: wheel, weight, altitude, profile, climbs ahead (sim) | SETTINGS > ME: MAX HR, LTHR, ZONES BY, ZONES, FTP, then sex, birth year and height (sim) |
| ![ZONES menu](img/zones-edit-menu.png) |  |  |
| ME > ZONES: the top of each heart-rate and power zone, edited one by one (an edited bound is kept; RESET at the end) (sim) |  |  |
| | | |
|:-:|:-:|:-:|
| ![ABOUT](img/about.png) | ![STORAGE](img/storage.png) | ![STORAGE's oldest ride](img/storage-ride.png) |
| ABOUT under SYSTEM: the version and the commit, Ferris, the map and font notices (sim) | STORAGE under SYSTEM: the card's size, free space, rides and the five oldest, read over the passes (sim) | The oldest ride's card from STORAGE, DELETE on it as in HISTORY (sim) |

## Sensors and phone

| | | |
|:-:|:-:|:-:|
| ![Sensors](img/sensors.png) | ![GPS](img/gps.png) | ![Power meter calibration](img/power-zero.png) |
| Sensors and built-in readings (sim) | Speed page (sim) | SENSORS > CALIBRATE POWER: zero offset and auto zero (sim meter) |
| ![Sensor names](img/sensors-names.png) | ![Media remote](img/phone-media.png) | ![A call](img/phone-call.png) |
| SENSORS with the strap's maker from ANT+ common page 80 (POLAR HR, device number 9: synthetic) (sim) | The phone's media remote: what plays (AMS), the keys (sim) | An incoming call from the phone (ANCS): the banner at the top, the buttons name their keys (sim) |
| The phone's media remote: what plays (AMS), the keys (sim) | An incoming call from the phone (ANCS): the banner at the top, the buttons name their keys (sim) |  |

## Alerts

| | |
|:-:|:-:|
| ![POWER HIGH](img/alert-power-high.png) | ![ALERTS menu](img/alerts-menu.png) |
| A power high limit crossed for 5 s: the red banner and three beeps (sim) | SETTINGS > ALERTS, scrolled to its end: limits and reminders, each OFF until set (sim) |

## Power match

| | |
|:-:|:-:|
| ![Power match](img/trainer-power-match.png) | |
| ERG with a power meter reading 20 W under the trainer: "PM+10%" left of the source line (sim) | |

## Themes

| | | |
|:-:|:-:|:-:|
| ![DARK](img/theme-dark.png) | ![OUTDOOR](img/theme-outdoor.png) | ![OUTDOOR, a ride page](img/theme-outdoor-ride.png) |
| DARK: the colours as they were (sim) | OUTDOOR: lighter notes and lines, brighter zones (sim) | OUTDOOR on a ride page (sim) |
| ![OUTDOOR menu](img/theme-outdoor-menu.png) | ![A theme file](img/theme-sunny.png) | ![LIGHT](img/theme-light.png) |
| SETTINGS > DISPLAY > THEME cycles DARK, OUTDOOR, LIGHT and the files of `\Rustfw\Themes` (sim) | A theme file from the card: SUNNY, OUTDOOR with a blue accent (sim) | LIGHT: black on white for the sun (sim) |
| ![LIGHT, a ride page](img/theme-light-ride.png) | ![LIGHT menu](img/theme-light-menu.png) | ![LIGHT map](img/theme-light-map.png) |
| LIGHT on a ride page (sim) | LIGHT menu (sim) | LIGHT map page: the basemap stays a picture (sim) |
| ![Menu, 44 px rows](img/metrics-menu-44.png) | ![Menu, 56 px rows](img/metrics-menu-56.png) | ![Zone bar 4 px](img/metrics-ride-bar4.png) |
| `menu_row = 44` (the built-ins): six rows (sim) | `menu_row = 56`: four rows, a tap lands on the row drawn (sim) | `zone_bar = 4`, the thinnest, on the heart rate box (sim) |
| ![Fields overview, bar 8](img/theme-fields-overview-0.png) | ![Fields overview, bar 4](img/theme-fields-overview-1.png) | |
| `overview = fields`: a 2 x 2 table (time, next workout, year km, week hours) in the card's place (sim) | The same with the metrics at their least (sim) | |

## Laps

| | |
|:-:|:-:|
| ![Lap banner](img/auto-lap.png) | |
| AUTO LAP BY DISTANCE 5 km: the banner of lap 1 with its number, time, distance and average speed (sim) | |

## Live segments

| | | |
|:-:|:-:|:-:|
| ![Segment page, ahead](img/segment-ahead.png) | ![Segment page, behind](img/segment-behind.png) | ![Result card](img/segment-card.png) |
| The SEGMENT page ahead of the PR: the delta in green, the two marks on the bar (sim) | Behind: the accent, never red (sim) | The result card, a new PR (sim) |
| ![SEGMENTS menu](img/segment-menu.png) | | |
| MENU > SEGMENTS: the two settings, the segments of the card, the skipped files (sim) | | |

## Units

| | |
|:-:|:-:|
| ![Imperial units](img/imperial-units.png) | |
| UNITS IMPERIAL and TEMPERATURE F on a ride page: mph, miles, feet, F and a trip in miles, with their labels (sim) | |
## Workouts

| | |
|:-:|:-:|
| ![Next-step strip](img/workout-next-strip.png) | ![Two targets](img/workout-two-targets.png) |
| The next-step strip in the status strip's place, 6 s before the step ends (sim) | A step with two targets: "200-250/90-100RPM" under the bar (sim) |

## Page switches

| | |
|:-:|:-:|
| ![Page menu](img/page-switches-menu.png) | ![Renamed page](img/page-renamed-list.png) |
| PAGES > ROAD PAGES > a page: SHOW, MOVE UP, MOVE DOWN and RENAME under its fields (sim) | A page renamed "CA B" in the list (four characters at most; sim) |

## Start reminder and ready check

| | |
|:-:|:-:|
| ![Start reminder](img/start-reminder.png) | ![Ready card](img/ready-card.png) |
| START THE RIDE? after 10 s of pedalling with no ride (sim) | The ready card over a new ride: GPS, battery 12 %, a power meter not heard (sim) |
