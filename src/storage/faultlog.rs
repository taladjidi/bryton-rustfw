//! The fault log: what hw.rs keeps of a fault or a panic in the fault marker (RTC backup register 29, [`crate::FAULT_BKP`]),
//! and the line the app appends for it to `\Rustfw\Log\faults.log` after the reset (app/faultlog.rs).
//!
//! **The marker** (one word, none of the 32 backup registers being free: settings.rs), written by hw.rs's handler
//! with [`marker`] just before it resets the unit:
//!
//! ```text
//! 31..24  0xfa                      a fault happened (as every build since the marker wrote it)
//! 23..11  detail (13 bits)          a panic: its source line (at most 8191); a CPU fault: its CFSR bits ([`CFSR_BITS`])
//! 10      logged                    set by the app once the line is in the log; a new fault's marker has it clear
//! 9..8    kind                      [`Kind`]: 0 for a marker an earlier build wrote; 3 with a detail: [`Kind::Cell`]
//! 7..0    faults in a row           as before: the handler parks the unit past 3; the app clears the word after a minute
//! ```
//!
//! What it cannot hold, so the line does not have it: the PC and LR of a CPU fault (the stacked frame would need a
//! new assembly entry in the fault handler, and the 13 bits left cannot hold an address), a panic's file, and the
//! uptime the fault came at. The line has the time the unit started again (seconds after the fault), the kind and its
//! detail, the faults in a row, and the build that reads the marker (normally the one that faulted).
//!
//! CFSR: the Arm Cortex-M7 Devices Generic User Guide (DUI 0646), "Configurable Fault Status Register" (0xE000ED28):
//! MMFSR in bits 0..7 (IACCVIOL 0, DACCVIOL 1), BFSR in 8..15 (IBUSERR 8, PRECISERR 9, IMPRECISERR 10, UNSTKERR 11,
//! STKERR 12), UFSR in 16..31 (UNDEFINSTR 16, INVSTATE 17, INVPC 18, NOCP 19, UNALIGNED 24, DIVBYZERO 25). The MPU's
//! stacking bits (MUNSTKERR, MSTKERR, MLSPERR) and the address-valid flags are left out: 13 bits.
//!
//! **The log** is plain text, a line each, in this firmware's own folder (AGENTS.md, "Share the storage with stock"),
//! in a folder of its own so its growing entry is never in `\Rustfw`'s directory sectors, which hold `ride.rec`'s
//! and the ride folders' entries (storage/recorder/mod.rs, "The app's files in `\Rustfw`"). An append rewrites the
//! file's last sector with the line added, then its size ([`Fat::set_size`]): a cut between leaves the file as it
//! was. A line that would take it past [`CAP`] starts it again (the old lines dropped): the line written at its start,
//! then the size, which frees the rest. Never emptied first: a cut then left an empty file on a cluster, which
//! fsck_msdos rejects (B1b-2's review). A cut leaves the old log, the new line alone, or (between the line and the
//! size) the new line then the old log from the next sector on.

use crate::fat::{DiskWrite, Error, Fat};
use crate::text::Line;

/// The log's folder, in [`crate::recorder::DIR`].
pub const LOG_DIR: &str = "Log";

/// The log's name.
pub const FILE: &str = "faults.log";

/// The log's largest size: past it, it starts again.
pub const CAP: u32 = 64 * 1024;

/// The marker's tag (its top byte).
pub const TAG: u32 = 0xfa;

/// Set in the marker once its line is in the log.
pub const LOGGED: u32 = 1 << 10;

/// What ended the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Kind {
    /// A marker an earlier build wrote: no kind kept.
    Unknown = 0,
    /// A CPU fault (every one escalates to HardFault: hw.rs), its CFSR bits as the detail.
    Cpu = 1,
    /// A panic, its line as the detail.
    Panic = 2,
    /// The stack canary found overwritten (app/main_loop.rs `stack_ok`), detail 0.
    Stack = 3,
    /// A static cell built twice (main.rs `once`), its line as the detail. The two kind bits are full (bit 10 is
    /// [`LOGGED`]), so it is kind 3 with a non-zero detail: an earlier build reads it as the canary.
    Cell = 4,
}

/// The CFSR bits kept, in the detail's order (module doc), with their names as the log writes them.
pub const CFSR_BITS: [(u8, &str); 13] = [
    (0, "IACCVIOL"),
    (1, "DACCVIOL"),
    (8, "IBUSERR"),
    (9, "PRECISERR"),
    (10, "IMPRECISERR"),
    (11, "UNSTKERR"),
    (12, "STKERR"),
    (16, "UNDEFINSTR"),
    (17, "INVSTATE"),
    (18, "INVPC"),
    (19, "NOCP"),
    (24, "UNALIGNED"),
    (25, "DIVBYZERO"),
];

/// A CPU fault's detail: the [`CFSR_BITS`] set in `cfsr`.
///
/// ```
/// use bryton_rustfw::storage::faultlog::cfsr_detail;
/// assert_eq!(cfsr_detail(1 << 16 | 1 << 9), 1 << 7 | 1 << 3);
/// ```
#[inline(always)]
pub fn cfsr_detail(cfsr: u32) -> u32 {
    let mut d = 0;
    for (i, bit) in CFSR_BITS.iter().enumerate() {
        d |= (cfsr >> bit.0 & 1) << i;
    }
    d
}

/// The marker a fault writes over `prev` (hw.rs): one more in a row if `prev` is a marker, the kind and detail
/// (clamped to 13 bits), not logged.
///
/// ```
/// use bryton_rustfw::storage::faultlog::{Kind, marker};
/// let m = marker(0, Kind::Panic, 1234);
/// assert_eq!((m >> 24, m & 0xff), (0xfa, 1));
/// assert_eq!(marker(m, Kind::Stack, 0) & 0xff, 2);
/// ```
#[inline(always)]
pub fn marker(prev: u32, kind: Kind, detail: u32) -> u32 {
    let n = if prev >> 24 == TAG { (prev & 0xff) + 1 } else { 1 };
    let (kind, detail) = match kind {
        Kind::Cell => (Kind::Stack, detail.clamp(1, 0x1fff)),
        Kind::Stack => (Kind::Stack, 0),
        k => (k, detail.min(0x1fff)),
    };
    TAG << 24 | detail << 11 | (kind as u32) << 8 | n.min(0xff)
}

/// The kind a marker holds.
pub fn kind(word: u32) -> Kind {
    match word >> 8 & 3 {
        1 => Kind::Cpu,
        2 => Kind::Panic,
        3 if word >> 11 & 0x1fff != 0 => Kind::Cell,
        3 => Kind::Stack,
        _ => Kind::Unknown,
    }
}

/// The log's line for marker `word`, found at local time `t` by build `build`, its newline included.
///
/// ```
/// use bryton_rustfw::storage::faultlog::{Kind, line, marker};
/// let l = line(marker(0, Kind::Panic, 412), (2026, 10, 30, 9, 0, 5), "abc1234");
/// assert_eq!(l.as_str(), "2026-10-30 09:00:05 panic at line 412, 1 in a row, build abc1234\n");
/// ```
pub fn line(word: u32, (y, mo, d, h, mi, s): (u16, u8, u8, u8, u8, u8), build: &str) -> Line<256> {
    let mut l = Line::new();
    let two = |l: &mut Line<256>, v: u8| {
        if v < 10 {
            l.push_str("0");
        }
        l.push_u32(u32::from(v));
    };
    l.push_u32(u32::from(y));
    for (sep, v) in [("-", mo), ("-", d), (" ", h), (":", mi), (":", s)] {
        l.push_str(sep);
        two(&mut l, v);
    }
    let detail = word >> 11 & 0x1fff;
    match kind(word) {
        Kind::Unknown => l.push_str(" fault (kind not kept by that build)"),
        Kind::Stack => l.push_str(" stack overflow (canary)"),
        Kind::Cell => {
            l.push_str(" static cell built twice at main.rs line ");
            l.push_u32(detail);
        }
        Kind::Panic => {
            l.push_str(" panic at line ");
            l.push_u32(detail);
        }
        Kind::Cpu => {
            l.push_str(" CPU fault");
            for (i, (_, name)) in CFSR_BITS.iter().enumerate() {
                if detail >> i & 1 != 0 {
                    l.push_str(" ");
                    l.push_str(name);
                }
            }
        }
    }
    l.push_str(", ");
    l.push_u32((word & 0xff).max(1)); // 0 after the minute cleared the count
    l.push_str(" in a row, build ");
    l.push_str(build.get(..40).unwrap_or(build));
    l.push_str("\n");
    l
}

/// Where a line of `len` bytes goes in a log of `size` bytes: at its end, or at 0 (the log started again) when it
/// would pass [`CAP`].
///
/// ```
/// use bryton_rustfw::storage::faultlog::{CAP, start};
/// assert_eq!((start(100, 60), start(CAP - 60, 60), start(CAP - 59, 60), start(u32::MAX, 60)), (100, CAP - 60, 0, 0));
/// ```
pub fn start(size: u32, len: u32) -> u32 {
    if size.checked_add(len).is_some_and(|end| end <= CAP) { size } else { 0 }
}

/// Appends `line` (at most 512 bytes) to the log in folder `top`'s [`LOG_DIR`], the log made if missing. The folder is
/// never made here (it would be an entry in `top`'s directory sector, `ride.rec`'s): `NotFound` until a ride's start
/// makes it (storage/recorder/mod.rs `make_kept`).
pub fn append(
    fat: &mut Fat,
    d: &mut impl DiskWrite,
    top: crate::fat::File,
    line: &[u8],
    t: (u16, u16),
) -> Result<(), Error> {
    let len = u32::try_from(line.len()).ok().filter(|&n| (1..=512).contains(&n)).ok_or(Error::Refused)?;
    let dir = fat.find(d, top, LOG_DIR).and_then(|f| if f.dir { Ok(f) } else { Err(Error::Refused) })?;
    let mut h = match fat.locate(d, dir, FILE) {
        Err(Error::NotFound) => fat.create(d, dir, FILE, t)?,
        other => other?,
    };
    // a size its chain cannot hold (a damaged entry) starts it again, rather than refusing every line to come
    let held = u64::from(h.clusters) * u64::from(fat.cluster_bytes());
    // started again (at 0): never emptied first, which left an empty file on a cluster at a cut (fsck_msdos: "too
    // many clusters allocated", B1b-2's review); the line written over the start, then the size cuts the rest. A cut
    // between leaves the old size: the new line first, then the old log from that sector on
    let at = if u64::from(h.size) > held { 0 } else { start(h.size, len) };
    let (first, keep) = (at / 512, (at % 512) as usize);
    // the line crosses one sector boundary at most: two sectors
    let mut buf = [0u8; 1024];
    if keep > 0 {
        let mut s = [0u8; 512];
        fat.read_sector(d, &h, first, &mut s)?;
        buf.get_mut(..keep).ok_or(Error::Corrupt)?.copy_from_slice(s.get(..keep).ok_or(Error::Corrupt)?);
    }
    buf.get_mut(keep..keep + line.len()).ok_or(Error::Corrupt)?.copy_from_slice(line);
    let end = at + len;
    let sectors = (end.div_ceil(512) - first) as usize;
    fat.write_sectors(d, &mut h, first, buf.get(..sectors * 512).ok_or(Error::Corrupt)?)?;
    fat.set_size(d, &mut h, end, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn markers_count_and_keep_kind_and_detail() {
        let m = marker(0xfa00_0003 | LOGGED, Kind::Cpu, cfsr_detail(1 << 25 | 1 << 1));
        assert_eq!(m & 0xff, 4, "one more in a row");
        assert_eq!(m & LOGGED, 0, "a new fault is not logged yet");
        assert_eq!(kind(m), Kind::Cpu);
        let l = line(m, (2026, 1, 2, 3, 4, 5), "b");
        assert_eq!(l.as_str(), "2026-01-02 03:04:05 CPU fault DACCVIOL DIVBYZERO, 4 in a row, build b\n");
        assert_eq!(marker(0x1234_5678, Kind::Stack, 0) & 0xff, 1, "not a marker: the first");
        assert_eq!(marker(0xfa00_00ff, Kind::Panic, 99_999) >> 11 & 0x1fff, 0x1fff, "count and line clamped");
        assert_eq!(marker(0xfa00_00ff, Kind::Panic, 0) & 0xff, 0xff);
        let c = marker(0, Kind::Cell, 1734);
        assert_eq!((kind(c), c >> 8 & 3), (Kind::Cell, 3), "a cell: kind 3 with its line");
        assert_eq!(kind(marker(0, Kind::Cell, 0)), Kind::Cell, "line 0 still a cell");
        assert_eq!(kind(marker(0, Kind::Stack, 9)), Kind::Stack, "the canary keeps detail 0");
        assert!(line(c, (2026, 1, 2, 3, 4, 5), "b").as_str().contains("static cell built twice at main.rs line 1734"));
        assert_eq!(cfsr_detail(u32::MAX), 0x1fff);
        assert!(line(0xfa00_0001, (2026, 1, 2, 3, 4, 5), "b").as_str().contains("kind not kept"));
        // the longest line fits its buffer, the newline kept
        let long = line(marker(0, Kind::Cpu, 0x1fff), (2026, 12, 31, 23, 59, 59), &"x".repeat(300));
        assert!(long.as_str().ends_with('\n'), "{}", long.as_str());
    }

    /// B1b-2's review (3): the log started again (full to [`CAP`]) cut after every sector, then the boot's FAT sync:
    /// never an empty log on a cluster (fsck_msdos: "too many clusters allocated"), the volume as fsck says (a chain
    /// past its size only between the size and its trim), the log the old one, the new line alone, or the new line
    /// then the old log from the next sector; and the next line makes it whole. The cut between the line and the
    /// size is in target/fat (check.sh runs fsck_msdos on it).
    #[test]
    fn a_restart_cut_at_every_sector() {
        use crate::fat::tests::{Ram, read_all};
        use crate::fat::write::tests::{Cut, fsck, fsck_cut};
        const T: (u16, u16) = (0, 0);
        let mut d = Ram(crate::fat::tests::image(), 0);
        let mut fat = Fat::mount(&mut d).unwrap();
        let top = fat.ensure_dir(&mut d, fat.root(), crate::recorder::DIR, T).unwrap();
        let dir = fat.ensure_dir(&mut d, top, LOG_DIR, T).unwrap();
        // lines of 29 bytes up to 30 bytes short of the cap
        let old: std::vec::Vec<u8> = (0..(CAP as usize - 30) / 29)
            .flat_map(|i| std::format!("old line {i:04} padded to 32 b\n").into_bytes())
            .collect();
        let mut h = fat.create(&mut d, dir, FILE, T).unwrap();
        let mut padded = old.clone();
        padded.resize(old.len().div_ceil(512) * 512, 0);
        fat.write_sectors(&mut d, &mut h, 0, &padded).unwrap();
        fat.set_size(&mut d, &mut h, old.len() as u32, T).unwrap();
        let (img, new, next) = (d.0, [b'n'; 59].iter().chain(b"\n").copied().collect::<std::vec::Vec<u8>>(), b"next\n");
        assert_eq!(start(old.len() as u32, new.len() as u32), 0, "started again");
        let mut whole = Cut(Ram(img.clone(), 0), u32::MAX);
        let mut fat = Fat::mount(&mut whole).unwrap();
        append(&mut fat, &mut whole, top, &new, T).unwrap();
        let writes = u32::MAX - whole.1;
        let mut mixed = 0;
        for k in 0..writes {
            let what = std::format!("cut after {k} of {writes}");
            let mut c = Cut(Ram(img.clone(), 0), k);
            let mut fat = Fat::mount(&mut c).unwrap();
            assert!(append(&mut fat, &mut c, top, &new, T).is_err(), "{what}");
            let mut m = c.0;
            let mut fat = Fat::mount(&mut m).unwrap();
            fat.sync_fats(&mut m).unwrap();
            let f = fat.open(&mut m, "Rustfw/Log/faults.log").unwrap();
            assert!(f.size > 0, "{what}: never emptied");
            let scan = fsck_cut(&m.0).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert!(scan.past.iter().all(|p| p.0 == f.cluster), "{what}: {:?}", scan.past);
            let got = read_all(&mut fat, &mut m, "Rustfw/Log/faults.log").unwrap();
            let rest =
                |g: &[u8]| g.len() == old.len() && g[new.len()..512].iter().all(|&b| b == 0) && g[512..] == old[512..];
            let is_mixed = got.starts_with(&new) && rest(&got);
            assert!(got == old || got == new || is_mixed, "{what}: the log");
            if is_mixed && mixed == 0 {
                assert_eq!(fsck(&m.0), Ok(0), "{what}: between the line and the size, the volume clean");
                crate::fat::tests::dump("faultlog_restart_cut", &m.0);
            }
            mixed += usize::from(is_mixed);
            // the power back: the next line, at the end of what the cut left
            append(&mut fat, &mut m, top, next, T).unwrap();
            let then = read_all(&mut fat, &mut m, "Rustfw/Log/faults.log").unwrap();
            assert!(then == [&got[..], next].concat(), "{what}: then");
            assert!(fsck(&m.0).is_ok(), "{what}: then {:?}", fsck(&m.0));
        }
        assert!(mixed == 1 && writes >= 3, "{mixed} mixed of {writes} writes");
    }
}
