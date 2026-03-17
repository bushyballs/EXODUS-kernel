/// Extended Page Tables (nested paging) for guest VMs.
///
/// Part of the AIOS hypervisor subsystem.
///
/// EPT provides a second level of page translation that maps guest-physical
/// addresses to host-physical addresses. This eliminates the need for
/// shadow page tables and reduces VM exit overhead.

use crate::{serial_print, serial_println};
use crate::sync::Mutex;
use alloc::vec::Vec;

/// Page size constants.
const PAGE_SIZE_4K: u64 = 4096;
const PAGE_SIZE_2M: u64 = 2 * 1024 * 1024;
const PAGE_SIZE_1G: u64 = 1024 * 1024 * 1024;

/// Number of entries per EPT page table level (512 = 4096 / 8).
const EPT_ENTRIES_PER_TABLE: usize = 512;

/// EPT entry permission bits.
pub const EPT_READ: u64 = 1 << 0;
pub const EPT_WRITE: u64 = 1 << 1;
pub const EPT_EXECUTE: u64 = 1 << 2;
pub const EPT_RWX: u64 = EPT_READ | EPT_WRITE | EPT_EXECUTE;

/// EPT memory type in bits [5:3].
pub const EPT_MEMTYPE_UC: u64 = 0 << 3; // Uncacheable
pub const EPT_MEMTYPE_WC: u64 = 1 << 3; // Write-combining
pub const EPT_MEMTYPE_WT: u64 = 4 << 3; // Write-through
pub const EPT_MEMTYPE_WP: u64 = 5 << 3; // Write-protect
pub const EPT_MEMTYPE_WB: u64 = 6 << 3; // Write-back

/// Large page bit (used at PDPT and PD levels).
const EPT_LARGE_PAGE: u64 = 1 << 7;

/// EPT violation qualification bits.
const EPT_VIOLATION_READ: u64 = 1 << 0;
const EPT_VIOLATION_WRITE: u64 = 1 << 1;
const EPT_VIOLATION_EXEC: u64 = 1 << 2;

/// Maximum number of EPT root structures.
const MAX_EPT_ROOTS: usize = 64;

/// Global EPT manager.
static EPT_MANAGER: Mutex<Option<EptManager>> = Mutex::new(None);

/// Manages EPT root allocations across all VMs.
struct EptManager {
    /// Whether EPT is supported by hardware.
    ept_supported: bool,
    /// Number of active EPT roots.
    active_roots: usize,
}

/// A single EPT page table entry.
#[derive(Clone, Copy)]
#[repr(transparent)]
struct EptEntry(u64);

impl EptEntry {
    const fn empty() -> Self {
        EptEntry(0)
    }

    fn is_present(&self) -> bool {
        (self.0 & EPT_RWX) != 0
    }

    fn is_large_page(&self) -> bool {
        (self.0 & EPT_LARGE_PAGE) != 0
    }

    fn physical_address(&self) -> u64 {
        self.0 & 0x000F_FFFF_FFFF_F000
    }

    fn set(&mut self, phys: u64, flags: u64) {
        self.0 = (phys & 0x000F_FFFF_FFFF_F000) | flags;
    }
}

/// Extended Page Table root for a guest VM.
///
/// Implements a 4-level EPT structure: PML4 -> PDPT -> PD -> PT.
pub struct EptRoot {
    /// PML4 table (512 entries, each 8 bytes = 4 KiB total).
    pml4: [EptEntry; EPT_ENTRIES_PER_TABLE],
    /// Allocated page table pages for teardown.
    allocated_tables: Vec<PageTablePage>,
    /// Default memory type for new mappings.
    default_memtype: u64,
    /// Count of mapped 4K pages.
    mapped_4k_pages: u64,
    /// Count of mapped 2M pages.
    mapped_2m_pages: u64,
}

/// A dynamically-allocated page table page.
struct PageTablePage {
    entries: [EptEntry; EPT_ENTRIES_PER_TABLE],
}

impl PageTablePage {
    fn new() -> Self {
        PageTablePage {
            entries: [EptEntry::empty(); EPT_ENTRIES_PER_TABLE],
        }
    }
}

impl EptRoot {
    pub fn new() -> Self {
        EptRoot {
            pml4: [EptEntry::empty(); EPT_ENTRIES_PER_TABLE],
            allocated_tables: Vec::new(),
            default_memtype: EPT_MEMTYPE_WB,
            mapped_4k_pages: 0,
            mapped_2m_pages: 0,
        }
    }

    /// Get the physical address of the PML4 table for use in the EPTP field.
    pub fn pml4_physical_address(&self) -> u64 {
        self.pml4.as_ptr() as u64
    }

    /// Map a guest physical address to a host physical address.
    ///
    /// `flags` should include EPT permission bits and memory type.
    pub fn map_page(&mut self, guest_phys: u64, host_phys: u64, flags: u64) {
        // Extract indices for 4-level walk.
        let pml4_idx = ((guest_phys >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((guest_phys >> 30) & 0x1FF) as usize;
        let pd_idx = ((guest_phys >> 21) & 0x1FF) as usize;
        let pt_idx = ((guest_phys >> 12) & 0x1FF) as usize;

        // Ensure PML4 entry points to a PDPT.
        if !self.pml4[pml4_idx].is_present() {
            let table_idx = self.allocate_table();
            let table_phys = self.table_physical_address(table_idx);
            self.pml4[pml4_idx].set(table_phys, EPT_RWX);
        }

        let pdpt_table_idx = self.find_table_index(self.pml4[pml4_idx].physical_address());

        // Ensure PDPT entry points to a PD.
        {
            let pdpt_present = self.allocated_tables[pdpt_table_idx].entries[pdpt_idx].is_present();
            if !pdpt_present {
                let table_idx = self.allocate_table();
                let table_phys = self.table_physical_address(table_idx);
                self.allocated_tables[pdpt_table_idx].entries[pdpt_idx].set(table_phys, EPT_RWX);
            }
        }

        let pd_phys = self.allocated_tables[pdpt_table_idx].entries[pdpt_idx].physical_address();
        let pd_table_idx = self.find_table_index(pd_phys);

        // Ensure PD entry points to a PT.
        {
            let pd_present = self.allocated_tables[pd_table_idx].entries[pd_idx].is_present();
            if !pd_present {
                let table_idx = self.allocate_table();
                let table_phys = self.table_physical_address(table_idx);
                self.allocated_tables[pd_table_idx].entries[pd_idx].set(table_phys, EPT_RWX);
            }
        }

        let pt_phys = self.allocated_tables[pd_table_idx].entries[pd_idx].physical_address();
        let pt_table_idx = self.find_table_index(pt_phys);

        // Write the final 4K page mapping.
        let entry_flags = flags | self.default_memtype;
        self.allocated_tables[pt_table_idx].entries[pt_idx].set(host_phys, entry_flags);
        self.mapped_4k_pages = self.mapped_4k_pages.saturating_add(1);
    }

    /// Map a 2 MiB large page (guest physical to host physical).
    pub fn map_large_page(&mut self, guest_phys: u64, host_phys: u64, flags: u64) {
        let pml4_idx = ((guest_phys >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((guest_phys >> 30) & 0x1FF) as usize;
        let pd_idx = ((guest_phys >> 21) & 0x1FF) as usize;

        // Ensure PML4 -> PDPT.
        if !self.pml4[pml4_idx].is_present() {
            let table_idx = self.allocate_table();
            let table_phys = self.table_physical_address(table_idx);
            self.pml4[pml4_idx].set(table_phys, EPT_RWX);
        }

        let pdpt_table_idx = self.find_table_index(self.pml4[pml4_idx].physical_address());

        // Ensure PDPT -> PD.
        {
            let pdpt_present = self.allocated_tables[pdpt_table_idx].entries[pdpt_idx].is_present();
            if !pdpt_present {
                let table_idx = self.allocate_table();
                let table_phys = self.table_physical_address(table_idx);
                self.allocated_tables[pdpt_table_idx].entries[pdpt_idx].set(table_phys, EPT_RWX);
            }
        }

        let pd_phys = self.allocated_tables[pdpt_table_idx].entries[pdpt_idx].physical_address();
        let pd_table_idx = self.find_table_index(pd_phys);

        // Write 2M large page entry directly in the PD.
        let entry_flags = flags | self.default_memtype | EPT_LARGE_PAGE;
        self.allocated_tables[pd_table_idx].entries[pd_idx].set(host_phys, entry_flags);
        self.mapped_2m_pages = self.mapped_2m_pages.saturating_add(1);
    }

    /// Handle an EPT violation (page fault in guest).
    ///
    /// `guest_phys` is the faulting guest-physical address.
    /// `qual` is the exit qualification with violation type bits.
    pub fn handle_violation(&mut self, guest_phys: u64, qual: u64) {
        let is_read = (qual & EPT_VIOLATION_READ) != 0;
        let is_write = (qual & EPT_VIOLATION_WRITE) != 0;
        let is_exec = (qual & EPT_VIOLATION_EXEC) != 0;

        serial_println!(
            "    [ept] EPT violation at GPA 0x{:016x} (read={}, write={}, exec={})",
            guest_phys, is_read, is_write, is_exec
        );

        // Check if the page is mapped at all.
        let pml4_idx = ((guest_phys >> 39) & 0x1FF) as usize;
        if !self.pml4[pml4_idx].is_present() {
            // Page not mapped — create an identity mapping for now.
            // In a real system this would allocate host memory or signal the VMM.
            serial_println!("    [ept] Unmapped GPA, creating identity map for 0x{:016x}", guest_phys);
            let aligned_gpa = guest_phys & !0xFFF;
            self.map_page(aligned_gpa, aligned_gpa, EPT_RWX);
            return;
        }

        // If we reach here, the page exists but permissions were insufficient.
        // Upgrade permissions to satisfy the access type.
        let mut new_flags = EPT_READ; // Always allow reads.
        if is_write {
            new_flags |= EPT_WRITE;
        }
        if is_exec {
            new_flags |= EPT_EXECUTE;
        }

        let aligned_gpa = guest_phys & !0xFFF;
        // Re-map with upgraded permissions (identity map for simplicity).
        self.map_page(aligned_gpa, aligned_gpa, new_flags);
        serial_println!("    [ept] Upgraded permissions for GPA 0x{:016x} (flags=0x{:x})", aligned_gpa, new_flags);
    }

    /// Invalidate all EPT-derived translations (INVEPT).
    pub fn invalidate_all(&self) {
        // INVEPT type 2 = all-context invalidation.
        let descriptor: [u64; 2] = [self.pml4_physical_address(), 0];
        unsafe {
            core::arch::asm!(
                "invept {}, [{}]",
                in(reg) 2u64, // type = all-context
                in(reg) descriptor.as_ptr(),
                options(nostack),
            );
        }
    }

    /// Allocate a new page table page and return its index.
    fn allocate_table(&mut self) -> usize {
        let idx = self.allocated_tables.len();
        self.allocated_tables.push(PageTablePage::new());
        idx
    }

    /// Get the physical address of an allocated table by index.
    fn table_physical_address(&self, index: usize) -> u64 {
        self.allocated_tables[index].entries.as_ptr() as u64
    }

    /// Find the table index whose entries start at the given physical address.
    fn find_table_index(&self, phys: u64) -> usize {
        for (i, table) in self.allocated_tables.iter().enumerate() {
            if table.entries.as_ptr() as u64 == phys {
                return i;
            }
        }
        // Should not happen if the EPT structure is consistent.
        // Allocate a new table as a fallback.
        serial_println!("    [ept] WARNING: table not found for phys 0x{:016x}, this is a bug", phys);
        0
    }
}

/// Check if EPT is supported via IA32_VMX_PROCBASED_CTLS2 and CPUID.
fn check_ept_support() -> bool {
    // Check CPUID for Intel first — EPT is an Intel VT-x feature.
    // For AMD, NPT (Nested Page Tables) is checked via CPUID 0x8000_000A EDX bit 0.
    // We check both.

    // Intel: IA32_VMX_PROCBASED_CTLS2 (MSR 0x48B) bit 33 = EPT.
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x48Bu32,
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack),
        );
    }
    let allowed_1 = ((hi as u64) << 32) | (lo as u64);
    let intel_ept = (allowed_1 >> 33) & 1 == 1;

    if intel_ept {
        return true;
    }

    // AMD: CPUID 0x8000_000A, EDX bit 0 = NPT.
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "mov eax, 0x8000000A",
            "cpuid",
            out("edx") edx,
            out("eax") _,
            out("ebx") _,
            out("ecx") _,
            options(nomem, nostack),
        );
    }
    (edx & 1) == 1
}

pub fn init() {
    let ept_supported = check_ept_support();

    let manager = EptManager {
        ept_supported,
        active_roots: 0,
    };

    *EPT_MANAGER.lock() = Some(manager);

    if ept_supported {
        serial_println!("    [ept] Extended Page Tables supported and initialized");
    } else {
        serial_println!("    [ept] Extended Page Tables not supported by hardware");
    }
}
