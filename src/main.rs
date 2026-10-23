//! bryton-rustfw: a minimal firmware for the Bryton Rider S510 bike computer, written from scratch. It shows the time
//! kept by the RTC, big, on the panel, the heart rate from an ANT+ strap, and on a second page the BLE devices
//! nearby. Keys: Page turns the ride pages, hold Page for the menu (sensors, pages, wheel, nearby devices), Back
//! toggles the backlight (level and time-out in the menu), hold Back (power) 3 s to switch off (standby; the power button starts it again).
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

mod hw;
mod usb;

use bryton_rustfw::dev::{self, Effect, Session, Stats};
use bryton_rustfw::emmc::BusError;
use bryton_rustfw::ui::UsbDot;
use bryton_rustfw::{App, Board, POLL_MS, TOUCH_MS};
use core::sync::atomic::{AtomicU32, Ordering};
use hw::{barrier, rd, wfi, wr, wr8};

// ANT_KEY: the ANT+ network key from the private ant_key.bin, or None (build.rs).
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

/// Starts a SysTick period of `ms` (at most 80: 24-bit counter at 200 MHz). Its end makes the SysTick exception
/// pending, which wakes WFI even with interrupts masked (ARMv7-M B1.5.19); the handler never runs.
// ponytail: WFI sleep keeps the core clock running; stop mode with an RTC / LPTIM wake-up is the next step down.
fn timer_start(ms: u32) {
    wr(SYST_CSR, 0);
    wr(SCB_ICSR, 1 << 25); // PENDSTCLR
    wr(SYST_RVR, ms.min(80) * (CPU_HZ / 1000) - 1);
    wr(SYST_CVR, 0);
    wr(SYST_CSR, 0b111); // ENABLE, TICKINT, CLKSOURCE = processor clock
}

/// The period has ended (COUNTFLAG; reading clears it, so act on a true at once).
fn timer_done() -> bool {
    rd(SYST_CSR) & (1 << 16) != 0
}

fn timer_stop() {
    wr(SYST_CSR, 0);
    wr(SCB_ICSR, 1 << 25); // PENDSTCLR
}

/// Sleeps (WFI) instead of spinning.
fn delay_ms(ms: u32) {
    let mut left = ms;
    while left > 0 {
        let chunk = left.min(80);
        left -= chunk;
        timer_start(chunk);
        while !timer_done() {
            wfi(); // another wake-up (USB activity): sleep again
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
    // SPI1 kernel clock = per_ck = HSI (64 MHz): independent of the PLL outputs the bootloader left on or off.
    modify(RCC_CR, 0, 1); // HSION
    let t0 = cycles();
    while rd(RCC_CR) & (1 << 2) == 0 && cycles().wrapping_sub(t0) < CPU_HZ / 100 {} // HSIRDY, 10 ms max
    modify(RCC_D1CCIPR, 3 << 28, 0); // CKPERSEL = hsi_ker_ck
    modify(RCC_D2CCIP1R, 7 << 12, 4 << 12); // SPI123SEL = per_ck
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
// (BOARD.md 6), software slave management (CS on PG10 by hand), 64 MHz / 8 = 8 MHz.
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
const SPI_TIMEOUT: u32 = CPU_HZ / 100; // 10 ms: a stuck SPI must not hang the loop

fn spi_init() {
    // SSI first: with SSM, MASTER set while SSI is 0 is a mode fault, which clears MASTER (seen on the unit,
    // diag v17: CFG2 read back without MASTER, so SPI1 took 16 bytes and never clocked; RM0399 SPI mode fault).
    wr(SPI_CR1, CR1_SSI);
    // MBR = /8, FTHLV = 4 frames (TXP then means room for a 32-bit write of 4 bytes: pixels go 2 per access),
    // DSIZE = 8 bits
    wr(SPI_CFG1, (2 << 28) | (3 << 5) | 7);
    // AFCNTR (keep pins driven), SSM, CPOL, CPHA, MASTER, COMM = simplex transmitter
    wr(SPI_CFG2, (1 << 31) | (1 << 26) | (1 << 25) | (1 << 24) | (1 << 22) | (1 << 17));
}

const CR1_SSI: u32 = 1 << 12; // with SSM: internal NSS high, no mode fault
const SR_EOT: u32 = 1 << 3;

/// Starts a transfer of exactly `n` bytes (1..=0xffff), as the stock HAL_SPI_Transmit does (BOARD.md 6).
fn spi_begin(n: u32) {
    wr(SPI_CR1, CR1_SSI); // SPE = 0: TSIZE is only writable then
    wr(SPI_IFCR, 0xff8); // clear every event flag
    wr(SPI_CR2, n & 0xffff);
    wr(SPI_CR1, CR1_SSI | 1); // SPE
    wr(SPI_CR1, CR1_SSI | (1 << 9) | 1); // CSTART
}

/// SPI waits that ran out (spi_byte, spi_end).
static SPI_TIMEOUTS: AtomicU32 = AtomicU32::new(0);

/// After a first timeout the SPI is taken as dead: later waits give up at once (still counted), so a stuck panel
/// costs nothing and the loop and the USB dev link keep running, so `dev.py peek` can find out why (that is how
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

/// Waits for the end of the transfer (EOT: the last byte is out), then disables SPI1 until the next one.
fn spi_end() {
    spi_wait(SR_EOT);
    wr(SPI_IFCR, 0xff8);
    wr(SPI_CR1, CR1_SSI);
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

/// RAMWR then `count` pixels from `next`.
/// RAMWR then `count` pixels from `next`, two per 32-bit FIFO write. Generic, so a fill gets its own copy in which
/// the colour is a constant. Transfers of at most 0x7ffe pixels (TSIZE is 16 bits, 2 bytes per pixel).
#[inline(always)]
fn lcd_ram(count: u32, mut next: impl FnMut() -> u16) {
    pin_set(LCD_CS.0, LCD_CS.1, false);
    pin_set(LCD_DC.0, LCD_DC.1, false);
    spi_begin(1);
    spi_byte(0x2c); // RAMWR
    spi_end();
    pin_set(LCD_DC.0, LCD_DC.1, true);
    let mut left = count;
    while left > 0 {
        let n = left.min(0x7ffe);
        left -= n;
        spi_begin(2 * n);
        for _ in 0..n / 2 {
            let (a, b) = (next(), next());
            // the SPI sends the word's bytes from the lowest; the panel wants each pixel high byte first
            let word = u32::from(a.swap_bytes()) | (u32::from(b.swap_bytes()) << 16);
            if spi_wait(SR_TXP) {
                wr(SPI_TXDR, word); // 4 bytes: FTHLV is 4 frames, so TXP means room for them
            }
        }
        if n % 2 == 1 {
            let c = next();
            spi_byte((c >> 8) as u8);
            spi_byte(c as u8);
        }
        spi_end();
    }
    pin_set(LCD_CS.0, LCD_CS.1, true);
}

fn lcd_ram_write(count: u32, next: &mut dyn FnMut() -> u16) {
    lcd_ram(count, next);
}

/// `count` pixels of one colour (most panel writes: clearing boxes, backgrounds): no per-pixel callback.
fn lcd_fill(count: u32, color: u16) {
    lcd_ram(count, || color);
}

// ---------------------------------------------------------------------------------------------------------------
// RTC (0x58004000, BOARD.md 8), clocked by the LSE in the backup domain: it keeps the time the stock firmware
// set (from GPS) across resets and firmware changes. TR / DR are BCD; reading TR locks the shadow DR until DR is read.
// The stock firmware may keep UTC there and add the time zone itself (Clk_LocalTime): not traced.
// ---------------------------------------------------------------------------------------------------------------
const RTC_TR: u32 = 0x5800_4000;
const RTC_DR: u32 = 0x5800_4004;
const RTC_ICSR: u32 = 0x5800_400c;
/// Backup registers 0..31 (RM0399 RTC_BKPxR; BOARD.md 8 lists those stock uses).
const RTC_BKP0R: u32 = 0x5800_4050;

/// RTC_TR then RTC_DR: reading TR locks the shadow DR until DR is read, so the pair is coherent.
fn rtc() -> (u32, u32) {
    let t0 = cycles();
    while rd(RTC_ICSR) & (1 << 5) == 0 && cycles().wrapping_sub(t0) < CPU_HZ / 100 {} // RSF
    let tr = rd(RTC_TR);
    (tr, rd(RTC_DR))
}

// ---------------------------------------------------------------------------------------------------------------
// Power off = standby, as Pwr_StopModePeriphTransition(0) + HAL_PWREx_EnterSTANDBYMode(D2) and (D1) in stock
// (BOARD.md 9): wake-up pin WKUP1 (PA0, the power button), rising edge, no pull. Waking resets the unit:
// bootloader, then this firmware again.
// ---------------------------------------------------------------------------------------------------------------
const PWR: u32 = 0x5802_4800;
const PWR_CR1: u32 = PWR;
const PWR_CPUCR: u32 = PWR + 0x10;
const PWR_WKUPCR: u32 = PWR + 0x20;
const PWR_WKUPEPR: u32 = PWR + 0x28;

fn standby() -> ! {
    wr(PWR_WKUPEPR, 0); // all wake-up pins off while changing them
    wr(PWR_WKUPCR, 0x3f); // clear the wake-up flags
    wr(PWR_WKUPEPR, 1); // WKUPEN1, polarity rising (WKUPP1 = 0), no pull
    modify(PWR_CPUCR, 0, 1 << 9); // CSSF: clear the standby / stop flags
    modify(PWR_CPUCR, 0, (1 << 1) | 1); // PDDS_D2, PDDS_D1
    modify(0xe000_ed10, 0, 1 << 2); // SCB_SCR.SLEEPDEEP
    barrier();
    loop {
        wfi();
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
    wr(U_CR1, (1 << 29) | (1 << 3) | (1 << 2) | 1); // FIFOEN, TE, RE, UE
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
    wr(LPTIM_CMP, u32::from(level.min(100)) - 1);
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
    // from reset: a RAM run (dev.py run) inherits an enabled ADC, whose configuration can't be written
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
const STA_RXOVERR: u32 = 1 << 5;
const STA_CMDREND: u32 = 1 << 6;
const STA_CMDSENT: u32 = 1 << 7;
const STA_DATAEND: u32 = 1 << 8;
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
const EXTI9_5_IRQ: u32 = 1 << 23;
const TOUCH_LINE: u32 = 1 << 6;

/// The touch INT line fell since the last call (and the wake-up it caused is cleared).
fn touch_edge() -> bool {
    if rd(EXTI_CPUPR1) & TOUCH_LINE == 0 {
        return false;
    }
    wr(EXTI_CPUPR1, TOUCH_LINE); // write 1 to clear
    wr(NVIC_ICPR0, EXTI9_5_IRQ); // else WFI would return at once from now on
    true
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
struct S510;

impl Board for S510 {
    fn panel_power_on(&mut self) {
        lcd_power_on();
    }
    fn panel_cmd(&mut self, cmd: u8, params: &[u8]) {
        lcd_cmd(cmd, params);
    }
    fn panel_ram_write(&mut self, count: u32, next: &mut dyn FnMut() -> u16) {
        lcd_ram_write(count, next);
    }
    fn panel_fill(&mut self, count: u32, color: u16) {
        lcd_fill(count, color);
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
    fn delay_ms(&mut self, ms: u32) {
        delay_ms(ms);
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
        emmc_read(index, arg, buf)
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

/// Boot stage at which a diagnostic build stops: `DIAG_STOP=n cargo build --release` (0: never; AGENTS.md layer 6).
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
/// 3 clocks and pins, 4 SPI, 5 panel up and first frame, 6 first app step, 7 first SysTick + WFI wait done.
fn checkpoint(n: u32) {
    if n == STOP {
        modify(RCC_AHB4ENR, 0, 1 << 2); // GPIOC, as v9: before board_init nothing else has set it
        gpio(VBUS.0, VBUS.1, IN, 0);
        while pin_low(VBUS.0, VBUS.1) {}
        system_reset();
    }
}

fn main() -> ! {
    mpu_init();
    checkpoint(1);
    dwt_init();
    checkpoint(2);
    board_init();
    checkpoint(3);
    spi_init();
    backlight_init();
    buzzer_init();
    let _ = adc_init(); // a failure leaves the battery unknown ("--%")
    gnss_init();
    i2c_init();
    checkpoint(4);
    let mut board = S510;
    let mut app = App::start(&mut board, ANT_KEY);
    if let Some(mem) = hw::map_memory() {
        app.set_map_memory(mem); // zeroes 1.5 MB of SDRAM once: about 10 to 30 ms (inferred), before the first map
    }
    checkpoint(5);
    let mut link: Option<usb::Link> = None;
    let mut usb_tried = false;
    let mut session = Session::new();
    let mut stats = Stats::default();
    let (mut awake, mut passes) = (0u32, 0u32);
    let mut last_touch = cycles();
    loop {
        let t0 = cycles();
        app.step(&mut board);
        checkpoint(6);
        let vbus = !pin_low(VBUS.0, VBUS.1);
        if vbus && !usb_tried {
            usb_tried = true;
            link = usb::start();
            app.set_usb(&mut board, if link.is_some() { UsbDot::Powered } else { UsbDot::Failed });
        } else if !vbus && usb_tried {
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
        'sleep: loop {
            let left = (POLL_MS * (CPU_HZ / 1000)).saturating_sub(cycles().wrapping_sub(sleep_start)) / (CPU_HZ / 1000);
            if left == 0 {
                break;
            }
            timer_start(if app.touching() { left.min(TOUCH_MS) } else { left });
            let edge = loop {
                if let Some(l) = &mut link {
                    match l.poll(&mut session, &app, stats) {
                        Effect::None => {}
                        Effect::Key(p) => press = Some(p),
                        Effect::Reset => {
                            l.linger(50);
                            system_reset();
                        }
                        Effect::Sector(lba) => {
                            let mut buf = [0u8; bryton_rustfw::emmc::SECTOR];
                            let r = app.emmc().read(&mut board, lba, &mut buf).map(|()| &buf);
                            dev::send_sector(l, r);
                            l.flush();
                            woke = true;
                        }
                        Effect::Emmc => {
                            dev::send_layout(l, app.emmc().layout(&mut board));
                            l.flush();
                            woke = true;
                        }
                        Effect::Run(size) => {
                            l.linger(50);
                            l.disconnect();
                            hw::run_staged(size);
                        }
                    }
                    let dot = if l.configured() { UsbDot::Ready } else { UsbDot::Powered };
                    app.set_usb(&mut board, dot);
                }
                let edge = touch_edge();
                if edge || timer_done() || press.is_some() || woke {
                    // a key from the link acts now, so the next command cannot replace it; board work done for the
                    // link (an eMMC read) may have slept, which takes over SysTick: this sleep's end is lost, so end it
                    break edge;
                }
                wfi();
            };
            timer_stop();
            if press.is_some() || woke {
                break;
            }
            if edge || app.touching() {
                let now = cycles();
                let dt = now.wrapping_sub(last_touch) / (CPU_HZ / 1000);
                last_touch = now;
                // a gesture acts after the sleep: acting may sleep too (delay_ms), which takes over SysTick
                touched = app.touch(&mut board, dt);
                if touched.is_some() {
                    break 'sleep;
                }
            }
        }
        let sleep_cycles = cycles().wrapping_sub(sleep_start);
        let t1 = cycles();
        if let Some(p) = press {
            app.press(&mut board, p);
        }
        if let Some(i) = touched {
            app.input(&mut board, i);
        }
        checkpoint(7);
        // the app's work (step and keys), not the USB polls: those happen only while a cable is in
        awake = awake.saturating_add(t1.wrapping_sub(t0) - sleep_cycles + cycles().wrapping_sub(t1));
        passes += 1;
        if passes * POLL_MS >= 1000 {
            stats.awake_permille = awake / (CPU_HZ / 1000);
            (awake, passes) = (0, 0);
        }
        stats.uptime_ms = stats.uptime_ms.wrapping_add(POLL_MS);
    }
}
