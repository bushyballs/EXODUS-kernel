/*
 * Genesis OS — I/O APIC
 *
 * Routes external interrupts (keyboard, disk, network) to CPUs.
 */

use core::ptr::{read_volatile, write_volatile};

const IOAPIC_REGSEL: u32 = 0x00;
const IOAPIC_IOWIN: u32 = 0x10;

const IOAPIC_ID: u32 = 0x00;
const IOAPIC_VER: u32 = 0x01;
const IOAPIC_REDTBL: u32 = 0x10;

static mut IOAPIC_BASE: u64 = 0;

/// Initialize I/O APIC
pub unsafe fn init() {
    IOAPIC_BASE = crate::acpi::io_apic_address() as u64;

    // Disable all interrupts initially
    let max_redirects = get_max_redirect();
    for i in 0..=max_redirects {
        set_redirect(i, 1 << 16, 0); // Masked
    }

    // Enable keyboard interrupt (IRQ 1 -> vector 33)
    set_redirect(1, 33, 0); // Deliver to BSP (APIC ID 0)

    // Enable timer interrupt (IRQ 0 -> vector 32) if using legacy PIT
    // (Not needed if using APIC timer)
}

/// Read from I/O APIC register
unsafe fn read_ioapic(reg: u32) -> u32 {
    let regsel = IOAPIC_BASE as *mut u32;
    let iowin = (IOAPIC_BASE + IOAPIC_IOWIN as u64) as *const u32;

    write_volatile(regsel, reg);
    read_volatile(iowin)
}

/// Write to I/O APIC register
unsafe fn write_ioapic(reg: u32, value: u32) {
    let regsel = IOAPIC_BASE as *mut u32;
    let iowin = (IOAPIC_BASE + IOAPIC_IOWIN as u64) as *mut u32;

    write_volatile(regsel, reg);
    write_volatile(iowin, value);
}

/// Get maximum redirection entry
unsafe fn get_max_redirect() -> u32 {
    (read_ioapic(IOAPIC_VER) >> 16) & 0xFF
}

/// Set redirection entry
/// irq: IRQ number (0-23)
/// vector: Interrupt vector to deliver
/// dest: Destination APIC ID
unsafe fn set_redirect(irq: u32, vector: u32, dest: u32) {
    let low = IOAPIC_REDTBL + irq * 2;
    let high = low + 1;

    // High: destination APIC ID
    write_ioapic(high, dest << 24);

    // Low: vector + flags
    // Bit 16: not masked
    // Bits 8-10: delivery mode (000 = fixed)
    // Bits 0-7: vector
    write_ioapic(low, vector & 0xFF);
}

/// Mask an IRQ
pub unsafe fn mask_irq(irq: u32) {
    let reg = IOAPIC_REDTBL + irq * 2;
    let val = read_ioapic(reg);
    write_ioapic(reg, val | (1 << 16));
}

/// Unmask an IRQ
pub unsafe fn unmask_irq(irq: u32) {
    let reg = IOAPIC_REDTBL + irq * 2;
    let val = read_ioapic(reg);
    write_ioapic(reg, val & !(1 << 16));
}

/// Route IRQ to specific CPU
pub unsafe fn route_irq(irq: u32, vector: u8, cpu_id: usize) {
    if let Some(apic_id) = crate::acpi::cpu_apic_id(cpu_id) {
        set_redirect(irq, vector as u32, apic_id as u32);
    }
}
