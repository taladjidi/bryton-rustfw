//! The main loop's pass, in order (main.rs `main` runs [`MainLoop::pass`] for ever; the simulator's tests run the
//! same code): the stack checks around the step, the cable, the sleep in slices with the USB link served in it and
//! its effects, the radio passes a byte from the Nordic chip wakes, the input after the sleep, the pass counted. The
//! decisions and their order are here, tested on the host (docs/review-2026-11-01.md, finding 31: the simulator ran a
//! hand-made copy that drifted); the rules each one asks are [`LoopPolicy`]'s; the board does only the transport
//! ([`LoopBoard`]: registers, WFI, the reset, the staged image's run; [`UsbLink`]: usb.rs's link).

use crate::dev::{Effect, Host, Session, Stats, UsbWatch};
use crate::prof::Sec;
use crate::stopbench::{BKP, Mode, Outcome, StopReport, WUTF, pack};
use crate::ui::UsbDot;
use crate::usbsnap::UsbSnap;
use crate::{App, Board, Cable, LoopPolicy, Woke};

/// Radio passes in one pass's sleep at most: past them the sleep ends and the step runs, whatever the chip sends (a
/// pass must end: the clock, the battery, AUTO OFF and the unplug reset are the step's). A sensor's broadcasts at 4 Hz
/// make about one a pass.
pub const MAX_RADIO_PASSES: u32 = 16;

/// What the main loop needs of the board besides [`Board`]: primitives, each one register access or one CPU
/// instruction's worth (main.rs has them, the simulator a model). Those that end the run on the unit (a reset, a
/// fault, the staged image) return in the simulator, and the pass returns after them.
pub trait LoopBoard: Board {
    /// The USB link once started.
    type Link: UsbLink;
    /// The word above `.bss` the stack must not reach ([`crate::stack_intact`] checks it).
    fn canary(&mut self) -> u32;
    /// The stack reached the canary: the fault marked, then a reset (never returns on the unit).
    fn stack_overflowed(&mut self);
    /// A diagnostic build's stop at boot stage `n` (main.rs `checkpoint`).
    fn checkpoint(&mut self, n: u32);
    /// The cable is in (VBUS).
    fn vbus(&mut self) -> bool;
    /// Starts USB: the link, or None if it did not come up.
    fn usb_start(&mut self) -> Option<Self::Link>;
    /// USB off, the link dropped first (usb.rs `stop`: as `power_down`, PA2 left as it is); nothing brings it back but
    /// a reset.
    fn usb_stop(&mut self);
    /// System reset (never returns on the unit).
    fn reset(&mut self);
    /// Runs the image the dev link staged, `size` bytes, after quieting the wake lines an older image's sleep does not
    /// clear (never returns on the unit).
    fn run_image(&mut self, size: u32);
    /// Runs the stack down past its canary on purpose (the dev link's `stackfault`), then returns.
    fn overflow_stack(&mut self);
    /// The dev link's Stop benches (docs/power.md "Stop mode", phase 1): the SDRAM in self-refresh around one WFI of
    /// `ms` ended by the RTC wake-up timer (or the power key), the system in Stop but for [`Mode::Refresh`] (CSleep,
    /// the clocks kept), USB fully off first for [`Mode::UsbOff`], the clocks and the SDRAM restored, the image's CRC
    /// compared; SysTick is left stopped (the slice's timer is lost).
    fn low_power(&mut self, ms: u32, mode: Mode) -> crate::stopbench::StopReport;
    /// An idle Stop of `ms` at most between passes (docs/power.md "Stop mode", phase 2a; [`App::stop_ok`] and
    /// [`crate::Stops::allowed`] said yes): the SDRAM in self-refresh, the system in Stop, ended by the RTC wake-up
    /// timer, a key, the touch INT, the cable or the power key, the clocks and the SDRAM restored; the Nordic chip held
    /// off meanwhile (RTS high). The slice's timer is not running. Returns the us slept by the RTC, from the call to its
    /// return, and the flags (stopbench.rs: [`crate::stopbench::PENDING`] when it did not try, [`crate::stopbench::PASS_OK`] when all went
    /// right, [`WUTF`] when its timer ended it). The key and touch EXTI flags stay for [`LoopBoard::key_edge`] and
    /// [`LoopBoard::touch_edge`]. `ride`: a ride's Stop (phase 2b, [`App::ride_stop_ok`]), built or not on its own
    /// (main.rs `POWER_STEPS` bit 6, the idle one's bit 5). It does not try while our bytes to the Nordic chip are
    /// still going out.
    fn idle_stop(&mut self, ms: u32, ride: bool) -> (u32, u32);
    /// The ride's Stop is built in (main.rs `POWER_STEPS` bit 6), for `info`.
    const RIDE_STOP_BUILT: bool;
    /// main.rs `POWER_STEPS` (the build's sleep steps), for `info`.
    const POWER_STEPS: u32;
    /// Drops what the Nordic chip sent (nobody reads it).
    fn nrf_flush(&mut self);
    /// The key lines end WFI (`keys`) or not, and the Nordic chip's bytes (`bytes`).
    fn wake_on_io(&mut self, keys: bool, bytes: bool);
    /// Starts a sleep slice's timer of `ms`; its end wakes WFI.
    fn timer_start(&mut self, ms: u32);
    /// The slice's timer ran out.
    fn timer_done(&mut self) -> bool;
    /// Microseconds of a slice of `ms` gone so far.
    fn timer_elapsed_us(&mut self, ms: u32) -> u32;
    /// Stops the slice's timer.
    fn timer_stop(&mut self);
    /// The touch INT fell since the last call.
    fn touch_edge(&mut self) -> bool;
    /// A key went down since the last call.
    fn key_edge(&mut self) -> bool;
    /// A byte from the Nordic chip waits.
    fn byte_waiting(&mut self) -> bool;
    /// WFI: until a wake source fires.
    fn sleep(&mut self);
    /// The profiler's I/O counts since the last call (pixels to the panel, eMMC sectors read, written).
    fn take_io(&mut self) -> (u32, u32, u32);
}

/// A Stop bench that answers instead of resetting (`bench srefresh`, `bench stop <ms> keep`): the sleep, then its line;
/// for `keep` the verdict goes to backup register 0 first, so it survives a link that does not.
fn bench_answer<B: LoopBoard>(b: &mut B, l: &mut B::Link, ms: u32, mode: Mode) {
    let r = b.low_power(ms, mode);
    if mode == Mode::Keep {
        let ([reg, _], [verdict, _]) = (BKP, pack(&r));
        b.backup_write(reg, verdict);
    }
    crate::stopbench::send(l, &r);
    l.flush();
}

/// After a Stop with the cable in or a stall, the link must be up this long after (ms), else it is reconnected, up to
/// `RECOVER_TRIES` times `RECOVER_RETRY_MS` apart. A poll later than `STALL_GAP_MS` after the last is a stall.
const RECOVER_MS: u32 = 3000;
const RECOVER_RETRY_MS: u32 = 10_000;
const RECOVER_TRIES: u8 = 3;
const STALL_GAP_MS: u32 = 3000;

/// The USB link as the pass serves it (main.rs: usb.rs `Link`).
pub trait UsbLink: Host {
    /// Services USB: a drive command (through the app), the dev link's bytes through `s`: what the board must do.
    fn poll(&mut self, s: &mut Session, app: &mut App, board: &mut impl Board, stats: Stats) -> Effect;
    /// Keeps servicing USB for `ms`, so the last answer reaches the computer before a reset or a jump.
    fn linger(&mut self, ms: u32);
    /// Soft disconnect, so the computer sees the device leave before the next image enumerates again.
    fn disconnect(&mut self);
    /// Watches the link once a poll (`now_ms`: the loop's clock; `failed`: the drive's last command failed).
    fn watch(&mut self, now_ms: u32, failed: bool) -> UsbWatch;
    /// The first failure's registers since the link came up, once.
    fn snapshot(&mut self) -> Option<UsbSnap>;
    /// Soft disconnect and connect, so the computer enumerates the device again (a link it forgot while we were away).
    fn recover(&mut self, now_ms: u32);
    /// Board work done for the link since the last call (it may have slept: the slice's end is lost).
    fn worked(&mut self) -> bool;
}

/// The main loop's state between passes: the USB link once started, the dev link's protocol, the loop's rules.
pub struct MainLoop<L> {
    link: Option<L>,
    /// When the link must be up again (ms), the reconnects left, and whether a Stop with the cable in armed it (only
    /// that one may reconnect a suspended link: after a stall, a suspend is a host that sleeps, not one that forgot us).
    recover: Option<(u32, u8, bool)>,
    /// The watch saw the link `Up` since the last reconnect (a charger with no data host never does: no stall recovery).
    seen_up: bool,
    /// The loop's clock at the last USB poll, ms (0: none yet).
    last_poll: u32,
    session: Session,
    policy: LoopPolicy,
}

impl<L: UsbLink> MainLoop<L> {
    /// A loop for a cycle counter at `cpu_hz` and sleep slices of at most `timer_max_ms` (SysTick's longest).
    pub const fn new(cpu_hz: u32, timer_max_ms: u32) -> Self {
        MainLoop {
            link: None,
            recover: None,
            seen_up: false,
            last_poll: 0,
            session: Session::new(),
            policy: LoopPolicy::new(cpu_hz, timer_max_ms),
        }
    }

    /// The figures `info` reports.
    #[cfg(test)]
    pub(crate) fn stats(&self) -> Stats {
        self.policy.stats()
    }

    /// One pass: the step, then the sleep until the next one (USB served, the touch read, a key or the link's input
    /// acted on after it). Returns the radio passes run in the sleep. On the unit it returns only to run again, or
    /// never (a reset, a fault, the staged image).
    pub fn pass<B: LoopBoard<Link = L>>(&mut self, app: &mut App, b: &mut B) -> u32 {
        // before the step too: an overflow in the last pass's sleep loop (USB, the touch) is caught before the
        // recorder runs on what it overwrote
        if !stack_ok(b) {
            return 0;
        }
        let t0 = b.cycles();
        app.step(b);
        if !stack_ok(b) {
            return 0;
        }
        app.prof().span(Sec::Step, t0, b.cycles());
        b.checkpoint(6);
        match self.policy.cable(b.vbus(), app.busy()) {
            Cable::Start => {
                self.link = b.usb_start();
                app.set_usb(b, if self.link.is_some() { UsbDot::Powered } else { UsbDot::Failed });
            }
            // unplugged, no ride or its card: the reset powers the PHY and the USB core down again
            Cable::Reset => {
                b.reset();
                return 0;
            }
            // unplugged during a ride or its save: USB powered down now (a reset would cost the ride's checkpoint
            // gap), not at the ride's end; a plug-in again gets no USB until that reset (its buffers are once-cells)
            // (once: a start that failed powered USB up too)
            Cable::Stop => {
                (self.link, self.recover, self.seen_up) = (None, None, false);
                b.usb_stop();
                app.usb_gone(b);
            }
            Cable::Later => app.notice("USB AFTER THE RIDE"),
            Cable::Keep => {}
        }
        // Sleep until the next pass, waking for USB activity. App work waits for the end of the sleep: it may sleep
        // too (delay_ms), which would take over this timer.
        let mut press = None;
        let mut touched = None;
        let mut woke = false;
        let (mut radio_passes, mut slice_cycles) = (0, 0u32);
        // the pass's sleep, in slices: TOUCH_MS ones while a finger is on the panel, and one ends early when the
        // touch INT falls, so the panel is read at once and then every TOUCH_MS (stock: an EXTI6 wake, then reads
        // while the finger is down)
        // idle: a longer sleep (App::pass_ms), ended at once by a key, and woken by a byte from the Nordic chip for a
        // radio pass (App::radio_pass), so neither waits for it (the chip's own buffers are not known: it is read as
        // soon as it sends, as before); a ride's pass is the short one (LoopPolicy::begin_sleep)
        let wake = self.policy.begin_sleep(app.pass_ms(), app.radio_up());
        // one Stop a pass, for the pass's whole sleep (docs/power.md "Stop mode"): an idle pass's (phase 2a) or a ride's
        // (2b: at most the rest of its POLL_MS, after the step that did the recorder's card work), the app's rules,
        // then the loop's (no USB link but for the bench, not after failures in a row)
        self.policy.stops.ride_built = B::RIDE_STOP_BUILT;
        let cable = self.link.is_some() || b.vbus();
        let ride = !wake.io && app.ride_stop_ok(b);
        let mut stop = (ride || wake.io && app.stop_ok(b)) && self.policy.stops.allowed(cable, self.policy.now_ms());
        if !wake.radio {
            b.nrf_flush();
        }
        if wake.io || wake.bytes {
            b.wake_on_io(wake.io, wake.bytes); // a short pass keeps the key lines off (a held key would end every WFI)
        }
        'sleep: loop {
            // a finger down makes the pass POLL_MS again
            let Some(slice) = self.policy.slice(app.pass_ms(), app.touching()) else {
                break;
            };
            let mut slice_at = b.cycles();
            // a Stop that slept is this slice; one that did not try (a wake source pending) leaves it to WFI
            let stopped = if core::mem::take(&mut stop) {
                if ride {
                    // a short pass never reads the keys' EXTI flags (`key_edge`): one left from a press would keep
                    // every ride Stop out (PENDING); the step polls the keys themselves
                    let _ = b.key_edge();
                }
                let (us, flags) = b.idle_stop(app.pass_ms().saturating_sub(self.policy.slept_ms()), ride);
                match self.policy.stops.count(us, flags, cable, self.policy.now_ms(), ride) {
                    // its look (the radio's 20 us) was work: the slice starts after it, or the app clock loses it
                    Outcome::NotTried => {
                        slice_at = b.cycles();
                        None
                    }
                    // the clocks or the SDRAM not as before: the report kept as `bench stop` keeps it, then a reset;
                    // a ride resumes from its checkpoint, and does not stop again while the report is there
                    // (`App::ride_stop_ok`, `stopbench::broke`)
                    Outcome::Broken => {
                        let r = StopReport { stop: true, flags, slept_ms: us / 1000, ..StopReport::default() };
                        for (n, w) in BKP.into_iter().zip(pack(&r)) {
                            b.backup_write(n, w);
                        }
                        b.reset();
                        return radio_passes;
                    }
                    o => {
                        // macOS drops a device that is not served for a Stop: it must be back up after the last one
                        if self.link.is_some() {
                            self.recover = Some((self.policy.now_ms().wrapping_add(RECOVER_MS), RECOVER_TRIES, true));
                        }
                        Some((us, flags, o))
                    }
                }
            } else {
                None
            };
            // SysTick's count before a USB command: one that slept (`worked`) took the timer over, so the slice's
            // length is this, and the command's own delays are in `waited` (counted once)
            let mut before_poll = 0;
            let mut before_cycles = slice_at; // the cycles at before_poll
            let mut took_over = false;
            let w = if let Some((_, flags, o)) = stopped {
                let now = b.cycles();
                app.prof().span(Sec::Wake, slice_at, now);
                self.policy.woke(now.wrapping_sub(slice_at));
                // its timer, or something to act on now; a failed one sleeps the rest of the pass in WFI
                let early = flags & WUTF == 0 && matches!(o, Outcome::Ok | Outcome::Interrupted);
                Woke { touch: b.touch_edge(), io: wake.io && b.key_edge(), stop: early, ..Woke::default() }
            } else {
                b.timer_start(slice);
                loop {
                    let usb_at = b.cycles();
                    if let Some(l) = &mut self.link {
                        before_poll = b.timer_elapsed_us(slice);
                        before_cycles = b.cycles();
                        let stats = Stats { power_steps: B::POWER_STEPS, ..self.policy.stats() };
                        match l.poll(&mut self.session, app, b, stats) {
                            Effect::Key(p) => press = Some(p),
                            Effect::Reset => {
                                l.linger(50);
                                b.reset();
                                return radio_passes;
                            }
                            Effect::StackFault => {
                                l.linger(50);
                                b.overflow_stack();
                                // a pass: resets with the fault marker; a miss (no marker): the tail of .bss is
                                // overwritten all the same, so never run on
                                if stack_ok(b) {
                                    b.reset();
                                }
                                return radio_passes;
                            }
                            // SysTick was stopped: the slice's end is lost, as after a drive command that slept
                            Effect::BenchSrefresh(ms) => {
                                bench_answer(b, l, ms, Mode::Refresh);
                                (took_over, woke) = (true, true);
                            }
                            Effect::BenchIdleStop(secs) => self.policy.stops.bench(self.policy.now_ms(), secs),
                            Effect::BenchStop(ms, Mode::Keep) => {
                                bench_answer(b, l, ms, Mode::Keep);
                                (took_over, woke) = (true, true);
                            }
                            Effect::BenchStop(ms, mode) => {
                                // the `ok` reaches the computer, then the device leaves the bus: USB needs VOS1 and its
                                // clocks, which Stop takes away (docs/power.md "Stop mode")
                                l.linger(50);
                                l.disconnect();
                                let r = b.low_power(ms, mode);
                                for (n, w) in BKP.into_iter().zip(pack(&r)) {
                                    b.backup_write(n, w);
                                }
                                b.reset();
                                return radio_passes;
                            }
                            Effect::Run(size) => {
                                l.linger(50);
                                l.disconnect();
                                b.run_image(size);
                                return radio_passes;
                            }
                            e => woke |= app.serve(b, l, &self.session, e),
                        }
                        // after the effect's own answer: the computer wrote to the volume and ejected it, a restart drops
                        // every cache of it
                        if app.restart_wanted() {
                            l.linger(50);
                            b.reset();
                            return radio_passes;
                        }
                        let now = self.policy.now_ms();
                        let watched = l.watch(now, app.drive_failed());
                        // a poll long after the last (a stall, a long job) loses the link the same way, if it was up
                        if self.last_poll != 0 && now.wrapping_sub(self.last_poll) > STALL_GAP_MS && self.seen_up {
                            let stop = self.recover.is_some_and(|r| r.2);
                            self.recover = Some((now.wrapping_add(RECOVER_MS), RECOVER_TRIES, stop));
                        }
                        self.last_poll = now.max(1);
                        self.seen_up = match watched {
                            UsbWatch::Up => true,
                            UsbWatch::Drop => false,
                            _ => self.seen_up,
                        };
                        // due and still not up (never enumerated; suspended for good after a Stop): the pull-up off and
                        // on, the only thing a host that forgot the port notices (inferred; docs/usb.md). A drop did
                        // that itself; it counts as a try.
                        if let Some((_, left, stop)) = self.recover.filter(|r| now.wrapping_sub(r.0) < 1 << 31) {
                            let next = Some((now.wrapping_add(RECOVER_RETRY_MS), left.saturating_sub(1), stop));
                            self.recover = match watched {
                                UsbWatch::Up => None,
                                UsbWatch::Suspended if !stop => None,
                                _ if left == 0 => None,
                                UsbWatch::Drop => next,
                                _ => {
                                    l.recover(now);
                                    self.seen_up = false;
                                    self.policy.stops.recovered();
                                    next
                                }
                            };
                        }
                        // before usb_dropped, which leaves registers 0 and 31 to a snapshot written for this drop
                        // a drop at the end of `bench idle-stop` is the bench's: not logged
                        let quiet = self.policy.stops.quiet_drop(self.policy.now_ms());
                        if let Some(s) = l.snapshot().filter(|_| !quiet) {
                            app.usb_snapshot(b, &s);
                        }
                        let dot = match watched {
                            UsbWatch::Up => UsbDot::Ready,
                            UsbWatch::Suspended => UsbDot::Suspended,
                            UsbWatch::Down => UsbDot::Powered,
                            UsbWatch::Drop => {
                                if !quiet {
                                    app.usb_dropped(b);
                                }
                                UsbDot::Powered
                            }
                        };
                        // a drive command read or wrote the card, which may have slept and taken over SysTick: without
                        // this, the sleep's end is lost and the main loop stalls while USB is still served (seen on the
                        // unit, 2026-10-24: the uptime stopped, the dev link kept answering)
                        let worked = l.worked();
                        took_over |= worked;
                        woke |= worked;
                        app.set_usb(b, dot);
                        app.prof().span(Sec::Usb, usb_at, b.cycles());
                    }
                    let w = Woke {
                        touch: b.touch_edge(),
                        timer: b.timer_done(),
                        io: wake.io && b.key_edge(),
                        bytes: wake.bytes && b.byte_waiting(),
                        key: press.is_some(),
                        usb: woke,
                        stop: false,
                    };
                    if w.any() {
                        // a key from the link acts now, so the next command cannot replace it; board work done for the
                        // link (an eMMC read) may have slept, which takes over SysTick: this sleep's end is lost, so end it
                        break w;
                    }
                    b.sleep();
                }
            };
            // SysTick keeps counting through WFI (ARMv7-M ARM B3.3), so its count is the wall time of the slice.
            // An early end counts SysTick's us, the sub-ms rest carried (floored to whole ms each, ~0.5 ms was lost per
            // radio wake: the app clock ran 0.35 % slow, 2026-11-02). A chip sending faster than a byte a ms would keep
            // every slice near 0: the rests add up, and MAX_RADIO_PASSES bounds the pass all the same
            if took_over {
                self.policy.count(true);
            }
            if let Some((us, ..)) = stopped {
                self.policy.slept_stop_us(us); // by the RTC: SysTick and the cycle counter stop in Stop
            } else if w.timer && !took_over {
                self.policy.slept(slice);
            } else {
                let us = if took_over { before_poll } else { b.timer_elapsed_us(slice) };
                self.policy.slept_us(us);
            }
            b.timer_stop();
            // only the cycles inside slices are "in the sleep's ms": the rest (the touch, the io toggles) is work
            // a taken-over slice is `before_poll` long: the command's own cycles after it are work, not that sleep's ms
            // (they were subtracted from the work: the 0.046 % left on the unit, 1200 such slices in 600 s)
            let end = if took_over { before_cycles } else { b.cycles() };
            slice_cycles = slice_cycles.wrapping_add(end.wrapping_sub(slice_at));
            // the panel before the early end below: with the USB drive busy, every slice ends early, and touch was
            // never read (seen on the unit, 2026-10-24: no gesture during a copy to the drive)
            if w.read_touch(app.touching()) {
                // a gesture acts after the sleep: acting may sleep too (delay_ms), which takes over SysTick
                let t = b.cycles();
                touched = app.touch(b, self.policy.take_touch_ms());
                app.prof().span(Sec::Touch, t, b.cycles());
                if touched.is_some() {
                    break 'sleep;
                }
            }
            if w.ends_pass() {
                break;
            }
            // between slices (the timer stopped: the pass may sleep), its key and byte lines off (a waiting byte
            // would end every WFI of its waits); its time is work, not sleep, in the step's dt and the awake share
            if w.bytes && self.policy.slept_ms() < app.pass_ms() {
                b.wake_on_io(false, false);
                let t = b.cycles();
                app.radio_pass(b, self.policy.slept_ms());
                let now = b.cycles();
                app.prof().span(Sec::Radio, t, now);
                radio_passes += 1;
                self.policy.count(false);
                if !app.radio_up() {
                    break; // the chip went down in it (a failed restart): nothing reads its bytes, as before
                }
                if radio_passes >= MAX_RADIO_PASSES {
                    break; // a chip that never stops sending: the step runs all the same, the pass is bounded
                }
                b.wake_on_io(wake.io, wake.bytes);
            }
        }
        if wake.io || wake.bytes {
            b.wake_on_io(false, false);
        }
        let sleep_cycles = slice_cycles;
        let slept = self.policy.slept_ms();
        app.prof().clock.slept(slept, sleep_cycles / (B::CPU_HZ / 1_000_000));
        let t1 = b.cycles();
        if press.is_some() || touched.is_some() {
            if let Some(p) = press {
                app.press(b, p);
            }
            if let Some(i) = touched {
                app.input(b, i);
            }
            app.prof().span(Sec::Input, t1, b.cycles());
        }
        b.checkpoint(7);
        // the app's work is the pass less its sleep loop (USB polls, a WFI that does not sleep)
        // the whole pass's waits in WFI (the step's DMA, a touch's, a radio pass's, an input's): work, not sleep
        let waited = b.waited_us();
        app.prof().clock.owed_us = app.prof().clock.owed_us.saturating_add(waited); // the next step's work
        let pass_ms = self.policy.account(b.cycles().wrapping_sub(t0), sleep_cycles, waited);
        app.prof().pass(pass_ms, slept);
        app.prof().io(b.take_io());
        radio_passes
    }
}

/// The stack canary still holds its pattern, or the fault (the marker, a reset, the ride resumes from its
/// checkpoint): false. An overflow cannot be caught when it happens: with interrupts masked (PRIMASK) an MPU guard's
/// fault escalates to HardFault, whose stacking on the overflowed stack locks the core up (seen on the unit
/// 2026-10-29, BOARD.md 2). One load and a compare.
#[inline(always)]
fn stack_ok(b: &mut impl LoopBoard) -> bool {
    let ok = crate::stack_intact(b.canary());
    if !ok {
        b.stack_overflowed();
    }
    ok
}
