//! The quick panel (`Screen::Quick`): a swipe down opens it on the overview and on every ride page, a swipe up or
//! Back closes it. What the rider wants to see right away, top to bottom (ui/quick.rs has the geometry):
//! 1. media: now playing and the remote's buttons (the same actions as the media screen, [`App::media`]);
//! 2. the power meter, when one is paired: watts, its battery and ZERO, which runs the zero-offset calibration in
//!    place (app/powercal.rs: the request, the retries, the answer) and writes its result on the same block;
//! 3. one line per paired sensor: its value or SEARCHING, and its battery when it said (ANT+ common page 82);
//! 4. GPS, phone link and unit battery, and the brightness buttons.
//!
//! Every box has a fingerprint ([`App::quick_prints`]); the screen is drawn whole when it opens (or when a sensor is
//! paired or forgotten under it, which moves the lines) and then a second's update, and a tap's, redraws only the boxes
//! whose fingerprint changed, each in its own fixed box.

use crate::*;
use phone::Media;
use powercal::Phase;
use text::{Align, Line};
use ui::quick::{self as q, Battery, Hit};

/// The boxes: now playing's title and artist, the media buttons, the power block (watts, battery, ZERO and its
/// result), the status line's three, the brightness level, then three a sensor line (name, value, battery).
const B_TITLE: usize = 0;
const B_ARTIST: usize = 1;
const B_MEDIA: usize = 2;
const B_WATTS: usize = 3;
const B_PBATT: usize = 4;
const B_ZERO: usize = 5;
const B_STATUS: usize = 6; // three
const B_LEVEL: usize = 9;
const B_ROWS: usize = 10;
/// The boxes in all.
pub(crate) const QUICK_BOXES: usize = B_ROWS + 3 * q::ROWS;

/// The fingerprint of a box's text and colour.
fn print(s: &str, color: u16) -> u32 {
    fnv(fnv(0x811c_9dc5, s.as_bytes()), &color.to_le_bytes()) | 1
}

impl App {
    /// A power meter is paired: the panel has its block.
    fn quick_power(&self) -> bool {
        matches!(self.radio.sensors.get(Kind::Power as usize), Some(Sensor::Paired(_)))
    }

    /// The paired sensors with a line (the power meter has its block), at most [`q::ROWS`].
    fn quick_kinds(&self) -> impl Iterator<Item = Kind> + '_ {
        Kind::ALL
            .into_iter()
            .filter(|&k| match k {
                Kind::Power => false,
                Kind::Trainer => self.trainer_on(),
                _ => matches!(self.radio.sensors.get(k as usize), Some(Sensor::Paired(_))),
            })
            .take(q::ROWS)
    }

    /// What the panel's lines are: the power block (bit 7) and the number of sensor lines.
    pub(crate) fn quick_shape(&self) -> u8 {
        u8::from(self.quick_power()) << 7 | self.quick_kinds().count() as u8
    }

    /// Opens the panel, with no calibration result left from before.
    pub(crate) fn open_quick(&mut self) {
        self.powercal.reset();
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

    /// The zero calibration's result on the power block, and its colour.
    fn quick_result(&self) -> (Line<16>, u16) {
        let mut l = Line::<16>::new();
        let color = match self.powercal.phase() {
            Phase::Asking => {
                l.push_str("ASKING...");
                ui::GREY
            }
            Phase::Done(offset, _) => {
                l.push_str("ZERO ");
                if offset < 0 {
                    l.push_str("-");
                }
                l.push_u32(u32::from(offset.unsigned_abs()));
                l.push_str(" OK");
                ui::WHITE
            }
            Phase::Failed => {
                l.push_str("FAILED");
                ui::RED
            }
            Phase::NoAnswer => {
                l.push_str("NO ANSWER");
                ui::RED
            }
            Phase::Ready | Phase::AutoSet => ui::GREY,
        };
        (l, color)
    }

    /// ZERO can be pressed: the meter is heard, no ride runs, no request is out.
    fn quick_zero_ok(&self) -> bool {
        self.power_heard() && self.ride.state() != ride::State::Running && !self.powercal.asking()
    }

    /// Box `i`'s text and colour, for the text boxes.
    pub(crate) fn quick_text(&self, i: usize) -> (Line<24>, u16) {
        let mut l = Line::<24>::new();
        let put = |l: &mut Line<24>, s: &str| l.push_str(s);
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
                        put(&mut l, t);
                        ui::WHITE
                    }
                    _ => {
                        put(&mut l, if n.title.is_some() { n.artist } else { "" });
                        ui::GREY
                    }
                }
            }
            B_WATTS => {
                if let Some(w) = self.power.rolling(0) {
                    l.push_u32(u32::from(w)); // the 3 s power, whatever window a tap chose for the fields
                } else {
                    l.push_str("--");
                }
                put(&mut l, "W");
                ui::WHITE
            }
            B_ZERO => {
                let (r, c) = self.quick_result();
                put(&mut l, r.as_str());
                c
            }
            _ if (B_STATUS..B_STATUS + 3).contains(&i) => match i - B_STATUS {
                0 => {
                    let sats = self.gps.pvt.filter(|p| p.good()).map(|p| p.sats);
                    put(&mut l, "GPS ");
                    match sats {
                        Some(n) => l.push_u32(u32::from(n)),
                        None => put(&mut l, "--"),
                    }
                    if sats.is_some() { ui::WHITE } else { ui::GREY }
                }
                1 => match self.phone.link() {
                    Link::Secure => {
                        put(&mut l, "PHONE");
                        ui::WHITE
                    }
                    Link::Pairing => {
                        put(&mut l, "PAIRING");
                        ui::GREY
                    }
                    _ => {
                        put(&mut l, "PHONE");
                        ui::GREY
                    }
                },
                _ => match self.strip.battery {
                    Some(p) => {
                        l.push_u32(u32::from(p));
                        put(&mut l, "%");
                        match (self.strip.charging, p <= LOW_BATTERY_PERCENT) {
                            (true, _) => ui::GREEN,
                            (_, true) => ui::RED,
                            _ => ui::WHITE,
                        }
                    }
                    None => {
                        put(&mut l, "--");
                        ui::GREY
                    }
                },
            },
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
            _ if i < B_ROWS => ui::BLACK, // the media buttons, the power battery: no text
            _ => {
                // a sensor line: name (3k), value (3k + 1)
                let k = (i - B_ROWS) / 3;
                match self.quick_kinds().nth(k) {
                    Some(kind) if (i - B_ROWS).is_multiple_of(3) => {
                        put(&mut l, kind.short());
                        ui::GREY
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
                B_PBATT if power => 1 + self.decoders.battery_status(Kind::Power).map_or(0, u32::from),
                B_WATTS if power => {
                    let (l, c) = self.quick_text(i);
                    print(l.as_str(), c)
                }
                B_ZERO if power => {
                    let (l, c) = self.quick_text(i);
                    print(l.as_str(), c) ^ u32::from(self.quick_zero_ok()) << 1
                }
                B_WATTS | B_PBATT | B_ZERO => 0,
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
                _ => {
                    let (l, c) = self.quick_text(i);
                    print(l.as_str(), c)
                }
            };
        }
        p
    }

    /// Draws box `i` (the media buttons and ZERO whole, each other box in its fixed place).
    fn quick_draw_box(&self, c: &mut impl Canvas, i: usize) {
        let power = self.quick_power();
        let sens_y = q::SENSORS_Y[usize::from(!power)];
        let (text, color) = self.quick_text(i);
        let s = text.as_str();
        match i {
            B_TITLE => q::draw_text(c, (8, q::TITLE_Y, ui::W - 16), Align::Left, color, s),
            B_ARTIST => q::draw_text(c, (8, q::ARTIST_Y, ui::W - 16), Align::Left, color, s),
            B_MEDIA => {
                let n = self.media_now();
                q::draw_media_buttons(c, n.playing, n.title.is_none());
            }
            B_WATTS if power => {
                let style = text::Style { scale: 3, fg: color, ..ui::LABEL };
                text::draw_in(c, &style, q::WATTS.0, q::WATTS.1, q::WATTS.2, Align::Left, s);
            }
            B_PBATT if power => {
                let tag = self.decoders.battery_status(Kind::Power).map_or(Battery::Unknown, q::battery_of_status);
                q::draw_battery(c, q::POWER_BATT.0, q::POWER_BATT.1, tag);
            }
            B_ZERO if power => {
                q::draw_zero(c, self.quick_zero_ok());
                q::draw_text(c, (q::RESULT.0, q::RESULT.1, q::RESULT.2), Align::Left, color, s);
            }
            _ if (B_STATUS..B_STATUS + 3).contains(&i) => {
                let (x, w) = q::STATUS[i - B_STATUS];
                let align = [Align::Left, Align::Centre, Align::Right][i - B_STATUS];
                q::draw_text(c, (x, q::STATUS_Y, w), align, color, s);
            }
            B_LEVEL => q::draw_text(c, q::LEVEL, Align::Centre, color, s),
            _ if i >= B_ROWS => {
                let (k, part) = ((i - B_ROWS) / 3, (i - B_ROWS) % 3);
                let y = sens_y + k as u32 * q::ROW;
                let Some(kind) = self.quick_kinds().nth(k) else { return };
                let (x, w) = q::SENSOR_BOXES[part];
                match part {
                    2 => q::draw_battery(c, x, y, self.quick_battery(kind)),
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
                    Some(Hit::Zero) => self.powercal_go(),
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
