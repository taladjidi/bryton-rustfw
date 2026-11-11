//! The GNSS receiver: power and configuration at start-up, the aiding sent (time, last position), NAV-PVT read a
//! bounded number of bytes per pass, the fix dropped when stale, and the aiding learned from good fixes.

use crate::*;

/// When the GNSS receiver is powered, counted from the app's first step: its power line (PI6) has then been
/// driven low for over a second (the board does it at boot). After a reset the line is left undriven, and powered
/// again ~100 ms after being driven low, the receiver did not start (seen on the unit, BOARD.md 13).
pub const GNSS_ON_MS: u32 = 1000;

/// When the GNSS receiver gets its configuration, counted from the app's first step (1 s after its power-up;
/// stock waits 3 s before reading it at all).
pub const GNSS_CONFIG_MS: u32 = GNSS_ON_MS + 1000;

/// No ride and no fix this long after the receiver was switched on (or a screen last needed it): off, so a unit
/// left on indoors does not search all day (11.8 mA, docs/power.md). 5 minutes is well past a start outdoors: about
/// half a minute with the time and position aiding and no ephemeris kept (the ephemeris takes 18 to 36 s to download
/// under a strong signal, u-blox SAM-M10Q integration manual UBX-22020019 R01, 2.4), a few minutes at worst under
/// trees or by a window; and costs about 1 mAh indoors.
pub const GNSS_SEARCH_MS: u32 = 5 * 60_000;

/// No ride, a fix, and neither a key, a touch nor a screen that needs a position for this long: off. Long enough
/// that a rider who got the fix and waits to start keeps it.
pub const GNSS_IDLE_MS: u32 = 10 * 60_000;

/// A fix older than this is dropped (the receiver stopped answering).
pub const PVT_STALE_MS: u32 = 3000;

/// What goes to the receiver in a pass: the bytes (padded with 0xff) and how many 32-byte transfers hold them.
pub(crate) type Msg = ([u8; mga::WIRE], usize);

/// A message of one transfer.
fn one(f: [u8; 32]) -> Msg {
    let mut m = [0xff; mga::WIRE];
    m[..32].copy_from_slice(&f);
    (m, 1)
}

/// Passes between two AssistNow Offline messages (one a pass would be 1.7 kB/s; mga.rs has the pace).
pub const ANO_GAP: u8 = 1;

/// GNSS bytes read per pass at most: 128 per 50 ms is 2560 bytes/s, twenty times NAV-PVT at 1 Hz. A pass stops
/// at the first 32 bytes of idle fill
/// between frames, so most passes read 32.
pub(crate) const GNSS_READ: usize = 128;

/// While the receiver has nothing to send (idle fill), it is read again this long after, ms (4 passes of
/// [`POLL_MS`]), not every pass: its output is NAV-PVT once a second (and the answers to the configuration), which
/// then waits at most this long in its buffer; a read that finds bytes keeps reading every pass until the fill comes
/// back. Each read is a 32-byte SPI4 transfer the CPU polls (113 us a pass on the unit, 0.24 % of the time:
/// 2026-10-26). Time, not passes, so the wait does not depend on the pass length.
pub const GNSS_FILL_MS: u32 = 200;

/// After a NAV-PVT read whole (the fill after it), with nothing left to send the receiver (the configuration, the
/// time and position, AssistNow Offline), the next read waits this long, ms: the next PVT is due a second after this
/// one, so one idle read before it instead of four (review of 2026-10-31, finding 10). Then [`GNSS_FILL_MS`] reads
/// until it comes: it lands at most that late, within its second.
pub const GNSS_PVT_WAIT_MS: u32 = 800;

impl App {
    /// Reads what the GNSS receiver has (sending its configuration once): the latest fix, its state in the strip,
    /// and the speed when no speed sensor is paired.
    pub(crate) fn poll_gnss(&mut self, b: &mut impl Board) {
        if self.gnss_switch(b) {
            // off: the start's clock still runs (the files read a few seconds after the start wait on it); the
            // receiver's sequence starts over from 0 when it is switched on
            self.gps.ms = self.gps.ms.saturating_add(self.real_ms);
            self.gps.ok_speed = None; // no spike check against a speed from before the receiver was off
            return;
        }
        // real time: the receiver's power-up is not hurried by a repaid debt (lib.rs `real_ms`)
        if self.gps.ms < GNSS_ON_MS && self.gps.ms.saturating_add(self.real_ms) >= GNSS_ON_MS {
            b.gnss_power(true);
        }
        self.gps.ms = self.gps.ms.saturating_add(self.real_ms);
        self.gps.pvt_ms = self.gps.pvt_ms.saturating_add(self.dt_ms);
        if let Some((_, age)) = self.gps.ok_speed.as_mut() {
            *age = age.saturating_add(self.dt_ms);
        }
        let mut send = if self.gps.ms >= GNSS_CONFIG_MS { self.next_gnss_message(b) } else { None };
        self.gps.wait = self.gps.wait.saturating_sub(self.dt_ms);
        if send.is_some() || self.gps.wait == 0 {
            self.read_gnss(b, send.take());
        }
        if self.gps.pvt_ms > PVT_STALE_MS && self.gps.pvt.take().is_some() {
            self.adapt(None); // silent: continuous tracking, if AUTO had chosen cyclic
        }
        self.strip.gps = self.gps.bars();
        // blinks while searching; steady otherwise, so the strip is not redrawn every second for nothing
        self.strip.blink = self.strip.gps != Some(0) || self.seconds.is_multiple_of(2);
    }

    /// A read now, whatever the wait: before a radio command that is known to be coming and may meet a silent chip
    /// (the probe), so the receiver is not left unread for the wait and the time-out together.
    pub(crate) fn read_gnss_now(&mut self, b: &mut impl Board) {
        if !self.gps.off && self.gps.ms >= GNSS_ON_MS {
            self.read_gnss(b, None);
        }
    }

    /// One read of the receiver: up to [`GNSS_READ`] bytes (`send` goes out with the first 32), the fixes in them
    /// taken. Found idle, the next read waits [`GNSS_FILL_MS`], or [`GNSS_PVT_WAIT_MS`] after a PVT with nothing
    /// left to send.
    fn read_gnss(&mut self, b: &mut impl Board, send: Option<Msg>) {
        self.gps.wait = 0;
        let mut pvt_read = false;
        // the start's messages all sent, and AssistNow Offline's (a change of power mode or systems goes at once,
        // whatever the wait)
        let nothing_to_send = send.is_none() && self.gps.sent >= 5 && self.gps.ano.done();
        let (out, chunks) = send.unwrap_or(([0xff; mga::WIRE], 0));
        for i in 0..GNSS_READ / 32 {
            let mut buf = [0xff; 32];
            if i < chunks {
                buf.copy_from_slice(out.get(32 * i..32 * i + 32).unwrap_or(&[0xff; 32]));
            }
            b.gnss_transfer(&mut buf);
            if i >= chunks && buf == [0xff; 32] && self.gps.parser.idle() {
                self.gps.wait = if pvt_read && nothing_to_send { GNSS_PVT_WAIT_MS } else { GNSS_FILL_MS };
                break; // nothing waiting in the receiver
            }
            for &byte in &buf {
                let (pvt, ack) = match self.gps.parser.push(byte) {
                    Some((c, i, p)) => (gnss::pvt(c, i, p), gnss::ack(c, i, p)),
                    None => (None, None),
                };
                if let Some(a) = ack {
                    self.note_ack(a);
                }
                if let Some(pvt) = pvt {
                    let speed = pvt.speed_x10(self.gps.ok_speed);
                    if speed.is_some() {
                        self.gps.ok_speed = Some((pvt.speed_mm_s, 0));
                    }
                    self.gps.pvt = Some(pvt);
                    self.gps.pvt_ms = 0;
                    pvt_read = true;
                    self.learn_aid(b, pvt);
                    self.adapt(Some(pvt));
                    // a speed sensor or a trainer heard lately gives the speed (indoors a fix through a window says
                    // 0 km/h and would stop the ride); a paired one that is silent (a dead battery, a magnet knocked
                    // off) must not leave the ride without one: then the GPS's
                    let sensor = [Kind::Speed, Kind::SpeedCadence, Kind::Trainer].iter().any(|&k| self.radio.heard(k));
                    if pvt.good() {
                        self.track.add(pvt.lat, pvt.lon, pvt.heading);
                        self.track.orient(self.settings.map_up, pvt.heading, pvt.speed_mm_s);
                        self.gps.fixed = true;
                    }
                    if let (Some(v), false) = (speed, sensor) {
                        self.live.set(Metric::Speed, v);
                    }
                }
            }
            // the PVT read whole and fill after it (a 100-byte PVT ends in the 4th transfer): nothing else waits.
            // Two 0xff, so a checksum byte of 0xff ending a transfer does not add the long wait
            if pvt_read && nothing_to_send && buf.ends_with(&[0xff, 0xff]) && self.gps.parser.idle() {
                self.gps.wait = GNSS_PVT_WAIT_MS;
                break;
            }
        }
    }

    /// The receiver's answer to a message ([`gnss::Ack`]) counted in `gps.answers` (taken, refused, given up, assistance
    /// not used). A refused configuration (UBX-ACK-NAK of CFG-VALSET) is sent once more, then given up: the receiver
    /// keeps its previous settings, which only cost power or the systems chosen, and the ride is not touched. One retry
    /// per change of the receiver's settings (`gps.retried`, cleared by the menus), so a message that is always refused
    /// cannot make a loop. `gps.answers`: [0] taken, [1] CFG-VALSET refused and retried, [2] CFG-VALSET given up,
    /// [3] MGA-ACK refused; the NAKs of other messages are not counted.
    fn note_ack(&mut self, a: gnss::Ack) {
        let n = match a {
            gnss::Ack::Ack(..) => 0,
            gnss::Ack::Nak(0x06, 0x8a) if !self.gps.retried => {
                (self.gps.retried, self.gps.reconfig, self.gps.resys) = (true, true, true); // resys waits for no ride
                1
            }
            gnss::Ack::Nak(0x06, 0x8a) => 2,
            gnss::Ack::Nak(..) => return,
            gnss::Ack::Mga(true, _) => return,
            gnss::Ack::Mga(false, _) => 3,
        };
        if let Some(c) = self.gps.answers.get_mut(n) {
            *c = c.saturating_add(1);
        }
    }

    /// Switches the receiver off for an indoor ride (a ride running with the INDOOR profile: its speed is the
    /// trainer's, no track is kept), between rides while BIKE > PROFILE is fixed to INDOOR, and between rides once
    /// idle ([`App::gnss_idle`]), and back on for anything else, saving about 12 mA. Returns true while off.
    /// Back on is a cold start of the sequence above: the line has been low for over a second by then
    /// ([`GNSS_ON_MS`]), the configuration, the time and the last position are sent again. The supply is cut rather
    /// than the receiver's backup mode (UBX-RXM-PMREQ): its wake source (EXTINT or SPI CS, u-blox M10 interface
    /// description) and the V_BCKP wiring are not known here (BOARD.md 13), and a receiver that does not wake would
    /// stay dead until the next boot. Cost: no ephemeris kept, a warm start after the ride, helped by the aiding.
    fn gnss_switch(&mut self, b: &mut impl Board) -> bool {
        let want_off = if self.ride.state() == ride::State::Idle {
            self.gnss_idle() || self.settings.profile == Some(ride::Profile::Indoor)
        } else {
            (self.gps.quiet_ms, self.gps.slept) = (0, false); // a ride open: on, and the idle time starts at its end
            self.profile == ride::Profile::Indoor
        };
        if want_off && !self.gps.off {
            b.gnss_power(false);
            self.gps.off = true;
            self.gps.pvt = None;
            self.gps.parser = gnss::Parser::new();
            self.strip.gps = None; // off: not searching
            self.strip.blink = true;
        } else if !want_off && self.gps.off {
            self.gps.off = false;
            self.gps.ms = 0; // powered again at GNSS_ON_MS, then configured and aided as at boot
            (self.gps.sent, self.gps.retried) = (0, false);
            self.gps.auto = gnss::Adaptive::default(); // continuous tracking first
            (self.gps.quiet_ms, self.gps.fixed) = (0, false);
        }
        self.gps.off
    }

    /// Between rides: true once the receiver should sleep, [`GNSS_SEARCH_MS`] without a fix since it was switched on,
    /// or [`GNSS_IDLE_MS`] with one and no input; then until a screen needs a position (a page with a map, the GPS
    /// menu, a course followed) or a ride starts. Keys alone do not wake it: indoors they would start the search again.
    fn gnss_idle(&mut self) -> bool {
        let needed = matches!(self.screen, Screen::Menu(Menu::Gps, _))
            || self.screen_has_map()
            || self.courses.as_deref().is_some_and(|c| c.on.is_some());
        if needed {
            (self.gps.quiet_ms, self.gps.slept) = (0, false);
        } else if !self.gps.slept {
            self.gps.quiet_ms = self.gps.quiet_ms.saturating_add(self.dt_ms);
            self.gps.slept = if self.gps.fixed {
                self.gps.quiet_ms >= GNSS_IDLE_MS && self.light.idle_ms >= GNSS_IDLE_MS
            } else {
                self.gps.quiet_ms >= GNSS_SEARCH_MS
            };
        }
        self.gps.slept
    }

    /// A page with a map is shown, worked out once per screen (and MORE page).
    pub(crate) fn screen_has_map(&mut self) -> bool {
        let key = (self.screen, self.more_shown(), self.ride_profile().page_set(), self.ride.state(), self.page_count);
        match self.gps.map_on {
            Some((k, on)) if k == key => on,
            _ => {
                let on = self.first_map().is_some();
                self.gps.map_on = Some((key, on));
                on
            }
        }
    }

    /// The next message for the receiver after power-up, one per pass: the configuration with the GNSS SYSTEMS, the time (UTC from the
    /// RTC and its learnt offset, or the RTC as is with a 14 h accuracy) and the last position when known, then today's
    /// AssistNow Offline messages ([`App::next_ano`]). Then None. A change of GPS POWER, or of the power mode in force ([`App::gnss_saver`]: AUTO's choice, an MTB ride
    /// started or ended), sends the configuration alone again (its power mode), in the RAM layer as at start
    /// (CFG-PM-OPERATEMODE takes effect at once, UBX-22020019 2.6.2.6).
    pub(crate) fn next_gnss_message(&mut self, b: &mut impl Board) -> Option<Msg> {
        let saver = self.gnss_saver();
        let config = gnss::config(saver);
        let changed = core::mem::take(&mut self.gps.reconfig) || saver != self.gps.saver;
        if self.gps.sent == 0 || changed {
            self.gps.saver = saver;
        }
        // a change of GNSS SYSTEMS waits for the end of a ride (the receiver may drop its fix while it changes
        // satellite systems: inferred, see gnss::systems); it goes with a power mode change if both are due
        let resys = self.gps.resys && self.ride.state() == ride::State::Idle;
        if self.gps.sent > 0 && (changed || resys) {
            let sys = gnss::systems(self.settings.gnss_systems);
            self.gps.resys &= !resys;
            if changed && resys {
                let mut m = one(config);
                m.0[32..64].copy_from_slice(&sys);
                return Some((m.0, 2));
            }
            return Some(one(if changed { config } else { sys }));
        }
        loop {
            self.gps.sent = self.gps.sent.saturating_add(1);
            match self.gps.sent {
                1 => {
                    self.gps.resys = false; // sent with the configuration
                    // both in one pass: the start-up takes no longer than before the systems were sent
                    let (mut m, sys) = (one(config), gnss::systems(self.settings.gnss_systems));
                    m.0[32..64].copy_from_slice(&sys);
                    return Some((m.0, 2));
                }
                2 => {
                    // the RTC holds local time: with the offset learnt at a fix, UTC within a couple of seconds;
                    // without it, the RTC as is, within 14 h (any time zone): the date still helps
                    let (offset, acc_s) = self.gps.aid.offset_15min.map_or((0, 14 * 3600), |o| (o, 2));
                    let (tr, dr) = b.rtc();
                    let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), rtc_date_from_dr(dr));
                    let utc = from_epoch_s(epoch_s((y, mo, d, h, m, s)) - i64::from(offset) * 900);
                    // without the learnt offset the RTC's date may be a day off UTC: a day of slack
                    self.gps.ano.start((utc.0, utc.1, utc.2), u8::from(self.gps.aid.offset_15min.is_none()));
                    self.gps.ano_file = None;
                    return Some(one(gnss::mga_time(utc, acc_s)));
                }
                3 => {
                    let Some((lat, lon)) = self.gps.aid.pos else { continue };
                    return Some(one(gnss::mga_pos(lat, lon, 10_000))); // where it was: within 10 km, most rides
                }
                _ => {
                    self.gps.sent = 5; // stays here
                    return self.next_ano(b);
                }
            }
        }
    }

    /// The next AssistNow Offline message of today from `\Rustfw\mga.ubx`, one every [`ANO_GAP`] + 1 passes, at
    /// most four sectors read looking for it. A file that cannot be read (missing, card lent to the computer or not
    /// ready yet, damaged) is given up without a word; the card is not read before the bond is
    /// ([`BOND_LOAD_MS`]), by when it is up.
    fn next_ano(&mut self, b: &mut impl Board) -> Option<Msg> {
        if self.gps.ano_gap > 0 {
            self.gps.ano_gap -= 1;
            return None;
        }
        if self.gps.ano.done() || self.emmc.lent() || self.gps.ms < BOND_LOAD_MS {
            return None;
        }
        for _ in 0..4 {
            if let Some(m) = self.gps.ano.take() {
                self.gps.ano_gap = ANO_GAP;
                return Some((m, 3));
            }
            let (at, buf) = self.gps.ano.want()?;
            let mut d = emmc::Volume(&mut self.emmc, b);
            let file = &mut self.gps.ano_file;
            let n = fat::Fat::mount(&mut d)
                .and_then(|mut fat| {
                    let (f, seek) = match file {
                        Some(fs) => fs,
                        None => file.insert((fat.open(&mut d, mga::FILE)?, fat::Seek::new())),
                    };
                    fat.read_from(&mut d, f, at, buf, seek)
                })
                .unwrap_or(0);
            self.gps.ano.filled(n);
        }
        None
    }

    /// The receiver's power mode now: GPS POWER, or for AUTO its choice.
    pub(crate) fn gnss_saver(&self) -> bool {
        if self.ride.state() != ride::State::Idle && self.profile.tracks() {
            return false; // off road: continuous tracking whatever GPS POWER says (the owner, 2026-10-27)
        }
        // BATTERY SAVER: cyclic tracking (docs/power.md), but not under a running ride: its distance comes from the
        // GPS speed and cyclic tracking can lose fixes, so a ride keeps the rider's GPS POWER (review 2026-10-31)
        if self.saver_on() && self.ride.state() != ride::State::Running {
            return true;
        }
        match self.settings.gps_power {
            GpsPower::Auto => self.gps.auto.saver(),
            GpsPower::Full => false,
            GpsPower::Saver => true,
        }
    }

    /// GPS POWER AUTO takes the second's fix (None: lost); a change of the power mode is sent by
    /// [`App::next_gnss_message`].
    fn adapt(&mut self, pvt: Option<gnss::Pvt>) {
        let riding = self.ride.state() == ride::State::Running;
        self.gps.auto.fix(pvt, riding);
    }

    /// Keeps what the next power-up needs from a fix: the RTC's offset from UTC (once the receiver's time is
    /// valid), the position (a good fix); written to the backup registers when it changes. The offset not while a
    /// ride is open or being saved ([`App::ride_settled`]): the FIT times are the RTC less it, so they would jump (as
    /// `phone_time` refuses the phone's).
    pub(crate) fn learn_aid(&mut self, b: &mut impl Board, pvt: gnss::Pvt) {
        let mut aid = self.gps.aid;
        let settled = pvt.time_ok && self.ride_settled();
        // a clock lost with the backup domain (it restarts in 2000): set to UTC, the offset then learnt as 0, so rides
        // are not named and dated 2000 until a phone comes (the review of 2026-11-11, R3); a clock at the build year
        // or later is never moved
        let lost = settled && rtc_date_from_dr(b.rtc().1).0 < BUILD_YEAR && pvt.utc.0 >= BUILD_YEAR;
        if lost {
            let (tr, dr) = rtc_words(pvt.utc);
            b.rtc_set(tr, dr);
            self.rec.counted_s = rtc_epoch_s(b); // a set, not seconds gone (app/pass.rs)
        }
        if settled {
            let diff = rtc_epoch_s(b) - epoch_s(pvt.utc);
            let steps = (diff + diff.signum() * 450) / 900; // nearest 15 minutes
            if steps.abs() <= 14 * 4 {
                aid.offset_15min = Some(steps as i8);
            }
        }
        if pvt.good() {
            // to the 1e-4 degrees kept: moving within 11 m does not rewrite the registers each second
            aid.pos = Some((pvt.lat / 1000 * 1000, pvt.lon / 1000 * 1000));
        }
        if aid != self.gps.aid {
            self.gps.aid = aid;
            self.write_aid(b);
        }
        if lost {
            self.show_clock(pvt.utc); // with the offset just learnt (a TIME ZONE needs it)
            self.seconds = pvt.utc.5;
        }
    }
}

/// The year the build was made (build.rs `GIT_DATE`; 2026 outside a git checkout, and under test so the simulator's
/// 2026 clock is never "lost"): an RTC earlier than it was reset with the backup domain ([`App::learn_aid`]).
const BUILD_YEAR: u16 = if cfg!(test) { 2026 } else { year_of(env!("GIT_DATE")) };
const _: () = assert!(year_of("2027-03-04") == 2027 && year_of("unknown") == 2026);

/// The year of a `YYYY-MM-DD` date, 2026 when it is not one.
const fn year_of(date: &str) -> u16 {
    match date.as_bytes() {
        [a @ b'0'..=b'9', b @ b'0'..=b'9', c @ b'0'..=b'9', d @ b'0'..=b'9', b'-', ..] => {
            (*a - b'0') as u16 * 1000 + (*b - b'0') as u16 * 100 + (*c - b'0') as u16 * 10 + (*d - b'0') as u16
        }
        _ => 2026,
    }
}
