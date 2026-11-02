//! The simulated sensor bus (I2C4, BOARD.md 16): LTR-308ALS (0x53), LPS22HH (0x5c), LIS2DH12 (0x18), strict like
//! `gnss_model.rs`: a rule broken fails the test with its reason unless the test expects it (`expect`). Sources (the
//! LPS22HH datasheet and AN5209 could not be fetched (st.com stalled, 2026-11-02): its rules rest on ST's driver and
//! docs.rs/lps22hh-rs register text; a rule nothing settles says "inferred"):
//! - S1 LPS22HH with BDU set: the output registers lock once PRESS_OUT_XL / L is read until the high byte is read, so
//!   a read that stops before the high byte (or a pressure read split in two transactions) mixes two conversions.
//!   ST LPS22HH datasheet (https://www.st.com/resource/en/datasheet/lps22hh.pdf) BDU bit ("not updated until MSB and LSB are read", checked in
//!   https://docs.rs/lps22hh-rs/2.0.0/src/lps22hh_rs/register/main.rs.html), and AN5209
//!   (https://www.st.com/resource/en/application_note/an5209-lps22hh-mems-nano-pressure-sensor-stmicroelectronics.pdf);
//!   the lock-start byte is **inferred**. A split read is a violation here, not a lock.
//! - S2 ONE_SHOT (CTRL_REG2 0x11 bit 0) reads 1 while converting and self-clears at the end, STATUS 0x27 P_DA / T_DA
//!   set then (self-clearing: ST community answer on the LPS22HB, same register; docs.rs register text); a one-shot asked
//!   with ODR != 0 is ignored (docs.rs: works "in power-down mode"); asked while converting is ignored (**inferred**:
//!   nothing found). The conversion time is **inferred** (AN5209 not read).
//! - S3 LTR-308 MAIN_STATUS 0x07 bit 3 (new data) is cleared after the data is read (Lite-On DS86-2016-0027 Rev C,
//!   https://optoelectronics.liteon.com/upload/download/DS86-2016-0027/LTR-308ALS-01_Final_%20DS_V2.1.PDF, p.13).
//!   Whether the status read or the data read clears it is not stated (**inferred**: the data read; the firmware
//!   reads both in one block, so either way holds). Registers 0x0D..0x0F are read as a group (pseudo code p.23).
//! - S4 18 bits conversion 100 ms (MEAS_RATE 0x22, p.12). Timing accuracy is +-20 % and wake-up takes up to 10 ms
//!   (3.3), so `light_late_ms` lets the conversion finish late. Data read before it ends is old: a violation unless
//!   the same read holds MAIN_STATUS, where the firmware can see it (ambient.rs `light_read`).
//! - S7 a supply glitch sets MAIN_STATUS bit 5 (power-on) and puts every register back to its default (GAIN 3x, not
//!   the firmware's 6; standby): [`Bus::light_glitch`]. The bit reads 1 after power-up too, cleared by reading it.
//! - S5 a stuck bus (`stuck_sda`: a slave holding SDA low after a reset mid-read): every transfer fails until
//!   [`Bus::recover`] (nine SCL pulses and a STOP, NXP UM10204 3.1.16 "bus clear"; the unit's clocking is
//!   `i2c_recover` in main.rs, never run on the host).
//!
//! Not modelled: the accelerometer (a stub that acknowledges), interrupts (PF9, PD11).

extern crate std;

use std::vec::Vec;

/// The chips on the sensor bus, as their registers behave (BOARD.md 16).
pub struct Bus {
    /// The ambient light, tenths of a lux; None: no light sensor (it never acknowledges).
    pub lux10: Option<u32>,
    /// The light sensor's registers (written ones kept).
    light: [u8; 0x20],
    /// When the light sensor left standby (None: in standby).
    light_on_since: Option<u64>,
    /// The conversion takes this much longer than 100 ms (+-20 % timing and up to 10 ms wake-up, datasheet 3.3).
    pub light_late_ms: u64,
    /// MAIN_STATUS bit 5: set at power-up, and by a supply glitch ([`Bus::light_glitch`]); cleared by reading it.
    light_por: bool,
    /// The conversion (100 ms period from `light_on_since`) whose data was last read (S3).
    light_epoch: Option<u64>,
    /// Time the light sensor has spent measuring, ms (up to its last standby).
    pub light_on_ms: u64,
    /// Light readings taken.
    pub light_reads: u32,
    /// Transactions on the bus (reads and writes, any chip, failed ones too): each one the CPU polls through on the
    /// unit.
    pub transactions: u32,
    /// The barometer's pressure and temperature (hPa x 10, C x 100); None: no barometer.
    pub baro: Option<(u32, i16)>,
    /// The barometer follows this altitude (mm, a function of the time in ms) on the sea-level pressure `baro_p0`
    /// (1/4096 hPa), instead of `baro`'s pressure: a simulated climb, with `baro_noise` counts of 1/4096 hPa of
    /// pseudo-random noise on each reading.
    pub baro_alt: Option<fn(u64) -> i32>,
    /// See `baro_alt`.
    pub baro_p0: u32,
    /// See `baro_alt`.
    pub baro_noise: u32,
    /// When the last one-shot conversion is done (None: none asked since the last read).
    baro_done: Option<u64>,
    /// One-shot conversions asked.
    pub baro_conversions: u32,
    /// CTRL_REG1 of the barometer (ODR bits 6:4, BDU bit 1).
    baro_ctrl1: u8,
    /// The accelerometer's CTRL_REG1 (0x77, running at 400 Hz, as stock leaves it).
    pub accel_ctrl1: u8,
    /// S5: a slave holds SDA low: every transfer fails until [`Bus::recover`].
    pub stuck_sda: bool,
    /// The slave holds SDA low whatever the clear does (a dead chip, a short): [`Bus::recover`] does not free it.
    pub stuck_hard: bool,
    /// The next this many transfers fail (a burst of NACKs).
    pub nack_left: u32,
    /// Transfers that failed so far.
    pub nacks: u32,
    /// Bus recoveries run ([`crate::Board::i2c_recover`]).
    pub recovers: u32,
    /// Part of the reason of a rule a test breaks on purpose; others still panic.
    pub expect: Option<&'static str>,
    /// Every rule broken so far (the reasons), expected or not.
    pub violations: Vec<&'static str>,
}

impl Bus {
    pub(super) fn new() -> Self {
        Bus {
            lux10: Some(3000), // 300 lux: indoors
            light: [0; 0x20],
            light_on_since: None,
            light_late_ms: 0,
            light_por: true,
            light_epoch: None,
            light_on_ms: 0,
            light_reads: 0,
            transactions: 0,
            baro: Some((10_132, 2150)),
            baro_alt: None,
            baro_p0: crate::altitude::P_STD,
            baro_noise: 0,
            baro_done: None,
            baro_conversions: 0,
            baro_ctrl1: 0,
            accel_ctrl1: 0x77,
            stuck_sda: false,
            stuck_hard: false,
            nack_left: 0,
            nacks: 0,
            recovers: 0,
            expect: None,
            violations: Vec::new(),
        }
    }

    /// A rule broken: panics with the reason unless a test expects it (then it is only recorded).
    fn violation(&mut self, why: &'static str) {
        self.violations.push(why);
        match self.expect {
            Some(e) if why.contains(e) => {}
            _ => panic!("the sensor bus would misbehave on this (expected {:?}): {why}", self.expect),
        }
    }

    /// The nine clock pulses and the STOP ran (S5): the slave lets go of SDA.
    /// False if the bus is still stuck (`stuck_hard`).
    pub(super) fn recover(&mut self) -> bool {
        self.recovers += 1;
        self.stuck_sda = self.stuck_hard;
        !self.stuck_sda
    }

    /// The transfer does not happen: a stuck bus or a NACK burst.
    fn fails(&mut self) -> bool {
        self.transactions += 1;
        let burst = self.nack_left > 0;
        self.nack_left = self.nack_left.saturating_sub(1);
        let f = self.stuck_sda || burst;
        self.nacks += u32::from(f);
        f
    }

    /// The light sensor's conversion period number at `now` and whether the first one is done.
    fn light_epoch_at(&self, now: u64) -> (u64, bool) {
        let t = self.light_on_since.map_or(0, |t| now.saturating_sub(t));
        let period = 100 + self.light_late_ms;
        (t / period, self.light_on_since.is_some() && t >= period)
    }

    /// A supply glitch on the light sensor (S7): every register back to its default (MEAS_RATE 0x22, GAIN 0x01 =
    /// 3x, standby), MAIN_STATUS bit 5 set until it is read (datasheet p.11 and p.13).
    pub fn light_glitch(&mut self, now: u64) {
        if let Some(t) = self.light_on_since.take() {
            self.light_on_ms += now - t;
        }
        (self.light[0], self.light[4], self.light[5]) = (0, 0x22, 0x01);
        self.light_por = true;
    }

    pub(super) fn read(&mut self, now: u64, addr: u8, reg: u8, buf: &mut [u8]) -> bool {
        use crate::ambient::*;
        if self.fails() {
            return false;
        }
        let (r, len) = (usize::from(reg), buf.len());
        match addr {
            LIGHT_ADDR => {
                let Some(lux10) = self.lux10 else { return false };
                let (epoch, ready) = self.light_epoch_at(now);
                let raw = lux10.min(0x3_ffff);
                let mut regs = self.light;
                regs[0x06] = LIGHT_ID;
                regs[0x07] =
                    if ready && self.light_epoch != Some(epoch) { 0x08 } else { 0 } | u8::from(self.light_por) << 5; // S3, S7
                let status_read = r <= 0x07 && r + len > 0x07;
                self.light_por &= !status_read;
                regs[0x0d..0x10].copy_from_slice(&raw.to_le_bytes()[..3]);
                if r <= 0x0f && r + len > 0x0d {
                    // with MAIN_STATUS in the same read the firmware can see the data is old or reset
                    if !ready && !status_read {
                        self.violation("S4: ALS data read inside the first conversion (old data)");
                    }
                    if (regs[0x04], regs[0x05]) != (0x22, 0x02) && !status_read {
                        self.violation("S6: the lux scale assumes 18 bits, gain 6 (MEAS_RATE, GAIN rewritten)");
                    }
                    self.light_epoch = Some(epoch);
                    self.light_reads += 1;
                }
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = regs[(r + i) % regs.len()];
                }
                true
            }
            BARO_ADDR => {
                let Some((p, t)) = self.baro else { return false };
                let converting = self.baro_done.is_some_and(|d| now < d);
                let ready = self.baro_done.is_some_and(|d| now >= d);
                let mut regs = [0u8; 0x30];
                regs[0x0f] = BARO_ID;
                regs[0x10] = self.baro_ctrl1;
                regs[0x11] = 0x10 | u8::from(converting); // IF_ADD_INC, ONE_SHOT while converting (S2)
                regs[0x27] = if ready { 0x03 } else { 0 };
                let raw = match self.baro_alt {
                    Some(f) => {
                        let h = f64::from(f(now)) / 44_307_694.0;
                        let noise = (now.wrapping_mul(2_654_435_761) >> 7) as u32 % (2 * self.baro_noise + 1);
                        ((f64::from(self.baro_p0) * (1.0 - h).powf(1.0 / 0.190284)) as u32)
                            .saturating_add(noise)
                            .saturating_sub(self.baro_noise)
                    }
                    None => p * 4096 / 10,
                };
                regs[0x28..0x2b].copy_from_slice(&raw.to_le_bytes()[..3]);
                regs[0x2b..0x2d].copy_from_slice(&t.to_le_bytes());
                let end = r + len; // one past the last register
                // S1: with BDU, a read that starts inside the pressure (or temperature) bytes and stops short of
                // the high byte leaves the output locked and mixes two conversions.
                if self.baro_ctrl1 & 2 != 0 && (r <= 0x29 && (0x29..=0x2a).contains(&end) || r <= 0x2b && end == 0x2c) {
                    self.violation("S1: BDU set, output read without its high byte (split read)");
                }
                if r <= 0x2a && end > 0x28 {
                    self.baro_done = None; // reading the pressure clears the status
                }
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = regs[(r + i) % regs.len()];
                }
                true
            }
            ACCEL_ADDR => true,
            _ => false,
        }
    }

    pub(super) fn write(&mut self, now: u64, addr: u8, reg: u8, data: &[u8]) -> bool {
        use crate::ambient::*;
        if self.fails() {
            return false;
        }
        let first = data.first().copied().unwrap_or(0);
        match addr {
            LIGHT_ADDR if self.lux10.is_some() => {
                for (i, &v) in data.iter().enumerate() {
                    self.light[(usize::from(reg) + i) % 0x20] = v;
                }
                if reg == 0 {
                    match (first & 0x02 != 0, self.light_on_since) {
                        (true, None) => {
                            self.light_on_since = Some(now);
                            self.light_epoch = None;
                        }
                        (false, Some(t)) => {
                            self.light_on_ms += now - t;
                            self.light_on_since = None;
                        }
                        _ => {}
                    }
                }
                true
            }
            BARO_ADDR if self.baro.is_some() => {
                for (i, &v) in data.iter().enumerate() {
                    // IF_ADD_INC: a multi-byte write goes on to the next register
                    match usize::from(reg) + i {
                        0x10 => self.baro_ctrl1 = v,
                        0x11 => {
                            let converting = self.baro_done.is_some_and(|d| now < d);
                            if v & 1 != 0 && !converting && self.baro_ctrl1 & 0x70 == 0 {
                                self.baro_done = Some(now + 20);
                                self.baro_conversions += 1;
                            }
                        }
                        _ => {}
                    }
                }
                true
            }
            ACCEL_ADDR => {
                if reg == 0x20 {
                    self.accel_ctrl1 = first;
                }
                true
            }
            _ => false,
        }
    }
}
