//! The FTP ramp test in the app (ramp.rs has the protocol, the stop rules and the arithmetic): the built-in workout
//! "RAMP TEST" played by the ordinary workout player, so ERG, the step banner, the beeps and the workout page are
//! the workout code's. Started from WORKOUTS or the trainer screen with a trainer heard. Each ride second feeds the
//! [`ramp::Ramp`]; a stop rule or Lap ends the test: the player jumps to the cool-down, and the result screen asks
//! whether to APPLY the FTP found (START yes, Back no). The ride is the normal one (saved through the end-ride
//! flow, then synced like any ride).

use crate::*;

/// What stands for the workout's file (the graph and the menus tell workouts apart by it): no card file has cluster 0.
const RAMP_FILE: fat::File = fat::File { cluster: 0, size: 0, dir: false };

impl App {
    /// The ramp is being ridden (a ramp step is the player's current one).
    pub(crate) fn ramp_running(&self) -> bool {
        self.ramp.is_some() && self.player.and_then(|p| p.step(self.plan.steps())).is_some_and(ramp::in_ramp)
    }

    /// Starts the ramp test: the workout, and the ride if none runs. Indoors only: a trainer must be heard.
    pub(crate) fn start_ramp(&mut self) {
        if !self.trainer_heard() {
            return self.say("NEEDS TRAINER");
        }
        if self.workout_running() {
            return self.say("WORKOUT RUNNING");
        }
        (self.plan, self.plan_file, self.player) = (ramp::workout(), Some(RAMP_FILE), None);
        (self.ramp, self.ramp_result) = (Some(ramp::Ramp::new()), None);
        self.start_workout(0);
    }

    /// One ride second, `ticked` if the timer moved: the ramp takes the readings while a ramp step runs.
    pub(crate) fn ramp_second(&mut self, ticked: bool) {
        let steps = self.plan.steps();
        let Some(i) = self.player.and_then(|p| p.step(steps)) else {
            self.ramp = None; // the workout ended or was stopped
            return;
        };
        if !ticked || !ramp::in_ramp(i) {
            return;
        }
        let (ftp, max_hr) = (self.settings.ftp, self.settings.max_hr);
        let target = steps.get(i).and_then(|s| s.target.range(ftp, max_hr)).map_or(0, |(lo, hi)| lo.midpoint(hi));
        let a1 = self.hrv.alpha1().filter(|_| self.live.get(Metric::Hr).is_some()).map(|a| (a * 100.0 + 0.5) as u16);
        let (power, cadence) = (self.live.get(Metric::Power), self.live.get(Metric::Cadence));
        if self.ramp.as_mut().is_some_and(|r| r.second(power, cadence, target, a1)) {
            self.finish_ramp();
        }
    }

    /// The test is over (a stop rule, Lap): the result kept, the cool-down played, the result screen due. Under a
    /// whole minute there is no result, and it says so.
    pub(crate) fn finish_ramp(&mut self) {
        let Some(r) = self.ramp.take() else { return };
        self.ramp_result = r.outcome();
        let (p, ev) = workout::Player::start(self.plan.steps(), ramp::COOL, self.ride.dist_m());
        self.player = Some(p);
        self.workout_event(ev);
        match self.ramp_result {
            Some(_) => self.ramp_show = true,
            None => self.say("TEST TOO SHORT"),
        }
    }

    /// The FTP START would set: the result to the setting's step (5 W: the backup register keeps FTP / 5, so a finer
    /// value would come back rounded down after a restart), within the setting's range.
    fn applied_ftp(o: &ramp::Outcome) -> u16 {
        let r = menu::FTP_RANGE;
        ((o.ftp + 2) / 5 * 5).clamp(*r.start(), *r.end())
    }

    /// START on the result screen: the FTP found becomes the setting ([`App::applied_ftp`]).
    pub(crate) fn apply_ramp(&mut self, b: &mut impl Board) {
        if let Some(o) = self.ramp_result.take() {
            self.settings.ftp = Self::applied_ftp(&o);
            self.save_misc(b);
            self.say("FTP SET");
        }
    }

    /// The result screen, drawn whole (it does not change while it is up).
    pub(crate) fn draw_ramp(&self, c: &mut impl Canvas) {
        use text::{Align, Style, draw_in};
        let Some(o) = self.ramp_result else { return };
        let grey = Style { fg: ui::GREY, ..ui::LABEL };
        let green = Style { fg: ui::GREEN, ..ui::VALUE };
        let mut y = ui::PAGE_Y + 4;
        draw_in(c, &ui::LABEL, 0, y, ui::W, Align::Centre, "RAMP TEST");
        y += 24;
        let mut ftp = Line::<8>::new();
        ftp.push_u32(u32::from(o.ftp));
        ftp.push_str(" W");
        draw_in(c, &green, 0, y, ui::W, Align::Centre, ftp.as_str());
        y += 46;
        let mut row = |s: &Style, l: &Line<24>| {
            draw_in(c, s, 0, y, ui::W, Align::Centre, l.as_str());
            y += 26;
        };
        let line = |a: &str, n: Option<u32>, unit: &str| {
            let mut l = Line::<24>::new();
            l.push_str(a);
            match n {
                Some(n) => {
                    l.push_u32(n);
                    l.push_str(unit);
                }
                None => l.push_str("--"),
            }
            l
        };
        row(&ui::LABEL, &line("BEST 1 MIN ", Some(u32::from(o.best_w)), " W"));
        let mut time = line("RAMP ", None, "");
        time.clear();
        time.push_str("RAMP ");
        push_clock(&mut time, o.secs);
        row(&ui::LABEL, &time);
        row(&ui::LABEL, &line("AET ", o.aet.map(u32::from), " W"));
        row(&grey, &line("NOW ", Some(u32::from(self.settings.ftp)), " W"));
        y += 6;
        let mut ask = Line::<16>::new();
        ask.push_str("APPLY ");
        ask.push_u32(u32::from(Self::applied_ftp(&o)));
        ask.push_str(" W?");
        draw_in(c, &ui::LABEL, 0, y, ui::W, Align::Centre, ask.as_str());
        draw_in(c, &Style { fg: ui::GREEN, ..ui::LABEL }, 0, y + 26, ui::W, Align::Centre, "START: YES");
        draw_in(c, &Style { fg: ui::RED, ..ui::LABEL }, 0, y + 52, ui::W, Align::Centre, "BACK: NO");
    }
}
