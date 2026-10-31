# Sync: rides to Intervals.icu and Strava, workouts back

`tools/sync.py` runs on the computer. It takes the new rides off the bike computer and uploads them, and it puts
the planned workouts of the next days from Intervals.icu on the card. Python 3 and its standard library are all it
needs. An iOS app is to do the same later over the BLE bond (roadmap, section 9).

## Why two files per ride

Bryton's server refuses a ride whose records carry developer fields (bryton-S510-mods docs/how-it-works.md, "Such
rides do not sync to Bryton": tested on 2026-10-24). Intervals.icu reads DFA a1 from a developer field named
`Alpha1` in those same records. So one file cannot serve both, and this firmware writes two:

| File | Format | Goes to |
|---|---|---|
| `\Activities\YYMMDDhhmmss.fit` | FIT protocol 1.0, no developer data, byte for byte what the recorder wrote before | stock, the Bryton app, Strava |
| `\Rustfw\Activities\YYMMDDhhmmss.fit` | FIT protocol 2.0: the same messages, plus `developer_data_id`, three `field_description`s and in every record `Alpha1` (float32, DFA a1), `RMSSD` (uint16, ms) and `ArtPct` (uint8, %), invalid while the strap gives no beats; and every beat the strap sent in standard `hrv` messages | Intervals.icu |

Both are recorded as the ride goes, each by its own recorder with its own checkpoints, recovery records
(`\Rustfw\ride.rec`, `\Rustfw\full.rec`) and save (src/storage/recorder/mod.rs, "Two copies"). The clean one is
always handled first and never waits for the full one. When the full copy fails, the clean one is still saved and
the banner says NO FULL COPY. The two checkpoint together, so after a reset the clean copy never has fewer seconds
than the full one (seen on the unit before: 22 s fewer, the full copy's bigger records having filled its buffer
first). A full copy goes on only with the clean copy's ride, and stops when the card has less than 8 MB free: the
rest is the clean copy's.

**Beat to beat.** The full copy also holds every RR interval the strap sent, raw (before the live DFA a1's 5 %
artifact rule) but within 250 to 2000 ms (the decoder drops what is outside as glitches; beats it could not time,
over several missed broadcasts, and a strap's drop-outs leave gaps), as Garmin's units write them: standard `hrv` messages (global 78, field `time`, five intervals a
message in ms, the unused slots invalid), between the records as the beats come (src/storage/fit.rs, "Beat to
beat"). Intervals.icu keeps the original file, so tools that compute DFA a1 from a ride's beats (Domestique,
Runalyze, Kubios) can check the live value against theirs. They go through the same buffer, checkpoints and
recovery records as the records (a checkpoint after a minute of beats while paused too); beats that find no room
(a card failing) wait in a queue of 40 and past it are dropped and counted, never holding the recording up. About
20 KB an hour at 150 bpm. The clean copy has none: no file shows that Strava, stock or Bryton's server take hrv
messages from this firmware (a stock firmware patch, bryton-S510-mods P8, wrote them into stock's files; bryton-S510-mods' docs do not say whether those
synced to Bryton or Strava), so it stays as it was.

**Artifacts.** `ArtPct` is the share of the beats in the live DFA a1's 2-minute window that its 5 % rule dropped.
FITNESS's HRVT1 / HRVT2 leave out a record over 5 % (src/training/hrvt.rs; Rogers et al. 2021, Sensors 21:821: DFA a1
biased from 1 to 3 % of missed beats on).

**Why not make one file from the other at the save?** The numbers come from `cargo test-host two_copies_cost`, a
15-minute ride on the synthetic volume, once the folders exist:

- one copy: 190 sectors written;
- two copies as the ride goes: 420 sectors, which is +15 sectors (7.5 KB) a minute, or about 1.4 MB for a 3-hour
  ride. That is nothing next to the eMMC's endurance, and a few milliseconds of card time a minute;
- deriving the clean copy at the save would write about 280 sectors (the full copy as the ride goes, then the
  clean file written once at the save, about 22 KB for 15 minutes), so about a third less.

The two live copies were kept anyway. Deriving at the save would put a FIT rewriter into the save, which is the
riskiest moment (AGENTS.md). It would also make the clean file, the one stock and Bryton read, depend on the code
that writes developer fields, and a 3-hour save would read about 390 KB and write about 330 KB more.

## What goes where

- Strava gets the clean copy of each ride in `\Activities`: this firmware's rides and stock's.
- Intervals.icu gets the full copy when there is one, else the clean one (stock's rides, or a ride whose full copy
  failed).
- Each file's FIT CRCs are checked before it is sent, so a copy that the USB link cut short is never uploaded.
- A full copy goes only if it begins with the clean copy's `file_id` (the ride's start): a full copy of another
  ride under the same name is never sent in its place.
- `\Activities\tmp.fit` (stock's ride in progress), names that are not `YYMMDDhhmmss.fit`, and rides older than
  `since` are left alone.
- Never twice: `state.json` remembers each upload by ride name, with the file's name, size and CRC-32. Intervals.icu
  also refuses a file it already has, by a hash of its bytes, and answers 200 instead of 201. Strava answers
  "duplicate of activity N", which is then remembered as done.
- Workouts: each Intervals.icu event of category `WORKOUT` from today to N days on is fetched as FIT
  (`/events/{id}/download.fit`) and written to `\Workout\calendar\YYYYMMDD\icu<id>.fit`, where stock's app puts
  them and the WORKOUTS menu lists them (src/storage/workout_file.rs). A file over 8 KB is skipped, since the unit does
  not show those. Workouts are written only on a mounted volume, because the dev link is read only. Each is written
  to a `.tmp` beside it, flushed to the drive itself (`F_FULLFSYNC`), renamed, then read back; one deleted on the
  unit is written again at the next run. Workouts deleted or moved on Intervals.icu are not removed from the unit.

## Indoor rides

A ride recorded with the INDOOR profile (src/app/profile.rs: chosen at Start when a trainer is heard, or set by
hand) has sport cycling (2) and sub_sport indoor_cycling (6) in its laps and session; a road ride has road (7), as
stock does with its bike type (`Fit_MapBikeTypeToSubSport`, Ghidra notes agent_r9.md). Not virtual_activity (58):
this is a ride on a trainer, not in a virtual world.

- **Strava** makes a FIT ride with sub_sport indoor_cycling a Ride, not a VirtualRide (only virtual_activity, 58,
  gives a VirtualRide: forum.intervals.icu/t/indoor-outdoor-ftp-usage/6328, a user who tried both; not documented by
  Strava, which says only that the sport is detected from the file). So sync.py reads the session's sub_sport and
  sends indoor_cycling (and virtual_activity) rides with the upload's `trainer=1`, "whether the resulting activity
  should be marked as having been performed on a trainer" (developers.strava.com/docs/reference, createUpload).
  Without a position, such a ride is matched to no segment.
- **Intervals.icu** sets an activity's trainer (indoor) flag itself when the sub_sport's name contains INDOOR, or
  the type is virtual, or a ride has no GPS; virtual_activity makes it a VirtualRide (its developer's code quoted
  in the same forum thread, 2022-01-24). Its indoor FTP then applies. Nothing to send: the file says it.

## Setup (once)

1. **Intervals.icu:** Settings, then "Developer Settings": copy the athlete id (`i12345`) and generate an API key.
2. **Strava:** at https://www.strava.com/settings/api, create an application. Any name and website will do; set
   **Authorization Callback Domain** to `localhost`. Copy its Client ID and Client Secret.
3. Write `~/.config/bryton-rustfw/sync.json` (outside the repository: never commit it) and make it private:

   ```json
   {
     "intervals": {"athlete_id": "i12345", "api_key": "..."},
     "strava": {"client_id": "12345", "client_secret": "..."},
     "volume": "/Volumes/BRYTON",
     "since": "2026-10-24",
     "days": 7
   }
   ```

   ```sh
   chmod 600 ~/.config/bryton-rustfw/sync.json
   ```

   Leave out the service you do not want. `since` is the first ride date to upload: set it to the day you start,
   or the whole history of stock's rides would go to Strava, where the Bryton app may have put them already. Without
   it, the last 14 days are uploaded. `INTERVALS_API_KEY` in the environment overrides `api_key`.
4. Run `tools/sync.py strava-login`. It opens Strava's authorization page and catches the answer on
   `http://127.0.0.1:8721/` (Strava accepts `localhost` and `127.0.0.1` whatever the callback domain); allow upload
   access. Only the answer with this login's random `state` is taken. The tokens go to `~/.config/bryton-rustfw/strava.json` (mode
   0600) and are refreshed by themselves.

## Each time

Plug the bike computer in and mount the drive: this firmware's USB drive (DRIVE in the menu) or stock's. Then:

```sh
tools/sync.py            # dry run: what would be uploaded and written
tools/sync.py --go       # do it
tools/sync.py status     # what is configured, how many rides were uploaded
```

- `--volume PATH` takes the volume from somewhere other than `/Volumes/BRYTON`.
- `--link` reads the rides through the dev link (tools/dev.py `ls` / `get`) without the drive; it uploads, but does not
  write workouts.
- `--days N` sets how many days of workouts to fetch.
- Eject the drive when it is done: the unit takes the card back and restarts if the computer wrote to it.

## Tests

`tools/sync.py selftest` runs a fake Intervals.icu and Strava on 127.0.0.1 against a fake volume of synthetic FIT
files, with no network, and check.sh runs it. It checks that a dry run sends and writes nothing; the full copy goes
to Intervals.icu and the clean one to Strava; a file cut short is refused; a Strava duplicate counts as done; the
token is refreshed and kept 0600; the workout is written in stock's layout; a second run sends nothing; and only the
indoor ride (session sub_sport 6, read past a compressed header) goes to Strava with `trainer=1`.
`tools/fit_check.py` decodes both copies of a ride with fitdecode and Garmin's FIT SDK (src/storage/fit.rs, module
doc), `--pair` checks the recorder's two copies against each other, and `--profile` the sport and sub_sport of an
indoor ride cut and resumed (indoor_cycling) and of a ride resumed from an older recovery record (road).

## Intervals.icu and Strava activities

Activities Intervals.icu imported from Strava are "not available via the API" (Strava's November 2024 terms), so apps
that read Intervals.icu's API (Domestique, for one) only see rides uploaded as files, which is what `sync.py` does.
`sync.py` also sends a User-Agent: Cloudflare, in front of Intervals.icu, refused Python's default (error 1010).

## The device the files name

Both copies carry FIT manufacturer 267 (Bryton) and product 2404 (the Rider S510), as stock's own ride files do, so
Intervals.icu, Strava and Domestique show the rides as from a Bryton Rider S510 ("BRYTON 2404" on Intervals.icu) and
treat its altitude as barometric. Rides recorded before 2026-10-26 carry manufacturer 255 (development) and show as
"DEVELOPMENT".

## AssistNow Offline (`tools/assist.py`)

Helps the GNSS receiver find its first fix sooner (docs/features.md, BIKE > GNSS). One-time setup: a free
u-blox Thingstream account gives an AssistNow token. Keep it outside the repository: in the environment
(`UBLOX_ASSISTNOW_TOKEN`) or in `~/.config/bryton-rustfw/assist.json` (`{"token": "..."}`, `chmod 600`). Then, with
the unit mounted as a drive:

    python3 tools/assist.py            # downloads a week of predictions for GPS, Galileo and BeiDou (the receiver's default set), copies to \Rustfw\mga.ubx

Add `--gnss gps,gal,glo` only with BIKE > GNSS set to GPS+GAL+GLO. The file is kept in date order (the firmware stops reading at a date past today). It lasts the week; run it again from time to time (weekly is enough). The token is never printed or written to the
unit. Stock's own file is `\System\mgaoffline.ubx`, which its phone app downloads; this firmware reads its own copy and
never touches stock's.

## Not yet seen

- Not yet run against the real Intervals.icu and Strava, nor with the unit's drive (docs/status.md).
- Stock's app puts a `<name>.json` beside each workout `.fit`. This firmware does not read it; whether stock does
  without it has not been checked.
- Whether Intervals.icu charts `RMSSD` or `ArtPct` from the developer fields is not known; `Alpha1` is the
  documented one. Its use of the hrv messages (its own HRV charts) has not been seen either.
- An indoor ride on Strava (a Ride with the trainer mark) and on Intervals.icu (the indoor flag) (docs/status.md).
