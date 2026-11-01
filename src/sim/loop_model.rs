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
    /// What starting USB hands out (taken by the first start).
    pub link: Option<SimLink>,
    /// USB starts asked.
    pub usb_starts: u32,
    /// Resets, stack faults, the staged image's run (its size) asked.
    pub resets: u32,
    pub faults: u32,
    pub ran: Option<u32>,
    /// The slice timer: started at, ms.
    timer: Option<(u64, u32)>,
    /// The touch INT and the keys at the last look (their edges latch in between, as the EXTI flags do).
    touch_was: bool,
    keys_was: u8,
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
    /// What each poll returns, in turn; then [`Effect::None`].
    pub effects: VecDeque<Effect>,
    /// What each watch returns, in turn; then [`UsbWatch::Up`].
    pub watches: VecDeque<UsbWatch>,
    /// The failure registers, handed out once.
    pub snap: Option<UsbSnap>,
    /// A drive command ran (the next `worked` says so once).
    pub worked: bool,
    /// The calls, in order.
    pub log: Vec<&'static str>,
}

/// A scripted USB link.
#[derive(Clone, Default)]
pub struct SimLink(pub Rc<RefCell<LinkScript>>);

impl Host for SimLink {
    fn send(&mut self, _: &[u8]) {}
    fn stage(&mut self, _: u32, _: &[u8]) {}
    fn peek(&mut self, _: u32) -> u32 {
        0
    }
    fn poke(&mut self, _: u32, _: u32) {}
}

impl UsbLink for SimLink {
    fn poll(&mut self, _: &mut Session, _: &mut App, _: &mut impl Board, _: Stats) -> Effect {
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
        s.watches.pop_front().unwrap_or(UsbWatch::Up)
    }
    fn snapshot(&mut self) -> Option<UsbSnap> {
        let mut s = self.0.borrow_mut();
        s.log.push("snapshot");
        s.snap.take()
    }
    fn worked(&mut self) -> bool {
        core::mem::take(&mut self.0.borrow_mut().worked)
    }
}

impl LoopBoard for Sim {
    type Link = SimLink;
    fn canary(&mut self) -> u32 {
        if self.main.overflowed { 0 } else { crate::STACK_CANARY }
    }
    fn stack_overflowed(&mut self) {
        self.main.faults += 1;
    }
    fn checkpoint(&mut self, _: u32) {}
    fn vbus(&mut self) -> bool {
        self.main.vbus
    }
    fn usb_start(&mut self) -> Option<SimLink> {
        self.main.usb_starts += 1;
        self.main.link.take()
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
    fn nrf_flush(&mut self) {
        self.nordic.out.clear();
        self.nordic.fifo.clear();
    }
    fn wake_on_io(&mut self, _: bool, _: bool) {}
    fn timer_start(&mut self, ms: u32) {
        self.main.timer = Some((self.ms, ms));
    }
    fn timer_done(&mut self) -> bool {
        self.main.timer.is_some_and(|(t, n)| self.ms >= t + u64::from(n))
    }
    fn timer_elapsed_ms(&mut self, ms: u32) -> u32 {
        self.main.timer.map_or(0, |(t, _)| (self.ms - t).min(u64::from(ms)) as u32)
    }
    fn timer_stop(&mut self) {
        self.main.timer = None;
    }
    fn touch_edge(&mut self) -> bool {
        let int = self.touch_int();
        let edge = int && !self.main.touch_was;
        self.main.touch_was = int;
        edge
    }
    fn key_edge(&mut self) -> bool {
        let keys = self.keys_down();
        let edge = keys & !self.main.keys_was != 0;
        self.main.keys_was = keys;
        edge
    }
    fn byte_waiting(&mut self) -> bool {
        self.nordic_due();
        let now = self.ms;
        for (_, bytes) in self.main.later.extract_if(.., |(t, _)| *t <= now) {
            self.nordic.out.extend(bytes);
        }
        self.main.bytes_always || self.nordic.has_bytes(now)
    }
    fn sleep(&mut self) {
        self.sleep_ms(1);
    }
    fn take_io(&mut self) -> (u32, u32, u32) {
        (0, 0, 0)
    }
}
