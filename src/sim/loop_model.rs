//! The simulator's side of the main loop (lib app/main_loop.rs [`LoopBoard`]), so the tests run the unit's own pass:
//! SysTick as a slice timer on the simulated clock, WFI as one millisecond of it, the touch INT's and the keys'
//! falling edges latched as the EXTI flags are, the Nordic chip's bytes as they come, the cable; a scripted USB link
//! ([`SimLink`]) that logs what the pass asks of it; the reset, the stack fault and the staged image's run counted
//! instead of taken (on the unit they never return).

extern crate std;

use super::Sim;
use crate::dev::{Effect, Host, Session, Stats, UsbWatch};
use crate::usbsnap::UsbSnap;
use crate::{App, Board, LoopBoard, UsbLink};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::vec::Vec;

/// The loop's board state in the simulator.
#[derive(Default)]
pub struct LoopModel {
    /// The cable is in.
    pub vbus: bool,
    /// Every `wake_on_io` call (keys, bytes), in order.
    pub wakes: Vec<(bool, bool)>,
    /// What starting USB hands out (taken by the first start).
    pub link: Option<SimLink>,
    /// USB starts asked.
    pub usb_starts: u32,
    /// USB power-downs asked (a cable pulled while busy).
    pub usb_stops: u32,
    /// Resets, stack faults, the staged image's run (its size) asked.
    pub resets: u32,
    pub faults: u32,
    pub ran: Option<u32>,
    /// The Stop benches asked (ms, mode), in order.
    pub low_power: Vec<(u32, crate::stopbench::Mode)>,
    /// The idle Stops that slept (ms asked, us slept), in order.
    pub stops: Vec<(u32, u32)>,
    /// The idle Stop: not built (`POWER_STEPS` without bit 5: never tried), or failing (it sleeps, but the system
    /// does not reach Stop: no STOPF).
    pub stop_off: bool,
    pub stop_fails: bool,
    /// The ride's Stop not built (`POWER_STEPS` without bit 6: never tried on a ride).
    pub ride_stop_off: bool,
    /// The ride's Stops that slept (also in `stops`).
    pub ride_stops: u32,
    /// Stops not tried for the radio: a byte from the Nordic chip in or coming, or ours going out.
    pub radio_pending: u32,
    /// The SDRAM did not come back whole after a Stop (main.rs `image_marks` differ): the wake is broken.
    pub sdram_lost: bool,
    /// The idle Stop's wake fails to put a clock back (PLL1 not locked again): the loop must reset.
    pub stop_broken: bool,
    /// The cable comes in at this time, ms (an edge that ends a Stop, as PC13's EXTI line).
    pub vbus_at: Option<u64>,
    /// The wake after a Stop: the clocks and the SDRAM back, work the cycle counter counts, ns.
    pub wake_ns: u64,
    /// The slice timer: started at, ms.
    timer: Option<(u64, u32)>,
    /// WFI wakes at uneven sub-ms instants (a byte, a key arrive at any time), as on the unit; off: every WFI is 1 ms.
    pub jitter: bool,
    /// Each look at the touch line costs this much CPU work, ns (a USB poll's own cycles, which SysTick counts too).
    pub poll_work_ns: u64,
    lcg: u32,
    /// The touch INT and the keys at the last look (their edges latch in between, as the EXTI flags do).
    touch_was: bool,
    keys_was: u8,
    /// The time of that look at the keys, ms: a key that went down since (a falling edge) has its EXTI flag up until
    /// the next look, whatever it is now.
    keys_look_ms: u64,
    /// The stack was run past its canary.
    overflowed: bool,
    /// The Nordic chip always has a byte waiting (one that sends faster than the pass can read: review of B4b).
    pub bytes_always: bool,
    /// Bytes the chip sends at a time, ms: into its stream once the clock gets there.
    pub later: Vec<(u64, Vec<u8>)>,
}

/// What a [`SimLink`] answers and what it was asked, shared with the test (the link itself goes into the loop).
#[derive(Default)]
pub struct LinkScript {
    /// What the pass sent the computer.
    pub sent: Vec<u8>,
    /// What each poll returns, in turn; then [`Effect::None`].
    pub effects: VecDeque<Effect>,
    /// What each watch returns, in turn; then [`UsbWatch::Up`].
    pub watches: VecDeque<UsbWatch>,
    /// What the watch returns once `watches` is empty (default: Up).
    pub rest: Option<UsbWatch>,
    /// The failure registers, handed out once.
    pub snap: Option<UsbSnap>,
    /// A drive command ran (the next `worked` says so once).
    pub worked: bool,
    /// Each poll runs a drive command that sleeps this long, ms (a busy USB drive: `delay_ms` takes SysTick over).
    pub busy_ms: u32,
    /// The next poll takes this long, once, ms (a stall: a long job inside the pass).
    pub stall_ms: u32,
    /// The calls, in order.
    pub log: Vec<&'static str>,
}

/// A scripted USB link.
#[derive(Clone, Default)]
pub struct SimLink(pub Rc<RefCell<LinkScript>>);

impl Host for SimLink {
    fn send(&mut self, data: &[u8]) {
        self.0.borrow_mut().sent.extend_from_slice(data);
    }
    fn stage(&mut self, _: u32, _: &[u8]) {}
    fn peek(&mut self, _: u32) -> u32 {
        0
    }
    fn poke(&mut self, _: u32, _: u32) {}
}

impl UsbLink for SimLink {
    fn poll(&mut self, _: &mut Session, _: &mut App, b: &mut impl Board, _: Stats) -> Effect {
        let stall = core::mem::take(&mut self.0.borrow_mut().stall_ms);
        if stall > 0 {
            b.delay_ms(stall);
        }
        let busy = self.0.borrow().busy_ms;
        if busy > 0 {
            b.delay_ms(busy);
            self.0.borrow_mut().worked = true;
        }
        let mut s = self.0.borrow_mut();
        s.log.push("poll");
        s.effects.pop_front().unwrap_or(Effect::None)
    }
    fn linger(&mut self, _: u32) {
        self.0.borrow_mut().log.push("linger");
    }
    fn disconnect(&mut self) {
        self.0.borrow_mut().log.push("disconnect");
    }
    fn watch(&mut self, _: u32, _: bool) -> UsbWatch {
        let mut s = self.0.borrow_mut();
        s.log.push("watch");
        s.watches.pop_front().or(s.rest).unwrap_or(UsbWatch::Up)
    }
    fn snapshot(&mut self) -> Option<UsbSnap> {
        let mut s = self.0.borrow_mut();
        s.log.push("snapshot");
        s.snap.take()
    }
    fn recover(&mut self, _: u32) {
        self.0.borrow_mut().log.push("recover");
    }
    fn worked(&mut self) -> bool {
        core::mem::take(&mut self.0.borrow_mut().worked)
    }
}

impl Sim {
    /// Wall time passes by `ns` (work or waiting).
    fn advance_ns(&mut self, ns: u64) {
        self.cost_rem += ns;
        self.ms += self.cost_rem / 1_000_000;
        self.cost_rem %= 1_000_000;
    }

    /// The wall clock, ns: the whole ms and the sub-ms rest of the work's cost.
    fn wall_ns(&self) -> u64 {
        self.ms * 1_000_000 + self.cost_rem
    }
}

/// The RTC's PREDIV_S on the unit (BOARD.md 8): RTC_SSR counts down 1024 steps a second.
const RTC_PREDIV_S: u32 = 1023;

impl Sim {
    /// (RTC_TR, RTC_SSR) as main.rs `rtc_ss` reads them: the calendar's seconds and the sub-second count, both from
    /// the wall clock's 1/1024 s ticks (not the ms: the unit's quantisation, at both ends of a Stop).
    fn rtc_ss(&mut self) -> (u32, u32) {
        let tick = self.wall_ns() * u64::from(RTC_PREDIV_S + 1) / 1_000_000_000;
        let ssr = RTC_PREDIV_S - (tick % u64::from(RTC_PREDIV_S + 1)) as u32;
        (self.rtc().0, ssr)
    }
}

impl LoopBoard for Sim {
    type Link = SimLink;
    const RIDE_STOP_BUILT: bool = true; // a test turns it off at run time (`ride_stop_off`)
    const POWER_STEPS: u32 = 0x7f;
    fn canary(&mut self) -> u32 {
        if self.main.overflowed { 0 } else { crate::STACK_CANARY }
    }
    fn stack_overflowed(&mut self) {
        self.main.faults += 1;
    }
    fn checkpoint(&mut self, _: u32) {}
    fn vbus(&mut self) -> bool {
        self.main.vbus || self.main.vbus_at.is_some_and(|t| self.ms >= t)
    }
    fn usb_start(&mut self) -> Option<SimLink> {
        self.main.usb_starts += 1;
        self.main.link.take()
    }
    fn usb_stop(&mut self) {
        self.main.usb_stops += 1;
    }
    fn reset(&mut self) {
        self.main.resets += 1;
    }
    fn run_image(&mut self, size: u32) {
        self.main.ran = Some(size);
    }
    fn overflow_stack(&mut self) {
        self.main.overflowed = true;
    }
    /// The sleep passes on the clock and everything went right; the slice's timer is stopped, as main.rs's SysTick.
    fn low_power(&mut self, ms: u32, mode: crate::stopbench::Mode) -> crate::stopbench::StopReport {
        use crate::stopbench::{Mode, REFRESH_OK, STOP_OK, StopReport};
        self.main.low_power.push((ms, mode));
        self.main.timer = None;
        self.advance_ns(u64::from(ms) * 1_000_000);
        let stop = mode != Mode::Refresh;
        let flags = if stop { STOP_OK } else { REFRESH_OK };
        StopReport {
            stop,
            flags,
            slept_ms: ms,
            vos: 320,
            hse: 64_000,
            pll: 3200,
            sw: 64,
            fmc: 200,
            ..StopReport::default()
        }
    }
    /// A sleep on the wake-up timer's 2048 Hz grid for `ms` (stopbench `wutr`), cut at the first ms a key goes down,
    /// the touch INT falls or the cable comes in (each latched as its EXTI flag, left for `key_edge` / `touch_edge`),
    /// then the wake's work; timed as main.rs does, by two readings of the RTC's TR and SSR at its 1/1024 s. The card
    /// must be quiet (no transfer open, not programming): a Stop never begins inside card work.
    fn idle_stop(&mut self, ms: u32, ride: bool) -> (u32, u32) {
        use crate::stopbench::{PASS_OK, PENDING, PLL, STOPF, WUTF, elapsed_us, wutr};
        let edge = |s: &mut Sim| {
            let vbus_in = s.main.vbus_at.is_some_and(|t| s.ms >= t) && !s.main.vbus;
            (s.touch_int() && !s.main.touch_was) || s.keys_down() & !s.main.keys_was != 0 || vbus_in
        };
        // a key's EXTI flag latched since the last look, released since or not (main.rs: a flag up, no Stop)
        let was = self.main.keys_was;
        let latched = (self.main.keys_look_ms..self.ms).any(|t| (self.keys)(t) & !was != 0);
        if (if ride { self.main.ride_stop_off } else { self.main.stop_off }) || latched || edge(self) {
            return (0, PENDING);
        }
        // timed from here, as main.rs; RTS high, its 20 us wait (CPU work), then a byte in or landing, or ours still
        // going out: no Stop
        let before = self.rtc_ss();
        self.awake_ns += 20_000;
        self.advance_ns(20_000);
        let now = self.ms;
        if self.nrf_ext.is_none() && !self.nordic.hold(now) {
            self.main.radio_pending += 1;
            return (0, PENDING);
        }
        assert!(self.emmc.quiet(self.ms), "a Stop with card work in flight");
        self.main.ride_stops += u32::from(ride);
        let mut left = u64::from(wutr(ms) + 1) * 1_000_000_000 / 2048;
        let mut woke = false;
        while left > 0 {
            let step = left.min(1_000_000);
            if step == 1_000_000 && self.cost_rem == 0 {
                self.sleep_ms(1);
            } else {
                self.advance_ns(step);
            }
            left -= step;
            if edge(self) {
                woke = true;
                break;
            }
        }
        if self.main.vbus_at.is_some_and(|t| self.ms >= t) {
            self.main.vbus = true;
        }
        let ns = self.main.wake_ns;
        self.awake_ns += ns;
        self.advance_ns(ns);
        let now = self.ms;
        if self.nrf_ext.is_none() {
            self.nordic.release(now); // RTS back after the wake, as main.rs
        }
        let us = elapsed_us(before, self.rtc_ss(), RTC_PREDIV_S);
        self.main.stops.push((ms, us));
        let flags = if self.main.sdram_lost {
            PASS_OK & !crate::stopbench::REFRESH_OUT
        } else if self.main.stop_broken {
            PASS_OK & !PLL
        } else if self.main.stop_fails {
            PASS_OK & !STOPF
        } else {
            PASS_OK
        };
        (us, if woke { flags } else { flags | WUTF })
    }
    fn nrf_flush(&mut self) {
        self.nordic.out.clear();
        self.nordic.fifo.clear();
    }
    fn wake_on_io(&mut self, keys: bool, bytes: bool) {
        self.main.wakes.push((keys, bytes));
    }
    fn timer_start(&mut self, ms: u32) {
        self.main.timer = Some((self.wall_ns(), ms));
    }
    fn timer_done(&mut self) -> bool {
        self.main.timer.is_some_and(|(t, n)| self.wall_ns() >= t + u64::from(n) * 1_000_000)
    }
    fn timer_elapsed_us(&mut self, ms: u32) -> u32 {
        let wall = self.wall_ns();
        self.main.timer.map_or(0, |(t, _)| ((wall - t) / 1000).min(u64::from(ms) * 1000) as u32)
    }
    fn timer_stop(&mut self) {
        self.main.timer = None;
    }
    fn touch_edge(&mut self) -> bool {
        let ns = self.main.poll_work_ns;
        self.awake_ns += ns;
        self.advance_ns(ns);
        let int = self.touch_int();
        let edge = int && !self.main.touch_was;
        self.main.touch_was = int;
        edge
    }
    fn key_edge(&mut self) -> bool {
        let keys = self.keys_down();
        let edge = keys & !self.main.keys_was != 0;
        (self.main.keys_was, self.main.keys_look_ms) = (keys, self.ms);
        edge
    }
    fn byte_waiting(&mut self) -> bool {
        self.nordic_due();
        let now = self.ms;
        for (_, bytes) in self.main.later.extract_if(.., |(t, _)| *t <= now) {
            self.nordic.out.extend(bytes);
        }
        if self.main.bytes_always {
            self.advance_ns(300_000); // a byte every 300 us: the time SysTick counts between wakes
            return true;
        }
        self.nordic.has_bytes(now)
    }
    fn sleep(&mut self) {
        let mut step = 1_000_000;
        if self.main.jitter {
            self.main.lcg = self.main.lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            step = 150_000 + u64::from(self.main.lcg >> 8) % 850_000;
        }
        let wall = self.wall_ns();
        if let Some((t, n)) = self.main.timer {
            step = step.min((t + u64::from(n) * 1_000_000).saturating_sub(wall).max(1)); // SysTick ends the WFI on time
        }
        let total = self.cost_rem + step;
        self.cost_rem = total % 1_000_000;
        self.sleep_ms((total / 1_000_000) as u32);
    }
    fn take_io(&mut self) -> (u32, u32, u32) {
        (0, 0, 0)
    }
}
