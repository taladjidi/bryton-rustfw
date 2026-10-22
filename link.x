/* S510 memory as the bootloader leaves it: it copies the OS area from the eMMC to SDRAM 0xd0000000, sets MSP from
   word 0 and jumps to word 1 (BOARD.md 2). Code and constants run in place from SDRAM;
   data and the stack live in DTCM (128 KB at 0x20000000, always on, not cached). The RAM-run trampoline (hw.rs)
   lives in ITCM, so it can overwrite the image it came from. SDRAM from 0xd0800000 is the staging area for
   images loaded over USB (hw.rs STAGE): nothing is linked there. */
MEMORY
{
  SDRAM (rx) : ORIGIN = 0xd0000000, LENGTH = 1M
  DTCM (rwx) : ORIGIN = 0x20000000, LENGTH = 128K
  ITCM (rwx) : ORIGIN = 0x00001000, LENGTH = 4K
}

ENTRY(reset)
_stack_top = ORIGIN(DTCM) + LENGTH(DTCM);

SECTIONS
{
  .vectors ORIGIN(SDRAM) : { KEEP(*(.vectors)) } > SDRAM
  .text : { *(.text .text.*) } > SDRAM
  .rodata : ALIGN(4) { *(.rodata .rodata.*) . = ALIGN(4); } > SDRAM   /* ends aligned, so the size word matches the file */
  .itcm : ALIGN(4) { _sitcm = .; KEEP(*(.itcm)) . = ALIGN(4); _eitcm = .; } > ITCM AT > SDRAM
  _siitcm = LOADADDR(.itcm);
  .data : ALIGN(4) { _sdata = .; *(.data .data.*); . = ALIGN(4); _edata = .; } > DTCM AT > SDRAM
  _sidata = LOADADDR(.data);
  .bss (NOLOAD) : ALIGN(4) { _sbss = .; *(.bss .bss.*) *(COMMON); . = ALIGN(4); _ebss = .; } > DTCM
  _image_size = LOADADDR(.data) + SIZEOF(.data) - ORIGIN(SDRAM);
  ASSERT(_ebss + 16K <= _stack_top, "less than 16 KB left for the stack")
  /DISCARD/ : { *(.ARM.exidx .ARM.exidx.* .ARM.extab.*) }
}
