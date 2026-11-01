//! The register values main.rs writes to the buzzer's timer, the backlight's and the RTC, computed here so the
//! arithmetic is tested on the host (docs/review-2026-11-01.md, finding 32); main.rs keeps the writes. Sources:
//! RM0399 (RCC_D1CFGR HPRE, RCC_D2CFGR D2PPRE2 and the timer clocks; TIM1 PSC / ARR / CCR3; LPTIM CMP / ARR; RTC_TR /
//! RTC_DR), BOARD.md 8 and 14.

/// A SysTick period's longest, ms: its 24-bit counter at 200 MHz (main.rs `timer_start`).
pub const TIMER_MAX_MS: u32 = 80;

/// The most pixels one DMA run sends: 0x7ffe of them, 65 532 B (main.rs `lcd_ram`).
pub const PANEL_MAX_PIXELS: u32 = 0x7ffe;

/// SPI1's bit clock for the divider `mbr` main.rs `spi_init` picks: 0 is PLL1 Q (100 MHz) / 2, 1 is per_ck (HSI,
/// 64 MHz) / 4, 2 is per_ck / 8 (BOARD.md 6). Anything else is taken as the slowest.
pub const fn spi_hz(mbr: u32) -> u32 {
    match mbr {
        0 => 50_000_000,
        1 => 16_000_000,
        _ => 8_000_000,
    }
}

/// Microseconds `pixels` RGB565 pixels (16 bits each) take to leave an SPI at `spi_hz`, rounded up: the one fill
/// time, used by the DMA time-out here and by the simulator's panel clock (`Cost::panel_pixel_ns` is its 50 MHz
/// value).
///
/// ```
/// use bryton_rustfw::periph::panel_fill_us;
/// assert_eq!(panel_fill_us(0x7ffe, 50_000_000), 10_486); // the 10 ms time-out of 2026-10-27 was below it
/// ```
pub const fn panel_fill_us(pixels: u32, spi_hz: u32) -> u32 {
    ((pixels as u64 * 16 * 1_000_000).div_ceil(spi_hz as u64)) as u32
}

/// A DMA wait's bound, ms, for SPI1's divider `mbr`: twice the longest run's time (margin 2), at least 30 ms (a
/// first bound of 10 ms cut full-screen fills short, seen on the unit 2026-10-27), at most SysTick's longest period.
/// At 8 MHz twice 65.5 ms does not fit SysTick: the margin there is 1.2 (a tested limit, not a goal).
pub const fn dma_timeout_ms(mbr: u32) -> u32 {
    let ms = 2 * panel_fill_us(PANEL_MAX_PIXELS, spi_hz(mbr)).div_ceil(1000);
    let ms = if ms < 30 { 30 } else { ms };
    if ms < TIMER_MAX_MS { ms } else { TIMER_MAX_MS }
}

/// TIM1's input clock from the core clock `cpu_hz` and the prescalers the bootloader left: RCC_D1CFGR's HPRE (bits
/// 3:0: 0xxx /1, 1000 /2, 1001 /4, 1010 /8, 1011 /16, 1100 /64, 1101 /128, 1110 /256, 1111 /512: no /32) and
/// RCC_D2CFGR's D2PPRE2 (bits 10:8: 0xx /1, 100 /2, 101 /4, 110 /8, 111 /16), doubled when APB2 is divided (RM0399,
/// the timer clocks with TIMPRE 0).
///
/// ```
/// use bryton_rustfw::periph::tim_clock_hz;
/// assert_eq!(tim_clock_hz(200_000_000, 0, 0), 200_000_000);
/// assert_eq!(tim_clock_hz(400_000_000, 0b1000, 0b100 << 8), 200_000_000); // AHB /2, APB2 /2, timers x2
/// ```
pub fn tim_clock_hz(cpu_hz: u32, d1cfgr: u32, d2cfgr: u32) -> u32 {
    let hpre = d1cfgr & 0xf;
    let ahb = match hpre {
        0..8 => cpu_hz,
        8..12 => cpu_hz >> (hpre - 7),
        _ => cpu_hz >> (hpre - 6),
    };
    let ppre2 = (d2cfgr >> 8) & 7;
    if ppre2 < 4 { ahb } else { (ahb >> (ppre2 - 3)) * 2 }
}

/// TIM1's PSC, ARR and CCR3 for a square wave at `hz` (> 0) from a `clk` timer clock: the smallest prescaler that
/// keeps the period in 16 bits, the period, half of it high. In 64 bits: `hz * 65_536` passes `u32` from 65 536 Hz.
///
/// ```
/// use bryton_rustfw::periph::tone;
/// assert_eq!(tone(200_000_000, 2500), (1, 39_999, 20_000)); // 200 MHz / 2 / 40 000 = 2500 Hz
/// ```
pub fn tone(clk: u32, hz: u32) -> (u32, u32, u32) {
    let psc = u64::from(clk).checked_div(u64::from(hz) * 65_536).unwrap_or(0) as u32;
    let arr = (clk / (psc + 1)).checked_div(hz).unwrap_or(0).clamp(2, 65_536) - 1;
    (psc, arr, arr.div_ceil(2))
}

/// LPTIM1's CMP for the backlight at `level` %, or None for 0 (off: the pin a plain low output). CMP stays below ARR
/// (99): RM0399's LPTIM wants ARR greater than CMP (inferred from its register notes; Renode's model refuses CMP =
/// ARR), so 100 % is 99 %: a step no eye sees.
///
/// ```
/// use bryton_rustfw::periph::backlight_cmp;
/// assert_eq!((backlight_cmp(0), backlight_cmp(1), backlight_cmp(100)), (None, Some(0), Some(98)));
/// ```
pub fn backlight_cmp(level: u8) -> Option<u32> {
    (level > 0).then(|| u32::from(level.min(99)) - 1)
}

/// RTC_TR's time bits (PM, HT, HU, MNT, MNU, ST, SU; RM0399 RTC_TR): the reserved ones are written 0.
pub const RTC_TR_BITS: u32 = 0x007f_7f7f;
/// RTC_DR's date bits (YT, YU, WDU, MT, MU, DT, DU; RM0399 RTC_DR).
pub const RTC_DR_BITS: u32 = 0x00ff_ff3f;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every HPRE code against RM0399's table, and every D2PPRE2.
    #[test]
    fn timer_clock_follows_the_prescaler_tables() {
        let ahb = [1, 1, 1, 1, 1, 1, 1, 1, 2, 4, 8, 16, 64, 128, 256, 512];
        for (code, div) in ahb.iter().enumerate() {
            assert_eq!(tim_clock_hz(512_000_000, code as u32, 0), 512_000_000 / div, "HPRE {code:04b}");
        }
        let apb = [1, 1, 1, 1, 2, 4, 8, 16];
        for (code, div) in apb.iter().enumerate() {
            let x2 = if *div == 1 { 1 } else { 2 };
            assert_eq!(tim_clock_hz(64_000_000, 0, (code as u32) << 8), 64_000_000 / div * x2, "PPRE2 {code:03b}");
        }
        assert_eq!(tim_clock_hz(1000, 0xffff_fff0, 0xffff_f8ff), 1000, "other bits ignored");
    }

    /// The tone's frequency is within a timer tick of `hz`, its period fits ARR and the prescaler, for every audible
    /// tone and well past it (the old `hz * 65_536` wrapped from 65 536 Hz: a zero prescaler, a 16-bit overflow).
    #[test]
    fn tone_period_fits_sixteen_bits() {
        for clk in [64_000_000u32, 200_000_000, 400_000_000] {
            for hz in (1..=20_000).step_by(7).chain([65_535, 65_536, 65_537, 1_000_000, u32::MAX]) {
                let (psc, arr, ccr) = tone(clk, hz);
                assert!(psc <= 0xffff && (1..=0xffff).contains(&arr), "{clk} {hz}: {psc} {arr}");
                assert_eq!(ccr, arr.div_ceil(2));
                let tick = clk as f64 / f64::from(psc + 1);
                let got = tick / f64::from(arr + 1);
                if f64::from(hz) * 3.0 <= tick {
                    let err = (got - f64::from(hz)).abs() / f64::from(hz);
                    assert!(err < 0.01 || (got - f64::from(hz)).abs() < 1.0, "{clk} {hz}: {got}");
                } else {
                    assert_eq!(arr, 1, "too high for the clock: the fastest wave");
                }
            }
        }
        // the smallest prescaler: one less would overflow the period
        let (psc, _, _) = tone(200_000_000, 50);
        assert!(200_000_000 / psc / 50 > 65_536 && 200_000_000 / (psc + 1) / 50 <= 65_536);
    }

    #[test]
    fn dma_timeout_covers_the_longest_fill_at_every_divider() {
        for mbr in 0..=2 {
            let fill_ms = f64::from(panel_fill_us(PANEL_MAX_PIXELS, spi_hz(mbr))) / 1000.0;
            let margin = f64::from(dma_timeout_ms(mbr)) / fill_ms;
            // margin 2; at 8 MHz SysTick's 80 ms is the ceiling (65.5 ms fill)
            assert!(margin >= if mbr < 2 { 2.0 } else { 1.2 }, "divider {mbr}: {margin:.2} x {fill_ms:.1} ms");
            assert!(dma_timeout_ms(mbr) <= TIMER_MAX_MS);
        }
        assert_eq!(dma_timeout_ms(0), 30); // seen on the unit 2026-10-27: 10 ms cut fills short
    }

    #[test]
    fn backlight_cmp_below_arr() {
        for level in 1..=u8::MAX {
            assert!(backlight_cmp(level).is_some_and(|c| c < 99));
        }
        assert_eq!(backlight_cmp(50), Some(49));
    }

    #[test]
    fn rtc_bits_drop_the_reserved() {
        // TR: bit 7, 15, 23 and up reserved; DR: 6, 7, 24 and up
        assert_eq!(RTC_TR_BITS & (1 << 7 | 1 << 15 | 1 << 23 | 0xff00_0000), 0);
        assert_eq!(RTC_DR_BITS & (1 << 6 | 1 << 7 | 0xff00_0000), 0);
        assert_eq!(0x0012_3456 & RTC_TR_BITS, 0x0012_3456); // 12:34:56
        assert_eq!(0x0026_d023 & RTC_DR_BITS, 0x0026_d023); // 2026, Saturday (6), October, 23
    }
}
