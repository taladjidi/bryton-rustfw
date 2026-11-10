# App audit, 2026-11-10

An adversarial audit of the phone app after a ride was saved on the phone with 342 bytes missing and 342 doubled
(size right), uploaded, refused by Intervals.icu and left on "Reading the ride..." for good. Three read-only
reviews: the transfer (app/core files.rs, BleFileTransport, RideBrowser, firmware filesvc.rs), the rest of the app's
logic and performance, and the UI (Simulator, light, dark, accessibility sizes; screenshots were kept outside the
repository). The previous UI review is docs/review-app-ui-2026-11-08.md.

Status: **fixed** (with its commit), or open. CONFIRMED: traced in the code or tested; PLAUSIBLE: read, not shown.

## Fixed the same day

| Id | What | Commit |
|---|---|---|
| F1 | An answer ended without a checked END (drop, silence) and the resume read on from the bytes kept: the hole | 31d07c32 |
| F2 | A wrong END count failed the download at once ("inconsistent", no retry): now asked again, 3 times | 01a84889 |
| F3 | A ride's FIT CRC is checked before it is saved; a bad one is read again | 31d07c32 |
| F4 | The ride build had the fast link off (MTU 23, about 1.3 KB/s): ride image with `FAST_LINK=1`, FLASHLOG #95 | 206b5d1 image |
| F5 | A ride Intervals.icu refused stayed rejected for good, with no green mark | 45989b4f |
| L1-L3 | 45989b4f deleted the phone's only copy on any file refusal (a ride the unit no longer holds was lost), needed a second launch, and could re-read at every launch: now only a copy failing its CRC, kept as `.fit.bad`, with the unit listing the ride | gate pending |
| U0 | A ride's download progress sat beside the glyph: "Cancel" wrapped a letter a line, the separators were cut | gate pending |

## Tooling against these regressions (owner's ask, same day)

- `app/core/src/files_fuzz.rs`: fault-injection fuzz of a ride download (lost, doubled, late and cut frames, BUSY with
  its count, one answer or windowed answers): a Finished download holds the unit's exact bytes, a failure is allowed
  only past what REDO_MAX absorbs, the bytes read again are bounded. It fails with the overrun redo reverted and with
  the CRC redo removed.
- **T13, fixed (found by the fuzz):** a doubled notification near an answer's end overran the size and failed the
  download at once (TooLong); now that answer is dropped and asked again within REDO_MAX.
- The re-read and upload decisions are core functions with truth-table tests (`reread_copy`, `upload_candidate`);
  the app never deletes a ride's file (damaged copies set aside as `.fit.bad`, `.2`, `.3`), and `tools/lint.py` checks
  every removal, move or overwrite in app/ios against a short allow-list.
- AGENTS.md "Review before push": ride-data changes are pushed only with a reviewed sha the runner checks.
- Open: `check.sh --quick` fails `sim::fuzz::file_service_commands` at FUZZ_SCALE 0.25 (a reach count of 4 under 5),
  a firmware fuzz flake at the quarter scale; the full gate passes.

## Open, by severity

### Transfer (T)

- **T1, CONFIRMED: deleting a workout from the phone always fails.** files.rs keeps one `files_in` / `list_folder` for
  both folders; RideBrowser `nextWorkout` ends with `refreshRoutes()`, so the courses' list replaces the workouts'
  and `delete_command(key)` finds nothing: "The unit's list changed. Look again.", and a calendar delete job is
  dropped. Fix: one list per folder in the core; test: list workouts, list courses, delete a workout.
- **T2, CONFIRMED: a drop restarts the file.** The unit streams a READ to the file's end in one answer, so F1's
  "drop the unchecked answer" drops everything since byte 0, and REDO_MAX is spent in whole files. Fine at 16 KB/s
  for today's rides, poor for a 3 MB ride or at MTU 23. Fix (firmware, link only): end each READ answer every 16 to
  32 KB with END OK and its count (the phone already resumes on that), better with a CRC of the answer in END.
- **T3, CONFIRMED: one failed background download stops the sync.** `fail()` sets `phase = .failed`; `autoSync`,
  `autoCalendar` and the profile push wait for `.idle`, which only a user action restores. Fix: a background
  failure returns to `.idle` (message kept).
- **T4, PLAUSIBLE: workouts and courses read back (READ_IN) get no whole-file check.** A corrupt read looks like "not
  on the unit": a delete job is taken as done, a send rewrites. Fix: the same FIT CRC check for `.fit` READ_IN.
- **T5, PLAUSIBLE: writes at MTU 23 use a long write the firmware refuses.** `maximumWriteValueLength(.withResponse)`
  is 512 whatever the MTU, the core clamps to 244, the unit has no long-write memory: "Write failed". Less likely now
  the fast link is on, but the FAST LINK row can turn it off. Fix: size writes from the negotiated MTU.
- **T6, PLAUSIBLE: SILENCE_MS (5 s) also times WRITE_END**, while the unit reads back up to 1 MB before answering:
  a course sent fine is reported failed. Fix: 30 s in `Ending`, or relist and compare.
- **T7, open question: why 19 frames went missing.** Phone side suspect: the CBCentralManager runs on `.main` and
  publishes per notification; firmware side, an HVX time-out is ambiguous (can double, not lose). Fix: the
  central on its own serial queue; an offset or CRC per answer (T2).
- T8 `busyRetries` reset only by `download()`: the 21st BUSY of a session fails. T9 speed and progress wrong after a
  truncation (reset `first_ms`). T10 `yieldBackground` aborts without sending Cancel. T11 FILE_MAX 64 MB, three
  copies in RAM. T12 tests missing: silence after a hole, BUSY with a wrong count, cancel then resume, READ_IN CRC,
  a fuzz seed reaching the CRC redo.

### Logic and performance (L)

- **L4, CONFIRMED: "Reading the ride..." can still stick.** An upload whose file is unreadable now re-reads (fixed with
  L1), but RideLibrary's `unreadable` set clears only when the download count grows, so with uploads off a damaged
  copy is never recovered. Fix: key it by name and size; report a ride unreadable twice to RideBrowser `reread`.
- **L5, CONFIRMED: RouteStore publishes from a background thread.** It is not `@MainActor`; `importGpx`'s Task writes
  `@Published` state and pushes the navigation path off main. Fix: `@MainActor` on RouteStore.
- L6 PLAUSIBLE: `RouteStore.reload` can overwrite a route just imported (blank route page). Fix: a generation count.
- L7 PLAUSIBLE: a non-file 4xx (404 athlete id...) rejects every queued ride; a passing 401/403 blocks uploads and
  the calendar until the key is entered again. Fix: stop the pass instead; clear `keyRefused` on foreground.
- L8 PLAUSIBLE: the planned list is empty for up to an hour after a launch (`planned` in memory, the read skipped).
- L9 performance: `RideLibrary.save()` rewrites the whole cache per ride analysed (quadratic catch-up);
  FitnessView recomputes on every brief and reads the Keychain in its task id (and puts the key in a view id);
  workout and GPX imports parse on main. Fix: debounce, task id from `keyVersion`, imports off main.
- L10 the insight cache is keyed by name and size: a re-read with the same size shows the old analysis.
- L11 calendar retries forever on a `retry: false` error; one `retryTask` shared by uploads and the calendar;
  `upload_outcome` takes any 200 without `activities` as uploaded. L12 dead code: `FitnessView.hasKey`,
  `IntervalsClient.getSession`, a DateFormatter per call.

### UI (U)

Broken or clipped:
- **U1: a ride row at accessibility sizes** wraps the date a few letters a line and "Intervals.icu" in four; the
  progress counts and Cancel clip. Fix: a VStack layout at `isAccessibilitySize`, the distance under the title.
- U2: the progress row leaves about 40 pt empty (the 44 pt Cancel frame) and its separator starts at the text inset
  under a full-width bar. U3: Cancel as a small bordered button, not an icon far from a caption word.
- **U4: DFA alpha1 threshold labels overprint** each other and the dashed lines (RideInsights.swift ~182).
- **U5: zone bars change height** between a 0 % zone and the others. Fix: a full-width track behind each fill, the
  values in a right-aligned monospaced column.

Inconsistent:
- U7: the ride page's and the fitness charts have no card; every other chart has one.
- U8: Speed axis labelled every 2 units (unreadable); the x unit floats apart. `desiredCount: 4`, unit in the title.
- U9: the condensed numerals (`numeral`) are on some key numbers only: extend them to lap times, workout steps,
  IF / TSS, profile values; SF stays for sentences.
- U10 one action-row style (icon column) for "All ...", "New workout", "Import"; U11 one section header style;
  U12 CTL and ATL nearly the same hue, zone colours reused for series (teal is zone 1 and speed); U13 the unit and a
  ride share the bicycle glyph.

HIG and accessibility:
- U14: Fitness and Profile share one unlabelled toolbar capsule. U15: the latest ride is below the fold (Garmin
  and Wahoo show it first): order the home as weekly panel, latest ride, then Intervals.icu. U16: contrast (the
  12 pt green check, unselected dark-mode bars at 0.4, header grey). U17: axis text under 12 pt. U18: three
  segmented control styles. U19: workout editor step labels at 11 pt.

## Visual direction

Keep it iOS: inset-grouped lists, SF text, system navigation, Dynamic Type everywhere. Carry the unit's retro
identity through three things only:
- the numerals: the condensed face on every key metric, tabular;
- the palette: the unit's zone ramp and accent as the one chart language, flat fills, no gradients;
- a few drawn objects: a pixel-grid unit glyph, the app icon, the empty states.

Never pixel fonts for sentences, axes or text under 17 pt, and no amber at 12 pt on black. Fix U1, U4, U5, U7 and
U8 before any decoration.

## Suggested order

1. T1, T3 (broken features, small), T5 / T6 (false failures), L5 (threading).
2. T2 in the firmware (answers in windows), with T7's serial queue and T4's READ_IN check.
3. L4, L7 to L11, T8 to T12 (stuck states, retries, performance, tests).
4. UI: U1 to U8, then U9 to U19 and the visual direction.
