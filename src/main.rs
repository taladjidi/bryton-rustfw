//! bryton-rustfw: a minimal firmware for the Bryton Rider S510 bike computer, written from scratch. It shows the time
//! kept by the RTC, big, on the panel, the heart rate from an ANT+ strap, and on a second page the BLE devices
//! nearby. Keys: Page switches pages, Back toggles the backlight, hold Back (power) 3 s to switch off (standby; the
//! power button starts it again).
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

use bryton_rustfw::dev::{Effect, Session};
use bryton_rustfw::ui::UsbDot;
use bryton_rustfw::{App, Board, POLL_MS};
use core::sync::atomic::{AtomicU32, Ordering};
use hw::{barrier, rd, wfi, wr, wr8};

// ANT_KEY: the ANT+ network key from the private ant_key.bin, or None (build.rs).
include!(concat!(env!("OUT_DIR"), "/ant_key.rs"));

#[inline(always)]
fn modify(a: u32, clear: u32, set: u32) {
    wr(a, (rd(a) & !clear) | set);
}

// ---------------------------------------------------------------------------------------------------------------
// MPU: one region over the 64 MB SDRAM, normal memory, not cached, executable, full access; everything else the
// default map (PRIVDEFENA). The MPU is only switched ON, never off: with it off, 0xc0000000..0xdfffffff would be
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
    // ENABLE, SIZE 25 (2^26 = 64 MB), TEX 001 C 0 B 0 (normal, not cacheable), AP 011 (full), XN 0
    wr(MPU_RASR, 1 | (25 << 1) | (1 << 19) | (3 << 24));
    wr(MPU_CTRL, rd(MPU_CTRL) | 0b101); // ENABLE + PRIVDEFENA
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
// PG13 backlight (LPTIM1 PWM in stock; a plain output here, high = on as derived from the inverted PWM).
const LCD_POWER: (Port, u32) = (Port::G, 3);
const LCD_RESET: (Port, u32) = (Port::I, 3);
const LCD_DC: (Port, u32) = (Port::G, 9);
const LCD_CS: (Port, u32) = (Port::G, 10);
const BACKLIGHT: (Port, u32) = (Port::G, 13);
// Keys (BOARD.md 5): PA1 back / power (0x01), PE3 page (0x08). PA0 is the power button's standby wake-up.
const KEY_BACK: (Port, u32) = (Port::A, 1);
const KEY_PAGE: (Port, u32) = (Port::E, 3);
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
    gpio(KEY_BACK.0, KEY_BACK.1, IN, 0);
    // PA0 (WKUP1, power button) as an input, as stock: left in its reset analog mode, the press never woke the unit
    // from standby (seen on the unit, 2026-10-23; inferred: the wake-up logic reads after the input buffer).
    gpio(Port::A, 0, IN, 0);
    gpio(KEY_PAGE.0, KEY_PAGE.1, IN, 0);
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
    wr(SPI_CFG1, (2 << 28) | 7); // MBR = /8, DSIZE = 8 bits
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
/// the SPI1 mode fault was found: README, "First run on the unit").
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
fn lcd_ram_write(count: u32, next: &mut dyn FnMut() -> u16) {
    pin_set(LCD_CS.0, LCD_CS.1, false);
    pin_set(LCD_DC.0, LCD_DC.1, false);
    spi_begin(1);
    spi_byte(0x2c); // RAMWR
    spi_end();
    pin_set(LCD_DC.0, LCD_DC.1, true);
    let mut left = count;
    while left > 0 {
        let n = left.min(0x7fff); // TSIZE is 16 bits: at most 0x7fff pixels (2 bytes each) per transfer
        left -= n;
        spi_begin(2 * n);
        for _ in 0..n {
            let color = next();
            spi_byte((color >> 8) as u8);
            spi_byte(color as u8);
        }
        spi_end();
    }
    pin_set(LCD_CS.0, LCD_CS.1, true);
}

// ---------------------------------------------------------------------------------------------------------------
// RTC (0x58004000, BOARD.md 8), clocked by the LSE in the backup domain: it keeps the time the stock firmware
// set (from GPS) across resets and firmware changes. TR / DR are BCD; reading TR locks the shadow DR until DR is read.
// The stock firmware may keep UTC there and add the time zone itself (Clk_LocalTime): not traced.
// ---------------------------------------------------------------------------------------------------------------
const RTC_TR: u32 = 0x5800_4000;
const RTC_DR: u32 = 0x5800_4004;
const RTC_ICSR: u32 = 0x5800_400c;

fn rtc_tr() -> u32 {
    let t0 = cycles();
    while rd(RTC_ICSR) & (1 << 5) == 0 && cycles().wrapping_sub(t0) < CPU_HZ / 100 {} // RSF
    let tr = rd(RTC_TR);
    let _ = rd(RTC_DR); // unlocks the shadow registers
    tr
}

// ---------------------------------------------------------------------------------------------------------------
// Power off = standby, as Pwr_StopModePeriphTransition(0) + HAL_PWREx_EnterSTANDBYMode(D2) and (D1) in stock
// (BOARD.md 9): wake-up pin WKUP1 (PA0, the power button), rising edge, no pull. Waking resets the unit:
// bootloader, then this firmware again.
// ---------------------------------------------------------------------------------------------------------------
const PWR: u32 = 0x5802_4800;
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
    fn backlight(&mut self, on: bool) {
        pin_set(BACKLIGHT.0, BACKLIGHT.1, on);
    }
    fn keys_down(&mut self) -> (bool, bool) {
        (pin_low(KEY_PAGE.0, KEY_PAGE.1), pin_low(KEY_BACK.0, KEY_BACK.1))
    }
    fn rtc_tr(&mut self) -> u32 {
        rtc_tr()
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
    checkpoint(4);
    let mut board = S510;
    let mut app = App::start(&mut board, ANT_KEY);
    checkpoint(5);
    let mut link: Option<usb::Link> = None;
    let mut usb_tried = false;
    let mut session = Session::new();
    let mut uptime_ms = 0u32;
    loop {
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
        let mut page = false;
        timer_start(POLL_MS);
        loop {
            if let Some(l) = &mut link {
                match l.poll(&mut session, &app, uptime_ms) {
                    Effect::None => {}
                    Effect::Page => page = true,
                    Effect::Reset => {
                        l.linger(50);
                        system_reset();
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
            if timer_done() {
                break;
            }
            wfi();
        }
        timer_stop();
        if page {
            app.next_page(&mut board);
        }
        checkpoint(7);
        uptime_ms = uptime_ms.wrapping_add(POLL_MS);
    }
}
