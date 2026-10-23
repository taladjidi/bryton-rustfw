# Features

## Screens

- **Status strip** (every screen): BLE (green, red if the radio chip did not answer), GPS (blinking grey while
  searching, then 1 to 4 signal bars), the time (green while a ride records, orange while paused), a USB symbol,
  the battery (green while charging, red at 15 % and under).
- **Overview** (the main screen): the time, big (no seconds: it changes once a minute), the date, four fields chosen
  in the menu (OVERVIEW; by default distance, timer, heart rate, temperature), and the latest notices (a GPS fix, a
  sensor paired, the battery low), or Ferris the Rust crab when there are none.
- **Ride pages:** up to three, each in one of eleven layouts (one to six fields: even grids, and uneven ones such as
  a big box over two or over a 2 x 2 grid, two over a big one, three rows), chosen in the menu: speed, heart rate, cadence,
  power, time, map, timer, distance, average and top speed, average heart rate and power, lap time and distance,
  ascent, a zone graph; heart rate variability from a strap's beats: DFA a1 (the aerobic threshold marker, about
  0.75 at the threshold, from the last 2 minutes once 64 clean beats are in) and RMSSD (ms, the last minute); and
  from the power meter and the ride: 3, 10 and 30 s power, NP, IF, TSS, kJ, kcal, W/kg, W' balance (kJ, green,
  yellow, orange, red as it drains), max power, max heart rate, average cadence, left / right pedal balance, VAM and
  grade. Page 3 is the map by default.
- **Map:** the track since power-on, north up, fitted to the whole track, an arrow for position and heading, a
  scale bar. Before a fix, a grey dot at the last known position. No basemap yet.
- Everything is in the project's own dot-matrix font. Only what changes is redrawn, in the smallest box.

## Keys

| Key | Main screen and ride pages | Menus |
|---|---|---|
| Page | next page (main, ride pages, main) | down |
| hold Page | the menu | |
| Lap | a lap (while riding) | up |
| Start | start, pause, resume the ride | select, or change the value |
| hold Start | end a paused ride | |
| Back | backlight on / off | back |
| hold Back 3 s | power off (the radio and GPS too) | power off |

The power button starts it again. After the backlight's time-out, any key brings it back.

## Touch

| Gesture | Main screen and ride pages | Menus | A number being edited |
|---|---|---|---|
| swipe left | next page (as Page) | | |
| swipe right | previous page | back (as Back) | keep it |
| swipe up / down | | the next / previous screen of rows | one step up / down |
| hold (600 ms, still) | the menu (as holding Page) | | |
| tap | | selects the row touched and acts on it (as Start) | keep it |

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
- Not saved yet (the FIT writer, `src/storage/fit.rs`, plugs in at `App::recorder`).

## Menu

Sensors (pair, forget; then the light, temperature and pressure readings), the overview's four fields, pages 1 to 3 (field count, each field),
wheel size, backlight level (or AUTO) and time-out, MAX HR (1 bpm steps) and FTP (5 W steps: Start, then Lap
up and Page down, held to repeat, Start to keep), weight (1 kg) and W' (0.5 kJ), GPS (fix, satellites, position, altitude, UTC),
BLE devices nearby.

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
- **Beeps** at power-on and power-off.

## USB

With a cable in, a dev link starts: screenshots, key presses, register and memory reads, the battery log, eMMC
sector reads, and running a new build from RAM (`dev.py`, see AGENTS.md). Unplugging resets the unit.
