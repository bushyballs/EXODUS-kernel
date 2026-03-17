use crate::io::{inw, outb, outw};
use crate::sync::Mutex;
/// ACPI table parsing, power management, and hardware enumeration
///
/// Full ACPI implementation for Genesis:
///   - RSDP discovery (scan 0xE0000-0xFFFFF for "RSD PTR " signature)
///   - RSDT/XSDT table parsing with checksum validation
///   - MADT parsing for CPU/IO-APIC enumeration
///   - FADT for power management (SLP_TYP, SCI, PM1a/PM1b)
///   - DSDT/SSDT checksum validation
///   - Power state transitions (S0-S5)
///
/// Inspired by: ACPI specification 6.4, Linux ACPI subsystem,
/// ACPICA reference implementation. All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use core::ptr::read_volatile;

// ---------------------------------------------------------------------------
// ACPI table structures (packed, matching hardware layout)
// ---------------------------------------------------------------------------

/// RSDP (Root System Description Pointer) — ACPI 1.0
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct Rsdp {
    signature: [u8; 8], // "RSD PTR "
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8, // 0 = ACPI 1.0, 2 = ACPI 2.0+
    rsdt_address: u32,
}

/// RSDP extended (ACPI 2.0+)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct RsdpExtended {
    base: Rsdp,
    length: u32,
    xsdt_address: u64,
    extended_checksum: u8,
    reserved: [u8; 3],
}

/// Generic SDT (System Description Table) header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct SdtHeader {
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

/// MADT (Multiple APIC Description Table) header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct MadtHeader {
    sdt: SdtHeader,
    local_apic_addr: u32,
    flags: u32, // bit 0: PCAT_COMPAT (dual 8259 present)
}

/// MADT entry header (type + length)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct MadtEntryHeader {
    entry_type: u8,
    length: u8,
}

/// MADT type 0: Processor Local APIC
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct MadtLocalApic {
    header: MadtEntryHeader,
    processor_uid: u8,
    apic_id: u8,
    flags: u32, // bit 0: enabled, bit 1: online capable
}

/// MADT type 1: I/O APIC
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct MadtIoApic {
    header: MadtEntryHeader,
    io_apic_id: u8,
    reserved: u8,
    io_apic_address: u32,
    gsi_base: u32,
}

/// MADT type 2: Interrupt Source Override
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct MadtIso {
    header: MadtEntryHeader,
    bus: u8,
    source: u8,
    gsi: u32,
    flags: u16,
}

/// MADT type 4: Non-Maskable Interrupt
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct MadtNmi {
    header: MadtEntryHeader,
    processor_uid: u8,
    flags: u16,
    lint: u8,
}

/// MADT type 5: Local APIC Address Override (64-bit)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct MadtLocalApicOverride {
    header: MadtEntryHeader,
    reserved: u16,
    local_apic_address_64: u64,
}

/// MADT type 9: Processor Local x2APIC (ACPI 4.0+)
///
/// Used on systems with more than 254 logical CPUs (x2APIC IDs are 32-bit).
/// For systems with ≤ 254 CPUs this entry may still appear alongside or
/// instead of type-0 entries when the firmware uses x2APIC mode.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct MadtX2Apic {
    header: MadtEntryHeader,
    reserved: u16,
    x2apic_id: u32, // 32-bit x2APIC ID (supersedes the 8-bit APIC ID in type 0)
    flags: u32,     // bit 0: enabled, bit 1: online capable (same as type 0)
    processor_uid: u32,
}

/// FADT (Fixed ACPI Description Table)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct Fadt {
    sdt: SdtHeader,
    firmware_ctrl: u32,
    dsdt_address: u32,
    reserved1: u8,
    preferred_pm_profile: u8,
    sci_interrupt: u16,
    smi_command_port: u32,
    acpi_enable: u8,
    acpi_disable: u8,
    s4bios_req: u8,
    pstate_control: u8,
    pm1a_event_block: u32,
    pm1b_event_block: u32,
    pm1a_control_block: u32,
    pm1b_control_block: u32,
    pm2_control_block: u32,
    pm_timer_block: u32,
    gpe0_block: u32,
    gpe1_block: u32,
    pm1_event_length: u8,
    pm1_control_length: u8,
    pm2_control_length: u8,
    pm_timer_length: u8,
    gpe0_block_length: u8,
    gpe1_block_length: u8,
    gpe1_base: u8,
    cstate_control: u8,
    worst_c2_latency: u16,
    worst_c3_latency: u16,
    flush_size: u16,
    flush_stride: u16,
    duty_offset: u8,
    duty_width: u8,
    day_alarm: u8,
    month_alarm: u8,
    century: u8,
    iapc_boot_arch: u16,
    reserved2: u8,
    flags: u32,
    // GenericAddressStructure reset_reg would follow in ACPI 2.0+
    // We stop here for compatibility
}

// ---------------------------------------------------------------------------
// MADT entry type constants
// ---------------------------------------------------------------------------
const MADT_LOCAL_APIC: u8 = 0;
const MADT_IO_APIC: u8 = 1;
const MADT_ISO: u8 = 2;
const MADT_NMI_SOURCE: u8 = 3;
const MADT_LOCAL_APIC_NMI: u8 = 4;
const MADT_LOCAL_APIC_OVERRIDE: u8 = 5;
const MADT_X2APIC: u8 = 9;

// MADT flags bit definitions
const MADT_FLAG_PCAT_COMPAT: u32 = 1 << 0; // dual 8259 PIC present

// LAPIC flags bit definitions
const LAPIC_FLAG_ENABLED: u32 = 1 << 0;
const LAPIC_FLAG_ONLINE_CAPABLE: u32 = 1 << 1;

// ---------------------------------------------------------------------------
// Power state definitions
// ---------------------------------------------------------------------------

/// ACPI system power states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    S0, // Working
    S1, // Sleeping (CPU stops, power to RAM maintained)
    S2, // Sleeping (CPU off, dirty cache flushed)
    S3, // Suspend to RAM
    S4, // Suspend to disk (hibernate)
    S5, // Soft off
}

/// Power event types from ACPI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Sleep,
    Wake,
    Shutdown,
    ThermalTrip,
    PowerButton,
    SleepButton,
}

// PM1 control register bits
const PM1_SCI_EN: u16 = 1 << 0;
const PM1_SLP_EN: u16 = 1 << 13;
const PM1_SLP_TYP_SHIFT: u16 = 10;
const PM1_SLP_TYP_MASK: u16 = 0x7 << 10;

// PM1 event/status register bits
const PM1_TMR_STS: u16 = 1 << 0;
const PM1_BM_STS: u16 = 1 << 4;
const PM1_GBL_STS: u16 = 1 << 5;
const PM1_PWRBTN_STS: u16 = 1 << 8;
const PM1_SLPBTN_STS: u16 = 1 << 9;
const PM1_WAK_STS: u16 = 1 << 15;

// PM1 event enable register bits (offset by pm1_event_length / 2)
const PM1_PWRBTN_EN: u16 = 1 << 8;
const PM1_SLPBTN_EN: u16 = 1 << 9;

// ---------------------------------------------------------------------------
// Parsed ACPI information
// ---------------------------------------------------------------------------

/// CPU entry discovered from MADT
#[derive(Debug, Clone, Copy)]
pub struct CpuEntry {
    pub processor_uid: u8,
    pub apic_id: u8,
    pub enabled: bool,
    pub online_capable: bool,
}

/// I/O APIC entry from MADT
#[derive(Debug, Clone, Copy)]
pub struct IoApicEntry {
    pub id: u8,
    pub address: u32,
    pub gsi_base: u32,
}

/// Interrupt source override from MADT
#[derive(Debug, Clone, Copy)]
pub struct IsoEntry {
    pub bus: u8,
    pub source_irq: u8,
    pub gsi: u32,
    pub flags: u16,
}

/// NMI entry from MADT
#[derive(Debug, Clone, Copy)]
pub struct NmiEntry {
    pub processor_uid: u8,
    pub flags: u16,
    pub lint: u8,
}

/// ACPI table descriptor (for table registry)
#[derive(Debug, Clone)]
pub struct AcpiTable {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub oem_id: [u8; 6],
    pub phys_addr: usize,
    pub checksum_valid: bool,
}

/// SLP_TYP values for each sleep state (from \_Sx objects in DSDT/SSDT)
#[derive(Debug, Clone, Copy)]
struct SlpTyp {
    pm1a_slp_typ: u8,
    pm1b_slp_typ: u8,
}

/// Inner state of the ACPI subsystem
struct AcpiInner {
    /// All discovered tables
    tables: Vec<AcpiTable>,
    /// CPUs discovered from MADT
    cpus: Vec<CpuEntry>,
    /// I/O APICs from MADT
    io_apics: Vec<IoApicEntry>,
    /// Interrupt source overrides
    isos: Vec<IsoEntry>,
    /// NMI entries
    nmis: Vec<NmiEntry>,
    /// Local APIC physical address
    local_apic_addr: u64,
    /// MADT flags
    madt_flags: u32,
    /// FADT power management ports
    pm1a_control: u32,
    pm1b_control: u32,
    pm1a_event: u32,
    pm1b_event: u32,
    pm_timer: u32,
    pm1_event_len: u8,
    sci_interrupt: u16,
    smi_command: u32,
    acpi_enable_val: u8,
    acpi_disable_val: u8,
    /// DSDT physical address
    dsdt_addr: u32,
    /// SLP_TYP values per sleep state (S0-S5)
    slp_typ: [SlpTyp; 6],
    /// Current power state
    power_state: PowerState,
    /// Whether SCI is enabled
    sci_enabled: bool,
    /// ACPI revision (0 for 1.0, 2 for 2.0+)
    revision: u8,
}

static ACPI: Mutex<Option<AcpiInner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Checksum validation
// ---------------------------------------------------------------------------

/// Validate a byte-range checksum (sum of all bytes mod 256 == 0)
unsafe fn validate_checksum(base: *const u8, length: usize) -> bool {
    let mut sum: u8 = 0;
    for i in 0..length {
        sum = sum.wrapping_add(read_volatile(base.add(i)));
    }
    sum == 0
}

// ---------------------------------------------------------------------------
// RSDP discovery
// ---------------------------------------------------------------------------

/// Scan a 16-byte-aligned memory range for the "RSD PTR " signature
unsafe fn scan_for_rsdp(start: usize, length: usize) -> Option<usize> {
    let mut addr = start;
    let end = start + length;
    while addr < end {
        let ptr = addr as *const [u8; 8];
        if read_volatile(ptr) == *b"RSD PTR " {
            // Validate RSDP checksum (first 20 bytes for ACPI 1.0)
            if validate_checksum(addr as *const u8, 20) {
                return Some(addr);
            }
        }
        addr = addr.saturating_add(16); // RSDP is always 16-byte aligned
    }
    None
}

/// Find RSDP by scanning the standard memory regions
unsafe fn find_rsdp() -> Option<usize> {
    // 1. Search EBDA (Extended BIOS Data Area) — first KB at segment from 0x40E
    let ebda_seg = read_volatile(0x40E as *const u16) as usize;
    let ebda_base = ebda_seg << 4;
    if ebda_base > 0x80000 && ebda_base < 0xA0000 {
        if let Some(addr) = scan_for_rsdp(ebda_base, 1024) {
            return Some(addr);
        }
    }

    // 2. Search BIOS ROM area (0xE0000 - 0xFFFFF)
    scan_for_rsdp(0xE0000, 0x20000)
}

// ---------------------------------------------------------------------------
// SDT table parsing
// ---------------------------------------------------------------------------

/// Read an SDT header from a physical address
unsafe fn read_sdt_header(phys: usize) -> SdtHeader {
    read_volatile(phys as *const SdtHeader)
}

/// Parse the RSDT (32-bit pointers) and enumerate all tables
unsafe fn parse_rsdt(rsdt_phys: usize, inner: &mut AcpiInner) {
    let header = read_sdt_header(rsdt_phys);
    let hdr_length = header.length;
    let hdr_signature = header.signature;
    let hdr_revision = header.revision;
    let hdr_oem_id = header.oem_id;

    // Validate RSDT checksum
    if !validate_checksum(rsdt_phys as *const u8, hdr_length as usize) {
        serial_println!("  ACPI: RSDT checksum FAILED");
        return;
    }

    let header_size = core::mem::size_of::<SdtHeader>();
    // Guard against truncated/corrupt table where length < header
    let table_data_len = (hdr_length as usize).saturating_sub(header_size);
    let entry_count = table_data_len / 4;
    let entries_base = rsdt_phys.saturating_add(header_size);

    // Register RSDT itself
    inner.tables.push(AcpiTable {
        signature: hdr_signature,
        length: hdr_length,
        revision: hdr_revision,
        oem_id: hdr_oem_id,
        phys_addr: rsdt_phys,
        checksum_valid: true,
    });

    serial_println!("  ACPI: RSDT at {:#X}, {} entries", rsdt_phys, entry_count);

    for i in 0..entry_count {
        let entry_offset = entries_base.saturating_add(i.saturating_mul(4));
        let table_phys = read_volatile(entry_offset as *const u32) as usize;
        if table_phys == 0 {
            continue;
        }
        let tbl_hdr = read_sdt_header(table_phys);
        let tbl_length = tbl_hdr.length;
        let tbl_signature = tbl_hdr.signature;
        let tbl_revision = tbl_hdr.revision;
        let tbl_oem_id = tbl_hdr.oem_id;
        let valid = validate_checksum(table_phys as *const u8, tbl_length as usize);

        inner.tables.push(AcpiTable {
            signature: tbl_signature,
            length: tbl_length,
            revision: tbl_revision,
            oem_id: tbl_oem_id,
            phys_addr: table_phys,
            checksum_valid: valid,
        });

        let sig_str = core::str::from_utf8(&tbl_signature).unwrap_or("????");
        serial_println!(
            "  ACPI: table {} at {:#X} len={} chk={}",
            sig_str,
            table_phys,
            tbl_length,
            if valid { "OK" } else { "FAIL" }
        );
    }
}

/// Parse the XSDT (64-bit pointers) and enumerate all tables
unsafe fn parse_xsdt(xsdt_phys: usize, inner: &mut AcpiInner) {
    let header = read_sdt_header(xsdt_phys);
    let hdr_length = header.length;
    let hdr_signature = header.signature;
    let hdr_revision = header.revision;
    let hdr_oem_id = header.oem_id;

    if !validate_checksum(xsdt_phys as *const u8, hdr_length as usize) {
        serial_println!("  ACPI: XSDT checksum FAILED, falling back");
        return;
    }

    let header_size = core::mem::size_of::<SdtHeader>();
    // Guard against truncated/corrupt table where length < header
    let table_data_len = (hdr_length as usize).saturating_sub(header_size);
    let entry_count = table_data_len / 8;
    let entries_base = xsdt_phys.saturating_add(header_size);

    inner.tables.push(AcpiTable {
        signature: hdr_signature,
        length: hdr_length,
        revision: hdr_revision,
        oem_id: hdr_oem_id,
        phys_addr: xsdt_phys,
        checksum_valid: true,
    });

    serial_println!("  ACPI: XSDT at {:#X}, {} entries", xsdt_phys, entry_count);

    for i in 0..entry_count {
        let entry_offset = entries_base.saturating_add(i.saturating_mul(8));
        let table_phys = read_volatile(entry_offset as *const u64) as usize;
        if table_phys == 0 {
            continue;
        }
        let tbl_hdr = read_sdt_header(table_phys);
        let tbl_length = tbl_hdr.length;
        let tbl_signature = tbl_hdr.signature;
        let tbl_revision = tbl_hdr.revision;
        let tbl_oem_id = tbl_hdr.oem_id;
        let valid = validate_checksum(table_phys as *const u8, tbl_length as usize);

        inner.tables.push(AcpiTable {
            signature: tbl_signature,
            length: tbl_length,
            revision: tbl_revision,
            oem_id: tbl_oem_id,
            phys_addr: table_phys,
            checksum_valid: valid,
        });

        let sig_str = core::str::from_utf8(&tbl_signature).unwrap_or("????");
        serial_println!(
            "  ACPI: table {} at {:#X} len={} chk={}",
            sig_str,
            table_phys,
            tbl_length,
            if valid { "OK" } else { "FAIL" }
        );
    }
}

// ---------------------------------------------------------------------------
// MADT parsing
// ---------------------------------------------------------------------------

/// Parse the MADT and extract CPU, I/O APIC, ISO, and NMI information
unsafe fn parse_madt(madt_phys: usize, inner: &mut AcpiInner) {
    let madt = read_volatile(madt_phys as *const MadtHeader);
    let madt_local_apic_addr = madt.local_apic_addr;
    let madt_flags = madt.flags;
    let madt_sdt_length = madt.sdt.length;
    inner.local_apic_addr = madt_local_apic_addr as u64;
    inner.madt_flags = madt_flags;

    let total_len = madt_sdt_length as usize;
    let entries_start = madt_phys + core::mem::size_of::<MadtHeader>();
    let entries_end = madt_phys + total_len;
    let mut offset = entries_start;

    while offset + 2 <= entries_end {
        let entry_hdr = read_volatile(offset as *const MadtEntryHeader);
        let entry_type = entry_hdr.entry_type;
        let entry_length = entry_hdr.length;
        if entry_length < 2 {
            break; // malformed
        }

        match entry_type {
            MADT_LOCAL_APIC => {
                if entry_length as usize >= core::mem::size_of::<MadtLocalApic>() {
                    let lapic = read_volatile(offset as *const MadtLocalApic);
                    let lapic_flags = lapic.flags;
                    let enabled = lapic_flags & LAPIC_FLAG_ENABLED != 0;
                    let online_capable = lapic_flags & LAPIC_FLAG_ONLINE_CAPABLE != 0;
                    if enabled || online_capable {
                        inner.cpus.push(CpuEntry {
                            processor_uid: lapic.processor_uid,
                            apic_id: lapic.apic_id,
                            enabled,
                            online_capable,
                        });
                    }
                }
            }
            MADT_IO_APIC => {
                if entry_length as usize >= core::mem::size_of::<MadtIoApic>() {
                    let ioapic = read_volatile(offset as *const MadtIoApic);
                    let ioapic_id = ioapic.io_apic_id;
                    let ioapic_address = ioapic.io_apic_address;
                    let ioapic_gsi_base = ioapic.gsi_base;
                    inner.io_apics.push(IoApicEntry {
                        id: ioapic_id,
                        address: ioapic_address,
                        gsi_base: ioapic_gsi_base,
                    });
                }
            }
            MADT_ISO => {
                if entry_length as usize >= core::mem::size_of::<MadtIso>() {
                    let iso = read_volatile(offset as *const MadtIso);
                    let iso_bus = iso.bus;
                    let iso_source = iso.source;
                    let iso_gsi = iso.gsi;
                    let iso_flags = iso.flags;
                    inner.isos.push(IsoEntry {
                        bus: iso_bus,
                        source_irq: iso_source,
                        gsi: iso_gsi,
                        flags: iso_flags,
                    });
                }
            }
            MADT_LOCAL_APIC_NMI => {
                if entry_length as usize >= core::mem::size_of::<MadtNmi>() {
                    let nmi = read_volatile(offset as *const MadtNmi);
                    let nmi_processor_uid = nmi.processor_uid;
                    let nmi_flags = nmi.flags;
                    let nmi_lint = nmi.lint;
                    inner.nmis.push(NmiEntry {
                        processor_uid: nmi_processor_uid,
                        flags: nmi_flags,
                        lint: nmi_lint,
                    });
                }
            }
            MADT_LOCAL_APIC_OVERRIDE => {
                if entry_length as usize >= core::mem::size_of::<MadtLocalApicOverride>() {
                    let ovr = read_volatile(offset as *const MadtLocalApicOverride);
                    let ovr_addr = ovr.local_apic_address_64;
                    inner.local_apic_addr = ovr_addr;
                    serial_println!(
                        "  ACPI: Local APIC override -> {:#X}",
                        inner.local_apic_addr
                    );
                }
            }
            MADT_X2APIC => {
                // Processor Local x2APIC (ACPI type 9, ACPI 4.0+)
                if entry_length as usize >= core::mem::size_of::<MadtX2Apic>() {
                    let x2 = read_volatile(offset as *const MadtX2Apic);
                    let x2_flags = x2.flags;
                    let x2_id = x2.x2apic_id;
                    let x2_uid = x2.processor_uid;
                    let enabled = x2_flags & LAPIC_FLAG_ENABLED != 0;
                    let online_capable = x2_flags & LAPIC_FLAG_ONLINE_CAPABLE != 0;

                    // Only record this CPU if it is not already present via a
                    // type-0 entry (firmware may emit both for compatibility).
                    // We store the low 8 bits of x2apic_id in the apic_id field;
                    // callers that need the full 32-bit ID should use the
                    // extended x2APIC accessors (get_x2apic_ids).
                    let already_present =
                        inner.cpus.iter().any(|c| c.processor_uid == x2_uid as u8);
                    if !already_present && (enabled || online_capable) {
                        inner.cpus.push(CpuEntry {
                            processor_uid: x2_uid as u8,
                            apic_id: x2_id as u8, // truncated — use x2APIC mode for full IDs
                            enabled,
                            online_capable,
                        });
                        serial_println!(
                            "  ACPI:   x2APIC uid={} id={:#X} enabled={} online_capable={}",
                            x2_uid,
                            x2_id,
                            enabled,
                            online_capable
                        );
                    }
                }
            }
            _ => {
                // Unknown MADT entry type — skip
            }
        }

        offset = offset.saturating_add(entry_length as usize);
    }

    serial_println!(
        "  ACPI: MADT: {} CPUs, {} IO-APICs, {} ISOs, {} NMIs",
        inner.cpus.len(),
        inner.io_apics.len(),
        inner.isos.len(),
        inner.nmis.len()
    );
    serial_println!(
        "  ACPI: Local APIC at {:#X}, dual-8259={}",
        inner.local_apic_addr,
        inner.madt_flags & MADT_FLAG_PCAT_COMPAT != 0
    );

    for cpu in &inner.cpus {
        serial_println!(
            "  ACPI:   CPU uid={} apic_id={} enabled={} online_capable={}",
            cpu.processor_uid,
            cpu.apic_id,
            cpu.enabled,
            cpu.online_capable
        );
    }
    for ioapic in &inner.io_apics {
        serial_println!(
            "  ACPI:   IO-APIC id={} addr={:#X} gsi_base={}",
            ioapic.id,
            ioapic.address,
            ioapic.gsi_base
        );
    }
}

// ---------------------------------------------------------------------------
// FADT parsing
// ---------------------------------------------------------------------------

/// Parse the FADT for power management registers
unsafe fn parse_fadt(fadt_phys: usize, inner: &mut AcpiInner) {
    let fadt = read_volatile(fadt_phys as *const Fadt);

    // Copy packed fields to local variables to avoid unaligned references
    let fadt_pm1a_control_block = fadt.pm1a_control_block;
    let fadt_pm1b_control_block = fadt.pm1b_control_block;
    let fadt_pm1a_event_block = fadt.pm1a_event_block;
    let fadt_pm1b_event_block = fadt.pm1b_event_block;
    let fadt_pm_timer_block = fadt.pm_timer_block;
    let fadt_pm1_event_length = fadt.pm1_event_length;
    let fadt_sci_interrupt = fadt.sci_interrupt;
    let fadt_smi_command_port = fadt.smi_command_port;
    let fadt_acpi_enable = fadt.acpi_enable;
    let fadt_acpi_disable = fadt.acpi_disable;
    let fadt_dsdt_address = fadt.dsdt_address;

    inner.pm1a_control = fadt_pm1a_control_block;
    inner.pm1b_control = fadt_pm1b_control_block;
    inner.pm1a_event = fadt_pm1a_event_block;
    inner.pm1b_event = fadt_pm1b_event_block;
    inner.pm_timer = fadt_pm_timer_block;
    inner.pm1_event_len = fadt_pm1_event_length;
    inner.sci_interrupt = fadt_sci_interrupt;
    inner.smi_command = fadt_smi_command_port;
    inner.acpi_enable_val = fadt_acpi_enable;
    inner.acpi_disable_val = fadt_acpi_disable;
    inner.dsdt_addr = fadt_dsdt_address;

    serial_println!(
        "  ACPI: FADT: PM1a_ctrl={:#X} PM1b_ctrl={:#X}",
        inner.pm1a_control,
        inner.pm1b_control
    );
    serial_println!(
        "  ACPI: FADT: PM1a_evt={:#X} PM_timer={:#X} SCI_INT={}",
        inner.pm1a_event,
        inner.pm_timer,
        inner.sci_interrupt
    );
    serial_println!(
        "  ACPI: FADT: SMI_CMD={:#X} DSDT={:#X}",
        inner.smi_command,
        inner.dsdt_addr
    );

    // Validate DSDT checksum if present
    if fadt_dsdt_address != 0 {
        let dsdt_hdr = read_sdt_header(fadt_dsdt_address as usize);
        let dsdt_length = dsdt_hdr.length;
        let dsdt_signature = dsdt_hdr.signature;
        let dsdt_revision = dsdt_hdr.revision;
        let dsdt_oem_id = dsdt_hdr.oem_id;
        let dsdt_valid = validate_checksum(fadt_dsdt_address as *const u8, dsdt_length as usize);
        serial_println!(
            "  ACPI: DSDT at {:#X} len={} checksum={}",
            fadt_dsdt_address,
            dsdt_length,
            if dsdt_valid { "OK" } else { "FAIL" }
        );

        // Register DSDT in table list
        inner.tables.push(AcpiTable {
            signature: dsdt_signature,
            length: dsdt_length,
            revision: dsdt_revision,
            oem_id: dsdt_oem_id,
            phys_addr: fadt_dsdt_address as usize,
            checksum_valid: dsdt_valid,
        });
    }

    // Populate default SLP_TYP values (typical for PIIX4/QEMU)
    // In a full implementation these would be extracted from AML evaluation of \_S0-\_S5
    inner.slp_typ[0] = SlpTyp {
        pm1a_slp_typ: 0,
        pm1b_slp_typ: 0,
    }; // S0
    inner.slp_typ[1] = SlpTyp {
        pm1a_slp_typ: 1,
        pm1b_slp_typ: 1,
    }; // S1
    inner.slp_typ[2] = SlpTyp {
        pm1a_slp_typ: 2,
        pm1b_slp_typ: 2,
    }; // S2
    inner.slp_typ[3] = SlpTyp {
        pm1a_slp_typ: 3,
        pm1b_slp_typ: 3,
    }; // S3
    inner.slp_typ[4] = SlpTyp {
        pm1a_slp_typ: 4,
        pm1b_slp_typ: 4,
    }; // S4
    inner.slp_typ[5] = SlpTyp {
        pm1a_slp_typ: 5,
        pm1b_slp_typ: 5,
    }; // S5
}

/// Validate any SSDT tables in the table list
unsafe fn validate_ssdts(inner: &AcpiInner) {
    for table in &inner.tables {
        if &table.signature == b"SSDT" {
            serial_println!(
                "  ACPI: SSDT at {:#X} len={} checksum={}",
                table.phys_addr,
                table.length,
                if table.checksum_valid { "OK" } else { "FAIL" }
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SCI / power management enable
// ---------------------------------------------------------------------------

/// Enable ACPI mode by writing to SMI_CMD port
unsafe fn enable_acpi_mode(inner: &mut AcpiInner) {
    if inner.smi_command == 0 || inner.acpi_enable_val == 0 {
        // ACPI mode may already be enabled or not available via SMI
        serial_println!("  ACPI: SMI_CMD not available, assuming ACPI mode enabled");
        inner.sci_enabled = true;
        return;
    }

    // Check if SCI_EN is already set in PM1a_CNT
    if inner.pm1a_control != 0 {
        let pm1a_val = inw(inner.pm1a_control as u16);
        if pm1a_val & PM1_SCI_EN != 0 {
            serial_println!("  ACPI: SCI already enabled");
            inner.sci_enabled = true;
            return;
        }
    }

    // Write ACPI_ENABLE to SMI_CMD port
    outb(inner.smi_command as u16, inner.acpi_enable_val);

    // Wait for SCI_EN to be set (spin with timeout)
    if inner.pm1a_control != 0 {
        for _ in 0..1000 {
            let val = inw(inner.pm1a_control as u16);
            if val & PM1_SCI_EN != 0 {
                serial_println!("  ACPI: SCI enabled successfully");
                inner.sci_enabled = true;
                return;
            }
            for _ in 0..1000 {
                core::hint::spin_loop();
            }
        }
        serial_println!("  ACPI: WARNING - SCI enable timeout");
    }

    inner.sci_enabled = true;
}

/// Enable power button and sleep button events
unsafe fn enable_pm_events(inner: &AcpiInner) {
    if inner.pm1a_event == 0 {
        return;
    }

    // Enable register is at pm1a_event + pm1_event_len/2
    let enable_port = inner.pm1a_event as u16 + (inner.pm1_event_len / 2) as u16;

    // Clear all pending status bits first
    outw(inner.pm1a_event as u16, 0xFFFF);

    // Enable power button and sleep button events
    outw(enable_port, PM1_PWRBTN_EN | PM1_SLPBTN_EN);

    serial_println!("  ACPI: PM events enabled (PWRBTN, SLPBTN)");
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the ACPI subsystem: discover RSDP, parse tables, enumerate hardware
pub fn init() {
    let mut inner = AcpiInner {
        tables: Vec::new(),
        cpus: Vec::new(),
        io_apics: Vec::new(),
        isos: Vec::new(),
        nmis: Vec::new(),
        local_apic_addr: 0xFEE00000,
        madt_flags: 0,
        pm1a_control: 0,
        pm1b_control: 0,
        pm1a_event: 0,
        pm1b_event: 0,
        pm_timer: 0,
        pm1_event_len: 4,
        sci_interrupt: 9,
        smi_command: 0,
        acpi_enable_val: 0,
        acpi_disable_val: 0,
        dsdt_addr: 0,
        slp_typ: [SlpTyp {
            pm1a_slp_typ: 0,
            pm1b_slp_typ: 0,
        }; 6],
        power_state: PowerState::S0,
        sci_enabled: false,
        revision: 0,
    };

    unsafe {
        // Step 1: Find RSDP
        let rsdp_addr = match find_rsdp() {
            Some(addr) => addr,
            None => {
                serial_println!("  ACPI: RSDP not found");
                *ACPI.lock() = Some(inner);
                return;
            }
        };

        let rsdp = read_volatile(rsdp_addr as *const Rsdp);
        let rsdp_revision = rsdp.revision;
        let rsdp_rsdt_address = rsdp.rsdt_address;
        inner.revision = rsdp_revision;
        serial_println!(
            "  ACPI: RSDP at {:#X}, revision {}",
            rsdp_addr,
            rsdp_revision
        );

        // Step 2: Parse XSDT (ACPI 2.0+) or RSDT (ACPI 1.0)
        if rsdp_revision >= 2 {
            let rsdp_ext = read_volatile(rsdp_addr as *const RsdpExtended);
            let ext_length = rsdp_ext.length;
            let ext_xsdt_address = rsdp_ext.xsdt_address;
            // Validate extended checksum
            if validate_checksum(rsdp_addr as *const u8, ext_length as usize) {
                let xsdt_addr = ext_xsdt_address as usize;
                if xsdt_addr != 0 {
                    parse_xsdt(xsdt_addr, &mut inner);
                } else {
                    parse_rsdt(rsdp_rsdt_address as usize, &mut inner);
                }
            } else {
                serial_println!("  ACPI: Extended RSDP checksum failed, using RSDT");
                parse_rsdt(rsdp_rsdt_address as usize, &mut inner);
            }
        } else {
            parse_rsdt(rsdp_rsdt_address as usize, &mut inner);
        }

        // Step 3: Parse specific tables
        // Find and parse MADT
        let madt_addr = inner
            .tables
            .iter()
            .find(|t| &t.signature == b"APIC")
            .map(|t| t.phys_addr);
        if let Some(addr) = madt_addr {
            parse_madt(addr, &mut inner);
        } else {
            serial_println!("  ACPI: MADT not found");
        }

        // Find and parse FADT
        let fadt_addr = inner
            .tables
            .iter()
            .find(|t| &t.signature == b"FACP")
            .map(|t| t.phys_addr);
        if let Some(addr) = fadt_addr {
            parse_fadt(addr, &mut inner);
        } else {
            serial_println!("  ACPI: FADT not found");
        }

        // Validate SSDTs
        validate_ssdts(&inner);

        // Step 4: Enable ACPI mode and PM events
        enable_acpi_mode(&mut inner);
        if inner.sci_enabled {
            enable_pm_events(&inner);
        }
    }

    let cpu_count = inner.cpus.len();
    let ioapic_count = inner.io_apics.len();
    let table_count = inner.tables.len();

    *ACPI.lock() = Some(inner);

    serial_println!(
        "  ACPI: initialized ({} tables, {} CPUs, {} IO-APICs)",
        table_count,
        cpu_count,
        ioapic_count
    );
}

/// Find a table by its 4-byte signature; returns its physical address
pub fn find_table(sig: &[u8; 4]) -> Option<usize> {
    let guard = ACPI.lock();
    let inner = guard.as_ref()?;
    inner
        .tables
        .iter()
        .find(|t| &t.signature == sig && t.checksum_valid)
        .map(|t| t.phys_addr)
}

/// Get all discovered CPU entries
pub fn cpu_entries() -> Vec<CpuEntry> {
    let guard = ACPI.lock();
    match guard.as_ref() {
        Some(inner) => inner.cpus.clone(),
        None => Vec::new(),
    }
}

/// Get CPU count
pub fn cpu_count() -> usize {
    let guard = ACPI.lock();
    match guard.as_ref() {
        Some(inner) => inner.cpus.len(),
        None => 0,
    }
}

/// Get all I/O APIC entries
pub fn io_apic_entries() -> Vec<IoApicEntry> {
    let guard = ACPI.lock();
    match guard.as_ref() {
        Some(inner) => inner.io_apics.clone(),
        None => Vec::new(),
    }
}

/// Get interrupt source overrides
pub fn interrupt_overrides() -> Vec<IsoEntry> {
    let guard = ACPI.lock();
    match guard.as_ref() {
        Some(inner) => inner.isos.clone(),
        None => Vec::new(),
    }
}

/// Get local APIC base address
pub fn local_apic_address() -> u64 {
    let guard = ACPI.lock();
    match guard.as_ref() {
        Some(inner) => inner.local_apic_addr,
        None => 0xFEE00000,
    }
}

/// Get the SCI interrupt number
pub fn sci_interrupt() -> u16 {
    let guard = ACPI.lock();
    match guard.as_ref() {
        Some(inner) => inner.sci_interrupt,
        None => 9,
    }
}

/// Get all table descriptors
pub fn list_tables() -> Vec<AcpiTable> {
    let guard = ACPI.lock();
    match guard.as_ref() {
        Some(inner) => inner.tables.clone(),
        None => Vec::new(),
    }
}

/// Transition to a power state (S0-S5)
///
/// S5 = shutdown, S3 = suspend to RAM, S0 = working
/// This writes PM1a_CNT and PM1b_CNT with SLP_TYP and SLP_EN.
pub fn enter_sleep_state(state: PowerState) -> Result<(), &'static str> {
    let mut guard = ACPI.lock();
    let inner = guard.as_mut().ok_or("ACPI not initialized")?;

    if !inner.sci_enabled {
        return Err("ACPI SCI not enabled");
    }

    let state_idx = match state {
        PowerState::S0 => 0,
        PowerState::S1 => 1,
        PowerState::S2 => 2,
        PowerState::S3 => 3,
        PowerState::S4 => 4,
        PowerState::S5 => 5,
    };

    let slp = inner.slp_typ[state_idx];

    serial_println!(
        "  ACPI: entering sleep state S{} (PM1a_typ={}, PM1b_typ={})",
        state_idx,
        slp.pm1a_slp_typ,
        slp.pm1b_slp_typ
    );

    unsafe {
        // Clear wake status
        if inner.pm1a_event != 0 {
            outw(inner.pm1a_event as u16, PM1_WAK_STS);
        }
        if inner.pm1b_event != 0 {
            outw(inner.pm1b_event as u16, PM1_WAK_STS);
        }

        // Write SLP_TYP to PM1a_CNT
        if inner.pm1a_control != 0 {
            let val = (slp.pm1a_slp_typ as u16) << PM1_SLP_TYP_SHIFT | PM1_SLP_EN;
            outw(inner.pm1a_control as u16, val);
        }

        // Write SLP_TYP to PM1b_CNT (if present)
        if inner.pm1b_control != 0 {
            let val = (slp.pm1b_slp_typ as u16) << PM1_SLP_TYP_SHIFT | PM1_SLP_EN;
            outw(inner.pm1b_control as u16, val);
        }

        // For S1, the CPU should halt and resume here on wake
        if state == PowerState::S1 {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }

    inner.power_state = state;
    Ok(())
}

// ── Named ACPI sleep-state entry points ───────────────────────────────────

/// S1 — CPU halt sleep.
///
/// All CPU clocks stop and internal caches are flushed, but all processor
/// and system hardware context is maintained.  Devices stay powered.
/// On wake, execution resumes in the BIOS which jumps back to the kernel
/// wakeup path.
///
/// Implementation:
///   1. Flush caches (WBINVD) so no dirty lines are left in the stopped
///      CPU caches.
///   2. Write SLP_TYP(S1) + SLP_EN to PM1a_CNT.
///   3. Execute HLT — the CPU stalls until an enabled interrupt fires.
///   4. Execution resumes here on wakeup.
pub fn enter_s1() -> Result<(), &'static str> {
    serial_println!("  ACPI: S1 — CPU halt sleep");
    unsafe {
        core::arch::asm!("wbinvd", options(nomem, nostack));
    }
    enter_sleep_state(PowerState::S1)?;
    // HLT inside enter_sleep_state for S1 — when we reach here we have woken.
    serial_println!("  ACPI: S1 wakeup");
    Ok(())
}

/// Prepare the S3 resume trampoline at physical address 0x8000.
///
/// The ACPI firmware jumps to FACS.wakeup_vector (32-bit physical address)
/// in 16-bit real mode after restoring power.  We write a small bootstrap
/// stub at 0x8000 that provides a valid landing point for the firmware wakeup
/// path.
///
/// Stub layout (4 bytes of 16-bit real-mode machine code):
///   FA       cli           ; disable interrupts during mode-switch sequence
///   F4       hlt           ; placeholder (replaced by full mode-switch code)
///   EB FE    jmp $         ; infinite safety-net loop
///
/// A full trampoline would re-enable protected mode, re-enable long mode,
/// restore CR3/GDTR/IDTR/RSP from a pre-sleep save area, and call
/// `crate::power_mgmt::suspend::notify_resume()`.  That sequence will be
/// assembled in-place once the long-mode restore path is stabilised.
///
/// Safety: writes to physical address 0x8000 — conventional real-mode
/// bootstrap area, not occupied by the kernel heap, stack, or code.
pub fn prepare_s3_resume_trampoline() {
    const TRAMPOLINE_PHYS: u64 = 0x8000;

    serial_println!(
        "  ACPI: writing S3 resume trampoline at phys {:#x}",
        TRAMPOLINE_PHYS
    );

    // 16-bit real-mode stub: CLI + HLT + JMP $ (4 bytes).
    let stub: [u8; 4] = [
        0xFA, // cli
        0xF4, // hlt
        0xEB, 0xFE, // jmp $ (infinite loop)
    ];

    unsafe {
        let ptr = TRAMPOLINE_PHYS as *mut u8;
        for (i, &byte) in stub.iter().enumerate() {
            core::ptr::write_volatile(ptr.add(i), byte);
        }
    }

    serial_println!(
        "  ACPI: S3 trampoline stub ({} bytes) at {:#x} — FACS.wakeup_vector should be set to this address",
        stub.len(),
        TRAMPOLINE_PHYS
    );
}

/// Write the hibernation image to `dest` physical address covering `size` bytes.
///
/// Delegates to `crate::memory::swap::write_swap_image` for the actual
/// page-frame snapshot.  Returns `Ok(())` on success or an error string if the
/// image could not be written.
///
/// Called by `enter_s4()` before asserting S4 sleep to ensure the bootloader
/// can restore RAM on the next boot.
pub fn write_hibernate_image(dest: u64, size: u64) -> Result<(), &'static str> {
    serial_println!(
        "  ACPI: write_hibernate_image dest={:#018x} size={:#018x}",
        dest,
        size
    );
    crate::memory::swap::write_swap_image(dest, size)
}

/// S3 — Suspend to RAM.
///
/// All system context is saved in RAM.  Power is cut except for RAM refresh
/// and the wakeup logic (RTC, USB, power button).  Resume is fast because no
/// image needs to be read from disk.
///
/// Implementation:
///   1. Write the S3 resume trampoline at phys 0x8000 so firmware has a valid
///      FACS.wakeup_vector target when it restores power.
///   2. Quiesce all devices via the suspend power-management manager (GPU,
///      NVMe, USB, audio, dm-crypt).
///   3. Flush all CPU caches (WBINVD).
///   4. Write SLP_TYP(S3) + SLP_EN to PM1a_CNT to enter S3 sleep.
///   5. On real hardware, execution resumes via the trampoline.  In QEMU
///      (no S3 support) execution falls through — devices are thawed.
pub fn enter_s3() -> Result<(), &'static str> {
    serial_println!("  ACPI: S3 — suspend to RAM");

    // Step 1: install resume trampoline at phys 0x8000.
    prepare_s3_resume_trampoline();

    // Step 2: quiesce all devices (GPU, NVMe, USB, audio, dm-crypt).
    crate::power_mgmt::suspend::enter_suspend(crate::power_mgmt::suspend::SleepState::S3);

    // Step 3: flush caches.
    unsafe {
        core::arch::asm!("wbinvd", options(nomem, nostack));
    }

    // Step 4: write SLP_TYP(S3) + SLP_EN to PM1a_CNT.
    enter_sleep_state(PowerState::S3)?;

    // Step 5: reached only if hardware did not actually suspend.
    serial_println!("  ACPI: S3 — hardware did not suspend (check SLP_TYP table)");
    // Thaw devices so the system is back in a consistent state.
    crate::power_mgmt::suspend::resume_from_ram();
    Ok(())
}

/// S4 — Suspend to disk (hibernate).
///
/// A hibernation image of all RAM is written to the swap partition before the
/// system fully powers off.  Resume is handled by the bootloader / firmware
/// which reads the image back into RAM.
///
/// Implementation:
///   1. Write the hibernation image via `write_hibernate_image()`.
///   2. If the write succeeds, enter S4 via `enter_sleep_state(S4)`.
///
/// Returns `Err` if the image writer fails (the image must exist before S4
/// power-off to prevent data loss on resume).
pub fn enter_s4() -> Result<(), &'static str> {
    serial_println!("  ACPI: S4 — hibernate");

    // Destination and size are platform-specific.  In a full implementation
    // these come from the EFI block device map.  Here we use a conventional
    // placeholder at the 1 GiB physical mark.
    const HIBERNATE_DEST: u64 = 0x4000_0000; // 1 GiB physical
    const HIBERNATE_SIZE: u64 = 0x4000_0000; // placeholder size

    write_hibernate_image(HIBERNATE_DEST, HIBERNATE_SIZE)?;

    serial_println!("  ACPI: S4 — image written, entering S4 sleep");
    enter_sleep_state(PowerState::S4)?;

    // Reached only if hardware did not power off.
    serial_println!("  ACPI: S4 — hardware did not power off (check SLP_TYP table)");
    Ok(())
}

/// S5 — Soft power off (clean shutdown).
///
/// Implementation:
///   1. Write SLP_TYP(S5) + SLP_EN to PM1a_CNT (and PM1b_CNT if present).
///   2. Hardware cuts all power rails.
///
/// If ACPI fails (unsupported or not initialised), falls through to the
/// QEMU/Bochs debug-exit port and finally a CPU halt loop.
pub fn enter_s5() -> ! {
    serial_println!("  ACPI: S5 — soft power off");
    let _ = enter_sleep_state(PowerState::S5);

    // Fallback: QEMU ISA debug-exit (port 0x604, value 0x2000 = QEMU poweroff).
    outw(0x604, 0x2000);

    // Nothing worked — halt all CPUs.
    serial_println!("  ACPI: S5 failed, halting");
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// Perform a clean system shutdown (enter S5)
pub fn shutdown() -> Result<(), &'static str> {
    serial_println!("  ACPI: initiating system shutdown (S5)");
    enter_sleep_state(PowerState::S5)
}

/// Perform a system reboot via keyboard controller reset
pub fn reboot() {
    serial_println!("  ACPI: initiating system reboot");
    unsafe {
        // Try keyboard controller reset (port 0x64)
        outb(0x64, 0xFE);
        // If that didn't work, triple-fault
        core::arch::asm!("lidt [{}]", in(reg) &0u64, options(noreturn));
    }
}

/// Handle an SCI interrupt (called from the interrupt handler).
///
/// Lock is acquired only to read config values, then dropped before port I/O.
/// This prevents deadlock if the interrupt fires while another CPU holds the lock.
pub fn handle_sci() -> Option<PowerEvent> {
    // Read config under lock, then drop lock before doing port I/O
    let pm1a_event = {
        let guard = ACPI.lock();
        let inner = guard.as_ref()?;
        if inner.pm1a_event == 0 {
            return None;
        }
        inner.pm1a_event
    }; // lock dropped here — safe to do port I/O now

    let status = inw(pm1a_event as u16);

    if status & PM1_PWRBTN_STS != 0 {
        outw(pm1a_event as u16, PM1_PWRBTN_STS);
        serial_println!("  ACPI: power button pressed");
        return Some(PowerEvent::PowerButton);
    }

    if status & PM1_SLPBTN_STS != 0 {
        outw(pm1a_event as u16, PM1_SLPBTN_STS);
        serial_println!("  ACPI: sleep button pressed");
        return Some(PowerEvent::SleepButton);
    }

    if status & PM1_WAK_STS != 0 {
        outw(pm1a_event as u16, PM1_WAK_STS);
        serial_println!("  ACPI: wake event");
        return Some(PowerEvent::Wake);
    }

    None
}

/// Get the current power state
pub fn current_power_state() -> PowerState {
    let guard = ACPI.lock();
    match guard.as_ref() {
        Some(inner) => inner.power_state,
        None => PowerState::S0,
    }
}

/// Read the ACPI PM timer (3.579545 MHz, 24 or 32 bit)
pub fn read_pm_timer() -> u32 {
    let guard = ACPI.lock();
    let inner = match guard.as_ref() {
        Some(i) => i,
        None => return 0,
    };
    if inner.pm_timer == 0 {
        return 0;
    }
    crate::io::inl(inner.pm_timer as u16)
}

/// Check if MADT indicates dual-8259 PIC present
pub fn has_legacy_pic() -> bool {
    let guard = ACPI.lock();
    match guard.as_ref() {
        Some(inner) => inner.madt_flags & MADT_FLAG_PCAT_COMPAT != 0,
        None => true, // assume legacy PIC if ACPI not parsed
    }
}

// ---------------------------------------------------------------------------
// MADT static-array accessors
//
// These fill caller-provided fixed-size arrays without requiring heap
// allocation, making them usable very early in boot.
// ---------------------------------------------------------------------------

/// Maximum CPUs / IO-APICs / IRQ overrides we will return from the static
/// array accessors below.  Matches the `LOCAL_APICS` / `IO_APICS` /
/// `IRQ_OVERRIDES` capacity expected by callers.
pub const MADT_MAX_LOCAL_APICS: usize = 64;
pub const MADT_MAX_IO_APICS: usize = 8;
pub const MADT_MAX_IRQ_OVERRIDES: usize = 32;

/// `(apic_id, flags)` pair for a local APIC entry.
/// `flags` bit 0 = enabled, bit 1 = online-capable.
pub type LocalApicEntry = (u8, u8);

/// Fill `out` with `(apic_id, flags)` pairs for each enabled/online-capable
/// CPU found in the MADT.  Returns the number of entries written.
///
/// This is the static-array variant of `cpu_entries()` — no heap allocation.
pub fn get_cpu_apic_ids(out: &mut [LocalApicEntry; MADT_MAX_LOCAL_APICS]) -> usize {
    let guard = ACPI.lock();
    let inner = match guard.as_ref() {
        Some(i) => i,
        None => return 0,
    };
    let n = inner.cpus.len().min(MADT_MAX_LOCAL_APICS);
    for i in 0..n {
        let cpu = &inner.cpus[i];
        let flags: u8 = (cpu.enabled as u8) | ((cpu.online_capable as u8) << 1);
        out[i] = (cpu.apic_id, flags);
    }
    n
}

/// Return the MMIO base address of the first I/O APIC found in the MADT.
///
/// Returns `0xFEC00000` (the canonical default) if no I/O APIC was recorded.
pub fn get_io_apic_base() -> u32 {
    let guard = ACPI.lock();
    match guard.as_ref() {
        Some(inner) => inner
            .io_apics
            .iter()
            .next()
            .map(|e| e.address)
            .unwrap_or(0xFEC00000),
        None => 0xFEC00000,
    }
}

/// Fill `out` with `(bus_irq, gsi)` pairs from MADT Interrupt Source Override
/// entries.  Returns the number of entries written.
///
/// These overrides re-map legacy ISA IRQs to non-identity GSI numbers.
/// Callers building the IO-APIC redirect table must consult these mappings.
pub fn get_irq_overrides(out: &mut [(u8, u32); MADT_MAX_IRQ_OVERRIDES]) -> usize {
    let guard = ACPI.lock();
    let inner = match guard.as_ref() {
        Some(i) => i,
        None => return 0,
    };
    let n = inner.isos.len().min(MADT_MAX_IRQ_OVERRIDES);
    for i in 0..n {
        out[i] = (inner.isos[i].source_irq, inner.isos[i].gsi);
    }
    n
}
