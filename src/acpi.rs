/*
 * Genesis OS — ACPI (Advanced Configuration and Power Interface)
 *
 * Parses ACPI tables to discover:
 * - Number of CPUs (from MADT)
 * - APIC IDs for each CPU
 * - I/O APIC address
 * - Interrupt routing information
 */

use core::mem::size_of;
use core::slice;

#[repr(C, packed)]
struct RsdpDescriptor {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
}

#[repr(C, packed)]
struct RsdpDescriptor20 {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
    length: u32,
    xsdt_address: u64,
    extended_checksum: u8,
    reserved: [u8; 3],
}

#[repr(C, packed)]
struct AcpiSdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

#[repr(C, packed)]
struct Madt {
    header: AcpiSdtHeader,
    local_apic_address: u32,
    flags: u32,
}

#[repr(C, packed)]
struct MadtEntryHeader {
    entry_type: u8,
    length: u8,
}

#[repr(C, packed)]
struct MadtLocalApic {
    header: MadtEntryHeader,
    processor_id: u8,
    apic_id: u8,
    flags: u32,
}

#[repr(C, packed)]
struct MadtIoApic {
    header: MadtEntryHeader,
    io_apic_id: u8,
    reserved: u8,
    io_apic_address: u32,
    global_system_interrupt_base: u32,
}

const MAX_CPUS: usize = 256;
static mut CPU_APIC_IDS: [u8; MAX_CPUS] = [0; MAX_CPUS];
static mut CPU_COUNT: usize = 0;
static mut IO_APIC_ADDRESS: u32 = 0xFEC00000;
static mut LOCAL_APIC_ADDRESS: u32 = 0xFEE00000;

/// Initialize ACPI and return number of CPUs detected
pub unsafe fn init() -> usize {
    // Search for RSDP in BIOS memory areas
    let rsdp = match find_rsdp() {
        Some(r) => r,
        None => {
            // No RSDP found — fall back to single CPU
            CPU_COUNT = 1;
            return 1;
        }
    };

    // Parse MADT (Multiple APIC Description Table)
    let madt = match find_madt(rsdp) {
        Some(m) => m,
        None => {
            // No MADT found — fall back to single CPU
            CPU_COUNT = 1;
            return 1;
        }
    };
    MADT_ADDRESS = madt as u64;
    parse_madt(madt);

    CPU_COUNT
}

/// Find RSDP in BIOS memory (EBDA or BIOS ROM area)
unsafe fn find_rsdp() -> Option<*const RsdpDescriptor> {
    // Search EBDA (Extended BIOS Data Area) - first 1 KB of 0x9FC00
    if let Some(rsdp) = search_rsdp(0x0009FC00, 0x400) {
        return Some(rsdp);
    }

    // Search BIOS ROM area (0xE0000 - 0xFFFFF)
    if let Some(rsdp) = search_rsdp(0x000E0000, 0x20000) {
        return Some(rsdp);
    }

    None
}

unsafe fn search_rsdp(start: usize, size: usize) -> Option<*const RsdpDescriptor> {
    let mut addr = start;
    while addr < start + size {
        let ptr = addr as *const RsdpDescriptor;
        if validate_rsdp(ptr) {
            return Some(ptr);
        }
        addr = addr.saturating_add(16); // RSDP is 16-byte aligned
    }
    None
}

unsafe fn validate_rsdp(ptr: *const RsdpDescriptor) -> bool {
    // Guard: pointer must not be null
    if ptr.is_null() {
        return false;
    }
    // Safety: ptr is a BIOS memory address we scan; we check signature before use.
    let rsdp = &*ptr;

    // Check signature "RSD PTR "
    if &rsdp.signature != b"RSD PTR " {
        return false;
    }

    // Verify checksum over the 20-byte RSDP v1 structure
    // Safety: ptr is a non-null BIOS memory pointer containing a valid signature.
    let bytes = slice::from_raw_parts(ptr as *const u8, size_of::<RsdpDescriptor>());
    let sum: u8 = bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));

    sum == 0
}

/// Find MADT using RSDP
unsafe fn find_madt(rsdp: *const RsdpDescriptor) -> Option<*const Madt> {
    if rsdp.is_null() {
        return None;
    }
    let rsdt_addr = (*rsdp).rsdt_address as usize;
    if rsdt_addr == 0 {
        return None;
    }
    let rsdt = rsdt_addr as *const AcpiSdtHeader;

    let rsdt_len = (*rsdt).length as usize;
    let header_size = size_of::<AcpiSdtHeader>();
    // Guard against underflow: RSDT must be at least as large as its header
    if rsdt_len < header_size {
        return None;
    }
    // Divide by 4 since entries are u32 pointers
    let entry_count = (rsdt_len - header_size) / 4;
    let entries = (rsdt as usize + header_size) as *const u32;

    for i in 0..entry_count {
        // Safety: i < entry_count, which is derived from the table length
        let entry_addr = *entries.add(i) as usize;
        if entry_addr == 0 {
            continue;
        }
        let entry_ptr = entry_addr as *const AcpiSdtHeader;
        if &(*entry_ptr).signature == b"APIC" {
            return Some(entry_ptr as *const Madt);
        }
    }

    None
}

/// Parse MADT to extract CPU and I/O APIC information
unsafe fn parse_madt(madt: *const Madt) {
    LOCAL_APIC_ADDRESS = (*madt).local_apic_address;

    let mut offset = size_of::<Madt>();
    let end = (*madt).header.length as usize;

    CPU_COUNT = 0;

    while offset < end {
        let entry = (madt as usize + offset) as *const MadtEntryHeader;
        let entry_type = (*entry).entry_type;
        let entry_length = (*entry).length as usize;

        // Guard: entry_length must be at least the header size to avoid infinite loop
        // and must not exceed remaining table space
        if entry_length < size_of::<MadtEntryHeader>() {
            break; // Corrupt MADT — stop parsing
        }

        match entry_type {
            0 => {
                // Local APIC (CPU) — ensure record is large enough before casting
                if entry_length >= size_of::<MadtLocalApic>() {
                    let lapic = entry as *const MadtLocalApic;
                    if (*lapic).flags & 1 != 0 && CPU_COUNT < MAX_CPUS {
                        CPU_APIC_IDS[CPU_COUNT] = (*lapic).apic_id;
                        CPU_COUNT = CPU_COUNT.saturating_add(1);
                    }
                }
            }
            1 => {
                // I/O APIC — ensure record is large enough before casting
                if entry_length >= size_of::<MadtIoApic>() {
                    let ioapic = entry as *const MadtIoApic;
                    IO_APIC_ADDRESS = (*ioapic).io_apic_address;
                }
            }
            _ => {}
        }

        offset = offset.saturating_add(entry_length);
    }
}

/// Get APIC ID for a specific CPU
pub fn cpu_apic_id(cpu_id: usize) -> Option<u8> {
    if cpu_id < unsafe { CPU_COUNT } {
        Some(unsafe { CPU_APIC_IDS[cpu_id] })
    } else {
        None
    }
}

/// Get local APIC base address
pub fn local_apic_address() -> u32 {
    unsafe { LOCAL_APIC_ADDRESS }
}

/// Get I/O APIC base address
pub fn io_apic_address() -> u32 {
    unsafe { IO_APIC_ADDRESS }
}

/// Get total number of detected CPUs
pub fn cpu_count() -> usize {
    unsafe { CPU_COUNT }
}

/// Return the physical address of the MADT table, or 0 if not yet discovered.
///
/// Used by `kernel::smp::init()` to perform its own structural MADT walk for
/// building the per-CPU topology table.
pub fn madt_address() -> u64 {
    unsafe { MADT_ADDRESS }
}

/// Cached physical address of the MADT, populated during `acpi::init()`.
static mut MADT_ADDRESS: u64 = 0;

/// Get APIC ID for CPU at given index (used by SMP code)
pub fn get_apic_id(cpu_index: usize) -> u32 {
    if let Some(apic_id) = cpu_apic_id(cpu_index) {
        apic_id as u32
    } else {
        u32::MAX
    }
}
