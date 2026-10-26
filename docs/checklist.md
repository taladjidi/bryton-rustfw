# Checklist

Every feature a rider could expect from this bike computer: what the stock S510 firmware does, what the patch
project (bryton-S510-mods) added, and what Garmin Edge, Wahoo ELEMNT, Hammerhead Karoo, Coros Dura and Bryton's
newer units offer. Tick a line when the feature lands; say where it was seen.

Legend:

- `- [x]` done: works on the unit ("unit") or so far only in the host simulator ("sim");
- `- [~]` partly: the few words after it say what is missing;
- `- [ ]` not yet.

Source tags: (stock) the S510's own firmware R039, from its user manual and the reverse engineering;
(patch P#) bryton-S510-mods; (Garmin), (Wahoo), (Karoo), (Coros), (S810) Bryton's newer units; (ours) only here.
A line with several tags is offered by all of them. Started 2026-10-24 from docs/features.md, docs/status.md and
the code; [features.md](features.md) says how each one works.

## Screens and pages

- [x] Status strip on every screen: BLE, GPS bars, time, ride state, USB, battery (stock): unit
- [x] Home screen with time, date and chosen fields (stock, Garmin): unit, overview with four fields (default: week, year and total km, week hours) and notices; apart from the ride pages as on stock
- [x] Up to three data pages per profile (stock): unit
- [x] Layouts of one to six fields, uneven ones too (stock has 12 grids) (stock, Garmin): sim, eleven layouts
- [x] Edit a page's fields on the unit (stock, Garmin, Wahoo): sim, on the page itself (rows of cells, `\Rustfw\pages.bin`)
- [~] More than three pages, pages turned on and off (Garmin, Karoo): sim, pages added and deleted in the editor, a MORE variant of each page (swipe up)
- [ ] Rename pages (Garmin)
- [ ] Field size follows the box, wide boxes show more (Garmin, Karoo)
- [x] Banners over any page for a few seconds (ride start, pause, lap, end) (stock, Garmin): sim
- [ ] Quick Status panel: sensors and their batteries, phone link, recording, five chosen fields (stock)
- [x] Ride summary at the end (stock, Garmin, Wahoo, Karoo): sim, RESUME / SAVE / DISCARD, then the ride card (map, elevation over distance, ten numbers)
- [ ] Lap summary banner (lap time, average power and heart rate) (Garmin, Wahoo)
- [ ] Day and night colour modes (stock Meter Color Mode, Garmin)
- [ ] Ring and bar gauges for speed, cadence, heart rate, power (stock SPD / CAD / HR / PW Ring and Bar)
- [x] Graphs on a page: power, heart rate, W' balance, DFA a1, elevation, by tap; whole ride by hold (stock, ours): sim
- [x] Values in zone colours (stock): sim, an 8 px zone bar on the box's top edge, following its number (Intervals.icu's palette); seen on the unit from RAM
- [x] Font choice for every text, PIXEL or SMOOTH (Barlow Semi Condensed Bold, OFL) (ours): unit, SETTINGS > DISPLAY > FONT seen on the panel, kept through a restart
- [x] Cog on the overview opens the menu (ours): sim, 44 px touch box, anti-aliased, level with the date (PIXEL and SMOOTH)
- [ ] Auto scroll through pages (stock, Garmin)
- [x] Screen lock (stock power key press, Garmin): sim, Back and Lap held 1 s lock the touch, a padlock in the strip
- [ ] Ready-to-ride dashboard: sensors, battery, GPS before the start (Wahoo Roam 3)
- [x] Indoor mode pages (no GPS, trainer) (stock, Garmin, Wahoo): unit, a second page set for the INDOOR profile, the workout page first while a workout runs
- [ ] Languages other than English (stock: German, Spanish, French...)
- [ ] Imperial units (stock, all)
- [x] Redraw only what changes, in the smallest box (ours): unit, 0.4 % awake on the main screen
- [ ] Lap list with each lap's numbers in a past ride (stock Results LapDetail, Garmin)
- [ ] Power phase page (stock MeterCyclePagePowerPhase, Garmin)
- [ ] Wind page: head or tail wind and what it costs in speed (Wahoo ACE, 2026)

## Data fields

Speed and distance

- [x] Speed, average, maximum (stock, all): unit (speed); sim (from a wheel sensor)
- [x] Distance (stock, all): unit
- [ ] Trip 1 / Trip 2 counters, reset by hand (stock)
- [x] Odometer: recorded or all distance (stock ODO, Garmin): unit, TOTAL KM on the overview from every ride on the card (stock's)

Time

- [x] Time of day (stock, all): unit
- [x] Ride timer (moving time) (stock Ride Time, all): unit
- [ ] Trip time (elapsed, stops included) (stock, all)
- [x] Lap time and lap distance (stock, all): unit
- [ ] Last lap time, lap count (stock LLapTime, Lap Count, all)
- [ ] Sunrise, sunset, time to sunset (stock, Garmin)
- [ ] Time and distance to destination, estimated arrival (stock, Garmin, Wahoo, Karoo)

Altitude and climbing

- [x] Ascent (stock Alt. Gain, all): sim, from the barometric altitude (median, 1/6 average, 3 m hysteresis, no step on a new calibration), GPS as the fallback
- [x] Altitude (stock, all): sim, barometric, calibrated (typed in 1 m steps: unit)
- [ ] Descent, max altitude (stock Alt. Loss, Max Altitude, all)
- [ ] Lap ascent and descent (stock)
- [ ] Uphill and downhill distance (stock)
- [x] Grade (stock, all): sim, from GPS altitude over 30 s
- [x] VAM (Garmin, Karoo): sim

Heart rate

- [x] Heart rate, average, maximum (stock, all): unit (heart rate); sim (average, maximum)
- [ ] Heart rate as % of max and of LTHR (stock MHR %, LTHR %, Garmin)
- [~] Heart rate zone (stock HR Zone, all): shown as colour, no zone number field
- [ ] Time in each heart rate zone (Garmin, Karoo)
- [x] DFA a1, live from the strap's beats (patch P9, Karoo AlphaHRV extension, Garmin Connect IQ): sim
- [x] RMSSD (ours): sim

Cadence

- [x] Cadence and average cadence (stock, all): sim (the decoder is checked against openant)
- [ ] Maximum cadence (stock Max CAD)

Power

- [x] Power, average, maximum (stock, all): sim
- [x] 3, 10 and 30 s power (stock, patch P6 before the start): sim, they move before the start too
- [ ] Power smoothing chosen on the big power value, kept across restarts (patch P5)
- [x] NP, IF, TSS (stock, Garmin, Wahoo, Karoo): sim
- [x] kJ and kcal (stock, all): sim
- [x] W/kg (stock, Garmin): sim
- [x] W' balance (patch P10 planned, Garmin Connect IQ, Karoo, Wahoo): sim
- [x] Left / right balance (stock CurPB-LR, Garmin): sim
- [ ] Average left / right balance (stock AvgPB-LR)
- [ ] Torque effectiveness and pedal smoothness (stock TE-LR, PS-LR, Garmin)
- [ ] Cycling dynamics: power phase, platform centre offset, seated / standing (Garmin, S800)
- [ ] % of FTP: current, average, lap, last lap, 3 / 10 / 30 s (stock)
- [~] Power zone (stock FTP zone, all): shown as colour, no zone number field
- [ ] Time in each power zone (stock, Garmin)
- [ ] Lap power, last lap power, lap NP (stock, Garmin, Wahoo)
- [ ] Real-time stamina (Garmin)
- [ ] Power curve, best efforts this ride (Karoo, Garmin)

Others

- [x] Temperature (stock, all): unit, from the barometer
- [ ] Heading, compass (stock, Garmin)
- [~] Gears, gear ratio, gear combo, Di2 battery (stock, Garmin, Wahoo, Karoo): sim, gear fields and shift counts; not seen on the unit
- [ ] E-bike: battery, range, assist mode and level (stock, Garmin, Wahoo, Karoo)
- [x] Workout fields: target power, cadence, heart rate, step and workout time left, interval count (stock, all): sim; the step's target, time left, NEXT with its length, the step count, WKT LEFT, ERG W
- [~] Navigation fields: turn info, distance to POI, to peak, to destination (stock, all): sim, distance to go, off course, the climb, ascent left; no turns or POIs
- [ ] Road surface type (Garmin)
- [ ] Battery of the unit and of each sensor as fields (Garmin, Wahoo)
- [ ] Component wear: the bike chosen per ride, kilometres added per part (Garmin, June 2026)

## Sensors and radios

- [x] ANT+ heart rate strap, paired and kept (stock, all): unit
- [x] ANT+ beat-to-beat intervals for HRV (patch P8, Garmin): sim, from a strap (HR seen on the unit)
- [x] ANT+ speed, cadence, speed and cadence (stock, all): sim (decoders checked against openant)
- [x] ANT+ power (stock, all): sim
- [~] Power meter calibration (zero offset, auto zero) (stock, Garmin, Wahoo, Karoo): sim; the page 0x01 layouts are from open implementations, SENSORS > CALIBRATE POWER with its screen, the sim meter and tests; not yet seen on the unit (Sigeyi AXO spider)
- [~] Sensor pairing menu: pair, forget (stock, all): one sensor per kind; no ID typed in, no names
- [ ] Sensor pool: several of a kind, switch when another is found (stock, Garmin)
- [ ] Sensor battery levels (ANT+ common page 82) (stock Quick Status, all)
- [ ] BLE heart rate, speed and cadence, power (stock, all)
- [~] BLE scan of nearby devices with signal strength and kind (ours): unit; no connection yet
- [ ] ANT+ radar (Garmin Varia, Bryton Gardia): vehicles beside the page, threat colours (stock, Garmin, Wahoo, Karoo, Coros)
- [ ] ANT+ lights: on with the ride, by ambient light (stock "Light" sensor, Garmin, Wahoo)
- [~] Shimano Di2 gears and hood buttons (private ANT channel) (stock, patch P12 ten actions, Garmin, Wahoo, Karoo): sim
- [~] SRAM AXS / ANT+ Shifting gears and buttons (stock e-shifting, Garmin, Wahoo, Karoo): sim
- [ ] E-bike: Shimano STEPS and ANT+ LEV (stock, Garmin, Wahoo, Karoo)
- [~] Smart trainer, ANT+ FE-C (stock, all): unit, paired and read (Van Rysel D500); ERG not yet seen on the trainer; sim, released at stop, end, pause, switch-off and once at boot, the release resent until TX_COMPLETED
- [x] Trainer let go when a workout, a course or the ride ends or pauses (ours): sim; on the D500 not yet seen
- [~] Smart trainer over BLE FTMS (Garmin, Wahoo, Karoo, Coros): sim, beside the phone; the two links accepted by the chip on the unit
- [~] Smart trainer over BLE without FTMS, FE-C over BLE (Tacx scheme; Van Rysel D500): unit, linked and read; commands not yet seen applied
- [ ] Remote buttons (Bryton ESS, Garmin Edge remote) (stock, Garmin)
- [ ] Running dynamics / MTB fork and shock sensors (Garmin, Karoo)
- [ ] Muscle oxygen (Moxy, Train.Red), core temperature (CORE) sensors as fields (Garmin, Wahoo announced 2026)
- [x] GPS (u-blox M10), with time and last position at power-up for a fast fix (stock, all): unit
- [ ] GNSS systems chosen per profile (GPS, Galileo, GLONASS, BeiDou, QZSS) (stock, all)
- [ ] Dual-frequency GNSS (Garmin 1050, Wahoo Roam 3, Coros; the S510's M10 is single band)
- [ ] AssistNow offline data from the update tool or the phone (stock)
- [x] Barometer: pressure and temperature (stock, all): unit
- [x] Barometric altitude, calibrated by hand, from GPS or at a ride's start (stock, Garmin): sim; not at saved places
- [x] Ambient light sensor (stock): unit
- [ ] Accelerometer use (crash, motion start) (stock start remind inferred, Garmin)
- [ ] Microphone (stock: voice search and group chat input)

## Ride recording and files

- [x] Start, pause, resume, lap, end (stock, all): unit (keys); banners in sim
- [x] Auto pause under 2 km/h (stock Smart Pause, all): unit
- [ ] Auto pause threshold set by hand (Garmin, Wahoo)
- [ ] Auto lap by distance, time or position (Garmin, Wahoo, Karoo)
- [ ] Start reminder when the bike moves (stock Start Remind, Garmin)
- [~] FIT activity file written as the ride goes (stock, all): sim (recorder.rs, eMMC writes), checked with Garmin's SDK and fitdecode
- [~] Checkpoint every minute, two copies of the recovery record, resume after any reset (stock, AGENTS.md): sim, power cut after every sector
- [~] Rides saved to \Activities, readable by stock and the computer (stock, all): sim (stock's name, the history lists them)
- [x] Discard a ride (stock, all): sim, moved into `\Rustfw\Discarded`, never erased
- [ ] Recording rate: 1 s or smart (stock, Garmin)
- [~] HRV (RR intervals) in the FIT file as hrv messages (patch P8, Garmin): sim, every beat raw in the full copy, with `ArtPct` on its records; not yet on a real ride with the strap
  (`\Rustfw\Activities`, five a message), cut at every sector, checked with fitdecode and Garmin's SDK; with the
  `ArtPct` share per record
- [~] DFA a1 in the FIT file as the Alpha1 developer field (patch P9, Garmin Connect IQ): sim, in a full copy beside the clean one (`\Rustfw\Activities`, with RMSSD), checked with Garmin's SDK
- [~] Laps, session and activity messages at the end (stock, all): sim (a lap message per Lap, timer events at pauses)
- [ ] Power off during a stop, resume at power on (stock Auto Power Off, Garmin)
- [~] Bike profiles: road, MTB, gravel, cyclocross, commute, indoor, three bikes (stock, all): ROAD / INDOOR, chosen by itself from the trainer heard (sub_sport in the FIT); no bikes
- [ ] Settings in a file, two copies (stock .ini files, all): in RTC backup registers for now
- [ ] Screenshots saved to the volume (stock \Screenshot): over USB only (`dev.py shot`)
- [~] Debug log file after a fault (stock \Debug, patch D1): sim, a fault resets the unit and is said at start (FAULT RESET n, backup register 29); no log file yet

## Navigation and maps

- [x] Track since power-on on a map, heading arrow, fitted view, scale bar (stock, all): unit; not on a real ride yet
- [x] Last known position before a fix (ours): unit
- [~] Basemap: roads and water from stock's tile cache (stock, all): sim with real tiles; not seen on the unit
- [~] Map zoom and pan by touch (stock, all): sim, "+" / "-" buttons and a drag; no pinch
- [ ] North up, heading up (stock Compass / Heading Mode, all)
- [~] IGN Plan v2 maps from packs made on the computer, "(c) IGN" on the map (ours; Garmin and Karoo have no IGN
  raster): sim with synthetic packs, real packs decoded on the computer; not seen on the unit
- [ ] Our own OpenStreetMap packs over USB (all have region maps)
- [~] Contour lines, POIs, street names on the map (Garmin, Wahoo, Karoo): in IGN's tiles (France only)
- [x] Follow a course (FIT or GPX): line on the map, distance to go (stock, all): sim, from stock's \PlanTrip (Bryton's own layout), Garmin FIT courses and GPX in \Courses
- [~] Off-course alert, back on track to the nearest point (stock, Wahoo Back on Track, all): sim, the alert and the distance off; no way back drawn
- [ ] Navigate to the start of a course first (stock)
- [ ] Reverse a course (stock, Garmin)
- [x] Turn-by-turn cues (stock, all): sim, from the file or the course's shape, said before and at each turn
- [ ] Voice turn-by-turn on a speaker (Garmin 1050, Wahoo Roam 3, Karoo)
- [ ] Rerouting on the unit (stock, all; Coros needs the phone)
- [ ] Retrace / back to start along the ride's own path (stock, Garmin, Wahoo, Karoo)
- [ ] Route to a pinned point on the map (stock, Garmin, Wahoo)
- [ ] Route to a saved place or POI (stock, Garmin, Wahoo, Karoo)
- [ ] Address and POI search (stock voice search over the phone, Garmin, Wahoo)
- [ ] Round trip / Surprise Me routes (stock SurpriseMeList inferred, Garmin)
- [ ] Popularity routing (Garmin Trendline, Karoo)
- [ ] Course creator on the unit (Garmin 1050)
- [ ] POIs and peaks along a course (stock, Garmin)
- [ ] Live segments (Strava) (stock LiveSegments, Garmin, Wahoo, Karoo)
- [ ] Segments from the rider's own rides (best sections ridden twice), no Strava (ours; Garmin Segments are account based)
- [ ] Road closures and hazards on the route (Garmin Edge 1060, live data)
- [ ] Favourite and recent places, navigate to one (stock My Favorites / History)
- [~] Course sync from Strava, Komoot, RideWithGPS (stock app, all): without an app, copied over USB: sim, the routes stock's app synced and GPX or FIT copied over USB
- [ ] Virtual ride: a course on a smart trainer (stock, Wahoo)

## Training: zones, workouts, plans, metrics

- [x] Heart rate zones from max HR (stock, all): sim (zone colours); MAX HR in the menu
- [ ] Heart rate zones from LTHR, zones edited one by one (stock, Garmin)
- [x] Power zones from FTP (Coggan, 7) (stock, all): sim; FTP in the menu
- [ ] Power zones edited one by one (stock, Garmin)
- [x] Weight and W' settings (Garmin, ours): sim
- [x] Bike weight setting (BIKE > WEIGHT), 5.0 to 25.0 kg in 0.1 kg, for the climb pacer and the trainer (ours): unit (the row seen, kept in register 16); the pacer on a real climb not yet
- [ ] Rider profile: sex, birthday, height (stock, Garmin)
- [x] Structured workouts from FIT files, step targets (stock, all): sim, from stock's \Workout\calendar
- [x] Countdown beeps before a step ends: 500 ms at 2 and 1 s left, 1 s at the change (patch P1's 3-2-1, Garmin, Wahoo; the owner's choice): sim
- [x] Workout profile graph with the rider's place (Garmin, Wahoo, Karoo): sim, the WKT GRAPH field
- [x] Target gauge: the power (3 / 10 / 30 s or instant) or heart rate on a bar around the step's band, coloured by how far off (Garmin, Wahoo, Karoo): sim, the WKT POWER field (GAUGE POWER in DISPLAY)
- [ ] Next-step strip instead of a full-screen popup (patch P3)
- [ ] Multiple targets per step (power and cadence) (Garmin)
- [ ] ZWO / ERG / MRC workouts, converted on the computer (Wahoo, Karoo)
- [x] Workouts synced from TrainingPeaks, intervals.icu (stock app, all): sim, read from what stock's app synced
- [x] Workout calendar: today and this week (stock, Garmin, Wahoo): sim, today and the next days (eight)
- [~] ERG mode to the workout target (stock Smart Workout, all): sim, Page / Lap adjust it; not yet seen on the trainer
- [~] Trainer resistance and power modes, by hand (stock): sim, the trainer screen (ERG, resistance, SIM); stock's level mode not done
- [~] Trainer slope from the course grade (Garmin, Wahoo, Karoo): sim
- [~] FTP, max HR and LTHR tests (stock Bryton Test, Garmin, Coros): FTP ramp test indoors (TrainerRoad's: 1 min steps from the FTP set, FTP 75 % of the best minute, HRVT1 from DFA a1 shown beside it): sim; no max HR or LTHR test, not yet on the trainer
- [ ] Trainer level mode (stock CourseLevelPlay) and a virtual course turned into a workout (stock VirtualToSmartWorkout)
- [ ] AI Coach / Pace Pilot on the unit: pacing sections and grams of carbohydrate along a course (stock AICoach, from the app)
- [ ] Progress Planner: seven-day plan that adapts to load and fatigue (stock app, S810)
- [ ] Targeted adaptive coaching / daily suggested workouts (Garmin)
- [ ] Pace Pilot: pacing plan for a course, with eat and drink reminders (stock app, S810)
- [ ] Power Guide: power targets along a course (Garmin)
- [ ] Training load, fitness / fatigue / form (CTL, ATL, TSB) (stock app PMC, Garmin, Coros, Karoo)
- [ ] Training status, training readiness, recovery time (Garmin)
- [ ] Load focus, aerobic / anaerobic split (Garmin, Coros)
- [ ] Power curve and rider profile (stock app Rider's Insight, Garmin cycling ability, Karoo)
- [~] VO2max and FTP estimates from rides (Garmin): sim, CP and W' fitted as an envelope from stock's rides, EST FTP and APPLY (estimates only raise the settings); no VO2max
- [x] Aerobic and anaerobic thresholds from DFA a1 over recent rides (HRVT1, HRVT2: Rogers 2021, Deming fit, 42-day median) (ours, AI Endurance's method): sim, from the full copies; not yet on real rides
- [ ] Heat and altitude acclimation (Garmin)
- [ ] hrTSS when there is no power meter (ours, intervals.icu)
- [ ] DFA a1 against the aerobic threshold during a ride (patch P9): field done, no alert

## Climbing

- [x] Elevation profile graph of the ride (Garmin, Karoo): sim
- [x] Climb detection on a course: each climb's length, grade, what is left (stock Climb Challenge 2.0, Garmin ClimbPro, Wahoo Summit, Karoo Climber): sim, Garmin's definition (500 m, 3 %), a banner and a beep at each start
- [ ] Climb page that opens by itself near a climb, can be turned off (stock Climb Page Switch, Garmin)
- [~] Climbs without a course, predicted from the road ahead (stock Climb Challenge 2.0, Karoo Climber, Garmin 1050): sim, the routing graph and elevations of stock's map files traced 4 km ahead, Garmin's definition as a course's, a banner and a beep, the CLIMB fields, BIKE > CLIMBS AHEAD; not seen on the unit
- [ ] Climb filter: all or medium and above (stock)
- [x] Gradient-coloured climb profile in sections (stock, all): sim, the PROFILE field
- [x] Pacing on a climb from FTP and W' balance (ours, Garmin Power Guide): sim, the TOP field on the course page, a constant-speed plan from CP, W' and the weights; not yet on a real climb
- [ ] Summit segments / climb challenges with others (Wahoo, Garmin 1050)
- [ ] Completed climbs listed with their times (Karoo: a climb counts once 250 m of it is done)
- [ ] Climb detection on or off per profile, descents too (Garmin 32.20, Karoo: off / routes only / everything)

## Alerts and safety

- [x] Beeps at power on and off (stock): unit
- [ ] Key click (stock Key Tone)
- [ ] Sound on or off for alerts (stock)
- [~] Eat, drink, rest reminders by trip time, clock, distance, kcal, kJ, heart rate, once or repeating, six of them (stock, Garmin, Wahoo): sim, eat and drink by ride time; not by distance, kJ or clock yet
- [~] Heart rate, power, cadence, speed high / low alerts (stock Alert, Garmin, Wahoo): sim, heart rate high; not power, cadence, speed or low limits yet
- [ ] Custom alerts with text (Wahoo, Karoo)
- [~] Notices: GPS fix, sensor paired, battery low (stock, all): sim, on the overview
- [x] Turn and off-course beeps (stock, all): sim
- [ ] Bike bell on the speaker or buzzer (Garmin 1050, Wahoo Roam 3)
- [ ] Radar alerts (stock, all)
- [ ] Incident detection with the accelerometer (Garmin, Karoo, Wahoo)
- [ ] Group ride incident detection (Garmin 1050)
- [ ] Road hazards, crowd-sourced (Garmin 1050)
- [ ] Bike alarm, find my Edge (Garmin)
- [ ] Assistance request to contacts (Garmin)
- [ ] Sunset alert (Garmin)

## Phone and connectivity

- [x] Pair with a phone (BLE bonding) (stock, all): sim; Just Works, one bond in `\Rustfw\ble.bin`
- [x] Call, text and e-mail notifications: iOS through ANCS with no app (stock app, all): unit (a call, answered); sim (messages, decline); a call shows over every screen, queued alerts say all
- [ ] Android notifications (stock app, all): need a companion app
- [x] Media remote: play, pause, next, previous, volume (patch P7, Garmin, Wahoo, Karoo): sim; AMS or HID
- [x] Now playing: title, artist, elapsed (ours, AMS): sim
- [ ] Media remote from Di2 buttons (patch P12)
- [x] Camera shutter on the phone (ours, BLE HID): sim
- [ ] Live tracking, 24 h link (stock app, all): needs a phone app
- [ ] Group ride: members on the map, messages, voice input (stock app with the microphone, Garmin, Karoo)
- [~] Ride upload to Strava and Intervals.icu (stock app, all): `tools/sync.py` on the computer, from the drive or the dev link (docs/sync.md); not yet run against the real services; the iOS app next
- [~] Planned workouts from Intervals.icu onto the unit (Garmin, Wahoo, Karoo): `tools/sync.py` writes them to `\Workout\calendar`
- [ ] Wi-Fi sync without the phone (Garmin, Wahoo, Karoo, Coros)
- [x] Time zone from the phone (ours, BLE Current Time Service): sim; the clock set too
- [ ] Settings edited from the phone (stock app, all)
- [ ] Contactless payments (Garmin Pay, 1050)
- [ ] Apps and extensions (Garmin Connect IQ, Karoo extensions)

## Device: battery, backlight, touch, buttons, storage, USB, updates

Battery and power

- [x] Battery percentage from the voltage (stock): unit
- [~] Switch off at low battery (stock): in the code and sim; the 3.3 V switch-off not reached on the unit
- [x] Battery history over four hours (ours, `dev.py bat`): unit
- [ ] Charging only between 0 and 40 C (stock): charger on whenever USB is in
- [ ] Battery hours left (Garmin, Coros)
- [ ] Battery saver mode (Garmin, Wahoo, Coros)
- [ ] Stop mode between seconds (ours; stock sleeps the same way): the largest battery gain left
- [ ] 30 hours of riding as stock claims (stock): to measure
- [x] Power off by holding Back (stock: long press on power), the radios and GPS off too (stock): unit

Backlight and panel

- [~] Backlight levels and time-out (stock, all): unit; the 60 / 30 / 10 % levels and the time-out still to see by eye
- [~] Automatic backlight from the light sensor, stock's curve (stock Auto Brightness, all): unit (rising with the light); outdoors by eye to see
- [ ] Backlight always on (stock "Stays on")
- [ ] Panel partial and idle modes (ours)

Touch and buttons

- [x] Four keys: Page, Lap, Start, Back with holds (stock): unit
- [x] Touch: swipe between pages, hold for the menu, tap a menu row (stock): sim; the chip answers on the unit
- [ ] Swipe up from the bottom to go home (stock)
- [ ] Tap a field to open Quick Status (stock)
- [ ] Long press a field to change it (stock, Garmin, Karoo)
- [ ] Pinch to zoom the map (Wahoo Roam 3, Garmin)
- [ ] Digital dial (Coros): no such hardware
- [x] Key repeat when a number is held (ours): unit
- [ ] Page key changes page before the click (patch P14): no click yet
- [x] Pages kept and fields refreshed at once (patch P17): sim, redraw only what changed

Storage, USB and updates

- [x] eMMC read: layout, FAT32 volume, files (stock): unit (sector reads); FAT reader in sim
- [~] eMMC and FAT32 writes, safe through a power cut (stock): sim (power cut after every sector write)
- [~] USB mass storage: the BRYTON drive on a computer (stock, all): sim; not yet seen on a computer
- [x] USB dev link: screenshots, keys, peek and poke, run from RAM (ours): unit
- [ ] Update this firmware from a file on the volume (stock IMG.bin + update.ini)
- [ ] Update over BLE from the phone (stock app, all)
- [x] Way back to stock: the bootloader's recovery mode (stock bootloader): unit
- [x] Shares the volume with stock, so the two can be swapped (ours): unit, read only so far
- [ ] GPS assistance and map updates from the computer (stock Update Tool)
- [ ] Memory: free space, delete old rides (stock File Saving, Memory)

## Settings

- [x] Wheel size in mm (stock, all): unit
- [x] Backlight level and time-out (stock): unit
- [x] MAX HR, FTP, weight, W' (stock, ours): unit (MAX HR, FTP); sim (weight, W')
- [x] Overview fields and pages 1 to 3 (stock): sim
- [x] Menus in groups, one screen each; a field picked by group then field (Garmin, Wahoo: categories): sim
- [x] The menu tree of the UI review (4.3): WORKOUTS, COURSES, HISTORY, PAGES, SENSORS, SETTINGS (ME, ALERTS, DISPLAY, BIKE, SYSTEM); any setting three levels down at most, units after the values: sim
- [x] A ride menu while riding (Garmin's controls, Karoo's drawer; UI review step 8): PAUSE / RESUME, END RIDE (asked first), WORKOUT (NEXT STEP, STOP), COURSE (STOP FOLLOWING), BRIGHTNESS, SENSORS, EDIT THIS PAGE, ALL SETTINGS; only the rows that apply: sim
- [x] GPS status: fix, satellites, position, altitude, UTC (stock About, Garmin): unit
- [~] Settings kept through power off (stock, all): RTC backup registers, not a file
- [ ] Time zone, daylight saving, date and time formats (stock, all)
- [ ] Units: metric or imperial, temperature (stock, all)
- [ ] Language (stock, all)
- [x] First start setup: stock's settings and pages taken by themselves (ours): sim
- [x] Import stock's settings: FTP, max HR, weight, W', wheel, paired ANT+ sensors, auto backlight, reminders, data pages (ours): sim (SETTINGS > SYSTEM > IMPORT FROM STOCK)
- [ ] Auto features per profile: smart pause, climb page switch, climb approach, map view (stock)
- [ ] Data reset: trips, odometer, factory settings (stock)
- [ ] About: firmware version, UUID, position (stock): over USB (`dev.py info`) only
- [ ] Altitude calibration at saved places (stock)
- [ ] Sensor names and gear teeth for Di2 (stock)
- [ ] Remote button actions for Di2 hood buttons (stock four, patch P12 ten)

## History and analysis

- [~] Ride list on the unit with summary and details (stock Results, all): sim, HISTORY: stock's rides newest first, each with its card (track, profile, ten numbers); no laps or graphs per ride
- [x] Delete rides on the unit (stock): sim, HISTORY's DELETE moves a ride into `\Rustfw\Deleted`
- [~] Totals: week, month, year, odometer (stock Overview, Garmin): unit, week, year and total km and week hours on the overview (read by themselves 30 s after start); no month
- [ ] Personal records and best efforts (Garmin, Karoo)
- [~] Power curve over 90 days, season, all time (stock app Rider's Insight, Garmin): sim, 90-day bests (1 s to 60 min) for CP and W'; no screen, no season
- [ ] Training load chart on the unit (stock app PMC, Garmin)
- [ ] Each ride matched to the planned workout (ours, roadmap 4b)
- [x] Rides read from stock's \Activities too (ours): sim (HISTORY > FITNESS > READ RIDES), checked on two backups
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
  a paid developer plan replaces free access from June 2026 (one source, inferred until checked). A personal
  `sync.py` step that fetches the owner's starred segments is likely allowed; KOM split times are not exposed, so
  a KOM ghost would run at a constant pace.
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
