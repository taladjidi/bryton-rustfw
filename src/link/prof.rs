//! The on-device profiler: where the CPU's awake time goes, by named section of the main loop, read over the dev
//! link (`prof`, dev.rs; `tools/dev.py prof`). Always on: a section costs one read of the DWT cycle counter
//! ([`crate::Board::cycles`]) and a few adds (the main loop measures it at start-up: `overhead` in `prof`), about
//! 25 sections a pass and 20 passes a second: about 0.01 % of the CPU at a few tens of cycles each (inferred).
//!
//! Each section adds its cycles to a [`Slot`]: calls, total (u64: no overflow over any window) and the longest. A
//! duration is `now.wrapping_sub(then)` of the 32-bit counter, right across its wrap (2^32 cycles, 21 s of work at
//! 200 MHz) as long as one section is shorter than that. The counter stops in WFI (main.rs), so a section counts its
//! work, never the sleep; the window's wall time and sleep come from the main loop (SysTick), and the panel's pixels
//! and the eMMC's sectors from the board's transport. `prof reset` starts a new window.

/// A named part of the main loop. [`Sec::Step`] holds every section of `App::step`; [`Sec::Tiles`] is inside
/// whatever drew the map (draw, or input when a key opened the page): those two are inclusive, the others are not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Sec {
    /// The whole of `App::step` (inclusive).
    Step,
    /// The RTC read, the ride's seconds, settings, alerts, tones.
    Second,
    /// The battery's ADC reading and the light sensor's.
    Battery,
    /// The backlight's level.
    Light,
    /// The Nordic chip: its bytes, the link's watchdog, ANT pairing and channels opened.
    Radio,
    /// The GNSS receiver's SPI and UBX.
    Gnss,
    /// The status strip.
    Strip,
    /// The scan of stock's rides.
    Scan,
    /// Courses, past rides, thresholds, `pages.bin` (card work).
    Files,
    /// The climbs and the map tiles of the road ahead.
    Ahead,
    /// The banner and the screen's update (`update_page`, menus, lists).
    Draw,
    /// Basemap tiles read and drawn into their buffer (inclusive: part of whatever drew).
    Tiles,
    /// The keys read and acted on.
    Keys,
    /// The ride recorder (eMMC writes at checkpoints).
    Recorder,
    /// The barometer.
    Altitude,
    /// The home trainer and the power meter's calibration.
    Trainer,
    /// The phone link and the media screen.
    Phone,
    /// The touch panel read (main loop).
    Touch,
    /// A key or gesture acted on (main loop: `App::press`, `App::input`).
    Input,
    /// The USB link served during the sleep (only while a cable is in).
    Usb,
}

/// How many sections.
pub const SECTIONS: usize = Sec::Usb as usize + 1;

impl Sec {
    /// Every section, in order.
    pub const ALL: [Sec; SECTIONS] = [
        Sec::Step,
        Sec::Second,
        Sec::Battery,
        Sec::Light,
        Sec::Radio,
        Sec::Gnss,
        Sec::Strip,
        Sec::Scan,
        Sec::Files,
        Sec::Ahead,
        Sec::Draw,
        Sec::Tiles,
        Sec::Keys,
        Sec::Recorder,
        Sec::Altitude,
        Sec::Trainer,
        Sec::Phone,
        Sec::Touch,
        Sec::Input,
        Sec::Usb,
    ];

    /// Its name in `prof`'s answer (tools/dev.py prints it).
    pub fn name(self) -> &'static str {
        match self {
            Sec::Step => "step",
            Sec::Second => "second",
            Sec::Battery => "battery",
            Sec::Light => "light",
            Sec::Radio => "radio",
            Sec::Gnss => "gnss",
            Sec::Strip => "strip",
            Sec::Scan => "scan",
            Sec::Files => "files",
            Sec::Ahead => "ahead",
            Sec::Draw => "draw",
            Sec::Tiles => "tiles",
            Sec::Keys => "keys",
            Sec::Recorder => "recorder",
            Sec::Altitude => "altitude",
            Sec::Trainer => "trainer",
            Sec::Phone => "phone",
            Sec::Touch => "touch",
            Sec::Input => "input",
            Sec::Usb => "usb",
        }
    }
}

/// One section's figures over the window, in CPU cycles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    /// Times it ran.
    pub count: u32,
    /// Cycles, all runs.
    pub total: u64,
    /// The longest run.
    pub max: u32,
}

/// The profiler: a slot per section and the window's wall time and I/O.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prof {
    /// A slot per [`Sec`].
    pub slots: [Slot; SECTIONS],
    /// Wall time of the window, ms (the main loop's passes: work and sleep).
    pub wall_ms: u32,
    /// Of it, asleep between passes, ms.
    pub slept_ms: u32,
    /// Pixels sent to the panel.
    pub pixels: u64,
    /// eMMC sectors read.
    pub read: u32,
    /// eMMC sectors written.
    pub written: u32,
    /// What one section costs, cycles (two counter reads and [`Prof::add`], measured at start-up; kept by a reset).
    pub overhead: u32,
}

impl Prof {
    /// Adds a run of `cycles` to `s`.
    pub fn add(&mut self, s: Sec, cycles: u32) {
        if let Some(slot) = self.slots.get_mut(s as usize) {
            slot.count = slot.count.saturating_add(1);
            slot.total = slot.total.saturating_add(u64::from(cycles));
            slot.max = slot.max.max(cycles);
        }
    }

    /// Adds the run from cycle counter value `from` to `to` (across a wrap of the counter).
    pub fn span(&mut self, s: Sec, from: u32, to: u32) {
        self.add(s, to.wrapping_sub(from));
    }

    /// A main loop pass of `ms`, `slept` of it asleep.
    pub fn pass(&mut self, ms: u32, slept: u32) {
        self.wall_ms = self.wall_ms.saturating_add(ms);
        self.slept_ms = self.slept_ms.saturating_add(slept);
    }

    /// The board's transport since the last call: pixels to the panel, sectors read and written.
    pub fn io(&mut self, (pixels, read, written): (u32, u32, u32)) {
        self.pixels = self.pixels.saturating_add(u64::from(pixels));
        self.read = self.read.saturating_add(read);
        self.written = self.written.saturating_add(written);
    }

    /// A new window (the overhead stays).
    pub fn reset(&mut self) {
        *self = Prof { overhead: self.overhead, ..Prof::default() };
    }
}

impl crate::App {
    /// The profiler, for the main loop (its own sections, the pass, the board's I/O) and the dev link.
    pub fn prof(&mut self) -> &mut Prof {
        &mut self.prof
    }

    /// The profiler's figures, read only (the dev link's `prof`).
    pub fn prof_figures(&self) -> &Prof {
        &self.prof
    }

    /// Ends section `s`, begun at counter value `from`, and returns the counter now: where the next one begins.
    pub(crate) fn timed(&mut self, b: &mut impl crate::Board, s: Sec, from: u32) -> u32 {
        let now = b.cycles();
        self.prof.span(s, from, now);
        now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_count_total_and_max() {
        let mut p = Prof::default();
        p.add(Sec::Draw, 100);
        p.add(Sec::Draw, 300);
        p.add(Sec::Gnss, 7);
        let d = p.slots[Sec::Draw as usize];
        assert_eq!((d.count, d.total, d.max), (2, 400, 300));
        assert_eq!(p.slots[Sec::Gnss as usize].total, 7);
        assert_eq!(p.slots[Sec::Radio as usize], Slot::default());
    }

    #[test]
    fn a_span_across_the_counter_wrap() {
        // 2^32 cycles is 21.5 s of work at 200 MHz: a span that starts before the wrap and ends after it
        let mut p = Prof::default();
        p.span(Sec::Usb, u32::MAX - 49, 50);
        assert_eq!(p.slots[Sec::Usb as usize].total, 100);
        p.span(Sec::Usb, 10, 10);
        assert_eq!(p.slots[Sec::Usb as usize].count, 2);
    }

    #[test]
    fn totals_pass_u32_over_a_long_window() {
        let mut p = Prof::default();
        for _ in 0..3 {
            p.add(Sec::Step, u32::MAX);
        }
        assert_eq!(p.slots[Sec::Step as usize].total, 3 * u64::from(u32::MAX));
    }

    #[test]
    fn reset_starts_a_window_and_keeps_the_overhead() {
        let mut p = Prof { overhead: 12, ..Prof::default() };
        p.add(Sec::Keys, 5);
        p.pass(50, 48);
        p.io((240, 1, 2));
        p.reset();
        assert_eq!(p, Prof { overhead: 12, ..Prof::default() });
    }

    #[test]
    fn names_are_in_order_and_distinct() {
        for (i, s) in Sec::ALL.iter().enumerate() {
            assert_eq!(*s as usize, i);
            assert!(Sec::ALL[..i].iter().all(|o| o.name() != s.name()));
        }
    }
}
