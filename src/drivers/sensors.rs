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
}

impl Kind {
    /// Every kind, in channel order.
    pub const ALL: [Kind; 5] = [Kind::Hr, Kind::Speed, Kind::Cadence, Kind::SpeedCadence, Kind::Power];

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
        [0x78, 0x7b, 0x7a, 0x79, 0x0b][self as usize]
    }

    /// Channel period, in 1/32768 s (stock's: cadence at half rate, power at 8 Hz).
    pub fn period(self) -> u16 {
        [8070, 8118, 16204, 8086, 4091][self as usize]
    }

    /// Its name in the sensors menu.
    pub fn name(self) -> &'static str {
        ["HEART RATE", "SPEED", "CADENCE", "SPEED+CAD", "POWER"][self as usize]
    }

    /// The RTC backup register that keeps the paired sensor (none of them is used by stock, BOARD.md 8).
    pub fn backup(self) -> usize {
        [5, 6, 7, 8, 10][self as usize]
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
}

/// A value older than this is dropped (the sensor stopped or went out of range).
pub const STALE_MS: u32 = 5000;

/// The latest value of each [`Metric`], and its age.
#[derive(Default)]
pub struct Live {
    values: [Option<u16>; 5],
    age_ms: [u32; 5],
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
        Some(u32::from(dn) * 1024 * 60 * 100 / u32::from(dt))
    }
}

/// Decoding state across broadcasts.
#[derive(Default)]
pub struct Decoders {
    speed: Revs,
    cadence: Revs,
    sc_speed: Revs,
    sc_cadence: Revs,
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
    /// Takes the 8 data bytes of a broadcast from `kind` into `live`.
    pub fn decode(&mut self, kind: Kind, d: [u8; 8], wheel_mm: u16, live: &mut Live) {
        let u16_at = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
        let rpm = |r: u32| (r / 100) as u16;
        match kind {
            // computed heart rate in byte 7; 0 means no reading
            Kind::Hr if d[7] > 0 => live.set(Metric::Hr, u16::from(d[7])),
            Kind::Hr => {}
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
