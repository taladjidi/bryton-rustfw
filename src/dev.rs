//! The USB dev link protocol, firmware side, free of any transport: bytes from the computer go into
//! [`Session::feed`], answers go out through [`Host::send`]. `dev.py` is the other end.
//!
//! Commands, one per line:
//! - `info`: `bryton-rustfw <version> up <ms>`.
//! - `shot`: `shot 240 320`, then 240 x 320 RGB565 pixels, big endian, row by row (what the app drew).
//! - `load <size> <sum>`: `ready`, then the computer sends `size` bytes of a packed image (pack.py, without its
//!   trailer) and the firmware stores them in SDRAM; `sum` is their additive sum. Answer `ok` and the firmware
//!   runs the image ([`Effect::Run`]), or `err <reason>`.
//! - `page`: `ok`, then the app acts as if Page was pressed ([`Effect::Page`]), to script tests on the unit.
//! - `reset`: `ok`, then a system reset (bootloader, then the firmware on the eMMC).
//! - `peek <addr> [n]`: `n` (default 1, at most 64) words from `addr` on, in hex, one per line. `poke <addr>
//!   <value>`: writes the word, `ok`. Numbers in decimal or `0x` hex; `addr` must be word aligned and in the
//!   peripheral space ([`IO`]), so a peek can't read through a Rust reference. A reserved address there can still
//!   fault, which parks the CPU (four-button restart).

use crate::App;
use crate::ui::{H, W};

/// The other end of the link, as the protocol needs it.
pub trait Host {
    /// Queues bytes for the computer.
    fn send(&mut self, data: &[u8]);
    /// Stores bytes of an image being loaded, `offset` bytes into the staging area.
    fn stage(&mut self, offset: u32, data: &[u8]);
    /// Reads the register at `addr` (in [`IO`], aligned).
    fn peek(&mut self, addr: u32) -> u32;
    /// Writes the register at `addr` (in [`IO`], aligned).
    fn poke(&mut self, addr: u32, value: u32);
}

/// The peripheral space (RM0399 memory map) that `peek` and `poke` may reach.
pub const IO: core::ops::RangeInclusive<u32> = 0x4000_0000..=0x5fff_fffc;

/// Largest image `load` accepts: the 1 MB link region (link.x).
pub const STAGE_MAX: u32 = 1 << 20;

/// What the main loop must do after a [`Session::feed`].
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// Nothing.
    None,
    /// System reset.
    Reset,
    /// Run the staged image of this many bytes.
    Run(u32),
    /// A Page press, from the computer.
    Page,
}

/// Bytes of the image kept for the checks: up to the image size word at 0x98.
const HEAD: usize = 0x9c;

struct Load {
    size: u32,
    sum: u32,
    got: u32,
    acc: u32,
    head: [u8; HEAD],
}

/// Protocol state: the current command line, or the image being loaded.
pub struct Session {
    line: [u8; 64],
    len: usize,
    load: Option<Load>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// Waiting for a command.
    pub const fn new() -> Self {
        Session { line: [0; 64], len: 0, load: None }
    }

    /// Takes bytes from the computer; `uptime_ms` is for `info`.
    pub fn feed(&mut self, mut data: &[u8], app: &App, uptime_ms: u32, h: &mut impl Host) -> Effect {
        while !data.is_empty() {
            if let Some(l) = &mut self.load {
                let n = data.len().min((l.size - l.got) as usize);
                h.stage(l.got, &data[..n]);
                for (i, &b) in data[..n].iter().enumerate() {
                    if let Some(slot) = l.head.get_mut(l.got as usize + i) {
                        *slot = b;
                    }
                    l.acc = l.acc.wrapping_add(u32::from(b));
                }
                l.got += n as u32;
                data = &data[n..];
                if let Some(l) = self.load.take_if(|l| l.got == l.size) {
                    match check_image(&l.head, l.size) {
                        _ if l.acc != l.sum => h.send(b"err checksum\n"),
                        Err(why) => {
                            h.send(b"err ");
                            h.send(why.as_bytes());
                            h.send(b"\n");
                        }
                        Ok(()) => {
                            h.send(b"ok\n");
                            return Effect::Run(l.size);
                        }
                    }
                }
                continue;
            }
            let b = data[0];
            data = &data[1..];
            if b == b'\n' || b == b'\r' {
                let effect = self.command(app, uptime_ms, h);
                self.len = 0;
                if effect != Effect::None {
                    return effect;
                }
            } else if self.len < self.line.len() {
                self.line[self.len] = b;
                self.len += 1;
            }
        }
        Effect::None
    }

    fn command(&mut self, app: &App, uptime_ms: u32, h: &mut impl Host) -> Effect {
        let line = &self.line[..self.len];
        let mut words = line.split(|&c| c == b' ').filter(|w| !w.is_empty());
        match words.next() {
            None => {}
            Some(b"info") => {
                h.send(concat!("bryton-rustfw ", env!("CARGO_PKG_VERSION"), " up ").as_bytes());
                h.send(decimal(uptime_ms, &mut [0; 10]));
                h.send(b" ms\n");
            }
            Some(b"shot") => {
                h.send(b"shot 240 320\n");
                let mut bytes = [0u8; 2 * W as usize];
                for y in 0..H {
                    let row = app.render_row(y);
                    for (pair, &p) in bytes.chunks_exact_mut(2).zip(&row.px) {
                        pair.copy_from_slice(&p.to_be_bytes());
                    }
                    h.send(&bytes);
                }
            }
            Some(b"load") => match (words.next().and_then(parse), words.next().and_then(parse)) {
                (Some(size), Some(sum)) if (HEAD as u32..=STAGE_MAX).contains(&size) => {
                    self.load = Some(Load { size, sum, got: 0, acc: 0, head: [0; HEAD] });
                    h.send(b"ready\n");
                }
                _ => h.send(b"err usage: load <size 156..1048576> <sum>\n"),
            },
            Some(b"peek") => match (words.next().and_then(parse), words.next().map_or(Some(1), parse)) {
                (Some(a), Some(n @ 1..=64)) if a % 4 == 0 && IO.contains(&a) && IO.contains(&(a + 4 * (n - 1))) => {
                    for i in 0..n {
                        let v = h.peek(a + 4 * i);
                        h.send(&hex(v));
                    }
                }
                _ => h.send(b"err usage: peek <addr 0x40000000..0x5ffffffc, aligned> [n 1..64]\n"),
            },
            Some(b"poke") => match (words.next().and_then(parse), words.next().and_then(parse)) {
                (Some(a), Some(v)) if a % 4 == 0 && IO.contains(&a) => {
                    h.poke(a, v);
                    h.send(b"ok\n");
                }
                _ => h.send(b"err usage: poke <addr 0x40000000..0x5ffffffc, aligned> <value>\n"),
            },
            Some(b"page") => {
                h.send(b"ok\n");
                return Effect::Page;
            }
            Some(b"reset") => {
                h.send(b"ok\n");
                return Effect::Reset;
            }
            Some(_) => h.send(b"err unknown command\n"),
        }
        Effect::None
    }
}

/// The checks pack.py makes, so a bad upload never runs: tag "R" + 3 digits at 0x94, size word at 0x98, initial
/// SP in DTCM, Thumb reset vector inside the image.
pub fn check_image(head: &[u8], size: u32) -> Result<(), &'static str> {
    let word = |o: usize| head.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (Some(sp), Some(reset), Some(size_word)) = (word(0), word(4), word(0x98)) else {
        return Err("short image");
    };
    if !(head[0x94] == b'R' && head[0x95..0x98].iter().all(u8::is_ascii_digit)) {
        return Err("no R### tag at 0x94");
    }
    if size_word != size {
        return Err("size word at 0x98 differs from the upload");
    }
    if !(0x2000_0000 < sp && sp <= 0x2002_0000) {
        return Err("initial SP not in DTCM");
    }
    if !(reset & 1 == 1 && (0xd000_0000..0xd000_0000 + size).contains(&reset)) {
        return Err("reset vector not Thumb code in the image");
    }
    Ok(())
}

/// A decimal or `0x` hex number.
fn parse(w: &[u8]) -> Option<u32> {
    let (digits, base) = match w {
        [b'0', b'x', rest @ ..] => (rest, 16),
        _ => (w, 10),
    };
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u32, |n, &c| n.checked_mul(base)?.checked_add((c as char).to_digit(base)?))
}

/// `v` as 8 hex digits and a newline.
fn hex(v: u32) -> [u8; 9] {
    let mut out = [b'\n'; 9];
    for (i, d) in out[..8].iter_mut().enumerate() {
        *d = b"0123456789abcdef"[(v >> (28 - 4 * i)) as usize & 0xf];
    }
    out
}

fn decimal(mut v: u32, buf: &mut [u8; 10]) -> &[u8] {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            return &buf[i..];
        }
    }
}
