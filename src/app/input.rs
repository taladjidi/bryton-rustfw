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
}

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
}

impl Keys {
    /// The keys down at the start (`mask`): ignored until each is released.
    pub fn at_start(mask: u8) -> Self {
        Keys { stuck: mask, ..Keys::default() }
    }

    /// One poll, `dt_ms` after the previous one, with the keys held down (`mask` of [`Key`] codes). One press at
    /// most per poll, the first key in `Key::ALL` order.
    pub fn poll(&mut self, mask: u8, dt_ms: u32) -> Option<Press> {
        self.stuck &= mask;
        let mask = mask & !self.stuck;
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
            press = press.or(p);
        }
        press
    }
}

/// What is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The main screen: the time, big, and Ferris.
    Main,
    /// Ride page `n`.
    Ride(usize),
    /// A menu, with the selected row.
    Menu(Menu, usize),
    /// The BLE devices heard.
    Nearby,
    /// The card of the ride just ended: its map, elevation profile and numbers.
    Summary,
    /// The card of a past ride (`App::past`); Back returns to the history.
    Past,
    /// The phone's media remote (app/phone.rs).
    Media,
    /// ABOUT: the version, the commit, Ferris.
    About,
}

/// Places in the round of the ride pages: the pages, then the course page.
const ROUND: usize = PAGES + 1;

/// Ride page `n`'s place in the round.
fn place(n: usize) -> usize {
    if n == COURSE { PAGES } else { n }
}

/// The ride page at place `k` in the round.
fn page_at(k: usize) -> usize {
    if k == PAGES { COURSE } else { k }
}

/// During a ride, holding Back a second time within this long switches off (the first hold only warns).
pub const OFF_CONFIRM_MS: u32 = 10_000;

impl App {
    /// The first ride page that is on from place `from` in the round (taken round), going forward by `step` (1, or
    /// [`ROUND`] - 1 for backward): the ride pages, then the course page while a course is followed; page 1 is never
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

    /// Ride page `n` is in the round: a page not off, the course page while a course is followed.
    pub(crate) fn page_on(&self, n: usize) -> bool {
        // a course followed, whether or not the map can draw it yet (no fix, a ride started without one: review)
        if n == COURSE {
            self.courses.as_deref().is_some_and(|c| c.on.is_some())
        } else {
            self.pages.get(n).is_some_and(|p| n < PAGES && p.layout > 0)
        }
    }

    /// Opens screen `s` over this one: Back returns here.
    pub(crate) fn open(&mut self, s: Screen) {
        if let Some(u) = self.under.get_mut(self.depth) {
            *u = self.screen;
            self.depth += 1;
        }
        self.screen = s;
    }

    /// Back to the screen under this one (a ride page switched off since shows the main screen).
    pub(crate) fn close(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        self.screen = match self.under.get(self.depth) {
            Some(&Screen::Ride(n)) if !self.page_on(n) => Screen::Main,
            Some(&s) if self.depth > 0 || matches!(s, Screen::Main | Screen::Ride(_)) => s,
            _ => Screen::Main,
        };
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
    /// toggles the backlight; in the menus Next / Prev move down / up, Select opens or changes, Back goes out; a
    /// number being edited goes up with Prev, down with Next. Off switches off anywhere.
    pub fn input(&mut self, b: &mut impl Board, i: Input) {
        let before = self.screen;
        let woke = self.light_shown == 0 && self.backlight;
        self.idle_ms = 0;
        if woke {
            self.update_light(b); // an input after the time-out: the light comes back (and the input acts)
            if i == Input::Back {
                return; // Back's own job is the light: done
            }
        }
        if self.banner_ms.take().is_some() {
            self.unbanner(b); // and the input acts
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
                return self.say("HOLD AGAIN: OFF");
            }
            (_, Input::Off) => return self.power_off(b),
            // the phone: Lap held plays or pauses from a page; the media remote and an incoming call
            (Screen::Main | Screen::Ride(_), Input::Media) => return self.media(b, Media::PlayPause, false),
            // a swipe down on a page opens the remote, without the menus
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
                        self.save_misc(b);
                    }
                    _ => {}
                }
                return self.update_menu_rows(b, menu, sel);
            }
            // the overview is apart from the ride pages, as on stock: Page goes to them, then round them; Back
            // comes back
            (Screen::Main | Screen::Ride(_), Input::Next | Input::Touch(Gesture::Swipe(Dir::Left))) => {
                self.screen = self.paged(true)
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
            // a tap on a graph shows the next plot (kept with the page), a hold switches between the last minutes and
            // the whole ride
            (Screen::Main | Screen::Ride(_), Input::Touch(Gesture::Tap(..) | Gesture::Hold(..)))
                if graph_hit.is_some() =>
            {
                let Some((k, f)) = graph_hit else { return };
                if matches!(i, Input::Touch(Gesture::Hold(..))) {
                    self.graph_whole = !self.graph_whole;
                } else if let Some(plot) = graph_plot(f) {
                    let all = ride::Plot::ALL;
                    let next = all[(all.iter().position(|&p| p == plot).unwrap_or(0) + 1) % all.len()];
                    let n = if let Screen::Ride(n) = self.screen { n } else { OVERVIEW };
                    let fixed = self.widgets().take_while(|(_, w)| !matches!(w, Widget::Field(_))).count();
                    if let Some(page) = self.pages.get_mut(n)
                        && let Some(slot) = page.fields.get_mut(k.saturating_sub(fixed))
                    {
                        *slot = graph_field(next);
                        save_page(b, n, page);
                    }
                }
                return self.show(b);
            }
            (Screen::Ride(_), Input::Touch(Gesture::Swipe(Dir::Right))) => self.screen = self.paged(false),
            (Screen::Ride(_), Input::Back) => self.screen = Screen::Main,
            (Screen::Main, Input::Touch(Gesture::Tap(x, y)))
                if (ui::GEAR_TAP.0..ui::GEAR_TAP.0 + ui::GEAR_TAP.2).contains(&u32::from(x))
                    && (ui::GEAR_TAP.1..ui::GEAR_TAP.1 + ui::GEAR_TAP.3).contains(&u32::from(y)) =>
            {
                self.open(Screen::Menu(Menu::Main, 0)) // the gear: the menu by a tap, as the hold anywhere
            }
            (Screen::Main | Screen::Ride(_), Input::Menu | Input::Touch(Gesture::Hold(..))) => {
                self.open(Screen::Menu(Menu::Main, 0))
            }
            // the ride: Select starts, pauses, resumes; End on a paused ride ends it; Prev marks a lap
            (Screen::Main | Screen::Ride(_), Input::Select) => return self.start_key(b),
            (Screen::Main | Screen::Ride(_), Input::End) => {
                let Some(ev) = self.ride.end() else { return };
                self.recorder(ev);
                self.player = None; // the workout ends with the ride
                self.screen = Screen::Summary; // the ride's card; any key goes on to the overview
            }
            (Screen::Summary, _) => self.screen = Screen::Main,
            (Screen::Ride(_), Input::Prev) => return self.lap(),
            (Screen::Main, Input::Back) => {
                self.backlight = !self.backlight;
                self.update_light(b);
            }
            (Screen::Menu(menu, sel), Input::Next | Input::Prev | Input::NextHeld | Input::PrevHeld) => {
                self.repeated |= matches!(i, Input::NextHeld | Input::PrevHeld);
                let rows = self.rows(menu).max(1);
                let down = matches!(i, Input::Next | Input::NextHeld);
                let next = if down { (sel + 1) % rows } else { (sel + rows - 1) % rows };
                self.screen = Screen::Menu(menu, next);
                if menu::first_shown(next, ui::MENU_ROWS) == menu::first_shown(sel, ui::MENU_ROWS) {
                    return self.update_menu_rows(b, menu, next);
                }
            }
            (Screen::Menu(menu, sel), Input::Touch(Gesture::Swipe(d @ (Dir::Up | Dir::Down)))) => {
                // a screen of rows at a time: up shows the next, down the previous
                let (first, rows) = (menu::first_shown(sel, ui::MENU_ROWS), self.rows(menu));
                let next = if d == Dir::Up { first + ui::MENU_ROWS } else { first.saturating_sub(ui::MENU_ROWS) };
                if next >= rows || next == first {
                    return;
                }
                self.screen = Screen::Menu(menu, next);
            }
            (Screen::Menu(menu, sel), Input::Select | Input::Touch(Gesture::Tap(..))) => {
                let sel = match i {
                    Input::Touch(Gesture::Tap(_, y)) => {
                        let slot = (u32::from(y).checked_sub(ui::LIST_Y)).map(|d| (d / ui::LIST_ROW) as usize);
                        match slot.map(|k| menu::first_shown(sel, ui::MENU_ROWS) + k) {
                            Some(row) if self.row(menu, row).is_some() => row,
                            _ => return,
                        }
                    }
                    _ => sel,
                };
                self.screen = Screen::Menu(menu, sel);
                let shown = self.screen;
                self.activate(b, menu, sel);
                if self.screen == shown {
                    return self.update_menu_rows(b, menu, sel);
                }
            }
            (
                Screen::Menu(..) | Screen::Nearby | Screen::Past | Screen::About,
                Input::Back | Input::Touch(Gesture::Swipe(Dir::Right)),
            ) => self.close(),
            _ => return,
        }
        if self.screen != before {
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
        if finger {
            self.idle_ms = 0;
        }
        g.map(Input::Touch)
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

/// What an input does to an incoming call: Some(true) answers (Start, the green button), Some(false) declines (Back,
/// the red one); None for a tap elsewhere.
fn call_choice(i: Input) -> Option<bool> {
    let inside = |(bx, by, w, h): (u32, u32, u32, u32), x: u16, y: u16| {
        (bx..bx + w).contains(&u32::from(x)) && (by..by + h).contains(&u32::from(y))
    };
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
