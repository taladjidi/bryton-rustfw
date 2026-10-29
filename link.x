/* S510 memory as the bootloader leaves it: it copies the OS area from the eMMC to SDRAM 0xd0000000, sets MSP from
   word 0 and jumps to word 1 (BOARD.md 2). Code and constants run in place from SDRAM;
   data and the stacks live in DTCM (128 KB at 0x20000000, always on, not cached): .data, .bss, a 256 B guard no code
   may touch (main.rs mpu_init: a no-access MPU region, so an overflow faults instead of overwriting .bss), the thread
   stack (PSP, _guard_end up to _psp_top: everything but the fault handler runs there) and at the top the 1 KB handler
   stack (MSP, up to _stack_top: hw.rs default_handler runs there, so a fault stacked on an overflowed PSP still
   resets the unit instead of locking the core). The RAM-run trampoline (hw.rs)
   lives in ITCM, so it can overwrite the image it came from. SDRAM from 0xd0800000 is the staging area for
   images loaded over USB (hw.rs STAGE): nothing is linked there. MAP (16 MB in, past the 6 MB the bootloader
   copies and the staging area) is the map's memory, handed out once by hw.rs map_memory: nothing is linked there
   either. AXISRAM (D1, 512 KB, unused by this firmware otherwise) holds the app itself (main.rs make_app's static
   cell), the course memory (make_courses), the scan's cache of stock's rides (make_scan_cache), the road ahead
   (make_ahead) and FITNESS's reading of the recent rides (make_thresholds; sizes: tools/numbers.py), in DTCM's .bss the app left too little
   stack (2026-10-24, an overflow on opening a workout). It is
   zeroed at reset as .bss is (hw.rs). Its top 4 KB (PANELDMA) are the panel DMA's buffers (hw.rs PanelDma). */
MEMORY
{
  SDRAM (rx) : ORIGIN = 0xd0000000, LENGTH = 1M
  DTCM (rwx) : ORIGIN = 0x20000000, LENGTH = 128K
  ITCM (rwx) : ORIGIN = 0x00001000, LENGTH = 4K
  MAP (rw)   : ORIGIN = 0xd1000000, LENGTH = 2560K
  AXISRAM (rw) : ORIGIN = 0x24000000, LENGTH = 508K
  PANELDMA (rw) : ORIGIN = 0x2407f000, LENGTH = 4K
}

ENTRY(reset)
_stack_top = ORIGIN(DTCM) + LENGTH(DTCM);
_msp_top = _stack_top;
_psp_top = _stack_top - 1K;
_smap = ORIGIN(MAP);
_emap = ORIGIN(MAP) + LENGTH(MAP);
/* the panel DMA's two buffers (hw.rs PanelDma): the top 4 KB of AXI SRAM, reached by DMA1; nothing is linked there */
_spaneldma = ORIGIN(PANELDMA);

SECTIONS
{
  .vectors ORIGIN(SDRAM) : { KEEP(*(.vectors)) } > SDRAM
  .text : { *(.text .text.*) } > SDRAM
  .rodata : ALIGN(4) { *(.rodata .rodata.*) . = ALIGN(4); } > SDRAM   /* ends aligned, so the size word matches the file */
  .itcm : ALIGN(4) { _sitcm = .; KEEP(*(.itcm)) . = ALIGN(4); _eitcm = .; } > ITCM AT > SDRAM
  _siitcm = LOADADDR(.itcm);
  .data : ALIGN(4) { _sdata = .; *(.data .data.*); . = ALIGN(4); _edata = .; } > DTCM AT > SDRAM
  _sidata = LOADADDR(.data);
  /* before .bss, so the app's cell lands here and not there */
  .axibss (NOLOAD) : ALIGN(4) {
    _saxi = .;
    *(.bss._ZN13bryton_rustfw8make_app3APP*)
    _scourses = .;
    *(.bss._ZN13bryton_rustfw12make_courses7COURSES*)
    _ecourses = .;
    _sscan = .;
    *(.bss._ZN13bryton_rustfw15make_scan_cache5CACHE*)
    _escan = .;
    _sahead = .;
    *(.bss._ZN13bryton_rustfw10make_ahead5AHEAD*)
    _eahead = .;
    _sthresholds = .;
    *(.bss._ZN13bryton_rustfw15make_thresholds4SCAN*)
    _ethresholds = .;
    . = ALIGN(4); _eaxi = .;
  } > AXISRAM
  ASSERT(_eaxi > _saxi, "the app's cell is not in AXISRAM: its section name changed (main.rs make_app)")
  ASSERT(_escan > _sscan, "the scan's cache is not in AXISRAM: its section name changed (main.rs make_scan_cache)")
  ASSERT(_eahead > _sahead, "the road ahead is not in AXISRAM: its section name changed (main.rs make_ahead)")
  ASSERT(_ethresholds > _sthresholds, "the HRV thresholds' reading is not in AXISRAM: its section name changed (main.rs make_thresholds)")
  ASSERT(_ecourses > _scourses, "the course memory is not in AXISRAM: its section name changed (main.rs make_courses)")
  .bss (NOLOAD) : ALIGN(4) { _sbss = .; *(.bss .bss.*) *(COMMON); . = ALIGN(4); _ebss = .; } > DTCM
  _image_size = LOADADDR(.data) + SIZEOF(.data) - ORIGIN(SDRAM);
  /* the stack guard: an ARMv7-M MPU region's base must be aligned to its size (ARMv7-M ARM B3.5.8, MPU_RBAR) */
  _guard = ALIGN(_ebss, 256);
  _guard_end = _guard + 256;
  ASSERT(_guard_end + 16K <= _psp_top, "less than 16 KB left for the thread stack")
  /DISCARD/ : { *(.ARM.exidx .ARM.exidx.* .ARM.extab.*) }
}
