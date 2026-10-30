//! The quick panel (`Screen::Quick`): a swipe down opens it on the overview and on every ride page, a swipe up or
//! Back closes it. What the rider wants to see right away, top to bottom (ui/quick.rs has the geometry):
//! 1. media: now playing and the remote's buttons (the same actions as the media screen, [`App::media`]);
//! 2. one line per paired sensor: its value or SEARCHING, and its battery when it said (ANT+ common page 82). The
//!    power meter's line is POWER, no watts: its value box is a ZERO button that runs the zero-offset calibration in
//!    place (app/powercal.rs: the request, the retries, the answer) and then shows the result for 5 s. A tap anywhere
//!    on the line arms it ("SURE?" for 3 s), a second tap in time calibrates: zeroing with the cranks turning gives
//!    a wrong offset, and the line sits under the volume buttons, which a thumb mistaps. The line sits where the
//!    power meter falls in `Kind::ALL`'s order;
//! 3. the brightness buttons.
//!
//! Every box has a fingerprint ([`App::quick_prints`]); the screen is drawn whole when it opens (or when a sensor is
//! paired or forgotten under it, which moves the lines) and then a second's update, and a tap's, redraws only the boxes
//! whose fingerprint changed, each in its own fixed box. A title or artist wider than its box scrolls (ui::quick::window):
//! `QUICK_STEP_MS` is one step, 4 characters a second: readable, and the idle pass (250 ms, `IDLE_POLL_MS`) is the
//! slowest the loop wakes, so a faster step would only be skipped. The window is part of the box's fingerprint, so a
//! step redraws the title's 224 x 20 box (the artist's only when its own text moves), and text that fits never moves.
//! Each text cell is 20 px high: the lines are 20 px apart, so a box never clears the one below.

use crate::*;
use phone::Media;
use powercal::Phase;
use text::{Align, Line};
use ui::quick::{self as q, Battery, Hit};

/// How often the open panel looks for changes, and so the scroll's step (see the module doc).
pub(crate) const QUICK_STEP_MS: u32 = 250;
/// How long ZERO stays armed, and how long a calibration's result stays on the button, in seconds (the clock counts whole
/// seconds, so the arming lasts 2 to 3 s).
const ARM_S: u32 = 3;
const RESULT_S: u32 = 5;
/// The boxes: now playing's title and artist, the media buttons, the brightness level, then three a sensor line (name,
/// value, battery).
const B_TITLE: usize = 0;
const B_ARTIST: usize = 1;
const B_MEDIA: usize = 2;
const B_LEVEL: usize = 3;
const B_ROWS: usize = 4;
/// The boxes in all.
pub(crate) const QUICK_BOXES: usize = B_ROWS + 3 * q::ROWS;

/// `t` (epoch seconds) has passed `now`.
fn expired(t: u32, now: u32) -> bool {
    now.wrapping_sub(t) as i32 >= 0
}

/// The fingerprint of a box's text and colour.
fn print(s: &str, color: u16) -> u32 {
    fnv(fnv(0x811c_9dc5, s.as_bytes()), &color.to_le_bytes()) | 1
}

impl App {
    /// The POWER line's index among the sensor lines, when a meter is paired.
    pub(crate) fn quick_power(&self) -> Option<usize> {
        self.quick_kinds().position(|k| k == Kind::Power)
    }

    /// The paired sensors with a line, at most [`q::ROWS`].
    fn quick_kinds(&self) -> impl Iterator<Item = Kind> + '_ {
        Kind::ALL
            .into_iter()
            .filter(|&k| match k {
                Kind::Trainer => self.trainer_on(),
                _ => matches!(self.radio.sensors.get(k as usize), Some(Sensor::Paired(_))),
            })
            .take(q::ROWS)
    }

    /// What the panel's lines are: the number of sensor lines.
    pub(crate) fn quick_shape(&self) -> u8 {
        self.quick_kinds().count() as u8
    }

    /// Opens the panel, with no calibration result left from before.
    pub(crate) fn open_quick(&mut self) {
        self.powercal.reset();
        self.quick_zero = (None, None);
        self.quick_scroll = (0, 0);
        self.open(Screen::Quick);
    }

    /// The sensor line's value and its colour: the live reading, or SEARCHING.
    fn quick_value(&self, kind: Kind) -> (Line<16>, u16) {
        let mut l = Line::<16>::new();
        let fresh = self.radio.heard_ms.get(kind as usize).is_some_and(|&ms| ms < sensors::STALE_MS);
        let with = |l: &mut Line<16>, f: Field, unit: &str| {
            l.push_str(self.field_text(f).as_str());
            l.push_str(unit);
        };
        match kind {
            _ if !fresh && kind != Kind::Trainer => l.push_str("SEARCHING"),
            Kind::Hr => with(&mut l, Field::Hr, " BPM"),
            Kind::Speed => with(&mut l, Field::Speed, if self.settings.imperial { " MPH" } else { " KMH" }),
            Kind::Cadence => with(&mut l, Field::Cadence, " RPM"),
            Kind::SpeedCadence => {
                with(&mut l, Field::Speed, if self.settings.imperial { "MPH " } else { "KMH " });
                with(&mut l, Field::Cadence, "RPM");
            }
            Kind::Shifting | Kind::Di2 => with(&mut l, Field::Gear, ""),
            Kind::Trainer => {
                l.push_str(self.field_text(Field::TrainerMode).as_str());
                if l.as_str().is_empty() || l.as_str() == "--" || !(fresh || self.ftms_on()) {
                    l.clear();
                    l.push_str("SEARCHING");
                }
            }
            Kind::Power => {}
        }
        (l, if l.as_str() == "SEARCHING" { ui::GREY } else { ui::WHITE })
    }

    /// The sensor's battery tag, from page 82 (the shifting systems: their metric, a percent for a Di2).
    fn quick_battery(&self, kind: Kind) -> Battery {
        match (kind, self.live.get(Metric::ShiftBattery)) {
            (Kind::Shifting | Kind::Di2, Some(s)) if s >= sensors::SHIFT_STATUS => {
                q::battery_of_status((s - sensors::SHIFT_STATUS) as u8)
            }
            (Kind::Shifting | Kind::Di2, Some(p)) => q::battery_of_percent(p.min(100) as u8),
            (Kind::Shifting | Kind::Di2, None) => Battery::Unknown,
            _ => self.decoders.battery_status(kind).map_or(Battery::Unknown, q::battery_of_status),
        }
    }

    /// The ZERO button's label, text colour and fill: the calibration in progress or its result (until
    /// `update_quick` takes it back after [`RESULT_S`]), else SURE? when armed, else ZERO.
    pub(crate) fn quick_button(&self) -> (Line<16>, u16, u16) {
        let mut l = Line::<16>::new();
        let now = self.now_s();
        let armed = self.quick_zero.0.is_some_and(|t| !expired(t, now));
        let (fg, bg) = match self.powercal.phase() {
            Phase::Asking => {
                l.push_str("ASKING...");
                (ui::GREY, ui::DARK)
            }
            _ if armed => {
                l.push_str("SURE?");
                (ui::ON_ACCENT, ui::ACCENT)
            }
            Phase::Done(offset, _) => {
                if offset < 0 {
                    l.push_str("-");
                }
                l.push_u32(u32::from(offset.unsigned_abs()));
                l.push_str(" OK");
                (ui::WHITE, ui::DARK)
            }
            Phase::Failed => {
                l.push_str("FAILED");
                (ui::RED, ui::DARK)
            }
            Phase::NoAnswer => {
                l.push_str("NO ANSWER");
                (ui::RED, ui::DARK)
            }
            Phase::Ready | Phase::AutoSet => {
                l.push_str("ZERO");
                if self.quick_zero_ok() { (ui::ON_ACCENT, ui::ACCENT) } else { (ui::GREY, ui::DARK) }
            }
        };
        (l, fg, bg)
    }

    /// A tap on the POWER line: the first arms ZERO, a second within [`ARM_S`] calibrates. Nothing when it cannot.
    fn quick_zero_tap(&mut self) {
        if !self.quick_zero_ok() {
            return;
        }
        let now = self.now_s();
        if self.quick_zero.0.is_some_and(|t| !expired(t, now)) {
            self.quick_zero = (None, None);
            self.powercal_go();
        } else {
            self.powercal.reset(); // a result on the button gives way to SURE?
            self.quick_zero = (Some(now.wrapping_add(ARM_S)), None);
        }
    }

    /// Takes back an expired arming, and a result shown for [`RESULT_S`].
    fn quick_zero_tick(&mut self) {
        let now = self.now_s();
        if self.quick_zero.0.is_some_and(|t| expired(t, now)) {
            self.quick_zero.0 = None;
        }
        if matches!(self.powercal.phase(), Phase::Done(..) | Phase::Failed | Phase::NoAnswer) {
            match self.quick_zero.1 {
                None => self.quick_zero.1 = Some(now.wrapping_add(RESULT_S)),
                Some(t) if expired(t, now) => {
                    self.powercal.reset();
                    self.quick_zero.1 = None;
                }
                Some(_) => {}
            }
        }
    }

    /// ZERO can be pressed: the meter is heard, no ride runs, no request is out.
    fn quick_zero_ok(&self) -> bool {
        self.power_heard() && self.ride.state() != ride::State::Running && !self.powercal.asking()
    }

    /// Box `i`'s text and colour, for the text boxes.
    pub(crate) fn quick_text(&self, i: usize) -> (Line<72>, u16) {
        let mut l = Line::<72>::new(); // a window of up to 18 characters of up to 4 bytes
        let put = |l: &mut Line<72>, s: &str| l.push_str(s);
        let color = match i {
            B_TITLE | B_ARTIST => {
                let n = self.media_now();
                match (i, n.title) {
                    (B_TITLE, None) => {
                        put(&mut l, "NO PHONE");
                        ui::GREY
                    }
                    (B_TITLE, Some("")) => {
                        put(&mut l, "NOTHING PLAYING");
                        ui::GREY
                    }
                    (B_TITLE, Some(t)) => {
                        put(&mut l, q::window(t, ui::W - 16, self.quick_scroll.1));
                        ui::WHITE
                    }
                    _ => {
                        let a = if n.title.is_some() { n.artist } else { "" };
                        put(&mut l, q::window(a, ui::W - 16, self.quick_scroll.1));
                        ui::GREY
                    }
                }
            }
            B_LEVEL => {
                match LIGHT_LEVELS.get(self.settings.light_level) {
                    Some(&v) => {
                        l.push_u32(u32::from(v));
                        put(&mut l, "%");
                    }
                    None => put(&mut l, "AUTO"),
                }
                ui::WHITE
            }
            _ if i < B_ROWS => ui::BLACK, // the media buttons: no text
            _ => {
                // a sensor line: name (3k), value (3k + 1)
                let k = (i - B_ROWS) / 3;
                match self.quick_kinds().nth(k) {
                    Some(kind) if (i - B_ROWS).is_multiple_of(3) => {
                        put(&mut l, kind.short());
                        ui::GREY
                    }
                    Some(Kind::Power) => {
                        let (v, c, _) = self.quick_button();
                        put(&mut l, v.as_str());
                        c
                    }
                    Some(kind) => {
                        let (v, c) = self.quick_value(kind);
                        put(&mut l, v.as_str());
                        c
                    }
                    None => ui::BLACK,
                }
            }
        };
        (l, color)
    }

    /// Every box's fingerprint (0: the box is not on the panel).
    pub(crate) fn quick_prints(&self) -> [u32; QUICK_BOXES] {
        let power = self.quick_power();
        let n = self.media_now();
        let mut p = [0; QUICK_BOXES];
        for (i, slot) in p.iter_mut().enumerate() {
            *slot = match i {
                B_MEDIA => 1 + u32::from(n.title.is_none()) * 2 + u32::from(n.playing),
                _ if i >= B_ROWS && (i - B_ROWS) % 3 == 2 => {
                    // a sensor line's battery
                    let k = (i - B_ROWS) / 3;
                    self.quick_kinds().nth(k).map_or(0, |kind| match self.quick_battery(kind) {
                        Battery::Unknown => 1,
                        Battery::Low => 2,
                        Battery::Bars(n) => 2 + u32::from(n),
                    })
                }
                _ if i >= B_ROWS && self.quick_kinds().nth((i - B_ROWS) / 3).is_none() => 0,
                _ if i >= B_ROWS && (i - B_ROWS) % 3 == 1 && power == Some((i - B_ROWS) / 3) => {
                    // the ZERO button: its fill too (the text and colour do not tell ready from greyed out)
                    let (l, c, bg) = self.quick_button();
                    print(l.as_str(), c ^ bg.rotate_left(3))
                }
                _ => {
                    let (l, c) = self.quick_text(i);
                    print(l.as_str(), c)
                }
            };
        }
        // A sensor line is 16 px apart but its text cell is 20 px high: redrawing line k's name or value clears the top
        // of line k + 1's, so line k + 1's box changes with line k's (drawn after it, in index order).
        for k in 1..q::ROWS {
            for part in 0..2 {
                let (cur, up) = (B_ROWS + 3 * k + part, B_ROWS + 3 * (k - 1) + part);
                if p[cur] != 0 && p[up] != 0 {
                    p[cur] = fnv(p[cur], &p[up].to_le_bytes()) | 1;
                }
            }
        }
        p
    }

    /// Draws box `i` (the media buttons whole, each other box in its fixed place).
    fn quick_draw_box(&self, c: &mut impl Canvas, i: usize) {
        let sens_y = q::SENSORS_Y;
        let (text, color) = self.quick_text(i);
        let s = text.as_str();
        match i {
            B_TITLE => q::draw_text(c, (8, q::TITLE_Y, ui::W - 16), Align::Left, color, s),
            B_ARTIST => q::draw_text(c, (8, q::ARTIST_Y, ui::W - 16), Align::Left, color, s),
            B_MEDIA => {
                let n = self.media_now();
                q::draw_media_buttons(c, n.playing, n.title.is_none());
            }
            B_LEVEL => q::draw_text(c, q::LEVEL, Align::Centre, color, s),
            _ if i >= B_ROWS => {
                let (k, part) = ((i - B_ROWS) / 3, (i - B_ROWS) % 3);
                let y = sens_y + k as u32 * q::ROW;
                let Some(kind) = self.quick_kinds().nth(k) else { return };
                let (x, w) = q::SENSOR_BOXES[part];
                match part {
                    2 => q::draw_battery(c, x, y, self.quick_battery(kind)),
                    // the button fills its own 16 px row only: the next line's ink is not touched
                    1 if kind == Kind::Power => {
                        let (l, fg, bg) = self.quick_button();
                        q::draw_button(c, (x, y, w, q::ROW), bg, fg, l.as_str());
                    }
                    _ => q::draw_text(c, (x, y, w), Align::Left, color, s),
                }
            }
            _ => {}
        }
    }

    /// The whole panel, drawn when it opens.
    pub(crate) fn draw_quick(&self, c: &mut impl Canvas) {
        q::draw_light_buttons(c);
        let prints = self.quick_prints();
        for (i, _) in prints.iter().enumerate().filter(|&(_, &p)| p != 0) {
            self.quick_draw_box(c, i);
        }
    }

    /// What the panel shows has been drawn: the fingerprints and the shape to compare with.
    pub(crate) fn quick_drawn(&mut self) {
        (self.quick_shown, self.quick_shape) = (self.quick_prints(), self.quick_shape());
    }

    /// Redraws the boxes that changed since they were drawn, each in its own place; the whole panel when the lines
    /// moved (a sensor paired or forgotten, a meter appeared).
    pub(crate) fn update_quick(&mut self, b: &mut impl Board) {
        if self.quick_shape != self.quick_shape() {
            return self.show(b);
        }
        self.quick_zero_tick();
        // a new track starts both lines over; else the scroll takes a step
        let track = fnv(fnv(0, self.media_now().title.unwrap_or("").as_bytes()), self.media_now().artist.as_bytes());
        self.quick_scroll =
            if track == self.quick_scroll.0 { (track, self.quick_scroll.1.wrapping_add(1)) } else { (track, 0) };
        let now = self.quick_prints();
        let mut p = Panel(b, self.area(), self.look());
        for (i, (&a, &s)) in now.iter().zip(&self.quick_shown).enumerate() {
            if a != s && a != 0 {
                self.quick_draw_box(&mut p, i);
            }
        }
        self.quick_shown = now;
    }

    /// An input on the panel: Back or a swipe up closes it, a tap acts on the button under it.
    pub(crate) fn input_quick(&mut self, b: &mut impl Board, i: Input, shown: Seen) {
        match i {
            Input::Back | Input::Touch(Gesture::Swipe(Dir::Up)) => {
                self.powercal.reset(); // a request out is given up (nothing more is sent)
                self.close();
            }
            Input::Touch(Gesture::Tap(x, y)) => {
                match q::hit(x, y, self.quick_power()) {
                    Some(Hit::Media(button)) => {
                        use ui::media::Button;
                        let key = match button {
                            Button::Prev => Media::Prev,
                            Button::Next => Media::Next,
                            Button::VolDown => Media::VolDown,
                            Button::VolUp => Media::VolUp,
                            _ => Media::PlayPause,
                        };
                        self.media(b, key, false);
                    }
                    Some(Hit::Zero) => self.quick_zero_tap(),
                    Some(Hit::Light(up)) => {
                        // dim to bright: the levels from the last (10 %) to the first (100 %), then AUTO
                        let n = LIGHT_LEVELS.len();
                        let rank = if self.settings.light_level >= n { n } else { n - 1 - self.settings.light_level };
                        let rank = if up { (rank + 1).min(n) } else { rank.saturating_sub(1) };
                        self.settings.light_level = if rank == n { LIGHT_AUTO } else { n - 1 - rank };
                        self.save_misc(b);
                        self.update_light(b);
                    }
                    None => {}
                }
                return self.update_quick(b);
            }
            _ => return,
        }
        self.reshow(b, shown);
    }
}
