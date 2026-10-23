//! A FIT reader that takes the file one byte at a time ([`Reader::push`]), so a ride of any length streams through
//! a small buffer: no heap, no message buffered whole. It gives only what the stock rides are read for: per
//! `record`, the time, power, heart rate and distance; per `session`, the start, timer time, distance, average and
//! maximum power, average heart rate, ascent. **The position only on request:** the fields are picked by number,
//! and the position and altitude fields only by a reader made with [`Reader::with_position`] (the card of a past
//! ride draws its track; the scan of all rides never asks for it).
//!
//! Protocol (Garmin's FIT protocol, developer.garmin.com/fit/protocol, as Garmin's Python SDK decodes it,
//! github.com/garmin/fit-python-sdk `garmin_fit_sdk/decoder.py`, checked 2026-10-23):
//! - header: size 12 or 14 bytes, protocol, profile, data size (u32 LE), ".FIT", and with 14 bytes a CRC of the
//!   first 12 (0: not set). After the data, the file CRC, so that the CRC of the whole file, header included,
//!   is 0 ([`crate::fit::crc`], the SDK's table).
//! - record header: bit 7 a compressed timestamp header (bits 5..6 the local message type 0..3, bits 0..4 the
//!   time offset); otherwise bit 6 a definition, bit 5 (in a definition) developer fields follow, bits 0..3 the
//!   local message type.
//! - definition: reserved, architecture (0 little endian, 1 big endian), global message number (in that order),
//!   field count, then (number, size, base type) per field; with bit 5, a developer field count and (number,
//!   size, developer data index) per field, which are only skipped here.
//! - compressed timestamp: from the last timestamp seen (field 253 of any message), the 5-bit offset rolled
//!   forward (fitdecode, github.com/polyvertex/fitdecode `reader.py` `_apply_compressed_accumulation`: the low 5
//!   bits replaced, plus 32 if the offset is below them).
//! - an invalid value is all ones for an unsigned type (FIT profile base types); such a field reads as None.
//! - message 20 `record`: 253 timestamp, 3 heart_rate (bpm), 5 distance (1/100 m), 7 power (W). Message 18
//!   `session`: 2 start_time, 8 total_timer_time (ms), 9 total_distance (1/100 m), 16 avg_heart_rate,
//!   20 avg_power, 21 max_power, 22 total_ascent (m) (FIT profile, Profile.xlsx of the SDK). With the position:
//!   record 0 position_lat and 1 position_long (sint32 semicircles, 2^31 per 180 degrees; invalid 0x7fffffff),
//!   2 altitude (uint16) and 78 enhanced_altitude (uint32), both (m + 500) x 5 (the same profile); when both are
//!   there, 78, the wider one, is taken.
//!
//! Only the first file of a chained FIT file is read. Nothing here panics or loops on any input: each byte moves a
//! small state machine one step (fuzzed in sim.rs `fuzz`).

use crate::fit::crc;

/// One second of ride as a `record` message gives it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// FIT time (seconds since 1989-12-31 UTC): its timestamp field, or the compressed header's.
    pub time: u32,
    /// Power, W.
    pub power: Option<u16>,
    /// Heart rate, bpm.
    pub hr: Option<u8>,
    /// Distance from the start, 1/100 m.
    pub dist_cm: Option<u32>,
    /// Latitude and longitude, semicircles (only from a reader [`Reader::with_position`]).
    pub pos: Option<(i32, i32)>,
    /// Altitude, decimetres (only from a reader [`Reader::with_position`]).
    pub alt_dm: Option<i32>,
}

/// The ride's totals as its `session` message gives them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// FIT time of the start.
    pub start: Option<u32>,
    /// Timer (moving) time, ms.
    pub timer_ms: Option<u32>,
    /// Distance, 1/100 m.
    pub dist_cm: Option<u32>,
    /// Average power, W.
    pub avg_power: Option<u16>,
    /// Maximum power, W.
    pub max_power: Option<u16>,
    /// Average heart rate, bpm.
    pub avg_hr: Option<u8>,
    /// Ascent, m.
    pub ascent_m: Option<u16>,
}

/// A message the reader gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    /// A `record`.
    Record(Record),
    /// A `session`.
    Session(Session),
}

/// Why a file is not a whole FIT file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The header is not a FIT header, or its CRC is wrong.
    Header,
    /// A data message on a local type with no definition.
    Undefined,
    /// The data size ends inside a message.
    Truncated,
    /// The file CRC is wrong.
    Crc,
}

/// Values picked per message, by slot: 0 is always the timestamp.
const SLOTS: usize = 8;
/// Picked fields per definition at most.
const PICKED: usize = 8;

/// The slot of field `num` of global message `global`, if it is one this reader keeps (the position and altitude
/// only with `position`).
fn slot(global: u16, num: u8, position: bool) -> Option<u8> {
    Some(match (global, num) {
        (_, 253) => 0,
        (20, 3) => 1,              // heart_rate
        (20, 5) => 2,              // distance
        (20, 7) => 3,              // power
        (20, 0) if position => 4,  // position_lat
        (20, 1) if position => 5,  // position_long
        (20, 2) if position => 6,  // altitude
        (20, 78) if position => 7, // enhanced_altitude
        (18, 2) => 1,              // start_time
        (18, 8) => 2,              // total_timer_time
        (18, 9) => 3,              // total_distance
        (18, 16) => 4,             // avg_heart_rate
        (18, 20) => 5,             // avg_power
        (18, 21) => 6,             // max_power
        (18, 22) => 7,             // total_ascent
        _ => return None,
    })
}

/// A definition: the global message, the byte order, the message size and where the picked fields are.
#[derive(Clone, Copy, Debug, Default)]
struct Def {
    global: u16,
    big: bool,
    size: u32,
    /// (offset in the message, size 1, 2 or 4, slot); a field past 64 KB into a message is not picked
    picked: [(u16, u8, u8); PICKED],
    n: u8,
    defined: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Header,
    /// Waiting for a record header.
    Next,
    /// The 5 fixed bytes of a definition.
    DefHead,
    /// Field definitions still to come.
    DefFields(u8),
    /// The developer field count.
    DevCount,
    /// Developer field definitions still to come.
    DevFields(u8),
    /// A data message's bytes.
    Data,
    /// The file CRC (bytes seen).
    Crc(u8),
    Done,
}

/// The reader's state: the definitions and the message in progress.
pub struct Reader {
    state: State,
    head: [u8; 14],
    head_len: u8,
    data_left: u32,
    crc: u16,
    defs: [Def; 16],
    /// The message in progress: its local type, the bytes of it seen, the definition being built.
    local: usize,
    at: u32,
    tmp: [u8; 5],
    dev: bool,
    building: Def,
    vals: [u32; SLOTS],
    present: u8,
    compressed: Option<u32>,
    last_ts: u32,
    pos: u32,
    result: Option<Result<(), Error>>,
    /// [`crate::fit::CRC_TABLE`], copied: with the reader, in DTCM (seen on the unit: reading the table from the
    /// image, in uncached SDRAM, costs most of the scan's time).
    table: [u16; 256],
    /// The position and altitude are read too ([`Reader::with_position`]).
    position: bool,
}

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader {
    /// A reader at the start of a file.
    pub const fn new() -> Self {
        const D: Def = Def { global: 0, big: false, size: 0, picked: [(0, 0, 0); PICKED], n: 0, defined: false };
        Reader {
            state: State::Header,
            head: [0; 14],
            head_len: 0,
            data_left: 0,
            crc: 0,
            defs: [D; 16],
            local: 0,
            at: 0,
            tmp: [0; 5],
            dev: false,
            building: D,
            vals: [0; SLOTS],
            present: 0,
            compressed: None,
            last_ts: 0,
            pos: 0,
            result: None,
            table: crate::fit::CRC_TABLE,
            position: false,
        }
    }

    /// A reader that also gives each record's position and altitude ([`Record::pos`], [`Record::alt_dm`]).
    pub const fn with_position() -> Self {
        Reader { position: true, ..Self::new() }
    }

    /// Ok once the file ended with a good CRC; an error as soon as one is found; None while it goes on.
    pub fn result(&self) -> Option<Result<(), Error>> {
        self.result
    }

    /// Bytes of the file taken so far.
    pub fn offset(&self) -> u32 {
        self.pos
    }

    fn fail(&mut self, e: Error) -> Option<Msg> {
        self.result = Some(Err(e));
        self.state = State::Done;
        None
    }

    /// The next byte of the file; a message when this byte completes one the reader keeps.
    pub fn push(&mut self, b: u8) -> Option<Msg> {
        if self.state == State::Done {
            return None;
        }
        self.pos = self.pos.saturating_add(1);
        self.crc = crc(self.crc, &[b]);
        match self.state {
            State::Header => return self.header(b),
            State::Crc(n) => {
                if n == 0 {
                    self.state = State::Crc(1);
                } else {
                    self.state = State::Done;
                    self.result = Some(if self.crc == 0 { Ok(()) } else { Err(Error::Crc) });
                }
                return None;
            }
            _ => {}
        }
        self.data_left = self.data_left.saturating_sub(1);
        let msg = match self.state {
            State::Next => self.record_header(b),
            State::DefHead => self.def_head(b),
            State::DefFields(left) => {
                self.def_field(b, left);
                None
            }
            State::DevCount => {
                self.dev_count(b);
                None
            }
            State::DevFields(left) => {
                self.dev_field(b, left);
                None
            }
            State::Data => self.data(b),
            State::Header | State::Crc(_) | State::Done => None,
        };
        if self.data_left == 0 && self.state != State::Done {
            if self.state != State::Next {
                return self.fail(Error::Truncated);
            }
            self.state = State::Crc(0);
        }
        msg
    }

    fn header(&mut self, b: u8) -> Option<Msg> {
        if let Some(slot) = self.head.get_mut(usize::from(self.head_len)) {
            *slot = b;
        }
        self.head_len += 1;
        let size = self.head[0];
        if size != 12 && size != 14 {
            return self.fail(Error::Header);
        }
        if self.head_len < size {
            return None;
        }
        let h = self.head;
        let header_crc = u16::from_le_bytes([h[12], h[13]]);
        if &h[8..12] != b".FIT" || (size == 14 && header_crc != 0 && crc(0, &h[..12]) != header_crc) {
            return self.fail(Error::Header);
        }
        self.data_left = u32::from_le_bytes([h[4], h[5], h[6], h[7]]);
        self.state = if self.data_left == 0 { State::Crc(0) } else { State::Next };
        None
    }

    fn record_header(&mut self, b: u8) -> Option<Msg> {
        if b & 0x80 != 0 {
            // compressed timestamp header
            let offset = u32::from(b & 0x1f);
            let mut t = (self.last_ts & !0x1f) + offset;
            if offset < self.last_ts & 0x1f {
                t = t.wrapping_add(0x20);
            }
            self.compressed = Some(t);
            self.last_ts = t;
            return self.data_start(usize::from(b >> 5 & 3));
        }
        self.compressed = None;
        self.local = usize::from(b & 0xf);
        if b & 0x40 != 0 {
            self.dev = b & 0x20 != 0;
            (self.at, self.state) = (0, State::DefHead);
            return None;
        }
        self.data_start(self.local)
    }

    fn data_start(&mut self, local: usize) -> Option<Msg> {
        self.local = local;
        let Some(def) = self.defs.get(local).filter(|d| d.defined) else {
            return self.fail(Error::Undefined);
        };
        (self.at, self.vals, self.present) = (0, [0; SLOTS], 0);
        if def.size == 0 {
            self.state = State::Next;
            return self.data_end();
        }
        self.state = State::Data;
        None
    }

    fn def_head(&mut self, b: u8) -> Option<Msg> {
        if let Some(t) = self.tmp.get_mut(self.at as usize) {
            *t = b;
        }
        self.at += 1;
        if self.at < 5 {
            return None;
        }
        let big = self.tmp[1] != 0;
        let g = [self.tmp[2], self.tmp[3]];
        let global = if big { u16::from_be_bytes(g) } else { u16::from_le_bytes(g) };
        self.building = Def { global, big, ..Def::default() };
        self.at = 0;
        let n = self.tmp[4];
        if n > 0 {
            self.state = State::DefFields(n);
        } else {
            self.def_fields_done();
        }
        None
    }

    fn def_field(&mut self, b: u8, left: u8) {
        if let Some(t) = self.tmp.get_mut(self.at as usize) {
            *t = b;
        }
        self.at += 1;
        if self.at < 3 {
            return;
        }
        self.at = 0;
        let (num, size) = (self.tmp[0], self.tmp[1]);
        let d = &mut self.building;
        if let Some(s) = slot(d.global, num, self.position)
            && matches!(size, 1 | 2 | 4)
            && let Ok(off) = u16::try_from(d.size)
            && let Some(p) = d.picked.get_mut(usize::from(d.n))
        {
            *p = (off, size, s);
            d.n += 1;
        }
        d.size += u32::from(size);
        if left > 1 {
            self.state = State::DefFields(left - 1);
        } else {
            self.def_fields_done();
        }
    }

    fn def_fields_done(&mut self) {
        if self.dev {
            self.state = State::DevCount;
        } else {
            self.def_end();
        }
    }

    fn dev_count(&mut self, n: u8) {
        self.at = 0;
        if n > 0 {
            self.state = State::DevFields(n);
        } else {
            self.def_end();
        }
    }

    fn dev_field(&mut self, b: u8, left: u8) {
        self.at += 1;
        if self.at == 2 {
            self.building.size += u32::from(b); // (number, size, developer data index): only the size matters
        }
        if self.at < 3 {
            return;
        }
        self.at = 0;
        if left > 1 {
            self.state = State::DevFields(left - 1);
        } else {
            self.def_end();
        }
    }

    fn def_end(&mut self) {
        if let Some(d) = self.defs.get_mut(self.local) {
            *d = Def { defined: true, ..self.building };
        }
        self.state = State::Next;
    }

    fn data(&mut self, b: u8) -> Option<Msg> {
        let Some(def) = self.defs.get(self.local) else { return self.fail(Error::Undefined) };
        let (n, size) = (usize::from(def.n), def.size);
        self.take(self.local, self.at, &[b], n);
        self.at += 1;
        if self.at < size {
            return None;
        }
        self.state = State::Next;
        self.data_end()
    }

    /// Bytes `bytes` of the data message on local type `local`, from its byte `at`: into the picked fields they
    /// belong to (the definition's first `n`).
    fn take(&mut self, local: usize, at: u32, bytes: &[u8], n: usize) {
        let Some(def) = self.defs.get(local) else { return };
        let end = at + bytes.len() as u32;
        for &(off, size, s) in def.picked.iter().take(n) {
            let (off, size) = (u32::from(off), u32::from(size));
            let (from, to) = (off.max(at), (off + size).min(end));
            let Some(v) = self.vals.get_mut(usize::from(s)) else { continue };
            for k in from..to {
                let b = u32::from(bytes.get((k - at) as usize).copied().unwrap_or(0));
                *v = if def.big { *v << 8 | b } else { *v | b << (8 * (k - off)) };
            }
            if from < to && to == off + size {
                let signed = def.global == 20 && (s == 4 || s == 5); // sint32: invalid is the largest value
                let invalid = if signed {
                    0x7fff_ffff
                } else if size == 4 {
                    u32::MAX
                } else {
                    (1 << (8 * size)) - 1
                };
                if *v != invalid {
                    self.present |= 1 << s;
                }
            }
        }
    }

    /// Bytes of the file, as many as `push` would take one by one, each message the reader keeps given to `f`;
    /// returns how many were taken (fewer than `buf.len()` once the file has ended, or an error was found). The
    /// bytes of a data message are taken together: its picked fields decoded straight from the buffer, its CRC
    /// over the run, so most of a file costs a table step a byte (what the scan of stock's rides runs on).
    ///
    /// ```
    /// use bryton_rustfw::fitread::Reader;
    /// let (mut a, mut b) = (Reader::new(), Reader::new());
    /// let file = [14, 0x20, 0, 0, 0, 0, 0, 0, b'.', b'F', b'I', b'T', 0, 0, 0, 0];
    /// let n = a.push_slice(&file, |_| {});
    /// file.iter().for_each(|&x| drop(b.push(x)));
    /// assert_eq!((n, a.result()), (file.len(), b.result()));
    /// ```
    pub fn push_slice(&mut self, buf: &[u8], mut f: impl FnMut(Msg)) -> usize {
        let mut i = 0;
        while let Some(rest) = buf.get(i..).filter(|r| !r.is_empty()) {
            if self.state == State::Done {
                break;
            }
            if self.state == State::Data
                && let Some(&Def { size, n, .. }) = self.defs.get(self.local)
            {
                let left = size.saturating_sub(self.at);
                let k = (left.min(self.data_left) as usize).min(rest.len());
                if k > 1 {
                    // all of the message the buffer has but its last byte, which `push` takes (it ends it)
                    let run = rest.get(..k - 1).unwrap_or(&[]);
                    self.take(self.local, self.at, run, usize::from(n));
                    for &b in run {
                        self.crc = (self.crc >> 8) ^ self.table[usize::from((self.crc as u8) ^ b)];
                    }
                    let len = run.len() as u32;
                    (self.at, self.pos, self.data_left) = (self.at + len, self.pos + len, self.data_left - len);
                    i += run.len();
                    continue;
                }
            }
            if let Some(m) = rest.first().and_then(|&b| self.push(b)) {
                f(m);
            }
            i += 1;
        }
        i
    }

    /// A data message is whole: the timestamp kept, and the message given if it is one the reader keeps.
    fn data_end(&mut self) -> Option<Msg> {
        let get = |s: usize| (self.present & 1 << s != 0).then(|| self.vals.get(s).copied().unwrap_or(0));
        if let (None, Some(t)) = (self.compressed, get(0)) {
            self.last_ts = t;
        }
        let time = self.compressed.or(get(0)).unwrap_or(self.last_ts);
        let global = self.defs.get(self.local).map_or(0, |d| d.global);
        // a field given a size larger than its type is cut to the type
        let u8_ = |s| get(s).map(|v| v.min(0xfe) as u8);
        let u16_ = |s| get(s).map(|v| v.min(0xfffe) as u16);
        match global {
            20 => {
                let pos = get(4).zip(get(5)).map(|(lat, lon)| (lat as i32, lon as i32));
                // (m + 500) x 5 to decimetres; a uint16 altitude given in 4 bytes still reads as its value
                let alt_dm = get(7).or(get(6)).map(|v| (i64::from(v) * 2 - 5000) as i32);
                Some(Msg::Record(Record { time, hr: u8_(1), dist_cm: get(2), power: u16_(3), pos, alt_dm }))
            }
            18 => Some(Msg::Session(Session {
                start: get(1),
                timer_ms: get(2),
                dist_cm: get(3),
                avg_hr: u8_(4),
                avg_power: u16_(5),
                max_power: u16_(6),
                ascent_m: u16_(7),
            })),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::fit::{Sample, Writer};
    use std::vec::Vec;

    fn read(f: &[u8]) -> (Vec<Msg>, Option<Result<(), Error>>) {
        let mut r = Reader::new();
        let msgs = f.iter().filter_map(|&b| r.push(b)).collect();
        (msgs, r.result())
    }

    const T0: u32 = 1_161_676_800;

    /// fit.rs's writer: a ride with power from 10 s and a dropout, then its session.
    fn written() -> Vec<u8> {
        let mut f = Vec::new();
        let (mut w, _) = Writer::start(&mut f, T0).unwrap();
        for t in 0..120u32 {
            let s = Sample {
                time: T0 + 1 + t,
                pos: Some((10, 20)),
                speed_mm_s: Some(5000),
                hr: Some(130),
                power: (t >= 10 && !(50..55).contains(&t)).then_some(200 + t as u16),
                ..Sample::default()
            };
            w.record(&mut f, &s).unwrap();
        }
        w.finish(&mut f, T0 + 121).unwrap();
        f
    }

    /// `push_slice` gives what `push` gives, byte by byte, on whole and damaged files cut into any chunks.
    #[test]
    fn push_slice_equals_push() {
        let good = written();
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..300 {
            let mut f = good.clone();
            for _ in 0..(round % 4) {
                let i = rnd() as usize % f.len();
                f[i] = rnd() as u8;
            }
            let (want, want_r) = read(&f);
            let mut r = Reader::new();
            let mut got = Vec::new();
            let mut at = 0;
            while at < f.len() {
                let n = 1 + rnd() as usize % 700;
                let chunk = &f[at..(at + n).min(f.len())];
                let took = r.push_slice(chunk, |m| got.push(m));
                at += chunk.len();
                if took < chunk.len() {
                    break;
                }
            }
            assert_eq!((got, r.result()), (want, want_r), "round {round}");
        }
    }

    /// Speed on a real ride (outside the repository; prints only the cost): `cargo test-host --release -- --ignored
    /// parse_speed --nocapture`.
    #[test]
    #[ignore]
    fn parse_speed() {
        let dir = "/Users/tangui/Documents/bryton/rides";
        let Some(f) = std::fs::read_dir(dir).ok().and_then(|d| {
            d.filter_map(|e| e.ok().map(|e| e.path())).find(|p| p.extension().is_some_and(|e| e == "fit"))
        }) else {
            return;
        };
        let data = std::fs::read(f).unwrap();
        for slices in [false, true] {
            let t = std::time::Instant::now();
            let mut n = 0;
            for _ in 0..20 {
                if slices {
                    let mut r = Reader::new();
                    for chunk in data.chunks(8192) {
                        r.push_slice(chunk, |_| n += 1);
                    }
                } else {
                    n += read(&data).0.len();
                }
            }
            let ns = t.elapsed().as_nanos() as f64 / (20.0 * data.len() as f64);
            let how = if slices { "push_slice" } else { "push" };
            std::println!("{} bytes, {} messages, {how}: {ns:.2} ns a byte on the host", data.len(), n / 20);
        }
    }

    #[test]
    fn reads_what_the_writer_wrote() {
        let f = written();
        let (m, r) = read(&f);
        assert_eq!(r, Some(Ok(())));
        let recs: Vec<Record> = m.iter().filter_map(|m| if let Msg::Record(r) = m { Some(*r) } else { None }).collect();
        assert_eq!(recs.len(), 120);
        let want =
            Record { time: T0 + 21, power: Some(220), hr: Some(130), dist_cm: Some(10_500), ..Record::default() };
        assert_eq!(recs[20], want, "no position unless asked");
        assert_eq!(recs[5].power, None);
        assert_eq!(recs[52].power, None, "dropout");
        let Some(Msg::Session(s)) = m.last() else { panic!("no session") };
        assert_eq!((s.start, s.max_power, s.avg_hr), (Some(T0), Some(319), Some(130)));
        assert_eq!(s.timer_ms, Some(121_000));
    }

    /// With the position asked for: the position in semicircles and the altitude, as fit.rs wrote them.
    #[test]
    fn position_on_request() {
        let mut f = Vec::new();
        let (mut w, _) = Writer::start(&mut f, T0).unwrap();
        let s =
            Sample { time: T0 + 1, pos: Some((450_000_000, -60_000_000)), alt_mm: Some(123_400), ..Sample::default() };
        w.record(&mut f, &s).unwrap();
        w.record(&mut f, &Sample { time: T0 + 2, ..Sample::default() }).unwrap();
        w.finish(&mut f, T0 + 3).unwrap();
        let mut r = Reader::with_position();
        let mut recs = Vec::new();
        r.push_slice(&f, |m| recs.extend(if let Msg::Record(x) = m { Some(x) } else { None }));
        assert_eq!(r.result(), Some(Ok(())));
        // 45 degrees: 2^29 semicircles; -6 degrees: -2^31 / 30
        assert_eq!(recs[0].pos, Some((1 << 29, -(1i64 << 31) as i32 / 30)));
        assert_eq!(recs[0].alt_dm, Some(1234));
        assert_eq!((recs[1].pos, recs[1].alt_dm), (None, None), "not in that record");
    }

    #[test]
    fn checks_the_crcs() {
        let mut f = written();
        let n = f.len();
        f[n - 1] ^= 0x40;
        assert_eq!(read(&f).1, Some(Err(Error::Crc)));
        let mut f = written();
        f[n / 2] ^= 0x01; // a value bit: only the CRC sees it
        assert!(read(&f).1.is_some_and(|r| r.is_err()));
        let mut f = written();
        f[12] ^= 1;
        assert_eq!(read(&f).1, Some(Err(Error::Header)));
        let f = written();
        assert_eq!(read(&f[..f.len() - 1]).1, None, "cut: never finished");
        assert_eq!(read(b"not a fit file").1, Some(Err(Error::Header)));
    }

    /// A file built by hand: a 12-byte header, a big-endian definition with a developer field and an unknown
    /// field, compressed timestamps rolling over, an unknown message, a data message before its definition.
    #[test]
    fn compressed_timestamps_big_endian_and_unknown_messages() {
        let mut d = Vec::new();
        // unknown message 999 (local 1), defined and used: skipped
        d.extend([0x41, 0, 0, 0xe7, 0x03, 1, 0, 2, 0x84]);
        d.extend([0x01, 0xaa, 0xbb]);
        // record, big endian, local 0: timestamp, power, an unknown field 99 (3 bytes), developer field of 4 bytes
        d.extend([0x60, 0, 1, 0, 20, 3, 253, 4, 0x86, 7, 2, 0x84, 99, 3, 0x0d, 1, 0, 4, 0]);
        let t: u32 = 0x3fe; // low 5 bits 0x1e
        d.push(0x00);
        d.extend(t.to_be_bytes());
        d.extend(250u16.to_be_bytes());
        d.extend([9, 9, 9, 1, 2, 3, 4]);
        // compressed headers, local 0: offset 0x1f (t + 1), then 0x01 (rolled over: t + 3)
        for off in [0x1fu8, 0x01] {
            d.push(0x80 | off);
            d.extend(t.to_be_bytes()); // a stale timestamp field: the compressed header's time wins
            d.extend(0xffffu16.to_be_bytes()); // invalid power
            d.extend([0; 7]);
        }
        let mut f = std::vec![12u8, 0x10, 0, 0];
        f.extend((d.len() as u32).to_le_bytes());
        f.extend(b".FIT");
        f.extend(&d);
        let c = crc(0, &f);
        f.extend(c.to_le_bytes());
        let (m, r) = read(&f);
        assert_eq!(r, Some(Ok(())));
        let times: Vec<(u32, Option<u16>)> =
            m.iter().filter_map(|m| if let Msg::Record(r) = m { Some((r.time, r.power)) } else { None }).collect();
        assert_eq!(times, [(t, Some(250)), (t + 1, None), (t + 3, None)]);
        // a data message on local 5, never defined
        let mut bad = f.clone();
        bad[12] = 0x05;
        assert_eq!(read(&bad).1, Some(Err(Error::Undefined)));
    }
}
