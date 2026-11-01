//! The ST7789-class panel (BOARD.md 6) as command bytes: power-up sequence, drawing (CASET / RASET / RAMWR)
//! and switching off. The board only carries the bytes ([`Board::panel_cmd`], [`Board::panel_ram_write`]).

use crate::Board;
use crate::text::Face;
use crate::ui::{BLACK, Canvas, H, Theme, W};

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
    Panel(b, (0, H), Theme::DARK).fill(0, 0, W, H, BLACK);
    b.panel_cmd(DISPON, &[]);
}

/// Display off and sleep, before standby.
pub fn off(b: &mut impl Board) {
    b.panel_cmd(DISPOFF, &[]);
    b.panel_cmd(SLPIN, &[]);
}

/// The panel as a [`Canvas`]: each fill or blit is a CASET / RASET window and one RAMWR of `w * h` pixels. Only
/// rows from the second field's first to before its second are drawn (not what a banner covers, or only what it
/// covered): `(0, H)` for the whole screen. The last field is the FONT setting and the colours ([`Canvas::face`], [`Canvas::ink`]).
pub struct Panel<'a, B: Board>(pub &'a mut B, pub (u32, u32), pub Theme);

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
        let (top, bottom) = (y.max(self.1.0), (y + h).min(self.1.1));
        if w == 0 || top >= bottom {
            return;
        }
        self.window(x, top, w, bottom - top);
        let color = self.2.ink(color);
        self.0.panel_fill(w * (bottom - top), color);
    }

    fn blit(&mut self, x: u32, y: u32, w: u32, h: u32, px: &mut dyn FnMut(u32, u32) -> u16) {
        let (top, bottom) = (y.max(self.1.0), (y + h).min(self.1.1));
        if w == 0 || top >= bottom {
            return;
        }
        let h = bottom - top;
        self.window(x, top, w, h);
        let (mut dx, mut dy) = (0, top - y);
        self.0.panel_ram_write(w * h, &mut || {
            let p = px(dx, dy);
            dx += 1;
            if dx == w {
                (dx, dy) = (0, dy + 1);
            }
            p
        });
    }

    fn clip(&self) -> (u32, u32, u32, u32) {
        (0, self.1.0, crate::ui::W, self.1.1 - self.1.0)
    }

    fn face(&self) -> Face {
        self.2.face
    }

    fn ink(&self, c: u16) -> u16 {
        self.2.ink(c)
    }

    fn theme(&self) -> &crate::ui::Theme {
        &self.2
    }
}

/// Pixels in one SPI transfer at most: SPI1's TSIZE (RM0399 SPI_CR2) is 16 bits, 2 bytes a pixel.
pub const TRANSFER_MAX: u32 = 0x7ffe;

/// The SPI transfers a RAMWR of `count` pixels goes in (main.rs `lcd_ram`): [`TRANSFER_MAX`] each, the rest last.
///
/// ```
/// use bryton_rustfw::panel::{TRANSFER_MAX, transfers};
/// assert!(transfers(2 * TRANSFER_MAX + 1).eq([TRANSFER_MAX, TRANSFER_MAX, 1]));
/// assert_eq!(transfers(0).count(), 0);
/// ```
pub fn transfers(count: u32) -> impl Iterator<Item = u32> {
    let mut left = count;
    core::iter::from_fn(move || {
        let n = left.min(TRANSFER_MAX);
        left -= n;
        (n > 0).then_some(n)
    })
}

/// The panel's DMA stream (main.rs: DMA1 stream 0 into SPI1, hw.rs `PanelDma`), as [`dma_fill`] and [`dma_blit`]
/// drive it: two buffers of [`Stream::WORDS`] words, each 32-bit word two pixels.
pub trait Stream {
    /// Words in each buffer.
    const WORDS: usize;
    /// Buffer `i` (0 or 1), or None while the stream reads it.
    fn buf(&mut self, i: usize) -> Option<&mut [u32]>;
    /// Starts sending words 0..n of buffer `i`, or its word 0 `n` times when `repeat`; false if it did not start.
    /// The SPI transfer runs afterwards either way, so the CPU can finish what the stream did not send.
    fn go(&mut self, i: usize, n: usize, repeat: bool) -> bool;
    /// Waits for what was started to be sent: false on an error or a timeout.
    fn wait(&mut self) -> bool;
    /// Stops the stream: the words it had not sent, or None when it will not stop.
    fn stop(&mut self) -> Option<usize>;
}

/// `words` copies of `word` by the stream, as one repeat. Err gives the words still to send by the CPU (all of them
/// when it did not start, what it had not sent on a failure); Err(None) when it will not stop: nothing more may be
/// written to the SPI.
pub fn dma_fill(d: &mut impl Stream, words: usize, word: u32) -> Result<(), Option<usize>> {
    let Some(w0) = d.buf(0).and_then(|b| b.first_mut()) else { return Err(Some(words)) };
    *w0 = word;
    if !d.go(0, words, true) {
        return Err(Some(words));
    }
    if d.wait() { Ok(()) } else { Err(d.stop().map(|rest| rest.min(words))) }
}

/// `words` words from `next` by the stream, double-buffered (the CPU fills one buffer while the stream sends the
/// other). On a failure, whatever the stream had not sent of the words already taken from `next` goes by `cpu`, and
/// Err gives the words of `next` still to send; Err(None) when the stream will not stop: nothing more may be written
/// to the SPI (the transfer is left short: main.rs then takes SPI1 as dead).
pub fn dma_blit<S: Stream>(
    d: &mut S,
    words: usize,
    next: &mut impl FnMut() -> u32,
    cpu: &mut impl FnMut(u32),
) -> Result<(), Option<usize>> {
    let mut left = words;
    let mut flight: Option<(usize, usize)> = None; // buffer, words in it
    let mut cur = 0;
    loop {
        // the next buffer, filled while the stream sends the one in flight
        let k = left.min(S::WORDS);
        let ready = match d.buf(cur) {
            Some(b) if k > 0 => {
                for w in b.iter_mut().take(k) {
                    *w = next();
                }
                left -= k;
                Some((cur, k))
            }
            _ => None,
        };
        if let Some((i, n)) = flight
            && !d.wait()
        {
            let Some(rest) = d.stop().map(|rest| rest.min(n)) else { return Err(None) };
            if let Some(b) = d.buf(i) {
                b.get(n - rest..n).unwrap_or(&[]).iter().for_each(|&w| cpu(w));
            }
            if let Some((j, m)) = ready
                && let Some(b) = d.buf(j)
            {
                b.iter().take(m).for_each(|&w| cpu(w));
            }
            return Err(Some(left));
        }
        let Some((i, n)) = ready else {
            return if left == 0 { Ok(()) } else { Err(Some(left)) };
        };
        if !d.go(i, n, false) {
            // not started (the stream is idle here, so not expected): these words by the CPU, and the rest
            if let Some(b) = d.buf(i) {
                b.iter().take(n).for_each(|&w| cpu(w));
            }
            return Err(Some(left));
        }
        flight = Some((i, n));
        cur ^= 1;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::cell::RefCell;
    use std::vec::Vec;

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Op {
        Buf,
        Go,
        Wait,
    }

    /// A stream of 4-word buffers that sends into `wire`, fails `fail`'s op at its nth call and, when `stuck`, will
    /// not stop; a failed wait has sent all but `rest(n)` of its `n` words.
    struct Fake<'a> {
        bufs: [[u32; 4]; 2],
        /// Started: buffer, words, repeat (still set after a failed wait, until the stop).
        flight: Option<(usize, usize, bool)>,
        unsent: usize,
        wire: &'a RefCell<Vec<u32>>,
        fail: Option<(Op, usize)>,
        stuck: bool,
        calls: [usize; 3],
        rest: fn(usize) -> usize,
    }

    impl<'a> Fake<'a> {
        fn new(wire: &'a RefCell<Vec<u32>>, fail: Option<(Op, usize)>, stuck: bool, rest: fn(usize) -> usize) -> Self {
            Fake { bufs: [[0; 4]; 2], flight: None, unsent: 0, wire, fail, stuck, calls: [0; 3], rest }
        }
        fn fails(&mut self, op: Op) -> bool {
            let n = &mut self.calls[op as usize];
            *n += 1;
            self.fail == Some((op, *n - 1))
        }
        fn send(&self, i: usize, n: usize, repeat: bool) {
            let b = self.bufs[i];
            self.wire.borrow_mut().extend((0..n).map(|k| if repeat { b[0] } else { b[k] }));
        }
    }

    impl Stream for Fake<'_> {
        const WORDS: usize = 4;
        fn buf(&mut self, i: usize) -> Option<&mut [u32]> {
            let reading = self.flight.is_some_and(|f| f.0 == i);
            if self.fails(Op::Buf) || reading { None } else { Some(&mut self.bufs[i]) }
        }
        fn go(&mut self, i: usize, n: usize, repeat: bool) -> bool {
            assert!(self.flight.is_none(), "started while the stream runs");
            assert!(n > 0 && (repeat || n <= 4));
            if self.fails(Op::Go) {
                return false;
            }
            self.flight = Some((i, n, repeat));
            true
        }
        fn wait(&mut self) -> bool {
            let Some((i, n, repeat)) = self.flight else { return true };
            if self.fails(Op::Wait) {
                self.unsent = (self.rest)(n).min(n);
                self.send(i, n - self.unsent, repeat);
                return false;
            }
            self.send(i, n, repeat);
            self.flight = None;
            true
        }
        fn stop(&mut self) -> Option<usize> {
            if self.stuck {
                return None;
            }
            self.flight = None;
            Some(core::mem::take(&mut self.unsent))
        }
    }

    const RESTS: [fn(usize) -> usize; 4] = [|_| 0, |_| 1, |n| n / 2, |n| n];

    /// The faults to try: none, then each op failed at each of its first calls.
    fn faults() -> impl Iterator<Item = Option<(Op, usize)>> {
        [None]
            .into_iter()
            .chain([Op::Buf, Op::Go, Op::Wait].into_iter().flat_map(|op| (0..12).map(move |n| Some((op, n)))))
    }

    /// Every op failed at each of its calls, for every split of a failed wait: the wire gets every word once, in
    /// order, once the caller sends what Err gives by the CPU; or, the stream stuck (Err(None)), a prefix of them.
    #[test]
    fn blit_sends_every_word_once_whatever_fails() {
        let mut stuck_seen = 0;
        for words in [1, 3, 4, 5, 8, 9, 17] {
            for rest in RESTS {
                for fail in faults() {
                    for stuck in [false, true] {
                        let wire = RefCell::new(Vec::new());
                        let mut d = Fake::new(&wire, fail, stuck, rest);
                        let mut k = 0;
                        let mut next = || {
                            k += 1;
                            k
                        };
                        let r = dma_blit(&mut d, words, &mut next, &mut |w| wire.borrow_mut().push(w));
                        let want: Vec<u32> = (1..=words as u32).collect();
                        match r {
                            Ok(()) => assert_eq!(*wire.borrow(), want, "{words} {fail:?}"),
                            Err(Some(left)) => {
                                wire.borrow_mut().extend((0..left).map(|_| next()));
                                assert_eq!(*wire.borrow(), want, "{words} {fail:?}");
                            }
                            Err(None) => {
                                assert!(stuck && want.starts_with(&wire.borrow()), "{words} {fail:?}");
                                stuck_seen += 1;
                            }
                        }
                        if fail.is_none() {
                            assert_eq!(r, Ok(()));
                        }
                    }
                }
            }
        }
        assert!(stuck_seen > 0);
    }

    #[test]
    fn fill_sends_its_word_every_time_whatever_fails() {
        for words in [1, 2, 300] {
            for rest in RESTS {
                for fail in faults() {
                    for stuck in [false, true] {
                        let wire = RefCell::new(Vec::new());
                        let mut d = Fake::new(&wire, fail, stuck, rest);
                        let r = dma_fill(&mut d, words, 0xabcd_1234);
                        match r {
                            Ok(()) => {}
                            Err(Some(left)) => wire.borrow_mut().extend((0..left).map(|_| 0xabcd_1234)),
                            Err(None) => assert!(stuck && fail == Some((Op::Wait, 0))),
                        }
                        let w = wire.borrow();
                        assert!(w.iter().all(|&x| x == 0xabcd_1234) && w.len() <= words);
                        if r != Err(None) {
                            assert_eq!(w.len(), words, "{words} {fail:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn transfers_cover_the_count() {
        for count in [1, 2, TRANSFER_MAX - 1, TRANSFER_MAX, TRANSFER_MAX + 1, 240 * 320, u32::MAX] {
            let t: Vec<u32> = transfers(count).collect();
            assert_eq!(t.iter().map(|&n| u64::from(n)).sum::<u64>(), u64::from(count));
            assert!(t.iter().all(|&n| (1..=TRANSFER_MAX).contains(&n)));
            assert!(t.iter().rev().skip(1).all(|&n| n == TRANSFER_MAX), "only the last one short");
        }
    }
}
