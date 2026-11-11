//! The only module with `unsafe`: the vector table, the reset entry, register access, three CPU instructions,
//! the staging area for images loaded over USB and the jump into them, the map's memory in SDRAM, the Stop mode
//! entry and wake that run from ITCM while the SDRAM is in self-refresh (`stop`), and the critical-section implementation the USB crates need. Everything else in the firmware is safe Rust built on these (AGENTS.md, "Safe Rust only").

#![allow(unsafe_code)]

use core::arch::{asm, global_asm};
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering};

use bryton_rustfw::storage::faultlog::{self, Kind};

// ---------------------------------------------------------------------------------------------------------------
// Vector table. Word 0x94 (vector 37) must hold "R039": the stock updater (BOARD.md 10) takes an IMG.bin with
// "R" + 3 digits there as a raw image, and tools/pack.py checks it. Word 0x98 is the image size (the
// stock image has 0x003d7000 there). Interrupts stay masked; every vector parks the CPU.
// ---------------------------------------------------------------------------------------------------------------
global_asm!(
    r#"
    .section .vectors, "a", %progbits
    .global __vectors
__vectors:
    .word _stack_top
    .word reset
    .rept 35
    .word default_handler
    .endr
    .word 0x39333052
    .word _image_size
    "#
);

/// RTC backup register 29: a fault's marker, 0xfa in the top byte, the kind and its detail, and the faults in a row
/// in the low byte (lib.rs `FAULT_BKP`, the layout in storage/faultlog.rs: the app says it restarted, logs it to the
/// card, and clears the count after a minute of running).
const FAULT_REG: u32 = 0x5800_4050 + 4 * 29;

/// SCB CFSR, the Configurable Fault Status Register (Arm DUI 0646, "Configurable Fault Status Register"): why a CPU
/// fault happened, read (never cleared: the reset does) for the marker's detail.
const CFSR: u32 = 0xe000_ed28;

/// A fault or a panic: marked in the RTC backup domain, then a system reset (the firmware starts again, and a ride
/// will resume from its checkpoint: AGENTS.md, the golden rule). Parked instead after 3 in a row without a minute of
/// running between, so a fault at start-up does not loop for ever. Interrupts are masked (PRIMASK), so this runs
/// only for a fault (every one escalates to HardFault), the panic handler or [`fault`] (main.rs: the stack canary, a
/// cell built twice); it uses no stack beyond rd / wr. A fault whose own stacking faults (the stack already
/// overflowed) cannot get here: the core locks up (seen on the unit 2026-10-29, BOARD.md 2), which is why the stack is
/// watched by a canary, not an MPU guard.
// SAFETY: the vector table refers to this symbol by name; nothing else is called default_handler.
#[unsafe(no_mangle)]
extern "C" fn default_handler() -> ! {
    mark(Kind::Cpu, faultlog::cfsr_detail(rd(CFSR)))
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    mark(Kind::Panic, info.location().map_or(0, |l| l.line()))
}

/// A fault found by the firmware itself, marked and reset as a CPU fault is: the stack canary overwritten
/// (lib app/main_loop.rs `stack_ok`, [`Kind::Stack`]), or a static cell built twice (main.rs `once`, [`Kind::Cell`] with its
/// line), without linking `core::panicking`. No `unsafe`: [`mark`] is rd / wr.
pub fn fault(kind: Kind, detail: u32) -> ! {
    mark(kind, detail)
}

/// [`default_handler`]'s marker (storage/faultlog.rs `marker`: plain arithmetic on two words, no memory touched but
/// the register), then the reset, or the park past 3 in a row.
#[inline(always)]
fn mark(kind: Kind, detail: u32) -> ! {
    let v = faultlog::marker(rd(FAULT_REG), kind, detail);
    wr(FAULT_REG, v); // PWR_CR1.DBP is left set by the bootloader (BOARD.md 8)
    if v & 0xff <= 3 {
        barrier();
        wr(0xe000_ed0c, 0x05fa_0004); // AIRCR: SYSRESETREQ
    }
    // Parked: nothing may wake WFI, or from thread mode ([`fault`]) a pending SysTick or a level OTG_HS or EXTI line
    // ends every WFI at once and the unit spins at full clock. SysTick off (SYST_CSR 0) and its pending state cleared
    // (ICSR PENDSTCLR), every NVIC line disabled and its pending bit cleared (ICER0..7, ICPR0..7): ARMv7-M ARM
    // B3.3.3, B3.2.4, B3.4.4, B3.4.7; B1.5.19 for what wakes WFI. Only the four-button restart leaves the park.
    wr(0xe000_e010, 0);
    wr(0xe000_ed04, 1 << 25);
    for k in 0..8 {
        wr(0xe000_e180 + 4 * k, u32::MAX);
        wr(0xe000_e280 + 4 * k, u32::MAX);
    }
    barrier();
    loop {
        wfi();
    }
}

/// Where `x` lives (no unsafe: the pointer is only turned into a number), so main.rs's `stack_fault` knows how deep
/// its frame is; raw pointers stay in this module (AGENTS.md).
pub fn address<T>(x: &T) -> u32 {
    core::ptr::from_ref(x) as u32
}

/// The stack canary's address (link.x `_stack_canary`: the word just above .bss, the stack's last, never zeroed
/// with .bss and holding no Rust object), for main.rs to write and check through rd / wr.
pub fn stack_canary() -> u32 {
    &raw const _stack_canary as u32
}

unsafe extern "C" {
    static _sidata: u32;
    static _sdata: u32;
    static _edata: u32;
    static _sbss: u32;
    static _ebss: u32;
    static _stack_canary: u32;
    static _saxi: u32;
    static _eaxi: u32;
    static _siitcm: u32;
    static _sitcm: u32;
    static _eitcm: u32;
}

/// Entry from the bootloader: MSP already set from word 0, interrupts possibly left enabled.
// SAFETY: the vector table refers to this symbol by name; nothing else is called reset.
#[unsafe(no_mangle)]
extern "C" fn reset() -> ! {
    // SAFETY: masking interrupts has no memory effect. They stay masked for good: nothing here takes one, and
    // the critical-section implementation below relies on it.
    unsafe { asm!("cpsid i") };
    wr(0xe000_ed88, rd(0xe000_ed88) | (0xf << 20)); // CPACR: FPU full access (hard-float target)
    barrier();
    wr(0xe000_ed08, 0xd000_0000); // VTOR: our table (faults land in default_handler)
    // .data and the ITCM code from their load addresses, .bss zeroed. Word copies through rd / wr: link.x keeps
    // all eight bounds 4-aligned, and no Rust code has run yet that could hold a reference into them.
    let (data, itcm, bss) = (&raw const _sdata, &raw const _sitcm, &raw const _sbss);
    copy_words(&raw const _sidata as u32, data as u32, &raw const _edata as u32);
    copy_words(&raw const _siitcm as u32, itcm as u32, &raw const _eitcm as u32);
    let mut a = bss as u32;
    while a < &raw const _ebss as u32 {
        wr(a, 0);
        a += 4;
    }
    // the app's cell in AXI SRAM (link.x .axibss), zeroed as .bss: the cell's "taken" flag must start false
    let mut a = &raw const _saxi as u32;
    while a < &raw const _eaxi as u32 {
        wr(a, 0);
        a += 4;
    }
    barrier();
    crate::main()
}

fn copy_words(mut from: u32, mut to: u32, end: u32) {
    while to < end {
        wr(to, rd(from));
        from += 4;
        to += 4;
    }
}

/// Reads the 32-bit word at `a`.
#[inline(always)]
pub fn rd(a: u32) -> u32 {
    // SAFETY: rd and wr are safe functions over any address, which the compiler cannot check: their soundness rests
    // on every caller, and these are all of them. Register addresses (constants, each naming RM0399 or BOARD.md);
    // in this module, the link.x section bounds and the fault marker; main.rs's stack canary (link.x _stack_canary,
    // a DTCM word no Rust object holds: an overflow may write it, which is what the check looks for); main.rs's
    // `image_crc` and `image_marks` (aligned words of the image in SDRAM below its size word, read-only code and constants); and usb.rs's peek / poke, the dev link's
    // computer-given addresses, which they check first (aligned; poke in the peripheral space only, peek there or in
    // the DTCM or the SDRAM: all mapped). A peek of RAM may read a value a Rust object holds (it writes nothing,
    // and the value may be stale). A reserved address inside the peripheral space can still bus-fault: a reset
    // (default_handler). What a poke cannot be checked for: a register that makes the hardware write memory (DMA,
    // MDMA, FMC): the dev link trusts its operator there (usb.rs).
    unsafe { read_volatile(a as *const u32) }
}

/// Writes the 32-bit word at `a`.
#[inline(always)]
pub fn wr(a: u32, v: u32) {
    // SAFETY: as rd, whose callers are all of wr's: register writes change hardware state (the firmware's own never
    // point a DMA at memory a Rust object holds; the dev link's poke: see rd), and the section copies in reset run
    // before any Rust object exists there.
    unsafe { write_volatile(a as *mut u32, v) }
}

/// Writes the 8-bit register at `a` (the SPI data register, where the access size sets the frame size).
#[inline(always)]
pub fn wr8(a: u32, v: u8) {
    // SAFETY: as wr.
    unsafe { write_volatile(a as *mut u8, v) }
}

/// Completes memory accesses and refetches instructions (after MPU or system register changes).
#[inline(always)]
pub fn barrier() {
    // SAFETY: barriers only order memory accesses.
    unsafe { asm!("dsb", "isb") };
}

/// Sleeps until an interrupt becomes pending (it wakes WFI even though interrupts are masked).
#[inline(always)]
pub fn wfi() {
    // SAFETY: waiting has no memory effect.
    unsafe { asm!("wfi") };
}

// ---------------------------------------------------------------------------------------------------------------
// Critical sections for the USB crates. Interrupts are masked from the first instruction of reset and never
// unmasked, and only this one core runs: nothing can preempt, so entering a critical section has nothing to do.
// ---------------------------------------------------------------------------------------------------------------
struct InterruptsAlwaysOff;
critical_section::set_impl!(InterruptsAlwaysOff);

// SAFETY: see above: with no preemption possible, every section is already exclusive.
unsafe impl critical_section::Impl for InterruptsAlwaysOff {
    unsafe fn acquire() -> critical_section::RawRestoreState {}
    unsafe fn release(_: critical_section::RawRestoreState) {}
}

// ---------------------------------------------------------------------------------------------------------------
// Running an image loaded over USB (dev.rs `load`): it is staged in SDRAM, then a trampoline in ITCM copies it
// over our own image at 0xd0000000 and starts it like the bootloader would (MSP from word 0, jump to word 1).
// ---------------------------------------------------------------------------------------------------------------

/// Staging area: SDRAM 8 MB in. link.x puts nothing there and nothing allocates, so no Rust object lives in it.
pub const STAGE: u32 = 0xd080_0000;
/// Its size: one image region (link.x SDRAM, 1 MB).
pub const STAGE_LEN: u32 = 0x0010_0000;

/// Stores `data` at `offset` into the staging area; out of range bytes are dropped (dev.rs never sends any).
pub fn stage_write(offset: u32, data: &[u8]) {
    for (i, &b) in data.iter().enumerate() {
        let o = offset as usize + i;
        if o < STAGE_LEN as usize {
            // SAFETY: inside the staging area, which no Rust object occupies (see STAGE).
            unsafe { write_volatile((STAGE as usize + o) as *mut u8, b) };
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The map's memory (tiles.rs: the basemap's pixels, stock's decoded tiles and IGN's, 2688 KB, about 2.6 MB): link.x
// reserves MAP in SDRAM for it and links nothing there. Handed out once, zeroed, as a plain slice: the rest of the
// firmware stays safe.
// ---------------------------------------------------------------------------------------------------------------

unsafe extern "C" {
    static _smap: u16;
    static _emap: u16;
}

/// The map's memory, zeroed: Some the first time, None after (so there is only ever one reference to it).
pub fn map_memory() -> Option<&'static mut [u16]> {
    static TAKEN: AtomicBool = AtomicBool::new(false);
    if TAKEN.swap(true, Ordering::Relaxed) {
        return None;
    }
    let (start, end) = (&raw const _smap as usize, &raw const _emap as usize);
    let (p, len) = (start as *mut u16, (end - start) / 2);
    // SAFETY: [_smap, _emap) is SDRAM that link.x reserves for this alone (MAP: 2-byte aligned, mapped by the
    // FMC as the bootloader set it up, normal memory in main.rs's MPU region) and links nothing into; the only
    // other users of SDRAM are the image below 0xd0600000 and the staging area below 0xd0900000. TAKEN makes this
    // the only reference ever made to it, for the life of the program. Every word is written (zeroed) through the
    // raw pointer before the reference exists, so the slice is initialised u16s.
    unsafe {
        core::ptr::write_bytes(p, 0, len);
        Some(core::slice::from_raw_parts_mut(p, len))
    }
}

global_asm!(
    r#"
    .section .itcm, "ax", %progbits
    .thumb_func
    .global ram_trampoline
ram_trampoline:             @ r0 = source, r1 = byte count (multiple of 4, not 0)
    ldr r2, =0xd0000000
1:  ldr r3, [r0], #4
    str r3, [r2], #4
    subs r1, r1, #4
    bne 1b
    ldr r2, =0xe000ef50     @ ICIALLU: drop any instruction the cache holds from the old image
    str r1, [r2]
    dsb
    isb
    ldr r2, =0xd0000000
    ldr r3, [r2]
    msr msp, r3
    ldr r3, [r2, #4]
    bx r3
    .ltorg
    "#
);

unsafe extern "C" {
    fn ram_trampoline(src: u32, len: u32) -> !;
}

/// Copies the first `size` staged bytes over this image and starts them. Callers check the image first
/// (dev.rs check_image: Thumb reset vector inside it, SP in DTCM).
pub fn run_staged(size: u32) -> ! {
    let len = (size.clamp(4, STAGE_LEN) + 3) & !3;
    barrier();
    // SAFETY: the trampoline runs from ITCM (copied there in reset), outside the region it overwrites, with
    // interrupts masked; nothing returns into the old image, since the new one starts with a fresh stack.
    unsafe { ram_trampoline(STAGE, len) }
}

// ---------------------------------------------------------------------------------------------------------------
// Stop mode with the image in SDRAM (docs/power.md "Stop mode", phase 1: the dev link's benches; main.rs
// `low_power`). In Stop the FMC has no clock, so the SDRAM must keep its data on its own (self-refresh) and nothing
// may fetch from it until the clocks and the FMC's normal mode are back: the entry, the WFI and the wake run from
// ITCM (`stop_itcm`, in link.x .itcm with the RAM-run trampoline, copied there at reset), the stack is the DTCM, and
// interrupts stay masked (a pending enabled IRQ still ends WFI: ARMv7-M ARM B1.5.19). Its body is register accesses
// (rd / wr / barrier / wfi, all #[inline(always)]), counted loops and stores to the caller's report: no call, no
// table, no literal pointing into the SDRAM, which tools/stack_check.py checks in the release ELF (`itcm_calls`).
// Registers (stm32h747xx.h, STM32CubeH7 cmsis_device_h7, for the addresses and bit positions; RM0399 for what they do):
// FMC_SDCMR 0x52004150 (MODE 0..2, CTB2 bit 3, NRFS 5..8), FMC_SDSR 0x52004158 (MODES2 bits 3..4: 00 normal,
// 01 self-refresh; stm32h7xx_ll_fmc.c `FMC_SDRAM_GetModeStatus`; there is no busy flag on the H7); RCC_CR (HSEON 16 /
// HSERDY 17, PLL1ON 24 / RDY 25, PLL2 26 / 27, PLL3 28 / 29: each ready flag the bit above its enable), RCC_CFGR (SW
// 0..2, SWS 3..5, STOPWUCK 6, STOPKERWUCK 7); PWR_CR1 (LPDS 0, SVOS 14..15: 11 SVOS3, 10 SVOS4, 01 SVOS5,
// stm32h7xx_hal_pwr_ex.h), PWR_CSR1 ACTVOSRDY 13, PWR_CPUCR (PDDS_D1..D3 0..2, STOPF 5, CSSF 9, RUN_D3 11), PWR_CPU2CR
// at +0x14, PWR_D3CR VOSRDY 13; SCB_SCR SLEEPDEEP bit 2 (ARMv7-M ARM B3.2.7); DWT_CYCCNT 0xe0001004.
// The sequence follows STM32CubeH7's HAL_PWREx_EnterSTOPMode (LPDS, PDDS_Dx cleared, SLEEPDEEP, DSB, ISB, WFI,
// SLEEPDEEP cleared) and stock's idle hook (bryton repo ghidra/HARDWARE.md 13: SDRAM self-refresh on bank 2 before, its
// SystemClock_Config after every wake: VOSRDY first, then HSE, PLL1, SYSCLK).
// ---------------------------------------------------------------------------------------------------------------

const FMC_SDCMR: u32 = 0x5200_4150;
const FMC_SDSR: u32 = 0x5200_4158;
/// SDCMR's MODE, CTB1 / CTB2, NRFS and MRD fields (bits 0..22); the rest is reserved and kept.
const SDCMR_FIELDS: u32 = 0x007f_ffff;
/// MODE 101 self-refresh, CTB2: bank 2 (0xd0000000, BOARD.md 2), NRFS 0 (one refresh), as stock's command.
const SDCMR_SELF_REFRESH: u32 = 0b101 | 1 << 3;
/// MODE 000 normal, bank 2.
const SDCMR_NORMAL: u32 = 1 << 3;
const SDSR_MODES2: u32 = 0b11 << 3;
const SDSR_SELF_REFRESH: u32 = 0b01 << 3;
const RCC_CR: u32 = 0x5802_4400;
const RCC_CFGR: u32 = 0x5802_4410;
const PWR_CR1: u32 = 0x5802_4800;
const PWR_CSR1: u32 = 0x5802_4804;
const PWR_CPUCR: u32 = 0x5802_4810;
const PWR_CPU2CR: u32 = 0x5802_4814;
const PWR_D3CR: u32 = 0x5802_4818;
const SCB_SCR: u32 = 0xe000_ed10;
const DWT_CYCCNT: u32 = 0xe000_1004;
/// The oscillators (HSE; HSI48 12 / 13, CSI 7 / 8 too, if the bootloader left them on) and the PLLs a Stop turns off
/// (inferred, RM0399 not re-read: HSE and the PLLs stop, the wake runs on HSI or CSI; stock turns them on at every wake).
const OSC_ON: u32 = 1 << 16 | 1 << 12 | 1 << 7;
const PLL_ON: u32 = 1 << 24 | 1 << 26 | 1 << 28;
/// Iterations of a wait at most: each one is at least a register read (several cycles), so a missing flag costs at
/// most a fraction of a second, never a hang.
const WAIT: u32 = 1_000_000;

/// Waits (at most [`WAIT`] reads) for `rd(a) & mask == want`; true if it came.
#[inline(always)]
fn wait_for(a: u32, mask: u32, want: u32) -> bool {
    let mut n = 0;
    while rd(a) & mask != want && n < WAIT {
        n += 1;
    }
    rd(a) & mask == want
}

/// RCC_D3AMR (stm32h747xx.h `RCC_TypeDef` D3AMR at 0xa8): the D3 peripherals in autonomous mode, which keep D3 running.
const RCC_D3AMR: u32 = 0x5802_44a8;
/// RCC_D1CCIPR (stm32h747xx.h, offset 0x4c): FMCSEL in bits 0..1, 00 hclk3, 01 pll1_q, 10 pll2_r, 11 per_ck.
const RCC_D1CCIPR: u32 = 0x5802_444c;
/// EXTI_CPUPR1..3: this core's pending lines (EXTI_D1 at 0x58000080, PR1 / 2 / 3 at + 0x08 / 0x18 / 0x28).
const EXTI_CPUPR1: u32 = 0x5800_0088;
/// The DTCM, where the report must be (the caller's stack): the only RAM sure to answer while the SDRAM sleeps.
const DTCM: core::ops::Range<u32> = 0x2000_0000..0x2002_0000;

/// The registers that say why the system did or did not stop (lib stopbench.rs `Regs`).
#[inline(always)]
fn snap(g: &mut bryton_rustfw::stopbench::Regs) {
    g.cpucr = rd(PWR_CPUCR);
    g.cpu2cr = rd(PWR_CPU2CR);
    g.d3cr = rd(PWR_D3CR);
    g.d3amr = rd(RCC_D3AMR);
    g.rcc_cr = rd(RCC_CR);
    g.cfgr = rd(RCC_CFGR);
    g.pr = [rd(EXTI_CPUPR1), rd(EXTI_CPUPR1 + 0x10), rd(EXTI_CPUPR1 + 0x20)];
}

/// The SDRAM in self-refresh around one WFI, in Stop (`deep`, the regulator at `svos`: 3 for SVOS3, 2 SVOS4, 1 SVOS5)
/// or in CSleep (the clocks kept), then the clocks as they were (HSE, PLLs, SYSCLK's source, read before) and the
/// SDRAM in normal mode. Flags, cycles and the registers just before and after the WFI go into `r` (lib
/// link/stopbench.rs). If the self-refresh is not confirmed, no WFI: the normal-mode command is sent and it returns.
/// Whatever failed on the wake, the SDRAM is put back in normal mode before returning: on a PLL that did not lock
/// again, the FMC's kernel clock is moved to hclk3 first (the HSI's 64 MHz then, which the SDRAM allows: its refresh
/// only comes more often). Nothing is done (no flag set) unless `r` is in the DTCM.
/// The caller (main.rs `low_power`) arms the wake-up first: the RTC wake-up timer's EXTI line and IRQ, every other
/// NVIC line off, SysTick off, no DMA running, USB off the bus for a Stop.
pub fn stop(deep: bool, svos: u32, r: &mut bryton_rustfw::stopbench::StopReport) {
    if !DTCM.contains(&address(r)) {
        return;
    }
    barrier(); // every write to the SDRAM done before the command
    stop_itcm(deep, svos, r);
    barrier();
}

// SAFETY: `link_section` places this function in .itcm (link.x: ITCM, copied there by `reset` before main runs, as
// `ram_trampoline`); nothing else is in a section of that name but the trampoline. It must stay free of any fetch from
// the SDRAM between the self-refresh command and the normal-mode one: its body only inlines rd / wr / barrier / wfi
// and stores into `r` (in the DTCM: `stop` checks), no call, no constant table (the literals LLVM puts beside the
// code are in .itcm too), checked in the ELF by tools/stack_check.py. No exception can fetch a vector from the SDRAM
// meanwhile: PRIMASK is set (reset's `cpsid i`), so a pending IRQ only ends WFI, and no fault can occur (the
// accesses are to fixed registers; `r` is a valid reference). PRIMASK does not mask NMI: its one source here, the HSE
// clock security system, is off (RCC_CR HSECSSON, bit 19, read clear on the unit, BOARD.md 8; only software sets it,
// RM0399 RCC_CR, and this firmware never does).
#[unsafe(link_section = ".itcm")]
#[inline(never)]
fn stop_itcm(deep: bool, svos: u32, r: &mut bryton_rustfw::stopbench::StopReport) {
    use bryton_rustfw::stopbench as sb;
    let (cr, sw) = (rd(RCC_CR), rd(RCC_CFGR) & 7);
    wr(FMC_SDCMR, (rd(FMC_SDCMR) & !SDCMR_FIELDS) | SDCMR_SELF_REFRESH);
    let mut flags = 0;
    if wait_for(FMC_SDSR, SDSR_MODES2, SDSR_SELF_REFRESH) {
        flags |= sb::REFRESH_IN;
        if deep {
            // main regulator (LPDS 0) at SVOS; Stop, not Standby, in D1, D2 and D3 (PDDS_Dx 0), D3 not held running
            // (RUN_D3 0), STOPF / SBF cleared (CSSF); the wake on the HSI (STOPWUCK, STOPKERWUCK 0)
            wr(PWR_CR1, (rd(PWR_CR1) & !(0b11 << 14 | 1)) | (svos & 0b11) << 14);
            wr(PWR_CPUCR, (rd(PWR_CPUCR) & !(0b111 | 1 << 11)) | 1 << 9);
            wr(RCC_CFGR, rd(RCC_CFGR) & !(0b11 << 6));
            wr(SCB_SCR, rd(SCB_SCR) | 1 << 2);
        }
        snap(&mut r.pre);
        barrier();
        // PDDS_Dx read back 0 before a Stop's WFI: one left set (by `standby` before a switch-off) would make it
        // DStandby, a reset of D1 with the image's code in the SDRAM; then no WFI, and no STOPF to report
        if !deep || rd(PWR_CPUCR) & 0b111 == 0 {
            wfi();
        }
        snap(&mut r.post);
        wr(SCB_SCR, rd(SCB_SCR) & !(1 << 2));
        if r.post.cfgr & (7 << 3) != sw << 3 {
            flags |= sb::CLOCKS;
        }
        // the regulator at its Run voltage before any faster clock (stock waits VOSRDY first; ACTVOSRDY too: inferred)
        let t = rd(DWT_CYCCNT);
        if wait_for(PWR_D3CR, 1 << 13, 1 << 13) && wait_for(PWR_CSR1, 1 << 13, 1 << 13) {
            flags |= sb::VOS;
        }
        let t1 = rd(DWT_CYCCNT);
        r.vos = t1.wrapping_sub(t);
        // the oscillators on before, then the PLLs (their input), then SYSCLK's source: each one's ready flag is the
        // bit above its enable
        let osc = cr & OSC_ON;
        wr(RCC_CR, rd(RCC_CR) | osc);
        if wait_for(RCC_CR, osc << 1, osc << 1) {
            flags |= sb::HSE;
        }
        let t2 = rd(DWT_CYCCNT);
        r.hse = t2.wrapping_sub(t1);
        let pll = cr & PLL_ON;
        wr(RCC_CR, rd(RCC_CR) | pll);
        if wait_for(RCC_CR, pll << 1, pll << 1) {
            flags |= sb::PLL;
        } else if matches!(rd(RCC_D1CCIPR) & 0b11, 1 | 2) {
            // the FMC's kernel clock is a PLL output that did not come back: hclk3 instead, so it has a clock
            wr(RCC_D1CCIPR, rd(RCC_D1CCIPR) & !0b11);
        }
        let t3 = rd(DWT_CYCCNT);
        r.pll = t3.wrapping_sub(t2);
        wr(RCC_CFGR, (rd(RCC_CFGR) & !7) | sw);
        if wait_for(RCC_CFGR, 7 << 3, sw << 3) {
            flags |= sb::SW;
        }
        r.sw = rd(DWT_CYCCNT).wrapping_sub(t3);
    }
    let t = rd(DWT_CYCCNT);
    wr(FMC_SDCMR, (rd(FMC_SDCMR) & !SDCMR_FIELDS) | SDCMR_NORMAL);
    if wait_for(FMC_SDSR, SDSR_MODES2, 0) {
        flags |= sb::REFRESH_OUT;
    }
    r.fmc = rd(DWT_CYCCNT).wrapping_sub(t);
    r.flags |= flags;
}

// ---------------------------------------------------------------------------------------------------------------
// The panel's DMA (main.rs `lcd_ram`): DMA1 stream 0 sends pixel words from two buffers in AXI SRAM to SPI1's TXDR
// while the CPU fills the other buffer or sleeps. link.x reserves PANELDMA (top 4 KB of AXI SRAM: DMA1 reaches it
// through the D2-to-D1 bus matrix, RM0399 "System architecture"; it cannot reach the DTCM) and links nothing there.
// Only this module writes the stream's address and enable registers (main.rs sets DMAMUX1, the clock, the NVIC line
// and reads the flags), so the DMA never reads memory a Rust object holds except these buffers, and a buffer is
// handed out only while the stream does not read it.
// Cache contract: the D-cache is off (BOARD.md 2), so no maintenance is done. PANELDMA is 4 KB, 4 KB-aligned, so a
// single MPU region (size 4 KB, TEX 1 C 0 B 0: normal, not cacheable; higher numbered than any region over AXI SRAM)
// can cover it exactly: the D-cache commit must add that region, or else clean the buffer by address (DCCMVAC over
// its words, then DSB) in `start` before EN is set.
// ---------------------------------------------------------------------------------------------------------------

/// Words in each of the two buffers (2 pixels a word): 128 words, 256 pixels, 82 us at 50 MHz.
pub const DMA_WORDS: usize = 128;
// RM0399 DMA registers (DMA1 0x40020000): stream 0's CR, NDTR, PAR, M0AR at 0x10..0x1c, LIFCR at 0x08.
const DMA1_LIFCR: u32 = 0x4002_0008;
const DMA1_S0CR: u32 = 0x4002_0010;
const DMA1_S0NDTR: u32 = 0x4002_0014;
const DMA1_S0PAR: u32 = 0x4002_0018;
const DMA1_S0M0AR: u32 = 0x4002_001c;
/// SPI1_TXDR (RM0399 SPI registers: SPI1 0x40013000 + 0x20).
const SPI1_TXDR: u32 = 0x4001_3020;

unsafe extern "C" {
    static _spaneldma: u32;
}

/// DMA1 stream 0 and its two buffers: one handle, so one owner ([`PanelDma::take`]).
pub struct PanelDma(());

impl PanelDma {
    /// The handle, the stream stopped (the bootloader may have left it running) and the buffers zeroed: Some the
    /// first time, None after, or when the stream will not stop.
    pub fn take() -> Option<PanelDma> {
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            return None;
        }
        let mut dma = PanelDma(());
        dma.stop()?;
        // SAFETY: PANELDMA is AXI SRAM that link.x reserves for these two buffers and links nothing into; the stream
        // has just been seen stopped (EN 0), and only this module enables it again. Zeroed through the raw pointer
        // before any reference to it exists.
        unsafe { core::ptr::write_bytes(Self::addr(0) as *mut u32, 0, 2 * DMA_WORDS) };
        Some(dma)
    }

    fn addr(i: usize) -> u32 {
        &raw const _spaneldma as u32 + 4 * (DMA_WORDS * (i & 1)) as u32
    }

    /// The stream is running (EN reads 1 until the last word has gone, or an abort has taken effect: RM0399 DMA).
    pub fn busy(&self) -> bool {
        rd(DMA1_S0CR) & 1 != 0
    }

    /// Buffer `i` (0 or 1) to write, or None while the stream reads it.
    pub fn buf(&mut self, i: usize) -> Option<&mut [u32; DMA_WORDS]> {
        if self.busy() && rd(DMA1_S0M0AR) == Self::addr(i) {
            return None;
        }
        // SAFETY: buffer i of PANELDMA (see take), initialised. The stream does not read it now, and cannot start
        // to before `start`, which takes `&mut self`: the reference returned here must be gone by then. The handle
        // is unique (take), so no other reference to the buffer exists.
        Some(unsafe { &mut *(Self::addr(i) as *mut [u32; DMA_WORDS]) })
    }

    /// Starts sending words 0..n of buffer `i` to SPI1_TXDR, or word 0 `n` times when `repeat` (a fill), one word
    /// per SPI1 request (direct mode, 32-bit both sides). The stream interrupts on its end and on errors, which only
    /// pends its NVIC line (interrupts stay masked): it wakes WFI. false when the stream is running or n is out of
    /// range (1..=DMA_WORDS, or 1..=0xffff for a repeat).
    pub fn start(&mut self, i: usize, n: usize, repeat: bool) -> bool {
        let max = if repeat { 0xffff } else { DMA_WORDS };
        if self.busy() || n == 0 || n > max {
            return false;
        }
        wr(DMA1_LIFCR, 0x3d); // stream 0's flags: TCIF HTIF TEIF DMEIF FEIF
        wr(DMA1_S0PAR, SPI1_TXDR);
        wr(DMA1_S0M0AR, Self::addr(i));
        wr(DMA1_S0NDTR, n as u32);
        // RM0399 DMA_SxCR: PSIZE 12:11 and MSIZE 14:13 = 32 bits, MINC 10 (not for a repeat), DIR 7:6 = memory to
        // peripheral, TCIE 4, TEIE 2, DMEIE 1; then EN 0
        let cr = (2 << 13) | (2 << 11) | (u32::from(!repeat) << 10) | (1 << 6) | (1 << 4) | (1 << 2) | (1 << 1);
        wr(DMA1_S0CR, cr);
        barrier(); // the buffer's writes land before the stream reads it (no cache: in order already)
        wr(DMA1_S0CR, cr | 1);
        true
    }

    /// Stops the stream (bounded wait for EN to read 0) and returns the words it had not sent (NDTR), or None when
    /// EN still reads 1: the stream may still write SPI1_TXDR, so the caller must not.
    pub fn stop(&mut self) -> Option<usize> {
        wr(DMA1_S0CR, rd(DMA1_S0CR) & !1);
        let mut n = 0;
        while self.busy() && n < 10_000 {
            n += 1;
        }
        wr(DMA1_LIFCR, 0x3d); // an abort sets TCIF (RM0399 DMA, "Stream disable")
        (!self.busy()).then(|| rd(DMA1_S0NDTR) as usize)
    }
}
