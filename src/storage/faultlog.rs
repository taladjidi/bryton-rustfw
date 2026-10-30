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
//! 9..8    kind                      [`Kind`]: 0 for a marker an earlier build wrote
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
//! was. A line that would take it past [`CAP`] starts it again (the old lines dropped).

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
    /// The stack canary found overwritten (main.rs `stack_check`).
    Stack = 3,
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
    let mut i = 0;
    while i < CFSR_BITS.len() {
        d |= (cfsr >> CFSR_BITS[i].0 & 1) << i;
        i += 1;
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
    TAG << 24 | detail.min(0x1fff) << 11 | (kind as u32) << 8 | n.min(0xff)
}

/// The kind a marker holds.
pub fn kind(word: u32) -> Kind {
    match word >> 8 & 3 {
        1 => Kind::Cpu,
        2 => Kind::Panic,
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
    l.push_u32(word & 0xff);
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

/// Appends `line` (at most 512 bytes) to the log in folder `top` (made if missing), the log and its folder made if
/// missing.
pub fn append(
    fat: &mut Fat,
    d: &mut impl DiskWrite,
    top: crate::fat::File,
    line: &[u8],
    t: (u16, u16),
) -> Result<(), Error> {
    let len = u32::try_from(line.len()).ok().filter(|&n| (1..=512).contains(&n)).ok_or(Error::Refused)?;
    let dir = fat.ensure_dir(d, top, LOG_DIR, t)?;
    let mut h = match fat.locate(d, dir, FILE) {
        Err(Error::NotFound) => fat.create(d, dir, FILE, t)?,
        other => other?,
    };
    // a size its chain cannot hold (a damaged entry) starts it again, rather than refusing every line to come
    let held = u64::from(h.clusters) * u64::from(fat.cluster_bytes());
    let at = if u64::from(h.size) > held { 0 } else { start(h.size, len) };
    let (first, keep) = (at / 512, (at % 512) as usize);
    // the line crosses one sector boundary at most: two sectors
    let mut buf = [0u8; 1024];
    if keep > 0 {
        let mut s = [0u8; 512];
        fat.read_sector(d, &h, first, &mut s)?;
        buf[..keep].copy_from_slice(s.get(..keep).ok_or(Error::Corrupt)?);
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
        assert_eq!(cfsr_detail(u32::MAX), 0x1fff);
        assert!(line(0xfa00_0001, (2026, 1, 2, 3, 4, 5), "b").as_str().contains("kind not kept"));
        // the longest line fits its buffer, the newline kept
        let long = line(marker(0, Kind::Cpu, 0x1fff), (2026, 12, 31, 23, 59, 59), &"x".repeat(300));
        assert!(long.as_str().ends_with('\n'), "{}", long.as_str());
    }
}
