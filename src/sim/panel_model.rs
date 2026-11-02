//! The ST7789V's timing and sleep rules, kept apart from the pixel model in `mod.rs`: a command that comes too soon
//! fails loudly with its reason, as the strict GNSS and sensor models do (a test that provokes one uses
//! `#[should_panic]`). The SPI transfer time is the cost model's (`Cost::panel_pixel_ns`, 16 bits at 50 MHz, the
//! same as [`crate::periph::panel_fill_us`]); window bounds (CASET / RASET end below start, outside the panel) are
//! checked by `span` in `mod.rs`.
//!
//! Sources: Sitronix ST7789V datasheet V1.3 (2014/03; https://newhavendisplay.com/content/datasheets/ST7789V.pdf) and
//! ST7789VW V1.0 (2017/09; https://www.waveshare.com/w/upload/a/ae/ST7789_Datasheet.pdf), checked 2026-11-02; the
//! command texts are identical in both (the repo's old "V1.4" is neither). Sections of V1.3:
//! - P1 SWRESET (9.1.2): "wait 5 msec before sending new command"; sent in Sleep In, "wait 120 msec before sending
//!   sleep out command"; it "cannot be sent during sleep out sequence" (the 120 ms after SLPOUT). Modelled as that:
//!   5 ms before any command, 120 ms before SLPOUT (every reset leaves Sleep In), none inside the sleep-out window.
//!   The firmware is stricter on purpose (panel.rs INIT waits 120 ms after SWRESET, as stock does).
//! - P2 SLPOUT / SLPIN (9.1.11, 9.1.12): "wait 5 msec before sending any new command"; "wait 120 msec after sending
//!   sleep out command before sending a sleep in command". The doc has no 120 ms rule for SLPIN then SLPOUT (searched,
//!   not found), so none is modelled.
//! - P3 (9.1.20 to 9.1.22, register availability) RAMWR / CASET / RASET work in Sleep In and the RAM keeps its
//!   contents, so writes while asleep or with the display off are accepted; counted ([`PanelRules::dark_writes`])
//!   because `panel::init` relies on it (it clears before DISPON). DISPON states no wait (9.1.19).
//! - RAMCTRL (9.2.1): default 00h/F0h after power-on and any reset; DM[1:0] 00 is the MCU interface, RM (bit 4) 1 sends
//!   RAM access to the RGB port, so RAMWR over SPI would not land (`mod.rs` fails it).
//! - Serial write cycle (TSCYCW): V1.3 (ST7789V) 66 ns min (15 MHz), VW V1.0 16 ns min (62.5 MHz). The unit runs
//!   SPI1 at 50 MHz (20 ns, BOARD.md 6): within the VW figure, outside the V one, so the controller on the unit must
//!   be the newer die (it works). Not modelled as a check.
//! - Not checked: HW reset pulse >= 10 us and 5 ms (120 ms if it hit Sleep Out) after RESX; CS-high width 40 ns.

/// Settle times, ms (datasheet values).
const AFTER_SWRESET_MS: u64 = 5;
const AFTER_SLEEP_CMD_MS: u64 = 5;
/// SLPOUT to SLPIN or SWRESET, and SWRESET to SLPOUT.
const SLEEP_OUT_MS: u64 = 120;

/// The clock side of the panel: when it accepts the next command, and the sleep switches' ages.
#[derive(Clone, Copy, Debug, Default)]
pub struct PanelRules {
    /// No command before this time (ms).
    ready_ms: u64,
    /// When SLPOUT came (None after SLPIN or a reset): SLPIN and SWRESET wait 120 ms after it.
    out_ms: Option<u64>,
    /// No SLPOUT before this time (ms): 120 ms after a SWRESET.
    out_ok_ms: u64,
    /// RAM writes made while asleep or with the display off.
    pub dark_writes: u32,
}

impl PanelRules {
    /// A command at `now` ms (the cost of its own bytes already paid).
    pub fn command(&mut self, cmd: u8, now: u64) {
        assert!(now >= self.ready_ms, "command 0x{cmd:02x} {} ms inside the panel's settle time", self.ready_ms - now);
        if matches!(cmd, 0x01 | 0x10) {
            if let Some(t) = self.out_ms {
                assert!(now >= t + SLEEP_OUT_MS, "sleep switch 0x{cmd:02x} {} ms after SLPOUT", now - t);
            }
            self.out_ms = None;
        }
        match cmd {
            0x10 => self.ready_ms = now + AFTER_SLEEP_CMD_MS,
            0x11 => {
                assert!(now >= self.out_ok_ms, "SLPOUT {} ms after SWRESET", self.out_ok_ms - now);
                self.out_ms = Some(now);
                self.ready_ms = now + AFTER_SLEEP_CMD_MS;
            }
            0x01 => {
                self.ready_ms = now + AFTER_SWRESET_MS;
                self.out_ok_ms = now + SLEEP_OUT_MS;
            }
            _ => {}
        }
    }

    /// A RAMWR at `now`; `dark` when the panel is asleep or its display is off.
    pub fn ram_write(&mut self, now: u64, dark: bool) {
        assert!(now >= self.ready_ms, "RAMWR inside the panel's settle time");
        self.dark_writes += u32::from(dark);
    }
}
