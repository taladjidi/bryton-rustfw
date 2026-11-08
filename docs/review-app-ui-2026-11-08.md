# UI review of the iPhone app (main 7697ba28), 2026-11-08

Method and coverage. Built for the Simulator and photographed with the demo arguments (`-demo`, `-demo-dl`,
`-demo-scroll graphs|laps`, `-demo-workout`, `-demo-edit`, `-demo-saved`, `-demo-send`, `-demo-send-go`); light, dark,
extra-extra-large and accessibility-extra-large. Images are in
`/private/tmp/claude-501/-Users-tangui-Documents-bryton-rustfw/7aa33efe-0b94-4a7e-b407-10226ab3eeb9/scratchpad/ui/`
(`home`, `nodemo`, `dl2`, `graphs`, `laps`, `workout`, `edit`, `d-*` dark, `x-*` XXL, `ax-*` and `se-*`).
The 18 Pro took the light, dark and XXL shots; the iPhone 17e (no SE or 16e exists here) took `ax-*` and `se-*`.

Seen: home (no demo, demo, dark, XXL, AX), download in progress, ride detail (summary, map, five graphs, laps, zones,
curve; light, dark, XXL, AX), workout detail, workout editor (copy of a workout, `-demo-saved`).
NOT seen (no hook, or the hook did not open it; judged from code only): the profile sheet, the weekly chart's tap
selection, the workout delete confirmation and swipe actions, the import (`fileImporter`), the step editor, the
"On the phone" section (below the fold, no scroll tool), the empty-state of a connected unit, the real BLE states
(searching, not found, failed), iPad and landscape (the project is iPhone only, `TARGETED_DEVICE_FAMILY = 1`).
`-demo-send` and `-demo-send-go` showed only the editor in 5 s (`sendgo.png` = `edit.png`): the sheet was not seen.
Comparison references: Garmin Connect, Wahoo ELEMNT, Bryton Active, Intervals.icu (from the survey and general
knowledge; not re-fetched).

## 1. Per screen

### 1.1 Home (ContentView.swift, RideLibrary.swift WeeklyPanel)

- **H1 (P1) status card is noise and leaks a debug line.** ContentView.swift:32-44 puts a grey prose card between
  the weekly panel and the rides: "Demo mode: a fake unit with made-up rides." plus `rides.debug`
  ("Demo: fake unit, write length 20 B", monospaced) which is not under `#if DEBUG` (RideBrowser.swift:121, 550). A rider
  sees an MTU line. Connected, the card shows Name / Maker / Battery rows; "Maker: rustfw" means nothing. What a rider
  checks first (Garmin, Wahoo, Bryton): is the unit connected, battery, last sync. Fix: one header row (bike computer
  glyph, unit name, state chip Connected / Searching / Not found, battery with a gauge symbol, "Synced 14:02"), above
  the weekly panel; keep `debug` behind `#if DEBUG` or in a Details disclosure; drop "Maker".
- **H2 (P1) rides are greyed while syncing.** `.disabled(locked)` (ContentView.swift:192) dims the whole row, so the
  top row in `home.png` looks dead while the sync works on the second. A disabled row has under 3:1 contrast and no
  reason shown. Fix: keep the row enabled and enter the viewer (saved rides open at once); for a row that cannot start a
  download show a spinner or "Waiting for the sync" in the subtitle.
- **H3 (P1) orange text for errors.** ContentView.swift:47 (`Text(m).foregroundStyle(.orange)`), also
  WorkoutEditor.swift:111, 135, 208, 236, 278, 440 and WorkoutViews.swift:105. systemOrange on white is 2.2:1 (fails AA 4.5),
  and colour is the only cue. Fix: `Label(text, systemImage: "exclamationmark.triangle.fill")` in the `warn` token
  (section 3), real failures in `alarm`.
- **H4 (P2) the large title says "Bike computer".** ContentView.swift:80. The app is Bryton, the unit is an S510; a
  title that is the unit's name (or "Rides") orients; the title eats 100 pt before any data. Fix: `.navigationTitle(unitName ?? "Rides")`,
  `.navigationBarTitleDisplayMode(.inline)` when the weekly panel is shown.
- **H5 (P2) empty states.** `nodemo.png`: with no ride the weekly chart is 120 pt of blank with only axis labels
  (RideLibrary.swift:173-183), "No rides listed" (ContentView.swift:58) says neither why nor what to do, "Searching for the
  unit..." has no spinner, the not-found text has no retry button (only pull to refresh), first launch has no explanation of
  pairing. Fix: empty chart replaced by "No rides in the last 8 weeks"; a ContentUnavailableView-style block per link state
  with a Retry button and the pairing steps (Garmin and Wahoo both walk a first pairing).
- **H6 (P2) saved rides while the unit is away.** The list is `rides.rides`, the unit's LIST (ContentView.swift:53-58). From
  the code (not seen), a ride on the phone is unreachable offline except through Files. Fix: show `library.briefs` rides
  too, marked "on phone", so the list works without the unit.
- **H7 (P2) weekly stats row.** RideLibrary.swift:159-167: four `stat` columns separated by `Spacer()`. At XXL the
  unit names ("distance") start to wrap; at AX they break mid-word (`ax-home.png`: "dis-tance", "2 h / 28"). Fix:
  `ViewThatFits { HStack{...} ; LazyVGrid(2 columns) }` and a `@ScaledMetric` chart height.
- **H8 (P2) weekly chart.** x labels every second week only and overlap at AX (`ax-home.png`, x labels collide); the
  y axis is trailing here and leading in the workout chart (WorkoutViews.swift:171); selection works by drag only,
  with no hint (survey: Intervals.icu shows the value on tap); in dark mode the unselected bars are accent at 0.4 opacity
  on #1C1C1E, about 1.7:1 (`d-home.png`), under the 3:1 for graphics. Fix: unselected = accent at 0.6 plus a 1 pt
  outline, or a separate `accentMuted` token with 3:1; show "Week of 2 Nov" selection on tap.
- **H9 (P2) wording of load.** The row says "HR load 80" (ContentView.swift:206), the weekly panel "TSS*" with a 11 pt
  footnote (RideLibrary.swift:155, 190). One name: "Load" with a "(HR)" suffix, one footnote style.
- **H10 (P3) rows have no icon or thumbnail.** Garmin Connect and Wahoo give each ride a sport glyph, the title and
  headline numbers (distance big, time, avg) and a small route thumbnail. Ours is two text lines. Fix later: an SF
  Symbol `bicycle` and the distance in the numeric face (section 3).
- **H11 (P3) "FTP not set" button** (RideLibrary.swift:196) is a blue caption, under 44 pt. Make it a full-width row with a
  chevron.
- **H12 (P3) the download row's Cancel** (ContentView.swift:205) is a `.borderless` caption button, about 20 pt tall inside a
  row whose tap is also a button. Use a `Label("Cancel", systemImage: "xmark.circle")` with `.contentShape`
  of 44 pt, or the row's swipe action.

### 1.2 Ride detail (RideDetail.swift, RideMap.swift)

- **R1 (P1) the altitude graph is wrong.** `graphs.png`: axis 0 to 2000 m for a 72 m climb, the line is flat. RideDetail.swift:246
  `AreaMark(x:y:)` has a zero baseline, which overrides `.chartYScale(domain: .automatic(includesZero: false))` (line 252).
  Fix: `AreaMark(x:, yStart: .value("lo", lo), yEnd: .value("y", p.y))` with `lo` the series minimum, and give the scale the
  explicit domain `lo...hi`, padded by 10 %.
- **R2 (P1) Dynamic Type breaks the viewer.** `ax-dl2.png`, `ax-laps.png`: the summary tiles hyphenate their labels
  ("Dis-tance", "Elapse / d") and clip values ("12.0..."), the lap row wraps digit by digit ("0:2 / 5:0 / 0"), the zone chart's
  "Z1..Z6" labels print on top of each other, the title ends "7:...". Causes: `.adaptive(minimum: 100)` fixed (RideDetail.swift:293),
  `lineLimit(1)` + `minimumScaleFactor(0.7)` (297), the lap `HStack` of five Texts (314-327), fixed `frame(height: n * 24 + 30)`
  (348), fixed chart heights 110 / 140 (263, 366). Fix: columns `minimum: 100 * scale` with `@ScaledMetric`; at
  `dynamicTypeSize.isAccessibilitySize` the lap row becomes a two-line card; heights `@ScaledMetric`; zone chart height
  from the scaled font.
- **R3 (P1) zone bars say nothing.** `laps.png`: hairline bars (about 6 pt in a 24 pt row), no time, no percent, no zone
  boundaries in watts or bpm. Garmin and Intervals.icu print "Z2 1:12:03 41 %" and the zone range. RideDetail.swift:342
  uses one hue with an opacity ramp, so power Z3 is pale orange while the workout page draws Z3 green. Fix: a custom
  horizontal bar (a `GeometryReader` or `BarMark` with `.height(.ratio(0.7))`) with the zone token colour, then the
  `h:mm` and the percent at the end of the bar, and the range ("152 to 205 W") under the label; the same palette for HR
  and power (firmware does: `hr_color` = first five of `power_color`).
- **R4 (P2) the viewer opens on 16 equal tiles.** Rank what a rider looks for first (Garmin: map + distance, time,
  avg speed; Wahoo: map + stats; Intervals: load): put the map and the headline trio (distance, time, TSS or avg speed)
  first and big, then the rest under "Details". Today the map is below a 5 row grid (`dl2.png`).
- **R5 (P2) the date twice, in two formats.** Title "8 Nov 2026 at 7:00" (RideDetail.swift:185), subtitle "Sunday 8 November
  2026 at 7:00" (291), the list says "Sun 8 Nov at 07:00" (Fmt.start, forced 24 h by the `HH` template, RideLibrary.swift:113),
  and a ride not yet downloaded "6 Nov 2026 at 17:00" (RideBrowser.swift:531). Fix: one `Fmt.when(date)`; title = the
  weekday and day ("Sun 8 Nov"), the subtitle only the time of day, or drop the subtitle.
- **R6 (P2) graph selection readout.** MetricChart shows only the y value as a bare number
  (RideDetail.swift:240, `%.0f`), no unit, no x. Fix: "12.4 km · 187 W" in the series colour, the same readout on all
  five charts at once (Garmin's linked charts), and a marker dot on the line.
- **R7 (P2) series colours are system colours**, HR in `.red` (RideDetail.swift:102), which the firmware reserves for
  alarms (theme check `RedNotAlarm`). Use the tokens of section 3.
- **R8 (P2) two profile icons.** Detail uses `person.crop.circle` (RideDetail.swift:191) for the sheet the home opens with
  `gearshape` (ContentView.swift:91, and WorkoutViews.swift:109). Pick `person.crop.circle` ("Profile": it holds FTP,
  max HR, the Intervals key) everywhere, with an accessibility label.
- **R9 (P2) no VoiceOver.** No `accessibilityLabel` anywhere in the app (grep): the gear, person and share buttons read
  "gearshape" style names or nothing; `RideMap` is an unlabelled `MKMapView`; the charts rely on Swift Charts' defaults,
  which read "x, y" without units. Fix: labels "Profile", "Share ride", `.accessibilityLabel("Route map, 12.0 km")` on the
  map, `.accessibilityChartDescriptor` or at least `.accessibilityLabel/Value` on each chart with min, max, average.
- **R10 (P2) map.** Fixed 280 pt, no full screen (Garmin and Wahoo open the map full screen on tap), two attributions at the
  corners (Apple's "Legal" bottom left, "(c) IGN" a plain Text over `.thinMaterial`, RideDetail.swift:151, not a link),
  and in dark mode the IGN tiles stay white (`d-*` has no map shot; from the code nothing dims them). Fix: tap opens a
  full screen sheet; make the credit a `Link` to the Plan IGN page; optionally darken with a `.colorMultiply` overlay in dark.
- **R11 (P3) speed, power and cadence drop to zero at three equal intervals** in `graphs.png`. Probably the demo ride's
  stops; if real, a stopped second is plotted as a line to 0 (ugly for power and cadence). Check with a real ride; for
  power and cadence use gaps (`nil`) when the speed is 0.
- **R12 (P3)** the x-axis label ("km", "min") sits detached under the chart's left (`chartXAxisLabel`), and the unit is in
  the title "(m)". Put the unit in the readout and drop the axis labels, or `position: .trailing`.
- **R13 (P3)** the Distance / Time picker scrolls away with the content; Garmin keeps the axis choice per chart. Fine for
  five charts; pin it only if the graphs grow.

### 1.3 Workouts (WorkoutViews.swift, WorkoutEditor.swift)

- **W1 (P1) decimal separators disagree on one screen set.** `workout.png` "IF 0,77" (Text interpolation with
  `specifier:` follows the locale, WorkoutViews.swift:103, WorkoutEditor.swift:205) against `dl2.png` "IF 0.72" and "28.8 km/h"
  (`String(format:)` is always a dot, RideDetail.swift:285, 273-277, RideLibrary.swift:129). The owner reads French: a
  rider sees both. Fix: one `Fmt` using `FormatStyle` (`.number.precision(.fractionLength(1))`) or `Measurement`
  everywhere; delete the `String(format:)` calls (list in section 4).
- **W2 (P2) wording that contradicts itself.** WorkoutEditor.swift:139 "Workouts made here wait for the unit's write support to be
  sent." while line 231 offers "Send to the unit" and the demo sends. Fix: delete the sentence if the unit accepts writes
  (the `.readOnly` error text, RideBrowser.swift:506, already covers the old firmware).
- **W3 (P2) section names.** "Workouts" (on the unit) and "On the phone" (ContentView, WorkoutSection) are not parallel; say
  "On the unit" / "On the phone", or "Workouts on the unit".
- **W4 (P2) step colours differ from the unit.** zoneColors (WorkoutViews.swift:18) are SwiftUI gray / blue / green / yellow /
  orange / red / purple. The unit's workout graph (docs/img/workout-graph.png) and its zone strip use teal, green, olive,
  brown, wine, purple, slate. Same workout, two colourings. Fix: the zone tokens.
- **W5 (P2) the chart has no step labels or zone legend**, and a grey bar means "not a power step" (rest steps drawn at
  FTP / 4) with no key. Add a legend row of zone chips (Z1..Z7 with the watt range), as the firmware's climb page does with
  its strip.
- **W6 (P2) Delete is swipe only** (WorkoutViews.swift:63, WorkoutEditor.swift:105), unreadable to a rider who does not
  know the gesture (Mail-style). VoiceOver exposes swipe actions, sighted riders may not find them; add a context menu
  and an Edit mode.
- **W7 (P2) the workout detail has four sections of buttons and footnotes** before the steps (profile link, "Edit a copy",
  two long footnotes, `workout.png`). Fold the profile line into the chart card caption, put "Edit a copy" in the toolbar.
- **W8 (P3)** the editor's title "New workout" for a copy of a unit workout (`edit.png`); use the workout's name.
- **W9 (P3)** the toolbar in `edit.png` has Cancel, title, Edit and Save: four items; "Edit" (reorder) is unlabelled
  as to purpose. Move it into the Steps section header.
- **W10 (P3)** "Repeat the 2 items above, 3 times in all" (WorkoutEditor.swift:276) reads as code; "Repeat the last 2 steps,
  3 times" says the same.

### 1.4 Profile sheet (RideDetail.swift:9-60) and Intervals.icu (not seen)

- **P-1 (P2)** FTP and max HR are steppers only (lines 26-27): from the default 200 W to 265 W is 13 taps, from 190 to 205 bpm
  15. Use a number-pad `TextField` with the stepper as a secondary control.
- **P-2 (P2)** read-only values (LTHR, weight, W') sit in the same list as editable steppers with the same look; mark them
  "from Intervals.icu" in a separate footer.
- **P-3 (P3)** the API key fields are two raw text fields, the athlete id hint "0 or i12345" is jargon; link to the Intervals
  page that holds the key.
- **P-4 (P3)** the "Done" button is a plain toolbar `Button`; use `ToolbarItem(placement: .confirmationAction)`.

## 2. Cross-cutting consistency (P2 unless noted)

| Topic | Today | Fix |
|---|---|---|
| Accent | system blue (no `AccentColor` asset), the firmware is orange | AccentColor asset, section 3 |
| Zone colours | three schemes: SwiftUI colours (workouts), single hue ramps (zone bars), unit palette | zone tokens |
| Red | HR series, errors in orange, stop red markers | red only for alarm and errors, as the firmware |
| Units | km, m, km/h, kg hard coded; the unit has imperial (docs/img/imperial-units.png) | `Fmt` reads the same setting (UserDefaults), or `MeasurementFormatter`; P2 |
| Dates | four formats (R5) | one `Fmt.when` |
| Durations | "2 h 28" / "24 min" (RideLibrary.swift:124, WorkoutViews.swift:26, duplicated), "0:24:00" (hms), "10:00" steps | one `Fmt.duration(style)`: short for lists, clock for tiles |
| Ellipsis | "..." typed three dots (ContentView.swift:~140, RideBrowser.swift:116, 368, 443) | the real ellipsis character |
| Capitalisation | "not on the phone yet" lower case row text vs sentence case elsewhere; "READ ONLY" shouted (RideBrowser.swift:354) | sentence case |
| Card style | grouped `List` for home (system radius ~20), 8 pt tiles in detail, 12 pt map, unframed charts | one `Card` modifier, radius 12 (tiles 8) |
| Chart axes | trailing y on metric charts and weekly, leading on the workout chart | trailing everywhere |
| Chart heights | 110, 120, 140, 150, 180, 280 literal | tokens, `@ScaledMetric` |
| Navigation | home pushes ride detail (full-screen push), workouts push a page, profile sheet, editor sheet, share sheet | fine (HIG); keep |
| Tap targets under 44 pt | Cancel (H12), FTP not set (H11), toolbar items are system 44 | see items |
| Contrast | orange errors 2.2:1, dim bars 1.7:1 (dark), disabled rows | tokens below, all checked |

## 3. Design tokens: one look for the app and the unit

### 3.1 What the firmware looks like (read from src/ui and docs/img)

- Black panel, white big numerals, grey UPPERCASE small labels, one orange accent bar (ACCENT #FFA600, Ferris RUST
  #F74D00 for the mascot), green for ok, yellow for warn, red only for alarms (the theme check `RedNotAlarm` refuses a theme
  where red is also a zone, the accent, ok, warn, Ferris or the course).
- Zones: seven colours, Intervals.icu's palette; the DARK theme draws them x1.5 so 8 px bars read on black:
  `ZONE_COLORS` 0x4c90 0x4c86 0xddc9 0xd408 0xb98b 0x6237 0x5a8d = #4A9284, #4A9231, #DEBA4A, #D68242, #BD315A,
  #6345BD, #5A516B (teal, green, olive, brown, wine, purple, slate). HR uses the first five (`hr_color`).
- Climb grades (`grade_color`): under 6 % zone 2 (green), under 9 % zone 3 (olive), under 12 % zone 4 (brown), above
  zone 5 (wine). Form (`form_color`): GREEN fresh, GREY neutral, YELLOW tired, RED overload.
- Themes: DARK, OUTDOOR (zones and greys brighter: #5EC4B2 #5EC446 #FFFA62 #FFAE54 #F84076 #885AF8 #786E90), LIGHT (black on white,
  every colour darkened to 4.5:1 text or 3:1 lines on white: zones #2F6259 #2F6223 #947D31 #8F572A #7C203B #442D7C #3C3748, accent
  #B04000, alarm #D00000, ok #007000, warn #8A6400).
- SMOOTH face = Barlow Semi Condensed Bold (tools/fonts/BarlowSemiCondensed-Bold.ttf, 111 772 bytes, SIL OFL 1.1,
  PostScript name `BarlowSemiCondensed-Bold`).
- App icon: Ferris on a teal gradient (teal is also zone 1).

The firmware's LIGHT theme already is an iOS light palette (it was made from Intervals.icu's values for white pages);
the app's dark mode can take DARK with three zones lifted (below).

### 3.2 Colour assets (Assets.xcassets, each a Color Set with Any and Dark appearances)

Contrast = WCAG 2 ratio, computed here (script in this session), against the iOS surfaces: light card white (#FFFFFF) and
grouped background #F2F2F7; dark card #1C1C1E and background #000000.

| Token (asset) | Light | Dark | Contrast light / dark | From the firmware |
|---|---|---|---|---|
| `accent` (AccentColor) | #B04000 | #FFA600 | 5.9 on white, 5.3 on F2F2F7 / 10.7 on black, 8.7 on 1C1C1E | LIGHT ink[4], ACCENT |
| `zone1` | #2F6259 | #4A9284 | 7.0 / 4.6 | ZONE_COLORS[0] (LIGHT, DARK) |
| `zone2` | #2F6223 | #4A9231 | 7.3 / 4.4 | [1] |
| `zone3` | #947D31 | #DEBA4A | 4.0 / 9.1 | [2] |
| `zone4` | #8F572A | #D68242 | 5.9 / 5.8 | [3] |
| `zone5` | #7C203B | #E0486F | 9.9 / 4.3 | [4] (dark lifted from #BD315A, 3.0) |
| `zone6` | #442D7C | #885AF8 | 11.0 / 3.9 | [5] (dark from OUTDOOR; DARK's #6345BD is 3.1 on black, about 2.2 on 1C1C1E) |
| `zone7` | #3C3748 | #786E90 | 11.5 / 3.6 | [6] (dark from OUTDOOR; DARK's #5A516B is 2.8 on black) |
| `ok` | #007000 | #30D158 | 6.3 / 8.4 | LIGHT ink[8]; GREEN is too neon on a phone, so the system green |
| `warn` | #8A6400 | #FFD60A | 5.4 / 12.1 | LIGHT ink[9], YELLOW |
| `alarm` | #D00000 | #FF453A | 5.7 / 5.0 | LIGHT ink[6], RED |

Derived (no new assets): `grade(x10)` = zone2, zone3, zone4, zone5 by the firmware's four bands; `form(tsb)` = ok,
`.secondary`, warn, alarm; `hrZone(z)` = zone z for 1 to 5. Zone 3 light (4.0) is below 4.5: use it only as a fill or
a bar (3:1 rule for graphics), never as text. Text and surfaces stay the system's (`.primary`, `.secondary`,
`systemBackground`), as the HIG asks; the firmware's own greys (#848284, #606060) are close to them, so nothing to add.

Notes for the firmware (found while checking): its DARK zone6 and zone7 are 3.1 and 2.8 on black, under the
3:1 its own LIGHT rule asks for; OUTDOOR's two values would fix DARK too. Theme tests pass because the theme check
looks at text and alarm only (inferred from the `Refused` list).

### 3.3 Where each token is used (and what then matches the unit)

| Token | App places | Unit counterpart |
|---|---|---|
| `accent` | buttons, links, segmented tint, selected weekly bar, progress bar, toolbar glyphs, power series line | menu orange bar, selected row, top bar |
| `zone1..7` | `ZoneBars` (power 7, HR 5), `StepChart` bars and the step dots (`zoneColor`), a zone strip in the viewer's header | the 7-colour strip in climb-page.png, the field zone bars (zone-bar.png), workout-graph.png bars, power-curve and gauge-zones |
| `hrZone` | HR chart line = zone5 token, HR zone bars | HR field's zone bar, hr_color |
| `grade` | altitude chart coloured by climb grade (like ClimbPro), a future climb list | climb-page.png profile and its grade percent, climbs-done.png, course-profile.png |
| `form` | a future fitness/form line (CTL/ATL/TSB) | fitness.png, form_color |
| `ok` | "Sent" check (WorkoutEditor.swift:439), synced state | ok / BLE green |
| `warn` | all current orange texts (H3), "reaches past the start", cut workout | yellow notices |
| `alarm` | failures only, discard buttons | red banners, LOW battery |
| series (derived) | speed = zone1, HR = zone5, power = accent, cadence = zone2, altitude = secondary fill | none in the unit; chosen so no series is the alarm red |

Screens that would then match one for one: the zone bars (app `ZoneBars` and unit field bar), the workout step colours
(app workout page and unit workout page), the grade colours on climbs (unit climb page and a future app climb list), the
power curve (accent bars) and the weekly panel's numerals (unit overview's big numbers, below).

Swift shape (no per-view hex, all in one file `Theme.swift`):

```swift
enum Tok {
    static func zone(_ z: Int) -> Color { Color("zone\(min(max(z, 1), 7))") }   // asset names zone1...zone7
    static func grade(_ x10: Int) -> Color { zone(x10 < 60 ? 2 : x10 < 90 ? 3 : x10 < 120 ? 4 : 5) }
    static let warn = Color("warn"), alarm = Color("alarm"), ok = Color("ok")
    static let radiusCard: CGFloat = 12, radiusTile: CGFloat = 8
}
```

### 3.4 Typography: Barlow Semi Condensed for numbers, SF for everything else

- **Pixel font: no.** It is a 1 bit font drawn at 2x and 4x for a 240 x 320 panel; on a 460 ppi Retina screen at any Dynamic
  Type size it would be blurred or mis-sized, has no accents beyond the panel's set, and fails accessibility scaling.
  The owner's guess is right.
- **Barlow Semi Condensed Bold: yes, for numerals only** (big values in the weekly panel, the summary tiles, the selection
  readout, the workout target, ride rows' distance), and optionally uppercase small labels. Reasons: it is the unit's SMOOTH
  face, so "177 W" looks the same on both; it is about 20 % narrower than SF, which cures the clipped tiles ("12.0...") of
  R2; SIL OFL allows embedding. Headlines, nav titles, body, footers, buttons stay SF (Dynamic Type, legibility, the
  HIG, other languages: Barlow is Latin only).
- Facts checked in the file: default digit widths differ (the "1" is 319 units, the "0" 511), so live values jitter
  unless the `tnum` feature is on; the font has `tnum` (GSUB features: aalt c2sc ccmp dnom frac liga locl numr ordn pnum
  smcp sups tnum). `.monospacedDigit()` does not apply to custom fonts: set it with a `UIFontDescriptor`
  `.featureSettings` (`kNumberSpacingType` = `kMonospacedNumbersSelector`).
- Steps:
  1. Copy `tools/fonts/BarlowSemiCondensed-Bold.ttf` to `app/ios/App/Fonts/` and `tools/fonts/OFL.txt` beside it
     (the copyright line "Copyright 2017 The Barlow Project Authors" must travel with it).
  2. Add both to the target's Copy Bundle Resources; add `UIAppFonts` = [`Fonts/BarlowSemiCondensed-Bold.ttf`] to
     `app/ios/Info-extra.plist`.
  3. `Font.custom("BarlowSemiCondensed-Bold", size: 28, relativeTo: .title2)` (relativeTo keeps Dynamic Type); wrap in one
     `extension Font { static func figure(_ style: Font.TextStyle) -> Font }` with the tnum descriptor.
  4. A "Licences" row (Profile sheet footer) naming Barlow, the OFL, and the repo's GPLv3; the OFL permits bundling in GPL
     software as long as the font is not sold on its own and keeps its notice and name rules (no rename needed: we do not
     modify it).
  5. Cost: +112 KB in the app; add the font to docs/layout.md / docs/app.md and run the docs guard.
  6. Fallback if the font fails to load: `.system(.title2, design: .rounded).monospacedDigit()` (the call returns SF
     silently; test with a launch argument that forces it).
- Optional: the firmware's UPPERCASE grey labels ("WEEK KM") as `Text(label).textCase(.uppercase).tracking(0.5)` on the
  tile captions: same character, no extra assets. Keep them short for VoiceOver (labels are read as typed).

## 4. Code-level issues

- **Formatting helpers: one `Fmt`.** Today:
  - duration: `Fmt.duration` (RideLibrary.swift:124) and `duration(_:)` (WorkoutViews.swift:26) are the same function;
    `SummaryGrid.hms` (RideDetail.swift:307); step clock `String(format: "%d:%02d")` in WorkoutViews.swift:200 and
    WorkoutEditor.swift:296.
  - distance and speed: `String(format: "%.1f km")` in RideLibrary.swift:129, RideDetail.swift:273, 322 (`%.2f`),
    WorkoutViews.swift:201, WorkoutEditor.swift:297, 347; `km/h` in RideDetail.swift:276, 277, 323; `%.0f` in 241.
  - IF / TSS line: WorkoutViews.swift:103 and WorkoutEditor.swift:205 (same text).
  - dates: Fmt.start / Fmt.week, RideBrowser.title, RideDetail's `.formatted`, WorkoutViews.day.
  Make `Fmt` the only place, locale-aware, reading a units setting; remove the `private func day` / `duration` globals.
- **`@AppStorage("ftp")` / `("maxHR")` declared in six views** (RideRow, WeeklyPanel, RideDetail, WorkoutSection,
  WorkoutDetail, ProfileSheet, WorkoutEditor); a typo in a key is silent, the default 200 / 190 repeats. One `Profile`
  `@Observable` or a `static let` of keys and defaults.
- **Hard-coded colours** (`.orange`, `.red`, `.gray`, `.blue`, `.green`, `.systemOrange`, `.systemGreen`, `.systemRed`, `.systemBlue`):
  ContentView.swift:47; RideDetail.swift:102-105, 345, 363; RideLibrary.swift:175; RideMap.swift:41, 51-53;
  WorkoutViews.swift:18, 105, 155; WorkoutEditor.swift:111, 135, 208, 219, 236, 278, 439, 440. All to tokens.
- **Hard-coded sizes**: chart heights 110, 120, 140, 150, 180, 280; tile radius 8, map radius 12; padding 8, 20; `width: 24`,
  `width: 28`, `width: 10` dots. Tokens and `@ScaledMetric`.
- **`debug` string published to the UI** in release (RideBrowser.swift:37, ContentView.swift:42).
- **Launch-argument hooks sprinkled through views** (ContentView.swift:94-130, RideDetail.swift:174-182, WorkoutEditor.swift:241, 449,
  IntervalsClient.swift:53, 114, 153): fine under `#if DEBUG`, but IntervalsClient's three checks are not (grep shows no guard
  at those lines in this review; verify): a release build must not read `-demo`.
- **Accessibility**: no `accessibilityLabel` or `accessibilityValue` in the module; add to icon buttons, the map, charts,
  the progress bar (`ProgressView(value:)` reads a percent, fine) and the weekly selection.
- **Equatable rows** (`RideRow ==` skips `rides`, `ftp`, `maxHR` changes): the subtitle depends on `@AppStorage` read inside
  the row, so the row is redrawn through the property wrapper; fine, noted only because a future token change must not
  be hidden by the `==`.

## 5. Prioritised list

### P1 (confusing or broken): 8
1. R1 altitude graph drawn from zero: axis 0 to 2000 m, flat line (RideDetail.swift:246, 252).
2. R2 Dynamic Type breaks the viewer: tiles, lap row, zone labels, title (RideDetail.swift:293-297, 314-327, 348).
3. R3 zone bars are hairlines with no time, percent or range, and a different palette from the workout page
   (RideDetail.swift:339-350).
4. W1 decimal separator differs between screens, "IF 0,77" against "IF 0.72" (WorkoutViews.swift:103, WorkoutEditor.swift:205 vs RideDetail.swift:285).
5. H1 the status card: a prose card plus a raw MTU/debug line in release; the unit's state and battery are not first
   (ContentView.swift:32-44, RideBrowser.swift:121).
6. H2 rides are greyed out while the sync runs (ContentView.swift:192).
7. H3 errors and warnings in orange, 2.2:1 on white, colour only (ContentView.swift:47 and six more).
8. R9 no VoiceOver labels on icon buttons, map and charts (whole module).

### P2 (inconsistent): 24
H4 title, H5 empty states, H6 saved rides offline, H7 weekly stats at large type, H8 weekly chart (axis side, dim bars in dark,
selection), H9 "load" naming, R4 viewer hierarchy, R5 dates, R6 readout, R7 series colours and red, R8 two profile icons,
R10 map (full screen, attribution link, dark), W2 contradictory editor footer, W3 section names, W4 step colours,
W5 no zone legend, W6 swipe-only delete, W7 detail page clutter, P-1 steppers only, P-2 read-only vs editable values, and
from section 2: accent (blue vs orange), units (no imperial), durations (three styles), ellipsis and capitalisation.

### P3 (polish): 14
H10 row icons and thumbnails, H11 FTP-not-set row, H12 Cancel size, R11 zero dips (verify), R12 axis labels, R13 axis picker,
W8 editor title, W9 toolbar items, W10 repeat wording, P-3 Intervals key help, P-4 Done placement, code: `@AppStorage` x6,
tokens for sizes, `#if DEBUG` on IntervalsClient demo checks (verify).

## 6. Suggested order of work

1. `Theme.swift` with the asset colours, `Fmt` (locale, units), `Font.figure`; replace every literal colour and `String(format:)`.
2. P1 items 1, 3, 5, 6, 7 (small, visible), then 2 and 8 (a pass over layouts with `ViewThatFits` and labels).
3. Re-shoot with the same arguments, light, dark, XXL, AX, and re-run contrast on the final asset values.
