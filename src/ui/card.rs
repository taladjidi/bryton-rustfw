//! A ride's card: the ride at a glance once it has ended, and for each past ride (the history): its title, the map
//! of its track, its elevation profile and ten numbers. Drawn once, when shown. The live ride and a past ride both
//! give their numbers as [`Stats`], which [`Card::of`] writes the same way.

use super::{Canvas, LABEL, PAGE_Y, W};
use crate::map;
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

/// A ride's numbers, as a card shows them (None: "--").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// The start, local time: date, hours and minutes.
    pub start: ((u16, u8, u8), (u8, u8)),
    /// Moving time, s.
    pub timer_s: u32,
    /// Distance, m.
    pub dist_m: u32,
    /// Average speed, km/h x 10.
    pub avg_speed_x10: Option<u16>,
    /// Ascent, m.
    pub ascent_m: Option<u32>,
    /// Normalized power, average power, W.
    pub np: Option<u16>,
    /// See `np`.
    pub avg_power: Option<u16>,
    /// Average and maximum heart rate, bpm.
    pub avg_hr: Option<u16>,
    /// See `avg_hr`.
    pub max_hr: Option<u16>,
    /// Training stress score.
    pub tss: Option<u32>,
    /// Work, kJ.
    pub kj: Option<u32>,
}

/// A distance in km to 0.1 km, as a card and the history list show it (a card's value box holds 5 characters).
///
/// ```
/// let km = |m| { let mut l = bryton_rustfw::text::Line::<8>::new(); bryton_rustfw::ui::card::push_km(&mut l, m); l };
/// assert_eq!([km(0).as_str(), km(14_455).as_str(), km(123_456).as_str()], ["0.0", "14.4", "123.4"]);
/// ```
pub fn push_km<const N: usize>(l: &mut Line<N>, m: u32) {
    l.push_u32(m / 1000);
    l.push_str(".");
    l.push_u32(m % 1000 / 100);
}

impl Card {
    /// No title, no numbers.
    pub const EMPTY: Card = Card { title: Line::new(), stats: [("", Line::new()); STATS] };

    /// The card of a ride with these numbers: "23 OCT 09:12", then time, distance, speed, ascent, NP, power,
    /// heart rate, TSS, work.
    pub fn of(s: &Stats) -> Card {
        let mut card = Card::default();
        let ((_, mo, d), (h, mi)) = s.start;
        card.title.push_u32(u32::from(d));
        card.title.push_str(" ");
        card.title.push_str(crate::MONTHS.get(usize::from(mo).wrapping_sub(1)).copied().unwrap_or(""));
        card.title.push_str(" ");
        for k in [h / 10, h % 10, 10, mi / 10, mi % 10] {
            card.title.push_str(if k == 10 { ":" } else { super::DIGITS[usize::from(k % 10)] });
        }
        let num = |v: Option<u32>| {
            let mut l = Line::<10>::new();
            match v {
                Some(v) => l.push_u32(v),
                None => l.push_str("--"),
            }
            l
        };
        let mut time = Line::<10>::new();
        time.push_u32(s.timer_s / 3600);
        time.push_str(":");
        time.push_str(super::DIGITS[(s.timer_s / 600 % 6) as usize]);
        time.push_str(super::DIGITS[(s.timer_s / 60 % 10) as usize]);
        let mut km = Line::<10>::new();
        push_km(&mut km, s.dist_m);
        let mut speed = Line::<10>::new();
        match s.avg_speed_x10 {
            Some(v) => {
                speed.push_u32(u32::from(v / 10));
                speed.push_str(".");
                speed.push_u32(u32::from(v % 10));
            }
            None => speed.push_str("--"),
        }
        let w = |v: Option<u16>| num(v.map(u32::from));
        card.stats = [
            ("TIME", time),
            ("KM", km),
            ("AVG KM/H", speed),
            ("ASCENT M", num(s.ascent_m)),
            ("NP", w(s.np)),
            ("AVG W", w(s.avg_power)),
            ("AVG HR", w(s.avg_hr)),
            ("MAX HR", w(s.max_hr)),
            ("TSS", num(s.tss)),
            ("KJ", num(s.kj)),
        ];
        card
    }
}

/// Draws `card` with `track` in [`MAP`] as `view` shows it (no basemap): the title, the map, the profile, the
/// numbers, a line under the profile. `elev(i)` gives column `i` of the profile ([`ELEV`]'s width) as a height and
/// a colour.
pub fn draw(
    c: &mut impl Canvas,
    card: &Card,
    (track, view): (&map::Track, Option<map::View>),
    elev: &dyn Fn(u32) -> (u32, u16),
) {
    map::draw(c, map::area(MAP), track, view, None);
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
        draw(&mut f, &card, (&map::Track::new(), None), &|i| (i % 30, super::super::GREY));
    }
}
