# Checklist

Comparison of this firmware with stock, the patch project and newer units, one line per feature.
Not here: how each feature works and its numbers (features.md), what the unit has not shown (status.md), what is not built, in order (roadmap.md).
Where to look: [features.md](features.md) for how each one works. The tags say what the line is compared with.

Legend:

- `- [x]` done: works on the unit ("unit") or so far only in the host simulator ("sim");
- `- [~]` partly: the few words after it say what is missing;
- `- [ ]` not yet.

Source tags: (stock) the S510's own firmware R039; (patch P#) bryton-S510-mods; (Garmin), (Wahoo), (Karoo), (Coros),
(S810) Bryton's newer units; (ours) only here. A line with several tags is offered by all of them.

## Screens and pages

- [x] Status strip on every screen: BLE, GPS bars, time, ride state, USB, battery (stock): unit
- [x] Home screen with time, date, chosen fields and notices (stock, Garmin): unit
- [x] Up to three data pages per profile (stock): unit
- [x] Layouts of one to six fields, uneven ones too (stock, Garmin): sim
- [x] Edit a page's fields on the unit (stock, Garmin, Wahoo): sim
- [~] More than three pages, pages turned on and off (Garmin, Karoo): sim; pages added, deleted, switched off and moved
- [~] Rename pages (Garmin): sim, four characters
- [x] Field size follows the box: a value's scale is the largest its box's width (five characters) and height allow, 8x in a 1-field page, and a ring sizes it to its hole (Garmin, Karoo): sim; wide boxes showing more (units, a second value) are not done
- [x] Banners over any page for a few seconds (stock, Garmin): sim
- [~] Quick Status panel (stock): sim; a swipe-down panel, and a tap on a field opens six chosen fields
- [x] Ride summary at the end (stock, Garmin, Wahoo, Karoo): sim
- [x] Lap summary banner (Garmin, Wahoo): sim
- [x] Day and night colour modes (stock, Garmin); themes DARK, OUTDOOR and LIGHT and theme files on the card are in (ours), and THEME AUTO switches them from the light: sim
- [~] Ring and bar gauges for speed, cadence, heart rate, power (stock): sim; POWER RING, HR RING and CAD BAR fields (ring and bar drawn once, then only the arc or columns that moved); speed has none; not seen on the unit
- [x] Graphs on a page: power, heart rate, W' balance, DFA a1, elevation (stock, ours): sim
- [x] Values in zone colours (stock): unit
- [x] Font choice for every text, PIXEL or SMOOTH (ours): unit
- [x] Cog on the overview opens the menu (ours): sim
- [~] Auto scroll through pages (stock, Garmin): sim
- [x] Screen lock by holding Back and Lap (stock, Garmin): sim
- [x] Ready-to-ride dashboard: sensors, battery, GPS before the start (Wahoo): sim; a two-line card over the ride, which starts at the press
- [x] Indoor mode pages (stock, Garmin, Wahoo): unit
- [ ] Languages other than English (stock)
- [~] Imperial units (stock): sim; speed, distance, ascent and altitude, the totals and the trips; the course fields stay metric
- [x] Redraw only what changes, in the smallest box (ours): unit
- [ ] Lap list with each lap's numbers in a past ride (stock, Garmin)
- [ ] Power phase page (stock, Garmin)
- [ ] Wind page: head or tail wind and its cost in speed (Wahoo)

## Data fields

Speed and distance

- [x] Speed, average, maximum (stock, all): unit
- [x] Distance (stock, all): unit
- [x] Trip 1 / Trip 2 counters, reset by hand (stock): sim
- [x] Odometer: recorded or all distance (stock, Garmin): unit

Time

- [x] Time of day (stock, all): unit
- [x] Ride timer, moving time (stock, all): unit
- [x] Trip time, elapsed with stops (stock, all): sim
- [x] Lap time and lap distance (stock, all): unit
- [x] Last lap time, lap count (stock, all): sim
- [x] Sunrise, sunset, time to sunset (stock, Garmin): sim
- [x] Time and distance to destination, estimated arrival (stock, Garmin, Wahoo, Karoo): sim; the distance is the course's distance to go

Altitude and climbing

- [x] Ascent (stock, all): sim, from the barometric altitude, GPS as the fallback
- [x] Altitude (stock, all): unit, barometric, calibrated by hand
- [x] Descent, max altitude (stock, all): sim
- [x] Lap ascent and descent (stock): sim
- [x] Uphill and downhill distance (stock): sim
- [x] Grade (stock, all): sim
- [x] VAM (Garmin, Karoo): sim

Heart rate

- [x] Heart rate, average, maximum (stock, all): unit
- [x] Heart rate as % of max and of LTHR (stock, Garmin): sim
- [x] Heart rate zone (stock, all): sim
- [~] Time in the heart rate zone of now (Garmin, Karoo): sim; not each zone's time
- [x] DFA a1, live from the strap's beats (patch P9, Karoo, Garmin): sim
- [x] RMSSD (ours): sim

Cadence

- [x] Cadence and average cadence (stock, all): sim
- [x] Maximum cadence (stock): sim

Power

- [x] Power, average, maximum (stock, all): sim
- [x] 3, 10 and 30 s power (stock, patch P6): sim
- [x] Power smoothing chosen on the big power value, kept across restarts (patch P5): sim
- [x] NP, IF, TSS (stock, Garmin, Wahoo, Karoo): sim
- [x] kJ and kcal (stock, all): sim
- [x] W/kg (stock, Garmin): sim
- [x] W' balance (patch P10, Garmin, Karoo, Wahoo): sim
- [x] Left / right balance (stock, Garmin): sim
- [x] Average left / right balance (stock): sim
- [~] Torque effectiveness and pedal smoothness (stock, Garmin): sim; not yet seen on the unit (a meter may not send page 0x13)
- [ ] Cycling dynamics: power phase, platform centre offset, seated / standing (Garmin, S800)
- [~] % of FTP: current, average, lap, 3 / 10 / 30 s (stock): sim; no last lap's
- [x] Power zone (stock, all): sim
- [~] Time in the power zone of now (stock, Garmin): sim; not each zone's time
- [x] Lap power, last lap power, lap NP (stock, Garmin, Wahoo): sim
- [ ] Real-time stamina (Garmin)
- [ ] Power curve, best efforts this ride (Karoo, Garmin)

Others

- [x] Temperature (stock, all): unit
- [x] Heading, compass (stock, Garmin): sim
- [~] Gears, gear ratio, gear combo, Di2 battery (stock, Garmin, Wahoo, Karoo): sim; gear fields and shift counts, not seen on the unit
- [ ] E-bike: battery, range, assist mode and level (stock, Garmin, Wahoo, Karoo)
- [x] Workout fields: target power, cadence, heart rate, time left, interval count (stock, all): sim
- [~] Navigation fields: turn info, distance to POI, to peak, to destination (stock, all): sim; distance to go, off course, the climb, ascent left, the next peak or col (NEXT POI); no turns
- [ ] Road surface type (Garmin)
- [x] Battery of the unit and of each sensor as fields (Garmin, Wahoo): sim; the Di2 and the power meter and HR strap (ANT+ status), not seen on the unit
- [ ] Component wear: the bike chosen per ride, kilometres added per part (Garmin)

## Sensors and radios

- [x] ANT+ heart rate strap, paired and kept (stock, all): unit
- [x] ANT+ beat-to-beat intervals for HRV (patch P8, Garmin): sim
- [x] ANT+ speed, cadence, speed and cadence (stock, all): sim
- [x] ANT+ power (stock, all): sim
- [~] Power curve screen: the 90-day curve and this ride's over it, CP and W', a cursor by key and tap (FITNESS, ride menu): sim; not yet seen on the unit
- [~] Power meter calibration: zero offset, auto zero (stock, Garmin, Wahoo, Karoo): sim; not yet seen on the unit
- [~] Sensor pairing menu: pair, forget (stock, all): unit; one sensor per kind; the maker's short name in the row (ANT+ common page 80): sim
- [ ] Sensor pool: several of a kind, switch when another is found (stock, Garmin)
- [~] Sensor battery levels (ANT+ common page, stock, all): sim; the heart rate strap's percent (page 7) on HR BATT too; not seen on the unit
- [ ] BLE heart rate, speed and cadence, power (stock, all)
- [~] BLE scan of nearby devices with signal strength and kind (ours): unit; no connection yet
- [ ] ANT+ radar (Garmin, Wahoo, Karoo, Coros): vehicles beside the page, threat colours
- [ ] ANT+ lights: on with the ride, by ambient light (stock, Garmin, Wahoo)
- [~] Shimano Di2 gears and hood buttons (stock, patch P12, Garmin, Wahoo, Karoo): sim; private ANT channel
- [~] SRAM AXS / ANT+ Shifting gears and buttons (stock, Garmin, Wahoo, Karoo): sim
- [ ] E-bike: Shimano STEPS and ANT+ LEV (stock, Garmin, Wahoo, Karoo)
- [~] Smart trainer, ANT+ FE-C (stock, all): unit; paired and read, ERG not yet seen on the trainer; sim for release rules
- [x] Trainer let go when a workout, a course or the ride ends or pauses (ours): sim
- [~] Smart trainer over BLE FTMS (Garmin, Wahoo, Karoo, Coros): sim; the chip accepts both links on the unit
- [~] Smart trainer over BLE without FTMS, FE-C over BLE (Tacx scheme): unit; linked and read, commands not yet seen applied
- [ ] Remote buttons (stock, Garmin)
- [ ] Running dynamics / MTB fork and shock sensors (Garmin, Karoo)
- [ ] Muscle oxygen and core temperature sensors as fields (Garmin, Wahoo)
- [x] GPS (u-blox M10), with last position at power-up for a fast fix (stock, all): unit
- [ ] GNSS systems chosen per profile (GPS, Galileo, GLONASS, BeiDou, QZSS) (stock, all)
- [ ] Dual-frequency GNSS (Garmin, Wahoo, Coros)
- [ ] AssistNow offline data from the update tool or the phone (stock)
- [x] Barometer: pressure and temperature (stock, all): unit
- [x] Barometric altitude, calibrated by hand, from GPS, at a ride's start or at a saved place (stock, Garmin): sim
- [x] Ambient light sensor (stock): unit
- [ ] Accelerometer use (crash, motion start) (stock, Garmin)
- [ ] Microphone (stock: voice search and group chat input)

## Ride recording and files

- [x] Start, pause, resume, lap, end (stock, all): unit
- [x] Auto pause under 2 km/h (stock, all): unit
- [x] Auto pause threshold set by hand (Garmin, Wahoo): sim; AUTO, OFF, 2, 3, 5, 8 km/h
- [~] Auto lap by distance or time (Garmin, Wahoo, Karoo): sim; by position is not in
- [x] Start reminder when the bike moves (stock, Garmin): sim
- [~] FIT activity file written as the ride goes (stock, all): sim
- [~] Checkpoint every minute, two copies of the recovery record, resume after any reset (stock): sim, power cut tested after every sector
- [~] Rides saved to \Activities, readable by stock and the computer (stock, all): sim
- [x] Discard a ride (stock, all): sim
- [ ] Recording rate: 1 s or smart (stock, Garmin)
- [~] HRV (RR intervals) in the FIT file as hrv messages (patch P8, Garmin): sim; not yet on a real ride with the strap
- [~] DFA a1 in the FIT file as the Alpha1 developer field (patch P9, Garmin): sim
- [~] Laps, session and activity messages at the end (stock, all): sim
- [~] AUTO OFF (SYSTEM): switches off after idle (stock, Garmin): sim; never with a ride open
- [~] Bike profiles: road, MTB, gravel, cyclocross, commute, indoor, three bikes (stock, all): sim; ROAD, INDOOR and MTB, no bikes
- [ ] Settings in a file, two copies (stock, all): backup registers for now
- [~] Screenshots saved to the volume (stock): sim; SYSTEM > SCREENSHOT, `\Screenshot\YYYYMMDDhhmmss.bmp`, no ride open
- [~] Debug log file after a fault (stock, patch D1): sim; `\Rustfw\Log\faults.log`, a line per fault (kind, panic line or CFSR bits), no PC / LR

## Navigation and maps

- [x] Track since power-on on a map, heading arrow, fitted view, scale bar (stock, all): unit
- [x] Last known position before a fix (ours): unit
- [~] Basemap: roads and water from stock's tile cache (stock, all): sim with real tiles; not seen on the unit
- [~] Map zoom and pan by touch (stock, all): sim; pinch to zoom in the driver (two fingers not seen on the unit)
- [~] Map heading up or north up (Garmin, Wahoo): sim (DISPLAY > MAP; turned basemap, track, arrow, north arrow)
- [~] IGN Plan v2 maps from packs made on the computer, "(c) IGN" on the map (ours): sim; not seen on the unit
- [ ] Our own OpenStreetMap packs over USB (all have region maps)
- [~] Contour lines, POIs, street names on the map (Garmin, Wahoo, Karoo): in IGN's tiles, France only
- [x] Follow a course (FIT or GPX): line on the map, distance to go (stock, all): sim
- [~] Off-course alert, back on track (stock, Wahoo, all): sim; the alert and the distance off, no way back drawn
- [~] Navigate to the start of a course first (stock): sim; a straight line (compass point and distance), no routing
- [x] Reverse a course (stock, Garmin): sim
- [~] Completed climbs listed with time, VAM, power (Garmin ClimbPro summary): sim; CLIMBS DONE in the ride menu, not kept across a reset
- [x] The way back: the ride's own track followed back as a course (Garmin Back to Start): sim
- [x] Turn-by-turn cues (stock, all): sim
- [ ] Voice turn-by-turn on a speaker (Garmin, Wahoo, Karoo)
- [ ] Rerouting on the unit (stock, all)
- [ ] Retrace / back to start along the ride's own path (stock, Garmin, Wahoo, Karoo)
- [ ] Route to a pinned point on the map (stock, Garmin, Wahoo)
- [ ] Route to a saved place or POI (stock, Garmin, Wahoo, Karoo)
- [ ] Address and POI search (stock, Garmin, Wahoo)
- [ ] Round trip / Surprise Me routes (stock, Garmin)
- [ ] Popularity routing (Garmin, Karoo)
- [ ] Course creator on the unit (Garmin)
- [~] POIs and peaks along a course (stock, Garmin): sim; peaks, volcanoes and cols with their elevation from stock's cached tiles (NEXT POI field, marks and the next name on the profile), where the cache has them; no water points or shops; not seen on the unit
- [ ] Live segments (Strava) (stock, Garmin, Wahoo, Karoo) (ours: page, banners, result card, `sim`; the effort is not saved yet)
- [ ] Segments from the rider's own rides (best sections ridden twice), no Strava (ours)
- [ ] Road closures and hazards on the route (Garmin)
- [ ] Favourite and recent places, navigate to one (stock)
- [~] Course sync from Strava, Komoot, RideWithGPS (stock, all): sim; copied over USB, no app
- [ ] Virtual ride: a course on a smart trainer (stock, Wahoo)

## Training: zones, workouts, plans, metrics

- [x] Heart rate zones from max HR (stock, all): sim
- [x] Heart rate zones from LTHR (Friel), zones edited one by one (stock, Garmin): sim
- [x] Power zones from FTP (Coggan) (stock, all): sim
- [x] Power zones edited one by one (stock, Garmin): sim
- [x] Weight and W' settings (Garmin, ours): sim
- [x] Bike weight setting for the climb pacer and the trainer (ours): unit
- [x] Rider profile: sex, birth year, height (stock, Garmin): sim
- [x] Structured workouts from FIT files, step targets (stock, all): sim
- [x] Countdown beeps before a step ends (patch P1, Garmin, Wahoo): sim
- [x] Workout profile graph with the rider's place (Garmin, Wahoo, Karoo): sim
- [x] Target gauge: power or heart rate on a bar around the step's band (Garmin, Wahoo, Karoo): sim
- [x] Next-step strip instead of a full-screen popup (patch P3): sim
- [x] Multiple targets per step (power and cadence) (Garmin): sim
- [ ] ZWO / ERG / MRC workouts, converted on the computer (Wahoo, Karoo)
- [x] Workouts synced from TrainingPeaks, intervals.icu (stock, all): sim
- [x] Workout calendar: today and the next days (stock, Garmin, Wahoo): sim
- [~] ERG mode to the workout target (stock, all): sim; not yet seen on the trainer
- [~] Trainer resistance and power modes, by hand (stock): sim; level mode (LVL, 1 to 20) sim only
- [~] Trainer slope from the course grade, with a scale of 100, 75, 50 or 25 % (Garmin, Wahoo, Karoo): sim
- [~] FTP ramp test indoors (stock, Garmin, Coros): sim; no max HR or LTHR test, not yet on the trainer
- [~] Trainer level mode (stock): sim
- [ ] A virtual course turned into a workout (stock)
- [ ] AI Coach / Pace Pilot on the unit: pacing sections and carbohydrate along a course (stock, from the app)
- [ ] Progress Planner: plan that adapts to load and fatigue (stock app, S810)
- [ ] Targeted adaptive coaching / daily suggested workouts (Garmin)
- [ ] Pace Pilot: pacing plan for a course, with eat and drink reminders (stock app, S810)
- [ ] Power Guide: power targets along a course (Garmin)
- [~] Training load, fitness / fatigue / form (CTL, ATL, TSB) (stock app, Garmin, Coros, Karoo): sim (CTL, ATL, TSB fields, FITNESS > TRAINING LOAD chart); not yet seen on the unit; hrTSS of a ride without power from its average heart rate
- [ ] Training status, training readiness, recovery time (Garmin)
- [ ] Load focus, aerobic / anaerobic split (Garmin, Coros)
- [ ] Power curve and rider profile (stock app, Garmin, Karoo)
- [~] VO2max and FTP estimates from rides (Garmin): sim; no VO2max
- [x] Aerobic and anaerobic thresholds from DFA a1 over recent rides (ours): sim; not yet on real rides
- [ ] Heat and altitude acclimation (Garmin)
- [x] hrTSS when there is no power meter (ours, intervals.icu): sim
- [x] DFA a1 against the aerobic threshold during a ride (patch P9): sim; banner under a set limit (default off), hold 5 s, hysteresis

## Climbing

- [x] Elevation profile graph of the ride (Garmin, Karoo): sim
- [x] Climb detection on a course: length, grade, what is left (stock, Garmin, Wahoo, Karoo): sim; a banner and a beep at each start
- [~] Climb page that opens by itself at a climb's start (stock, Garmin): sim; no off switch
- [~] Climbs without a course, predicted from the road ahead (stock, Karoo, Garmin): sim; not seen on the unit
- [~] Descents and a steep descent warning (Garmin, Karoo): sim; STEEP DESCENT 8, 10 or 12 % banner 300 m ahead, DESCENT AHEAD field; not seen on the unit
- [ ] Climb filter: all or medium and above (stock)
- [x] Gradient-coloured climb profile in sections (stock, all): sim
- [x] Pacing on a climb from FTP and W' balance (ours, Garmin): sim; not yet on a real climb
- [ ] Summit segments / climb challenges with others (Wahoo, Garmin)
- [ ] Completed climbs listed with their times (Karoo)
- [ ] Climb detection on or off per profile, descents too (Garmin, Karoo)

## Alerts and safety

- [x] Beeps at power on and off (stock): unit
- [ ] Key click (stock)
- [x] Sound on or off for alerts (stock): sim; off silences every tone, the power-on and power-off melodies stay
- [~] Eat, drink, rest reminders by trip time, clock, distance, kcal, kJ, heart rate (stock, Garmin, Wahoo): sim; ride time, distance, kcal, kJ; no clock or heart rate reminder
- [x] Heart rate, power, cadence, speed high / low alerts (stock, Garmin, Wahoo): sim
- [ ] Custom alerts with text (Wahoo, Karoo)
- [~] Notices: GPS fix, sensor paired, battery low (stock, all): sim, on the overview
- [x] Turn and off-course beeps (stock, all): sim
- [ ] Bike bell on the speaker or buzzer (Garmin, Wahoo)
- [ ] Radar alerts (stock, all)
- [ ] Incident detection with the accelerometer (Garmin, Karoo, Wahoo)
- [ ] Group ride incident detection (Garmin)
- [ ] Road hazards, crowd-sourced (Garmin)
- [ ] Bike alarm, find my Edge (Garmin)
- [ ] Assistance request to contacts (Garmin)
- [x] Sunset alert (Garmin): sim (ALERTS > SUNSET ALERT, 15 / 30 / 60 min before the set, once a ride)
- [x] Time zone and summer time (Garmin, Wahoo): sim (DISPLAY > ZONE, CLOCK or UTC - 12 to + 14 in quarter hours;
  SUMMER TIME OFF, EU or US; a ride across the change keeps its name and its FIT times)

## Phone and connectivity

- [x] Pair with a phone (BLE bonding) (stock, all): sim
- [x] Call, text and e-mail notifications: iOS through ANCS, no app (stock app, all): unit for calls; sim for messages and decline
- [ ] Android notifications (stock app, all): need a companion app
- [x] Media remote: play, pause, next, previous, volume (patch P7, Garmin, Wahoo, Karoo): sim
- [x] Now playing: title, artist, elapsed (ours): sim
- [ ] Media remote from Di2 buttons (patch P12)
- [x] Camera shutter on the phone (ours): sim
- [ ] Live tracking (stock app, all): needs a phone app
- [ ] Group ride: members on the map, messages, voice input (stock app, Garmin, Karoo)
- [~] Ride upload to Strava and Intervals.icu (stock app, all): sim; `tools/sync.py` on the computer, not yet run against the real services
- [~] Planned workouts from Intervals.icu onto the unit (Garmin, Wahoo, Karoo): sim
- [ ] Wi-Fi sync without the phone (Garmin, Wahoo, Karoo, Coros)
- [x] Time zone from the phone (ours): sim
- [ ] Settings edited from the phone (stock app, all)
- [ ] Contactless payments (Garmin)
- [ ] Apps and extensions (Garmin, Karoo)

## Device: battery, backlight, touch, buttons, storage, USB, updates

Battery and power

- [x] Battery percentage from the voltage (stock): unit
- [~] Switch off at low battery (stock): sim; the 3.3 V switch-off not reached on the unit
- [x] Battery history (ours): unit
- [ ] Charging only in a safe temperature range (stock): charger on whenever USB is in
- [~] Battery hours left (Garmin, Coros): sim; BATT HRS fits the drain of the last 20 minutes on the battery, "--" while it is under 3.4 % an hour; not seen on the unit
- [ ] Battery saver mode (Garmin, Wahoo, Coros)
- [ ] Stop mode between seconds (ours): the largest battery gain left
- [ ] Battery life as stock claims (stock): to measure
- [x] Power off by holding Back (stock): unit

Backlight and panel

- [~] Backlight levels and time-out (stock, all): unit; the levels and time-out still to see by eye
- [~] Automatic backlight from the light sensor (stock, all): unit; outdoors still to see by eye
- [x] Backlight always on (stock): LIGHT OFF NEVER (the time-out choice 0)
- [ ] Panel partial and idle modes (ours)

Touch and buttons

- [x] Four keys: Page, Lap, Start, Back, with holds (stock): unit
- [x] Touch: swipe between pages, hold for the menu, tap a menu row (stock): sim
- [~] Swipe up from the bottom to go home (stock): sim, on a ride page after MORE
- [x] Tap a field to open Quick Status (stock): sim (PAGES > QUICK STATUS, QUICK FIELDS; Back, tap or 10 s closes)
- [ ] Long press a field to change it (stock, Garmin, Karoo)
- [ ] Pinch to zoom the map (Wahoo, Garmin)
- [ ] Digital dial (Coros): no such hardware
- [x] Key repeat when a number is held (ours): unit
- [ ] Page key changes page before the click (patch P14)
- [x] Pages kept and fields refreshed at once (patch P17): sim

Storage, USB and updates

- [x] eMMC read: layout, FAT32 volume, files (stock): unit
- [~] eMMC and FAT32 writes, safe through a power cut (stock): sim
- [~] USB mass storage: the BRYTON drive on a computer (stock, all): sim; not yet seen on a computer
- [x] USB dev link: screenshots, keys, peek and poke, run from RAM (ours): unit
- [ ] Update this firmware from a file on the volume (stock)
- [ ] Update over BLE from the phone (stock app, all)
- [x] Way back to stock: the bootloader's recovery mode (stock bootloader): unit
- [x] Shares the volume with stock, so the two can be swapped (ours): unit; read only so far
- [ ] GPS assistance and map updates from the computer (stock Update Tool)
- [ ] Memory: free space, delete old rides (stock)

## Settings

- [x] Wheel size in mm (stock, all): unit
- [x] Backlight level and time-out (stock): unit
- [x] MAX HR, FTP, weight, W' (stock, ours): unit for MAX HR and FTP; sim for weight and W'
- [x] Overview fields and pages (stock): sim
- [x] Menus in groups, one screen each (Garmin, Wahoo): sim
- [x] The menu tree: WORKOUTS, COURSES, HISTORY, PAGES, SENSORS, SETTINGS (ME, ALERTS, DISPLAY, BIKE, SYSTEM): sim
- [x] FONT (DISPLAY): PIXEL or BARLOW, kept in the settings file, the smooth cells the same size as the pixel ones: sim
- [x] ABOUT and STORAGE (SYSTEM): the version and commit, the map and font notices; the card's size, free space, rides and
  oldest rides (each opens HISTORY's card, DELETE there), read over the passes: sim
- [x] The laps of a past ride (HISTORY, the ride, LAPS): number, time, distance, speed, power, heart rate; NO LAPS: sim
- [x] A ride menu while riding (Garmin, Karoo): sim
- [x] GPS status: fix, satellites, position, altitude, UTC (stock, Garmin): unit
- [~] Settings kept through power off (stock, all): unit; backup registers, not a file
- [ ] Time zone, daylight saving, date and time formats (stock, all)
- [x] Units: metric or imperial, temperature (stock, all): sim
- [x] Date format D/M/Y, M/D/Y, Y-M-D (ours): sim (fields unit test)
- [x] Climbs ahead ALL or MEDIUM+, CLIMB PAGE on or off (ours): sim
- [ ] Language (stock, all)
- [x] First start setup: stock's settings and pages taken by themselves (ours): sim
- [x] Import stock's settings: FTP, max HR, weight, W', wheel, paired sensors, backlight, reminders, pages (ours): sim
- [ ] Auto features per profile: smart pause, climb page switch, climb approach, map view (stock)
- [ ] Data reset: trips, odometer, factory settings (stock)
- [ ] About: firmware version, UUID, position (stock): over USB only
- [x] Altitude calibration at saved places (stock): sim
- [x] Favourite places: SAVE PLACE, PLACES with distance and bearing, altitude by hand, DELETE (stock, Garmin): sim
- [x] Sensor names and gear teeth for Di2 (stock): sim (CHAINRINGS and CASSETTE presets); teeth from the Di2 itself open
- [x] Remote button actions for Di2 hood buttons (stock, patch P12): sim (DI2 KEYS)

## History and analysis

- [~] Ride list on the unit with summary and details (stock, all): sim; no laps or graphs per ride
- [x] Delete rides on the unit (stock): sim
- [~] Totals: week, month, year, odometer (stock, Garmin): unit; week, year and total; FITNESS > MONTHS rows (km, hours, rides, ascent, TSS): sim
- [ ] Personal records and best efforts (Garmin, Karoo)
- [~] Power curve over season and all time (stock app, Garmin): sim; bests for CP and W' only, no screen, no season
- [~] Training load chart on the unit (stock app, Garmin): sim, not yet on the unit
- [ ] Each ride matched to the planned workout (ours)
- [x] Rides read from stock's \Activities too (ours): sim
- [ ] Ride upload to intervals.icu or Strava from a computer tool (ours)

## Climbs ahead and live segments: what it would take

Researched 2026-10-26 (stock reverse engineered, other units from their docs and reviews).

Climbs without a course:
- **Stock (Climb Challenge 2.0)** follows the road ahead in its own map's routing graph (`\Map\MapData\<region>\*.dat`
  section 1, z14 tiles, already on the unit): the same road, else the same road class, else the smallest turn, under
  110 degrees. It fits straight pieces to the smoothed elevation, keeps pieces over 2 % as a candidate (a dip up to
  150 m kept inside), drops candidates under 300 m, glues climbs up to 2 km apart while the whole stays over 3 %, and
  keeps a climb over 300 m (1 km with a setting), over 3 % and over 12 m of gain.
- **Garmin ClimbPro** in free ride (Edge 540 and newer) and **Karoo Climber** (Predictive Path) do the same from the
  map: Karoo leaves out climbs under 400 m or under 3 % and filters by size; Garmin's thresholds are not published
  (500 m and 3 % is the forum's reading).
- **Here (built 2026-10-26, src/nav/climbs_ahead.rs):** the elevation is a node field of the routing graph (`field / 5 - 500`
  m, STORAGE_AND_MAPS.md 4.9), and the graph's keys are sign and magnitude (found on a real file, storage/route.rs); the
  climb finder of `course.rs` runs on the traced road as on a course (Garmin's definition: one rule for both, rather
  than stock's, which the notes above keep for a comparison on the unit).

Live segments:
- **Stock** has the whole feature (pages, a ghost that follows each target's real splits point by point: KOM, QOM,
  PR, rivals; finished and lost screens), but only Bryton's app fills `\PlanTrip\LiveSegment`, for Strava
  subscribers. The package format is reconstructed (live_segments.c); stock never imports a package copied over USB.
- **Garmin, Wahoo, Karoo** get Strava's starred segments through their apps, Strava subscription required.
- **Strava's API** still lists starred segments and segment streams, but the 2024 terms limit what an app may do and
  since 30 June 2026 its Standard tier needs the developer's own Strava subscription (checked 2026-10-29,
  docs/segments.md). A personal tool that fetches the owner's segments is allowed; KOM split times are not exposed
  without the Live Segments licence, so a KOM ghost would run at a constant pace. The plan: docs/segments.md.
- **Here, without Strava:** segments from the rider's own rides (a stretch ridden more than once, the best effort as
  the ghost), matched by GPS on the unit as `course.rs` matches a course, are possible with what is on the card.

## Sources

- Bryton Rider S510 user manual (2024): https://zerge.hu/storage/files/uploads/bryton-rider-s510-user-manual.pdf
  (also https://www.manualslib.com/manual/3705555/Bryton-Rider-S510.html)
- S510 retailer pages: https://www.tiso.com/products/bryton-rider-s510-black, https://www.bike24.de/p11049274.html
- The Sweet Cyclists, S510 review: https://thesweetcyclists.com/bryton-rider-s510-gps-bike-computer-color-touchscreen-review/
- Bryton "Your Intelligent Pilot" for S810 and S510 (Pace Pilot, Progress Planner, PMC, Rider's Insight):
  https://www.interempresas.net/Ciclismo/659802-Nuevas-funciones-inteligentes-para-los-Rider-S810-y-Rider-S510-de-Bryton.html
- Bryton S810: https://www.sigmasports.com/item/Bryton/Rider-S810-GPS-Cycling-Computer/171FX
- Stock firmware, reverse engineered: bryton-S510-mods docs/how-it-works.md (reminders, ride saving, touch, battery),
  the local Ghidra notes (STRUCTURE.md tasks, STORAGE_AND_MAPS.md folders, HARDWARE.md)
- Patches P1 to P17: https://github.com/taladjidi/bryton-S510-mods
- DC Rainmaker, Garmin Edge 1050: https://dcrainmaker.com/2024/06/garmin-edge-1050-in-depth-review-brilliance-battery.html
- DC Rainmaker, Garmin Edge 840: https://dcrainmaker.com/2023/04/garmin-edge840-cycling.html
- Garmin Edge 1050 long-term review: https://headless-velo.outsideonline.com/road/road-gear/garmin-edge-1050-long-term-review-2026
- Granfondo, Edge 1050 (hazards, bell): https://granfondo-cycling.com/garmin-edge-1050-review/
- ClimbPro: https://the5krunner.com/garmin-features/navigation/climbpro/
- the5krunner, Wahoo ROAM 3: https://the5krunner.com/2025/07/09/wahoo-roam-3-review/
- bikepacking.com, Wahoo ROAM 3: https://bikepacking.com/gear/wahoo-elemnt-roam-3-review/
- Wahoo and Varia radar: https://road.cc/content/tech-news/265979-yes-wahoo-computers-now-compatible-garmins-varia-radar-lights
- Wahoo planned workouts: https://dcrainmaker.com/2017/08/planned-workout-elemntbolt.html
- Hammerhead Karoo Climber without a route: https://www.slowtwitch.com/industry/hammerhead-karoo-2-long-term-review-predictive-path-technology/
- Karoo Climber: https://bikerumor.com/garmin-updates-navigation-and-hammerheads-karoo-2-gets-precise-elevation-mapping-with-climber/
- Karoo change log: https://www.eu.hammerhead.io/blogs/change-logs/karoo-software-build-version-1-559-2149
- BikeRadar, Coros Dura: https://www.bikeradar.com/reviews/training/gps-computers/coros-dura-review
- 220 Triathlon, Coros Dura: https://www.220triathlon.com/gear/bike/accessories/bike-computers/coros-dura-review-the-brands-first-bike-computer-impressed-me-in-several-ways
- Garmin incident detection: https://www8.garmin.com/manuals/webhelp/edge520plus/EN-GB/GUID-F109FB24-B55D-49D2-B388-D042E7FBA965.html
- Apple ANCS: https://developer.apple.com/Library/ios/documentation/CoreBluetooth/Reference/AppleNotificationCenterServiceSpecification/Introduction/Introduction.html
- FIT workout files: https://developer.garmin.com/fit/articles/file-types/workout.html
- Garmin June 2026 update (component wear, Bosch): https://www8.garmin.com/wearables/PDF/WearablesSoftwareUpdate/2026/June2026.pdf
- Garmin Edge 32.20 (climb and descent detection per profile): https://www.garminnews.com/garmin-releases-fresh-stable-update-32-20-for-edge-1050-850-550-and-mtb-whats-new/
- Garmin Edge 1060 (hazards, road closures, voice): https://escapecollective.com/garmins-latest-releases-include-a-new-flagship-1060-computer/
- Wahoo release notes (wind, Komoot live sync): https://support.wahoofitness.com/hc/en-us/articles/22860229203346
- Karoo Climber: https://support.hammerhead.io/hc/en-us/articles/25602163627163-Karoo-OS-CLIMBER
- ClimbPro without a course: https://the5krunner.com/garmin-features/navigation/climbpro/, scoring discussed at
  https://forums.garmin.com/sports-fitness/cycling/f/edge-830/168010/understanding-climbpro-on-the-edge
- Strava Live Segments need a subscription: https://www.cyclingweekly.com/news/product-news/strava-live-segments-188811
- Strava API reference (starred segments, segment streams): https://developers.strava.com/docs/reference/; the 2024
  terms: https://dcrainmaker.com/2024/11/stravas-changes-to-kill-off-apps.html; the 2026 paid plan:
  https://tryterra.co/blog/strava-api-changes-2026
- Stock's climbs ahead and live segments, reconstructed: the Ghidra repo's recovered_src/ride/route_climb.c and
  live_segments.c, STORAGE_AND_MAPS.md 4.7
