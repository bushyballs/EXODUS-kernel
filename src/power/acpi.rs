use crate::boot_protocol;
use crate::sync::Mutex;
/// ACPI table parsing
///
/// Finds and parses ACPI tables from firmware to discover:
///   - Power management registers (PM1a/PM1b)
///   - Sleep control registers
///   - Reset mechanism
///   - CPU info, interrupt routing, etc.
use crate::{serial_print, serial_println};
use alloc::string::String;

static ACPI_INFO: Mutex<Option<AcpiInfo>> = Mutex::new(None);

/// Parsed ACPI information
#[derive(Debug, Clone)]
pub struct AcpiInfo {
    pub oem_id: String,
    /// PM1a control port (for sleep/shutdown)
    pub pm1a_cnt_blk: u16,
    /// PM1b control port
    pub pm1b_cnt_blk: u16,
    /// SLP_TYPa value for S5 (shutdown)
    pub slp_typa_s5: u16,
    /// SLP_TYPb value for S5
    pub slp_typb_s5: u16,
    /// ACPI enable port
    pub smi_cmd: u16,
    /// Value to write to enable ACPI
    pub acpi_enable: u8,
    /// Reset register address
    pub reset_reg: u64,
    /// Reset value
    pub reset_val: u8,
    /// Century register in CMOS
    pub century: u8,
}

/// RSDP structure (Root System Description Pointer)
#[repr(C, packed)]
struct Rsdp {
    signature: [u8; 8], // "RSD PTR "
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
}

/// RSDT/XSDT header
#[repr(C, packed)]
struct AcpiHeader {
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

/// FADT (Fixed ACPI Description Table) — partial
#[repr(C, packed)]
struct Fadt {
    header: AcpiHeader,
    firmware_ctrl: u32,
    dsdt: u32,
    _reserved: u8,
    preferred_pm_profile: u8,
    sci_int: u16,
    smi_cmd: u32,
    acpi_enable: u8,
    acpi_disable: u8,
    s4bios_req: u8,
    pstate_cnt: u8,
    pm1a_evt_blk: u32,
    pm1b_evt_blk: u32,
    pm1a_cnt_blk: u32,
    pm1b_cnt_blk: u32,
    pm2_cnt_blk: u32,
    pm_tmr_blk: u32,
    gpe0_blk: u32,
    gpe1_blk: u32,
    pm1_evt_len: u8,
    pm1_cnt_len: u8,
    pm2_cnt_len: u8,
    pm_tmr_len: u8,
    gpe0_blk_len: u8,
    gpe1_blk_len: u8,
    gpe1_base: u8,
    cst_cnt: u8,
    p_lvl2_lat: u16,
    p_lvl3_lat: u16,
    flush_size: u16,
    flush_stride: u16,
    duty_offset: u8,
    duty_width: u8,
    day_alarm: u8,
    month_alarm: u8,
    century: u8,
}

/// Search for RSDP in BIOS memory regions
fn find_rsdp() -> Option<u64> {
    if let Some(info) = boot_protocol::boot_info() {
        if info.rsdp_address != 0 {
            return Some(info.rsdp_address);
        }
    }

    // RSDP is in EBDA (Extended BIOS Data Area) or 0xE0000-0xFFFFF
    let regions = [
        (0x000E0000u64, 0x00100000u64), // Main BIOS area
    ];

    for (start, end) in regions {
        let mut addr = start;
        while addr < end {
            let sig = unsafe { core::slice::from_raw_parts(addr as *const u8, 8) };
            if sig == b"RSD PTR " {
                // Verify checksum
                let rsdp_bytes = unsafe { core::slice::from_raw_parts(addr as *const u8, 20) };
                let sum: u8 = rsdp_bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b));
                if sum == 0 {
                    return Some(addr);
                }
            }
            addr += 16; // RSDP is always 16-byte aligned
        }
    }
    None
}

/// Parse ACPI tables
fn parse_acpi() -> Option<AcpiInfo> {
    let rsdp_addr = find_rsdp()?;
    // Use read_unaligned for packed ACPI structures
    let rsdp: Rsdp = unsafe { (rsdp_addr as *const Rsdp).read_unaligned() };

    let oem_id = String::from_utf8_lossy(&rsdp.oem_id).trim().into();

    let rsdt_addr = rsdp.rsdt_address as u64;
    let rsdt_header: AcpiHeader = unsafe { (rsdt_addr as *const AcpiHeader).read_unaligned() };
    let entry_count = (rsdt_header.length as usize - core::mem::size_of::<AcpiHeader>()) / 4;

    let entries_ptr = (rsdt_addr + core::mem::size_of::<AcpiHeader>() as u64) as *const u32;

    // Find FADT (signature "FACP")
    for i in 0..entry_count {
        let table_addr = unsafe { entries_ptr.add(i).read_unaligned() } as u64;
        let header: AcpiHeader = unsafe { (table_addr as *const AcpiHeader).read_unaligned() };

        if &header.signature == b"FACP" {
            let fadt: Fadt = unsafe { (table_addr as *const Fadt).read_unaligned() };

            // Parse S5 sleep type from DSDT (simplified — real impl parses AML)
            let slp_typa_s5 = 5; // typical value
            let slp_typb_s5 = 0;

            return Some(AcpiInfo {
                oem_id,
                pm1a_cnt_blk: fadt.pm1a_cnt_blk as u16,
                pm1b_cnt_blk: fadt.pm1b_cnt_blk as u16,
                slp_typa_s5,
                slp_typb_s5,
                smi_cmd: fadt.smi_cmd as u16,
                acpi_enable: fadt.acpi_enable,
                reset_reg: 0, // from FADT reset register (extended field)
                reset_val: 0,
                century: fadt.century,
            });
        }
    }

    None
}

pub fn init() {
    match parse_acpi() {
        Some(info) => {
            serial_println!("    [acpi] Found ACPI tables (OEM: {})", info.oem_id);
            serial_println!(
                "    [acpi] PM1a={:#x}, SMI={:#x}",
                info.pm1a_cnt_blk,
                info.smi_cmd
            );
            *ACPI_INFO.lock() = Some(info);
        }
        None => {
            serial_println!("    [acpi] No ACPI tables found (using fallback)");
        }
    }
}

/// Get ACPI info
pub fn get_info() -> Option<AcpiInfo> {
    ACPI_INFO.lock().clone()
}
