//! The ride file: a FIT activity (Garmin's public FIT protocol), written as the ride goes so that a reset or a
//! power cut never loses more than the data since the last checkpoint (AGENTS.md, "never lose a ride").
//!
//! Layout: a 14-byte header (data size, its own CRC), then definition and data messages (little-endian), then the
//! file CRC. Messages: file_id, event (timer start, stop at a pause, start at a resume), one record a second (only
//! the fields that are present: a new record definition goes out when that set changes), a lap at each Lap key,
//! a gear change event at each shift of an electronic groupset ([`Writer::gear`]), and at the end the last lap,
//! session and activity.
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
//! The caller keeps two copies of the record (old / new) and takes the valid one with the larger
//! [`Writer::data_len`]. Doing this twice is harmless.
//!
//! **Errors.** A failed write leaves the writer where it was (the next message goes to the same offset), so a
//! failing sink loses samples, never the file. Nothing here panics: buffers are fixed and accessed with checks.
//!
//! The file CRC covers the header too, and the header changes with the size: [`crc_combine`] joins the header's
//! CRC to the running data CRC in about 400 steps, whatever the file length, so a checkpoint never reads the file.
//!
//! **Developer fields** ([`Writer::begin_with`] `dev`): the full copy of a ride (recorder.rs) is protocol 2.0 with,
//! after file_id, a `developer_data_id` (207: application_id, developer_data_index 0) and two `field_description`s
//! (206: developer_data_index, field_definition_number, fit_base_type_id, field_name, units, native_mesg_num 20),
//! `Alpha1` (float32, no units: the name Intervals.icu reads DFA a1 from, as AlphaHRV writes it,
//! forum.intervals.icu/t/average-dfa-a1-and-wbal-added-to-interval-fields/7528) and `RMSSD` (uint16, ms); every
//! record then carries both (invalid when not known), its definition with the developer flag (header bit 5) and
//! (field number, size, developer data index) per field. Message and field numbers: `garmin_fit_sdk/profile.py`
//! 21.218 (github.com/garmin/fit-python-sdk), the protocol: developer.garmin.com/fit/protocol. Without `dev` the
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

use crate::epoch_s;

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
    (epoch_s(utc) - 631_065_600).clamp(0, i64::from(u32::MAX)) as u32
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
}

/// Size of a recovery record ([`Writer::checkpoint`]).
pub const RECORD_LEN: usize = 4 * WORDS + 4;

const HEADER: u32 = 14;
const WORDS: usize = 21;
const TAG: u16 = 0xf17f; // first two bytes of a recovery record (and a version: change it with the layout)

// base types (FIT profile)
const ENUM: u8 = 0x00;
const S8: u8 = 0x01;
const U8: u8 = 0x02;
const U16: u8 = 0x84;
const S32: u8 = 0x85;
const U32: u8 = 0x86;
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

/// The full copy's developer data, after file_id (module doc): developer_data_id, then a field_description each
/// for `Alpha1` and `RMSSD`, definitions included.
const DEV_MESSAGES: [u8; 90] = {
    const APP: &[u8; 16] = b"bryton-rustfw-01"; // application_id: any 16 bytes that name this firmware
    let mut m = [0u8; 90];
    let head: [u8; 12] = [0x40 | L_DEV_ID, 0, 0, 207, 0, 2, 1, 16, BYTE, 3, 1, U8];
    let def: [u8; 24] =
        [0x40 | L_FIELD_DESC, 0, 0, 206, 0, 6, 0, 1, U8, 1, 1, U8, 2, 1, U8, 3, 8, STRING, 8, 4, STRING, 14, 2, U16];
    // index, number, base type, name (8), units (4), native_mesg_num record (20)
    let alpha: [u8; 18] = [L_FIELD_DESC, 0, 0, F32, b'A', b'l', b'p', b'h', b'a', b'1', 0, 0, 0, 0, 0, 0, 20, 0];
    let rmssd: [u8; 18] = [L_FIELD_DESC, 0, 1, U16, b'R', b'M', b'S', b'S', b'D', 0, 0, 0, b'm', b's', 0, 0, 20, 0];
    let mut i = 0;
    while i < 90 {
        m[i] = match i {
            0..12 => head[i],
            12 => L_DEV_ID,
            13..29 => APP[i - 13],
            29 => 0, // developer_data_index
            30..54 => def[i - 30],
            54..72 => alpha[i - 54],
            _ => rmssd[i - 72],
        };
        i += 1;
    }
    m
};
/// The developer fields of a full copy's record: (field_definition_number, size, developer_data_index).
const DEV_RECORD: [Field; 2] = [(0, 4, 0), (1, 2, 0)];

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
/// cadence, power, event, event_type. Lap and session take the same values; the field numbers differ.
const LAP: [Field; 15] = [
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
];
/// [`LAP`]'s fields, then sport, first_lap_index, num_laps.
const SESSION: [Field; 18] = [
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
    (5, 1, ENUM),
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
    /// The current lap's start (FIT time) and distance then (mm); laps ended so far.
    lap_start: u32,
    lap_dist: u32,
    laps: u32,
    /// Seconds paused (Start key) so far; when the current pause began (0: not paused).
    paused_s: u32,
    paused_at: u32,
    /// The full copy: protocol 2.0, developer fields in the records (module doc).
    dev: bool,
}

impl Writer {
    /// Nothing written yet ([`Writer::begin`] starts a file).
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
            lap_start: 0,
            lap_dist: 0,
            laps: 0,
            paused_s: 0,
            paused_at: 0,
            dev: false,
        }
    }

    /// Starts a ride file at FIT time `time`: header, file_id, timer start, then a checkpoint (its record is
    /// returned). The sink should be empty.
    pub fn start(sink: &mut impl Sink, time: u32) -> Result<(Writer, [u8; RECORD_LEN]), Error> {
        let mut w = Writer::begin(sink, time)?;
        let rec = w.checkpoint(sink)?;
        Ok((w, rec))
    }

    /// [`Writer::start`] without its checkpoint: file_id and timer start only (the recorder buffers them in RAM
    /// and checkpoints once the file exists).
    pub fn begin(sink: &mut impl Sink, time: u32) -> Result<Writer, Error> {
        Writer::begin_with(sink, time, false)
    }

    /// [`Writer::begin`], with developer fields when `dev` (the full copy: module doc).
    pub fn begin_with(sink: &mut impl Sink, time: u32, dev: bool) -> Result<Writer, Error> {
        let mut w = Writer { start: time, last: time, lap_start: time, dev, ..Writer::default() };
        // 255: development (not a registered manufacturer: this is not Bryton's firmware)
        w.message(sink, L_FILE_ID, 0, &FILE_ID, &[4, 255, 0, time])?;
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

    /// A lap ended at `time` (the Lap key): a lap message from the last one. Its averages and maxima are left out
    /// (invalid; the session has the ride's): they would cost three more stats in the recovery record.
    pub fn lap(&mut self, sink: &mut impl Sink, time: u32) -> Result<(), Error> {
        self.last = self.last.max(time);
        let mut v = self.lap_values();
        v[7..13].fill(u32::MAX);
        self.message(sink, L_LAP, 19, &LAP, &v)?;
        (self.lap_start, self.lap_dist, self.laps) = (self.last, self.dist_mm, self.laps + 1);
        Ok(())
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
    fn lap_values(&self) -> [u32; 15] {
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
        self.stats[0].add(s.hr.map(u32::from));
        self.stats[1].add(s.cadence.map(u32::from));
        self.stats[2].add(s.power.map(u32::from));

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
        if mask != self.mask {
            self.defined &= !(1 << L_RECORD);
        }
        let (fields, values) = (fields.get(..n).unwrap_or(&[]), values.get(..n).unwrap_or(&[]));
        if self.dev {
            // invalid: float32 all ones, uint16 0xffff
            let alpha = s.alpha1_milli.map_or(u32::MAX, |a| (f32::from(a) / 1000.0).to_bits());
            let dev = [alpha, s.rmssd_ms.map_or(0xffff, u32::from)];
            self.message_dev(sink, L_RECORD, 20, fields, values, &DEV_RECORD, &dev)?;
        } else {
            self.message(sink, L_RECORD, 20, fields, values)?;
        }
        self.mask = mask;
        Ok(())
    }

    /// Ends the ride at FIT time `time`: [`Writer::end`], then a checkpoint (the finished file).
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
        let mut lap = self.lap_values();
        if self.laps > 0 {
            lap[7..13].fill(u32::MAX);
        }
        self.message(sink, L_LAP, 19, &LAP, &lap)?;
        // the ride as one lap: the session's values
        let whole = Writer { lap_start: self.start, lap_dist: 0, ..*self }.lap_values();
        // session: event session (8), stop (1), cycling (2), first lap 0, the laps
        let mut v = [0u32; 18];
        v[..15].copy_from_slice(&whole);
        (v[13], v[15], v[16], v[17]) = (8, 2, 0, self.laps + 1);
        self.message(sink, L_SESSION, 18, &SESSION, &v)?;
        // activity: one session, manual (0), event activity (26), stop (1)
        self.message(sink, L_ACTIVITY, 34, &ACTIVITY, &[self.last, whole[3], 1, 0, 26, 1])?;
        Ok(())
    }

    /// Makes the file on the sink valid as it stands (CRC after the data, header with its size), flushes it, and
    /// returns the recovery record to store (only once this returns Ok). Also the resume step (module doc).
    pub fn checkpoint(&mut self, sink: &mut impl Sink) -> Result<[u8; RECORD_LEN], Error> {
        let mut h = [0u8; HEADER as usize];
        h[0] = HEADER as u8;
        h[1] = if self.dev { 0x20 } else { 0x10 }; // protocol 2.0 for developer fields, else 1.0
        h[2..4].copy_from_slice(&2140u16.to_le_bytes()); // profile 21.40
        h[4..8].copy_from_slice(&self.len.to_le_bytes());
        h[8..12].copy_from_slice(b".FIT");
        let hc = crc(0, &h[..12]);
        h[12..14].copy_from_slice(&hc.to_le_bytes());
        let file_crc = crc_combine(crc(0, &h), self.crc, self.len);
        let end = HEADER + self.len;
        sink.write_at(end, &file_crc.to_le_bytes())?;
        sink.write_at(0, &h)?;
        sink.set_len(end + 2)?;
        sink.flush()?;
        Ok(self.to_record())
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

    /// The writer a recovery record holds, if the record is whole.
    pub fn from_record(r: &[u8; RECORD_LEN]) -> Option<Writer> {
        let (body, tail) = r.split_at(RECORD_LEN - 2);
        if crc(0, body).to_le_bytes() != tail || body.get(..2)? != TAG.to_le_bytes() {
            return None;
        }
        let mut w = [0u32; WORDS];
        for (x, c) in w.iter_mut().zip(body.get(2..)?.chunks_exact(4)) {
            *x = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        }
        let s = |i: usize| Stat { sum: w[i], n: w[i + 1], max: w[i + 2] };
        Some(Writer {
            len: w[0],
            crc: w[1] as u16,
            defined: (w[1] >> 16) as u16,
            mask: w[2] as u16,
            dev: w[2] >> 16 & 1 != 0,
            start: w[3],
            last: w[4],
            dist_mm: w[5],
            max_speed: w[6],
            stats: [s(7), s(10), s(13)],
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
            u32::from(self.crc) | u32::from(self.defined) << 16,
            u32::from(self.mask) | u32::from(self.dev) << 16,
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
        let mut buf = [0u8; 128]; // the largest, session with its definition: 109; a full record: 87
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
            put(&[0x40 | flag | local, 0, 0, g0, g1, f.len() as u8]);
            for &(num, size, ty) in f {
                put(&[num, size, ty]);
            }
            if !df.is_empty() {
                put(&[df.len() as u8]);
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
        sink.write_at(HEADER + self.len, bytes)?;
        self.len += bytes.len() as u32;
        self.crc = crc(self.crc, bytes);
        Ok(())
    }
}

/// Degrees x 1e-7 to FIT semicircles (2^31 per 180 degrees).
fn semicircles(deg_e7: i32) -> u32 {
    (i64::from(deg_e7) * (1 << 31) / 1_800_000_000) as i32 as u32
}

/// FIT's CRC-16 (CRC-16/ARC: reflected polynomial 0xa001), continued from `crc` over `data`, a byte a step from a
/// 256-entry table (512 bytes of flash; the SDK's 4-bit table takes two steps a byte).
///
/// ```
/// assert_eq!(bryton_rustfw::fit::crc(0, b"123456789"), 0xbb3d); // CRC-16/ARC
/// ```
pub fn crc(mut crc: u16, data: &[u8]) -> u16 {
    for &b in data {
        crc = (crc >> 8) ^ CRC_TABLE[usize::from((crc as u8) ^ b)];
    }
    crc
}

/// [`crc`]'s table: the CRC of each byte from 0. In the image, so in the SDRAM, which is read uncached (the D-cache is
/// off): a reader that runs it over megabytes keeps a copy where it lives ([`crate::fitread::Reader`]).
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
    fn times(m: &[u16; 16], mut v: u16) -> u16 {
        let (mut s, mut i) = (0, 0);
        while v != 0 {
            if v & 1 != 0 {
                s ^= m[i & 15];
            }
            v >>= 1;
            i += 1;
        }
        s
    }
    let square = |m: &[u16; 16]| core::array::from_fn(|i| times(m, m[i]));
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
mod tests {
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
    type Msg = (u16, Vec<(u8, u64)>);

    /// A strict FIT reader for what this writer writes: header and its CRC, data size, file CRC, normal headers,
    /// every data message on a definition. Developer fields come out as field 230 + their number.
    fn decode(f: &[u8]) -> Result<Vec<Msg>, &'static str> {
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
        let mut defs: [Option<(u16, Vec<(u8, u8)>)>; 16] = Default::default();
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

    fn field(m: &Msg, num: u8) -> Option<u64> {
        m.1.iter().find(|f| f.0 == num).map(|f| f.1)
    }

    const T0: u32 = 1_161_676_800; // 2026-10-23 08:00 UTC

    /// A synthetic ride: GPS from the start, HR from 10 s, power and cadence from 30 s, a power dropout at 200 s.
    fn ride(secs: u32) -> Vec<Sample> {
        (0..secs)
            .map(|t| Sample {
                time: T0 + 1 + t,
                pos: Some((488_566_000 + 300 * t as i32, 23_522_000 - 200 * t as i32)),
                alt_mm: Some(35_000 + 100 * (t % 50) as i32),
                speed_mm_s: Some(7000 + 10 * (t % 100)),
                hr: (t >= 10).then_some(120 + (t % 40) as u8),
                cadence: (t >= 30).then_some(85 + (t % 10) as u8),
                power: (t >= 30 && !(200..215).contains(&t)).then_some(200 + (t % 60) as u16),
                temp_c: Some((t % 30) as i8 - 5),
                alpha1_milli: (t >= 60).then_some(500 + (t % 700) as u16),
                rmssd_ms: (t >= 60).then_some(20 + (t % 40) as u16),
            })
            .collect()
    }

    /// A gear change of the test ride at `time`, if one: every 45 s from 7 s, front every third time.
    fn shift(time: u32) -> Option<(bool, u8, u8)> {
        let t = time - T0;
        (t % 45 == 7).then_some((t % 135 == 7, (t / 135 % 2 + 1) as u8, (t / 45 % 11 + 1) as u8))
    }

    /// Writes `samples` from `w`, a checkpoint every 60 s; the last record stored whole, None if the sink failed.
    fn write(sink: &mut impl Sink, w: &mut Writer, samples: &[Sample], rec: &mut [u8; RECORD_LEN]) -> Option<()> {
        for s in samples {
            w.record(sink, s).ok()?;
            if let Some((front, f, r)) = shift(s.time) {
                w.gear(sink, s.time, front, f, r).ok()?;
            }
            if (s.time - T0) % 60 == 0 {
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
        let r = records[100];
        assert_eq!(field(r, 253), Some(u64::from(T0 + 101)));
        assert_eq!(field(r, 0), Some(u64::from(semicircles(488_596_000))));
        assert_eq!(field(r, 3), Some(120 + 100 % 40));
        assert_eq!(field(r, 7), Some(200 + 100 % 60));
        assert_eq!(field(records[0], 3), None, "no HR yet: the field is left out");
        assert_eq!(field(records[205], 7), None, "power dropout");
        let session = m.iter().find(|x| x.0 == 18).unwrap();
        assert_eq!(field(session, 17), Some(159), "max HR");
        assert_eq!(field(session, 21), Some(259), "max power");
        assert_eq!(Writer::from_record(&rec).map(|w| w.data_len() as usize), Some(f.len() - 16));
    }

    /// A pause (timer stop and start events, left out of the timer time), a lap, temperatures below zero; and the
    /// writer survives a recovery record taken while paused.
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
                    w = Writer::from_record(&rec).unwrap();
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
        assert_eq!(field(laps[0], 15), Some(0xff), "no averages per lap");
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
            if let Some((front, f, r)) = shift(s.time) {
                w.gear(&mut full, s.time, front, f, r).unwrap(); // the gear events cut and resumed too
            }
            if (s.time - T0) % 60 == 0 {
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
                assert_eq!(decode(&disk).unwrap().iter().filter(|x| x.0 == 20).count(), 200, "cut at {budget}");
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
        let bare: Vec<Sample> = samples.iter().map(|s| Sample { alpha1_milli: None, rmssd_ms: None, ..*s }).collect();
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
        assert!(decode(&clean).unwrap().iter().all(|m| m.1.iter().all(|f| !(230..240).contains(&f.0))));
        let full = file(true, &samples);
        assert_eq!(full[1], 0x20);
        let m = decode(&full).unwrap();
        let globals: Vec<u16> = m.iter().map(|x| x.0).take(5).collect();
        assert_eq!(globals, [0, 207, 206, 206, 21], "file_id, developer data, timer start");
        let records: Vec<&Msg> = m.iter().filter(|x| x.0 == 20).collect();
        assert_eq!(records.len(), 300);
        assert_eq!(field(records[10], 230), Some(0xffff_ffff), "no DFA a1 yet: invalid");
        assert_eq!(field(records[10], 231), Some(0xffff));
        let a = f32::from_bits(field(records[100], 230).unwrap() as u32);
        assert!((a - (500 + 100 % 700) as f32 / 1000.0).abs() < 1e-6, "{a}");
        assert_eq!(field(records[100], 231), Some(20 + 100 % 40));
        // the same records otherwise: the clean file's fields, in the same order
        let cm = decode(&clean).unwrap();
        let cr: Vec<&Msg> = cm.iter().filter(|x| x.0 == 20).collect();
        for (c, f) in cr.iter().zip(&records) {
            assert_eq!(c.1[..], f.1[..f.1.len() - 2]);
        }
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
    }
}
