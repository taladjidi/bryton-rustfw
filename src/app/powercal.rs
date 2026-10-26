//! The power meter calibration in the app (powercal.rs is the model): the CALIBRATE POWER row of SENSORS, the screen
//! (what to do, the request, the result, the AUTO ZERO switch), and the request sent as an acknowledged message on
//! the power channel as the trainer's are (app/trainer.rs): a message the chip refuses or the meter does not take
//! (`EVENT_TRANSFER_TX_FAILED`) is sent again until the time-out. Not while a ride runs: the cranks must be still.
//!
//! The screen is drawn whole when the phase changes (a rare event, a request and its answer), never per second.

use crate::*;
use powercal::{AutoZero, Phase};
use text::{Align, Line, Style, draw, draw_in};

/// The buttons, as (x, y, w, h): the AUTO ZERO switch, and CALIBRATE (CANCEL while asking).
const AUTO_BTN: (u32, u32, u32, u32) = (8, 200, 224, 48);
const MAIN_BTN: (u32, u32, u32, u32) = (8, 256, 224, 48);

impl App {
    /// The power meter is paired and heard (the CALIBRATE POWER row is offered).
    pub(crate) fn power_heard(&self) -> bool {
        let i = Kind::Power as usize;
        matches!(self.radio.sensors.get(i), Some(Sensor::Paired(_)))
            && self.radio.heard_ms.get(i).is_some_and(|&ms| ms < sensors::STALE_MS)
    }

    /// Opens the calibration screen, unless a ride runs.
    pub(crate) fn open_powercal(&mut self) {
        if self.ride.state() == ride::State::Running {
            return self.say("NO CALIBRATION RIDING");
        }
        self.powercal.reset();
        self.powercal_shown = None;
        self.open(Screen::PowerCal);
    }

    /// CALIBRATE, or CANCEL while a request is out.
    fn powercal_go(&mut self) {
        if self.powercal.asking() {
            self.powercal.reset();
        } else if self.ride.state() == ride::State::Running {
            self.say("NO CALIBRATION RIDING");
        } else if !self.power_heard() {
            self.say("NO POWER METER HEARD");
        } else {
            self.powercal.start_zero();
        }
    }

    /// An input on the calibration screen: Start calibrates (or cancels), Page and Lap switch the auto zero, Back
    /// leaves; the touch buttons do the same.
    pub(crate) fn powercal_input(&mut self, i: Input) {
        match i {
            Input::Select => self.powercal_go(),
            Input::Next | Input::Prev => self.powercal.toggle_auto(),
            Input::Touch(Gesture::Tap(x, y)) if inside(MAIN_BTN, x, y) => self.powercal_go(),
            Input::Touch(Gesture::Tap(x, y)) if inside(AUTO_BTN, x, y) => self.powercal.toggle_auto(),
            _ => {}
        }
    }

    /// Back from the screen: a request is given up (nothing is sent: the meter finishes on its own).
    pub(crate) fn powercal_leave(&mut self) {
        self.powercal.reset();
        self.close();
    }

    /// Once a pass: the request, the clocks, and the screen when the phase changed.
    pub(crate) fn step_powercal(&mut self, b: &mut impl Board) {
        if self.screen != Screen::PowerCal {
            return;
        }
        if self.powercal.asking() && (self.ride.state() == ride::State::Running || !self.ant) {
            self.powercal.reset(); // a ride started under it
        }
        self.powercal.tick(self.dt_ms);
        if let Some(page) = self.powercal.due()
            && nrf::ant_ack(b, &mut self.rx, Kind::Power.channel(), page).is_err()
        {
            self.powercal.unsent(); // refused (the last still queued) or a byte lost: sent again
        }
        let now = (self.powercal.phase(), self.powercal.auto());
        if self.powercal_shown != Some(now) {
            self.powercal_shown = Some(now);
            self.show(b);
        }
    }

    /// The whole calibration screen.
    pub(crate) fn draw_powercal(&self, c: &mut impl Canvas) {
        let w = ui::W - 16;
        ui::draw_title(c, "POWER ZERO");
        let grey = Style { fg: ui::GREY, ..ui::LABEL };
        let mut y = ui::LIST_Y + 4;
        let (color, status) = match self.powercal.phase() {
            Phase::Ready | Phase::Asking => {
                for line in ["UNCLIP,", "CRANKS VERTICAL,", "NO PRESSURE"] {
                    draw_in(c, &ui::LABEL, 8, y, w, Align::Centre, line);
                    y += 26;
                }
                let asking = self.powercal.asking();
                (ui::GREY, if asking { "ASKING..." } else { "" })
            }
            Phase::Done(offset, before) => {
                let mut l = Line::<12>::new();
                if offset < 0 {
                    l.push_str("-");
                }
                l.push_u32(u32::from(offset.unsigned_abs()));
                draw_in(c, &grey, 8, y, w, Align::Centre, "ZERO OFFSET");
                draw_in(c, &Style { fg: ui::WHITE, ..ui::VALUE }, 8, y + 26, w, Align::Centre, l.as_str());
                if let Some(p) = before {
                    let mut l = Line::<20>::new();
                    l.push_str("WAS ");
                    if p < 0 {
                        l.push_str("-");
                    }
                    l.push_u32(u32::from(p.unsigned_abs()));
                    draw_in(c, &grey, 8, y + 76, w, Align::Centre, l.as_str());
                }
                (ui::WHITE, "")
            }
            Phase::Failed => (ui::RED, "FAILED"),
            Phase::NoAnswer => (ui::RED, "NO ANSWER"),
            Phase::AutoSet => {
                (ui::WHITE, if self.powercal.auto() == Some(AutoZero::On) { "AUTO ZERO ON" } else { "AUTO ZERO OFF" })
            }
        };
        if !status.is_empty() {
            draw_in(c, &Style { fg: color, ..ui::LABEL }, 8, 150, w, Align::Centre, status);
        }
        if let Some(a @ (AutoZero::On | AutoZero::Off)) = self.powercal.auto() {
            let label = if a == AutoZero::On { "AUTO ZERO: ON" } else { "AUTO ZERO: OFF" };
            btn(c, AUTO_BTN, ui::DARK, ui::WHITE, label);
        } else if self.powercal.auto() == Some(AutoZero::Unsupported) {
            draw_in(c, &grey, 8, AUTO_BTN.1 + 14, w, Align::Centre, "NO AUTO ZERO");
        }
        if self.powercal.asking() {
            btn(c, MAIN_BTN, ui::DARK, ui::WHITE, "CANCEL");
        } else {
            btn(c, MAIN_BTN, ui::ACCENT, ui::BLACK, "CALIBRATE");
        }
    }
}

/// A button: its box in `bg`, the label centred.
fn btn(c: &mut impl Canvas, r: (u32, u32, u32, u32), bg: u16, fg: u16, label: &str) {
    c.fill(r.0, r.1, r.2, r.3, bg);
    let st = Style { fg, bg, ..ui::LABEL };
    draw(c, &st, r.0 + r.2 / 2, r.1 + (r.3 - st.height()) / 2, Align::Centre, label);
}
