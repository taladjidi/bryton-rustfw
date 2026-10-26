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

/// A fix older than this is dropped (the receiver stopped answering).
pub const PVT_STALE_MS: u32 = 3000;

/// GNSS bytes read per pass at most: 128 per 50 ms is 2560 bytes/s, twenty times NAV-PVT at 1 Hz. A pass stops at
/// the first 32 bytes of idle fill between frames, so most passes read 32.
pub(crate) const GNSS_READ: usize = 128;

impl App {
    /// Reads what the GNSS receiver has (sending its configuration once): the latest fix, its state in the strip,
    /// and the speed when no speed sensor is paired.
    pub(crate) fn poll_gnss(&mut self, b: &mut impl Board) {
        if self.gnss_switch(b) {
            return;
        }
        if self.gnss_ms < GNSS_ON_MS && self.gnss_ms.saturating_add(self.dt_ms) >= GNSS_ON_MS {
            b.gnss_power(true);
        }
        self.gnss_ms = self.gnss_ms.saturating_add(self.dt_ms);
        self.pvt_ms = self.pvt_ms.saturating_add(self.dt_ms);
        let mut send = if self.gnss_ms >= GNSS_CONFIG_MS { self.next_gnss_message(b) } else { None };
        for _ in 0..GNSS_READ / 32 {
            let mut buf = send.take().unwrap_or([0xff; 32]);
            b.gnss_transfer(&mut buf);
            if buf == [0xff; 32] && self.gnss.idle() {
                break; // nothing waiting in the receiver
            }
            for &byte in &buf {
                if let Some(pvt) = self.gnss.push(byte).and_then(|(c, i, p)| gnss::pvt(c, i, p)) {
                    self.pvt = Some(pvt);
                    self.pvt_ms = 0;
                    self.learn_aid(b, pvt);
                    // a speed sensor or a trainer heard lately gives the speed (indoors a fix through a window says
                    // 0 km/h and would stop the ride); a paired one that is silent (a dead battery, a magnet knocked
                    // off) must not leave the ride without one: then the GPS's
                    let sensor = [Kind::Speed, Kind::SpeedCadence, Kind::Trainer]
                        .iter()
                        .any(|&k| self.heard_ms.get(k as usize).is_some_and(|&ms| ms < sensors::STALE_MS));
                    if pvt.good() {
                        self.track.add(pvt.lat, pvt.lon, pvt.heading);
                    }
                    if pvt.good() && !sensor {
                        // mm/s to km/h x 10
                        self.live.set(Metric::Speed, (pvt.speed_mm_s * 36 / 1000).min(u32::from(u16::MAX)) as u16);
                    }
                }
            }
        }
        if self.pvt_ms > PVT_STALE_MS {
            self.pvt = None;
        }
        self.strip.gps = self.pvt.map(|p| if p.good() { gnss::bars(p.sats) } else { 0 });
        // blinks while searching; steady otherwise, so the strip is not redrawn every second for nothing
        self.strip.blink = self.strip.gps != Some(0) || self.seconds.is_multiple_of(2);
    }

    /// Switches the receiver off for an indoor ride (a ride running with the INDOOR profile: its speed is the
    /// trainer's, no track is kept) and back on for anything else, saving about 7 to 8 mA. Returns true while off.
    /// Back on is a cold start of the sequence above: the line has been low for over a second by then
    /// ([`GNSS_ON_MS`]), the configuration, the time and the last position are sent again. The supply is cut rather
    /// than the receiver's backup mode (UBX-RXM-PMREQ): its wake source (EXTINT or SPI CS, u-blox M10 interface
    /// description) and the V_BCKP wiring are not known here (BOARD.md 13), and a receiver that does not wake would
    /// stay dead until the next boot. Cost: no ephemeris kept, a warm start after the ride, helped by the aiding.
    fn gnss_switch(&mut self, b: &mut impl Board) -> bool {
        let want_off = self.ride.state() != ride::State::Idle && self.profile == ride::Profile::Indoor;
        if want_off && !self.gnss_off {
            b.gnss_power(false);
            self.gnss_off = true;
            self.pvt = None;
            self.gnss = gnss::Parser::new();
            self.strip.gps = None; // off: not searching
            self.strip.blink = true;
        } else if !want_off && self.gnss_off {
            self.gnss_off = false;
            self.gnss_ms = 0; // powered again at GNSS_ON_MS, then configured and aided as at boot
            self.gnss_sent = 0;
        }
        self.gnss_off
    }

    /// The next message for the receiver after power-up, one per pass: the configuration, the time (UTC from the
    /// RTC and its learnt offset, or the RTC as is with a 14 h accuracy) and the last position when known. Then
    /// None.
    pub(crate) fn next_gnss_message(&mut self, b: &mut impl Board) -> Option<[u8; 32]> {
        loop {
            self.gnss_sent = self.gnss_sent.saturating_add(1);
            match self.gnss_sent {
                1 => return Some(gnss::config()),
                2 => {
                    // the RTC holds local time: with the offset learnt at a fix, UTC within a couple of seconds;
                    // without it, the RTC as is, within 14 h (any time zone): the date still helps
                    let (offset, acc_s) = self.aid.offset_15min.map_or((0, 14 * 3600), |o| (o, 2));
                    let (tr, dr) = b.rtc();
                    let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), rtc_date_from_dr(dr));
                    let utc = epoch_s((y, mo, d, h, m, s)) - i64::from(offset) * 900;
                    return Some(gnss::mga_time(from_epoch_s(utc), acc_s));
                }
                3 => {
                    let Some((lat, lon)) = self.aid.pos else { continue };
                    return Some(gnss::mga_pos(lat, lon, 10_000)); // where it was: within 10 km, most rides
                }
                _ => return None,
            }
        }
    }

    /// Keeps what the next power-up needs from a fix: the RTC's offset from UTC (once the receiver's time is
    /// valid), the position (a good fix); written to the backup registers when it changes.
    pub(crate) fn learn_aid(&mut self, b: &mut impl Board, pvt: gnss::Pvt) {
        let mut aid = self.aid;
        if pvt.time_ok {
            let (tr, dr) = b.rtc();
            let ((h, m, s), (y, mo, d)) = (rtc_time_from_tr(tr), rtc_date_from_dr(dr));
            let diff = epoch_s((y, mo, d, h, m, s)) - epoch_s(pvt.utc);
            let steps = (diff + diff.signum() * 450) / 900; // nearest 15 minutes
            if steps.abs() <= 14 * 4 {
                aid.offset_15min = Some(steps as i8);
            }
        }
        if pvt.good() {
            // to the 1e-4 degrees kept: moving within 11 m does not rewrite the registers each second
            aid.pos = Some((pvt.lat / 1000 * 1000, pvt.lon / 1000 * 1000));
        }
        if aid != self.aid {
            self.aid = aid;
            for (r, w) in AID_BKP.into_iter().zip(aid.to_words()) {
                b.backup_write(r, w);
            }
        }
    }
}
