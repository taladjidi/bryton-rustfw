# Live segments: a plan

Status: S1, S2 and S3 built (2026-10-29/30: `src/nav/segment.rs` the reader, `src/nav/segment_match.rs` the matcher, `src/app/segments.rs` the index and the ride's feed; tests and fuzz; not yet seen on the unit). S4 next.
Order of work: docs/roadmap.md, "Order of work", block 2.
Not here: how stock does it in detail (bryton repo ghidra/recovered_src/ride/live_segments.c), the comparison with
other units (checklist.md, "Climbs ahead and live segments").

## Summary

- **One engine on the unit, several sources.** The unit reads segment files from `\Rustfw\Segments\`. It does not
  care where they came from: Strava through a computer tool now, the rider's own rides later, the phone app and
  Strava's licensed Live Segments API last.
- **The file is stock's package, unzipped.** Stock's live segment package (`LiveSegmentData.bin`: a 0x200 header,
  the split times, a FIT course) already holds what a live segment needs, and its target types are exactly the
  effort types of Strava's Live Segments API (5 KOM, 6 QOM, 7 PR, 9 carrot, 11 wolf, 12 most recent, 13 best of the
  last 5). So a licensed feed later maps 1:1, and nothing new is invented.
- **A ghost per target, point by point.** Each target has its elapsed time at each point of the segment, so the
  delta shown is against where that rider really was, not a constant pace (a constant pace only when the source
  gives a total time alone, as Strava's KOM time).
- **Nothing optional can stop the recording** (AGENTS.md): the engine is an extra fed by checked data; a bad file is
  skipped, a failing segment is dropped, and nothing is written to the card while a ride is open.

## Access to Strava (checked 2026-10-29)

- The leaderboard endpoint (`/segments/:id/leaderboard`) was removed in June 2020; leaderboard fields became
  subscriber-only (developers.strava.com/docs/segment-changes). Scraping strava.com is against Strava's terms: out.
- Since 30 June 2026 the API's Standard tier needs the developer's own Strava subscription (heise.de 11315017,
  forum.intervals.icu "Strava API update", June 2026). **Owner's decision needed before S5:** a Strava subscription,
  or the segments come from own rides only (S7).
- Still open to a Standard app with the owner's token: starred segments, `segments/explore` (a box, the riding type,
  climb categories; about 10 segments per call, the popular ones: inferred, to check on the first call), segment
  details (distance, elevation gain, start and end; KOM / QOM times in `xoms`: inferred, to check), segment streams
  (latlng, distance, altitude).
- **Licensed Live Segments** (developers.strava.com/docs/partner/strava-live-segments): the app must be licensed
  (email developers@strava.com, subject "Strava Live Segments Request", with the API app id) and the rider needs a
  subscription (`summit`). It adds `/live/segments/:id`: KOM, QOM, PR, carrot, wolf, most recent, best of 5, each with
  its stream (lat/lon, distance, altitude, elapsed time per point). **Owner action now:** send that email; the answer
  takes time and changes only the source (S8), not the unit.
- Strava's November 2024 terms: data shown only to the athlete it belongs to. A personal tool and the owner's own
  unit fit. No segment, stream or place ever enters this repository (AGENTS.md, public repository): tests use
  synthetic segments.

## The file

`\Rustfw\Segments\<id>.seg`, one per segment, `<id>` the source's decimal id (Strava's segment id; own segments
from 4 000 000 000 down, so they never collide). Layout: stock's `LiveSegmentData.bin` (live_segments.c), little
endian:
Checked against stock's `live_segments.c` (2026-10-29, S1): no difference in offsets, sizes, types or the fit_offset
formula. Left as inferred: the enabled bytes' meaning (its comment gives 1/2/3), the u32 at 0x31, the byte after each
target's type, and the data section's ntargets (its comment says the low byte is used; read as a whole word that must
equal the header's).

| Part | Offset | Content |
|---|---|---|
| header | 0x000, 0x200 bytes | `b0` 0; `id` u32 at 0x01; `version` u32 at 0x05 (bumped on each rewrite); `fit_offset` u32 at 0x09; `name` 32 bytes at 0x0d (ASCII, NUL padded); `distance` u32 m at 0x2d; 0x31 u32 0; `elevation` u32 m at 0x35 (gain); `latlon` 40 pairs of f32 (lat, lon) at 0x39, the first points of the segment for start detection; `ntargets` u8 at 0x179; targets of 0x11 bytes from 0x17a (type 2 ASCII digits, `c2` 0, label 10 bytes, total time u32 s), at most 7; 0x1fd..0x1ff enabled per bike (1, 0, 0) |
| data | 0x200 | `npoints` u32, `ntargets` u32, `ntargets` x 13 bytes of descriptors (zeros), then `time[npoints][ntargets]` u32, each target's elapsed seconds at each point |
| course | `fit_offset` | a FIT course file (file_id type course, `record`s with position, distance, altitude), CRCs valid |

Limits checked on read (anything else: the file is skipped and counted, never a panic): file at most 64 KB;
`fit_offset` = 0x200 + 8 + 13 x `ntargets` + 4 x `npoints` x `ntargets`; `npoints` 2 to 400 and equal to the
course's record count; `ntargets` 1 to 7; times rising along the points; positions on Earth; the FIT CRCs. The tool
decimates a segment to 400 points at most (a 10 km segment at 25 m spacing).

Decided in S1 (2026-10-29): a start pair of the header that is not finite or not on the globe is unused: stored as
(0, 0) and ignored, not a rejection (stock's start detection never reads an unused pair: inferred). A kept target's
time over 65535 s rejects the file (`Error::Times`), since the segment keeps u16 seconds. `total_s` (the target's total
time) is not checked against its column's last time: the file's own figure is shown as it is.

Stock reads its own copies in `\PlanTrip\LiveSegment\` (`LSshortcut.bin`, `%d.data`) and `\PlanTrip\%d.fit`, written
by its import. This firmware never writes there (AGENTS.md, never break it for stock). Reading stock's imported
segments too is S9, optional.

## On the unit

**Memory.** AXI SRAM had about 15 KB left before the segments took 12.9 KB; the scan's cache then went from
4096 rides to 3072 (2026-10-30), and about <!-- n:axi_free -->58 KB<!-- /n --> is left (AGENTS.md, Memory). The index of all segments (id, start lat / lon,
bounding box, distance, name's first 12 characters, enabled: 32 bytes) for at most 128 segments is 4 KB; the active
segment (400 points as i32 lat / lon = 3.2 KB, times as u16 seconds for up to 4 targets = 3.2 KB) is 6.4 KB. One
active segment at a time. If it does not fit next to the courses, stop and report the numbers before moving any
buffer: link.x is a hw-adjacent change.

**Index.** Built at the first main loop pass after the card is mounted (as `pages.bin` is read), from the headers
only (0x200 bytes a file), at most 128 files, rebuilt when the drive is ejected. Never at boot, never during a ride
(the list is fixed when a ride starts).

**Detection** (every fix, about 1 s):
1. Near: the segments whose bounding box, grown by 300 m, holds the position (a loop over the index: integers, an
   equirectangular distance, as course.rs does).
2. Approach: within 300 m of a start, a banner `SEGMENT <name> 300 M` once.
3. Start: the rider passes within 25 m of the first point, heading within 60 degrees of the segment's first
   bearing (stock: under 30 m / 20 m on the projection). The start time is interpolated between the two fixes
   around the closest approach, then the active segment is loaded from its file (one read, its own
   `#[inline(never)]` function for the stack).
4. Progress: the projection on the segment's track with a moving window, as course.rs follows a course (reuse its
   matching code; do not write a second one). The distance along is the point's distance plus the projection.
5. Lost: more than 50 m off the track for 10 s, or the ride paused for more than 5 minutes: banner `SEGMENT LOST`,
   the segment dropped.
6. Finish: the projection reaches the last point (within 25 m of it): the time interpolated, the result shown.
7. Two segments at once: the first one started wins; the others are ignored until it ends (stock does the same,
   inferred). A reset mid-segment drops it: the ride resumes, the segment does not.

**Ghost.** For the chosen target, its time at the current distance, interpolated between points; the delta is the
elapsed time minus the target's. Shown as `-0:12` (ahead) or `+0:08` (behind). Colours: ahead `GREEN`, behind `ACCENT`, never `RED` (red is
for alarms only, features.md).

**Screens.**
- The segment's page is the CLIMB page (one page for segments and climbs, 2026-10-30): one page in the round of ride
  pages, it opens by itself at the segment's start (as at a climb's), over the ride page or the overview the rider is
  on; paging away and back finds it again, and it leaves the round at the finish or when lost, the card following the
  finish. It is built of fields (layout 16, `Field::SegHead`, `ClimbProfile`, `ClimbPace`, `ClimbTime`, `ClimbLeft`):
  the head (the name, the delta in big digits, the largest, and the target's label; within 2 s of target the delta keeps
  the colour it had, so a flip between ahead and behind does not redraw it); the segment's profile (the file's
  altitudes, `Segments::alt_at`) with the rider white and the ghost a grey mark, or, without altitudes, the bar with both
  marks; the pacer's gauge (its target over the segment, the time to the top in its title); the elapsed time on the
  segment and the distance left. On a climb alone the head shows its title and the gain left in whole metres. Redraw:
  the head's changed characters, and each field's own change (AGENTS.md, redraw only what changes): 4650 px/s measured
  in the sim's ride (`page_redraw_cost`).
- The result: a banner with the time and the delta, and a result card (time, against each target, the new PR if
  any), closed by any key or after 10 s.
- MENU > SEGMENTS (the main menu has no NAVIGATION row: it sits beside COURSES): LIVE SEGMENTS on / off; TARGET (AUTO: the
  PR if there is one, else the KOM; PR, KOM, QOM, RECENT in S4; the others when the file keeps them); the list of segments (name, distance, gain), each enabled or not (the enable: S5). On / off and
  the target are kept in the settings file (roadmap block 1, step T0); the per-segment enable in the index, written
  as the header's 0x1fd byte between rides.

**Efforts** (S5, done 2026-10-30; app/segments.rs). While a segment is ridden, its time at each point is kept as the
rider passes it (interpolated between the two fixes around the point; the points left at the finish between the last
fix and the finish), in one of 4 slots (820 B each, 3.3 KB in all beside the segments' static); a fifth finish in a
ride, or one over 65535 s, is counted and dropped. At the finish the effort (id, start date, time, the times) is kept.
Written only once the ride is saved: the clean copy's save done (read back whole, named in `\Activities`), then both
copies done and no ride open (`App::ride_settled`), one effort a pass, on an `EMMC_WRITE=1` build. A ride discarded,
kept in `\Rustfw\Rides` or given up has its efforts dropped, unwritten; a reset loses those not written yet, never
the ride. For each:
- `\Rustfw\Segments\<id>.pr` when it beats the PR (strictly faster than the newest whole `.pr` for the same points,
  and than the PR the segment had when ridden), the copy with the higher sequence number written over the other;
- a line `id,date,elapsed_s` appended to `\Rustfw\Segments\efforts.csv` (made with that header line if missing; the
  date is the effort's start, UTC, `2026-10-30T09:12:05Z`): the last sector written again, then the size.

The folder is never made there (a segment was loaded from it) and nothing goes in `\Rustfw` itself. A failure (a card
full, an error) is counted and the effort dropped, never tried again. Cut at every sector (segment_tests.rs): the ride
file and its records as they were, the `.pr` old or new and whole, the csv with or without the line, at most one FAT
sector apart and one cluster lost (the cut inside a cluster's taking, as any FAT write); a csv whose new line takes a
new cluster is cut too. The index build at the next start finds a `.pr` or the csv a cut left unfinished (its size, or
a chain longer than its size) and has the next settings or pages write compare the two FATs first; a cluster taken
for a new file before its entry is a lost cluster no file names, seen only by a whole compare (as any FAT create).

At the next load of the segment, a `.pr` whose id and point count are the segment's gives the PR column (it wins over
the source's PR, or fills it); anything else is ignored. The per-segment enable byte (0x1fd) is not written yet (its
meaning is S4's). A FIT `segment_lap` message in the ride file: not in v1.

The `.pr` file: 2048 bytes, two copies of 1024 (nav/segment.rs `PrCopy`), little endian:

| Offset | Content |
|---|---|
| 0 | `RSPR` |
| 4 | version u16, 1 |
| 6 | npoints u16, 2 to 400 |
| 8 | seq u32 (higher is newer) |
| 12 | id u32 |
| 16 | date u32, FIT time of the start |
| 20 | total u32 s, the last point's time |
| 24 | npoints x u16 s, the first 0, never falling |
| 1020 | CRC-32 of 0..1020 |

## On the computer: tools/segments.py

Python 3 standard library only, as `tools/sync.py`, and it reuses sync.py's config (`sync.json`, block `intervals`,
docs/sync.md) and HTTP helper (with its User-Agent). **Source: Intervals.icu** (owner's decision 2026-10-29: no Strava
API, it needs a subscription): Intervals.icu already holds the Strava segment matches of the owner's rides, and the
ghost is the owner's own best effort. The cache lives in `~/.cache/bryton-rustfw/segments/`, never in the repository.

- `segments.py fetch [--since YYYY-MM-DD]`: the activities with `has_segments`, for each its `/segments` list
  (`segment_id`, `start_index`, `end_index`, `name`) and its streams (time, latlng, distance, altitude), one JSON file
  per activity in the cache. Resumable (a cached activity is not fetched again unless `--refresh`; one without GPS is cached empty; an activity
  under 2 days old with no segment yet is not cached), polite (0.3 s between calls), stops on HTTP 429 or a network
  error and says so. API: https://intervals.icu/api/v1/docs.
- `segments.py build [--min-km 0.5] [--min-gain-m 10] [--min-rides 2] [--max 128] [--kom ID=M:SS ...] [--out DIR]`:
  - Efforts are grouped by segment id. A segment is kept when it has at least `min-rides` efforts left after the sanity
    drop, its length is at least `min-km` and its gain at least `min-gain-m`; longest first, at most `max`.
  - Geometry: the fastest effort among those within 10 % of the median length; its track between the segment's
    indices, thinned to at most 400 points spaced evenly along the distance (about 25 m; first and last kept). The
    gain is the ascent of its altitude with a 2 m hysteresis.
  - Sanity drop: an effort whose length differs from the geometry's by more than 10 %, or whose first or last point is
    over 50 m from the geometry's, or with a gap in its GPS or time stream, a time or distance that falls, or a
    pause (over 60 s with under 1 m moved). The same activity, segment and start index counts once, and
    `min-rides` counts distinct activities.
  - Targets: PR (fastest effort), KOM (only with `--kom`, at constant pace; the owner reads the time on Strava),
    RECENT (latest), BEST5 (fastest of the last 5); the unit keeps the first three it knows plus the QOM (`PREFER` in
    segment.rs) and reads past BEST5. Each effort's time at a point: its time at the same distance from its own start,
    scaled by its length over the segment's, never falling.
  - Output: `<id>.seg` per segment in `--out` (default `~/.cache/bryton-rustfw/segments/out`). A segment id over 32
    bits is skipped and counted (the header's id is a u32).
- `segments.py write --volume PATH`: the built files to `PATH/Rustfw/Segments/`, each written beside then renamed, read
  back and compared (the USB link can drop); it deletes only the `.seg` files an earlier write made (listed in the
  cache's `written.json`) that are no longer wanted, never another file: the manifest (`state/written.json` in the cache, one list per destination) is written before
  anything is, the temporary file is read back (bypassing the page cache where the OS allows: macOS `F_NOCACHE`)
  before the rename, and only names `<digits>.seg` are ever deleted. It refuses a `--volume` that is not an existing
  directory holding `Rustfw/` (`--init` creates it) and an empty build (`--prune` allows it). An eMMC write: only
  with the owner present.
- `segments.py own (--rides DIR | --volume PATH) [--dry-run | --list] [--min-km 1] [--min-gain-m 0] [--min-rides 2] [--max 128] [--max-per-cell 8] [--out DIR]`
  (S7, no Strava, no network): the rider's own FIT rides, from `--rides` or the drive's `Activities/` (whole files only,
  not indoor, each ride once; the FIT records are read by the tool itself, `fit_records`).
  - **Finding**: each ride is thinned to a point every 30 m and put in a grid of 75 m cells, so a point meets only the
    points around it (no all-pairs scan). A cell indexes the points of at most `--max-per-cell` rides (the oldest); later
    rides are still matched against those and join the stretch through them, so a road ridden 100 times is compared
    with 8 rides a point, not 99: the work is linear in the rides (30 same-road rides: 0.3 s, 139 k candidates against
    252 k unbounded). Two points match when they are within **25 m** (the unit's `START_M`, so a file is matched the way
    it was found; GNSS error is 3 to 5 m a pass) and head the same way (within 45 degrees). A run of matches along two
    rides over **1 km** is an overlap; the opposite direction never matches, and a ride's own points never count (a loop
    or a repeat inside one ride is not a second ride).
  - **Merging**: the longest overlap claims its stretch. It is cut to the piece most of the rides covering 1 km of it
    share (lead-ins running side by side do not stretch it), the rides covering 90 % of that are its members, and it
    ends where all of them are within the tolerance: its longest contiguous piece (a detour of one ride is not in it),
    dropped under 1 km. An overlap more than half claimed already is the same stretch and is dropped, so a stretch
    ridden 10 times is one segment. Each stretch then goes through the S6 `build_segment`: same sanity drop, geometry
    (the fastest ride), altitude profile and gain, targets PR, RECENT, BEST5 (no KOM); the best effort is the ghost.
  - **Ids** are stable: `ids.json` in the output folder keeps `{id, start, end, length_m}` for every stretch ever made.
    A stretch takes the id of an entry whose start and end are each within 50 m and whose length is within 10 %, so a
    ride added to a stretch does not renumber it; else the next id below all given so far, from 4 000 000 000 down
    (a dropped entry keeps its id, which is never given to another road). Written beside the files, atomically. The
    name is `OWN <km> KM <n>`, `n` from the id. A stretch that moves more than that (a much longer one) is a new id.
  - **Output**: `<id>.seg` and `ids.json`, default folder `~/.cache/bryton-rustfw/segments/own`. `own` refuses `--out`
    equal to `build`'s folder (each removes the `.seg` files it does not make). `--dry-run` (or `--list`) prints the
    stretches and writes nothing; the summary says how many `--max` cut.
  - **Both sources on one drive**: `write --volume PATH` without `--out` takes both folders (`build`'s `out` and
    `own`), writes the union and removes only earlier files neither lists; `--out DIR` writes that one folder.
- `segments.py selftest` (check.sh): synthetic efforts (straight lines at 0 N 10 E, invented ids): grouping, the sanity
  drop, thresholds, decimation, rising times, the layout read by an independent reader, write and verify into a temp
  dir, and the committed fixture equal to a fresh one. The `own` cases write synthetic FIT rides (made-up place) and check: two rides
  sharing 2 km give one segment of about 2 km, a shared 500 m none, the opposite direction none, three rides one segment
  whose PR is the fastest ride's time; a ride on 95 % of the stretch is a member and one on 80 % is not, with its overlap
  dropped as already claimed; a ride's detour is not in the stretch; ids stay when a ride is added, a new road gets the
  next one down, none is reused; 30 same-road rides give one stretch of 30 members with a bounded candidate count; `write`
  of two folders and its prune; `fit_records` on a compressed timestamp, a developer field, junk positions and a cut file,
  and a 200-seed mutation fuzz of a FIT ride (never raises, and reaches the record code); the written file is read back by
  the independent reader.
- `segments.py fixture [OUT]` writes `test-data/segment/synthetic.seg`, which `src/nav/segment.rs` parses in a unit
  test. An ignored test there, `built_files_parse` (`SEGMENTS_DIR=... cargo test-host -- --ignored built_files_parse`),
  parses a real output directory.

## Steps (each one merged, through check.sh and a review, before the next)

| Step | What | Agent |
|---|---|---|
| S1 (done 2026-10-29) | `src/nav/segment.rs`: the `.seg` reader (pure, over a byte slice), the checks above; unit tests, doc tests; fuzz (src/sim/fuzz.rs): files generated from the layout (header fields, counts, times, a FIT course), cut, wild values; a count that every check is reached | implementer |
| S2 (done 2026-10-30) | The matcher in `src/nav/segment_match.rs` (decisions below the table): near, start, progress (course.rs's code), lost, finish, the ghost's delta; host tests with synthetic tracks: a clean pass, a start missed by 30 m, a U-turn, a stop, lost and found, two overlapping segments, a segment along a course | implementer (sonnet) |
| S3 (done 2026-10-30) | `src/app/segments.rs`: the index at the first pass, the active segment loaded, fed from the GNSS fix each second; nothing written; sim tests over a whole ride on a synthetic segment | implementer (sonnet) |
| S4 (done 2026-10-30) | The SEGMENT page, the banners, the result card, the menu; sim screenshots to the gallery; redraw cost measured with the profiler in the simulator | implementer (sonnet) |
| S5 (done 2026-10-30) | The efforts written after the save (`.pr`, `efforts.csv`); the per-segment enable byte left to S4 | critical, then reviewer on opus |
| S6 (done 2026-10-30) | `tools/segments.py` from Intervals.icu (fetch, build, write); selftest in check.sh | implementer (sonnet) |
| S7 (done 2026-10-30) | Segments from the rider's own rides, no Strava: `segments.py own` reads the FIT rides, finds stretches ridden at least twice in the same direction (overlap of two tracks over 1 km, 25 m tolerance, near-duplicates merged) and writes `.seg` files with the same writer as S6; selftest in check.sh. On the unit later if wanted | implementer (sonnet) |
| S8 | Licensed Live Segments, if Strava grants it: `segments.py live` fills carrot, wolf, most recent, best of 5 and the KOM's real splits from `/live/segments/:id`; later the phone app does the same over BLE | implementer |
| S9 | Optional: stock's imported segments (`\PlanTrip\LiveSegment\`) read too, read only | implementer |

Decided in S3 (2026-10-30), memory and placement: one static `Segments` (main.rs `make_segments`, link.x `.axibss`, a new
line there like the other cells; 12,904 B, all zeros at start: `Effort` counts from 1 so that `None` is 0, and the event ring is
not an `Option`) holds the index (128 `segment_match::Entry`, 32 B each = 4 KB: the id, the box of the start pairs, the first
pair and the direction to the next one as two i16; the name, distance and gain are not kept, S4's list reads a header when it
shows it), the one active `Segment` (10.2 KB: each point's altitude is kept, 1.6 KB, so the climb pacer can plan the rest of the segment; a file whose records lack an altitude parses without a profile), the matcher and the counters. AXI SRAM has 1.6 KB left after it (it had 15 KB before S3, 1.6 KB after).
No buffer of the whole file: `Segment::parse_with` reads 1 KB at a time (`segment::CHUNK`), so a 64 KB file loads with about 3 KB
of stack, in `Segments::load` (`#[inline(never)]`). The index is built 8 files a pass (worst pass in the sim: 19 KB read, 8 ms at
the unit's 2.5 MB/s) 5 s after the start, only with no ride open; a computer that wrote to the drive restarts the unit, which
rebuilds it. The per-segment enabled bytes are not read yet. The idle matcher takes `Entry` (or `Header`, through `AsEntry`).
API for S4: `App::segment_now()` (name, elapsed, delta against AUTO = PR else KOM, target, along, left, ghost), `App::segments()`
with `events()` (the last 4 with their clock), `indexed()`, `skipped`, `dropped`. LIVE SEGMENTS and TARGET are settings since S4.

Decided in S2 (2026-10-30), API for S3: `Matcher::new` (about 90 bytes, a field of App), `step_idle(headers, lat, lon, t)`
(an iterator of `&Header`, the index; gives `Approach(id)` or `Start(id)`), then the caller parses the file into the
segment's cell and feeds `step_active(&Segment, lat, lon, t)` (gives `Lost(id)` or `Finish{id, elapsed_s, deltas}`);
`active_id`, `along_m`, `elapsed_s(now)` for the page; `target_s` / `delta_s(seg, column, distance, elapsed)` for the
ghost; `near(header, lat, lon)` is the index's filter. Time `t` is whole seconds of the ride clock. Course.rs's matcher
is shared, not copied: `Course::nearest` became `course::nearest_by` over a closure, and `seg_hit` is the projection
on one step. Choices: the "bounding box" is that of the header's 40 start pairs (the header holds no other point); a
start needs 2 m of movement between two fixes and the closest approach inside the pass (the start time is
interpolated there); lost is 10 s beyond 50 m of the track, or 10 s back more than 50 m behind the furthest point
(a U-turn rides the track the wrong way), or 300 s still (within 10 m of one spot, so GNSS jitter at a stop counts) or without a fix; the finish is the
crossing of the last point's perpendicular, within 25 m of the last point at the crossing (so a 30 m/s rider finishes) (stopping short never finishes), its time interpolated between the fixes; the
Approach banner is said once per segment, the next one only when the rider is beyond 300 m of the one said, and not again after a lost segment near
its start. After any Lost or Finish the matcher is idle again and the segment is the caller's to drop.

On the unit after S5 (docs/status.md, "Not yet seen on the unit"): a ride over two known segments with an EMMC_WRITE=1
build: the approach banner, the page opening, the delta against a ride's split, the finish, the `.pr` written after
the save and used as the target on the next pass.

Decided in S4 (2026-10-30), changed the same day (the owner: the page should be in the round, and share the climb
coach's layout): the page was its own `Screen::Segment`; it is now the CLIMB page, `Screen::Ride(CLIMB)`, opened by
app/profile.rs `open_climb` (once per start, over a ride page of a running ride, not while a key or finger is down;
`climb.seg` is the request app/segment_ui.rs makes), so any key acts as on a ride page. The result card is
`Screen::SegmentResult` (any input, or 10 s). The delta is drawn per character cell (only a
changed character): measured in the simulator, with the first design (its own screen: delta, time, distance, a bar) 4,400 px and 9
panel windows a second (a whole-box redraw of the same boxes was 18,700 px); on the fused page, at the PR's pace where the
delta flips its colour most seconds, 8,400 px and 17 windows a second (the profile, the gauge and two values added). The approach banner's name and the list's distance and gain come from one
read of the file's header in the pass loop (`Segments::header`: mount, folder lookup and one sector, 3 KB in the sim's pass), the list only with no ride running and for the rows
around the selection. Settings: LIVE SEGMENTS at data 97 (0 off, 1 on), TARGET at 98 (0 to 4); older files read ON and AUTO.
MAIN got a seventh row (SEGMENTS, after COURSES), so it scrolls. A TARGET the file lacks falls back to AUTO.
A climb and a segment at once: the segment drives the one page (its head, profile and distance); the climb's latch counts
the page as opened for it. Times from an hour show as `h:mm:ss` (the
page's time cell is seven characters wide; the card draws it at the smaller size). Input on the card: Start and
Lap are the ride's and never swallowed (the card closes first); Lap held, Page held and Start held go to the ride page after
the screen closes; Page and Lap still held are ignored; while recording a tap, hold or drag is ignored and only a swipe closes.
