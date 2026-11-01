//! The main loop's pass, in order (main.rs `main` runs [`MainLoop::pass`] for ever; the simulator's tests run the
//! same code): the stack checks around the step, the cable, the sleep in slices with the USB link served in it and
//! its effects, the radio passes a byte from the Nordic chip wakes, the input after the sleep, the pass counted. The
//! decisions and their order are here, tested on the host (docs/review-2026-11-01.md, finding 31: the simulator ran a
//! hand-made copy that drifted); the rules each one asks are [`LoopPolicy`]'s; the board does only the transport
//! ([`LoopBoard`]: registers, WFI, the reset, the staged image's run; [`UsbLink`]: usb.rs's link).

use crate::dev::{Effect, Host, Session, Stats, UsbWatch};
use crate::prof::Sec;
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
    /// System reset (never returns on the unit).
    fn reset(&mut self);
    /// Runs the image the dev link staged, `size` bytes, after quieting the wake lines an older image's sleep does not
    /// clear (never returns on the unit).
    fn run_image(&mut self, size: u32);
    /// Runs the stack down past its canary on purpose (the dev link's `stackfault`), then returns.
    fn overflow_stack(&mut self);
    /// Drops what the Nordic chip sent (nobody reads it).
    fn nrf_flush(&mut self);
    /// The key lines end WFI (`keys`) or not, and the Nordic chip's bytes (`bytes`).
    fn wake_on_io(&mut self, keys: bool, bytes: bool);
    /// Starts a sleep slice's timer of `ms`; its end wakes WFI.
    fn timer_start(&mut self, ms: u32);
    /// The slice's timer ran out.
    fn timer_done(&mut self) -> bool;
    /// Milliseconds of a slice of `ms` gone so far.
    fn timer_elapsed_ms(&mut self, ms: u32) -> u32;
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
    /// Board work done for the link since the last call (it may have slept: the slice's end is lost).
    fn worked(&mut self) -> bool;
}

/// The main loop's state between passes: the USB link once started, the dev link's protocol, the loop's rules.
pub struct MainLoop<L> {
    link: Option<L>,
    session: Session,
    policy: LoopPolicy,
}

impl<L: UsbLink> MainLoop<L> {
    /// A loop for a cycle counter at `cpu_hz` and sleep slices of at most `timer_max_ms` (SysTick's longest).
    pub const fn new(cpu_hz: u32, timer_max_ms: u32) -> Self {
        MainLoop { link: None, session: Session::new(), policy: LoopPolicy::new(cpu_hz, timer_max_ms) }
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
            Cable::Keep => {}
        }
        // Sleep until the next pass, waking for USB activity. App work waits for the end of the sleep: it may sleep
        // too (delay_ms), which would take over this timer.
        let mut press = None;
        let mut touched = None;
        let sleep_start = b.cycles();
        let mut woke = false;
        let (mut radio_passes, mut radio_cycles) = (0, 0u32);
        // the pass's sleep, in slices: TOUCH_MS ones while a finger is on the panel, and one ends early when the
        // touch INT falls, so the panel is read at once and then every TOUCH_MS (stock: an EXTI6 wake, then reads
        // while the finger is down)
        // idle: a longer sleep (App::pass_ms), ended at once by a key, and woken by a byte from the Nordic chip for a
        // radio pass (App::radio_pass), so neither waits for it (the chip's own buffers are not known: it is read as
        // soon as it sends, as before); a ride's pass is the short one (LoopPolicy::begin_sleep)
        let wake = self.policy.begin_sleep(app.pass_ms(), app.radio_up());
        if !wake.radio {
            b.nrf_flush();
        }
        if wake.io {
            b.wake_on_io(true, wake.bytes);
        }
        'sleep: loop {
            // a finger down makes the pass POLL_MS again
            let Some(slice) = self.policy.slice(app.pass_ms(), app.touching()) else {
                break;
            };
            b.timer_start(slice);
            let w = loop {
                let usb_at = b.cycles();
                if let Some(l) = &mut self.link {
                    match l.poll(&mut self.session, app, b, self.policy.stats()) {
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
                    let watched = l.watch(self.policy.now_ms(), app.drive_failed());
                    // before usb_dropped, which leaves registers 0 and 31 to a snapshot written for this drop
                    if let Some(s) = l.snapshot() {
                        app.usb_snapshot(b, &s);
                    }
                    let dot = match watched {
                        UsbWatch::Up => UsbDot::Ready,
                        UsbWatch::Suspended => UsbDot::Suspended,
                        UsbWatch::Down => UsbDot::Powered,
                        UsbWatch::Drop => {
                            app.usb_dropped(b);
                            UsbDot::Powered
                        }
                    };
                    // a drive command read or wrote the card, which may have slept and taken over SysTick: without
                    // this, the sleep's end is lost and the main loop stalls while USB is still served (seen on the
                    // unit, 2026-10-24: the uptime stopped, the dev link kept answering)
                    woke |= l.worked();
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
                };
                if w.any() {
                    // a key from the link acts now, so the next command cannot replace it; board work done for the
                    // link (an eMMC read) may have slept, which takes over SysTick: this sleep's end is lost, so end it
                    break w;
                }
                b.sleep();
            };
            // a byte-ended slice counts 1 ms at least: SysTick counts whole ms, and a chip sending faster than a byte
            // a ms (BOARD.md 11: 4 us after a full FIFO is read) would keep every slice at 0 and the pass from ending
            self.policy.slept(if w.timer { slice } else { b.timer_elapsed_ms(slice).max(u32::from(w.bytes)) });
            b.timer_stop();
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
                radio_cycles = radio_cycles.wrapping_add(now.wrapping_sub(t));
                radio_passes += 1;
                if !app.radio_up() {
                    break; // the chip went down in it (a failed restart): nothing reads its bytes, as before
                }
                if radio_passes >= MAX_RADIO_PASSES {
                    break; // a chip that never stops sending: the step runs all the same, the pass is bounded
                }
                b.wake_on_io(true, wake.bytes);
            }
        }
        if wake.io {
            b.wake_on_io(false, false);
        }
        let sleep_cycles = b.cycles().wrapping_sub(sleep_start).wrapping_sub(radio_cycles);
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
        let pass_ms = self.policy.account(b.cycles().wrapping_sub(t0), sleep_cycles);
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
