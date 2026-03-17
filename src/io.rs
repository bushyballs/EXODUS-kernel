/// Low-level I/O primitives for Hoags Kernel Genesis — built from scratch
///
/// Provides port I/O (in/out), CPU control (hlt, sti, cli),
/// and other architecture-level operations.
///
/// No external crates. Pure inline assembly.

/// Write a byte to an I/O port
// hot path: called from every UART write, PIC EOI, PIT update (~50K+/s combined)
#[inline(always)]
pub fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
    }
}

/// Read a byte from an I/O port
// hot path: called from UART status poll and keyboard IRQ handler
#[inline(always)]
pub fn inb(port: u16) -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack));
    }
    val
}

/// Write a 16-bit word to an I/O port
// hot path: PCI config space writes, DMA address programming
#[inline(always)]
pub fn outw(port: u16, val: u16) {
    unsafe {
        core::arch::asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack));
    }
}

/// Read a 16-bit word from an I/O port
// hot path: PCI config space reads
#[inline(always)]
pub fn inw(port: u16) -> u16 {
    let val: u16;
    unsafe {
        core::arch::asm!("in ax, dx", out("ax") val, in("dx") port, options(nomem, nostack));
    }
    val
}

/// Write a 32-bit dword to an I/O port
// hot path: e1000/virtio MMIO register writes, PCI BAR access
#[inline(always)]
pub fn outl(port: u16, val: u32) {
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack));
    }
}

/// Read a 32-bit dword from an I/O port
// hot path: e1000 RX/TX ring tail register reads
#[inline(always)]
pub fn inl(port: u16) -> u32 {
    let val: u32;
    unsafe {
        core::arch::asm!("in eax, dx", out("eax") val, in("dx") port, options(nomem, nostack));
    }
    val
}

/// Halt the CPU until the next interrupt
#[inline(always)]
pub fn hlt() {
    unsafe {
        core::arch::asm!("hlt", options(nomem, nostack));
    }
}

/// Enable interrupts (STI)
#[inline(always)]
pub fn sti() {
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
}

/// Disable interrupts (CLI)
#[inline(always)]
pub fn cli() {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
}

/// Small I/O delay (write to unused port 0x80)
#[inline(always)]
pub fn io_wait() {
    outb(0x80, 0);
}

/// CPU pause hint (PAUSE instruction for spinloops)
// hot path: called in every spinlock retry iteration
#[inline(always)]
pub fn pause() {
    core::hint::spin_loop();
}

/// Delay approximately 10ms using PIT channel 2 one-shot
/// Used for SMP AP startup timing
pub fn pit_delay_10ms() {
    // Enable PIT channel 2 gate
    let gate: u8 = inb(0x61);
    outb(0x61, (gate & 0xFC) | 0x01);

    // Mode 0, lobyte/hibyte, channel 2
    outb(0x43, 0xB0);

    // Count value for ~10ms at 1.193182 MHz = 11932
    outb(0x42, (11932 & 0xFF) as u8);
    outb(0x42, (11932 >> 8) as u8);

    // Wait for PIT to count down (output bit goes high)
    while inb(0x61) & 0x20 == 0 {
        core::hint::spin_loop();
    }

    // Restore gate
    outb(0x61, gate);
}
