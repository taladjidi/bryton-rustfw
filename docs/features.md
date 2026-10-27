# Features

## Screens

- **Status strip** (every screen): BLE (green, red if the radio chip did not answer), GPS (blinking grey while
  searching, then 1 to 4 signal bars), the time, the ride's state (a red dot while it records, orange bars while paused), a USB symbol,
  the battery (green while charging, red at 15 % and under).
- **Overview** (the main screen): the date, big (3x, centred left of the cog; the time is in the strip), under it the TODAY card
  (below, as tall as the rest allows: about 110 px), four fields in two rows of 60 px chosen in the menu (OVERVIEW; by default this week's distance and time, the total distance, the temperature: the totals come from the
  rides on the card, shown at once from the scan's cache, `\Rustfw\scan.bin`, and checked 30 s after start-up, when
  only the rides new or changed since are read), and the newest notice on one row at the foot (a GPS fix, a
  sensor paired, the battery low; a tap opens them), nothing when there are none (Ferris the Rust crab lives in ABOUT). A cog ends the
  date's row (a ring with eight square teeth and a round hole, anti-aliased, level with the date; a tap anywhere in its
  44 x 44 px box opens the menu). A swipe down on the overview, before a ride, opens the **quick panel** (QUICK:
  REMOTE (the media remote, first), CALIBRATE POWER when a meter is heard, TRAINER CONTROL when a trainer is paired, PROFILE, BRIGHTNESS, USB DRIVE,
  NOTIFICATIONS; a swipe up or Back closes it); on a ride page a swipe down still opens the media remote.
  **TODAY card** (the owner's idea, 2026-10-26: the big clock went, the strip has the time): "TODAY" and the planned
  length (H:MM) on one line, the workout's name under it (cut to whole characters), and its profile at the bottom: a
  bar per column (56 columns of 4 px, the width of the card) in the colour of the step's zone and as high as its
  intensity (the WORKOUT GRAPH's look and scale: power up to 150 % of FTP, heart rate from half the maximum), so a
  sweet spot, a VO2 session and a long easy ride tell apart at a glance. "REST DAY" in grey when nothing is planned
  today. A tap anywhere on the card opens today's workout, its steps with START selected (a tap on START, or Start, plays
  it: two inputs from the overview; Start held on the overview does the same, while no ride is open), and on a rest day
  WORKOUTS (not while a ride records, as every touch). Drawn once: when the
  overview opens and when the calendar or the day changes; nothing per second (the overview now costs 4 px/s, the
  strip's, where the clock's digits made it 53). The calendar is read 8 s after the start (the card is not touched at
  boot; the card shows only its title until then, and a tap before reads it at once), and again at midnight and when
  FTP or the maximum heart rate changes (the profiles are recoloured).
- **Readings at 1 Hz:** the power, speed, cadence and heart rate values on the pages are the readings of the second's
  tick, not of each 4 Hz broadcast (the digits were unreadable and each change a redraw: 8,301 px/s against 11,339 on
  six values with a power meter and a strap, windows 10 a second against 20); the recorder and the averages still take
  each second's reading.
- **Ride pages:** one to six, each made of rows (up to five, each a share of the height, one or two boxes side by
  side, eight fields at most), made on the page itself in the page editor (below) or from a layout of the menu (one
  to eight fields: even grids, and uneven ones such as a big box over two or over a 2 x 2 grid, two over a big one,
  three rows, four rows of two): speed, heart rate, cadence,
  power, time, map, timer, distance, average and top speed, average heart rate and power, lap time and distance,
  ascent, a zone graph; heart rate variability from a strap's beats: DFA a1 (the aerobic threshold marker, about
  0.75 at the threshold, from the last 2 minutes once 64 clean beats are in) and RMSSD (ms, the last minute); and
  from the power meter and the ride: 3, 10 and 30 s power, NP, IF, TSS, kJ, kcal, W/kg, W' balance (kJ, green,
  the zone colours as it drains), max power, max heart rate, average cadence, left / right pedal balance, VAM and
  grade. **Default pages** (as Garmin, Wahoo and Hammerhead ship theirs: data, climbing, map): 1, speed, moving
  time, distance, average speed, heart rate, power; 2, the elevation profile with ascent, grade, VAM and cadence;
  3, the map, whole. While a course is followed, a course page joins the round after the ride pages (and opens
  when the course is loaded): its profile large, the next turn, the distance to go, the climb pacer (TOP, below) and moving time.
- **Value sizes:** one scale per box size, the largest at which five characters ("88.88", "12:34") fit: every value in
  a box of that size has the same digit height (3x in a half-width box, 7x or 8x across the screen; a value's ink
  stays 3 px from its box's rules, so two values side by side never read as one); only a longer
  value takes the largest smaller scale it fits. A range and a climb keep the box's scale: the target is its middle
  big and its half-width small after it ("210" and "+-10" for 200-220), the climb its length big and its grade small
  ("1.6K" and "6%"). Times: "MM:SS" under an hour (the step left
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
  - **Zoom:** on a full-page map, "+" and "-" top right (48 x 48 pixels, 8 apart, for a gloved finger). Each tap is one
    level (steps of about 1.4) from the one shown, finer or coarser than the fit alike (indoors the fit is already
    at 2 m a pixel: "-" zooms out by hand), from 1.5 m a pixel (IGN's zoom 16 about 1:1 in France) out. At a
    level set by hand the map follows the rider: centred a little ahead of them, and centred again only when the
    arrow leaves the middle of the map (a new basemap each time, about every 26 s at 1.5 m a pixel and 29 km/h,
    35 s at 2 m). A third button, a crosshair under "-", shown whenever the zoom is set by hand, goes back to
    fitting the whole track (and course). Kept until the unit is switched off. A hold on the map opens the page
    editor with the map's box selected (not while a ride records, as on any box); a map of half a page or less has no buttons. The fitted map keeps the arrow clear of the buttons.
  - **Pan:** at a zoom set by hand, drag the map: it moves with the finger, redrawn once when the finger lifts
    (one new basemap, no redraw while dragging), at most two map widths past the track and the course. The map
    then stays put instead of following; "+" and "-" zoom around the centre panned to; the crosshair goes back to
    the fit. While the zoom is set by hand, a drag on the map pans instead of changing page or opening the media
    remote; the keys still change pages. Fitted, swipes keep their meanings. A new ride, or a course followed or
    dropped, fits the map again.
- Everything is in the project's own dot-matrix font, or in Barlow Semi Condensed Bold with FONT = SMOOTH (Touch,
  below). Only what changes is redrawn, in the smallest box.

- **End of a ride:** Start held while paused (or END RIDE in the ride menu) opens the end-ride screen, which is the
  confirmation: the ride's time and distance, and three large buttons (64 px, gloves): RESUME, SAVE (selected when it opens: the ride
  is over, so Start held then Start saves it), DISCARD. Page and Lap move, Start acts, a tap acts at once, Back leaves
  the ride paused on its page ("PAUSED"; with a banner up, it clears it first). RESUME restarts the timer and goes back to
  the page; SAVE ends and saves it (below) and shows its card. DISCARD asks once more ("DISCARD
  RIDE?", KEEP selected, the two buttons apart so a second tap cannot hit DISCARD again; Back keeps), then ends the
  ride and moves both copies to `\Rustfw\Discarded` (the full one to `\Rustfw\Discarded\Full`): never erased, a
  computer can move it back, but in no history, totals, curve or sync ("RIDE DISCARDED"). While the screen is up
  the ride is paused and recorded as ever: a reset brings it back paused.
- **Ride card:** after SAVE, its card: the date and start time, the map of the track (over the basemap, as the
  history's cards), the elevation profile, time, distance, average speed, ascent, NP, average power, average and max
  heart rate, TSS, kJ. It stays until Back or Start (no other key, swipe or tap dismisses it), then the overview.
  Each past ride has the same card (HISTORY, below).

- **Reminders and alerts** (menu ALERTS, each off by default): eat and drink every so many minutes of ride time
  (5 min steps), heart rate over a limit (after 10 s over it; again only once it has come 5 bpm under it, at most
  once a minute). Each is a banner ("EAT REMINDER", "DRINK REMINDER", "HEART RATE HIGH" in red) and three beeps,
  played without stopping the loop; nothing while paused. Alerts due in the same second, or a catch-up after a busy
  pass, are queued (one bit each) and said one after the other, the heart rate first, none lost to the one up.
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
| Lap | a lap (while riding), on the overview too | up (stops at the first row) |
| Start | start, pause, resume the ride (from the overview: and show page 1); a banner says which | select, or change the value |
| hold Start | a paused ride: the end-ride screen (RESUME, SAVE, DISCARD; or END RIDE in the ride menu); the overview with no ride: today's workout; on a past ride's card: DELETE, asked | |
| Back | on a ride page, the overview; on the overview, nothing; with a banner up, it only clears the banner | back |
| hold Back 3 s | power off (the radio and GPS too) | power off |
| hold Lap | the phone's music: play / pause (PHONE below); on the trainer screen while riding: the target one big step down (Page is up) | |
| hold Back and Lap together 1 s | the touch lock on or off, on every screen: all touch is ignored and a padlock shows in the strip (until unlocked or switched off); neither key acts on its own | the same |

The power button starts it again. After the backlight's time-out, any key brings it back. A fresh unit (no saved
settings) starts with BRIGHTNESS AUTO and OFF AFTER 30 S; saved settings, or stock's imported, keep theirs.

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
  keeps what was done ("CHANGES KEPT"; Back never drops). DROP CHANGES, the last row of the box's actions and there
  only once something changed, asks ("SURE?") and puts the page back as it was.
- **Where the pages are kept:** `\Rustfw\pages.bin`, two copies of one sector (a sequence number and a CRC each;
  a write goes over the older, so a cut leaves the other), written when the pages changed and no ride is open; then
  the first three pages in backup registers 20 to 25 as earlier builds kept them (a custom page as the layout of
  its field count, six fields at most). At a start the registers give the pages at once, the file 2 s later (the
  card is not touched at boot) and wins, unless another build changed the registers since: then its pages there
  win, the file's others kept. A build that may not write the card keeps its pages (three) in the registers only.
- **Pages per profile:** an INDOOR ride (a trainer heard at Start, or INDOOR set by hand or in BIKE) has its own set
  of up to six pages beside the road set: the trainer (3 s power large, the trainer's target, heart rate, cadence,
  timer, work), the power numbers (average, NP, W' balance, IF, work, TSS) and the power and heart rate graphs, with
  their MORE variants; the overview is shared. The editor and PAGES work on the profile's set while a ride is open,
  else the one PAGES > ROAD PAGES or INDOOR PAGES opened. The indoor set is in `pages.bin` only (after the MORE part, a CRC of its own,
  so earlier builds ignore it and a file without it gives the defaults), never in the backup registers.
- **Graphs:** a graph field shows power, heart rate, W' balance, DFA alpha1 or the elevation profile: a tap on it
  goes to the next one until the unit is switched off (the page as kept changes only in PAGES), a hold switches every
  graph between the last minutes and the whole ride; neither while a ride records (below).
  Each has its scale on the left and its time span under it; power and heart rate are coloured by zone, W' balance as
  it drains, DFA alpha1 against the aerobic threshold (the zone 2 green above 0.75, the zone 3 olive to 0.5, the zone 4 brown under).

## Touch

| Gesture | Overview and ride pages | Menus | A number being edited |
|---|---|---|---|
| swipe left | next page (as Page) | | |
| swipe right | previous ride page (round) | back (as Back) | keep it |
| swipe up / down | down on the overview before a ride: the quick panel; down on a ride page: the media remote; up on a ride page: MORE | the next / previous screen of rows (up closes the quick panel) | one step up / down |
| hold (600 ms, still) | on a ride page's box: the page editor, that box selected (on a graph: the whole ride or the last minutes); on the overview: the menu; not while a ride records | | |
| tap | the cog ending the overview's date row (a 44 x 44 px touch box): the menu; a graph: its next plot; neither while a ride records | acts on the row touched (as Start on it; a destructive row asks first); while the ride runs and moves, it selects and a second tap on the row acts; the "<" left of the title: back | keep it |

A swipe is a drag of at least 37 px across or 48 px up or down, given at the release; a finger that moves under
10 px is a tap (at the release) or a hold (while still down). A touch brings the backlight back, as a key does.
Gestures reach the app as the same `Input` events as the keys (src/ui/menu.rs). **Touch safety:** while a ride
records (not paused), a tap or a hold on a page does nothing but the map's buttons: no menu, no graph changed (a wet
glove, rain); swipes still turn pages and the keys do everything. TOUCH in SETTINGS > DISPLAY (ON / OFF) switches gestures off
altogether, for rain: the keys only, the call's buttons included; the touch lock (Back and Lap held together 1 s) is the
quick, temporary switch for a pocket or a glove, and TOUCH stays the permanent one.

**Font.** FONT in SETTINGS > DISPLAY chooses the look of every text: PIXEL (the dot matrix of Ferris and the Game Boy
era, the default) or SMOOTH (Barlow Semi Condensed Bold, anti-aliased). The cells are the same size, so no page or menu
moves; the screen is redrawn at once and the choice is kept through a restart (register 16, bit 24). Scale 1 (small
text), the cue arrows and, from scale 5, characters a value never shows stay pixel. Barlow is under the SIL Open
Font Licence 1.1 (`tools/fonts/`, LICENSE); `tools/font.py` rasterizes it into the same cell as the pixel font.

## Ride

- Moving time (auto-pause under 2 km/h), distance from the wheel sensor or else GPS, ascent from the barometric altitude (a median of 5 readings, then an average of 1/6; 3 m hysteresis; a reading a
  second while riding), else from GPS altitude (5 m). A new calibration or source never steps the ride's altitude: it
  is let go at 0.5 m/s at most and that part is never counted as climbing, and a calibration being made at the start
  leaves the barometer on its old one. On a real 60 km ride this took the ascent from 341 m to 123 m.
- Zones: heart rate (5, from MAX HR) and power (7, Coggan, from FTP), in Intervals.icu's palette (teal, green, olive, brown,
  wine, purple, slate), each channel x 1.5 for the black panel; heart rate's five take the first five, so a colour
  means the same zone on both, and none is the menu's orange (selection, info banners) or the alarms' red. The
  gauge, W' balance, grade and DFA a1 colours follow by zone.
  The plain power fields (POWER, 3S, 10S, 30S W) and HR show their zone as an 8 px bar along the top of the box,
  the number white; each bar follows the number in its own box at once (live power, 3, 10, 30 s, heart rate each
  zoned on their own value), with a margin on the value (2 % of FTP, 1 % of max HR) so a value on an edge does not
  flip it, and the bar is all that is redrawn. NP, max and the other derived
  values are white too, with their state in an edge bar. The graph shows the last
  8 minutes of power (or heart rate) as a filled area, like stock's: one pixel column per 2 s, smoothed (a moving
  average, about 7 s), each column in its zone's colour, scrolling every 10 s.
- Power (src/training/power.rs): NP is Coggan's (30 s average, 4th power mean, 4th root), IF = NP / FTP, TSS from NP and
  FTP, kcal = kJ / (4.184 x 24 % efficiency), about 1 per kJ. W' balance is the differential model (Froncioni and
  Clarke, Skiba 2015, as Golden Cheetah and Intervals.icu), with the FTP as CP; it starts full at each ride and
  recovers while stopped. VAM over the last minute of moving time, grade over the last 30 s (barometric altitude, else GPS).
- **Saved as a FIT file** (`src/storage/recorder.rs`; seen on the unit 2026-10-24, docs/status.md): the ride goes to
  `\Rustfw\Rides\YYMMDDhhmmss.fit` as it is ridden (a record a second: time, position, altitude, speed, distance, heart
  rate, cadence, power, temperature; timer events at pause and resume; a lap message per Lap and per workout step), from RAM to the card
  at a checkpoint at least every minute, at each pause and when switched off. A reset or a power cut loses at most
  the last minute: at the next start the ride comes back paused ("RIDE RESUMED"). Its end card's NP, TSS and kJ
  then cover the part after the restart only (the recovery record keeps the ride's totals, not the power sums); the
  file holds every second's power, so the history's and a computer's numbers cover the whole ride. At the end it is
  read back whole, then moved to
  `\Activities\YYMMDDhhmmss.fit` (stock's name and place: stock, its app, a computer and the history see it;
  "RIDE SAVED"). A card that fails shows "NOT SAVING" and the strip's dot grey, and is tried again; the ride goes
  on in RAM. A file that does not read back whole is left in `\Rustfw\Rides` ("SAVE FAILED"), never in the way of
  the next ride. The save makes room for the new entry in the target folder before it removes the ride's old one
  (`Fat::make_room`: a zeroed cluster linked when the folder must grow), so a full card fails with the ride still
  named. A save the card keeps failing puts the ride back in `\Rustfw\Rides` ("SAVE FAILED: KEPT IN RUSTFW/RIDES");
  if even that fails, the ride stays open and is tried again (backing off up to 160 s) until the card takes it, the
  USB drive lent read only meanwhile, and a ride started meanwhile waits for it ("NOT RECORDING: CARD"). The way out
  on a card that keeps failing is **GIVE UP SAVE**, first in SETTINGS > SYSTEM only while a save is stuck: copy the
  card first (USB, read only), then two banners say the ride stays on the card unnamed and it asks twice (Start: yes,
  Back: no, at any step). It tries once more to name the ride in `\Rustfw\Rides` ("KEPT IN RUSTFW/RIDES"), else
  under a free name in `\Rustfw\Rescue` (`Rescue\Full` for the full copy; only if neither folder that may name the
  ride does, and with 8 MB free: "KEPT IN RUSTFW/RESCUE"), else marks it given up, its clusters left lost on the card
  for a computer's fsck to turn into `FILE0000.CHK` ("GIVEN UP: COPY THE CARD"). Then the ride held behind it starts
  recording and the drive is lent writable again. Refused while the card is lent ("USB DRIVE ON") and while a ride
  that is being recorded runs ("END THE RIDE FIRST"); a ride held behind the stuck save has nothing recorded, so it
  is allowed then, and that ride starts recording at once. Both copies
  share one name: the full copy saves after the clean one, under the name that one took. A ride discarded goes
  through the same steps into `\Rustfw\Discarded` instead, its records saying so from the end on, so a cut during it
  finishes there at the next start (never in `\Activities`).
- **A full copy of each ride** (seen on the unit without a strap; with one, docs/status.md): `\Rustfw\Activities\YYMMDDhhmmss.fit` has the same records,
  each also carrying DFA a1 as the `Alpha1` developer field (the name Intervals.icu reads), `RMSSD` and
  `ArtPct` (the share of beats the live DFA a1 dropped), from the strap's beats; and every beat itself, raw, in
  standard `hrv` messages as Garmin's units write them (about 20 KB an hour), for tools that compute DFA a1
  offline. It is a second file because Bryton's server refuses developer fields, and the `\Activities` file
  stays exactly as before. It is recorded beside the clean one with its own checkpoints and resume, and it never
  holds the clean one up: if it fails, the ride is saved all the same and the banner says "NO FULL COPY".
- **Sync on the computer** (`tools/sync.py`, docs/sync.md): new rides go to Strava (the clean copy) and to
  Intervals.icu (the full copy), never twice. The next days' planned workouts come from Intervals.icu into
  `\Workout\calendar`. It works from the mounted drive, or reads rides over the dev link.

## Menu

Hold Page (or tap the cog) for the menu: what to ride first, then what was ridden, then the set-up (UI review
4.3). A row that opens a menu or a screen ends in ">"; a value sits on the right with its unit ("FTP  200 W"). Any
setting is three levels down at most (MENU > SETTINGS > ME > FTP; a page's own rows are four: MENU > PAGES >
ROAD PAGES > PAGE 1 > LAYOUT), the diagnostics under SYSTEM:

```
MENU
 +- WORKOUTS >     the planned workouts, from today -> a workout's steps: START, or start from a step; last row RAMP TEST
 +- COURSES >      the course files (green: followed); Start follows or stops
 +- HISTORY >      FITNESS > (READ RIDES, CP, W', CP FIT, EST FTP, MODEL, APPLY FTP, APPLY W', HRVT1 AET, HRVT2,
 |                 RIDES, WEEK / YEAR / TOTAL KM and H), then the rides -> a card
 +- PAGES >        ROAD PAGES >, INDOOR PAGES > -> OVERVIEW (four fields), PAGE 1..6 (EDIT ON PAGE, LAYOUT,
 |                 FIELD n -> group -> field), ADD PAGE (a page's own rows are four levels down)
 +- SENSORS >      CALIBRATE POWER (a meter heard) and TRAINER CONTROL (a trainer paired) first; then HEART RATE,
                   SPEED, CADENCE, SPEED+CAD, POWER, SHIFTING, BT TRAINER; DI2 KEYS >; PHONE >
 +- SETTINGS >
     +- ME >       MAX HR, FTP, WEIGHT, W'
     +- ALERTS >   EAT, DRINK (every so many minutes), HR ABOVE
     +- DISPLAY >  BRIGHTNESS (or AUTO), OFF AFTER, TOUCH, FONT, GAUGE POWER (the workout gauge's window)
     +- BIKE >     WHEEL, WEIGHT (the bike's, 5.0 to 25.0 kg), ALTITUDE, FROM GPS (the barometer's calibration),
     |             PROFILE (AUTO / ROAD / INDOOR), CLIMBS AHEAD (ON / OFF), GPS POWER (AUTO / FULL / SAVER)
     +- SYSTEM >   USB DRIVE, IMPORT FROM STOCK, GPS >, BLE NEARBY >, BUILT IN > (lux, temperature, pressure), ABOUT >
```

- **SENSORS:** Start on a row: an off sensor searches; a searching one stops; a paired one heard asks "SURE?" and
  a second Start forgets it; a paired one silent searches for it again at once (not after its rest), and a second
  Start forgets it. DI2 KEYS: what each Di2 hood button press does. PHONE: LINK (OFF, PAIRING, WAITING, LINKED,
  SECURE; ERR and a command's code if the radio refused the set-up), PAIR, REMOTE (the media remote), NOTIFY on /
  off, MEDIA KEY on / off, FORGET (BONDED while a phone is).
- **Taps act:** one tap on a menu row acts on it, as Start on that row (a destructive row still asks first). While the
  ride runs and moves a tap only selects the row and a second one acts, so a bump or a wet glove does nothing.
- **Asked first:** forgetting a sensor or the phone and IMPORT FROM STOCK show "SURE?" in the row and ask in the
  banner ("START: YES BACK: NO"); a second Start (or a tap on the row) does it, Back says no (it only cancels, the
  menu stays), any other key or touch drops it.
- **PAGES:** ROAD PAGES and INDOOR PAGES, each opening its set's list: the overview's four fields; the ride pages, each with EDIT ON PAGE (the page editor), LAYOUT (the next
  layout its fields fit) and its fields; ADD PAGE (up to six: a 2 x 2 page, the editor open on it). A field row
  opens the field groups
  (RIDE, POWER, HEART, CLIMB, GRAPHS, WORKOUT, COURSE, TOTALS, PHONE), the current field's group selected and the field
  beside it; a group lists its fields, the current one selected; Select puts it in the box and goes back to the
  page. Any of the 56 fields is a few presses away, not up to 55.
- **The ride menu:** Page held while a ride records or is paused opens RIDE instead, the rows that apply only:
  ```
  RIDE
   +- PAUSE / RESUME   as Start; PAUSE stays (END RIDE right under it), RESUME goes back to the page
   +- END RIDE         the end-ride screen (paused first if it was running): RESUME, SAVE, DISCARD
   +- WORKOUT >        while a step runs: NEXT STEP (makes a lap, through `step_lap`, as the Lap key does), STOP WORKOUT (the ride goes on)
   +- COURSE >         while one is followed: STOP FOLLOWING (the ride goes on)
   +- TRAINER CONTROL  with a trainer paired: its screen (mode, target)
   +- INDOOR ON / OFF  the ride's profile by hand, for this ride
   +- BRIGHTNESS       changed in place
   +- SENSORS >        the same menu as off the ride
   +- EDIT THIS PAGE > paused only: the page editor on the page (the overview: its fields; the course page has none)
   +- ALL SETTINGS >   the full menu above
  ```
  An action row (RESUME, NEXT STEP, STOP...) goes back to the page it was opened over (the first ride page when
  that page is gone, as the course page once the course stops). A row that goes while its menu shows (WORKOUT when
  the workout ends) moves the selection to the last row left.
- **Numbers** (MAX HR 1 bpm, FTP 5 W, WEIGHT 1 kg, the bike's WEIGHT 0.1 kg, W' 0.5 kJ, the alerts 5): Start, then Lap up and Page down, held
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

## Interactions

The owner's heuristic (2026-10-26): "minimize button clicks / swipes on the screen". A review counted what each task
costs; `src/sim/interaction_tests.rs` (and the tests named here) assert the counts below, so a menu that gets deeper
again fails a test. A key press or a tap is one, a swipe is one, Page held is one.

| Task | Before | Now | Test |
|---|---|---|---|
| A menu row acts (PAGES from the overview, touch) | 3 (hold, tap selects, tap acts) | 2 | `one_tap_acts_on_a_row_unless_the_ride_moves` |
| Today's workout started, touch | 5 | 2 (the card, START) | `todays_workout_in_two_inputs` |
| Today's workout started, keys | 4 | 2 (Start held, Start) | `todays_workout_in_two_inputs` |
| Calibrate the power meter from the overview, touch | 6 | 2 (swipe down, a tap) | `the_quick_panel_from_the_overview` |
| The same by keys (meter heard) | 17 | 7 (SENSORS lists it first) | `sensors_actions_come_first` |
| TRAINER CONTROL from the overview, keys | 4 (a MAIN row) | 7 (SENSORS row 0); 2 touches by the quick panel | `the_trainer_screen_is_one_step_from_the_menu_and_the_fields` |
| The trainer's target down, riding, keys | none (a tap on "-") | 1 (Lap held) | `riding_the_long_keys_on_the_trainer_screen` |
| The trainer's mode, riding, keys | none (a tap on MODE) | 1 (Start held) | same |
| Save a ride by keys | 3 (Start held, Page, Start) | 2 (Start held, Start) | `end_ride_screen_by_keys` |
| Open the indoor pages' list | 4 (a toggle first) | 4, with no state to remember | `the_indoor_page_set_is_a_row` |

The rest of the rule: **a tap acts, but never by accident.** A destructive row still asks ("SURE?"); while the ride
runs and moves a tap only selects and a second one acts; **Back always means no** (it clears a banner, declines a
question, leaves the editor with its changes kept, leaves a ride paused), and what is destructive is an explicit row:
DISCARD (asked twice), DELETE on a past ride, DROP CHANGES in the page editor.

## History

HISTORY lists the rides in `\Activities` (stock's and this firmware's), newest first: the day ("TUE 20 OCT") and the
distance in km, a screen of rows at a time. Opening it reads the rides as READ RIDES does (below), and the rows
appear as the rides are read; the newest 128 are kept. Start on a ride opens its card, the same as at the end of a
ride: the date and start time, the track (fitted to the map), the elevation profile, time, distance, average speed,
ascent, NP (from the ride's records), average power, average and max heart rate, TSS (at the FTP set now) and kJ.
The ride's file is read once to draw it, 96 KB a main loop pass ("READING RIDE", about 0.1 s for a 300 KB ride;
a long ride takes a few passes, the loop never stalls); the card opens when it is read. Back returns to the list.

**Delete** (storage/trash.rs): on a ride's card, Start dismisses the card (as on the ride just ended), and DELETE (a red
label at the right of the title, a 88 x 44 px touch box; or Start held) asks "DELETE RIDE? START: YES BACK: NO" (any other
input drops it), Start (or a tap on DELETE again) moves the ride out of sight, never erases it: `\Activities\<name>.fit` into `\Rustfw\Deleted\`,
and this firmware's full copy of the same ride (`\Rustfw\Activities\<name>.fit`), if any, into
`\Rustfw\Deleted\Full\`, under the same name (a name already there gets `-1`, `-2`...). A computer sees them
there; moving a file back puts the ride back in both firmwares' histories. The list, the totals and the power curve
lose it at once (the scan's cache drops its entry, no ride read again), and the list stays on the same row. Refused
("NOT DURING A RIDE") while a ride runs or waits to be resumed or saved. Stock's summaries of the ride
(`\System\History\Summary\<name>.sum / .csum`) are left as they are: stock's own files; stock's history is
built from the FIT files it finds, so the ride goes from it too (inferred). A power cut during the move loses
nothing: `\Rustfw\delete.rec` says what is being moved, and the next start finishes it (tested by cutting the
power after every sector written).

## FTP ramp test

Indoors, with a trainer heard: **WORKOUTS > RAMP TEST** (last row, its FTP in white), or RAMP TEST beside MODE on the trainer screen
before a ride. It is a built-in workout played by the ordinary player in ERG (ramp.rs, app/ramp.rs), as
TrainerRoad's Ramp Test: 5 min at 50 % of the FTP set, then one-minute steps from 56 % in +6 % each (FTP 250 W:
140 W, then +15 W a minute; the last step open), then 5 min at 45 % to cool down. A stale FTP still gives a sensible
start. It ends when cadence is under 60 rpm for 5 s, power under half the target for 10 s (not in the ramp's first
10 s), Lap, STOP WORKOUT, or the ride not moving (paused by hand, or by itself after 3 s without power or cadence);
the cool-down starts at once. The result screen shows FTP (75 % of the best rolling minute since the ramp began,
1 W; a dropped power reading holds its last value for 3 s), the best minute, the ramp's length, and RAMP HRVT1 (where
DFA a1 crosses 0.75 on a line through points with a1 between 0.50 and 1.00, one per 5 s, on both sides of 0.75:
Rogers et al. 2021; the power marked "~" because it lags on a ramp, and the heart rate; `--` without a strap's RR
intervals or enough clean data; not kept; not a steady-state zone 2 power). APPLY (touch) sets the FTP (to the
setting's 5 W step), KEEP or Back keeps the one set; Start and Lap keep their ride meaning on the screen. A ramp under
a whole minute has no result. The ride is recorded and saved as any ride.

**Release.** The trainer is let go (free resistance: FE-C page 48 at 0 %, FTMS Set Target Resistance Level 0) whenever
what held it stops, so ERG's last target does not stay on it:
- **stop:** STOP WORKOUT, the workout's end, a course no longer followed (a +/- on the trainer screen under a workout
  or a course adjusts its target, it does not take the trainer over);
- **end:** the ride saved or discarded, the rider's own take-over (MODE, +/- with nothing driving) included;
- **pause:** ERG while the ride is paused or auto-paused (3 s without power or cadence indoors), so the rider never
  restarts at 0 rpm against the target; the trainer screen says PAUSED: FREE, and the target goes out again on
  resume. SIM and RESISTANCE stand (SIM brakes by the speed, like the road from a standstill);
- **switch-off:** one free resistance before the radio chip goes off (bounded: two tries over ANT+ 330 ms apart, then
  as long for it to go on air; over Bluetooth the answer to a write still out is waited for, 3 s at most);
- **boot:** one try at the first contact after a start (a unit switched on beside another controller, an app, does
  not keep forcing the trainer free).

A release after our own hold goes again every 4 s until it is known to have arrived (ANT+ TX_COMPLETED, the BLE write's
answer, the FTMS response), five times at most. The rider's own hold ends with the ride even when the trainer was
silent at its end. A chip that never answers the first acknowledged message turns the ANT+ control off
until the chip is started again; once one was answered, a later silence only has the message sent again. An FTMS
trainer that refused the control is asked again 10 s later, then less often (up to a minute apart), while it is driven.
With BIKE > PROFILE fixed to INDOOR the GNSS receiver stays off between rides too (AUTO keeps it on).
BIKE > GPS POWER: AUTO (the default) puts the receiver in u-blox cyclic tracking only while a ride runs fast (over
20 km/h, until under 15) and straight (no turn over 30 degrees in 5 s) with a good fix (10 minutes in, 8 satellites,
under 5 m) for a minute, and back in continuous tracking at the first fix that is not, so MTB and climbs keep the full
resolution; about 4.8 mA less while in it (estimated from u-blox's data sheet: [power.md](power.md)). FULL is
continuous always, SAVER cyclic always.

## Workouts

The workouts Bryton's app syncs to the card (`\Workout\calendar\YYYYMMDD\*.fit`, often from intervals.icu or
TrainingPeaks), read only:
- **WORKOUTS** lists today's and the next days' (eight at most; only the earliest eight day folders are looked
  into, at most 16 files read), a row each: the day over the planned time on the left ("FRI 23" over "1:08"), the
  workout's profile on the right (56 columns of 2 px, on black even when the row is selected, in the zones' colours,
  as the TODAY card's), so a workout is recognised without opening it; the rows stay 44 px (two lines of 2x text fit
  beside a 32 px thumbnail). The profiles are computed once, when the calendar is read (a byte a column: height and
  colour, 56 bytes an entry, 448 bytes for the eight; the name 16 more each), so the list and the card draw without
  the card. RAMP TEST keeps its plain row. Select
  shows the workout's steps, its name as the title: what ends each step (a time, a distance, or LAP) and its target
  (power in W from % FTP, heart rate in bpm from % max HR, cadence), coloured by zone; repeats are written out.
- **START** (or Select on a step: from that step) starts the workout, and the ride if none runs, on page 1. The
  step's clock follows the ride's timer (a pause or an auto-pause stops both); a 500 ms beep at 2 and at 1 s
  left, then a 1 s one and a banner with the new step and its target ("2/6 200-220W"). Lap goes to the next
  step; a step with no end (LAP) waits for it. One workout at a time; it ends with its last step or with the ride.
- **Fields** to place on a page: TARGET (the step's range, middle +- half-width: the digits white, an edge bar green in it, olive within 5 %, the zone 1 colour (teal) under, the zone 5 colour (wine) over,
  from the power over the GAUGE POWER window, the heart rate or the cadence), STEP LEFT (time, km, or LAP), NEXT (the next step's range, END on the last), and
  WORKOUT (during one, the step of all, "3/14"; otherwise today's planned time, for the overview).
- **WKT GRAPH**, a graph field: the workout opened over its whole length, each step as high as its target (power
  from 0 to 150 % of FTP, heart rate from half the max to it, a quarter for an open target) in its zone colour; a
  white line where the rider is, the part done dark. A distance step is drawn at 30 km/h, an open step as 5 min.
  Only the columns the line leaves and reaches are redrawn (about 300 px/s in a full-page box, sim).
- **WKT POWER**, a gauge field: the reading the step targets big (power over the window set by DISPLAY > GAUGE
  POWER: 3 s, 10 s, 30 s or each second, "POWER 3S" in its title; heart rate for a heart-rate step, cadence for
  a cadence one), a bar 3 times as thick as it was (as thick as the box leaves room for the number) spanning every zone: power from
  40 to 165 % of FTP (Z1 to Z7), heart rate from 50 to 100 % of max HR, widened to keep 10 % around a band outside
  them (cadence, which has no zones: 30 % around the band), painted with the rider's zones along that scale (the zone
  palette of the power and heart-rate fields), so the band shows where the target sits among them. The band
  is a white outline over the zones; the reading is a white tick with a black outline, its ends beyond the track
  coloured by how far it is from the band (green in it, olive within 5 %, the zone 1 colour (teal) under, the zone 5 colour (wine) over); the number is white.
  The scale is fixed for a step, so the zones do not slide. The climb pacer's TOP field draws the same gauge. With an
  open step or no workout, the power alone. Only the number's changed characters and the tick's old and new columns
  are redrawn (about 4 400 px/s for the page with a top half box, sim).
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
- **Climb pacer** (pacing.rs, the TOP field, on the course page in place of the distance): within 5 km of a climb
  and on it, the power to ride each 200 m section to reach the top soonest, from one CP / W' model (the fit of the
  rider's bests when it is trusted, else FTP and the W' setting, taken at the ride's start and kept to its end: it
  also drains the W' balance) and the W' balance
  now, planned again at each section from where the rider is, with the rider's weight plus the bike's (SETTINGS >
  BIKE > WEIGHT, 9.0 kg by default, also told to the trainer; a new ride forgets the plan). On the climb the power (3 s, or the GAUGE POWER window) shows in the workout gauge against the target's band
  (5 % each side, the target in numbers under the bar, or at the top right of a short box); ahead of it only the time.
  The time to the top ("TOP 4:49", in the title) counts down between plans. Power equation of Martin et al. 1998 for
  the speed; W' recovery inside the climb ignored; all the W' goes to the climb in hand.
  The plan is a closed form: with the budget spent above CP, every section not at a bound has the same optimal speed
  (the optimal-control result, "constant speed"), so a plan is one bisection on that speed: about 75 us at 200 MHz
  for 32 sections instead of about 1.9 ms (survey and sources in the module doc).
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
  it the rider is in to the end, the rider's place, the ridden part dark, the climbs in the zone 2 to 5 colours by
  grade.

## Climbs ahead (no course)

Riding outdoors with no course followed, the unit finds the climbs on the road ahead from the routing graph and
elevations of Bryton's own map files (`\Map\MapData\<Region>` or `\Map\Update\<Region>`, copied by the owner as for
stock: the maps are not in this repository), as Bryton's Climb Challenge 2.0, Garmin's ClimbPro in free ride and
Hammerhead's Climber do.
- **The road ahead:** the nearest road within 150 m (one in the direction of travel wins over a nearer one across it),
  then forward node by node for 4 km: the same road, else a road of the same class, else the smallest turn, never
  back, never a turn over 110 degrees; a dead end or a hairpin ends it. The tiles it crosses are read one a pass
  (about 5 KB each on the card), and a tile is loaded in slices (inflate resumed, then the decode and the sort, about
  3 ms a slice at most, estimated), where one tile took up to 238 ms in a single pass on the unit, so keys and touch
  keep answering.
- **Elevation:** the map's, a whole metre a node, smoothed over about 560 m of road by stock's 9-point filter (the
  steps of whole metres would show as grades of 7 %); a trace under 630 m is not smoothed.
- **Climbs:** the same rule as a course's (Garmin's: 500 m at 3 % at least), so a hill is one thing whether a course
  or the road ahead says so. At a climb's start (50 m before) the banner ("CLIMB 2.1 KM 6%") and the beep, once.
- **Fields:** CLIMB and CLIMB M as with a course (what is left of the climb and its grade; before it, its length, grade
  and gain), coloured by grade; "--" while the road ahead has no climb.
- **Again:** when the rider leaves the road traced (3 s more than 40 m from it), or nears the end of a trace that
  stopped at its length (1.5 km), at most every 5 s; after a failure (no map file, no road within 150 m) every 30 s.
  A U-turn (heading against the trace at 7 km/h or more) counts as leaving it. Only while riding (a pause keeps
  the trace, the climb said and the fields, and does the work no more; nothing happens slower than 7 km/h), never
  indoors, never while a course is followed. A long climb is said once, not again by the trace made inside it.
- **Roads:** edges stock marks not routable (speed index 0, 6 or over 14) are never taken or snapped to, and the slow
  class (index 1, inferred paths and steps) only when the rider is on one already.
- **BIKE > CLIMBS AHEAD** turns it off (ON by default; kept in register 11's tag).

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
  - **CP and W'** from the best powers of the last 90 days (1 s to 60 min), the 2-parameter model fitted as an
    envelope (power_curve.rs: Golden Cheetah's default fit, and Intervals.icu's 2-parameter model through a 3 to 5
    minute and a 10 to 30 minute effort): the line through one best of 2 to 5 minutes and one of 12 to 30 that no
    best rises above, so 2 and 3 minute bests never ridden all-out do not pull W' down as a least-squares fit did.
    **CP FIT** ("4/7 PTS 2%"): the bests of 2 to 30 minutes within 3 % of the model, of those there are, and the
    largest gap. The fit is trusted from 3 bests on it within 5 %, two of them of 2 to 5 minutes (one inflated
    best alone never sets W'), CP and W' in the settings' ranges. A stop or a power dropout ends an effort (a best
    never spans a cafe stop);
  - **A curve's bests are lower bounds**, so the estimates only raise the settings: **EST FTP** is CP when the fit
    is trusted ("CP"), else HRVT2 when three recent rides gave one ("HRV"), shown as a floor (">=236 W CP") when
    under the FTP setting; **APPLY FTP** offers it only above the setting, **APPLY W'** the fit's W' only above the setting too (Start twice,
    the source named: "SET FTP 240 W (CP)?"); **MODEL**: which CP and W' a ride takes, CP FIT (the trusted fit, its
    CP at least the FTP setting) or SETTINGS (the FTP and W' settings), chosen at the ride's start and kept
    to its end (during a ride: the ride's). A ride resumed after a restart takes what is there at its resume
    (usually the settings: no room is left in the backup registers to keep its pair);
  - **HRVT1 AET, HRVT2, RIDES**: the power at DFA alpha1 0.75 and 0.5 (Rogers et al. 2021) from the full copies
    of the last 42 days (`\Rustfw\Activities`, read when FITNESS shows, a bounded amount a pass, again after a
    failure at the next visit), each ride a linear fit of power on alpha1 over 2-minute windows every 5 s (Andriolo
    et al. 2024, the AI Endurance method) by Deming regression (least squares on a noisy alpha1 pulls the
    thresholds toward the ride's mean power), from the power where alpha1's mean falls under 1.0, gated (half the
    points under 1.0, 5 minutes of points over 40 W of power, a falling slope, r^2 0.3, at most 10 % above the
    ride's hardest 2 minutes), then the median of the 32 most recent rides that gave one; RIDES: "3/2 OF 7", the
    rides that gave HRVT1, HRVT2, of those read. A guide within some 20 to 40 W (hrvt.rs). A point whose
    2-minute window had over 5 % of its beats dropped as artifacts (the full copy's `ArtPct`) is left out;
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
  through power off, so a friend's is never taken. A silent sensor searches 30 s, then rests a minute, then 2,
  then 4 minutes at most while it stays away (a ride's Start, or the sensor heard, starts over from a minute). Needs the
  ANT+ key from your own stock firmware ([install.md](install.md)).
- **Power meter calibration** (SENSORS > CALIBRATE POWER, offered while the power meter is paired and heard; not
  yet seen on the unit): the screen says UNCLIP, CRANKS VERTICAL, NO PRESSURE; Start (or the CALIBRATE button) sends
  the ANT+ Bicycle Power calibration request (page 0x01, id 0xAA, as an acknowledged message, sent again if the air
  loses it) and waits 10 s for the meter: ZERO OFFSET with the number (and WAS, the offset before it this session),
  FAILED, or NO ANSWER. Once the meter has said it supports auto zero, an AUTO ZERO ON / OFF button (Page or Lap
  too) sends the configuration and shows the state the meter confirms. Not while a ride runs (a notice; paused is
  fine). Layouts and sources: src/drivers/powercal.rs.
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
- **Barometric altitude** (`src/nav/altitude.rs`, `src/app/barometer.rs`): the international barometric formula on the
  LPS22HH's pressure, smoothed (a median of 5 readings, then an average of 1/6: the median takes out gusts whole), calibrated by SETTINGS > BIKE > ALTITUDE (type the altitude, 1 m steps, a held key repeats),
  FROM GPS (30 s of 3D fix, six satellites), or by itself at a ride's start when the last calibration is over 3 hours
  old; kept in `\Rustfw\altcal.bin` between rides; stock's `Alt_P0` imported (not trusted). The ride, its graphs and the
  FIT records take the barometer's altitude when it reads, the receiver's otherwise. No drift correction in a ride
  (module doc: why).
- **Built in:** ambient light (automatic backlight, opt-in), barometer (temperature, pressure), battery.
- **Beeps** at power-on and power-off, for a workout's steps, alerts and courses: one tone player, so two sounds
  due together play one after the other.

## Settings registers

The settings live in the RTC backup registers (32 words kept through standby and a reset; the table is in
`src/app/settings.rs`), each word behind a tag so an empty or foreign one reads as unset, and earlier tags are still
read. All 32 are taken (the two diagnostics share 0 and 31), so a new setting needs the settings file (roadmap 3.4).
- **11:** wheel, backlight, max HR, FTP, touch, ride profile, climbs ahead; tag 0xa7 (AUTO), 0xa8 (ROAD), 0xa9 (INDOOR),
  0xaa to 0xac the same with CLIMBS AHEAD off.
- **16, tag 0xa:** the rider's weight less 30 kg in bits 0..6, W' in 0.5 kJ in 7..13, the gauge's window in 14..15, the
  bike's weight in 100 g in 16..23 (0: the 9.0 kg default), the smooth font in bit 24. Read from earlier builds: tag
  0xb (bike in kg, the single presses' actions), tag 0xc1 (weight in kg, W', window, actions; no bike weight, read as
  the default).
- **28, tag 0x9:** the eat and drink reminders in 5 min steps (bits 0..4 and 8..12), the long presses' actions (5..7,
  13..15), the heart rate alert in 5 bpm steps (16..21), the single presses' actions (22..24, 25..27). Read from earlier
  builds: tag 0xc2 (the heart rate in bpm, the single presses in register 16).
- **Others:** 5..8 and 10 the paired ANT+ sensors, 14 the shifting system, 17 and 18 the GNSS aiding, 20..27 the ride
  pages and the overview (0xd / 0xe), 29 the fault marker, 30 a ride open on the card.

## Phone (BLE, no app)

The unit pairs with a phone as a Bluetooth accessory, with nothing to install on the phone:
- **Pairing:** SENSORS > PHONE > PAIR advertises for a minute ("S510"; the phone sees a HID accessory, appearance generic HID);
  pair from the phone's Bluetooth settings (Just Works: no code). One phone is kept (its key and the notifications
  it enabled), in `\Rustfw\ble.bin` on the card, read 3 s after the start; FORGET erases it. A build that may not
  write the card keeps it until switched off. Not paired, the unit does not advertise at all; paired and not
  connected, it advertises once a second so the phone comes back by itself.
- **Media keys:** on an iPhone, through Apple's media service (AMS): play / pause, next, previous, volume; otherwise
  as a BLE keyboard's media keys (HID consumer control, also mute). Hold Lap on any page: play / pause. SENSORS > PHONE >
  REMOTE is a player, also opened by a swipe down on a ride page (a swipe up closes it; on the overview a swipe down is the quick panel): the title on two lines, the artist, the album, a progress bar with the elapsed and remaining
  time, and buttons: previous, play / pause, next, then skip back, volume down, volume up, skip forward (AMS's skip:
  the app sets how far, often 15 or 30 s in a podcast app; there is no "go to a position"). Keys: Start play / pause,
  Page next, Lap previous, held Page / Lap volume; swipes: left / right track; a skip the app does not take says NOT IN THIS APP; a hold takes a
  photo (the camera's shutter is volume up, always sent as HID).
- **Now playing** (AMS): title, artist, album, duration, playing or paused and the elapsed time; the title in the
  banner when the track changes; the MUSIC field (group PHONE) shows the elapsed time.
- **Notifications** (iPhone, ANCS): new ones in the banner and the overview's notices, "sender: text" (as much as
  two banner lines hold; accents folded to plain letters). Those already on the phone when it connects are not shown. NOTIFY
  OFF stops them.
- **Calls:** over every screen (a page, a menu, the media remote, MORE): "INCOMING CALL" and the caller in the banner at the top, a red decline and a green answer button
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
