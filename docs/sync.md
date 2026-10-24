# Sync: rides to Intervals.icu and Strava, workouts back

`tools/sync.py` runs on the computer. It takes the new rides off the bike computer and uploads them, and it puts
the planned workouts of the next days from Intervals.icu on the card. Python 3 and its standard library are all it
needs. An iOS app is to do the same later over the BLE bond (roadmap, phase 8).

## Why two files per ride

Bryton's server refuses a ride whose records carry developer fields (bryton-S510-mods docs/how-it-works.md, "Such
rides do not sync to Bryton": tested on 2026-10-24). Intervals.icu reads DFA a1 from a developer field named
`Alpha1` in those same records. So one file cannot serve both, and this firmware writes two:

| File | Format | Goes to |
|---|---|---|
| `\Activities\YYMMDDhhmmss.fit` | FIT protocol 1.0, no developer data, byte for byte what the recorder wrote before | stock, the Bryton app, Strava |
| `\Rustfw\Activities\YYMMDDhhmmss.fit` | FIT protocol 2.0: the same messages, plus `developer_data_id`, two `field_description`s and in every record `Alpha1` (float32, DFA a1) and `RMSSD` (uint16, ms), invalid while the strap gives no beats | Intervals.icu |

Both are recorded as the ride goes, each by its own recorder with its own checkpoints, recovery records
(`\Rustfw\ride.rec`, `\Rustfw\full.rec`) and save (src/storage/recorder.rs, "Two copies"). The clean one is
always handled first and never waits for the full one. When the full copy fails, the clean one is still saved and
the banner says NO FULL COPY. The two checkpoint together, so after a reset the clean copy never has fewer seconds
than the full one (seen on the unit before: 22 s fewer, the full copy's bigger records having filled its buffer
first). A full copy goes on only with the clean copy's ride, and stops when the card has less than 8 MB free: the
rest is the clean copy's.

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
  them and the WORKOUTS menu lists them (src/storage/workout.rs). A file over 8 KB is skipped, since the unit does
  not show those. Workouts are written only on a mounted volume, because the dev link is read only. Each is written
  to a `.tmp` beside it, flushed to the drive itself (`F_FULLFSYNC`), renamed, then read back; one deleted on the
  unit is written again at the next run. Workouts deleted or moved on Intervals.icu are not removed from the unit.

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
- `--link` reads the rides through the dev link (dev.py `ls` / `get`) without the drive; it uploads, but does not
  write workouts.
- `--days N` sets how many days of workouts to fetch.
- Eject the drive when it is done: the unit takes the card back and restarts if the computer wrote to it.

## Tests

`tools/sync.py selftest` runs a fake Intervals.icu and Strava on 127.0.0.1 against a fake volume of synthetic FIT
files, with no network, and check.sh runs it. It checks that a dry run sends and writes nothing; the full copy goes
to Intervals.icu and the clean one to Strava; a file cut short is refused; a Strava duplicate counts as done; the
token is refreshed and kept 0600; the workout is written in stock's layout; and a second run sends nothing.
`tools/fit_check.py` decodes both copies of a ride with fitdecode and Garmin's FIT SDK (src/storage/fit.rs, module
doc), and `--pair` checks the recorder's two copies against each other.

## Not yet seen

- Not yet run against the real Intervals.icu and Strava, nor with the unit's drive (docs/status.md).
- Stock's app puts a `<name>.json` beside each workout `.fit`. This firmware does not read it; whether stock does
  without it has not been checked.
- Whether Intervals.icu charts `RMSSD` from the developer field is not known; `Alpha1` is the documented one.
