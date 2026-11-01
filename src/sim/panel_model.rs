//! The ST7789V's timing and sleep rules, kept apart from the pixel model in `mod.rs`: a command that comes too soon
//! fails loudly with its reason, as the strict GNSS and sensor models do (a test that provokes one uses
//! `#[should_panic]`). The SPI transfer time is the cost model's (`Cost::panel_pixel_ns`, 16 bits at 50 MHz, the
//! same as [`crate::periph::panel_fill_us`]); window bounds (CASET / RASET end below start, outside the panel) are
//! checked by `span` in `mod.rs`.
//!
//! Sources: ST7789V datasheet (Sitronix, V1.4), command descriptions, from memory and **not re-read online** (no
//! network tool was used in this run; to be settled by reading them):
//! - P1 SWRESET: "wait 5 msec before sending new command"; "if Software Reset is applied during Sleep Out mode, it
//!   will be necessary to wait 120 msec before sending Sleep Out" (SWRESET, 0x01). Here the whole 120 ms, as stock
//!   waits (panel.rs INIT): stricter than the 5 ms, so a 50 ms gap fails (**inferred** as a rule for this firmware).
//! - P2 SLPOUT: "wait 5 msec before sending any new command", "wait 120 msec after Sleep Out before Sleep In can be
//!   sent" (SLPOUT, 0x11). SLPIN: 5 ms before a new command, 120 ms before Sleep Out (SLPIN, 0x10).
//! - P3 RAM writes while asleep or with the display off are accepted (the RAM is kept); counted
//!   ([`PanelRules::dark_writes`]) because `panel::init` relies on it (it clears before DISPON).

/// Settle times, ms. 120 for SWRESET is the firmware's rule (see P1); the others are the datasheet's.
const AFTER_SWRESET_MS: u64 = 120;
const AFTER_SLEEP_CMD_MS: u64 = 5;
const SLEEP_SWITCH_MS: u64 = 120;

/// The clock side of the panel: when it accepts the next command, and the sleep switches' ages.
#[derive(Clone, Copy, Debug, Default)]
pub struct PanelRules {
    /// No command before this time (ms).
    ready_ms: u64,
    /// When SLPOUT / SLPIN last came: the next sleep switch waits 120 ms after it.
    slept_ms: Option<u64>,
    /// RAM writes made while asleep or with the display off.
    pub dark_writes: u32,
}

impl PanelRules {
    /// A command at `now` ms (the cost of its own bytes already paid).
    pub fn command(&mut self, cmd: u8, now: u64) {
        assert!(now >= self.ready_ms, "command 0x{cmd:02x} {} ms inside the panel's settle time", self.ready_ms - now);
        if matches!(cmd, 0x10 | 0x11) {
            if let Some(t) = self.slept_ms {
                assert!(now >= t + SLEEP_SWITCH_MS, "sleep switch 0x{cmd:02x} {} ms after the last", now - t);
            }
            self.slept_ms = Some(now);
            self.ready_ms = now + AFTER_SLEEP_CMD_MS;
        }
        if cmd == 0x01 {
            self.slept_ms = None;
            self.ready_ms = now + AFTER_SWRESET_MS;
        }
    }

    /// A RAMWR at `now`; `dark` when the panel is asleep or its display is off.
    pub fn ram_write(&mut self, now: u64, dark: bool) {
        assert!(now >= self.ready_ms, "RAMWR inside the panel's settle time");
        self.dark_writes += u32::from(dark);
    }
}
