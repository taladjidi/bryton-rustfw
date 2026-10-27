//! The media player screen: what the phone says is playing (AMS: title, artist, album, duration, elapsed), a
//! progress bar, and touch buttons, laid out like a music player's. Pure drawing against [`Canvas`], each part in its
//! own fixed box so a second's update redraws only the time and the bar's new columns.

use super::{ACCENT, BLACK, Canvas, DARK, GREY, LABEL, PAGE_Y, W, WHITE};
use crate::text::{self, Align, Line, Style};

/// A button of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Previous track.
    Prev,
    /// Play or pause.
    Play,
    /// Next track.
    Next,
    /// Skip backward in the track (the app sets how far; podcasts).
    SkipBack,
    /// Volume down.
    VolDown,
    /// Volume up.
    VolUp,
    /// Skip forward.
    SkipFwd,
}

/// The title: two lines from here.
const TITLE_Y: u32 = PAGE_Y + 8;
/// The artist, then the album, under the title.
const ARTIST_Y: u32 = TITLE_Y + 2 * ROW;
const ALBUM_Y: u32 = ARTIST_Y + ROW;
/// A text row's height (the 2x pixel font and a gap).
const ROW: u32 = 20;
/// The progress bar: x, y, width, height; the times under it.
pub const BAR: (u32, u32, u32, u32) = (12, ALBUM_Y + ROW + 6, W - 24, 6);
const TIMES_Y: u32 = BAR.1 + BAR.3 + 6;
/// The buttons, as (button, x, y, w, h): transport keys big, then skip and volume (the camera's shutter is a hold
/// anywhere).
pub const BUTTONS: [(Button, u32, u32, u32, u32); 7] = {
    const Y1: u32 = TIMES_Y + ROW + 10;
    const Y2: u32 = Y1 + 64 + 12;
    [
        (Button::Prev, 8, Y1, 64, 64),
        (Button::Play, 88, Y1, 64, 64),
        (Button::Next, 168, Y1, 64, 64),
        (Button::SkipBack, 4, Y2, 52, 44),
        (Button::VolDown, 64, Y2, 52, 44),
        (Button::VolUp, 124, Y2, 52, 44),
        (Button::SkipFwd, 184, Y2, 52, 44),
    ]
};

/// The button under a touch at (`x`, `y`), if any (a little margin around each: fingers are wide).
pub fn hit(x: u16, y: u16) -> Option<Button> {
    BUTTONS.iter().find(|&&(_, bx, by, w, h)| super::Rect(bx, by, w, h).grown(6).contains(x, y)).map(|b| b.0)
}

/// What is on screen, so an update redraws only what changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// Title, artist and album's fingerprint.
    pub text: u32,
    /// Playing.
    pub playing: Option<bool>,
    /// The bar's filled width, px.
    pub bar: u32,
    /// The two times' fingerprint.
    pub times: u32,
}

/// What to show.
#[derive(Clone, Copy, Debug, Default)]
pub struct Now<'a> {
    /// The track (None: no phone).
    pub title: Option<&'a str>,
    /// Artist and album.
    pub artist: &'a str,
    /// See `artist`.
    pub album: &'a str,
    /// Playing.
    pub playing: bool,
    /// Elapsed and total seconds (total 0: unknown).
    pub elapsed: u32,
    /// See `elapsed`.
    pub duration: u32,
}

/// The whole screen: the buttons, then everything [`update`] draws.
pub fn draw(c: &mut impl Canvas, now: &Now) -> Shown {
    for &(b, x, y, w, h) in &BUTTONS {
        button(c, b, (x, y, w, h), b == Button::Play && now.playing, false);
    }
    let mut shown = Shown::default();
    update(c, now, &mut shown, true);
    shown
}

/// What [`draw`] leaves on screen, without drawing (for a screen drawn elsewhere, a screenshot row).
pub fn shown(now: &Now) -> Shown {
    struct Nothing;
    impl Canvas for Nothing {
        fn fill(&mut self, _: u32, _: u32, _: u32, _: u32, _: u16) {}
        fn blit(&mut self, _: u32, _: u32, _: u32, _: u32, _: &mut dyn FnMut(u32, u32) -> u16) {}
    }
    let mut s = Shown::default();
    update(&mut Nothing, now, &mut s, true);
    s
}

/// Redraws what changed since `shown` (all of it if `all`).
pub fn update(c: &mut impl Canvas, now: &Now, shown: &mut Shown, all: bool) {
    let print = [now.title.unwrap_or("\u{1}"), now.artist, now.album].iter().fold(0x811c_9dc5u32, |h, s| fnv(h, s));
    if all || print != shown.text {
        shown.text = print;
        draw_text(c, now);
    }
    if all || shown.playing != Some(now.playing) {
        shown.playing = Some(now.playing);
        let &(b, x, y, w, h) = &BUTTONS[1];
        button(c, b, (x, y, w, h), now.playing, false);
    }
    let (bx, by, bw, bh) = BAR;
    let px = if now.duration > 0 { (now.elapsed.min(now.duration) * bw / now.duration).min(bw) } else { 0 };
    if all || px < shown.bar {
        c.fill(bx, by, bw, bh, DARK);
        shown.bar = 0;
    }
    if px > shown.bar {
        c.fill(bx + shown.bar, by, px - shown.bar, bh, ACCENT); // only the new columns
        shown.bar = px;
    }
    let mut left = Line::<8>::new();
    clock(&mut left, now.elapsed);
    let mut right = Line::<8>::new();
    if now.duration > 0 {
        right.push_str("-");
        clock(&mut right, now.duration.saturating_sub(now.elapsed));
    }
    let tprint = fnv(fnv(0x811c_9dc5, left.as_str()), right.as_str());
    if all || tprint != shown.times {
        shown.times = tprint;
        // each in a box as wide as its longest text ("1:02:45", "-1:02:45"), not half the row: a second redraws
        // about 3,600 px (5 % of the screen)
        let grey = Style { fg: GREY, ..LABEL };
        let (lw, rw) = (grey.width("0:00:00"), grey.width("-0:00:00"));
        text::draw_in(c, &grey, bx, TIMES_Y, lw, Align::Left, left.as_str());
        text::draw_in(c, &grey, bx + bw - rw, TIMES_Y, rw, Align::Right, right.as_str());
    }
}

fn fnv(h: u32, s: &str) -> u32 {
    s.bytes().chain([0]).fold(h, |h, x| (h ^ u32::from(x)).wrapping_mul(0x0100_0193))
}

/// "3:07", "1:02:45".
fn clock(l: &mut Line<8>, s: u32) {
    let s = s.min(99 * 3600);
    if s >= 3600 {
        l.push_u32(s / 3600);
        l.push_str(":");
        l.push_str(super::DIGITS[(s / 600 % 6) as usize]);
        l.push_str(super::DIGITS[(s / 60 % 10) as usize]);
    } else {
        l.push_u32(s / 60); // "1:29", not "01:29"
    }
    l.push_str(":");
    l.push_str(super::DIGITS[(s / 10 % 6) as usize]);
    l.push_str(super::DIGITS[(s % 10) as usize]);
}

/// The title on two lines (cut at a space when it can), the artist and the album under it.
fn draw_text(c: &mut impl Canvas, now: &Now) {
    let x = 8;
    let w = W - 16;
    let Some(title) = now.title else {
        text::draw_in(c, &LABEL, x, TITLE_Y, w, Align::Centre, "NO PHONE");
        for y in [TITLE_Y + ROW, ARTIST_Y, ALBUM_Y] {
            c.fill(0, y, W, ROW, BLACK);
        }
        return;
    };
    let (a, b) = split(title, w);
    text::draw_in(c, &LABEL, x, TITLE_Y, w, Align::Left, a);
    text::draw_in(c, &LABEL, x, TITLE_Y + ROW, w, Align::Left, b);
    text::draw_in(c, &Style { fg: GREY, ..LABEL }, x, ARTIST_Y, w, Align::Left, now.artist);
    text::draw_in(c, &Style { fg: DARK, ..LABEL }, x, ALBUM_Y, w, Align::Left, now.album);
}

/// `s` as two lines of at most `w` px: cut at the last space that fits, else anywhere.
fn split(s: &str, w: u32) -> (&str, &str) {
    if LABEL.width(s) <= w {
        return (s, "");
    }
    let fit = s.char_indices().map(|(i, _)| i).chain([s.len()]).take_while(|&i| LABEL.width(&s[..i]) <= w).last();
    let fit = fit.unwrap_or(0);
    let cut = s[..fit].rfind(' ').filter(|&i| i > 0).unwrap_or(fit);
    (&s[..cut], s[cut..].trim_start())
}

/// A button: its box, then its icon or label; `dim` (no phone) greys it. A box under 52 px wide has "-" and "+"
/// for the volume.
pub fn button(c: &mut impl Canvas, b: Button, (x, y, w, h): (u32, u32, u32, u32), playing: bool, dim: bool) {
    let (bg, fg) = match (b, dim) {
        (_, true) => (DARK, GREY),
        (Button::Play, _) => (ACCENT, BLACK),
        _ => (DARK, WHITE),
    };
    c.fill(x, y, w, h, bg);
    let (cx, cy) = (x + w / 2, y + h / 2);
    match b {
        Button::Play if playing => {
            c.fill(cx - 11, cy - 13, 8, 26, fg); // pause: two bars
            c.fill(cx + 3, cy - 13, 8, 26, fg);
        }
        Button::Play => triangle(c, cx - 9, cy - 14, 28, true, fg),
        Button::Next => {
            triangle(c, cx - 14, cy - 10, 20, true, fg);
            triangle(c, cx - 4, cy - 10, 20, true, fg);
            c.fill(cx + 7, cy - 10, 3, 20, fg);
        }
        Button::Prev => {
            c.fill(cx - 10, cy - 10, 3, 20, fg);
            triangle(c, cx - 7, cy - 10, 20, false, fg);
            triangle(c, cx + 3, cy - 10, 20, false, fg);
        }
        // a narrow volume button (the quick panel's): a speaker, then the sign, so it never reads as brightness
        Button::VolDown | Button::VolUp if w < 52 => {
            c.fill(cx - 15, cy - 4, 4, 8, fg);
            triangle(c, cx - 15, cy - 9, 18, false, fg);
            let style = Style { fg, bg, ..LABEL };
            let sign = if b == Button::VolUp { "+" } else { "-" };
            text::draw(c, &style, cx + 1, cy - style.height() / 2, Align::Left, sign);
        }
        _ => {
            let label = match b {
                Button::VolDown => "VOL-",
                Button::VolUp => "VOL+",
                Button::SkipBack => "<<",
                _ => ">>",
            };
            let style = Style { fg, bg, ..LABEL };
            text::draw_in(c, &style, x + 2, cy - style.height() / 2, w - 4, Align::Centre, label);
        }
    }
}

/// A triangle `h` high and `h / 2` wide at (`x`, `y`), its point to the right (`right`) or the left, a row per fill.
fn triangle(c: &mut impl Canvas, x: u32, y: u32, h: u32, right: bool, color: u16) {
    for i in 0..h {
        let w = (i.min(h - 1 - i) + 1).min(h / 2);
        let x0 = if right { x } else { x + h / 2 - w };
        c.fill(x0, y + i, w, 1, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks() {
        let c = |s| {
            let mut l = Line::<8>::new();
            clock(&mut l, s);
            l
        };
        assert_eq!(
            [c(5).as_str(), c(89).as_str(), c(600).as_str(), c(3725).as_str()],
            ["0:05", "1:29", "10:00", "1:02:05"]
        );
    }

    #[test]
    fn layout_fits_and_hits() {
        let last = BUTTONS[6];
        assert!(last.2 + last.4 <= super::super::H, "the buttons fit the screen");
        for &(b, x, y, w, h) in &BUTTONS {
            assert_eq!(hit((x + w / 2) as u16, (y + h / 2) as u16), Some(b));
        }
        assert_eq!(hit(120, TITLE_Y as u16), None, "the title is not a button");
    }

    extern crate std;

    #[test]
    fn titles_split_at_a_space() {
        let w = W - 16;
        assert_eq!(split("Short", w), ("Short", ""));
        let (a, b) = split("Get Down On It (Single Version) Remastered", w);
        assert!(LABEL.width(a) <= w && a.ends_with(|c: char| c != ' ') && !b.starts_with(' '));
        assert_eq!(std::format!("{a} {b}"), "Get Down On It (Single Version) Remastered");
    }

    #[test]
    fn a_second_redraws_the_time_and_the_new_columns_only() {
        let mut f = super::super::tests::Frame::new();
        let now = |elapsed| Now {
            title: Some("Song"),
            artist: "Artist",
            album: "Album",
            playing: true,
            elapsed,
            duration: 200,
        };
        let mut shown = draw(&mut f, &now(10));
        let before = f.filled();
        update(&mut f, &now(11), &mut shown, false);
        let px = f.filled() - before;
        assert!(px < 4000, "a second's update: {px} px (the two times, about 3,600; per-digit cells would cut it)");
        assert_eq!(shown.bar, 11 * BAR.2 / 200);
    }
}
