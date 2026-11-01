//! The panel's timing rules (`panel_model.rs`): each one provoked, then the firmware's own power-up and power-down
//! against them, and the simulated SPI time against the pure fill time the DMA time-out uses.

use super::*;
use crate::{Board, panel, periph};

fn powered() -> Sim {
    let mut s = Sim::new(8, 0, 0);
    s.panel_power_on();
    s
}

#[test]
#[should_panic(expected = "settle time")]
fn a_command_50_ms_after_swreset_fails() {
    let mut s = powered();
    s.panel_cmd(panel::SWRESET, &[]);
    s.delay_ms(50);
    s.panel_cmd(0x13, &[]);
}

#[test]
fn a_command_120_ms_after_swreset_passes() {
    let mut s = powered();
    s.panel_cmd(panel::SWRESET, &[]);
    s.delay_ms(120);
    s.panel_cmd(0x13, &[]);
}

#[test]
#[should_panic(expected = "sleep switch")]
fn sleep_in_within_120_ms_of_sleep_out_fails() {
    let mut s = powered();
    s.panel_cmd(panel::SLPOUT, &[]);
    s.delay_ms(10);
    s.panel_cmd(panel::SLPIN, &[]);
}

#[test]
#[should_panic(expected = "settle time")]
fn a_command_right_after_sleep_out_fails() {
    let mut s = powered();
    s.panel_cmd(panel::SLPOUT, &[]);
    s.panel_cmd(0x13, &[]);
}

#[test]
fn init_then_off_follows_the_rules_and_clears_before_dispon() {
    let mut s = powered();
    panel::init(&mut s);
    assert!(s.panel.dark_writes >= 1, "init clears the RAM before DISPON, relying on writes while dark");
    let n = s.panel.dark_writes;
    s.delay_ms(200);
    panel::off(&mut s);
    assert_eq!(s.panel.dark_writes, n);
}

#[test]
fn sim_spi_time_is_the_pure_fill_time() {
    let px = u64::from(periph::PANEL_MAX_PIXELS);
    let sim_us = px * Cost::default().panel_pixel_ns / 1000;
    let pure_us = u64::from(periph::panel_fill_us(periph::PANEL_MAX_PIXELS, periph::spi_hz(0)));
    assert!(pure_us - sim_us <= 1, "{sim_us} vs {pure_us}");
}
