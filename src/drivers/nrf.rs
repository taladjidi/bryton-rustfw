//! The Nordic BLE / ANT chip (BOARD.md 11): Nordic's nRF5 serialization over a byte link. The board only moves
//! bytes ([`Board::nrf_reset`], [`Board::nrf_send`], [`Board::nrf_recv`]); the framing and the commands are here.
//!
//! Framing: a 2-byte little-endian length, then the payload `[type][op][arguments]`. A response is
//! `[RESP][op][result u32][outputs]`. There is no marker to resync on: a lost or stray byte leaves the length framing
//! out of step. [`Rx`] counts the signs of that (`strikes`, `timeouts`); `app/nrf_watch.rs` watches them, drains and
//! retries, and failing that resets the chip and starts it again.

use crate::Board;
use crate::storage::bytes;

/// Packet type: a BLE command to the chip.
pub const CMD: u8 = 0;
/// Packet type: the chip's response to a command.
pub const RESP: u8 = 1;
/// Packet type: a BLE event from the chip.
pub const BLE_EVT: u8 = 2;
/// Packet type: an ANT command to the chip. Its response comes back as [`RESP`] (seen on the unit).
pub const ANT_CMD: u8 = 6;
/// Packet type: an ANT event from the chip.
pub const ANT_EVT: u8 = 8;

/// `sd_ble_enable`: starts the BLE stack (no arguments here; the RAM base is not serialized).
pub const SD_BLE_ENABLE: u8 = 0x60;
/// `sd_ble_version_get`: the version fields (Bryton fills them with its own numbers, BOARD.md 11).
pub const SD_BLE_VERSION_GET: u8 = 0x65;
/// `sd_power_system_off`: the chip powers down until its next reset. No response.
pub const SD_POWER_SYSTEM_OFF: u8 = 0x32;
/// `sd_ble_gap_scan_start` (API v5: flags, interval, window, timeout).
pub const SD_BLE_GAP_SCAN_START: u8 = 0x8a;
/// BLE event id: an advertising report.
pub const EVT_ADV_REPORT: u16 = 0x1d;
/// BLE event id: a timeout (the scan's, here).
pub const EVT_TIMEOUT: u16 = 0x1b;
/// BLE event id: a notification or indication from the peer (BLE_GATTC_EVT_HVX).
pub const EVT_HVX: u16 = 0x39;

/// Largest payload kept; a longer one is read through and dropped.
pub const MAX: usize = 264;

/// How long a command may take before it is given up (stock waits 5 s; replies take a few ms). Not shorter: a slow
/// but alive chip must not be reset. The GNSS bound does not come from here but from [`Rx::stalled`] (one time-out
/// per span between two receiver reads) and the receiver read right after a stalled pass (app/gps.rs).
pub const TIMEOUT_MS: u32 = 500;

/// A length header past this cannot be a frame (a BLE event is at most one ATT packet, about 250 bytes, and a
/// header): it counts as garbage and is not read through, so a stray byte does not swallow up to 64 KB of real frames.
pub const GARBAGE: usize = 512;

/// Longest BLE event kept while a command waits ([`Rx::pop`]): a Chars discovery answer of 14 characteristics at
/// 11 + 14 x 9 = 137 bytes (review 2026-11-01); on the fast link (ATT MTU 247) an HVX is a 14-byte header and up to
/// 244 bytes of value (an ANCS attribute answer of about 194), so 264, [`MAX`].
pub const KEPT: usize = 264;
/// BLE event id: notifications left the queue (BLE_GATTS_EVT_HVN_TX_COMPLETE).
pub const EVT_HVN_TX_COMPLETE: u16 = 0x57;
/// Events kept at most: an ANCS answer of the longest message the inbox keeps (120 bytes) and a long title is about
/// 180 bytes, ten notifications of 20 at the 23-byte ATT MTU, read in one pass (eight lost it).
pub const KEPT_N: usize = 16;

/// Reassembles packets from the byte stream, and keeps the BLE events that arrive while a command waits for its
/// response (a phone's connection or pairing must not be lost to an ANT command's wait; advertising reports are).
pub struct Rx {
    buf: [u8; MAX],
    header: [u8; 2],
    got: usize,
    want: Option<usize>,
    /// The last packet's length (in `buf`).
    last: usize,
    kept: [[u8; KEPT]; KEPT_N],
    kept_len: [u16; KEPT_N],
    /// The oldest event kept is `kept[head]`: a ring, so a pop moves nothing.
    head: usize,
    kept_n: usize,
    /// The op of a call that timed out, whose answer may still come: the next call swallows one answer of that op.
    pub(crate) late: Option<u8>,
    /// ANT queue overflows the SoftDevice reported (EVENT_QUE_OVERFLOW): ANT events lost between the chip and us.
    /// Survives a reset (a count for `dev.py info`, next to the radio resets).
    pub ant_lost: u16,
    /// The last command that failed: its op and the error (`u32::MAX`: no answer). For the unit's first runs: the
    /// PHONE menu shows it when the link could not be set up.
    pub failed: Option<(u8, u32)>,
    /// A good frame (a type the chip sends) arrived since this was last taken (`app/nrf_watch.rs`).
    pub heard: bool,
    /// Garbage in a row since the last good frame: a length header past [`GARBAGE`], a frame of a type the chip
    /// never sends. Two in a row mean the framing is out of step.
    pub strikes: u8,
    /// Commands in a row that got no response, since the last good frame.
    pub timeouts: u8,
    /// A frame went out partly (the chip held CTS for ever): the chip is mis-framed. No frame goes to it until it is
    /// reset ([`Rx::reset`]); the watchdog resets it at once (`app/nrf_watch.rs`).
    pub broken: bool,
    /// A command timed out ([`TIMEOUT_MS`] of silence): every other command is refused at once (`Err(Timeout)`, not
    /// counted, nothing sent) until [`Rx::reset`] or the app clears it after its next receiver read (`App::step`), so
    /// a dead chip costs one time-out between two GNSS reads, not one per command. Callers that must not lose
    /// work to a refusal (the restart's stages, the probe) check it first and try again the next pass.
    pub stalled: bool,
    /// ms slept in puts and waits, always: a command's send and its wait share one [`TIMEOUT_MS`] (`call` reads the
    /// difference), so a slow send then a silent wait cannot cost twice a time-out between two receiver reads.
    paused_ms: u32,
}

impl Default for Rx {
    fn default() -> Self {
        Self::new()
    }
}

impl Rx {
    /// Waiting for a length header.
    pub const fn new() -> Self {
        Rx {
            buf: [0; MAX],
            header: [0; 2],
            got: 0,
            want: None,
            last: 0,
            kept: [[0; KEPT]; KEPT_N],
            kept_len: [0; KEPT_N],
            head: 0,
            kept_n: 0,
            late: None,
            ant_lost: 0,
            failed: None,
            heard: false,
            strikes: 0,
            timeouts: 0,
            broken: false,
            stalled: false,
            paused_ms: 0,
        }
    }

    /// Forgets the frame being read and the signs of a broken link: the stream is read again from a frame
    /// boundary (after a drain). The events already kept are whole frames and stay (a connection's end among them:
    /// review); the app asks the chip what the dropped bytes may have held (`App::phone_recheck`).
    pub fn resync(&mut self) {
        (self.got, self.want, self.last) = (0, None, 0);
        (self.strikes, self.timeouts) = (0, 0);
    }

    /// A receiver read was made: the radio budget starts again.
    pub fn span_end(&mut self) {
        self.stalled = false;
    }

    /// [`Rx::resync`] after a chip reset: the events kept belong to a link that is gone, too.
    pub fn reset(&mut self) {
        self.resync();
        (self.kept_n, self.head, self.late, self.broken) = (0, 0, None, false);
        self.span_end();
    }

    /// The last packet answered op `answer` (a response's op): true, and the swallow cleared, if that is the late
    /// answer of a call that timed out. Every reader of the stream asks (a send's drain, the main read).
    pub fn swallow(&mut self, answer: Option<u8>) -> bool {
        let late = answer.is_some() && answer == self.late;
        if late {
            self.late = None;
        }
        late
    }

    /// Keeps the last packet for [`Rx::pop`] if it is a BLE event other than an advertising report, or an ANT event,
    /// and fits ([`KEPT_N`] at most). When full, room is made by the oldest ANT broadcast (a newer one is on its way,
    /// and a lost one is a missed beat); failing that the oldest notification ([`EVT_HVX`]) makes room for another
    /// kind of BLE event; else the new one is dropped. A connection's end, a channel's close or any other BLE
    /// event is never pushed out. The main loop keeps packets too, and acts on them once the bytes read are all
    /// framed: a command sent from the middle of a read would take the rest of the stream out of order.
    pub fn keep_last(&mut self) {
        let p = self.buf.get(..self.last).unwrap_or(&[]);
        let id = event_id(p);
        let broadcast = ant_broadcast(p).is_some();
        if ant_queue_overflow(p) {
            self.ant_lost = self.ant_lost.saturating_add(1);
        }
        if p.first() == Some(&ANT_EVT) {
            // only what the app acts on: a broadcast, a close, a failed or completed acknowledged message (the
            // rest, RX_FAIL and the like, would push a close or a disconnection out of a full ring)
            let handled =
                broadcast || ant_closed(p).is_some() || ant_tx_failed(p).is_some() || ant_tx_completed(p).is_some();
            if !handled {
                return;
            }
        } else if id.is_none_or(|id| id == EVT_ADV_REPORT) {
            return;
        }
        if p.len() > KEPT {
            return;
        }
        // one TX-complete is enough (they only say "refill"): a stream of them must not push the phone's HVX out
        if id == Some(EVT_HVN_TX_COMPLETE) && (0..self.kept_n).any(|i| self.kept_id(i) == Some(EVT_HVN_TX_COMPLETE)) {
            return;
        }
        if self.kept_n == self.kept.len() {
            let is = |me: &Self, i: usize, f: fn(&[u8]) -> bool| {
                let at = me.slot(i);
                me.kept
                    .get(at)
                    .zip(me.kept_len.get(at))
                    .is_some_and(|(k, &l)| f(k.get(..usize::from(l)).unwrap_or(&[])))
            };
            let victim = (0..self.kept_n).find(|&i| is(self, i, |k| ant_broadcast(k).is_some())).or_else(|| {
                (!broadcast && id != Some(EVT_HVX))
                    .then(|| (0..self.kept_n).find(|&i| is(self, i, |k| event_id(k) == Some(EVT_HVX))))
                    .flatten()
            });
            if let Some(i) = victim {
                for j in i..self.kept_n - 1 {
                    let (a, b) = (self.slot(j), self.slot(j + 1));
                    if let (Some(&row), Some(&len)) = (self.kept.get(b), self.kept_len.get(b))
                        && let (Some(r), Some(l)) = (self.kept.get_mut(a), self.kept_len.get_mut(a))
                    {
                        (*r, *l) = (row, len);
                    }
                }
                self.kept_n -= 1;
            }
        }
        let at = self.slot(self.kept_n);
        if self.kept_n < KEPT_N
            && let (Some(slot), Some(len)) = (self.kept.get_mut(at), self.kept_len.get_mut(at))
            && let Some(dst) = slot.get_mut(..p.len())
        {
            dst.copy_from_slice(p);
            *len = p.len() as u16;
            self.kept_n += 1;
        }
    }

    /// The BLE event id of the `i`th event kept.
    fn kept_id(&self, i: usize) -> Option<u16> {
        let at = self.slot(i);
        let n = usize::from(self.kept_len.get(at).copied()?);
        event_id(self.kept.get(at)?.get(..n)?)
    }

    /// The ring slot of the `i`th event kept (always in range: the modulo is the array's length).
    fn slot(&self, i: usize) -> usize {
        (self.head + i) % KEPT_N
    }

    /// The oldest event kept while a command waited, copied into `out` (its first `n` bytes); its length `n`.
    pub fn pop(&mut self, out: &mut [u8; KEPT]) -> Option<usize> {
        if self.kept_n == 0 {
            return None;
        }
        let at = self.head;
        let n = usize::from(self.kept_len.get(at).copied().unwrap_or(0));
        let src = self.kept.get(at).and_then(|k| k.get(..n)).unwrap_or(&[]);
        out.iter_mut().zip(src).for_each(|(o, &s)| *o = s);
        (self.head, self.kept_n) = (self.slot(1), self.kept_n - 1);
        Some(n)
    }

    /// Takes one byte; returns the payload when it completes a packet. A payload longer than [`MAX`] is read
    /// through (so the stream stays in step) and dropped.
    ///
    /// ```
    /// let mut rx = bryton_rustfw::nrf::Rx::new();
    /// let got: Vec<_> = [2, 0, 1, 0x60].iter().filter_map(|&b| rx.push(b).map(|p| p.to_vec())).collect();
    /// assert_eq!(got, [vec![1, 0x60]]);
    /// ```
    pub fn push(&mut self, byte: u8) -> Option<&[u8]> {
        let Some(want) = self.want else {
            if let Some(h) = self.header.get_mut(self.got) {
                *h = byte;
            }
            self.got += 1;
            if self.got == 2 {
                self.got = 0;
                let n = usize::from(u16::from_le_bytes(self.header));
                if n > GARBAGE {
                    self.strikes = self.strikes.saturating_add(1);
                } else if n > 0 {
                    self.want = Some(n);
                }
            }
            return None;
        };
        if let Some(slot) = self.buf.get_mut(self.got) {
            *slot = byte;
        }
        self.got += 1;
        if self.got < want {
            return None;
        }
        self.want = None;
        self.got = 0;
        self.last = if want <= MAX { want } else { 0 };
        let good = match self.buf.get(..want) {
            Some([RESP | BLE_EVT | ANT_EVT, ..]) => Some(true),
            Some(_) => Some(false), // a type the chip never sends
            None => None,           // did not fit: dropped
        };
        match good {
            Some(true) => (self.heard, self.strikes, self.timeouts) = (true, 0, 0),
            Some(false) => self.strikes = self.strikes.saturating_add(1),
            None => {}
        }
        self.buf.get(..want)
    }
}

/// Why a command did not succeed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No response within [`TIMEOUT_MS`] of silence: the chip is not running, or the link is out of step.
    Timeout,
    /// The chip answered with this error code (NRF_ERROR_*, BLE_ERROR_*).
    Code(u32),
}

/// Sends one packet, the length header and `payload` as one put, so no byte of it is ever sent alone. A frame is never
/// cut short while the chip lets us on: while it holds CTS (it has read a command's header, and the previous response
/// waits for an event frame that waits for our RTS) the chip's bytes are read into `rx`, which lets it finish the
/// event, queue the response and release CTS; the rest goes on from there. The bound is time without progress: no
/// byte moved either way for [`TIMEOUT_MS`] (the chip is hung: a reset cures it), then `Err(Timeout)`; each wait
/// sleeps 1 ms, nothing spins. If some bytes went and not all, the chip is mis-framed: `rx.broken` is set, the
/// watchdog resets it at once and no frame is sent to it before. What arrives meanwhile is treated as in [`call`]'s
/// wait: BLE and ANT events kept, an answer to a call that timed out swallowed (`rx.late`), the rest dropped.
#[inline(never)] // its 386-byte frame in its own stack frame
pub fn send(b: &mut impl Board, rx: &mut Rx, payload: &[u8]) -> Result<(), Error> {
    if rx.broken || rx.stalled {
        return Err(Error::Timeout);
    }
    let mut frame = [0u8; 2 + MAX_CMD];
    let Some(dst) = frame.get_mut(2..2 + payload.len()) else { return Err(Error::Code(u32::MAX)) };
    dst.copy_from_slice(payload);
    frame[..2].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    let all = frame.get(..2 + payload.len()).unwrap_or(&[]);
    put(b, Some(rx), all, TIMEOUT_MS).map_err(|taken| {
        rx.broken |= taken > 0;
        rx.stalled = true;
        Error::Timeout
    })
}

/// The longest command payload: `sd_ble_gattc_write` of 20 bytes is 34, ANT's longest about 20; room for a 384-byte
/// ATT value.
const MAX_CMD: usize = 384;

/// A packet arrived while a command is being sent: an answer of a timed-out call is swallowed, events are kept.
fn rx_byte(rx: &mut Rx, x: u8) {
    let Some(p) = rx.push(x) else { return };
    let answer = if let [RESP, o, ..] = *p { Some(o) } else { None };
    if !rx.swallow(answer) {
        rx.keep_last();
    }
}

/// `data` to the chip whole, reading whatever it sends while it holds CTS into `rx` (if any). `Err`: how many bytes
/// went before `limit_ms` without progress.
fn put(b: &mut impl Board, mut rx: Option<&mut Rx>, mut data: &[u8], limit_ms: u32) -> Result<(), usize> {
    let total = data.len();
    let mut stall = Stall::default();
    loop {
        let took = b.nrf_send(data);
        data = data.get(took..).unwrap_or(&[]);
        if data.is_empty() {
            return Ok(());
        }
        if took > 0 {
            b.nrf_tx_wait(); // the FIFO is full and drains in a few byte times: not a 1 ms sleep
        }
        let mut buf = [0; 16];
        let n = b.nrf_recv(&mut buf);
        if let Some(rx) = rx.as_deref_mut() {
            buf.get(..n).unwrap_or(&[]).iter().for_each(|&x| rx_byte(rx, x));
        }
        match stall.note(took, n, limit_ms) {
            Ok(true) => {
                b.delay_ms(1); // sleep, not spin: the chip needs the host's reads, not its time
                if let Some(rx) = rx.as_deref_mut() {
                    rx.paused_ms = rx.paused_ms.saturating_add(1);
                }
            }
            Ok(false) => {}
            Err(()) => return Err(total - data.len()),
        }
    }
}

/// How long a put has made no progress: time is counted in the 1 ms sleeps taken with nothing moving either way, and
/// only a sent byte clears it (a flood of the chip's events, a scan's, read while CTS is held is not progress of
/// ours). Two bounds that no sent byte resets: the bytes read since the last sent one ([`FLOOD_BYTES`]), and the
/// whole put's time (sleeps and reads at 10 us a byte), the idle limit, twice it once a byte of the frame went.
#[derive(Default)]
struct Stall {
    idle_ms: u32,
    flood: u32,
    total_us: u32,
    /// A byte of the frame went: a slow chip does not make us abandon a frame that was started.
    started: bool,
}

impl Stall {
    /// A pass took `took` bytes in and read `n` bytes: `Ok(true)` to sleep 1 ms, `Ok(false)` to go on, `Err` to give up.
    fn note(&mut self, took: usize, n: usize, limit_ms: u32) -> Result<bool, ()> {
        let idle = took + n == 0;
        if took > 0 {
            (self.idle_ms, self.flood, self.started) = (0, 0, true);
        } else if idle {
            self.idle_ms += 1;
        } else {
            self.flood = self.flood.saturating_add(n as u32);
        }
        let us = if idle { 1000 } else { 10 * (took + n) as u32 };
        self.total_us = self.total_us.saturating_add(us);
        if self.idle_ms >= limit_ms
            || self.flood >= FLOOD_BYTES
            || self.total_us >= limit_ms.saturating_mul(if self.started { 2000 } else { 1000 })
        {
            Err(())
        } else {
            Ok(idle)
        }
    }
}

/// Bytes read with nothing sent (or, in a wait, in all) before a put or a wait gives up: 32 KB (0.33 s of the chip
/// talking at 1 Mbaud). It must cover the biggest legitimate burst: 96 queued events (the chip's queue) of at most an
/// ATT MTU of 247 plus headers, about 264 B each, 25 KB; a larger MTU needs a larger value.
const FLOOD_BYTES: u32 = 0x8000;

/// Reads packets until `f` returns Some for one (other packets are dropped), or [`TIMEOUT_MS`] of waiting (the 1 ms pauses summed: a chip that chatters but never answers still times out), or
/// 64 KB of other packets (a scan's events). One byte at a time: what follows stays in the chip's stream.
fn wait<T>(b: &mut impl Board, rx: &mut Rx, limit_ms: u32, mut f: impl FnMut(&[u8]) -> Option<T>) -> Option<T> {
    let mut buf = [0; 1];
    let (mut idle_ms, mut bytes) = (0, 0u32);
    while idle_ms < limit_ms && bytes < FLOOD_BYTES {
        let n = b.nrf_recv(&mut buf);
        bytes += n as u32;
        if n == 0 {
            b.delay_ms(1);
            idle_ms += 1;
            rx.paused_ms = rx.paused_ms.saturating_add(1);
        } else if let Some(p) = rx.push(buf[0]) {
            if let Some(t) = f(p) {
                return Some(t);
            }
            rx.keep_last();
        }
    }
    None
}

/// Sends the command `payload` (`[type][op][arguments]`) and waits for its response; copies the outputs into `out`
/// and returns their length. Packets that arrive meanwhile are dropped, BLE events but advertising reports kept
/// ([`Rx::pop`]).
pub fn call(b: &mut impl Board, rx: &mut Rx, payload: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    if rx.broken || rx.stalled {
        return Err(Error::Timeout); // no frame goes to a mis-framed chip; the watchdog resets it (not a time-out)
    }
    // a failed send leaves `rx.late` as the drain left it (an answer swallowed there stays swallowed)
    let paused = rx.paused_ms;
    let sent = send(b, rx, payload);
    let op = payload.get(1).copied();
    // a call that timed out may still be answered: that late answer is not this call's (the next channel's open has
    // the same op), so one answer of its op is swallowed. If it never came, this call fails once (it swallowed the
    // real answer) and does not arm another: the next call is clean.
    let armed = if sent.is_ok() { rx.late.take() } else { None };
    let mut late = armed;
    let response = |p: &[u8]| match *p {
        [RESP, o, ..] if late == Some(o) => {
            late = None;
            None
        }
        [RESP, o, r0, r1, r2, r3, ref outputs @ ..] if Some(o) == op => {
            Some(match u32::from_le_bytes([r0, r1, r2, r3]) {
                0 => {
                    let len = outputs.len().min(out.len());
                    out.iter_mut().zip(outputs).for_each(|(o, &s)| *o = s);
                    Ok(len)
                }
                code => Err(Error::Code(code)),
            })
        }
        _ => None,
    };
    let r = match sent {
        Ok(()) => {
            wait(b, rx, TIMEOUT_MS.saturating_sub(rx.paused_ms - paused), response).unwrap_or(Err(Error::Timeout))
        }
        Err(e) => Err(e), // the chip took no whole command: nothing to wait for
    };
    if r == Err(Error::Timeout) {
        rx.stalled = true;
        rx.timeouts = rx.timeouts.saturating_add(1);
        if sent.is_ok() {
            rx.late = op.filter(|_| armed.is_none() || late.is_some()); // not after a swallow
        }
    }
    if let (Err(e), Some(op)) = (r, op) {
        rx.failed = Some((op, if let Error::Code(c) = e { c } else { u32::MAX }));
    }
    r
}

/// The version fields as Bryton fills them (stock logs "Image / Bootloader / SD").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    /// Nordic firmware image number (stock keeps it in RTC backup register 12).
    pub image: u8,
    /// Bootloader number.
    pub bootloader: u16,
    /// SoftDevice number.
    pub sd: u16,
    /// The two links were configured ([`CFG`]): a trainer as central besides the phone, on tag [`crate::ble::TAG`].
    /// False: the chip's defaults, one link (the phone's), tag 0.
    pub central: bool,
    /// The ATT MTU the chip was configured for ([`FAST_MTU`]), 23 when the fast link is off or the chip refused it.
    pub mtu: u16,
}

/// `sd_ble_cfg_set`.
pub const SD_BLE_CFG_SET: u8 = 0x69;

/// What [`start`] configures before `sd_ble_enable` (stock does the same with more of everything, BOARD.md 11):
/// - `BLE_CONN_CFG_GAP` (0x20) for tag 1: 2 links, event length 3 (3.75 ms, the default; stock: 6 links, 5);
/// - `BLE_GAP_CFG_ROLE_COUNT` (0x40): 1 peripheral, 1 central, 0 central security procedures (stock: 1, 5).
///
/// Layout, `ble_cfg_set_req_enc` (pc-ble-driver `src/sd_api_v5/sdk/components/serialization/application/codecs/
/// ble/serializers/ble_app.c`): the id as a u32, a presence byte, for a connection configuration its tag, then the
/// structure (`ble_gap_conn_cfg_t_enc`: count u8, event length u16; `ble_gap_cfg_role_count_t_enc` in API v5:
/// three u8). The rest stays at the defaults (ATT MTU 23, 10 vendor UUIDs, attribute table 1408 B), each no larger
/// than stock's.
pub const CFG: [&[u8]; 2] =
    [&[CMD, SD_BLE_CFG_SET, 0x20, 0, 0, 0, 1, 1, 2, 3, 0], &[CMD, SD_BLE_CFG_SET, 0x40, 0, 0, 0, 1, 1, 1, 0]];

/// The ATT MTU of the fast link (2026-11-07, docs/roadmap.md 3c): the largest, so a notification carries 244 bytes.
/// Eight times the 18 bytes of a file service frame at MTU 23 and a tenth of the UART's commands per byte.
pub const FAST_MTU: u16 = 247;
/// Notifications the stack queues per link on the fast link (default 1): enough to fill a connection event of three
/// full packets with one to spare. Each queued value holds an MTU-sized buffer in the chip's RAM, which Bryton's app
/// fixes; when the chip refuses, [`start`] falls back.
pub const FAST_QUEUE: u8 = 4;
/// The connection event length of the fast link, 1.25 ms units: 3 (3.75 ms), as the two-link configuration has had it
/// since the start, so nothing in the chip's radio schedule changes outside a download beyond the MTU. A 251-byte
/// packet at the 2M PHY takes about 1 ms, so an event still holds two or three during a READ; at 1M it holds one
/// (a longer event, 6, was the first design and is a later step if the unit's rate needs it).
pub const FAST_EVENT_LEN: u8 = 3;

/// The fast link's configuration, in place of [`CFG`]: the same two links, and
/// - `BLE_CONN_CFG_GATT` (0x23, `ble_gatt_conn_cfg_t`: `att_mtu` u16);
/// - `BLE_CONN_CFG_GATTS` (0x22, `ble_gatts_conn_cfg_t`: `hvn_tx_queue_size` u8).
///
/// The ids are from the enumeration in ble.h (`BLE_CONN_CFG_GAP` = 0x20 then GATTC, GATTS, GATT, L2CAP), **inferred**
/// as the 2026-10-31 attempt's 0x23 was; Bryton's real app decodes them (ids and values checked at its layer, nrf_session_tests.rs; the SoftDevice's acceptance is not) (`ble_cfg_set_req_dec`, run by
/// `cargo test-host -- --ignored nrf_emu`, which cannot say whether the SoftDevice's RAM holds them).
pub const CFG_FAST: [&[u8]; 4] = [
    &[CMD, SD_BLE_CFG_SET, 0x20, 0, 0, 0, 1, 1, 2, FAST_EVENT_LEN, 0],
    &[CMD, SD_BLE_CFG_SET, 0x40, 0, 0, 0, 1, 1, 1, 0],
    &[CMD, SD_BLE_CFG_SET, 0x23, 0, 0, 0, 1, 1, FAST_MTU as u8, (FAST_MTU >> 8) as u8],
    &[CMD, SD_BLE_CFG_SET, 0x22, 0, 0, 0, 1, 1, FAST_QUEUE],
];

/// Resets the chip as stock does, configures two links ([`CFG`]), enables BLE and reads the version: proof that the
/// chip runs and the link works. A chip that refuses the configuration (an error code from `sd_ble_cfg_set`, or
/// NO_MEM from `sd_ble_enable` when its RAM cannot hold it) is reset and started on its defaults, as before: the
/// phone link works either way, the trainer's needs [`Version::central`]. A chip that does not answer is not tried
/// twice (the start's time with a dead chip stays one reset and one time-out).
pub fn start(b: &mut impl Board) -> Result<Version, Error> {
    let mut level = start_level(b);
    loop {
        match start_as(b, level) {
            Err(Error::Code(_)) if level > 0 => level -= 1,
            r => return r,
        }
    }
}

/// The configuration a start tries first: 2, the fast link, when the build has it ([`Board::fast_link`]); else 1.
pub fn start_level(b: &impl Board) -> u8 {
    if b.fast_link() { 2 } else { 1 }
}

/// `level` 2: the links and the fast link's MTU, queue and event length ([`CFG_FAST`]), 1: the links, 0: the chip's
/// defaults.
fn start_as(b: &mut impl Board, level: u8) -> Result<Version, Error> {
    b.nrf_reset();
    configure(b, level)
}

/// PA6 held low, ms (BOARD.md 11: stock's 150 ms, USART2 up meanwhile).
pub const RESET_HOLD_MS: u32 = 150;
/// The chip's boot after PA6 is released, ms (BOARD.md 11: stock waits 800 ms).
pub const BOOT_MS: u32 = 800;

/// Reads and drops what is pending (at most 16 reads): the glitch byte at a reset's release.
pub fn flush(b: &mut impl Board) {
    let mut buf = [0; 16];
    for _ in 0..16 {
        if b.nrf_recv(&mut buf) == 0 {
            break;
        }
    }
}

/// The part of the start after the reset and its boot time: the configuration at `level`, BLE enabled, the version
/// read ([`start`]). `App` runs the reset itself across passes (app/nrf_watch.rs) and calls this when the boot is over.
pub fn configure(b: &mut impl Board, level: u8) -> Result<Version, Error> {
    let mut rx = Rx::new();
    let mut out = [0; 8];
    if level > 1 {
        for c in CFG_FAST {
            call(b, &mut rx, c, &mut [])?;
        }
    } else if level > 0 {
        for c in CFG {
            call(b, &mut rx, c, &mut [])?;
        }
    }
    let (central, mtu) = (level > 0, if level > 1 { FAST_MTU } else { 23 });
    call(b, &mut rx, &[CMD, SD_BLE_ENABLE], &mut out)?;
    let n = call(b, &mut rx, &[CMD, SD_BLE_VERSION_GET, 1], &mut out)?;
    match out.get(..n) {
        Some(&[image, b0, b1, s0, s1, ..]) => Ok(Version {
            image,
            bootloader: u16::from_le_bytes([b0, b1]),
            sd: u16::from_le_bytes([s0, s1]),
            central,
            mtu,
        }),
        _ => Err(Error::Code(u32::MAX)),
    }
}

/// After the power-off command, how long the chip has to be seen off, ms: it sends the frame it has in flight first
/// (at most 386 bytes, 4 ms), since the command needs its single TX buffer (BOARD.md 11 "Power off").
const OFF_WAIT_MS: u32 = 50;
/// Checks in a row, 1 ms apart, that must see it off: the line let go for good, not between two of its frames; then
/// [`floating`] tells a released line from one a hung chip drives high.
const OFF_SEEN: u32 = 3;

/// Powers the chip down until its next reset (`sd_power_system_off`; the next start's reset pulse wakes it), and makes
/// sure of it (BOARD.md 11 "Power off"): the command is on the wire, then the chip is seen to let go of its RTS (our
/// CTS) with nothing more to send. A chip not known to take commands (`fresh` false: never started, in a reset, broken
/// or silent) is reset first; one that is not seen off (or holds its RTS high: hung) is reset and asked once more;
/// failing that it is held in reset (about 0.2 mA, against mA for a hung chip whose radio may run) rather than left
/// running. The lines are parked once it is off, and only then. Returns the attempt that saw it off (0 the first, 1 after
/// the extra reset), None when it was held. Bounded: about 2.1 s at worst (two resets), only for a chip that fails.
pub fn system_off(b: &mut impl Board, rx: &mut Rx, fresh: bool) -> Option<u8> {
    let r = system_off_unparked(b, rx, fresh);
    if r.is_some() {
        b.nrf_park();
    }
    r
}

/// [`system_off`] without the parking: the lines stay as the USART has them (the dev link's `bench nrfoff` reads the
/// chip's RTS after it).
pub fn system_off_unparked(b: &mut impl Board, rx: &mut Rx, fresh: bool) -> Option<u8> {
    for attempt in 0..2 {
        if attempt > 0 || !fresh || rx.broken || rx.timeouts > 0 {
            // a mis-framed or silent chip would read the command as the rest of a frame
            b.nrf_hold(true);
            b.delay_ms(RESET_HOLD_MS);
            b.nrf_hold(false);
            b.delay_ms(BOOT_MS);
            flush(b);
            rx.reset();
        }
        if put(b, None, &[2, 0, CMD, SD_POWER_SYSTEM_OFF], 20).is_ok() && seen_off(b) {
            return Some(attempt);
        }
    }
    b.nrf_hold(true);
    None
}

/// Reads (and drops) what the chip still sends, so that its frame in flight ends and the command gets its TX buffer,
/// until our bytes are all out and its RTS stays released ([`OFF_SEEN`] checks), at most [`OFF_WAIT_MS`] (the reads
/// count 10 us a byte).
fn seen_off(b: &mut impl Board) -> bool {
    let (mut buf, mut seen, mut us) = ([0; 64], 0, 0);
    while us < OFF_WAIT_MS * 1000 {
        let n = b.nrf_recv(&mut buf);
        if n > 0 {
            (seen, us) = (0, us + 10 * n as u32);
            continue;
        }
        seen = if b.nrf_sent() && !b.nrf_cts() { seen + 1 } else { 0 };
        if seen >= OFF_SEEN {
            return floating(b);
        }
        b.delay_ms(1);
        us += 1000;
    }
    false
}

/// The chip's RTS read high on our pull-up: released (System OFF: a disconnected input, BOARD.md 11) or driven high
/// (a hung chip holds it). Our PD3 pulled down for 1 ms tells them apart: released, it follows the pull and reads low.
fn floating(b: &mut impl Board) -> bool {
    b.nrf_cts_pull(false);
    b.delay_ms(1);
    let low = b.nrf_cts();
    b.nrf_cts_pull(true);
    low
}

/// Starts an active scan for `seconds` (0: until stopped), with stock's low-power timing: a 30 ms window every
/// 60 ms. Reports then arrive as events ([`adv_report`]); a [`EVT_TIMEOUT`] event ends it.
pub fn scan_start(b: &mut impl Board, rx: &mut Rx, seconds: u16) -> Result<(), Error> {
    let [t0, t1] = seconds.to_le_bytes();
    // params present, flags = active, interval 0x60 and window 0x30 in 0.625 ms units, timeout in s
    call(b, rx, &[CMD, SD_BLE_GAP_SCAN_START, 1, 1, 0x60, 0, 0x30, 0, t0, t1], &mut []).map(|_| ())
}

/// The id of BLE event packet `p` (`[BLE_EVT][id u16][fields]`).
pub fn event_id(p: &[u8]) -> Option<u16> {
    match p {
        [BLE_EVT, a, b, ..] => Some(u16::from_le_bytes([*a, *b])),
        _ => None,
    }
}

/// One advertising report: who, how loud, and the advertised data.
pub struct AdvReport<'a> {
    /// The address byte as serialized (`addr_id_peer | addr_type << 1`): what [`crate::ble::connect`] takes back.
    pub addr_type: u8,
    /// Address, least significant byte first.
    pub addr: [u8; 6],
    /// Received signal strength, dBm.
    pub rssi: i8,
    /// Advertising data: `[len][type][len - 1 bytes]` structures.
    pub data: &'a [u8],
}

/// Decodes `p` if it is a BLE_GAP_EVT_ADV_REPORT (API v5 layout, BOARD.md 11): `[2][1d 00][conn u16][peer addr 7]
/// [direct addr 7][rssi][flags][dlen][present][data]`.
pub fn adv_report(p: &[u8]) -> Option<AdvReport<'_>> {
    if event_id(p)? != EVT_ADV_REPORT || p.len() < 23 {
        return None;
    }
    let h: &[u8; 23] = p.get(..23)?.try_into().ok()?;
    let addr = h[6..12].try_into().ok()?;
    let data = p.get(23..23 + usize::from(h[21]))?;
    Some(AdvReport { addr_type: h[5], addr, rssi: h[19] as i8, data })
}

/// The advertised name in `data` (complete or shortened), if any.
///
/// ```
/// let data = [2, 0x01, 0x06, 5, 0x09, b'B', b'i', b'k', b'e'];
/// assert_eq!(bryton_rustfw::nrf::ad_name(&data), Some(&b"Bike"[..]));
/// ```
pub fn ad_name(data: &[u8]) -> Option<&[u8]> {
    let mut rest = data;
    while let [len, kind, ..] = *rest {
        let field = rest.get(2..1 + usize::from(len))?;
        if kind == 0x09 || kind == 0x08 {
            return Some(field);
        }
        rest = rest.get(1 + usize::from(len)..)?;
    }
    None
}

// ---------------------------------------------------------------------------------------------------------------
// ANT+ (BOARD.md 11): one channel per sensor kind (sensors.rs), all assigned at start, then opened and closed.
// ---------------------------------------------------------------------------------------------------------------

/// A paired sensor: its ANT channel ID, so the channel finds that sensor and no other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorId {
    /// Device number (the sensor's own, printed on some).
    pub device: u16,
    /// Transmission type (its upper nibble extends the device number to 20 bits).
    pub trans: u8,
}

impl SensorId {
    /// As kept in a 32-bit backup register, behind a tag byte so an empty or foreign value reads as no sensor.
    ///
    /// ```
    /// use bryton_rustfw::nrf::SensorId;
    /// let s = SensorId { device: 25126, trans: 0xf1 };
    /// assert_eq!(SensorId::from_word(s.to_word()), Some(s));
    /// assert_eq!(SensorId::from_word(0), None);
    /// ```
    pub fn to_word(self) -> u32 {
        (0xa5 << 24) | (u32::from(self.trans) << 16) | u32::from(self.device)
    }

    /// The sensor kept in `w`, if [`SensorId::to_word`] wrote it.
    pub fn from_word(w: u32) -> Option<Self> {
        (w >> 24 == 0xa5).then_some(SensorId { device: w as u16, trans: (w >> 16) as u8 })
    }
}

/// ANT event code: the channel is closed (after [`ant_close`], or a search that timed out).
const EVENT_CHANNEL_CLOSED: u8 = 0x07;
/// EVENT_QUE_OVERFLOW (S212 v4.0.2 docs, `ant_parameters.h`; the S332 headers are not public).
const EVENT_QUE_OVERFLOW: u8 = 0x35;
/// An acknowledged message was not acknowledged by the other end (ANT message 0x40 event code 6).
const EVENT_TRANSFER_TX_FAILED: u8 = 0x06;
/// An acknowledged message was acknowledged (event code 5; the same layout as code 6 here, inferred).
const EVENT_TRANSFER_TX_COMPLETED: u8 = 0x05;

/// Low-priority search time-out, in 2.5 s units: 12 = 30 s. Then the chip closes the channel and the app reopens it
/// later ([`crate::SEARCH_REST_MS`]), so a sensor that is not sending (a strap not worn) does not keep the radio
/// listening all the time. Stock uses 0xff (searching for ever).
pub const SEARCH_TIMEOUT: u8 = 12;

/// True if ANT event `p` is EVENT_QUE_OVERFLOW (0x35, Nordic `ant_parameters.h`): the SoftDevice's event queue was
/// full and events were lost. The 44-byte layout (code at byte 42) is **inferred** from the other ANT events, not seen
/// on the unit. Its length is `usNumberOfEvents` of the enable call (32, the floor).
///
/// ```
/// let mut p = [0u8; 44];
/// p[0] = 8;
/// p[42] = 0x35;
/// assert!(bryton_rustfw::nrf::ant_queue_overflow(&p));
/// assert!(!bryton_rustfw::nrf::ant_queue_overflow(&p[..43]), "short");
/// ```
pub fn ant_queue_overflow(p: &[u8]) -> bool {
    matches!(p, [ANT_EVT, .., EVENT_QUE_OVERFLOW, _] if p.len() == 44)
}

/// The channel of ANT event `p` if it reports a channel closed.
///
/// ```
/// let mut p = [0u8; 44];
/// p[0] = 8;
/// (p[42], p[43]) = (7, 3);
/// assert_eq!(bryton_rustfw::nrf::ant_closed(&p), Some(3));
/// assert_eq!(bryton_rustfw::nrf::ant_closed(&[2, 0x1b, 0]), None, "any other packet, short ones included");
/// ```
pub fn ant_closed(p: &[u8]) -> Option<u8> {
    match p {
        [ANT_EVT, .., EVENT_CHANNEL_CLOSED, ch] if p.len() == 44 => Some(*ch),
        _ => None,
    }
}

/// The channel of ANT event `p` if it reports an acknowledged message not delivered (`EVENT_TRANSFER_TX_FAILED`): the
/// message is sent again.
///
/// ```
/// let mut p = [0u8; 44];
/// p[0] = 8;
/// (p[42], p[43]) = (6, 7);
/// assert_eq!(bryton_rustfw::nrf::ant_tx_failed(&p), Some(7));
/// assert_eq!(bryton_rustfw::nrf::ant_tx_failed(&[8, 6, 7]), None, "short");
/// ```
pub fn ant_tx_failed(p: &[u8]) -> Option<u8> {
    match p {
        [ANT_EVT, .., EVENT_TRANSFER_TX_FAILED, ch] if p.len() == 44 => Some(*ch),
        _ => None,
    }
}

/// The channel of ANT event `p` if it reports an acknowledged message delivered (`EVENT_TRANSFER_TX_COMPLETED`).
///
/// ```
/// let mut p = [0u8; 44];
/// p[0] = 8;
/// (p[42], p[43]) = (5, 7);
/// assert_eq!(bryton_rustfw::nrf::ant_tx_completed(&p), Some(7));
/// let mut q = [0u8; 44];
/// q[..6].copy_from_slice(&[8, 1, 0x40, 3, 1, 5]); // the embedded message: [0x40, ch, 0x01, code]
/// assert_eq!(bryton_rustfw::nrf::ant_tx_completed(&q), Some(3));
/// assert_eq!(bryton_rustfw::nrf::ant_tx_completed(&[8, 5, 7]), None, "short");
/// ```
pub fn ant_tx_completed(p: &[u8]) -> Option<u8> {
    match p {
        [ANT_EVT, .., EVENT_TRANSFER_TX_COMPLETED, ch] if p.len() == 44 => Some(*ch),
        // the channel event message itself (ANT Message Protocol 9.5.6.1: 0x40, channel, 0x01, the event code)
        [ANT_EVT, _, 0x40, ch, 1, EVENT_TRANSFER_TX_COMPLETED, ..] if p.len() == 44 => Some(*ch),
        _ => None,
    }
}

/// Enables ANT, sets the network keys (network 0 the ANT+ one, licensed; network 1 Shimano's, if given: build.rs
/// reads both from private files, never from this repository) and assigns channel `ch` as a receiver on network
/// `nets[ch]`, as stock does at start. A channel on network 1 without its key is left unassigned (its opening then
/// fails). Returns whether network 1 is up: its key and its channels accepted (else only the Di2 is off).
pub fn ant_start(
    b: &mut impl Board,
    rx: &mut Rx,
    key: [u8; 8],
    private: Option<[u8; 8]>,
    nets: &[u8],
) -> Result<bool, Error> {
    // enable: 15 channels, 0 encrypted, event queue 32 (usNumberOfEvents = 0x20, the floor: Nordic's
    // MIN_ANT_EVENT_QUEUE_SIZE; overflow is reported as event 0x35, counted in `Rx::ant_lost`), then stock's buffer pointer and size (sent as stock sends
    // them; the chip allocates its own, inferred)
    call(b, rx, &[ANT_CMD, 0xfa, 1, 0x0f, 0, 0x20, 0, 0x7c, 0x16, 0x06, 0x24, 0xfc, 0x05], &mut [])?;
    fn set_key(
        b: &mut impl Board,
        rx: &mut Rx,
        net: u8,
        [k0, k1, k2, k3, k4, k5, k6, k7]: [u8; 8],
    ) -> Result<usize, Error> {
        call(b, rx, &[ANT_CMD, 0xcc, net, k0, k1, k2, k3, k4, k5, k6, k7], &mut [])
    }
    set_key(b, rx, 0, key)?;
    // Shimano's network failing, its key or its channel, leaves the ANT+ sensors as they are (review of 571e613: a
    // refused channel 6 took ANT down whole)
    let mut private = private.is_some_and(|k| set_key(b, rx, 1, k).is_ok());
    for (ch, &net) in (0..).zip(nets) {
        let assign = [ANT_CMD, 0xc2, ch, 0, net, 0]; // assign: slave, network `net`, no extended
        if net == 0 {
            call(b, rx, &assign, &mut [])?;
        } else if private {
            private = call(b, rx, &assign, &mut []).is_ok();
        }
    }
    Ok(private)
}

/// Opens (closed, assigned) channel `ch` for sensors of `device_type` with stock's settings: for `id` only, or for
/// the first one heard (pairing) when None.
pub fn ant_open(
    b: &mut impl Board,
    rx: &mut Rx,
    ch: u8,
    device_type: u8,
    period: u16,
    id: Option<SensorId>,
) -> Result<(), Error> {
    let SensorId { device, trans } = id.unwrap_or(SensorId { device: 0, trans: 0 }); // 0, 0: any
    let [d0, d1] = device.to_le_bytes();
    let [p0, p1] = period.to_le_bytes();
    let cmds: [&[u8]; 5] = [
        &[ANT_CMD, 0xd3, ch, d0, d1, device_type, trans], // channel id
        &[ANT_CMD, 0xda, ch, SEARCH_TIMEOUT],             // low-priority search time-out
        &[ANT_CMD, 0xcd, ch, 0x39],                       // 2457 MHz, ANT+
        &[ANT_CMD, 0xd1, ch, p0, p1],
        &[ANT_CMD, 0xc4, ch, 0, 0], // open
    ];
    for cmd in cmds {
        call(b, rx, cmd, &mut [])?;
    }
    Ok(())
}

/// Sends 8 data bytes as an acknowledged message on channel `ch` (`sd_ant_acknowledge_message_tx`, op 0xc8; stock's
/// encoder 0xd02bf3c8 writes the op, the channel, the size, then the bytes: Ghidra notes/agent_b2.md; the layout is
/// inferred from those notes, not yet seen on the unit). The trainer's control pages go this way. The chip refusing
/// it (the last one still queued) or not answering it (the layout is a guess) is a lost command, not a fault of the
/// link: the caller decides, and the link watchdog does not see it (neither the `failed` note nor the `timeouts`
/// count, which would reset the chip after two).
pub fn ant_ack(b: &mut impl Board, rx: &mut Rx, ch: u8, data: [u8; 8]) -> Result<(), Error> {
    let [d0, d1, d2, d3, d4, d5, d6, d7] = data;
    let (failed, timeouts) = (rx.failed, rx.timeouts);
    let r = call(b, rx, &[ANT_CMD, 0xc8, ch, 8, d0, d1, d2, d3, d4, d5, d6, d7], &mut []);
    if r.is_err() {
        (rx.failed, rx.timeouts) = (failed, timeouts);
    }
    r.map(drop)
}

/// Closes channel `ch` and waits for the chip to report it closed, so it can be opened again with another ID.
pub fn ant_close(b: &mut impl Board, rx: &mut Rx, ch: u8) -> Result<(), Error> {
    call(b, rx, &[ANT_CMD, 0xc5, ch], &mut [])?;
    let closed = |p: &[u8]| (ant_closed(p) == Some(ch)).then_some(());
    wait(b, rx, TIMEOUT_MS, closed).ok_or(Error::Timeout)
}

/// The sensor channel `ch` locked onto (`sd_ant_channel_id_get`, op 0xd4, seen on the unit): the reply's outputs
/// are the device number, the device type and the transmission type. None while it still searches (number 0).
pub fn ant_channel_id(b: &mut impl Board, rx: &mut Rx, ch: u8) -> Result<Option<SensorId>, Error> {
    let mut out = [0; 4];
    match (call(b, rx, &[ANT_CMD, 0xd4, ch], &mut out)?, out) {
        (4, [d0, d1, _, trans]) if [d0, d1] != [0, 0] => {
            Ok(Some(SensorId { device: u16::from_le_bytes([d0, d1]), trans }))
        }
        _ => Ok(None),
    }
}

/// The channel and the 8 data bytes of ANT event `p`, if it is data: a broadcast `[ANT_EVT][size][0x4e][channel]
/// [8 data bytes]...`, or the same as acknowledged data (0x4f: Shimano's button page, stock's
/// `Ant_Di2_OnAntEvent`), 44 bytes in all.
///
/// ```
/// let mut p = [0u8; 44];
/// p[..12].copy_from_slice(&[8, 9, 0x4e, 3, 4, 0, 0, 0, 0, 0, 0, 72]);
/// assert_eq!(bryton_rustfw::nrf::ant_broadcast(&p), Some((3, [4, 0, 0, 0, 0, 0, 0, 72])));
/// p[2] = 0x4f;
/// assert_eq!(bryton_rustfw::nrf::ant_broadcast(&p), Some((3, [4, 0, 0, 0, 0, 0, 0, 72])), "acknowledged");
/// ```
pub fn ant_broadcast(p: &[u8]) -> Option<(u8, [u8; 8])> {
    match p {
        [ANT_EVT, _, 0x4e | 0x4f, ch, d0, d1, d2, d3, d4, d5, d6, d7, ..] if p.len() == 44 => {
            Some((*ch, [*d0, *d1, *d2, *d3, *d4, *d5, *d6, *d7]))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Devices nearby
// ---------------------------------------------------------------------------------------------------------------

/// One device heard while scanning.
#[derive(Clone, Copy)]
pub struct Device {
    /// Address, least significant byte first.
    pub addr: [u8; 6],
    /// Signal strength of its last report, dBm.
    pub rssi: i8,
    name: [u8; Device::NAME],
    name_len: u8,
    /// What kind of device it is, from its manufacturer data ([`ad_maker`]).
    pub maker: Option<&'static str>,
}

impl Device {
    /// Longest name kept (the screen shows fewer).
    pub const NAME: usize = 16;

    /// The advertised name, printable ASCII only (other bytes become '?'); empty if it never sent one.
    pub fn name(&self) -> &str {
        bytes::ascii(&self.name, self.name_len)
    }
}

/// What kind of device sent `data`, from its manufacturer data (AD type 0xff: a Bluetooth SIG company ID, then
/// the company's own bytes). Phones and laptops advertise no name, only this and a random address that changes
/// every few minutes. Apple's first byte is its Continuity message type and Microsoft's beacon carries a device type
/// (both documented by public reverse engineering, e.g. furiousMAC/continuity; not from the stock firmware).
///
/// ```
/// use bryton_rustfw::nrf::ad_maker;
/// assert_eq!(ad_maker(&[2, 0x01, 0x1a, 5, 0xff, 0x4c, 0, 0x12, 2]), Some("Find My"));
/// assert_eq!(ad_maker(&[6, 0xff, 0x06, 0, 0x01, 0x09, 0x20]), Some("Windows PC"));
/// ```
// ponytail: a handful of companies and types; the full SIG list (thousands) is not worth the image size.
pub fn ad_maker(data: &[u8]) -> Option<&'static str> {
    let mut rest = data;
    while let [len, kind, ..] = *rest {
        let field = rest.get(2..1 + usize::from(len))?;
        if let (0xff, [a, b, own @ ..]) = (kind, field) {
            return Some(match (u16::from_le_bytes([*a, *b]), own) {
                (0x004c, [0x02, ..]) => "iBeacon",
                (0x004c, [0x07, ..]) => "AirPods",
                (0x004c, [0x09, ..]) => "AirPlay",
                (0x004c, [0x12, ..]) => "Find My", // an AirTag, or an Apple device's lost-mode beacon
                (0x004c, [0x05 | 0x0c | 0x0f | 0x10, ..]) => "iPhone/Mac", // AirDrop, Handoff, Nearby
                (0x004c, _) => "Apple",
                (0x0006, [0x01, t, ..]) if matches!(t & 0x1f, 9 | 15) => "Windows PC",
                (0x0006, [0x01, t, ..]) if t & 0x1f == 1 => "Xbox",
                (0x0006, _) => "Microsoft",
                (0x0059, _) => "Nordic",
                (0x006b, _) => "Polar",
                (0x0075, _) => "Samsung",
                (0x0087, _) => "Garmin",
                (0x00e0, _) => "Google",
                (0x0171, _) => "Amazon",
                _ => return None,
            });
        }
        rest = rest.get(1 + usize::from(len)..)?;
    }
    None
}

/// The distinct devices heard while scanning (up to [`Nearby::MAX`] remembered; the count stops there).
pub struct Nearby {
    devices: [Device; Nearby::MAX],
    count: usize,
}

impl Default for Nearby {
    fn default() -> Self {
        Self::new()
    }
}

impl Nearby {
    /// How many distinct addresses are remembered.
    pub const MAX: usize = 32;

    /// Nobody heard yet.
    pub const fn new() -> Self {
        const NONE: Device = Device { addr: [0; 6], rssi: 0, name: [0; Device::NAME], name_len: 0, maker: None };
        Nearby { devices: [NONE; Self::MAX], count: 0 }
    }

    /// Notes a report: its signal strength, and its name if it carries one. True if the device is new, its name changed
    /// or its signal moved to another 4 dBm bucket (a jitter inside one is not worth a redraw).
    pub fn add(&mut self, r: &AdvReport) -> bool {
        let known = self.devices.iter().take(self.count).position(|d| d.addr == r.addr);
        let new = known.is_none() && self.count < Self::MAX;
        let Some(d) = (match known {
            Some(i) => self.devices.get_mut(i),
            None if new => {
                self.count += 1;
                self.devices.get_mut(self.count - 1)
            }
            None => None,
        }) else {
            return false;
        };
        d.addr = r.addr;
        // the list draws the signal in buckets of 4 dBm and sorts by it: a move into another bucket is a change
        let moved = d.rssi >> 2 != r.rssi >> 2;
        d.rssi = r.rssi;
        if let Some(m) = ad_maker(r.data) {
            d.maker = Some(m);
        }
        let renamed = ad_name(r.data).is_some_and(|name| bytes::set_ascii(&mut d.name, &mut d.name_len, name));
        new || renamed || moved
    }

    /// Distinct devices heard.
    pub fn count(&self) -> usize {
        self.count
    }

    /// The devices, strongest signal first (a sorted copy: 32 x 24 bytes of stack).
    pub fn strongest(&self) -> impl Iterator<Item = Device> {
        let mut d = self.devices;
        if let Some(s) = d.get_mut(..self.count) {
            crate::storage::bytes::sort_by_key(s, |d| core::cmp::Reverse(d.rssi));
        }
        d.into_iter().take(self.count)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// Frames `p` into `rx` and keeps it, as the main loop does.
    fn keep(rx: &mut Rx, p: &[u8]) {
        for &byte in (p.len() as u16).to_le_bytes().iter().chain(p) {
            if rx.push(byte).is_some() {
                rx.keep_last();
            }
        }
    }

    #[test]
    fn a_flood_of_notifications_does_not_push_out_a_disconnection() {
        // an iPhone's notifications after subscribing (ANCS's pre-existing ones), then the link's end
        let mut rx = Rx::new();
        for uid in 0..KEPT_N as u8 + 4 {
            keep(&mut rx, &[BLE_EVT, 0x39, 0, 0, 0, 0, 0, 0, 0, 0x12, 0, 1, 1, 0, uid]);
        }
        keep(&mut rx, &[BLE_EVT, 0x11, 0, 0, 0, 0x08]);
        let mut out = [0; KEPT];
        let mut ids = std::vec::Vec::new();
        while let Some(n) = rx.pop(&mut out) {
            ids.push(event_id(&out[..n]));
        }
        assert_eq!(ids.len(), KEPT_N);
        assert_eq!(ids.last(), Some(&Some(0x11)), "the disconnection kept, an old notification dropped");
    }

    #[test]
    fn a_fast_link_hvx_is_kept_whole_and_tx_completes_do_not_crowd_it_out() {
        let mut rx = Rx::new();
        // an HVX at ATT MTU 247: 14 bytes of header and 244 of value (258; a u8 length wrapped it to 2)
        let mut hvx = [0x5a; 258];
        hvx[..3].copy_from_slice(&[BLE_EVT, 0x39, 0]);
        keep(&mut rx, &hvx);
        for _ in 0..KEPT_N * 2 {
            keep(&mut rx, &[BLE_EVT, 0x57, 0, 0, 0, 1]); // a stream of TX-completes
        }
        keep(&mut rx, &[BLE_EVT, 0x11, 0, 0, 0, 0x08]);
        let mut out = [0; KEPT];
        assert_eq!(rx.pop(&mut out), Some(258));
        assert_eq!(&out[..258], &hvx[..]);
        let mut ids = std::vec::Vec::new();
        while let Some(n) = rx.pop(&mut out) {
            ids.push(event_id(&out[..n]));
        }
        assert_eq!(ids, [Some(0x57), Some(0x11)], "one TX-complete is kept, the disconnection too");
    }

    #[test]
    fn a_long_event_is_kept_and_the_ring_wraps() {
        let mut rx = Rx::new();
        let mut out = [0; KEPT];
        // 137 bytes: a Chars answer of 14 characteristics; and many more events than the ring holds, popped
        // as they come, so the head goes round
        let mut long = std::vec![BLE_EVT, 0x39];
        long.resize(137, 7);
        for round in 0..KEPT_N as u8 * 3 {
            keep(&mut rx, &long);
            keep(&mut rx, &[BLE_EVT, 0x11, round, 0, 0, 0x08]);
            assert_eq!(rx.pop(&mut out), Some(137));
            assert_eq!(&out[..137], &long[..]);
            assert_eq!(rx.pop(&mut out), Some(6));
            assert_eq!(out[2], round);
            assert_eq!(rx.pop(&mut out), None);
        }
        long.resize(KEPT + 1, 7);
        keep(&mut rx, &long);
        assert_eq!(rx.pop(&mut out), None, "past KEPT: dropped");
    }

    #[test]
    fn a_late_answer_is_not_the_next_calls() {
        let frame = [6u8, 0, RESP, SD_BLE_VERSION_GET, 5, 0, 0, 0]; // an answer with an error code, from before
        let mut out = [0; 16];
        let mut call_with = |late: Option<u8>| {
            let mut sim = crate::sim::Sim::new(9, 0, 0);
            let _app = crate::App::start(&mut sim, None);
            let sim = &mut sim;
            let mut rx = Rx::new();
            rx.late = late;
            frame.iter().for_each(|&b| sim.nordic.stray(b));
            call(sim, &mut rx, &[CMD, SD_BLE_VERSION_GET, 1], &mut out).map(|_| ())
        };
        assert_eq!(call_with(None), Err(Error::Code(5)), "taken for the answer");
        assert_eq!(call_with(Some(SD_BLE_VERSION_GET)), Ok(()), "swallowed: the real answer follows");
        // the chip never answered A: B loses its answer to the swallow and fails once, C is clean
        let mut sim = crate::sim::Sim::new(9, 0, 0);
        let _app = crate::App::start(&mut sim, None);
        let mut rx = Rx::new();
        let ask = [CMD, SD_BLE_VERSION_GET, 1];
        sim.nordic.hung = true;
        assert_eq!(call(&mut sim, &mut rx, &ask, &mut out), Err(Error::Timeout), "A: silent");
        sim.nordic.unhang();
        assert!(rx.stalled, "the time-out stalls the rest of the pass");
        assert_eq!(call(&mut sim, &mut rx, &ask, &mut out), Err(Error::Timeout), "B: refused at once");
        rx.span_end(); // the next span
        assert_eq!(call(&mut sim, &mut rx, &ask, &mut out), Err(Error::Timeout), "B: its answer swallowed");
        rx.span_end();
        assert!(call(&mut sim, &mut rx, &ask, &mut out).is_ok(), "C: clean");
    }

    #[test]
    fn a_late_answer_read_during_a_send_is_swallowed_there() {
        let mut rx = Rx::new();
        rx.late = Some(0x65);
        for b in [6, 0, RESP, 0x66, 0, 0, 0, 0] {
            rx_byte(&mut rx, b); // another op's answer: not the late one
        }
        assert_eq!(rx.late, Some(0x65));
        for b in [6, 0, RESP, 0x65, 5, 0, 0, 0] {
            rx_byte(&mut rx, b);
        }
        assert_eq!(rx.late, None, "swallowed in the drain, so the next answer of that op is the real one");
    }

    #[test]
    fn a_queue_overflow_event_is_counted_and_not_kept() {
        let mut rx = Rx::new();
        let mut lost = [0u8; 44];
        (lost[0], lost[42]) = (ANT_EVT, EVENT_QUE_OVERFLOW);
        keep(&mut rx, &lost);
        keep(&mut rx, &lost);
        assert_eq!(rx.ant_lost, 2);
        assert_eq!(rx.pop(&mut [0; KEPT]), None);
        rx.reset();
        assert_eq!(rx.ant_lost, 2, "a reset keeps the count");
    }

    #[test]
    fn ant_events_are_kept_and_a_broadcast_makes_room_for_anything_else() {
        let mut rx = Rx::new();
        let mut bcast = [0u8; 44];
        (bcast[0], bcast[2]) = (ANT_EVT, 0x4e);
        let mut closed = [0u8; 44];
        (closed[0], closed[42], closed[43]) = (ANT_EVT, EVENT_CHANNEL_CLOSED, 3);
        for _ in 0..KEPT_N {
            keep(&mut rx, &bcast);
        }
        keep(&mut rx, &closed); // full of broadcasts: the oldest one makes room
        keep(&mut rx, &[BLE_EVT, 0x11, 0, 0, 0, 0x08]); // and for a disconnection
        let mut out = [0; KEPT];
        let mut kinds = (0, 0, 0);
        while let Some(n) = rx.pop(&mut out) {
            let p = &out[..n];
            kinds.0 += usize::from(ant_broadcast(p).is_some());
            kinds.1 += usize::from(ant_closed(p).is_some());
            kinds.2 += usize::from(event_id(p) == Some(0x11));
        }
        assert_eq!(kinds, (KEPT_N - 2, 1, 1));
        for _ in 0..KEPT_N {
            keep(&mut rx, &closed);
        }
        keep(&mut rx, &bcast); // full of closes: a broadcast never pushes one out
        let mut n = 0;
        while rx.pop(&mut out).is_some() {
            n += 1;
        }
        assert_eq!(n, KEPT_N);
    }

    #[test]
    fn events_nothing_handles_never_push_out_a_close_or_a_disconnection() {
        let mut rx = Rx::new();
        let mut fail = [0u8; 44];
        (fail[0], fail[42], fail[43]) = (ANT_EVT, 2, 3); // EVENT_RX_FAIL
        for _ in 0..KEPT_N + 2 {
            keep(&mut rx, &fail);
        }
        assert_eq!(rx.pop(&mut [0; KEPT]), None, "an event nothing handles is not kept");
        let mut closed = [0u8; 44];
        (closed[0], closed[42], closed[43]) = (ANT_EVT, EVENT_CHANNEL_CLOSED, 3);
        keep(&mut rx, &closed);
        for _ in 0..KEPT_N + 2 {
            keep(&mut rx, &fail);
        }
        keep(&mut rx, &[BLE_EVT, 0x11, 0, 0, 0, 0x08]);
        let mut out = [0; KEPT];
        let n = rx.pop(&mut out).expect("the close");
        assert_eq!(ant_closed(&out[..n]), Some(3));
        let n = rx.pop(&mut out).expect("the disconnection");
        assert_eq!(event_id(&out[..n]), Some(0x11));
    }

    #[test]
    fn a_flood_of_received_bytes_is_no_stall_while_the_sending_progresses() {
        let mut st = Stall::default();
        for i in 0..2000u32 {
            // 16 bytes read per pass, one byte sent every 100 passes: no stall within the put's time
            assert_eq!(st.note(usize::from(i % 100 == 0), 16, 500), Ok(false));
        }
        // no sent byte resets the put's total time: 1 s for a 500 ms limit
        assert!((2000..100_000u32).any(|i| st.note(usize::from(i % 100 == 0), 16, 500).is_err()), "a put is bounded");
        let mut st = Stall::default();
        let passes = (1..=100_000).find(|_| st.note(0, 16, 500).is_err()).unwrap();
        assert!(passes <= 4096, "a flood with nothing sent stops at 64 KB ({passes} passes)");
        let mut st = Stall::default();
        assert_eq!((0..500).filter(|_| st.note(0, 0, 500) == Ok(true)).count(), 499, "500 ms of silence is bounded");
        assert!(st.note(0, 0, 500).is_err());
    }

    #[test]
    fn a_resync_keeps_the_events_already_framed() {
        let mut rx = Rx::new();
        keep(&mut rx, &[BLE_EVT, 0x11, 0, 0, 0, 0x08]);
        rx.push(5); // half a frame
        rx.resync();
        let mut out = [0; KEPT];
        let n = rx.pop(&mut out).expect("the disconnection kept");
        assert_eq!(event_id(&out[..n]), Some(0x11));
        keep(&mut rx, &[BLE_EVT, 0x11, 0, 0, 0, 0x08]);
        rx.reset();
        assert_eq!(rx.pop(&mut out), None, "a chip reset forgets them");
    }

    fn report(addr: u8, rssi: i8, data: &[u8]) -> AdvReport<'_> {
        AdvReport { addr_type: 0, addr: [addr, 0, 0, 0, 0, 0], rssi, data }
    }

    #[test]
    fn nearby_keeps_names_and_the_last_rssi_strongest_first() {
        let mut n = Nearby::new();
        assert!(n.add(&report(1, -80, &[5, 0xff, 0x4c, 0, 0x10, 6])));
        assert_eq!(n.strongest().next().unwrap().maker, Some("iPhone/Mac"));
        assert!(n.add(&report(2, -50, &[4, 0x09, b'H', b'R', 0xff])));
        assert!(n.add(&report(1, -40, &[3, 0x08, b'P', b'C'])), "same address, a name: listed again");
        assert!(!n.add(&report(1, -39, &[3, 0x08, b'P', b'C'])), "a jitter inside the bucket: no change");
        assert!(n.add(&report(1, -50, &[3, 0x08, b'P', b'C'])), "another bucket: the row and the order change");
        n.add(&report(1, -40, &[3, 0x08, b'P', b'C']));
        let list: std::vec::Vec<_> =
            n.strongest().map(|d| (d.addr[0], d.rssi, std::string::String::from(d.name()))).collect();
        assert_eq!(list, [(1, -40, "PC".into()), (2, -50, "HR?".into())]);
        for i in 0..40 {
            n.add(&report(10 + i, -60, &[]));
        }
        assert_eq!(n.count(), Nearby::MAX);
    }
}
