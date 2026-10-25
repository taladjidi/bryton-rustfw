//! The phone's notifications kept for reading: the last few ([`ph::Inbox`]) come from the ANCS fetch that makes the
//! banner ([`App::keep_notif`]), the NOTIFICATIONS list is a menu ([`Menu::Notifs`]: a tap on the overview's
//! notices, or its row at the end of the PHONE menu), and one notification is shown in full on [`Screen::Notif`],
//! its text wrapped over lines. All of it is drawn and acted on when the screen is open; the ride loop does
//! nothing for it, and nothing here touches the recorder.

use crate::*;
use phone as ph;

impl App {
    /// Keeps a notification fetched from the phone. A detail open on a newer one moves with it (the list is newest
    /// first), so it keeps showing the same notification.
    pub(crate) fn keep_notif(&mut self, a: &ph::Attributes) {
        self.phone.inbox.add(a, self.strip.time);
        if let Screen::Notif(k, top) = self.screen {
            self.screen = Screen::Notif(k + 1, top);
        }
    }

    /// Notification `k` as lines: its title (true), then its message wrapped to the screen (false).
    pub(crate) fn notif_text(&self, k: usize) -> impl Iterator<Item = (&str, bool)> {
        let n = self.phone.inbox.get(k);
        let cols = ui::notif_cols();
        let (t, m) = n.map_or(("", ""), |n| (n.title.as_str(), n.message.as_str()));
        ph::wrap(t, cols).map(|l| (l, true)).chain(ph::wrap(m, cols).map(|l| (l, false)))
    }

    /// How many lines notification `k` takes.
    pub(crate) fn notif_lines(&self, k: usize) -> usize {
        self.notif_text(k).count()
    }

    /// Notification `k` drawn whole, from line `top`.
    pub(crate) fn draw_notif(&self, c: &mut impl Canvas, k: usize, top: usize) {
        let Some(n) = self.phone.inbox.get(k) else { return ui::draw_title(c, "GONE") };
        let more = self.notif_lines(k) > top + ui::NOTIF_LINES;
        ui::draw_notif(c, n.app.as_str(), hm_text(n.time).as_str(), more, self.notif_text(k).skip(top));
    }

    /// Back from a detail to the list.
    pub(crate) fn leave_notif(&mut self, b: &mut impl Board) {
        self.close();
        self.show(b);
    }
}
