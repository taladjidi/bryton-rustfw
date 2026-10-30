//! The FTP ramp test in the app (ramp.rs has the protocol, the stop rules and the arithmetic): the built-in workout
//! "RAMP TEST" played by the ordinary workout player, so ERG, the step banner, the beeps and the workout page are
//! the workout code's. Started from WORKOUTS or the trainer screen with a trainer heard. Each ride second feeds the
//! [`ramp::Ramp`]; a stop rule or Lap ends the test: the player jumps to the cool-down, and the result screen asks
//! whether to APPLY the FTP found (START yes, Back no). The ride is the normal one (saved through the end-ride
//! flow, then synced like any ride).

use crate::*;

/// What stands for the workout's file (the graph and the menus tell workouts apart by it): no card file has cluster 0.
const RAMP_FILE: fat::File = fat::File { cluster: 0, size: 0, dir: false };

/// The result screen's touch buttons, APPLY and KEEP: (x, y, w, h).
const RAMP_BUTTONS: [(u32, u32, u32, u32); 2] = [(8, 266, 108, 48), (124, 266, 108, 48)];

impl App {
    /// The ramp is being ridden (a ramp step is the player's current one).
    pub(crate) fn ramp_running(&self) -> bool {
        self.ramp.run.is_some() && self.plan.player.and_then(|p| p.step(self.plan.steps())).is_some_and(ramp::in_ramp)
    }

    /// Starts the ramp test: the workout, and the ride if none runs. Indoors only: a trainer must be heard.
    pub(crate) fn start_ramp(&mut self) {
        if !self.trainer_heard() {
            return self.say("NEEDS TRAINER");
        }
        if self.workout_running() {
            return self.say("WORKOUT RUNNING");
        }
        (self.plan.workout, self.plan.file, self.plan.player) = (ramp::workout(), Some(RAMP_FILE), None);
        self.plan.planned = false;
        (self.ramp.run, self.ramp.result) = (Some(ramp::Ramp::new()), None);
        self.start_workout(0);
    }

    /// One wall second, `ticked` if the ride's timer moved: the ramp takes the readings while a ramp step runs. A
    /// ride that is not moving (paused by hand, or by itself after 3 s without power or cadence) ends the ramp at
    /// once: the trainer would hold the step's target for a rider who has stopped.
    pub(crate) fn ramp_second(&mut self, ticked: bool) {
        let steps = self.plan.steps();
        let Some(i) = self.plan.player.and_then(|p| p.step(steps)) else {
            self.ramp.run = None; // the workout ended or was stopped
            return;
        };
        if self.ramp.run.is_none() || !ramp::in_ramp(i) {
            return;
        }
        if !ticked {
            return self.finish_ramp();
        }
        let target =
            steps.get(i).and_then(|s| s.target.range(&self.settings.zones())).map_or(0, |(lo, hi)| lo.midpoint(hi));
        let hr = self.live.get(Metric::Hr);
        let a1 = self.hrv.alpha1().filter(|_| hr.is_some()).map(|a| (a * 100.0 + 0.5) as u16);
        let (power, cadence) = (self.live.get(Metric::Power), self.live.get(Metric::Cadence));
        if self.ramp.run.as_mut().is_some_and(|r| r.second(power, cadence, target, (a1, hr))) {
            self.finish_ramp();
        }
    }

    /// The test is over (a stop rule, Lap): the result kept, the cool-down played, the result screen due. Under a
    /// whole minute there is no result, and it says so.
    pub(crate) fn finish_ramp(&mut self) {
        let Some(r) = self.ramp.run.take() else { return };
        self.ramp.result = r.outcome();
        let (p, ev) = workout::Player::start(self.plan.steps(), ramp::COOL, self.ride.dist_m());
        self.plan.player = Some(p);
        self.workout_event(ev);
        match self.ramp.result {
            Some(_) => self.ramp.show = true,
            None => self.say("TEST TOO SHORT"),
        }
    }

    /// The FTP START would set: the result to the setting's step (5 W: the backup register keeps FTP / 5, so a finer
    /// value would come back rounded down after a restart), within the setting's range.
    fn applied_ftp(o: &ramp::Outcome) -> u16 {
        let r = menu::FTP_RANGE;
        ((o.ftp + 2) / 5 * 5).clamp(*r.start(), *r.end())
    }

    /// APPLY on the result screen: the FTP found becomes the setting ([`App::applied_ftp`]).
    pub(crate) fn apply_ramp(&mut self, b: &mut impl Board) {
        if let Some(o) = self.ramp.result.take() {
            self.settings.ftp = Self::applied_ftp(&o);
            self.save_misc(b);
            self.say("FTP SET");
        }
    }

    /// A tap on the result screen: APPLY sets the FTP, KEEP leaves it; either closes the screen.
    pub(crate) fn ramp_tap(&mut self, b: &mut impl Board, x: u16, y: u16) -> bool {
        let hit = RAMP_BUTTONS.iter().position(|&bx| inside(bx, x, y));
        match hit {
            Some(0) => self.apply_ramp(b),
            Some(_) => {}
            None => return false,
        }
        self.close();
        true
    }

    /// The result screen, drawn whole (it does not change while it is up). The Start and Lap keys keep their ride
    /// meaning here, so APPLY and KEEP are touch buttons (Back keeps).
    pub(crate) fn draw_ramp(&self, c: &mut impl Canvas) {
        use text::{Align, Style, draw_in};
        let Some(o) = self.ramp.result else { return };
        let grey = Style { fg: ui::GREY, ..ui::LABEL };
        let mut y = ui::PAGE_Y + 2;
        draw_in(c, &ui::LABEL, 0, y, ui::W, Align::Centre, "RAMP TEST");
        y += 24;
        let mut ftp = Line::<8>::new();
        ftp.push_u32(u32::from(o.ftp));
        ftp.push_str(" W");
        draw_in(c, &ui::VALUE, 0, y, ui::W, Align::Centre, ftp.as_str());
        y += 46;
        let mut row = |s: &Style, l: &Line<24>| {
            draw_in(c, s, 0, y, ui::W, Align::Centre, l.as_str());
            y += 26;
        };
        let line = |a: &str, n: Option<u32>, unit: &str| {
            let mut l = Line::<24>::new();
            l.push_str(a);
            if let Some(n) = n {
                l.push_u32(n);
            }
            l.push_str(unit);
            l
        };
        row(&ui::LABEL, &line("BEST 1 MIN ", Some(u32::from(o.best_w)), " W"));
        let mut time = Line::<24>::new();
        time.push_str("RAMP ");
        push_clock(&mut time, o.secs);
        row(&ui::LABEL, &time);
        // HRVT1 from a ramp: the heart rate is the figure; the power lags and is marked "~"
        match o.aet {
            Some(h) => {
                row(&ui::LABEL, &line("RAMP HRVT1 ~", Some(u32::from(h.watts)), " W"));
                match h.bpm {
                    Some(b) => row(&ui::LABEL, &line("AT ", Some(u32::from(b)), " BPM")),
                    None => row(&grey, &line("NO HEART RATE", None, "")),
                }
            }
            None => {
                row(&ui::LABEL, &line("RAMP HRVT1 --", None, ""));
                row(&grey, &line("", None, ""));
            }
        }
        row(&grey, &line("NOW ", Some(u32::from(self.settings.ftp)), " W"));
        let mut ask = Line::<16>::new();
        ask.push_str("APPLY ");
        ask.push_u32(u32::from(Self::applied_ftp(&o)));
        ask.push_str(" W?");
        draw_in(c, &ui::LABEL, 0, y, ui::W, Align::Centre, ask.as_str());
        for (k, (&(bx, by, bw, bh), label)) in RAMP_BUTTONS.iter().zip(["APPLY", "KEEP"]).enumerate() {
            let (bg, fg) = if k == 0 { (ui::ACCENT, ui::ON_ACCENT) } else { (ui::BLACK, ui::WHITE) };
            let frame = if k == 0 { ui::ACCENT } else { ui::GREY };
            c.fill(bx, by, bw, bh, frame);
            c.fill(bx + 2, by + 2, bw - 4, bh - 4, bg);
            let ty = by + (bh - ui::LABEL.height()) / 2;
            draw_in(c, &Style { fg, bg, ..ui::LABEL }, bx + 2, ty, bw - 4, Align::Centre, label);
        }
    }
}
