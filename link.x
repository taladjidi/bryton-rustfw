/* S510 memory as the bootloader leaves it: it copies the OS area from the eMMC to SDRAM 0xd0000000, sets MSP from
   word 0 and jumps to word 1 (BOARD.md 2). Code and constants run in place from SDRAM;
   data and the stack live in DTCM (128 KB at 0x20000000, always on, not cached). The RAM-run trampoline (hw.rs)
   lives in ITCM, so it can overwrite the image it came from. SDRAM from 0xd0800000 is the staging area for
   images loaded over USB (hw.rs STAGE): nothing is linked there. MAP (16 MB in, past the 6 MB the bootloader
   copies and the staging area) is the map's memory, handed out once by hw.rs map_memory: nothing is linked there
   either. AXISRAM (D1, 512 KB, unused by this firmware otherwise) holds the app itself (main.rs make_app's static
   cell, about 37 KB): in DTCM's .bss it left too little stack (2026-10-24, an overflow on opening a workout). It is
   zeroed at reset as .bss is (hw.rs). */
MEMORY
{
  SDRAM (rx) : ORIGIN = 0xd0000000, LENGTH = 1M
  DTCM (rwx) : ORIGIN = 0x20000000, LENGTH = 128K
  ITCM (rwx) : ORIGIN = 0x00001000, LENGTH = 4K
  MAP (rw)   : ORIGIN = 0xd1000000, LENGTH = 1536K
  AXISRAM (rw) : ORIGIN = 0x24000000, LENGTH = 512K
}

ENTRY(reset)
_stack_top = ORIGIN(DTCM) + LENGTH(DTCM);
_smap = ORIGIN(MAP);
_emap = ORIGIN(MAP) + LENGTH(MAP);

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
  .axibss (NOLOAD) : ALIGN(4) { _saxi = .; *(.bss._ZN13bryton_rustfw8make_app3APP*) . = ALIGN(4); _eaxi = .; } > AXISRAM
  ASSERT(_eaxi > _saxi, "the app's cell is not in AXISRAM: its section name changed (main.rs make_app)")
  .bss (NOLOAD) : ALIGN(4) { _sbss = .; *(.bss .bss.*) *(COMMON); . = ALIGN(4); _ebss = .; } > DTCM
  _image_size = LOADADDR(.data) + SIZEOF(.data) - ORIGIN(SDRAM);
  ASSERT(_ebss + 16K <= _stack_top, "less than 16 KB left for the stack")
  /DISCARD/ : { *(.ARM.exidx .ARM.exidx.* .ARM.extab.*) }
}
