//! The ride recorder (AGENTS.md, "Golden rule: never lose a ride"): the ride as a FIT file ([`fit::Writer`]) on
//! stock's FAT volume (`fat::write`), a checkpoint at least every minute, two recovery records, a resume after any
//! reset or power cut, and a save that cannot lose the ride.
//!
//! **Files** (AGENTS.md, "Share the storage with stock"): the ride in progress is `\Rustfw\Rides\YYMMDDhhmmss.fit`
//! (local start time, stock's name) and its recovery records `\Rustfw\ride.rec`, in this firmware's own folder, so
//! neither firmware resumes the other's ride; a saved ride is `\Activities\YYMMDDhhmmss.fit`, where stock, its app,
//! the history and a computer find it. Stock's `\Activities\tmp.fit` and `\System\tmp*.fit` are never touched.
//! The ride's entry is rewritten at every checkpoint (its size), `ride.rec`'s only when it is made: in two folders,
//! they never share a directory sector, so one torn by a cut cannot take both.
//!
//! **RAM, not the card, each second.** A record is about 30 bytes, appended to a [`TAIL`]-byte buffer that holds the
//! file from the sector the next byte goes in. The card is written at a checkpoint only: every [`CHECKPOINT_S`]
//! seconds of ride, at a pause, at the end, at switching off, and earlier when the buffer has less than [`LOW`]
//! bytes left (a minute of every field is about 1.9 KB with the sector carried over). So the card sees about four
//! commands a minute and a power cut loses at most the last minute, as stock (ghidra/STORAGE_AND_MAPS.md 3: a
//! checkpoint every 60 samples). Writing each sector as it fills would not shorten that (the file and its size are
//! only valid at a checkpoint) and would cost more commands.
//!
//! **A checkpoint**, in this order: the FIT trailer and header in the buffer ([`fit::Writer::checkpoint`]); the
//! new sectors (clusters taken first, `fat::write`); sector 0 with the new header; the size in the directory
//! entry; then the recovery record. A cut before the record leaves the previous record, whose file is the bytes
//! up to its length, which the writes after it never changed except two sectors rewritten in place: the one its
//! 2-byte trailer is in and sector 0 (the header). The previous record holds both as they were (below), and the
//! resume writes them again: so a cut that tears either (garbage in the sector being written, on a card without
//! reliable write: emmc.rs) loses nothing either.
//!
//! **The recovery record** is a slot of three sectors of `ride.rec`, the two slots written in turn (a cut while
//! writing one leaves the other whole), in one write: the ride file's sector 0 and the sector its trailer ends
//! in, as the checkpoint left them; then the record: a tag (its layout version), a sequence number (the larger
//! valid one wins: never the data length, so a finished ride's record cannot pass for a new ride's), the phase
//! (done, starting, recording, saving), the file's identity (its name and first cluster: a resume opens only that
//! file and checks both), the FIT writer's state ([`fit::RECORD_LEN`] bytes, its own CRC) and the app's ride totals
//! (`ride::Ride::to_words`, their count checked); the sector's CRC last.
//!
//! **The save** (End), each step done again after a reset by [`Recorder::recover`] from the phase in the record:
//! 1. the end messages and a checkpoint whose record says "saving";
//! 2. the file read back whole from the card: its FIT CRC must be right. A read that fails is tried again; a file
//!    that reads but is not whole (a damaged sector) is left as it is in `\Rustfw\Rides` and the record says
//!    "done" ([`Outcome::Kept`]): it is never tried again, so it never stops the next ride;
//! 3. `\Activities` made if missing and a free name found (a name taken by another file: a second later; the full
//!    copy, saved after the clean one, starts from the name that one took, so the two keep one name), and room made
//!    there for the entry ([`Fat::make_room`]: a zeroed cluster linked if the folder must grow), so that 5 takes no
//!    cluster: a full card fails here, the ride still named where it is;
//! 4. its entry in `\Rustfw\Rides` marked deleted (0xe5 only: `fat::write`);
//! 5. the new entry `\Activities\YYMMDDhhmmss.fit` on the same chain;
//! 6. a record "done".
//!
//! Between 4 and 5 the chain is named by no entry for a moment (a lost chain, never a cross-link); the record
//! names it, and 5 is done again at the next start. So the ride in progress exists until the saved one does.
//!
//! **A save given up** ([`GIVE_UP`] card errors, or at once for one no retry mends) never writes "done" over the
//! only record that may name the ride: the ride is first named where it was recorded (`Recorder::keep_in_rides`:
//! its entry there as it is, or, gone (step 4 done, by this run or before a cut), linked again under its name in the
//! room step 3 made; unless the save's own link is there, which is the save done), then the record "done":
//! [`Outcome::Kept`]. Whoever gives up (the same run, the boot, a Start finding the save in the record) does it
//! that way. Only if that fails too is the save stuck ([`Recorder::stuck`]): it stays "saving" (the ride open: the
//! marker kept, the drive lent read only), tried again with a back-off up to 160 s, and no new ride's record goes
//! over its record. The read-back (2) is done once a run, not at every retry. A boot that cannot read the record
//! (the card not mounting) never concludes either: it may name a save cut after 4, so it is tried again, stuck
//! past its retries; and a ride left open whose entry is gone is a save from the moment that is seen.
//!
//! **GIVE UP SAVE** (SETTINGS > SYSTEM, only while stuck: [`Recorder::give_up_save`]) is the owner's way out on a
//! card that keeps failing, after copying it (the drive lent read only): the ride named where it was recorded
//! (`keep_in_rides`, once more); else under a free name in `\Rustfw\Rescue` (`Rescue\Full` for the full copy),
//! only if neither folder that may name the chain does (both read whole: never a second entry on it) and the card
//! has [`FULL_RESERVE`] free (the folder made or grown takes a cluster), the link, then its record "done" (a cut
//! between leaves the record "saving": the next save, a boot's or a start's, finds the chain named in the rescue
//! folder and only writes the record "done", never a second entry; the review of 2026-10-29, S6, where the
//! opposite order lost the ride to a cut at the link); else the record "done" alone, the chain left lost (a computer's
//! fsck may make it `FILE0000.CHK`: a lost chain, nothing cross-linked). Either way the stuck state ends: a ride held
//! behind it starts, and the drive is lent writable again.
//!
//! **A Start during a save** (its retries take up to a minute, or for good when stuck): the new ride is kept and
//! started once the save is over ([`Recorder::start`]), its timer stopped from its start to its first second
//! recorded; behind a stuck save the app says the new ride is not recorded ([`Recorder::held`]). Its end drops it
//! (the last ride, still in the recorder, is never the one ended or discarded). The full copy's ended ride puts its
//! end on the card ("saving") at once and waits only for its save.
//!
//! **A discarded ride** (the end-ride screen's DISCARD: [`Recorder::discard`]) is saved the same way, step for step,
//! into `\Rustfw\Discarded` (the full copy into `\Rustfw\Discarded\Full`) instead of the saved folder: never
//! erased, recoverable from a computer, and seen by no history, totals, curve or sync (they read `\Activities`
//! and `\Rustfw\Activities` only). Every record from the end's on says so (its byte 22), so a save
//! finished after a cut goes to the same folder: the ride ends up in `\Rustfw\Rides` or in `\Rustfw\Discarded`,
//! never in `\Activities`. A record of a build before it has 0 there: a ride saved. Its names are looked for in
//! both folders at the start too, so the two copies keep one name there as well, and both folders are made at the
//! start (`Recorder::setup`), so the discard's save writes no directory sector of `\Rustfw` (where `ride.rec`'s
//! entry is). One case on a card without reliable write only: a torn sector that takes the ride's entry in the end's
//! checkpoint, before its record (the first to say "discard"), has the ride saved whole into `\Activities` from its
//! last record, as any ride whose entry a cut tore; in one place still, never lost.
//!
//! **The app's files in `\Rustfw`** ([`KEPT`]: the pages, the settings, the phone's bond, the altitude, the
//! trainer, the trash's record, and `\Rustfw\Deleted\Full`) are made zeroed at a ride's start too, the last ride
//! done (`make_kept`): later writes then go inside their own sectors, never to `\Rustfw`'s directory sector, which
//! holds `ride.rec`'s and the ride folders' entries (a torn write there would lose every kept ride: the review of
//! 2026-10-29, S7). Each reader takes zeros as "none"; each writer still makes its file if missing. Tried once a
//! boot: one that fails (a name taken by a folder, a full card) is left to the writers until the next boot.
//!
//! **One writer, one ride.** What the last run left unfinished is finished before a new ride: by the boot
//! ([`Recorder::recover`]), or, if the boot did not (its marker lost, or the card failed then), at the new ride's
//! first card work: a ride in progress is resumed in its place if the marker said it was open or it stopped less
//! than [`RESUME_S`] ago, else ended at its last record and saved; a save is finished. `recover` does nothing
//! once a ride has started.
//!
//! **After a cut** the second FAT may differ from the first in the sector being written (or that sector torn) and
//! FSInfo's free count may be a few clusters high: `fat::Fat::sync_fats` runs whenever a record is found
//! unfinished, and when the folders are made (a cut there leaves no record). A ride whose entry the cut tore is
//! saved from its record (its chain and length), not resumed.
//!
//! That sync reads both FATs whole in one call (about 1.5 to 3 s on the unit's 15 MB, estimated: the perf review of
//! 2026-10-25), and it stays in one call on purpose. It runs only at the first ride on a card and after a cut, the
//! ride's seconds missed meanwhile are caught up ([`crate::MAX_CATCH_UP`]: 10 s), and it must come before any chain is
//! read or written. Spread over passes (`Fat::sync_fats_from`, as the scan's cache does) it would hold the first
//! checkpoint back by seconds and put a resumable cursor, and a state per sync position for the power-cut tests to
//! cover, into the one path that must never lose a ride: a pause the rider sees twice per card is the smaller cost.
//!
//! **Two copies of each ride** (docs/sync.md): Bryton's server refuses a ride whose records hold developer fields,
//! and Intervals.icu reads DFA a1 from one (`Alpha1`), so one file cannot serve both. Two recorders run side by side
//! ([`Kind`]), each with everything above of its own: the clean one (protocol 1.0, no developer data: what stock,
//! Bryton and Strava take, byte for byte what it was) in `\Rustfw\Rides`, `\Rustfw\ride.rec` and `\Activities`; the
//! full one (`fit::Writer::begin_with` `dev`: `Alpha1`, `RMSSD` and `ArtPct` in every record, the strap's beats
//! in `hrv` messages: [`Recorder::beat`]) in `\Rustfw\Full`,
//! `\Rustfw\full.rec` and `\Rustfw\Activities`, same name. The app drives the clean one first in each pass, so the
//! full one never stands between the clean one and the card, and a failing full one is only said
//! (`App::step_recorder`). Three rules keep the full one from ever costing the clean one (the review of 2026-10-24):
//! the two checkpoint together ([`Recorder::keep_up`]: the full copy's bigger records used to run its buffer low
//! first, and the clean one was 22 s behind it after a reset on the unit); the full one waits for the clean one to
//! make its file and carries on a ride left open only if it is the clean one's ([`Recorder::io_after`],
//! [`Recorder::resumed_after`]: never a full copy of one ride carried on with another's seconds); and it stops when
//! the card has less than [`FULL_RESERVE`] free (FSInfo's count, counted once when it is unknown; no FSInfo: no
//! full copy), the rest being the clean one's. Writing both as the ride goes was chosen over making one from the
//! other at the save: the save, the riskiest moment, stays as short as it was, and the clean file never depends on
//! code that writes developer fields. Cost (`two_copies_cost` below, a 900 s ride): about twice the sectors
//! written, a few more a minute; nothing near the eMMC's endurance.
//!
//! Errors: nothing here panics; a failing card leaves the data in RAM and is tried again after [`RETRY`] calls;
//! samples that do not fit the buffer meanwhile are counted ([`Recorder::dropped`]), never the file lost. Only a
//! card error is tried again, and a save or a ride left unfinished only [`GIVE_UP`] times in a row: then it is
//! left as it is ([`Outcome::Kept`]), so that the next ride records. A checkpoint refused for a reason a retry
//! cannot mend (the card full, or `Corrupt`) stops the ride's recording ([`Outcome::Stopped`]): the file is saved as
//! its last checkpoint left it, which takes no new cluster, so a full card never leaves a ride open for good (nor the
//! card unlendable to the computer that would free it).
#![cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::cast_possible_truncation))]

use crate::fat::write::{Handle, dos_time};
use crate::fat::{DiskWrite, Error, Fat, File, SECTOR, SECTOR_U32};
use crate::fit::{self, Sample, Sink, Writer};
use crate::ride::RIDE_WORDS;

mod rec;
use rec::{Rec, file_name};
pub use rec::{next_name, ride_name};
// the record keeps the count of ride words in a byte (byte 21)
const _: () = assert!(RIDE_WORDS < 256);

/// This firmware's folder, its recovery records, the folder of the ride in progress in it, stock's rides folder.
pub const DIR: &str = "Rustfw";
/// See [`DIR`].
pub const REC: &str = "ride.rec";
/// See [`DIR`].
pub const RIDES: &str = "Rides";
/// See [`DIR`].
pub const ACTIVITIES: &str = "Activities";
/// `\Rustfw\Discarded`: the rides discarded at their end (module doc), their full copies in `Full` under it.
pub const DISCARDED: &str = "Discarded";
/// `\Rustfw\Discarded` and `\Rustfw\Discarded\Full` as paths.
pub const DISCARDED_PATHS: [&str; 2] = ["Rustfw/Discarded", "Rustfw/Discarded/Full"];

/// Which copy of the ride a recorder writes (module doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Protocol 1.0, no developer data: `\Rustfw\Rides`, `\Rustfw\ride.rec`, then `\Activities`.
    Clean,
    /// Developer fields (`Alpha1`, `RMSSD`): `\Rustfw\Full`, `\Rustfw\full.rec`, then `\Rustfw\Activities`.
    Full,
}

impl Kind {
    /// Its recovery records' file in [`DIR`].
    fn rec(self) -> &'static str {
        match self {
            Kind::Clean => REC,
            Kind::Full => "full.rec",
        }
    }

    /// The folder of the ride in progress in [`DIR`], and its path.
    fn rides(self) -> (&'static str, &'static str) {
        match self {
            Kind::Clean => (RIDES, "Rustfw/Rides"),
            Kind::Full => ("Full", "Rustfw/Full"),
        }
    }

    /// The saved rides' folder.
    fn saved(self) -> &'static str {
        match self {
            Kind::Clean => ACTIVITIES,
            Kind::Full => FULL_PATH,
        }
    }

    /// The saved rides' folder, made if missing: `\Activities` in the root, `\Rustfw\Activities` in [`DIR`].
    fn saved_dir(self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<File, Error> {
        let parent = match self {
            Kind::Clean => fat.root(),
            Kind::Full => fat.ensure_dir(d, fat.root(), DIR, t)?,
        };
        fat.ensure_dir(d, parent, ACTIVITIES, t)
    }

    /// The discarded rides' folder: `\Rustfw\Discarded`, or `Full` in it.
    fn discarded_path(self) -> &'static str {
        match self {
            Kind::Clean => DISCARDED_PATHS[0],
            Kind::Full => DISCARDED_PATHS[1],
        }
    }

    /// Where GIVE UP SAVE names a stuck save's ride: `\Rustfw\Rescue`, or `Full` in it.
    fn rescue_path(self) -> &'static str {
        match self {
            Kind::Clean => RESCUE_PATHS[0],
            Kind::Full => RESCUE_PATHS[1],
        }
    }

    /// Made if missing, at [`Recorder::setup`] only.
    fn discarded_dir(self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<File, Error> {
        let mine = fat.ensure_dir(d, fat.root(), DIR, t)?;
        let dir = fat.ensure_dir(d, mine, DISCARDED, t)?;
        match self {
            Kind::Clean => Ok(dir),
            Kind::Full => fat.ensure_dir(d, dir, "Full", t),
        }
    }
}

/// `\Rustfw\Rescue` and `\Rustfw\Rescue\Full`: where GIVE UP SAVE names a stuck save's ride when its rides folder
/// cannot (module doc).
pub const RESCUE_PATHS: [&str; 2] = ["Rustfw/Rescue", "Rustfw/Rescue/Full"];

/// What GIVE UP SAVE ([`Recorder::give_up_save`]) did with a stuck save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GaveUp {
    /// Named by `keep_in_rides` after all: `Kept` (in its rides folder), or `Saved` / `Discarded` (the save's own
    /// link found there).
    Named(Outcome),
    /// Named in [`RESCUE_PATHS`] under that name.
    Rescue([u8; 12]),
    /// Named nowhere (the card or the record unreadable): the chain left lost, the card to be copied.
    Lost,
}

/// The app's files in [`DIR`] and their sizes, made zeroed by `make_kept` (module doc): pages.bin and settings.bin
/// (two copies: app/editor.rs `write_copy`), ble.bin, altcal.bin, sensors.bin, delete.rec (one sector each).
pub const KEPT: [(&str, u32); 6] = [
    (crate::page::FILE, 2 * SECTOR_U32),
    (crate::SETTINGS_FILE, 2 * SECTOR_U32),
    (crate::BOND_FILE, SECTOR_U32),
    (crate::ALT_FILE, SECTOR_U32),
    (crate::sensors::SENSORS_FILE, SECTOR_U32),
    (crate::trash::REC, SECTOR_U32),
];

/// [`KEPT`] made where missing or not their size (zeros where they were shorter), then `\Rustfw\Deleted\Full`; the
/// FATs synced first when anything is to be made, as in [`Recorder::setup`] (a cut while making them leaves no record
/// to say so). Reads only when all are there.
fn make_kept(fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<(), Error> {
    let dir = fat.find(d, fat.root(), DIR)?;
    let mut whole =
        fat.find(d, dir, crate::trash::DELETED).and_then(|del| fat.find(d, del, "Full")).is_ok_and(|f| f.dir);
    for (name, size) in KEPT {
        whole &= fat.locate(d, dir, name).is_ok_and(|h| h.size == size);
    }
    if whole {
        return Ok(());
    }
    fat.sync_fats(d)?;
    for (name, size) in KEPT {
        let mut h = match fat.locate(d, dir, name) {
            Ok(h) => h,
            Err(Error::NotFound) => fat.create(d, dir, name, t)?,
            Err(e) => return Err(e),
        };
        if h.size != size {
            // shorter: never whole for its reader (each writes its sectors, then the size)
            for k in h.size.div_ceil(SECTOR_U32)..size / SECTOR_U32 {
                fat.write_sectors(d, &mut h, k, &[0; SECTOR])?;
            }
            fat.set_size(d, &mut h, size, t)?;
        }
    }
    let deleted = fat.ensure_dir(d, dir, crate::trash::DELETED, t)?;
    fat.ensure_dir(d, deleted, "Full", t)?;
    Ok(())
}

/// `\Rustfw\Activities`: the full copies of the saved rides ([`Kind::Full`]).
pub const FULL_PATH: &str = "Rustfw/Activities";
/// Free bytes (FSInfo's count) under which the full copy stops: the rest is the clean copy's. A day of riding is
/// about 3 MB of clean copy (a record about 33 bytes a second).
pub const FULL_RESERVE: u64 = 8 << 20;
/// Sectors of `ride.rec`: two slots of three (module doc).
const REC_SECTORS: u32 = 6;
/// Card failures in a row after which a save, or a ride left by the last run, is given up (kept where it is): a
/// minute of [`RETRY`]s. Only `Io` is tried again at all: any other error comes from what is on the card.
pub const GIVE_UP: u32 = 12;
/// A ride left unfinished whose last record is at most this old when a new one starts is resumed instead (the
/// boot's marker lost); an older one is saved as it is and the new ride starts.
pub const RESUME_S: u32 = 3600;
/// Bytes of file buffered in RAM.
pub const TAIL: usize = 2048;
/// Seconds of ride between checkpoints at most.
pub const CHECKPOINT_S: u32 = 60;
/// A checkpoint is due when the buffer has less than this left (room for some seconds more if it fails).
pub const LOW: usize = 512;
/// Calls of [`Recorder::io`] to wait after an error before trying the card again.
pub const RETRY: u32 = 100;
/// Bytes a record message takes at most, with a new definition (and its developer fields) and the trailer after it.
const RECORD_MAX: usize = 6 + 3 * 10 + 10 + 1 + 27 + 7 + 2;
/// RR intervals waiting for an `hrv` message ([`Recorder::beat`]): 16 s at 150 bpm, while the buffer has no room.
pub const BEATS_QUEUED: usize = 40;
/// Record tag and layout version: change it with any change of [`Rec`]'s layout or of `ride::Ride::to_words`
/// (the record also holds [`RIDE_WORDS`], byte 21, checked).
const TAG: [u8; 4] = *b"RFR2";
const _: () = assert!(128 + 4 * RIDE_WORDS <= SECTOR - 2 && RIDE_WORDS < 256);

/// Local time: year, month, day, hour, minute, second.
pub type Local = (u16, u8, u8, u8, u8, u8);

/// Where the recording is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No ride (or saved).
    Done = 0,
    /// Started: the file is being made.
    Starting = 1,
    /// Recording.
    Recording = 2,
    /// Ended: being saved.
    Saving = 3,
}

/// What [`Recorder::io`] or [`Recorder::recover`] did that the app should know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to tell.
    Nothing,
    /// A ride in progress was found and reopened at its last checkpoint: its totals and name (its start).
    Resumed([u32; RIDE_WORDS], [u8; 12]),
    /// A ride was saved as `\Activities\<name>.fit`.
    Saved([u8; 12]),
    /// A ride discarded was moved to `\Rustfw\Discarded\<name>.fit` ([`Recorder::discard`]).
    Discarded([u8; 12]),
    /// A ride could not be saved (its file read back damaged, or the card failed [`GIVE_UP`] times in a row) and is
    /// left as it is, `\Rustfw\Rides\<name>.fit` as a rule, so that the next ride can be recorded.
    Kept([u8; 12]),
    /// The card failed (tried again later).
    Failed(Error),
    /// The card refused a checkpoint for a reason a retry cannot mend: `Refused` (no free cluster: the card full)
    /// or `Corrupt`. The ride stops being recorded; what reached the card is saved as its last checkpoint left it
    /// (`Saved` follows), or, before its first checkpoint, nothing was.
    Stopped(Error),
}

/// The file's trailer (FIT CRC), `size` bytes long, written again where it ends in the sector after the one a
/// record keeps (the sector its trailer starts in): that sector was the next checkpoint's, and a cut after it
/// leaves the next data there. Found by the beats' power-cut test (a torn directory sector, a file of 4609 bytes):
/// the ride was kept unsaved, its CRC wrong by that byte. A resume rewrites the trailer anyway (its checkpoint);
/// this is for the saves from the chain as it is.
fn trailer_end(
    fat: &mut Fat,
    d: &mut impl DiskWrite,
    file: &mut Handle,
    size: u32,
    trailer: [u8; 2],
) -> Result<(), Error> {
    let (start, end) = (size.saturating_sub(2) / SECTOR_U32, size.saturating_sub(1) / SECTOR_U32);
    if start == end {
        return Ok(());
    }
    let mut s = [0u8; SECTOR];
    fat.read_sector(d, file, end, &mut s)?;
    s[0] = trailer[1];
    fat.write_sectors(d, file, end, &s)
}

/// The ride being recorded: the FIT writer, the RAM buffer of the file's tail, and where the file is.
pub struct Recorder {
    kind: Kind,
    phase: Phase,
    writer: Writer,
    /// The ride file, once made; the recovery records' file.
    file: Option<Handle>,
    rec: Option<Handle>,
    seq: u32,
    name: [u8; 12],
    /// The file from byte `tail_off` (a sector's start) on; the file is `len` bytes.
    tail: [u8; TAIL],
    tail_off: u32,
    len: u32,
    /// The file's length at the last checkpoint (what the card holds), and its trailer then; the card refused a
    /// checkpoint ([`Outcome::Stopped`]): the save is of that length.
    on_card: u32,
    on_card_trailer: [u8; 2],
    stopped: bool,
    /// The FIT header (bytes 0..14), kept for rewriting sector 0.
    head: [u8; 14],
    /// Seconds of ride since the last checkpoint; a checkpoint is wanted now; the end is written in the buffer.
    since: u32,
    want: bool,
    ended: bool,
    /// The app's ride totals at the last second; the last record's time.
    ride: [u32; RIDE_WORDS],
    last_rec: u32,
    /// The full copy, resumed: the seconds up to here are in its file already (its checkpoint may be later than the
    /// clean one's, which the app goes on from), so they are not written twice.
    skip_to: u32,
    /// Calls to wait before trying the card again; card failures in a row.
    wait: u32,
    fails: u32,
    /// The boot's marker said a ride was open: a ride left unfinished is resumed whatever its age ([`RESUME_S`]).
    resume_old: bool,
    /// The ride ended is discarded: saved into `\Rustfw\Discarded` (module doc).
    discard: bool,
    /// The full copy's RR intervals not yet in an `hrv` message, oldest first ([`Recorder::beat`]).
    beats: [u16; BEATS_QUEUED],
    beats_n: usize,
    /// The beats written since the last checkpoint, ms: a minute of them wants one, paused too (no records then).
    beats_ms: u32,
    /// Samples (or laps, pauses) that did not fit in the buffer.
    pub dropped: u32,
    /// RR intervals dropped: the queue full (the buffer without room for long), or left at the end.
    pub beats_dropped: u32,
    /// Card errors so far, every one, those the recorder goes on after included (never lowered: the app compares
    /// the FAT copies again when it grows, a FAT sector may have reached one copy only).
    pub errors: u32,
    /// A Start while the last ride was still being saved: (time, name, resume_old), started once the save is done
    /// ([`Recorder::start`]).
    pending: Option<(u32, [u8; 12], bool)>,
    /// The last save: the ride's name and the name it was saved under (a second later when one was taken), which the
    /// full copy's save takes too ([`Recorder::io_after`]); and that name, for this copy's save.
    saved: Option<([u8; 12], [u8; 12])>,
    save_as: Option<[u8; 12]>,
    /// The ride being saved read back whole in this run (its step 2): not read again at each retry.
    checked: bool,
    /// A save that could neither finish nor put the ride back in its rides folder ([`Recorder::keep_in_rides`]):
    /// tried again for good, every later ride held behind it ([`Recorder::held`]).
    stuck: bool,
    /// A start put off behind a save: its timer stopped at the start, resumed at the first second recorded.
    gap: bool,
    /// The full copy's save waits for the clean one's ([`Recorder::io_after`]): its end checkpointed, not saved.
    hold: bool,
    /// The ride's start ([`Recorder::start`]'s `time`), for a start put off behind a save.
    begun: u32,
    /// `make_kept` tried since boot: once, so one that keeps failing costs no FAT sync at every start.
    kept_tried: bool,
}

/// The buffer as the FIT writer's [`Sink`]: no card access (a checkpoint's flush is [`Recorder::flush`]).
struct Ram<'a> {
    tail: &'a mut [u8; TAIL],
    tail_off: u32,
    len: &'a mut u32,
    head: &'a mut [u8; 14],
}

impl Sink for Ram<'_> {
    fn write_at(&mut self, offset: u32, data: &[u8]) -> Result<(), fit::Error> {
        let end = offset as usize + data.len();
        if end > self.tail_off as usize + TAIL || (offset < self.tail_off && end > self.head.len()) {
            return Err(fit::Error); // only the header goes before the buffer
        }
        for (i, &b) in data.iter().enumerate() {
            let at = offset as usize + i;
            if let Some(h) = self.head.get_mut(at) {
                *h = b;
            }
            if let Some(t) = at.checked_sub(self.tail_off as usize).and_then(|k| self.tail.get_mut(k)) {
                *t = b;
            }
        }
        Ok(())
    }

    fn set_len(&mut self, len: u32) -> Result<(), fit::Error> {
        *self.len = len;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), fit::Error> {
        Ok(())
    }
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    /// No ride: the clean copy's recorder ([`Kind::Clean`]).
    pub const fn new() -> Self {
        Recorder::of(Kind::Clean)
    }

    /// No ride, for copy `kind`.
    pub const fn of(kind: Kind) -> Self {
        Recorder {
            kind,
            phase: Phase::Done,
            writer: Writer::new(),
            file: None,
            rec: None,
            seq: 0,
            name: [b'0'; 12],
            tail: [0; TAIL],
            tail_off: 0,
            len: 0,
            on_card: 0,
            on_card_trailer: [0; 2],
            stopped: false,
            head: [0; 14],
            since: 0,
            want: false,
            ended: false,
            ride: [0; RIDE_WORDS],
            last_rec: 0,
            skip_to: 0,
            wait: 0,
            fails: 0,
            resume_old: false,
            discard: false,
            beats: [0; BEATS_QUEUED],
            beats_n: 0,
            beats_ms: 0,
            dropped: 0,
            beats_dropped: 0,
            errors: 0,
            pending: None,
            saved: None,
            save_as: None,
            checked: false,
            stuck: false,
            gap: false,
            hold: false,
            begun: 0,
            kept_tried: false,
        }
    }

    /// The ride's name (its start, a second later per name already taken).
    pub fn name(&self) -> [u8; 12] {
        self.name
    }

    /// A checkpoint here whenever `other` (the ride's other copy) has one due, unless it is waiting after a failure:
    /// the two copies checkpoint together, so at a cut neither holds seconds the other lost (the full copy's buffer
    /// runs low first, its records being bigger). Before each pass's card work.
    pub fn keep_up(&mut self, other: &Recorder) {
        // a checkpoint the full copy wants only for a minute of beats while paused: the clean one has nothing new
        let beats_only =
            other.writer.paused() && other.beats_ms >= CHECKPOINT_S * 1000 && other.room() >= LOW + Writer::END_BYTES;
        if other.want && !beats_only && other.fails == 0 && matches!(self.phase, Phase::Starting | Phase::Recording) {
            self.want = true;
        }
    }

    /// The full copy's card work ([`Recorder::io`]), after the clean copy `clean`'s in the same pass (module doc,
    /// "Two copies"): the clean one's name while this one's file is not made (the clean one may have moved a second
    /// on, or resumed the ride left open); nothing until the clean one has made its file, so that a ride left open
    /// is carried on only when it is the clean one's; and a save only once the clean one's of the same ride is over,
    /// under the name that one was saved under, so the two keep one name.
    pub fn io_after(&mut self, clean: &Recorder, d: &mut impl DiskWrite, now: Local) -> Outcome {
        if self.phase == Phase::Done && self.pending.is_some() {
            if clean.pending.is_some() {
                return Outcome::Nothing; // held with the clean copy ([`Recorder::start_after`])
            }
            self.start_pending();
        }
        if self.phase == Phase::Starting && clean.pending.is_some() {
            return Outcome::Nothing; // the clean one's name is still the ride being saved
        }
        if self.phase == Phase::Starting && self.file.is_none() {
            self.name = clean.name;
        }
        let saving = matches!(clean.phase, Phase::Recording | Phase::Saving) && clean.name == self.name;
        if clean.phase == Phase::Starting || (self.ended && saving && self.phase != Phase::Recording) {
            return Outcome::Nothing;
        }
        // its end on the card meanwhile (the "saving" checkpoint), not its save: never a minute in RAM only
        self.hold = self.ended && saving;
        if let Some((from, to)) = clean.saved.filter(|s| self.ended && s.0 == self.name) {
            self.save_as = Some(to).filter(|&to| to != from);
        }
        self.io(d, now)
    }

    /// The full copy reopened by [`Recorder::recover`] (`Resumed`) goes on only with the clean copy `clean`'s ride:
    /// else (the clean one saved, kept, or another ride) it is ended at its last record and saved as it is, or
    /// discarded with the clean one (a cut between the two copies' "saving" records of a discard).
    pub fn resumed_after(&mut self, clean: &Recorder) {
        if self.phase == Phase::Recording && (clean.phase != Phase::Recording || clean.name != self.name) {
            self.end(0, self.ride);
            if clean.discard && clean.name == self.name {
                self.discard();
            }
        }
    }

    /// The ride's profile (its FIT writer's: kept in the recovery record, so a resumed ride has it).
    pub fn profile(&self) -> crate::ride::Profile {
        self.writer.profile()
    }

    /// That profile was set by hand (kept in the recovery record too).
    pub fn by_hand(&self) -> bool {
        self.writer.by_hand()
    }

    /// The profile in force ([`fit::Writer::set_profile`]), for the end and the next record even with no second
    /// recorded since (a change while paused). Only for a ride being recorded and not ended: the file's end
    /// already says what it was.
    pub fn set_profile(&mut self, profile: crate::ride::Profile, by_hand: bool) {
        if matches!(self.phase, Phase::Starting | Phase::Recording) && !self.ended {
            self.writer.set_profile(profile, by_hand);
        }
    }

    /// Where it is.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    fn ram(&mut self) -> (Ram<'_>, &mut Writer) {
        let sink = Ram { tail: &mut self.tail, tail_off: self.tail_off, len: &mut self.len, head: &mut self.head };
        (sink, &mut self.writer)
    }

    fn room(&self) -> usize {
        TAIL.saturating_sub(self.len.saturating_sub(self.tail_off) as usize)
    }

    /// Runs `f` on the writer if `need` bytes fit (the end's bytes kept for the end); else counts it dropped.
    fn put(&mut self, need: usize, f: impl FnOnce(&mut Writer, &mut Ram) -> Result<(), fit::Error>) {
        if !matches!(self.phase, Phase::Starting | Phase::Recording) || self.ended {
            return;
        }
        let ok = self.room() >= need + Writer::END_BYTES && {
            let (mut sink, w) = self.ram();
            f(w, &mut sink).is_ok()
        };
        if !ok {
            self.dropped = self.dropped.saturating_add(1);
        }
        // the data length is the writer's: the buffer's length follows it (the trailer is a checkpoint's)
        self.len = self.len.max(14 + self.writer.data_len());
        if self.room() < LOW + Writer::END_BYTES {
            self.want = true;
        }
    }

    /// A new ride at FIT time `time`, named after its local start: the file's first bytes, in RAM (the file is
    /// made at the next [`Recorder::io`], after what the last run left unfinished: resumed in this one's place if
    /// `resume_old` (the boot's marker said a ride was open) or if it stopped less than [`RESUME_S`] before `time`,
    /// else saved as it is). While the last ride is still being saved (its retries take up to a minute), kept and
    /// started once that is done ([`Recorder::io`]): the seconds until then are not in the file, the rest is.
    pub fn start(&mut self, time: u32, name: [u8; 12], resume_old: bool) {
        if self.phase != Phase::Done {
            if self.ended || self.phase == Phase::Saving {
                self.pending = Some((time, name, resume_old));
            }
            return;
        }
        (self.tail, self.tail_off, self.len, self.head) = ([0; TAIL], 0, 0, [0; 14]);
        let dev = self.kind == Kind::Full;
        let (mut sink, _) = self.ram();
        let Ok(w) = Writer::begin_with(&mut sink, time, dev) else { return };
        (self.writer, self.last_rec) = (w, time);
        self.len = 14 + w.data_len();
        (self.phase, self.file, self.name, self.since, self.want, self.ended, self.stopped) =
            (Phase::Starting, None, name, 0, true, false, false);
        (self.resume_old, self.discard, self.beats_n, self.beats_ms) = (resume_old, false, 0, 0);
        (self.pending, self.save_as, self.checked, self.stuck, self.gap, self.begun) =
            (None, None, false, false, false, time);
    }

    /// A Start waits behind a save that cannot finish ([`Recorder::stuck`]): the new ride is not being recorded
    /// (said), the old one's save goes on being tried.
    pub fn held(&self) -> bool {
        self.pending.is_some() && self.stuck
    }

    /// A save neither finished nor put back in its rides folder after its retries: tried again for good (the ride
    /// open: the drive lent read only, for the owner to copy what is on the card).
    pub fn stuck(&self) -> bool {
        self.stuck
    }

    /// GIVE UP SAVE (module doc): only while [`Recorder::stuck`] (else None). Names the ride where it can, else
    /// leaves its chain lost; the stuck state ends whatever the card does, and a start held behind it starts.
    #[inline(never)] // its sector buffers in its own frame
    pub fn give_up_save(&mut self, d: &mut impl DiskWrite, now: Local) -> Option<GaveUp> {
        if !self.stuck {
            return None;
        }
        let t = dos_time(now);
        let r = match Fat::mount(d) {
            Ok(mut fat) if self.phase == Phase::Saving => self.give_up_on(&mut fat, d, t),
            // the record never read (the card not mounting at boot): nothing known to name, nothing written; the
            // next ride's first card work reads it (one writer, one ride)
            _ => GaveUp::Lost,
        };
        (self.phase, self.stuck, self.want, self.stopped, self.hold, self.checked) =
            (Phase::Done, false, false, false, false, false);
        (self.fails, self.wait) = (0, 0);
        self.start_pending();
        Some(r)
    }

    fn give_up_on(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> GaveUp {
        match self.keep_in_rides(fat, d, t) {
            Ok(Outcome::Nothing) => return GaveUp::Lost, // the chain no longer the ride's: the record done
            Ok(o) => return GaveUp::Named(o),
            Err(_) => {}
        }
        self.errors = self.errors.saturating_add(1);
        if self.rescue(fat, d, t).is_ok() {
            return GaveUp::Rescue(self.name);
        }
        self.errors = self.errors.saturating_add(1);
        // named nowhere: the record "done", the chain lost. Failing too, the record still says "saving": the next
        // start tries the save again (stuck again if the card still fails)
        let first = self.file.map_or(0, |h| h.first);
        if self.write_record(fat, d, Phase::Done, first).is_err() {
            self.errors = self.errors.saturating_add(1);
        }
        GaveUp::Lost
    }

    /// The ride named in [`RESCUE_PATHS`] (module doc, "GIVE UP SAVE"): the link, then the record "done".
    fn rescue(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<(), Error> {
        let (first, size) = (self.file.map_or(0, |h| h.first), self.len);
        if first < 2 || self.still_ours(fat, d, first, size, None)?.is_none() {
            return Err(Error::Corrupt); // no chain, or no longer the ride's: never an entry on it
        }
        // never a second entry on the chain: both folders that may name it read whole, neither does
        let target = if self.discard { self.kind.discarded_path() } else { self.kind.saved() };
        for path in [target, self.kind.rides().1] {
            let dir = match fat.open(d, path) {
                Ok(dir) => dir,
                Err(Error::NotFound) => continue,
                Err(e) => return Err(e),
            };
            let mut c = fat.cursor(dir)?;
            while let Some(e) = fat.list(d, &mut c)? {
                if e.file.cluster == first {
                    return Err(Error::Refused);
                }
            }
        }
        // a folder made or grown takes a cluster: never from the reserve the next ride's clean copy needs. FSInfo's
        // count counted again after any card error (a failed FSInfo write after a grow leaves it high: the second
        // review of S5 and S6); a rescue follows errors as a rule: a whole FAT read, once, on the owner's request
        let free = match fat.free_clusters(d)? {
            Some(n) if self.errors == 0 => Some(n),
            _ => {
                fat.sync_fats(d)?;
                fat.free_clusters(d)?
            }
        };
        if free.is_none_or(|n| u64::from(n) * u64::from(fat.cluster_bytes()) < FULL_RESERVE) {
            return Err(Error::Refused);
        }
        let mut dir = fat.root();
        for name in self.kind.rescue_path().split('/') {
            dir = fat.ensure_dir(d, dir, name, t)?;
        }
        let mut buf = [0u8; 16];
        let mut name = self.name;
        for _ in 0..10 {
            match fat.find(d, dir, file_name(&name, &mut buf)?) {
                Ok(_) => name = next_name(&name),
                Err(Error::NotFound) => break,
                Err(e) => return Err(e),
            }
        }
        let n = file_name(&name, &mut buf)?;
        if !matches!(fat.find(d, dir, n), Err(Error::NotFound)) {
            return Err(Error::Refused); // ten names taken
        }
        fat.make_room(d, dir, n)?;
        fat.link(d, dir, n, &Handle { first, size, ..Handle::default() }, t)?;
        self.name = name;
        // named: a record left "saving" (this write failing) is done by the next save, which finds it here
        if self.write_record(fat, d, Phase::Done, first).is_err() {
            self.errors = self.errors.saturating_add(1);
        }
        Ok(())
    }

    /// The record "done" if GIVE UP SAVE's rescue named the chain at `first` in this copy's [`RESCUE_PATHS`] folder
    /// (its link done, its record not: a cut or a failure between): `Kept` under the name it has there, never a
    /// second entry on the chain. Asked only of a chain named in neither its saved nor its rides folder (what the
    /// rescue requires), so a damaged rescue folder never stops an ordinary save; one that cannot be read then is an
    /// error, tried again.
    fn rescued(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, first: u32) -> Result<Option<Outcome>, Error> {
        if first < 2 {
            return Ok(None); // no chain (a damaged record): an empty file there has cluster 0
        }
        let dir = match fat.open(d, self.kind.rescue_path()) {
            Ok(dir) => dir,
            Err(Error::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut c = fat.cursor(dir)?;
        while let Some(e) = fat.list(d, &mut c)? {
            if !e.file.dir && e.file.cluster == first {
                // the rescue may have taken a second later: the record and the owner get the name it has
                if let Some(n) = e.name().get(..12).and_then(|n| <[u8; 12]>::try_from(n).ok()) {
                    self.name = n;
                }
                self.write_record(fat, d, Phase::Done, first)?;
                (self.phase, self.stuck) = (Phase::Done, false);
                return Ok(Some(Outcome::Kept(self.name)));
            }
        }
        Ok(None)
    }

    /// A Start is waiting for the last ride's save ([`Recorder::start`]).
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Calls of [`Recorder::io`] to wait after a failure: [`RETRY`], doubling once the save is stuck (each try
    /// mounts and scans folders: about 5 s, then up to 160 s).
    fn backoff(&self) -> u32 {
        if self.stuck { RETRY << self.fails.saturating_sub(GIVE_UP).min(5) } else { RETRY }
    }

    /// A ride started ([`Recorder::start`]), or waiting for the last one's save to start.
    pub fn starting(&self) -> bool {
        self.phase == Phase::Starting || self.pending.is_some()
    }

    /// A checkpoint at the next [`Recorder::io`], even while waiting after a failure (switching off: the last
    /// chance to put what is in RAM on the card).
    pub fn checkpoint_now(&mut self) {
        if matches!(self.phase, Phase::Starting | Phase::Recording) {
            (self.want, self.wait) = (true, 0);
        }
    }

    /// The card failed at the last try: what is in RAM since the last checkpoint is not on the card.
    pub fn failing(&self) -> bool {
        self.fails > 0
    }

    /// Card failures in a row (0 after a success).
    pub fn fails(&self) -> u32 {
        self.fails
    }

    /// Whether to try again after `e`: only a card error, and only [`GIVE_UP`] times in a row.
    fn retry(&self, e: Error) -> bool {
        e == Error::Io && self.fails + 1 < GIVE_UP
    }

    /// One second of ride: its record, and the app's totals for the next recovery record.
    pub fn second(&mut self, s: &Sample, ride: [u32; RIDE_WORDS]) {
        self.ride = ride;
        // one record a second, each later than the last (seconds caught up after a long pass share a clock)
        if self.kind == Kind::Full && s.time <= self.skip_to {
            return;
        }
        let time = if s.time > self.last_rec { s.time } else { self.last_rec + 1 };
        if core::mem::take(&mut self.gap) && self.writer.paused() {
            self.put(16, |w, sink| w.resume(sink, time)); // the ride recorded from here (a start put off)
        }
        self.last_rec = time;
        let s = Sample { time, ..*s };
        // the trainer's fields are optional: without room for them (a card behind), the second without them
        let s = if self.room() < RECORD_MAX + self.writer.trainer_bytes(&s) + Writer::END_BYTES {
            Sample { trainer: None, ..s }
        } else {
            s
        };
        self.put(RECORD_MAX + self.writer.trainer_bytes(&s), |w, sink| w.record(sink, &s));
        self.since += 1;
        if self.since >= CHECKPOINT_S {
            self.want = true;
        }
    }

    /// Paused at `time` (a checkpoint follows, as stock).
    pub fn pause(&mut self, time: u32, ride: [u32; RIDE_WORDS]) {
        self.ride = ride;
        self.put(16, |w, sink| w.pause(sink, time));
        self.want = true;
    }

    /// Resumed at `time`.
    pub fn resume(&mut self, time: u32) {
        self.put(16, |w, sink| w.resume(sink, time));
    }

    /// A lap at `time`.
    pub fn lap(&mut self, time: u32) {
        self.put(110, |w, sink| w.lap(sink, time));
    }

    /// A gear change at `time` ([`Writer::gear`]): in RAM like the rest, dropped (counted) when there is no room, so
    /// a shifting system can never hold the ride up.
    pub fn gear(&mut self, time: u32, front: bool, front_n: u8, rear_n: u8) {
        self.put(32, |w, sink| w.gear(sink, time, front, front_n, rear_n));
    }

    /// One RR interval from the strap, ms, as it came (the full copy only; nothing without a ride being recorded):
    /// queued, written as an `hrv` message ([`fit::Writer::hrv`]) once there are five, between the records. RAM
    /// only, like the rest: when the buffer has no room (a card failing) they wait in a bounded queue, and when that
    /// is full the newest is dropped and counted ([`Recorder::beats_dropped`]); never more than that.
    pub fn beat(&mut self, ms: u16) {
        if self.kind != Kind::Full || !matches!(self.phase, Phase::Starting | Phase::Recording) || self.ended {
            return;
        }
        match self.beats.get_mut(self.beats_n) {
            Some(slot) => {
                *slot = ms;
                self.beats_n += 1;
            }
            None => self.beats_dropped = self.beats_dropped.saturating_add(1),
        }
        self.write_beats(false);
    }

    /// The queued beats into `hrv` messages of five while there is room, and the last few too if `all` (the end).
    fn write_beats(&mut self, all: bool) {
        while self.beats_n >= fit::HRV_PER_MESSAGE || (all && self.beats_n > 0) {
            let k = self.beats_n.min(fit::HRV_PER_MESSAGE);
            let mut batch = [0u16; fit::HRV_PER_MESSAGE];
            batch.copy_from_slice(self.beats.get(..fit::HRV_PER_MESSAGE).unwrap_or(&[0; fit::HRV_PER_MESSAGE]));
            let ok = self.room() >= fit::HRV_MAX + Writer::END_BYTES && {
                let (mut sink, w) = self.ram();
                w.hrv(&mut sink, batch.get(..k).unwrap_or(&[])).is_ok()
            };
            if !ok {
                return; // they wait (a checkpoint is wanted already: the room is under LOW)
            }
            self.beats.copy_within(k.., 0);
            self.beats_n -= k;
            self.len = self.len.max(14 + self.writer.data_len());
            let ms: u32 = batch.get(..k).unwrap_or(&[]).iter().map(|&b| u32::from(b)).sum();
            self.beats_ms = self.beats_ms.saturating_add(ms);
            if self.room() < LOW + Writer::END_BYTES || self.beats_ms >= CHECKPOINT_S * 1000 {
                self.want = true;
            }
        }
    }

    /// The ride ended at `time`: the end messages in RAM, the save at the next [`Recorder::io`]. A ride held behind
    /// the last one's save ([`Recorder::pending`]) is dropped instead (nothing of it recorded): the last one is never
    /// the one ended.
    pub fn end(&mut self, time: u32, ride: [u32; RIDE_WORDS]) {
        if self.pending.take().is_some() {
            return; // started and ended while the last one was saved: nothing of it was recorded
        }
        self.ride = ride;
        if !matches!(self.phase, Phase::Starting | Phase::Recording) || self.ended {
            return;
        }
        self.write_beats(true);
        self.beats_dropped = self.beats_dropped.saturating_add(u32::try_from(self.beats_n).unwrap_or(u32::MAX));
        self.beats_n = 0;
        let (mut sink, w) = self.ram();
        if w.end(&mut sink, time).is_err() {
            self.dropped = self.dropped.saturating_add(1); // cannot happen: END_BYTES are kept
        }
        self.len = self.len.max(14 + self.writer.data_len());
        (self.ended, self.want) = (true, true);
    }

    /// The ride just ended ([`Recorder::end`]) goes to `\Rustfw\Discarded` instead of the saved folder (module
    /// doc). Only before its save has begun: a save under way (a full card stopped the ride) goes on where it goes.
    pub fn discard(&mut self) -> bool {
        if self.ended && matches!(self.phase, Phase::Starting | Phase::Recording) {
            self.discard = true;
        }
        self.discard
    }

    /// The card has work to do ([`Recorder::io`]).
    pub fn due(&self) -> bool {
        (self.want && self.phase != Phase::Done)
            || self.phase == Phase::Saving // a save is due until done
            || (self.phase == Phase::Done && self.pending.is_some()) // a full copy's start held (start_after)
    }

    /// The full copy's [`Recorder::start`]: held as well while the clean copy `clean` holds the ride behind its last
    /// save (never a full copy of a ride the clean one does not record); started by [`Recorder::io_after`] once the
    /// clean one has started.
    pub fn start_after(&mut self, clean: &Recorder, time: u32, name: [u8; 12], resume_old: bool) {
        if clean.pending.is_some() && self.phase == Phase::Done {
            self.pending = Some((time, name, resume_old));
        } else {
            self.start(time, name, resume_old);
        }
    }

    /// The start put off ([`Recorder::start`]) now: its timer stopped from its start until the first second recorded
    /// (the file says when it was not recorded).
    fn start_pending(&mut self) {
        if let Some((time, name, resume_old)) = self.pending {
            self.start(time, name, resume_old);
            self.put(16, |w, sink| w.pause(sink, time));
            self.gap = true;
        }
    }

    /// Does the card work that is due: make the file, a checkpoint, the save. Call it from the main loop (it takes
    /// a few ms when it writes, once a minute; the save reads the file back).
    #[inline(never)] // its sector buffers in its own frame, not the main loop's
    pub fn io(&mut self, d: &mut impl DiskWrite, now: Local) -> Outcome {
        if !self.due() {
            return Outcome::Nothing;
        }
        if self.wait > 0 {
            self.wait -= 1;
            return Outcome::Nothing;
        }
        let o = match self.io_inner(d, now) {
            Ok(o) => {
                self.fails = 0;
                o
            }
            Err(e) if self.kind == Kind::Full && self.phase == Phase::Starting && !self.retry(e) => {
                // the full copy could not be made (refused, or the card kept failing): this ride goes without it,
                // never held up for it
                self.errors = self.errors.saturating_add(1);
                (self.phase, self.want, self.fails) = (Phase::Done, false, 0);
                Outcome::Kept(self.name)
            }
            Err(e) if e != Error::Io && matches!(self.phase, Phase::Starting | Phase::Recording) => {
                // a checkpoint the card refuses (full: no free cluster) or cannot take (Corrupt) would be refused
                // forever, the ride never saved and the card never lent to free space (the review of 2026-10-25):
                // the ride stops here, saved as its last checkpoint left it (moving it takes no new cluster), and
                // so the card is settled. Before that checkpoint there is nothing to save (a "starting" record is
                // marked done by the next recovery)
                self.errors = self.errors.saturating_add(1);
                self.fails = 0;
                if self.phase == Phase::Recording && self.on_card > 0 {
                    (self.phase, self.stopped) = (Phase::Saving, true);
                } else {
                    self.phase = Phase::Done;
                }
                self.want = false;
                Outcome::Stopped(e)
            }
            Err(e) => {
                self.errors = self.errors.saturating_add(1);
                self.fails = self.fails.saturating_add(1);
                self.wait = self.backoff();
                Outcome::Failed(e)
            }
        };
        if self.phase == Phase::Done {
            self.start_pending(); // the save is over: the ride started meanwhile
        }
        o
    }

    fn io_inner(&mut self, d: &mut impl DiskWrite, now: Local) -> Result<Outcome, Error> {
        let t = dos_time(now);
        let mut fat = Fat::mount(d)?;
        let low = |fat: &mut Fat, d: &mut _| -> Result<bool, Error> {
            let free = match fat.free_clusters(d)? {
                Some(n) => Some(n),
                // FSInfo's count unknown (0xffffffff, as a formatter may leave it): counted once, which writes it
                None => {
                    fat.sync_fats(d)?;
                    fat.free_clusters(d)?
                }
            };
            // still unknown (no FSInfo at all): the reserve cannot be kept, so no full copy
            Ok(free.is_none_or(|n| u64::from(n) * u64::from(fat.cluster_bytes()) < FULL_RESERVE))
        };
        if self.kind == Kind::Full && !self.ended && self.phase != Phase::Saving && low(&mut fat, d)? {
            // the card nearly full: what is left is the clean copy's. This one stops where it is (its record done:
            // never resumed nor saved, a part of the ride only), said once
            if let Some(f) = self.file {
                self.write_record(&mut fat, d, Phase::Done, f.first)?;
            }
            (self.phase, self.want) = (Phase::Done, false);
            return Ok(Outcome::Kept(self.name));
        }
        let mut told = Outcome::Nothing;
        if self.phase == Phase::Starting {
            let dir = self.setup(&mut fat, d, t)?;
            let best = self.read_records(&mut fat, d)?;
            // optional, as the discarded folders: a name taken there never stops the ride (the app makes its file
            // when it writes it)
            if self.kind == Kind::Clean && !self.kept_tried && best.as_ref().is_none_or(|r| r.phase == Phase::Done) {
                self.kept_tried = true;
                if make_kept(&mut fat, d, t).is_err() {
                    self.errors = self.errors.saturating_add(1);
                }
            }
            if let Some(r) = best.filter(|r| r.phase != Phase::Done) {
                // what the last run left unfinished comes first (the boot did not finish it: its marker was lost, or
                // the card failed then): one writer, one ride. A ride in progress goes on in place of this one if
                // the marker said so or it stopped a moment ago; else it is saved as it is
                let last = Writer::from_record(&r.fit).map_or(0, |w| w.last_time());
                let resume = match self.kind {
                    Kind::Clean => self.resume_old || self.last_rec.wrapping_sub(last) <= RESUME_S,
                    // the clean copy decided first ([`Recorder::io_after`]): its name is the ride left open if it
                    // carries that one on
                    Kind::Full => r.name == self.name,
                };
                fat.sync_fats(d)?; // the cut may have been between the two FATs, or torn one
                let mut old = Recorder::of(self.kind);
                (old.rec, old.seq, old.errors, old.kept_tried) = (self.rec, self.seq, self.errors, self.kept_tried);
                let got = old.recover_from(&mut fat, d, Some(r), t, resume);
                if got.is_err() {
                    old.errors = old.errors.saturating_add(1); // whatever follows: the arms below go on after some
                }
                match got {
                    Ok(o @ Outcome::Resumed(..)) => {
                        *self = old;
                        return Ok(o);
                    }
                    Ok(o) => told = o,
                    Err(e) if self.retry(e) => return Err(e),
                    // a save given up: the ride named where it was recorded first (its entry may be gone: the save's
                    // step 4), never overwritten while the record alone names it
                    Err(e) if old.phase == Phase::Saving => match old.give_up(&mut fat, d, t) {
                        Ok(o) => told = o,
                        Err(_) => {
                            // stuck: this recorder carries on with that save (tried again for good) and this ride
                            // waits behind it, as a Start during a save does
                            (old.pending, old.fails) = (Some((self.begun, self.name, self.resume_old)), self.fails);
                            *self = old;
                            return Err(e);
                        }
                    },
                    // a ride left open that could not be resumed: named in its folder, its record overwritten next
                    Err(_) => told = Outcome::Kept(r.name),
                }
                (self.seq, self.errors) = (self.seq.max(old.seq), self.errors.max(old.errors));
            }
            if self.file.is_none() {
                // a name free in this copy's folder and in both saved and both discarded folders, so that the two
                // copies keep one name
                let acts = fat.open(d, ACTIVITIES)?;
                // missing or damaged: never a reason to stop this one
                let others = [FULL_PATH, DISCARDED_PATHS[0], DISCARDED_PATHS[1]].map(|p| fat.open(d, p).ok());
                let mut buf = [0u8; 16];
                for _ in 0..10 {
                    let n = file_name(&self.name, &mut buf)?;
                    let free = |r: Result<File, Error>| matches!(r, Err(Error::NotFound));
                    if free(fat.find(d, dir, n))
                        && free(fat.find(d, acts, n))
                        && others.iter().all(|o| o.is_none_or(|f| free(fat.find(d, f, n))))
                    {
                        break;
                    }
                    self.name = next_name(&self.name);
                }
                self.write_record(&mut fat, d, Phase::Starting, 0)?;
                let n = file_name(&self.name, &mut buf)?;
                self.file = Some(fat.create(d, dir, n, t)?);
            }
            self.checkpoint(&mut fat, d, t, Phase::Recording)?;
            self.phase = Phase::Recording;
            if !self.ended {
                return Ok(told);
            }
        }
        if self.phase == Phase::Recording && !self.ended {
            self.checkpoint(&mut fat, d, t, Phase::Recording)?;
            return Ok(Outcome::Nothing);
        }
        if self.phase == Phase::Recording {
            self.checkpoint(&mut fat, d, t, Phase::Saving)?;
            self.phase = Phase::Saving;
        }
        if self.phase == Phase::Saving && !core::mem::take(&mut self.hold) {
            let first = self.file.map_or(0, |h| h.first);
            let size = self.len;
            let saved =
                if self.stopped { self.save_as_is(&mut fat, d, t) } else { self.save(&mut fat, d, t, first, size) };
            let o = match saved {
                Ok(o) => o,
                Err(e) if self.retry(e) => return Err(e),
                // given up: the ride named where it was recorded, its record done; else tried again for good
                Err(e) => self.give_up(&mut fat, d, t).map_err(|_| e)?,
            };
            (self.phase, self.want, self.stopped) = (Phase::Done, false, false);
            return Ok(o);
        }
        Ok(Outcome::Nothing)
    }

    /// A save given up ([`GIVE_UP`] card errors, or one no retry mends: `Corrupt`, `Refused`): the ride named where
    /// it was recorded ([`Recorder::keep_in_rides`]), `Kept`; if even that fails, stuck: the phase stays "saving",
    /// tried again for good, and the record is never overwritten.
    fn give_up(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<Outcome, Error> {
        let r = self.keep_in_rides(fat, d, t);
        self.stuck = r.is_err();
        r
    }

    /// Makes sure the ride being saved is named by an entry, then writes its record "done": its entry in its rides
    /// folder as it is, or, gone (the save's step 4 done, 5 not), linked again there under its name (step 3 made the
    /// room: no cluster taken); unless the save's own link (5) is there already, which is the save done. Never two
    /// entries on one chain: a folder that cannot be read is an error, tried again. A chain no longer the ride's
    /// ([`Recorder::still_ours`]) is never linked: the record done, `Nothing` (the ride removed by a computer).
    fn keep_in_rides(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<Outcome, Error> {
        let (first, size) = (self.file.map_or(0, |h| h.first), self.len);
        if first < 2 {
            return Err(Error::Corrupt); // no chain to name (cluster 0 or 1): never an entry on it
        }
        let mut buf = [0u8; 16];
        let target = if self.discard { self.kind.discarded_path() } else { self.kind.saved() };
        match fat.open(d, target) {
            Ok(acts) => {
                let mut name = self.save_as.unwrap_or(self.name);
                for _ in 0..10 {
                    match fat.find(d, acts, file_name(&name, &mut buf)?) {
                        Ok(f) if f.cluster == first => {
                            (self.saved, self.name) = (Some((self.name, name)), name);
                            self.write_record(fat, d, Phase::Done, first)?;
                            (self.phase, self.stuck) = (Phase::Done, false);
                            return Ok(if self.discard { Outcome::Discarded(name) } else { Outcome::Saved(name) });
                        }
                        Ok(_) => name = next_name(&name),
                        Err(Error::NotFound) => break,
                        Err(e) => return Err(e),
                    }
                }
            }
            Err(Error::NotFound) => {}
            Err(e) => return Err(e),
        }
        let dir = fat.open(d, self.kind.rides().1)?;
        let n = file_name(&self.name, &mut buf)?;
        match fat.locate(d, dir, n) {
            Ok(h) if h.first == first => {}
            Ok(_) => return Err(Error::Refused), // its name another file's: never a second entry, tried again
            Err(Error::NotFound) => {
                if let Some(o) = self.rescued(fat, d, first)? {
                    return Ok(o);
                }
                if self.still_ours(fat, d, first, size, None)?.is_none() {
                    // no longer the ride's (a computer removed it): never linked, the record done, nothing named
                    self.write_record(fat, d, Phase::Done, first)?;
                    (self.phase, self.stuck) = (Phase::Done, false);
                    return Ok(Outcome::Nothing);
                }
                fat.link(d, dir, n, &Handle { first, size, ..Handle::default() }, t)?;
            }
            Err(e) => return Err(e),
        }
        self.write_record(fat, d, Phase::Done, first)?;
        (self.phase, self.stuck) = (Phase::Done, false);
        Ok(Outcome::Kept(self.name))
    }

    /// The state a save is tried again from by [`Recorder::io`] (a boot's or a Start's recovery calls the save
    /// directly): saving, ended, the chain at `first` of `size` bytes.
    fn saving(&mut self, first: u32, size: u32) {
        (self.phase, self.ended, self.stopped, self.len) = (Phase::Saving, true, false, size);
        if self.file.is_none_or(|h| h.first != first) {
            self.file = Some(Handle { first, size, ..Handle::default() });
        }
    }

    /// The chain at `first`, if it is still this ride's `size` bytes; None if not (the review of 2026-10-29, S1 and
    /// S2). A chain no entry names may have been removed by a computer (or stock's USB drive), its clusters freed
    /// or taken by another file: written or linked then, it would overwrite that file or cross-link the next one.
    /// Taken, long enough, and its sector 0 as the latest record's copy wherever no later checkpoint rewrites it:
    /// the header but its size and CRC, the data before the trailer (as trash.rs checks a chain with `first_crc`;
    /// a cut during the next checkpoint may have left that one's header there). `marked` ([`Rec::checked`]): a
    /// repair a cut tore may have left sector 0 anything, so sector 1's CRC instead, which the repair never writes.
    #[inline(never)] // its two sectors in its own frame
    fn still_ours(
        &self,
        fat: &mut Fat,
        d: &mut impl DiskWrite,
        first: u32,
        size: u32,
        marked: Option<u16>,
    ) -> Result<Option<Handle>, Error> {
        let h = match fat.chain(d, first) {
            Ok(h) => h,
            Err(Error::Corrupt) => return Ok(None), // freed, or out of the volume
            Err(e) => return Err(e),
        };
        if first < 2 || h.clusters.checked_mul(fat.cluster_bytes()).is_some_and(|b| b < size) {
            return Ok(None);
        }
        let (rec, mut copy, mut s) = (self.rec.ok_or(Error::NotFound)?, [0u8; SECTOR], [0u8; SECTOR]);
        if let Some(crc) = marked {
            fat.read_sector(d, &h, 1, &mut s)?;
            if fit::crc(0, &s) != crc {
                return Ok(None);
            }
            if h.clusters > 1 {
                return Ok(Some(h));
            }
            // one cluster: a small file written there since (an .ini, a .sum) changes sector 0 alone, sector 1
            // keeping the ride's stale bytes: sector 0 compared too (both the sector before the repair and the
            // record's copy pass; a sector 0 torn by a cut on a card without reliable write lets such a ride go)
        }
        fat.read_sector(d, &rec, 3 * (self.seq % 2), &mut copy)?;
        fat.read_sector(d, &h, 0, &mut s)?;
        let end = (size.saturating_sub(2) as usize).clamp(14, SECTOR);
        let same = |r: core::ops::Range<usize>| s.get(r.clone()) == copy.get(r);
        Ok((same(0..4) && same(8..12) && same(14..end)).then_some(h))
    }

    /// The record `seq` (a ride left open, its chain `h` of `size` bytes named by it alone) written again, its copies
    /// with it, marked [`Rec::checked`] with the CRC of the file's sector 1: the chain found still the ride's,
    /// before its repair rewrites sector 0. Not for a file whose repair writes sector 1 (under 3 sectors: a few
    /// seconds of ride), which a torn repair then lets go.
    #[inline(never)] // its sectors in its own frame
    fn mark_checked(
        &mut self,
        fat: &mut Fat,
        d: &mut impl DiskWrite,
        seq: u32,
        h: &Handle,
        size: u32,
    ) -> Result<(), Error> {
        if size.saturating_sub(2) / SECTOR_U32 < 2 {
            return Ok(());
        }
        let (rec, slot) = (self.rec.ok_or(Error::NotFound)?, 3 * (seq % 2));
        let (mut first, mut last) = ([0u8; SECTOR], [0u8; SECTOR]);
        fat.read_sector(d, h, 1, &mut first)?;
        let crc = fit::crc(0, &first);
        fat.read_sector(d, &rec, slot, &mut first)?;
        fat.read_sector(d, &rec, slot + 1, &mut last)?;
        self.write_record_with(fat, d, Phase::Recording, h.first, Some((&first, &last)), Some(crc))
    }

    /// Makes `\Rustfw`, `\Rustfw\ride.rec` (two zeroed sectors), `\Rustfw\Rides`, `\Activities` and
    /// `\Rustfw\Discarded\Full` when missing
    /// (the FATs synced first: a cut while making them leaves no record to say so). `\Rustfw\Rides`.
    ///
    /// `ride.rec`'s entry is in `\Rustfw`, written here only; the ride's is in `\Rustfw\Rides`, rewritten at every
    /// checkpoint (its size): a sector torn by a cut cannot take both (the second review, R3).
    fn setup(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<File, Error> {
        let root = fat.root();
        let dir = fat.find(d, root, DIR);
        let (rec, rides) = match dir {
            Ok(f) => (fat.locate(d, f, self.kind.rec()).ok(), fat.find(d, f, self.kind.rides().0).is_ok()),
            Err(_) => (None, false),
        };
        let whole = rides
            && rec.is_some_and(|r| r.size >= REC_SECTORS * SECTOR_U32)
            && fat.open(d, self.kind.saved()).is_ok()
            && fat.open(d, DISCARDED_PATHS[1]).is_ok();
        if !whole {
            fat.sync_fats(d)?;
        }
        let dir = fat.ensure_dir(d, root, DIR, t)?;
        let mut rec = match fat.locate(d, dir, self.kind.rec()) {
            Ok(h) => h,
            Err(Error::NotFound) => fat.create(d, dir, self.kind.rec(), t)?,
            Err(e) => return Err(e),
        };
        if rec.size < REC_SECTORS * SECTOR_U32 {
            fat.write_sectors(d, &mut rec, 0, &[0; REC_SECTORS as usize * SECTOR])?;
            fat.set_size(d, &mut rec, REC_SECTORS * SECTOR_U32, t)?;
        }
        self.rec = Some(rec);
        let rides = fat.ensure_dir(d, dir, self.kind.rides().0, t)?;
        fat.ensure_dir(d, root, ACTIVITIES, t)?; // both copies' names are looked for in it
        self.kind.saved_dir(fat, d, t)?;
        // both discarded folders now, before any record: a discard's save then makes no folder, so it never writes
        // the directory sector `ride.rec`'s entry is in, nor the one its clean copy's entry went to (a cut tearing
        // either there lost the ride, or its clean copy: a_discard_cut_at_every_sector)
        // optional: a damaged or taken name there must not stop the ride's recording (a discard then keeps the ride
        // in its folder)
        if Kind::Full.discarded_dir(fat, d, t).is_err() {
            self.errors = self.errors.saturating_add(1); // counted all the same (the app's FAT compare)
        }
        Ok(rides)
    }

    /// The newer valid record of the two (its sequence number taken as the last one used); None if neither is.
    fn read_records(&mut self, fat: &mut Fat, d: &mut impl DiskWrite) -> Result<Option<Rec>, Error> {
        let rec = self.rec.ok_or(Error::NotFound)?;
        let mut best: Option<Rec> = None;
        for k in 0..2 {
            let mut s = [0u8; SECTOR];
            fat.read_sector(d, &rec, 3 * k + 2, &mut s)?;
            if let Some(r) = Rec::from_sector(&s)
                && best.is_none_or(|b| r.seq > b.seq)
            {
                best = Some(r);
            }
        }
        self.seq = best.map_or(0, |r| r.seq);
        Ok(best)
    }

    fn write_record(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, phase: Phase, cluster: u32) -> Result<(), Error> {
        self.write_record_with(fat, d, phase, cluster, None, None)
    }

    /// The record in its slot (module doc), after the copies of the file's sector 0 and last sector when given:
    /// one write, the record last. `checked`: [`Rec::checked`].
    fn write_record_with(
        &mut self,
        fat: &mut Fat,
        d: &mut impl DiskWrite,
        phase: Phase,
        cluster: u32,
        copies: Option<(&[u8; SECTOR], &[u8; SECTOR])>,
        checked: Option<u16>,
    ) -> Result<(), Error> {
        let mut rec = self.rec.ok_or(Error::NotFound)?;
        let seq = self.seq.wrapping_add(1);
        let (name, fit, ride, discard) = (self.name, self.writer.to_record(), self.ride, self.discard);
        let r = Rec { seq, phase, name, cluster, fit, ride, discard, checked };
        let slot = 3 * (seq % 2);
        match copies {
            Some((first, last)) => {
                let mut buf = [0u8; 3 * SECTOR];
                buf[..SECTOR].copy_from_slice(first);
                buf[SECTOR..2 * SECTOR].copy_from_slice(last);
                buf[2 * SECTOR..].copy_from_slice(&r.to_sector());
                fat.write_sectors(d, &mut rec, slot, &buf)?;
            }
            None => fat.write_sectors(d, &mut rec, slot + 2, &r.to_sector())?,
        }
        self.seq = seq;
        Ok(())
    }

    /// A checkpoint (module doc), its record in `phase`.
    fn checkpoint(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16), phase: Phase) -> Result<(), Error> {
        let mut file = self.file.ok_or(Error::NotFound)?;
        {
            let (mut sink, w) = self.ram();
            w.checkpoint(&mut sink).map_err(|_| Error::Corrupt)?;
        }
        // the sectors from the buffer's start to the end of the file, then sector 0's header, then the size
        let n = (self.len - self.tail_off).div_ceil(SECTOR_U32) as usize;
        let bytes = self.tail.get(..n * SECTOR).ok_or(Error::Corrupt)?;
        let wrote = fat.write_sectors(d, &mut file, self.tail_off / SECTOR_U32, bytes);
        // the chain as grown, kept whatever fails from here: a retry from the old one would take another cluster and
        // relink the last to it, the one taken left named by nothing (the review of 2026-10-29, S5). The handle's
        // chain may then be longer than its size: the next checkpoint's `set_size`, a recovery or the save as it is
        // (`save_as_is`) trims it
        self.file = Some(file);
        wrote?;
        let mut first = [0u8; SECTOR];
        if self.tail_off > 0 {
            fat.read_sector(d, &file, 0, &mut first)?;
            first[..14].copy_from_slice(&self.head);
            fat.write_sectors(d, &mut file, 0, &first)?;
        } else {
            first.copy_from_slice(self.tail.get(..SECTOR).ok_or(Error::Corrupt)?);
        }
        fat.set_size(d, &mut file, self.len, t)?;
        self.file = Some(file);
        // the record, with the two sectors the next checkpoint rewrites in place as they are now
        let next = (self.len - 2) / SECTOR_U32 * SECTOR_U32;
        let from = (next - self.tail_off) as usize;
        let mut last = [0u8; SECTOR];
        last.copy_from_slice(self.tail.get(from..from + SECTOR).ok_or(Error::Corrupt)?);
        self.write_record_with(fat, d, phase, file.first, Some((&first, &last)), None)?;
        // the buffer keeps the sector the next byte (over the trailer) goes in
        self.tail.copy_within(from.., 0);
        let keep = (self.len - next) as usize;
        if let Some(rest) = self.tail.get_mut(keep..) {
            rest.fill(0);
        }
        (self.tail_off, self.since, self.want, self.on_card, self.beats_ms) = (next, 0, false, self.len, 0);
        self.on_card_trailer = self.writer.trailer();
        Ok(())
    }

    /// The save of the file as its last checkpoint left it, [`Recorder::on_card`] bytes (the card refused the next
    /// one): the sector its trailer ends in and sector 0 written again from the record first. A full card refuses
    /// before any sector is written (`write_sectors` takes the clusters first), but a checkpoint that failed later
    /// (`Corrupt` at its size or its record) has rewritten them; within the file's clusters: no new one taken.
    fn save_as_is(&mut self, fat: &mut Fat, d: &mut impl DiskWrite, t: (u16, u16)) -> Result<Outcome, Error> {
        let mut file = self.file.ok_or(Error::NotFound)?;
        let (rec, slot, size) = (self.rec.ok_or(Error::NotFound)?, 3 * (self.seq % 2), self.on_card);
        let mut s = [0u8; SECTOR];
        fat.read_sector(d, &rec, slot + 1, &mut s)?;
        fat.write_sectors(d, &mut file, size.saturating_sub(2) / SECTOR_U32, &s)?;
        trailer_end(fat, d, &mut file, size, self.on_card_trailer)?;
        fat.read_sector(d, &rec, slot, &mut s)?;
        fat.write_sectors(d, &mut file, 0, &s)?;
        // a checkpoint that grew the chain, then failed: cut back to the size (its entry's size first), else saved
        // a cluster longer than its size
        if file.clusters > size.div_ceil(fat.cluster_bytes()).max(1) {
            fat.set_size(d, &mut file, size, t)?;
            self.file = Some(file);
        }
        self.save(fat, d, t, file.first, size)
    }

    /// The save from step 2 on (module doc): the file at `first`, `size` bytes, checked, moved, the record done:
    /// `Saved`. A file that reads back but is not a whole FIT file (a damaged sector, its chain cut short) is not
    /// moved and not tried again: it stays where it is, the record done: `Kept` (the second review, R1).
    fn save(
        &mut self,
        fat: &mut Fat,
        d: &mut impl DiskWrite,
        t: (u16, u16),
        first: u32,
        size: u32,
    ) -> Result<Outcome, Error> {
        self.len = size; // what keep_in_rides names, if it comes to that (a save as it is: less than the buffer's)
        // a chain the save's step 4 left named by the record alone may have been freed by a computer since (its
        // clusters still reading as the ride): moved (5), it would be linked onto free clusters. Not the ride's:
        // left to keep_in_rides, which names it only where its entry still is
        if !self.checked && self.still_ours(fat, d, first, size, None)?.is_none() {
            return self.keep_in_rides(fat, d, t);
        }
        // 2: read back whole, through the buffer (the file is all on the card by now); once a run (a retry would
        // read it all again: the stuck save's cost)
        let f = File { cluster: first, size, dir: false };
        let (mut at, mut crc, mut whole) = (if self.checked { size } else { 0 }, 0u16, size >= 16);
        while whole && at < size {
            let n = match fat.read_at(d, &f, at, &mut self.tail) {
                Ok(n) => n,
                Err(Error::Corrupt) => 0, // the chain is not what the record says
                Err(e) => return Err(e),
            };
            whole = n > 0 && (at > 0 || (n >= 14 && self.tail[4..8] == (size - 16).to_le_bytes()));
            crc = fit::crc(crc, self.tail.get(..n).unwrap_or(&[]));
            at += u32::try_from(n).map_err(|_| Error::Corrupt)?;
        }
        if !whole || crc != 0 {
            return self.keep_in_rides(fat, d, t); // (named again if its entry went, at a save cut after step 4)
        }
        self.checked = true;
        // 3: the folder and a free name, before 4, so that between 4 and 5 only the link is left
        let mut buf = [0u8; 16];
        // a discard never makes a folder (that would write `\Rustfw`'s directory sector, which holds `ride.rec`'s
        // entry): missing ones (a ride resumed across the update, a folder removed) keep the ride where it is
        let acts = if self.discard {
            match fat.open(d, self.kind.discarded_path()) {
                Ok(f) => f,
                Err(_) => return self.keep_in_rides(fat, d, t),
            }
        } else {
            self.kind.saved_dir(fat, d, t)?
        };
        // the full copy, saved after the clean one, from the name that one took ([`Recorder::io_after`]): one name
        // for both, the start having picked one free in all four folders
        let (mut name, mut linked) = (self.save_as.unwrap_or(self.name), None);
        for _ in 0..10 {
            match fat.find(d, acts, file_name(&name, &mut buf)?) {
                Ok(f) if f.cluster == first => linked = Some(true), // a cut after 5: done already
                Ok(_) => name = next_name(&name),                   // a name another file has: a second later
                Err(Error::NotFound) => linked = Some(false),
                Err(e) => return Err(e),
            }
            if linked.is_some() {
                break;
            }
        }
        let linked = linked.ok_or(Error::Refused)?; // ten names taken: never "done" without the link
        let dir = fat.open(d, self.kind.rides().1)?;
        if !linked {
            // its rides entry gone: a GIVE UP SAVE's rescue whose record a cut left "saving" is done now, before a
            // `make_room` that could grow a folder on the card that failed
            match fat.locate(d, dir, file_name(&self.name, &mut buf)?) {
                Ok(h) if h.first == first => {}
                Ok(_) | Err(Error::NotFound | Error::Corrupt) => {
                    if let Some(o) = self.rescued(fat, d, first)? {
                        return Ok(o);
                    }
                }
                Err(e) => return Err(e),
            }
            // the entry's room before 4, in the folder it goes to and in the one it leaves (to be named there again if
            // 5 cannot be done: [`Recorder::keep_in_rides`]): a folder that must grow (a cluster) on a full card fails
            // here, the ride still named where it is, never between 4 and 5
            fat.make_room(d, acts, file_name(&name, &mut buf)?)?;
            fat.make_room(d, dir, file_name(&self.name, &mut buf)?)?;
        }
        // 4: the entry in the folder, if still there and still this chain
        match fat.locate(d, dir, file_name(&self.name, &mut buf)?) {
            Ok(h) if h.first == first => fat.remove(d, &h)?,
            Ok(_) | Err(Error::NotFound | Error::Corrupt) => {} // gone, or its sector torn (the record is the name)
            Err(e) => return Err(e),
        }
        // 5: the new entry
        if !linked {
            let h = Handle { first, size, ..Handle::default() };
            fat.link(d, acts, file_name(&name, &mut buf)?, &h, t)?;
        }
        (self.saved, self.name) = (Some((self.name, name)), name);
        // 6
        self.write_record(fat, d, Phase::Done, first)?;
        (self.phase, self.stuck) = (Phase::Done, false); // (made "saving" by a boot's recovery: `saving`)
        Ok(if self.discard { Outcome::Discarded(name) } else { Outcome::Saved(name) })
    }

    /// At boot: a ride the last run did not finish is reopened at its last checkpoint (the app shows it paused),
    /// or its save is finished. Reads only `\Rustfw\ride.rec` when there is nothing to do.
    /// After a card failure, waits [`RETRY`] calls before trying again; gives up after [`GIVE_UP`] in a row, or at
    /// once on any other error (`Kept`: left as it is, and tried once more at the next start).
    /// Nothing once a new ride has started: its first [`Recorder::io`] does this first (one writer, one ride).
    #[inline(never)]
    pub fn recover(&mut self, d: &mut impl DiskWrite, now: Local) -> Outcome {
        if self.phase != Phase::Done {
            return Outcome::Nothing;
        }
        if self.wait > 0 {
            self.wait -= 1;
            return Outcome::Nothing;
        }
        let t = dos_time(now);
        let mut read = false;
        let mut run = |s: &mut Self, d: &mut _| -> Result<Outcome, Error> {
            let mut fat = Fat::mount(d)?;
            fat.sync_fats(d)?; // first: a FAT sector torn by the cut would break the chains read next
            let dir = fat.open(d, DIR)?;
            s.rec = Some(fat.locate(d, dir, s.kind.rec())?);
            let best = s.read_records(&mut fat, d)?;
            read = true;
            s.recover_from(&mut fat, d, best, t, true)
        };
        match run(self, d) {
            Ok(o) => {
                (self.fails, self.stuck) = (0, false);
                o
            }
            Err(e) if self.phase == Phase::Saving => {
                // a save: tried again by [`Recorder::io`] (the phase stays "saving"); given up, the ride named where
                // it was recorded, else stuck (never "done" while the record alone may name it)
                self.errors = self.errors.saturating_add(1);
                if !self.retry(e) {
                    let t = dos_time(now);
                    match Fat::mount(d).and_then(|mut fat| self.give_up(&mut fat, d, t)) {
                        Ok(o) => {
                            self.fails = 0;
                            return o;
                        }
                        Err(_) => self.stuck = true,
                    }
                }
                self.fails = self.fails.saturating_add(1);
                self.wait = self.backoff();
                Outcome::Failed(e)
            }
            Err(Error::NotFound) => Outcome::Nothing, // never recorded here
            Err(e) if !read => {
                // the record not read (the card not mounted, or failing): what it says is unknown, a save cut after
                // its step 4 perhaps, whose chain only the record names. Never concluded: tried again (the boot's
                // marker kept, the ride not settled) and, past its retries, stuck (the drive lent read only)
                self.errors = self.errors.saturating_add(1);
                self.stuck |= !self.retry(e);
                self.fails = self.fails.saturating_add(1);
                self.wait = self.backoff();
                Outcome::Failed(e)
            }
            Err(e) => {
                self.phase = Phase::Done; // whatever was reopened: done again from the record
                self.errors = self.errors.saturating_add(1);
                if self.retry(e) {
                    self.fails += 1;
                    self.wait = RETRY;
                    Outcome::Failed(e)
                } else {
                    self.fails = 0;
                    Outcome::Kept(self.name)
                }
            }
        }
    }

    /// Whether the card's record says a ride is left unfinished (recording or saving; or a card error reading it),
    /// read only: one the boot did not take up (its marker lost, a flat battery), which the next Start does. While
    /// so, the card is not lent writable: a computer could free or reuse the chain the record alone may name (the
    /// review of 2026-10-29, S3).
    #[inline(never)]
    pub fn left_open(&mut self, d: &mut impl DiskWrite) -> bool {
        let read = |s: &mut Self, d: &mut _| -> Result<bool, Error> {
            let mut fat = Fat::mount(d)?;
            let dir = fat.open(d, DIR)?;
            s.rec = Some(fat.locate(d, dir, s.kind.rec())?);
            Ok(s.read_records(&mut fat, d)?.is_some_and(|r| r.phase != Phase::Done))
        };
        match read(self, d) {
            Ok(open) => open,
            // a card error: what the record says is unknown
            Err(Error::Io) => true,
            // never recorded here; or a damaged volume, which the recorder cannot use either: the drive lent
            // writable, for a computer to repair it
            Err(_) => false,
        }
    }

    /// Waiting before trying the card again after a failure.
    pub fn waiting(&self) -> bool {
        self.wait > 0
    }

    fn recover_from(
        &mut self,
        fat: &mut Fat,
        d: &mut impl DiskWrite,
        best: Option<Rec>,
        t: (u16, u16),
        resume: bool,
    ) -> Result<Outcome, Error> {
        if let Some(r) = best.filter(|r| r.phase == Phase::Done && self.phase == Phase::Done) {
            // the last ride's name and whether it was discarded: its full copy, left open by a cut after this one
            // was done, follows it ([`Recorder::resumed_after`])
            (self.name, self.discard) = (r.name, r.discard);
        }
        let Some(r) = best.filter(|r| r.phase != Phase::Done) else { return Ok(Outcome::Nothing) };
        // the FATs were synced by the caller: the cut may have been between the two, or torn one
        let writer = Writer::from_record(&r.fit);
        (self.name, self.ride, self.discard) = (r.name, r.ride, r.discard);
        let size = writer.map_or(0, |w| 16 + w.data_len());
        match (r.phase, writer) {
            (Phase::Saving, Some(w)) => {
                self.writer = w;
                self.saving(r.cluster, size);
                self.save(fat, d, t, r.cluster, size)
            }
            (Phase::Recording, Some(w)) => {
                let dir = fat.open(d, self.kind.rides().1)?;
                let mut buf = [0u8; 16];
                let fits =
                    |h: &Handle, cluster_bytes: u32| h.clusters.checked_mul(cluster_bytes).is_none_or(|b| b >= size);
                let entry = match fat.locate(d, dir, file_name(&r.name, &mut buf)?) {
                    Ok(h) if h.first == r.cluster && fits(&h, fat.cluster_bytes()) => Some(h),
                    // its entry gone: a cut tore its directory sector (a card without reliable write), or a computer
                    // removed it. Then nothing to resume: the chain the record names is saved as it is, if it is
                    // still this ride's and reads back as its whole file, else left (`Kept`)
                    Ok(_) | Err(Error::NotFound | Error::Corrupt) => None,
                    Err(e) => return Err(e),
                };
                let mut file = match entry {
                    Some(h) => h,
                    None => {
                        // a stopped ride's save given up and rescued, its record not done: nothing written on it
                        if let Some(o) = self.rescued(fat, d, r.cluster)? {
                            return Ok(o);
                        }
                        // checked by an earlier boot (its mark), whose repair below a cut may have torn: sector 1
                        // checked instead of sector 0, the repair redone
                        let Some(h) = self.still_ours(fat, d, r.cluster, size, r.checked)? else {
                            // freed, or another file's now: nothing written on it, the record done
                            self.write_record(fat, d, Phase::Done, r.cluster)?;
                            return Ok(Outcome::Nothing);
                        };
                        // named by the record alone: a save from here, never given up as a ride left open
                        self.writer = w;
                        self.saving(r.cluster, size);
                        if r.checked.is_none() {
                            // before the repair's first write; a ride under 3 sectors is not marked: a torn repair
                            // lets it go
                            self.mark_checked(fat, d, r.seq, &h, size)?;
                        }
                        h
                    }
                };
                // the buffer from the sector the trailer starts in, and sector 0: from the record's copies, as they
                // were at its checkpoint (a cut while the next one rewrote them may have torn them in the file)
                (self.writer, self.last_rec, self.skip_to) = (w, w.last_time(), w.last_time());
                self.len = size;
                self.tail_off = (size - 2) / SECTOR_U32 * SECTOR_U32;
                self.tail = [0; TAIL];
                let (rec, slot) = (self.rec.ok_or(Error::NotFound)?, 3 * (r.seq % 2));
                let fits = fits(&file, fat.cluster_bytes());
                let mut s = [0u8; SECTOR];
                fat.read_sector(d, &rec, slot + 1, &mut s)?;
                let keep = (size - self.tail_off) as usize;
                for (x, &b) in self.tail.iter_mut().zip(&s).take(keep) {
                    *x = b;
                }
                if entry.is_none() && fits {
                    fat.write_sectors(d, &mut file, self.tail_off / SECTOR_U32, &s)?;
                    trailer_end(fat, d, &mut file, size, w.trailer())?;
                }
                fat.read_sector(d, &rec, slot, &mut s)?;
                self.head.copy_from_slice(&s[..14]);
                if fits && (self.tail_off > 0 || entry.is_none()) {
                    fat.write_sectors(d, &mut file, 0, &s)?;
                }
                if entry.is_none() {
                    if fits {
                        fat.trim(d, &mut file, size)?; // the clusters the next checkpoint took: free again
                    }
                    self.saving(r.cluster, size);
                    return self.save(fat, d, t, r.cluster, size);
                }
                self.file = Some(file);
                (self.phase, self.ended) = (Phase::Recording, false);
                // the file cut back to the checkpoint, its trailer and header written again from the record
                self.checkpoint(fat, d, t, Phase::Recording)?;
                if !resume {
                    // too old to go on (the second review, R6): ended at its last record, and saved
                    let (last, ride) = (self.writer.last_time(), self.ride);
                    self.end(last, ride);
                    self.phase = Phase::Saving;
                    let o = match self.checkpoint(fat, d, t, Phase::Saving) {
                        Ok(()) => self.save(fat, d, t, file.first, self.len)?,
                        // the card full: the end messages do not fit, the ride is saved as it is
                        Err(Error::Refused) => self.save_as_is(fat, d, t)?,
                        Err(e) => return Err(e),
                    };
                    (self.phase, self.want) = (Phase::Done, false);
                    return Ok(o);
                }
                // back paused, to be ended again: a DISCARD chosen before the cut (a ride ended before its first
                // checkpoint) is asked again, never carried to a later SAVE
                self.discard = false;
                // the time the unit was off is a pause: timer stop at the last record (the app shows it paused)
                if !self.writer.paused() {
                    let last = self.writer.last_time();
                    self.put(16, |w, sink| w.pause(sink, last));
                }
                Ok(Outcome::Resumed(self.ride, self.name))
            }
            _ => {
                // starting (no ride in the file yet), or a record whose FIT state is damaged: done
                self.write_record(fat, d, Phase::Done, r.cluster)?;
                Ok(Outcome::Nothing)
            }
        }
    }
}

#[cfg(test)]
mod tests;
