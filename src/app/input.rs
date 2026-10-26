//! Keys and the screen stack: a key's presses (short, held, repeating), what is on screen and the screens under it
//! (`open` / `close`, so no screen knows where Back goes), and [`App::input`], which turns a key or a gesture into
//! what the screen on top does with it.

use crate::*;
use phone::Media;

/// The four keys, with stock's key codes (BOARD.md 5) as their bit in [`Board::keys_down`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Back / power (PA1).
    Back = 0x01,
    /// Lap (PB2).
    Lap = 0x02,
    /// Start / stop (PB4).
    Start = 0x04,
    /// Page (PE3).
    Page = 0x08,
}

impl Key {
    const ALL: [Key; 4] = [Key::Back, Key::Lap, Key::Start, Key::Page];

    /// How long the key must be held for a long press: Back (power off) longer than the others.
    pub fn long_ms(self) -> u32 {
        if self == Key::Back { 3000 } else { 2000 }
    }

    /// From its name (the dev link's `key` command).
    pub fn from_name(name: &[u8]) -> Option<Key> {
        match name {
            b"back" => Some(Key::Back),
            b"lap" => Some(Key::Lap),
            b"start" => Some(Key::Start),
            b"page" => Some(Key::Page),
            _ => None,
        }
    }
}

/// A key press, short (acted on at the release) or long (acted on once, while still held).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// Pressed and released before [`Key::long_ms`].
    Short(Key),
    /// Held for [`Key::long_ms`].
    Long(Key),
    /// Still held: one every [`REPEAT_MS`] from [`REPEAT_AFTER_MS`] on (a value editor steps on it). The
    /// [`Press::Short`] or [`Press::Long`] still follows.
    Repeat(Key),
    /// Back and Lap held together for [`CHORD_MS`]: the touch lock ([`Input::TouchLock`]). While they are down
    /// together neither acts on its own, at its release or held.
    Chord,
}

/// Back and Lap held together this long toggle the touch lock.
pub const CHORD_MS: u32 = 1000;

/// A held key repeats from then on ([`Press::Repeat`]).
pub const REPEAT_AFTER_MS: u32 = 500;

/// Then once per this.
pub const REPEAT_MS: u32 = 100;

/// How long each key has been held, between polls.
#[derive(Default)]
pub struct Keys {
    held_ms: [u32; 4],
    /// Keys down since before the start, not seen until released: the power button is Back's line, and the press
    /// that woke the unit from standby, held on, was taken for Back held 3 s, which switched it off again.
    stuck: u8,
    /// How long Back and Lap have been down together.
    chord_ms: u32,
    /// Back and Lap were down together and one is still down: their own presses are dropped until both are up.
    chord: bool,
}

impl Keys {
    /// The keys down at the start (`mask`): ignored until each is released.
    pub fn at_start(mask: u8) -> Self {
        Keys { stuck: mask, ..Keys::default() }
    }

    /// The keys of `mask` that count: not one down since the start and not released since.
    pub fn live(&self, mask: u8) -> u8 {
        mask & !self.stuck
    }

    /// One poll, `dt_ms` after the previous one, with the keys held down (`mask` of [`Key`] codes). One press at
    /// most per poll, the first key in `Key::ALL` order.
    pub fn poll(&mut self, mask: u8, dt_ms: u32) -> Option<Press> {
        self.stuck &= mask;
        let mask = mask & !self.stuck;
        let pair = Key::Back as u8 | Key::Lap as u8;
        let chord_was = self.chord;
        let both = mask & pair == pair;
        self.chord = (self.chord || both) && mask & pair != 0;
        let chord_ms = if both { self.chord_ms + dt_ms } else { 0 };
        let fired = chord_ms >= CHORD_MS && self.chord_ms < CHORD_MS;
        self.chord_ms = chord_ms;
        let mut press = None;
        for (key, held) in Key::ALL.into_iter().zip(&mut self.held_ms) {
            let long = key.long_ms();
            let now = if mask & key as u8 != 0 { *held + dt_ms } else { 0 };
            let p = match (now, *held) {
                (0, 1..) if *held < long => Some(Press::Short(key)),
                (1.., _) if *held < long && now >= long => Some(Press::Long(key)),
                (REPEAT_AFTER_MS.., _) if now / REPEAT_MS != *held / REPEAT_MS => Some(Press::Repeat(key)),
                _ => None,
            };
            *held = now;
            if !((self.chord || chord_was) && pair & key as u8 != 0) {
                press = press.or(p);
            }
        }
        if fired { Some(Press::Chord) } else { press }
    }
}

/// What is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The main screen: the time, big, the date and four fields.
    Main,
    /// Ride page `n`.
    Ride(usize),
    /// The page editor on ride page `n` (its copy, [`EDITED`]), cell `k` selected (app/editor.rs).
    Edit(usize, usize),
    /// A menu, with the selected row.
    Menu(Menu, usize),
    /// The BLE devices heard.
    Nearby,
    /// The end-ride screen (app/riding.rs), button slot `k` selected ([`ui::end_labels`]): RESUME, SAVE, DISCARD.
    End(usize),
    /// Its DISCARD's question: DISCARD or KEEP.
    Discard(usize),
    /// The card of the ride just ended: its map, elevation profile and numbers.
    Summary,
    /// The card of a past ride (`App::past`); Back returns to the history.
    Past,
    /// The phone's media remote (app/phone.rs).
    Media,
    /// The trainer: mode and target (app/trainer.rs).
    Trainer,
    /// The power meter calibration (app/powercal.rs).
    PowerCal,
    /// The ramp test's result, APPLY or keep (app/ramp.rs).
    RampResult,
    /// ABOUT: the version, the commit, Ferris.
    About,
    /// Notification `k` of the inbox in full (app/notifs.rs), its first line shown `top`.
    Notif(usize, usize),
}

/// Places in the round of the ride pages: the workout page, the pages, then the course page.
const ROUND: usize = PAGES + 2;

/// Ride page `n`'s place in the round.
fn place(n: usize) -> usize {
    match n {
        WORKOUT => 0,
        COURSE => PAGES + 1,
        n => n + 1,
    }
}

/// The ride page at place `k` in the round.
fn page_at(k: usize) -> usize {
    match k {
        0 => WORKOUT,
        k if k > PAGES => COURSE,
        k => k - 1,
    }
}

/// During a ride, holding Back a second time within this long switches off (the first hold only warns).
pub const OFF_CONFIRM_MS: u32 = 10_000;

/// The value of `App::confirm` while a past ride's DELETE waits for its second Start: no menu row has it.
pub(crate) const PAST_DELETE: (Menu, usize) = (Menu::History, usize::MAX);

impl App {
    /// The first ride page that is on from place `from` in the round (taken round), going forward by `step` (1, or
    /// [`ROUND`] - 1 for backward): the workout page while one runs, the ride pages, then the course page while a course is followed; page 1 is never
    /// off.
    pub(crate) fn ride_page(&self, from: usize, step: usize) -> Screen {
        let n = (0..ROUND).map(|k| page_at((from + k * step) % ROUND)).find(|&n| self.page_on(n)).unwrap_or(0);
        Screen::Ride(n)
    }

    /// The screen Page (`next`) or a swipe right (back) goes to from a ride page or the overview: the next or the
    /// previous ride page in the round, the first one from the overview (none before it); else the screen as is.
    fn paged(&self, next: bool) -> Screen {
        match (self.screen, next) {
            (Screen::Main, true) => self.ride_page(0, 1),
            (Screen::Ride(n), true) => self.ride_page(place(n) + 1, 1),
            (Screen::Ride(n), false) => self.ride_page(place(n) + ROUND - 1, ROUND - 1),
            (s, _) => s,
        }
    }

    /// The next or the previous ride page ([`App::paged`]), drawn: a shifter's press ([`App::remote`]).
    pub(crate) fn page_by(&mut self, b: &mut impl Board, next: bool) {
        let before = self.screen;
        self.screen = self.paged(next);
        if self.screen != before {
            self.show(b);
        }
    }

    /// Ride page `n` is in the round: a page not off, the course page while a course is followed, the workout page
    /// while a workout is loaded.
    pub(crate) fn page_on(&self, n: usize) -> bool {
        // a course followed, whether or not the map can draw it yet (no fix, a ride started without one: review)
        match n {
            COURSE => self.courses.as_deref().is_some_and(|c| c.on.is_some()),
            WORKOUT => self.player.is_some(),
            n => n < self.ride_count() && !(self.indoors() && self.indoor_blank(n)),
        }
    }

    /// Opens screen `s` over this one: Back returns here.
    pub(crate) fn open(&mut self, s: Screen) {
        if let Some(u) = self.under.get_mut(self.depth) {
            *u = self.screen;
            self.depth += 1;
        }
        self.screen = s;
        self.menu_chosen = false;
        if s == Screen::Trainer {
            self.ftms_retry(); // the rider wants the trainer: a Bluetooth one given up is tried again
        }
    }

    /// The menu Page held opens: the ride menu while a ride records or is paused (over the page it was held on),
    /// else the main menu.
    fn menu_root(&self) -> Menu {
        match (self.ride.state(), self.screen) {
            (ride::State::Idle, _) => Menu::Main,
            (_, Screen::Ride(n)) => Menu::Ride(n),
            _ => Menu::Ride(OVERVIEW),
        }
    }

    /// Out of every menu at once, to the page the first one was opened over (the first ride page if that one is
    /// off now, as the course page once the course is stopped): a ride menu's row that acted.
    pub(crate) fn leave_menus(&mut self) {
        let page = match self.under.first().copied() {
            Some(Screen::Ride(n)) if !self.page_on(n) => self.ride_page(0, 1),
            Some(s @ (Screen::Main | Screen::Ride(_))) => s,
            _ => Screen::Main,
        };
        (self.depth, self.screen, self.menu_chosen) = (0, page, false);
    }

    /// Back to the screen under this one (a ride page switched off since shows the main screen).
    pub(crate) fn close(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        self.screen = match self.under.get(self.depth) {
            Some(&Screen::Ride(n)) if !self.page_on(n) => Screen::Main,
            Some(&s) if self.depth > 0 || matches!(s, Screen::Main | Screen::Ride(_)) => s,
            _ => Screen::Main,
        };
        self.menu_chosen = false;
        self.clamp_menu();
    }

    /// The selection on a row the menu still has: rows go when what they act on does (the ride menu's WORKOUT when
    /// the workout ends, COURSE when the course is stopped from COURSES).
    pub(crate) fn clamp_menu(&mut self) {
        if let Screen::Menu(m, sel) = self.screen {
            self.screen = Screen::Menu(m, sel.min(self.rows(m).max(1) - 1));
        }
    }

    /// Acts on a key press ([`Input::of`]). A key held to repeat that acted (a list scrolled, a value stepped) is
    /// not acted on again at its release.
    pub fn press(&mut self, b: &mut impl Board, p: Press) {
        if matches!(p, Press::Short(_)) && core::mem::take(&mut self.repeated) {
            return;
        }
        if let Some(i) = Input::of(p) {
            self.input(b, i);
        }
    }

    /// Acts on an input: Next goes from the main screen through the ride pages and back, Menu opens the menu, Back
    /// goes from a ride page to the overview; in the menus Next / Prev move down / up, Select opens or changes, Back goes out; a
    /// number being edited goes up with Prev, down with Next. Off switches off anywhere.
    pub fn input(&mut self, b: &mut impl Board, i: Input) {
        if matches!(i, Input::Touch(_)) && (!self.settings.touch || self.strip.locked) {
            return; // TOUCH OFF, or locked: the keys only
        }
        let (before, more_before) = (self.screen, self.more_on);
        self.clamp_menu(); // a row gone while the menu showed (drawn again below if the screen of rows changed)
        let recording = self.ride.state() == ride::State::Running;
        let woke = self.light_shown == 0;
        self.idle_ms = 0;
        if woke {
            self.update_light(b); // an input after the time-out: the light comes back (and the input acts)
            if i == Input::Back {
                return; // Back that wakes the screen only wakes it: nothing is acted on blind
            }
        }
        if i == Input::TouchLock {
            self.strip.locked = !self.strip.locked; // the strip shows it; the banner says it
            return self.say(if self.strip.locked { "TOUCH LOCKED" } else { "TOUCH UNLOCKED" });
        }
        let cleared = self.banner_ms.take().is_some();
        if cleared {
            self.unbanner(b); // and the input acts, except Back (below)
        }
        // a row waiting to be confirmed (App::confirmed): only Start or a tap on it confirms, anything else drops it
        let armed = self.confirm.take();
        if let (Some(_), Screen::Menu(menu, sel)) = (armed, self.screen)
            && !matches!(i, Input::Select | Input::Touch(Gesture::Tap(..)))
        {
            self.update_menu_rows(b, menu, sel); // its "SURE?" goes
        }
        // Back that clears a banner only clears it (it is "NO" to a confirmation); a call's Back declines
        if cleared && i == Input::Back && !self.phone.ringing {
            return;
        }
        let graph_hit = match (self.screen, i) {
            (Screen::Main | Screen::Ride(_), Input::Touch(Gesture::Tap(x, y) | Gesture::Hold(x, y))) => {
                self.graph_at(x, y)
            }
            _ => None,
        };
        match (self.screen, i) {
            // during a ride (or on its card), a first hold only warns: switching off ends the riding (the recorder
            // checkpoints first, and the next start resumes the ride paused)
            (_, Input::Off) if self.busy() && self.off_armed_ms == 0 => {
                self.off_armed_ms = OFF_CONFIRM_MS;
                return self.alarm("HOLD BACK AGAIN TO SWITCH OFF");
            }
            (_, Input::Off) => return self.power_off(b),
            // an incoming call, over any screen: Start or the green button answers, Back or the red one declines
            (_, Input::Select | Input::Back | Input::Touch(Gesture::Tap(..)))
                if self.phone.ringing && call_choice(i).is_some() =>
            {
                let accept = call_choice(i) == Some(true);
                self.answer(b, accept);
                self.end_ring(b);
                self.say(if accept { "ANSWERED" } else { "DECLINED" });
                return;
            }
            // the phone: Lap held plays or pauses from a page; the media remote and an incoming call
            (Screen::Main | Screen::Ride(_), Input::Media) => return self.media(b, Media::PlayPause, false),
            // MORE: a swipe up on a ride page shows its denser variant, a swipe down or Back the page again
            (Screen::Ride(n), Input::Touch(Gesture::Swipe(Dir::Up))) if n < PAGES && self.more_shown().is_none() => {
                self.more_on = Some(n)
            }
            (Screen::Ride(_), Input::Back | Input::Touch(Gesture::Swipe(Dir::Down))) if self.more_shown().is_some() => {
                self.more_on = None
            }
            // a swipe down on the overview before a ride opens the quick panel; on a page (or the overview of a ride)
            // it opens the remote, without the menus
            (Screen::Main, Input::Touch(Gesture::Swipe(Dir::Down))) if self.ride.state() == ride::State::Idle => {
                self.open(Screen::Menu(Menu::Quick, 0))
            }
            (Screen::Main | Screen::Ride(_), Input::Touch(Gesture::Swipe(Dir::Down))) => self.open(Screen::Media),
            // Back, or a swipe up (the swipe down that opened it, undone)
            (Screen::Media, Input::Back | Input::Touch(Gesture::Swipe(Dir::Up))) => self.close(),
            (Screen::Media, Input::Touch(Gesture::Tap(x, y))) => {
                use ui::media::Button;
                let (key, photo) = match ui::media::hit(x, y) {
                    Some(Button::Prev) => (Media::Prev, false),
                    Some(Button::Play) => (Media::PlayPause, false),
                    Some(Button::Next) => (Media::Next, false),
                    Some(Button::VolDown) => (Media::VolDown, false),
                    Some(Button::VolUp) => (Media::VolUp, false),
                    Some(Button::SkipBack) => (Media::SkipBack, false),
                    Some(Button::SkipFwd) => (Media::SkipFwd, false),
                    None => return,
                };
                return self.media(b, key, photo);
            }
            (Screen::Media, _) => {
                let key = match i {
                    Input::Select => Media::PlayPause,
                    Input::Next | Input::Touch(Gesture::Swipe(Dir::Left)) => Media::Next,
                    Input::Prev | Input::Touch(Gesture::Swipe(Dir::Right)) => Media::Prev,
                    Input::NextHeld => Media::VolDown,
                    Input::PrevHeld => Media::VolUp,
                    Input::Touch(Gesture::Hold(..)) => return self.media(b, Media::VolUp, true),
                    _ => return,
                };
                self.repeated |= matches!(i, Input::NextHeld | Input::PrevHeld);
                return self.media(b, key, false);
            }
            // a tap on the overview's notices opens their list, even while recording (it only reads; the touch lock
            // returns before this)
            (Screen::Main, Input::Touch(Gesture::Tap(_, y))) if u32::from(y) >= ui::NOTICES_Y => {
                self.open(Screen::Menu(Menu::Notifs, 0))
            }
            (Screen::Notif(k, top), _) => {
                let top = match i {
                    Input::Next | Input::Touch(Gesture::Swipe(Dir::Up)) => top + 1,
                    Input::Prev | Input::Touch(Gesture::Swipe(Dir::Down)) => top.saturating_sub(1),
                    Input::Back | Input::Touch(Gesture::Swipe(Dir::Right)) => return self.leave_notif(b),
                    Input::Touch(Gesture::Tap(x, y)) if inside(ui::BACK_TAP, x, y) => return self.leave_notif(b),
                    _ => return,
                };
                let top = top.min(self.notif_lines(k).saturating_sub(ui::NOTIF_LINES));
                if self.screen != Screen::Notif(k, top) {
                    self.screen = Screen::Notif(k, top);
                    self.show(b);
                }
                return;
            }
            // APPLY and KEEP are touch buttons; Back (or a swipe right) keeps the FTP set; Start and Lap are the ride's
            (Screen::RampResult, Input::Touch(Gesture::Tap(x, y))) if self.ramp_tap(b, x, y) => {}
            (Screen::RampResult, Input::Back | Input::Touch(Gesture::Swipe(Dir::Right))) => self.close(),
            (Screen::RampResult, Input::Select) => return self.start_key(b),
            (Screen::RampResult, Input::Prev) => return self.lap(),
            (Screen::RampResult, _) => return,
            (Screen::PowerCal, Input::Back | Input::Touch(Gesture::Swipe(Dir::Right))) => self.powercal_leave(),
            (Screen::PowerCal, Input::Touch(Gesture::Tap(x, y))) if inside(ui::BACK_TAP, x, y) => self.powercal_leave(),
            (Screen::PowerCal, _) => return self.powercal_input(i),
            (Screen::Trainer, Input::Back | Input::Touch(Gesture::Swipe(Dir::Right))) => {
                self.trainer.cancel_calibration(); // Back gives up a calibration, then leaves
                self.close()
            }
            (Screen::Trainer, Input::Touch(Gesture::Tap(x, y))) if inside(ui::BACK_TAP, x, y) => {
                self.trainer.cancel_calibration(); // Back gives up a calibration, then leaves
                self.close()
            }
            // riding, Start and Lap are the ride's (pause, lap) as on any ride page: a key meant for the ride must
            // not change the trainer's mode; the touch buttons change the mode and the target down
            (Screen::Trainer, Input::Select) if self.ride.state() != ride::State::Idle => return self.start_key(b),
            (Screen::Trainer, Input::Prev) if self.ride.state() != ride::State::Idle => return self.lap(),
            (Screen::Trainer, Input::PrevHeld) if self.ride.state() != ride::State::Idle => return,
            (Screen::Trainer, Input::End) if self.ride.state() == ride::State::Paused => {
                self.open(Screen::End(ui::END_SAVE))
            }
            // riding, the long presses are the trainer's: Lap held steps the target down (Page is up), Start held
            // cycles the mode; the short ones stay a lap and a pause
            (Screen::Trainer, Input::Media | Input::End) if self.ride.state() == ride::State::Running => {
                return self.trainer_long(b, i);
            }
            (Screen::Trainer, _) => return self.trainer_input(b, i),
            (Screen::Edit(..), _) => return self.editor_input(b, i),
            (Screen::End(_) | Screen::Discard(_), _) => return self.end_input(b, i),
            (Screen::Menu(menu, sel), _) if self.editing => {
                let Some(menu::Row::Set(s)) = self.row(menu, sel) else {
                    self.editing = false;
                    return;
                };
                match i {
                    Input::Next | Input::Prev => self.settings.step(s, i == Input::Prev),
                    Input::Touch(Gesture::Swipe(d @ (Dir::Up | Dir::Down))) => self.settings.step(s, d == Dir::Up),
                    Input::NextHeld | Input::PrevHeld => {
                        self.repeated = true;
                        self.settings.step(s, i == Input::PrevHeld);
                    }
                    Input::Select | Input::Back | Input::Touch(Gesture::Tap(..) | Gesture::Swipe(Dir::Right)) => {
                        self.editing = false;
                        // Back (and a swipe back) leaves the altitude as it was; Select or a tap sets it
                        if s == menu::Setting::Altitude && matches!(i, Input::Select | Input::Touch(Gesture::Tap(..))) {
                            self.alt_typed();
                        }
                        self.save_misc(b);
                    }
                    _ => {}
                }
                return self.update_menu_rows(b, menu, sel);
            }
            // the overview is apart from the ride pages, as on stock: Page goes to them, then round them; Back
            // comes back
            (Screen::Main | Screen::Ride(_), Input::Next | Input::Touch(Gesture::Swipe(Dir::Left))) => {
                (self.screen, self.more_on) = (self.paged(true), None) // the next page's normal variant
            }
            // a tap on the map's "+" or "-" zooms (only the map is drawn again, at its new view)
            (Screen::Ride(_), Input::Touch(Gesture::Tap(x, y))) if self.zoom_at(x, y).is_some() => {
                if let Some(z) = self.zoom_at(x, y) {
                    (self.map_zoom, self.map_centre) = (z.map(|z| z.level), z.and_then(|z| z.centre));
                }
                return self.update_page(b);
            }
            // a drag on the zoomed map pans it (App::touch makes it from the drag released; else it is a swipe)
            (Screen::Ride(_), Input::Touch(Gesture::Drag(_, _, dx, dy))) => {
                if let Some(c) = self.pan(dx, dy) {
                    self.map_centre = Some(c);
                    return self.update_page(b);
                }
                return;
            }
            // a tap on a TRAINER MODE / TARGET field opens the trainer screen, even while recording (the touch lock
            // above is what stops a stray touch)
            (Screen::Main | Screen::Ride(_), Input::Touch(Gesture::Tap(x, y))) if self.trainer_field_at(x, y) => {
                self.open(Screen::Trainer)
            }
            // while recording, touch on a page only turns pages and works the map: no menu by a hold or the gear, no
            // field changed by a tap (a wet glove, a drop of rain; the keys still do all of it)
            (Screen::Main | Screen::Ride(_), Input::Touch(Gesture::Tap(..) | Gesture::Hold(..))) if recording => return,
            // a tap on a graph shows the next plot, until the unit is switched off (the page as kept is not changed: a
            // stray touch never edits it for good); a hold switches between the last minutes and the whole ride
            (Screen::Main | Screen::Ride(_), Input::Touch(Gesture::Tap(..) | Gesture::Hold(..)))
                if graph_hit.is_some() =>
            {
                let Some((k, f)) = graph_hit else { return };
                if matches!(i, Input::Touch(Gesture::Hold(..))) {
                    self.graph_whole = !self.graph_whole;
                } else if let Some(plot) = graph_plot(f) {
                    let all = ride::Plot::ALL;
                    let next = all[(all.iter().position(|&p| p == plot).unwrap_or(0) + 1) % all.len()];
                    let n = match self.screen {
                        Screen::Ride(_) if self.more_shown().is_some() => return, // no slot, nothing to redraw
                        Screen::Ride(n) => n,
                        _ => OVERVIEW,
                    };
                    let fixed = self.widgets().take_while(|(_, w)| !matches!(w, Widget::Field(_))).count();
                    if let Some(page) = self.plot_taps.get_mut(n)
                        && let Some(slot) = page.get_mut(k.saturating_sub(fixed))
                    {
                        *slot = Some(graph_field(next)); // until power-off: the editor changes a page for good
                    }
                }
                return self.show(b);
            }
            (Screen::Ride(_), Input::Touch(Gesture::Swipe(Dir::Right))) => {
                (self.screen, self.more_on) = (self.paged(false), None)
            }
            (Screen::Ride(_), Input::Back) => (self.screen, self.more_on) = (Screen::Main, None),
            // the TODAY card: the calendar
            // today's workout: its steps with START selected; Start held does the same off a ride
            (Screen::Main, Input::Touch(Gesture::Tap(x, y))) if inside(ui::TODAY_BOX, x, y) => self.open_today(b),
            (Screen::Main, Input::End) if self.ride.state() == ride::State::Idle => self.open_today(b),
            (Screen::Main, Input::Touch(Gesture::Tap(x, y)))
                if (ui::GEAR_TAP.0..ui::GEAR_TAP.0 + ui::GEAR_TAP.2).contains(&u32::from(x))
                    && (ui::GEAR_TAP.1..ui::GEAR_TAP.1 + ui::GEAR_TAP.3).contains(&u32::from(y)) =>
            {
                self.open(Screen::Menu(self.menu_root(), 0)) // the gear: the menu by a tap, as the hold anywhere
            }
            // a hold on a ride page's box: the editor on it, that box selected (stock's long press; not while
            // recording, above)
            (Screen::Ride(n), Input::Touch(Gesture::Hold(x, y))) if n < PAGES => {
                self.pages_indoor = self.indoors(); // the set on screen
                let k = self.widgets().position(|(bx, _)| inside(bx, x, y)).unwrap_or(0);
                self.edit_page(b, n, k)
            }
            (Screen::Main | Screen::Ride(_), Input::Menu | Input::Touch(Gesture::Hold(..))) => {
                self.open(Screen::Menu(self.menu_root(), 0))
            }
            // the ride: Select starts, pauses, resumes; End on a paused ride asks how to end it (the end-ride screen);
            // Prev marks a lap (on the overview too)
            (Screen::Main | Screen::Ride(_), Input::Select) => return self.start_key(b),
            (Screen::Main | Screen::Ride(_), Input::End) if self.ride.state() == ride::State::Paused => {
                self.open(Screen::End(ui::END_SAVE))
            }
            // the ride's card stays until Back or Start: a key, a swipe or a raindrop does not throw it away
            (Screen::Summary, Input::Back | Input::Select) => self.screen = Screen::Main,
            (Screen::Summary, _) => return,
            (Screen::Main | Screen::Ride(_), Input::Prev) => return self.lap(),
            (Screen::Menu(menu, sel), Input::Next | Input::Prev | Input::NextHeld | Input::PrevHeld) => {
                self.repeated |= matches!(i, Input::NextHeld | Input::PrevHeld);
                let rows = self.rows(menu).max(1);
                let down = matches!(i, Input::Next | Input::NextHeld);
                // no wrap at the ends: a held key stops at the first or the last row
                let next = if down { (sel + 1).min(rows - 1) } else { sel.saturating_sub(1) };
                self.menu_chosen = true;
                self.screen = Screen::Menu(menu, next);
                if menu::first_shown(next, ui::MENU_ROWS) == menu::first_shown(sel, ui::MENU_ROWS) {
                    return self.update_menu_rows(b, menu, next);
                }
            }
            // the quick panel is one screen: the swipe down that opened it, undone
            (Screen::Menu(Menu::Quick, _), Input::Touch(Gesture::Swipe(Dir::Up))) => self.close(),
            (Screen::Menu(menu, sel), Input::Touch(Gesture::Swipe(d @ (Dir::Up | Dir::Down)))) => {
                // a screen of rows at a time: up shows the next, down the previous
                let (first, rows) = (menu::first_shown(sel, ui::MENU_ROWS), self.rows(menu));
                let next = if d == Dir::Up { first + ui::MENU_ROWS } else { first.saturating_sub(ui::MENU_ROWS) };
                if next >= rows || next == first {
                    return;
                }
                self.menu_chosen = false; // turned to, not chosen
                self.screen = Screen::Menu(menu, next);
            }
            // the "<" left of the title: back
            (Screen::Menu(..), Input::Touch(Gesture::Tap(x, y))) if inside(ui::BACK_TAP, x, y) => self.close(),
            // a tap acts on the row (as Start on it); while the ride is moving, a tap selects and a second one on the
            // row chosen acts (a wet glove or a bump must not do something)
            (Screen::Menu(menu, sel), Input::Touch(Gesture::Tap(_, y))) => {
                // the bottom rows under the last slot (316 to 319) are the last slot's, not the next screen's
                let slot = (u32::from(y).checked_sub(ui::LIST_Y))
                    .map(|d| ((d / ui::MENU_ROW) as usize).min(ui::MENU_ROWS - 1));
                let row = match slot.map(|k| menu::first_shown(sel, ui::MENU_ROWS) + k) {
                    Some(row) if self.row(menu, row).is_some() => row,
                    _ => return self.update_menu_rows(b, menu, sel), // a row's "SURE?" goes
                };
                if self.ride.moving() && (row != sel || !self.menu_chosen) {
                    self.menu_chosen = true;
                    self.screen = Screen::Menu(menu, row);
                    return self.update_menu_rows(b, menu, row);
                }
                self.menu_chosen = true;
                return self.select_row(b, menu, row, armed.filter(|&a| a == (menu, row)));
            }
            (Screen::Menu(menu, sel), Input::Select) => return self.select_row(b, menu, sel, armed),
            // a past ride's card: Start dismisses it, as the ride summary. DELETE asks (a tap on its label, or Start
            // held), and the answer is Start, a tap on it again or Start held again; Back says no (moved, kept:
            // trash.rs)
            (Screen::Past, Input::Select) if armed != Some(PAST_DELETE) => self.close(),
            (Screen::Past, Input::Select | Input::End) => {
                self.confirm = armed;
                self.delete_past(b);
            }
            (Screen::Past, Input::Touch(Gesture::Tap(x, y))) if inside(ui::card::DELETE_TAP, x, y) => {
                self.confirm = armed;
                self.delete_past(b);
            }
            (
                Screen::Menu(..) | Screen::Nearby | Screen::Past | Screen::About,
                Input::Back | Input::Touch(Gesture::Swipe(Dir::Right)),
            ) => self.close(),
            (Screen::Nearby, Input::Touch(Gesture::Tap(x, y))) if inside(ui::BACK_TAP, x, y) => self.close(),
            _ => return,
        }
        if self.screen != before || self.more_on != more_before {
            self.show(b);
        }
    }

    /// The touch panel, called by the main loop as soon as INT falls and then every [`TOUCH_MS`] while a finger is
    /// down ([`App::touching`]); `dt_ms` since the previous call. Reads the point only if INT is low or a finger was
    /// down (the release makes the gesture). A touch keeps the backlight on like a key. The gesture, if one ended
    /// here, comes back as an input for the caller to act on once it has stopped sleeping ([`App::input`]).
    pub fn touch(&mut self, b: &mut impl Board, dt_ms: u32) -> Option<Input> {
        let i = self.touch_gesture(b, dt_ms);
        // a drag that starts on the map zoomed in pans it, whatever gesture it made (a swipe, or one too short)
        match self.touch.take_drag() {
            // a gloved tap that slides a little on a button is that button's tap, not a pan (review of 6de15ed)
            Some(((x, y), (dx, dy)))
                if self.pans_at(x, y) && dx.abs() < 24 && dy.abs() < 24 && self.zoom_at(x, y).is_some() =>
            {
                Some(Input::Touch(Gesture::Tap(x, y)))
            }
            Some(((x, y), (dx, dy))) if self.pans_at(x, y) => Some(Input::Touch(Gesture::Drag(x, y, dx, dy))),
            _ => i,
        }
    }

    /// The gesture waiting, or the one this sample ends ([`App::touch`]).
    fn touch_gesture(&mut self, b: &mut impl Board, dt_ms: u32) -> Option<Input> {
        if let Some(i) = self.touch_pending.take() {
            self.idle_ms = 0;
            return Some(i);
        }
        // after a drive command sampled the panel, the sleep since the main loop's last read is time counted already:
        // charged again, a tap still down at the command's end was a hold (review)
        let dt_ms = if core::mem::take(&mut self.touch_sampled) { 0 } else { dt_ms };
        let (g, finger) = sample_touch(b, &mut self.touch, self.touch_ok, false, dt_ms);
        if self.strip.locked {
            return None; // locked: a finger neither acts nor keeps the light on
        }
        if finger {
            self.idle_ms = 0;
        }
        g.map(Input::Touch)
    }

    /// Start (or a tap on the chosen row) on row `sel` of `menu`: it acts, then the rows that changed are drawn, or
    /// the screen it opened.
    fn select_row(&mut self, b: &mut impl Board, menu: Menu, sel: usize, armed: Option<(Menu, usize)>) {
        let shown = Screen::Menu(menu, sel);
        self.confirm = armed; // a second Start on a row waiting to be confirmed confirms it
        self.screen = shown;
        self.activate(b, menu, sel);
        if self.screen == shown {
            self.update_menu_rows(b, menu, sel);
        } else {
            self.show(b);
        }
    }

    /// A finger is on the panel (sample it every [`TOUCH_MS`]), or a gesture waits.
    pub fn touching(&self) -> bool {
        self.touch.down() || self.touch_pending.is_some()
    }
}

/// One sample of the touch panel into `r`, `dt_ms` after the previous one, if INT is low, a finger was down or
/// `always`: the gesture that ended, and whether a finger is on the panel.
pub(crate) fn sample_touch(
    b: &mut impl Board,
    r: &mut touch::Recogniser,
    ok: bool,
    always: bool,
    dt_ms: u32,
) -> (Option<Gesture>, bool) {
    if !ok || !(always || r.down() || b.touch_int()) {
        return (None, false);
    }
    let mut buf = [0; 3];
    let p = if b.touch_read(touch::REG_POINT, &mut buf) { touch::point(buf) } else { None };
    (r.feed(p, dt_ms), p.is_some())
}

/// (`x`, `y`) is in box (left, top, width, height).
pub(crate) fn inside((bx, by, w, h): (u32, u32, u32, u32), x: u16, y: u16) -> bool {
    (bx..bx + w).contains(&u32::from(x)) && (by..by + h).contains(&u32::from(y))
}

/// What an input does to an incoming call: Some(true) answers (Start, the green button), Some(false) declines (Back,
/// the red one); None for a tap elsewhere.
fn call_choice(i: Input) -> Option<bool> {
    match i {
        Input::Select => Some(true),
        Input::Back => Some(false),
        Input::Touch(Gesture::Tap(x, y)) => {
            let [red, green] = ui::CALL_BUTTONS;
            if inside(green, x, y) {
                Some(true)
            } else if inside(red, x, y) {
                Some(false)
            } else {
                None
            }
        }
        _ => None,
    }
}
