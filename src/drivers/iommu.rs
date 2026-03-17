use crate::serial_println;
/// Intel VT-d IOMMU driver for Genesis
///
/// Provides DMA remapping for device isolation.
/// Reads DMAR ACPI table, initializes root/context tables,
/// and manages second-level page tables (SLPTE) for each device domain.
///
/// DMAR = DMA Remapping Reporting (ACPI table, signature "DMAR")
/// DRHD = DMA Remapping Hardware unit Definition
/// RMRR = Reserved Memory Region Reporting (BIOS-reserved ranges)
/// ANDD = ACPI Name-space Device Declaration
///
/// Registers (at DRHD base address):
///   0x00  VER_REG         — version
///   0x08  CAP_REG         — capability (64-bit)
///   0x10  ECAP_REG        — extended capability (64-bit)
///   0x18  GCMD_REG        — global command (32-bit, write-only)
///   0x1C  GSTS_REG        — global status  (32-bit, read-only)
///   0x20  RTADDR_REG      — root table address (64-bit)
///   0x28  CCMD_REG        — context command (64-bit)
///   0xB8  IOTLB_REG       — IOTLB invalidation (64-bit, at ECAP.IRO offset)
///
/// Inspired by: Intel VT-d specification (rev 3.4) and Linux
/// drivers/iommu/intel/iommu.c. All code is original.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// DMAR ACPI table structures
// ---------------------------------------------------------------------------

/// DMAR table signature bytes ("DMAR")
const DMAR_SIGNATURE: [u8; 4] = *b"DMAR";

/// Remapping structure types in the DMAR table
const DRHD_TYPE: u16 = 0; // DMA Remapping Hardware unit Definition
const RMRR_TYPE: u16 = 1; // Reserved Memory Region Reporting
const ANDD_TYPE: u16 = 3; // ACPI Namespace Device Declaration

/// DRHD flag: Include all PCI devices on the segment
const DRHD_FLAG_INCLUDE_PCI_ALL: u8 = 1;

// ---------------------------------------------------------------------------
// MMIO register offsets inside a DRHD unit
// ---------------------------------------------------------------------------

const VER_REG: u32 = 0x000;
const CAP_REG: u32 = 0x008;
const ECAP_REG: u32 = 0x010;
const GCMD_REG: u32 = 0x018;
const GSTS_REG: u32 = 0x01C;
const RTADDR_REG: u32 = 0x020;
const CCMD_REG: u32 = 0x028;

/// GCMD_REG bits
const GCMD_TE: u32 = 1 << 31; // Translation Enable
const GCMD_SRTP: u32 = 1 << 30; // Set Root Table Pointer

/// GSTS_REG bits
const GSTS_TES: u32 = 1 << 31; // Translation Enable Status
const GSTS_RTPS: u32 = 1 << 30; // Root Table Pointer Status

// ---------------------------------------------------------------------------
// Root / Context table sizes
// ---------------------------------------------------------------------------

/// Root table: 256 entries × 16 bytes = 4096 bytes (one page)
const ROOT_ENTRY_SIZE: usize = 16;
const ROOT_TABLE_ENTRIES: usize = 256;
const ROOT_TABLE_SIZE: usize = ROOT_TABLE_ENTRIES * ROOT_ENTRY_SIZE;

/// Context table: 256 entries × 32 bytes = 8192 bytes (two pages)
const CTX_ENTRY_SIZE: usize = 32;
const CTX_TABLE_ENTRIES: usize = 256;
const CTX_TABLE_SIZE: usize = CTX_TABLE_ENTRIES * CTX_ENTRY_SIZE;

/// Second-level page table entry size
const SLPTE_SIZE: usize = 8;

/// Page size used for SLPT (4 KiB)
const PAGE_SIZE: u64 = 0x1000;

// ---------------------------------------------------------------------------
// DRHD descriptor
// ---------------------------------------------------------------------------

/// Descriptor for one DMA Remapping Hardware unit
#[derive(Debug, Clone, Copy)]
pub struct Drhd {
    /// MMIO base address of the DRHD registers
    pub base_address: u64,
    /// PCI segment group number
    pub segment: u16,
    /// DRHD flags (bit 0 = INCLUDE_PCI_ALL)
    pub flags: u8,
    /// Whether this DRHD has been successfully initialized
    pub initialized: bool,
    /// IOTLB register offset (from ECAP.IRO field × 16)
    pub iotlb_reg_offset: u32,
}

impl Drhd {
    const fn empty() -> Self {
        Drhd {
            base_address: 0,
            segment: 0,
            flags: 0,
            initialized: false,
            iotlb_reg_offset: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Domain descriptor
// ---------------------------------------------------------------------------

/// An IOMMU domain — one domain per isolated device (or shared among trusted peers)
#[derive(Clone, Copy)]
struct IommuDomain {
    /// Domain ID (also called PASID in newer specs; here just an index 1-255)
    domain_id: u16,
    /// PCI function this domain was created for (bus << 8 | dev << 3 | func)
    bdf: u16,
    /// Physical address of the second-level page table root (4 KiB aligned)
    sl_root_phys: u64,
    /// Number of active mappings in this domain
    mapping_count: u32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Maximum DRHD units supported
const MAX_DRHD: usize = 8;

/// Maximum concurrent IOMMU domains (one per isolated device)
const MAX_DOMAINS: usize = 64;

/// Root table storage: one 4 KiB region per DRHD (up to MAX_DRHD)
/// Statically allocated; aligned to 4 KiB.
#[repr(align(4096))]
#[derive(Clone, Copy)]
struct AlignedPage([u8; 4096]);

static mut ROOT_TABLES: [[AlignedPage; 1]; MAX_DRHD] = {
    // const-initialise all to zero
    let zero = AlignedPage([0u8; 4096]);
    // SAFETY: zero-initialised bytes are valid for all-zero bitpattern types
    [[zero]; MAX_DRHD]
};

/// Second-level page table pool: one 4 KiB page per domain (simplification;
/// a real driver would allocate from the frame allocator).
#[repr(align(4096))]
#[derive(Clone, Copy)]
struct SlptPage([u8; 4096]);

static mut SLPT_POOL: [SlptPage; MAX_DOMAINS] = {
    let zero = SlptPage([0u8; 4096]);
    [zero; MAX_DOMAINS] // const-init
};

/// DRHD unit registry
static DRHD_UNITS: Mutex<[Option<Drhd>; MAX_DRHD]> = Mutex::new([None; MAX_DRHD]);

/// Domain registry
static DOMAINS: Mutex<[Option<IommuDomain>; MAX_DOMAINS]> = Mutex::new(
    // const-init with None
    {
        const NONE_DOMAIN: Option<IommuDomain> = None;
        [NONE_DOMAIN; MAX_DOMAINS]
    },
);

/// Global flag: translation is active (at least one DRHD has TE bit set)
static IOMMU_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Next domain ID to allocate (1-based; 0 is reserved by VT-d spec)
static NEXT_DOMAIN_ID: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// Low-level MMIO accessors
// ---------------------------------------------------------------------------

/// Read a 32-bit MMIO register from a DRHD base address
#[inline(always)]
unsafe fn mmio_read32(base: u64, offset: u32) -> u32 {
    let addr = (base + offset as u64) as *const u32;
    core::ptr::read_volatile(addr)
}

/// Write a 32-bit MMIO register to a DRHD base address
#[inline(always)]
unsafe fn mmio_write32(base: u64, offset: u32, value: u32) {
    let addr = (base + offset as u64) as *mut u32;
    core::ptr::write_volatile(addr, value);
}

/// Read a 64-bit MMIO register
#[inline(always)]
unsafe fn mmio_read64(base: u64, offset: u32) -> u64 {
    let addr = (base + offset as u64) as *const u64;
    core::ptr::read_volatile(addr)
}

/// Write a 64-bit MMIO register
#[inline(always)]
unsafe fn mmio_write64(base: u64, offset: u32, value: u64) {
    let addr = (base + offset as u64) as *mut u64;
    core::ptr::write_volatile(addr, value);
}

// ---------------------------------------------------------------------------
// TASK 1 — parse_dmar_table
// ---------------------------------------------------------------------------

/// Parse the Intel DMAR ACPI table starting at `dmar_ptr`.
///
/// The DMAR table layout:
///   Offset 0: signature (4 bytes, "DMAR")
///   Offset 4: length (4 bytes, total table size)
///   Offset 8: revision (1 byte)
///   Offset 9: checksum (1 byte)
///   Offset 10: OEM ID (6 bytes)
///   Offset 16: OEM table ID (8 bytes)
///   Offset 24: OEM revision (4 bytes)
///   Offset 28: creator ID (4 bytes)
///   Offset 32: creator revision (4 bytes)
///   Offset 36: host address width (1 byte)
///   Offset 37: flags (1 byte)
///   Offset 38: reserved (10 bytes)
///   Offset 48: remapping structures (variable length)
///
/// Remapping structure header:
///   Offset 0: type  (2 bytes)
///   Offset 2: length (2 bytes)
///
/// DRHD body (starts after header):
///   Offset 4: flags (1 byte)
///   Offset 5: reserved (1 byte)
///   Offset 6: segment number (2 bytes)
///   Offset 8: register base address (8 bytes)
///   Offset 16: device scope list (variable)
///
/// # Safety
/// `dmar_ptr` must point to a valid DMAR ACPI table.
pub unsafe fn parse_dmar_table(dmar_ptr: *const u8) {
    if dmar_ptr.is_null() {
        serial_println!("  [iommu] parse_dmar_table: null pointer");
        return;
    }

    // Verify signature
    let sig = core::slice::from_raw_parts(dmar_ptr, 4);
    if sig != DMAR_SIGNATURE {
        serial_println!("  [iommu] parse_dmar_table: bad signature ({:?})", sig);
        return;
    }

    // Read table length (little-endian u32 at offset 4)
    let table_len = core::ptr::read_unaligned(dmar_ptr.add(4) as *const u32) as usize;
    if table_len < 48 {
        serial_println!(
            "  [iommu] parse_dmar_table: table too short ({})",
            table_len
        );
        return;
    }

    let host_aw = *dmar_ptr.add(36); // host address width (in bits minus 1)
    let flags = *dmar_ptr.add(37);
    serial_println!(
        "  [iommu] DMAR table found: len={} host_aw={} flags={:#x}",
        table_len,
        host_aw + 1,
        flags
    );

    let mut drhd_units = DRHD_UNITS.lock();
    let mut drhd_count = 0usize;

    // Walk remapping structures starting at offset 48
    let mut offset = 48usize;
    while offset + 4 <= table_len {
        let struct_ptr = dmar_ptr.add(offset);
        let struct_type = core::ptr::read_unaligned(struct_ptr as *const u16);
        let struct_len = core::ptr::read_unaligned(struct_ptr.add(2) as *const u16) as usize;

        if struct_len < 4 || offset + struct_len > table_len {
            // Malformed entry — stop walking
            break;
        }

        match struct_type {
            _ if struct_type == DRHD_TYPE => {
                if struct_len < 16 {
                    serial_println!("  [iommu] DRHD too short at offset {}", offset);
                } else if drhd_count < MAX_DRHD {
                    let drhd_flags = *struct_ptr.add(4);
                    // byte at offset 5 is reserved
                    let segment = core::ptr::read_unaligned(struct_ptr.add(6) as *const u16);
                    let base_address = core::ptr::read_unaligned(struct_ptr.add(8) as *const u64);

                    serial_println!(
                        "  [iommu] DRHD[{}]: base={:#018x} seg={} flags={:#x}",
                        drhd_count,
                        base_address,
                        segment,
                        drhd_flags
                    );

                    drhd_units[drhd_count] = Some(Drhd {
                        base_address,
                        segment,
                        flags: drhd_flags,
                        initialized: false,
                        iotlb_reg_offset: 0,
                    });
                    drhd_count += 1;
                } else {
                    serial_println!("  [iommu] Too many DRHD units (max {}), skipping", MAX_DRHD);
                }
            }
            _ if struct_type == RMRR_TYPE => {
                if struct_len >= 24 {
                    let segment = core::ptr::read_unaligned(struct_ptr.add(6) as *const u16);
                    let base_addr = core::ptr::read_unaligned(struct_ptr.add(8) as *const u64);
                    let limit_addr = core::ptr::read_unaligned(struct_ptr.add(16) as *const u64);
                    serial_println!(
                        "  [iommu] RMRR: seg={} base={:#018x} limit={:#018x}",
                        segment,
                        base_addr,
                        limit_addr
                    );
                    // Reserved regions — we note these but don't need to act at init
                    let _ = (segment, base_addr, limit_addr);
                }
            }
            _ if struct_type == ANDD_TYPE => {
                // ACPI Namespace Device Declaration — informational
                serial_println!("  [iommu] ANDD structure at offset {}", offset);
            }
            other => {
                serial_println!(
                    "  [iommu] Unknown DMAR structure type {} at offset {}",
                    other,
                    offset
                );
            }
        }

        offset += struct_len;
    }

    serial_println!("  [iommu] Parsed {} DRHD unit(s)", drhd_count);
}

// ---------------------------------------------------------------------------
// TASK 1 — iommu_init
// ---------------------------------------------------------------------------

/// Initialize the IOMMU subsystem.
///
/// For each discovered DRHD unit:
///   1. Read and log the VER register.
///   2. Read ECAP to find the IOTLB register offset.
///   3. Allocate a root table (4 KiB of zeroes, already in ROOT_TABLES).
///   4. Write the root table physical address to RTADDR_REG.
///   5. Issue SRTP command (Set Root Table Pointer) and wait for RTPS.
///   6. Issue TE command (Translation Enable) and wait for TES.
///
/// After this function returns, DMA from uninitialised devices is blocked
/// until the driver explicitly maps memory via `iommu_map`.
pub fn iommu_init() {
    let mut drhd_units = DRHD_UNITS.lock();
    let mut any_enabled = false;

    for (idx, slot) in drhd_units.iter_mut().enumerate() {
        let drhd = match slot {
            Some(d) => d,
            None => continue,
        };

        let base = drhd.base_address;

        // --- 1. Read version ---
        let ver = unsafe { mmio_read32(base, VER_REG) };
        let major = (ver >> 4) & 0xF;
        let minor = ver & 0xF;
        serial_println!(
            "  [iommu] DRHD[{}] base={:#018x} VT-d version {}.{}",
            idx,
            base,
            major,
            minor
        );

        // --- 2. Read ECAP to get IOTLB register offset ---
        // ECAP.IRO (bits 17:8) × 16 gives the offset of the IOTLB invalidation register
        let ecap = unsafe { mmio_read64(base, ECAP_REG) };
        let iro = ((ecap >> 8) & 0x3FF) as u32;
        let iotlb_offset = iro * 16;
        drhd.iotlb_reg_offset = iotlb_offset;
        serial_println!(
            "  [iommu] DRHD[{}] ECAP={:#018x} IOTLB_offset={:#x}",
            idx,
            ecap,
            iotlb_offset
        );

        // --- 3. Root table physical address ---
        // ROOT_TABLES[idx] is a statically allocated 4 KiB page (already zeroed at BSS init).
        // In a real kernel we'd call the frame allocator; here we use our static pool.
        let root_table_phys = unsafe { ROOT_TABLES[idx][0].0.as_ptr() as u64 };

        // Zero out the root table to ensure all entries are marked not-present
        unsafe {
            let ptr = root_table_phys as *mut u8;
            core::ptr::write_bytes(ptr, 0, ROOT_TABLE_SIZE);
        }

        serial_println!(
            "  [iommu] DRHD[{}] root table at phys={:#018x}",
            idx,
            root_table_phys
        );

        // --- 4. Write root table pointer ---
        // RTADDR_REG holds the physical address; bit 11:0 are type bits (0 = legacy mode)
        unsafe {
            mmio_write64(base, RTADDR_REG, root_table_phys);
        }

        // --- 5. Issue SRTP (Set Root Table Pointer) and wait for RTPS ---
        unsafe {
            mmio_write32(base, GCMD_REG, GCMD_SRTP);
        }

        // Poll GSTS_REG until RTPS (Root Table Pointer Status) is set
        let mut timeout = 100_000u32;
        loop {
            let gsts = unsafe { mmio_read32(base, GSTS_REG) };
            if gsts & GSTS_RTPS != 0 {
                break;
            }
            timeout = timeout.saturating_sub(1);
            if timeout == 0 {
                serial_println!("  [iommu] DRHD[{}] SRTP timeout! GSTS={:#010x}", idx, gsts);
                break;
            }
            core::hint::spin_loop();
        }

        // --- 6. Enable translation (TE bit) ---
        unsafe {
            mmio_write32(base, GCMD_REG, GCMD_TE);
        }

        // Poll GSTS_REG until TES (Translation Enable Status) is set
        timeout = 100_000u32;
        loop {
            let gsts = unsafe { mmio_read32(base, GSTS_REG) };
            if gsts & GSTS_TES != 0 {
                serial_println!(
                    "  [iommu] DRHD[{}] translation ENABLED (GSTS={:#010x})",
                    idx,
                    gsts
                );
                drhd.initialized = true;
                any_enabled = true;
                break;
            }
            timeout = timeout.saturating_sub(1);
            if timeout == 0 {
                serial_println!("  [iommu] DRHD[{}] TE timeout! GSTS={:#010x}", idx, gsts);
                break;
            }
            core::hint::spin_loop();
        }
    }

    if any_enabled {
        IOMMU_ACTIVE.store(true, Ordering::SeqCst);
        serial_println!("  [iommu] IOMMU translation active");
    } else {
        serial_println!("  [iommu] IOMMU init complete (no DRHD units enabled)");
    }
}

// ---------------------------------------------------------------------------
// Domain management helpers
// ---------------------------------------------------------------------------

/// Find the domain for a given BDF, or allocate a new one.
/// Returns a mutable reference index into DOMAINS, the domain_id, and the SLPT root.
/// Returns None if MAX_DOMAINS is exhausted.
fn get_or_create_domain(
    domains: &mut [Option<IommuDomain>; MAX_DOMAINS],
    bdf: u16,
) -> Option<usize> {
    // Check for existing domain
    for (i, slot) in domains.iter().enumerate() {
        if let Some(ref d) = slot {
            if d.bdf == bdf {
                return Some(i);
            }
        }
    }

    // Find a free slot
    let free_idx = domains.iter().position(|s| s.is_none())?;

    // Allocate domain ID
    let domain_id = NEXT_DOMAIN_ID.fetch_add(1, Ordering::Relaxed) as u16;
    if domain_id as usize >= MAX_DOMAINS {
        serial_println!("  [iommu] Domain ID exhausted");
        return None;
    }

    // Allocate a SLPT page from our static pool.
    // Pool index == free_idx (one pool page per domain slot).
    let slpt_phys = unsafe { SLPT_POOL[free_idx].0.as_ptr() as u64 };

    // Zero the SLPT page
    unsafe {
        core::ptr::write_bytes(slpt_phys as *mut u8, 0, 4096);
    }

    domains[free_idx] = Some(IommuDomain {
        domain_id,
        bdf,
        sl_root_phys: slpt_phys,
        mapping_count: 0,
    });

    Some(free_idx)
}

/// Write the root-table and context-table entries for a device to bind it to a domain.
///
/// The root table has one entry per PCI bus number (256 entries × 16 bytes).
/// Each root entry points to a context table (256 entries × 32 bytes), one per device+func.
///
/// Root entry format (16 bytes):
///   Bits 63:12 — context table pointer (phys addr >> 12, i.e., page-aligned)
///   Bit  0     — Present
///   Bytes 8-15 — upper (reserved / EXT mode; zeroed here)
///
/// Context entry format (32 bytes):
///   Word 0, bits 63:12 — second-level page table pointer
///   Word 0, bit  0     — Present
///   Word 1, bits 31:16 — domain ID
///   Word 1, bits  2:0  — address width (3=39-bit, 4=48-bit, 5=57-bit)
///   Words 2-3          — reserved / zero
unsafe fn program_root_context(
    root_table_phys: u64,
    bus: u8,
    dev: u8,
    func: u8,
    domain_id: u16,
    sl_root_phys: u64,
) {
    // ---- Root entry ----
    let root_entry_ptr = (root_table_phys + (bus as u64 * ROOT_ENTRY_SIZE as u64)) as *mut u64;

    let existing_lo = core::ptr::read_volatile(root_entry_ptr);
    let ctx_table_phys: u64;

    if existing_lo & 1 != 0 {
        // Context table already exists for this bus
        ctx_table_phys = existing_lo & !0xFFF;
    } else {
        // No context table yet — we'd need to allocate one from a frame allocator.
        // For this driver, we reuse the root table page as a stub context table
        // if we are out of allocation space.  In production this must be a real
        // 8 KiB allocation; here we skip writing rather than corrupting memory.
        serial_println!("  [iommu] No context table for bus {:#x}; root entry not present — skipping context bind", bus);
        return;
    }

    // ---- Context entry ----
    // Context table index: device * 8 + function (device 0-31, function 0-7)
    let ctx_idx = (dev as u64 * 8) + func as u64;
    let ctx_entry_ptr = (ctx_table_phys + ctx_idx * CTX_ENTRY_SIZE as u64) as *mut u64;

    // Word 0: SLPT root phys | Present
    core::ptr::write_volatile(ctx_entry_ptr, sl_root_phys | 1u64);

    // Word 1: domain_id in bits 31:16, address width 4 (48-bit) in bits 2:0
    let word1: u64 = ((domain_id as u64) << 16) | 4u64;
    core::ptr::write_volatile(ctx_entry_ptr.add(1), word1);

    // Words 2-3: zeroed
    core::ptr::write_volatile(ctx_entry_ptr.add(2), 0u64);
    core::ptr::write_volatile(ctx_entry_ptr.add(3), 0u64);
}

/// Allocate a context table page and link it into the root table for a bus.
/// Returns the physical address of the context table, or 0 on failure.
///
/// Uses a static pool of context table pages (one per bus, lazily allocated).
unsafe fn ensure_context_table(root_table_phys: u64, bus: u8) -> u64 {
    let root_entry_ptr = (root_table_phys + (bus as u64 * ROOT_ENTRY_SIZE as u64)) as *mut u64;

    let existing_lo = core::ptr::read_volatile(root_entry_ptr);
    if existing_lo & 1 != 0 {
        return existing_lo & !0xFFF; // already present
    }

    // We need to allocate a context table.  In this implementation we carve
    // space from SLPT_POOL (which is 4 KiB per slot).  We use the upper half
    // of the bus-numbered slot as the context table (4 KiB contexts fit in one
    // 4 KiB page per 128 devices with 32-byte entries).
    //
    // This is a simplification.  A real driver calls the page frame allocator.
    // For now we use a separate static pool.
    static mut CTX_POOL: [[u8; 8192]; 8] = [[0u8; 8192]; 8];
    static CTX_POOL_NEXT: AtomicU64 = AtomicU64::new(0);

    let slot = CTX_POOL_NEXT.fetch_add(1, Ordering::Relaxed) as usize;
    if slot >= 8 {
        serial_println!("  [iommu] Context table pool exhausted");
        return 0;
    }

    let ctx_phys = CTX_POOL[slot].as_ptr() as u64;
    core::ptr::write_bytes(ctx_phys as *mut u8, 0, 8192);

    // Write root entry: ctx_phys | Present
    core::ptr::write_volatile(root_entry_ptr, ctx_phys | 1u64);
    // Upper 8 bytes of root entry = 0
    core::ptr::write_volatile(root_entry_ptr.add(1), 0u64);

    ctx_phys
}

// ---------------------------------------------------------------------------
// TASK 1 — iommu_map
// ---------------------------------------------------------------------------

/// Map a device DMA range: IOVA → physical address with access protection.
///
/// - `bus`, `dev`, `func` identify the PCI function.
/// - `iova` is the I/O virtual address (as seen by the device).
/// - `paddr` is the host physical address.
/// - `size` is the number of bytes to map (rounded up to PAGE_SIZE).
/// - `prot` bit 0 = READ allowed, bit 1 = WRITE allowed.
///
/// This inserts page-table entries into the domain's second-level page table (SLPT).
/// SLPT uses a single-level 4 KiB page table for simplicity
/// (sufficient for a 2 MiB addressable range with 4 KiB pages × 512 entries).
pub fn iommu_map(bus: u8, dev: u8, func: u8, iova: u64, paddr: u64, size: usize, prot: u8) {
    if !IOMMU_ACTIVE.load(Ordering::Acquire) {
        return; // IOMMU not enabled
    }

    let bdf = ((bus as u16) << 8) | ((dev as u16) << 3) | (func as u16);

    let mut domains = DOMAINS.lock();
    let dom_idx = match get_or_create_domain(&mut domains, bdf) {
        Some(i) => i,
        None => {
            serial_println!(
                "  [iommu] iommu_map: failed to allocate domain for {:02x}:{:02x}.{}",
                bus,
                dev,
                func
            );
            return;
        }
    };

    let (domain_id, sl_root_phys) = {
        let d = domains[dom_idx].as_mut().unwrap();
        (d.domain_id, d.sl_root_phys)
    };

    // Bind the device to its domain in the root/context tables of each DRHD
    {
        let drhd_units = DRHD_UNITS.lock();
        for (idx, slot) in drhd_units.iter().enumerate() {
            if let Some(ref drhd) = slot {
                if !drhd.initialized {
                    continue;
                }
                let root_phys = unsafe { ROOT_TABLES[idx][0].0.as_ptr() as u64 };
                unsafe {
                    let ctx_phys = ensure_context_table(root_phys, bus);
                    if ctx_phys != 0 {
                        program_root_context(root_phys, bus, dev, func, domain_id, sl_root_phys);
                    }
                }
            }
        }
    }

    // --- Map pages in the second-level page table ---
    // SLPT layout: 512 entries × 8 bytes = 4 KiB (one page, indexed by IOVA[20:12])
    //
    // SLPTE format:
    //   Bits 63:12  — physical page address (phys >> 12)
    //   Bit  1      — WRITE permission (SW)
    //   Bit  0      — READ permission (SR)
    //
    // We only map the first 512 pages (2 MiB IOVA range) for this single-level SLPT.
    // A full implementation would use a multi-level table (2/3/4 levels).

    let read_bit: u64 = if prot & 1 != 0 { 1 } else { 0 };
    let write_bit: u64 = if prot & 2 != 0 { 2 } else { 0 };
    let perm_bits = read_bit | write_bit;

    let pages = size.saturating_add(PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;
    let slpte_base = sl_root_phys as *mut u64;

    for i in 0..pages {
        let page_iova = iova + (i as u64 * PAGE_SIZE);
        let page_paddr = paddr + (i as u64 * PAGE_SIZE);
        let slpt_idx = ((page_iova >> 12) & 0x1FF) as usize; // bits 20:12

        if slpt_idx >= 512 {
            serial_println!(
                "  [iommu] iommu_map: IOVA {:#x} out of single-level SLPT range",
                page_iova
            );
            break;
        }

        let entry = (page_paddr & !0xFFF) | perm_bits;
        unsafe {
            core::ptr::write_volatile(slpte_base.add(slpt_idx), entry);
        }
    }

    // Update mapping count
    if let Some(ref mut d) = domains[dom_idx] {
        d.mapping_count = d.mapping_count.saturating_add(pages as u32);
    }

    // Invalidate the IOTLB for this domain
    iommu_invalidate_iotlb(domain_id);

    serial_println!("  [iommu] Mapped IOVA {:#018x}→phys {:#018x} sz={} prot={:#x} for {:02x}:{:02x}.{} domain={}",
        iova, paddr, size, prot, bus, dev, func, domain_id);
}

// ---------------------------------------------------------------------------
// TASK 1 — iommu_unmap
// ---------------------------------------------------------------------------

/// Remove a DMA mapping for the given IOVA range from the device's domain.
///
/// Clears the SLPTE entries and invalidates the IOTLB.
pub fn iommu_unmap(bus: u8, dev: u8, func: u8, iova: u64, size: usize) {
    if !IOMMU_ACTIVE.load(Ordering::Acquire) {
        return;
    }

    let bdf = ((bus as u16) << 8) | ((dev as u16) << 3) | (func as u16);
    let mut domains = DOMAINS.lock();

    let dom_idx = match domains
        .iter()
        .position(|s| s.as_ref().map_or(false, |d| d.bdf == bdf))
    {
        Some(i) => i,
        None => {
            serial_println!(
                "  [iommu] iommu_unmap: no domain for {:02x}:{:02x}.{}",
                bus,
                dev,
                func
            );
            return;
        }
    };

    let (domain_id, sl_root_phys) = {
        let d = domains[dom_idx].as_ref().unwrap();
        (d.domain_id, d.sl_root_phys)
    };

    let pages = size.saturating_add(PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;
    let slpte_base = sl_root_phys as *mut u64;

    for i in 0..pages {
        let page_iova = iova + (i as u64 * PAGE_SIZE);
        let slpt_idx = ((page_iova >> 12) & 0x1FF) as usize;
        if slpt_idx >= 512 {
            break;
        }
        unsafe {
            core::ptr::write_volatile(slpte_base.add(slpt_idx), 0u64);
        }
    }

    if let Some(ref mut d) = domains[dom_idx] {
        d.mapping_count = d.mapping_count.saturating_sub(pages as u32);
    }

    iommu_invalidate_iotlb(domain_id);

    serial_println!(
        "  [iommu] Unmapped IOVA {:#018x} sz={} for {:02x}:{:02x}.{} domain={}",
        iova,
        size,
        bus,
        dev,
        func,
        domain_id
    );
}

// ---------------------------------------------------------------------------
// IOTLB invalidation
// ---------------------------------------------------------------------------

/// Invalidate the IOTLB for a specific domain across all DRHD units.
///
/// Issues a domain-selective IOTLB invalidation (IOTLB_REG write).
///
/// IOTLB_REG format (64-bit, at ECAP.IRO*16 + 8 from DRHD base):
///   Bit  63     — IIRG (Invalidation request granularity): 1 = domain-selective
///   Bits 47:32  — DID (Domain ID)
///   Bit  31     — IVT (Invalidate IOTLB — set to trigger)
fn iommu_invalidate_iotlb(domain_id: u16) {
    let drhd_units = DRHD_UNITS.lock();
    for slot in drhd_units.iter() {
        if let Some(ref drhd) = slot {
            if !drhd.initialized {
                continue;
            }
            let base = drhd.base_address;
            let iotlb_offset = drhd.iotlb_reg_offset;

            if iotlb_offset == 0 {
                continue;
            }

            // Domain-selective invalidation:
            // IVT=1, IIRG=10 (domain-selective), DID=domain_id
            let iotlb_cmd: u64 = (1u64 << 63)          // IVT
                | (0b10u64 << 60)                       // IIRG = domain-selective
                | ((domain_id as u64) << 16); // DID

            unsafe {
                // The IOTLB invalidation register pair is at iotlb_offset and iotlb_offset+8
                mmio_write64(base, iotlb_offset + 8, iotlb_cmd);
            }

            // Wait for IVT to clear (hardware clears it when invalidation is done)
            let mut timeout = 100_000u32;
            loop {
                let val = unsafe { mmio_read64(base, iotlb_offset + 8) };
                if val & (1u64 << 63) == 0 {
                    break;
                }
                timeout = timeout.saturating_sub(1);
                if timeout == 0 {
                    serial_println!(
                        "  [iommu] IOTLB invalidation timeout for domain {}",
                        domain_id
                    );
                    break;
                }
                core::hint::spin_loop();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TASK 1 — iommu_enabled
// ---------------------------------------------------------------------------

/// Returns `true` if the IOMMU is active (at least one DRHD unit has
/// translation enabled).
pub fn iommu_enabled() -> bool {
    IOMMU_ACTIVE.load(Ordering::Acquire)
}

// ---------------------------------------------------------------------------
// Diagnostic / informational
// ---------------------------------------------------------------------------

/// Print a summary of the IOMMU state to the serial console.
pub fn iommu_dump_state() {
    let drhd_units = DRHD_UNITS.lock();
    let domains = DOMAINS.lock();

    let drhd_count = drhd_units.iter().filter(|s| s.is_some()).count();
    let dom_count = domains.iter().filter(|s| s.is_some()).count();

    serial_println!(
        "  [iommu] State: active={} drhd={} domains={}",
        IOMMU_ACTIVE.load(Ordering::Relaxed),
        drhd_count,
        dom_count
    );

    for (i, slot) in drhd_units.iter().enumerate() {
        if let Some(ref d) = slot {
            let gsts = unsafe { mmio_read32(d.base_address, GSTS_REG) };
            serial_println!(
                "    DRHD[{}] base={:#018x} init={} GSTS={:#010x}",
                i,
                d.base_address,
                d.initialized,
                gsts
            );
        }
    }

    for slot in domains.iter() {
        if let Some(ref d) = slot {
            serial_println!(
                "    Domain {} bdf={:#06x} mappings={}",
                d.domain_id,
                d.bdf,
                d.mapping_count
            );
        }
    }
}

/// Initialize the IOMMU module (called from main init sequence).
/// This function only sets up the subsystem; call `parse_dmar_table` first,
/// then call `iommu_init` to enable translation.
pub fn init() {
    serial_println!(
        "  [iommu] Intel VT-d IOMMU driver loaded (call parse_dmar_table + iommu_init to enable)"
    );
}
