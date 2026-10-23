//! A ride's card: the ride at a glance once it has ended (and, later, for each ride kept): its title, the map of its
//! track, its elevation profile and ten numbers. Drawn once, when shown: nothing on it changes.

use super::{Canvas, LABEL, PAGE_Y, W};
use crate::text::{self, Align, Line, Style};

/// The map of the track: under the title, the full width.
pub const MAP: (u32, u32, u32, u32) = (0, PAGE_Y + 22, W, 110);
/// The elevation profile, under the map.
pub const ELEV: (u32, u32, u32, u32) = (4, MAP.1 + MAP.3 + 2, W - 8, 34);
/// The numbers: [`STATS`] in two columns of rows this high, under the profile.
const STATS_Y: u32 = ELEV.1 + ELEV.3 + 4;
const STAT_H: u32 = 20;
/// Numbers on a card.
pub const STATS: usize = 10;

/// What a card shows besides its map and profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Card {
    /// "23 OCT 09:12".
    pub title: Line<20>,
    /// Label and value, in reading order (left column, then right, row by row).
    pub stats: [(&'static str, Line<10>); STATS],
}

/// Draws `card` but its map (the caller draws the track in [`MAP`]) and its profile: the title, the numbers, a
/// line under the profile. `elev(i)` gives column `i` of the profile ([`ELEV`]'s width) as a height and a colour.
pub fn draw(c: &mut impl Canvas, card: &Card, elev: &dyn Fn(u32) -> (u32, u16)) {
    let grey = Style { fg: super::GREY, ..LABEL };
    text::draw_in(c, &grey, 0, PAGE_Y + 2, W, Align::Centre, card.title.as_str());
    let (x, y, w, h) = ELEV;
    super::draw_graph(c, (x, y, w, h), elev);
    c.fill(x, y + h, w, 1, super::DARK);
    let small = Style { scale: 1, ..grey };
    for (i, (label, value)) in card.stats.iter().enumerate() {
        let (col, row) = ((i % 2) as u32, (i / 2) as u32);
        let (cx, cy) = (col * W / 2 + 6, STATS_Y + row * STAT_H);
        let cw = W / 2 - 12;
        text::draw_in(c, &small, cx, cy + 6, cw / 2, Align::Left, label);
        text::draw_in(c, &LABEL, cx + cw / 2, cy, cw - cw / 2, Align::Right, value.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::super::H;
    use super::*;

    #[test]
    fn fits_the_screen() {
        assert!(STATS_Y + (STATS as u32).div_ceil(2) * STAT_H <= H);
        let mut f = super::super::tests::Frame::new();
        let mut card = Card::default();
        card.title.push_str("23 OCT 09:12");
        for (label, value) in &mut card.stats {
            *label = "AVG KM/H";
            value.push_str("1:32:10");
        }
        draw(&mut f, &card, &|i| (i % 30, super::super::GREY));
    }
}
