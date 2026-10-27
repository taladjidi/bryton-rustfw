//! The only module with `unsafe`: the vector table, the reset entry, register access, three CPU instructions,
//! the staging area for images loaded over USB and the jump into them, the map's memory in SDRAM, and the
//! critical-section implementation the USB crates need. Everything else in the firmware is safe Rust built on these (AGENTS.md, "Safe Rust only").

#![allow(unsafe_code)]

use core::arch::{asm, global_asm};
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------------------------------------------
// Vector table. Word 0x94 (vector 37) must hold "R039": the stock updater (BOARD.md 10) takes an IMG.bin with
// "R" + 3 digits there as a raw image, and pack.py checks it. Word 0x98 is the image size (the
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

/// RTC backup register 29: a fault's marker, 0xfa in the top byte and the faults in a row in the low byte (lib.rs
/// `FAULT_BKP`: the app says it restarted, and clears the count after a minute of running).
const FAULT_REG: u32 = 0x5800_4050 + 4 * 29;

/// A fault or a panic: marked in the RTC backup domain, then a system reset (the firmware starts again, and a ride
/// will resume from its checkpoint: AGENTS.md, the golden rule). Parked instead after 3 in a row without a minute of
/// running between, so a fault at start-up does not loop for ever. Interrupts are masked, so this runs only for a
/// fault (HardFault and the others escalate) or the panic handler; it uses no stack beyond rd / wr.
// SAFETY: the vector table refers to this symbol by name; nothing else is called default_handler.
#[unsafe(no_mangle)]
extern "C" fn default_handler() -> ! {
    let v = rd(FAULT_REG);
    let n = if v >> 24 == 0xfa { (v & 0xff) + 1 } else { 1 };
    wr(FAULT_REG, 0xfa00_0000 | n.min(0xff)); // PWR_CR1.DBP is left set by the bootloader (BOARD.md 8)
    if n <= 3 {
        barrier();
        wr(0xe000_ed0c, 0x05fa_0004); // AIRCR: SYSRESETREQ
    }
    loop {
        wfi();
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    default_handler()
}

unsafe extern "C" {
    static _sidata: u32;
    static _sdata: u32;
    static _edata: u32;
    static _sbss: u32;
    static _ebss: u32;
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
    // in this module, the link.x section bounds and the fault marker; and usb.rs's peek / poke, the dev link's
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
// The map's memory (tiles.rs: the basemap's pixels, stock's decoded tiles and IGN's, 2560 KB, about 2.5 MB): link.x
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
