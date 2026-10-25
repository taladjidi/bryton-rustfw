//! The home trainer, whatever the link to it (ANT+ FE-C now, `drivers/fec.rs`; BLE FTMS later): what is read from
//! it ([`Reading`]), what is commanded ([`Command`], [`Mode`]) and when to send it again. Pure logic, fed from the
//! main loop's passes; the transports only turn a [`Command`] into their bytes and their bytes into a
//! [`Reading`], so a second transport changes none of this.
//!
//! The modes (the trainer profile's control pages, whose names every trainer app uses):
//! - **ERG**: hold a target power, whatever the cadence. Driven by the workout's power step ([`Drive::erg_w`]) or,
//!   with none, by hand.
//! - **RESISTANCE**: a fixed brake, percent of the trainer's maximum.
//! - **SIM**: the road: a grade (the course's at the distance ridden, [`Drive::grade_x100`], or by hand), with a
//!   rolling resistance coefficient ([`CRR`]).
//!
//! Nothing is sent until the rider takes the trainer over ([`Trainer::engaged`]: a key on the trainer screen, a
//! workout step with a power target, a course followed): a trainer just paired keeps the resistance it had.

/// How long a command stands before it is sent again, ms. A trainer keeps the last command for as long as it hears
/// from the head unit; sending again every few seconds also puts right a command it never took (the radio is not
/// reliable).
pub const RESEND_MS: u32 = 4000;

/// Longest target power, W (page 49's field is 0.25 W units in 16 bits, so it could say 16383 W; no trainer goes
/// past 2000).
pub const MAX_W: u16 = 2000;

/// How many times the capabilities are asked for before giving up (a trainer that never answers page 70), and the
/// wait between two asks, ms.
pub const CAPS_TRIES: u8 = 5;
/// See [`CAPS_TRIES`].
pub const CAPS_RETRY_MS: u32 = 2000;

/// A grade past these is not sent, % x 100: most trainers stop at 15 to 20 %.
pub const GRADE_RANGE: core::ops::RangeInclusive<i16> = -1000..=2000;

/// Rolling resistance coefficient sent in SIM, in 5e-5 units: 0.004, tarmac (Garmin's Connect IQ table gives
/// 0.004 for an asphalt road, developer.garmin.com/connect-iq/api-docs/Toybox/AntPlus/SimulationSettings.html).
pub const CRR: u8 = 80;

/// How the trainer is driven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A target power.
    Erg,
    /// A fixed resistance.
    Resistance,
    /// A grade.
    Sim,
}

impl Mode {
    /// The next one in the round the Start key goes through.
    pub fn next(self) -> Mode {
        match self {
            Mode::Erg => Mode::Resistance,
            Mode::Resistance => Mode::Sim,
            Mode::Sim => Mode::Erg,
        }
    }

    /// Its name on screen.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Erg => "ERG",
            Mode::Resistance => "RES",
            Mode::Sim => "SIM",
        }
    }
}

/// What the trainer is told, in the units of the ANT+ profile's pages (FTMS converts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Resistance, 0.5 % units (0 to 200).
    Resistance(u8),
    /// Target power, 0.25 W units.
    Power(u16),
    /// Grade in 0.01 % units, and the rolling resistance coefficient in 5e-5 units.
    Track {
        /// Grade, % x 100.
        grade_x100: i16,
        /// Rolling resistance coefficient, 5e-5 units.
        crr: u8,
    },
    /// The rider and the bike: weights in 0.01 kg and 0.05 kg, the wheel's diameter in mm.
    User {
        /// Rider, 0.01 kg.
        weight_cg: u16,
        /// Bike, 0.05 kg.
        bike_50g: u16,
        /// Wheel diameter, mm.
        wheel_mm: u16,
    },
}

/// What the trainer reported, as of its last pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// Instantaneous power, W.
    pub power: Option<u16>,
    /// Cadence, rpm.
    pub cadence: Option<u8>,
    /// Speed (the trainer's virtual one), km/h x 10.
    pub speed_x10: Option<u16>,
    /// Heart rate the trainer relays, bpm.
    pub hr: Option<u8>,
    /// Status bits of the trainer-specific page: 1 power calibration needed, 2 resistance calibration needed, 4 user
    /// configuration needed.
    pub status: u8,
    /// Whether the power is at the target: 0 yes, 1 speed too low for it, 2 speed too high, 3 not known.
    pub limits: u8,
    /// The trainer's state: 1 off, 2 ready, 3 in use, 4 finished.
    pub state: u8,
    /// Maximum resistance, N (capabilities page).
    pub max_resistance: Option<u16>,
    /// Modes it supports (capabilities page): bit 0 resistance, bit 1 target power, bit 2 simulation.
    pub modes: Option<u8>,
    /// The last command it acknowledged on the command status page: (the page of the command, its status: 0 done,
    /// 1 failed, 2 not supported, 3 rejected, 4 pending).
    pub last_cmd: Option<(u8, u8)>,
    /// Manufacturer (common page 80).
    pub maker: Option<u16>,
    /// Serial number (common page 81).
    pub serial: Option<u32>,
    /// The pages understood so far, one bit each ([`crate::fec::seen`]); for the tests.
    pub seen: u8,
}

impl Reading {
    /// The trainer asks to be calibrated (spin-down).
    pub fn needs_calibration(&self) -> bool {
        self.status & 3 != 0
    }

    /// The trainer asks for the rider's configuration.
    pub fn needs_user(&self) -> bool {
        self.status & 4 != 0
    }
}

/// What the app says drives the trainer now (the workout, the course); the trainer's own state fills the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drive {
    /// The workout's power target now, W (the midpoint of a range).
    pub erg_w: Option<u16>,
    /// Which step it is (an adjustment made by hand goes with its step).
    pub step: usize,
    /// The course's grade at the distance ridden, % x 100.
    pub grade_x100: Option<i16>,
}

/// What to send once after the trainer is heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setup {
    /// Ask for its capabilities.
    Capabilities,
    /// Tell it the rider and the bike.
    User,
}

/// What went out last, so a message that did not get through is the one sent again ([`Trainer::unsent`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sent {
    Nothing,
    Caps,
    User,
    Command,
}

/// The trainer's state: what was read, what the rider chose, what was sent last.
#[derive(Clone, Copy, Debug)]
pub struct Trainer {
    /// What it reported.
    pub reading: Reading,
    mode: Mode,
    manual_w: u16,
    adjust_w: i16,
    res_half: u8,
    grade_x100: i16,
    engaged: bool,
    last: Option<Command>,
    sent_ms: u32,
    step: usize,
    caps_tries: u8,
    caps_ms: u32,
    user_sent: bool,
    user_ms: u32,
    user_kg: u16,
    what: Sent,
}

impl Default for Trainer {
    fn default() -> Self {
        Trainer {
            reading: Reading::default(),
            mode: Mode::Erg,
            manual_w: 100,
            adjust_w: 0,
            res_half: 40,
            grade_x100: 0,
            engaged: false,
            last: None,
            sent_ms: 0,
            step: 0,
            caps_tries: 0,
            caps_ms: CAPS_RETRY_MS,
            user_sent: false,
            user_ms: 0,
            user_kg: 0,
            what: Sent::Nothing,
        }
    }
}

impl Trainer {
    /// The mode in force: the workout's ERG or the course's SIM when they drive, else the rider's choice.
    pub fn mode(&self, d: &Drive) -> Mode {
        if d.erg_w.is_some() {
            Mode::Erg
        } else if d.grade_x100.is_some() {
            Mode::Sim
        } else {
            self.mode
        }
    }

    /// The target power now, W.
    pub fn target_w(&self, d: &Drive) -> u16 {
        let w = i32::from(d.erg_w.unwrap_or(self.manual_w)) + i32::from(self.adjust_w);
        w.clamp(0, i32::from(MAX_W)) as u16
    }

    /// The resistance, percent.
    pub fn resistance_pct(&self) -> u8 {
        self.res_half / 2
    }

    /// The grade now, % x 100.
    pub fn grade(&self, d: &Drive) -> i16 {
        d.grade_x100.unwrap_or(self.grade_x100).clamp(*GRADE_RANGE.start(), *GRADE_RANGE.end())
    }

    /// What the mode in force tells the trainer.
    ///
    /// ```
    /// use bryton_rustfw::trainer::{Command, Drive, Trainer};
    /// let t = Trainer::default();
    /// let d = Drive { erg_w: Some(180), ..Drive::default() };
    /// assert_eq!(t.command(&d), Command::Power(720), "180 W in quarter watts");
    /// ```
    pub fn command(&self, d: &Drive) -> Command {
        match self.mode(d) {
            Mode::Erg => Command::Power(self.target_w(d) * 4),
            Mode::Resistance => Command::Resistance(self.res_half),
            Mode::Sim => Command::Track { grade_x100: self.grade(d), crr: CRR },
        }
    }

    /// The rider took the trainer over (or the workout or the course did): commands go out.
    pub fn engaged(&self, d: &Drive) -> bool {
        self.engaged || d.erg_w.is_some() || d.grade_x100.is_some()
    }

    /// Start: the next mode (the adjustment made on a workout step is dropped). Not while a workout or a course drives
    /// the trainer: the mode in force is theirs.
    pub fn cycle_mode(&mut self, d: &Drive) {
        if d.erg_w.is_some() || d.grade_x100.is_some() {
            return;
        }
        self.mode = self.mode(d).next();
        (self.engaged, self.adjust_w) = (true, 0);
    }

    /// Page or Lap on the trainer screen: the target one step up or down (`big`: a held key, twice the step): 5 W,
    /// 5 % of resistance or 0.5 % of grade. Under a workout step the adjustment is added to the step's target.
    pub fn step_by(&mut self, d: &Drive, up: bool, big: bool) {
        self.engaged = true;
        let n = if big { 2 } else { 1 };
        let n = if up { n } else { -n };
        match self.mode(d) {
            Mode::Erg if d.erg_w.is_some() => {
                self.adjust_w = (i32::from(self.adjust_w) + 5 * n).clamp(-500, 500) as i16;
            }
            Mode::Erg => self.manual_w = (i32::from(self.manual_w) + 5 * n).clamp(0, i32::from(MAX_W)) as u16,
            Mode::Resistance => self.res_half = (i32::from(self.res_half) + 10 * n).clamp(0, 200) as u8,
            Mode::Sim => {
                let g = i32::from(self.grade_x100) + 50 * n;
                self.grade_x100 = g.clamp(i32::from(*GRADE_RANGE.start()), i32::from(*GRADE_RANGE.end())) as i16;
            }
        }
    }

    /// The last message (a command, the capabilities request, the rider's page) did not get through, the chip
    /// refusing it or the trainer not acknowledging it: it is due again at the next free slot.
    pub fn unsent(&mut self) {
        match core::mem::replace(&mut self.what, Sent::Nothing) {
            Sent::Command => self.last = None,
            Sent::Caps => (self.caps_tries, self.caps_ms) = (self.caps_tries.saturating_sub(1), CAPS_RETRY_MS),
            Sent::User => self.user_sent = false,
            Sent::Nothing => {}
        }
    }

    /// The trainer went silent (or was never heard): what was sent is not known to stand any more.
    pub fn lost(&mut self) {
        (self.last, self.caps_tries, self.caps_ms) = (None, 0, CAPS_RETRY_MS);
        (self.user_sent, self.what) = (false, Sent::Nothing);
        self.reading = Reading::default();
    }

    /// One pass, `dt_ms` long, with the trainer `heard` or not: the command to send now, if any: at once when it
    /// changed, else again every [`RESEND_MS`].
    pub fn due(&mut self, dt_ms: u32, heard: bool, d: &Drive) -> Option<Command> {
        if d.step != self.step {
            (self.step, self.adjust_w) = (d.step, 0); // an adjustment belongs to the step it was made on
        }
        self.sent_ms = self.sent_ms.saturating_add(dt_ms);
        (self.user_ms, self.caps_ms) = (self.user_ms.saturating_add(dt_ms), self.caps_ms.saturating_add(dt_ms));
        if !heard || !self.engaged(d) {
            return None;
        }
        let c = self.command(d);
        if self.last != Some(c) || self.sent_ms >= RESEND_MS {
            (self.last, self.sent_ms, self.what) = (Some(c), 0, Sent::Command);
            return Some(c);
        }
        None
    }

    /// What to send once the trainer is heard, one thing per call: its capabilities (again every [`CAPS_RETRY_MS`]
    /// until its page 54 is seen, [`CAPS_TRIES`] times at most), the rider and the bike, and the rider again when it
    /// asks for them (at most every 10 s) or when the rider's weight (`weight_kg`) changed.
    pub fn setup(&mut self, weight_kg: u16) -> Option<Setup> {
        if self.reading.seen & crate::fec::seen::P54 == 0
            && self.caps_tries < CAPS_TRIES
            && self.caps_ms >= CAPS_RETRY_MS
        {
            (self.caps_tries, self.caps_ms, self.what) = (self.caps_tries + 1, 0, Sent::Caps);
            return Some(Setup::Capabilities);
        }
        if !self.user_sent || self.user_kg != weight_kg || (self.reading.needs_user() && self.user_ms >= 10_000) {
            (self.user_sent, self.user_ms, self.user_kg, self.what) = (true, 0, weight_kg, Sent::User);
            return Some(Setup::User);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_until_taken_over() {
        let mut t = Trainer::default();
        let none = Drive::default();
        assert_eq!(t.due(250, true, &none), None, "a trainer just paired keeps what it had");
        t.step_by(&none, true, false);
        assert_eq!(t.due(250, true, &none), Some(Command::Power(420)), "105 W");
        assert_eq!(t.due(250, true, &none), None, "unchanged: not again at once");
        assert!(t.due(RESEND_MS, true, &none).is_some(), "but again later");
        assert_eq!(t.due(250, false, &none), None, "silent trainer");
    }

    #[test]
    fn workout_drives_and_adjusts_per_step() {
        let mut t = Trainer::default();
        let d = Drive { erg_w: Some(200), step: 1, ..Drive::default() };
        assert_eq!(t.due(250, true, &d), Some(Command::Power(800)));
        t.step_by(&d, false, true);
        assert_eq!(t.command(&d), Command::Power(760), "10 W less");
        let next = Drive { erg_w: Some(250), step: 2, ..Drive::default() };
        assert_eq!(t.due(250, true, &next), Some(Command::Power(1000)), "the next step: adjustment gone");
    }

    #[test]
    fn modes() {
        let mut t = Trainer::default();
        let none = Drive::default();
        t.cycle_mode(&none);
        assert_eq!(t.command(&none), Command::Resistance(40));
        t.step_by(&none, true, false);
        assert_eq!(t.command(&none), Command::Resistance(50));
        t.cycle_mode(&none);
        t.step_by(&none, true, true);
        assert_eq!(t.command(&none), Command::Track { grade_x100: 100, crr: CRR });
        let course = Drive { grade_x100: Some(-350), ..Drive::default() };
        assert_eq!(t.mode(&course), Mode::Sim);
        assert_eq!(t.grade(&course), -350);
        let mut t = Trainer::default();
        t.reading.seen = crate::fec::seen::P54; // its capabilities came
        assert_eq!(t.setup(70), Some(Setup::User));
        assert_eq!(t.setup(70), None);
        t.reading.status = 4;
        assert_eq!(t.setup(70), None, "asked, but too soon after");
        t.due(10_000, false, &none);
        assert_eq!(t.setup(70), Some(Setup::User));
    }

    /// A start-up message that did not get through is sent again; the capabilities are asked until they come, a
    /// few times; a new weight is told; a workout or a course keeps the mode.
    #[test]
    fn setup_is_retried_and_bounded() {
        let mut t = Trainer::default();
        let none = Drive::default();
        assert_eq!(t.setup(70), Some(Setup::Capabilities));
        t.unsent(); // refused by the chip
        assert_eq!(t.setup(70), Some(Setup::Capabilities), "at once, and a refusal is not a try");
        assert_eq!(t.setup(70), Some(Setup::User));
        t.unsent(); // TX_FAILED
        assert_eq!(t.setup(70), Some(Setup::User), "the rider's page again");
        assert_eq!(t.setup(70), None, "the capabilities are asked again in a while");
        let mut asks = 1;
        for _ in 0..20 {
            t.due(CAPS_RETRY_MS, false, &none);
            asks += u8::from(t.setup(70) == Some(Setup::Capabilities));
        }
        assert_eq!(asks, CAPS_TRIES, "asked again until the limit, no more");
        t.reading.seen = 0;
        t.lost();
        assert_eq!(t.setup(70), Some(Setup::Capabilities), "a trainer heard again starts over");
        assert_eq!(t.setup(70), Some(Setup::User));
        assert_eq!(t.setup(75), Some(Setup::User), "the rider's weight changed: told again");
        assert_eq!(t.setup(75), None);
        // a command refused is due again, not waited for
        t.step_by(&none, true, false);
        assert!(t.due(250, true, &none).is_some());
        assert_eq!(t.due(250, true, &none), None);
        t.unsent();
        assert!(t.due(250, true, &none).is_some());
    }

    #[test]
    fn a_workout_or_a_course_keeps_the_mode() {
        let mut t = Trainer::default();
        let workout = Drive { erg_w: Some(200), ..Drive::default() };
        t.cycle_mode(&workout);
        assert_eq!((t.mode(&Drive::default()), t.engaged), (Mode::Erg, false), "ignored");
        let course = Drive { grade_x100: Some(100), ..Drive::default() };
        t.cycle_mode(&course);
        assert_eq!(t.mode(&Drive::default()), Mode::Erg);
        t.cycle_mode(&Drive::default());
        assert_eq!(t.mode(&Drive::default()), Mode::Resistance);
    }
}
