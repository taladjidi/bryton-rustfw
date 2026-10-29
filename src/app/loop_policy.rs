//! The main loop's policy (main.rs `main`): when USB starts and when an unplug resets the unit, which wake sources
//! an idle sleep arms, how long each sleep slice is, what ends the pass's sleep, and how a pass is counted in the
//! awake and CPU shares that the dev link's `info` reports. main.rs keeps the primitives (the cycle counter, SysTick,
//! the NVIC and EXTI lines, WFI, the reset) and asks [`LoopPolicy`] what to do with what they measured, so the
//! rules are tested on the host (docs/review-2026-10-29.md A8).

use crate::dev::Stats;
use crate::{POLL_MS, TOUCH_MS};

/// What main.rs does with the USB cable this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cable {
    /// The cable just came in: start the USB link (once per boot).
    Start,
    /// The cable went, the link was started and no ride would be lost: reset. The simplest way to power the PHY and
    /// the USB core down again; never during a ride or on its card (a loose cable or a power bank unplugged must not
    /// lose it): after it.
    Reset,
    /// Nothing to do.
    Keep,
}

/// The wake sources of this pass's sleep, besides SysTick, the touch INT and the USB link (always on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wake {
    /// An idle sleep (the pass longer than [`POLL_MS`]): the key lines end it at once, and the Nordic chip's bytes
    /// if `radio` (main.rs `wake_on_io` around the sleep, `io_edge` in it). Never in a short one: a held key's flag
    /// or a waiting byte would end every WFI.
    pub io: bool,
    /// The app reads the Nordic chip's bytes (`App::radio_up`): they may wake an idle sleep. Otherwise main.rs
    /// flushes them, so a byte nobody reads neither ends every WFI nor waits stale for the next start.
    pub radio: bool,
}

/// What ended one wait of a slice (main.rs checks them after each WFI).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Woke {
    /// The touch INT fell.
    pub touch: bool,
    /// The slice's SysTick period ran out.
    pub timer: bool,
    /// A key or a byte from the Nordic chip, in an idle sleep ([`Wake::io`]).
    pub io: bool,
    /// A key pressed over the dev link: it acts now, so the next command cannot replace it.
    pub key: bool,
    /// Board work done for the USB link (an eMMC read) may have slept, which takes over SysTick: this sleep's end is
    /// lost, so end it (seen on the unit, 2026-10-24: the uptime stopped, the dev link kept answering).
    pub usb: bool,
}

impl Woke {
    /// The wait ends: anything fired.
    pub fn any(&self) -> bool {
        self.touch || self.timer || self.io || self.key || self.usb
    }

    /// Read the panel after this slice: its INT fell or a finger is down (`App::touching`). Before the early end of
    /// [`Woke::ends_pass`]: with the USB drive busy, every slice ends early, and touch was never read (seen on the
    /// unit, 2026-10-24: no gesture during a copy to the drive).
    pub fn read_touch(&self, touching: bool) -> bool {
        self.touch || touching
    }

    /// The pass's sleep ends after this slice (a gesture ends it too, once read): the input acts after the sleep,
    /// since acting may sleep (delay_ms), which takes over SysTick.
    pub fn ends_pass(&self) -> bool {
        self.io || self.key || self.usb
    }
}

/// The main loop's state between passes and its rules; see the module doc.
#[derive(Clone, Copy, Debug)]
pub struct LoopPolicy {
    /// The cycle counter's rate (main.rs `CPU_HZ`).
    cpu_hz: u32,
    /// A SysTick period's longest, ms (main.rs `TIMER_MAX_MS`).
    timer_max_ms: u32,
    /// The USB link was started (the cable came in once since boot).
    usb_tried: bool,
    /// Slept so far in this pass, ms, by SysTick: the cycle counter stops in WFI.
    slept: u32,
    /// Slept since the panel was last read, ms.
    since_touch: u32,
    /// Awake cycles in the current window, all cycles run (the sleep loop's too), and the window's length, ms.
    awake: u32,
    ran: u32,
    window_ms: u32,
    /// What `info` reports.
    stats: Stats,
}

impl LoopPolicy {
    /// A policy for a counter at `cpu_hz` and SysTick periods of at most `timer_max_ms`.
    pub const fn new(cpu_hz: u32, timer_max_ms: u32) -> Self {
        Self {
            cpu_hz,
            timer_max_ms,
            usb_tried: false,
            slept: 0,
            since_touch: 0,
            awake: 0,
            ran: 0,
            window_ms: 0,
            stats: Stats { uptime_ms: 0, awake_permille: 0, cpu_permille: 0 },
        }
    }

    /// The cable's rule, after the step: `vbus` is the cable's line, `busy` is `App::busy` (a ride or its card).
    ///
    /// ```
    /// use bryton_rustfw::{Cable, LoopPolicy};
    /// let mut p = LoopPolicy::new(200_000_000, 80);
    /// assert_eq!(p.cable(true, false), Cable::Start);
    /// assert_eq!(p.cable(false, true), Cable::Keep); // a ride: after it
    /// assert_eq!(p.cable(false, false), Cable::Reset);
    /// ```
    pub fn cable(&mut self, vbus: bool, busy: bool) -> Cable {
        if vbus && !self.usb_tried {
            self.usb_tried = true;
            Cable::Start
        } else if !vbus && self.usb_tried && !busy {
            Cable::Reset
        } else {
            Cable::Keep
        }
    }

    /// Starts the pass's sleep: `pass_ms` is `App::pass_ms`, `radio_up` is `App::radio_up`. Idle (a longer sleep,
    /// ended at once by a key or a byte from the Nordic chip, so neither waits for it; the chip's own buffers are not
    /// known: it is read as soon as it sends, as before) when the pass is longer than [`POLL_MS`].
    pub fn begin_sleep(&mut self, pass_ms: u32, radio_up: bool) -> Wake {
        self.slept = 0;
        Wake { io: pass_ms > POLL_MS, radio: radio_up }
    }

    /// The next slice of the pass's sleep, ms, or `None` when the pass has slept `pass_ms` (read again each slice: a
    /// finger down makes it [`POLL_MS`] again). [`TOUCH_MS`] slices while a finger is on the panel, so it is read
    /// every `TOUCH_MS` (stock: an EXTI6 wake, then reads while the finger is down); else SysTick's longest.
    pub fn slice(&self, pass_ms: u32, touching: bool) -> Option<u32> {
        let left = pass_ms.saturating_sub(self.slept);
        if left == 0 {
            return None;
        }
        Some(left.min(if touching { TOUCH_MS } else { self.timer_max_ms }))
    }

    /// A slice slept `ms` (the whole slice when its timer ran out, else what SysTick counted).
    pub fn slept(&mut self, ms: u32) {
        self.slept += ms;
        self.since_touch = self.since_touch.saturating_add(ms);
    }

    /// Slept since the panel was last read, ms: it is read now.
    pub fn take_touch_ms(&mut self) -> u32 {
        core::mem::take(&mut self.since_touch)
    }

    /// Slept so far in this pass, ms.
    pub fn slept_ms(&self) -> u32 {
        self.slept
    }

    /// The uptime with this pass's sleep so far, ms (the USB link's watch).
    pub fn now_ms(&self) -> u32 {
        self.stats.uptime_ms.wrapping_add(self.slept)
    }

    /// The running figures for the dev link's `info`.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Counts the pass that ends: `pass_cycles` from the step's start to now, `sleep_cycles` the sleep loop's (USB
    /// polls, and anything that keeps WFI from sleeping). Returns the pass's length, ms: its work plus its sleep (a
    /// long one, an eMMC scan, is longer than [`POLL_MS`]). Each window of at least a second sets the awake share
    /// (the app's work: step and keys, not the USB polls, which happen only while a cable is in) and the CPU share
    /// (all cycles run: CYCCNT stops in WFI, so the sleep's cycles are the time the CPU ran while it should have
    /// slept), per mille of the window.
    pub fn account(&mut self, pass_cycles: u32, sleep_cycles: u32) -> u32 {
        let per_ms = self.cpu_hz / 1000;
        let worked = pass_cycles.wrapping_sub(sleep_cycles);
        self.awake = self.awake.saturating_add(worked);
        self.ran = self.ran.saturating_add(worked).saturating_add(sleep_cycles);
        let pass_ms = worked / per_ms + self.slept;
        self.window_ms = self.window_ms.wrapping_add(pass_ms);
        if self.window_ms >= 1000 {
            let w = u64::from(self.window_ms);
            self.stats.awake_permille = (u64::from(self.awake / per_ms) * 1000 / w) as u32;
            self.stats.cpu_permille = (u64::from(self.ran / per_ms) * 1000 / w) as u32;
            (self.awake, self.ran, self.window_ms) = (0, 0, 0);
        }
        self.stats.uptime_ms = self.stats.uptime_ms.wrapping_add(pass_ms);
        pass_ms
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    const HZ: u32 = 200_000_000;
    const MAX: u32 = 80;
    const IDLE: u32 = crate::IDLE_POLL_MS;

    /// The slices of one pass, each slept whole.
    fn slices(p: &mut LoopPolicy, pass_ms: u32, touching: bool) -> std::vec::Vec<u32> {
        let mut v = std::vec::Vec::new();
        while let Some(s) = p.slice(pass_ms, touching) {
            p.slept(s);
            v.push(s);
        }
        v
    }

    #[test]
    fn idle_pass_sleeps_250_ms_in_timer_slices() {
        let mut p = LoopPolicy::new(HZ, MAX);
        assert_eq!(IDLE, 250);
        p.begin_sleep(IDLE, false);
        assert_eq!(slices(&mut p, IDLE, false), [80, 80, 80, 10]);
        assert_eq!(p.slept_ms(), 250);
    }

    #[test]
    fn short_pass_sleeps_poll_ms_in_one_slice() {
        // a ride, a key lately, a tone, the drive lent: App::pass_ms is POLL_MS, whether the cable is in or not
        let mut p = LoopPolicy::new(HZ, MAX);
        p.begin_sleep(POLL_MS, true);
        assert_eq!(slices(&mut p, POLL_MS, false), [50]);
    }

    #[test]
    fn touching_sleeps_touch_ms_slices() {
        let mut p = LoopPolicy::new(HZ, MAX);
        p.begin_sleep(POLL_MS, false);
        assert_eq!(slices(&mut p, POLL_MS, true), [TOUCH_MS; 5]);
    }

    #[test]
    fn a_finger_mid_pass_shortens_the_rest() {
        // an idle pass, then a finger down: pass_ms falls to POLL_MS, of which 80 ms were already slept
        let mut p = LoopPolicy::new(HZ, MAX);
        p.begin_sleep(IDLE, false);
        assert_eq!(p.slice(IDLE, false), Some(80));
        p.slept(80);
        assert_eq!(p.slice(POLL_MS, true), None);
    }

    #[test]
    fn a_slice_ended_early_leaves_the_rest() {
        let mut p = LoopPolicy::new(HZ, MAX);
        p.begin_sleep(POLL_MS, false);
        p.slept(13); // a USB wake after 13 ms
        assert_eq!(p.slice(POLL_MS, false), Some(37));
        p.begin_sleep(POLL_MS, false); // a new pass starts from nothing slept
        assert_eq!(p.slice(POLL_MS, false), Some(50));
    }

    #[test]
    fn idle_arms_the_keys_and_the_radio_only_when_read() {
        let mut p = LoopPolicy::new(HZ, MAX);
        assert_eq!(p.begin_sleep(IDLE, true), Wake { io: true, radio: true });
        assert_eq!(p.begin_sleep(IDLE, false), Wake { io: true, radio: false });
        assert_eq!(p.begin_sleep(POLL_MS, true), Wake { io: false, radio: true });
        assert_eq!(p.begin_sleep(TOUCH_MS, false), Wake { io: false, radio: false });
    }

    #[test]
    fn wake_reasons() {
        let none = Woke::default();
        assert!(!none.any() && !none.ends_pass() && !none.read_touch(false));
        assert!(none.read_touch(true)); // a finger down: read every slice
        let timer = Woke { timer: true, ..none };
        assert!(timer.any() && !timer.ends_pass() && !timer.read_touch(false));
        let touch = Woke { touch: true, ..none };
        assert!(touch.any() && touch.read_touch(false) && !touch.ends_pass());
        for w in [Woke { io: true, ..none }, Woke { key: true, ..none }, Woke { usb: true, ..none }] {
            assert!(w.any() && w.ends_pass() && !w.read_touch(false));
        }
    }

    #[test]
    fn touch_time_runs_across_passes_until_read() {
        let mut p = LoopPolicy::new(HZ, MAX);
        p.begin_sleep(POLL_MS, false);
        p.slept(50);
        p.begin_sleep(POLL_MS, false);
        p.slept(10);
        assert_eq!(p.take_touch_ms(), 60);
        assert_eq!(p.take_touch_ms(), 0);
    }

    #[test]
    fn cable_starts_once_and_unplug_resets_unless_busy() {
        let mut p = LoopPolicy::new(HZ, MAX);
        assert_eq!(p.cable(false, false), Cable::Keep); // never plugged: nothing to power down
        assert_eq!(p.cable(true, false), Cable::Start);
        assert_eq!(p.cable(true, false), Cable::Keep); // once per boot, even if the start failed
        assert_eq!(p.cable(false, true), Cable::Keep); // a ride or its card: after it
        assert_eq!(p.cable(false, false), Cable::Reset);
        let mut busy = LoopPolicy::new(HZ, MAX);
        assert_eq!(busy.cable(true, true), Cable::Start); // plugged in during a ride: USB starts all the same
    }

    #[test]
    fn permille_over_a_window() {
        let mut p = LoopPolicy::new(HZ, MAX);
        let ms = HZ / 1000;
        // 20 passes of 1 ms work, 49 ms slept, 0.5 ms of USB polls in the sleep: 20 * 50 = 1000 ms
        for i in 0..20 {
            p.begin_sleep(POLL_MS, false);
            p.slept(49);
            let pass = p.account(ms + ms / 2, ms / 2);
            assert_eq!(pass, 50);
            if i < 19 {
                assert_eq!(p.stats().awake_permille, 0); // not a second yet
            }
        }
        assert_eq!(p.stats(), Stats { uptime_ms: 1000, awake_permille: 20, cpu_permille: 30 });
        // the window starts again: a long pass (an eMMC scan, 1.2 s awake) is a window by itself
        p.begin_sleep(POLL_MS, false);
        let pass = p.account(1200 * ms, 0);
        assert_eq!(pass, 1200);
        assert_eq!(p.stats(), Stats { uptime_ms: 2200, awake_permille: 1000, cpu_permille: 1000 });
    }

    #[test]
    fn uptime_includes_the_sleep_so_far() {
        let mut p = LoopPolicy::new(HZ, MAX);
        p.begin_sleep(POLL_MS, false);
        p.slept(30);
        assert_eq!(p.now_ms(), 30);
        p.slept(20);
        assert_eq!(p.account(HZ / 1000 * 3, 0), 53);
        assert_eq!(p.now_ms(), 53 + 50); // slept is cleared only by the next pass's begin_sleep
        p.begin_sleep(POLL_MS, false);
        assert_eq!(p.now_ms(), 53);
    }
}
