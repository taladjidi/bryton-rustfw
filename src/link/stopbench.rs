//! The dev link's Stop mode benches (docs/power.md, "Stop mode", phase 1): what the board's low-power primitive saw
//! (main.rs `low_power`, hw.rs `stop`), the answer line of `bench srefresh` and `bench stop <ms> keep`, and the
//! backup-register words `bench stop` leaves for after its reset (`tools/dev.py stopreport` mirrors [`pack`]). Pure
//! logic, tested on the host; the registers it names are main.rs's and hw.rs's (sources there and in BOARD.md 1, 3, 8).
//!
//! The RTC wake-up timer counts RTCCLK / 16 (RTC_CR WUCKSEL 000; STM32CubeH7 `RTC_WAKEUPCLOCK_RTCCLK_DIV16`), the LSE's
//! 32 768 Hz (BOARD.md 1) / 16 = 2048 Hz, so [`wutr`] is the count of those less one (WUTF is set every WUT + 1
//! cycles: inferred from the HAL's use, RM0399 not re-read).

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
/// SYSCLK was not on its source from before right after the wake (RCC_CFGR SWS): the clocks had stopped.
pub const CLOCKS: u32 = 1 << 9;
/// The wake-up timer did not arm (WUTWF never seen, or WUTE did not read back): no sleep was tried.
pub const NO_TIMER: u32 = 1 << 10;
/// The power key (WKUP1, PWR_WKUPFR bit 0) ended the sleep.
pub const KEY: u32 = 1 << 11;

/// The main loop's idle Stop (main.rs `idle_stop`, phase 2a) did not try: a wake source was already pending (a key, the
/// touch INT), a byte from the Nordic chip was coming in, the panel's DMA ran, or the build has no Stop (main.rs
/// POWER_STEPS bit 5). Not a failure: the pass sleeps in WFI.
/// Above register 0's 12 flag bits: never set by a bench.
pub const PENDING: u32 = 1 << 12;

/// After an idle Stop's wake a key's, the touch's or the cable's EXTI flag, or a WKUPF flag, was up: a wake source that
/// came between the board's check and the WFI, so a Stop that did not happen (no STOPF) is no failure. Never set by a
/// bench.
pub const WAKE_PENDING: u32 = 1 << 13;

/// The wake's steps after a WFI in self-refresh: the regulator, the oscillators, the PLLs, SYSCLK's source.
pub const WAKE_STEPS: u32 = VOS | HSE | PLL | SW;

/// What an idle Stop's flags mean ([`outcome`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not tried ([`PENDING`]): nothing to count.
    NotTried,
    /// In Stop and back, all as before ([`PASS_OK`]).
    Ok,
    /// Not in Stop (no STOPF) because a wake source came just before the WFI ([`WAKE_PENDING`]): no failure.
    Interrupted,
    /// Harmless but no Stop: no timer, no self-refresh (so no WFI), or the system stayed in Run.
    Failed,
    /// The wake did not put everything back (the SDRAM's normal mode, the regulator, an oscillator, a PLL, SYSCLK's
    /// source): the unit may run on the HSI, the FMC on hclk3, SPI1 and SDMMC1 on a dead PLL. Only a reset is safe.
    Broken,
}

/// What an idle Stop's `flags` mean.
///
/// ```
/// use bryton_rustfw::stopbench::*;
/// assert_eq!(outcome(PENDING), Outcome::NotTried);
/// assert_eq!(outcome(PASS_OK), Outcome::Ok);
/// assert_eq!(outcome(NO_TIMER), Outcome::Failed);
/// assert_eq!(outcome(REFRESH_OUT), Outcome::Failed); // no self-refresh, no WFI
/// let run = PASS_OK & !(STOPF | CLOCKS); // stayed in Run, everything back
/// assert_eq!(outcome(run), Outcome::Failed);
/// assert_eq!(outcome(run | WAKE_PENDING), Outcome::Interrupted);
/// assert_eq!(outcome(PASS_OK & !PLL), Outcome::Broken);
/// assert_eq!(outcome(PASS_OK & !REFRESH_OUT), Outcome::Broken);
/// ```
pub fn outcome(flags: u32) -> Outcome {
    if flags & PENDING != 0 {
        Outcome::NotTried
    } else if flags & NO_TIMER != 0 {
        Outcome::Failed
    } else if flags & REFRESH_OUT == 0 || (flags & REFRESH_IN != 0 && flags & WAKE_STEPS != WAKE_STEPS) {
        Outcome::Broken
    } else if flags & PASS_OK == PASS_OK {
        Outcome::Ok
    } else if flags & REFRESH_IN != 0 && flags & WAKE_PENDING != 0 {
        Outcome::Interrupted
    } else {
        Outcome::Failed
    }
}

/// What a self-refresh needs to have gone right.
pub const REFRESH_OK: u32 = REFRESH_IN | REFRESH_OUT | WUTF | CRC;
/// What a Stop needs to have gone right.
pub const STOP_OK: u32 = REFRESH_OK | VOS | HSE | PLL | SW | STOPF | CLOCKS;

/// What the main loop's idle Stop needs to have gone right: as [`STOP_OK`], but its end may be a key or the touch
/// (no [`WUTF`]) and the image's CRC is not taken (a 780 KB read, every pass).
pub const PASS_OK: u32 = STOP_OK & !(WUTF | CRC);

/// The longest sleep a bench asks, ms: the wake-up timer's 16 bits at 2048 Hz hold 32 s.
pub const MAX_MS: u32 = 30_000;

/// The backup registers `bench stop` writes (src/app/settings.rs's map): 0 and 31, the shared diagnostics words, each
/// behind its own tag ([`TAG_A`], [`TAG_B`]), so a USB drop's or the standby's word is never read as one of these.
/// Register 0 holds the verdict: the installed firmware counts its USB drops in 31 after the reset.
pub const BKP: [usize; 2] = [crate::USB_LAST_BKP, crate::USB_DROPS_BKP];
/// Register 0's tag, bits 24..31.
pub const TAG_A: u32 = 0x57;
/// Register 31's tag, bits 24..31.
pub const TAG_B: u32 = 0x58;

/// The bench asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `bench srefresh`: a CSleep WFI with the SDRAM in self-refresh, the clocks kept; the line answered.
    Refresh,
    /// `bench stop <ms>`: USB off the bus (soft disconnect), Stop, the report kept, a reset.
    Stop,
    /// `bench stop <ms> keep`: Stop with USB as it is, no reset; the line answered if the link survives.
    Keep,
    /// `bench stop <ms> usboff`: USB fully off first (core and ULPI clocks, the PHY, VOS3), then as [`Mode::Stop`].
    UsbOff,
}

/// The registers that say why the system did or did not stop, read just before WFI and just after it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Regs {
    /// PWR_CPUCR (PDDS_D1..D3, STOPF, SBF, SBF_D1, SBF_D2, RUN_D3).
    pub cpucr: u32,
    /// PWR_CPU2CR, the M4's side.
    pub cpu2cr: u32,
    /// PWR_D3CR (VOS, VOSRDY).
    pub d3cr: u32,
    /// RCC_D3AMR: the D3 peripherals that keep D3 running on their own.
    pub d3amr: u32,
    /// RCC_CR: the oscillators and PLLs on and ready.
    pub rcc_cr: u32,
    /// RCC_CFGR: SYSCLK's source (SW) and what runs it (SWS).
    pub cfgr: u32,
    /// EXTI_CPUPR1..3: this core's pending lines.
    pub pr: [u32; 3],
}

/// What the low-power primitive saw, raw: the flags above, the registers around the WFI, and the core's cycles each
/// step of the wake took (the DWT counter, which runs only while the core does: on the HSI, 64 MHz, until SYSCLK is
/// back).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StopReport {
    /// Stop was asked (false: a CSleep WFI with the SDRAM in self-refresh, the clocks kept).
    pub stop: bool,
    /// [`REFRESH_IN`]..[`KEY`].
    pub flags: u32,
    /// The registers just before the WFI.
    pub pre: Regs,
    /// The same just after it, before anything is restored.
    pub post: Regs,
    /// Cycles waiting for the regulator, the oscillators, the PLLs, the SYSCLK switch, the SDRAM's normal mode.
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
    /// HSI after a Stop that stopped the clocks (64 MHz; RCC_CFGR STOPWUCK = 0), the rest at 200 MHz.
    ///
    /// ```
    /// use bryton_rustfw::stopbench::{CLOCKS, StopReport};
    /// let r = StopReport { stop: true, flags: CLOCKS, vos: 640, hse: 128_000, fmc: 2000, ..Default::default() };
    /// assert_eq!(r.wake_us(), 10 + 2000 + 10);
    /// ```
    pub fn wake_us(&self) -> u32 {
        let early = self.vos.saturating_add(self.hse).saturating_add(self.pll).saturating_add(self.sw);
        let hz = if self.flags & CLOCKS != 0 { 64 } else { 200 };
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

/// Microseconds from `a` to `b`, each (RTC_TR, RTC_SSR) read together; `prediv_s` is RTC_PRER's PREDIV_S (SSR counts
/// down from it, RM0399 RTC_SSR). Across midnight too. In us, so the main loop carries the sub-ms rest of each Stop.
///
/// ```
/// use bryton_rustfw::stopbench::elapsed_us;
/// assert_eq!(elapsed_us((0x0012_3456, 255), (0x0012_3457, 127), 255), 1_500_000);
/// assert_eq!(elapsed_us((0x0023_5959, 0), (0x0000_0000, 255), 255), 3907); // 255 / 256 s in, then midnight
/// assert_eq!(elapsed_us((0x0012_3456, 1023), (0x0012_3456, 767), 1023), 250_000);
/// ```
pub fn elapsed_us(a: (u32, u32), b: (u32, u32), prediv_s: u32) -> u32 {
    const DAY_US: u64 = 86_400_000_000;
    let at = |(tr, ssr): (u32, u32)| {
        let (h, m, s) = crate::rtc_time_from_tr(tr);
        let sub = u64::from(prediv_s.saturating_sub(ssr)) * 1_000_000 / (u64::from(prediv_s) + 1);
        (u64::from(h) * 3600 + u64::from(m) * 60 + u64::from(s)) * 1_000_000 + sub
    };
    u32::try_from((at(b) + DAY_US - at(a)) % DAY_US).unwrap_or(u32::MAX)
}

/// The two backup words of a Stop's report, for [`BKP`]: register 0 is [`TAG_A`], the flags in bits 12..23 and the
/// sleep, ms, in bits 0..11 (4095 at most); register 31 is [`TAG_B`], PWR_CPU2CR's PDDS_D1, PDDS_D2, PDDS_D3 and RUN_D3
/// before the WFI in bits 20..23, the regulator's wait, us, in bits 12..19 (255 at most), the whole wake, us, in bits
/// 0..11 (4095).
///
/// ```
/// use bryton_rustfw::stopbench::{Regs, STOP_OK, StopReport, pack};
/// let pre = Regs { cpu2cr: 1 << 11 | 0b100, ..Default::default() };
/// let r = StopReport { stop: true, flags: STOP_OK, pre, vos: 64 * 5, hse: 64 * 2000, slept_ms: 1000, ..Default::default() };
/// assert_eq!(pack(&r), [0x57 << 24 | STOP_OK << 12 | 1000, 0x58 << 24 | 0b1100 << 20 | 5 << 12 | 2005]);
/// ```
pub fn pack(r: &StopReport) -> [u32; 2] {
    let c2 = r.pre.cpu2cr;
    let d3 = (c2 & 0b111) | (c2 >> 11 & 1) << 3;
    let vos_us = r.vos / if r.flags & CLOCKS != 0 { 64 } else { 200 };
    [
        TAG_A << 24 | (r.flags & 0xfff) << 12 | r.slept_ms.min(0xfff),
        TAG_B << 24 | d3 << 20 | vos_us.min(0xff) << 12 | r.wake_us().min(0xfff),
    ]
}

fn send_hex(h: &mut impl Host, v: u32) {
    h.send(b" ");
    h.send(&crate::dev::hex(v)[..8]);
}

/// The answer to `bench srefresh` and `bench stop <ms> keep`: `<srefresh | stop> <ok | fail> flags <hex> slept <ms>
/// wake <us> cycles <vos> <hse> <pll> <sw> <fmc>`, then `pre` and `post`, each the [`Regs`] in their order (cpucr
/// cpu2cr d3cr d3amr rcc_cr cfgr pr1 pr2 pr3), in hex.
pub fn send(h: &mut impl Host, r: &StopReport) {
    h.send(if r.stop { b"stop " } else { b"srefresh " });
    h.send(if r.ok() { b"ok" } else { b"fail" });
    h.send(b" flags");
    send_hex(h, r.flags);
    h.send(b" slept ");
    h.send(crate::dev::decimal(u64::from(r.slept_ms), &mut [0; 20]));
    h.send(b" wake ");
    h.send(crate::dev::decimal(u64::from(r.wake_us()), &mut [0; 20]));
    h.send(b" cycles");
    for c in [r.vos, r.hse, r.pll, r.sw, r.fmc] {
        h.send(b" ");
        h.send(crate::dev::decimal(u64::from(c), &mut [0; 20]));
    }
    for (name, g) in [(&b" pre"[..], &r.pre), (b" post", &r.post)] {
        h.send(name);
        for v in [g.cpucr, g.cpu2cr, g.d3cr, g.d3amr, g.rcc_cr, g.cfgr].into_iter().chain(g.pr) {
            send_hex(h, v);
        }
    }
    h.send(b"\n");
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// Every flag fits the 12 bits register 0 keeps, the sleep saturates instead of spilling into them, and the tags
    /// differ from the other users of registers 0 and 31 (settings.rs: 0xd1, 0x5b, 0xd2, 0x0b, 0x5c).
    #[test]
    fn the_words_keep_their_fields_apart() {
        assert_eq!((STOP_OK | NO_TIMER | KEY) >> 12, 0);
        let r = StopReport { stop: true, flags: u32::MAX, slept_ms: u32::MAX, vos: u32::MAX, ..Default::default() };
        let [a, b] = pack(&r);
        assert_eq!((a >> 24, a >> 12 & 0xfff, a & 0xfff), (TAG_A, 0xfff, 0xfff));
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

    /// A self-refresh is judged on its own flags, a Stop on all of them: the 2026-11-03 unit run (flags 0xff, no
    /// STOPF, the clocks never stopped) is a failed Stop and a good self-refresh.
    #[test]
    fn ok_asks_what_the_mode_needs() {
        let r = StopReport { flags: 0xff, ..Default::default() };
        assert!(r.ok());
        assert!(!StopReport { stop: true, ..r }.ok());
        assert!(StopReport { stop: true, flags: STOP_OK, ..r }.ok());
        assert!(!StopReport { flags: REFRESH_OK & !CRC, ..r }.ok());
    }

    /// The answer line carries both register sets, nine words each, in order.
    #[test]
    fn the_line_has_both_register_sets() {
        #[derive(Default)]
        struct Out(std::vec::Vec<u8>);
        impl Host for Out {
            fn send(&mut self, data: &[u8]) {
                self.0.extend_from_slice(data);
            }
            fn stage(&mut self, _: u32, _: &[u8]) {}
            fn peek(&mut self, _: u32) -> u32 {
                0
            }
            fn poke(&mut self, _: u32, _: u32) {}
        }
        let post = Regs { cpucr: 0x1a3, pr: [0, 0, 0x80_0000], ..Default::default() };
        let r = StopReport { stop: true, flags: 0xff, post, ..Default::default() };
        let mut o = Out::default();
        send(&mut o, &r);
        let line = std::string::String::from_utf8(o.0).unwrap_or_default();
        assert!(line.starts_with("stop fail flags 000000ff slept 0 wake 0 cycles 0 0 0 0 0 pre "), "{line}");
        assert!(
            line.ends_with(" post 000001a3 00000000 00000000 00000000 00000000 00000000 00000000 00000000 00800000\n")
        );
    }
}
