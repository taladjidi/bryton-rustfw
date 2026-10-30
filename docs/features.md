# Features

What the firmware does, built and tested: each feature, how it works and its numbers.
Not here: what is not built (roadmap.md), what the unit has not shown yet (status.md), dated findings (journal.md).
Where to look: checklist.md compares the features with stock, Garmin and others; docs/layout.md maps the files.

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
  44 x 44 px box opens the menu). A swipe down, on the overview and on every ride page, opens the **quick panel**, what to see right away and not
  a way to the settings (a swipe up or Back closes it). Top to bottom: **media** (the title and the artist, one line
  each, which scroll at 4 characters a second with 2 s holds at each end when wider than the screen; a row of previous,
  play / pause and next, then VOL- and VOL+ below it, 44 px high with 8 px between, the media remote's actions; greyed
  without a phone); the **power meter** when one is paired (the 3 s watts, its battery, and ZERO, which runs the zero-offset
  calibration in place and writes "ZERO -12 OK", "FAILED" or "NO ANSWER" on the same block; refused while the ride
  runs); one line per **paired sensor** (at most 4 under the power block, 6 without; heart rate, speed, cadence, speed and cadence, shifting, trainer: its value or
  SEARCHING, and its battery when the sensor said it, ANT+ common page 82: bars, or LOW in red); the heart rate strap's
  battery in percent from its page 7 (HR BATT shows it, else the page 82 status; layout as openant reads it,
  `devices/heart_rate.py`, checked 2026-10-30, not yet against the ANT+ HRM document); SENSORS names a paired sensor
  with its maker's short name from common page 80 (manufacturer id in bytes 4-5, `devices/common.py`; the FIT SDK
  list: POLAR HR, SRAM SPD+CAD; device number as the value; unknown ids keep the type name), and the panel's lines
  keep their type; and **brightness** buttons (the levels, AUTO at the bright end). There is no status line: the
  top strip already shows the phone link, the time and the battery. Each box is redrawn alone, and the boxes are
  checked every 250 ms. PROFILE, USB DRIVE, NOTIFICATIONS (still a tap on the overview's notices) and
  TRAINER CONTROL stay in their menus.
  **TODAY card** (the owner's idea, 2026-10-26: the big clock went, the strip has the time): "TODAY" and the planned
  length (H:MM) on one line, the workout's name under it (cut to whole characters), and its profile at the bottom: a
  bar per column (56 columns of 4 px, the width of the card) in the colour of the step's zone and as high as its
  intensity (the WORKOUT GRAPH's look and scale: power up to 150 % of FTP, heart rate from half the maximum), so a
  sweet spot, a VO2 session and a long easy ride tell apart at a glance. "REST DAY" in grey when nothing is planned
  today. A tap anywhere on the card opens today's workout, its steps with START selected (a tap on START, or Start, plays
  it: two inputs from the overview; Start held on the overview does the same, while no ride is open), and on a rest day
  WORKOUTS (not while a ride records, as every touch). Drawn once: when the
  overview opens and when the calendar or the day changes; nothing per second (the overview now costs 4 px/s, the
  strip's, where the clock's digits made it 53). The calendar is read 1 s after the start (the card is not touched at
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
  grade; and the trip and the laps: trip time (seconds running, auto-pauses in, a Start pause out), lap count, last
  lap time, lap average power, last lap average power, lap NP, top cadence, top altitude, total descent (the ascent's
  hysteresis: 5 m from GNSS, 3 m from the barometer), lap ascent and lap descent, and the distance uphill and downhill
  (the grade of the last 30 s at +1 % or more, and at -1 % or less, and the 30 s window must also rise or fall by the
  ascent's hysteresis; flat riding and GNSS noise count in neither). **Not kept over a resume** (the recovery record
  has no room for them): after a resume the laps, the last lap, the lap's power and NP, the top cadence and altitude,
  the descent, the lap's ascent and descent, and the uphill and downhill restart from zero. The trip time restarts
  from the moving time (the timer), so it is never less than the timer. Top altitude and the descent's high point also
  restart when the altitude source switches from GNSS to the barometer (or back). Heart rate and power against the rider's numbers: percent of the max heart rate and of LTHR (LTHR is in ME,
  "--" until it is typed), the heart-rate zone (1 to 5, from the max) and the power zone (1 to 7, from the FTP) now, the
  time this ride in the zone of now (heart rate and power: the moving seconds), percent of FTP now (3 s), average, lap,
  10 s and 30 s, and the average left / right balance (over the readings that had one). Percents are whole numbers, at
  most 999. **The zone times and the average balance restart from zero after a resume** (not kept in the recovery record), as the
  laps do. The zones are the ones the bars show (held, with the bars' margin), so a number, its colour and its time agree. POWER
  SMOOTH (BIKE: the live reading, 3, 5 or 10 s; the live reading by default) sets the window of the big power value and
  of its zone colour; it is kept in settings.bin after the alert limits (LTHR at byte 66, POWER SMOOTH at 68), and an older file reads the default. **Default pages** (as Garmin, Wahoo and Hammerhead ship theirs: data, climbing, map): 1, speed, moving
  time, distance, average speed, heart rate, power; 2, the elevation profile with ascent, grade, VAM and cadence;
  3, the map, whole. While a course is followed, a course page joins the round after the ride pages (and opens
  when the course is loaded): its profile large, the next turn, the distance to go, the climb pacer (TOP, below) and moving time.
- **Sun, heading, arrival and batteries (nav/sun.rs, app/fields.rs)**: heading as one of 8 compass points (N first)
  from the GNSS course over ground, "--" under 5 km/h or without a fix. Sunrise and sunset in local time, and the time
  left to today's sunset (h:mm, "--" once it has set): NOAA's solar equations (Meeus, ch. 25 and 28, as NOAA's calculator
  writes them; cited in the module), from the fix's place and the RTC's date. The RTC is local time, so the zone is the
  offset learnt from the phone or the fix; no fix, no zone, or polar day or night: "--". The sun is worked out once a
  minute, not per pass. On a course: the time to the end at the average moving speed ("--" under 5 km/h average) and the
  clock of arrival (ETA); the distance to go is the course's own field. Batteries: the unit's percent (the status bar's),
  and the power meter's and the heart-rate strap's ANT+ status (page 82: NEW, GOOD, OK, LOW, CRIT). The Di2 battery is the
  shifting field. Not yet: the sunset alert.
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
- **Map:** the track since power-on, north up, about 1 km across around the rider (fitted to the whole track until a fix), an arrow for position and heading, a
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
    the default view. Kept until the unit is switched off.
  - **Default view:** with a position and no zoom set by hand, the map is about 1 km across its smaller side (4 m a
    pixel on the full-page map), centred a quarter of the map ahead of the rider and redrawn when they leave the
    middle three fifths; no fix yet: the whole track fitted. With a course followed, the crosshair toggles between
    that view and the whole route (fitted) and back. A hold on the map opens the page
    editor with the map's box selected (not while a ride records, as on any box); a map of half a page or less has no buttons. The fitted map keeps the arrow clear of the buttons.
  - **Pan:** at a zoom set by hand, drag the map: it moves with the finger, redrawn once when the finger lifts
    (one new basemap, no redraw while dragging), at most two map widths past the track and the course. The map
    then stays put instead of following; "+" and "-" zoom around the centre panned to; the crosshair goes back to
    the fit. While the zoom is set by hand, a drag on the map pans instead of changing page or opening the media
    remote; the keys still change pages. Fitted, swipes keep their meanings. A new ride, or a course followed or
    dropped, returns to the default view.
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
  Also off by default (ALERTS, the list scrolls): power above / below (10 W steps), cadence above / below (5 rpm),
  speed above / below (km/h), heart rate below (5 bpm), and DFA a1 below (hundredths, 5 steps: 75 reads 0.75, the
  aerobic threshold of patch P9; only with the strap's beats). A limit alerts once its value has stayed past it for
  5 s (not on one sample), re-arms once the value is back inside by 5 % of the limit (at least 1), and says at most
  once a minute; no reading from a sensor, no alert; only while the ride runs (not paused, not outside a ride).
  Each is a red banner ("POWER HIGH", "POWER LOW", "CADENCE HIGH", "CADENCE UNDER", "SPEED HIGH", "SPEED LOW",
  "HEART RATE LOW", "DFA A1 LOW") with the same three beeps and queue. Reminders by distance (every N km, 1 to 99), kcal and kJ
  (every N, steps of 10) say "DISTANCE REMINDER", "KCAL REMINDER", "KJ REMINDER" once each time the ride crosses a multiple of the
  step (the first second of a ride, and a step set mid-ride, do not fire). Speed limits are in km/h whatever the units.
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
| tap the cog | the menu; while recording too (settings change as on the keys, the ride goes on) | |
| Lap | a lap (while riding), on the overview too | up (stops at the first row) |
| Start | start, pause, resume the ride (from the overview: and show page 1); a banner says which | select, or change the value |
| hold Start | a paused ride: the end-ride screen (RESUME, SAVE, DISCARD; or END RIDE in the ride menu); the overview with no ride: today's workout; on a past ride's card: DELETE, asked | |
| Back | on a ride page, the overview; on the overview, nothing; with a banner up, it only clears the banner | back |
| hold Back 3 s | power off (the radio and GPS too) | power off |
| hold Lap | the phone's music: play / pause (PHONE below); on the trainer screen while riding: the target one big step down (Page is up) | |
| hold Back and Lap together 1 s | the touch lock on or off, on every screen: all touch is ignored and a padlock shows in the strip (until unlocked or switched off); neither key acts on its own | the same |

The power button starts it again. After the backlight's time-out, any key brings it back. A fresh unit (no saved
settings) starts with BRIGHTNESS AUTO and OFF AFTER 30 S; saved settings, or stock's imported, keep theirs.
AUTO OFF (SYSTEM > AUTO OFF: off, or after 5, 10, 30 or 60 min idle) switches the unit off with no ride, cable, drive
or write waiting and no movement; details: docs/power.md.

- **Menus:** rows 44 px tall (about 8 mm, a fingertip), six a screen, a longer menu turned a screen at a time;
  a "<" left of the title goes back. The selected row is black on orange (10:1 contrast; white on that orange was
  2:1); a row's value is white like its label, green when it says a state that is on (a sensor heard, the phone
  linked).

- **The page editor** (app/editor.rs): a hold on a ride page's box, PAGES > a page > EDIT ON
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
- **Page switches (PAGES > ROAD PAGES > a page):** SHOW (ON or OFF: Next and the swipes skip a page that is OFF; at least one stays ON), MOVE UP and MOVE DOWN (the page swaps places with its neighbour, its MORE variant, switch and name with it; every set), RENAME (a name of up to four characters, A to Z, 0 to 9 and space: Next and Prev step the character, Start goes to the next one, Back ends; the name shows in the list and the page's menu title, "PAGE n" while empty). PAGES > AUTO SCROLL (OFF, 5, 10, 20 s): during a ride the next ON ride page comes by itself; it waits on the overview, a menu, a banner, MORE, the CLIMB, course and workout pages, and a key or a touch starts the count again. Switches and names are the road set's and are kept in `pages.bin` after the MTB part (a flags byte, the OFF mask, six names of 24 bits, a CRC of their own; an older file gives every page ON and no names; the sector has room for four characters, not eight). A swipe up on a ride page shows its MORE variant, a second one (or one on the course, workout or climb page) goes home, to the overview.
- **Pages per profile:** an INDOOR ride (a trainer heard at Start, or INDOOR set by hand or in BIKE) has its own set
  of up to six pages beside the road set: the trainer (3 s power large, the trainer's target, heart rate, cadence,
  timer, work), the power numbers (average, NP, W' balance, IF, work, TSS) and the power and heart rate graphs, with
  their MORE variants; the overview is shared. The editor and PAGES work on the profile's set while a ride is open,
  else the one PAGES > ROAD PAGES or INDOOR PAGES opened. The indoor set is in `pages.bin` only (after the MORE part, a CRC of its own,
  so earlier builds ignore it and a file without it gives the defaults), never in the backup registers.
**MTB** (BIKE > PROFILE): chosen by hand and kept until changed; a trainer heard still makes the ride INDOOR, and MTB comes back when it goes quiet. The ride file says mountain biking, the GNSS receiver tracks continuously whatever GPS POWER says, auto-pause waits for 5 s under 2 km/h (slow climbs keep the timer running), its own 4 pages (time, distance, speed, heart rate; the altitude graph, grade, ascent, VAM; totals; the map), and climbs ahead follows paths and tracks.

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
| swipe up / down | down on the overview before a ride: the quick panel; down on a ride page: the media remote; up on a ride page: MORE, then the overview | the next / previous screen of rows (up closes the quick panel) | one step up / down |
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

**Theme AUTO.** THEME's list starts with AUTO: the day theme (DAY THEME, OUTDOOR at first) and the night one (NIGHT THEME,
DARK at first) then follow the ambient light: night under 20 lux, day again over 50 lux, each held 30 s, at most one
switch a minute, a full redraw at each. The light sensor measures while AUTO is on. Details: theming.md 4b.

**Theme.** THEME in SETTINGS > DISPLAY (Select) goes through AUTO (when on, see above), DARK (the colours as they were), OUTDOOR (lighter notes
and lines, brighter zones), LIGHT (black on white, for the sun) and the `.ini` files of `\Rustfw\Themes` on the drive (up to 14, names of letters, digits,
`-` and `_`, 12 characters at most), drawing the screen again at once. The choice is kept by name in settings.bin (not
in the backup registers), and a file is read again at each start. A file is an INI, read, never written. `tools/theme.py examples` writes the three built-in themes as files with every key
set (a comment line per key); copy one to `\Rustfw\Themes` and edit the copy. The keys are in `[theme]`, `[metrics]` and
`[colors]`; the defaults are DARK's values in those files. `[metrics]` values are whole px, clamped into their range.
Colours are `#RRGGBB`. A key left out keeps the base's value.

| Key | Meaning | Range |
|---|---|---|
| `version` | the format; above 1 the file is refused (THEME TOO NEW) | 1 |
| `name` | a label; THEME shows the file's own name | any text |
| `base` | the built-in theme the keys left out are taken from | dark, outdoor or light |
| `overview` | the overview's layout: classic (the TODAY card over four fields) or fields (a 2 x 2 table; the Workout cell shows the step counter or DONE) | classic or fields |
| `zone_bar` | height of a field's zone bar, px | 4 to 8 |
| `frame` | the page editor's frame round a cell, px | 1 to 4 |
| `menu_row` | menu row pitch, px; a taller row shows fewer rows (4 at 56) | 44 to 56 |
| `background` | every screen's background | colour, 4.5:1 for the text on it |
| `text` | labels and values | colour, 4.5:1 on background |
| `line` | menu row lines, empty bars | colour, 3:1 on background |
| `dim` | notes and axes | colour, 4.5:1 on background |
| `accent` | the selected row, banners, frames | colour; on_accent on it needs 4.5:1 |
| `on_accent` | text on the accent | colour, 4.5:1 on accent |
| `alarm` | alarm banners, BACK, bad readings: the only red | colour, 4.5:1 on background |
| `on_alarm` | text on the alarm | colour, 4.5:1 on alarm |
| `ok` | the strip's good dots | colour, 4.5:1 on background |
| `warn` | the trainer's states | colour, 4.5:1 on background |
| `ferris` | Ferris | colour, 3:1 on background |
| `cog` | the overview's menu cog | colour, 3:1 on background |
| `course` | the course on the map | colour, 3:1 on background |
| `zone1` | power and heart rate zone 1 (also grade, gauge, W' balance) | colour, 3:1 on background, all zones distinct |
| `zone2` | power and heart rate zone 2 | colour, 3:1 on background, all zones distinct |
| `zone3` | power and heart rate zone 3 | colour, 3:1 on background, all zones distinct |
| `zone4` | power and heart rate zone 4 | colour, 3:1 on background, all zones distinct |
| `zone5` | power and heart rate zone 5 | colour, 3:1 on background, all zones distinct |
| `zone6` | power and heart rate zone 6 | colour, 3:1 on background, all zones distinct |
| `zone7` | power and heart rate zone 7 | colour, 3:1 on background, all zones distinct |


A value that is not a colour or a key that is not in the list is ignored. A file that is not a theme, over 4 KB, with a
version above 1, or that fails the whole-theme checks is refused, with the reason in a notice, and DARK is drawn:
Text (`text`, `dim`, `accent`, `alarm`, `ok` and `warn` on `background`; `on_accent` on `accent`, `ok` and `warn`;
`on_alarm` on `alarm`) needs a contrast ratio of 4.5:1 and the rest (`line`, `ferris`, `cog`, `course`, the zones, on `background`) 3:1 (WCAG 2.1 1.4.3 and 1.4.11; a pair whose two colours are the base's is not checked: DARK's white on red is 4.0:1, its `line` and `zone7` are under 3:1); red is for alarms (`alarm` is no other colour of the file) and no two zones are alike. Pictures (the basemap, IGN tiles) are never recoloured.
Built-in names cannot be a file's. Fonts in a theme come later (docs/theming.md).

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
- **Power average by tap:** during a ride a tap on any 3 s / 10 s / 30 s power field (ride pages, MORE, workout and CLIMB pages, the overview) cycles the window 3, 10, 30 s; every power-average field, the workout gauge and the climb pacer follow it, and it is kept across a restart (the former GAUGE POWER setting). The page editor shows each field's own window; the quick panel's watts stay 3 s.
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
  offline. While a home trainer is connected (ANT+ FE-C or Bluetooth), to diagnose ERG drops, each record also
  carries `target_w` (the ERG target the trainer should hold, invalid outside ERG), `trainer_w` (the trainer's power), `meter_w` (the power
  meter's), `fe_state`, `trn_stat` (the trainer status bits and the target power limits of FE-C page 25),
  `link` (0 ANT+, 1 Bluetooth), `cmd_sent` (a bit per control page sent that second: 1 resistance, 2 target
  power, 4 track, 8 set-up) and `ack_fail` (ANT TX_FAILED events that second): 11 bytes a second, about 40 KB an
  hour; and the trainer's manufacturer, model, serial and software version (ANT+ pages 80 and 81) once, in a
  `device_info`. It is a second file because Bryton's server refuses developer fields, and the `\Activities` file
  stays exactly as before. It is recorded beside the clean one with its own checkpoints and resume, and it never
  holds the clean one up: if it fails, the ride is saved all the same and the banner says "NO FULL COPY".
- **Sync on the computer** (`tools/sync.py`, docs/sync.md): new rides go to Strava (the clean copy) and to
  Intervals.icu (the full copy), never twice. The next days' planned workouts come from Intervals.icu into
  `\Workout\calendar`. It works from the mounted drive, or reads rides over the dev link.

## Menu

Hold Page (or tap the cog) for the menu: what to ride first, then what was ridden, then the set-up. A row that opens a menu or a screen ends in ">"; a value sits on the right with its unit ("FTP  200 W"). Any
setting is three levels down at most (MENU > SETTINGS > ME > FTP; a page's own rows are four: MENU > PAGES >
ROAD PAGES > PAGE 1 > LAYOUT), the diagnostics under SYSTEM:

```
MENU
 +- WORKOUTS >     the planned workouts, from today -> a workout's steps: START, or start from a step; last row RAMP TEST
 +- COURSES >      the course files (green: followed); Start follows or stops
 +- SEGMENTS >     LIVE SEGMENTS on / off, TARGET (AUTO, PR, KOM, QOM, RECENT), the segments of the card (name, distance,
 |                 gain; read only), the files skipped
 +- HISTORY >      FITNESS > (READ RIDES, CP, W', CP FIT, EST FTP, MODEL, APPLY FTP, APPLY W', HRVT1 AET, HRVT2,
 |                 RIDES, WEEK / YEAR / TOTAL KM and H), then the rides -> a card
 +- PAGES >        ROAD PAGES >, INDOOR PAGES > -> OVERVIEW (four fields), PAGE 1..6 (EDIT ON PAGE, LAYOUT,
 |                 FIELD n -> group -> field), ADD PAGE (a page's own rows are four levels down)
 +- SENSORS >      CALIBRATE POWER (a meter heard) and TRAINER CONTROL (a trainer paired) first; then HEART RATE,
                   SPEED, CADENCE, SPEED+CAD, POWER, SHIFTING, BT TRAINER; DI2 KEYS >; PHONE >
 +- SETTINGS >
     +- ME >       MAX HR, FTP, WEIGHT, W'
     +- ALERTS >   EAT, DRINK (every so many minutes), HR ABOVE
     +- DISPLAY >  BRIGHTNESS (or AUTO), OFF AFTER, TOUCH, FONT, THEME
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
  to repeat, Start to keep. A choice (BRIGHTNESS, OFF AFTER, TOUCH, WHEEL) goes to its next value.
- **SYSTEM:** USB DRIVE (below), IMPORT FROM STOCK (stock's settings, below), GPS (fix, satellites, position,
  altitude, UTC), BLE devices nearby, the built-in sensors' readings, ABOUT (Ferris, the version, the commit the
  build is made from and its date; `tools/dev.py info` gives the same commit; the notices of the map data, "(c)
  OpenStreetMap contributors (ODbL)", "(c) OpenMapTiles", "(c) IGN (Plan IGN v2, Etalab 2.0)", and the font's, "BARLOW,
  SIL OFL 1.1"; drawn once when it opens), STORAGE (the card: TOTAL and FREE from the volume, FREE shows FSInfo's count with a
  "~" (only a hint: stale after a power cut) until the FAT is counted over the passes, then the counted value, RIDES in `\Activities`, then the five oldest by name, each as its
  day and size; a tap opens the ride's HISTORY card, where DELETE is). STORAGE is read only, a pass at a time while the
  screen is open and no ride runs or waits to be saved (`--` until then, or while the card is lent to the computer);
  DELETE restarts the count.
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
| Calibrate the power meter from the overview, touch | 6 | 2 (swipe down, ZERO; result in place) | `the_quick_panel_has_the_remote_the_meter_and_the_light` |
| Play / pause the phone's media, touch (overview or any ride page) | 2 (swipe down, a tap; a ride page: swipe down, a tap on the remote) | 2 (swipe down, a tap), the panel also shows the meter, sensors and light | same |
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

## Laps

- **AUTO LAP** (SETTINGS > BIKE): OFF (the default), BY DISTANCE or BY TIME, with LAP EVERY 1 to 50 km (5) or 1 to 60 min (10). An auto lap is the Lap key's lap (`Ride::lap`, the same recorder event), so the FIT file holds the same lap message; it does not move a workout on. Only while the ride runs.
- **BY DISTANCE** counts from the last lap, manual or auto, not from the ride's total (as Garmin restarts it after a manual lap): with 5 km, a manual lap at 7 km makes the next at 12 km. The lap falls on the first second past the step, so it can be a few metres over.
- **BY TIME** counts moving time (the lap time field): a stop or a pause does not count.
- **The lap banner** (Lap key or auto lap, 8 s or until a key): lap number, lap time and distance, then average power (speed without power) and average heart rate if a strap was heard. In a workout the step banner shows instead of it (Lap moves the step on, and the step change tells the rider); so do the step laps.
- Kept in settings.bin at data 94 (mode), 95 (km) and 96 (minutes); an older file reads OFF, 5 km, 10 min. Not yet: the lap list of a past ride.

![Lap banner](img/auto-lap.png)

## Power match (ERG)

A trainer's own power is a little off from a power meter's (the owner's D900 holds about 20 W from the Sigeyi). With
**SETTINGS > BIKE > POWER MATCH** ON (the default), ERG corrects the target it sends so the **power meter** reads the
workout's target (Garmin's power match, Zwift's "power meter as the power source"). Once a second, with ERG held, the
meter and the trainer both heard within 2 s, pedalling, and both powers above 30 W, the ratio trainer / meter feeds an
average (about 30 s, kept between 0.85 and 1.15); the target sent is the rider's times it. Nothing is sampled for 5 s
after a target change or a resume. A meter lost keeps the ratio 60 s, then it eases back to 1.0 over 30 s; a new
trainer or meter, or a new ride, starts at 1.0. A page 49 goes out only when the corrected target moves by 2 W, or
at a step or the 4 s resend. The trainer screen shows the correction ("PM+6%") left of the source line; the target
shown, recorded (`target_w`) and compared is always the rider's. Kept in `\Rustfw\settings.bin` only (an older file
reads ON). Code: `src/training/power_match.rs`.

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
With BIKE > PROFILE fixed to INDOOR the GNSS receiver stays off between rides too. Otherwise, between rides, it
searches 5 minutes after it is switched on, and stays on with a fix until 10 minutes pass without a key, then switches
off; it comes back at Start, for a page with a map, the GPS menu or a course followed (a fix about 30 s later outdoors:
the time and last position are sent to it, the ephemeris is not kept).
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
- **DONE:** a planned workout shown as done on the TODAY card (a tick and DONE) and in WORKOUTS (a tick).
- **START** (or Select on a step: from that step) starts the workout, and the ride if none runs, on page 1. The
  step's clock follows the ride's timer (a pause or an auto-pause stops both); a 500 ms beep at 2 and at 1 s
  left, then a 1 s one. The step change says itself through the **next-step strip** (patch P3, stock's popup as a
  thin line): for the last 9 s of a timed step (stock's window; the last 49 m of a distance one) the status strip is
  replaced by "NEXT 275W 1:00" (the next step's target middle with its unit and length; "LAST STEP" before the end)
  and a countdown at the right ("  7S", " 45M"); a second redraws one cell (240 px, sim; the strip whole is 10 560 px,
  once). The status strip comes back at the step change, with no banner then (the beeps stay). A step that was not
  announced (the first, a Lap skip) still gets the banner with the new step and its target ("2/6 200-220W"). A banner
  over the strip hides it, and the status strip's icons (pause, USB, lock, time) wait while it is up, and the strip is drawn again when the banner goes. Lap goes to the next
  step; a step with no end (LAP) waits for it. One workout at a time; it ends with its last step or with the ride.
- **Fields** to place on a page: TARGET (the step's range, middle +- half-width: the digits white, an edge bar green in it, olive within 5 %, the zone 1 colour (teal) under, the zone 5 colour (wine) over,
  from the power over the tapped window (3, 10 or 30 s), the heart rate or the cadence), STEP LEFT (time, km, or LAP), NEXT (the next step's range, END on the last), and
  WORKOUT (during one, the step of all, "3/14"; otherwise today's planned time, for the overview).
- **Two targets per step:** a FIT workout step may carry a secondary target (`secondary_target_*`, fields 19 to 22 of
  `workout_step`: power, heart rate or cadence, the same custom values and zones as the primary; speed targets are not
  followed). WKT POWER shows it after the primary under its bar ("200-250/90-100RPM"), and its marker colour (and the
  TARGET edge bar's) is the primary's state, or the secondary's only while the primary is in range (green): one thing
  to fix at a time. Without a reading for the secondary, the primary's colour stays.
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
- **CLIMB page:** one page for a live segment and for a climb (a followed course, or climbs ahead without one), in the round of ride pages. It opens once at each segment's start and each climb's start, over the ride page or the overview the rider is on (only while recording, not over a menu), and leaves the round at the segment's finish or loss and at the climb's top. From the top: the head (a segment: its name, the delta in big digits and the target; a climb: its title and the gain left in whole metres), the profile coloured by grade with the part done darker, the rider white and, on a segment, the target's place as a grey mark, the climb coach's gauge over it (the power against the band for this section, the time to the top in its title), then the elapsed time and the distance left. A segment and a climb at once: the segment drives the page (the profile and the distance are the segment's, the coach plans over the segment when its file has altitudes, else over the climb). Not added for a climb alone when one of the rider's pages already shows climb fields; a segment's page always opens.
- **Climb pacer** (pacing.rs, the TOP field, on the course page in place of the distance): within 5 km of a climb
  and on it (the followed course's, else the road ahead's; and, in a live segment being ridden, the rest of the segment
  from its file's altitudes, on its own 200 m grid, until it ends or is lost), the power to ride each 200 m section to reach the top soonest, from one CP / W' model (the fit of the
  rider's bests when it is trusted, else FTP and the W' setting, taken at the ride's start and kept to its end: it
  also drains the W' balance) and the W' balance
  now, planned again at each section from where the rider is, with the rider's weight plus the bike's (SETTINGS >
  BIKE > WEIGHT, 9.0 kg by default, also told to the trainer; a new ride forgets the plan). On the climb the power (the tapped window, 3 s by default) shows in the workout gauge against the target's band
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

## Zones

One zones table (training/ride.rs `Zones`, from `Settings::zones`): the bars, the colours, the HR ZONE and PWR ZONE fields and their
times, the graphs, the workout profiles and gauge read it, so a change moves all of them.

- **ZONES BY** (ME): MAX HR (default, 60, 70, 80, 90 % of the maximum: five zones) or LTHR (the maximum's while LTHR is unset).
  LTHR zones are Joe Friel's cycling zones (TrainingPeaks, "Joe Friel's quick guide to setting zones"; the 220 Triathlon table of them):
  zone 2 from 81 %, zone 3 from 90 %, zone 4 from 94 %, zone 5 from 100 % of LTHR. Friel's Z5a, Z5b and Z5c (from 100, 103 and 107 %)
  are one zone 5 here, since the bars, colours and times have five heart-rate zones. Power zones stay Coggan's seven of the FTP.
- **ZONES** (ME > ZONES): the top of each heart-rate zone 1 to 4 (bpm, step 1) and power zone 1 to 6 (W, step 5), edited in place
  between the neighbouring bounds, and RESET (back to the computed zones). The list shows the computed bounds until one is edited;
  the first edit makes all of them the rider's own, which stay as they are when FTP, MAX HR, LTHR or ZONES BY change, until RESET.
  A workout file's zone target (a zone number, `target_value`) takes its band from these zones (`Target::range`), so the step's
  band, target and colours agree with the bars; a target written as a range is as written.
- **SEX, BORN (year) and HEIGHT** (ME): male or female, a year (1920 to 2020) and cm; unset until the first press (1985, 170).
  SEX picks hrTSS's constant; BORN sets MAX HR to 220 minus the age (banner "MAX HR n FROM AGE") when it is still the unset default 190 or was itself
  worked out from the age (a flag, data 149, recomputed when BORN changes; a MAX HR edit by hand clears it, so a maximum that was
  set is never replaced). With the clock unset (year before 2024) or an age under 5 it says "SET THE CLOCK" / does nothing. HEIGHT is kept for later: no formula uses it yet, and kcal come from the work (power) only, so none uses
  sex or age.
- **hrTSS** (training/hrtss.rs): on a ride without power the TSS field and the end card show the TRIMP of the ride's moving seconds
  (Banister, k 1.92 men and 1.67 women, a resting rate of 60 bpm assumed) against an hour at LTHR, times 100. "--" while LTHR is unset.
  It restarts after a resume, as the zone times do; HISTORY's TSS stays power-only (hrTSS is live and on the end card only), so past rides without power show "--". The FIT file is unchanged.
- **Kept** in settings.bin after the odometer: ZONES BY (data 123), a custom flag (124), the ten bounds (u16, 125..145), SEX (145), BORN
  (u16, 146), HEIGHT (a byte, 148) and MAX HR FROM AGE (a byte, 149): 150 bytes. A file of 44 to 123 bytes reads the computed zones, MAX HR, male and none set;
  bounds that do not rise read as the computed zones. Fuzzed (`settings_file_zone_bytes`).

## Units, sound, trips

- **UNITS** (SETTINGS > DISPLAY): METRIC (the default) or IMPERIAL, and **TEMPERATURE** C or F. One place formats every value (`app/fields.rs`
  `field_text`, with `mph_x10`, `feet`, `fahrenheit_x10`, `push_dist`, `push_trip`). UNITS affects: the ride page fields (speed KM/H, AVG,
  MAX in mph with one decimal; distance KM, LAP KM, UP KM, DOWN KM in miles, 2 decimals under 100 then 1, truncated as the metric
  one is; ascent, descent, lap ascent and descent, max altitude and VAM in feet, rounded; the whole-unit totals WEEK, YEAR, TOTAL in
  miles; the trips and the odometer; the temperature in F) with their labels (`Field::label_in`: MPH, MI, FT, TEMP F), the end-ride
  screen, the lap banner, the summary card and the HISTORY cards and rows (MI, AVG MPH, ASCENT FT), the sensors panel (MPH), and the
  SPEED ABOVE / BELOW alert rows (shown in mph, kept and compared in km/h, so editing steps 1 km/h). **These stay metric:** the FIT
  file, the history files and summaries, the map's scale bar, the course fields (TO GO KM, OFF CRS M, CLIMB M, UP LEFT M), the climbs
  ahead and the CLIMB page, the segment values and banners, the workout steps' distances, the settings' own units (wheel mm, weight kg,
  LAP KM, AUTO PAUSE km/h), the trainer screen and the stats and storage figures of SETTINGS. Feet are 4 or 5 digits where metres were 3 or 4: the
  value takes a smaller size past the box's width, as every long value does.
  ![Imperial units](img/imperial-units.png)
- **SOUND** (DISPLAY, ON): OFF drops every tone the app plays (alerts, course and workout tones, the start and pause beeps) and cuts
  the one sounding. The alerts' banners still show. It also silences the heart rate alarm, the off-course alarm and the low battery tone: only
  the banner shows. The power-on and power-off melodies stay (they play before the settings are read and after the screen is off). There is no key click: this unit's keys have no beep today, so a key click was not added.
- **Start reminder and ready check** (SETTINGS > BIKE: START REMINDER and READY CHECK, both ON; app/start_cues.rs). Neither starts, stops or touches the recording by itself.
  - START REMINDER: with no ride (not running, not paused), on the overview or a ride page, the cable out, the bike moving for 10 s in a row (GPS speed over 8 km/h, a paired wheel sensor turning, or power above 0) says START THE RIDE? in a banner with one beep (SOUND OFF: the banner only), at most every 5 minutes. Start then starts the ride as it always does. A menu open, USB power in, or a ride running or paused: nothing, and the count starts again.
  - READY CHECK: Start on a new ride (a resume or a pause shows nothing) shows a two-line card for 3 s: the GPS fix (YES / NO) and the unit's battery, then the warning: LOW BATT at 15 % or less, NO and the names of the paired sensors not heard in the last 5 s, else the sensors found and OK (NO SENSORS PAIRED with none). Names are written while they fit the banner, then "+N" for the rest. Shifting and Di2 count only when heard (they may stay silent at a standstill, which cannot be told from the sensor being gone without the unit). Any input skips it; Start on it only skips it, with a short tone (SOUND) and a RECORDING notice, so a rider who meant to pause knows it did not. **The recording starts at the press**, in the same call as without the card (the FIT start is the press, no data is lost); the card is a banner over the ride page, so it can neither delay nor prevent the start. The sensors are judged as heard at the press: the channels that were resting search again from it. Kept in settings.bin data 150 and 151 (ON in a file that predates them).
  ![Start reminder](img/start-reminder.png) ![Ready card](img/ready-card.png)
- **BACKLIGHT ALWAYS ON** is LIGHT OFF NEVER (SETTINGS > DISPLAY), as before.
- **AUTO PAUSE** (SETTINGS > BIKE): AUTO (the profile's own rule: road under 2 km/h at once, MTB under 2 km/h for 5 s, indoor
  on no power and cadence for 3 s), OFF (the timer never auto-pauses, any profile), or 2, 3, 5 or 8 km/h replacing the 2
  km/h of the road and MTB rules (the indoor rule is power and cadence, not speed, and stays).
- **TRIP 1, TRIP 2 and the ODOMETER** (SETTINGS > BIKE, and the fields TRIP1 KM, TRIP2 KM, ODO KM, TRIP1 H, TRIP2 H in the TOTALS group):
  the distance (and moving time) of the rides saved since each trip was reset, and of every ride saved. A ride is added only
  when its save is done (`App::pay_trips`; a discarded ride adds nothing, a ride kept in `\Rustfw\Rides` adds with the next
  save), and the totals reach `settings.bin` between rides: nothing is written to the card during a ride. The fields show the
  saved total plus the ride running and the rides ended and not yet saved, as WEEK KM does. Select on a TRIP row asks twice (START: YES) and resets it; the ODOMETER
  is never reset. They count from the build that has them: earlier rides are not added.
- **Not done:** the CLIMB filter (no climb has a category today, so "medium and above" needs thresholds chosen) and the time zone,
  daylight saving and the date and time formats (the clock takes its offset from the GPS fix; a 12 h or DD/MM format touches
  every time and date text).

## Live segments

Segment files in `\Rustfw\Segments\` (`<id>.seg`, docs/segments.md) are matched against the ride once a second while it
runs (SEGMENTS > LIVE SEGMENTS, ON by default). Put them on the card with `tools/segments.py` (`fetch`, `build`, `write
--volume`, docs/segments.md): the unit writes there only the rider's efforts (below), after a ride is saved.
- **Approach**: a banner `SEGMENT <name> 300 M`, once. **Lost** (off the track 10 s, a U-turn, stopped 5 min): `SEGMENT LOST`.
- **The segment's page** is the CLIMB page (above): one page in the round, it opens by itself at the segment's start, over
  the ride page or the overview the rider is on (only while recording, not over a menu), and Page finds it again when
  the rider has paged away. It leaves the round at the finish (the result card follows) or when lost. The head: the name,
  the delta in big digits (`-0:12` ahead in GREEN, `+0:08` behind in the accent, never red; within 2 s of target it keeps
  the colour it had, so a flip at the line does not redraw it) and the target's label (`VS PR`). The segment's own
  profile with the rider's place (white) and the target's (grey); a segment file without altitudes shows its bar with
  both marks instead. Under the profile, the pacer's gauge: the power to ride for the ideal pacing over the segment,
  against its target band, with the time to the top in its title. Then the elapsed time on the segment and the distance
  left. Keys act as on any ride page. Only the changed characters and marks are redrawn, once a second.
- **The finish**: a banner with the time and the delta, then the result card (the time, the delta against each target
  the file keeps, NEW PR when the PR is beaten or there was none: the same rule as the `.pr` written), closed by any key or after 10 s.
- **Efforts:** at a finish the effort (its time at each of the segment's points, as ridden) is kept in RAM, at most 4
  a ride. Only after the ride is saved (read back whole, in `\Activities`) and never for a ride discarded, each is
  written between rides: `\Rustfw\Segments\<id>.pr` when it beats the PR (two copies with a CRC, as settings.bin, so
  a cut leaves the old PR or the new one), and a line `id,date,elapsed_s` in `\Rustfw\Segments\efforts.csv` for the
  computer. A write that fails (a card full) drops the effort, never the ride.
- **Your PR is the target:** the next time the segment is loaded, its `.pr` times are the PR column (in place of the
  source's, or where it had none); a `.pr` for other points (the segment rebuilt) is ignored.
- **TARGET**: AUTO is the PR if the file has one, else the KOM; a target the file lacks falls back to AUTO.
- LIVE SEGMENTS and TARGET are in settings.bin at data 97 and 98; an older file reads ON and AUTO.

![CLIMB page on a segment, ahead](img/segment-ahead.png) ![CLIMB page on a segment, behind](img/segment-behind.png) ![Result card](img/segment-card.png) ![SEGMENTS menu](img/segment-menu.png)

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
- **Power meter calibration** (SENSORS > CALIBRATE POWER, offered while the power meter is paired and heard): the screen says UNCLIP, CRANKS VERTICAL, NO PRESSURE; Start (or the CALIBRATE button) sends
  the ANT+ Bicycle Power calibration request (page 0x01, id 0xAA, as an acknowledged message, sent again if the air
  loses it) and waits 10 s for the meter: ZERO OFFSET with the number (and WAS, the offset before it this session),
  FAILED, or NO ANSWER. Once the meter has said it supports auto zero, an AUTO ZERO ON / OFF button (Page or Lap
  too) sends the configuration and shows the state the meter confirms. Not while a ride runs (a notice; paused is
  fine). Layouts and sources: src/drivers/powercal.rs.
- **Electronic shifting:** **DI2** (Shimano's own ANT channel, as stock and the patched
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

## Settings

The settings live in `\Rustfw\settings.bin` (`src/app/settings_file.rs`): 1024 bytes, two copies of one 512-byte sector.
Each copy holds "RSET", a version (1), the length of the settings in it, a sequence number and a CRC-32 of the sector.
The newest whole copy (the higher sequence number) wins, and a write goes over the other copy, so a write cut at any
point leaves the newest copy before it whole. The file is read when `\Rustfw\pages.bin` is read, a few seconds after
the start (never at boot), and written between rides only. Its settings: the wheel size, the numbers (max HR, FTP,
weight, bike weight, W', eat, drink, heart rate alert), then the choices, one byte each, then the 11 alert limits
(`Settings::limits`, u16 each, after POWER MATCH). A value out of its range reads as its default, and a file that ends
before the limits (an earlier build's) reads them OFF. A later build appends its settings and raises the length: SOUND,
UNITS, TEMPERATURE and AUTO PAUSE are bytes 99 to 102 of the data, then the trips and the odometer as five u32 (TRIP 1 m
and s, TRIP 2 m and s, the odometer m) at 103 to 122, 123 bytes in all; a file that ends before them reads ON, METRIC,
C, AUTO and zeros. START REMINDER and READY CHECK are the last two bytes (150, 151; 152 in all) and read ON when missing.

**Which copy wins at a start** (`Settings::with_registers` in `src/app/settings.rs`):
- the registers held none of this firmware's settings (a flat backup battery): the file's settings, and the registers
  are written from them;
- else, register by register: a register that differs from the word the newest copy was written with brings only the
  fields it carries, and the file is written again with them. Every other field is the file's;
- no file, or no whole copy: the registers' settings are written as its first copy, once.

A read that fails for any reason but "no file" is tried again, never taken for no file.

**Backup registers still written.** Registers 11, 16 and 28, and AUTO OFF's bits of register 18, are written at every
change, because the first screen needs them before the card is read (backlight level and time-out, font, AUTO OFF).
A change made during a ride, with the USB drive lent, or by a build that may not write the card (it reads the file and
never writes it) waits in the registers, and survives a reset or a switch-off there; the registers win over the file
where they differ from what it was written with. A setting added from now on goes in the file only.

The register layout, for the words still written:

- **11:** wheel, backlight, max HR, FTP, touch, ride profile, climbs ahead; tag 0xa7 (AUTO), 0xa8 (ROAD), 0xa9 (INDOOR),
  0xaa to 0xac the same with CLIMBS AHEAD off. MTB reads as AUTO here; it is kept in register 16 bit 27.
- **16, tag 0xa:** the rider's weight less 30 kg in bits 0..6, W' in 0.5 kJ in 7..13, the gauge's window in 14..15, the
  bike's weight in 100 g in 16..23 (0: the 9.0 kg default), the smooth font in bit 24, the MTB profile in bit 27. Read from earlier builds: tag
  0xb (bike in kg, the single presses' actions), tag 0xc1 (weight in kg, W', window, actions; no bike weight, read as
  the default).
- **28, tag 0x9:** the eat and drink reminders in 5 min steps (bits 0..4 and 8..12), the long presses' actions (5..7,
  13..15), the heart rate alert in 5 bpm steps (16..21), the single presses' actions (22..24, 25..27). Read from earlier
  builds: tag 0xc2 (the heart rate in bpm, the single presses in register 16).
- **Others:** 5..8 and 10 the paired ANT+ sensors, 14 the shifting system, 17 and 18 the GNSS aiding, 20..27 the ride
  pages and the overview (0xd / 0xe), 29 the fault marker, 30 a ride open on the card.
- **18, tags 0xa or 0x5:** AUTO OFF in bits 22..24 (an index into the AUTO OFF choices, 0 off), kept through a restart.

## Phone (BLE, no app)

The unit pairs with a phone as a Bluetooth accessory, with nothing to install on the phone:
- **Pairing:** SENSORS > PHONE > PAIR advertises for a minute ("S510"; the phone sees a HID accessory, appearance generic HID);
  pair from the phone's Bluetooth settings (Just Works: no code). One phone is kept (its key and the notifications
  it enabled), in `\Rustfw\ble.bin` on the card, read 3 s after the start; FORGET erases it. A build that may not
  write the card keeps it until switched off. Not paired, the unit does not advertise at all; paired and not
  connected, it advertises once a second so the phone comes back by itself.
- **Media keys:** on an iPhone, through Apple's media service (AMS): play / pause, next, previous, volume; otherwise
  as a BLE keyboard's media keys (HID consumer control, also mute). Hold Lap on any page: play / pause. SENSORS > PHONE >
  REMOTE is the full player (the quick panel has its main buttons, one swipe down from any page): the title on two lines, the artist, the album, a progress bar with the elapsed and remaining
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
sector reads, and running a new build from RAM (`tools/dev.py`, see docs/devloop.md). Unplugging resets the unit.

**USB drive:** the computer can mount the eMMC's FAT volume (BRYTON: rides, routes, workouts, map tiles) without
going back to stock. The unit is a card reader with the card out until SETTINGS > SYSTEM > USB DRIVE (or `tools/dev.py drive`) lends
it; refused with a banner while a ride runs (END THE RIDE FIRST), waits to be saved (RIDE NOT SAVED YET) or without
the dev link up (CONNECT USB). Lent, nothing else touches the card: no ride can start (USB DRIVE ON), the scans, the
basemap and the dev link's card commands wait. Ejecting on the computer gives it back; so does USB DRIVE again on
the unit, once the computer has let it go (else EJECT ON COMPUTER). After the computer wrote, the unit restarts so
that nothing it read before is trusted. Only the FAT volume is offered (sectors 0 .. B-1), never stock's reserved
area. A build without EMMC_WRITE=1 offers it write protected.
