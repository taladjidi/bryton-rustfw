//! The GNSS receiver model ([`Gnss`], the one source of its rules) served over a pipe to Renode's SPI4
//! (`renode/GnssSpi.cs`, `renode/gnss_run.py`; docs/devloop.md, "Renode with the GNSS model"). `#[ignore]`d: the
//! script starts it, a plain test run does not. The track is synthetic (a straight walk north from [`FIX_LAT`]).
//!
//! Wire protocol, numbers little endian, a reply to every request; after the line `GNSS-READY` the stream is binary:
//! - `P ms:u64 on:u8` supply on or off, reply 1 byte;
//! - `C ms:u64` chip select low, reply 1 byte; `W ms:u64 b[4]` four bytes exchanged, reply the 4 bytes from MISO;
//!   `E ms:u64` chip select high, reply 1 byte;
//! - `Q`: write the summary and exit. After a model panic the pipe is closed.
//!
//! `GNSS_LOG` (a path): a line `ev <ms> acks= naks= valsets= pvt= lost= viol=` whenever one of the counts changes, and
//! `summary` at the end. `GNSS_FAULT`: `record` (violations logged, not fatal; gnss_run's default), `nak` (the first CFG-VALSET is refused) or `stall` (nothing answered from 15 s to
//! 17 s: the stream time-out of rule G2 is then expected, not a failure).

use super::*;
use std::io::{Read, Write};
use std::{string::String, vec::Vec};

const STALL_MS: core::ops::Range<u64> = 15_000..17_000;

/// A walk north at 4.5 m/s from the fixed start (1 unit = 1e-7 degree, about 1.1 cm).
fn track(ms: u64) -> (i32, i32, i32) {
    (FIX_LAT + (ms / 1000 * 400) as i32, FIX_LON, 0)
}

/// Occurrences of `pat` starting from `*from` in `v`; `*from` moves past the windows looked at.
fn count(v: &[u8], from: &mut usize, pat: [u8; 4]) -> u32 {
    let end = v.len().saturating_sub(3);
    let n = (*from..end).filter(|&i| v[i..i + 4] == pat).count();
    *from = end.max(*from);
    n as u32
}

#[test]
#[ignore]
fn serve() {
    let fault = std::env::var("GNSS_FAULT").unwrap_or_default();
    let mut log = std::env::var("GNSS_LOG").ok().and_then(|p| std::fs::File::create(p).ok());
    let mut g = Gnss::default();
    g.fix = Some(4500);
    g.route = Some(track);
    g.expect = match fault.as_str() {
        "stall" => Some("stream time-out"),
        "record" => Some(""), // any violation is logged, not fatal
        _ => None,
    };
    let (mut i, mut o) = (std::io::stdin().lock(), std::io::stdout().lock());
    o.write_all(b"GNSS-READY\n").unwrap();
    o.flush().unwrap();
    let (mut ins, mut outs, mut at) = (Vec::new(), Vec::new(), (0, 0));
    let (mut valsets, mut pvt, mut last, mut stalled, mut ms) = (0, 0, String::new(), false, 0);
    let mut powered_once = false;
    let mut note = |g: &Gnss, ms: u64, valsets: u32, pvt: u32, tag: &str| {
        let counts = format!(
            "acks={} naks={} valsets={valsets} pvt={pvt} lost={} viol={} gap={}",
            g.acks,
            g.naks,
            g.lost_epochs,
            g.violations.len(),
            g.longest_gap_ms
        );
        if counts != last || tag == "summary" {
            if let Some(f) = log.as_mut() {
                writeln!(f, "{tag} {ms} {counts}").unwrap();
            }
            last = counts;
        }
    };
    loop {
        let mut cmd = [0u8; 1];
        if i.read_exact(&mut cmd).is_err() || cmd[0] == b'Q' {
            break;
        }
        let mut u = [0u8; 8];
        i.read_exact(&mut u).unwrap();
        ms = u64::from_le_bytes(u);
        let utc = 1_700_000_000 + (ms / 1000) as i64; // synthetic UTC
        let mut reply = [0u8; 4];
        let mut n = 1;
        match cmd[0] {
            b'P' => {
                i.read_exact(&mut u[..1]).unwrap();
                g.power(u[0] != 0, ms);
                g.nak_next = u[0] != 0 && !powered_once && fault == "nak"; // only the first power-on refuses
                powered_once |= u[0] != 0;
            }
            b'C' => {
                stalled = fault == "stall" && STALL_MS.contains(&ms);
                if !stalled {
                    g.begin(ms, utc);
                }
            }
            b'W' => {
                let mut w = [0u8; 4];
                i.read_exact(&mut w).unwrap();
                for (r, b) in reply.iter_mut().zip(w) {
                    *r = if stalled { 0xff } else { g.byte(ms, b) };
                }
                ins.extend(w);
                outs.extend(reply);
                n = 4;
            }
            _ => {
                if !stalled {
                    g.end(ms);
                }
                valsets += count(&ins, &mut at.0, [0xb5, 0x62, 0x06, 0x8a]);
                pvt += count(&outs, &mut at.1, [0xb5, 0x62, 0x01, 0x07]);
                // keep only the 3 bytes a pattern could straddle
                ins.drain(..at.0);
                outs.drain(..at.1);
                at = (0, 0);
                note(&g, ms, valsets, pvt, "ev");
            }
        }
        o.write_all(&reply[..n]).unwrap();
        o.flush().unwrap();
    }
    note(&g, ms, valsets, pvt, "summary");
}
