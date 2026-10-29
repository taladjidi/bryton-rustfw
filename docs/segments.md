# Live segments: a plan

Status: S1 and S2 built (2026-10-29/30: `src/nav/segment.rs` the reader, `src/nav/segment_match.rs` the matcher, their tests and fuzz; nothing on the unit yet). S3 next.
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

**Memory.** AXI SRAM has about 15 KB left (AGENTS.md, Memory). The index of all segments (id, start lat / lon,
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
- The SEGMENT page opens by itself at the start, as the CLIMB page does at a climb (reuse that mechanism), and
  closes at the finish or when lost, back to the page the rider was on. It shows: the name, the delta (largest), the
  elapsed time, the distance left, a bar with two marks (the rider, the ghost), the target's label. Redraw: the
  delta's and the numbers' own cells only (AGENTS.md, redraw only what changes).
- The result: a banner with the time and the delta, and a result card (time, against each target, the new PR if
  any), closed by any key or after 10 s.
- MENU > NAVIGATION > SEGMENTS: LIVE SEGMENTS on / off; TARGET (PR, KOM, QOM, BEST OF 5, RECENT, CARROT, AUTO: the
  PR if there is one, else the KOM); the list of segments (name, distance, gain), each enabled or not. On / off and
  the target are kept in the settings file (roadmap block 1, step T0); the per-segment enable in the index, written
  as the header's 0x1fd byte between rides.

**Efforts.** At the finish the effort (id, date, time, and its time at each point as u16) is kept in RAM. After the
ride is saved, never before, it is written to `\Rustfw\Segments\<id>.pr` when it beats the PR or there is none
(two copies with a CRC, as the settings file), and `\Rustfw\Segments\efforts.csv` gets a line (id, date, time) for
the computer tool. The `.pr` file becomes the PR target the next time (it wins over the source's PR). A ride
discarded writes nothing. A FIT `segment_lap` message in the ride file: not in v1.

## On the computer: tools/segments.py

Python 3 standard library only, as `tools/sync.py`, and it reuses sync.py's config and Strava token (`sync.json`,
docs/sync.md); the cache and the downloaded segments live in the user's home, never in the repository.

- `segments.py starred`: the owner's starred segments.
- `segments.py near LAT LON --radius-km R --min-km X --min-gain-m Y [--max N]`: the box around the point cut in cells
  of about 5 km; `segments/explore` per cell (riding); duplicates dropped; details fetched; kept when distance >= X
  km and gain >= Y m; at most N, the longest first. The owner's idea: "all segments within R km, longer than X km,
  with more than Y m of ascent".
- For each kept segment: the streams, decimated to 400 points; targets: the PR from the owner's own rides on the
  mounted volume (each FIT in `\Activities` matched with the same rules as the unit, so the PR has real splits
  even without Strava's effort API), the most recent and the best of the last 5 the same way, the KOM / QOM from
  the details at constant pace; the `.pr` files written by the unit win.
- `segments.py write`: the `.seg` files to `\Rustfw\Segments\` on the mounted drive, each verified by reading it
  back (AGENTS.md, the USB link can drop), the ones no longer wanted deleted, at most 128.
- Rate limits: read `X-RateLimit-Limit` / `X-RateLimit-Usage` on each answer, stop below 10 % left and resume from
  the cache on the next run.
- `segments.py selftest` in check.sh, as sync.py's: synthetic segments and rides, the matcher, the writer (a file
  that the Rust reader's tests also read: a fixture generated by the selftest, not a real place).

## Steps (each one merged, through check.sh and a review, before the next)

| Step | What | Agent |
|---|---|---|
| S1 (done 2026-10-29) | `src/nav/segment.rs`: the `.seg` reader (pure, over a byte slice), the checks above; unit tests, doc tests; fuzz (src/sim/fuzz.rs): files generated from the layout (header fields, counts, times, a FIT course), cut, wild values; a count that every check is reached | implementer |
| S2 (done 2026-10-30) | The matcher in `src/nav/segment_match.rs` (decisions below the table): near, start, progress (course.rs's code), lost, finish, the ghost's delta; host tests with synthetic tracks: a clean pass, a start missed by 30 m, a U-turn, a stop, lost and found, two overlapping segments, a segment along a course | implementer (sonnet) |
| S3 | `src/app/segments.rs`: the index at the first pass, the active segment loaded, fed from the GNSS fix each second; nothing written; sim tests over a whole ride on a synthetic segment | implementer (sonnet) |
| S4 | The SEGMENT page, the banners, the result card, the menu; sim screenshots to the gallery; redraw cost measured with the profiler in the simulator | implementer (sonnet) |
| S5 | The efforts written after the save (`.pr`, `efforts.csv`), the per-segment enable byte | critical, then reviewer on opus |
| S6 | `tools/segments.py` (needs the owner's Strava answer above); selftest in check.sh | implementer (sonnet) |
| S7 | Segments from the rider's own rides, no Strava: stretches ridden at least twice (the overlap of two rides' tracks over 1 km), made into `.seg` files by `segments.py own` on the computer first; on the unit later if wanted | implementer (sonnet) |
| S8 | Licensed Live Segments, if Strava grants it: `segments.py live` fills carrot, wolf, most recent, best of 5 and the KOM's real splits from `/live/segments/:id`; later the phone app does the same over BLE | implementer |
| S9 | Optional: stock's imported segments (`\PlanTrip\LiveSegment\`) read too, read only | implementer |

Decided in S2 (2026-10-30), API for S3: `Matcher::new` (about 90 bytes, a field of App), `step_idle(headers, lat, lon, t)`
(an iterator of `&Header`, the index; gives `Approach(id)` or `Start(id)`), then the caller parses the file into the
segment's cell and feeds `step_active(&Segment, lat, lon, t)` (gives `Lost(id)` or `Finish{id, elapsed_s, deltas}`);
`active_id`, `along_m`, `elapsed_s(now)` for the page; `target_s` / `delta_s(seg, column, distance, elapsed)` for the
ghost; `near(header, lat, lon)` is the index's filter. Time `t` is whole seconds of the ride clock. Course.rs's matcher
is shared, not copied: `Course::nearest` became `course::nearest_by` over a closure, and `seg_hit` is the projection
on one step. Choices: the "bounding box" is that of the header's 40 start pairs (the header holds no other point); a
start needs 2 m of movement between two fixes and the closest approach inside the pass (the start time is
interpolated there); lost is 10 s beyond 50 m of the track, or 10 s back more than 50 m behind the furthest point
(a U-turn rides the track the wrong way), or 300 s still or without a fix; the finish is the crossing of the last
point's perpendicular within 25 m of it (stopping short never finishes), its time interpolated between the fixes; the
Approach banner is said once until the rider is beyond 300 m of every start, and not again after a lost segment near
its start. After any Lost or Finish the matcher is idle again and the segment is the caller's to drop.

On the unit after S5 (docs/status.md, "Not yet seen on the unit"): a ride over two known segments with an EMMC_WRITE=1
build: the approach banner, the page opening, the delta against a ride's split, the finish, the `.pr` written after
the save and used as the target on the next pass.
