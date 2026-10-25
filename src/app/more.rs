//! MORE: a second, denser layout for each ride page (the owner's decision, UI decisions). A swipe up on a ride page
//! shows its MORE variant, a swipe down or Back returns to the page, Page turns to the next page's normal variant.
//! "MORE" in the strip, where BLE and the GPS bars are, says which is up ([`ui::Strip::more`]).
//!
//! The variant is the same page model ([`page::Page`]): the rider's own when the editor made one (a hold on a box of
//! MORE edits it), else [`page::Page::more_of`] of the page as it is now. They are kept in `\Rustfw\pages.bin`
//! beside the pages (page.rs [`page::Saved`]) and follow their page when pages are added or deleted. Up is only
//! this state: the screen stays `Screen::Ride(n)`, so every rule about ride pages (the touch lock, recording,
//! the map, the menus) holds as it is.

use crate::*;
use page::Page;

impl App {
    /// The ride page whose MORE variant is on screen.
    pub(crate) fn more_shown(&self) -> Option<usize> {
        match (self.screen, self.more_on) {
            (Screen::Ride(n), Some(m)) if n == m && n < PAGES => Some(n),
            _ => None,
        }
    }

    /// Ride page `n`'s MORE variant: the rider's, else the page's default.
    pub(crate) fn more_page(&self, n: usize) -> Page {
        match (self.more.get(n).copied().flatten(), self.pages.get(n)) {
            (Some(m), _) => m,
            (None, Some(p)) => p.more_of(),
            (None, None) => BLANK,
        }
    }

    /// An editor's work kept: the page's, or its MORE variant.
    pub(crate) fn keep_edit(&mut self, e: &Editing) {
        if e.more {
            if let Some(m) = self.more.get_mut(e.page) {
                *m = Some(e.work);
            }
        } else if let Some(p) = self.pages.get_mut(e.page) {
            *p = e.work;
        }
    }
}
