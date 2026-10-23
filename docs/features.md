# Features

## Screens

- **Status strip** (every screen): BLE (green, red if the radio chip did not answer), GPS (blinking grey while
  searching, then 1 to 4 signal bars), the time (green while a ride records, orange while paused), a USB symbol,
  the battery (green while charging, red at 15 % and under).
- **Main screen:** the time, big, with its seconds, and Ferris the Rust crab.
- **Ride pages:** up to three, each of 1, 2, 3, 4 or 6 fields, chosen in the menu: speed, heart rate, cadence,
  power, time, map, timer, distance, average and top speed, average heart rate and power, lap time and distance,
  ascent, and a zone graph. Page 3 is the map by default.
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

## Ride

- Moving time (auto-pause under 2 km/h), distance from the wheel sensor or else GPS, ascent from GPS altitude.
- Zones: heart rate (5, from MAX HR) and power (7, Coggan, from FTP), in stock's palette. The graph shows the last
  minutes of power (or heart rate), one bar per 10 s, each in its zone's colour.
- Not saved yet (the FIT writer, `src/fit.rs`, plugs in at `App::recorder`).

## Menu

Sensors (pair, forget; then the light, temperature and pressure readings), pages 1 to 3 (field count, each field),
wheel size, backlight level (or AUTO) and time-out, MAX HR, FTP, GPS (fix, satellites, position, altitude, UTC),
BLE devices nearby.

## Sensors and radios

- **ANT+:** heart rate, speed, cadence, speed and cadence, power, on stock's channels. A paired sensor is kept
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
