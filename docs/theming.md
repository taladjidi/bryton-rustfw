# Theming: a plan

Status: phase 1 built and sim-tested: T1 (colours, DARK, OUTDOOR, files) and T2 (LIGHT, the black-background
assumptions fixed), 2026-10-29, and T5 (THEME AUTO: day and night by the light), 2026-10-30; T3 fonts (PIXEL, BARLOW; sim only) and T4 metrics are built; layouts are not (first written 2026-10-26). The owner's wish: "the UI layout / look could be defined from a
theme file on the SD card, and the user could pick whatever theme suits them". The "SD card" is the eMMC's FAT
volume (the BRYTON drive); this firmware reads files from it with src/storage/fat/ and keeps its own in `\Rustfw`.

## Summary

- A theme changes **colours first**: background, text, the dim and line greys, the accent, the alarm, the zone
  palette, the course on the map. That is most of the look for little code and no risk to the redraw model.
- Colours are applied as a **colour lookup table in the canvas**: the code keeps drawing with today's named
  constants, and `Panel` (the panel) and `Row` (the screenshot rows) translate each built-in colour to the
  theme's one. The built-in dark theme is the identity table, so every test and screenshot stays as it is.
- A theme file is a small INI in `\Rustfw\Themes\<name>.ini`, read with storage/ini.rs, every value range-checked,
  unknown keys ignored, anything wrong falls back to the built-in values. Picked in SETTINGS > DISPLAY > THEME.
- Three themes ship built in: DARK (today), OUTDOOR (high contrast), LIGHT (black on white, for the sun on this
  transflective panel).
- Fonts and a few bounded metrics come second, layouts last and only as choices among built-in variants: ride
  pages are already the rider's own (the page editor, `pages.bin`).

## 1. What a theme can change

| Level | What | Cost | Risk |
|---|---|---|---|
| a. Colours | background, text, dim text, lines, accent and the text on it, alarm and the text on it, ok / warn, Ferris, the cog, the course, zones 1 to 7 | small: about 20 values, one lookup per `fill` and per glyph | none to the redraw model: a theme change is a full redraw, as a screen opening is |
| b. Fonts and scales | PIXEL or BARLOW (FONT today, T3), the label and value scales | small for the font (the choice exists: `Canvas::face`); scales change what fits | text widths feed `menu_label_w`, `box_scale`, `lines`, the "every row fits" tests |
| c. Metrics | zone bar thickness (`ZONE_BAR_H` 8), frame (`FRAME` 3), menu row (`MENU_ROW` 44), strip height (`STRIP_H` 22), margins | medium: consts become fields read through the canvas and through `App` for touch | hit boxes (44 px targets), `page::min_size` / `fits`, every layout test; a value the tests never ran is a screen nobody saw |
| d. Layouts | the overview's boxes (`DATE`, `TODAY_BOX`, `OVERVIEW_Y`, `NOTICES`), where the card goes, the ride page presets | large if free coordinates: the file would need a layout checker as strict as page.rs (no overlap, every field big enough, every target 44 px) | high: each box is also a dirty box (`App::update_page`) and a touch box |

Levels a and b leave every box where it is, so "redraw only what changes, in the smallest box" (AGENTS.md) is
untouched: the boxes, the `Drawn` values compared each second and the dirty logic stay the same. Only a theme
change itself costs a full redraw, once. Levels c and d move boxes: each value a file can set multiplies the
screens to test.

## 2. The file

**Format: INI**, as stock's `\System\*.ini`. storage/ini.rs already reads it (`get`, `number`, lines up to 512
bytes, any case, LF or CRLF, NUL ends a line), it is fuzzed for the stock import, and a rider can edit it in any
text editor once the drive is mounted. A binary file with a CRC (as `pages.bin`) would be smaller to parse but
needs a tool to write; the INI is parsed once, so its cost does not matter.

```ini
; \Rustfw\Themes\SUNNY.ini
[theme]
version = 1
name = SUNNY
base = light          ; dark, outdoor or light: the values not given here

[colors]
background = #FFFFFF
text = #000000
accent = #C85000
on_accent = #FFFFFF
zone1 = #2F6259
```

stock never sees it). The firmware only reads them. The theme chosen is kept by name (12 bytes) in `\Rustfw\settings.bin`,
appended after the first build's 31 bytes of settings (built, T1), written between rides only as that file is; the backup registers have no room (app/settings.rs).

**Reading.** At the first main loop pass, with `pages.bin` (the card is not touched at boot): `settings.bin`, then
the named file, parsed into a `Theme`. Until then the screen is in DARK, so a file theme costs one full redraw at
the start. The file is parsed again at each start, so a theme edited over USB applies after the eject (the drive
restarts the unit when the computer wrote).

**Keys** (version 1). Colours as `#RRGGBB` (or `RRGGBB`), converted to RGB565 by rounding each channel.

| Section | Key | Today (DARK) | Used for |
|---|---|---|---|
| theme | `version` | 1 | the format; a higher one is refused |
| theme | `name` | DARK | shown in THEME, up to 12 characters, A to Z, 0 to 9, space |
| theme | `base` | dark | the built-in theme the file starts from |
| colors | `background` | `#000000` | every screen's background, text's background |
| colors | `text` | `#FFFFFF` | labels, values (white digits), the date |
| colors | `dim` | `#848284` | notes, axes, a value's tail |
| colors | `line` | `#393D39` | menu row lines, empty bars, the ridden part |
| colors | `accent` | `#FFA500` (approximately; 0xfd20 in the code) | the selected row, banners, frames in the editor |
| colors | `on_accent` | `#000000` | text on the accent |
| colors | `alarm` | `#FF0000` | alarm banners, the BACK button, bad readings |
| colors | `on_alarm` | `#FFFFFF` | text on the alarm |
| colors | `ok`, `warn` | `#00FF00`, `#FFFF00` | the strip's dots, the trainer's states |
| colors | `ferris` | `#F74D00` | Ferris |
| colors | `cog` | `#C5C2C5` | the overview's menu cog |
| colors | `course` | `#CE00FF` | the course on the map |
| colors | `zone1` to `zone7` | Intervals.icu's palette x 1.5 | zones, grade, gauge, W' balance, DFA a1, profiles |

(The hex values are the RGB565 constants of src/ui/mod.rs, src/ui/map.rs and `ride::ZONE_COLORS` written back as
RGB888; the table in the code is the reference, and the key list above is generated into the doc by a test or kept
in step by one.)

**Validation.** Never a panic, never an unreadable screen:
- A value that does not parse, or a key not in the table, is ignored: the base theme's value stays.
- A file over 4 KB, or with no `[theme]` section, is not a theme: not listed.
- `version` above 1 is refused as a whole (a banner: THEME TOO NEW); a missing `version` reads as 1.
- After the values are read, the theme is checked as a whole, and refused (the built-in DARK used, banner THEME
  REFUSED) when: `text` on `background`, `on_accent` on `accent` or `on_alarm` on `alarm` have a contrast ratio
  under 4.5:1 (WCAG 2's formula, integers only); or `alarm` equals `accent` or a zone colour (red only for alarms,
  docs/features.md); or two zones are the same.
- The checks are pure functions in a new src/ui/theme.rs, with doc tests.

**Size limits.** A file of 4 KB at most (read into a buffer in its own `#[inline(never)]` function, as
`load_pages` does, so main's frame stays as it is: tools/stack_check.py), 16 files listed at most, names up to 12
characters shown.

**Versioning.** `version` is the format's major number. New keys are added without a new version (old firmware
ignores them, old files get the base theme's value); a key whose meaning changes gets a new name, never a new
meaning.

## 3. In the code

### The table in the canvas (recommended for colours)

Every colour is already a named constant (src/ui/mod.rs `BLACK`, `WHITE`, `ACCENT`, `RUST`, `RED`, `GREEN`, `GREY`,
`DARK`, `YELLOW`; `map::COURSE`; `ride::ZONE_COLORS`), with one literal (the cog's `0xc618`). Pure logic returns them
too: `ride::hr_color`, `power::wbal_color`, `workout::gauge_color`, `ui::grade_color`, and storage/workout_file.rs packs a
profile column's colour as its index in the palette. So a theme can be a colour lookup table, as indexed-colour
displays do it:

```rust
/// A theme, resolved: the colour each built-in colour is drawn in. Copy, no heap, about 60 bytes.
#[derive(Clone, Copy)]
pub struct Theme {
    /// The built-in colours (keys) and what each is drawn as, same order as `KEYS`.
    pub ink: [u16; KEYS.len()],
    /// The font (level b).
    pub face: Face,
}

trait Canvas {
    // ...
    /// The colour `c` (a built-in constant) is drawn in. The identity unless a theme is on.
    fn ink(&self, c: u16) -> u16 { c }
}
```

- `Panel(b, clip, smooth)` becomes `Panel(b, clip, look)` with `look: Theme` (a copy, so no borrow of `App` is held
  while a `&mut self` method runs; 60 bytes copied per `Panel`, nothing beside a window's SPI bytes). `Row` takes
  the same, so screenshots show the theme.
- `Panel::fill` draws `self.ink(color)`. `text::draw` translates the style's `fg` and `bg` once per call, before it
  builds the 16-shade palette, so anti-aliased edges blend into the theme's background.
- The few `blit` sites that make colours (the gauge's `gauge_px`, the cog) translate their handful of colours once
  before the blit, never per pixel. The basemap and the IGN tiles are never translated: they are pictures.
- `ink` is a `match` over about 20 constants per `fill` or per glyph, not per pixel. No measurable time awake;
  image about +2 KB (inferred: measure, write it in the commit).
- **Roles that share a colour today are split first**, since a table keyed by colour cannot tell them apart:
  black is both the background and the text on the accent (banners, the selected menu row) and on the call's
  green button; white is both the text and the text on red. New constants `ON_ACCENT` and `ON_ALARM` get their own
  value, one step off (`0x0020`, `0xffdf`: the same look), and a `const` assert keeps every key distinct.
- Pure logic, the `Drawn` values, the profile cache and every doc test keep today's constants: they are the keys.
  Only the canvas knows the theme.
- No global: the `Theme` is a field of `App`, set when the file is read or picked, handed to each `Panel` as the
  FONT setting is today.

### The alternative: theme fields at every call site

`c.theme().text` instead of `WHITE`, `Style` built from the theme, pure logic returning a zone number that the
drawing code turns into a colour. It is the explicit way and the only one that also carries metrics (level c),
but it touches about 450 lines in some 30 files, changes the signatures of the colour functions in training/, and
every test that compares a colour. For colours alone it buys nothing the table does not; level c will need
`c.theme()` anyway (below), and then the table moves into the same struct.

### Tests

- DARK's table is the identity (a const test), so every existing screenshot and pixel test is unchanged.
- A **scramble test**: a theme mapping each key to a colour no key has; every screen of the fuzz tour drawn; no
  pixel outside the basemap may still hold a key's value. It catches a `fill` or `blit` that skips `ink`.
- Each built-in theme: the preview and the main screens saved (`Sim::save`) for the gallery, and the contrast
  checks on its values.
- The parser: unit tests (a good file, each kind of bad value, a too-new version, a refused contrast) and doc tests.
- **Fuzz** (src/sim/fuzz.rs, AGENTS.md "every new input path"): files generated from the key table (sections, known
  and unknown keys, good, cut and wild hex values, long lines, NULs, CRLF), parsed; asserts: no panic, the result
  passes the whole-theme checks or is DARK, and a count over the seeds that every key was set from a file at least
  once. Plus damaged names in `settings.bin`.

## 4. Built-in themes, and making one

| Theme | For | Values |
|---|---|---|
| DARK | today, night, saving power is no reason (below) | today's constants, exactly |
| OUTDOOR | high contrast on black | `dim` light grey (`#C0C0C0`), `line` brighter, the zones brighter (Intervals.icu's x 2, clamped) |
| LIGHT | sun: black on white | `background` white, `text` black, `dim` `#606060`, `line` `#8C8C8C` (3:1 on white), `accent` dark orange with white text on it, the zones at Intervals.icu's own values (made for white pages) |

- The panel is transflective (BOARD.md, backlight: "in daylight the transflective panel is read by reflected
  light"), so a white background reflects the sun where black does not: LIGHT is the sun theme (inferred: check
  on the unit outdoors). An LCD's power is its backlight, not its pixels, so no theme saves power (inferred from
  the panel type).
- **T2, done:** LIGHT is built in (`Theme::LIGHT`, white background, black text, dim `#606060`, line `#8C8C8C`, accent
  `#B04000` with white on it, alarm `#D00000` red, zones at Intervals.icu's own values; it passes every whole-theme
  check with no base to excuse it, unlike DARK). What assumed black and was fixed: the screenshot row (`App::render_row`
  started from black, so the gaps nothing draws, such as the strip's gap, were black in a light theme; it now starts from
  the theme's background). The cog already blended over `ink(BLACK)`, the text and gauge blends already took the
  theme's background, the zones are the palette itself, and the map's arrow, outline and track go through `fill`.
  The `light_assumes_no_black` test draws every scene in LIGHT with its text one step off black: no pixel may be the
  built-in black. Left as it is: the basemap and the IGN tiles are pictures (a light map on a light theme, a dark map
  on DARK are the map's own look), and the IGN copyright box (black on white, a picture's overlay).
- Before T2, LIGHT needed what assumes a black background fixed first: the cog's `dim` blends over black; the zone colours are
  brightened for black (`ride::ZONE_COLORS`' doc); check the map's track and the basemap's edges. A screen opening
  already fills its whole area, so a white background costs no more than black.
- The ST7789's inversion (INVOFF instead of INVON) is not a theme: it inverts every colour, the zones and the map
  included.

**Making one (T7, built).** docs/features.md's theme table lists every key with its meaning and range; a test
(`features_lists_every_theme_key`) fails when that table and the parser's keys differ.
1. `python3 tools/theme.py examples OUT_DIR` writes `DARK.ini`, `OUTDOOR.ini` and `LIGHT.ini`: each built-in theme with
   every key set and a comment line above each. It writes to a folder you name, never to a mounted drive (`/Volumes`)
   unless you pass `--drive`.
2. Copy one to `\Rustfw\Themes\` on the drive, rename it (12 characters at most,
   letters, digits, `-` and `_`), and edit the copy. A key you delete keeps the base's value (`base = dark, outdoor or
   light`).
3. `python3 tools/theme.py preview FILE [--out PNG]` runs the ignored test `theme_preview` (`THEME_FILE` names the file):
   the overview, a ride page and the menu drawn by the simulator, as `target/sim/theme_preview_*.bmp`, and one PNG of
   the three when PIL is installed. A file the checks refuse makes the test fail with the reason.
4. Put it on the drive, then pick it in SETTINGS > DISPLAY > THEME.

The fixture `test-data/themes/` is `examples` run into the repository: `tools/theme.py selftest` checks it equals a fresh
build, and `examples_parse_back_to_the_builtins` (ui/theme.rs) parses it back to the built-in themes, so the Python
values and the Rust constants cannot drift apart unnoticed. No theme with a place or a person in it enters the
repository.

**Picking.** SETTINGS > DISPLAY > THEME: the row shows the name; it opens a list (the built-ins, then the files,
16 at most) as COURSES does. A tap applies it at once (full redraw); `settings.bin` is written when no ride is open.
FONT stays its own setting; a theme's `font` key (level b) sets FONT when the theme is picked (T3), not on an AUTO switch.

**4b. THEME AUTO (T5, built).** THEME's list starts with AUTO. With it, the theme in use is DAY THEME (OUTDOOR at
first) or NIGHT THEME (DARK at first), each picked as THEME is (the built-ins, then the files), and the ambient light
chooses between them. The thresholds are stock's backlight curve (drivers/ambient.rs `AUTO_BANDS`): night under 20 lux,
the top of its dusk band, where the backlight is at 20 %; day again over 50 lux, the top of its 30 % band, 2.5 times
the night threshold, so a dusk that flickers stays on its side. Each is held 30 s (the mean of each second's readings,
checked once a second, integers only), and two switches are at least a minute apart (`ui::theme::DayNight`, unit
tests). The first checked reading decides at once, with no hold. AUTO's day and night themes are resolved into RAM
between rides (`App::load_day_night`: a missing file gives its side's default, with a notice), so a switch never reads
the card. A switch is a full redraw, only when the
name in use changes. While AUTO is on the light sensor is read once a second when only AUTO wants it (every 250 ms while the backlight
follows it too), a cost on
the battery that the unit has not measured yet (status.md). The settings file keeps AUTO (a byte), DAY THEME and
NIGHT THEME (12 bytes each) after POWER SMOOTH (data bytes 69 to 93); a file of 44, 66 or 69 bytes reads AUTO off.

## 5. Phases

### Phase 1: colours (the most value, the least code)

- Split the shared roles (`ON_ACCENT`, `ON_ALARM`, the cog), `Canvas::ink`, `Theme` in src/ui/theme.rs, `Panel` and
  `Row` carrying it, `text::draw` and the two colour-making blits translating.
- The parser, the whole-theme checks, `\Rustfw\Themes` listed, the name in `settings.bin`, the THEME row.
- DARK, OUTDOOR and LIGHT (built in; the black-background assumptions fixed, after a look at every screen in
  the scramble test's shots).
- Tests: identity, scramble, parser units, contrast, fuzz, gallery shots per theme; on the unit, OUTDOOR and LIGHT
  read in the sun (docs/status.md, "Not yet seen on the unit").
- Effort: about two days. Risks: a draw path that skips `ink` (the scramble test), an unreadable theme (the
  contrast checks), the first-pass redraw seen as a flash (accepted; a free backup register bit could remember a
  built-in choice if it shows).

### Phase 2: fonts and bounded metrics

Fonts (T3, sim only: PIXEL and BARLOW) and bounded metrics (T4, sim only, `[metrics]` in the file, below) are done.

- `font = pixel | barlow` in the file, as the FONT setting's default.
- A few metrics, each a range every test runs at both ends: `zone_bar` 4 to 12 (today 8; built as 4 to 8, see below), `frame` 1 to 4 (3),
  `menu_row` 44 to 56 (44: never under a fingertip). Here `Canvas` gets `fn theme(&self) -> &Theme`, the consts
  become fields read through it, and the touch code reads the same values from `App`.
- New fonts from files: not planned. `Font` holds `&'static` data; a font from the card would need a buffer in a
  static cell as the courses have, a loader to fuzz, and its own fits tests. The two fonts linked today cover the
  wish.
- Done: `zone_bar`, `frame`, `menu_row` in `[metrics]` of a theme file, whole px, **clamped** into range (a word that
  is no number is ignored, as a bad colour is: the file stays usable and every value drawn is one the tests ran).
  `Theme` carries them, `Canvas::theme()` hands them to the drawing code, and `App` reads `theme.menu_row` for the hit
  test and the rows a screen shows (`App::menu_rows`: 6 at 44, 4 at 56), so a tap lands on the row drawn. The
  built-ins are 8, 3 and 44. `zone_bar` stops at 8: the label's ink under the bar ends at bar + 14 px and the value
  band starts at 22 (`FIELD_LABEL_H`, the same layout for every theme), so a bar of 12 put the label 4 px over the value. Not in settings.bin.
- Tests: the sim suite and the "every row fits" tests at the minimum and maximum of each metric (a theme per end);
  the fuzz above with the new keys.
- Effort: about a week. Risk: a combination of values never drawn in a test; hence few knobs, each bounded.

### Phase 3: layouts, as variants

- The ride pages are already the rider's (the page editor, `pages.bin`, MORE); a theme does not carry them.
- The overview: a few layouts in code (today's, six fields with a smaller TODAY card, the card on top or not),
  chosen by a key `overview = classic | fields`; each one a table of boxes checked by tests like page.rs's
  (`fits`, 44 px targets, no overlap) and in the gallery.
- Coordinates from a file: not recommended. Their checker would be a layout engine, each box is also a dirty box
  and a touch box, and a screen the tests never drew would reach the ride.
- Done (T6): `[theme] overview = classic | fields` (a word that is neither is ignored, the base's stays). `fields` puts a
  2 x 2 table of field boxes in the TODAY card's place (time, next workout, year km, week hours: `ui::TABLE`,
  `table_box`), above the four chosen fields; each box is a field cell redrawn on its own as the fields are. A tap
  anywhere in the old card's area still opens today's workout (`TODAY_BOX`). Tests: `overview_variants_tile_and_fit`
  (tiling, fits at both metric ends), the sim test at both ends, a DARK classic hash taken on main, the fuzz.
- Effort: a day or two per variant. Risk: low once variants are code.

## Not planned

- Themes switching by themselves (light by day, dark at night from the light sensor or sunset): easy on top of
  phase 1 (two names and the ambient reading), once the owner wants it.
- Icons, Ferris and the map style in a theme.
