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

// SAFETY: the vector table refers to this symbol by name; nothing else is called default_handler.
#[unsafe(no_mangle)]
extern "C" fn default_handler() -> ! {
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
    // all six bounds 4-aligned, and no Rust code has run yet that could hold a reference into them.
    let (data, itcm, bss) = (&raw const _sdata, &raw const _sitcm, &raw const _sbss);
    copy_words(&raw const _sidata as u32, data as u32, &raw const _edata as u32);
    copy_words(&raw const _siitcm as u32, itcm as u32, &raw const _eitcm as u32);
    let mut a = bss as u32;
    while a < &raw const _ebss as u32 {
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
    // SAFETY: only called with peripheral register addresses (RM0399 / BOARD.md) or, from this module, the
    // link.x section bounds: always mapped and aligned, and no Rust reference points there while it runs, so a
    // volatile access cannot break any.
    unsafe { read_volatile(a as *const u32) }
}

/// Writes the 32-bit word at `a`.
#[inline(always)]
pub fn wr(a: u32, v: u32) {
    // SAFETY: as rd; register writes only change hardware state, and the section copies in reset run before
    // any Rust object exists there.
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
// The map's memory (tiles.rs: the basemap's pixels and the decoded tiles, 1.3 MB): link.x reserves MAP in SDRAM
// for it and links nothing there. Handed out once, zeroed, as a plain slice: the rest of the firmware stays safe.
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
