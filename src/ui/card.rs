//! A ride's card: the ride at a glance once it has ended, and for each past ride (the history): its title, the map
//! of its track, its elevation profile and ten numbers. Drawn once, when shown. The live ride and a past ride both
//! give their numbers as [`Stats`], which [`Card::of`] writes the same way.

use super::{Canvas, H, LABEL, PAGE_Y, W};
use crate::map;
use crate::text::{self, Align, Line, Style};

/// The map of the track: under the title, the full width, as tall as the rest leaves (130).
pub const MAP: (u32, u32, u32, u32) = (0, PAGE_Y + 22, W, 130);
/// The elevation profile, under the map.
pub const ELEV: (u32, u32, u32, u32) = (4, MAP.1 + MAP.3 + 2, W - 8, 34);
/// The numbers: [`STATS`] in two columns of rows this high, under the profile.
const STATS_Y: u32 = ELEV.1 + ELEV.3 + 4;
const STAT_H: u32 = 20;
/// A stat value's box: five characters at 2x ("109.3", "9999.").
const VALUE_W: u32 = 60;
/// Numbers on a card.
pub const STATS: usize = 10;

/// The touch box of a past ride's DELETE (44 px tall for a glove, over the map's top corner: a card has no other touch).
pub const DELETE_TAP: (u32, u32, u32, u32) = (W - 88, PAGE_Y, 88, 44);

/// "DELETE" in red at the right end of a past ride's title line; a tap in [`DELETE_TAP`] (or Start held) asks.
pub fn draw_delete(c: &mut impl Canvas) {
    let red = Style { fg: super::RED, scale: 1, ..LABEL };
    text::draw_in(c, &red, W - 44, PAGE_Y + 8, 42, Align::Right, "DELETE");
}

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

/// The touch box of LAPS, at the left end of a past ride's title line (as [`DELETE_TAP`], over the map's top corner).
pub const LAPS_TAP: (u32, u32, u32, u32) = (0, PAGE_Y, 64, 44);

/// "LAPS" at the left end of a past ride's title line; a tap in [`LAPS_TAP`] (or Page) opens the list of its laps.
pub fn draw_laps_label(c: &mut impl Canvas) {
    let white = Style { scale: 1, ..LABEL };
    // its own width: a wider box cleared the title's first digit
    text::draw_in(c, &white, 4, PAGE_Y + 8, white.width("LAPS"), Align::Left, "LAPS");
}

/// Laps a list screen shows at once: rows of 20 px under the title and a column header.
pub const LAP_ROWS: usize = ((H - LAPS_Y) / LAP_ROW) as usize;
const LAP_ROW: u32 = 20;
const LAPS_Y: u32 = super::LIST_Y + 16;

/// One lap of a past ride as its list shows it, in 10 bytes: a field all ones is not known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LapRow {
    timer_s: u16,
    dist_10m: u16,
    speed_x10: u16,
    power: u16,
    hr: u8,
}

impl LapRow {
    /// A lap with nothing known (the array's filler).
    pub const EMPTY: LapRow =
        LapRow { timer_s: u16::MAX, dist_10m: u16::MAX, speed_x10: u16::MAX, power: u16::MAX, hr: u8::MAX };

    /// A lap from its values (moving time s, distance m, speed km/h x 10, power W, heart rate bpm), each saturating
    /// under the "not known" all ones.
    pub fn new(
        timer_s: Option<u32>,
        dist_m: Option<u32>,
        speed_x10: Option<u32>,
        power: Option<u16>,
        hr: Option<u8>,
    ) -> Self {
        let u16_ = |v: Option<u32>| v.map_or(u16::MAX, |v| v.min(0xfffe) as u16);
        LapRow {
            timer_s: u16_(timer_s),
            dist_10m: u16_(dist_m.map(|m| m / 10)),
            speed_x10: u16_(speed_x10),
            power: u16_(power.map(u32::from)),
            hr: hr.map_or(u8::MAX, |v| v.min(0xfe)),
        }
    }

    /// Moving time, s.
    pub fn timer_s(&self) -> Option<u32> {
        (self.timer_s != u16::MAX).then_some(u32::from(self.timer_s))
    }

    /// Distance, m (to 10 m).
    pub fn dist_m(&self) -> Option<u32> {
        (self.dist_10m != u16::MAX).then_some(u32::from(self.dist_10m) * 10)
    }

    /// Average speed, km/h x 10.
    pub fn speed_x10(&self) -> Option<u16> {
        (self.speed_x10 != u16::MAX).then_some(self.speed_x10)
    }

    /// Average power, W.
    pub fn power(&self) -> Option<u16> {
        (self.power != u16::MAX).then_some(self.power)
    }

    /// Average heart rate, bpm.
    pub fn hr(&self) -> Option<u8> {
        (self.hr != u8::MAX).then_some(self.hr)
    }
}

/// The list of a past ride's laps from row `top`: number, time, distance, average speed, power and heart rate (a
/// missing one "--"); "NO LAPS" for a ride with none; the title says "FIRST 99 OF 120" when `total` laps are more than
/// `laps`. Drawn whole (a screen opens or scrolls).
pub fn draw_lap_list(c: &mut impl Canvas, (laps, total): (&[LapRow], usize), top: usize, imperial: bool) {
    let mut title = Line::<20>::new();
    if total > laps.len() {
        title.push_str("FIRST ");
        title.push_u32(laps.len() as u32);
        title.push_str(" OF ");
        title.push_u32(total.min(99_999) as u32);
    } else {
        title.push_str("LAPS");
    }
    super::draw_title(c, title.as_str());
    let grey = Style { fg: super::GREY, scale: 1, ..LABEL };
    if laps.is_empty() {
        text::draw_in(c, &grey, 0, super::LIST_Y + 20, W, Align::Centre, "NO LAPS");
        return;
    }
    // (left, width) of each column: #, time, distance, speed, W, HR
    const COLS: [(u32, u32); 6] = [(2, 22), (28, 48), (80, 34), (118, 34), (156, 34), (194, 34)];
    let head = ["#", "TIME", if imperial { "MI" } else { "KM" }, if imperial { "MPH" } else { "KMH" }, "W", "HR"];
    for (&(x, w), h) in COLS.iter().zip(head) {
        text::draw_in(c, &grey, x, super::LIST_Y + 2, w, Align::Right, h);
    }
    let white = Style { scale: 1, ..LABEL };
    for (r, (i, l)) in laps.iter().enumerate().skip(top).take(LAP_ROWS).enumerate() {
        let y = LAPS_Y + r as u32 * LAP_ROW + 4;
        let mut cells = [Line::<10>::new(); 6];
        let [no, time, dist, speed, power, hr] = &mut cells;
        no.push_u32(i as u32 + 1);
        // time to 99:59:59, distance to 999.9 km, power to 9999 W: what a column holds
        match l.timer_s().map(|t| t.min(99 * 3600 + 3599)) {
            Some(t) => {
                let (h, m, sec) = (t / 3600, t / 60 % 60, t % 60);
                if h > 0 {
                    time.push_u32(h);
                    time.push_str(":");
                    time.push_str(super::DIGITS[(m / 10) as usize]);
                    time.push_str(super::DIGITS[(m % 10) as usize]);
                } else {
                    time.push_u32(m);
                }
                time.push_str(":");
                time.push_str(super::DIGITS[(sec / 10) as usize]);
                time.push_str(super::DIGITS[(sec % 10) as usize]);
            }
            None => time.push_str("--"),
        }
        match l.dist_m() {
            Some(m) => crate::push_trip(dist, m.min(999_900), imperial),
            None => dist.push_str("--"),
        }
        match l.speed_x10().map(|v| if imperial { crate::mph_x10(u32::from(v)) } else { u32::from(v) }) {
            Some(v) => {
                speed.push_u32(v.min(9999) / 10);
                speed.push_str(".");
                speed.push_u32(v.min(9999) % 10);
            }
            None => speed.push_str("--"),
        }
        match l.power() {
            Some(v) => power.push_u32(u32::from(v.min(9999))),
            None => power.push_str("--"),
        }
        match l.hr() {
            Some(v) => hr.push_u32(u32::from(v)),
            None => hr.push_str("--"),
        }
        for (&(x, w), cell) in COLS.iter().zip(&cells) {
            text::draw_in(c, &white, x, y, w, Align::Right, cell.as_str());
        }
    }
}

impl Card {
    /// No title, no numbers.
    pub const EMPTY: Card = Card { title: Line::new(), stats: [("", Line::new()); STATS] };

    /// The card of a ride with these numbers: "23 OCT 09:12", then time, distance, speed, ascent, NP, power,
    /// heart rate, TSS, work.
    pub fn of(s: &Stats, imperial: bool) -> Card {
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
        crate::push_hm(&mut time, s.timer_s);
        let mut km = Line::<10>::new();
        crate::push_trip(&mut km, s.dist_m, imperial); // 1 decimal, as the lists' km
        let mut speed = Line::<10>::new();
        match s.avg_speed_x10.map(|v| if imperial { crate::mph_x10(u32::from(v)) } else { u32::from(v) }) {
            Some(v) => {
                speed.push_u32(v / 10);
                speed.push_str(".");
                speed.push_u32(v % 10);
            }
            None => speed.push_str("--"),
        }
        let w = |v: Option<u16>| num(v.map(u32::from));
        card.stats = [
            ("TIME", time),
            (if imperial { "MI" } else { "KM" }, km),
            (if imperial { "AVG MPH" } else { "AVG KM/H" }, speed),
            if imperial {
                ("ASCENT FT", num(s.ascent_m.map(|m| crate::feet(m.min(i32::MAX as u32) as i32) as u32)))
            } else {
                ("ASCENT M", num(s.ascent_m))
            },
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

/// Draws `card` with `track` in [`MAP`] as `view` shows it, over `base` when it holds that view: the title, the
/// map, the profile, the numbers, a line under the profile. `elev(i)` gives column `i` of the profile ([`ELEV`]'s width) as a height and
/// a colour.
pub fn draw(
    c: &mut impl Canvas,
    card: &Card,
    (track, view, base): (&map::Track, Option<map::View>, Option<&crate::tiles::Basemap>),
    elev: &dyn Fn(u32) -> (u32, u16),
) {
    map::draw(c, map::area(MAP), track, view, base, None, false);
    let grey = Style { fg: super::GREY, ..LABEL };
    text::draw_in(c, &grey, 0, PAGE_Y + 2, W, Align::Centre, card.title.as_str());
    let (x, y, w, h) = ELEV;
    super::draw_graph(c, (x, y, w, h), elev);
    c.fill(x, y + h, w, 1, super::DARK);
    let small = Style { scale: 1, ..grey };
    for (i, (label, value)) in card.stats.iter().enumerate() {
        let (col, row) = ((i % 2) as u32, (i / 2) as u32);
        let (cx, cy) = (col * W / 2 + 2, STATS_Y + row * STAT_H);
        let cw = W / 2 - 4;
        text::draw_in(c, &small, cx, cy + 6, cw - VALUE_W, Align::Left, label);
        text::draw_in(c, &LABEL, cx + cw - VALUE_W, cy, VALUE_W, Align::Right, value.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::super::H;
    use super::*;

    /// The widest realistic values and labels fit their cells whole, and LAPS leaves the title alone.
    #[test]
    fn stat_cells_and_title_are_whole() {
        let (cw, small) = (W / 2 - 4, Style { scale: 1, ..LABEL });
        for v in ["109.3", "999.9", "9999", "1234.", "99:59", "187"] {
            assert!(LABEL.width(v) <= VALUE_W, "{v}");
        }
        for l in [
            "TIME",
            "KM",
            "MI",
            "AVG KM/H",
            "AVG MPH",
            "ASCENT FT",
            "ASCENT M",
            "NP",
            "AVG W",
            "AVG HR",
            "MAX HR",
            "TSS",
            "KJ",
        ] {
            assert!(small.width(l) <= cw - VALUE_W, "{l}");
        }
        let mut card = Card::default();
        card.title.push_str("30 OCT 10:17");
        let (mut a, mut b) = (super::super::tests::Frame::new(), super::super::tests::Frame::new());
        for f in [&mut a, &mut b] {
            draw(f, &card, (&map::Track::new(), None, None), &|_| (0, super::super::GREY));
        }
        draw_laps_label(&mut b);
        draw_delete(&mut b);
        let tw = LABEL.width("30 OCT 10:17");
        for y in PAGE_Y + 2..PAGE_Y + 2 + LABEL.height() {
            for x in (W - tw) / 2..(W + tw) / 2 {
                assert_eq!(a.at(x, y), b.at(x, y), "{x},{y}");
            }
        }
    }

    #[test]
    fn fits_the_screen() {
        assert!(STATS_Y + (STATS as u32).div_ceil(2) * STAT_H <= H);
        assert!(STATS_Y + (STATS as u32).div_ceil(2) * STAT_H + STAT_H > H, "no row's room left over");
        let mut f = super::super::tests::Frame::new();
        let mut card = Card::default();
        card.title.push_str("23 OCT 09:12");
        for (label, value) in &mut card.stats {
            *label = "AVG KM/H";
            value.push_str("1:32:10");
        }
        draw(&mut f, &card, (&map::Track::new(), None, None), &|i| (i % 30, super::super::GREY));
    }
}
