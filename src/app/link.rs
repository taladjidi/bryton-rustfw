//! The Nordic link's watchdog. The framing has no marker to resync on (drivers/nrf.rs), so one stray or lost byte
//! leaves it out of step for good, and the sensors and the phone are gone. Signs of that, none of which a quiet
//! but healthy link gives:
//! - two garbage frames in a row (a length header past [`nrf::GARBAGE`], a type the chip never sends): [`nrf::Rx`]
//!   `strikes`;
//! - two commands in a row with no response at all: `timeouts`;
//! - no frame of any kind for [`PROBE_MS`]: then the chip is *asked* something cheap (its version). An answer,
//!   even an error code, proves the link in step, so a link with no sensor and no phone, where silence is
//!   normal, is never reset; a chip that hangs, or a link whose length bytes swallowed the stream, does not
//!   answer.
//!
//! Recovery: drain the stream and start reading at a frame boundary, ask again; failing that reset the chip, start
//! ANT again (keys, channels), reopen every channel that was open, and set the phone link up again (a bonded
//! phone reconnects by itself). Attempts back off (15 s, doubling to 5 min) and the backoff relaxes after 10 quiet
//! minutes. The recorder never waits on any of it beyond the commands' own time-outs (500 ms each, at most
//! about 3 s for a whole restart with a dead chip): it records from the app's own state, and a pass that took
//! longer loses no ride time (`App::step` counts the seconds gone).

use crate::*;

/// No frame from the chip for this long: ask it something.
pub const PROBE_MS: u32 = 30_000;
/// The first wait between recoveries; doubled each time, up to [`MAX_WAIT_MS`].
const FIRST_WAIT_MS: u32 = 15_000;
/// The longest wait between recoveries.
const MAX_WAIT_MS: u32 = 300_000;
/// After this long without a recovery the backoff starts again (and "RADIO RESET" may be said again).
const CALM_MS: u32 = 600_000;
/// The wait after a resync alone, so a link that keeps slipping is not drained every pass.
const RESYNC_WAIT_MS: u32 = 5_000;

/// The watchdog's state.
pub struct Watch {
    /// Since the last frame from the chip.
    quiet_ms: u32,
    /// Until the next recovery may run.
    wait_ms: u32,
    /// The wait after the last chip reset (0: none yet in this calm).
    delay_ms: u32,
    /// Since the last recovery.
    calm_ms: u32,
    /// Chip resets since the start (shown by the dev link's `info`).
    pub resets: u16,
    /// Resyncs that were enough, since the start.
    pub resyncs: u16,
    /// "RADIO RESET" was said in this calm.
    told: bool,
    /// Channels a restart could not reopen: asked for again at the next one (a restart on a damaged link).
    pub(crate) owed: [Sensor; Kind::ALL.len()],
    /// The phone link's setup got no answer at a restart (a refusal is not this: it would be refused again).
    phone_owed: bool,
}

impl Watch {
    /// The start. A chip (or its ANT) that did not start at boot was just tried: the first recovery waits.
    pub const fn new(started: bool) -> Self {
        let wait_ms = if started { 0 } else { FIRST_WAIT_MS };
        Watch {
            quiet_ms: 0,
            wait_ms,
            delay_ms: 0,
            calm_ms: 0,
            resets: 0,
            resyncs: 0,
            told: false,
            owed: [Sensor::Off; Kind::ALL.len()],
            phone_owed: false,
        }
    }
}

/// Reads and drops what the chip sends until the link has been idle for 20 ms (bounded: at most 200 reads).
fn drain(b: &mut impl Board) {
    let (mut buf, mut idle) = ([0; 64], 0);
    for _ in 0..200 {
        if b.nrf_recv(&mut buf) > 0 {
            idle = 0;
        } else if idle >= 20 {
            break;
        } else {
            idle += 1;
            b.delay_ms(1);
        }
    }
}

impl App {
    /// Chip resets since the start.
    pub fn radio_resets(&self) -> u16 {
        self.watch.resets
    }

    /// Asks the chip its version: whether the link answers (an error code is an answer).
    fn probe(&mut self, b: &mut impl Board) -> bool {
        let r = nrf::call(b, &mut self.rx, &[nrf::CMD, nrf::SD_BLE_VERSION_GET, 1], &mut [0; 8]);
        r != Err(nrf::Error::Timeout)
    }

    /// Once a pass, after the chip's events were read: counts the quiet, and recovers when the link is out of step.
    pub(crate) fn watch_radio(&mut self, b: &mut impl Board) {
        let heard = core::mem::take(&mut self.rx.heard);
        let w = &mut self.watch;
        w.quiet_ms = if heard { 0 } else { w.quiet_ms.saturating_add(POLL_MS) };
        w.wait_ms = w.wait_ms.saturating_sub(POLL_MS);
        w.calm_ms = w.calm_ms.saturating_add(POLL_MS);
        if w.calm_ms >= CALM_MS {
            (w.delay_ms, w.told) = (0, false);
        }
        if w.wait_ms > 0 {
            return;
        }
        // ANT wanted (a key) but not up, or a channel not reopened: a restart on a damaged link failed: another
        let ant_down = (self.ant_key.is_some() && !self.ant)
            || self.watch.phone_owed
            || self.watch.owed.iter().any(|&s| s != Sensor::Off);
        let broken = self.ble.is_none() || ant_down || self.rx.strikes >= 2 || self.rx.timeouts >= 2;
        if !broken {
            if self.watch.quiet_ms >= PROBE_MS {
                // a probe that gets no answer is asked again in a second (one time-out may be a busy chip); two
                // in a row (`timeouts`) are a broken link
                self.watch.quiet_ms = if self.probe(b) { 0 } else { PROBE_MS - 1000 };
            }
            return;
        }
        // 1. start reading at a frame boundary and ask again
        if self.ble.is_some() && !ant_down {
            drain(b);
            self.rx.resync();
            if self.probe(b) {
                let w = &mut self.watch;
                (w.quiet_ms, w.wait_ms, w.resyncs) = (0, RESYNC_WAIT_MS, w.resyncs.saturating_add(1));
                return;
            }
        }
        // 2. reset the chip
        let w = &mut self.watch;
        w.delay_ms = if w.delay_ms == 0 { FIRST_WAIT_MS } else { (w.delay_ms * 2).min(MAX_WAIT_MS) };
        (w.wait_ms, w.calm_ms, w.quiet_ms, w.resets) = (w.delay_ms, 0, 0, w.resets.saturating_add(1));
        if !core::mem::replace(&mut w.told, true) {
            self.notice("RADIO RESET");
        }
        self.radio_restart(b);
    }

    /// The chip reset and started again as at boot (`App::new_with`), every channel that was open opened again, the
    /// phone link set up again. What the sensors showed goes stale by itself ("--") and comes back with their data.
    #[inline(never)] // its buffers in its own frame
    fn radio_restart(&mut self, b: &mut impl Board) {
        let mut open = self.sensors;
        for (s, owed) in open.iter_mut().zip(self.watch.owed) {
            if *s == Sensor::Off {
                *s = owed;
            }
        }
        self.watch.owed = open;
        (self.scanning, self.ask) = (false, 0);
        self.resting_ms = [u32::MAX; Kind::ALL.len()];
        self.heard_ms = [u32::MAX; Kind::ALL.len()]; // a sensor back counts as after a drop-out (decoders restart)
        self.sensors = [Sensor::Off; Kind::ALL.len()];
        self.phone_down(b);
        self.ble = nrf::start(b).ok();
        self.rx.resync();
        self.strip.radio = self.ble.is_some();
        self.ant = false;
        self.di2 = false;
        let nets = Kind::ALL.map(Kind::network);
        if let (Some(_), Some(key)) = (self.ble, self.ant_key)
            && let Ok(di2) = nrf::ant_start(b, &mut self.rx, key, self.di2_key, &nets)
        {
            (self.ant, self.di2) = (true, di2);
        }
        for kind in Kind::ALL {
            let want = open[kind as usize];
            let id = match want {
                Sensor::Paired(id) => Some(id),
                _ => None,
            };
            if want != Sensor::Off
                && self.ant
                && (kind.network() == 0 || self.di2)
                && nrf::ant_open(b, &mut self.rx, kind.channel(), kind.device_type(), kind.period(), id).is_ok()
            {
                self.sensors[kind as usize] = want;
                self.watch.owed[kind as usize] = Sensor::Off;
            }
        }
        self.watch.phone_owed = false;
        if self.ble.is_some() {
            self.rx.failed = None;
            self.phone_up(b);
            self.watch.phone_owed = self.phone.link() == Link::NoBle && matches!(self.rx.failed, Some((_, u32::MAX)));
        }
    }
}
