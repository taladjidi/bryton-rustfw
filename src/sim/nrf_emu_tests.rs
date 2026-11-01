//! Bryton's real radio-chip app, run by `tools/nrf_emu.py --serve` (docs/devloop.md, "The radio chip's real app"), driven
//! by the library's real `nrf` code through the nrf primitives of [`Sim`] ([`NrfExt`]). `#[ignore]`d: it needs
//! python3 (`NRF_PYTHON` may name another, as `FIT_PYTHON` does in check.sh), the `unicorn` package and the binary (`NRF_BIN`, default `~/Documents/bryton/firmware/R039/nordic.bin`);
//! without them it prints `skip:` and passes. Run it with `cargo test-host -- --ignored nrf_emu`.
//!
//! Wire protocol: see `serve` in the Python file. Time: a byte costs 10 us of emulated time (1 Mbaud), a `delay_ms`
//! advances the emulated chip by as much.

use super::*;
use crate::nrf::{self, Rx};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::Rc;
use std::{cell::RefCell, string::String, vec, vec::Vec};

struct Pipe {
    child: Child,
    to: ChildStdin,
    from: ChildStdout,
    /// Flags of the last answer: bit 0 the chip holds CTS, bit 1 the host's RTS is held, bit 2 the app is dead.
    flags: u8,
}

impl Pipe {
    fn spawn() -> Option<Pipe> {
        let bin = std::env::var("NRF_BIN").unwrap_or_else(|_| {
            format!("{}/Documents/bryton/firmware/R039/nordic.bin", std::env::var("HOME").unwrap_or_default())
        });
        let py = std::env::var("NRF_PYTHON").unwrap_or_else(|_| "python3".into());
        if !std::path::Path::new(&bin).is_file() {
            std::println!("skip: nrf_emu ({bin} missing)");
            return None;
        }
        let dir = env!("CARGO_MANIFEST_DIR");
        let tool = format!("{dir}/tools/nrf_emu.py");
        if !Command::new(&py).args(["-I", "-c", "import unicorn"]).output().is_ok_and(|o| o.status.success()) {
            std::println!("skip: nrf_emu (python unicorn missing)");
            return None;
        }
        let mut child = Command::new(&py)
            .args(["-I", tool.as_str(), "--serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let (to, from) = (child.stdin.take()?, child.stdout.take()?);
        Some(Pipe { child, to, from, flags: 0 })
    }

    /// One request; returns the answer's payload. Panics when the harness reports a hang.
    fn req(&mut self, msg: &[u8], has_payload: bool) -> Vec<u8> {
        self.to.write_all(msg).unwrap();
        self.to.flush().unwrap();
        let mut h = [0u8; 2];
        self.from.read_exact(&mut h).unwrap();
        self.flags = h[1];
        if h[0] != 0 {
            let t = self.chunk();
            panic!("the emulated chip stopped: {}", String::from_utf8_lossy(&t));
        }
        if has_payload { self.chunk() } else { Vec::new() }
    }

    /// A u16 length and that many bytes.
    fn chunk(&mut self) -> Vec<u8> {
        let mut n = [0u8; 2];
        self.from.read_exact(&mut n).unwrap();
        let mut v = vec![0; usize::from(u16::from_le_bytes(n))];
        self.from.read_exact(&mut v).unwrap();
        v
    }

    fn advance_us(&mut self, us: u32) {
        self.req(&[b"T".as_slice(), &us.to_le_bytes()].concat(), false);
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        let _ = self.to.write_all(b"Q");
        let _ = self.child.wait();
    }
}

/// Shares the pipe between the test (events, flow control) and the [`Sim`] (the primitives).
#[derive(Clone)]
struct Shared(Rc<RefCell<Pipe>>);

impl NrfExt for Shared {
    fn reset(&mut self) {
        self.0.borrow_mut().req(b"X", false);
    }

    fn send(&mut self, data: &[u8]) {
        let mut p = self.0.borrow_mut();
        let n = u16::try_from(data.len()).unwrap().to_le_bytes();
        p.req(&[b"W".as_slice(), &n, data].concat(), false);
        p.advance_us(10 * data.len() as u32);
    }

    fn recv(&mut self, buf: &mut [u8]) -> usize {
        let max = u16::try_from(buf.len()).unwrap().to_le_bytes();
        let got = self.0.borrow_mut().req(&[b"R".as_slice(), &max].concat(), true);
        buf[..got.len()].copy_from_slice(&got);
        got.len()
    }

    fn advance_ms(&mut self, ms: u32) {
        self.0.borrow_mut().advance_us(ms * 1000);
    }
}

#[test]
#[ignore = "needs python3 + unicorn + Bryton's nordic.bin; cargo test-host -- --ignored nrf_emu"]
fn nrf_emu_ant_broadcasts_through_the_real_parser() {
    let Some(pipe) = Pipe::spawn() else { return };
    let pipe = Shared(Rc::new(RefCell::new(pipe)));
    let mut sim = Sim::new(8, 0, 0);
    sim.nrf_ext = Some(Box::new(pipe.clone()));

    let v = nrf::start(&mut sim).expect("start against the real app");
    assert!(v.central, "the configured start was accepted");

    // a stand-in key: the emulated SoftDevice ignores it (the licensed one is never here)
    let mut rx = Rx::new();
    nrf::ant_start(&mut sim, &mut rx, [0xa5; 8], None, &[0]).expect("ant_start");
    nrf::ant_open(&mut sim, &mut rx, 0, 120, 8070, None).expect("ant_open");

    // 5 broadcasts, one per 250 ms of emulated time, on channel 0
    let mut req = b"B\0\x80".to_vec();
    req.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    req.extend_from_slice(&250_000u32.to_le_bytes());
    req.extend_from_slice(&5u16.to_le_bytes());
    pipe.0.borrow_mut().req(&req, false);
    let mut got = Vec::new();
    let mut buf = [0u8; 1];
    for _ in 0..1500 {
        if sim.nrf_recv(&mut buf) == 1 {
            if let Some(p) = rx.push(buf[0]) {
                got.extend(nrf::ant_broadcast(p));
                rx.keep_last();
            }
        } else {
            sim.delay_ms(1);
        }
    }
    assert_eq!(got.len(), 5, "five broadcasts read through ant_broadcast");
    assert!(got.iter().all(|&(ch, d)| ch == 0 && d == [1, 2, 3, 4, 5, 6, 7, 8]));
    assert_eq!(pipe.0.borrow().flags & 4, 0, "the app is alive");
}
