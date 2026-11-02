//! The simulated GNSS receiver (u-blox M10, ROM SPG 5.10) over SPI4, strict like `chip_table.rs` is for the Nordic
//! chip: a rule table with its sources, the answers the real receiver gives (UBX-ACK-ACK / NAK, UBX-MGA-ACK), and a
//! loud failure with its reason when the firmware does what the real receiver would refuse, drop or stall on. A test
//! that provokes a rule on purpose says so (`expect`), as `Nordic::expect_hang` does.
//!
//! Sources, by rule (the interface description is UBX-21035062 R02, u-blox M10 SPG 5.10; "verified" = read in its
//! text, "inferred" = a guess marked as such; BOARD.md 13 lists them with the unit checks that would settle them):
//! - G1 output buffer 4 kB, then new messages are dropped: MAX-M10S integration manual UBX-20053088 R01, 2.2.2.2
//!   ("buffer can store up to 4 kB") and 2.2.1 (new messages dropped once it is exceeded); for SPI, NEO-M9N integration
//!   manual UBX-19014286 R10, 3.8.5.1 (4 kB of undelivered bytes per port). Checked 2026-11-02.
//! - G2 about 1.5 s without a read stops the stream, which restarts at the next access: interface description 4.9.22
//!   CFG-SPI-EXTENDEDTIMEOUT "disable timeouting the interface after 1.5s", and NEO-M9N manual 3.8.5.1 (SPI and I2C):
//!   "approximately 2 seconds" without host traffic and no more packets are scheduled; MAX-M10S 2.2.2.2 says 1.5 s and
//!   2.2.3.6.1 about 2 s. The model keeps 1.5 s, the safe side of both, as a parameter ([`Gnss::timeout_ms`]).
//! - G3 CFG-SPI-MAXFF (default 50, 0: off): "bytes containing 0xFF to receive before switching off reception", 4.9.22
//!   (verified; range 0 to 63); consecutive: NEO-M9N manual 3.7.3.1 "50 subsequent bytes containing 0xFF", reception
//!   re-enabled by the first other byte. That they are counted from the chip select is **inferred** (searched: no
//!   text on CS): here per transfer.
//! - G4 a frame with a bad checksum or an absurd length is dropped without an answer (**inferred**; searched 3.4 to 3.5: the checksum is defined, the reaction to a bad one is not).
//! - G5 every CFG message gets ACK or NAK; CFG-VALSET is refused for an unknown key, no layer, an invalid
//!   configuration (checked for the RAM layer), more than 64 pairs: 3.9.1, 3.9.2, 3.10.5.1 (verified).
//! - G6 at most three concurrent constellations: integration manual 1 and CFG-SIGNAL, 3.10.x "simultaneous"
//!   (MAX-M10S manual 1.1: concurrent reception "limited to three"; UBX-MON-GNSS `simultaneous`; whether VALSET NAKs a
//!   fourth is **inferred**: not stated).
//! - G7 UBX-MGA-ACK with CFG-NAVSPG-ACKAIDING on, its types and info codes: 3.13.1 (verified); ANO is 76 bytes
//!   (3.13.2, verified); that ANO needs the time first is the info code 1 text.
//! - G8 time aiding off by more than its stated accuracy degrades the receiver: 3.13 INI-TIME_UTC text (verified);
//!   here `degraded` is set (not a violation: tests that set the RTC move the world's UTC under the receiver);
//!   the boot tests assert it stays false. No time-to-first-fix model.
//! - G10 the power line low for over a second before power-on: BOARD.md 13 (seen on the unit).
//!
//! Not modelled: G9 PSMCT behaviour, G11 RXM-PMREQ, accuracy and time to first fix.

extern crate std;

use super::{FIX_LAT, FIX_LON, Route};
use std::vec;
use std::vec::Vec;

/// The output buffer, bytes (G1).
const OUT_MAX: usize = 4096;
/// The stream time-out, ms (G2).
pub const STREAM_TIMEOUT_MS: u64 = 1500;
/// The power line must have been low this long before power-on, ms (G10).
pub const POWER_LOW_MS: u64 = 1000;
/// The largest input frame accepted (G4, inferred).
const IN_MAX: usize = 1024;

/// The configuration items the model knows: key, name, where it is described (interface description section).
/// A key not here is refused like an unknown one: add a row, with its source, when the firmware starts to use one.
pub(super) const KEYS: [(u32, &str, &str); 11] = [
    (0x2091_000a, "CFG-MSGOUT-UBX_NAV_PVT_SPI", "4.9 CFG-MSGOUT, inferred"),
    (0x107a_0002, "CFG-SPIOUTPROT-NMEA", "4.9.24"),
    (0x1023_0001, "CFG-ANA-USE_ANA", "4.9.2"),
    (0x20d0_0001, "CFG-PM-OPERATEMODE", "4.9.15"),
    (0x1031_001f, "CFG-SIGNAL-GPS_ENA", "4.9.21 table 42"),
    (0x1031_0021, "CFG-SIGNAL-GAL_ENA", "4.9.21 table 42"),
    (0x1031_0022, "CFG-SIGNAL-BDS_ENA", "4.9.21 table 42"),
    (0x1031_0025, "CFG-SIGNAL-GLO_ENA", "4.9.21 table 42"),
    (0x1011_0025, "CFG-NAVSPG-ACKAIDING", "4.9 CFG-NAVSPG, inferred"),
    (0x2064_0001, "CFG-SPI-MAXFF", "4.9.22 table 43"),
    (0x1064_0005, "CFG-SPI-EXTENDEDTIMEOUT", "4.9.22 table 43"),
];
const SIGNALS: [u32; 4] = [0x1031_001f, 0x1031_0021, 0x1031_0022, 0x1031_0025];
const EXTENDEDTIMEOUT: u32 = 0x1064_0005;

/// The size of a configuration value in bytes, from the key's bits 30..28 (4.9 "Configuration interface").
fn value_size(key: u32) -> usize {
    match key >> 28 & 7 {
        1 | 2 => 1,
        3 => 2,
        4 => 4,
        _ => 8,
    }
}

/// The GNSS receiver as seen over SPI: once powered, a NAV-PVT frame every second (with `fix`: 3D at that speed,
/// else no fix, as indoors on the unit); 0xff when it has nothing. Answers CFG and MGA messages.
#[derive(Default)]
pub struct Gnss {
    pub powered: bool,
    /// Ground speed of a 3D fix in mm/s; None: no fix.
    pub fix: Option<i32>,
    /// Where a fix is, as a function of the simulated time in ms: latitude, longitude (degrees x 1e-7) and heading
    /// (degrees x 1e-5); None: [`FIX_LAT`], [`FIX_LON`], heading north.
    pub route: Option<Route>,
    /// Altitude of a fix in mm as a function of the simulated time in ms; None: 0.
    pub alt: Option<fn(u64) -> i32>,
    /// Every byte received, in order (0xff included).
    pub got: Vec<u8>,
    /// CFG-PM-OPERATEMODE as the last CFG-VALSET set it (0 FULL, 2 PSMCT; 0 at power-up, the receiver's default),
    /// and each change: (simulated ms, mode).
    pub pm_mode: u8,
    pub pm_log: Vec<(u64, u8)>,
    /// Each CFG-SIGNAL key a CFG-VALSET set (CFG-SIGNAL-GPS_ENA ... GLO_ENA), in order: (key, value).
    pub signals: Vec<(u32, u8)>,
    /// Each UBX-MGA-ANO received whole: (simulated ms, svId, gnssId, year since 2000, month, day).
    pub ano: Vec<(u64, u8, u8, u8, u8, u8)>,
    /// Horizontal accuracy of a fix, mm (0: 3 m).
    pub h_acc_mm: u32,
    /// The stream time-out, ms (G2); None: [`STREAM_TIMEOUT_MS`].
    pub timeout_ms: Option<u64>,
    /// The next CFG-VALSET is refused (UBX-ACK-NAK) as if the receiver judged it invalid: a fault to inject, not a
    /// violation.
    pub nak_next: bool,
    /// Every CFG-VALSET is refused while this is set (counted in `naks`).
    pub nak_all: bool,
    /// A violation of the rules below the test provokes on purpose: its reason contains this text ("" for any).
    pub expect: Option<&'static str>,
    /// Every violation so far (the reasons), expected or not.
    pub violations: Vec<&'static str>,
    /// Answers sent: ACK, NAK, MGA-ACK not used.
    pub acks: u32,
    pub naks: u32,
    pub mga_refused: u32,
    /// Messages dropped for a full output buffer (G1), and NAV-PVT epochs lost to the stream time-out (G2).
    pub dropped: u32,
    pub lost_epochs: u32,
    /// The longest gap between two SPI transfers while powered, ms: the stream's time-out (G2) must not be reached.
    pub longest_gap_ms: u64,
    /// After a stream time-out the stream stays stopped until the host sends a frame or the supply is cycled
    /// (**inferred**, G2b: seen on the unit 2026-11-02, a 60 s stall left the receiver silent for about 2 minutes, which
    /// "restarts at the next access" does not explain; a test knob, off by default).
    pub wedge_after_timeout: bool,
    /// The time aiding was wrong by more than its accuracy (G8).
    pub degraded: bool,
    /// The configuration in force (RAM layer; the receiver's defaults at power-up).
    pub signals_on: [bool; 4],
    pub maxff: u8,
    pub extended_timeout: bool,
    pub ack_aiding: bool,
    /// UTC of the next report, seconds since 1970 (set by the board: its RTC minus 2 h).
    pub(super) utc: i64,
    /// When its power line was last driven low (0: from reset, as the board does at boot).
    pub(super) off_since: u64,
    pub(super) next_ms: u64,
    /// The time of the last transfer, and whether a TIME_UTC was accepted since power-up.
    pub(super) last_xfer: u64,
    time_known: bool,
    wedged: bool,
    out: std::collections::VecDeque<u8>,
    /// Answers made during this transfer: they come out from the next one (the receiver takes time to process).
    late: Vec<u8>,
    in_xfer: bool,
    /// The frame being received, from its sync bytes; 0xff bytes in a row in this transfer, and reception off (G3).
    rx: Vec<u8>,
    ff: u8,
    rx_off: bool,
}

impl Gnss {
    /// Asserts the stream was read often enough: the longest gap and the epochs lost to the time-out stay within the
    /// bounds a test measured (plus 10 %). Prints the figures with the test's name (`--nocapture`).
    pub fn assert_reads(&self, max_gap_ms: u64, max_lost: u32) {
        std::eprintln!(
            "gnss reads: longest gap {} ms, lost epochs {} ({:?})",
            self.longest_gap_ms,
            self.lost_epochs,
            std::thread::current().name()
        );
        assert!(self.longest_gap_ms <= max_gap_ms, "longest unread gap {} ms > {max_gap_ms}", self.longest_gap_ms);
        assert!(self.lost_epochs <= max_lost, "{} epochs lost > {max_lost}", self.lost_epochs);
    }

    /// The receiver's defaults (RAM layer): lost with the supply.
    fn defaults(&mut self) {
        (self.pm_mode, self.signals_on) = (0, [true, true, true, false]); // GPS, Galileo, BeiDou B1I (data sheet)
        (self.maxff, self.extended_timeout, self.ack_aiding) = (50, false, false);
        (self.time_known, self.wedged) = (false, false);
    }

    /// The supply on or off at simulated time `now` (G10).
    pub(super) fn power(&mut self, on: bool, now: u64) {
        if on && !self.powered {
            // as on the unit: switched on too soon after its line was driven low, the receiver does not start
            assert!(
                now >= self.off_since + POWER_LOW_MS,
                "GNSS powered {} ms after off (it needs {POWER_LOW_MS}, BOARD.md 13)",
                now - self.off_since
            );
            self.defaults();
            (self.next_ms, self.last_xfer) = (now + 500, now); // its first fix report
        }
        if !on && self.powered {
            self.off_since = now;
            self.defaults(); // its RAM layer lost with the supply
        }
        self.powered = on;
    }

    /// One SPI transfer (one chip select) at `now`: `buf` in, the receiver's bytes out.
    pub(super) fn transfer(&mut self, now: u64, utc: i64, buf: &mut [u8]) {
        self.begin(now, utc);
        for b in buf.iter_mut() {
            *b = self.byte(now, *b);
        }
        self.end(now);
    }

    /// Chip select low: the three steps of [`Gnss::transfer`], apart for the Renode bridge (`gnss_serve.rs`), whose
    /// bus hands the bytes over a word at a time.
    pub(super) fn begin(&mut self, now: u64, utc: i64) {
        self.utc = utc;
        if self.powered {
            self.longest_gap_ms = self.longest_gap_ms.max(now.saturating_sub(self.last_xfer));
        }
        self.tick(now);
        (self.ff, self.rx_off, self.in_xfer) = (0, false, true);
    }

    /// One byte exchanged while chip select is low.
    pub(super) fn byte(&mut self, now: u64, b: u8) -> u8 {
        self.got.push(b);
        if self.powered {
            self.receive(now, b);
        }
        if self.powered { self.out.pop_front().unwrap_or(0xff) } else { 0xff }
    }

    /// Chip select high.
    pub(super) fn end(&mut self, now: u64) {
        self.in_xfer = false;
        self.out.extend(self.late.drain(..));
        self.last_xfer = now;
    }

    /// A rule broken: panics with the reason unless a test expects it (then it is only recorded).
    fn violate(&mut self, why: &'static str) {
        self.violations.push(why);
        match self.expect {
            Some(e) if why.contains(e) => {}
            _ => panic!("the GNSS receiver would refuse or stall on this (expected {:?}): {why}", self.expect),
        }
    }

    /// Queues a message for the host, or drops it when the buffer is full (G1).
    fn send(&mut self, class: u8, id: u8, payload: &[u8]) {
        let mut f = vec![0xb5, 0x62, class, id, payload.len() as u8, (payload.len() >> 8) as u8];
        f.extend(payload);
        let (mut a, mut b) = (0u8, 0u8);
        for &x in &f[2..] {
            a = a.wrapping_add(x);
            b = b.wrapping_add(a);
        }
        f.extend([a, b]);
        if self.out.len() + self.late.len() + f.len() > OUT_MAX {
            self.dropped += 1;
            self.violate("output buffer full (G1)");
        } else if self.in_xfer {
            self.late.extend(f);
        } else {
            self.out.extend(f);
        }
    }

    /// The report due at or before `now`: its own 1 Hz clock, not the reads' (a read every 200 ms would stretch it to
    /// 1.2 s). Not read for [`Gnss::timeout_ms`] (G2): the stream stopped there, the epochs after it are lost.
    fn tick(&mut self, now: u64) {
        if !self.powered || now < self.next_ms {
            return;
        }
        if self.wedged {
            self.next_ms = now + 1000;
            return;
        }
        let cutoff = self.last_xfer + self.timeout_ms.unwrap_or(STREAM_TIMEOUT_MS);
        if self.extended_timeout || now <= cutoff {
            // late by a second or more (powered, or not read for a while): from now
            self.next_ms = if now >= self.next_ms + 1000 { now + 1000 } else { self.next_ms + 1000 };
            return self.pvt(now);
        }
        let (mut buffered, mut lost) = (false, 0);
        while self.next_ms <= now {
            if self.next_ms <= cutoff {
                buffered = true;
            } else {
                lost += 1;
            }
            self.next_ms += 1000;
        }
        self.next_ms = now + 1000; // restarts at this access
        self.lost_epochs += lost;
        self.wedged = lost > 0 && self.wedge_after_timeout;
        if buffered {
            self.pvt(now);
        }
        if lost > 0 {
            self.violate("stream time-out: not read for over 1.5 s, NAV-PVT epochs lost (G2)");
        }
    }

    /// A NAV-PVT report.
    fn pvt(&mut self, now: u64) {
        let mut p = [0u8; 92];
        p[4..12].copy_from_slice(&[0xe5, 0x07, 3, 7, 0, 0, 0, 0xf0]);
        if let Some(speed) = self.fix {
            let (y, mo, d, h, mi, sec) = crate::from_epoch_s(self.utc);
            p[4..6].copy_from_slice(&y.to_le_bytes());
            p[6..12].copy_from_slice(&[mo, d, h, mi, sec, 0x07]); // date and time valid
            (p[20], p[21], p[23]) = (3, 1, 9);
            let (lat, lon, heading) = self.route.map_or((FIX_LAT, FIX_LON, 0), |f| f(now));
            p[24..28].copy_from_slice(&lon.to_le_bytes());
            p[28..32].copy_from_slice(&lat.to_le_bytes());
            p[36..40].copy_from_slice(&self.alt.map_or(0, |f| f(now)).to_le_bytes());
            p[40..44].copy_from_slice(&(if self.h_acc_mm == 0 { 3000 } else { self.h_acc_mm }).to_le_bytes());
            p[60..64].copy_from_slice(&speed.to_le_bytes());
            p[64..68].copy_from_slice(&heading.to_le_bytes());
        }
        self.send(0x01, 0x07, &p);
    }

    /// A byte from the host: reception switched off after MAXFF 0xff (G3), then UBX framing (G4).
    fn receive(&mut self, now: u64, b: u8) {
        if self.rx_off {
            if b != 0xff && self.ff != u8::MAX {
                self.ff = u8::MAX; // said once per transfer
                self.violate("MAXFF: a byte after reception was switched off by the 0xff run (G3)");
            }
            return;
        }
        if b == 0xff {
            self.ff = self.ff.saturating_add(1);
            self.rx_off = self.maxff != 0 && self.ff >= self.maxff;
            if self.rx_off {
                self.rx.clear(); // the partial frame goes with reception (inferred, BOARD.md 13 G3)
            }
        } else {
            self.ff = 0;
        }
        match self.rx.len() {
            0 => {
                if b == 0xb5 {
                    self.rx.push(b);
                }
            }
            1 => {
                if b == 0x62 {
                    self.rx.push(b);
                } else if b != 0xb5 {
                    self.rx.clear();
                }
            }
            _ => {
                self.rx.push(b);
                if self.rx.len() < 6 {
                    return;
                }
                let n = usize::from(self.rx[4]) | usize::from(self.rx[5]) << 8;
                if n > IN_MAX {
                    self.rx.clear();
                    self.violate("frame longer than the input buffer (G4, inferred)");
                } else if self.rx.len() == n + 8 {
                    let f = std::mem::take(&mut self.rx);
                    let (mut a, mut c) = (0u8, 0u8);
                    for &x in &f[2..n + 6] {
                        a = a.wrapping_add(x);
                        c = c.wrapping_add(a);
                    }
                    if (a, c) != (f[n + 6], f[n + 7]) {
                        return self.violate("bad checksum: the frame is dropped without an answer (G4)");
                    }
                    self.message(now, f[2], f[3], &f[6..n + 6]);
                }
            }
        }
    }

    /// A whole, valid frame.
    fn message(&mut self, now: u64, class: u8, id: u8, p: &[u8]) {
        self.wedged = false;
        match (class, id) {
            (0x06, 0x8a) => self.valset(now, p),
            (0x13, 0x20 | 0x40) => self.mga(now, id, p),
            (0x06, _) => {
                self.violate("CFG message the model does not know (answered with a NAK)");
                self.answer(false, class, id);
            }
            _ => self.violate("message class and id not in the model's table"),
        }
    }

    /// UBX-ACK-ACK (`ok`) or UBX-ACK-NAK for a message.
    fn answer(&mut self, ok: bool, class: u8, id: u8) {
        if ok {
            self.acks += 1;
        } else {
            self.naks += 1;
        }
        self.send(0x05, u8::from(ok), &[class, id]);
    }

    /// Refuses a CFG-VALSET (UBX-ACK-NAK), a violation when `why` names one.
    fn refuse(&mut self, why: Option<&'static str>) {
        if let Some(w) = why {
            self.violate(w);
        }
        self.answer(false, 0x06, 0x8a);
    }

    /// UBX-CFG-VALSET (G5, G6): all or nothing.
    fn valset(&mut self, now: u64, p: &[u8]) {
        if std::mem::take(&mut self.nak_next) || self.nak_all {
            return self.refuse(None);
        }
        let (Some(&version), Some(&layers)) = (p.first(), p.get(1)) else {
            return self.refuse(Some("VALSET shorter than its header"));
        };
        if version != 0 {
            return self.refuse(Some("VALSET version other than 0 (the firmware sends none)"));
        }
        if layers & 7 == 0 {
            return self.refuse(Some("VALSET names no layer: NAK (G5)"));
        }
        if layers & 6 != 0 {
            self.violate("VALSET to the BBR or flash layer: this firmware writes the RAM layer only");
        }
        let (mut at, mut items) = (4, Vec::new());
        while at < p.len() {
            let key = p.get(at..at + 4).map(|k| u32::from_le_bytes([k[0], k[1], k[2], k[3]]));
            let Some(key) = key.filter(|&k| KEYS.iter().any(|r| r.0 == k)) else {
                return self.refuse(Some("VALSET with a key the receiver does not know: NAK (G5)"));
            };
            let size = value_size(key);
            let Some(v) = p.get(at + 4..at + 4 + size) else {
                return self.refuse(Some("VALSET item cut short: NAK (G5)"));
            };
            items.push((key, v[0]));
            at += 4 + size;
        }
        if items.len() > 64 {
            return self.refuse(Some("VALSET with more than 64 pairs: NAK (G5)"));
        }
        let mut sig = self.signals_on;
        for &(key, v) in &items {
            if let Some(i) = SIGNALS.iter().position(|&k| k == key) {
                sig[i] = v != 0;
            }
        }
        if layers & 1 != 0 && sig.iter().filter(|&&on| on).count() > 3 {
            return self.refuse(Some("VALSET leaves more than 3 constellations on: NAK (G6, inferred)"));
        }
        self.signals_on = sig;
        for (key, v) in items {
            match key {
                _ if SIGNALS.contains(&key) => self.signals.push((key, v)),
                0x20d0_0001 => {
                    if v != self.pm_mode || self.pm_log.is_empty() {
                        self.pm_log.push((now, v));
                    }
                    self.pm_mode = v;
                }
                0x2064_0001 => self.maxff = v,
                EXTENDEDTIMEOUT => self.extended_timeout = v != 0,
                0x1011_0025 => self.ack_aiding = v != 0,
                _ => {}
            }
        }
        self.answer(true, 0x06, 0x8a);
    }

    /// UBX-MGA-INI-TIME_UTC (0x13 0x40, type 0x10, 24 bytes), INI-POS_LLH (type 0x01, 20 bytes) and MGA-ANO
    /// (0x13 0x20, 76 bytes), each answered with a UBX-MGA-ACK when CFG-NAVSPG-ACKAIDING is on (G7).
    fn mga(&mut self, now: u64, id: u8, p: &[u8]) {
        let mut first = [0u8; 4];
        for (o, x) in first.iter_mut().zip(p) {
            *o = *x;
        }
        let (ty, info) = match (id, p.first().copied(), p.len()) {
            (0x20, _, 76) if !self.time_known => {
                self.violate("MGA-ANO before an accepted TIME_UTC: the receiver would not use it (G7)");
                (0, 1)
            }
            (0x20, _, 76) => {
                self.ano.push((now, p[2], p[3], p[4], p[5], p[6]));
                (1, 0)
            }
            (0x40, Some(0x10), 24) => {
                let t = (u16::from_le_bytes([p[4], p[5]]), p[6], p[7], p[8], p[9], p[10]);
                let acc = i64::from(u16::from_le_bytes([p[16], p[17]]));
                if (crate::epoch_s(t) - self.utc).abs() > acc + 1 {
                    // not a violation: a test that sets the RTC (the phone's time) moves the simulated world's clock
                    // and its offset from UTC under the receiver, which cannot know it; the boot tests assert it
                    self.degraded = true;
                }
                self.time_known = true;
                (1, 0)
            }
            (0x40, Some(0x01), 20) => (1, 0),
            _ => {
                // 3: a known type of the wrong size; 6: a type the receiver does not know (MGA-ACK infoCode)
                let info = match (id, p.first().copied()) {
                    (0x40, Some(0x10 | 0x01)) | (0x20, _) => 3,
                    _ => 6,
                };
                self.violate("MGA message of the wrong length or type: info code 3 or 6 (G7)");
                (0, info)
            }
        };
        if self.ack_aiding {
            self.mga_refused += u32::from(ty == 0);
            let mut a = vec![ty, 0, info, id];
            a.extend(first);
            self.send(0x13, 0x60, &a);
        }
    }
}
