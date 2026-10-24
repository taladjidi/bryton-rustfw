# Features

## Screens

- **Status strip** (every screen): BLE (green, red if the radio chip did not answer), GPS (blinking grey while
  searching, then 1 to 4 signal bars), the time, the ride's state (a red dot while it records, orange bars while paused), a USB symbol,
  the battery (green while charging, red at 15 % and under).
- **Overview** (the main screen): the time, big (no seconds: it changes once a minute), the date, four fields chosen
  in the menu (OVERVIEW; by default this week's distance and time, the total distance, the temperature: the totals come from the
  rides on the card, read by themselves 30 s after start-up), and the latest notices (a GPS fix, a
  sensor paired, the battery low), or Ferris the Rust crab when there are none.
- **Ride pages:** up to three, each in one of eleven layouts (one to six fields: even grids, and uneven ones such as
  a big box over two or over a 2 x 2 grid, two over a big one, three rows), chosen in the menu: speed, heart rate, cadence,
  power, time, map, timer, distance, average and top speed, average heart rate and power, lap time and distance,
  ascent, a zone graph; heart rate variability from a strap's beats: DFA a1 (the aerobic threshold marker, about
  0.75 at the threshold, from the last 2 minutes once 64 clean beats are in) and RMSSD (ms, the last minute); and
  from the power meter and the ride: 3, 10 and 30 s power, NP, IF, TSS, kJ, kcal, W/kg, W' balance (kJ, green,
  yellow, orange, red as it drains), max power, max heart rate, average cadence, left / right pedal balance, VAM and
  grade. **Default pages** (as Garmin, Wahoo and Hammerhead ship theirs: data, climbing, map): 1, speed, moving
  time, distance, average speed, heart rate, power; 2, the elevation profile with ascent, grade, VAM and cadence;
  3, the map, whole. While a course is followed, page 2 (left as it was) shows the course: its profile large, the
  next climb, the distance to go, distance, moving time and average speed.
- **Map:** the track since power-on, north up, fitted to the whole track, an arrow for position and heading, a
  scale bar. Before a fix, a grey dot at the last known position. Under it, a basemap:
  - **IGN Plan v2** (France: roads, paths, buildings, names, contour lines, hill shading) where the owner copied
    packs to `\Rustfw\Maps` (made on the computer by `tools/ign_pack.py`, docs/install.md, "IGN maps"): zooms 11
    to 16, drawn at 1x to 2x (never shrunk, so no thin line or letter is lost), "(c) IGN" in the map's corner as
    the licence (Etalab 2.0) asks;
  - else roads and water from stock's tile cache (`\Map\Data\Temp`), also under IGN at the edge of the packed
    area.
  The map is read and drawn once per view change (at most 6 IGN tiles, about 150 KB read); the arrow and the track
  restore it from SDRAM.
- Everything is in the project's own dot-matrix font. Only what changes is redrawn, in the smallest box.

- **Ride card:** when a ride ends (Start held while paused), its card: the date and start time, the map of the track,
  the elevation profile, time, distance, average speed, ascent, NP, average power, average and max heart rate, TSS,
  kJ. Any key goes on to the overview. Each past ride has the same card (HISTORY, below).

- **Reminders and alerts** (menu ALERTS, each off by default): eat and drink every so many minutes of ride time
  (5 min steps), heart rate over a limit (after 10 s over it; again only once it has come 5 bpm under it, at most
  once a minute). Each is a banner and three beeps, played without stopping the loop; nothing while paused.

## Keys

| Key | Overview and ride pages | Menus |
|---|---|---|
| Page | from the overview, the ride pages; then round them (the overview is apart, as on stock) | down |
| hold Page | the menu | |
| Lap | a lap (while riding) | up |
| Start | start, pause, resume the ride (from the overview: and show page 1); a banner says which | select, or change the value |
| hold Start | end a paused ride | |
| Back | on a ride page, the overview; on the overview, backlight on / off | back |
| hold Back 3 s | power off (the radio and GPS too) | power off |
| hold Lap | the phone's music: play / pause (PHONE below) | |

The power button starts it again. After the backlight's time-out, any key brings it back.

- **Graphs:** a graph field shows power, heart rate, W' balance, DFA alpha1 or the elevation profile: a tap on it
  goes to the next one (kept with the page), a hold switches every graph between the last minutes and the whole ride.
  Each has its scale on the left and its time span under it; power and heart rate are coloured by zone, W' balance as
  it drains, DFA alpha1 against the aerobic threshold (green above 0.75, orange to 0.5, red under).

## Touch

| Gesture | Overview and ride pages | Menus | A number being edited |
|---|---|---|---|
| swipe left | next page (as Page) | | |
| swipe right | previous ride page (round) | back (as Back) | keep it |
| swipe up / down | | the next / previous screen of rows | one step up / down |
| hold (600 ms, still) | the menu (as holding Page) | | |
| tap | the gear right of the overview's date: the menu; a graph: its next plot | selects the row touched and acts on it (as Start) | keep it |

A swipe is a drag of at least 37 px across or 48 px up or down, given at the release; a finger that moves under
10 px is a tap (at the release) or a hold (while still down). A touch brings the backlight back, as a key does.
Gestures reach the app as the same `Input` events as the keys (src/ui/menu.rs).

## Ride

- Moving time (auto-pause under 2 km/h), distance from the wheel sensor or else GPS, ascent from GPS altitude.
- Zones: heart rate (5, from MAX HR) and power (7, Coggan, from FTP), in stock's palette. The graph shows the last
  8 minutes of power (or heart rate) as a filled area, like stock's: one pixel column per 2 s, smoothed (a moving
  average, about 7 s), each column in its zone's colour, scrolling every 10 s.
- Power (src/power.rs): NP is Coggan's (30 s average, 4th power mean, 4th root), IF = NP / FTP, TSS from NP and
  FTP, kcal = kJ / (4.184 x 24 % efficiency), about 1 per kJ. W' balance is the differential model (Froncioni and
  Clarke, Skiba 2015, as Golden Cheetah and Intervals.icu), with the FTP as CP; it starts full at each ride and
  recovers while stopped. VAM over the last minute of moving time, grade over the last 30 s (GPS altitude).
- **Saved as a FIT file** (`src/storage/recorder.rs`; simulator only so far, docs/status.md): the ride goes to
  `\Rustfw\Rides\YYMMDDhhmmss.fit` as it is ridden (a record a second: time, position, altitude, speed, distance, heart
  rate, cadence, power, temperature; timer events at pause and resume; a lap message per Lap), from RAM to the card
  at a checkpoint at least every minute, at each pause and when switched off. A reset or a power cut loses at most
  the last minute: at the next start the ride comes back paused ("RIDE RESUMED"). At the end it is read back whole,
  then moved to
  `\Activities\YYMMDDhhmmss.fit` (stock's name and place: stock, its app, a computer and the history see it;
  "RIDE SAVED"). A card that fails shows "NOT SAVING" and the strip's dot grey, and is tried again; the ride goes
  on in RAM. A file that does not read back whole is left in `\Rustfw\Rides` ("SAVE FAILED"), never in the way of
  the next ride.
- **A full copy of each ride** (simulator only so far): `\Rustfw\Activities\YYMMDDhhmmss.fit` has the same records,
  each also carrying DFA a1 as the `Alpha1` developer field (the name Intervals.icu reads) and `RMSSD`, from the
  strap's beats. It is a second file because Bryton's server refuses developer fields, and the `\Activities` file
  stays exactly as before. It is recorded beside the clean one with its own checkpoints and resume, and it never
  holds the clean one up: if it fails, the ride is saved all the same and the banner says "NO FULL COPY".
- **Sync on the computer** (`tools/sync.py`, docs/sync.md): new rides go to Strava (the clean copy) and to
  Intervals.icu (the full copy), never twice. The next days' planned workouts come from Intervals.icu into
  `\Workout\calendar`. It works from the mounted drive, or reads rides over the dev link.

## Menu

One screen of groups, nothing more than a screen of rows away:
- **SENSORS:** pair, forget; then the light, temperature and pressure readings.
- **PAGES:** the overview's four fields, pages 1 to 3 (field count, each field). A field row opens the field groups
  (RIDE, POWER, HEART, CLIMB, GRAPHS, WORKOUT, COURSE, TOTALS, PHONE), the current field's group selected and the field
  beside it; a group lists its fields, the current one selected; Select puts it in the box and goes back to the
  page. Any of the 56 fields is a few presses away, not up to 55.
- **RIDER:** MAX HR (1 bpm steps) and FTP (5 W steps: Start, then Lap up and Page down, held to repeat, Start to
  keep), weight (1 kg), W' (0.5 kJ), WKT POWER (the workout gauge's window).
- **DEVICE:** backlight level (or AUTO) and time-out, wheel size, GPS (fix, satellites, position, altitude, UTC), BLE
  devices nearby, USB DRIVE (below).
- **ALERTS, WORKOUTS, COURSES, HISTORY, STOCK DATA.**
- **PHONE:** LINK (OFF, PAIRING, WAITING, LINKED, SECURE; ERR and a command's code if the radio refused the set-up),
  PAIR, REMOTE (the media remote), NOTIFY on / off, MEDIA KEY on / off, FORGET (BONDED while a phone is).

## History

HISTORY lists the rides in `\Activities` (stock's, read only), newest first: the day ("TUE 20 OCT") and the
distance in km, a screen of rows at a time. Opening it reads the rides as READ RIDES does (below), and the rows
appear as the rides are read; the newest 128 are kept. Start on a ride opens its card, the same as at the end of a
ride: the date and start time, the track (fitted to the map), the elevation profile, time, distance, average speed,
ascent, NP (from the ride's records), average power, average and max heart rate, TSS (at the FTP set now) and kJ.
The ride's file is read once to draw it, 96 KB a main loop pass ("READING RIDE", about 0.1 s for a 300 KB ride;
a long ride takes a few passes, the loop never stalls); the card opens when it is read. Back returns to the list.

## Workouts

The workouts Bryton's app syncs to the card (`\Workout\calendar\YYYYMMDD\*.fit`, often from intervals.icu or
TrainingPeaks), read only:
- **WORKOUTS** lists today's and the next days' (eight at most; only the earliest eight day folders are looked
  into, at most 16 files read), a row each: the day and the planned time. Select
  shows the workout's steps, its name as the title: what ends each step (a time, a distance, or LAP) and its target
  (power in W from % FTP, heart rate in bpm from % max HR, cadence), coloured by zone; repeats are written out.
- **START** (or Select on a step: from that step) starts the workout, and the ride if none runs, on page 1. The
  step's clock follows the ride's timer (a pause or an auto-pause stops both); a 500 ms beep at 2 and at 1 s
  left, then a 1 s one and a banner with the new step and its target ("2/6 200-220W"). Lap goes to the next
  step; a step with no end (LAP) waits for it. One workout at a time; it ends with its last step or with the ride.
- **Fields** to place on a page: TARGET (the step's range: green in it, orange under, red over, from the 3 s power,
  the heart rate or the cadence), STEP LEFT (time, km, or LAP), NEXT (the next step's range, END on the last), and
  WORKOUT (during one, the step of all, "3/14"; otherwise today's planned time, for the overview).
- **WKT GRAPH**, a graph field: the workout opened over its whole length, each step as high as its target (power
  from 0 to 150 % of FTP, heart rate from half the max to it, a quarter for an open target) in its zone colour; a
  white line where the rider is, the part done dark. A distance step is drawn at 30 km/h, an open step as 5 min.
  Only the columns the line leaves and reaches are redrawn (about 300 px/s in a full-page box, sim).
- **WKT POWER**, a gauge field: the reading the step targets big (power over the window set by the menu's WKT
  POWER row: 3 s, 10 s, 30 s or each second, "POWER 3S" in its title; heart rate for a heart-rate step, cadence for
  a cadence one), a bar from 30 % under the target band to 30 % over it with the band as a green box and a marker
  at the reading, and the band in numbers under it ("200-220"). The number and the marker are coloured by how far
  the reading is: green in the band, yellow within 5 %, blue further under, red further over (the zone palette's
  easy and hard colours). With an open step or no workout, the power alone. Only the number's changed characters
  and the marker's old and new columns are redrawn (about 6 500 px/s for the page with a top half box, sim).
- Any FIT workout file is read (Garmin's profile: time, distance and open steps, repeats, % FTP or watts, % max HR
  or bpm, zones), and Bryton's own encoding of power targets.

## Courses

The routes Bryton's app syncs to the card (`\PlanTrip\*.fit`, in Bryton's own layout) and FIT courses or GPX files
copied over USB into `\Courses`, read only:
- **COURSES** (main menu) lists them at once, then their lengths come a file at a time ("..." until then, 96 KB a
  main loop pass while the list shows); Select reads one ("READING COURSE") and follows it (green in the list; the
  first ride page opens), Select again stops.
- **On a lap course** (or any that passes a place more than once), a rider found again after losing the course
  (a gap in the fixes, a detour) is placed on the lap being ridden: the pass nearest along the course to where
  they were.
- **The map** draws the course in magenta under the track, and fits the course and the track.
- **Where the rider is:** the nearest point of the course, looked for around the last one each second; on an
  out-and-back or a loop the leg the rider is on wins. More than 50 m off for 5 s: "OFF COURSE" and the alert's
  beeps (once the course was joined); "BACK ON COURSE" within 25 m. A course over 330 km, kept a point every 160 m,
  has a wider threshold (half the spacing plus 25 m), since its chords cut the bends.
- **Climbs** as Garmin's ClimbPro defines them (500 m at 3 % at least), found when the course is read; at a
  climb's start a banner ("CLIMB 2.0 KM 6%") and a beep.
- **Fields:** TO GO KM, OFF CRS M (red while off course), CLIMB (on a climb what is left and the grade of the next
  200 m, "1.6K 6%"; before it the next climb's length and grade; coloured by grade), CLIMB M (the climb's gain left,
  or the next climb's), UP LEFT M (the course's ascent left), and PROFILE: the course's elevation from the eighth of
  it the rider is in to the end, the rider's place, the ridden part dark, the climbs green, yellow, orange or red by
  grade.

## Stock data

What Bryton's firmware left on the card, read only (nothing is ever written), from the STOCK DATA menu:
- **READ RIDES** reads every ride in `\Activities` (stock's `YYMMDDhhmmss.fit`; its ride in progress `tmp.fit` and
  files that are not whole rides are left out), a bounded amount per pass of the main loop, more while no ride
  runs. The menu then shows:
  - **CP and W'** from the best powers of the last 90 days (1 s to 60 min), fitted with the 2-parameter model
    on the 2 to 20 minute bests, and **CP FIT**: the points used and the model's largest error. A stop or a
    power dropout ends an effort (a best never spans a cafe stop);
  - **distance and riding time** of this week (from Monday), this year and all rides.
- **IMPORT** applies stock's settings where this firmware has them: FTP, max heart rate, weight and W'
  (UserProfile.ini), the wheel (a speed sensor's, Sensor.ini), the paired ANT+ sensors (Sensor.ini), the
  automatic backlight (DeviceData.ini), and from the bike in use (DeviceData.ini `ActBike`): the eat and drink
  reminders and the heart rate alert that are on, and the data pages (its first three enabled pages, each with
  the fields of the grid shown that have an equivalent here, 53 of stock's field ids, at most 6 a page).
- **The first start does the same by itself:** with no settings of this firmware's own yet (a unit coming from
  stock: no backup register holds its settings, a paired sensor or a page), it starts with stock's settings and
  pages, says IMPORTED n, and never imports again (the menu row does, on demand); never with a ride to resume.
  Stock's files are only read.

## Sensors and radios

- **ANT+:** heart rate (and the beat-to-beat intervals, for DFA a1 and RMSSD), speed, cadence, speed and cadence,
  power, on stock's channels. A paired sensor is kept
  through power off, so a friend's is never taken. A silent sensor searches 30 s, then rests a minute. Needs the
  ANT+ key from your own stock firmware ([install.md](install.md)).
- **GPS** (u-blox M10): speed when no wheel sensor is paired; at power-up it gets the time and the last position,
  for a faster fix.
- **BLE:** a 10 s scan lists the devices nearby with their signal strength; phones and laptops show their kind
  ("iPhone/Mac", "Find My", "Windows PC"), since they advertise no name.
- **Built in:** ambient light (automatic backlight, opt-in), barometer (temperature, pressure), battery.
- **Beeps** at power-on and power-off, for a workout's steps, alerts and courses: one tone player, so two sounds
  due together play one after the other.

## Phone (BLE, no app)

The unit pairs with a phone as a Bluetooth accessory, with nothing to install on the phone:
- **Pairing:** PHONE > PAIR advertises for a minute ("S510"; the phone sees a HID accessory, appearance generic HID);
  pair from the phone's Bluetooth settings (Just Works: no code). One phone is kept (its key and the notifications
  it enabled), in `\Rustfw\ble.bin` on the card, read 3 s after the start; FORGET erases it. A build that may not
  write the card keeps it until switched off. Not paired, the unit does not advertise at all; paired and not
  connected, it advertises once a second so the phone comes back by itself.
- **Media keys:** on an iPhone, through Apple's media service (AMS): play / pause, next, previous, volume; otherwise
  as a BLE keyboard's media keys (HID consumer control, also mute). Hold Lap on any page: play / pause. PHONE >
  REMOTE is a player, also opened by a swipe down on the overview or a ride page (a swipe up closes it): the title on two lines, the artist, the album, a progress bar with the elapsed and remaining
  time, and buttons: previous, play / pause, next, then skip back, volume down, volume up, skip forward (AMS's skip:
  the app sets how far, often 15 or 30 s in a podcast app; there is no "go to a position"). Keys: Start play / pause,
  Page next, Lap previous, held Page / Lap volume; swipes: left / right track; a skip the app does not take says NOT IN THIS APP; a hold takes a
  photo (the camera's shutter is volume up, always sent as HID).
- **Now playing** (AMS): title, artist, album, duration, playing or paused and the elapsed time; the title in the
  banner when the track changes; the MUSIC field (group PHONE) shows the elapsed time.
- **Notifications** (iPhone, ANCS): new ones in the banner and the overview's notices, "sender: text" (as much as a
  line holds; accents folded to plain letters). Those already on the phone when it connects are not shown. NOTIFY
  OFF stops them.
- **Calls:** over the bottom of any screen (the ride page above keeps going): a red decline and a green answer
  button with handsets, the caller in the orange banner; Start or the green button answers, Back or the red one
  declines; a call answered or ended on the phone takes them away. No call time and no hang-up: iOS removes the
  call's notification at the answer (seen on the unit), so nothing tells the unit when the call ends.
- **Time:** the clock is set from the phone (Current Time Service) at each connection, and the time zone kept for
  the GPS and the ride files (it replaces the one learnt at a fix); not while a ride is open, so its times never jump.
- **Battery** level for the phone (Battery Service), sent when it changes.
- ANT+ sensors keep working alongside: the Nordic chip runs both radios.

## USB

With a cable in, a dev link starts: screenshots, key presses, register and memory reads, the battery log, eMMC
sector reads, and running a new build from RAM (`dev.py`, see AGENTS.md). Unplugging resets the unit.

**USB drive:** the computer can mount the eMMC's FAT volume (BRYTON: rides, routes, workouts, map tiles) without
going back to stock. The unit is a card reader with the card out until DEVICE > USB DRIVE (or `dev.py drive`) lends
it; refused with a banner while a ride runs (END THE RIDE FIRST), waits to be saved (RIDE NOT SAVED YET) or without
the dev link up (CONNECT USB). Lent, nothing else touches the card: no ride can start (USB DRIVE ON), the scans, the
basemap and the dev link's card commands wait. Ejecting on the computer gives it back; so does USB DRIVE again on
the unit, once the computer has let it go (else EJECT ON COMPUTER). After the computer wrote, the unit restarts so
that nothing it read before is trusted. Only the FAT volume is offered (sectors 0 .. B-1), never stock's reserved
area. A build without EMMC_WRITE=1 offers it write protected.
