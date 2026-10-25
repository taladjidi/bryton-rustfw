//! ANT+ sensors: the profiles stock opens (one channel each), the decoding of their broadcasts, and the live values
//! the ride pages show. Pure logic: the radio side is in nrf.rs.
//!
//! Channel, device type and period are stock's (`Ant_OpenSensorChannel` 0xd025e7f0, BOARD.md 11); the data layouts
//! are the public ANT+ device profiles, checked against openant's decoders (github.com/Tigge/openant,
//! `devices/bike_speed_cadence.py` and `devices/power_meter.py`, 2026-10-23): speed or cadence alone, event time in
//! bytes 4-5 and count in 6-7; combined, cadence in 0-3 and speed in 4-7; power page 0x10, cadence in byte 3
//! (0xff: none) and instantaneous power in 6-7, all little endian, times in 1/1024 s. Page 0x10's byte 2 is the
//! pedal power: bit 7 set means the meter tells the pedals apart and bits 0-6 are the right pedal's percent of the
//! power (0xff: not used); openant reads it the same way (`right_power = power x (data[2] ^ 0x80) / 100`).
//!
//! Heart rate (openant `devices/heart_rate.py`, the ANT+ heart rate monitor profile): every page carries the last
//! beat's event time in bytes 4-5 (1/1024 s), a beat count in byte 6 and the computed rate in byte 7; page 4 (byte 0,
//! bit 7 is a toggle) also carries the beat before it in bytes 2-3. The RR intervals ([`Beats`]) follow the patch
//! project's P8 (bryton-S510-mods `patches/P8-hrv-recording.c`): one new beat gives one interval; after missed
//! broadcasts, page 4's previous beat still gives the last one, and with two new beats the first one too.
//!
//! **Electronic shifting**, two channels as stock opens them (BOARD.md 11):
//! - [`Kind::Shifting`], the public ANT+ Shifting profile (device type 34, 4 Hz; SRAM AXS, Campagnolo, any system
//!   that implements it). Its pages are ANT+ members' documents; what is decoded here is what an open
//!   implementation and stock agree on: page 1 (shift system status) as github.com/cujomalainey/antplus-arduino
//!   `src/Profiles/Shifting/DataPages/ANTPLUS_ShiftingShiftSystemStatus.cpp` lays it out (byte 1 event count, byte 3
//!   current gear: rear in bits 0-4, front in 5-7; byte 4 the gear counts the same way) and as stock reads it
//!   (`Ant_EssHandleMessage` 0xd02a2a6c: front 7 and rear 31 are "no value", the gears shown one more than sent);
//!   the common battery page 82 (0x52; byte 2 the battery identifier, its upper nibble the component, 0xff for one
//!   battery; byte 7 bits 4-6 the status, 1 new to 5 critical, 7 invalid: the ANT+ common pages, read so by stock's
//!   0xd02a28a8, which shows the worst component's). A 1x system sends no front (7): the rear alone is kept, front 0
//!   (stock's `Ant_EssHandleMessage` keeps the rear when the front is invalid), and GEAR shows only the rear.
//! - [`Kind::Di2`], Shimano's own ANT channel (device type 1, period 8198, on a private network whose key is
//!   Shimano's, copied from the owner's stock image like the ANT+ one: build.rs). No public document: the layout is
//!   stock's decoder (`Ant_Di2_OnAntEvent` 0xd02a2744, `Sensor_Di2ReadData` 0xd0288cc8, `Sensor_Di2UpdateGearRatio`
//!   0xd0288ae0; ghidra recovered_src/sensors/di2_buttons.c): page 0, bytes 2 and 3 the front and rear gear (from 1;
//!   stock takes none while either is 0), byte 4 the battery in percent (inferred from stock's Sensor Details screen,
//!   which shows it as "60%"); page 4 (sent acknowledged, about every 205 ms), byte 1 the right hood button and
//!   byte 2 the left one, upper nibble 1 single press or 2 long press, lower nibble a press counter, so a new press
//!   changes the byte (stock's `Sensor_Di2Poll` 0xd0288d8c acts on a change, as here). Stock (and P12) start from 0,
//!   so the Di2's first page 4 after a boot, which repeats its last press byte, counts as a press; here the first
//!   page 4 after the start or a drop-out is only the baseline (review of 571e613). The button
//!   decoding is the one the patch project's P12 hooks and ran on the unit with the owner's Di2 (bryton-S510-mods
//!   `patches/P12-di2-button-actions.c`, `docs/how-it-works.md` "Di2 hood buttons (P12)", FLASHLOG #19 to #31); its
//!   test's press sequence is replayed by the `di2_presses` test. Double presses are not decoded (stock neither).

use crate::nrf::SensorId;

/// A kind of ANT+ sensor, one ANT channel each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Heart-rate strap.
    Hr,
    /// Wheel speed sensor.
    Speed,
    /// Crank cadence sensor.
    Cadence,
    /// Combined speed and cadence sensor.
    SpeedCadence,
    /// Power meter.
    Power,
    /// Electronic shifting, the public ANT+ Shifting profile (module doc).
    Shifting,
    /// Shimano Di2, its own ANT channel on a private network (module doc).
    Di2,
    /// A smart trainer, ANT+ FE-C (device type 17; fec.rs).
    Trainer,
}

impl Kind {
    /// Every kind, in channel order.
    pub const ALL: [Kind; 8] = [
        Kind::Hr,
        Kind::Speed,
        Kind::Cadence,
        Kind::SpeedCadence,
        Kind::Power,
        Kind::Shifting,
        Kind::Di2,
        Kind::Trainer,
    ];

    /// Stock's ANT channel for it.
    pub fn channel(self) -> u8 {
        self as u8
    }

    /// The kind on ANT channel `ch`.
    pub fn from_channel(ch: u8) -> Option<Kind> {
        Self::ALL.get(usize::from(ch)).copied()
    }

    /// ANT+ device type.
    pub fn device_type(self) -> u8 {
        [0x78, 0x7b, 0x7a, 0x79, 0x0b, 0x22, 0x01, 0x11][self as usize]
    }

    /// Channel period, in 1/32768 s (stock's: cadence at half rate, power at 8 Hz, the shifting ones at 4 Hz).
    pub fn period(self) -> u16 {
        [8070, 8118, 16204, 8086, 4091, 8192, 8198, 8192][self as usize]
    }

    /// The ANT network of its channel: 0 the ANT+ one, 1 Shimano's (stock assigns channel 6 to network 1,
    /// 0xd027d2a0, whose key is the second `Ant_SetNetworkKeys` 0xd027d6d4 sets).
    pub fn network(self) -> u8 {
        u8::from(self == Kind::Di2)
    }

    /// Its name in the sensors menu.
    pub fn name(self) -> &'static str {
        ["HEART RATE", "SPEED", "CADENCE", "SPEED+CAD", "POWER", "SHIFTING", "DI2", "TRAINER"][self as usize]
    }

    /// The RTC backup register that keeps the paired sensor (none of them is used by stock, BOARD.md 8). The two
    /// shifting kinds and the trainer share 14, told apart by their tags ([`Kind::to_word`]): no register is free, so
    /// the trainer is kept there only while no shifting system is (app/trainer.rs `keep_pairing`).
    pub fn backup(self) -> usize {
        [5, 6, 7, 8, 10, 14, 14, 14][self as usize]
    }

    /// Its paired sensor as kept in [`Kind::backup`]: [`SensorId::to_word`] (tag 0xa5), and for the shifting kinds
    /// tags 0x61 and 0x62, whose top nibble is no ride page of the earlier builds that used register 14
    /// ([`crate::PageCfg::from_word`] wants 0xa).
    ///
    /// ```
    /// use bryton_rustfw::{nrf::SensorId, sensors::Kind};
    /// let s = SensorId { device: 4321, trans: 5 };
    /// assert_eq!(Kind::Di2.from_word(Kind::Di2.to_word(s)), Some(s));
    /// assert_eq!(Kind::Shifting.from_word(Kind::Di2.to_word(s)), None, "the other shifting kind's");
    /// assert_eq!(Kind::Hr.from_word(Kind::Hr.to_word(s)), Some(s));
    /// ```
    pub fn to_word(self, id: SensorId) -> u32 {
        (id.to_word() & 0x00ff_ffff) | u32::from(self.tag()) << 24
    }

    /// The sensor [`Kind::to_word`] kept in `w`, if it is this kind's.
    pub fn from_word(self, w: u32) -> Option<SensorId> {
        (w >> 24 == u32::from(self.tag())).then_some(SensorId { device: w as u16, trans: (w >> 16) as u8 })
    }

    fn tag(self) -> u8 {
        match self {
            Kind::Shifting => 0x61,
            Kind::Di2 => 0x62,
            Kind::Trainer => 0x63,
            _ => 0xa5,
        }
    }
}

/// What a ride page can show from the sensors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// Beats per minute.
    Hr,
    /// km/h x 10.
    Speed,
    /// Revolutions per minute.
    Cadence,
    /// Watts.
    Power,
    /// The right pedal's share of the power, percent (a dual-sided power meter).
    Balance,
    /// The gear, front in the upper byte and rear in the lower, both from 1 (electronic shifting).
    Gear,
    /// The shifting system's battery: percent (Di2), or [`SHIFT_STATUS`] plus the ANT+ battery status (1 new, 2
    /// good, 3 ok, 4 low, 5 critical) of its worst component.
    ShiftBattery,
}

/// [`Metric::ShiftBattery`] holds an ANT+ battery status from here up, a percent under it.
pub const SHIFT_STATUS: u16 = 0x100;

/// A shifter button press ([`Decoders::press`]); its number is the index of its action in
/// [`crate::Settings::remote`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// Left button, single press.
    LeftShort = 0,
    /// Right button, single press.
    RightShort = 1,
    /// Left button, long press.
    LeftLong = 2,
    /// Right button, long press.
    RightLong = 3,
}

/// A value older than this is dropped (the sensor stopped or went out of range).
pub const STALE_MS: u32 = 5000;

/// The latest value of each [`Metric`], and its age.
#[derive(Default)]
pub struct Live {
    values: [Option<u16>; 8],
    age_ms: [u32; 8],
    /// The values as of the last [`Live::snapshot`]: what the pages show.
    shown: [Option<u16>; 8],
}

impl Live {
    /// Sets `m` to `v`, fresh.
    pub fn set(&mut self, m: Metric, v: u16) {
        self.values[m as usize] = Some(v);
        self.age_ms[m as usize] = 0;
    }

    /// The current value of `m`, if fresh enough.
    pub fn get(&self, m: Metric) -> Option<u16> {
        self.values[m as usize]
    }

    /// The value of `m` as of the last [`Live::snapshot`]: what a page shows. The sensors broadcast at 4 Hz and the
    /// digits changed at every one (UI review 2: unreadable, and a redraw each time); the page takes one reading a
    /// second, in the second's tick.
    pub fn shown(&self, m: Metric) -> Option<u16> {
        self.shown[m as usize]
    }

    /// Takes the values now as the ones shown, once a second.
    pub fn snapshot(&mut self) {
        self.shown = self.values;
    }

    /// Ages the values by `dt_ms`, dropping the stale ones.
    pub fn tick(&mut self, dt_ms: u32) {
        for (v, age) in self.values.iter_mut().zip(&mut self.age_ms) {
            *age = age.saturating_add(dt_ms);
            if *age > STALE_MS {
                *v = None;
            }
        }
    }
}

/// A revolution counter as ANT+ sends it (the time of the last event in 1/1024 s and a cumulative count, both
/// wrapping at 16 bits), turned into a rate.
#[derive(Default, Clone, Copy)]
pub struct Revs {
    last: Option<(u16, u16)>,
    idle: u8,
}

impl Revs {
    /// Broadcasts with no new event before the rate reads 0 (about 3 s at 4 Hz).
    const IDLE: u8 = 12;

    /// Takes one broadcast; the rate in revolutions per minute x 100, or None until there is one.
    ///
    /// ```
    /// let mut r = bryton_rustfw::sensors::Revs::default();
    /// assert_eq!(r.update(1000, 7), None);
    /// assert_eq!(r.update(2024, 8), Some(6000), "one turn in 1 s: 60 rpm");
    /// assert_eq!(r.update(2024, 8), None, "no new event yet");
    /// ```
    pub fn update(&mut self, time: u16, count: u16) -> Option<u32> {
        let (t0, n0) = self.last.replace((time, count))?;
        let (dt, dn) = (time.wrapping_sub(t0), count.wrapping_sub(n0));
        if dt == 0 {
            self.idle = self.idle.saturating_add(1);
            return (self.idle == Self::IDLE).then_some(0);
        }
        self.idle = 0;
        // in u64: dn x 6 144 000 passes u32 above 699 revolutions (a counter that jumped: a battery change, a
        // sensor back after a drop-out); a rate past MAX_RPM_X100 is such a jump, not a reading
        let r = u64::from(dn) * 1024 * 60 * 100 / u64::from(dt);
        u32::try_from(r).ok().filter(|&r| r <= Self::MAX_RPM_X100)
    }

    /// Faster than this (rpm x 100) is a counter that jumped: a wheel at 2000 rpm is 250 km/h.
    pub const MAX_RPM_X100: u32 = 200_000;
}

/// RR intervals from a heart-rate strap's beat time and count.
#[derive(Default)]
pub struct Beats {
    /// The last broadcast's beat time and count.
    last: Option<(u16, u8)>,
    /// The intervals from the last broadcast, in ms (at most two).
    rr: [u16; 2],
    n: usize,
}

impl Beats {
    /// Intervals outside this, in ms, are glitches (30 to 240 bpm).
    pub const RANGE: core::ops::RangeInclusive<u16> = 250..=2000;

    /// Takes one broadcast's 8 bytes; the intervals it adds are then in [`Beats::rr`].
    ///
    /// ```
    /// let mut b = bryton_rustfw::sensors::Beats::default();
    /// b.update([0, 0, 0, 0, 0x00, 0x04, 7, 60]);
    /// b.update([0, 0, 0, 0, 0x00, 0x08, 8, 60]);
    /// assert_eq!(b.rr(), &[1000], "one beat 1024/1024 s later");
    /// ```
    pub fn update(&mut self, d: [u8; 8]) {
        self.n = 0;
        let (t1, c1) = (u16::from_le_bytes([d[4], d[5]]), d[6]);
        let Some((t0, c0)) = self.last.replace((t1, c1)) else { return };
        let span = t1.wrapping_sub(t0);
        match c1.wrapping_sub(c0) {
            1 => self.push(span),
            beats @ 2.. if d[0] & 0x7f == 4 => {
                // page 4: the beat before the last; fresh only if it falls after the last one we saw
                let p = u16::from_le_bytes([d[2], d[3]]);
                let first = p.wrapping_sub(t0);
                if first > 0 && first < span {
                    if beats == 2 {
                        self.push(first); // 3 and more: that first span holds several beats
                    }
                    self.push(t1.wrapping_sub(p));
                }
            }
            _ => {}
        }
    }

    /// An interval in 1/1024 s, kept in ms when in [`Beats::RANGE`].
    fn push(&mut self, t: u16) {
        let ms = ((u32::from(t) * 1000 + 512) >> 10) as u16;
        if let (true, Some(slot)) = (Self::RANGE.contains(&ms), self.rr.get_mut(self.n)) {
            *slot = ms;
            self.n += 1;
        }
    }

    /// The intervals the last broadcast added, oldest first, in ms.
    pub fn rr(&self) -> &[u16] {
        self.rr.get(..self.n).unwrap_or(&[])
    }
}

/// Decoding state across broadcasts.
#[derive(Default)]
pub struct Decoders {
    /// The heart-rate strap's beats.
    pub beats: Beats,
    speed: Revs,
    cadence: Revs,
    sc_speed: Revs,
    sc_cadence: Revs,
    /// The Di2 button bytes last seen (page 4: right, left); None until the first page 4 after the start or a
    /// drop-out, which is the baseline: the Di2 keeps resending its last press byte, so taking 0 for the bytes
    /// before it would act on an old press at every boot (review of 571e613).
    buttons: Option<[u8; 2]>,
    /// The ANT+ shifting system's battery status per component (0: not heard).
    battery: [u8; 16],
    /// A shifter button pressed, per side (right, left), for the app to take: the newest of each side since it last
    /// took them, so both buttons changing in one page, or a press on each side in one pass, are both kept.
    pub press: [Option<Press>; 2],
}

/// Wheel revolutions per minute x 100 to km/h x 10, for a wheel of `wheel_mm` circumference.
///
/// ```
/// assert_eq!(bryton_rustfw::sensors::speed_x10(20_000, 2105), 252, "200 rpm on a 700x25 wheel: 25.26 km/h");
/// ```
pub fn speed_x10(rpm100: u32, wheel_mm: u16) -> u16 {
    (u64::from(rpm100) * u64::from(wheel_mm) * 6 / 1_000_000).min(u64::from(u16::MAX)) as u16
}

impl Decoders {
    /// `kind`'s sensor was silent (stale): its next broadcast starts a new series (its counters may have moved on
    /// by any amount meanwhile).
    pub fn restart(&mut self, kind: Kind) {
        match kind {
            Kind::Hr => self.beats = Beats::default(),
            Kind::Speed => self.speed = Revs::default(),
            Kind::Cadence => self.cadence = Revs::default(),
            Kind::SpeedCadence => (self.sc_speed, self.sc_cadence) = (Revs::default(), Revs::default()),
            Kind::Shifting => self.battery = [0; 16],
            // the next button page is the baseline (a press missed while silent is not made up later)
            Kind::Di2 => self.buttons = None,
            Kind::Power | Kind::Trainer => {}
        }
    }

    /// Takes the 8 data bytes of a broadcast from `kind` into `live`.
    pub fn decode(&mut self, kind: Kind, d: [u8; 8], wheel_mm: u16, live: &mut Live) {
        let u16_at = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
        let rpm = |r: u32| (r / 100) as u16;
        match kind {
            // computed heart rate in byte 7; 0 means no reading
            Kind::Hr => {
                self.beats.update(d);
                if d[7] > 0 {
                    live.set(Metric::Hr, u16::from(d[7]));
                }
            }
            // every page: event time in bytes 4-5, cumulative count in 6-7
            Kind::Speed => {
                if let Some(r) = self.speed.update(u16_at(4), u16_at(6)) {
                    live.set(Metric::Speed, speed_x10(r, wheel_mm));
                }
            }
            Kind::Cadence => {
                if let Some(r) = self.cadence.update(u16_at(4), u16_at(6)) {
                    live.set(Metric::Cadence, rpm(r));
                }
            }
            // cadence time, cadence count, speed time, wheel count
            Kind::SpeedCadence => {
                if let Some(r) = self.sc_cadence.update(u16_at(0), u16_at(2)) {
                    live.set(Metric::Cadence, rpm(r));
                }
                if let Some(r) = self.sc_speed.update(u16_at(4), u16_at(6)) {
                    live.set(Metric::Speed, speed_x10(r, wheel_mm));
                }
            }
            // ponytail: the standard power-only page (0x10) only, which every power meter sends; torque pages
            // (0x11, 0x12) when a meter that needs them shows up
            Kind::Power if d[0] == 0x10 => {
                live.set(Metric::Power, u16_at(6));
                if d[2] != 0xff && d[2] & 0x80 != 0 && d[2] & 0x7f <= 100 {
                    live.set(Metric::Balance, u16::from(d[2] & 0x7f));
                }
                if d[3] != 0xff {
                    live.set(Metric::Cadence, u16::from(d[3]));
                }
            }
            Kind::Power => {}
            // page 1, shift system status: rear gear in bits 0-4, front in 5-7, from 0 (all ones: no value); a 1x
            // system has no front (7): the rear alone, front 0, as stock keeps it (`Ant_EssHandleMessage`)
            Kind::Shifting if d[0] == 1 => {
                let (front, rear) = (d[3] >> 5, d[3] & 0x1f);
                if rear != 0x1f {
                    let front = if front == 7 { 0 } else { front + 1 };
                    live.set(Metric::Gear, u16::from(front) << 8 | u16::from(rear + 1));
                }
            }
            // common page 82, battery status: the worst component's
            Kind::Shifting if d[0] == 0x52 => {
                let status = d[7] >> 4 & 7;
                if let (1..=5, Some(slot)) = (status, self.battery.get_mut(usize::from(d[2] >> 4))) {
                    *slot = status;
                }
                if let Some(&worst) = self.battery.iter().max().filter(|&&s| s > 0) {
                    live.set(Metric::ShiftBattery, SHIFT_STATUS + u16::from(worst));
                }
            }
            Kind::Shifting => {}
            // Shimano's page 0: front and rear gear from 1 (0: not known yet), the battery in percent
            Kind::Di2 if d[0] == 0 => {
                if d[2] > 0 && d[3] > 0 {
                    live.set(Metric::Gear, u16::from(d[2]) << 8 | u16::from(d[3]));
                }
                if d[4] <= 100 {
                    live.set(Metric::ShiftBattery, u16::from(d[4]));
                }
            }
            // page 4: a button byte that changed is a press (upper nibble 1 single, 2 long); the first one after the
            // start or a drop-out only sets the bytes
            Kind::Di2 if d[0] == 4 => {
                let now = [d[1], d[2]];
                let Some(last) = self.buttons.replace(now) else { return };
                for (side, ((&b, &was), slot)) in now.iter().zip(&last).zip(&mut self.press).enumerate() {
                    if b == was {
                        continue;
                    }
                    *slot = match (b >> 4, side) {
                        (1, 0) => Some(Press::RightShort),
                        (1, _) => Some(Press::LeftShort),
                        (2, 0) => Some(Press::RightLong),
                        (2, _) => Some(Press::LeftLong),
                        _ => *slot,
                    };
                }
            }
            Kind::Di2 => {}
            // FE-C is decoded by the app into the trainer model (fec.rs), which decides what feeds `live`
            Kind::Trainer => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A counter that jumped (a battery change, a sensor back after minutes) gives no reading, not an overflow
    /// or a huge speed.
    #[test]
    fn jumped_counter_is_no_reading() {
        let mut r = Revs::default();
        r.update(0, 0);
        assert_eq!(r.update(1024, 60_000), None, "60000 turns in 1 s");
        assert_eq!(r.update(2048, 60_001), Some(6000), "then readings again");
    }

    #[test]
    fn counters_wrap_and_stop() {
        let mut r = Revs::default();
        r.update(65400, 65535);
        assert_eq!(r.update(376, 0), Some(12000), "one turn in 0.5 s, both counters wrapping");
        for _ in 0..Revs::IDLE - 1 {
            assert_eq!(r.update(376, 0), None);
        }
        assert_eq!(r.update(376, 0), Some(0), "no event for about 3 s: stopped");
    }

    /// One interval per new beat (the time wrapping), two from page 4 after one missed broadcast, none after
    /// more beats without page 4, glitches dropped.
    #[test]
    fn rr_intervals() {
        // page, previous beat time (page 4), beat time, count
        let page = |p: u8, prev: u16, t: u16, n: u8| {
            let ([p0, p1], [t0, t1]) = (prev.to_le_bytes(), t.to_le_bytes());
            [p, 0, p0, p1, t0, t1, n, 70]
        };
        let mut b = Beats::default();
        let none: &[u16] = &[];
        b.update(page(0x84, 0, 65000, 255));
        assert_eq!(b.rr(), none, "first broadcast: nothing to compare");
        b.update(page(0x84, 65000, 65000, 255));
        assert_eq!(b.rr(), none, "no new beat");
        b.update(page(0x04, 65000, 488, 0));
        assert_eq!(b.rr(), &[1000], "time and count both wrap");
        b.update(page(0x84, 1300, 2124, 2));
        assert_eq!(b.rr(), &[793, 805], "one missed: page 4 gives both");
        b.update(page(0x00, 0, 4000, 4));
        assert_eq!(b.rr(), none, "two beats, no previous time: none");
        b.update(page(0x04, 4000, 5000, 6));
        assert_eq!(b.rr(), none, "two beats, page 4's previous time not after the last seen: stale, none");
        b.update(page(0x04, 5800, 6800, 9));
        assert_eq!(b.rr(), &[977], "three beats: only the last, from page 4");
        b.update(page(0x04, 6800, 6900, 10));
        assert_eq!(b.rr(), none, "98 ms: a glitch");
        b.update(page(0x04, 6900, 9200, 11));
        assert_eq!(b.rr(), none, "2246 ms: out of range");
    }

    /// The Di2 button page, with the press sequence of the patch project's test of the same decoding on stock
    /// (bryton-S510-mods tests/test_tools.py `test_di2_presses_run_their_action_once`): a new byte is one press,
    /// the same byte again none, another kind (3) none, each side on its own.
    #[test]
    fn di2_presses() {
        let (mut dec, mut live) = (Decoders::default(), Live::default());
        let mut press = |right: u8, left: u8| {
            dec.decode(Kind::Di2, [4, right, left, 0, 0, 0, 0, 0], 2105, &mut live);
            let [r, l] = core::mem::take(&mut dec.press);
            r.or(l)
        };
        assert_eq!(press(0, 0), None, "the first page: the baseline");
        assert_eq!(press(0x11, 0), Some(Press::RightShort));
        assert_eq!(press(0x11, 0), None, "repeated: the same press");
        assert_eq!(press(0x22, 0), Some(Press::RightLong));
        assert_eq!(press(0x22, 0x13), Some(Press::LeftShort));
        assert_eq!(press(0x22, 0x24), Some(Press::LeftLong));
        assert_eq!(press(0x35, 0x24), None, "unknown kind");
        assert_eq!(press(0x17, 0x24), Some(Press::RightShort));
    }

    /// The Di2 resends its last press byte: the first page 4 after the start, or after a drop-out
    /// ([`Decoders::restart`]), is not a press (it paused or resumed the ride by itself at every boot: review of
    /// 571e613); a change after it is.
    #[test]
    fn di2_first_button_page_is_the_baseline() {
        let (mut dec, mut live) = (Decoders::default(), Live::default());
        dec.decode(Kind::Di2, [4, 0x13, 0x24, 0, 0, 0, 0, 0], 2105, &mut live);
        assert_eq!(dec.press, [None, None], "at the start");
        dec.decode(Kind::Di2, [4, 0x14, 0x24, 0, 0, 0, 0, 0], 2105, &mut live);
        assert_eq!(dec.press, [Some(Press::RightShort), None]);
        dec.press = [None; 2];
        dec.restart(Kind::Di2);
        dec.decode(Kind::Di2, [4, 0x15, 0x25, 0, 0, 0, 0, 0], 2105, &mut live);
        assert_eq!(dec.press, [None, None], "after a drop-out");
    }

    /// Both buttons changing in one page are two presses, each kept until taken (review of 571e613: the second
    /// replaced the first).
    #[test]
    fn di2_both_buttons_in_one_page() {
        let (mut dec, mut live) = (Decoders::default(), Live::default());
        dec.decode(Kind::Di2, [4, 0, 0, 0, 0, 0, 0, 0], 2105, &mut live);
        dec.decode(Kind::Di2, [4, 0x21, 0x11, 0, 0, 0, 0, 0], 2105, &mut live);
        assert_eq!(dec.press, [Some(Press::RightLong), Some(Press::LeftShort)]);
    }

    /// A 1x ANT+ Shifting system (no front, 7): the rear alone, front 0 (review of 571e613: never a gear).
    #[test]
    fn shifting_one_by() {
        let (mut dec, mut live) = (Decoders::default(), Live::default());
        dec.decode(Kind::Shifting, [1, 0, 0xff, 7 << 5 | 10, 12, 0, 0, 0], 2105, &mut live);
        assert_eq!(live.get(Metric::Gear), Some(11));
    }

    /// The shifting systems' gear and battery (sensors.rs module doc): ANT+ page 1 from 0 with all ones for none,
    /// the common battery page's worst component; Shimano's page 0 from 1, nothing while a gear is 0.
    #[test]
    fn shifting_pages() {
        let (mut dec, mut live) = (Decoders::default(), Live::default());
        dec.decode(Kind::Shifting, [1, 0, 0xff, 0xff, 2 << 5 | 12, 0, 0, 0], 2105, &mut live);
        assert_eq!(live.get(Metric::Gear), None, "no gear yet");
        dec.decode(Kind::Shifting, [1, 1, 0xff, 1 << 5 | 3, 2 << 5 | 12, 0, 0, 0], 2105, &mut live);
        assert_eq!(live.get(Metric::Gear), Some(0x0204), "2-4");
        dec.decode(Kind::Shifting, [0x52, 0xff, 0x11, 0, 0, 0, 0, 2 << 4], 2105, &mut live);
        dec.decode(Kind::Shifting, [0x52, 0xff, 0x21, 0, 0, 0, 0, 4 << 4], 2105, &mut live);
        dec.decode(Kind::Shifting, [0x52, 0xff, 0x31, 0, 0, 0, 0, 7 << 4], 2105, &mut live);
        assert_eq!(live.get(Metric::ShiftBattery), Some(SHIFT_STATUS + 4), "the low one; 7 is no status");
        let mut live = Live::default();
        dec.decode(Kind::Di2, [0, 0, 0, 11, 255, 0, 0, 0], 2105, &mut live);
        assert_eq!((live.get(Metric::Gear), live.get(Metric::ShiftBattery)), (None, None));
        dec.decode(Kind::Di2, [0, 0, 2, 11, 60, 0, 0, 0], 2105, &mut live);
        assert_eq!((live.get(Metric::Gear), live.get(Metric::ShiftBattery)), (Some(0x020b), Some(60)));
    }

    #[test]
    fn decodes_each_profile() {
        let (mut dec, mut live) = (Decoders::default(), Live::default());
        dec.decode(Kind::Hr, [4, 0, 0, 0, 0, 0, 0, 72], 2105, &mut live);
        assert_eq!(live.get(Metric::Hr), Some(72));
        dec.decode(Kind::SpeedCadence, [0, 0, 0, 0, 0, 0, 0, 0], 2105, &mut live);
        // 1 crank turn and 3 wheel turns in 1 s, 2105 mm wheel: 60 rpm, 22.7 km/h
        dec.decode(Kind::SpeedCadence, [0, 4, 1, 0, 0, 4, 3, 0], 2105, &mut live);
        assert_eq!(live.get(Metric::Cadence), Some(60));
        assert_eq!(live.get(Metric::Speed), Some(227));
        dec.decode(Kind::Power, [0x10, 1, 0xff, 85, 0, 0, 0x2c, 0x01], 2105, &mut live);
        assert_eq!((live.get(Metric::Power), live.get(Metric::Cadence)), (Some(300), Some(85)));
        assert_eq!(live.get(Metric::Balance), None, "0xff: no pedal power");
        dec.decode(Kind::Power, [0x10, 2, 0x80 | 52, 85, 0, 0, 0x2c, 0x01], 2105, &mut live);
        assert_eq!(live.get(Metric::Balance), Some(52), "right pedal 52 %");
        live.tick(STALE_MS + 1);
        assert_eq!(live.get(Metric::Hr), None, "stale");
    }
}
