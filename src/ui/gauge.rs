//! The workout gauge field: the reading big, a bar around the step's target band with a marker at the reading, the
//! band in numbers under it

use super::*;

/// The marker's white core, columns, centred on the reading's column; a black column each side outlines it.
const MARK_W: u32 = 3;
/// Rows of the bar above and below the track at most, where the marker reaches out of it.
const OVERSHOOT: u32 = 4;
/// Rows of the white outline round the target band, and columns at its two edges.
const BRACKET: u32 = 2;

/// A gauge's bar: the target band's first and last columns, the marker's column and the colour of its caps (None: no
/// reading), and the zones behind the track (each one's first column and colour, [`crate::workout::gauge_zones`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GaugeBar {
    /// The band's first and last columns.
    pub band: (u32, u32),
    /// The marker's column and the colour of its ends.
    pub marker: Option<(u32, u16)>,
    /// The zones: first column and colour, in order; `u32::MAX` for one off the scale.
    pub zones: [(u32, u16); 7],
}

/// A gauge bar's place: left, top, columns, height.
type GaugeArea = (u32, u32, u32, u32);

/// The gauge field's parts in box `bx`: the bar (left, top, columns, height), the number's top and style, the
/// target line's top and style (none in a box under 100 high: its 2x text would leave the number no room; the band shows
/// on the bar). Fixed for a box, so the number and the marker are redrawn in place. The track is three times what it was
/// (6 to 12 rows), as thick as the box leaves the number room for 2x text.
pub(super) fn gauge_parts(bx: (u32, u32, u32, u32)) -> (GaugeArea, u32, Style, (u32, Style)) {
    let (x, y, w, h) = bx;
    let small = Style { fg: GREY, ..LABEL };
    let target_y = (y + h).saturating_sub(2 + if h < 100 { 0 } else { small.height() }).max(y);
    let wanted = 3 * ((h / 8).clamp(12, 24) / 2) + 2 * OVERSHOOT;
    let room_2x = target_y.saturating_sub(y + FIELD_LABEL_H + 2 + LABEL.height() + 2 + 2);
    let gh = wanted.min(room_2x).max(12);
    let bar = (x + 8, target_y.saturating_sub(2 + gh).max(y), w.saturating_sub(16), gh);
    let top = y + FIELD_LABEL_H;
    let room = bar.1.saturating_sub(top + 2);
    let mut style = Style { scale: 1, ..LABEL };
    for scale in (1..=8).rev() {
        style.scale = scale;
        if style.width("8888") + 8 <= w && style.height() <= room {
            break;
        }
    }
    (bar, top + room.saturating_sub(style.height()) / 2, style, (target_y, small))
}

/// The columns a gauge in box `bx` has.
pub fn gauge_cols(bx: (u32, u32, u32, u32)) -> u32 {
    gauge_parts(bx).0.2
}

/// Pixel (`i`, `dy`) of a gauge bar `h` high: the marker (a white tick through the track with a black column each
/// side, its ends out of the track in the state's colour) over all; else black above and below the track, the target
/// band's white outline (its inside shows the zones), and the zone's colour.
fn gauge_px(g: &GaugeBar, h: u32, i: u32, dy: u32) -> u16 {
    let over = (h / 4).min(OVERSHOOT);
    let track = dy >= over && dy + over < h;
    if let Some((m, color)) = g.marker.filter(|&(m, _)| i + MARK_W / 2 + 1 >= m && i <= m + MARK_W / 2 + 1) {
        let core = i + MARK_W / 2 >= m && i <= m + MARK_W / 2;
        return match (core, track) {
            (true, true) => WHITE,
            (true, false) => color,
            _ => BLACK,
        };
    }
    if !track {
        return BLACK;
    }
    let (a, b) = g.band;
    let edge = dy < over + BRACKET || dy + over + BRACKET >= h;
    if (a..=b).contains(&i) && (edge || i < a + BRACKET || i + BRACKET > b) {
        return WHITE;
    }
    g.zones.iter().rev().find(|z| z.0 <= i).map_or(DARK, |z| z.1)
}

/// Columns `from` to `to` (included) of the gauge bar `bar` showing `g`.
fn draw_gauge_columns(c: &mut impl Canvas, bar: (u32, u32, u32, u32), g: &GaugeBar, from: u32, to: u32) {
    let to = to.min(bar.2.saturating_sub(1));
    if from <= to {
        // the keys' translations once, not a call into the canvas per pixel
        let table = theme::KEYS.map(|k| c.ink(k));
        c.blit(bar.0 + from, bar.1, to - from + 1, bar.3, &mut |i, dy| {
            let p = gauge_px(g, bar.3, from + i, dy);
            theme::KEYS.iter().position(|&k| k == p).and_then(|k| table.get(k).copied()).unwrap_or(p)
        });
    }
}

/// A gauge's title row: `title` at the left in the label style, and `tag` (the pacer's target, big and green) at the
/// right end when there is one, then the title is at 1x to leave it room.
pub fn draw_gauge_title(c: &mut impl Canvas, (x, y, w, _): (u32, u32, u32, u32), title: &str, tag: &str) {
    let big = Style { fg: GREEN, ..LABEL };
    let tag_w = if tag.is_empty() { 0 } else { big.width(tag) };
    let room = w.saturating_sub(12 + if tag_w > 0 { tag_w + 6 } else { 0 });
    let (style, dy) = gauge_title_style(tag);
    if tag_w > 0 {
        c.fill(x + 6, y + 4, room, LABEL.height(), BLACK); // the 1x title is shorter than what the 2x one left
    }
    text::draw_in(c, &style, x + 6, y + dy, room, Align::Left, title);
    if tag_w > 0 {
        c.fill(x + 6 + room, y + 4, w.saturating_sub(12 + room), LABEL.height(), BLACK); // the gap and the tag's box
        text::draw_in(c, &big, (x + w).saturating_sub(6 + tag_w), y + 4, tag_w, Align::Left, tag);
    }
}

/// The title's style and its offset in the title row: the label style, or at 1x (centred in the row) beside a tag.
fn gauge_title_style(tag: &str) -> (Style, u32) {
    let small = Style { scale: 1, fg: GREY, ..LABEL };
    if tag.is_empty() {
        (Style { fg: GREY, ..LABEL }, 4)
    } else {
        (Style { ..small }, 4 + (LABEL.height() - small.height()) / 2)
    }
}

/// A gauge's title that was `old` now `new` (the tag as it was): only the characters that changed when the length is the
/// same, else the whole row.
pub fn update_gauge_title(c: &mut impl Canvas, bx: (u32, u32, u32, u32), (old, new): (&str, &str), tag: &str) {
    if old.len() == new.len() {
        let (style, dy) = gauge_title_style(tag);
        draw_changed_chars(c, &style, bx.0 + 6, bx.1 + dy, old, new);
    } else {
        draw_gauge_title(c, bx, new, tag);
    }
}

/// The gauge field whole in box `bx`: `title` (and `tag`) in its label line, `value` big in `fg`, the bar `g` (none:
/// the bar's place black) and `target` under it.
pub fn draw_gauge(
    c: &mut impl Canvas,
    bx: (u32, u32, u32, u32),
    (title, tag, target): (&str, &str, &str),
    value: &str,
    fg: u16,
    g: Option<GaugeBar>,
) {
    let (x, _, w, h) = bx;
    let (bar, _, _, (target_y, small)) = gauge_parts(bx);
    draw_gauge_title(c, bx, title, tag);
    draw_gauge_value(c, bx, "", value, fg);
    match g {
        Some(g) => draw_gauge_columns(c, bar, &g, 0, bar.2),
        None => c.fill(bar.0, bar.1, bar.2, bar.3, BLACK),
    }
    if h >= 100 {
        text::draw_in(c, &small, x + 1, target_y, w.saturating_sub(2), Align::Centre, target);
    }
}

/// The gauge's number that was `old` ("": not drawn) now `new` in `fg`: only the characters that changed when the
/// length is the same, else its whole box.
pub fn draw_gauge_value(c: &mut impl Canvas, bx: (u32, u32, u32, u32), old: &str, new: &str, fg: u16) {
    let (x, _, w, _) = bx;
    let (_, top, style, _) = gauge_parts(bx);
    let style = Style { fg, ..style };
    if old.len() == new.len() && style.width(new) + 2 <= w {
        draw_changed_chars(c, &style, x + 1 + (w - 2 - style.width(new)) / 2, top, old, new);
    } else {
        text::draw_in(c, &style, x + 1, top, w.saturating_sub(2), Align::Centre, new);
    }
}

/// The gauge's marker moved from `old` to where `g` has it: only the old marker's columns and the new one's.
pub fn draw_gauge_marker(c: &mut impl Canvas, bx: (u32, u32, u32, u32), old: Option<(u32, u16)>, g: GaugeBar) {
    let bar = gauge_parts(bx).0;
    for (m, _) in [old, g.marker].into_iter().flatten() {
        draw_gauge_columns(c, bar, &g, m.saturating_sub(MARK_W / 2 + 1), m + MARK_W / 2 + 1);
    }
}
