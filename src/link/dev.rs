//! The USB dev link protocol, firmware side, free of any transport: bytes from the computer go into
//! [`Session::feed`], answers go out through [`Host::send`]. `tools/dev.py` is the other end.
//!
//! Commands, one per line:
//! - `where`: `where TITLE|row/rows|LABEL` (row from 0) in a menu, else `where OVERVIEW`, `RIDE n` or
//!   `SCREEN name`; read-only (tools/walk.py moves the cursor by name).
//! - `info`: `bryton-rustfw <version> up <ms> ms, awake <n> permille ... auto off held <n>, ride idle|running|paused|ending` (CPU awake in the app's work, last second).
//! - `bat`: the battery history, one line a minute, oldest first: millivolts, ` c` while charging; then `end`.
//! - `shot`: `shot 240 320`, then 240 x 320 RGB565 pixels, big endian, row by row (what the app drew).
//! - `load <size> <sum>`: `ready`, then the computer sends `size` bytes of a packed image (tools/pack.py, without its
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
//!   only, through the firmware's own FAT reader (storage/fat.rs): `tools/dev.py get` copies a file off the unit.
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
//! - `prof`: the profiler's window (link/prof.rs): `prof <wall ms> <slept ms> <overhead cycles> <pixels> <sectors
//!   read> <sectors written>`, then a line per section `<name> <calls> <cycles> <max cycles>`, then `end`; all in
//!   decimal. `prof reset`: `ok`, and a new window starts ([`Effect::ProfReset`]).
//! - `reset`: `ok`, then a system reset (bootloader, then the firmware on the eMMC).
//! - `stackfault`: `ok`, then the firmware runs its stack down past the stack canary on purpose ([`Effect::StackFault`],
//!   main.rs `stack_fault`) and returns, so the pass's check must find it. Pass: the unit resets and RTC backup
//!   register 29 (`peek 0x580040c4`) reads 0xfa0000nn within a minute (the app clears it after a minute of running).
//!   A reset with no marker is a miss of the test (about 1 build in 38: main.rs `stack_fault`).
//!   `err ride open` while a ride runs or waits to be saved.
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
    /// Sends what [`Host::send`] queued, so the answer reaches the computer now. Default: nothing queues.
    fn flush(&mut self) {}
}

/// The peripheral space (RM0399 memory map) that `peek` and `poke` may reach.
pub const IO: core::ops::RangeInclusive<u32> = 0x4000_0000..=0x5fff_fffc;

/// Memory `peek` may read too (never `poke`: that could change running code or the app's state): the DTCM (data
/// and stack, 128 KB) and the SDRAM (BOARD.md 2: 64 MB from 0xd0000000, set up by the bootloader, where the image
/// runs). Both are always mapped, so a read cannot fault. Then, read only, the NVIC's enable, pending and active
/// registers (ISER to IABR) and the SCB's ICSR (VECTPENDING): which interrupt wakes WFI (docs/usb.md, a main loop
/// that never slept). Reading them changes nothing (ARMv7-M ARM B3.4); SysTick's CSR is left out, since reading it
/// clears COUNTFLAG, the main loop's end of sleep.
pub const MEMORY: [core::ops::RangeInclusive<u32>; 4] =
    [0x2000_0000..=0x2001_fffc, 0xd000_0000..=0xd3ff_fffc, 0xe000_e100..=0xe000_e33c, 0xe000_ed04..=0xe000_ed04];

/// Largest image `load` accepts: the 1 MB link region (link.x).
pub const STAGE_MAX: u32 = 1 << 20;

/// The running figures `info` reports, measured by the main loop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Time since start.
    pub uptime_ms: u32,
    /// Share of the last second the CPU spent awake in the app's work, per mille (the rest: WFI).
    pub awake_permille: u32,
    /// Share of the last second the CPU ran at all, per mille: the app's work and everything inside the sleep loop
    /// (USB polls, and anything that keeps WFI from sleeping: a stuck interrupt kept it near 820, 2026-10-27).
    pub cpu_permille: u32,
}

/// Configured, but no start of frame for this long (and no suspend seen): the computer dropped the device.
pub const SOF_LOST_MS: u32 = 200;
/// Configured once, then not for this long (or suspended after a failed transfer): the computer gave up on it.
pub const UNCONFIGURED_MS: u32 = 2000;

/// The USB device's state, as the board's watch reads it (usb-device's state, the crate's suspend from
/// OTG_GINTSTS.USBSUSP).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbState {
    /// Configured: the dev link is usable.
    Configured,
    /// The bus idle for 3 ms (USB 2.0 7.1.7.6; RM0399 OTG_GINTSTS USBSUSP, OTG_DSTS SUSPSTS); `configured`: it
    /// was configured just before, and not reconnected since.
    Suspended {
        /// Configured just before the suspend.
        configured: bool,
    },
    /// Default or addressed: not configured (yet, or again after a bus reset).
    Unconfigured,
}

/// What the watch makes of [`UsbState`]: the dot to show, or a drop to recover from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbWatch {
    /// Configured and starts of frame coming.
    Up,
    /// Suspended by the computer: a normal state (macOS suspends an idle device), the link resumes on its own at
    /// the next traffic (RM0399 OTG_GINTSTS WKUPINT, or a bus reset).
    Suspended,
    /// Not configured.
    Down,
    /// Dropped: reconnect (docs/usb.md, "What was seen").
    Drop,
}

/// The USB watch's decision, once per poll. `was_configured`: configured since the last reconnect; `still_ms`:
/// time in this state, or while configured, since the frame number last moved; `failed`: the drive's last command
/// ended in a phase error (its data phase found nobody taking packets for a second). A computer suspends a device
/// only when idle, never in the middle of a transfer, so a suspend after a failed transfer is a drop (the port
/// disabled, docs/usb.md 2026-10-24); any other suspend from configured is left alone, however long.
///
/// ```
/// use bryton_rustfw::dev::{UsbState, UsbWatch, watch_usb};
/// let suspended = UsbState::Suspended { configured: true };
/// assert_eq!(watch_usb(suspended, true, 3_600_000, false), UsbWatch::Suspended);
/// assert_eq!(watch_usb(suspended, true, 2000, true), UsbWatch::Drop);
/// assert_eq!(watch_usb(UsbState::Configured, true, 200, false), UsbWatch::Drop);
/// ```
pub fn watch_usb(bus: UsbState, was_configured: bool, still_ms: u32, failed: bool) -> UsbWatch {
    match bus {
        UsbState::Configured if still_ms >= SOF_LOST_MS => UsbWatch::Drop,
        UsbState::Configured => UsbWatch::Up,
        UsbState::Suspended { configured: true } if !failed => UsbWatch::Suspended,
        _ if was_configured && still_ms >= UNCONFIGURED_MS => UsbWatch::Drop,
        _ => UsbWatch::Down,
    }
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
    /// Start a new profiler window ([`crate::prof::Prof::reset`]).
    ProfReset,
    /// Run the stack down past its canary on purpose, to prove the check (main.rs `stack_fault`).
    StackFault,
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
                h.send(decimal(u64::from(stats.uptime_ms), &mut [0; 20]));
                h.send(b" ms, awake ");
                h.send(decimal(u64::from(stats.awake_permille), &mut [0; 20]));
                h.send(b" permille, cpu ");
                h.send(decimal(u64::from(stats.cpu_permille), &mut [0; 20]));
                h.send(b" permille, radio resets ");
                h.send(decimal(u64::from(app.radio_resets()), &mut [0; 20]));
                h.send(b", climb tile us last ");
                h.send(decimal(u64::from(app.ahead_tile_us().0), &mut [0; 20]));
                h.send(b" max ");
                h.send(decimal(u64::from(app.ahead_tile_us().1), &mut [0; 20]));
                h.send(b" slice max ");
                h.send(decimal(u64::from(app.ahead_tile_us().2), &mut [0; 20]));
                h.send(b", auto off held ");
                h.send(decimal(u64::from(app.auto_off_held()), &mut [0; 20]));
                h.send(b", ride ");
                h.send(app.ride_word());
                h.send(b"\n");
            }
            Some(b"where") => {
                h.send(b"where ");
                h.send(app.where_text().as_str().as_bytes());
                h.send(b"\n");
            }
            Some(b"bat") => {
                // the battery history: one line a minute, oldest first, "mV" or "mV c" while charging; then "end"
                for (mv, charging) in app.battery_history() {
                    h.send(decimal(u64::from(mv), &mut [0; 20]));
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
                        && core::iter::once(&IO)
                            .chain(&MEMORY)
                            .any(|r| r.contains(&a) && r.contains(&(a + 4 * (n - 1)))) =>
                {
                    for i in 0..n {
                        let v = h.peek(a + 4 * i);
                        h.send(&hex(v));
                    }
                }
                _ => h.send(b"err usage: peek <addr: IO, DTCM, SDRAM, NVIC, ICSR; aligned> [n 1..64]\n"),
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
            Some(b"prof") => match words.next() {
                None => send_prof(h, app.prof_figures()),
                Some(b"reset") => {
                    h.send(b"ok\n");
                    return Effect::ProfReset;
                }
                Some(_) => h.send(b"err usage: prof [reset]\n"),
            },
            Some(b"reset") => {
                h.send(b"ok\n");
                return Effect::Reset;
            }
            // a fault resumes a ride from its checkpoint, but a test has no business risking one
            Some(b"stackfault") if !app.ride_settled() => h.send(b"err ride open\n"),
            Some(b"stackfault") => {
                h.send(b"ok\n");
                return Effect::StackFault;
            }
            Some(_) => h.send(b"err unknown command\n"),
        }
        Effect::None
    }
}

/// The checks tools/pack.py makes, so a bad upload never runs: tag "R" + 3 digits at 0x94, size word at 0x98, initial
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

/// Reads `count` times `n` sectors (at most 32) from `lba` on and gives the time, us ([`Effect::Bench`]). The 16 KB
/// buffer is on this frame, not on the caller's.
#[inline(never)]
fn bench_reads(app: &mut App, b: &mut impl crate::Board, lba: u32, n: u32, count: u32) -> Result<u32, emmc::Error> {
    let mut buf = [0u8; 32 * emmc::SECTOR];
    let len = (n as usize * emmc::SECTOR).min(buf.len());
    let t = b.awake_us();
    for k in 0..count {
        app.emmc().read(b, lba + k * n, &mut buf[..len])?;
    }
    Ok(b.awake_us().wrapping_sub(t))
}

impl App {
    /// Answers the effect `session` asked for that needs the app and the board (the eMMC, the FAT reader, the
    /// benchmarks, the drive, the profiler), then flushes `h`. True when board work may have slept, which takes over
    /// SysTick: the caller's sleep slice is lost, so it ends the slice. The keys, the reset and the image run are the
    /// caller's (main.rs): false for them.
    pub fn serve(&mut self, b: &mut impl crate::Board, h: &mut impl Host, session: &Session, effect: Effect) -> bool {
        match effect {
            Effect::None | Effect::Key(_) | Effect::Reset | Effect::Run(_) | Effect::StackFault => return false,
            Effect::ProfReset => {
                self.prof().reset();
                return false;
            }
            Effect::Sector(lba) => {
                let mut buf = [0u8; emmc::SECTOR];
                let r = self.emmc().read(b, lba, &mut buf).map(|()| &buf);
                send_sector(h, r);
            }
            Effect::WriteTest(lba) => write_test(h, self.emmc(), b, lba),
            Effect::Bench(lba, n, count) => {
                let r = bench_reads(self, b, lba, n, count);
                send_bench(h, r);
            }
            Effect::BenchScan(bytes) => {
                let t = b.awake_us();
                // the bytes it read are about the ones asked (the protocol answers the time only)
                self.bench_scan(b, bytes);
                send_bench(h, Ok(b.awake_us().wrapping_sub(t)));
            }
            Effect::BenchMap(level, dx, dy) => {
                let t = b.awake_us();
                let got = self.bench_map(b, level, dx, dy);
                let us = b.awake_us().wrapping_sub(t);
                send_bench_map(h, got.map(|s| (us, s)));
            }
            Effect::Find(lba, count, lo, hi) => {
                let emmc = self.emmc();
                find_sectors(h, &mut |at, buf| emmc.read(b, at, buf), lba, count, (lo, hi));
            }
            Effect::Ls => send_ls(h, &mut emmc::Volume(self.emmc(), b), session.path()),
            Effect::Get(off, len) => send_get(h, &mut emmc::Volume(self.emmc(), b), session.path(), off, len),
            Effect::Drive => match self.drive_on(b) {
                Ok(()) => h.send(b"ok\n"),
                Err(why) => {
                    h.send(b"err ");
                    h.send(why.as_bytes());
                    h.send(b"\n");
                }
            },
            Effect::Emmc => send_layout(h, self.emmc().card(b)),
        }
        h.flush();
        true
    }
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
            h.send(decimal(u64::from(c), &mut [0; 20]));
            h.send(match why {
                BusError::Timeout => b" timeout\n",
                BusError::Crc => b" crc\n",
                BusError::Overrun => b" overrun\n",
            });
        }
        Error::Busy => h.send(b"err busy\n"),
        Error::Status(c, s) => {
            h.send(b"err status cmd ");
            h.send(decimal(u64::from(c), &mut [0; 20]));
            h.send(b" ");
            h.send(&hex(s));
        }
        Error::Range => h.send(b"err range\n"),
        Error::ExtCsd => h.send(b"err ext_csd\n"),
        Error::Lent => h.send(b"err lent as usb drive\n"),
        Error::ReadOnly => h.send(b"err read-only build\n"),
    }
}

/// The profiler's window, as `prof` answers it.
fn send_prof(h: &mut impl Host, p: &crate::prof::Prof) {
    // each number after a space
    let nums = |h: &mut dyn Host, vs: &[u64]| {
        for &v in vs {
            h.send(b" ");
            h.send(decimal(v, &mut [0; 20]));
        }
        h.send(b"\n");
    };
    h.send(b"prof");
    let (wall, slept, overhead) = (u64::from(p.wall_ms), u64::from(p.slept_ms), u64::from(p.overhead));
    nums(h, &[wall, slept, overhead, p.pixels, u64::from(p.read), u64::from(p.written)]);
    for (s, slot) in crate::prof::Sec::ALL.iter().zip(&p.slots) {
        h.send(s.name().as_bytes());
        nums(h, &[u64::from(slot.count), slot.total, u64::from(slot.max)]);
    }
    h.send(b"end\n");
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

fn decimal(mut v: u64, buf: &mut [u8; 20]) -> &[u8] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspend_is_not_a_drop() {
        let s = UsbState::Suspended { configured: true };
        // the Mac suspends an idle device: left alone, however long, the dot says so
        for ms in [0, SOF_LOST_MS, UNCONFIGURED_MS, u32::MAX] {
            assert_eq!(watch_usb(s, true, ms, false), UsbWatch::Suspended);
        }
        // a suspend right after a transfer failed is the port disabled (docs/usb.md, 2026-10-24): the old recovery
        assert_eq!(watch_usb(s, true, UNCONFIGURED_MS - 1, true), UsbWatch::Down);
        assert_eq!(watch_usb(s, true, UNCONFIGURED_MS, true), UsbWatch::Drop);
        // after that reconnect, nothing more until configured again
        let gone = UsbState::Suspended { configured: false };
        assert_eq!(watch_usb(gone, false, u32::MAX, true), UsbWatch::Down);
    }

    #[test]
    fn drops_still_recovered() {
        // configured, frame number frozen, no suspend: the green-dot drop
        assert_eq!(watch_usb(UsbState::Configured, true, SOF_LOST_MS - 1, false), UsbWatch::Up);
        assert_eq!(watch_usb(UsbState::Configured, true, SOF_LOST_MS, false), UsbWatch::Drop);
        // reset and never configured again, or suspended before being configured again
        for bus in [UsbState::Unconfigured, UsbState::Suspended { configured: false }] {
            assert_eq!(watch_usb(bus, true, UNCONFIGURED_MS - 1, false), UsbWatch::Down);
            assert_eq!(watch_usb(bus, true, UNCONFIGURED_MS, false), UsbWatch::Drop);
            // never configured (still enumerating, or nothing on the other end): no reconnect
            assert_eq!(watch_usb(bus, false, u32::MAX, false), UsbWatch::Down);
        }
    }
}
