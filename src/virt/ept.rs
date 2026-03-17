use crate::virt::vmx;
/// Extended Page Tables (EPT) for guest memory isolation.
///
/// EPT provides a second level of address translation mapping guest-physical
/// addresses (GPAs) to host-physical addresses (HPAs).  This eliminates the
/// need for shadow page tables and drastically reduces VM-exit frequency for
/// memory accesses.
///
/// Structure:
///   PML4 (512 entries, 48-bit GPA)
///     └─ PDPT  (512 × 512 GB regions)
///          └─ PD    (512 × 1 GB regions)
///               └─ PT   (512 × 2 MB regions)
///                    └─ 4 KB pages
///
/// Only 4 KB leaf mappings are implemented here; 2 MB large-page support can
/// be added by stopping the walk at the PD level and setting the large-page
/// bit (bit 7).
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Page size constants
// ---------------------------------------------------------------------------

const PAGE_SIZE: usize = 4096;
const ENTRIES: usize = 512; // entries per level (4096 / 8)

// ---------------------------------------------------------------------------
// EPT entry permission / attribute bits
// ---------------------------------------------------------------------------

/// Read permission (bit 0).
pub const EPT_READ: u8 = 1 << 0;
/// Write permission (bit 1).
pub const EPT_WRITE: u8 = 1 << 1;
/// Execute permission (bit 2).
pub const EPT_EXEC: u8 = 1 << 2;
/// Read + Write + Execute shorthand.
pub const EPT_RWX: u8 = EPT_READ | EPT_WRITE | EPT_EXEC;

/// Memory type: write-back (bits [5:3] = 6).
const EPT_MEMTYPE_WB: u64 = 6 << 3;
/// Memory type: uncacheable (bits [5:3] = 0).
const EPT_MEMTYPE_UC: u64 = 0 << 3;

/// Large-page bit at PDPT/PD level (bit 7).
const EPT_LARGE: u64 = 1 << 7;

// ---------------------------------------------------------------------------
// EPT page-table entry
// ---------------------------------------------------------------------------

/// A single EPT entry (8 bytes).
#[derive(Clone, Copy)]
#[repr(transparent)]
struct EptEntry(u64);

impl EptEntry {
    const fn empty() -> Self {
        EptEntry(0)
    }

    /// True if any of R/W/X is set (entry is present).
    fn is_present(self) -> bool {
        self.0 & (EPT_RWX as u64) != 0
    }

    /// Physical address stored in this entry (bits [51:12]).
    fn phys_addr(self) -> u64 {
        self.0 & 0x000F_FFFF_FFFF_F000
    }

    /// Build a non-leaf (directory) entry pointing to the next-level table.
    fn new_table(table_phys: u64) -> Self {
        // Non-leaf entries: R/W/X + physical address (no memory type bits).
        EptEntry((table_phys & 0x000F_FFFF_FFFF_F000) | (EPT_RWX as u64))
    }

    /// Build a 4 KB leaf entry.
    fn new_page(hpa: u64, prot: u8, memtype: u64) -> Self {
        EptEntry((hpa & 0x000F_FFFF_FFFF_F000) | memtype | (prot as u64))
    }
}

// ---------------------------------------------------------------------------
// EptLevel — four-level page table (4 KiB arrays)
// ---------------------------------------------------------------------------

/// A 4 KiB EPT page table (one level, 512 entries of 8 bytes each).
#[repr(C, align(4096))]
struct EptTable {
    entries: [EptEntry; ENTRIES],
}

impl EptTable {
    const fn new() -> Self {
        EptTable {
            entries: [EptEntry::empty(); ENTRIES],
        }
    }
}

// ---------------------------------------------------------------------------
// Static pool of page tables
// ---------------------------------------------------------------------------
// In a production kernel these would be dynamically allocated from the
// physical-page allocator.  Here we use a fixed-size static pool.

const POOL_SIZE: usize = 64; // Supports mapping up to ~64 GB with 2 MB granularity.

/// Global pool of EPT page tables (zero-initialised in BSS).
static mut TABLE_POOL: [EptTable; POOL_SIZE] = {
    // const-initialise the pool; can't use a loop here (no const loops over
    // non-Copy until Rust 2024), so we rely on the zero-init of BSS.
    unsafe { core::mem::zeroed() }
};

/// Index of the next free table in the pool.
static mut POOL_NEXT: usize = 0;

/// Allocate one table from the static pool.
///
/// Returns a raw pointer to the newly zeroed table, or null if the pool is
/// exhausted.
///
/// # Safety
/// Must be called with interrupts disabled or from a single thread.
unsafe fn pool_alloc() -> *mut EptTable {
    if POOL_NEXT >= POOL_SIZE {
        serial_println!(
            "[EPT] WARNING: page table pool exhausted (POOL_SIZE={})",
            POOL_SIZE
        );
        return core::ptr::null_mut();
    }
    let idx = POOL_NEXT;
    POOL_NEXT += 1;
    // Zero-initialise the table.
    TABLE_POOL[idx] = EptTable::new();
    &mut TABLE_POOL[idx] as *mut EptTable
}

// ---------------------------------------------------------------------------
// EptPageTable — the public API
// ---------------------------------------------------------------------------

/// Root of an EPT hierarchy for one guest VM.
pub struct EptPageTable {
    /// Pointer to the PML4 table (root of the 4-level hierarchy).
    pml4: *mut EptTable,
}

impl EptPageTable {
    /// Allocate and initialise a new (empty) EPT page table hierarchy.
    ///
    /// Returns `None` if the static pool is exhausted.
    pub fn new() -> Option<Self> {
        let pml4 = unsafe { pool_alloc() };
        if pml4.is_null() {
            return None;
        }
        serial_println!("[EPT] PML4 allocated at phys=0x{:016x}", pml4 as u64);
        Some(EptPageTable { pml4 })
    }

    /// Physical address of the PML4 root.
    pub fn pml4_phys(&self) -> u64 {
        self.pml4 as u64
    }

    // -----------------------------------------------------------------------
    // ept_map — add a GPA→HPA mapping
    // -----------------------------------------------------------------------

    /// Map `size` bytes starting at guest-physical `gpa` to host-physical
    /// `hpa` with protection bits `prot` (combination of `EPT_READ`,
    /// `EPT_WRITE`, `EPT_EXEC`).
    ///
    /// `size` must be a multiple of `PAGE_SIZE` (4 KiB).  Larger sizes are
    /// silently rounded down to the nearest page boundary.
    ///
    /// Returns `Ok(())` if all pages were mapped, or `Err` with a description
    /// if the pool was exhausted mid-mapping.
    pub fn ept_map(
        &mut self,
        gpa: u64,
        hpa: u64,
        size: usize,
        prot: u8,
    ) -> Result<(), &'static str> {
        if size == 0 {
            return Ok(());
        }
        let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        for i in 0..pages {
            let offset = (i * PAGE_SIZE) as u64;
            self.map_page(gpa + offset, hpa + offset, prot)?;
        }
        Ok(())
    }

    /// Map a single 4 KiB page GPA→HPA.
    fn map_page(&mut self, gpa: u64, hpa: u64, prot: u8) -> Result<(), &'static str> {
        unsafe {
            let pml4 = &mut *self.pml4;

            // Indices for each level of the 4-level walk.
            let pml4_idx = ((gpa >> 39) & 0x1FF) as usize;
            let pdpt_idx = ((gpa >> 30) & 0x1FF) as usize;
            let pd_idx = ((gpa >> 21) & 0x1FF) as usize;
            let pt_idx = ((gpa >> 12) & 0x1FF) as usize;

            // --- PML4 → PDPT ---
            let pdpt = ensure_table(&mut pml4.entries[pml4_idx])?;

            // --- PDPT → PD ---
            let pd = ensure_table(&mut (*pdpt).entries[pdpt_idx])?;

            // --- PD → PT ---
            let pt = ensure_table(&mut (*pd).entries[pd_idx])?;

            // --- PT → 4 KB page ---
            (*pt).entries[pt_idx] = EptEntry::new_page(hpa, prot, EPT_MEMTYPE_WB);
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // ept_unmap — remove a GPA mapping
    // -----------------------------------------------------------------------

    /// Remove mappings for `size` bytes starting at `gpa`.
    ///
    /// Clears the leaf PT entries; directory entries are left intact to avoid
    /// having to walk back up the tree (they will be reused for future maps).
    pub fn ept_unmap(&mut self, gpa: u64, size: usize) {
        if size == 0 {
            return;
        }
        let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        for i in 0..pages {
            let offset = (i * PAGE_SIZE) as u64;
            self.unmap_page(gpa + offset);
        }
    }

    fn unmap_page(&mut self, gpa: u64) {
        unsafe {
            let pml4 = &mut *self.pml4;
            let pml4_idx = ((gpa >> 39) & 0x1FF) as usize;
            let pdpt_idx = ((gpa >> 30) & 0x1FF) as usize;
            let pd_idx = ((gpa >> 21) & 0x1FF) as usize;
            let pt_idx = ((gpa >> 12) & 0x1FF) as usize;

            if !pml4.entries[pml4_idx].is_present() {
                return;
            }
            let pdpt = pml4.entries[pml4_idx].phys_addr() as *mut EptTable;

            if !(*pdpt).entries[pdpt_idx].is_present() {
                return;
            }
            let pd = (*pdpt).entries[pdpt_idx].phys_addr() as *mut EptTable;

            if !(*pd).entries[pd_idx].is_present() {
                return;
            }
            let pt = (*pd).entries[pd_idx].phys_addr() as *mut EptTable;

            (*pt).entries[pt_idx] = EptEntry::empty();
        }
    }

    // -----------------------------------------------------------------------
    // ept_get_pointer — build the EPTP value for the VMCS
    // -----------------------------------------------------------------------

    /// Format the EPT Pointer (EPTP) field value for the VMCS.
    ///
    /// Per Intel SDM 24.6.11:
    ///   bits [2:0]  = EPT memory type for PML4 accesses (6 = WB)
    ///   bits [5:3]  = EPT page-walk length minus 1 (3 = 4 levels)
    ///   bits [11:6] = reserved (0)
    ///   bits [N-1:12] = physical address of PML4 table
    ///
    /// `pml4_ptr` is the physical address of the PML4 root.
    pub fn ept_get_pointer(pml4_ptr: u64) -> u64 {
        // Memory type WB=6, walk length 4 (encoded as 3).
        let eptp = (pml4_ptr & !0xFFF_u64)
            | (3 << 3)  // page-walk length = 4 (value 3)
            | 6; // memory type = WB
        eptp
    }

    /// Convenience: build the EPTP using this table's own PML4 address.
    pub fn eptp(&self) -> u64 {
        Self::ept_get_pointer(self.pml4_phys())
    }

    // -----------------------------------------------------------------------
    // ept_invalidate — flush EPT-derived TLB entries
    // -----------------------------------------------------------------------

    /// Invalidate EPT TLB entries.
    ///
    /// Two types are defined by Intel SDM 30.3:
    ///   - Single-context (type 1): flushes entries tagged with the given EPTP.
    ///   - All-context    (type 2): flushes all EPT-derived entries.
    ///
    /// Falls back to all-context invalidation if single-context fails.
    pub fn ept_invalidate(eptp: u64) {
        unsafe {
            // INVEPT descriptor: [0..7] = EPTP, [8..15] = reserved (0).
            let descriptor: [u64; 2] = [eptp, 0];

            // Try single-context invalidation (type 1) first.
            let cf1: u8;
            core::arch::asm!(
                "invept {}, [{}]",
                in(reg) 1u64,           // type = 1 (single-context)
                in(reg) descriptor.as_ptr(),
                out("al") cf1,          // setc equivalent — we actually read CF below
                options(nostack),
            );
            // The inline asm above doesn't reliably capture CF; use a separate setc.
            let cf: u8;
            core::arch::asm!("setc {}", out(reg_byte) cf, options(nomem, nostack));

            if cf != 0 {
                // Single-context failed; try global invalidation (type 2).
                core::arch::asm!(
                    "invept {}, [{}]",
                    in(reg) 2u64,
                    in(reg) descriptor.as_ptr(),
                    options(nostack),
                );
                serial_println!("[EPT] INVEPT type 2 (global) executed");
            } else {
                serial_println!(
                    "[EPT] INVEPT type 1 (single-context) executed for EPTP=0x{:016x}",
                    eptp
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: walk helper — ensure a next-level table exists
// ---------------------------------------------------------------------------

/// Given a directory entry, ensure the next-level table it points to exists.
///
/// If the entry is not present, allocates a new table from the static pool and
/// writes a non-leaf entry.  Returns a raw pointer to the next-level table.
///
/// # Safety
/// Caller must hold any relevant locks and have interrupts disabled.
unsafe fn ensure_table(entry: &mut EptEntry) -> Result<*mut EptTable, &'static str> {
    if entry.is_present() {
        return Ok(entry.phys_addr() as *mut EptTable);
    }

    let table = pool_alloc();
    if table.is_null() {
        return Err("[EPT] page table pool exhausted");
    }

    *entry = EptEntry::new_table(table as u64);
    Ok(table)
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

/// Initialise the EPT subsystem.
///
/// Checks whether EPT is supported via IA32_VMX_PROCBASED_CTLS2 MSR bit 1.
pub fn init() {
    let supported = unsafe { ept_is_supported() };
    if supported {
        serial_println!("[EPT] Extended Page Tables supported and enabled");
    } else {
        serial_println!("[EPT] Extended Page Tables NOT supported on this CPU");
    }
}

/// Check EPT support via the IA32_VMX_PROCBASED_CTLS2 MSR (0x48B / 0x48C).
///
/// Bit 33 of the 64-bit MSR value (bit 1 of the high 32 bits) corresponds to
/// EPT support in the secondary proc-based VM-execution controls.
unsafe fn ept_is_supported() -> bool {
    if !vmx::vmx_supported() {
        return false;
    }
    // IA32_VMX_PROCBASED_CTLS2: 0x48B (allowed-0) / 0x48C (allowed-1).
    // If bit 1 is set in the allowed-1 MSR, EPT can be enabled.
    let allowed1 = vmx::rdmsr(0x48C);
    (allowed1 >> 1) & 1 == 1
}
