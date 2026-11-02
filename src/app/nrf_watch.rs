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
//! minutes. The recorder never waits on any of it beyond the commands' own time-outs (500 ms each, one per span between two GNSS reads: `Rx::stalled`): it records from the app's own state, and a pass that took
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

/// The stages of a chip reset. PA6 is held low for [`nrf::RESET_HOLD_MS`], then released, then the chip boots for
/// [`nrf::BOOT_MS`] (BOARD.md 11: stock's 150 ms and 800 ms), then the start sequence runs one stage per pass, each
/// stage waiting only for its own commands' answers. The main loop passes at its normal period meanwhile (the GNSS
/// is read, the screen and keys served): the reset used to block it for about a second.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Reset {
    /// No reset in progress.
    None,
    /// PA6 low.
    Held,
    /// PA6 released, the chip boots.
    Boot,
    /// The chip is configured (BLE enabled, version read): ANT and the channels are next.
    Configure,
    /// ANT started and the channels that were open reopened: the phone link is next.
    Channels,
}

/// The watchdog's state.
pub struct Watch {
    /// Since the last frame from the chip.
    pub(crate) quiet_ms: u32,
    /// Until the next recovery may run.
    pub(crate) wait_ms: u32,
    /// Where a chip reset is (it runs across passes, nothing waits inside one: [`App::restart_step`]).
    pub(crate) reset: Reset,
    /// In that stage, ms.
    reset_ms: u32,
    /// The configuration being tried by this reset (1: two links, 0: the chip's defaults; as `nrf::start`).
    level: u8,
    /// The PAIR window was open when the reset began.
    pub(crate) pairing: bool,
    /// The wait after the last chip reset (0: none yet in this calm).
    pub(crate) delay_ms: u32,
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
    /// What the rider chose during a restart (a chip not yet ready): set when it is over, never lost.
    pub(crate) later: [Option<Sensor>; Kind::ALL.len()],
    /// The phone link's setup got no answer at a restart (a refusal is not this: it would be refused again).
    phone_owed: bool,
    /// ANT, or a channel, was refused with an error code: the chip answers, so it is no broken link, and a reset
    /// would be refused again (review: it reset a working chip every 5 min for ever).
    pub(crate) refused: bool,
    /// The last refusal seen (command, code). A refusal is believed when it comes twice: a damaged link can
    /// turn a response into an error code, and a restart is cheap.
    refusal: Option<(u8, u32)>,
    /// The time radio passes (App::radio_pass) counted since the last step: the step counts the rest.
    pub(crate) ahead_ms: u32,
}

impl Watch {
    /// The start. A chip (or its ANT) that did not start at boot was just tried: the first recovery waits.
    pub const fn new(started: bool, refusal: Option<(u8, u32)>) -> Self {
        let wait_ms = if started { 0 } else { FIRST_WAIT_MS };
        Watch {
            quiet_ms: 0,
            wait_ms,
            reset: Reset::None,
            reset_ms: 0,
            level: 1,
            pairing: false,
            delay_ms: 0,
            calm_ms: 0,
            resets: 0,
            resyncs: 0,
            told: false,
            owed: [Sensor::Off; Kind::ALL.len()],
            later: [None; Kind::ALL.len()],
            phone_owed: false,
            refused: false,
            refusal,
            ahead_ms: 0,
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
    /// The Nordic chip is started and its bytes read each pass (main.rs wakes an idle sleep on them only then, and
    /// flushes them otherwise).
    pub fn radio_up(&self) -> bool {
        self.ble.is_some()
    }

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

    /// After the chip's events were read (a step, a radio pass), `dt` ms after the last time: counts the quiet, and
    /// recovers when the link is out of step.
    pub(crate) fn watch_radio(&mut self, b: &mut impl Board, dt: u32) {
        let heard = core::mem::take(&mut self.rx.heard);
        let w = &mut self.watch;
        w.quiet_ms = if heard { 0 } else { w.quiet_ms.saturating_add(dt) };
        w.wait_ms = w.wait_ms.saturating_sub(dt);
        w.calm_ms = w.calm_ms.saturating_add(dt);
        if w.calm_ms >= CALM_MS {
            (w.delay_ms, w.told) = (0, false);
        }
        if w.reset != Reset::None {
            self.restart_step(b, dt);
            return;
        }
        // a frame that went out partly proves the chip mis-framed: the first such break of a calm period resets at
        // once (later ones keep the backoff, as a chip that breaks again at once is not cured by resets)
        if w.wait_ms > 0 && !(self.rx.broken && w.delay_ms == 0) {
            return;
        }
        // ANT wanted (a key) but not up, or a channel not reopened: a restart on a damaged link failed: another
        let ant_down = (self.ant_key.is_some() && !self.ant && !self.watch.refused)
            || self.watch.phone_owed
            || self.watch.owed.iter().any(|&s| s != Sensor::Off);
        let broken = self.ble.is_none() || ant_down || self.rx.broken || self.rx.strikes >= 2 || self.rx.timeouts >= 2;
        if !broken {
            if self.watch.quiet_ms >= PROBE_MS && !self.rx.stalled {
                self.read_gnss_now(b);
                // a probe that gets no answer is asked again in a second (one time-out may be a busy chip); two
                // in a row (`timeouts`) are a broken link
                self.watch.quiet_ms = if self.probe(b) { 0 } else { PROBE_MS - 1000 };
            }
            return;
        }
        // a command timed out in this span: the probe would be refused unasked, and two real time-outs already make the
        // link broken. Not asked: try again next pass, never a reset of a chip nobody probed
        if self.rx.stalled && self.rx.timeouts < 2 && !self.rx.broken {
            return;
        }
        // 1. start reading at a frame boundary and ask again
        // (not after a frame went out partly: the chip is mis-framed, only a reset cures it)
        if self.ble.is_some() && !ant_down && !self.rx.broken {
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
        self.restart_begin(b);
    }

    /// A command was refused with an error code: true (and noted) when the same refusal came before.
    fn refusal(&mut self) -> bool {
        let seen = self.rx.failed.is_some() && self.rx.failed == self.watch.refusal;
        self.watch.refusal = self.rx.failed;
        self.watch.refused |= seen;
        seen
    }

    /// Starts a chip reset: what the old chip held is forgotten, PA6 goes low, and [`App::restart_step`] does the
    /// rest, a stage a pass. As at boot (`App::new_with`) every channel that was open is opened again and the phone
    /// link set up again; what the sensors showed goes stale by itself ("--") and comes back with their data.
    #[inline(never)] // its buffers in its own frame
    pub(crate) fn restart_begin(&mut self, b: &mut impl Board) {
        let mut open = self.radio.sensors;
        for (s, owed) in open.iter_mut().zip(self.watch.owed) {
            if *s == Sensor::Off {
                *s = owed;
            }
        }
        self.watch.owed = open;
        (self.radio.scanning, self.radio.ask) = (false, 0);
        (self.radio.resting_ms, self.radio.misses) = ([u32::MAX; Kind::ALL.len()], [0; Kind::ALL.len()]);
        self.radio.heard_ms = [u32::MAX; Kind::ALL.len()]; // a sensor back counts as after a drop-out (decoders restart)
        self.radio.sensors = [Sensor::Off; Kind::ALL.len()];
        self.watch.pairing = self.phone.pairing();
        self.phone_down(b);
        self.ftms_down(); // connected again once the chip is back (app/ftms.rs)
        // the trainer's acknowledged messages are tried again on the new chip (app/trainer.rs)
        (self.tlink.no_control, self.tlink.acked) = (false, false);
        self.ble = None;
        self.strip.radio = false;
        self.ant = false;
        self.di2 = false;
        // nothing goes to the chip until its boot is over (`call` / `send` return at once); the hold flushes our TX FIFO
        self.rx.broken = true;
        b.nrf_hold(true);
        (self.watch.reset, self.watch.reset_ms, self.watch.level) = (Reset::Held, 0, 1);
    }

    /// A chip reset restarted by hand and run to its end, for the tests that need the chip back at once.
    #[cfg(test)]
    pub(crate) fn radio_restart(&mut self, b: &mut impl Board) {
        if self.watch.reset == Reset::None {
            self.restart_begin(b);
        }
        for _ in 0..16 {
            if self.watch.reset == Reset::None {
                break;
            }
            let ms = match self.watch.reset {
                Reset::Held => nrf::RESET_HOLD_MS,
                Reset::Boot => nrf::BOOT_MS,
                _ => 0,
            };
            b.delay_ms(ms);
            self.restart_step(b, ms);
        }
    }

    /// `dt` ms later: the next stage of a chip reset, if its time has come. Stages: PA6 low for 150 ms, released,
    /// 800 ms of boot (BOARD.md 11), then the configuration, the ANT channels and the phone link, one per pass; a
    /// chip that refuses the two-link configuration is reset once more and started on its defaults (as `nrf::start`).
    /// No stage waits for time, only for the answers of its own commands (at most their time-outs).
    pub(crate) fn restart_step(&mut self, b: &mut impl Board, dt: u32) {
        let w = &mut self.watch;
        w.reset_ms = w.reset_ms.saturating_add(dt);
        // a time-out in this span refuses every command: a stage that sends would lose its configuration, so it waits
        if self.rx.stalled && w.reset != Reset::Held {
            return;
        }
        match w.reset {
            Reset::Held if w.reset_ms >= nrf::RESET_HOLD_MS => {
                b.nrf_hold(false);
                (w.reset, w.reset_ms) = (Reset::Boot, 0);
            }
            Reset::Boot if w.reset_ms >= nrf::BOOT_MS => {
                self.read_gnss_now(b); // the stages that send may meet a silent chip: the receiver first
                nrf::flush(b); // a glitch byte at the release (seen on the unit)
                self.rx.reset();
                let level = self.watch.level;
                match nrf::configure(b, level) {
                    Err(nrf::Error::Code(_)) if level > 0 => {
                        (self.watch.level, self.watch.reset, self.watch.reset_ms) = (0, Reset::Held, 0);
                        self.rx.broken = true;
                        b.nrf_hold(true);
                    }
                    r => {
                        self.ble = r.ok();
                        self.strip.radio = self.ble.is_some();
                        self.watch.reset = Reset::Configure;
                    }
                }
            }
            Reset::Configure => {
                self.read_gnss_now(b);
                self.restart_channels(b);
                self.watch.reset = Reset::Channels;
            }
            Reset::Channels => {
                self.read_gnss_now(b);
                self.watch.reset = Reset::None; // the phone setup is the restart's own: not deferred
                self.restart_phone(b);
                self.apply_later(b);
            }
            _ => {}
        }
    }

    /// ANT started again and the channels that were open opened (the third stage).
    #[inline(never)]
    fn restart_channels(&mut self, b: &mut impl Board) {
        let open = self.watch.owed;
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
            let want = open.at(kind);
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
                    Err(_) => break, // owed: asked for again; the rest too, so one pass is one time-out at most
                };
            if opened {
                *self.radio.sensors.at_mut(kind) = want;
            }
            // opened, refused or impossible (no Shimano key): not owed. Only a link that does not answer is.
            if self.ant || self.watch.refused {
                *self.watch.owed.at_mut(kind) = Sensor::Off;
            }
        }
        if self.watch.refused && !was_refused {
            self.notice("ANT REFUSED");
        }
    }

    /// The sensors the rider chose during the restart are set now, the restart being over.
    fn apply_later(&mut self, b: &mut impl Board) {
        for kind in Kind::ALL {
            if let Some(want) = self.watch.later.at_mut(kind).take() {
                self.set_sensor(b, kind, want);
            }
        }
    }

    /// The phone link set up again, its pairing window reopened if it was open (the last stage).
    fn restart_phone(&mut self, b: &mut impl Board) {
        let pairing = self.watch.pairing;
        self.watch.phone_owed = false;
        if self.ble.is_some() {
            self.rx.failed = None;
            self.phone_up(b);
            if pairing && self.phone.ready {
                self.phone_pair(b); // the PAIR window was open: it goes on (the screen still says so)
            }
            self.watch.phone_owed = self.phone.link() == Link::NoBle && matches!(self.rx.failed, Some((_, u32::MAX)));
        }
    }
}
