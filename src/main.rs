//! bryton-rustfw: a minimal firmware for the Bryton Rider S510 bike computer, written from scratch. It shows the time
//! kept by the RTC, big, on the panel, the heart rate from an ANT+ strap, and on a second page the BLE devices
//! nearby. Keys: Page turns the ride pages, hold Page for the menu (sensors, pages, wheel, nearby devices), Back
//! goes back (level and time-out of the backlight in the menu), hold Back (power) 3 s to switch off (standby; the power button starts it again).
//!
//! Every register access below is traced to BOARD.md (the board as the stock firmware sets it up) or
//! to the STM32H747 reference manual (RM0399). No interrupts, no RTOS: one loop. The only crates are the USB
//! stack for the dev link (usb.rs), started when a cable is plugged in.
//!
//! Hand-off from the bootloader (BOARD.md 2): clocks set (CPU 200 MHz, same PLL1 as the OS), FMC SDRAM on, the
//! OS area copied to 0xd0000000, MSP = word 0, jump to word 1. It never checks the image.
//! The stock OS runs its first code from SDRAM in exactly this state, before its own MPU setup.

#![no_std]
#![no_main]
#![deny(unsafe_code)] // only hw.rs may use unsafe

mod board;

use board::{hw, usb};
use bryton_rustfw::dev::{Effect, Session, Stats, UsbWatch};
use bryton_rustfw::emmc::BusError;
use bryton_rustfw::prof::Sec;
use bryton_rustfw::ui::UsbDot;
use bryton_rustfw::{App, Board, POLL_MS, STANDBY_BKP, STANDBY_FLAGS_BKP, TOUCH_MS, standby_flags_word};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use hw::{barrier, rd, wfi, wr, wr8};

// ANT_KEY, DI2_KEY: the ANT+ and Shimano network keys from the private ant_key.bin / di2_key.bin, or None (build.rs).
include!(concat!(env!("OUT_DIR"), "/ant_key.rs"));

#[inline(always)]
fn modify(a: u32, clear: u32, set: u32) {
    wr(a, (rd(a) & !clear) | set);
}

// ---------------------------------------------------------------------------------------------------------------
// MPU: one region over the 64 MB SDRAM, normal memory, write-through cacheable (for the I-cache), executable, full
// access; everything else the default map (PRIVDEFENA). The MPU is only switched ON, never off: with it off, 0xc0000000..0xdfffffff would be
// device memory (execute-never under the default map), and this code runs there. Highest region number so it
// wins over anything the bootloader could have left.
// ---------------------------------------------------------------------------------------------------------------
fn mpu_init() {
    const MPU_CTRL: u32 = 0xe000_ed94;
    const MPU_RNR: u32 = 0xe000_ed98;
    const MPU_RBAR: u32 = 0xe000_ed9c;
    const MPU_RASR: u32 = 0xe000_eda0;
    let regions = (rd(0xe000_ed90) >> 8) & 0xff; // MPU_TYPE.DREGION
    if regions == 0 {
        return;
    }
    barrier();
    wr(MPU_RNR, regions - 1);
    wr(MPU_RBAR, 0xd000_0000);
    // ENABLE, SIZE 25 (2^26 = 64 MB), TEX 000 C 1 B 0 (normal, write-through: the I-cache may hold its code; the
    // D-cache stays off, so data accesses remain uncached), AP 011 (full), XN 0
    wr(MPU_RASR, 1 | (25 << 1) | (1 << 17) | (3 << 24));
    wr(MPU_CTRL, rd(MPU_CTRL) | 0b101); // ENABLE + PRIVDEFENA
    barrier();
    // I-cache on (ARMv7-M: invalidate all, then CCR.IC). The bootloader leaves it off (BOARD.md 2); the RAM-run
    // trampoline invalidates it before jumping to a new image (hw.rs).
    wr(0xe000_ef50, 0); // ICIALLU
    barrier();
    modify(0xe000_ed14, 0, 1 << 17); // SCB_CCR.IC
    barrier();
}

// ---------------------------------------------------------------------------------------------------------------
// Time: the DWT cycle counter at the bootloader's 200 MHz (BOARD.md 1: the bootloader uses the OS PLL1 setup).
// ---------------------------------------------------------------------------------------------------------------
const CPU_HZ: u32 = 200_000_000;

fn dwt_init() {
    modify(0xe000_edfc, 0, 1 << 24); // DEMCR.TRCENA
    wr(0xe000_1fb0, 0xc5ac_ce55); // DWT_LAR unlock (Cortex-M7)
    modify(0xe000_1000, 0, 1); // DWT_CTRL.CYCCNTENA
}

fn cycles() -> u32 {
    rd(0xe000_1004)
}

const SYST_CSR: u32 = 0xe000_e010;
const SYST_RVR: u32 = 0xe000_e014;
const SYST_CVR: u32 = 0xe000_e018;
const SCB_ICSR: u32 = 0xe000_ed04;

/// SysTick runs a period (timer_start to timer_stop): a DMA wait (dma_wait) leaves it alone then.
static TIMER_ON: AtomicBool = AtomicBool::new(false);

/// A SysTick period's longest, ms: its 24-bit counter at 200 MHz.
const TIMER_MAX_MS: u32 = 80;

/// Starts a SysTick period of `ms` (at most [`TIMER_MAX_MS`]). Its end makes the SysTick exception
/// pending, which wakes WFI even with interrupts masked (ARMv7-M B1.5.19); the handler never runs.
// ponytail: WFI sleep keeps the core clock running; stop mode with an RTC / LPTIM wake-up is the next step down.
fn timer_start(ms: u32) {
    TIMER_ON.store(true, Ordering::Relaxed);
    wr(SYST_CSR, 0);
    wr(SCB_ICSR, 1 << 25); // PENDSTCLR
    wr(SYST_RVR, ms.min(TIMER_MAX_MS) * (CPU_HZ / 1000) - 1);
    wr(SYST_CVR, 0);
    wr(SYST_CSR, 0b111); // ENABLE, TICKINT, CLKSOURCE = processor clock
}

/// The period has ended (COUNTFLAG; reading clears it, so act on a true at once).
fn timer_done() -> bool {
    rd(SYST_CSR) & (1 << 16) != 0
}

/// Milliseconds of a period of `ms` gone so far (SysTick keeps counting through WFI; the DWT cycle counter does not:
/// the core clock stops in sleep, DBGMCU_CR being 0, seen on the unit 2026-10-23).
fn timer_elapsed_ms(ms: u32) -> u32 {
    let total = ms.min(TIMER_MAX_MS) * (CPU_HZ / 1000);
    total.saturating_sub(rd(SYST_CVR)) / (CPU_HZ / 1000)
}

fn timer_stop() {
    TIMER_ON.store(false, Ordering::Relaxed);
    wr(SYST_CSR, 0);
    wr(SCB_ICSR, 1 << 25); // PENDSTCLR
}

/// WFI, every wake source's NVIC pending bit cleared first: the one way this firmware sleeps.
///
/// The wake sources (the touch line's EXTI9_5, DMA1 stream 0, OTG_HS) are enabled in the NVIC only to end WFI
/// (interrupts stay masked, so no handler runs and nothing else clears them). Each source is level: its peripheral
/// flag (EXTI_CPUPR1, DMA1_LISR, GINTSTS) holds the line, and the line pends its IRQ again after this clear, so an
/// event not yet consumed still ends the sleep. A pending bit whose flag was already cleared did not: cleared by each
/// source after its own flag, it stuck whenever the flag's write reached the line after the NVIC clear (the
/// peripheral is on another bus and domain), and every WFI then returned at once. Seen on the unit twice: IRQ 23
/// (2026-10-27, ISPR0 0x00800400) and IRQ 11 (2026-10-27, ISPR0 0x00000c00, ICSR.VECTPENDING 27, DMA1_LISR 0: the
/// DMA's clear in `dma_wait`, 5 runs in 6 with its read back removed). Clearing here, after the sources were read and
/// just before sleeping, depends on no clear's timing.
fn sleep() {
    wr(NVIC_ICPR0, EXTI9_5_IRQ | DMA1_S0_IRQ | KEY_IRQS);
    wr(NVIC_ICPR0 + 4, USART2_IRQ);
    wr(NVIC_ICPR0 + 8, usb::OTG_HS_IRQ);
    wfi();
}

/// Sleeps (WFI) instead of spinning.
fn delay_ms(ms: u32) {
    let mut left = ms;
    while left > 0 {
        let chunk = left.min(TIMER_MAX_MS);
        left -= chunk;
        timer_start(chunk);
        while !timer_done() {
            sleep(); // another wake-up (USB activity): sleep again
        }
        timer_stop();
    }
}

/// System reset (AIRCR SYSRESETREQ): bootloader, then the firmware on the eMMC.
fn system_reset() -> ! {
    barrier();
    wr(0xe000_ed0c, 0x05fa_0004);
    loop {
        wfi();
    }
}

// ---------------------------------------------------------------------------------------------------------------
// RCC and GPIO (RM0399; pin roles from BOARD.md 4)
// ---------------------------------------------------------------------------------------------------------------
const RCC: u32 = 0x5802_4400;
const RCC_CR: u32 = RCC;
const RCC_D1CCIPR: u32 = RCC + 0x04c;
const RCC_D2CCIP1R: u32 = RCC + 0x050;
const RCC_AHB4ENR: u32 = RCC + 0x0e0;
const RCC_APB2ENR: u32 = RCC + 0x0f0;
const RCC_APB4ENR: u32 = RCC + 0x0f4;

#[derive(Clone, Copy)]
enum Port {
    A = 0,
    B = 1,
    C = 2,
    D = 3,
    E = 4,
    G = 6,
    H = 7,
    I = 8,
}

fn gpio_base(p: Port) -> u32 {
    0x5802_0000 + 0x400 * p as u32
}

const IN: u32 = 0;
const OUT: u32 = 1;
const AF: u32 = 2;

/// Mode (input / output / alternate), very high speed (ULPI runs at 60 MHz), no pull, push-pull; alternate
/// function number for AF.
fn gpio(p: Port, pin: u32, mode: u32, af: u32) {
    let b = gpio_base(p);
    modify(b + 0x08, 3 << (2 * pin), 3 << (2 * pin)); // OSPEEDR: very high
    modify(b + 0x0c, 3 << (2 * pin), 0); // PUPDR: none
    modify(b + 0x04, 1 << pin, 0); // OTYPER: push-pull
    if mode == AF {
        let (reg, sh) = if pin < 8 { (b + 0x20, 4 * pin) } else { (b + 0x24, 4 * (pin - 8)) };
        modify(reg, 0xf << sh, af << sh);
    }
    modify(b, 3 << (2 * pin), mode << (2 * pin)); // MODER last
}

fn pin_set(p: Port, pin: u32, high: bool) {
    wr(gpio_base(p) + 0x18, if high { 1 << pin } else { 1 << (pin + 16) }); // BSRR
}

fn pin_low(p: Port, pin: u32) -> bool {
    rd(gpio_base(p) + 0x10) & (1 << pin) == 0 // IDR; the keys are active low
}

// Display pins (BOARD.md 6): PG3 display + touch power (inferred), PI3 panel reset release (inferred),
// PG9 D/C (0 command, 1 parameter), PG10 SPI1 NSS (driven by hand here), PG11 SPI1 SCK AF5, PD7 SPI1 MOSI AF5,
// PG13 backlight: LPTIM1_OUT (AF1), a PWM as in stock (BOARD.md 6); a plain output, low, when off.
const LCD_POWER: (Port, u32) = (Port::G, 3);
const LCD_RESET: (Port, u32) = (Port::I, 3);
const LCD_DC: (Port, u32) = (Port::G, 9);
const LCD_CS: (Port, u32) = (Port::G, 10);
const BACKLIGHT: (Port, u32) = (Port::G, 13);
// Keys (BOARD.md 5), active low, in stock's key code order: PA1 back / power (0x01), PB2 lap (0x02), PB4 start /
// stop (0x04), PE3 page (0x08). PA0 is the power button's standby wake-up.
const KEYS: [(Port, u32); 4] = [(Port::A, 1), (Port::B, 2), (Port::B, 4), (Port::E, 3)];
// USB power present (BOARD.md 7: VBUS detect is PC13, high when plugged in).
const VBUS: (Port, u32) = (Port::C, 13);

fn board_init() {
    // GPIOA..GPIOI clocks (AHB4ENR bits 0..8), SPI1 (APB2ENR bit 12), RTC APB (APB4ENR bit 16)
    modify(RCC_AHB4ENR, 0, 0x1ff);
    modify(RCC_APB4ENR, 0, 1 << 16);
    // per_ck = HSI (64 MHz): independent of the PLL outputs the bootloader left on or off (SPI1's slow clock: spi_init).
    modify(RCC_CR, 0, 1); // HSION
    let t0 = cycles();
    while rd(RCC_CR) & (1 << 2) == 0 && cycles().wrapping_sub(t0) < CPU_HZ / 100 {} // HSIRDY, 10 ms max
    modify(RCC_D1CCIPR, 3 << 28, 0); // CKPERSEL = hsi_ker_ck
    modify(RCC_APB2ENR, 0, 1 << 12);
    let _ = rd(RCC_APB2ENR);

    gpio(LCD_POWER.0, LCD_POWER.1, OUT, 0);
    gpio(LCD_RESET.0, LCD_RESET.1, OUT, 0);
    gpio(LCD_DC.0, LCD_DC.1, OUT, 0);
    pin_set(LCD_CS.0, LCD_CS.1, true);
    gpio(LCD_CS.0, LCD_CS.1, OUT, 0);
    pin_set(BACKLIGHT.0, BACKLIGHT.1, false);
    gpio(BACKLIGHT.0, BACKLIGHT.1, OUT, 0);
    gpio(Port::G, 11, AF, 5); // SPI1_SCK
    gpio(Port::D, 7, AF, 5); // SPI1_MOSI
    for (port, pin) in KEYS {
        // PB4 leaves reset as NJTRST (alternate function, pull-up, seen on the unit): an input with a pull-up. PA1,
        // PB2 and PE3 read high without one (external pull-ups); the internal one does no harm.
        gpio(port, pin, IN, 0);
        pull_up(port, pin);
    }
    // PA0 (WKUP1, power button) as an input, as stock: left in its reset analog mode, the press never woke the unit
    // from standby (seen on the unit, 2026-10-23; inferred: the wake-up logic reads after the input buffer).
    gpio(Port::A, 0, IN, 0);
    gpio(VBUS.0, VBUS.1, IN, 0);
}

// ---------------------------------------------------------------------------------------------------------------
// SPI1 (0x40013000, the STM32H7 SPI): master, transmit only, 8-bit, CPOL 1 / CPHA 1 like the stock setup
// (BOARD.md 6), software slave management (CS on PG10 by hand), 50 MHz from PLL1Q (8 MHz from the HSI without
// POWER_STEPS bit 3). Pixel runs go by DMA1 stream 0 (hw.rs PanelDma) unless POWER_STEPS bit 4 is clear.
// ---------------------------------------------------------------------------------------------------------------
const SPI1: u32 = 0x4001_3000;
const SPI_CR1: u32 = SPI1;
const SPI_CR2: u32 = SPI1 + 0x04;
const SPI_CFG1: u32 = SPI1 + 0x08;
const SPI_CFG2: u32 = SPI1 + 0x0c;
const SPI_SR: u32 = SPI1 + 0x14;
const SPI_IFCR: u32 = SPI1 + 0x18;
const SPI_TXDR: u32 = SPI1 + 0x20;
const SR_TXP: u32 = 1 << 1;
const SR_MODF: u32 = 1 << 9;
const CFG1_TXDMAEN: u32 = 1 << 15;
const SPI_TIMEOUT: u32 = CPU_HZ / 100; // 10 ms: a stuck SPI must not hang the loop
const RCC_PLLCFGR: u32 = RCC + 0x02c;
const RCC_AHB1ENR: u32 = RCC + 0x0d8;
const RCC_AHB1LPENR: u32 = RCC + 0x100;
const RCC_APB2LPENR: u32 = RCC + 0x118;
/// DMAMUX1 channel 0, which feeds DMA1 stream 0 (RM0399 DMAMUX: channels 0..7 are DMA1's streams 0..7).
const DMAMUX1_C0CR: u32 = 0x4002_0800;
/// DMAMUX1 request input 38: spi1_tx_dma (RM0399 table "DMAMUX1: assignment of multiplexer inputs to resources";
/// stm32h7xx_hal_dma.h DMA_REQUEST_SPI1_TX 38).
const SPI1_TX_REQ: u32 = 38;
/// DMA1 stream 0's interrupt line (RM0399 NVIC table: DMA1_Stream0 at position 11).
const DMA1_S0_IRQ: u32 = 1 << 11;
const DMA1_LISR: u32 = 0x4002_0000;
const DMA1_LIFCR: u32 = 0x4002_0008;
// LISR, stream 0: TCIF0 5, TEIF0 3, DMEIF0 2 (RM0399 DMA_LISR)
const TCIF0: u32 = 1 << 5;
const DMA_ERR0: u32 = (1 << 3) | (1 << 2);

/// SPI1 up; returns the panel's DMA when POWER_STEPS asks for it.
fn spi_init() -> Option<hw::PanelDma> {
    // SSI first: with SSM, MASTER set while SSI is 0 is a mode fault, which clears MASTER (seen on the unit,
    // diag v17: CFG2 read back without MASTER, so SPI1 took 16 bytes and never clocked; RM0399 SPI mode fault).
    wr(SPI_CR1, CR1_SSI);
    // Kernel clock: pll1_q_ck (SPI123SEL 0), 100 MHz (BOARD.md 1: PLL1 Q 8, the SDMMC's clock too, so it always
    // runs: no cost in keeping it), divided by 2 (MBR 0): 50 MHz. The ST7789's serial write cycle is 16 ns at least,
    // 62.5 MHz (ST7789V datasheet, "Serial interface characteristics", tSCYCW), and SPI1's master clock goes to
    // 80 MHz (DS12930 table 119). Without POWER_STEPS bit 3, or with PLL1's Q output off (DIVQ1EN, RCC_PLLCFGR bit
    // 17), per_ck (the HSI, 64 MHz: board_init): / 4 = 16 MHz with bit 3 (as before PLL1Q), / 8 = 8 MHz without.
    let fast = POWER_STEPS & SPI1_FAST != 0;
    let pll1q = fast && rd(RCC_PLLCFGR) & (1 << 17) != 0;
    modify(RCC_D2CCIP1R, 7 << 12, if pll1q { 0 } else { 4 << 12 }); // SPI123SEL
    let mbr = if pll1q {
        0
    } else if fast {
        1
    } else {
        2
    };
    // FTHLV = 4 frames (TXP then means room for a 32-bit write of 4 bytes: pixels go 2 per access), DSIZE = 8 bits
    wr(SPI_CFG1, (mbr << 28) | (3 << 5) | 7);
    // AFCNTR (keep pins driven), SSM, CPOL, CPHA, MASTER, COMM = simplex transmitter
    wr(SPI_CFG2, (1 << 31) | (1 << 26) | (1 << 25) | (1 << 24) | (1 << 22) | (1 << 17));
    if POWER_STEPS & PANEL_DMA == 0 {
        return None;
    }
    modify(RCC_AHB1ENR, 0, 1); // DMA1EN
    let _ = rd(RCC_AHB1ENR);
    core::hint::black_box(&DMA_FAIL); // kept in the image though only peeked
    let dma = hw::PanelDma::take()?; // stops whatever the bootloader left on stream 0
    wr(DMAMUX1_C0CR, SPI1_TX_REQ);
    wr(DMA1_LIFCR, 0x3d);
    wr(NVIC_ICPR0, DMA1_S0_IRQ);
    wr(NVIC_ISER0, DMA1_S0_IRQ); // its end pends the line, which wakes WFI (interrupts masked: no handler)
    Some(dma)
}

const CR1_SSI: u32 = 1 << 12; // with SSM: internal NSS high, no mode fault
const SR_EOT: u32 = 1 << 3;

/// Sets up a transfer of exactly `n` bytes (1..=0xffff), SPI1 disabled (TSIZE is only writable then).
fn spi_setup(n: u32) {
    wr(SPI_CR1, CR1_SSI); // SPE = 0
    wr(SPI_IFCR, 0xff8); // clear every event flag
    wr(SPI_CR2, n & 0xffff);
}

/// Enables SPI1 and starts the transfer set up (CSTART), as the stock HAL_SPI_Transmit does (BOARD.md 6).
fn spi_go() {
    wr(SPI_CR1, CR1_SSI | 1); // SPE
    wr(SPI_CR1, CR1_SSI | (1 << 9) | 1); // CSTART
}

/// Starts a transfer of exactly `n` bytes (1..=0xffff) fed by the CPU.
fn spi_begin(n: u32) {
    spi_setup(n);
    spi_go();
}

/// SPI waits that ran out (spi_byte, spi_end).
static SPI_TIMEOUTS: AtomicU32 = AtomicU32::new(0);

/// After a first timeout the SPI is taken as dead: later waits give up at once (still counted), so a stuck panel
/// costs nothing and the loop and the USB dev link keep running, so `tools/dev.py peek` can find out why (that is how
/// the SPI1 mode fault was found: docs/journal.md).
// ponytail: dead until reset, even after a one-off timeout; retry later if one ever happens on a working panel.
fn spi_wait(flag: u32) -> bool {
    let t0 = cycles();
    while rd(SPI_SR) & flag == 0 {
        if SPI_TIMEOUTS.load(Ordering::Relaxed) > 0 || cycles().wrapping_sub(t0) > SPI_TIMEOUT {
            SPI_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            return false;
        }
    }
    true
}

fn spi_byte(b: u8) {
    if !spi_wait(SR_TXP) {
        return;
    }
    wr8(SPI_TXDR, b); // 8-bit access: one frame
}

/// One 32-bit FIFO write: 4 frames (FTHLV is 4, so TXP means room for them).
fn spi_word(w: u32) {
    if spi_wait(SR_TXP) {
        wr(SPI_TXDR, w);
    }
}

/// Waits for the end of the transfer (EOT: the last byte is out), then disables SPI1 until the next one.
fn spi_end() {
    spi_wait(SR_EOT);
    wr(SPI_IFCR, 0xff8);
    wr(SPI_CR1, CR1_SSI);
}

/// A DMA wait's bound: the longest transfer, 0x7ffe pixels, takes 10.5 ms at 50 MHz (a first bound of 10 ms cut
/// full-screen fills short, seen on the unit 2026-10-27).
const DMA_TIMEOUT_MS: u32 = 30;

/// Sleeps until DMA1 stream 0 has sent its words: true, or false on a DMA error, a mode fault or a timeout.
/// WFI wakes on the stream's interrupt line (TCIE / TEIE / DMEIE: hw.rs PanelDma::start). The timeout: SysTick, when
/// nothing else runs it (the cycle counter stops in WFI); when the main loop's sleep runs it, its end wakes WFI, and
/// from then on its pending bit keeps WFI from sleeping, so the cycle counter bounds the wait.
/// Why the last DMA wait failed, for `tools/dev.py peek` (its address: `nm` on the ELF): 0x1000_0000 | DMA1_LISR on a DMA
/// error; 0x2000_0000 | own SysTick << 24 | LISR << 16 | SPI1_SR's low half on a timeout or a mode fault; 0 never.
static DMA_FAIL: AtomicU32 = AtomicU32::new(0);
fn dma_wait() -> bool {
    let own = !TIMER_ON.load(Ordering::Relaxed);
    if own {
        timer_start(DMA_TIMEOUT_MS);
    }
    // DMA1 and SPI1 keep their clocks while the core sleeps, for this wait only (sleep_power_init clears their LPEN
    // bits: DMA1 alone is 37 uA/MHz, DS12930 table 48)
    let lp = (rd(RCC_AHB1LPENR), rd(RCC_APB2LPENR));
    wr(RCC_AHB1LPENR, lp.0 | 1); // DMA1LPEN
    wr(RCC_APB2LPENR, lp.1 | (1 << 12)); // SPI1LPEN
    let t0 = cycles();
    let ok = loop {
        let isr = rd(DMA1_LISR);
        if isr & DMA_ERR0 != 0 {
            DMA_FAIL.store(0x1000_0000 | isr, Ordering::Relaxed);
            break false;
        }
        if isr & TCIF0 != 0 {
            break true;
        }
        let sr = rd(SPI_SR);
        if sr & SR_MODF != 0 || (own && timer_done()) || cycles().wrapping_sub(t0) > DMA_TIMEOUT_MS * (CPU_HZ / 1000) {
            DMA_FAIL.store(0x2000_0000 | (u32::from(own) << 24) | (sr & 0xffff) | (isr << 16), Ordering::Relaxed);
            break false;
        }
        sleep();
    };
    wr(DMA1_LIFCR, 0x3d);
    let _ = rd(DMA1_LISR); // the clear done before the next sleep's NVIC clear
    wr(RCC_AHB1LPENR, lp.0);
    wr(RCC_APB2LPENR, lp.1);
    if own {
        timer_stop();
    }
    ok
}

// ---------------------------------------------------------------------------------------------------------------
// Panel: 240 x 320, ST7789-class (BOARD.md 6), on SPI1. Only the byte transport is here; the command
// sequences are in panel.rs.
// ---------------------------------------------------------------------------------------------------------------
fn lcd_cmd(cmd: u8, params: &[u8]) {
    pin_set(LCD_CS.0, LCD_CS.1, false);
    pin_set(LCD_DC.0, LCD_DC.1, false);
    spi_begin(1);
    spi_byte(cmd);
    spi_end();
    pin_set(LCD_DC.0, LCD_DC.1, true);
    if !params.is_empty() {
        spi_begin(params.len() as u32);
        for &p in params {
            spi_byte(p);
        }
        spi_end();
    }
    pin_set(LCD_CS.0, LCD_CS.1, true);
}

/// Power and reset pulse (BOARD.md 6), before the first command.
fn lcd_power_on() {
    pin_set(LCD_POWER.0, LCD_POWER.1, true);
    delay_ms(20);
    pin_set(LCD_RESET.0, LCD_RESET.1, false);
    delay_ms(10);
    pin_set(LCD_RESET.0, LCD_RESET.1, true);
    delay_ms(120);
}

/// Two pixels as one FIFO word: the SPI sends the word's bytes from the lowest; the panel wants each pixel high
/// byte first.
#[inline(always)]
fn pixel_pair(a: u16, b: u16) -> u32 {
    u32::from(a.swap_bytes()) | (u32::from(b.swap_bytes()) << 16)
}

/// Pixels in a run under these go by the CPU: a blit's first buffer is filled before the DMA starts, so a short one
/// gains nothing; a fill's DMA set-up and wake-up cost about what 64 pixels take on the wire.
const DMA_MIN_BLIT: u32 = 2 * hw::DMA_WORDS as u32;
const DMA_MIN_FILL: u32 = 64;

/// What a pixel run is: a fill (one colour) or a blit (each pixel from a function).
enum Run<'a> {
    Fill(u16),
    Blit(&'a mut dyn FnMut() -> u16),
}

impl Run<'_> {
    #[inline(always)]
    fn pair(&mut self) -> u32 {
        match self {
            Run::Fill(c) => pixel_pair(*c, *c),
            Run::Blit(f) => {
                let a = f();
                pixel_pair(a, f())
            }
        }
    }
    #[inline(always)]
    fn one(&mut self) -> u16 {
        match self {
            Run::Fill(c) => *c,
            Run::Blit(f) => f(),
        }
    }
}

/// RAMWR then `count` pixels, two per 32-bit FIFO write, in transfers of at most 0x7ffe pixels (TSIZE is 16 bits,
/// 2 bytes a pixel). A transfer long enough goes by DMA when `dma` is there; a DMA failure finishes the transfer by
/// the CPU and drops the DMA for good.
fn lcd_ram(count: u32, dma: &mut Option<hw::PanelDma>, mut run: Run<'_>) {
    pin_set(LCD_CS.0, LCD_CS.1, false);
    pin_set(LCD_DC.0, LCD_DC.1, false);
    spi_begin(1);
    spi_byte(0x2c); // RAMWR
    spi_end();
    pin_set(LCD_DC.0, LCD_DC.1, true);
    let min = if matches!(run, Run::Fill(_)) { DMA_MIN_FILL } else { DMA_MIN_BLIT };
    let mut left = count;
    while left > 0 {
        let n = left.min(0x7ffe);
        left -= n;
        spi_setup(2 * n);
        let mut words = (n / 2) as usize;
        let by_dma = n >= min && dma.is_some();
        let mut stuck = false; // the stream would not stop: the CPU must not write TXDR either
        if let Some(d) = dma.as_mut().filter(|_| by_dma) {
            let r = dma_words(d, words, &mut run);
            stuck = r == Err(None);
            words = match r {
                Ok(()) | Err(None) => 0,
                Err(Some(w)) => w,
            };
            if r.is_err() {
                *dma = None; // dead until reset: the CPU from here on
                wr(NVIC_ICER0, DMA1_S0_IRQ); // its line no longer wakes WFI (the abort set TCIF: hw.rs PanelDma::stop)
                wr(NVIC_ICPR0, DMA1_S0_IRQ);
            }
        } else {
            spi_go();
        }
        for _ in 0..words {
            spi_word(run.pair());
        }
        if n % 2 == 1 && !stuck {
            let c = run.one();
            spi_byte((c >> 8) as u8);
            spi_byte(c as u8);
        }
        spi_end();
        if by_dma {
            modify(SPI_CFG1, CFG1_TXDMAEN, 0); // SPE is 0 again: CFG1 is writable
        }
    }
    pin_set(LCD_CS.0, LCD_CS.1, true);
}

/// Starts DMA1 stream 0 on buffer `i`, then, for a transfer's first start, TXDMAEN and SPE (RM0399 SPI
/// "Communication using DMA": the stream enabled, then TXDMAEN, then SPE; CFG1 is writable only while SPE is 0).
/// The SPI runs afterwards either way, so the CPU can finish a transfer the stream did not.
fn dma_go(d: &mut hw::PanelDma, i: usize, n: usize, repeat: bool) -> bool {
    let started = d.start(i, n, repeat);
    if rd(SPI_CR1) & 1 == 0 {
        modify(SPI_CFG1, 0, CFG1_TXDMAEN);
        spi_go();
    }
    started
}

/// `words` words of a transfer set up (spi_setup) sent by DMA: a fill as one repeat of its word, a blit
/// double-buffered (the CPU fills one buffer while the stream sends the other). On a failure, whatever the stream
/// had not sent of the words taken from `run` goes by the CPU, and Err gives the words of `run` still to send; Err(None)
/// when the stream will not stop: nothing more may be written to TXDR (the transfer is left short, so SPI1 times out
/// at its end and is taken as dead: spi_wait).
fn dma_words(d: &mut hw::PanelDma, words: usize, run: &mut Run<'_>) -> Result<(), Option<usize>> {
    if let Run::Fill(_) = run {
        let Some(b) = d.buf(0) else { return Err(Some(words)) };
        b[0] = run.pair();
        if !dma_go(d, 0, words, true) {
            return Err(Some(words));
        }
        return if dma_wait() { Ok(()) } else { Err(d.stop().map(|rest| rest.min(words))) }; // the same word, by the CPU
    }
    let mut left = words;
    let mut flight: Option<(usize, usize)> = None; // buffer, words in it
    let mut cur = 0;
    loop {
        // the next buffer, filled while the stream sends the one in flight
        let k = left.min(hw::DMA_WORDS);
        let ready = match d.buf(cur) {
            Some(b) if k > 0 => {
                for w in &mut b[..k] {
                    *w = run.pair();
                }
                left -= k;
                Some((cur, k))
            }
            _ => None,
        };
        if let Some((i, n)) = flight
            && !dma_wait()
        {
            let Some(rest) = d.stop().map(|rest| rest.min(n)) else { return Err(None) };
            if let Some(b) = d.buf(i) {
                b[n - rest..n].iter().for_each(|&w| spi_word(w));
            }
            if let Some((j, m)) = ready
                && let Some(b) = d.buf(j)
            {
                b[..m].iter().for_each(|&w| spi_word(w));
            }
            return Err(Some(left));
        }
        let Some((i, n)) = ready else {
            return if left == 0 { Ok(()) } else { Err(Some(left)) };
        };
        if !dma_go(d, i, n, false) {
            // not started (the stream is idle here, so not expected): these words by the CPU, and the rest
            if let Some(b) = d.buf(i) {
                b[..n].iter().for_each(|&w| spi_word(w));
            }
            return Err(Some(left));
        }
        flight = Some((i, n));
        cur ^= 1;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// RTC (0x58004000, BOARD.md 8), clocked by the LSE in the backup domain: it keeps the time the stock firmware
// set (from GPS) across resets and firmware changes. TR / DR are BCD; reading TR locks the shadow DR until DR is read.
// The stock firmware may keep UTC there and add the time zone itself (Clk_LocalTime): not traced.
// ---------------------------------------------------------------------------------------------------------------
const RTC_TR: u32 = 0x5800_4000;
const RTC_DR: u32 = 0x5800_4004;
const RTC_ICSR: u32 = 0x5800_400c;
/// RTC_WPR: the write protection key register (RM0399 RTC registers, offset 0x24).
const RTC_WPR: u32 = 0x5800_4024;
/// Backup registers 0..31 (RM0399 RTC_BKPxR; BOARD.md 8 lists those stock uses).
const RTC_BKP0R: u32 = 0x5800_4050;

/// RTC_TR then RTC_DR: reading TR locks the shadow DR until DR is read, so the pair is coherent.
fn rtc() -> (u32, u32) {
    let t0 = cycles();
    while rd(RTC_ICSR) & (1 << 5) == 0 && cycles().wrapping_sub(t0) < CPU_HZ / 100 {} // RSF
    let tr = rd(RTC_TR);
    (tr, rd(RTC_DR))
}

/// Sets the RTC (RM0399, RTC "Calendar initialization and configuration"): backup domain writable (DBP), the
/// write protection unlocked (0xca then 0x53 into RTC_WPR), INIT set in RTC_ISR (here RTC_ICSR, bit 7) and INITF
/// (bit 6) awaited, at most 10 ms (the manual gives up to two RTCCLK periods plus the synchronisation), TR and DR
/// written, INIT cleared, RSF (bit 5) cleared so the next read waits for the shadow registers, locked again. The
/// prescalers stay as stock set them. Not yet seen on the unit.
fn rtc_set(tr: u32, dr: u32) {
    modify(PWR_CR1, 0, 1 << 8);
    wr(RTC_WPR, 0xca);
    wr(RTC_WPR, 0x53);
    modify(RTC_ICSR, 0, 1 << 7);
    let t0 = cycles();
    while rd(RTC_ICSR) & (1 << 6) == 0 && cycles().wrapping_sub(t0) < CPU_HZ / 100 {}
    if rd(RTC_ICSR) & (1 << 6) != 0 {
        wr(RTC_TR, tr & 0x007f_7f7f);
        wr(RTC_DR, dr & 0x00ff_ff3f);
    }
    modify(RTC_ICSR, (1 << 7) | (1 << 5), 0);
    wr(RTC_WPR, 0xff);
}

// ---------------------------------------------------------------------------------------------------------------
// Power off = standby, as Pwr_StopModePeriphTransition(0) + HAL_PWREx_EnterSTANDBYMode(D2) and (D1) in stock
// (BOARD.md 9): wake-up pin WKUP1 (PA0, the power button), rising edge, no pull. Waking resets the unit:
// bootloader, then this firmware again.
//
// PDDS_D3 stays 0, as in stock: D1 and D2 power down but D3 only stops, so the system is in Stop, not Standby
// (STM32CubeH7 HAL, stm32h7xx_hal_pwr_ex.c, HAL_PWREx_EnterSTANDBYMode: "the whole system in STOP if
// PWR_CPUCR_PDDS_D3 = 0"; the system enters Standby only when D1, D2 and D3 all are in DStandby). D3 keeps the
// GPIO levels (Nordic PI4 / PA6, GNSS power, touch reset); in Standby they would float. In Stop the WKUP pins reach
// the core only through their EXTI lines 55..60 (same file, "Wakeup Pins configuration"), and EXTI_CPUIMR2 resets
// to 0 (ST's STM32H747_CM7 SVD), so stock unmasks line 55 (HAL_EXTI_D1_EventInputConfig(55, IT, 1) and IRQ 149,
// ghidra/recovered_src/system/board.c Pwr_StopModePeriphTransition). Without it the press woke nothing (seen on
// the unit, 2026-10-25: WFI never returned, PWR_CPUCR.SBF clear at the next boot).
// ---------------------------------------------------------------------------------------------------------------
const PWR: u32 = 0x5802_4800;
const PWR_CR1: u32 = PWR;
const PWR_CPUCR: u32 = PWR + 0x10;
const PWR_WKUPCR: u32 = PWR + 0x20;
const PWR_WKUPFR: u32 = PWR + 0x24;
const PWR_WKUPEPR: u32 = PWR + 0x28;
const EXTI_CPUIMR2: u32 = 0x5800_0090; // stm32h747xx.h: EXTI_D1 at 0x58000080, IMR2 at +0x10; line 55 = bit 23
const WKUP1_LINE: u32 = 1 << (55 - 32);
const WKUP_IRQ: u32 = 149; // stm32h747xx.h WAKEUP_PIN_IRQn

// Standby breadcrumbs (the power button does not start the unit after switching off, 2026-10-25: to find where it
// stops): bryton_rustfw::STANDBY_BKP (non-zero WFI returns: the core spun instead of sleeping) and, at the first
// boot after it, STANDBY_FLAGS_BKP; registers shared with the USB drop diagnostics, each word tagged (settings.rs).
const STANDBY_CRUMB: u32 = 0x5b00_0000;
const CRUMB: u32 = STANDBY_BKP as u32;
const RCC_RSR: u32 = RCC + 0x0d0; // stm32h747xx.h RCC_TypeDef: RSR at 0xD0; RMVF bit 16

fn crumb_read(n: u32) -> u32 {
    rd(RTC_BKP0R + 4 * n)
}

fn crumb_write(n: u32, v: u32) {
    modify(PWR_CR1, 0, 1 << 8); // DBP
    wr(RTC_BKP0R + 4 * n, v);
}

/// At boot: counts a boot after standby was entered, and at the first one keeps the reset flags.
fn standby_crumb_at_boot() {
    let c = crumb_read(CRUMB);
    if c >> 24 != STANDBY_CRUMB >> 24 {
        return;
    }
    let boots = (c >> 16) & 0xff;
    if boots == 0 {
        crumb_write(STANDBY_FLAGS_BKP as u32, standby_flags_word(rd(RCC_RSR), rd(PWR_CPUCR)));
    }
    crumb_write(CRUMB, (c & 0xff00_ffff) | (boots.saturating_add(1).min(0xff) << 16));
}

fn standby() -> ! {
    crumb_write(CRUMB, STANDBY_CRUMB);
    wr(PWR_WKUPEPR, 0); // all wake-up pins off while changing them
    wr(PWR_WKUPCR, 0x3f); // clear the wake-up flags
    wr(PWR_WKUPEPR, 1); // WKUPEN1, polarity rising (WKUPP1 = 0), no pull
    modify(PWR_CPUCR, 0, 1 << 9); // CSSF: clear the standby / stop flags
    // RMVF set then cleared (cleared too in case the flags stay held while it is set): the reset flags read at the
    // next boot are then only this switch-off's (they otherwise add up from the last power-on reset)
    modify(RCC_RSR, 0, 1 << 16);
    modify(RCC_RSR, 1 << 16, 0);
    modify(PWR_CPUCR, 0, (1 << 1) | 1); // PDDS_D2, PDDS_D1
    modify(0xe000_ed10, 0, 1 << 2); // SCB_SCR.SLEEPDEEP
    loop {
        // nothing pending may wake the WFI, or it returns, the core spins with the panel off and the power button
        // has no standby to wake from: the unit looks dead until the four-button restart and drains its battery
        // (seen on the unit, 2026-10-25: the touch line's EXTI and its NVIC wake-up, added 2026-10-23; reproduced
        // on Renode, renode/run.py --off). Cleared again at every return, so a wake-up source added later cannot
        // bring it back. Then only the power button is let back in.
        modify(EXTI_CPUIMR1, TOUCH_LINE | KEY_LINES, 0);
        wr(EXTI_CPUPR1, TOUCH_LINE | KEY_LINES);
        timer_stop();
        wr(SCB_ICSR, 1 << 25); // PENDSTCLR
        for k in 0..8 {
            wr(NVIC_ICER0 + 4 * k, u32::MAX);
            wr(NVIC_ICPR0 + 4 * k, u32::MAX);
        }
        // the power button's one way in: EXTI line 55 for this core, and its IRQ in the NVIC so that WFI returns
        // if D1 only stopped (interrupts stay masked: no handler runs)
        modify(EXTI_CPUIMR2, 0, WKUP1_LINE);
        wr(NVIC_ISER0 + 4 * (WKUP_IRQ / 32), 1 << (WKUP_IRQ % 32));
        barrier();
        wfi();
        let c = crumb_read(CRUMB); // WFI returned: not in standby
        crumb_write(CRUMB, (c & 0xffff_0000) | ((c & 0xffff) + 1).min(0xffff));
        if rd(PWR_WKUPFR) & 1 != 0 {
            system_reset(); // WKUPF1: the power button, so start
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Power while the core sleeps (WFI is CSleep: the CPU clock stops, every peripheral clock whose LPEN bit is set keeps
// running; RM0399 RCC, "Peripheral clock gating control"). The core is awake under 1 % of the time, so this is where
// the battery goes. Each step is a bit of POWER_STEPS (build time, default all), so a hang on the unit can be bisected:
//   POWER_STEPS=0 ./check.sh && ./dev.py run      the build as before these steps
// ---------------------------------------------------------------------------------------------------------------
/// Bit 0: idle peripherals and the flash interface clocked off in sleep. Bit 1: the GPIO ports too. Bit 2: PLL2 and
/// PLL3 off. Bit 3: SPI1 (the panel) at 50 MHz from PLL1Q instead of 8 from the HSI. Bit 4: the panel's pixel runs by DMA.
const POWER_STEPS: u32 = match option_env!("POWER_STEPS") {
    Some(s) => parse_u32(s),
    // bits 0..2 seen on the unit 2026-10-25 (keys, touch, backlight, USB drive, a ride, power button); 3 and 4 2026-10-27
    None => 0x1f,
};
const SLEEP_GATE: u32 = 1;
const SLEEP_GATE_GPIO: u32 = 2;
const PLLS_OFF: u32 = 4;
const SPI1_FAST: u32 = 8;
const PANEL_DMA: u32 = 16;

/// The sleep clock registers RCC_AHB3LPENR..RCC_APB4LPENR (RCC + 0xfc..0x11c: stm32h747xx.h `RCC_TypeDef`, AHB3 AHB1
/// AHB2 AHB4 APB3 APB1L APB1H APB2 APB4), with the bits kept: every other one is cleared (bit positions:
/// stm32h747xx.h `RCC_*LPENR_*_Pos`). At these addresses each core reaches its own copy (RM0399 RCC: for the
/// Cortex-M7, RCC_C1_xxxLPENR at RCC + 0x130 + 0x3c..), as the RCC_xxxENR writes in this file do (seen working on
/// the unit); Renode 1.17's RCC model overflows its stack on a read of the RCC_C1 copies.
/// A peripheral not enabled (xxxEN 0) has no clock either
/// way; clearing all but a keep list also stops whatever the bootloader left enabled and this firmware never uses
/// (DMA1 / DMA2 alone are 37 uA/MHz each at VOS3 on a 100 MHz AHB, DS12930 table 48). Kept, with why:
const SLEEP_KEEP: [(u32, u32); 9] = [
    // FMC 12 (the SDRAM's auto-refresh: the image and its data live there), SDMMC1 16 (an eMMC command may wait in a
    // sleep), DTCM1 28, DTCM2 29 (stack, .data, .bss), ITCM 30 (the RAM-run trampoline), AXISRAM 31 (the app).
    // Cleared: MDMA, DMA2D, JPEG, QSPI, and FLASH 8 (the image runs from SDRAM, VTOR points there: hw.rs; 12 uA/MHz,
    // 1.2 mA at 100 MHz).
    (0xfc, (1 << 12) | (1 << 16) | (0xf << 28)),
    // USB1 OTG HS 25 and its ULPI 26: the dev link and the drive are served from WFI wake-ups (BOARD.md 7).
    (0x100, (1 << 25) | (1 << 26)),
    // D2 SRAM1..3 29..31: memories, kept (nothing to gain that is worth a doubt).
    (0x104, 0x7 << 29),
    // GPIOA..K 0..10 unless SLEEP_GATE_GPIO (below), BKPRAM 28, SRAM4 29. Cleared: CRC, BDMA, ADC3 24 (a conversion
    // is polled to its end: adc_read).
    (0x108, (1 << 28) | (1 << 29)),
    // nothing: LTDC, DSI, WWDG1 (no watchdog runs: AGENTS.md)
    (0x10c, 0),
    // LPTIM1 9 (the backlight PWM), USART2 17 (the Nordic chip's bytes arrive while the core sleeps: its FIFO and RTS
    // hold them, BOARD.md 11). Cleared: I2C1 (touch, polled to its STOP: i2c_read).
    (0x110, (1 << 9) | (1 << 17)),
    // nothing
    (0x114, 0),
    // TIM1 0 (the buzzer plays through delay_ms). Cleared: SPI1 (spi_end waits for EOT), SPI4 (gnss_transfer
    // polled).
    (0x118, 1),
    // SYSCFG 1 (the EXTI line mux of the touch INT wake, BOARD.md 17), RTCAPB 16. Cleared: I2C4 (polled), the rest.
    (0x11c, (1 << 1) | (1 << 16)),
];

/// Applies the sleep steps of POWER_STEPS. Called once at boot, after every kernel clock this firmware uses has been
/// moved off PLL2 and PLL3 (spi_init, gnss_init, i2c_init, adc_init; nrf_reset and usb::start move theirs before
/// enabling them).
fn sleep_power_init() {
    if POWER_STEPS & SLEEP_GATE != 0 {
        for (off, keep) in SLEEP_KEEP {
            // the GPIO ports' clock only latches inputs (IDR) and serves register access; outputs, the alternate
            // functions and the EXTI lines go through without it (they work in Stop, where it is off: RM0399 GPIO,
            // EXTI). Its own bit, for a bisect: 0.6 to 1.1 uA/MHz per port at VOS3, 9 ports at 100 MHz: about 0.7 mA
            let keep = if off == 0x108 && POWER_STEPS & SLEEP_GATE_GPIO == 0 { keep | 0x7ff } else { keep };
            modify(RCC + off, !keep, 0);
        }
    }
    if POWER_STEPS & PLLS_OFF != 0 {
        // RM0399 RCC_CR: PLL2ON 26, PLL3ON 28. Nothing here runs from them (BOARD.md 1: SPI1, SPI4, USART2, I2C, the
        // ADC on the HSI, USB on HSI48, LPTIM1 on the LSE, SDMMC1 on PLL1Q). The FMC could (RCC_D1CCIPR.FMCSEL 2 =
        // pll2_r_ck, stm32h7xx_hal_rcc_ex.h RCC_FMCCLKSOURCE_PLL2), and the SDRAM must keep its clock: then PLL2 stays.
        // PLL3 feeds no FMC or SDMMC choice; the LTDC's pixel clock (pll3_r_ck) is not used here. DS12930 table 58:
        // 180 to 590 uA on VDDA plus 280 to 720 uA on VCORE per PLL, by VCO frequency.
        let d1 = rd(RCC_D1CCIPR);
        let pll2_needed = d1 & 3 == 2 || d1 & (1 << 16) != 0; // FMCSEL = PLL2R, or SDMMCSEL = PLL2R
        modify(RCC_CR, if pll2_needed { 0 } else { 1 << 26 } | (1 << 28), 0);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Nordic BLE / ANT chip (BOARD.md 11): USART2 at 1 Mbaud with RTS / CTS on PD3..PD6 (AF7, pull-up), reset PA6,
// PI4 high = its application (low through a reset would start its DFU bootloader). Kernel clock HSI 64 MHz, as
// SPI1, so it does not depend on the PLL3 the bootloader leaves running.
// ---------------------------------------------------------------------------------------------------------------
const USART2: u32 = 0x4000_4400;
const U_CR1: u32 = USART2;
const U_CR3: u32 = USART2 + 0x08;
const U_BRR: u32 = USART2 + 0x0c;
const U_ISR: u32 = USART2 + 0x1c;
const U_RQR: u32 = USART2 + 0x18;
const U_ICR: u32 = USART2 + 0x20;
const U_RDR: u32 = USART2 + 0x24;
const U_TDR: u32 = USART2 + 0x28;
const ISR_RXFNE: u32 = 1 << 5;
const ISR_TXFNF: u32 = 1 << 7;
const RCC_D2CCIP2R: u32 = RCC + 0x054;
const RCC_APB1LENR: u32 = RCC + 0x0e8;
const NRF_RESET: (Port, u32) = (Port::A, 6);
const NRF_APP: (Port, u32) = (Port::I, 4);

fn pull_up(p: Port, pin: u32) {
    modify(gpio_base(p) + 0x0c, 3 << (2 * pin), 1 << (2 * pin)); // PUPDR: pull-up
}

/// Stock's sequence (BOARD.md 11): application mode, reset held while the UART comes up, release, 800 ms.
fn nrf_reset() {
    modify(RCC_D2CCIP2R, 7, 3); // USART234578SEL = hsi_ker_ck (HSI is on: spi_init)
    modify(RCC_APB1LENR, 0, 1 << 17); // USART2EN
    for pin in 3..=6 {
        gpio(Port::D, pin, AF, 7); // CTS, RTS, TX, RX
        pull_up(Port::D, pin);
    }
    pin_set(NRF_APP.0, NRF_APP.1, true);
    gpio(NRF_APP.0, NRF_APP.1, OUT, 0);
    pull_up(NRF_APP.0, NRF_APP.1);
    pin_set(NRF_RESET.0, NRF_RESET.1, false);
    gpio(NRF_RESET.0, NRF_RESET.1, OUT, 0);
    pull_up(NRF_RESET.0, NRF_RESET.1);
    wr(U_CR1, 0);
    wr(U_BRR, 64); // 64 MHz / 1 Mbaud
    wr(U_CR3, (1 << 9) | (1 << 8)); // CTSE, RTSE
    // FIFOEN, RXFNEIE (a byte pends IRQ 38, enabled in the NVIC only for an idle sleep: wake_on_io), TE, RE, UE
    wr(U_CR1, (1 << 29) | (1 << 5) | (1 << 3) | (1 << 2) | 1);
    delay_ms(150);
    pin_set(NRF_RESET.0, NRF_RESET.1, true);
    delay_ms(800);
    while rd(U_ISR) & ISR_RXFNE != 0 {
        let _ = rd(U_RDR); // a glitch byte at the release (seen on the unit)
    }
    wr(U_ICR, 0xf); // PE, FE, NE, ORE
}

fn nrf_send(data: &[u8]) {
    for &b in data {
        let t0 = cycles();
        while rd(U_ISR) & ISR_TXFNF == 0 {
            if cycles().wrapping_sub(t0) > CPU_HZ / 100 {
                return; // 10 ms: the chip holds CTS (not running); the command will time out
            }
        }
        wr(U_TDR, u32::from(b));
    }
}

/// Reads what has arrived, and while bytes keep coming waits up to 200 us for the next one: reading frees the FIFO,
/// RTS lets the chip go on (16 bytes take 160 us at 1 Mbaud), so a burst of events comes in one call instead of 16
/// bytes per main loop pass. The wait is bounded by `buf.len()` bytes; an idle link returns at once.
fn nrf_recv(buf: &mut [u8]) -> usize {
    let mut n = 0;
    let mut last = cycles();
    while n < buf.len() {
        if rd(U_ISR) & ISR_RXFNE != 0 {
            buf[n] = rd(U_RDR) as u8;
            n += 1;
            last = cycles();
        } else if n == 0 || cycles().wrapping_sub(last) > CPU_HZ / 5000 {
            break;
        }
    }
    wr(U_ICR, 0xf); // an overrun cannot happen with RTS, but never let an error flag stop reception
    n
}

// ---------------------------------------------------------------------------------------------------------------
// Backlight PWM: LPTIM1 (0x40002400, RM0399 LPTIM) on the LSE (32.768 kHz), as stock's Pwr_BacklightPwmInit
// (BOARD.md 6): ARR 99, output polarity inverted (WAVPOL), CMP = level - 1, so PG13 is high for level % of each
// 3.05 ms period (327.68 Hz). The LSE keeps running in stop mode, and so does this PWM.
// ---------------------------------------------------------------------------------------------------------------
const LPTIM1: u32 = 0x4000_2400;
const LPTIM_ISR: u32 = LPTIM1;
const LPTIM_ICR: u32 = LPTIM1 + 0x04;
const LPTIM_CFGR: u32 = LPTIM1 + 0x0c;
const LPTIM_CR: u32 = LPTIM1 + 0x10;
const LPTIM_CMP: u32 = LPTIM1 + 0x14;
const LPTIM_ARR: u32 = LPTIM1 + 0x18;

fn backlight_init() {
    modify(RCC_D2CCIP2R, 7 << 28, 3 << 28); // LPTIM1SEL = LSE
    modify(RCC_APB1LENR, 0, 1 << 9); // LPTIM1EN
    wr(LPTIM_CR, 0);
    wr(LPTIM_CFGR, 1 << 21); // WAVPOL: inverted; internal clock, no prescaler, registers written at once
    wr(LPTIM_CR, 1); // ENABLE: ARR and CMP can only be written enabled
    wr(LPTIM_ICR, (1 << 4) | (1 << 3));
    wr(LPTIM_ARR, 99);
    let _ = wait_us(1000, || rd(LPTIM_ISR) & (1 << 4) != 0); // ARROK: the write crosses into the LSE domain
    wr(LPTIM_CR, (1 << 2) | 1); // CNTSTRT: continuous
}

/// Backlight at `level` % (0: off, the pin a plain low output).
fn backlight(level: u8) {
    if level == 0 {
        pin_set(BACKLIGHT.0, BACKLIGHT.1, false);
        gpio(BACKLIGHT.0, BACKLIGHT.1, OUT, 0);
        return;
    }
    wr(LPTIM_ICR, 1 << 3);
    // CMP below ARR (99): RM0399's LPTIM wants ARR greater than CMP (inferred from its register notes; Renode's model
    // refuses CMP = ARR and warned at every tick), so 100 % is 99 %: a step no eye sees
    wr(LPTIM_CMP, u32::from(level.min(99)) - 1);
    let _ = wait_us(1000, || rd(LPTIM_ISR) & (1 << 3) != 0); // CMPOK
    gpio(BACKLIGHT.0, BACKLIGHT.1, AF, 1); // LPTIM1_OUT
}

// ---------------------------------------------------------------------------------------------------------------
// Buzzer: TIM1 channel 3 PWM on PA10 (AF1), amplifier enable on PH2, as stock (Buzzer_PwmInit 0xd018b3f0, BOARD.md 14):
// a square wave (pulse = half the period) at the tone's frequency.
// ---------------------------------------------------------------------------------------------------------------
const TIM1: u32 = 0x4001_0000;
const BUZZER_AMP: (Port, u32) = (Port::H, 2);
const RCC_D1CFGR: u32 = RCC + 0x018;
const RCC_D2CFGR: u32 = RCC + 0x01c;

/// TIM1's input clock: the core clock through the AHB (D1CFGR.HPRE) and APB2 (D2CFGR.D2PPRE2) prescalers, doubled
/// when APB2 is divided (RM0399, timer clocks), read from the registers the bootloader left rather than assumed.
fn tim1_hz() -> u32 {
    let hpre = rd(RCC_D1CFGR) & 0xf;
    let ahb = if hpre < 8 { CPU_HZ } else { CPU_HZ >> (hpre - 7).min(9) };
    let ppre2 = (rd(RCC_D2CFGR) >> 8) & 7;
    if ppre2 < 4 { ahb } else { (ahb >> (ppre2 - 3)) * 2 }
}

fn buzzer_init() {
    modify(RCC_APB2ENR, 0, 1); // TIM1EN
    pin_set(BUZZER_AMP.0, BUZZER_AMP.1, false);
    gpio(BUZZER_AMP.0, BUZZER_AMP.1, OUT, 0);
    pin_set(Port::A, 10, false);
    gpio(Port::A, 10, OUT, 0); // quiet until a tone: a plain low output
}

/// A tone at `hz` (0: silence).
fn beep(hz: u32) {
    if hz == 0 {
        wr(TIM1, 0); // CR1: stop
        gpio(Port::A, 10, OUT, 0);
        pin_set(BUZZER_AMP.0, BUZZER_AMP.1, false);
        return;
    }
    let clk = tim1_hz();
    let psc = clk / (hz * 65_536); // smallest prescaler that keeps the period in 16 bits
    let arr = (clk / (psc + 1) / hz).clamp(2, 65_536) - 1;
    wr(TIM1, 0);
    wr(TIM1 + 0x28, psc); // PSC
    wr(TIM1 + 0x2c, arr); // ARR
    wr(TIM1 + 0x3c, arr.div_ceil(2)); // CCR3: half the period of arr + 1 ticks
    wr(TIM1 + 0x1c, (6 << 4) | (1 << 3)); // CCMR2: OC3M = PWM mode 1, preload
    wr(TIM1 + 0x20, 1 << 8); // CCER: CC3E
    wr(TIM1 + 0x44, 1 << 15); // BDTR: MOE (an advanced timer's outputs stay off without it)
    wr(TIM1 + 0x14, 1); // EGR: UG, load the registers
    wr(TIM1, (1 << 7) | 1); // CR1: ARPE, CEN
    gpio(Port::A, 10, AF, 1); // TIM1_CH3
    pin_set(BUZZER_AMP.0, BUZZER_AMP.1, true);
}

// ---------------------------------------------------------------------------------------------------------------
// Battery: ADC3 (0x58026000, RM0399 ADC; BOARD.md 12), as stock reads it: VREFINT (channel 19) to know VDDA, then
// the battery through a divider on PC3_C (channel 1), both with the longest sampling time. Kernel clock per_ck (HSI
// 64 MHz) / 16 = 4 MHz, under every BOOST limit, so BOOST stays 0. 16-bit (the reset resolution).
// ---------------------------------------------------------------------------------------------------------------
const ADC3: u32 = 0x5802_6000;
const ADC_ISR: u32 = ADC3;
const ADC_CR: u32 = ADC3 + 0x08;
const ADC_SMPR1: u32 = ADC3 + 0x14;
const ADC_SMPR2: u32 = ADC3 + 0x18;
const ADC_PCSEL: u32 = ADC3 + 0x1c;
const ADC_SQR1: u32 = ADC3 + 0x30;
const ADC_DR: u32 = ADC3 + 0x40;
const ADC3_CCR: u32 = ADC3 + 0x308;
const RCC_D3CCIPR: u32 = RCC + 0x058;
const RCC_AHB4RSTR: u32 = RCC + 0x088;
const SYSCFG_PMCR: u32 = 0x5800_0404;
/// VREFINT_CAL: the VREFINT reading at VDDA = 3.3 V, 16-bit, written at the factory (RM0399, device signature).
const VREFINT_CAL: u32 = 0x1ff1_e860;

/// Waits up to `us` microseconds for `done`; false on time-out.
fn wait_us(us: u32, mut done: impl FnMut() -> bool) -> bool {
    let t0 = cycles();
    while !done() {
        if cycles().wrapping_sub(t0) > us * (CPU_HZ / 1_000_000) {
            return false;
        }
    }
    true
}

/// Powers ADC3 up, calibrates and enables it; false if a step timed out (the battery then reads as unknown).
fn adc_init() -> bool {
    modify(RCC_D3CCIPR, 3 << 16, 2 << 16); // ADCSEL = per_ck
    modify(RCC_AHB4ENR, 0, 1 << 24); // ADC3EN
    // from reset: a RAM run (tools/dev.py run) inherits an enabled ADC, whose configuration can't be written
    modify(RCC_AHB4RSTR, 0, 1 << 24);
    modify(RCC_AHB4RSTR, 1 << 24, 0);
    modify(RCC_APB4ENR, 0, 1 << 1); // SYSCFGEN
    modify(SYSCFG_PMCR, 1 << 27, 0); // PC3SO = 0: PC3_C connected to the ADC (it reads 0 already after stock)
    modify(ADC3_CCR, (0xf << 18) | (3 << 16), (7 << 18) | (1 << 22)); // PRESC /16, async clock, VREFEN
    modify(ADC_CR, 1 << 29, 1 << 28); // DEEPPWD off, ADVREGEN on
    if !wait_us(100, || rd(ADC_ISR) & (1 << 12) != 0) {
        return false; // LDORDY
    }
    // channels before the calibration (once enabled, the ADC no longer takes them)
    wr(ADC_PCSEL, (1 << 1) | (1 << 19));
    modify(ADC_SMPR1, 7 << 3, 7 << 3); // channel 1: 810.5 cycles
    modify(ADC_SMPR2, 7 << 27, 7 << 27); // channel 19: 810.5 cycles
    modify(ADC_CR, 1 << 30, (1 << 31) | (1 << 16)); // ADCAL, linearity too (ADCALLIN), single ended
    if !wait_us(200_000, || rd(ADC_CR) & (1 << 31) == 0) {
        return false; // it takes 82 ms (seen on the unit), most of it the linearity part
    }
    // ADEN: set until the ADC reports ready (RM0399: ignored for a few ADC clock cycles after ADCAL clears)
    wr(ADC_ISR, 1); // ADRDY cleared
    wait_us(10_000, || {
        modify(ADC_CR, 0, 1); // ADEN
        rd(ADC_ISR) & 1 != 0
    })
}

/// One conversion of channel `ch` (about 205 us).
fn adc_read(ch: u32) -> Option<u16> {
    if rd(ADC_CR) & 1 == 0 {
        return None; // not enabled: adc_init failed
    }
    wr(ADC_SQR1, ch << 6); // one conversion, SQ1 = ch
    wr(ADC_ISR, 1 << 2); // EOC cleared
    modify(ADC_CR, 0, 1 << 2); // ADSTART
    wait_us(2000, || rd(ADC_ISR) & (1 << 2) != 0).then(|| rd(ADC_DR) as u16)
}

// ---------------------------------------------------------------------------------------------------------------
// GNSS: a u-blox M10 on SPI4 (0x40013400, BOARD.md 13), power on PI6. SPI4 as SPI1 (RM0399 SPI): master, mode 0,
// software chip select (PE4 by hand), kernel clock HSI 64 MHz / 16 = 4 MHz (stock: 1 MHz; u-blox receivers take
// up to 5.5 MHz, inferred from their datasheets, and this one answers at 4: seen on the unit). Transfers move 4 bytes
// per FIFO access (32-bit writes and reads pack four 8-bit frames).
// ---------------------------------------------------------------------------------------------------------------
const SPI4: u32 = 0x4001_3400;
const RCC_APB2RSTR: u32 = RCC + 0x098;
const GNSS_POWER: (Port, u32) = (Port::I, 6);
const GNSS_CS: (Port, u32) = (Port::E, 4);

fn gnss_init() {
    modify(RCC_APB2ENR, 0, 1 << 13); // SPI4EN
    modify(RCC_APB2RSTR, 0, 1 << 13); // from reset: a RAM run finds it as the last image left it
    modify(RCC_APB2RSTR, 1 << 13, 0);
    modify(RCC_D2CCIP1R, 7 << 16, 3 << 16); // SPI45SEL = hsi_ker_ck
    for pin in [2, 5, 6] {
        gpio(Port::E, pin, AF, 5); // SCK, MISO, MOSI
    }
    modify(gpio_base(Port::E) + 0x0c, 3 << 4, 2 << 4); // PE2 (SCK) pull-down, as stock
    pin_set(GNSS_CS.0, GNSS_CS.1, true);
    gpio(GNSS_CS.0, GNSS_CS.1, OUT, 0);
    pin_set(GNSS_POWER.0, GNSS_POWER.1, false);
    gpio(GNSS_POWER.0, GNSS_POWER.1, OUT, 0);
    wr(SPI4 + 0x08, (3 << 28) | (3 << 5) | 7); // CFG1: /16 = 4 MHz, FIFO threshold 4 frames, 8-bit
    wr(SPI4, 1 << 12); // CR1.SSI before CFG2.MASTER, or a mode fault clears MASTER (BOARD.md 6)
    wr(SPI4 + 0x0c, (1 << 31) | (1 << 26) | (1 << 22)); // CFG2: AFCNTR, SSM, MASTER; mode 0
    wr(SPI4 + 0x04, 0); // CR2: TSIZE 0, endless
    wr(SPI4, (1 << 12) | 1); // SPE
    wr(SPI4, (1 << 12) | (1 << 9) | 1); // CSTART
}

/// Exchanges `buf` with the receiver in place (its length a multiple of 4); gives up on a stuck SPI (0xff back).
fn gnss_transfer(buf: &mut [u8]) {
    pin_set(GNSS_CS.0, GNSS_CS.1, false);
    for chunk in buf.chunks_exact_mut(4) {
        let out = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let ok = wait_us(100, || rd(SPI4 + 0x14) & (1 << 1) != 0) && {
            wr(SPI4 + 0x20, out); // TXDR
            wait_us(100, || rd(SPI4 + 0x14) & 1 != 0) // RXP: 32 bits at 4 MHz take 8 us
        };
        let got = if ok { rd(SPI4 + 0x30) } else { u32::MAX }; // RXDR
        chunk.copy_from_slice(&got.to_le_bytes());
    }
    pin_set(GNSS_CS.0, GNSS_CS.1, true);
}

// ---------------------------------------------------------------------------------------------------------------
// eMMC: SDMMC1 (0x52007000, RM0399 SDMMC; BOARD.md 15), polled through the FIFO as stock does (no IDMA). Kernel
// clock PLL1Q, 100 MHz (BOARD.md 1), as stock; bus clock 100 MHz / (2 x CLKDIV): 400 kHz for identification, 25 MHz
// after. Falling-edge output and hardware flow control as stock (flow control stops the bus clock while the FIFO
// is full, so a slow FIFO loop cannot overrun). Only the transport is here; the command sequence is emmc.rs.
// ---------------------------------------------------------------------------------------------------------------
const SDMMC1: u32 = 0x5200_7000;
const SD_POWER: u32 = SDMMC1;
const SD_CLKCR: u32 = SDMMC1 + 0x04;
const SD_ARG: u32 = SDMMC1 + 0x08;
const SD_CMD: u32 = SDMMC1 + 0x0c;
const SD_RESP1: u32 = SDMMC1 + 0x14;
const SD_DTIMER: u32 = SDMMC1 + 0x24;
const SD_DLEN: u32 = SDMMC1 + 0x28;
const SD_DCTRL: u32 = SDMMC1 + 0x2c;
const SD_STA: u32 = SDMMC1 + 0x34;
const SD_ICR: u32 = SDMMC1 + 0x38;
const SD_FIFO: u32 = SDMMC1 + 0x80;
const RCC_AHB3RSTR: u32 = RCC + 0x07c;
const RCC_AHB3ENR: u32 = RCC + 0x0d4;
// STA flags (RM0399)
const STA_CCRCFAIL: u32 = 1 << 0;
const STA_DCRCFAIL: u32 = 1 << 1;
const STA_CTIMEOUT: u32 = 1 << 2;
const STA_DTIMEOUT: u32 = 1 << 3;
const STA_TXUNDERR: u32 = 1 << 4;
const STA_RXOVERR: u32 = 1 << 5;
const STA_CMDREND: u32 = 1 << 6;
const STA_CMDSENT: u32 = 1 << 7;
const STA_DATAEND: u32 = 1 << 8;
const STA_TXFIFOF: u32 = 1 << 16;
const STA_RXFIFOE: u32 = 1 << 19;
/// Every static flag (stock's clear value).
const ICR_ALL: u32 = 0x1fe0_0fff;

fn emmc_clkcr(fast: bool, wide: bool) -> u32 {
    let div = if fast { 2 } else { 125 };
    // PWRSAV (clock only while the bus is busy) at 25 MHz only: the card needs a running clock to identify
    let pwrsav = if fast { 1 << 12 } else { 0 };
    div | pwrsav | (u32::from(wide) << 14) | (1 << 16) | (1 << 17) // WIDBUS, NEGEDGE, HWFC_EN
}

fn emmc_power_on() {
    modify(RCC_D1CCIPR, 1 << 16, 0); // SDMMCSEL = pll1_q_ck, as stock
    modify(RCC_AHB3ENR, 0, 1 << 16); // SDMMC1EN
    // from reset: a RAM run, or an earlier failed init, may have left the controller mid-transfer
    modify(RCC_AHB3RSTR, 0, 1 << 16);
    modify(RCC_AHB3RSTR, 1 << 16, 0);
    for pin in 8..=12 {
        gpio(Port::C, pin, AF, 12); // D0..D3, CK
    }
    gpio(Port::D, 2, AF, 12); // CMD
    wr(SD_CLKCR, emmc_clkcr(false, false));
    wr(SD_POWER, 3); // PWRCTRL: on
    delay_ms(2); // the card wants 1 ms and 74 clocks (185 us at 400 kHz) before CMD0 (JEDEC)
}

fn emmc_cmd(index: u8, arg: u32, resp: bryton_rustfw::emmc::Resp, data: bool) -> Result<[u32; 4], BusError> {
    use bryton_rustfw::emmc::Resp;
    wr(SD_ICR, ICR_ALL);
    wr(SD_ARG, arg);
    let waitresp = match resp {
        Resp::None => 0,
        Resp::R1 | Resp::R3 => 1,
        Resp::R2 => 3,
    };
    // CMDTRANS: this command starts the data path; CMDSTOP for CMD12 (RM0399: the stop command of a transfer)
    let flags = (u32::from(data) << 6) | (u32::from(index == 12) << 7);
    wr(SD_CMD, u32::from(index & 0x3f) | flags | (waitresp << 8) | (1 << 12)); // CPSMEN
    let done = if resp == Resp::None { STA_CMDSENT } else { STA_CMDREND | STA_CCRCFAIL | STA_CTIMEOUT };
    // the card answers within 64 bus clocks (CTIMEOUT: 160 us at 400 kHz); 10 ms covers a missing kernel clock
    if !wait_us(10_000, || rd(SD_STA) & done != 0) {
        return Err(BusError::Timeout);
    }
    let sta = rd(SD_STA);
    if sta & STA_CTIMEOUT != 0 {
        return Err(BusError::Timeout);
    }
    if sta & STA_CCRCFAIL != 0 && resp != Resp::R3 {
        return Err(BusError::Crc); // R3 has no CRC: the flag is expected there
    }
    Ok([rd(SD_RESP1), rd(SD_RESP1 + 4), rd(SD_RESP1 + 8), rd(SD_RESP1 + 12)])
}

fn emmc_read(index: u8, arg: u32, buf: &mut [u8]) -> Result<u32, BusError> {
    let len = buf.len() as u32;
    if len == 0 || !len.is_multiple_of(512) || len >= 1 << 25 {
        return Err(BusError::Overrun); // not whole blocks (emmc.rs never asks that)
    }
    wr(SD_DCTRL, 0);
    wr(SD_DTIMER, 2_500_000); // 100 ms at 25 MHz for each block to start
    wr(SD_DLEN, len);
    wr(SD_DCTRL, (9 << 4) | (1 << 1)); // 512-byte blocks, card to host, block mode; CMDTRANS starts it
    let r1 = match emmc_cmd(index, arg, bryton_rustfw::emmc::Resp::R1, true) {
        Ok(r) => r[0],
        Err(e) => {
            wr(SD_DCTRL, 0);
            return Err(e);
        }
    };
    let limit = (100 + len / 512).min(10_000) * (CPU_HZ / 1000); // 100 ms + 1 ms a block
    let t0 = cycles();
    let mut words = buf.chunks_exact_mut(4);
    let mut next = words.next();
    let result = loop {
        let sta = rd(SD_STA);
        if sta & STA_DCRCFAIL != 0 {
            break Err(BusError::Crc);
        } else if sta & STA_DTIMEOUT != 0 {
            break Err(BusError::Timeout);
        } else if sta & STA_RXOVERR != 0 {
            break Err(BusError::Overrun);
        }
        if sta & STA_RXFIFOE == 0
            && let Some(w) = next
        {
            w.copy_from_slice(&rd(SD_FIFO).to_le_bytes()); // the first byte received is the word's lowest
            next = words.next();
            continue;
        }
        if sta & STA_DATAEND != 0 && next.is_none() {
            break Ok(r1);
        }
        if cycles().wrapping_sub(t0) > limit {
            break Err(BusError::Timeout);
        }
    };
    wr(SD_DCTRL, 0);
    wr(SD_ICR, ICR_ALL);
    result
}

/// A write (CMD24 / CMD25), mirroring [`emmc_read`]: the data path set card-bound (DCTRL DTDIR 0), the command
/// with CMDTRANS, then the FIFO fed a word at a time while TXFIFOF is clear until every word is in and DATAEND
/// (RM0399 SDMMC data path; ST's HAL_MMC_WriteBlocks feeds it on TXFIFOHE and stops on TXUNDERR, DCRCFAIL, DTIMEOUT
/// or DATAEND, the same flags). Hardware flow control holds the bus clock while the FIFO is empty, so a slow loop
/// cannot underrun. DCRCFAIL here is the card's CRC status token saying the block came in damaged. The card's
/// programming busy after DATAEND is emmc.rs's (CMD13). **Not yet seen on the unit** (docs/status.md).
fn emmc_write(index: u8, arg: u32, buf: &[u8]) -> Result<u32, BusError> {
    let len = buf.len() as u32;
    if len == 0 || !len.is_multiple_of(512) || len >= 1 << 25 {
        return Err(BusError::Overrun); // not whole blocks (emmc.rs never asks that)
    }
    wr(SD_DCTRL, 0);
    wr(SD_DTIMER, 2_500_000); // 100 ms at 25 MHz: the card's CRC status and busy between blocks
    wr(SD_DLEN, len);
    wr(SD_DCTRL, 9 << 4); // 512-byte blocks, host to card, block mode; CMDTRANS starts it
    let r1 = match emmc_cmd(index, arg, bryton_rustfw::emmc::Resp::R1, true) {
        Ok(r) => r[0],
        Err(e) => {
            wr(SD_DCTRL, 0);
            return Err(e);
        }
    };
    let limit = (100 + len / 512).min(10_000) * (CPU_HZ / 1000); // 100 ms + 1 ms a block
    let t0 = cycles();
    let mut words = buf.chunks_exact(4);
    let mut next = words.next();
    let result = loop {
        let sta = rd(SD_STA);
        if sta & STA_DCRCFAIL != 0 {
            break Err(BusError::Crc);
        } else if sta & STA_DTIMEOUT != 0 {
            break Err(BusError::Timeout);
        } else if sta & STA_TXUNDERR != 0 {
            break Err(BusError::Overrun);
        }
        if sta & STA_TXFIFOF == 0
            && let Some(w) = next
        {
            wr(SD_FIFO, u32::from_le_bytes([w[0], w[1], w[2], w[3]])); // the word's lowest byte goes out first
            next = words.next();
            continue;
        }
        if sta & STA_DATAEND != 0 && next.is_none() {
            break Ok(r1);
        }
        if cycles().wrapping_sub(t0) > limit {
            break Err(BusError::Timeout);
        }
    };
    wr(SD_DCTRL, 0);
    wr(SD_ICR, ICR_ALL);
    result
}

// Sensor bus: I2C4 (0x58001c00, D3 domain; BOARD.md 16), PD12 SCL / PD13 SDA, AF4, open drain, no pull (the bus has
// its own pull-ups, inferred: stock sets none). Touch: I2C1 (0x40005400, BOARD.md 17), PB6 SCL / PB7 SDA, the same
// way. Kernel clock HSI 64 MHz (I2C4SEL = 2, I2C123SEL = 2), like the other drivers here, so it does not depend on the
// APB dividers the bootloader leaves. Polled, every wait bounded. Registers: RM0399 I2C (the same block at each base).
// ---------------------------------------------------------------------------------------------------------------
const I2C4: u32 = 0x5800_1c00;
const I2C1: u32 = 0x4000_5400;
const I2C_CR1: u32 = 0x00;
const I2C_CR2: u32 = 0x04;
const I2C_TIMINGR: u32 = 0x10;
const I2C_ISR: u32 = 0x18;
const I2C_ICR: u32 = 0x1c;
const I2C_RXDR: u32 = 0x24;
const I2C_TXDR: u32 = 0x28;
const ISR_TXIS: u32 = 1 << 1;
const ISR_RXNE: u32 = 1 << 2;
const ISR_NACKF: u32 = 1 << 4;
const ISR_STOPF: u32 = 1 << 5;
const ISR_TC: u32 = 1 << 6;
const ISR_ERRORS: u32 = (1 << 8) | (1 << 9); // BERR, ARLO
const CR2_RD: u32 = 1 << 10;
const CR2_START: u32 = 1 << 13;
const CR2_AUTOEND: u32 = 1 << 25;
const RCC_APB4RSTR: u32 = RCC + 0x09c;
const RCC_APB1LRSTR: u32 = RCC + 0x090;
// Touch controller pins (BOARD.md 17): reset PH3 (high: run), INT PC6 (low: a point to read). Its falling edge
// (EXTI6) wakes WFI: interrupts stay masked, so no handler runs; the main loop reads the pending flag and clears it.
// Addresses from ST's CMSIS header stm32h747xx.h: EXTI at 0x58000000 (FTSR1 +0x04, the CPU's IMR1 +0x80, PR1
// +0x88), SYSCFG EXTICR[1] (lines 4..7) at 0x5800040c, RCC_APB4ENR SYSCFGEN bit 1, EXTI9_5 IRQ 23.
const TOUCH_RESET: (Port, u32) = (Port::H, 3);
const TOUCH_INT: (Port, u32) = (Port::C, 6);
const TOUCH_ADDR: u8 = 0x55;
const EXTI_FTSR1: u32 = 0x5800_0004;
const EXTI_CPUIMR1: u32 = 0x5800_0080;
const EXTI_CPUPR1: u32 = 0x5800_0088;
const SYSCFG_EXTICR2: u32 = 0x5800_040c;
const NVIC_ISER0: u32 = 0xe000_e100;
const NVIC_ICPR0: u32 = 0xe000_e280;
const NVIC_ICER0: u32 = 0xe000_e180;
const EXTI9_5_IRQ: u32 = 1 << 23;
const TOUCH_LINE: u32 = 1 << 6;

/// The touch INT line fell since the last call. Its NVIC pending bit is cleared by [`sleep`], with every wake
/// source's (a clear here, after the EXTI write, stuck: see there).
fn touch_edge() -> bool {
    let edge = rd(EXTI_CPUPR1) & TOUCH_LINE != 0;
    if edge {
        wr(EXTI_CPUPR1, TOUCH_LINE); // write 1 to clear
        let _ = rd(EXTI_CPUPR1); // the write done (same peripheral, Device memory: in order)
    }
    edge
}

// The keys' falling edges (BOARD.md 5) wake an idle sleep, as the touch INT does: EXTI lines 1 (PA1), 2 (PB2), 3
// (PE3) and 4 (PB4), each its own IRQ, EXTI1..EXTI4 = 7..10, and USART2 is IRQ 38 (stm32h747xx.h `IRQn_Type`;
// SYSCFG EXTICR1, lines 0..3, at 0x58000408, port codes A 0, B 1, E 4). Enabled in the NVIC only around an idle
// sleep (`wake_on_io`): in a short one, and in delay_ms, a held key's flag or a waiting byte would end every WFI.
const SYSCFG_EXTICR1: u32 = 0x5800_0408;
const KEY_LINES: u32 = 0b1_1110;
const KEY_IRQS: u32 = 0b1111 << 7;
const USART2_IRQ: u32 = 1 << (38 - 32);

/// The keys' EXTI lines: falling edge, unmasked for this core (SYSCFG's clock is on: i2c_init).
fn key_lines_init() {
    modify(SYSCFG_EXTICR1, 0xfff << 4, (1 << 8) | (4 << 12)); // line 1 port A, 2 port B, 3 port E
    modify(SYSCFG_EXTICR2, 0xf, 1); // line 4 port B
    modify(EXTI_FTSR1, 0, KEY_LINES);
    modify(EXTI_CPUIMR1, 0, KEY_LINES);
    wr(EXTI_CPUPR1, KEY_LINES);
}

/// The key lines end WFI (`keys`) or not, and the Nordic chip's bytes (`radio`: only while the app reads them, or
/// a byte nobody reads would end every WFI and the loop would spin). A pending IRQ wakes WFI with PRIMASK set
/// (ARMv7-M ARM B1.5.19), the way the touch line already does; no handler runs.
fn wake_on_io(keys: bool, radio: bool) {
    wr(if keys { NVIC_ISER0 } else { NVIC_ICER0 }, KEY_IRQS);
    wr(if radio { NVIC_ISER0 + 4 } else { NVIC_ICER0 + 4 }, USART2_IRQ);
}

/// A key went down since the last call (its EXTI flag, cleared here), or (`radio`) a byte from the Nordic chip
/// waits.
fn io_edge(radio: bool) -> bool {
    let keys = rd(EXTI_CPUPR1) & KEY_LINES;
    if keys != 0 {
        wr(EXTI_CPUPR1, keys); // write 1 to clear
        let _ = rd(EXTI_CPUPR1); // the write done
    }
    keys != 0 || radio && rd(U_ISR) & ISR_RXFNE != 0
}

/// Drops what the Nordic chip sent while the app does not read it (no chip started: `App::radio_up`): RQR.RXFRQ
/// flushes the RX FIFO (RM0399 USART_RQR), so RTS lets it on and nothing stale waits for the next start.
fn nrf_flush() {
    wr(U_RQR, 1 << 3);
}

/// SCL and SDA of an I2C bus: AF4, open drain, not driven while being set up.
fn i2c_pins(port: Port, pins: [u32; 2]) {
    for pin in pins {
        gpio(port, pin, IN, 0);
        modify(gpio_base(port) + 0x04, 0, 1 << pin); // OTYPER: open drain
        let (reg, sh) = if pin < 8 { (0x20, 4 * pin) } else { (0x24, 4 * (pin - 8)) }; // AFRL / AFRH
        modify(gpio_base(port) + reg, 0xf << sh, 4 << sh);
        modify(gpio_base(port), 3 << (2 * pin), AF << (2 * pin));
    }
}

/// 64 MHz: PRESC 15 (250 ns), SCLL 9 (2.5 us), SCLH 7 (2 us), SDADEL 2, SCLDEL 4: about 200 kHz, inside the fast mode
/// limits (RM0399 I2C timings). Stock runs about 400 kHz; a few bytes per poll need no more.
fn i2c_enable(i2c: u32) {
    wr(i2c + I2C_TIMINGR, (15 << 28) | (4 << 20) | (2 << 16) | (7 << 8) | 9);
    wr(i2c + I2C_CR1, 1); // PE
}

fn i2c_init() {
    modify(RCC_D3CCIPR, 3 << 8, 2 << 8); // I2C4SEL = hsi_ker_ck
    modify(RCC_APB4ENR, 0, 1 << 7); // I2C4EN
    modify(RCC_APB4RSTR, 0, 1 << 7); // from reset: a RAM run finds it as the last image left it
    modify(RCC_APB4RSTR, 1 << 7, 0);
    i2c_pins(Port::D, [12, 13]);
    i2c_enable(I2C4);
    // touch: the chip held in reset (the app releases it), INT an input, then I2C1 as I2C4
    pin_set(TOUCH_RESET.0, TOUCH_RESET.1, false);
    gpio(TOUCH_RESET.0, TOUCH_RESET.1, OUT, 0);
    gpio(TOUCH_INT.0, TOUCH_INT.1, IN, 0);
    modify(RCC_APB4ENR, 0, 1 << 1); // SYSCFGEN
    modify(SYSCFG_EXTICR2, 0xf << 8, 2 << 8); // EXTI6 from port C
    modify(EXTI_FTSR1, 0, TOUCH_LINE);
    modify(EXTI_CPUIMR1, 0, TOUCH_LINE);
    wr(EXTI_CPUPR1, TOUCH_LINE);
    wr(NVIC_ICPR0, EXTI9_5_IRQ);
    wr(NVIC_ISER0, EXTI9_5_IRQ); // pending wakes WFI (interrupts masked: no handler)
    modify(RCC_D2CCIP2R, 3 << 12, 2 << 12); // I2C123SEL = hsi_ker_ck
    modify(RCC_APB1LENR, 0, 1 << 21); // I2C1EN
    modify(RCC_APB1LRSTR, 0, 1 << 21);
    modify(RCC_APB1LRSTR, 1 << 21, 0);
    i2c_pins(Port::B, [6, 7]);
    i2c_enable(I2C1);
}

/// Waits for `flag` in I2C_ISR; false on a NACK, a bus error or 1 ms (a byte takes 45 us). A failed transfer leaves
/// the peripheral off and on again (PE cleared resets its state machine, RM0399), so the next one starts clean.
fn i2c_wait(i2c: u32, flag: u32) -> bool {
    let mut isr = 0;
    let ok = wait_us(1000, || {
        isr = rd(i2c + I2C_ISR);
        isr & (flag | ISR_NACKF | ISR_ERRORS) != 0
    }) && isr & (ISR_NACKF | ISR_ERRORS) == 0;
    if !ok {
        wr(i2c + I2C_CR1, 0);
        wr(i2c + I2C_ICR, 0x3f38); // every flag
        let _ = wait_us(10, || rd(i2c + I2C_CR1) & 1 == 0);
        wr(i2c + I2C_CR1, 1);
    }
    ok
}

/// Ends a transfer started with AUTOEND: waits for its STOP.
fn i2c_stop(i2c: u32) -> bool {
    let ok = i2c_wait(i2c, ISR_STOPF);
    wr(i2c + I2C_ICR, ISR_STOPF);
    ok
}

/// The register address, then `data` (at most 254 bytes), in one write transfer.
fn i2c_write(i2c: u32, addr: u8, reg: u8, data: &[u8]) -> bool {
    let n = data.len().min(254) as u32 + 1;
    wr(i2c + I2C_CR2, u32::from(addr) << 1 | n << 16 | CR2_AUTOEND | CR2_START);
    for &b in [reg].iter().chain(data).take(n as usize) {
        if !i2c_wait(i2c, ISR_TXIS) {
            return false;
        }
        wr(i2c + I2C_TXDR, u32::from(b));
    }
    i2c_stop(i2c)
}

/// The register address written, then a repeated start and `buf.len()` bytes read (at most 255).
fn i2c_read(i2c: u32, addr: u8, reg: u8, buf: &mut [u8]) -> bool {
    let n = buf.len().min(255);
    wr(i2c + I2C_CR2, u32::from(addr) << 1 | 1 << 16 | CR2_START);
    if !i2c_wait(i2c, ISR_TXIS) {
        return false;
    }
    wr(i2c + I2C_TXDR, u32::from(reg));
    if !i2c_wait(i2c, ISR_TC) {
        return false;
    }
    wr(i2c + I2C_CR2, u32::from(addr) << 1 | CR2_RD | (n as u32) << 16 | CR2_AUTOEND | CR2_START);
    for b in buf.iter_mut().take(n) {
        if !i2c_wait(i2c, ISR_RXNE) {
            return false;
        }
        *b = rd(i2c + I2C_RXDR) as u8;
    }
    i2c_stop(i2c)
}

// ---------------------------------------------------------------------------------------------------------------
// The board for the app in lib.rs: each method is the transport only.
// ---------------------------------------------------------------------------------------------------------------
/// The board: its state is the awake time counted so far ([`Board::awake_us`]): the cycle counter's last value
/// and the cycles before it (the counter wraps every 21 s of work).
struct S510 {
    cycles_at: u32,
    cycles: u64,
    /// The last sleep between passes: ms, and the work in it (USB), us ([`Board::last_sleep`]).
    sleep: (u32, u32),
    /// For the profiler (`App::prof`), since the main loop last took them: pixels to the panel, eMMC sectors read,
    /// written.
    io: (u32, u32, u32),
    /// The panel's DMA (None: pixels by the CPU, without POWER_STEPS bit 4 or after a DMA failure).
    dma: Option<hw::PanelDma>,
}

impl Board for S510 {
    fn panel_power_on(&mut self) {
        lcd_power_on();
    }
    fn panel_cmd(&mut self, cmd: u8, params: &[u8]) {
        lcd_cmd(cmd, params);
    }
    fn panel_ram_write(&mut self, count: u32, next: &mut dyn FnMut() -> u16) {
        self.io.0 = self.io.0.wrapping_add(count);
        lcd_ram(count, &mut self.dma, Run::Blit(next));
    }
    fn panel_fill(&mut self, count: u32, color: u16) {
        self.io.0 = self.io.0.wrapping_add(count);
        lcd_ram(count, &mut self.dma, Run::Fill(color));
    }
    fn backlight(&mut self, level: u8) {
        backlight(level);
    }
    fn beep(&mut self, hz: u32) {
        beep(hz);
    }
    fn keys_down(&mut self) -> u8 {
        KEYS.iter().enumerate().fold(0, |m, (i, &(port, pin))| m | (u8::from(pin_low(port, pin)) << i))
    }
    fn rtc(&mut self) -> (u32, u32) {
        rtc()
    }
    fn rtc_set(&mut self, tr: u32, dr: u32) {
        rtc_set(tr, dr);
    }
    fn delay_ms(&mut self, ms: u32) {
        delay_ms(ms);
    }
    fn awake_us(&mut self) -> u32 {
        // DWT_CYCCNT stops in WFI (seen on the unit, 2026-10-23): it counts work only; read at least every 21 s of
        // it (each main loop pass does)
        let now = cycles();
        self.cycles += u64::from(now.wrapping_sub(core::mem::replace(&mut self.cycles_at, now)));
        (self.cycles / u64::from(CPU_HZ / 1_000_000)) as u32
    }
    fn cycles(&mut self) -> u32 {
        cycles()
    }
    fn last_sleep(&mut self) -> (u32, u32) {
        self.sleep
    }
    fn standby(&mut self) {
        standby();
    }
    fn nrf_reset(&mut self) {
        nrf_reset();
    }
    fn nrf_send(&mut self, data: &[u8]) {
        nrf_send(data);
    }
    fn nrf_recv(&mut self, buf: &mut [u8]) -> usize {
        nrf_recv(buf)
    }
    fn battery_adc(&mut self) -> Option<(u16, u16, u16)> {
        let vref = adc_read(19)?;
        let battery = adc_read(1)?;
        Some((battery, vref, rd(VREFINT_CAL) as u16))
    }
    fn gnss_power(&mut self, on: bool) {
        pin_set(GNSS_POWER.0, GNSS_POWER.1, on);
    }
    fn gnss_transfer(&mut self, buf: &mut [u8]) {
        gnss_transfer(buf);
    }
    fn charger_present(&mut self) -> bool {
        !pin_low(VBUS.0, VBUS.1)
    }
    fn backup_read(&mut self, n: usize) -> u32 {
        rd(RTC_BKP0R + 4 * (n as u32 % 32))
    }
    fn backup_write(&mut self, n: usize, v: u32) {
        modify(PWR_CR1, 0, 1 << 8); // DBP: backup domain writable (the bootloader leaves it set; seen on the unit)
        wr(RTC_BKP0R + 4 * (n as u32 % 32), v);
    }
    fn emmc_power_on(&mut self) {
        emmc_power_on();
    }
    fn emmc_bus(&mut self, fast: bool, wide: bool) {
        wr(SD_CLKCR, emmc_clkcr(fast, wide));
    }
    fn emmc_cmd(&mut self, index: u8, arg: u32, resp: bryton_rustfw::emmc::Resp) -> Result<[u32; 4], BusError> {
        emmc_cmd(index, arg, resp, false)
    }
    fn emmc_read(&mut self, index: u8, arg: u32, buf: &mut [u8]) -> Result<u32, BusError> {
        self.io.1 = self.io.1.wrapping_add((buf.len() / bryton_rustfw::emmc::SECTOR) as u32);
        emmc_read(index, arg, buf)
    }
    fn emmc_writable(&self) -> bool {
        EMMC_WRITE
    }
    fn emmc_write(&mut self, index: u8, arg: u32, buf: &[u8]) -> Result<u32, BusError> {
        // a build without EMMC_WRITE=1 never gets here: emmc::Card::write refuses first (emmc_writable)
        self.io.2 = self.io.2.wrapping_add((buf.len() / bryton_rustfw::emmc::SECTOR) as u32);
        emmc_write(index, arg, buf)
    }
    fn i2c_read(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> bool {
        i2c_read(I2C4, addr, reg, buf)
    }
    fn i2c_write(&mut self, addr: u8, reg: u8, data: &[u8]) -> bool {
        i2c_write(I2C4, addr, reg, data)
    }
    fn touch_read(&mut self, reg: u8, buf: &mut [u8]) -> bool {
        i2c_read(I2C1, TOUCH_ADDR, reg, buf)
    }
    fn touch_reset(&mut self, run: bool) {
        pin_set(TOUCH_RESET.0, TOUCH_RESET.1, run);
    }
    fn touch_int(&mut self) -> bool {
        pin_low(TOUCH_INT.0, TOUCH_INT.1)
    }
}

/// eMMC writes allowed: only in a build made with EMMC_WRITE=1 (the first writes are done with the owner present).
const EMMC_WRITE: bool = match option_env!("EMMC_WRITE") {
    Some(s) => parse_u32(s) == 1,
    None => false,
};

/// Boot stage at which a diagnostic build stops: `DIAG_STOP=n cargo build --release` (0: never; docs/devloop.md layer 6).
const STOP: u32 = match option_env!("DIAG_STOP") {
    Some(s) => parse_u32(s),
    None => 0,
};

const fn parse_u32(s: &str) -> u32 {
    let b = s.as_bytes();
    let (mut i, mut n) = (0, 0);
    while i < b.len() {
        n = n * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    n
}

/// At stage STOP, waits for USB power, then resets. Power on, hold Page, plug in: Engineer Mode means the firmware
/// got there (the signal proven by diag v9; the backlight is no readout). Stages, in boot order: 1 MPU, 2 DWT,
/// 3 clocks and pins, 4 SPI and the sleep power steps (POWER_STEPS), 5 panel up and first frame, 6 first app step,
/// 7 first SysTick + WFI wait done.
fn checkpoint(n: u32) {
    if n == STOP {
        modify(RCC_AHB4ENR, 0, 1 << 2); // GPIOC, as v9: before board_init nothing else has set it
        gpio(VBUS.0, VBUS.1, IN, 0);
        while pin_low(VBUS.0, VBUS.1) {}
        system_reset();
    }
}

/// The app, kept in `.bss` (a static cell), not on main's stack: as a local, `App` (tens of KB) was held in main's
/// frame several times over (its construction, the move), which made main's frame 111 KB of the 123 KB of stack, and
/// opening a workout (a 17 KB frame more) overflowed into `.bss` (2026-10-24). Not inlined, so the one temporary
/// copy lives in this frame only while the app starts. `App::new` builds the app as one struct expression and
/// `App::begin` (drawing, the backup registers) runs on it in place: `App::start` doing both and returning a local
/// made this frame 107 KB, the split 43 KB (2026-10-24, tools/stack_check.py).
#[inline(never)]
fn make_app(board: &mut S510) -> &'static mut App {
    static APP: static_cell::StaticCell<App> = static_cell::StaticCell::new();
    let app = APP.init_with(|| App::new_with(board, ANT_KEY, DI2_KEY));
    app.begin(board);
    app
}

/// The course memory (about 70 KB: the course followed, its list), beside the app in AXI SRAM (link.x `.axibss`).
/// Built once on the stack and moved in, as the app is: this frame is about 36 KB (tools/stack_check.py budgets it;
/// review 2 found the old "cleared in place" untrue).
#[inline(never)]
fn make_courses() -> &'static mut bryton_rustfw::storage::course_file::Courses {
    static COURSES: static_cell::StaticCell<bryton_rustfw::storage::course_file::Courses> =
        static_cell::StaticCell::new();
    COURSES.init_with(bryton_rustfw::storage::course_file::Courses::new)
}

/// The road ahead (climbs_ahead.rs `Ahead`: the routing graph, the trace, about 110 KB), in AXI SRAM (link.x
/// `.axibss`). A const cell of all zeros (no `Option`, no non-zero start: the compiler keeps it in `.bss`), never
/// built on the stack.
fn make_ahead() -> &'static mut bryton_rustfw::climbs_ahead::Ahead {
    static AHEAD: static_cell::ConstStaticCell<bryton_rustfw::climbs_ahead::Ahead> =
        static_cell::ConstStaticCell::new(bryton_rustfw::climbs_ahead::Ahead::new());
    AHEAD.take()
}

/// The scan's cache of stock's rides (storage/stock.rs `Cache`, 256 KB), in AXI SRAM (link.x `.axibss`). A const
/// cell: zeroed with the section at reset, never built on the stack.
fn make_scan_cache() -> &'static mut bryton_rustfw::stock::Cache {
    static CACHE: static_cell::ConstStaticCell<bryton_rustfw::stock::Cache> =
        static_cell::ConstStaticCell::new(bryton_rustfw::stock::Cache::new());
    CACHE.take()
}

/// The reading of the recent rides for FITNESS's heart rate variability thresholds (storage/thresholds.rs `Scan`,
/// about 5 KB), beside the app in AXI SRAM (link.x `.axibss`): built once on the stack and moved in, as the courses.
#[inline(never)]
fn make_thresholds() -> &'static mut bryton_rustfw::thresholds::Scan {
    static SCAN: static_cell::StaticCell<bryton_rustfw::thresholds::Scan> = static_cell::StaticCell::new();
    SCAN.init_with(bryton_rustfw::thresholds::Scan::new)
}

/// The stack canary still holds its pattern, or a fault (hw.rs `fault`: the marker, a reset, the ride resumes from its
/// checkpoint). An overflow cannot be caught when it happens: with interrupts masked (PRIMASK) an MPU guard's fault
/// escalates to HardFault, whose stacking on the overflowed stack locks the core up (seen on the unit 2026-10-29,
/// BOARD.md 2). One DTCM load and a compare.
#[inline(always)]
fn stack_check() {
    if !bryton_rustfw::stack_intact(rd(hw::stack_canary())) {
        hw::fault();
    }
}

/// Runs the stack down past its canary on purpose (the dev link's `stackfault`, never the UI), then returns, so the
/// pass's [`stack_check`] must reset the unit. 128 B a call, all written before the next call (black_box), so the
/// frames cover the canary's word; it stops in the first frame that reaches below the canary, so less than a frame
/// of `.bss` under it is overwritten before the check, which follows at once.
#[inline(never)]
fn stack_fault(canary: u32) -> u32 {
    let mut frame = [canary; 32];
    core::hint::black_box(&mut frame);
    if hw::address(&frame) <= canary {
        return frame[0];
    }
    // used after the call, so each frame stays live and the recursion is not made a loop
    stack_fault(canary).wrapping_add(core::hint::black_box(&frame)[1])
}

fn main() -> ! {
    wr(hw::stack_canary(), bryton_rustfw::STACK_CANARY); // before anything runs deep: the stack is near its top
    mpu_init();
    checkpoint(1);
    dwt_init();
    checkpoint(2);
    board_init();
    standby_crumb_at_boot();
    checkpoint(3);
    let dma = spi_init();
    backlight_init();
    buzzer_init();
    let _ = adc_init(); // a failure leaves the battery unknown ("--%")
    gnss_init();
    i2c_init();
    key_lines_init();
    sleep_power_init();
    checkpoint(4);
    let mut board = S510 { cycles_at: 0, cycles: 0, sleep: (POLL_MS, 0), io: (0, 0, 0), dma };
    let app = make_app(&mut board);
    // what one profiler section costs (link/prof.rs): two counter reads and an add, the second time (the first
    // fills the I-cache from SDRAM)
    let mut overhead = 0;
    for _ in 0..2 {
        let t = cycles();
        let from = board.cycles();
        app.prof().span(Sec::Usb, from, board.cycles());
        overhead = cycles().wrapping_sub(t);
    }
    app.prof().reset();
    app.prof().overhead = overhead;
    if let Some(mem) = hw::map_memory() {
        // zeroes all of link.x MAP (2560 KB of SDRAM) once: about 15 to 40 ms (inferred), before the first map. The
        // basemap takes the first words; the rest (link.x MAP has room for it) is the buffer the map tiles of the road
        // ahead are inflated into
        match mem.split_at_mut_checked(bryton_rustfw::tiles::MEM_WORDS) {
            Some((map, rest)) if rest.len() >= bryton_rustfw::route::SCRATCH_WORDS => {
                app.set_map_memory(map);
                // exactly the size the tests use (route::SCRATCH_WORDS): a tile too big for it is absent there too
                app.set_ahead_memory(make_ahead(), rest.split_at_mut(bryton_rustfw::route::SCRATCH_WORDS).0);
            }
            _ => {}
        }
    }
    app.set_course_memory(make_courses());
    app.set_scan_cache(make_scan_cache());
    app.set_threshold_memory(make_thresholds());
    checkpoint(5);
    let mut link: Option<usb::Link> = None;
    let mut usb_tried = false;
    let mut session = Session::new();
    let mut stats = Stats::default();
    // awake cycles in the current second, all cycles run (the sleep loop's too), its length
    let (mut awake, mut ran, mut window_ms) = (0u32, 0u32, 0u32);
    let mut since_touch = 0u32; // ms slept since the panel was last read
    loop {
        let t0 = cycles();
        app.step(&mut board);
        stack_check();
        app.prof().span(Sec::Step, t0, cycles());
        checkpoint(6);
        let vbus = !pin_low(VBUS.0, VBUS.1);
        if vbus && !usb_tried {
            usb_tried = true;
            link = usb::start();
            app.set_usb(&mut board, if link.is_some() { UsbDot::Powered } else { UsbDot::Failed });
        } else if !vbus && usb_tried && !app.busy() {
            // not during a ride or on its card (a loose cable or a power bank unplugged must not lose it): after it
            // ponytail: unplugged = reset, the simplest way to power the PHY and the USB core down again
            system_reset();
        }
        // Sleep until the next pass, waking for USB activity. App work waits for the end of the sleep: it may sleep
        // too (delay_ms), which would take over this timer.
        let mut press = None;
        let mut touched = None;
        let sleep_start = cycles();
        let mut woke = false;
        // the pass's sleep, in slices: TOUCH_MS ones while a finger is on the panel, and one ends early when the
        // touch INT falls, so the panel is read at once and then every TOUCH_MS (stock: an EXTI6 wake, then reads
        // while the finger is down)
        let mut slept = 0; // ms, by SysTick: cycles() stops counting in WFI
        // idle: a longer sleep (App::pass_ms), ended at once by a key or a byte from the Nordic chip, so neither
        // waits for it (the chip's own buffers are not known: it is read as soon as it sends, as before)
        let idle = app.pass_ms() > POLL_MS;
        let radio = app.radio_up();
        if !radio {
            nrf_flush();
        }
        if idle {
            wake_on_io(true, radio);
        }
        'sleep: loop {
            let left = app.pass_ms().saturating_sub(slept); // a finger down makes it POLL_MS again
            if left == 0 {
                break;
            }
            let slice = left.min(if app.touching() { TOUCH_MS } else { TIMER_MAX_MS });
            timer_start(slice);
            let (edge, done, io) = loop {
                let usb_at = cycles();
                if let Some(l) = &mut link {
                    match l.poll(&mut session, app, &mut board, stats) {
                        Effect::Key(p) => press = Some(p),
                        Effect::Reset => {
                            l.linger(50);
                            system_reset();
                        }
                        Effect::StackFault => {
                            l.linger(50);
                            stack_fault(hw::stack_canary());
                            stack_check(); // resets; if it does not, the unit keeps running: the test failed
                        }
                        Effect::Run(size) => {
                            l.linger(50);
                            l.disconnect();
                            // an older image's sleep does not clear these lines: a pending one would end its every
                            // WFI, the spin standby's WFI showed on 2026-10-25 (main.rs `standby`, BOARD.md 9). So
                            // the key lines masked and cleared (RM0399 EXTI CPUIMR1 / CPUPR1), the NVIC lines off,
                            // and USART2's RXFNEIE (CR1 bit 5, RM0399 USART_CR1) off
                            wake_on_io(false, false);
                            modify(EXTI_CPUIMR1, KEY_LINES, 0);
                            wr(EXTI_CPUPR1, KEY_LINES);
                            modify(U_CR1, 1 << 5, 0);
                            hw::run_staged(size);
                        }
                        e => woke |= app.serve(&mut board, l, &session, e),
                    }
                    if app.restart_wanted() {
                        // the computer wrote to the volume and ejected it: a restart drops every cache of it
                        l.linger(50);
                        system_reset();
                    }
                    let dot = match l.watch(stats.uptime_ms.wrapping_add(slept), app.drive_failed()) {
                        UsbWatch::Up => UsbDot::Ready,
                        UsbWatch::Suspended => UsbDot::Suspended,
                        UsbWatch::Down => UsbDot::Powered,
                        UsbWatch::Drop => {
                            app.usb_dropped(&mut board);
                            UsbDot::Powered
                        }
                    };
                    // a drive command read or wrote the card, which may have slept and taken over SysTick: without
                    // this, the sleep's end is lost and the main loop stalls while USB is still served (seen on the
                    // unit, 2026-10-24: the uptime stopped, the dev link kept answering)
                    woke |= l.worked();
                    app.set_usb(&mut board, dot);
                    app.prof().span(Sec::Usb, usb_at, cycles());
                }
                let (edge, done, io) = (touch_edge(), timer_done(), idle && io_edge(radio));
                if edge || done || io || press.is_some() || woke {
                    // a key from the link acts now, so the next command cannot replace it; board work done for the
                    // link (an eMMC read) may have slept, which takes over SysTick: this sleep's end is lost, so end it
                    break (edge, done, io);
                }
                sleep();
            };
            let ms = if done { slice } else { timer_elapsed_ms(slice) };
            timer_stop();
            (slept, since_touch) = (slept + ms, since_touch.saturating_add(ms));
            // the panel before the early end below: with the USB drive busy, every slice ends early, and touch was
            // never read (seen on the unit, 2026-10-24: no gesture during a copy to the drive)
            if edge || app.touching() {
                // a gesture acts after the sleep: acting may sleep too (delay_ms), which takes over SysTick
                let t = cycles();
                touched = app.touch(&mut board, core::mem::take(&mut since_touch));
                app.prof().span(Sec::Touch, t, cycles());
                if touched.is_some() {
                    break 'sleep;
                }
            }
            if io || press.is_some() || woke {
                break;
            }
        }
        if idle {
            wake_on_io(false, false);
        }
        let sleep_cycles = cycles().wrapping_sub(sleep_start);
        board.sleep = (slept, sleep_cycles / (CPU_HZ / 1_000_000));
        let t1 = cycles();
        if press.is_some() || touched.is_some() {
            if let Some(p) = press {
                app.press(&mut board, p);
            }
            if let Some(i) = touched {
                app.input(&mut board, i);
            }
            app.prof().span(Sec::Input, t1, cycles());
        }
        checkpoint(7);
        // the app's work (step and keys), not the USB polls: those happen only while a cable is in
        let worked = t1.wrapping_sub(t0) - sleep_cycles + cycles().wrapping_sub(t1);
        awake = awake.saturating_add(worked);
        // CYCCNT stops in WFI, so the sleep's cycles are the time the CPU ran while it should have slept
        ran = ran.saturating_add(worked).saturating_add(sleep_cycles);
        // a pass is its work plus its sleep (a long one, an eMMC scan, is longer than POLL_MS)
        let pass_ms = worked / (CPU_HZ / 1000) + slept;
        window_ms += pass_ms;
        if window_ms >= 1000 {
            stats.awake_permille = (u64::from(awake / (CPU_HZ / 1000)) * 1000 / u64::from(window_ms)) as u32;
            stats.cpu_permille = (u64::from(ran / (CPU_HZ / 1000)) * 1000 / u64::from(window_ms)) as u32;
            (awake, ran, window_ms) = (0, 0, 0);
        }
        stats.uptime_ms = stats.uptime_ms.wrapping_add(pass_ms);
        app.prof().pass(pass_ms, slept);
        app.prof().io(core::mem::take(&mut board.io));
    }
}
