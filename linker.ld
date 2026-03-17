/*
 * Hoags OS Genesis — Kernel Linker Script
 *
 * Memory layout (physical):
 *   0x00000000 - 0x000FFFFF  — Real mode IVT, BIOS, VGA buffer (0xB8000)
 *   0x00100000 (1 MB)        — Kernel load address (multiboot convention)
 *     .multiboot_header       — Multiboot header (must be in first 8 KB)
 *     .boot                   — 32→64 bit transition code
 *     .text                   — Rust kernel code
 *     .rodata                 — Read-only data (GDT, strings, tables)
 *     .data                   — Initialized mutable data
 *     .bss                    — Uninitialized data (page tables, stack)
 *     _kernel_end             — End of kernel (frame allocator starts here)
 *
 * Built from scratch by Hoags Inc.
 */

ENTRY(_boot32)

SECTIONS {
    /* Load the kernel at 1 MB — standard for multiboot kernels */
    . = 1M;

    /* Multiboot header must appear within the first 8 KB of the image */
    .multiboot_header : ALIGN(4) {
        KEEP(*(.multiboot_header))
    }

    /* Boot stub: 32-bit entry + long mode transition */
    .boot : ALIGN(16) {
        *(.boot)
    }

    /* Kernel code (Rust .text sections) */
    .text : ALIGN(4K) {
        *(.text .text.*)
    }

    /* Read-only data: string literals, GDT, lookup tables */
    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    /* Per-CPU data section */
    .percpu : ALIGN(4K) {
        __percpu_start = .;
        KEEP(*(.percpu .percpu.*))
        __percpu_end = .;
    }

    /* Kernel exported symbol table */
    .ksymtab : ALIGN(8) {
        __kernel_sym_start = .;
        KEEP(*(.ksymtab .ksymtab.*))
        __kernel_sym_end = .;
    }

    /* Initialized mutable data */
    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    /* Uninitialized data: page tables, stack, zeroed globals */
    .bss : ALIGN(4K) {
        __bss_start = .;
        *(COMMON)
        *(.bss .bss.*)
        __bss_end = .;
    }

    /* Mark the end of the kernel image — frame allocator starts here */
    . = ALIGN(4K);
    _kernel_end = .;

    /* Discard debug sections to keep the image small */
    /DISCARD/ : {
        *(.comment)
        *(.note.*)
        *(.eh_frame*)
    }
}
