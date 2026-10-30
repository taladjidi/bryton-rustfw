//! The overview (the main screen): the date, the TODAY card, four fields of the user's choice, the notices

use super::*;

/// The date: the pixel font at 3x, one size up from the labels, white. The time is in the strip.
pub const DATE: Style = Style { font: &PIXEL, scale: 3, fg: WHITE, bg: BLACK };
/// Top of the date row.
pub const DATE_Y: u32 = PAGE_Y + 1;
/// Height of the date row: the date's text box.
pub const DATE_H: u32 = PIXEL.height as u32 * DATE.scale;
/// The TODAY card (left, top, width, height): a tap anywhere in it opens the calendar. Its top 4 rows are also in
/// the cog's touch box, where the cog wins. It takes all the height the fields leave (owner, 2026-10-26: a taller
/// profile).
pub const TODAY_BOX: (u32, u32, u32, u32) = (4, DATE_Y + DATE_H + 2, W - 8, OVERVIEW_Y - 2 - (DATE_Y + DATE_H + 2));
/// The overview's fields: top and height (a 2 x 2 grid of [`field_box`] boxes), right above the notices.
pub const OVERVIEW_Y: u32 = NOTICES_Y - 4 - OVERVIEW_H;
/// See [`OVERVIEW_Y`]: two rows of 60 px, the 4x digits clear of their label.
pub const OVERVIEW_H: u32 = 120;
/// Top of the notices: [`NOTICES`] rows of [`LIST_ROW`] at the screen's foot.
pub const NOTICES_Y: u32 = H - 8 - NOTICES as u32 * LIST_ROW;
/// Notices shown at most: the newest (a tap opens them all); one, so the TODAY card has the height (owner, 2026-10-26).
pub const NOTICES: usize = 1;

/// Field box `i` of the overview's four.
pub fn overview_box(i: usize) -> Option<(u32, u32, u32, u32)> {
    let (x, y, w, h) = field_box(4, OVERVIEW_H, i)?;
    Some((x, y - RIDE_Y + OVERVIEW_Y, w, h))
}

/// The date row: the date centred in the row left of the cog (the screen's centre would run a 10-character date at
/// 3x into it), white, the cog right.
pub fn draw_date(c: &mut impl Canvas, date: &str) {
    text::draw_in(c, &DATE, 4, DATE_Y, GEAR_AT.0 - 4 - 4, Align::Centre, date);
    draw_cog(c); // after the date: its box ends the row
}

/// The longest start of `s` that fits `w` in `style`, whole characters.
fn fit<'a>(style: &Style, s: &'a str, w: u32) -> &'a str {
    let end =
        s.char_indices().map(|(i, ch)| i + ch.len_utf8()).take_while(|&e| style.width(s.get(..e).unwrap_or("")) <= w);
    s.get(..end.last().unwrap_or(0)).unwrap_or("")
}

/// Bars side by side from `x`, `colw` px each, on the baseline `y + h`: a bar is a byte of a workout's profile
/// ([`unpack_bar`]: a height in 1/[`PROF_H`] of `h`, a colour), 0 for none; neighbours alike are one fill. The area
/// is cleared before.
pub fn draw_profile_bars(c: &mut impl Canvas, (x, y, h): (u32, u32, u32), colw: u32, bars: &[u8]) {
    let mut i = 0;
    while let Some(&bar) = bars.get(i) {
        let mut j = i + 1;
        while bars.get(j) == Some(&bar) {
            j += 1;
        }
        let (level, color) = unpack_bar(bar);
        if level > 0 {
            let bh = (u32::from(level) * h / PROF_H).clamp(1, h);
            c.fill(x + i as u32 * colw, y + h - bh, (j - i) as u32 * colw, bh, color);
        }
        i = j;
    }
}

/// What the TODAY card shows.
#[derive(Clone, Copy)]
pub enum Today<'a> {
    /// The calendar is not read yet (it is, a few seconds after the start): the title alone.
    Pending,
    /// Nothing planned: REST DAY.
    Rest,
    /// A workout: its name, its length ("1:30"), its profile ([`draw_profile_bars`]) and whether it is done ([`DONE_BOX`]).
    Planned(&'a str, &'a str, &'a [u8], bool),
}

/// The TODAY card whole:"TODAY" and the length (H:MM) on the first line, the workout's name cut to the width on
/// the second, its profile (a bar per column in its zone's colour, [`draw_profile_bars`]) under them; None: REST DAY in grey.
/// Drawn once, when the overview opens and when the day or the calendar changes.
pub fn draw_today(c: &mut impl Canvas, card: Today) {
    let (x, y, w, h) = TODAY_BOX;
    c.fill(x, y, w, h, BLACK);
    for (rx, ry, rw, rh) in [(x, y, w, 1), (x, y + h - 1, w, 1), (x, y, 1, h), (x + w - 1, y, 1, h)] {
        c.fill(rx, ry, rw, rh, DARK);
    }
    let (tx, tw) = (x + 6, w - 12);
    let grey = Style { fg: GREY, ..LABEL };
    text::draw(c, &grey, tx, y + 3, Align::Left, "TODAY");
    match card {
        Today::Planned(name, length, prof, done) => {
            draw_done(c, done);
            text::draw(c, &LABEL, x + w - 6, y + 3, Align::Right, length);
            text::draw(c, &LABEL, tx, y + 23, Align::Left, fit(&LABEL, name, tw));
            draw_profile_bars(c, (x + 4, y + 46, h - 51), (w - 8) / prof.len().max(1) as u32, prof);
        }
        Today::Rest => {
            text::draw(c, &grey, tx, y + 23, Align::Left, "REST DAY");
        }
        Today::Pending => {}
    }
}

/// The TODAY card's DONE tag: a tick and the word in green, in its own fixed box (blank when not done).
pub const DONE_BOX: (u32, u32, u32, u32) = (TODAY_BOX.0 + 80, TODAY_BOX.1 + 2, 80, 18);

/// A green tick, 16 x 12 px, at (`x`, `y`): 2 px blocks along its two strokes.
pub fn draw_tick(c: &mut impl Canvas, x: u32, y: u32) {
    for (i, dy) in [6u32, 8, 10, 8, 6, 4, 2, 0].into_iter().enumerate() {
        c.fill(x + 2 * i as u32, y + dy, 2, 2, GREEN);
    }
}

/// [`DONE_BOX`] painted: the tag when `done`, else black.
pub fn draw_done(c: &mut impl Canvas, done: bool) {
    let (x, y, w, h) = DONE_BOX;
    c.fill(x, y, w, h, BLACK);
    if done {
        draw_tick(c, x + 2, y + 2);
        text::draw(c, &Style { fg: GREEN, ..LABEL }, x + 22, y + 1, Align::Left, "DONE");
    }
}

/// A WORKOUTS list row in slot `slot`: the day over the length on the left, the workout's profile on the right
/// ([`draw_profile_bars`], 2 px a column, on black even when the row is selected), a green tick between them when `done`.
pub fn draw_workout_row(
    c: &mut impl Canvas,
    slot: usize,
    (day, length): (&str, &str),
    selected: bool,
    prof: &[u8],
    done: bool,
) {
    let row = c.theme().menu_row;
    let y = LIST_Y + slot as u32 * row;
    let (bg, fg) = if selected { (ACCENT, ON_ACCENT) } else { (BLACK, WHITE) };
    c.fill(0, y, W, row - 1, bg);
    c.fill(0, y + row - 1, W, 1, DARK);
    let style = Style { fg, bg, ..LABEL };
    text::draw(c, &style, NEARBY.0, y + 1, Align::Left, day);
    text::draw(c, &style, NEARBY.0, y + 21, Align::Left, length);
    if done {
        draw_tick(c, 96, y + 16);
    }
    let (bx, bw) = (W - 8 - 2 * prof.len() as u32 - 4, 2 * prof.len() as u32 + 4);
    c.fill(bx, y + 3, bw, row - 7, BLACK);
    draw_profile_bars(c, (bx + 2, y + 5, row - 11), 2, prof);
}

/// The menu button on the overview: a cog, [`COG`] px square.
pub const COG: u32 = 25;
/// Where the cog is drawn: 6 px from the right edge, in the date's row, level with the date.
pub const GEAR_AT: (u32, u32) = (W - 6 - COG, DATE_Y + (DATE_H - COG) / 2);
/// The cog's centre, the middle of its touch box.
pub const GEAR_C: (u32, u32) = (GEAR_AT.0 + COG / 2, GEAR_AT.1 + COG / 2);
/// Where a tap opens the menu: 44 x 44 px round the cog (the menu rows' rule), over the strip's bottom rows and the
/// TODAY card's top right corner, where the cog wins.
pub const GEAR_TAP: (u32, u32, u32, u32) = (W - 44, GEAR_C.1 - 22, 44, 44);

/// Coverage (0 to 9, nine samples) of the cog's metal at pixel (`i`, `j`) of its [`COG`] square, symmetric: a ring
/// (two thirds of the radius) with a round hole (3/10) and eight square teeth (7/20 of the radius wide) at the compass points, each sample taken in sixths of a pixel.
fn cog_coverage(i: u32, j: u32) -> u32 {
    const C: i32 = COG as i32 * 3; // the centre, in sixths
    const ROOT: i32 = C * 2 / 3; // the ring's outside, a third of the way in from the teeth's tips
    const HOLE: i32 = C * 3 / 10;
    const TOOTH: i32 = C - 3; // the tips, half a pixel from the edge
    const HALF: i32 = C * 7 / 40; // half a tooth's width
    let mut n = 0;
    for sy in 0..3 {
        for sx in 0..3 {
            let (dx, dy) = ((i * 6 + 1 + 2 * sx) as i32 - C, (j * 6 + 1 + 2 * sy) as i32 - C);
            let (u, v) = ((dx - dy).abs() * 5, (dx + dy).abs() * 5); // the diagonals' axes, times 5 / sqrt 2 ~ 7 / 2
            let tooth = (dy.abs() <= HALF && dx.abs() <= TOOTH)
                || (dx.abs() <= HALF && dy.abs() <= TOOTH)
                || (u <= HALF * 7 && v <= TOOTH * 7)
                || (v <= HALF * 7 && u <= TOOTH * 7);
            let r2 = dx * dx + dy * dy;
            n += u32::from((r2 <= ROOT * ROOT || tooth) && r2 > HOLE * HOLE);
        }
    }
    n
}

/// RGB565 `color` at `n` ninths over `bg` (black in DARK).
fn dim(color: u16, bg: u16, n: u32) -> u16 {
    let ch = |shift: u32, max: u32| {
        (((u32::from(color) >> shift & max) * n + (u32::from(bg) >> shift & max) * (9 - n)) / 9) << shift
    };
    (ch(11, 31) | ch(5, 63) | ch(0, 31)) as u16
}

/// The cog at [`GEAR_AT`], anti-aliased over black.
fn draw_cog(c: &mut impl Canvas) {
    let (color, bg) = (c.ink(COG_COLOR), c.ink(BLACK));
    c.blit(GEAR_AT.0, GEAR_AT.1, COG, COG, &mut |i, j| dim(color, bg, cog_coverage(i, j)));
}

/// The notices, newest first; with none, the area stays black.
pub fn draw_notices(c: &mut impl Canvas, notices: &[&str]) {
    c.fill(0, NOTICES_Y, W, H - NOTICES_Y, BLACK);
    if notices.is_empty() {
        return; // nothing to say: nothing drawn (Ferris is in ABOUT)
    }
    for (r, n) in notices.iter().take(NOTICES).enumerate() {
        // "19:36 A CALLER": the time small and grey, the text in the large font for every row (cut, not shrunk:
        // one row smaller than the other read as inconsistent on the unit)
        let style = Style { fg: if r == 0 { WHITE } else { GREY }, ..LABEL };
        let small = Style { fg: GREY, scale: 1, ..LABEL };
        let y = NOTICES_Y + 4 + r as u32 * LIST_ROW;
        let (time, text) = n.split_at(n.find(' ').map_or(0, |i| i + 1));
        let tw = small.width("00:00 ");
        let text = notice_fit(&style, text, W - 8 - tw);
        text::draw_in(c, &small, 4, y + style.height() - small.height(), tw, Align::Left, time);
        text::draw_in(c, &style, 4 + tw, y, W - 8 - tw, Align::Left, text);
    }
}

/// The part of a notice that fits `w`: cut at the last word that fits ("Remote control", not "Remote control e"). A
/// word cut under 4 characters ("Re:" before a long name) is not enough to read, so the text is cut at its last
/// character instead, as draw_in does.
fn notice_fit<'a>(style: &Style, text: &'a str, w: u32) -> &'a str {
    let t = text.trim_start();
    let (word, _) = lines(style, t, w);
    if word.chars().count() >= 4 {
        return word;
    }
    let cut = t
        .char_indices()
        .map(|(i, _)| i)
        .chain(core::iter::once(t.len()))
        .take_while(|&i| style.width(t.get(..i).unwrap_or("")) <= w)
        .last();
    t.get(..cut.unwrap_or(0)).unwrap_or("")
}

/// Text lines the notification detail shows at once.
pub const NOTIF_LINES: usize = ((H - LIST_Y - LIST_ROW) / LIST_ROW) as usize;

/// Characters of the pixel font that fit a detail line.
pub fn notif_cols() -> usize {
    ((W - 8) / LABEL.width("W").max(1)) as usize
}

/// A notification in full: `app` in the title, `time` and a MORE cue under it, then `lines` (title lines first, in
/// the accent colour: the flag is true) from the first one shown, at most [`NOTIF_LINES`].
pub fn draw_notif<'a>(
    c: &mut impl Canvas,
    app: &str,
    time: &str,
    more: bool,
    lines: impl Iterator<Item = (&'a str, bool)>,
) {
    draw_title(c, app);
    let grey = Style { fg: GREY, ..LABEL };
    // the time and MORE each in a box of their own, so neither wipes the other
    let mw = grey.width("MORE");
    text::draw_in(c, &grey, 4, LIST_Y, W - 12 - mw, Align::Left, time);
    text::draw_in(c, &grey, W - 4 - mw, LIST_Y, mw, Align::Right, if more { "MORE" } else { "" });
    for (r, (line, head)) in lines.take(NOTIF_LINES).enumerate() {
        let style = Style { fg: if head { ACCENT } else { WHITE }, ..LABEL };
        text::draw_in(c, &style, 4, LIST_Y + LIST_ROW * (r as u32 + 1), W - 8, Align::Left, line);
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// A notice too long for its row is cut at a space, and a short one is drawn whole.
    #[test]
    fn a_long_notice_is_cut_at_a_word() {
        let style = Style { fg: WHITE, ..LABEL };
        let w = W - 8 - LABEL.width("00:00 ");
        let long = "Remote control entering the room now";
        let (cut, _) = lines(&style, long, w);
        assert!(style.width(cut) <= w && cut.len() < long.len(), "{cut}");
        assert_eq!(long.as_bytes().get(cut.len()), Some(&b' '), "cut at a word: {cut}");
        assert_eq!(lines(&style, "Mum", w), ("Mum", ""));
        // "Re:" alone would hide the name: the text is cut at its last character instead
        let name = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmn";
        let re = std::format!("Re: {name}");
        assert!(notice_fit(&style, &re, w).len() > 4, "{}", notice_fit(&style, &re, w));
        // leading spaces (the text after the time) are not a cut
        assert_eq!(notice_fit(&style, &std::format!("  {long}"), w), notice_fit(&style, long, w));
        assert!(!notice_fit(&style, &std::format!("  {name}"), w).is_empty());
    }
}
