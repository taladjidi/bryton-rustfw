//! The quick panel's geometry and drawing: what the rider wants to see right away, top to bottom: the media
//! remote (now playing and five touch buttons), the power meter (watts, battery, ZERO), the paired sensors (one line
//! each), the status line (GPS, phone, unit battery) and the brightness buttons. Pure drawing against [`Canvas`];
//! the app (app/quick.rs) decides what each box says and redraws only the ones that changed.

use super::media::{self, Button};
use super::{ACCENT, BLACK, Canvas, DARK, GREY, LABEL, PAGE_Y, RED, Rect, WHITE};
use crate::text::{self, Align, Style};

/// Now playing: the title, then the artist, one line each.
pub const TITLE_Y: u32 = PAGE_Y + 4;
/// See [`TITLE_Y`].
pub const ARTIST_Y: u32 = TITLE_Y + 16;
/// The media buttons, 44 px targets with 3 px between: previous, play / pause, next, volume down, volume up.
pub const MEDIA: [(Button, u32, u32, u32, u32); 5] = {
    const Y: u32 = ARTIST_Y + 18;
    [
        (Button::Prev, 4, Y, 44, 44),
        (Button::Play, 51, Y, 44, 44),
        (Button::Next, 98, Y, 44, 44),
        (Button::VolDown, 145, Y, 44, 44),
        (Button::VolUp, 192, Y, 44, 44),
    ]
};
/// The power meter's block starts here (44 px high); the sensors, or the status, follow without it.
pub const POWER_Y: u32 = MEDIA[0].2 + 44 + 4;
/// The ZERO button, in the power block's right side.
pub const ZERO: (u32, u32, u32, u32) = (150, POWER_Y, 86, 44);
/// The watts (3x): x, y, width.
pub const WATTS: (u32, u32, u32) = (8, POWER_Y + 2, 72);
/// The power meter's battery tag.
pub const POWER_BATT: (u32, u32) = (88, POWER_Y + 6);
/// The calibration's result under the watts: x, y, width.
pub const RESULT: (u32, u32, u32) = (8, POWER_Y + 28, 138);
/// A sensor line's height.
pub const ROW: u32 = 16;
/// Where the sensor lines start with the power block, and without it.
pub const SENSORS_Y: [u32; 2] = [POWER_Y + 44 + 4, POWER_Y];
/// The most sensor lines (one a kind but the power meter).
pub const ROWS: usize = 6;
/// A sensor line's name, value and battery boxes (x, width).
pub const SENSOR_BOXES: [(u32, u32); 3] = [(4, 84), (92, 108), (202, 38)];
/// The status line: GPS, phone, unit battery.
pub const STATUS_Y: u32 = 248;
/// The status line's three boxes (x, width).
pub const STATUS: [(u32, u32); 3] = [(4, 72), (78, 96), (178, 58)];
/// The brightness row: the two buttons (44 px high), the level between.
pub const BRIGHT_Y: u32 = 268;
/// Darker and brighter.
pub const BRIGHT: [(u32, u32, u32, u32); 2] = [(8, BRIGHT_Y, 64, 44), (168, BRIGHT_Y, 64, 44)];
/// The level between the buttons: x, y, width.
pub const LEVEL: (u32, u32, u32) = (76, BRIGHT_Y + 15, 88);

/// What a touch hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// A media button.
    Media(Button),
    /// ZERO.
    Zero,
    /// Brightness down (false) or up (true).
    Light(bool),
}

/// The control under a touch at (`x`, `y`), if any (`zero`: the power block is shown).
pub fn hit(x: u16, y: u16, zero: bool) -> Option<Hit> {
    let at = |(bx, by, w, h): (u32, u32, u32, u32)| Rect(bx, by, w, h).contains(x, y);
    MEDIA
        .iter()
        .find(|&&(_, bx, by, w, h)| at((bx, by, w, h)))
        .map(|b| Hit::Media(b.0))
        .or_else(|| (zero && at(ZERO)).then_some(Hit::Zero))
        .or_else(|| BRIGHT.iter().position(|&r| at(r)).map(|i| Hit::Light(i == 1)))
}

/// The media buttons (all five), greyed without a phone.
pub fn draw_media_buttons(c: &mut impl Canvas, playing: bool, dim: bool) {
    for &(b, x, y, w, h) in &MEDIA {
        media::button(c, b, (x, y, w, h), b == Button::Play && playing, dim);
    }
}

/// `s` cut to the characters that fit `w` px.
pub fn fit(s: &str, w: u32) -> &str {
    let end = s.char_indices().map(|(i, _)| i).chain([s.len()]).take_while(|&i| LABEL.width(&s[..i]) <= w).last();
    s.get(..end.unwrap_or(0)).unwrap_or("")
}

/// A battery level as a small tag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Battery {
    /// Not known: nothing is drawn.
    #[default]
    Unknown,
    /// 1 to 4 segments of 4.
    Bars(u8),
    /// Low or critical: the word, in red (an alarm).
    Low,
}

/// An ANT+ battery status (common page 82: 1 new, 2 good, 3 ok, 4 low, 5 critical) as a tag.
pub fn battery_of_status(s: u8) -> Battery {
    match s {
        1 => Battery::Bars(4),
        2 => Battery::Bars(3),
        3 => Battery::Bars(2),
        4 | 5 => Battery::Low,
        _ => Battery::Unknown,
    }
}

/// A percent as a tag (a Di2).
pub fn battery_of_percent(p: u8) -> Battery {
    match p {
        0..=20 => Battery::Low,
        21..=50 => Battery::Bars(2),
        51..=75 => Battery::Bars(3),
        _ => Battery::Bars(4),
    }
}

/// The tag in its box at (`x`, `y`), 38 x 14: the box is cleared, then the bars or LOW.
pub fn draw_battery(c: &mut impl Canvas, x: u32, y: u32, b: Battery) {
    c.fill(x, y, 38, 14, BLACK);
    match b {
        Battery::Unknown => {}
        Battery::Low => {
            text::draw(c, &Style { fg: RED, ..LABEL }, x, y, Align::Left, "LOW");
        }
        Battery::Bars(n) => {
            for k in 0..4u32 {
                let on = k < u32::from(n);
                c.fill(x + k * 9, y + 2, 7, 10, if on { WHITE } else { DARK });
            }
        }
    }
}

/// A button with a centred label.
pub fn draw_button(c: &mut impl Canvas, (x, y, w, h): (u32, u32, u32, u32), bg: u16, fg: u16, label: &str) {
    c.fill(x, y, w, h, bg);
    let st = Style { fg, bg, ..LABEL };
    text::draw_in(c, &st, x + 2, y + (h - st.height()) / 2, w - 4, Align::Centre, label);
}

/// The ZERO button: the accent when it can be pressed, dark when not (a ride moves, or a request is out).
pub fn draw_zero(c: &mut impl Canvas, can: bool) {
    let (bg, fg) = if can { (ACCENT, BLACK) } else { (DARK, GREY) };
    draw_button(c, ZERO, bg, fg, "ZERO");
}

/// The brightness buttons.
pub fn draw_light_buttons(c: &mut impl Canvas) {
    draw_button(c, BRIGHT[0], DARK, WHITE, "-");
    draw_button(c, BRIGHT[1], DARK, WHITE, "+");
}

/// Text in a fixed box of one label line, cut to fit.
pub fn draw_text(c: &mut impl Canvas, (x, y, w): (u32, u32, u32), align: Align, fg: u16, s: &str) {
    text::draw_in(c, &Style { fg, ..LABEL }, x, y, w, align, fit(s, w));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_fits_and_hits() {
        assert!(BRIGHT[0].1 + BRIGHT[0].3 <= super::super::H, "the brightness row fits");
        assert!(SENSORS_Y[0] + 5 * ROW <= STATUS_Y, "five sensor lines with the power block");
        assert!(SENSORS_Y[1] + ROWS as u32 * ROW <= STATUS_Y, "six without");
        const { assert!(STATUS_Y + 14 <= BRIGHT_Y) };
        for &(b, x, y, w, h) in &MEDIA {
            assert!(w >= 44 && h >= 44, "a 44 px target");
            assert_eq!(hit((x + w / 2) as u16, (y + h / 2) as u16, true), Some(Hit::Media(b)));
        }
        assert_eq!(hit(190, (ZERO.1 + 20) as u16, true), Some(Hit::Zero));
        assert_eq!(hit(190, (ZERO.1 + 20) as u16, false), None);
        assert_eq!(hit(40, (BRIGHT_Y + 20) as u16, false), Some(Hit::Light(false)));
        assert_eq!(hit(200, (BRIGHT_Y + 20) as u16, false), Some(Hit::Light(true)));
        assert_eq!(hit(120, TITLE_Y as u16, true), None);
    }

    #[test]
    fn text_is_cut_to_its_box() {
        assert_eq!(fit("SHORT", 100), "SHORT");
        assert!(LABEL.width(fit("A VERY LONG TITLE INDEED", 96)) <= 96);
    }
}
