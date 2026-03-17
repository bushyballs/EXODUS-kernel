/// Hoags OS Genesis — Boot Stub
///
/// Multiboot 1 header + 32-bit → 64-bit long mode transition.
/// All assembly, no external assembler — compiled by LLVM via global_asm!().
///
/// Flow:
///   GRUB/QEMU drops us in 32-bit protected mode, A20 on, paging OFF.
///   1. Verify multiboot magic (EAX = 0x2BADB002)
///   2. Set up PML4 → PDPT → PD page tables (4 × 512 × 2 MB = 4 GB identity map)
///   3. Enable PAE (CR4.PAE)
///   4. Enable Long Mode (IA32_EFER.LME)
///   5. Enable Paging + Write Protect (CR0.PG | CR0.WP)
///   6. Load temporary 64-bit GDT
///   7. Far jump to 64-bit code segment
///   8. Reload segments, set RSP, call _start (Rust kernel entry)
///
/// Built from scratch by Hoags Inc. All code original.
use core::arch::global_asm;

// =============================================================================
// Multiboot 1 Header — must appear within the first 8 KB of the OS image
// =============================================================================
//   Magic:    0x1BADB002
//   Flags:    0x00000003 (ALIGN | MEMINFO)
//   Checksum: 0xE4524FFB (magic + flags + checksum = 0 mod 2^32)
global_asm!(
    ".section .multiboot_header, \"a\", @progbits",
    ".align 4",
    ".long 0x1BADB002",
    ".long 0x00000003",
    ".long 0xE4524FFB",
);

// =============================================================================
// BSS: Page Tables + Kernel Stack
// =============================================================================
global_asm!(
    ".section .bss, \"aw\", @nobits",
    ".align 4096",
    "p4_table:   .skip 4096",      // PML4 (Page Map Level 4)
    "p3_table:   .skip 4096",      // PDPT (Page Directory Pointer Table)
    "p2_table_0: .skip 4096",      // PD 0: 0x0000_0000 – 0x3FFF_FFFF (1st GB)
    "p2_table_1: .skip 4096",      // PD 1: 0x4000_0000 – 0x7FFF_FFFF (2nd GB)
    "p2_table_2: .skip 4096",      // PD 2: 0x8000_0000 – 0xBFFF_FFFF (3rd GB)
    "p2_table_3: .skip 4096",      // PD 3: 0xC000_0000 – 0xFFFF_FFFF (4th GB, LAPIC/IOAPIC)
    "stack_bottom: .skip 16777216", // 16 MB kernel stack (1500+ modules, deep init chain)
    "stack_top:",
);

// =============================================================================
// 32-bit Boot Entry → Long Mode Transition → Call _start
// =============================================================================
global_asm!(
    // --- 32-bit entry point (called by multiboot bootloader) ---
    ".section .boot, \"ax\", @progbits",
    ".code32",
    ".global _boot32",
    "",
    "_boot32:",
    "    movl $stack_top, %esp",
    "",
    "    cmpl $0x2BADB002, %eax",
    "    jne 2f",
    "",
    "    movl %ebx, %edi",
    "",
    // --- PML4[0] = &p3_table | present | writable ---
    "    movl $p3_table, %eax",
    "    orl  $0b11, %eax",
    "    movl %eax, (p4_table)",
    "",
    // --- PDPT[0..3] = &p2_table_N | present | writable  (4 GB identity map) ---
    "    movl $p2_table_0, %eax",
    "    orl  $0b11, %eax",
    "    movl %eax, (p3_table)",
    "    movl $p2_table_1, %eax",
    "    orl  $0b11, %eax",
    "    movl %eax, (p3_table + 8)",
    "    movl $p2_table_2, %eax",
    "    orl  $0b11, %eax",
    "    movl %eax, (p3_table + 16)",
    "    movl $p2_table_3, %eax",
    "    orl  $0b11, %eax",
    "    movl %eax, (p3_table + 24)",
    "",
    // --- Fill each PD with 512 × 2MB huge pages ---
    // PD 0: pages 0..511 -> 0x0000_0000 – 0x3FFF_FFFF
    "    xorl %ecx, %ecx",
    "1:",
    "    movl %ecx, %eax",
    "    shll $21, %eax",
    "    orl  $0b10000011, %eax",
    "    movl %eax, p2_table_0(,%ecx,8)",
    "    incl %ecx",
    "    cmpl $512, %ecx",
    "    jne  1b",
    // PD 1: pages 0..511 -> 0x4000_0000 – 0x7FFF_FFFF
    "    xorl %ecx, %ecx",
    "5:",
    "    movl %ecx, %eax",
    "    addl $512, %eax",
    "    shll $21, %eax",
    "    orl  $0b10000011, %eax",
    "    movl %eax, p2_table_1(,%ecx,8)",
    "    incl %ecx",
    "    cmpl $512, %ecx",
    "    jne  5b",
    // PD 2: pages 0..511 -> 0x8000_0000 – 0xBFFF_FFFF
    "    xorl %ecx, %ecx",
    "6:",
    "    movl %ecx, %eax",
    "    addl $1024, %eax",
    "    shll $21, %eax",
    "    orl  $0b10000011, %eax",
    "    movl %eax, p2_table_2(,%ecx,8)",
    "    incl %ecx",
    "    cmpl $512, %ecx",
    "    jne  6b",
    // PD 3: pages 0..511 -> 0xC000_0000 – 0xFFFF_FFFF (LAPIC, IOAPIC, etc.)
    "    xorl %ecx, %ecx",
    "7:",
    "    movl %ecx, %eax",
    "    addl $1536, %eax",
    "    shll $21, %eax",
    "    orl  $0b10000011, %eax",
    "    movl %eax, p2_table_3(,%ecx,8)",
    "    incl %ecx",
    "    cmpl $512, %ecx",
    "    jne  7b",
    "",
    // --- Load PML4 into CR3 ---
    "    movl $p4_table, %eax",
    "    movl %eax, %cr3",
    "",
    // --- Enable PAE (CR4 bit 5) ---
    "    movl %cr4, %eax",
    "    orl  $(1 << 5), %eax",
    "    movl %eax, %cr4",
    "",
    // --- Enable Long Mode: IA32_EFER MSR bit 8 ---
    "    movl $0xC0000080, %ecx",
    "    rdmsr",
    "    orl  $(1 << 8), %eax",
    "    wrmsr",
    "",
    // --- Enable Paging (CR0 bit 31) + Write Protect (bit 16) ---
    "    movl %cr0, %eax",
    "    orl  $0x80010000, %eax",
    "    movl %eax, %cr0",
    "",
    // --- Load 64-bit GDT and far jump to long mode ---
    "    lgdt (gdt64_pointer)",
    "    ljmp $0x08, $long_mode_entry",
    "",
    // --- Error: not loaded by multiboot bootloader ---
    "2:",
    "    movl $0x4F524F45, (0xB8000)",
    "    movl $0x4F3A4F52, (0xB8004)",
    "    movl $0x4F4E4F20, (0xB8008)",
    "    movl $0x4F204F4F, (0xB800C)",
    "    movl $0x4F424F4D, (0xB8010)",
    "    cli",
    "3: hlt",
    "    jmp  3b",
    "",
    // --- 64-bit Long Mode Entry ---
    ".code64",
    "long_mode_entry:",
    "    movw $0x10, %ax",
    "    movw %ax, %ds",
    "    movw %ax, %es",
    "    movw %ax, %fs",
    "    movw %ax, %gs",
    "    movw %ax, %ss",
    "",
    "    movabs $stack_top, %rsp",
    "    cld",
    "    xorq %rbp, %rbp",
    "",
    // SSE/AVX2 init moved to Rust code (after paging is fully set up)
    // The boot asm runs before page tables cover all memory
    // SIMD is enabled later via a safe Rust function call
    "",
    "    call _start",
    "",
    "    cli",
    "4: hlt",
    "    jmp  4b",
    options(att_syntax),
);

// =============================================================================
// Temporary 64-bit GDT (replaced by gdt::init() during kernel boot)
// =============================================================================
//   Entry 0: Null descriptor
//   Entry 1 (0x08): Code — executable, present, 64-bit long mode
//   Entry 2 (0x10): Data — writable, present
global_asm!(
    ".section .rodata, \"a\", @progbits",
    ".align 8",
    "gdt64:",
    "    .quad 0",
    "    .quad (1 << 43) | (1 << 44) | (1 << 47) | (1 << 53)",
    "    .quad (1 << 44) | (1 << 47) | (1 << 41)",
    "gdt64_pointer:",
    "    .word gdt64_pointer - gdt64 - 1",
    "    .quad gdt64",
);
