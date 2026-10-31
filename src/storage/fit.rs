//! The ride file: a FIT activity (Garmin's public FIT protocol), written as the ride goes so that a reset or a
//! power cut never loses more than the data since the last checkpoint (AGENTS.md, "never lose a ride").
//!
//! Layout: a 14-byte header (data size, its own CRC), then definition and data messages (little-endian), then the
//! file CRC. Messages: file_id, event (timer start, stop at a pause, start at a resume), one record a second (only
//! the fields that are present: a new record definition goes out when that set changes), a lap at each Lap key,
//! a gear change event at each shift of an electronic groupset ([`Writer::gear`]), and at the end the last lap,
//! session and activity. Laps and session carry sport cycling (2) and sub_sport indoor_cycling (6) for an indoor
//! ride ([`Sample::profile`]), mountain (8) for an MTB one, road (7) else (FIT profile 21.218, `sub_sport`:
//! github.com/garmin/fit-python-sdk garmin_fit_sdk/profile.py; stock maps its bike type the same way,
//! `Fit_MapBikeTypeToSubSport` 0xd021a704); no sport message.
//!
//! **Checkpoints.** [`Writer`] holds no file data, only where it is: the data length, the running CRC of the data
//! bytes, what is defined, and the ride totals. [`Writer::checkpoint`] writes the file CRC after the data and the
//! header with the current size, flushes, and returns that state as a recovery record ([`RECORD_LEN`] bytes, with
//! its own CRC). From then on, the bytes on the sink are a valid FIT file. Data written after it goes over the
//! trailing CRC and on; the header and the bytes before stay as they are.
//!
//! **Resume.** After a reset, [`Writer::from_record`] with the last record that was stored whole, then
//! [`Writer::checkpoint`]: it writes the trailer and header from the record and cuts the file to that length,
//! which gives the checkpoint's file back exactly (the data before it was never rewritten). Recording goes on.
//! The caller keeps two copies of the record (old / new) and takes the valid one with the larger sequence number
//! (storage/recorder/mod.rs: never the larger [`Writer::data_len`], so a finished ride's record cannot pass for a new ride's).
//! Doing this twice is harmless.
//!
//! **Laps.** A lap carries its own avg / max heart rate (fields 15 / 16, uint8, bpm), cadence (17 / 18, uint8,
//! rpm) and power (19 / 20, uint16, W) over the records written since the last lap (FIT profile 21.218 `lap`,
//! garmin_fit_sdk/profile.py; invalid 0xff / 0xffff, never 0, when no record had the value). The lap definition
//! always had these fields (written invalid before), so no new definition: every file of every build stays as it
//! was. Records are written every running second, auto-paused ones included, never while paused by hand; a lap's
//! averages leave out the auto-paused ones too ([`Sample::auto_paused`]), so they are the lap banner's numbers
//! (`ride::Ride`'s lap power and heart rate, over the moving seconds; until a reset: the banner then starts again
//! from the resume), while the session's cover every record. The lap's sums survive a reset beside the recovery
//! record ([`Writer::lap_words`]: the recorder stores them after the ride words, a record
//! of a build before has zeros there and its lap averages from the resume). The last lap of a ride with one lap
//! is the session's values.
//!
//! **The record across builds.** Its third word holds the record mask (bits 0..9), bit 11 (MTB), bit 12 (the full
//! copy describes `ArtPct`: `ARTIFACTS`), bit 13 (the profile set by hand), bit 14 (this layout: lap and session with total_ascent, sport and sub_sport), bit 15 (indoor), bit 16
//! (developer fields), the ascent (17..30) and bit 31 (the build before sub_sport: total_ascent, no sport). A record
//! without this layout reads as a road ride and has its lap and session defined again. This build never sets bit
//! 31, so the build before, resuming its ride, defines its own lap and session again too. The builds before the
//! ride profile (706e1ca) read bits 0..15 as the mask and only compare it with the next record's
//! (`mask != self.mask`), so bits 11 to 15 there only define the record message again at the next second (a later
//! definition of a local type replaces the earlier one: FIT protocol); from 706e1ca on, builds read the mask as bits
//! 0..9 (`& 0x3ff`), so the ones before MTB ignore bit 11. Either way a build before MTB resumes an MTB ride as a
//! road ride: its lap and session say road, nothing is lost. A build before total_ascent reads neither layout bit
//! and is not covered (as before).
//!
//! The profile ([`Sample::profile`], [`Sample::by_hand`]) is the last second's, or the one given since by
//! [`Writer::set_profile`] (a change while paused): what the end and the record carry.
//!
//! **Errors.** A failed write leaves the writer where it was (the next message goes to the same offset), so a
//! failing sink loses samples, never the file. Nothing here panics: buffers are fixed and accessed with checks.
//!
//! The file CRC covers the header too, and the header changes with the size: [`crc_combine`] joins the header's
//! CRC to the running data CRC in about 400 steps, whatever the file length, so a checkpoint never reads the file.
//!
//! **Developer fields** ([`Writer::begin_with`] `dev`): the full copy of a ride (storage/recorder/mod.rs) is protocol 2.0 with,
//! after file_id, a `developer_data_id` (207: application_id, developer_data_index 0) and three `field_description`s
//! (206: developer_data_index, field_definition_number, fit_base_type_id, field_name, units, native_mesg_num 20),
//! `Alpha1` (float32, no units: the name Intervals.icu reads DFA a1 from, as AlphaHRV writes it,
//! forum.intervals.icu/t/average-dfa-a1-and-wbal-added-to-interval-fields/7528), `RMSSD` (uint16, ms) and
//! `ArtPct` (uint8, %: the share of beats hrv.rs's 5 % rule dropped over the DFA window); every record then
//! carries them (invalid when not known), its definition with the developer flag (header bit 5) and (field number,
//! size, developer data index) per field. A full copy begun by a build before `ArtPct` (its recovery record
//! without `ARTIFACTS`) goes on with two fields: its file has no description of the third.
//!
//! **The trainer** ([`Sample::trainer`], the full copy only, a file that describes `ArtPct`): while a trainer is
//! connected, to diagnose ERG (the target, what the trainer and the meter say, what went out and failed), every record
//! also carries eight more developer fields after those three, numbers 3 to 10 of developer data index 0:
//! `target_w` (uint16, W: the ERG target the trainer should hold, invalid when not in ERG), `trainer_w` (uint16, W: the trainer's own power),
//! `meter_w` (uint16, W: the power meter's, invalid when not heard), `fe_state` (uint8: FE-C page 25 byte 7 bits
//! 4-6), `trn_stat` (uint8: page 25 byte 6 bits 4-6, the trainer status, in bits 4-6; byte 7 bits 0-1, the target
//! power limits, in bits 0-1), `link` (uint8: 0 ANT+, 1 Bluetooth), `cmd_sent` (uint8: [`cmd_bit`] of each control
//! page sent since the last record) and `ack_fail` (uint8: the ANT TX_FAILED events on the trainer's channel since
//! the last record). 11 bytes a second. Their `field_description`s (a 16-byte name, with their own definition, local
//! type 10: [`TRN_MESSAGES`]) go in at the first such second, just before its record, once a file (bit 10 of the
//! recovery record's third word: a build before it ignores the bit, or reads it into its mask, which only defines
//! the record again); a resume after a build that dropped the bit describes them again, which readers take (a later
//! description of a field replaces the earlier one). A second without a trainer is the three-field record (a new
//! definition at each change). With the trainer's common page 80 heard, a `device_info` (global 23: device_index 1,
//! device_type antplus fitness_equipment (17) or ble bike_trainer (7), manufacturer, serial_number, product (the model number), software_version
//! (scale 100), hardware_version, source_type antplus (1) or bluetooth_low_energy (3); garmin_fit_sdk/profile.py
//! 21.218) goes before the first record that has it, once a run of the writer (a resume may write it again).
//!
//! **Beat to beat** ([`Writer::hrv`], the full copy only): every RR interval the strap's decoder gives, raw (before
//! hrv.rs's artifact rule) but within its range (`sensors::Beats::RANGE`, 250 to 2000 ms: a value outside is a
//! glitch and is not written), in standard `hrv` messages (global 78, field 0 `time`: an array of uint16, scale
//! 1000, units s, so the value is ms; 0xffff invalid: garmin_fit_sdk/profile.py 21.218, `hrv`), five to a message
//! with the unused slots invalid, written between the records as the beats come, as Garmin's units and
//! bryton-S510-mods' `P8` write them (its docs/how-it-works.md, "HRV recording"). Tools that compute DFA a1 offline
//! (Domestique, Runalyze, Kubios) read the beats from there. The messages carry no time: readers line them up from
//! the ride's start, and a beat the decoder could not time (a broadcast missed with more than one beat in it) or the
//! strap's restarts (silent, then back) leave gaps, not filled, where that sum drifts. The clean copy has none
//! (Strava and Bryton: no file seen to show they take them).
//!
//! Message and field numbers (developer data and hrv): `garmin_fit_sdk/profile.py` 21.218
//! (github.com/garmin/fit-python-sdk), the protocol: developer.garmin.com/fit/protocol. Without `dev` the
//! file is byte for byte what it was: Bryton's server refuses developer fields in records (bryton-S510-mods
//! docs/how-it-works.md, "Such rides do not sync to Bryton").
//!
//! **Independent check** (fitdecode, a Python FIT decoder, and Garmin's FIT SDK for Python): `cargo test-host fit_sample` writes a 20-minute
//! synthetic ride to `target/fit/sample.fit` and its checkpoint-cut copy to `target/fit/cut.fit`, its full copy (developer fields) to
//! `target/fit/sample_full.fit`, then
//!
//! ```text
//! python3 -m venv /tmp/fitvenv && /tmp/fitvenv/bin/pip install fitdecode garmin-fit-sdk
//! /tmp/fitvenv/bin/python tools/fit_check.py target/fit/sample.fit target/fit/cut.fit target/fit/sample_full.fit
//! ```
#![cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::cast_possible_truncation))]

use crate::{epoch_s, ride};

/// A storage failure (the sink could not write, cut, or flush).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error;

/// Where the file goes: a file that can be written at any offset, cut to a length, and flushed to the medium.
pub trait Sink {
    /// Writes `data` at `offset`, growing the file if needed.
    fn write_at(&mut self, offset: u32, data: &[u8]) -> Result<(), Error>;
    /// Sets the file length (cuts what is beyond).
    fn set_len(&mut self, len: u32) -> Result<(), Error>;
    /// Everything written so far is on the medium when this returns Ok.
    fn flush(&mut self) -> Result<(), Error>;
}

/// FIT time (seconds since 1989-12-31 00:00 UTC) of a UTC date and time, as the GNSS gives it.
///
/// ```
/// assert_eq!(bryton_rustfw::fit::fit_time((1989, 12, 31, 0, 0, 0)), 0);
/// assert_eq!(bryton_rustfw::fit::fit_time((2026, 10, 23, 8, 0, 0)), 1_161_676_800);
/// ```
pub fn fit_time(utc: (u16, u8, u8, u8, u8, u8)) -> u32 {
    u32::try_from((epoch_s(utc) - 631_065_600).clamp(0, i64::from(u32::MAX))).unwrap_or(0)
}

/// One second of ride, as the record message takes it. None: not available (the field is left out).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    /// FIT time ([`fit_time`]).
    pub time: u32,
    /// Latitude and longitude, degrees x 1e-7 (NAV-PVT).
    pub pos: Option<(i32, i32)>,
    /// Altitude above mean sea level, mm.
    pub alt_mm: Option<i32>,
    /// Speed, mm/s. The distance is its sum.
    pub speed_mm_s: Option<u32>,
    /// Heart rate, beats per minute.
    pub hr: Option<u8>,
    /// Cadence, rpm.
    pub cadence: Option<u8>,
    /// Power, W.
    pub power: Option<u16>,
    /// Temperature, degrees C.
    pub temp_c: Option<i8>,
    /// DFA alpha1 x 1000 (hrv.rs): the full copy's `Alpha1` developer field only.
    pub alpha1_milli: Option<u16>,
    /// RMSSD, ms (hrv.rs): the full copy's `RMSSD` developer field only.
    pub rmssd_ms: Option<u16>,
    /// Beats dropped by hrv.rs's artifact rule over its DFA window, % (hrv.rs): the full copy's `ArtPct` only.
    pub artifacts_pct: Option<u8>,
    /// The ride's ascent so far, m (ride.rs): the lap's and the session's total_ascent.
    pub ascent_m: Option<u32>,
    /// The trainer this second, while one is connected: the full copy's trainer fields only (module doc).
    pub trainer: Option<TrainerSample>,
    /// The ride's profile this second: the lap's and the session's sub_sport, the last second's at the end (a ride
    /// the trainer left goes back to ROAD or MTB).
    pub profile: ride::Profile,
    /// The rider set that profile by hand (the app's `profile_by_hand`): only kept in the recovery record, so a
    /// resumed ride is not given back to the automatic choice.
    pub by_hand: bool,
    /// Auto-paused this second (the app's `Ride::moving` false): recorded, but left out of the lap's averages, as
    /// the lap banner leaves it out (module doc, "Laps"). False (the default): a moving second.
    pub auto_paused: bool,
}

/// One second of the trainer (module doc, "The trainer"): plain values the app took from the trainer's reading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrainerSample {
    /// The ERG target the trainer should hold this second, W (None: not in ERG). Whether it went out is `cmds`.
    pub target_w: Option<u16>,
    /// The trainer's own power, W.
    pub trainer_w: Option<u16>,
    /// The power meter's power, W (None: not heard).
    pub meter_w: Option<u16>,
    /// The FE state (FE-C page 25 byte 7 bits 4-6: 1 off, 2 ready, 3 in use, 4 finished).
    pub fe_state: u8,
    /// The trainer status bits (page 25 byte 6 bits 4-6) in bits 4-6, the target power limits (byte 7 bits 0-1)
    /// in bits 0-1.
    pub status: u8,
    /// The link is Bluetooth (FTMS or FE-C over BLE), else ANT+.
    pub ble: bool,
    /// The control pages sent since the last record, [`cmd_bit`] each.
    pub cmds: u8,
    /// TX_FAILED events on the trainer's ANT channel since the last record.
    pub ack_fails: u8,
    /// The trainer's common pages 80 and 81, once page 80 was heard.
    pub device: Option<TrainerDevice>,
}

/// The trainer's manufacturer and product pages (ANT+ common pages 80 and 81), for the `device_info` message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrainerDevice {
    /// Manufacturer ID (page 80 bytes 4-5).
    pub maker: u16,
    /// Model number (page 80 bytes 6-7).
    pub model: Option<u16>,
    /// Hardware revision (page 80 byte 3).
    pub hw: Option<u8>,
    /// Serial number (page 81 bytes 4-7).
    pub serial: Option<u32>,
    /// Software version x 100 (page 81 bytes 2-3, as `fec::decode` reads them).
    pub sw_x100: Option<u16>,
}

/// The `cmd_sent` bit of a control page sent to the trainer: 1 basic resistance (0x30), 2 target power (0x31),
/// 4 track resistance (0x33), 8 anything else (the set-up: user configuration, a data request).
///
/// ```
/// assert_eq!([0x30, 0x31, 0x33, 0x37, 0x46].map(bryton_rustfw::fit::cmd_bit), [1, 2, 4, 8, 8]);
/// ```
pub fn cmd_bit(page: u8) -> u8 {
    match page {
        0x30 => 1,
        0x31 => 2,
        0x33 => 4,
        _ => 8,
    }
}

/// Size of a recovery record ([`Writer::checkpoint`]).
pub const RECORD_LEN: usize = 4 * WORDS + 4;
/// Words of the current lap's sums ([`Writer::lap_words`]), stored beside the recovery record.
pub const LAP_WORDS: usize = 9;

/// The ride's ascent in a recovery record: bits 17..30 of its third word, m (16383 m at most).
const ASCENT_MAX: u32 = 0x3fff;
/// Bit 31 of that word: the record's file has lap and session with total_ascent and no sport (the build before
/// sub_sport sets it; this one never does, so that build, resuming this one's ride, defines its lap and session
/// again instead of writing them on this layout).
const LAYOUT_ASCENT: u32 = 1 << 31;
/// Bit 14 of that word (the record mask's bits 10..15 are never used): the record's file has lap and session with
/// total_ascent, sport and sub_sport. A build before reads it as part of the mask, which only defines the record
/// again at the next second.
const LAYOUT_SPORT: u32 = 1 << 14;
/// Bit 15: the ride is indoors (sub_sport indoor_cycling). Records before it: 0, a road ride.
const INDOOR: u32 = 1 << 15;
/// Bit 11: an MTB ride (sub_sport mountain); never with [`INDOOR`]. Records before it: 0, a road ride. Older builds
/// ignore it or take it as part of the mask (module doc).
const MTB: u32 = 1 << 11;
/// Bit 13: the profile was set by hand. Records before it: 0, automatic. Older builds: as bit 14 (module doc).
const BY_HAND: u32 = 1 << 13;

const HEADER: u32 = 14;
// written as a byte (the header's size field): a larger header fails the build, never the file
const _: () = assert!(HEADER < 256);
const WORDS: usize = 21;
const TAG: u16 = 0xf17f; // first two bytes of a recovery record (and a version: change it with the layout)

// base types (FIT profile)
const ENUM: u8 = 0x00;
const S8: u8 = 0x01;
const U8: u8 = 0x02;
const U16: u8 = 0x84;
const S32: u8 = 0x85;
const U32: u8 = 0x86;
const U32Z: u8 = 0x8c;
const STRING: u8 = 0x07;
const BYTE: u8 = 0x0d;
const F32: u8 = 0x88;

/// (field number, size, base type)
type Field = (u8, u8, u8);

// local message types
const L_FILE_ID: u8 = 0;
const L_EVENT: u8 = 1;
const L_RECORD: u8 = 2;
const L_LAP: u8 = 3;
const L_SESSION: u8 = 4;
const L_ACTIVITY: u8 = 5;
const L_DEV_ID: u8 = 6;
const L_FIELD_DESC: u8 = 7;
const L_GEAR: u8 = 8;
const L_HRV: u8 = 9;
const L_TRN_DESC: u8 = 10;
const L_DEVICE: u8 = 11;
/// RR intervals per `hrv` message (module doc).
pub const HRV_PER_MESSAGE: usize = 5;
// the hrv message's field size, 2 bytes a value, is written as a byte
const _: () = assert!(2 * HRV_PER_MESSAGE < 256);
/// Bytes an `hrv` message takes at most, its definition with it.
pub const HRV_MAX: usize = 9 + 1 + 2 * HRV_PER_MESSAGE;
/// Bit 12 of the recovery record's third word (the record mask's bits 10..12 were never used): the full copy's
/// file describes `ArtPct` (module doc).
const ARTIFACTS: u32 = 1 << 12;

/// The full copy's developer data, after file_id (module doc): developer_data_id, then a field_description each
/// for `Alpha1`, `RMSSD` and `ArtPct`, definitions included.
#[allow(clippy::indexing_slicing)] // evaluated by the compiler: an index out of range fails the build, never the unit
const DEV_MESSAGES: [u8; 108] = {
    const APP: &[u8; 16] = b"bryton-rustfw-01"; // application_id: any 16 bytes that name this firmware
    let mut m = [0u8; 108];
    let head: [u8; 12] = [0x40 | L_DEV_ID, 0, 0, 207, 0, 2, 1, 16, BYTE, 3, 1, U8];
    let def: [u8; 24] =
        [0x40 | L_FIELD_DESC, 0, 0, 206, 0, 6, 0, 1, U8, 1, 1, U8, 2, 1, U8, 3, 8, STRING, 8, 4, STRING, 14, 2, U16];
    // index, number, base type, name (8), units (4), native_mesg_num record (20)
    let alpha: [u8; 18] = [L_FIELD_DESC, 0, 0, F32, b'A', b'l', b'p', b'h', b'a', b'1', 0, 0, 0, 0, 0, 0, 20, 0];
    let rmssd: [u8; 18] = [L_FIELD_DESC, 0, 1, U16, b'R', b'M', b'S', b'S', b'D', 0, 0, 0, b'm', b's', 0, 0, 20, 0];
    let art: [u8; 18] = [L_FIELD_DESC, 0, 2, U8, b'A', b'r', b't', b'P', b'c', b't', 0, 0, b'%', 0, 0, 0, 20, 0];
    let mut i = 0;
    while i < 108 {
        m[i] = match i {
            0..12 => head[i],
            12 => L_DEV_ID,
            13..29 => APP[i - 13],
            29 => 0, // developer_data_index
            30..54 => def[i - 30],
            54..72 => alpha[i - 54],
            72..90 => rmssd[i - 72],
            _ => art[i - 90],
        };
        i += 1;
    }
    m
};
/// The developer fields of a full copy's record: (field_definition_number, size, developer_data_index): `Alpha1`,
/// `RMSSD`, `ArtPct`, then the trainer's (module doc).
const DEV_RECORD: [Field; 11] = [
    (0, 4, 0),
    (1, 2, 0),
    (2, 1, 0),
    (3, 2, 0),
    (4, 2, 0),
    (5, 2, 0),
    (6, 1, 0),
    (7, 1, 0),
    (8, 1, 0),
    (9, 1, 0),
    (10, 1, 0),
];
/// Bit 10 of the recovery record's third word (the record mask's bits 10..12 were never used): the file describes
/// the trainer's fields (module doc).
const TRN_DESC: u32 = 1 << 10;
/// A trainer field's `field_description` (local [`L_TRN_DESC`]): index 0, `num`, base type `ty`, name (16 bytes),
/// units (4), native_mesg_num record (20).
#[allow(clippy::indexing_slicing)] // evaluated by the compiler: an index out of range fails the build, never the unit
const fn trn_desc(num: u8, ty: u8, name: &[u8], units: &[u8]) -> [u8; 26] {
    let mut d = [0u8; 26];
    (d[0], d[2], d[3], d[24]) = (L_TRN_DESC, num, ty, 20);
    let mut i = 0;
    while i < name.len() {
        d[4 + i] = name[i];
        i += 1;
    }
    i = 0;
    while i < units.len() {
        d[20 + i] = units[i];
        i += 1;
    }
    d
}
/// The trainer fields' descriptions, their definition first (module doc, "The trainer").
#[allow(clippy::indexing_slicing)] // evaluated by the compiler, as above
pub const TRN_MESSAGES: [u8; 24 + 8 * 26] = {
    let def: [u8; 24] =
        [0x40 | L_TRN_DESC, 0, 0, 206, 0, 6, 0, 1, U8, 1, 1, U8, 2, 1, U8, 3, 16, STRING, 8, 4, STRING, 14, 2, U16];
    let descs: [[u8; 26]; 8] = [
        trn_desc(3, U16, b"target_w", b"W"),
        trn_desc(4, U16, b"trainer_w", b"W"),
        trn_desc(5, U16, b"meter_w", b"W"),
        trn_desc(6, U8, b"fe_state", b""),
        trn_desc(7, U8, b"trn_stat", b""),
        trn_desc(8, U8, b"link", b""),
        trn_desc(9, U8, b"cmd_sent", b""),
        trn_desc(10, U8, b"ack_fail", b""),
    ];
    let mut m = [0u8; 24 + 8 * 26];
    let mut i = 0;
    while i < m.len() {
        m[i] = if i < 24 { def[i] } else { descs[(i - 24) / 26][(i - 24) % 26] };
        i += 1;
    }
    m
};
/// timestamp, device_index, device_type, manufacturer, serial_number (uint32z), product, software_version,
/// hardware_version, source_type (module doc).
const DEVICE: [Field; 9] = [
    (253, 4, U32),
    (0, 1, U8),
    (1, 1, U8),
    (2, 2, U16),
    (3, 4, U32Z),
    (4, 2, U16),
    (5, 2, U16),
    (6, 1, U8),
    (25, 1, ENUM),
];
/// Bytes a `device_info` takes, its definition with it.
const DEVICE_MAX: usize = 6 + 3 * 9 + 1 + 19;

/// FIT manufacturer and product of the ride files: Bryton (267), the Rider S510 (2404), as stock's own files.
pub const MANUFACTURER: u32 = 267;
/// See [`MANUFACTURER`].
pub const PRODUCT: u32 = 2404;
const FILE_ID: [Field; 4] = [(0, 1, ENUM), (1, 2, U16), (2, 2, U16), (4, 4, U32)];
const EVENT: [Field; 3] = [(253, 4, U32), (0, 1, ENUM), (1, 1, ENUM)];
/// [`EVENT`] with its data field (3, gear_change_data: rear_gear_num in bits 0..7, rear_gear (teeth) 8..15,
/// front_gear_num 16..23, front_gear 24..31; garmin_fit_sdk/profile.py and fitdecode's profile, its components).
const GEAR_EVENT: [Field; 4] = [(253, 4, U32), (0, 1, ENUM), (1, 1, ENUM), (3, 4, U32)];
/// timestamp, position_lat, position_long, altitude, speed, distance, heart_rate, cadence, power, temperature: the
/// mask bits of [`Writer`] in this order.
const RECORD: [Field; 10] = [
    (253, 4, U32),
    (0, 4, S32),
    (1, 4, S32),
    (2, 2, U16),
    (6, 2, U16),
    (5, 4, U32),
    (3, 1, U8),
    (4, 1, U8),
    (7, 2, U16),
    (13, 1, S8),
];
/// timestamp, start_time, total_elapsed_time, total_timer_time, total_distance, avg / max speed, heart rate,
/// cadence, power, event, event_type, total_ascent (m), sport, sub_sport. Lap and session take the same values;
/// the field numbers differ (FIT profile: total_ascent lap 21, session 22; sport lap 25, session 5; sub_sport lap
/// 39, session 6; garmin_fit_sdk/profile.py 21.218).
const LAP: [Field; 18] = [
    (253, 4, U32),
    (2, 4, U32),
    (7, 4, U32),
    (8, 4, U32),
    (9, 4, U32),
    (13, 2, U16),
    (14, 2, U16),
    (15, 1, U8),
    (16, 1, U8),
    (17, 1, U8),
    (18, 1, U8),
    (19, 2, U16),
    (20, 2, U16),
    (0, 1, ENUM),
    (1, 1, ENUM),
    (21, 2, U16),
    (25, 1, ENUM),
    (39, 1, ENUM),
];
/// [`LAP`]'s fields, then first_lap_index, num_laps.
const SESSION: [Field; 20] = [
    (253, 4, U32),
    (2, 4, U32),
    (7, 4, U32),
    (8, 4, U32),
    (9, 4, U32),
    (14, 2, U16),
    (15, 2, U16),
    (16, 1, U8),
    (17, 1, U8),
    (18, 1, U8),
    (19, 1, U8),
    (20, 2, U16),
    (21, 2, U16),
    (0, 1, ENUM),
    (1, 1, ENUM),
    (22, 2, U16),
    (5, 1, ENUM),
    (6, 1, ENUM),
    (25, 2, U16),
    (26, 2, U16),
];
const ACTIVITY: [Field; 6] = [(253, 4, U32), (0, 4, U32), (1, 2, U16), (2, 1, ENUM), (3, 1, ENUM), (4, 1, ENUM)];

/// Sum, count and maximum of one sensor value over the ride.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Stat {
    sum: u32,
    n: u32,
    max: u32,
}

impl Stat {
    fn add(&mut self, v: Option<u32>) {
        if let Some(v) = v {
            self.sum = self.sum.saturating_add(v);
            self.n = self.n.saturating_add(1);
            self.max = self.max.max(v);
        }
    }

    /// (average, maximum), u32::MAX (FIT's invalid, once cut to the field size) when never seen.
    fn avg_max(&self) -> (u32, u32) {
        if self.n == 0 { (u32::MAX, u32::MAX) } else { (self.sum / self.n, self.max) }
    }
}

/// The ride file's state: everything needed to go on writing it, small enough to be the recovery record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Writer {
    /// Bytes of messages written (the file is header, these, CRC).
    len: u32,
    /// CRC of those bytes.
    crc: u16,
    /// Local message types whose definition is in the file.
    defined: u16,
    /// [`RECORD`] fields in the current record definition.
    mask: u16,
    start: u32,
    last: u32,
    dist_mm: u32,
    max_speed: u32,
    /// heart rate, cadence, power
    stats: [Stat; 3],
    /// The same over the current lap's records (module doc, "Laps"); not in the recovery record, in
    /// [`Writer::lap_words`].
    lap_stats: [Stat; 3],
    /// The current lap's start (FIT time) and distance then (mm); laps ended so far.
    lap_start: u32,
    lap_dist: u32,
    laps: u32,
    /// Seconds paused (Start key) so far; when the current pause began (0: not paused).
    paused_s: u32,
    paused_at: u32,
    /// The full copy: protocol 2.0, developer fields in the records (module doc).
    dev: bool,
    /// Its file describes `ArtPct` (a full copy begun by a build before it does not: module doc).
    art: bool,
    /// The ride's ascent so far, m (the samples' [`Sample::ascent_m`]).
    ascent_m: u32,
    /// The profile (the last sample's [`Sample::profile`]).
    profile: ride::Profile,
    /// That profile was set by hand (the last sample's [`Sample::by_hand`]).
    by_hand: bool,
    /// The file describes the trainer's fields (module doc).
    trn_desc: bool,
    /// The record definition in force has them (not in the recovery record: a resumed full copy with `ArtPct`
    /// defines its record again).
    trn: bool,
    /// The trainer's `device_info` was written by this writer.
    device: bool,
}

impl Writer {
    /// Nothing written yet (`Writer::begin_with` starts a file).
    pub const fn new() -> Writer {
        const S: Stat = Stat { sum: 0, n: 0, max: 0 };
        Writer {
            len: 0,
            crc: 0,
            defined: 0,
            mask: 0,
            start: 0,
            last: 0,
            dist_mm: 0,
            max_speed: 0,
            stats: [S; 3],
            lap_stats: [S; 3],
            lap_start: 0,
            lap_dist: 0,
            laps: 0,
            paused_s: 0,
            paused_at: 0,
            dev: false,
            art: false,
            ascent_m: 0,
            profile: ride::Profile::Road,
            by_hand: false,
            trn_desc: false,
            trn: false,
            device: false,
        }
    }

    /// Starts a ride file at FIT time `time`: header, file_id, timer start, then a checkpoint (its record is
    /// returned). The sink should be empty.
    #[cfg(test)]
    pub fn start(sink: &mut impl Sink, time: u32) -> Result<(Writer, [u8; RECORD_LEN]), Error> {
        let mut w = Writer::begin(sink, time)?;
        let rec = w.checkpoint(sink)?;
        Ok((w, rec))
    }

    /// [`Writer::start`] without its checkpoint: file_id and timer start only (the recorder buffers them in RAM
    /// and checkpoints once the file exists).
    #[cfg(test)]
    pub fn begin(sink: &mut impl Sink, time: u32) -> Result<Writer, Error> {
        Writer::begin_with(sink, time, false)
    }

    /// Like `Writer::begin`, with developer fields when `dev` (the full copy: module doc).
    pub fn begin_with(sink: &mut impl Sink, time: u32, dev: bool) -> Result<Writer, Error> {
        let mut w = Writer { start: time, last: time, lap_start: time, dev, art: dev, ..Writer::default() };
        // the hardware as it is: manufacturer 267 (bryton, garmin_fit_sdk profile.py), product 2404 (the Rider S510:
        // what stock R039's ride files carry; Intervals.icu shows them as "BRYTON 2404"). Was 255 (development),
        // which services showed as "DEVELOPMENT" and do not trust for barometric altitude (owner's ask, 2026-10-26)
        w.message(sink, L_FILE_ID, 0, &FILE_ID, &[4, MANUFACTURER, PRODUCT, time])?;
        if dev {
            w.put(sink, &DEV_MESSAGES)?;
        }
        w.event(sink, time, 0)?;
        Ok(w)
    }

    /// The Start key paused the ride at FIT time `time`: timer stop.
    pub fn pause(&mut self, sink: &mut impl Sink, time: u32) -> Result<(), Error> {
        self.last = self.last.max(time);
        self.event(sink, self.last, 4)?;
        self.paused_at = self.last;
        Ok(())
    }

    /// Resumed at `time`: timer start; the pause is left out of the timer time.
    pub fn resume(&mut self, sink: &mut impl Sink, time: u32) -> Result<(), Error> {
        self.last = self.last.max(time);
        self.event(sink, self.last, 0)?;
        if self.paused_at != 0 {
            self.paused_s = self.paused_s.saturating_add(self.last.saturating_sub(self.paused_at));
            self.paused_at = 0;
        }
        Ok(())
    }

    /// A lap ended at `time` (the Lap key): a lap message from the last one, with the lap's own heart rate, cadence
    /// and power (module doc, "Laps").
    pub fn lap(&mut self, sink: &mut impl Sink, time: u32) -> Result<(), Error> {
        self.last = self.last.max(time);
        let v = self.this_lap();
        self.message(sink, L_LAP, 19, &LAP, &v)?;
        (self.lap_start, self.lap_dist, self.laps) = (self.last, self.dist_mm, self.laps + 1);
        self.lap_stats = [Stat::default(); 3];
        Ok(())
    }

    /// [`Writer::lap_values`] with the lap's own averages and maxima instead of the ride's; its ascent is not kept
    /// (left out).
    fn this_lap(&self) -> [u32; 18] {
        let mut v = self.lap_values();
        for (k, (a, m)) in self.lap_stats.map(|s| s.avg_max()).into_iter().enumerate() {
            if let Some(s) = v.get_mut(7 + 2 * k..9 + 2 * k) {
                s.copy_from_slice(&[a, m]);
            }
        }
        v[15] = u32::MAX;
        v
    }

    /// The current lap's sums for the recovery record (module doc, "Laps"): heart rate, cadence, power, each sum,
    /// count, maximum. Kept out of [`Writer::to_record`], whose size the records of every build share.
    pub fn lap_words(&self) -> [u32; LAP_WORDS] {
        let [a, b, c] = self.lap_stats;
        [a.sum, a.n, a.max, b.sum, b.n, b.max, c.sum, c.n, c.max]
    }

    /// This writer with the lap sums [`Writer::lap_words`] gave (all zeros in a record of a build before them: the
    /// lap then averages from the resume).
    pub fn with_lap_words(self, w: &[u32; LAP_WORDS]) -> Writer {
        let s = |[sum, n, max]: [u32; 3]| Stat { sum, n, max };
        let [a, b, c, d, e, f, g, h, i] = *w;
        Writer { lap_stats: [s([a, b, c]), s([d, e, f]), s([g, h, i])], ..self }
    }

    /// A gear change at `time` (the front one if `front`, else the rear): event front_gear_change (42) or
    /// rear_gear_change (43), type marker (3), with the gears after it, from 1 (0: none, a 1x system's front). The
    /// data field's components (FIT profile, gear_change_data): rear_gear_num, rear_gear, front_gear_num, front_gear
    /// from the low byte up, all uint8z, so the teeth, not known, are 0 (invalid), never 0xff (255 teeth: review of
    /// 571e613). Stock writes the same events (`Fit_WriteGearChangeEvent` 0xd021b9a4).
    pub fn gear(&mut self, sink: &mut impl Sink, time: u32, front: bool, front_n: u8, rear_n: u8) -> Result<(), Error> {
        self.last = self.last.max(time);
        let data = u32::from(front_n) << 16 | u32::from(rear_n);
        self.message(sink, L_GEAR, 21, &GEAR_EVENT, &[self.last, if front { 42 } else { 43 }, 3, data])
    }

    /// [`LAP`]'s values for the lap from `lap_start` to `last`: elapsed and timer time and distance, with the
    /// ride's averages and maxima (the lap's when it is the only one).
    fn lap_values(&self) -> [u32; 18] {
        let pausing =
            if self.paused_at != 0 { self.last.saturating_sub(self.paused_at.max(self.lap_start)) } else { 0 };
        let elapsed = self.last.saturating_sub(self.lap_start);
        // ponytail: pauses are counted for the ride, not per lap: a lap after the first leaves out only the pause
        // in progress
        let paused = if self.lap_start == self.start { self.paused_s + pausing } else { pausing };
        let timer = elapsed.saturating_sub(paused);
        let dist = self.dist_mm.saturating_sub(self.lap_dist);
        let avg_speed = if timer == 0 { 0 } else { dist / timer };
        let [(ah, mh), (ac, mc), (ap, mp)] = self.stats.map(|s| s.avg_max());
        [
            self.last,
            self.lap_start,
            elapsed.saturating_mul(1000),
            timer.saturating_mul(1000),
            dist / 10,
            avg_speed.min(0xfffe),
            self.max_speed.min(0xfffe),
            ah,
            mh,
            ac,
            mc,
            ap,
            mp,
            9, // event lap
            1, // stop
            self.ascent_m.min(0xfffe),
            2, // cycling
            // sub_sport (module doc)
            match self.profile {
                ride::Profile::Road => 7,
                ride::Profile::Indoor => 6,
                ride::Profile::Mtb => 8,
                // garmin fit-python-sdk garmin_fit_sdk/profile.py types["sub_sport"]: cyclocross 11, gravel_cycling 46, commuting 48
                ride::Profile::Gravel => 46,
                ride::Profile::Cx => 11,
                ride::Profile::Commute => 48,
            },
        ]
    }

    /// Appends one record. The totals count it even if the write fails.
    pub fn record(&mut self, sink: &mut impl Sink, s: &Sample) -> Result<(), Error> {
        // ponytail: dt capped at 5 s, so a gap (a reset, a lost fix) adds no distance; no auto-pause yet
        let dt = s.time.saturating_sub(self.last).min(5);
        self.last = self.last.max(s.time);
        if let Some(v) = s.speed_mm_s {
            self.dist_mm = self.dist_mm.saturating_add(v.saturating_mul(dt));
            self.max_speed = self.max_speed.max(v);
        }
        if let Some(a) = s.ascent_m {
            self.ascent_m = self.ascent_m.max(a.min(ASCENT_MAX));
        }
        self.set_profile(s.profile, s.by_hand);
        // the session's over every record; the lap's over the moving ones, as the lap banner (module doc)
        let lap: &mut [Stat] = if s.auto_paused { &mut [] } else { &mut self.lap_stats };
        for st in [&mut self.stats[..], lap] {
            if let [h, c, p] = st {
                h.add(s.hr.map(u32::from));
                c.add(s.cadence.map(u32::from));
                p.add(s.power.map(u32::from));
            }
        }

        let (lat, lon) = s.pos.map_or((None, None), |(a, o)| (Some(semicircles(a)), Some(semicircles(o))));
        // altitude: (m + 500) x 5, rounded
        let alt = s.alt_mm.map(|mm| ((i64::from(mm) + 500_100) / 200).clamp(0, 0xfffe) as u32);
        let speed = s.speed_mm_s.map(|v| v.min(0xfffe));
        let dist = s.speed_mm_s.map(|_| self.dist_mm / 10);
        let (hr, cad, power) = (s.hr.map(u32::from), s.cadence.map(u32::from), s.power.map(u32::from));
        let temp = s.temp_c.map(|t| u32::from(t as u8));
        let all = [Some(s.time), lat, lon, alt, speed, dist, hr, cad, power, temp];
        let mut mask = 0u16;
        let (mut fields, mut values, mut n) = ([(0, 0, 0); 10], [0u32; 10], 0);
        for (i, (f, v)) in RECORD.iter().zip(all).enumerate() {
            if let (Some(v), Some(fs), Some(vs)) = (v, fields.get_mut(n), values.get_mut(n)) {
                (*fs, *vs) = (*f, v);
                n += 1;
                mask |= 1 << i;
            }
        }
        // the trainer's fields only in a full copy that describes `ArtPct` (they follow it)
        let trn = s.trainer.filter(|_| self.dev && self.art);
        if mask != self.mask || trn.is_some() != self.trn {
            self.defined &= !(1 << L_RECORD);
        }
        let (fields, values) = (fields.get(..n).unwrap_or(&[]), values.get(..n).unwrap_or(&[]));
        if self.dev {
            if let Some(t) = trn {
                // their descriptions before the first record that has them, in the same checkpoint
                if !self.trn_desc {
                    self.put(sink, &TRN_MESSAGES)?;
                    self.trn_desc = true;
                }
                if let (false, Some(d)) = (self.device, t.device) {
                    self.device_info(sink, s.time, &d, t.ble)?;
                    self.device = true;
                }
            }
            // invalid: float32 all ones, uint16 0xffff, uint8 0xff
            let alpha = s.alpha1_milli.map_or(u32::MAX, |a| (f32::from(a) / 1000.0).to_bits());
            let w = |v: Option<u16>| v.map_or(0xffff, |v| u32::from(v.min(0xfffe)));
            let t = trn.unwrap_or_default();
            let dev = [
                alpha,
                s.rmssd_ms.map_or(0xffff, u32::from),
                s.artifacts_pct.map_or(0xff, |p| u32::from(p.min(100))),
                w(t.target_w),
                w(t.trainer_w),
                w(t.meter_w),
                u32::from(t.fe_state & 7),
                u32::from(t.status & 0x73),
                u32::from(t.ble),
                u32::from(t.cmds & 0xf),
                u32::from(t.ack_fails.min(0xfe)),
            ];
            let n = if trn.is_some() {
                DEV_RECORD.len()
            } else if self.art {
                3
            } else {
                2
            };
            let (df, dv) = (DEV_RECORD.get(..n).unwrap_or(&[]), dev.get(..n).unwrap_or(&[]));
            self.message_dev(sink, L_RECORD, 20, fields, values, df, dv)?;
        } else {
            self.message(sink, L_RECORD, 20, fields, values)?;
        }
        self.mask = mask;
        self.trn = trn.is_some();
        Ok(())
    }

    /// Bytes [`Writer::record`] writes for `s`'s trainer beyond a record without one, at most: its fields, and the
    /// first time their descriptions and the device (a buffered sink keeps this much more room).
    pub fn trainer_bytes(&self, s: &Sample) -> usize {
        match s.trainer {
            Some(t) if self.dev && self.art => {
                3 * 8
                    + 11
                    + if self.trn_desc { 0 } else { TRN_MESSAGES.len() }
                    + if t.device.is_some() && !self.device { DEVICE_MAX } else { 0 }
            }
            _ => 0,
        }
    }

    /// The trainer's `device_info` (module doc); a field not known is FIT's invalid (uint32z 0).
    fn device_info(&mut self, sink: &mut impl Sink, time: u32, d: &TrainerDevice, ble: bool) -> Result<(), Error> {
        // device_type follows source_type: ble_device_type bike_trainer (7) over bluetooth_low_energy (3),
        // antplus_device_type fitness_equipment (17) over antplus (1)
        let (ty, src) = if ble { (7, 3) } else { (17, 1) };
        let v = [
            time,
            1, // device_index: not the creator (0)
            ty,
            u32::from(d.maker),
            d.serial.unwrap_or(0),
            d.model.map_or(0xffff, u32::from),
            d.sw_x100.map_or(0xffff, u32::from),
            d.hw.map_or(0xff, u32::from),
            src,
        ];
        self.message(sink, L_DEVICE, 23, &DEVICE, &v)
    }

    /// An `hrv` message (module doc) with up to [`HRV_PER_MESSAGE`] RR intervals `rr`, ms, oldest first (more are
    /// left out; the slots after them invalid); its definition first if the file does not have it yet. The full copy
    /// only, and only with a beat: the clean one, or no beat, writes nothing.
    pub fn hrv(&mut self, sink: &mut impl Sink, rr: &[u16]) -> Result<(), Error> {
        if !self.dev || rr.is_empty() {
            return Ok(());
        }
        let mut b = [0u8; HRV_MAX];
        let mut n = 0;
        if self.defined & (1 << L_HRV) == 0 {
            // global 78, one field: 0 `time`, 10 bytes of uint16 (an array of 5)
            let size = u8::try_from(2 * HRV_PER_MESSAGE).unwrap_or(0); // a constant: 10
            b[..9].copy_from_slice(&[0x40 | L_HRV, 0, 0, 78, 0, 1, 0, size, U16]);
            n = 9;
        }
        if let Some(h) = b.get_mut(n) {
            *h = L_HRV;
        }
        n += 1;
        for k in 0..HRV_PER_MESSAGE {
            let v = rr.get(k).copied().unwrap_or(0xffff);
            if let Some(s) = b.get_mut(n..n + 2) {
                s.copy_from_slice(&v.to_le_bytes());
            }
            n += 2;
        }
        self.put(sink, b.get(..n).unwrap_or(&[]))?;
        self.defined |= 1 << L_HRV;
        Ok(())
    }

    /// Ends the ride at FIT time `time`: [`Writer::end`], then a checkpoint (the finished file).
    #[cfg(test)]
    pub fn finish(&mut self, sink: &mut impl Sink, time: u32) -> Result<[u8; RECORD_LEN], Error> {
        self.end(sink, time)?;
        self.checkpoint(sink)
    }

    /// Bytes [`Writer::end`] writes at most (a buffered sink keeps this much room for it).
    pub const END_BYTES: usize = 320;

    /// The ride's end without the checkpoint: timer stop (unless paused), the last lap, session, activity.
    pub fn end(&mut self, sink: &mut impl Sink, time: u32) -> Result<(), Error> {
        self.last = self.last.max(time);
        if self.paused_at == 0 {
            self.event(sink, self.last, 4)?; // timer, stop all
            self.paused_at = self.last;
        }
        // the only lap is the ride (its sums survive any resume); a last lap after others has its own
        let lap = if self.laps > 0 { self.this_lap() } else { self.lap_values() };
        self.message(sink, L_LAP, 19, &LAP, &lap)?;
        // the ride as one lap: the session's values
        let whole = Writer { lap_start: self.start, lap_dist: 0, ..*self }.lap_values();
        // session: event session (8), stop (1), the lap's sport and sub_sport, first lap 0, the laps
        let mut v = [0u32; 20];
        v[..18].copy_from_slice(&whole);
        (v[13], v[18], v[19]) = (8, 0, self.laps + 1);
        self.message(sink, L_SESSION, 18, &SESSION, &v)?;
        // activity: one session, manual (0), event activity (26), stop (1)
        self.message(sink, L_ACTIVITY, 34, &ACTIVITY, &[self.last, whole[3], 1, 0, 26, 1])?;
        Ok(())
    }

    /// Makes the file on the sink valid as it stands (CRC after the data, header with its size), flushes it, and
    /// returns the recovery record to store (only once this returns Ok). Also the resume step (module doc).
    pub fn checkpoint(&mut self, sink: &mut impl Sink) -> Result<[u8; RECORD_LEN], Error> {
        let h = self.header();
        let end = HEADER + self.len;
        sink.write_at(end, &self.trailer())?;
        sink.write_at(0, &h)?;
        sink.set_len(end + 2)?;
        sink.flush()?;
        Ok(self.to_record())
    }

    /// The file header for the data so far.
    fn header(&self) -> [u8; HEADER as usize] {
        let mut h = [0u8; HEADER as usize];
        h[0] = u8::try_from(HEADER).unwrap_or(0); // a constant: 14
        h[1] = if self.dev { 0x20 } else { 0x10 }; // protocol 2.0 for developer fields, else 1.0
        h[2..4].copy_from_slice(&2140u16.to_le_bytes()); // profile 21.40
        h[4..8].copy_from_slice(&self.len.to_le_bytes());
        h[8..12].copy_from_slice(b".FIT");
        let hc = crc(0, &h[..12]);
        h[12..14].copy_from_slice(&hc.to_le_bytes());
        h
    }

    /// The file CRC a checkpoint now writes after the data (its last two bytes).
    pub fn trailer(&self) -> [u8; 2] {
        crc_combine(crc(0, &self.header()), self.crc, self.len).to_le_bytes()
    }

    /// Bytes of messages in the file (grows with every write).
    pub fn data_len(&self) -> u32 {
        self.len
    }

    /// The FIT time of the latest message.
    pub fn last_time(&self) -> u32 {
        self.last
    }

    /// Paused (a timer stop with no start after it).
    pub fn paused(&self) -> bool {
        self.paused_at != 0
    }

    /// The ride's profile (the last sample's [`Sample::profile`]; a record from before it: a road ride).
    pub fn profile(&self) -> ride::Profile {
        self.profile
    }

    /// The profile was set by hand (a record from before it: automatic).
    pub fn by_hand(&self) -> bool {
        self.by_hand
    }

    /// The profile in force (and set by hand) from now on: the next [`Writer::end`] and record carry it, even
    /// with no sample since (a change while paused).
    pub fn set_profile(&mut self, profile: ride::Profile, by_hand: bool) {
        (self.profile, self.by_hand) = (profile, by_hand);
    }

    /// The writer a recovery record holds, if the record is whole.
    pub fn from_record(r: &[u8; RECORD_LEN]) -> Option<Writer> {
        let (body, tail) = r.split_at(RECORD_LEN - 2);
        if crc(0, body).to_le_bytes() != tail || body.get(..2)? != TAG.to_le_bytes() {
            return None;
        }
        let mut w = [0u32; WORDS];
        for (x, c) in w.iter_mut().zip(body.get(2..)?.as_chunks::<4>().0) {
            *x = u32::from_le_bytes(*c);
        }
        let s = |[sum, n, max]: [u32; 3]| Stat { sum, n, max };
        // a record from a build before sub_sport (or before total_ascent): its file defines lap and session
        // without them, so they are defined again (a later definition of a local type replaces the earlier one:
        // FIT protocol). That build sets bit 31, and may have checkpointed this build's ride (bit 14 kept in its
        // mask) after defining its own lap: only bit 14 without bit 31 is this layout
        let ours = w[2] & (LAYOUT_SPORT | LAYOUT_ASCENT) == LAYOUT_SPORT;
        let old = |d: u16| if ours { d } else { d & !(1 << L_LAP | 1 << L_SESSION) };
        Some(Writer {
            len: w[0],
            crc: (w[1] & 0xffff) as u16,
            defined: old((w[1] >> 16) as u16),
            mask: (w[2] & 0x3ff) as u16, // the record's 10 fields; bits 11, 13..15 are the profile's and the layout's
            profile: match (w[2] & INDOOR != 0, w[2] & MTB != 0) {
                (true, _) => ride::Profile::Indoor,
                (false, true) => ride::Profile::Mtb,
                (false, false) => ride::Profile::Road,
            },
            by_hand: w[2] & BY_HAND != 0,
            dev: w[2] >> 16 & 1 != 0,
            art: w[2] & ARTIFACTS != 0,
            trn_desc: w[2] & TRN_DESC != 0,
            trn: false,
            device: false,
            ascent_m: w[2] >> 17 & ASCENT_MAX,
            start: w[3],
            last: w[4],
            dist_mm: w[5],
            max_speed: w[6],
            stats: [s([w[7], w[8], w[9]]), s([w[10], w[11], w[12]]), s([w[13], w[14], w[15]])],
            lap_stats: [Stat::default(); 3], // [`Writer::with_lap_words`]
            lap_start: w[16],
            lap_dist: w[17],
            laps: w[18],
            paused_s: w[19],
            paused_at: w[20],
        })
    }

    /// The recovery record of this state ([`Writer::from_record`] reads it back).
    pub fn to_record(self) -> [u8; RECORD_LEN] {
        let [a, b, c] = self.stats;
        let w: [u32; WORDS] = [
            self.len,
            // a full copy with `ArtPct` stores its record message as not defined: a build before it, resuming
            // the ride, defines its own two-field record again instead of writing it on this three-field one (this
            // build's resume costs one definition)
            u32::from(self.crc) | u32::from(self.defined & if self.art { !(1 << L_RECORD) } else { !0 }) << 16,
            u32::from(self.mask)
                | match self.profile {
                    // GRAVEL, CX and COMMUTE are kept as ROAD: the resume takes them back from BIKE (riding.rs)
                    ride::Profile::Road | ride::Profile::Gravel | ride::Profile::Cx | ride::Profile::Commute => 0,
                    ride::Profile::Indoor => INDOOR,
                    ride::Profile::Mtb => MTB,
                }
                | if self.by_hand { BY_HAND } else { 0 }
                | LAYOUT_SPORT
                | u32::from(self.dev) << 16
                | if self.art { ARTIFACTS } else { 0 }
                | if self.trn_desc { TRN_DESC } else { 0 }
                | self.ascent_m.min(ASCENT_MAX) << 17,
            self.start,
            self.last,
            self.dist_mm,
            self.max_speed,
            a.sum,
            a.n,
            a.max,
            b.sum,
            b.n,
            b.max,
            c.sum,
            c.n,
            c.max,
            self.lap_start,
            self.lap_dist,
            self.laps,
            self.paused_s,
            self.paused_at,
        ];
        let mut r = [0u8; RECORD_LEN];
        r[..2].copy_from_slice(&TAG.to_le_bytes());
        for (c, x) in r[2..RECORD_LEN - 2].chunks_exact_mut(4).zip(w) {
            c.copy_from_slice(&x.to_le_bytes());
        }
        let c = crc(0, &r[..RECORD_LEN - 2]);
        r[RECORD_LEN - 2..].copy_from_slice(&c.to_le_bytes());
        r
    }

    /// An event message: timer (0), `kind` start (0) or stop all (4).
    fn event(&mut self, sink: &mut impl Sink, time: u32, kind: u32) -> Result<(), Error> {
        self.message(sink, L_EVENT, 21, &EVENT, &[time, 0, kind])
    }

    /// Data message `local` (global number `global`), its definition first if the file does not have it yet.
    fn message(&mut self, sink: &mut impl Sink, local: u8, global: u16, f: &[Field], v: &[u32]) -> Result<(), Error> {
        self.message_dev(sink, local, global, f, v, &[], &[])
    }

    /// [`Writer::message`] with developer fields `df` (number, size, developer data index) and their values `dv`.
    #[allow(clippy::too_many_arguments)]
    fn message_dev(
        &mut self,
        sink: &mut impl Sink,
        local: u8,
        global: u16,
        f: &[Field],
        v: &[u32],
        df: &[Field],
        dv: &[u32],
    ) -> Result<(), Error> {
        let mut buf = [0u8; 128]; // the largest, a full record with the trainer and its definition: 114
        let mut n = 0;
        let mut put = |bytes: &[u8]| {
            if let Some(s) = buf.get_mut(n..n + bytes.len()) {
                s.copy_from_slice(bytes);
                n += bytes.len();
            }
        };
        if self.defined & (1 << local) == 0 {
            let [g0, g1] = global.to_le_bytes();
            let flag = if df.is_empty() { 0 } else { 0x20 }; // developer fields follow the definition
            put(&[0x40 | flag | local, 0, 0, g0, g1, u8::try_from(f.len()).map_err(|_| Error)?]);
            for &(num, size, ty) in f {
                put(&[num, size, ty]);
            }
            if !df.is_empty() {
                put(&[u8::try_from(df.len()).map_err(|_| Error)?]);
                for &(num, size, index) in df {
                    put(&[num, size, index]);
                }
            }
        }
        put(&[local]);
        for (&(_, size, _), x) in f.iter().chain(df).zip(v.iter().chain(dv)) {
            put(x.to_le_bytes().get(..usize::from(size)).unwrap_or(&[]));
        }
        self.put(sink, buf.get(..n).unwrap_or(&[]))?;
        self.defined |= 1 << local;
        Ok(())
    }

    /// Appends `bytes` (whole messages) to the data.
    fn put(&mut self, sink: &mut impl Sink, bytes: &[u8]) -> Result<(), Error> {
        let len = u32::try_from(bytes.len()).map_err(|_| Error)?;
        sink.write_at(HEADER + self.len, bytes)?;
        self.len += len;
        self.crc = crc(self.crc, bytes);
        Ok(())
    }
}

/// Degrees x 1e-7 to FIT semicircles (2^31 per 180 degrees).
fn semicircles(deg_e7: i32) -> u32 {
    // the low 32 bits (two's complement), as `as i32 as u32` gives
    let [a, b, c, d, ..] = (i64::from(deg_e7) * (1 << 31) / 1_800_000_000).to_le_bytes();
    u32::from_le_bytes([a, b, c, d])
}

/// FIT's CRC-16 (CRC-16/ARC: reflected polynomial 0xa001), continued from `crc` over `data`, a byte a step from a
/// 256-entry table (512 bytes of flash; the SDK's 4-bit table takes two steps a byte).
///
/// ```
/// assert_eq!(bryton_rustfw::fit::crc(0, b"123456789"), 0xbb3d); // CRC-16/ARC
/// ```
pub fn crc(mut crc: u16, data: &[u8]) -> u16 {
    for &b in data {
        // a u8 index into 256 entries: the compiler drops the check
        crc = (crc >> 8) ^ CRC_TABLE.get(usize::from(crc.to_le_bytes()[0] ^ b)).copied().unwrap_or(0);
    }
    crc
}

/// [`crc`]'s table: the CRC of each byte from 0. In the image, so in the SDRAM, which is read uncached (the D-cache is
/// off): a reader that runs it over megabytes keeps a copy where it lives ([`crate::fitread::Reader`]).
#[allow(clippy::indexing_slicing, clippy::cast_possible_truncation)] // evaluated by the compiler; i < 256
pub const CRC_TABLE: [u16; 256] = {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u16;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xa001 } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// The CRC of A then B from the CRC of A, the CRC of B and B's length (zlib's crc32_combine, for this CRC):
/// shifts `a` through `len_b` zero bytes with squared GF(2) matrices.
///
/// ```
/// use bryton_rustfw::fit::{crc, crc_combine};
/// assert_eq!(crc_combine(crc(0, b"1234"), crc(0, b"56789"), 5), crc(0, b"123456789"));
/// ```
pub fn crc_combine(a: u16, b: u16, mut len_b: u32) -> u16 {
    fn times(m: &[u16; 16], v: u16) -> u16 {
        m.iter().enumerate().filter(|&(i, _)| v >> i & 1 != 0).fold(0, |s, (_, &x)| s ^ x)
    }
    let square = |m: &[u16; 16]| m.map(|x| times(m, x));
    // one zero bit: the reflected polynomial, then a shift
    let one: [u16; 16] = core::array::from_fn(|i| if i == 0 { 0xa001 } else { 1 << (i - 1) });
    let mut op = square(&square(&square(&one))); // one zero byte
    let mut a = a;
    while len_b != 0 {
        if len_b & 1 != 0 {
            a = times(&op, a);
        }
        len_b >>= 1;
        op = square(&op);
    }
    a ^ b
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;

    /// The byte table gives the SDK's 4-bit table's CRC.
    #[test]
    fn crc_table_is_the_sdks() {
        const T: [u16; 16] = [
            0x0000, 0xcc01, 0xd801, 0x1400, 0xf001, 0x3c00, 0x2800, 0xe401, 0xa001, 0x6c00, 0x7800, 0xb401, 0x5000,
            0x9c01, 0x8801, 0x4400,
        ];
        let sdk = |mut c: u16, d: &[u8]| {
            for &b in d {
                for nib in [b & 0xf, b >> 4] {
                    c = (c >> 4) ^ T[usize::from(c & 0xf)] ^ T[usize::from(nib)];
                }
            }
            c
        };
        let data: std::vec::Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        for start in [0, 1, 0xbeef] {
            assert_eq!(crc(start, &data), sdk(start, &data));
        }
    }
    use std::vec::Vec;

    impl Sink for Vec<u8> {
        fn write_at(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
            let end = offset as usize + data.len();
            if self.len() < end {
                self.resize(end, 0);
            }
            self[offset as usize..end].copy_from_slice(data);
            Ok(())
        }
        fn set_len(&mut self, len: u32) -> Result<(), Error> {
            self.resize(len as usize, 0);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }

    /// A medium that loses power after `budget` bytes: the write that crosses it is cut in the middle, and
    /// nothing after it happens.
    struct Cut {
        disk: Vec<u8>,
        budget: usize,
    }

    impl Sink for Cut {
        fn write_at(&mut self, offset: u32, data: &[u8]) -> Result<(), Error> {
            let n = data.len().min(self.budget);
            self.budget -= n;
            self.disk.write_at(offset, &data[..n])?;
            if n < data.len() { Err(Error) } else { Ok(()) }
        }
        fn set_len(&mut self, len: u32) -> Result<(), Error> {
            if self.budget == 0 { Err(Error) } else { Sink::set_len(&mut self.disk, len) }
        }
        fn flush(&mut self) -> Result<(), Error> {
            if self.budget == 0 { Err(Error) } else { Ok(()) }
        }
    }

    /// A data message: global number, then (field number, raw little-endian value) per field.
    pub(crate) type Msg = (u16, Vec<(u8, u64)>);

    /// A strict FIT reader for what this writer writes: header and its CRC, data size, file CRC, normal headers,
    /// every data message on a definition. Developer fields come out as field 230 + their number.
    pub(crate) fn decode(f: &[u8]) -> Result<Vec<Msg>, &'static str> {
        if f.len() < 16 || f[0] != 14 || &f[8..12] != b".FIT" {
            return Err("header");
        }
        if crc(0, &f[..12]).to_le_bytes() != f[12..14] {
            return Err("header crc");
        }
        let size = u32::from_le_bytes(f[4..8].try_into().unwrap()) as usize;
        if f.len() != 14 + size + 2 {
            return Err("size");
        }
        if crc(0, f) != 0 {
            return Err("file crc");
        }
        type Def = Option<(u16, Vec<(u8, u8)>)>; // global message number, (field, size) each
        let mut defs: [Def; 16] = Default::default();
        let (mut i, mut out) = (14, Vec::new());
        while i < 14 + size {
            let h = f[i];
            i += 1;
            if h & 0x80 != 0 {
                return Err("compressed header");
            }
            let local = usize::from(h & 0xf);
            if h & 0x40 != 0 {
                let g = u16::from_le_bytes([f[i + 2], f[i + 3]]);
                let n = usize::from(f[i + 4]);
                let mut fields: Vec<(u8, u8)> = (0..n).map(|k| (f[i + 5 + 3 * k], f[i + 6 + 3 * k])).collect();
                i += 5 + 3 * n;
                if h & 0x20 != 0 {
                    if f[1] != 0x20 {
                        return Err("developer fields in protocol 1.0");
                    }
                    let n = usize::from(f[i]);
                    fields.extend((0..n).map(|k| (230 + f[i + 1 + 3 * k], f[i + 2 + 3 * k])));
                    i += 1 + 3 * n;
                }
                defs[local] = Some((g, fields));
            } else {
                let (g, fields) = defs[local].as_ref().ok_or("no definition")?;
                let mut m = Vec::new();
                if *g == 78 {
                    // hrv: each uint16 of the array as its own (0, value)
                    for &(_, size) in fields {
                        m.extend(
                            f[i..i + usize::from(size)].chunks(2).map(|c| (0, u64::from(c[0]) | u64::from(c[1]) << 8)),
                        );
                        i += usize::from(size);
                    }
                    out.push((*g, m));
                    continue;
                }
                for &(num, size) in fields {
                    let mut v = [0u8; 8];
                    let k = usize::from(size).min(8); // a byte array or a string: its first 8 bytes
                    v[..k].copy_from_slice(&f[i..i + k]);
                    m.push((num, u64::from_le_bytes(v)));
                    i += usize::from(size);
                }
                out.push((*g, m));
            }
        }
        if i == 14 + size { Ok(out) } else { Err("message past the data") }
    }

    pub(crate) fn field(m: &Msg, num: u8) -> Option<u64> {
        m.1.iter().find(|f| f.0 == num).map(|f| f.1)
    }

    const T0: u32 = 1_161_676_800; // 2026-10-23 08:00 UTC

    /// A synthetic ride: GPS from the start, HR from 10 s, power and cadence from 30 s, a power dropout at 200 s.
    fn ride(secs: u32) -> Vec<Sample> {
        (0..secs)
            .map(|t| Sample {
                time: T0 + 1 + t,
                pos: Some((5_123_000 + 300 * t as i32, 4_234_000 - 200 * t as i32)),
                alt_mm: Some(35_000 + 100 * (t % 50) as i32),
                speed_mm_s: Some(7000 + 10 * (t % 100)),
                hr: (t >= 10).then_some(120 + (t % 40) as u8),
                cadence: (t >= 30).then_some(85 + (t % 10) as u8),
                power: (t >= 30 && !(200..215).contains(&t)).then_some(200 + (t % 60) as u16),
                temp_c: Some((t % 30) as i8 - 5),
                alpha1_milli: (t >= 60).then_some(500 + (t % 700) as u16),
                rmssd_ms: (t >= 60).then_some(20 + (t % 40) as u16),
                artifacts_pct: (t >= 60).then_some((t % 9) as u8),
                ascent_m: Some(t / 10), // the ride's ascent so far: 1 m every 10 s
                trainer: trainer(t),
                profile: ride::Profile::Road,
                by_hand: false,
                auto_paused: false,
            })
            .collect()
    }

    /// The test ride's trainer in second `t`: connected from 100 s to 300 s but for 150..160 (a drop), its pages 80
    /// and 81 from 110 s; the target 200 W, its power 195 + t % 10, the meter's from 120 s, a command every 4 s and a
    /// TX_FAILED every 7.
    pub(crate) fn trainer(t: u32) -> Option<TrainerSample> {
        ((100..300).contains(&t) && !(150..160).contains(&t)).then(|| TrainerSample {
            target_w: Some(200),
            trainer_w: Some(195 + (t % 10) as u16),
            meter_w: (t >= 120).then_some(190 + (t % 10) as u16),
            fe_state: 3,
            status: 0x10 | (t % 4) as u8,
            ble: false,
            cmds: if t.is_multiple_of(4) { 2 } else { 0 },
            ack_fails: u8::from(t.is_multiple_of(7)),
            device: (t >= 110).then_some(TrainerDevice {
                maker: 15,
                model: Some(0x1234),
                hw: Some(1),
                serial: Some(0x1234_5678),
                sw_x100: Some(10),
            }),
        })
    }

    /// A gear change of the test ride at `time`, if one: every 45 s from 7 s, front every third time.
    fn shift(time: u32) -> Option<(bool, u8, u8)> {
        let t = time - T0;
        (t % 45 == 7).then_some((t % 135 == 7, (t / 135 % 2 + 1) as u8, (t / 45 % 11 + 1) as u8))
    }

    /// The test ride's beats after the record at `time`, ms: none for the first 10 s, then 2 or 3 a second.
    pub(crate) fn beats(time: u32) -> Vec<u16> {
        let t = time - T0;
        if t < 10 { Vec::new() } else { (0..2 + t % 2).map(|k| (600 + (t * 37 + k * 11) % 400) as u16).collect() }
    }

    /// Writes `samples` from `w`, each second's beats after its record (the full copy), a checkpoint every 60 s; the
    /// last record stored whole, None if the sink failed.
    fn write(sink: &mut impl Sink, w: &mut Writer, samples: &[Sample], rec: &mut [u8; RECORD_LEN]) -> Option<()> {
        for s in samples {
            w.record(sink, s).ok()?;
            if w.dev {
                w.hrv(sink, &beats(s.time)).ok()?;
            }
            if let Some((front, f, r)) = shift(s.time) {
                w.gear(sink, s.time, front, f, r).ok()?;
            }
            if (s.time - T0).is_multiple_of(60) {
                *rec = w.checkpoint(sink).ok()?;
            }
        }
        *rec = w.finish(sink, samples.last()?.time + 1).ok()?;
        Some(())
    }

    #[test]
    fn crc_combine_matches_direct() {
        let data: Vec<u8> = (0..3000u32).map(|i| (i * 7 + i / 13) as u8).collect();
        for cut in [0, 1, 14, 255, 256, 1000, 2999, 3000] {
            let (a, b) = data.split_at(cut);
            assert_eq!(crc_combine(crc(0, a), crc(0, b), b.len() as u32), crc(0, &data), "{cut}");
        }
    }

    /// A ride open under a build before total_ascent, resumed by this one: the record lacks the layout bit, so the
    /// lap and session are defined again before their new layout is written; the file decodes, the session has
    /// the ascent (the golden rule across an update with a ride open).
    #[test]
    fn a_ride_from_before_total_ascent_resumes() {
        let (mut f, mut rec) = (Vec::new(), [0; RECORD_LEN]);
        let (mut w, _) = Writer::start(&mut f, T0).unwrap();
        let samples = ride(240);
        for s in &samples[..120] {
            w.record(&mut f, s).unwrap();
        }
        rec = w.checkpoint(&mut f).unwrap_or(rec);
        // the record as the earlier build wrote it: bit 31 of its third word clear, the CRC made again
        let (at, mut word) = (2 + 4 * 2, [0u8; 4]);
        word.copy_from_slice(&rec[at..at + 4]);
        let old = u32::from_le_bytes(word) & !(LAYOUT_ASCENT | LAYOUT_SPORT);
        rec[at..at + 4].copy_from_slice(&old.to_le_bytes());
        let c = crc(0, &rec[..RECORD_LEN - 2]);
        rec[RECORD_LEN - 2..].copy_from_slice(&c.to_le_bytes());
        let mut w = Writer::from_record(&rec).unwrap();
        assert_eq!(w.defined & (1 << L_LAP | 1 << L_SESSION), 0, "lap and session to be defined again");
        w.resume(&mut f, samples[120].time).unwrap();
        let mut r = [0; RECORD_LEN];
        write(&mut f, &mut w, &samples[120..], &mut r).unwrap();
        let m = decode(&f).unwrap();
        let session = m.iter().rev().find(|x| x.0 == 18).unwrap();
        assert_eq!(field(session, 22), Some(23), "the session's total_ascent after the resume");
        assert_eq!((field(session, 5), field(session, 6)), (Some(2), Some(7)), "a road ride: cycling, road");
    }

    /// The record as the build before sub_sport wrote it (bit 31 set, bits 11, 13 to 15 clear), from a ride of
    /// `profile`: the writer it gives, and the file it is for.
    fn record_before_sub_sport(profile: ride::Profile) -> (Writer, Vec<u8>) {
        let mut f = Vec::new();
        let (mut w, _) = Writer::start(&mut f, T0).unwrap();
        for s in &ride(120) {
            w.record(&mut f, &Sample { profile, by_hand: profile != ride::Profile::Road, ..*s }).unwrap();
            if s.time == T0 + 60 {
                w.lap(&mut f, s.time).unwrap(); // the lap defined in the file, on this build's layout here
            }
        }
        let mut rec = w.checkpoint(&mut f).unwrap();
        let (at, mut word) = (2 + 4 * 2, [0u8; 4]);
        word.copy_from_slice(&rec[at..at + 4]);
        let old = u32::from_le_bytes(word) & !(LAYOUT_SPORT | INDOOR | MTB | BY_HAND) | LAYOUT_ASCENT;
        rec[at..at + 4].copy_from_slice(&old.to_le_bytes());
        let c = crc(0, &rec[..RECORD_LEN - 2]);
        rec[RECORD_LEN - 2..].copy_from_slice(&c.to_le_bytes());
        (Writer::from_record(&rec).unwrap(), f)
    }

    /// The ride profile in the file: cycling (2) in the lap and the session, sub_sport indoor_cycling (6) for an
    /// indoor ride, mountain (8) for an MTB one, road (7) else (FIT profile; stock maps its bike type the same way,
    /// `Fit_MapBikeTypeToSubSport`); kept in the recovery record, so a resumed ride keeps its profile; a record from
    /// before it reads as a road ride and defines lap and session again (its file's have no sport).
    #[test]
    fn the_profile_goes_into_the_file_and_the_record() {
        use ride::Profile::{Commute, Cx, Gravel, Indoor, Mtb, Road};
        for profile in [Road, Indoor, Mtb] {
            let (mut f, mut rec) = (Vec::new(), [0; RECORD_LEN]);
            let (mut w, _) = Writer::start(&mut f, T0).unwrap();
            let samples: Vec<Sample> = ride(240).iter().map(|s| Sample { profile, ..*s }).collect();
            for s in &samples[..120] {
                w.record(&mut f, s).unwrap();
            }
            w.lap(&mut f, T0 + 120).unwrap();
            let r = w.checkpoint(&mut f).unwrap();
            let mut w = Writer::from_record(&r).unwrap();
            assert_eq!(w.profile(), profile, "kept in the record");
            assert_ne!(w.defined & 1 << L_LAP, 0, "this build's lap definition kept");
            // the record's word: never bit 31 (the build before would write its lap on this layout)
            assert_eq!(u32::from_le_bytes(r[10..14].try_into().unwrap()) & LAYOUT_ASCENT, 0);
            write(&mut f, &mut w, &samples[120..], &mut rec).unwrap();
            let m = decode(&f).unwrap();
            let sub = match profile {
                Road => 7,
                Indoor => 6,
                Mtb => 8,
                Gravel => 46,
                Cx => 11,
                Commute => 48,
            };
            for lap in m.iter().filter(|x| x.0 == 19) {
                assert_eq!((field(lap, 25), field(lap, 39)), (Some(2), Some(sub)), "lap: cycling, {sub}");
            }
            let session = m.iter().find(|x| x.0 == 18).unwrap();
            assert_eq!((field(session, 5), field(session, 6)), (Some(2), Some(sub)), "session: cycling, {sub}");
        }
        // a record of the build before (indoor and MTB rides did not exist there): road, lap and session defined again
        for profile in [Road, Indoor, Mtb] {
            let (mut w, mut f) = record_before_sub_sport(profile);
            assert!(w.profile() == Road && !w.by_hand(), "an older record reads as a road ride, automatic");
            assert_eq!(w.defined & (1 << L_LAP | 1 << L_SESSION), 0, "lap and session to be defined again");
            w.end(&mut f, T0 + 121).unwrap(); // ended from the record alone, no sample after it
            w.checkpoint(&mut f).unwrap();
            let m = decode(&f).unwrap();
            let session = m.iter().find(|x| x.0 == 18).unwrap();
            assert_eq!(field(session, 6), Some(7), "road");
        }
    }

    /// "By hand" kept in the record (bit 13); a profile given while paused ([`Writer::set_profile`], no sample
    /// since) goes into the record and the end; and an older build, which reads bits 0..15 as the record mask, only
    /// defines the record message again.
    #[test]
    fn the_profile_by_hand_and_set_while_paused() {
        let mut f = Vec::new();
        let (mut w, _) = Writer::start(&mut f, T0).unwrap();
        let samples = ride(120);
        for s in &samples[..60] {
            w.record(&mut f, &Sample { profile: ride::Profile::Indoor, by_hand: true, ..*s }).unwrap();
        }
        let r = w.checkpoint(&mut f).unwrap();
        let back = Writer::from_record(&r).unwrap();
        assert!(back.profile() == ride::Profile::Indoor && back.by_hand(), "kept in the record");
        assert_eq!(back.mask, w.mask, "bit 13 is not a record field");
        // an older build: its mask is the word's low 16 bits, so the record is defined again, nothing else
        let word = u32::from_le_bytes(r[10..14].try_into().unwrap());
        assert_eq!(word & 0xfc00, BY_HAND | LAYOUT_SPORT | INDOOR, "only bits 13..15 above the mask");
        let mut old = Writer { mask: word as u16, ..back };
        for s in &samples[60..90] {
            old.record(&mut f, s).unwrap();
        }
        assert!(!old.by_hand() && old.profile() == ride::Profile::Road, "the old build's samples: automatic road");
        old.pause(&mut f, T0 + 91).unwrap();
        // paused: road by hand, then the end with no sample since
        old.set_profile(ride::Profile::Road, true);
        let back = Writer::from_record(&old.checkpoint(&mut f).unwrap()).unwrap();
        assert!(back.profile() == ride::Profile::Road && back.by_hand(), "set while paused: in the record");
        old.set_profile(ride::Profile::Indoor, true);
        old.end(&mut f, T0 + 100).unwrap();
        old.checkpoint(&mut f).unwrap();
        let m = decode(&f).unwrap();
        assert_eq!(m.iter().filter(|x| x.0 == 20).count(), 90, "every record decodes");
        let session = m.iter().find(|x| x.0 == 18).unwrap();
        assert_eq!(field(session, 6), Some(6), "indoor, set while paused");
    }

    #[test]
    fn a_ride_decodes_with_its_values() {
        let (mut f, mut rec) = (Vec::new(), [0; RECORD_LEN]);
        let (mut w, _) = Writer::start(&mut f, T0).unwrap();
        let samples = ride(300);
        write(&mut f, &mut w, &samples, &mut rec).unwrap();
        let m = decode(&f).unwrap();
        let globals: Vec<u16> = m.iter().map(|x| x.0).collect();
        assert_eq!(&globals[..3], [0, 21, 20], "file_id, timer start, records");
        assert_eq!(&globals[globals.len() - 4..], [21, 19, 18, 34], "timer stop, lap, session, activity");
        let records: Vec<&Msg> = m.iter().filter(|x| x.0 == 20).collect();
        assert_eq!(records.len(), 300);
        // the ride's ascent in the lap and the session (FIT profile: lap field 21, session field 22)
        let last = |g: u16| m.iter().rev().find(|x| x.0 == g).unwrap();
        assert_eq!((field(last(19), 21), field(last(18), 22)), (Some(29), Some(29)), "total_ascent, m");
        let r = records[100];
        assert_eq!(field(r, 253), Some(u64::from(T0 + 101)));
        assert_eq!(field(r, 0), Some(u64::from(semicircles(5_153_000))));
        assert_eq!(field(r, 3), Some(120 + 100 % 40));
        assert_eq!(field(r, 7), Some(200 + 100 % 60));
        assert_eq!(field(records[0], 3), None, "no HR yet: the field is left out");
        assert_eq!(field(records[205], 7), None, "power dropout");
        let session = m.iter().find(|x| x.0 == 18).unwrap();
        assert_eq!(field(session, 17), Some(159), "max HR");
        assert_eq!(field(session, 21), Some(259), "max power");
        assert_eq!(Writer::from_record(&rec).map(|w| w.data_len() as usize), Some(f.len() - 16));
    }

    /// The lap's heart rate and power as the FIT profile has them (21.218 `lap`, garmin_fit_sdk/profile.py):
    /// avg_heart_rate 15 and max_heart_rate 16 uint8 (0x02), avg_power 19 and max_power 20 uint16 (0x84), in the
    /// one lap definition the file has always had; a lap without a meter or a strap is invalid (0xff, 0xffff),
    /// never 0. A resume from a record of a build before the lap sums (zeros beside it) averages the lap from the
    /// resume; with them, across it, as if never cut.
    #[test]
    fn lap_heart_rate_and_power() {
        let plain = |t: u32| Sample { time: T0 + t, speed_mm_s: Some(5000), ..Sample::default() };
        let with = |t: u32| Sample { hr: Some(140 + 20 * (t % 2) as u8), power: Some(100 + t as u16), ..plain(t) };
        // (resume at 25 s: None never cut, Some(true) with the lap sums, Some(false) a record without them)
        let run = |resume: Option<bool>| {
            let mut f = Vec::new();
            let mut w = Writer::begin(&mut f, T0).unwrap();
            for t in 1..=10 {
                w.record(&mut f, &plain(t)).unwrap();
            }
            w.lap(&mut f, T0 + 10).unwrap();
            for t in 11..=40 {
                w.record(&mut f, &with(t)).unwrap();
                if let (25, Some(kept)) = (t, resume) {
                    let rec = w.checkpoint(&mut f).unwrap();
                    let words = if kept { w.lap_words() } else { [0; LAP_WORDS] };
                    w = Writer::from_record(&rec).unwrap().with_lap_words(&words);
                    w.checkpoint(&mut f).unwrap();
                }
            }
            w.lap(&mut f, T0 + 40).unwrap();
            w.finish(&mut f, T0 + 41).unwrap();
            f
        };
        let f = run(None);
        // the lap definition: local 3, global 19, the four fields as the profile types them
        let def = f.windows(6).position(|b| b == [0x40 | L_LAP, 0, 0, 19, 0, 18]).expect("lap definition");
        let fields: Vec<&[u8]> = f[def + 6..def + 6 + 18 * 3].chunks(3).collect();
        for want in [[15, 1, U8], [16, 1, U8], [17, 1, U8], [18, 1, U8], [19, 2, U16], [20, 2, U16]] {
            assert!(fields.contains(&&want[..]), "{want:?} in {fields:?}");
        }
        let laps = |f: &[u8]| -> Vec<[Option<u64>; 4]> {
            let m = decode(f).unwrap();
            m.iter().filter(|x| x.0 == 19).map(|l| [15, 16, 19, 20].map(|n| field(l, n))).collect()
        };
        let none = [Some(0xff), Some(0xff), Some(0xffff), Some(0xffff)];
        // 11..=40: heart rate 140 / 160 in turn, power 111..140; the first lap had neither, the last is empty
        let whole = [Some(150), Some(160), Some(125), Some(140)];
        assert_eq!(laps(&f), [none, whole, none]);
        assert_eq!(laps(&run(Some(true))), [none, whole, none], "resumed with the lap's sums: as never cut");
        // a record without the sums: 26..=40 only (8 x 140, 7 x 160 bpm; power 126..140), still that lap's own, never the ride's
        assert_eq!(laps(&run(Some(false))), [none, [Some(149), Some(160), Some(133), Some(140)], none]);
    }

    /// A lap with an auto-paused stop (a minute at 0 km/h, zero power, the strap still on): the lap's average power
    /// and heart rate in the file are the lap banner's (`Ride`'s, over the moving seconds), the stop's zeros left
    /// out; the session's cover every record.
    #[test]
    fn lap_averages_are_the_banners() {
        let mut f = Vec::new();
        let mut w = Writer::begin(&mut f, T0).unwrap();
        let mut ride = ride::Ride::new();
        ride.start_pause();
        for t in 1..=200u32 {
            let stop = (60..120).contains(&t);
            let i = ride::Input {
                speed_x10: Some(if stop { 0 } else { 250 }),
                hr: Some(if stop { 100 } else { 140 + (t % 7) as u16 }),
                power: Some(if stop { 0 } else { 180 + (t % 50) as u16 }),
                ..ride::Input::default()
            };
            ride.second(&i);
            let s = Sample {
                time: T0 + t,
                speed_mm_s: i.speed_x10.map(|v| u32::from(v) * 1000 / 36),
                hr: i.hr.map(|v| v as u8),
                power: i.power,
                auto_paused: !ride.moving(),
                ..Sample::default()
            };
            w.record(&mut f, &s).unwrap();
        }
        assert!(ride.lap().is_some());
        w.lap(&mut f, T0 + 200).unwrap();
        w.finish(&mut f, T0 + 201).unwrap();
        let m = decode(&f).unwrap();
        let lap = m.iter().find(|x| x.0 == 19).unwrap();
        assert_eq!(field(lap, 19), ride.last_lap_avg_power().map(u64::from), "the banner's lap power");
        assert_eq!(field(lap, 15), ride.last_lap_hr().map(u64::from), "the banner's lap heart rate");
        let session = m.iter().find(|x| x.0 == 18).unwrap();
        assert!(field(session, 20) < field(lap, 19), "the session's average has the stop's zeros");
    }

    /// A pause (timer stop and start events, left out of the timer time), a lap, temperatures below zero; and the
    /// writer survives a recovery record taken while paused, the lap's sums with it: each lap's heart rate and power
    /// are its own records', the first lap's across the resume.
    #[test]
    fn pauses_laps_and_temperature() {
        let mut f = Vec::new();
        let mut w = Writer::begin(&mut f, T0).unwrap();
        let samples = ride(300);
        for s in &samples {
            let t = s.time - T0;
            if (101..161).contains(&t) {
                continue; // paused
            }
            w.record(&mut f, s).unwrap();
            match t {
                100 => {
                    w.pause(&mut f, s.time).unwrap();
                    let rec = w.checkpoint(&mut f).unwrap();
                    w = Writer::from_record(&rec).unwrap().with_lap_words(&w.lap_words());
                }
                161 => w.resume(&mut f, s.time).unwrap(),
                200 => w.lap(&mut f, s.time).unwrap(),
                _ => {}
            }
        }
        w.finish(&mut f, T0 + 301).unwrap();
        let m = decode(&f).unwrap();
        let events: Vec<u64> = m.iter().filter(|x| x.0 == 21).map(|x| field(x, 1).unwrap()).collect();
        assert_eq!(events, [0, 4, 0, 4], "start, pause, resume, stop");
        let laps: Vec<&Msg> = m.iter().filter(|x| x.0 == 19).collect();
        assert_eq!(laps.len(), 2);
        assert_eq!(field(laps[0], 2), Some(u64::from(T0)), "the first lap from the start");
        assert_eq!(field(laps[1], 2), Some(u64::from(T0 + 200)));
        // (avg, max) of the records from `a` to `b` s (times from T0), as the lap must carry them
        let over = |a: u32, b: u32, v: &dyn Fn(&Sample) -> Option<u16>| {
            let x: Vec<u64> = samples
                .iter()
                .filter(|s| (a..=b).contains(&(s.time - T0)) && !(101..161).contains(&(s.time - T0)))
                .filter_map(|s| v(s).map(u64::from))
                .collect();
            (Some(x.iter().sum::<u64>() / x.len() as u64), x.iter().max().copied())
        };
        for (lap, (a, b)) in laps.iter().zip([(1, 200), (201, 300)]) {
            assert_eq!((field(lap, 15), field(lap, 16)), over(a, b, &|s| s.hr.map(u16::from)), "lap HR {a}");
            assert_eq!((field(lap, 17), field(lap, 18)), over(a, b, &|s| s.cadence.map(u16::from)), "cadence {a}");
            assert_eq!((field(lap, 19), field(lap, 20)), over(a, b, &|s| s.power), "lap power {a}");
            assert_eq!(field(lap, 21), Some(0xffff), "the lap's own ascent is not kept");
        }
        assert_ne!(field(laps[0], 19), field(laps[1], 19), "the laps' own, not the ride's");
        let session = m.iter().find(|x| x.0 == 18).unwrap();
        assert_eq!(field(session, 7), Some(301_000), "elapsed");
        assert_eq!(field(session, 8), Some(240_000), "timer: the 61 s pause left out");
        assert_eq!(field(session, 26), Some(2), "laps");
        let r = m.iter().filter(|x| x.0 == 20).nth(3).unwrap();
        assert_eq!(field(r, 13), Some(0xfe), "-2 C");
    }

    /// Gear changes: front_gear_change (42) and rear_gear_change (43) events, marker, with the gears in their data;
    /// the definition, written once, is known again from a recovery record (local 8, past the record's old byte).
    #[test]
    fn gear_change_events() {
        let mut f = Vec::new();
        let mut w = Writer::begin(&mut f, T0).unwrap();
        for s in &ride(60) {
            w.record(&mut f, s).unwrap();
            match s.time - T0 {
                10 => w.gear(&mut f, s.time, false, 2, 10).unwrap(),
                20 => {
                    let rec = w.checkpoint(&mut f).unwrap();
                    w = Writer::from_record(&rec).unwrap();
                }
                30 => w.gear(&mut f, s.time, true, 1, 10).unwrap(),
                _ => {}
            }
        }
        w.finish(&mut f, T0 + 61).unwrap();
        let m = decode(&f).unwrap();
        let gears: Vec<_> = m.iter().filter(|x| x.0 == 21 && field(x, 1) == Some(3)).collect();
        assert_eq!(gears.len(), 2);
        assert_eq!(
            (field(gears[0], 0), field(gears[0], 3)),
            (Some(43), Some(0x0002_000a)),
            "rear: 2-10, teeth invalid (0)"
        );
        assert_eq!((field(gears[1], 0), field(gears[1], 253)), (Some(42), Some(u64::from(T0 + 30))), "front");
        let defs = f.windows(6).filter(|d| d[0] == 0x40 | L_GEAR && d[3..6] == [21, 0, 4]).count();
        assert_eq!(defs, 1, "defined once");
    }

    /// The golden rule: cut the power after every byte of a ride. The last stored recovery record always gives
    /// back a valid file (the checkpoint's, byte for byte), and the ride goes on from it to a valid finished file.
    #[test]
    fn power_cut_at_every_byte_resumes_from_the_last_checkpoint() {
        power_cut_at_every_byte(false);
    }

    /// The same for the full copy (developer fields).
    #[test]
    fn power_cut_at_every_byte_with_developer_fields() {
        power_cut_at_every_byte(true);
    }

    fn start(sink: &mut impl Sink, dev: bool) -> Result<(Writer, [u8; RECORD_LEN]), Error> {
        let mut w = Writer::begin_with(sink, T0, dev)?;
        let rec = w.checkpoint(sink)?;
        Ok((w, rec))
    }

    fn power_cut_at_every_byte(dev: bool) {
        let samples = ride(200);
        // the uncut ride, and the file at each checkpoint, by data length
        let mut full = Vec::new();
        let (mut w, _) = start(&mut full, dev).unwrap();
        let mut at_checkpoint = std::collections::HashMap::new();
        at_checkpoint.insert(w.data_len(), full.clone());
        for s in &samples {
            w.record(&mut full, s).unwrap();
            if dev {
                w.hrv(&mut full, &beats(s.time)).unwrap(); // the beats cut and resumed too
            }
            if let Some((front, f, r)) = shift(s.time) {
                w.gear(&mut full, s.time, front, f, r).unwrap(); // the gear events cut and resumed too
            }
            if (s.time - T0).is_multiple_of(60) {
                w.checkpoint(&mut full).unwrap();
                at_checkpoint.insert(w.data_len(), full.clone());
            }
        }
        w.finish(&mut full, samples.last().unwrap().time + 1).unwrap();
        at_checkpoint.insert(w.data_len(), full.clone());

        let total = 4 * full.len(); // more than all bytes written, trailers and headers included
        let mut resumed = 0;
        for budget in 0..total {
            let mut sink = Cut { disk: Vec::new(), budget };
            let mut rec = None;
            let mut last = [0; RECORD_LEN];
            if let Ok((mut w, r)) = start(&mut sink, dev) {
                rec = Some(r);
                let done = write(&mut sink, &mut w, &samples, &mut last).is_some();
                if last != [0; RECORD_LEN] {
                    rec = Some(last);
                }
                if done {
                    assert_eq!(sink.disk, full);
                    break;
                }
            }
            let Some(rec) = rec else { continue }; // cut before the first checkpoint: nothing was promised
            let mut disk = sink.disk;
            let mut w = Writer::from_record(&rec).unwrap();
            w.checkpoint(&mut disk).unwrap();
            assert_eq!(Some(&disk), at_checkpoint.get(&w.data_len()), "cut at byte {budget}");
            decode(&disk).unwrap();
            // the ride goes on: the samples after the checkpoint
            let next: Vec<Sample> = samples.iter().filter(|s| s.time > w.last).copied().collect();
            if !next.is_empty() {
                let mut r = [0; RECORD_LEN];
                write(&mut disk, &mut w, &next, &mut r).unwrap();
                let m = decode(&disk).unwrap();
                assert_eq!(m.iter().filter(|x| x.0 == 20).count(), 200, "cut at {budget}");
                if dev {
                    // the beats of every second the file has, after a resume too: none lost, none twice
                    let rr = m.iter().filter(|x| x.0 == 78).flat_map(|x| x.1.iter().map(|f| f.1));
                    let fed = samples.iter().flat_map(|s| beats(s.time)).map(u64::from);
                    assert!(rr.filter(|&v| v != 0xffff).eq(fed), "cut at {budget}");
                    // the trainer's fields described before the first record that has them, a resume or not
                    let desc = m.iter().position(|x| x.0 == 206 && field(x, 1) == Some(3));
                    let first = m.iter().position(|x| x.0 == 20 && field(x, 233).is_some());
                    assert!(first.is_some(), "cut at {budget}: the test ride has a trainer");
                    assert!(desc < first && desc.is_some(), "cut at {budget}: target_w described before use");
                }
            }
            resumed += 1;
        }
        assert!(resumed > 1000);
    }

    /// The clean file ignores the developer values (byte for byte the same without them); the full copy is
    /// protocol 2.0 and carries them in every record, invalid when not known, after its developer data.
    #[test]
    fn developer_fields_only_in_the_full_copy() {
        let samples = ride(300);
        let bare: Vec<Sample> =
            samples.iter().map(|s| Sample { alpha1_milli: None, rmssd_ms: None, artifacts_pct: None, ..*s }).collect();
        let file = |dev: bool, samples: &[Sample]| {
            let (mut f, mut rec) = (Vec::new(), [0; RECORD_LEN]);
            let (mut w, _) = start(&mut f, dev).unwrap();
            write(&mut f, &mut w, samples, &mut rec).unwrap();
            assert_eq!(Writer::from_record(&rec).map(|w| w.dev), Some(dev));
            f
        };
        let clean = file(false, &samples);
        assert_eq!(clean, file(false, &bare));
        assert_eq!(clean[1], 0x10);
        assert!(decode(&clean).unwrap().iter().all(|m| m.0 != 23 && m.1.iter().all(|f| !(230..=240).contains(&f.0))));
        let full = file(true, &samples);
        assert_eq!(full[1], 0x20);
        let m = decode(&full).unwrap();
        let globals: Vec<u16> = m.iter().map(|x| x.0).take(6).collect();
        assert_eq!(globals, [0, 207, 206, 206, 206, 21], "file_id, developer data, timer start");
        let records: Vec<&Msg> = m.iter().filter(|x| x.0 == 20).collect();
        assert_eq!(records.len(), 300);
        assert_eq!(field(records[10], 230), Some(0xffff_ffff), "no DFA a1 yet: invalid");
        assert_eq!(field(records[10], 231), Some(0xffff));
        let a = f32::from_bits(field(records[100], 230).unwrap() as u32);
        assert!((a - (500 + 100) as f32 / 1000.0).abs() < 1e-6, "{a}");
        assert_eq!(field(records[100], 231), Some(20 + 100 % 40));
        assert_eq!((field(records[10], 232), field(records[100], 232)), (Some(0xff), Some(100 % 9)), "ArtPct");
        // the beats: every one, in order, five to a message with the slots after them invalid
        let rr: Vec<u64> = m.iter().filter(|x| x.0 == 78).flat_map(|x| x.1.iter().map(|f| f.1)).collect();
        let fed: Vec<u64> = samples.iter().flat_map(|s| beats(s.time)).map(u64::from).collect();
        assert_eq!(rr.iter().copied().filter(|&v| v != 0xffff).collect::<Vec<_>>(), fed);
        assert!(m.iter().filter(|x| x.0 == 78).all(|x| x.1.len() == 5));
        // the same records otherwise: the clean file's fields, in the same order
        let cm = decode(&clean).unwrap();
        let cr: Vec<&Msg> = cm.iter().filter(|x| x.0 == 20).collect();
        for (c, f) in cr.iter().zip(&records) {
            assert_eq!(c.1[..], f.1.iter().copied().filter(|x| !(230..=240).contains(&x.0)).collect::<Vec<_>>()[..]);
        }
        // the trainer (module doc): its descriptions once, before its first record (100 s); its fields only while
        // it is connected; the device once, before the first record from 110 s
        let at = |g: u16| m.iter().position(|x| x.0 == g);
        let first_trn = m.iter().position(|x| x.0 == 20 && field(x, 233).is_some());
        let descs: Vec<usize> = (0..m.len()).filter(|&i| m[i].0 == 206 && field(&m[i], 1).unwrap() >= 3).collect();
        assert_eq!(descs.len(), 8, "eight trainer fields described once");
        assert!(descs.iter().all(|&i| Some(i) < first_trn), "described before use");
        assert_eq!(m.iter().filter(|x| x.0 == 23).count(), 1, "one device_info");
        let dev = &m[at(23).unwrap()];
        assert_eq!(
            [2, 3, 4, 5, 6, 25, 1].map(|f| field(dev, f)),
            [15, 0x1234_5678, 0x1234, 10, 1, 1, 17].map(Some),
            "maker, serial, model, software 0.1, hardware, antplus, fitness equipment"
        );
        assert_eq!(field(dev, 253), Some(u64::from(T0 + 1 + 110)), "at 110 s");
        for (t, r) in records.iter().enumerate() {
            let on = (100..300).contains(&t) && !(150..160).contains(&t);
            assert_eq!(field(r, 233).is_some(), on, "second {t}");
            if on {
                let got = (234..=240).map(|f| field(r, f).unwrap()).collect::<Vec<_>>();
                let meter = if t >= 120 { 190 + t as u64 % 10 } else { 0xffff };
                let want =
                    [195 + t as u64 % 10, meter, 3, 0x10 | (t as u64 % 4), 0, 2 * u64::from(t.is_multiple_of(4)), 0];
                assert_eq!(field(r, 233), Some(200), "target, second {t}");
                assert_eq!(got[..6], want[..6], "second {t}");
                assert_eq!(got[6], u64::from(t % 7 == 0), "ack_fail, second {t}");
            }
        }
    }

    /// A full copy begun by a build before `ArtPct` (two developer fields) has no trainer fields: they follow it.
    #[test]
    fn no_trainer_fields_without_art_pct() {
        let mut f = Vec::new();
        let mut w = Writer::begin_with(&mut f, T0, true).unwrap();
        w.art = false;
        let s = &ride(300)[200];
        assert!(s.trainer.is_some());
        assert_eq!(w.trainer_bytes(s), 0);
        w.record(&mut f, s).unwrap();
        w.checkpoint(&mut f).unwrap();
        let m = decode(&f).unwrap();
        assert!(m.iter().all(|x| x.0 != 23 && !x.1.iter().any(|f| (233..=240).contains(&f.0))));
    }

    /// The beats' cost: an hour at 150 bpm is 9000 beats, 1800 messages of 11 bytes, 19.8 KB (and the definition).
    #[test]
    fn an_hour_of_beats() {
        let size = |beats: bool| {
            let mut f = Vec::new();
            let mut w = Writer::begin_with(&mut f, T0, true).unwrap();
            let rr = [400u16; 5]; // 150 bpm
            for t in 0..3600u32 {
                w.record(&mut f, &Sample { time: T0 + 1 + t, hr: Some(150), ..Default::default() }).unwrap();
                if beats && t % 2 == 1 {
                    w.hrv(&mut f, &rr).unwrap(); // five every two seconds
                }
            }
            w.finish(&mut f, T0 + 3601).unwrap();
            f.len()
        };
        let (with, without) = (size(true), size(false));
        assert_eq!(with - without, 9 + 1800 * 11);
        std::println!("an hour at 150 bpm: {without} B without the beats, {with} B with them");
    }

    #[test]
    fn a_damaged_record_is_refused() {
        let (_, mut rec) = Writer::start(&mut Vec::new(), T0).unwrap();
        assert!(Writer::from_record(&rec).is_some());
        rec[9] ^= 1;
        assert!(Writer::from_record(&rec).is_none());
    }

    /// target/fit/sample.fit (a 20-minute ride) and cut.fit (cut mid-ride, resumed, not finished), for
    /// tools/fit_check.py.
    #[test]
    fn fit_sample() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fit");
        std::fs::create_dir_all(&dir).unwrap();
        let (mut f, mut rec) = (Vec::new(), [0; RECORD_LEN]);
        let (mut w, _) = Writer::start(&mut f, T0).unwrap();
        write(&mut f, &mut w, &ride(1200), &mut rec).unwrap();
        std::fs::write(dir.join("sample.fit"), &f).unwrap();
        let mut full = Vec::new();
        let (mut w, _) = start(&mut full, true).unwrap();
        write(&mut full, &mut w, &ride(1200), &mut rec).unwrap();
        std::fs::write(dir.join("sample_full.fit"), &full).unwrap();

        let mut sink = Cut { disk: Vec::new(), budget: f.len() / 2 };
        let (mut w, mut rec) = Writer::start(&mut sink, T0).unwrap();
        assert!(write(&mut sink, &mut w, &ride(1200), &mut rec).is_none());
        let mut w = Writer::from_record(&rec).unwrap();
        w.checkpoint(&mut sink.disk).unwrap();
        std::fs::write(dir.join("cut.fit"), &sink.disk).unwrap();

        // indoor.fit: an indoor ride cut mid-ride, resumed from its record and finished (sub_sport
        // indoor_cycling); older.fit: a ride resumed from a record of the build before sub_sport (road)
        // mtb.fit the same with an MTB ride (sub_sport mountain)
        for (profile, name) in [(ride::Profile::Indoor, "indoor.fit"), (ride::Profile::Mtb, "mtb.fit")] {
            let samples: Vec<Sample> = ride(1200).iter().map(|s| Sample { profile, ..*s }).collect();
            let mut sink = Cut { disk: Vec::new(), budget: f.len() / 2 };
            let (mut w, mut rec) = Writer::start(&mut sink, T0).unwrap();
            assert!(write(&mut sink, &mut w, &samples, &mut rec).is_none());
            let (mut disk, mut w) = (sink.disk, Writer::from_record(&rec).unwrap());
            w.checkpoint(&mut disk).unwrap();
            assert_eq!(w.profile(), profile, "resumed with its profile");
            let next: Vec<Sample> = samples.iter().filter(|s| s.time > w.last).copied().collect();
            write(&mut disk, &mut w, &next, &mut rec).unwrap();
            std::fs::write(dir.join(name), &disk).unwrap();
        }
        let (mut w, mut f) = record_before_sub_sport(ride::Profile::Road);
        w.finish(&mut f, T0 + 121).unwrap();
        std::fs::write(dir.join("older.fit"), &f).unwrap();
    }
}
