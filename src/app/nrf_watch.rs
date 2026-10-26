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
//! A refusal (an error code) is an answer, not a broken link: it is believed when it comes twice, noted, and not
//! retried by resets. After a resync the phone link is asked about ([`App::phone_recheck`]: the dropped bytes may
//! have held its end); the events already framed are kept.
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
    pub(crate) quiet_ms: u32,
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
    /// ANT, or a channel, was refused with an error code: the chip answers, so it is no broken link, and a reset
    /// would be refused again (review: it reset a working chip every 5 min for ever).
    pub(crate) refused: bool,
    /// The last refusal seen (command, code). A refusal is believed when it comes twice: a damaged link can
    /// turn a response into an error code, and a restart is cheap.
    refusal: Option<(u8, u32)>,
}

impl Watch {
    /// The start. A chip (or its ANT) that did not start at boot was just tried: the first recovery waits.
    pub const fn new(started: bool, refusal: Option<(u8, u32)>) -> Self {
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
            refused: false,
            refusal,
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
    pub(crate) fn probe(&mut self, b: &mut impl Board) -> bool {
        let failed = self.rx.failed; // a silence is not a failure of the phone's setup (PHONE menu): keep the last
        let r = nrf::call(b, &mut self.rx, &[nrf::CMD, nrf::SD_BLE_VERSION_GET, 1], &mut [0; 8]);
        if r == Err(nrf::Error::Timeout) {
            self.rx.failed = failed;
        }
        r != Err(nrf::Error::Timeout)
    }

    /// Once a pass, after the chip's events were read: counts the quiet, and recovers when the link is out of step.
    pub(crate) fn watch_radio(&mut self, b: &mut impl Board) {
        let heard = core::mem::take(&mut self.rx.heard);
        let (w, dt) = (&mut self.watch, self.dt_ms);
        w.quiet_ms = if heard { 0 } else { w.quiet_ms.saturating_add(dt) };
        w.wait_ms = w.wait_ms.saturating_sub(dt);
        w.calm_ms = w.calm_ms.saturating_add(dt);
        if w.calm_ms >= CALM_MS {
            (w.delay_ms, w.told) = (0, false);
        }
        if w.wait_ms > 0 {
            return;
        }
        // ANT wanted (a key) but not up, or a channel not reopened: a restart on a damaged link failed: another
        let ant_down = (self.ant_key.is_some() && !self.ant && !self.watch.refused)
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
                self.phone_recheck(b);
                self.ftms_recheck(b);
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

    /// A command was refused with an error code: true (and noted) when the same refusal came before.
    fn refusal(&mut self) -> bool {
        let seen = self.rx.failed.is_some() && self.rx.failed == self.watch.refusal;
        self.watch.refusal = self.rx.failed;
        self.watch.refused |= seen;
        seen
    }

    /// The chip reset and started again as at boot (`App::new_with`), every channel that was open opened again, the
    /// phone link set up again. What the sensors showed goes stale by itself ("--") and comes back with their data.
    #[inline(never)] // its buffers in its own frame
    pub(crate) fn radio_restart(&mut self, b: &mut impl Board) {
        let mut open = self.radio.sensors;
        for (s, owed) in open.iter_mut().zip(self.watch.owed) {
            if *s == Sensor::Off {
                *s = owed;
            }
        }
        self.watch.owed = open;
        (self.radio.scanning, self.radio.ask) = (false, 0);
        self.radio.resting_ms = [u32::MAX; Kind::ALL.len()];
        self.radio.heard_ms = [u32::MAX; Kind::ALL.len()]; // a sensor back counts as after a drop-out (decoders restart)
        self.radio.sensors = [Sensor::Off; Kind::ALL.len()];
        let pairing = self.phone.pairing();
        self.phone_down(b);
        self.ftms_down(); // connected again once the chip is back (app/ftms.rs)
        // the trainer's acknowledged messages are tried again on the new chip (app/trainer.rs)
        (self.trainer_no_control, self.trainer_acked) = (false, false);
        self.ble = nrf::start(b).ok();
        self.rx.reset();
        self.strip.radio = self.ble.is_some();
        self.ant = false;
        self.di2 = false;
        let was_refused = core::mem::take(&mut self.watch.refused);
        let nets = Kind::ALL.map(Kind::network);
        if let (Some(_), Some(key)) = (self.ble, self.ant_key) {
            match nrf::ant_start(b, &mut self.rx, key, self.di2_key, &nets) {
                Ok(di2) => (self.ant, self.di2) = (true, di2),
                Err(nrf::Error::Code(_)) => drop(self.refusal()),
                Err(nrf::Error::Timeout) => {}
            }
        }
        for kind in Kind::ALL {
            let want = open[kind as usize];
            let id = match want {
                Sensor::Paired(id) => Some(id),
                _ => None,
            };
            if want == Sensor::Off {
                continue;
            }
            let possible = self.ant && (kind.network() == 0 || self.di2);
            let opened = possible
                && match nrf::ant_open(b, &mut self.rx, kind.channel(), kind.device_type(), kind.period(), id) {
                    Ok(()) => true,
                    Err(nrf::Error::Code(_)) if self.refusal() => false,
                    Err(_) => continue, // owed: asked for again
                };
            if opened {
                self.radio.sensors[kind as usize] = want;
            }
            // opened, refused or impossible (no Shimano key): not owed. Only a link that does not answer is.
            if self.ant || self.watch.refused {
                self.watch.owed[kind as usize] = Sensor::Off;
            }
        }
        if self.watch.refused && !was_refused {
            self.notice("ANT REFUSED");
        }
        self.watch.phone_owed = false;
        if self.ble.is_some() {
            self.rx.failed = None;
            self.phone_up(b);
            if pairing {
                self.phone_pair(b); // the PAIR window was open: it goes on (the screen still says so)
            }
            self.watch.phone_owed = self.phone.link() == Link::NoBle && matches!(self.rx.failed, Some((_, u32::MAX)));
        }
    }
}
