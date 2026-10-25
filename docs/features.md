# Features

## Screens

- **Status strip** (every screen): BLE (green, red if the radio chip did not answer), GPS (blinking grey while
  searching, then 1 to 4 signal bars), the time, the ride's state (a red dot while it records, orange bars while paused), a USB symbol,
  the battery (green while charging, red at 15 % and under).
- **Overview** (the main screen): the time, big (no seconds: it changes once a minute), the date, four fields chosen
  in the menu (OVERVIEW; by default this week's distance and time, the total distance, the temperature: the totals come from the
  rides on the card, shown at once from the scan's cache, `\Rustfw\scan.bin`, and checked 30 s after start-up, when
  only the rides new or changed since are read), and the latest notices (a GPS fix, a
  sensor paired, the battery low), or Ferris the Rust crab when there are none.
- **Ride pages:** one to six, each made of rows (up to five, each a share of the height, one or two boxes side by
  side, eight fields at most), made on the page itself in the page editor (below) or from a layout of the menu (one
  to eight fields: even grids, and uneven ones such as a big box over two or over a 2 x 2 grid, two over a big one,
  three rows, four rows of two): speed, heart rate, cadence,
  power, time, map, timer, distance, average and top speed, average heart rate and power, lap time and distance,
  ascent, a zone graph; heart rate variability from a strap's beats: DFA a1 (the aerobic threshold marker, about
  0.75 at the threshold, from the last 2 minutes once 64 clean beats are in) and RMSSD (ms, the last minute); and
  from the power meter and the ride: 3, 10 and 30 s power, NP, IF, TSS, kJ, kcal, W/kg, W' balance (kJ, green,
  yellow, orange, red as it drains), max power, max heart rate, average cadence, left / right pedal balance, VAM and
  grade. **Default pages** (as Garmin, Wahoo and Hammerhead ship theirs: data, climbing, map): 1, speed, moving
  time, distance, average speed, heart rate, power; 2, the elevation profile with ascent, grade, VAM and cadence;
  3, the map, whole. While a course is followed, a course page joins the round after the ride pages (and opens
  when the course is loaded): its profile large, the next turn, the distance to go, distance and moving time.
- **Value sizes:** one scale per box size, the largest at which five characters ("88.88", "12:34") fit: every value in
  a box of that size has the same digit height (3x in a half-width box, 7x or 8x across the screen; a value's ink
  stays 3 px from its box's rules, so two values side by side never read as one); only a longer
  value ("200-220", "1.6K 6%") takes the largest smaller scale it fits. Times: "MM:SS" under an hour (the step left
  "M:SS"), then "H:MM" at the box's size with the seconds small and grey after it ("1:23 45"); the seconds go first
  when even that does not fit. No 1x text on the ride pages: graph scales, spans and figures are 2x, drawn on graphs
  across the screen at least 120 px tall (a smaller graph shows its title and bars), and the workout gauge's target
  line only in a box 100 px tall or more.
- **Map:** the track since power-on, north up, fitted to the whole track, an arrow for position and heading, a
  scale bar. Before a fix, a grey dot at the last known position. Under it, a basemap:
  - **IGN Plan v2** (France: roads, paths, buildings, names, contour lines, hill shading) where the owner copied
    packs to `\Rustfw\Maps` (made on the computer by `tools/ign_pack.py`, docs/install.md, "IGN maps"): zooms 11
    to 16, the finest zoom whose tiles are drawn at least 200 pixels wide (0.78x to 1.56x: zoom 16 at 2 m a pixel
    up to 49 N, finer than zoom 15 enlarged; a one-pixel line can lose a pixel here and there when shrunk), "(c)
    IGN" in the map's corner as
    the licence (Etalab 2.0) asks;
  - else roads and water from stock's tile cache (`\Map\Data\Temp`), also under IGN at the edge of the packed
    area.
  The map is read and drawn once per view change (at most 6 IGN tiles, about 150 KB read); the arrow and the track
  restore it from SDRAM.
  - **Zoom:** on a full-page map, "+" and "-" top right (40 x 40 pixels, for a gloved finger). Each tap is one
    level (steps of about 1.4) from the one shown, finer or coarser than the fit alike (indoors the fit is already
    at 2 m a pixel: "-" zooms out by hand), from 1.5 m a pixel (IGN's zoom 16 about 1:1 in France) out. At a
    level set by hand the map follows the rider: centred a little ahead of them, and centred again only when the
    arrow leaves the middle of the map (a new basemap each time, about every 26 s at 1.5 m a pixel and 29 km/h,
    35 s at 2 m). A third button, a crosshair under "-", shown whenever the zoom is set by hand, goes back to
    fitting the whole track (and course). Kept until the unit is switched off. A hold on the map still opens the
    menu; a map of half a page or less has no buttons. The fitted map keeps the arrow clear of the buttons.
  - **Pan:** at a zoom set by hand, drag the map: it moves with the finger, redrawn once when the finger lifts
    (one new basemap, no redraw while dragging), at most two map widths past the track and the course. The map
    then stays put instead of following; "+" and "-" zoom around the centre panned to; the crosshair goes back to
    the fit. While the zoom is set by hand, a drag on the map pans instead of changing page or opening the media
    remote; the keys still change pages. Fitted, swipes keep their meanings. A new ride, or a course followed or
    dropped, fits the map again.
- Everything is in the project's own dot-matrix font. Only what changes is redrawn, in the smallest box.

- **Ride card:** when a ride ends (Start held while paused), its card: the date and start time, the map of the track
  (over the basemap, as the history's cards),
  the elevation profile, time, distance, average speed, ascent, NP, average power, average and max heart rate, TSS,
  kJ. Any key goes on to the overview. Each past ride has the same card (HISTORY, below).

- **Reminders and alerts** (menu ALERTS, each off by default): eat and drink every so many minutes of ride time
  (5 min steps), heart rate over a limit (after 10 s over it; again only once it has come 5 bpm under it, at most
  once a minute). Each is a banner ("EAT REMINDER", "DRINK REMINDER", "HEART RATE HIGH" in red) and three beeps,
  played without stopping the loop; nothing while paused.
- **Banners:** at the top of the screen, over the strip and the page's first 20 px (the values below stay in view),
  for 4 s or until a key or a touch; 2x text on one or two lines, black on orange, or white on red for an alarm
  (heart rate high, off course, the battery low, a ride not recording or not saving, the second hold of Back that
  switches off). No time in front: the strip shows it. Banners have a rank: an alarm, then a turn now, then what
  the ride says (a workout step, a reminder, a climb), then a turn ahead, then the rest. A lower one never covers
  a higher one; the other waits (one at most, the higher kept) and shows next, so an alarm and a workout step
  said in the same second both show. A newer one of the same rank replaces the one up, which does not come back,
  and a turn cue is good for its moment only: one waiting, or one pushed off, goes when another cue is said. An
  incoming call is above them all: a banner said during it waits and shows when the call ends.

## Keys

| Key | Overview and ride pages | Menus |
|---|---|---|
| Page | from the overview, the ride pages; then round them (the overview is apart, as on stock) | down (stops at the last row) |
| hold Page | the menu; during a ride (recording or paused), the ride menu | |
| Lap | a lap (while riding) | up (stops at the first row) |
| Start | start, pause, resume the ride (from the overview: and show page 1); a banner says which | select, or change the value |
| hold Start | end a paused ride (or END RIDE in the ride menu) | |
| Back | on a ride page, the overview; on the overview, backlight on / off | back |
| hold Back 3 s | power off (the radio and GPS too) | power off |
| hold Lap | the phone's music: play / pause (PHONE below) | |

The power button starts it again. After the backlight's time-out, any key brings it back.

- **Menus:** rows 44 px tall (about 8 mm, a fingertip), six a screen, a longer menu turned a screen at a time;
  a "<" left of the title goes back. The selected row is black on orange (10:1 contrast; white on that orange was
  2:1); a row's value is white like its label, green when it says a state that is on (a sensor heard, the phone
  linked).

- **The page editor** (app/editor.rs; UI review 2, 5.2): a hold on a ride page's box, PAGES > a page > EDIT ON
  PAGE, or RIDE > EDIT THIS PAGE while paused; never while a ride records (recording again behind it closes it,
  its work kept). The page shows at its real size with live values, the selected box in a 3 px orange frame, and
  the hint in the strip's place ("PAGE 2  START: MENU"):
  ```
  +------------------------+
  |  PAGE 2  START: MENU   |   the hint, orange, where the strip was
  +#########+--------------+
  #KM/H     #HR            |   the selected box framed
  #  28.4   #  142         |
  ##########+--------------+
  |TIMER      KM           |
  |  1:02      21.84       |
  +------------------------+
  ```
  Page / Lap (or a tap) move the frame; Start (or a tap on the framed box) opens its actions: FIELD (the groups,
  then the fields), SPLIT with a side-by-side icon (a second box in its row), SPLIT with a stacked icon (a row
  under it, half its height), MERGE (with the other box of its row, else the row under it, whose fields go),
  TALLER, SHORTER, UNDO (the last action, again to redo), ADD PAGE (after this one), DELETE PAGE (asked first; one
  page stays). An action that would leave a field too small to read says why in the banner ("TOO SMALL",
  "2 A ROW AT MOST", "5 ROWS AT MOST", "8 BOXES AT MOST") and changes nothing: a value wants five characters at 3x
  (93 x 53 px), a graph, the course profile or the workout gauge 120 x 118, the map half the page. Back leaves:
  at once when nothing changed, else "KEEP CHANGES?" (Start keeps, Back drops).
- **Where the pages are kept:** `\Rustfw\pages.bin`, two copies of one sector (a sequence number and a CRC each;
  a write goes over the older, so a cut leaves the other), written when the pages changed and no ride is open; then
  the first three pages in backup registers 20 to 25 as earlier builds kept them (a custom page as the layout of
  its field count, six fields at most). At a start the registers give the pages at once, the file 2 s later (the
  card is not touched at boot) and wins, unless another build changed the registers since: then its pages there
  win, the file's others kept. A build that may not write the card keeps its pages (three) in the registers only.
- **Graphs:** a graph field shows power, heart rate, W' balance, DFA alpha1 or the elevation profile: a tap on it
  goes to the next one until the unit is switched off (the page as kept changes only in PAGES), a hold switches every
  graph between the last minutes and the whole ride; neither while a ride records (below).
  Each has its scale on the left and its time span under it; power and heart rate are coloured by zone, W' balance as
  it drains, DFA alpha1 against the aerobic threshold (green above 0.75, orange to 0.5, red under).

## Touch

| Gesture | Overview and ride pages | Menus | A number being edited |
|---|---|---|---|
| swipe left | next page (as Page) | | |
| swipe right | previous ride page (round) | back (as Back) | keep it |
| swipe up / down | | the next / previous screen of rows | one step up / down |
| hold (600 ms, still) | on a ride page's box: the page editor, that box selected (on a graph: the whole ride or the last minutes); on the overview: the menu; not while a ride records | | |
| tap | the gear right of the overview's date: the menu; a graph: its next plot; neither while a ride records | selects the row touched; a tap on the row already chosen acts on it (as Start); the "<" left of the title: back | keep it |

A swipe is a drag of at least 37 px across or 48 px up or down, given at the release; a finger that moves under
10 px is a tap (at the release) or a hold (while still down). A touch brings the backlight back, as a key does.
Gestures reach the app as the same `Input` events as the keys (src/ui/menu.rs). **Touch safety:** while a ride
records (not paused), a tap or a hold on a page does nothing but the map's buttons: no menu, no graph changed (a wet
glove, rain); swipes still turn pages and the keys do everything. TOUCH in SETTINGS > DISPLAY (ON / OFF) switches gestures off
altogether, for rain: the keys only, the call's buttons included.

## Ride

- Moving time (auto-pause under 2 km/h), distance from the wheel sensor or else GPS, ascent from GPS altitude.
- Zones: heart rate (5, from MAX HR) and power (7, Coggan, from FTP), in stock's palette. The plain power fields
  (POWER, 3S, 10S, 30S W) and HR fill their box with their zone's colour, the number in black or white over it, so
  the zone shows at a glance (POWER's box takes its 3 s average's zone, its number stays live); NP, max and the other
  derived values keep a coloured number on black. The graph shows the last
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

Hold Page (or tap the gear) for the menu: what to ride first, then what was ridden, then the set-up (UI review
4.3). A row that opens a menu or a screen ends in ">"; a value sits on the right with its unit ("FTP  200 W"). Any
setting is three levels down at most (MENU > SETTINGS > ME > FTP), the diagnostics under SYSTEM:

```
MENU
 +- WORKOUTS >     the planned workouts, from today -> a workout's steps: START, or start from a step
 +- COURSES >      the course files (green: followed); Start follows or stops
 +- HISTORY >      FITNESS > (READ RIDES, CP, W', CP FIT, WEEK / YEAR / TOTAL KM and H), then the rides -> a card
 +- PAGES >        OVERVIEW (four fields), PAGE 1..6 (EDIT ON PAGE, LAYOUT, FIELD n -> group -> field), ADD PAGE
 +- SENSORS >      HEART RATE, SPEED, CADENCE, SPEED+CAD, POWER, SHIFTING; DI2 KEYS >; PHONE >
 +- SETTINGS >
     +- ME >       MAX HR, FTP, WEIGHT, W'
     +- ALERTS >   EAT, DRINK (every so many minutes), HR ABOVE
     +- DISPLAY >  BRIGHTNESS (or AUTO), OFF AFTER, TOUCH, GAUGE POWER (the workout gauge's window)
     +- BIKE >     WHEEL
     +- SYSTEM >   USB DRIVE, IMPORT FROM STOCK, GPS >, BLE NEARBY >, BUILT IN > (lux, temperature, pressure), ABOUT >
```

- **SENSORS:** Start on a row: an off sensor searches; a searching one stops; a paired one heard asks "SURE?" and
  a second Start forgets it; a paired one silent searches for it again at once (not after its rest), and a second
  Start forgets it. DI2 KEYS: what each Di2 hood button press does. PHONE: LINK (OFF, PAIRING, WAITING, LINKED,
  SECURE; ERR and a command's code if the radio refused the set-up), PAIR, REMOTE (the media remote), NOTIFY on /
  off, MEDIA KEY on / off, FORGET (BONDED while a phone is).
- **Asked first:** forgetting a sensor or the phone and IMPORT FROM STOCK show "SURE?" in the row and ask in the
  banner; a second Start (or a tap on the row) does it, any other key or touch drops it.
- **PAGES:** the overview's four fields; the ride pages, each with EDIT ON PAGE (the page editor), LAYOUT (the next
  layout its fields fit) and its fields; ADD PAGE (up to six: a 2 x 2 page, the editor open on it). A field row
  opens the field groups
  (RIDE, POWER, HEART, CLIMB, GRAPHS, WORKOUT, COURSE, TOTALS, PHONE), the current field's group selected and the field
  beside it; a group lists its fields, the current one selected; Select puts it in the box and goes back to the
  page. Any of the 56 fields is a few presses away, not up to 55.
- **The ride menu:** Page held while a ride records or is paused opens RIDE instead, the rows that apply only:
  ```
  RIDE
   +- PAUSE / RESUME   as Start; PAUSE stays (END RIDE right under it), RESUME goes back to the page
   +- END RIDE         asked first ("SURE?", Start again), then the ride's card (paused first if it was running)
   +- WORKOUT >        while a step runs: NEXT STEP (no lap), STOP WORKOUT (the ride goes on)
   +- COURSE >         while one is followed: STOP FOLLOWING (the ride goes on)
   +- BRIGHTNESS       changed in place
   +- SENSORS >        the same menu as off the ride
   +- EDIT THIS PAGE > paused only: the page editor on the page (the overview: its fields; the course page has none)
   +- ALL SETTINGS >   the full menu above
  ```
  An action row (RESUME, NEXT STEP, STOP...) goes back to the page it was opened over (the first ride page when
  that page is gone, as the course page once the course stops). A row that goes while its menu shows (WORKOUT when
  the workout ends) moves the selection to the last row left.
- **Numbers** (MAX HR 1 bpm, FTP 5 W, WEIGHT 1 kg, W' 0.5 kJ, the alerts 5): Start, then Lap up and Page down, held
  to repeat, Start to keep. A choice (BRIGHTNESS, OFF AFTER, TOUCH, GAUGE POWER, WHEEL) goes to its next value.
- **SYSTEM:** USB DRIVE (below), IMPORT FROM STOCK (stock's settings, below), GPS (fix, satellites, position,
  altitude, UTC), BLE devices nearby, the built-in sensors' readings, ABOUT (Ferris, the version, the commit the
  build is made from and its date; `dev.py info` gives the same commit).
- **Where the rows were** before this tree (2026-10-25), old path -> new path: SENSORS > LUX / TEMP C / HPA -> SYSTEM >
  BUILT IN; RIDER > MAX HR, FTP W, WEIGHT KG, W' KJ -> SETTINGS > ME > MAX HR, FTP, WEIGHT, W'; RIDER > WKT POWER ->
  DISPLAY > GAUGE POWER; DEVICE > LIGHT % / LIGHT OFF / TOUCH -> DISPLAY > BRIGHTNESS / OFF AFTER / TOUCH; DEVICE >
  WHEEL MM -> BIKE > WHEEL; DEVICE > GPS, NEARBY BLE, USB DRIVE, ABOUT -> SYSTEM > GPS, BLE NEARBY, USB DRIVE, ABOUT;
  ALERTS > EAT MIN, DRINK MIN, HR OVER -> SETTINGS > ALERTS > EAT, DRINK, HR ABOVE; STOCK DATA > IMPORT -> SYSTEM >
  IMPORT FROM STOCK; STOCK DATA's other rows -> HISTORY > FITNESS; PHONE -> SENSORS > PHONE. WORKOUTS, COURSES,
  HISTORY, PAGES, SENSORS (the ANT rows, DI2 KEYS) stay where they were. The settings are kept as before (same
  backup registers, same formats).

## History

HISTORY lists the rides in `\Activities` (stock's and this firmware's), newest first: the day ("TUE 20 OCT") and the
distance in km, a screen of rows at a time. Opening it reads the rides as READ RIDES does (below), and the rows
appear as the rides are read; the newest 128 are kept. Start on a ride opens its card, the same as at the end of a
ride: the date and start time, the track (fitted to the map), the elevation profile, time, distance, average speed,
ascent, NP (from the ride's records), average power, average and max heart rate, TSS (at the FTP set now) and kJ.
The ride's file is read once to draw it, 96 KB a main loop pass ("READING RIDE", about 0.1 s for a 300 KB ride;
a long ride takes a few passes, the loop never stalls); the card opens when it is read. Back returns to the list.

**Delete** (storage/trash.rs): on a ride's card, Start asks "DELETE RIDE? START AGAIN: YES" (any other key drops
it), Start again moves the ride out of sight, never erases it: `\Activities\<name>.fit` into `\Rustfw\Deleted\`,
and this firmware's full copy of the same ride (`\Rustfw\Activities\<name>.fit`), if any, into
`\Rustfw\Deleted\Full\`, under the same name (a name already there gets `-1`, `-2`...). A computer sees them
there; moving a file back puts the ride back in both firmwares' histories. The list, the totals and the power curve
lose it at once (the scan's cache drops its entry, no ride read again), and the list stays on the same row. Refused
("NOT DURING A RIDE") while a ride runs or waits to be resumed or saved. Stock's summaries of the ride
(`\System\History\Summary\<name>.sum / .csum`) are left as they are: stock's own files; stock's history is
built from the FIT files it finds, so the ride goes from it too (inferred). A power cut during the move loses
nothing: `\Rustfw\delete.rec` says what is being moved, and the next start finishes it (tested by cutting the
power after every sector written).

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
- **WKT POWER**, a gauge field: the reading the step targets big (power over the window set by DISPLAY > GAUGE
  POWER: 3 s, 10 s, 30 s or each second, "POWER 3S" in its title; heart rate for a heart-rate step, cadence for
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
- **Turn by turn** (course/cues.rs), as Garmin's, Wahoo's and Hammerhead's: the cues the file states (a FIT
  course's course points: left, right, slight and sharp turns, U-turn, straight on, summit, water, food, danger,
  first aid; GPX waypoints or track points whose `type`, `sym` or `name` is a turn word, as BRouter and RideWithGPS
  write them, BRouter's roundabouts with their exit), else the turns of the course's shape (a GPX track, Bryton's
  routes): 45 degrees or more over 25 m either side, corners drawn with several points merged; slight under 60
  degrees, sharp from 110, a U-turn from 159 (BRouter's classes). A roundabout is one cue with its exit ("EXIT 2",
  a ring arrow; the exit counted in quarter turns, either direction of traffic), told from a junction's corners by
  its ring (a turn spread over several points, none as sharp as a corner): a jog is a turn at each corner, a
  turn with kinks before and after its own way, a swerve back to the same heading nothing. A road that winds says
  nothing: bends wider than about 20 m of radius and hairpins (a U-turn whose way out does not come back along the way
  in) give no cue, a junction's corner does (without a road map, a junction taken in one wide sweep says nothing
  either). Up to 256 a course, thinned evenly along it past that (turns kept before points of interest). The next one is
  said in the banner with its arrow 200 m before (or 20 s at the speed ridden, if farther) with a beep ("LEFT
  200 M"), and at the turn (32 m or 4 s) with two ("LEFT NOW"); nothing off course. Not read: the turns of
  Bryton's `voiceTrip.fit` (their values are not known), cue names (street names).
- **Fields:** TO GO KM, NEXT TURN (the next cue's arrow and distance, "350M", "1.2K"; END past the last one;
  "--" off course), OFF CRS M
  (red while off course), CLIMB (on a climb what is left and the grade of the next
  200 m, "1.6K 6%"; before it the next climb's length and grade; coloured by grade), CLIMB M (the climb's gain left,
  or the next climb's), UP LEFT M (the course's ascent left), and PROFILE: the course's elevation from the eighth of
  it the rider is in to the end, the rider's place, the ridden part dark, the climbs green, yellow, orange or red by
  grade.

## Stock data

What Bryton's firmware left on the card, read only (nothing of stock's is ever written; HISTORY's DELETE moves a ride's file, above), from HISTORY > FITNESS (and SETTINGS > SYSTEM > IMPORT FROM
STOCK):
- **READ RIDES** reads every ride in `\Activities` (stock's `YYMMDDhhmmss.fit` and this firmware's saved rides; stock's
  ride in progress `tmp.fit` and files that are not whole rides are left out), a bounded amount per pass of the main
  loop (15 ms of work at most, 4 ms during a ride), so keys and touch keep answering. **Each ride is decoded once
  ever:** what the overview, the power curve and the history need of it (64 bytes: distance, time, ascent, power,
  heart rate, its best powers) goes into the scan's cache, `\Rustfw\scan.bin`, written between rides; a start shows
  the totals from it at once and reads only the rides that are new or changed (name, size, first cluster), and drops
  those deleted. A new day counts the week, the year and the 90 days again from it. A cache that does not check out
  (a cut while it was written) is ignored whole and rebuilt. The menu then shows:
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
- **Electronic shifting** (not yet seen on the unit): **DI2** (Shimano's own ANT channel, as stock and the patched
  stock firmware's P12 use it; needs Shimano's network key, copied from your stock image with the ANT+ one by
  `tools/ant_key.py`) and **SHIFTING** (the public ANT+ Shifting profile: SRAM AXS, Campagnolo), paired from the
  sensors menu; one shifting system at a time (pairing one forgets the other). Fields (group SHIFTING): **GEAR**
  front-rear ("2-11"; a 1x system the rear alone, "11"),
  **SHIFTS** in this ride, **SHIFT BAT** (Di2: percent; ANT+: NEW, GOOD, OK, LOW, CRIT; red when low). Each shift
  while riding goes in the ride file as a FIT gear change event (front or rear, the gears after it), as stock
  writes them. **DI2 KEYS** (sensors menu): what each hood button press does (left / right, single / long): next
  or previous page, lap, start, play / pause, next or previous track, or nothing. Page, lap and start act on the
  ride pages and the overview only (a lap on the overview too while riding), never as keys in a menu or on a call;
  play and the tracks work everywhere. The Di2 repeats its last press, so its first button page after a start is
  not a press. IMPORT takes the Di2 and its
  button actions from stock's Sensor.ini (P12's numbers included). Not done: the gear ratio (needs the chainring
  and cassette teeth), the ANT+ Shifting remotes' buttons (no public layout).
- **GPS** (u-blox M10): speed when no wheel sensor is paired; at power-up it gets the time and the last position,
  for a faster fix.
- **BLE:** a 10 s scan lists the devices nearby with their signal strength; phones and laptops show their kind
  ("iPhone/Mac", "Find My", "Windows PC"), since they advertise no name.
- **Built in:** ambient light (automatic backlight, opt-in), barometer (temperature, pressure), battery.
- **Beeps** at power-on and power-off, for a workout's steps, alerts and courses: one tone player, so two sounds
  due together play one after the other.

## Phone (BLE, no app)

The unit pairs with a phone as a Bluetooth accessory, with nothing to install on the phone:
- **Pairing:** SENSORS > PHONE > PAIR advertises for a minute ("S510"; the phone sees a HID accessory, appearance generic HID);
  pair from the phone's Bluetooth settings (Just Works: no code). One phone is kept (its key and the notifications
  it enabled), in `\Rustfw\ble.bin` on the card, read 3 s after the start; FORGET erases it. A build that may not
  write the card keeps it until switched off. Not paired, the unit does not advertise at all; paired and not
  connected, it advertises once a second so the phone comes back by itself.
- **Media keys:** on an iPhone, through Apple's media service (AMS): play / pause, next, previous, volume; otherwise
  as a BLE keyboard's media keys (HID consumer control, also mute). Hold Lap on any page: play / pause. SENSORS > PHONE >
  REMOTE is a player, also opened by a swipe down on the overview or a ride page (a swipe up closes it): the title on two lines, the artist, the album, a progress bar with the elapsed and remaining
  time, and buttons: previous, play / pause, next, then skip back, volume down, volume up, skip forward (AMS's skip:
  the app sets how far, often 15 or 30 s in a podcast app; there is no "go to a position"). Keys: Start play / pause,
  Page next, Lap previous, held Page / Lap volume; swipes: left / right track; a skip the app does not take says NOT IN THIS APP; a hold takes a
  photo (the camera's shutter is volume up, always sent as HID).
- **Now playing** (AMS): title, artist, album, duration, playing or paused and the elapsed time; the title in the
  banner when the track changes; the MUSIC field (group PHONE) shows the elapsed time.
- **Notifications** (iPhone, ANCS): new ones in the banner and the overview's notices, "sender: text" (as much as
  two banner lines hold; accents folded to plain letters). Those already on the phone when it connects are not shown. NOTIFY
  OFF stops them.
- **Calls:** "INCOMING CALL" and the caller in the banner at the top, a red decline and a green answer button
  at the bottom, each with a handset and its key's name (BACK, START); the page between keeps going. Start or the
  green button answers, Back or the red one declines; a call answered or ended on the phone takes them away. No call time and no hang-up: iOS removes the
  call's notification at the answer (seen on the unit), so nothing tells the unit when the call ends.
- **Time:** the clock is set from the phone (Current Time Service) at each connection, and the time zone kept for
  the GPS and the ride files (it replaces the one learnt at a fix); not while a ride is open, so its times never jump.
- **Battery** level for the phone (Battery Service), sent when it changes.
- ANT+ sensors keep working alongside: the Nordic chip runs both radios.

## USB

With a cable in, a dev link starts: screenshots, key presses, register and memory reads, the battery log, eMMC
sector reads, and running a new build from RAM (`dev.py`, see AGENTS.md). Unplugging resets the unit.

**USB drive:** the computer can mount the eMMC's FAT volume (BRYTON: rides, routes, workouts, map tiles) without
going back to stock. The unit is a card reader with the card out until SETTINGS > SYSTEM > USB DRIVE (or `dev.py drive`) lends
it; refused with a banner while a ride runs (END THE RIDE FIRST), waits to be saved (RIDE NOT SAVED YET) or without
the dev link up (CONNECT USB). Lent, nothing else touches the card: no ride can start (USB DRIVE ON), the scans, the
basemap and the dev link's card commands wait. Ejecting on the computer gives it back; so does USB DRIVE again on
the unit, once the computer has let it go (else EJECT ON COMPUTER). After the computer wrote, the unit restarts so
that nothing it read before is trusted. Only the FAT volume is offered (sectors 0 .. B-1), never stock's reserved
area. A build without EMMC_WRITE=1 offers it write protected.
