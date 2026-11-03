//! The dev link's Stop mode benches (docs/power.md, "Stop mode", phase 1): what the board's low-power primitive saw
//! (main.rs `low_power`, hw.rs `stop`), the answer to `bench srefresh`, and the two backup-register words `bench stop`
//! leaves for after its reset (`tools/dev.py stopreport` mirrors [`pack`]). Pure logic, tested on the host; the
//! registers it names are main.rs's and hw.rs's (sources there and in BOARD.md 2 and 8).
//!
//! The RTC wake-up timer counts RTCCLK / 16 (RTC_CR WUCKSEL 000; STM32CubeH7 `RTC_WAKEUPCLOCK_RTCCLK_DIV16`), the LSE's
//! 32 768 Hz (BOARD.md 1) / 16 = 2048 Hz, so [`wutr`] is the count of those less one (RM0399 RTC, "Periodic auto-wakeup":
//! WUTF is set every WUT + 1 cycles).

use crate::dev::Host;

/// The SDRAM reached self-refresh (FMC_SDSR MODES2 = 01) after the command.
pub const REFRESH_IN: u32 = 1 << 0;
/// The SDRAM is back in normal mode (MODES2 = 00) after the wake.
pub const REFRESH_OUT: u32 = 1 << 1;
/// The regulator was ready after the wake (PWR_D3CR VOSRDY and PWR_CSR1 ACTVOSRDY).
pub const VOS: u32 = 1 << 2;
/// The oscillators on before (HSE, HSI48, CSI) were ready again (none on: set).
pub const HSE: u32 = 1 << 3;
/// The PLLs on before were locked again.
pub const PLL: u32 = 1 << 4;
/// SYSCLK was back on its source from before (RCC_CFGR SWS = SW).
pub const SW: u32 = 1 << 5;
/// The RTC wake-up timer's flag (RTC_ISR WUTF) was set after the sleep: its end came from the timer.
pub const WUTF: u32 = 1 << 6;
/// The image's CRC after the sleep equals the one just before: the SDRAM kept its data.
pub const CRC: u32 = 1 << 7;
/// PWR_CPUCR STOPF after the wake: the system really was in Stop (RM0399 PWR, PWR_CPUCR; stm32h747xx.h bit 5).
pub const STOPF: u32 = 1 << 8;

/// What a self-refresh needs to have gone right.
pub const REFRESH_OK: u32 = REFRESH_IN | REFRESH_OUT | WUTF | CRC;
/// What a Stop needs to have gone right.
pub const STOP_OK: u32 = REFRESH_OK | VOS | HSE | PLL | SW | STOPF;

/// The longest sleep a bench asks, ms: the wake-up timer's 16 bits at 2048 Hz hold 32 s.
pub const MAX_MS: u32 = 30_000;

/// The backup registers `bench stop` writes (src/app/settings.rs's map): 0 and 31, the shared diagnostics words, each
/// behind its own tag ([`TAG_A`], [`TAG_B`]), so a USB drop's or the standby's word is never read as one of these.
pub const BKP: [usize; 2] = [crate::USB_LAST_BKP, crate::USB_DROPS_BKP];
/// Register 0's tag, bits 24..31.
pub const TAG_A: u32 = 0x57;
/// Register 31's tag, bits 24..31.
pub const TAG_B: u32 = 0x58;

/// What the low-power primitive saw, raw: the flags above, the power registers, and the core's cycles each step of
/// the wake took (the DWT counter, which runs only while the core does: on the HSI, 64 MHz, until SYSCLK is back).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StopReport {
    /// Stop was asked (false: a CSleep WFI with the SDRAM in self-refresh, the clocks kept).
    pub stop: bool,
    /// [`REFRESH_IN`]..[`STOPF`].
    pub flags: u32,
    /// PWR_CPUCR after the wake.
    pub cpucr: u32,
    /// PWR_CPU2CR before the entry (the M4's side of D3: PDDS_D3 bit 2, RUN_D3 bit 11).
    pub cpu2cr: u32,
    /// Cycles waiting for the regulator, the HSE, the PLLs, the SYSCLK switch, the SDRAM's normal mode.
    pub vos: u32,
    /// See [`StopReport::vos`].
    pub hse: u32,
    /// See [`StopReport::vos`].
    pub pll: u32,
    /// See [`StopReport::vos`].
    pub sw: u32,
    /// See [`StopReport::vos`].
    pub fmc: u32,
    /// The sleep as the RTC timed it (seconds and sub-seconds), ms.
    pub slept_ms: u32,
}

impl StopReport {
    /// The wake's length, us, from WFI's return to the SDRAM's normal mode: the steps before the SYSCLK switch on the
    /// HSI after a Stop (64 MHz; RCC_CFGR STOPWUCK = 0), the rest at 200 MHz (CSleep keeps the PLL).
    ///
    /// ```
    /// let r = bryton_rustfw::stopbench::StopReport { stop: true, vos: 640, hse: 128_000, fmc: 2000, ..Default::default() };
    /// assert_eq!(r.wake_us(), 10 + 2000 + 10);
    /// ```
    pub fn wake_us(&self) -> u32 {
        let early = self.vos.saturating_add(self.hse).saturating_add(self.pll).saturating_add(self.sw);
        let hz = if self.stop { 64 } else { 200 };
        early / hz + self.fmc / 200
    }

    /// Everything the mode needs went right.
    pub fn ok(&self) -> bool {
        let want = if self.stop { STOP_OK } else { REFRESH_OK };
        self.flags & want == want
    }
}

/// RTC_WUTR for a sleep of `ms` (1 to [`MAX_MS`]) at RTCCLK / 16.
///
/// ```
/// assert_eq!(bryton_rustfw::stopbench::wutr(1000), 2047);
/// assert_eq!(bryton_rustfw::stopbench::wutr(0), 0);
/// ```
pub fn wutr(ms: u32) -> u32 {
    (ms.min(MAX_MS) * 2048 / 1000).saturating_sub(1)
}

/// Milliseconds from `a` to `b`, each (RTC_TR, RTC_SSR) read together; `prediv_s` is RTC_PRER's PREDIV_S (SSR counts
/// down from it, RM0399 RTC_SSR). Across midnight too.
///
/// ```
/// use bryton_rustfw::stopbench::elapsed_ms;
/// assert_eq!(elapsed_ms((0x0012_3456, 255), (0x0012_3457, 127), 255), 1500);
/// assert_eq!(elapsed_ms((0x0023_5959, 0), (0x0000_0000, 255), 255), 4); // 255 / 256 s in, then midnight
/// ```
pub fn elapsed_ms(a: (u32, u32), b: (u32, u32), prediv_s: u32) -> u32 {
    const DAY_MS: u32 = 86_400_000;
    let at = |(tr, ssr): (u32, u32)| {
        let (h, m, s) = crate::rtc_time_from_tr(tr);
        let sub = prediv_s.saturating_sub(ssr) * 1000 / (prediv_s + 1);
        (u32::from(h) * 3600 + u32::from(m) * 60 + u32::from(s)) * 1000 + sub
    };
    (at(b) + DAY_MS - at(a)) % DAY_MS
}

/// The two backup words of a Stop's report, for [`BKP`]: register 0 is [`TAG_A`], the flags in bits 14..22 and the
/// sleep, ms, in bits 0..13 (16 s at most); register 31 is [`TAG_B`], PWR_CPU2CR's PDDS_D1, PDDS_D2, PDDS_D3 and RUN_D3
/// in bits 20..23, the regulator's wait, us, in bits 12..19 (255 at most), the whole wake, us, in bits 0..11 (4095).
///
/// ```
/// use bryton_rustfw::stopbench::{STOP_OK, StopReport, pack};
/// let r = StopReport { stop: true, flags: STOP_OK, cpu2cr: 1 << 11 | 0b100, vos: 64 * 5, hse: 64 * 2000, slept_ms: 1000, ..Default::default() };
/// assert_eq!(pack(&r), [0x57 << 24 | STOP_OK << 14 | 1000, 0x58 << 24 | 0b1100 << 20 | 5 << 12 | 2005]);
/// ```
pub fn pack(r: &StopReport) -> [u32; 2] {
    let d3 = (r.cpu2cr & 0b111) | (r.cpu2cr >> 11 & 1) << 3;
    [
        TAG_A << 24 | (r.flags & 0x1ff) << 14 | r.slept_ms.min(0x3fff),
        TAG_B << 24 | d3 << 20 | (r.vos / 64).min(0xff) << 12 | r.wake_us().min(0xfff),
    ]
}

/// The answer to `bench srefresh` (and to `bench stop` on a board that returns from it, the simulator): `<srefresh |
/// stop> <ok | fail> flags <hex> slept <ms> wake <us> cycles <vos> <hse> <pll> <sw> <fmc> cpucr <hex> cpu2cr <hex>`.
pub fn send(h: &mut impl Host, r: &StopReport) {
    h.send(if r.stop { b"stop " } else { b"srefresh " });
    h.send(if r.ok() { b"ok" } else { b"fail" });
    h.send(b" flags ");
    h.send(&crate::dev::hex(r.flags)[..8]);
    h.send(b" slept ");
    h.send(crate::dev::decimal(u64::from(r.slept_ms), &mut [0; 20]));
    h.send(b" wake ");
    h.send(crate::dev::decimal(u64::from(r.wake_us()), &mut [0; 20]));
    h.send(b" cycles");
    for c in [r.vos, r.hse, r.pll, r.sw, r.fmc] {
        h.send(b" ");
        h.send(crate::dev::decimal(u64::from(c), &mut [0; 20]));
    }
    h.send(b" cpucr ");
    h.send(&crate::dev::hex(r.cpucr)[..8]);
    h.send(b" cpu2cr ");
    h.send(&crate::dev::hex(r.cpu2cr)[..8]);
    h.send(b"\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every flag fits the 9 bits register 0 keeps, the sleep saturates instead of spilling into them, and the tags
    /// differ from the other users of registers 0 and 31 (settings.rs: 0xd1, 0x5b, 0xd2, 0x0b, 0x5c).
    #[test]
    fn the_words_keep_their_fields_apart() {
        assert_eq!(STOP_OK >> 9, 0);
        let r = StopReport { stop: true, flags: u32::MAX, slept_ms: u32::MAX, vos: u32::MAX, ..Default::default() };
        let [a, b] = pack(&r);
        assert_eq!((a >> 24, a >> 14 & 0x3ff, a & 0x3fff), (TAG_A, 0x1ff, 0x3fff));
        assert_eq!((b >> 24, b >> 12 & 0xff, b & 0xfff), (TAG_B, 0xff, 0xfff));
        for t in [TAG_A, TAG_B] {
            assert!(![0xd1, 0x5b, 0xd2, 0x0b, 0x5c].contains(&t));
        }
        assert_eq!(BKP, [0, 31]);
    }

    /// The wake-up timer's count for every allowed sleep fits its 16 bits and is the sleep at 2048 Hz.
    #[test]
    fn wutr_fits_its_register() {
        for ms in 1..=MAX_MS {
            let w = wutr(ms);
            assert!(w <= 0xffff);
            assert!(((w + 1) * 1000 / 2048).abs_diff(ms) <= 1, "{ms} ms: {w}");
        }
    }

    /// A self-refresh is judged on its own flags, a Stop on all of them.
    #[test]
    fn ok_asks_what_the_mode_needs() {
        let r = StopReport { flags: REFRESH_OK, ..Default::default() };
        assert!(r.ok());
        assert!(!StopReport { stop: true, ..r }.ok());
        assert!(StopReport { stop: true, flags: STOP_OK, ..r }.ok());
        assert!(!StopReport { flags: REFRESH_OK & !CRC, ..r }.ok());
    }
}
