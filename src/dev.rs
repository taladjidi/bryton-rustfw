//! The USB dev link protocol, firmware side, free of any transport: bytes from the computer go into
//! [`Session::feed`], answers go out through [`Host::send`]. `dev.py` is the other end.
//!
//! Commands, one per line:
//! - `info`: `bryton-rustfw <version> up <ms> ms, awake <n> permille` (CPU awake in the app's work, last second).
//! - `bat`: the battery history, one line a minute, oldest first: millivolts, ` c` while charging; then `end`.
//! - `shot`: `shot 240 320`, then 240 x 320 RGB565 pixels, big endian, row by row (what the app drew).
//! - `load <size> <sum>`: `ready`, then the computer sends `size` bytes of a packed image (pack.py, without its
//!   trailer) and the firmware stores them in SDRAM; `sum` is their additive sum. Answer `ok` and the firmware
//!   runs the image ([`Effect::Run`]), or `err <reason>`.
//! - `key <page|lap|start|back> [long]`: `ok`, then the app acts as if the key was pressed, or held
//!   ([`Effect::Key`]), to script tests on the unit.
//! - `emmc`: identifies the eMMC if needed ([`Effect::Emmc`]) and answers `emmc <SEC_COUNT> <B> <OS start> <rel>`, four
//!   hex numbers (BOARD.md 15; `rel`: EXT_CSD WR_REL_PARAM << 8 | REL_WR_SEC_C, emmc.rs), or `err <why>`.
//! - `sector <n>`: reads eMMC sector `n` ([`Effect::Sector`]) and answers its 512 bytes as one line of 1024 hex
//!   digits, or `err <why>`. Read only; the eMMC is touched by nothing else, and only from the first of these two.
//! - `bench <lba> <sectors> <count>`: reads `count` times `sectors` eMMC sectors from `lba` on ([`Effect::Bench`]) and
//!   answers `bench <microseconds>` (the time the reads took, to measure the eMMC path), or `err <why>`.
//! - `bench map <level> <dx> <dy>`: draws the full-page map's basemap at the unit's position moved by (`dx`, `dy`)
//!   m, at zoom level `level` ([`Effect::BenchMap`], [`App::bench_map`]); answers `map <us> <tiles read> <bytes
//!   read> <IGN tiles decoded> <pixels> <zoom>` in hex (`err nofix` without a position): what a re-centre costs.
//! - `bench scan <kb>`: runs the scan of stock's rides over about `kb` KB ([`Effect::BenchScan`]) and answers
//!   `bench <microseconds>` (the bytes read are about `kb` KB).
//! - `ls <path>`: lists a folder of the eMMC's FAT volume ([`Effect::Ls`]): a line per entry, `<size hex> <d|f>
//!   <name>`, then `end`; or `err <why>`. `get <path> <offset> <len>`: up to 4096 bytes of a file from `offset`
//!   ([`Effect::Get`]), as one line of hex (two digits a byte; fewer at the end of the file), or `err <why>`. Read
//!   only, through the firmware's own FAT reader (storage/fat.rs): `dev.py get` copies a file off the unit.
//! - `find <lba> <count> <lo> <hi>`: reads `count` sectors (at most 65536) from `lba` ([`Effect::Find`]) and answers a
//!   line `<lba hex> <hits hex>` for each sector holding 8 or more little-endian u32 values in `lo..=hi` at any byte
//!   offset (FIT timestamps of a time window: carving a lost ride's records from free space), at most 512 lines,
//!   then `end`. Read only.
//! - `wtest <lba>`: the first write on the unit, on one sector the computer picked in free space ([`Effect::WriteTest`],
//!   [`write_test`]): reads it, writes it back changed, reads that back, writes the original back, reads that back;
//!   `ok`, or `err <step>` (`err read`, `err write`, `err pattern`, `err restore`...). Refused by a read-only build,
//!   and while a ride runs or waits to be saved (`err ride open`).
//! - `drive`: lends the eMMC's FAT volume to the computer as a USB drive ([`Effect::Drive`], app/drive.rs): `ok`,
//!   or `err <why>` (no ride may run or wait to be saved). The computer's eject gives it back.
//! - `drivelog`: the USB drive's last 32 commands, oldest first, a line each: `op lba count status residue` in hex
//!   (op 0xff a bus reset, 0xfe a Bulk-Only reset, 0xfd a CBW not valid); then `end`.
//! - `reset`: `ok`, then a system reset (bootloader, then the firmware on the eMMC).
//! - `peek <addr> [n]`: `n` (default 1, at most 64) words from `addr` on, in hex, one per line; `addr` in the
//!   peripheral space or the SDRAM (read only). `poke <addr>
//!   <value>`: writes the word, `ok`. Numbers in decimal or `0x` hex; `addr` must be word aligned and in the
//!   peripheral space ([`IO`]), so a peek can't read through a Rust reference. A reserved address there can still
//!   fault, which parks the CPU (four-button restart).

use crate::ui::{H, W};
use crate::{App, emmc};

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

/// Memory `peek` may read too (never `poke`: that could change running code or the app's state): the DTCM (data
/// and stack, 128 KB) and the SDRAM (BOARD.md 2: 64 MB from 0xd0000000, set up by the bootloader, where the image
/// runs). Both are always mapped, so a read cannot fault.
pub const MEMORY: [core::ops::RangeInclusive<u32>; 2] = [0x2000_0000..=0x2001_fffc, 0xd000_0000..=0xd3ff_fffc];

/// Largest image `load` accepts: the 1 MB link region (link.x).
pub const STAGE_MAX: u32 = 1 << 20;

/// The running figures `info` reports, measured by the main loop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Time since start.
    pub uptime_ms: u32,
    /// Share of the last second the CPU spent awake in the app's work, per mille (the rest: WFI).
    pub awake_permille: u32,
}

/// What the main loop must do after a [`Session::feed`].
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// Nothing.
    None,
    /// System reset.
    Reset,
    /// Run the staged image of this many bytes.
    Run(u32),
    /// A key press from the computer.
    Key(crate::Press),
    /// Read this eMMC sector and answer with [`send_sector`].
    Sector(u32),
    /// Answer the eMMC layout with [`send_layout`].
    Emmc,
    /// Write test on this sector ([`write_test`]).
    WriteTest(u32),
    /// Time eMMC reads: from a sector, sectors a read (1 to 32), reads; answer with [`send_bench`].
    Bench(u32, u32, u32),
    /// Find sectors with many u32 values in a range ([`find_sectors`]): from, count, low, high.
    Find(u32, u32, u32, u32),
    /// List the folder whose path [`Session::path`] holds ([`send_ls`]).
    Ls,
    /// Send this many bytes from this offset of the file whose path [`Session::path`] holds ([`send_get`]).
    Get(u32, u32),
    /// Time the scan of stock's rides over about this many bytes ([`App::bench_scan`]); answer with
    /// [`send_bench`].
    BenchScan(u32),
    /// Time the basemap of the full-page map at zoom level, moved by (dx, dy) m ([`App::bench_map`]).
    BenchMap(u32, i32, i32),
    /// Lend the card to the computer ([`App::drive_on`]); answer `ok` or `err <why>`.
    Drive,
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
    path: [u8; 64],
    path_len: usize,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// Waiting for a command.
    pub const fn new() -> Self {
        Session { line: [0; 64], len: 0, load: None, path: [0; 64], path_len: 0 }
    }

    /// The path of the last `ls` or `get`.
    pub fn path(&self) -> &str {
        core::str::from_utf8(self.path.get(..self.path_len).unwrap_or(&[])).unwrap_or("")
    }

    /// Takes bytes from the computer; `stats` is for `info`.
    pub fn feed(&mut self, mut data: &[u8], app: &App, stats: Stats, h: &mut impl Host) -> Effect {
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
                let effect = self.command(app, stats, h);
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

    fn command(&mut self, app: &App, stats: Stats, h: &mut impl Host) -> Effect {
        let line = &self.line[..self.len];
        let mut words = line.split(|&c| c == b' ').filter(|w| !w.is_empty());
        match words.next() {
            None => {}
            Some(b"info") => {
                h.send(
                    concat!("bryton-rustfw ", env!("CARGO_PKG_VERSION"), " ", env!("GIT_DESCRIBE"), " up ").as_bytes(),
                );
                h.send(decimal(stats.uptime_ms, &mut [0; 10]));
                h.send(b" ms, awake ");
                h.send(decimal(stats.awake_permille, &mut [0; 10]));
                h.send(b" permille\n");
            }
            Some(b"bat") => {
                // the battery history: one line a minute, oldest first, "mV" or "mV c" while charging; then "end"
                for (mv, charging) in app.battery_history() {
                    h.send(decimal(u32::from(mv), &mut [0; 10]));
                    h.send(if charging { b" c\n" } else { b"\n" });
                }
                h.send(b"end\n");
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
                (Some(a), Some(n @ 1..=64))
                    if a % 4 == 0
                        && [IO, MEMORY[0].clone(), MEMORY[1].clone()]
                            .iter()
                            .any(|r| r.contains(&a) && r.contains(&(a + 4 * (n - 1)))) =>
                {
                    for i in 0..n {
                        let v = h.peek(a + 4 * i);
                        h.send(&hex(v));
                    }
                }
                _ => h.send(b"err usage: peek <addr 0x40000000..0x5ffffffc or DTCM / SDRAM, aligned> [n 1..64]\n"),
            },
            Some(b"poke") => match (words.next().and_then(parse), words.next().and_then(parse)) {
                (Some(a), Some(v)) if a % 4 == 0 && IO.contains(&a) => {
                    h.poke(a, v);
                    h.send(b"ok\n");
                }
                _ => h.send(b"err usage: poke <addr 0x40000000..0x5ffffffc, aligned> <value>\n"),
            },
            Some(b"key") => match (words.next().and_then(crate::Key::from_name), words.next()) {
                (Some(k), None) => {
                    h.send(b"ok\n");
                    return Effect::Key(crate::Press::Short(k));
                }
                (Some(k), Some(b"long")) => {
                    h.send(b"ok\n");
                    return Effect::Key(crate::Press::Long(k));
                }
                _ => h.send(b"err usage: key <page|lap|start|back> [long]\n"),
            },
            Some(b"sector") => match (words.next().and_then(parse), words.next()) {
                (Some(n), None) => return Effect::Sector(n),
                _ => h.send(b"err usage: sector <n>\n"),
            },
            Some(b"emmc") => return Effect::Emmc,
            Some(b"drive") => return Effect::Drive,
            Some(b"drivelog") => {
                // the drive's last commands, oldest first: op lba count status residue (hex), then end
                for &(op, lba, count, status, residue) in app.drive_log().iter() {
                    for (k, v) in [u32::from(op), lba, count, u32::from(status), residue].into_iter().enumerate() {
                        h.send(if k == 0 { b"" } else { b" " });
                        h.send(&hex(v)[..8]);
                    }
                    h.send(b"\n");
                }
                h.send(b"end\n");
            }
            Some(b"wtest") => match (words.next().and_then(parse), words.next()) {
                // the recorder is the card's only writer while a ride is open: a cut between the flip and the
                // restore would leave that sector wrong, maybe the ride's file or the FAT (the review of 2026-10-25)
                (Some(_), None) if !app.ride_settled() => h.send(b"err ride open\n"),
                (Some(n), None) => return Effect::WriteTest(n),
                _ => h.send(b"err usage: wtest <lba>\n"),
            },
            Some(b"find") => {
                let mut n = [0u32; 4];
                for v in &mut n {
                    *v = words.next().and_then(parse).unwrap_or(u32::MAX);
                }
                match n {
                    [lba, count @ 1..=65_536, lo, hi] if lo <= hi && hi != u32::MAX => {
                        return Effect::Find(lba, count, lo, hi);
                    }
                    _ => h.send(b"err usage: find <lba> <count 1..65536> <lo> <hi>\n"),
                }
            }
            Some(cmd @ (b"ls" | b"get")) => {
                let get = cmd == b"get";
                let path = words.next().unwrap_or(b"/");
                let n = path.len().min(self.path.len());
                self.path[..n].copy_from_slice(&path[..n]);
                self.path_len = n;
                if !get {
                    return Effect::Ls;
                }
                match (words.next().and_then(parse), words.next().and_then(parse)) {
                    (Some(off), Some(len @ 1..=4096)) => return Effect::Get(off, len),
                    _ => h.send(b"err usage: get <path> <offset> <len 1..4096>\n"),
                }
            }
            Some(b"bench") if line.starts_with(b"bench map") => {
                let mut n = words.skip(1).map(|w| {
                    let (neg, w) = w.strip_prefix(b"-").map_or((false, w), |w| (true, w));
                    parse(w).and_then(|v| i32::try_from(v).ok()).map(|v| if neg { -v } else { v })
                });
                match (n.next().flatten(), n.next().flatten(), n.next().flatten()) {
                    (Some(level @ 0..=60), Some(dx), Some(dy)) => return Effect::BenchMap(level as u32, dx, dy),
                    _ => h.send(b"err usage: bench map <level> <dx m> <dy m>\n"),
                }
            }
            Some(b"bench") if line.starts_with(b"bench scan") => match words.nth(1).and_then(parse) {
                Some(kb @ 1..=65_536) => return Effect::BenchScan(kb * 1024),
                _ => h.send(b"err usage: bench scan <kb>\n"),
            },
            Some(b"bench") => {
                match (words.next().and_then(parse), words.next().and_then(parse), words.next().and_then(parse)) {
                    (Some(lba), Some(n @ 1..=32), Some(count @ 1..=100_000)) => return Effect::Bench(lba, n, count),
                    _ => h.send(b"err usage: bench <lba> <sectors 1..32> <count>\n"),
                }
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

/// The answer to `sector`: 1024 hex digits and a newline, or the error.
pub fn send_sector(h: &mut impl Host, r: Result<&[u8; emmc::SECTOR], emmc::Error>) {
    match r {
        Ok(data) => {
            for chunk in data.chunks_exact(4) {
                let w = hex(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                h.send(&w[..8]);
            }
            h.send(b"\n");
        }
        Err(e) => send_error(h, e),
    }
}

/// The answer to `wtest`: sector `lba` read, written inverted, read back and compared, then the original written
/// and read back. Every step on the card's own write path ([`emmc::Emmc::write`]: reliable writes when offered).
pub fn write_test(h: &mut impl Host, e: &mut emmc::Emmc, b: &mut impl crate::Board, lba: u32) {
    let (mut orig, mut got) = ([0u8; emmc::SECTOR], [0u8; emmc::SECTOR]);
    let step = (|| -> Result<(), &str> {
        e.read(b, lba, &mut orig).map_err(|_| "read")?;
        let flipped = orig.map(|x| !x);
        e.write(b, lba, &flipped).map_err(|_| "write")?;
        e.read(b, lba, &mut got).map_err(|_| "read back")?;
        let same = got == flipped;
        e.write(b, lba, &orig).map_err(|_| "restore")?;
        if !same {
            return Err("pattern");
        }
        e.read(b, lba, &mut got).map_err(|_| "read restored")?;
        if got == orig { Ok(()) } else { Err("restored differs") }
    })();
    match step {
        Ok(()) => h.send(b"ok\n"),
        Err(why) => {
            h.send(b"err ");
            h.send(why.as_bytes());
            h.send(b"\n");
        }
    }
}

/// The answer to `find`: reads `count` sectors from `lba` with `read` (16 at a time) and sends each sector holding
/// at least 8 little-endian u32 values in `lo..=hi`, at any byte offset, as `<lba> <hits>`; then `end`.
pub fn find_sectors(
    h: &mut impl Host,
    read: &mut impl FnMut(u32, &mut [u8]) -> Result<(), emmc::Error>,
    lba: u32,
    count: u32,
    (lo, hi): (u32, u32),
) {
    let mut buf = [0u8; 16 * emmc::SECTOR];
    let (mut at, mut lines) = (lba, 0);
    let end = lba.saturating_add(count);
    while at < end && lines < 512 {
        let n = (end - at).min(16);
        let chunk = &mut buf[..n as usize * emmc::SECTOR];
        if let Err(e) = read(at, chunk) {
            return send_error(h, e);
        }
        for (k, sector) in chunk.chunks_exact(emmc::SECTOR).enumerate() {
            let hits =
                sector.windows(4).filter(|w| (lo..=hi).contains(&u32::from_le_bytes([w[0], w[1], w[2], w[3]]))).count()
                    as u32;
            if hits >= 8 && lines < 512 {
                h.send(&hex(at + k as u32)[..8]);
                h.send(b" ");
                h.send(&hex(hits));
                lines += 1;
            }
        }
        at += n;
    }
    h.send(b"end\n");
}

/// The answer to `ls`: each entry of folder `path` on the FAT volume `d` as `<size> <d|f> <name>`, then `end`.
pub fn send_ls(h: &mut impl Host, d: &mut impl crate::fat::Disk, path: &str) {
    use crate::fat;
    let mut fat = match fat::Fat::mount(d) {
        Ok(f) => f,
        Err(_) => return h.send(b"err no volume\n"),
    };
    let Ok(mut c) = fat.open(d, path).and_then(|dir| fat.cursor(dir)) else {
        return h.send(b"err not found\n");
    };
    for _ in 0..4096 {
        match fat.list(d, &mut c) {
            Ok(Some(e)) => {
                h.send(&hex(e.file.size)[..8]);
                h.send(if e.file.dir { b" d " } else { b" f " });
                h.send(e.name());
                h.send(b"\n");
            }
            Ok(None) => break,
            Err(_) => return h.send(b"err read\n"),
        }
    }
    h.send(b"end\n");
}

/// The answer to `get`: up to `len` bytes of file `path` from `offset`, as hex.
pub fn send_get(h: &mut impl Host, d: &mut impl crate::fat::Disk, path: &str, offset: u32, len: u32) {
    use crate::fat;
    let mut buf = [0u8; 4096];
    let n = match fat::Fat::mount(d).and_then(|mut fat| {
        let f = fat.open(d, path)?;
        fat.read_at(d, &f, offset, buf.get_mut(..len as usize).unwrap_or(&mut []))
    }) {
        Ok(n) => n,
        Err(_) => return h.send(b"err not found or unreadable\n"),
    };
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for chunk in buf.get(..n).unwrap_or(&[]).chunks(64) {
        let mut line = [0u8; 128];
        for (i, &b) in chunk.iter().enumerate() {
            line[2 * i] = DIGITS[usize::from(b >> 4)];
            line[2 * i + 1] = DIGITS[usize::from(b & 15)];
        }
        h.send(&line[..2 * chunk.len()]);
    }
    h.send(b"\n");
}

/// The answer to `bench map`: the time and what was read and drawn, or `err nofix`.
pub fn send_bench_map(h: &mut impl Host, r: Option<(u32, crate::tiles::Stats)>) {
    let Some((us, s)) = r else {
        h.send(b"err nofix\n");
        return;
    };
    h.send(b"map");
    for v in [us, s.tiles_read, s.bytes_read, s.ign_tiles, s.px, u32::from(s.zoom)] {
        h.send(b" ");
        h.send(&hex(v)[..8]);
    }
    h.send(b"\n");
}

/// The answer to `bench`: how long the reads took, or why they failed.
pub fn send_bench(h: &mut impl Host, r: Result<u32, emmc::Error>) {
    match r {
        Ok(us) => {
            h.send(b"bench ");
            h.send(&hex(us)[..8]);
            h.send(b"\n");
        }
        Err(e) => send_error(h, e),
    }
}

/// The answer to `emmc`: the layout and the reliable write bytes, or why it could not be read.
pub fn send_layout(h: &mut impl Host, r: Result<emmc::Card, emmc::Error>) {
    match r {
        Ok(c) => {
            let l = c.layout;
            h.send(b"emmc");
            for v in [l.sectors, l.fat_end, l.os_start, u32::from(c.rel_param) << 8 | u32::from(c.rel_sectors)] {
                h.send(b" ");
                h.send(&hex(v)[..8]);
            }
            h.send(b"\n");
        }
        Err(e) => send_error(h, e),
    }
}

/// `err cmd <n> timeout|crc|overrun`, `err busy`, `err status cmd <n> <status>`, `err range`, `err ext_csd` or `err lent as usb drive`.
fn send_error(h: &mut impl Host, e: emmc::Error) {
    use emmc::{BusError, Error};
    match e {
        Error::Bus(c, why) => {
            h.send(b"err cmd ");
            h.send(decimal(u32::from(c), &mut [0; 10]));
            h.send(match why {
                BusError::Timeout => b" timeout\n",
                BusError::Crc => b" crc\n",
                BusError::Overrun => b" overrun\n",
            });
        }
        Error::Busy => h.send(b"err busy\n"),
        Error::Status(c, s) => {
            h.send(b"err status cmd ");
            h.send(decimal(u32::from(c), &mut [0; 10]));
            h.send(b" ");
            h.send(&hex(s));
        }
        Error::Range => h.send(b"err range\n"),
        Error::ExtCsd => h.send(b"err ext_csd\n"),
        Error::Lent => h.send(b"err lent as usb drive\n"),
        Error::ReadOnly => h.send(b"err read-only build\n"),
    }
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
