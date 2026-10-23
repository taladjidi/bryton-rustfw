//! The ST7789-class panel (BOARD.md 6) as command bytes: power-up sequence, drawing (CASET / RASET / RAMWR)
//! and switching off. The board only carries the bytes ([`Board::panel_cmd`], [`Board::panel_ram_write`]).

use crate::Board;
use crate::ui::{BLACK, Canvas, H, W};

/// Software reset.
pub const SWRESET: u8 = 0x01;
/// Sleep in.
pub const SLPIN: u8 = 0x10;
/// Sleep out.
pub const SLPOUT: u8 = 0x11;
/// Display off.
pub const DISPOFF: u8 = 0x28;
/// Display on.
pub const DISPON: u8 = 0x29;
/// Column address window.
pub const CASET: u8 = 0x2a;
/// Row address window.
pub const RASET: u8 = 0x2b;
/// Memory write (pixels follow).
pub const RAMWR: u8 = 0x2c;
/// Pixel format.
pub const COLMOD: u8 = 0x3a;
/// RAM control: where pixels come from.
pub const RAMCTRL: u8 = 0xb0;

/// (command, parameters, wait after in ms): Lcd_PanelInitSequence 0xd023f950 as in stock, except RAMCTRL. Stock
/// ends with B0 10 F0 (pixels from the LTDC RGB interface); B0 00 F0 keeps RAM writes on SPI (RAMWR), so no LTDC.
pub const INIT: &[(u8, &[u8], u32)] = &[
    (SWRESET, &[], 120),
    (0x13, &[], 0),        // NORON
    (0x21, &[], 0),        // INVON
    (0x36, &[0x00], 0),    // MADCTL
    (COLMOD, &[0x05], 10), // RGB565
    (0xb7, &[0x05], 10),
    (0xbb, &[0x29], 10),
    (0xc0, &[0x2c], 10),
    (0xc2, &[0x01], 0),
    (0xc3, &[0x1d], 0),
    (0xc4, &[0x20], 0),
    (0xc6, &[0x0f], 0),
    (0xd0, &[0xa4, 0xa1], 0),
    (0xe0, &[0xf0, 0x1f, 0x2b, 0x16, 0x17, 0x0f, 0x4c, 0x44, 0x5b, 0x39, 0x16, 0x15, 0x27, 0x28], 0),
    (0xe1, &[0xf0, 0x13, 0x1e, 0x09, 0x0a, 0x15, 0x4c, 0x43, 0x5b, 0x0f, 0x1d, 0x1d, 0x28, 0x29], 0),
    (RAMCTRL, &[0x00, 0xf0], 0),
    (SLPOUT, &[], 120),
    (0xb1, &[0x41, 0x04, 0x0a], 0),
    (0xb2, &[0x0c, 0x0c, 0x00, 0x33, 0x33], 0),
];

/// Powers the panel, runs [`INIT`], clears it to black, then switches the display on.
pub fn init(b: &mut impl Board) {
    b.panel_power_on();
    for &(cmd, params, wait) in INIT {
        b.panel_cmd(cmd, params);
        if wait > 0 {
            b.delay_ms(wait);
        }
    }
    Panel(b).fill(0, 0, W, H, BLACK);
    b.panel_cmd(DISPON, &[]);
}

/// Display off and sleep, before standby.
pub fn off(b: &mut impl Board) {
    b.panel_cmd(DISPOFF, &[]);
    b.panel_cmd(SLPIN, &[]);
}

/// The panel as a [`Canvas`]: each fill or blit is a CASET / RASET window and one RAMWR of `w * h` pixels.
pub struct Panel<'a, B: Board>(pub &'a mut B);

impl<B: Board> Panel<'_, B> {
    /// CASET / RASET: the window the next RAMWR fills.
    fn window(&mut self, x: u32, y: u32, w: u32, h: u32) {
        let (x1, y1) = (x + w - 1, y + h - 1);
        self.0.panel_cmd(CASET, &[(x >> 8) as u8, x as u8, (x1 >> 8) as u8, x1 as u8]);
        self.0.panel_cmd(RASET, &[(y >> 8) as u8, y as u8, (y1 >> 8) as u8, y1 as u8]);
    }
}

impl<B: Board> Canvas for Panel<'_, B> {
    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, color: u16) {
        if w == 0 || h == 0 {
            return;
        }
        self.window(x, y, w, h);
        self.0.panel_fill(w * h, color);
    }

    fn blit(&mut self, x: u32, y: u32, w: u32, h: u32, px: &mut dyn FnMut(u32, u32) -> u16) {
        if w == 0 || h == 0 {
            return;
        }
        self.window(x, y, w, h);
        let (mut dx, mut dy) = (0, 0);
        self.0.panel_ram_write(w * h, &mut || {
            let p = px(dx, dy);
            dx += 1;
            if dx == w {
                (dx, dy) = (0, dy + 1);
            }
            p
        });
    }
}
