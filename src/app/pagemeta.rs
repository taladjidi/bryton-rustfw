//! The rider's page switches (roadmap 3b): ride pages switched OFF, moved up or down, renamed, and the auto scroll.
//!
//! The road set only has the switches and names ([`page::Meta`], kept in `\Rustfw\pages.bin` after the MTB part: the
//! sector has room for four characters a name); MOVE UP / DOWN work on every set. A move swaps two pages in place
//! (with their MORE variants, switch and name), so the order is the pages' own and nothing else keeps it. All of it
//! is written the way the pages are ([`App::pages_changed`]): between rides.
//!
//! AUTO SCROLL (OFF, 5, 10, 20 s): during a ride the next page ON comes by itself, among the ride pages only; it waits
//! on the overview, a menu, a banner, the CLIMB page, the course and workout pages, and any key or touch starts the
//! count again.

use crate::*;
use page::{ALPHABET, AUTO_SECS, Meta, NAME_CHARS};

/// The switches' state: what pages.bin keeps, the name being typed, the auto scroll's count.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Switches {
    /// Kept in pages.bin.
    pub(crate) meta: Meta,
    /// RENAME typing: the page and the character under the cursor.
    pub(crate) rename: Option<(usize, usize)>,
    /// Milliseconds on the page shown, toward the next auto scroll.
    pub(crate) auto_ms: u32,
}

impl Switches {
    /// Every page ON, nothing typed.
    pub(crate) const fn new() -> Self {
        Switches { meta: Meta::DEFAULT, rename: None, auto_ms: 0 }
    }
}

impl App {
    /// The auto scroll step in seconds (0: off).
    pub(crate) fn auto_secs(&self) -> u32 {
        AUTO_SECS.get(usize::from(self.sw.meta.auto)).copied().map_or(0, u32::from)
    }

    /// Page `n`'s label in the menus: its name, else "PAGE n".
    pub(crate) fn page_label(&self, n: usize, l: &mut Line<16>) {
        let name = self.sw.meta.name(n);
        if name.is_empty() || n >= PAGES || self.view_set() != ride::Profile::Road {
            l.push_str("PAGE ");
            l.push_u32(n as u32 + 1);
        } else {
            l.push_str(name);
        }
    }

    /// The switch rows of page `n`'s menu, after its fields: SHOW and RENAME on the road set, MOVE UP and DOWN where
    /// the page has somewhere to go.
    pub(crate) fn page_ops(&self, n: usize) -> impl Iterator<Item = menu::PageOp> + use<> {
        let (road, count) = (self.view_set() == ride::Profile::Road, self.view_count());
        use menu::PageOp::*;
        [(Show, road), (Up, n > 0), (Down, n + 1 < count), (Rename, road)].into_iter().filter(|r| r.1).map(|r| r.0)
    }

    /// The road page shown ON? (The indoor and MTB sets have no switches: every page is.)
    pub(crate) fn switched_on(&self, n: usize) -> bool {
        self.ride_profile().page_set() != ride::Profile::Road || self.sw.meta.on(n)
    }

    /// A switch, name or order changed: written with the pages.
    fn switches_changed(&mut self, b: &mut impl Board) {
        self.keep_one_on();
        self.pages_changed(b);
    }

    /// At least one road page stays ON.
    fn keep_one_on(&mut self) {
        let count = self.page_count;
        self.sw.meta.keep_one(count);
    }

    /// The menu's SHOW on page `n`: ON or OFF; the last page ON stays.
    pub(crate) fn toggle_page(&mut self, b: &mut impl Board, n: usize) {
        if !self.pages_ready(b) {
            return;
        }
        let others = (0..self.page_count).any(|k| k != n && self.sw.meta.on(k));
        if self.sw.meta.on(n) && !others {
            return self.say("ONE PAGE AT LEAST");
        }
        if n < PAGES {
            self.sw.meta.off ^= 1 << n;
            self.switches_changed(b);
        }
    }

    /// The menu's MOVE UP or MOVE DOWN on page `n` of the set shown; the menu follows the page.
    pub(crate) fn move_page(&mut self, b: &mut impl Board, n: usize, up: bool) {
        if !self.pages_ready(b) {
            return;
        }
        let which = self.view_set();
        let m = if up { n.wrapping_sub(1) } else { n + 1 };
        let (pages, count, more) = self.set_mut(which);
        if n >= *count || m >= *count {
            return;
        }
        pages.swap(n, m);
        more.swap(n, m);
        if which == ride::Profile::Road {
            self.sw.meta.swap(n, m);
        }
        self.switches_changed(b);
        // the menu is the moved page's now, and its list selects it
        if let Screen::Menu(Menu::Page(_), _) = self.screen {
            // the same op's row if the moved page still has it, else the other direction, else SHOW
            let want = if up { [menu::PageOp::Up, menu::PageOp::Down] } else { [menu::PageOp::Down, menu::PageOp::Up] };
            let at = want.into_iter().chain([menu::PageOp::Show]).find_map(|o| {
                (0..self.rows(Menu::Page(m))).find(|&r| self.row(Menu::Page(m), r) == Some(menu::Row::PageOp(m, o)))
            });
            self.screen = Screen::Menu(Menu::Page(m), at.unwrap_or(0));
        }
        if let Some(Screen::Menu(l @ Menu::PageList(_), _)) =
            self.depth.checked_sub(1).and_then(|d| self.under.get(d)).copied()
            && let Some(u) = self.depth.checked_sub(1).and_then(|d| self.under.get_mut(d))
        {
            *u = Screen::Menu(l, m + 1);
        }
        self.clamp_menu(); // the moved page has other rows (no MOVE UP at the top)
        self.show(b);
    }

    /// The menu's AUTO SCROLL: the next step.
    pub(crate) fn cycle_auto(&mut self, b: &mut impl Board) {
        if self.pages_ready(b) {
            self.sw.meta.auto = (self.sw.meta.auto + 1) % AUTO_SECS.len() as u8;
            self.pages_changed(b);
        }
    }

    /// The menu's RENAME on page `n`: typing starts at the first character (module doc).
    pub(crate) fn start_rename(&mut self, b: &mut impl Board, n: usize) {
        if n < PAGES && self.pages_ready(b) {
            self.sw.rename = Some((n, 0));
        }
    }

    /// An input while a name is typed: Next and Prev (the swipes up and down) change the character, Start (a tap)
    /// goes to the next one, the last or Back ends. True while still typing.
    pub(crate) fn rename_input(&mut self, b: &mut impl Board, i: Input) -> bool {
        let Some((n, at)) = self.sw.rename else { return false };
        let step = |c: u8, back: bool| {
            let k = ALPHABET.iter().position(|&a| a == c).unwrap_or(0);
            let len = ALPHABET.len();
            ALPHABET.get((k + if back { len - 1 } else { 1 }) % len).copied().unwrap_or(b' ')
        };
        let change = |app: &mut App, back: bool| {
            if let Some(c) = app.sw.meta.names.get_mut(n).and_then(|name| name.get_mut(at)) {
                *c = step(*c, back);
            }
        };
        match i {
            Input::Next | Input::NextHeld => change(self, false),
            Input::Prev | Input::PrevHeld => change(self, true),
            Input::Touch(Gesture::Swipe(Dir::Up)) => change(self, false),
            Input::Touch(Gesture::Swipe(Dir::Down)) => change(self, true),
            Input::Select | Input::Touch(Gesture::Tap(..)) if at + 1 < NAME_CHARS => self.sw.rename = Some((n, at + 1)),
            _ => {
                self.sw.rename = None;
                self.pages_changed(b);
                return false;
            }
        }
        self.repeated |= matches!(i, Input::NextHeld | Input::PrevHeld);
        true
    }

    /// The name of row `RENAME` as the menu shows it: the name, the character under the cursor in brackets while typing.
    pub(crate) fn rename_text(&self, n: usize, l: &mut Line<12>) {
        let name = self.sw.meta.names.get(n).copied().unwrap_or([b' '; NAME_CHARS]);
        let typing = self.sw.rename.filter(|r| r.0 == n).map(|r| r.1);
        for (k, &c) in name.iter().enumerate() {
            let ch = if c == b' ' && typing == Some(k) { '_' } else { char::from(c) };
            let mut buf = [0u8; 4];
            if typing == Some(k) {
                l.push_str("[");
            }
            l.push_str(ch.encode_utf8(&mut buf));
            if typing == Some(k) {
                l.push_str("]");
            }
        }
        if typing.is_none() && self.sw.meta.name(n).is_empty() {
            l.clear();
            l.push_str("-");
        }
    }

    /// The auto scroll's count: one pass of `dt` ms. During a ride, on a ride page, with nothing over it.
    pub(crate) fn auto_scroll(&mut self, b: &mut impl Board, dt: u32) {
        if self.screen == Screen::Status {
            // QUICK STATUS closes on its own after STATUS_MS without an input (every input zeroes the count)
            self.sw.auto_ms = self.sw.auto_ms.saturating_add(dt);
            if self.sw.auto_ms >= super::status::STATUS_MS {
                self.close_status(b);
            }
            return;
        }
        let secs = self.auto_secs();
        let idle = matches!(self.screen, Screen::Ride(n) if n < PAGES)
            && secs > 0
            && self.ride.state() == ride::State::Running
            && self.more_on.is_none()
            && self.notes.ms.is_none();
        if !idle {
            self.sw.auto_ms = 0;
            return;
        }
        self.sw.auto_ms = self.sw.auto_ms.saturating_add(dt);
        if self.sw.auto_ms < secs * 1000 {
            return;
        }
        self.sw.auto_ms = 0;
        // among the ride pages: after the last, the first one ON (not the course or the workout page)
        let next = match self.paged(true) {
            s @ Screen::Ride(m) if m < PAGES => s,
            _ => self.ride_page(1, 1),
        };
        if next != self.screen {
            self.screen = next;
            self.show(b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The roadmap's pages: OFF, order, names and the one ON, on the Sim (the menu and the auto scroll: sim/page_tests.rs).
    #[test]
    fn meta_follows_its_pages() {
        let mut m = Meta::DEFAULT;
        m.off = 0b000010; // page 1 OFF
        m.names[1] = *b"HILL";
        m.swap(1, 2);
        assert!(m.on(1) && !m.on(2) && m.name(2) == "HILL" && m.name(1).is_empty());
        m.insert(0);
        assert!(!m.on(3) && m.on(2) && m.name(3) == "HILL");
        m.remove(0);
        assert!(!m.on(2) && m.name(2) == "HILL");
        m.off = 0b111111;
        m.keep_one(6);
        assert!(m.on(0));
    }
}
