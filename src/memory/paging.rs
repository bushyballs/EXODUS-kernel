use super::frame_allocator::{self, PhysFrame, FRAME_SIZE};
/// 4-level page table management for x86_64
///
/// x86_64 uses a 4-level page table hierarchy:
///   PML4 (Page Map Level 4) -> PDPT (Page Directory Pointer Table)
///        -> PD (Page Directory) -> PT (Page Table) -> Physical Frame
///
/// Each table has 512 entries. Each entry is 8 bytes (u64).
/// Virtual address breakdown (48-bit):
///   Bits 47-39: PML4 index (9 bits)
///   Bits 38-30: PDPT index (9 bits)
///   Bits 29-21: PD index (9 bits)
///   Bits 20-12: PT index (9 bits)
///   Bits 11-0:  Page offset (12 bits)
///
/// Features:
///   - Full 4-level page walk with translate / virt_to_phys
///   - Map and unmap pages (4KB and 2MB huge pages)
///   - Permission flag management (change_permissions)
///   - Clone page table for fork (COW-ready)
///   - User address space creation (fresh PML4 with kernel half)
///   - Guard page installation (stack overflow protection)
///   - Page table statistics (mapped pages, table pages)
///   - Page fault handler with demand paging + COW
///
/// Inspired by: Linux page table walker, seL4's capability-based mapping,
/// and Redox's Rust page table implementation. All code is original.
use crate::serial_println;

/// Page table entry flags (lower 12 bits of each entry)
pub mod flags {
    pub const PRESENT: u64 = 1 << 0;
    pub const WRITABLE: u64 = 1 << 1;
    pub const USER_ACCESSIBLE: u64 = 1 << 2;
    pub const WRITE_THROUGH: u64 = 1 << 3;
    pub const NO_CACHE: u64 = 1 << 4;
    pub const ACCESSED: u64 = 1 << 5;
    pub const DIRTY: u64 = 1 << 6;
    pub const HUGE_PAGE: u64 = 1 << 7;
    pub const GLOBAL: u64 = 1 << 8;
    pub const NO_EXECUTE: u64 = 1 << 63;

    /// Convenience: typical kernel code page (present, not writable, NX off)
    pub const KERNEL_RO: u64 = PRESENT;
    /// Convenience: typical kernel data page (present, writable)
    pub const KERNEL_RW: u64 = PRESENT | WRITABLE;
    /// Convenience: typical user code page (present, user, NX off)
    pub const USER_RO: u64 = PRESENT | USER_ACCESSIBLE;
    /// Convenience: typical user data page (present, writable, user)
    pub const USER_RW: u64 = PRESENT | WRITABLE | USER_ACCESSIBLE;
    /// Convenience: guard page (not present, nothing else)
    pub const GUARD: u64 = 0;

    /// Mask for extracting physical address from a PTE (bits 12..51)
    pub const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
    /// Mask for extracting flags from a PTE
    pub const FLAGS_MASK: u64 = !ADDR_MASK;
}

/// Number of entries per page table (512 for x86_64)
const ENTRIES_PER_TABLE: usize = 512;

/// 2MB huge page size
const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

/// Guard page marker (stored in unused PTE bits to identify guard pages)
const GUARD_MARKER: u64 = 0x0000_0000_0000_0200; // bit 9, software-available

/// A single page table (4KB aligned, 512 entries of 8 bytes each = 4096 bytes)
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [u64; ENTRIES_PER_TABLE],
}

impl PageTable {
    /// Zero out all entries
    pub fn zero(&mut self) {
        for entry in self.entries.iter_mut() {
            *entry = 0;
        }
    }

    /// Count non-zero (present or guard) entries
    pub fn entry_count(&self) -> usize {
        let mut count = 0;
        for entry in &self.entries {
            if *entry != 0 {
                count += 1;
            }
        }
        count
    }

    /// Check if this table is completely empty (all entries zero)
    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(|&e| e == 0)
    }
}

/// Page table walk statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct PageTableStats {
    /// Number of mapped 4KB pages
    pub mapped_4k_pages: usize,
    /// Number of mapped 2MB huge pages
    pub mapped_2m_pages: usize,
    /// Number of page table frames (PML4 + PDPT + PD + PT)
    pub table_frames: usize,
    /// Number of guard pages installed
    pub guard_pages: usize,
}

/// Extract PML4 index from virtual address (bits 47-39)
fn pml4_index(virt: usize) -> usize {
    (virt >> 39) & 0x1FF
}

/// Extract PDPT index from virtual address (bits 38-30)
fn pdpt_index(virt: usize) -> usize {
    (virt >> 30) & 0x1FF
}

/// Extract PD index from virtual address (bits 29-21)
fn pd_index(virt: usize) -> usize {
    (virt >> 21) & 0x1FF
}

/// Extract PT index from virtual address (bits 20-12)
fn pt_index(virt: usize) -> usize {
    (virt >> 12) & 0x1FF
}

/// Read the current PML4 table address from CR3
pub fn read_cr3() -> usize {
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    }
    (cr3 & 0x000F_FFFF_FFFF_F000) as usize // mask out flags
}

/// Write a new PML4 table address to CR3 (flushes TLB)
pub unsafe fn write_cr3(pml4_addr: usize) {
    core::arch::asm!("mov cr3, {}", in(reg) pml4_addr as u64, options(nostack));
}

/// Flush a single TLB entry for a virtual address
pub fn flush_tlb(virt_addr: usize) {
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) virt_addr, options(nostack));
    }
}

/// Flush the entire TLB (by reloading CR3)
pub fn flush_tlb_all() {
    let cr3 = read_cr3();
    unsafe {
        write_cr3(cr3);
    }
}

/// Get a mutable reference to a page table at a physical address.
///
/// SAFETY: We assume identity mapping (phys addr == virt addr) for kernel memory.
/// This is true during early boot when the bootloader sets up identity mapping.
unsafe fn table_at(phys_addr: usize) -> &'static mut PageTable {
    &mut *(phys_addr as *mut PageTable)
}

/// Walk the page tables to find the physical address for a virtual address.
/// Returns None if the address is not mapped.
pub fn translate(virt_addr: usize) -> Option<usize> {
    let pml4_addr = read_cr3();
    virt_to_phys_in(pml4_addr, virt_addr)
}

/// Walk a specific page table (given PML4 address) to resolve a virtual address.
pub fn virt_to_phys_in(pml4_addr: usize, virt_addr: usize) -> Option<usize> {
    let pml4 = unsafe { table_at(pml4_addr) };

    // Level 4: PML4
    let pml4e = pml4.entries[pml4_index(virt_addr)];
    if pml4e & flags::PRESENT == 0 {
        return None;
    }
    let pdpt_addr = (pml4e & flags::ADDR_MASK) as usize;

    // Level 3: PDPT
    let pdpt = unsafe { table_at(pdpt_addr) };
    let pdpte = pdpt.entries[pdpt_index(virt_addr)];
    if pdpte & flags::PRESENT == 0 {
        return None;
    }
    // Check for 1GB huge page
    if pdpte & flags::HUGE_PAGE != 0 {
        let frame_addr = (pdpte & 0x000F_FFFF_C000_0000) as usize;
        return Some(frame_addr | (virt_addr & 0x3FFF_FFFF));
    }
    let pd_addr = (pdpte & flags::ADDR_MASK) as usize;

    // Level 2: PD
    let pd = unsafe { table_at(pd_addr) };
    let pde = pd.entries[pd_index(virt_addr)];
    if pde & flags::PRESENT == 0 {
        return None;
    }
    // Check for 2MB huge page
    if pde & flags::HUGE_PAGE != 0 {
        let frame_addr = (pde & 0x000F_FFFF_FFE0_0000) as usize;
        return Some(frame_addr | (virt_addr & 0x001F_FFFF));
    }
    let pt_addr = (pde & flags::ADDR_MASK) as usize;

    // Level 1: PT
    let pt = unsafe { table_at(pt_addr) };
    let pte = pt.entries[pt_index(virt_addr)];
    if pte & flags::PRESENT == 0 {
        return None;
    }
    let frame_addr = (pte & flags::ADDR_MASK) as usize;
    Some(frame_addr | (virt_addr & 0xFFF))
}

/// Get the raw page table entry for a virtual address. Returns None if not mapped.
pub fn get_pte(virt_addr: usize) -> Option<u64> {
    let virt_aligned = virt_addr & !0xFFF;
    let pml4_addr = read_cr3();
    let pml4 = unsafe { table_at(pml4_addr) };

    let pml4e = pml4.entries[pml4_index(virt_aligned)];
    if pml4e & flags::PRESENT == 0 {
        return None;
    }
    let pdpt = unsafe { table_at((pml4e & flags::ADDR_MASK) as usize) };

    let pdpte = pdpt.entries[pdpt_index(virt_aligned)];
    if pdpte & flags::PRESENT == 0 {
        return None;
    }
    if pdpte & flags::HUGE_PAGE != 0 {
        return Some(pdpte);
    }
    let pd = unsafe { table_at((pdpte & flags::ADDR_MASK) as usize) };

    let pde = pd.entries[pd_index(virt_aligned)];
    if pde & flags::PRESENT == 0 {
        return None;
    }
    if pde & flags::HUGE_PAGE != 0 {
        return Some(pde);
    }
    let pt = unsafe { table_at((pde & flags::ADDR_MASK) as usize) };

    Some(pt.entries[pt_index(virt_aligned)])
}

/// Map a virtual page to a physical frame.
///
/// Allocates intermediate page tables as needed using the frame allocator.
/// Returns Ok(()) on success, Err(()) if allocation fails.
pub fn map_page(virt_addr: usize, phys_addr: usize, page_flags: u64) -> Result<(), ()> {
    let virt_aligned = virt_addr & !0xFFF;
    let phys_aligned = phys_addr & !0xFFF;
    let entry_flags = page_flags | flags::PRESENT;

    let pml4_addr = read_cr3();
    map_page_in(pml4_addr, virt_aligned, phys_aligned, entry_flags)
}

/// Map a virtual page in a specific page table (given PML4 address).
pub fn map_page_in(
    pml4_addr: usize,
    virt_addr: usize,
    phys_addr: usize,
    entry_flags: u64,
) -> Result<(), ()> {
    let virt_aligned = virt_addr & !0xFFF;
    let phys_aligned = phys_addr & !0xFFF;
    let pml4 = unsafe { table_at(pml4_addr) };

    // Ensure PML4 -> PDPT
    let pdpt_addr = ensure_table(&mut pml4.entries[pml4_index(virt_aligned)], entry_flags)?;
    let pdpt = unsafe { table_at(pdpt_addr) };

    // Ensure PDPT -> PD
    let pd_addr = ensure_table(&mut pdpt.entries[pdpt_index(virt_aligned)], entry_flags)?;
    let pd = unsafe { table_at(pd_addr) };

    // Ensure PD -> PT
    let pt_addr = ensure_table(&mut pd.entries[pd_index(virt_aligned)], entry_flags)?;
    let pt = unsafe { table_at(pt_addr) };

    // Set the final PT entry
    pt.entries[pt_index(virt_aligned)] = (phys_aligned as u64) | entry_flags;

    flush_tlb(virt_aligned);
    Ok(())
}

/// Map a 2MB huge page. The virtual and physical addresses must be 2MB-aligned.
pub fn map_huge_page(virt_addr: usize, phys_addr: usize, page_flags: u64) -> Result<(), ()> {
    let virt_aligned = virt_addr & !(HUGE_PAGE_SIZE - 1);
    let phys_aligned = phys_addr & !(HUGE_PAGE_SIZE - 1);

    if virt_addr != virt_aligned || phys_addr != phys_aligned {
        return Err(()); // Not 2MB aligned
    }

    let entry_flags = page_flags | flags::PRESENT | flags::HUGE_PAGE;
    let pml4_addr = read_cr3();
    map_huge_page_in(pml4_addr, virt_aligned, phys_aligned, entry_flags)
}

/// Map a 2MB huge page in a specific page table.
pub fn map_huge_page_in(
    pml4_addr: usize,
    virt_addr: usize,
    phys_addr: usize,
    entry_flags: u64,
) -> Result<(), ()> {
    let pml4 = unsafe { table_at(pml4_addr) };

    // Ensure PML4 -> PDPT
    let pdpt_addr = ensure_table(&mut pml4.entries[pml4_index(virt_addr)], entry_flags)?;
    let pdpt = unsafe { table_at(pdpt_addr) };

    // Ensure PDPT -> PD
    let pd_addr = ensure_table(&mut pdpt.entries[pdpt_index(virt_addr)], entry_flags)?;
    let pd = unsafe { table_at(pd_addr) };

    // Set PD entry directly as a huge page (no PT needed)
    pd.entries[pd_index(virt_addr)] = (phys_addr as u64) | entry_flags;

    // Flush TLB for the entire 2MB range
    let pages = HUGE_PAGE_SIZE / FRAME_SIZE;
    for i in 0..pages {
        flush_tlb(virt_addr + i * FRAME_SIZE);
    }

    Ok(())
}

/// Unmap a 2MB huge page. Frees the 512 underlying physical frames.
pub fn unmap_huge_page(virt_addr: usize) -> Result<(), ()> {
    let virt_aligned = virt_addr & !(HUGE_PAGE_SIZE - 1);
    let pml4_addr = read_cr3();
    let pml4 = unsafe { table_at(pml4_addr) };

    let pml4e = pml4.entries[pml4_index(virt_aligned)];
    if pml4e & flags::PRESENT == 0 {
        return Err(());
    }
    let pdpt = unsafe { table_at((pml4e & flags::ADDR_MASK) as usize) };

    let pdpte = pdpt.entries[pdpt_index(virt_aligned)];
    if pdpte & flags::PRESENT == 0 {
        return Err(());
    }
    let pd = unsafe { table_at((pdpte & flags::ADDR_MASK) as usize) };

    let pde = pd.entries[pd_index(virt_aligned)];
    if pde & flags::HUGE_PAGE == 0 {
        return Err(());
    }

    let phys_base = (pde & 0x000F_FFFF_FFE0_0000) as usize;
    pd.entries[pd_index(virt_aligned)] = 0;

    // Flush TLB for the entire 2MB range
    let pages = HUGE_PAGE_SIZE / FRAME_SIZE;
    for i in 0..pages {
        flush_tlb(virt_aligned + i * FRAME_SIZE);
    }

    // Free the physical frames (512 x 4KB = 2MB)
    for i in 0..pages {
        frame_allocator::deallocate_frame(PhysFrame::from_addr(phys_base + i * FRAME_SIZE));
    }

    Ok(())
}

/// Ensure a page table entry points to a valid next-level table.
/// If the entry is not present, allocate a new frame for it.
fn ensure_table(entry: &mut u64, parent_flags: u64) -> Result<usize, ()> {
    if *entry & flags::PRESENT != 0 {
        // Already present — return the address, but also promote permissions
        // (e.g., if parent is writable and entry is read-only, make it writable)
        let addr = (*entry & flags::ADDR_MASK) as usize;
        // Promote writable and user bits upward
        *entry |= parent_flags & (flags::WRITABLE | flags::USER_ACCESSIBLE);
        Ok(addr)
    } else {
        // Allocate a new frame for this table
        let frame = frame_allocator::allocate_frame().ok_or(())?;
        // Zero out the new table
        unsafe {
            let table = table_at(frame.addr);
            table.zero();
        }
        // Set the entry with PRESENT + WRITABLE + optional USER
        *entry = (frame.addr as u64)
            | flags::PRESENT
            | flags::WRITABLE
            | (parent_flags & flags::USER_ACCESSIBLE);
        Ok(frame.addr)
    }
}

/// Unmap a virtual page. Does NOT free the physical frame.
pub fn unmap_page(virt_addr: usize) {
    let virt_aligned = virt_addr & !0xFFF;

    let pml4_addr = read_cr3();
    let pml4 = unsafe { table_at(pml4_addr) };

    let pml4e = pml4.entries[pml4_index(virt_aligned)];
    if pml4e & flags::PRESENT == 0 {
        return;
    }
    let pdpt = unsafe { table_at((pml4e & flags::ADDR_MASK) as usize) };

    let pdpte = pdpt.entries[pdpt_index(virt_aligned)];
    if pdpte & flags::PRESENT == 0 {
        return;
    }
    let pd = unsafe { table_at((pdpte & flags::ADDR_MASK) as usize) };

    let pde = pd.entries[pd_index(virt_aligned)];
    if pde & flags::PRESENT == 0 {
        return;
    }
    let pt = unsafe { table_at((pde & flags::ADDR_MASK) as usize) };

    // Clear the PT entry
    pt.entries[pt_index(virt_aligned)] = 0;
    flush_tlb(virt_aligned);
}

/// Unmap a virtual page and free the underlying physical frame.
/// Returns the physical address that was freed, or None if not mapped.
pub fn unmap_page_free(virt_addr: usize) -> Option<usize> {
    let virt_aligned = virt_addr & !0xFFF;
    let pml4_addr = read_cr3();
    let pml4 = unsafe { table_at(pml4_addr) };

    let pml4e = pml4.entries[pml4_index(virt_aligned)];
    if pml4e & flags::PRESENT == 0 {
        return None;
    }
    let pdpt = unsafe { table_at((pml4e & flags::ADDR_MASK) as usize) };

    let pdpte = pdpt.entries[pdpt_index(virt_aligned)];
    if pdpte & flags::PRESENT == 0 {
        return None;
    }
    let pd = unsafe { table_at((pdpte & flags::ADDR_MASK) as usize) };

    let pde = pd.entries[pd_index(virt_aligned)];
    if pde & flags::PRESENT == 0 {
        return None;
    }
    let pt = unsafe { table_at((pde & flags::ADDR_MASK) as usize) };

    let pte = pt.entries[pt_index(virt_aligned)];
    if pte & flags::PRESENT == 0 {
        return None;
    }

    let phys_addr = (pte & flags::ADDR_MASK) as usize;
    pt.entries[pt_index(virt_aligned)] = 0;
    flush_tlb(virt_aligned);

    // Free the physical frame
    frame_allocator::deallocate_frame(PhysFrame::from_addr(phys_addr));
    Some(phys_addr)
}

/// Change the permission flags on an existing mapped page without changing the
/// physical frame. Returns Err if the page is not mapped.
pub fn change_permissions(virt_addr: usize, new_flags: u64) -> Result<(), ()> {
    let virt_aligned = virt_addr & !0xFFF;
    let pml4_addr = read_cr3();
    let pml4 = unsafe { table_at(pml4_addr) };

    let pml4e = pml4.entries[pml4_index(virt_aligned)];
    if pml4e & flags::PRESENT == 0 {
        return Err(());
    }
    let pdpt = unsafe { table_at((pml4e & flags::ADDR_MASK) as usize) };

    let pdpte = pdpt.entries[pdpt_index(virt_aligned)];
    if pdpte & flags::PRESENT == 0 {
        return Err(());
    }
    if pdpte & flags::HUGE_PAGE != 0 {
        // Cannot change permissions on individual pages within a huge page
        return Err(());
    }
    let pd = unsafe { table_at((pdpte & flags::ADDR_MASK) as usize) };

    let pde = pd.entries[pd_index(virt_aligned)];
    if pde & flags::PRESENT == 0 {
        return Err(());
    }
    if pde & flags::HUGE_PAGE != 0 {
        // 2MB huge page — change its permissions directly
        let phys = pde & 0x000F_FFFF_FFE0_0000;
        let pd_mut = unsafe { table_at((pdpte & flags::ADDR_MASK) as usize) };
        pd_mut.entries[pd_index(virt_aligned)] =
            phys | new_flags | flags::PRESENT | flags::HUGE_PAGE;
        // Flush for entire 2MB range
        let pages = HUGE_PAGE_SIZE / FRAME_SIZE;
        for i in 0..pages {
            flush_tlb(virt_aligned + i * FRAME_SIZE);
        }
        return Ok(());
    }
    let pt = unsafe { table_at((pde & flags::ADDR_MASK) as usize) };

    let pte = pt.entries[pt_index(virt_aligned)];
    if pte & flags::PRESENT == 0 {
        return Err(());
    }

    // Keep the physical address, replace flags
    let phys_addr = pte & flags::ADDR_MASK;
    pt.entries[pt_index(virt_aligned)] = phys_addr | new_flags | flags::PRESENT;
    flush_tlb(virt_aligned);

    Ok(())
}

/// Install a guard page at a virtual address. The page is unmapped and marked
/// with a special marker so page faults on it can be identified as stack overflows.
pub fn install_guard_page(virt_addr: usize) -> Result<(), ()> {
    let virt_aligned = virt_addr & !0xFFF;
    let pml4_addr = read_cr3();
    let pml4 = unsafe { table_at(pml4_addr) };

    // We need the page table to exist so we can store the guard marker
    let entry_flags = flags::PRESENT | flags::WRITABLE;

    let pdpt_addr = ensure_table(&mut pml4.entries[pml4_index(virt_aligned)], entry_flags)?;
    let pdpt = unsafe { table_at(pdpt_addr) };

    let pd_addr = ensure_table(&mut pdpt.entries[pdpt_index(virt_aligned)], entry_flags)?;
    let pd = unsafe { table_at(pd_addr) };

    let pt_addr = ensure_table(&mut pd.entries[pd_index(virt_aligned)], entry_flags)?;
    let pt = unsafe { table_at(pt_addr) };

    // Set PTE to not-present with guard marker (so we can detect it in fault handler)
    pt.entries[pt_index(virt_aligned)] = GUARD_MARKER; // not PRESENT, but marker set
    flush_tlb(virt_aligned);

    Ok(())
}

/// Check if a virtual address has a guard page installed
pub fn is_guard_page(virt_addr: usize) -> bool {
    let virt_aligned = virt_addr & !0xFFF;
    if let Some(pte) = get_pte(virt_aligned) {
        // Guard page: not present, but has the guard marker bit
        pte & flags::PRESENT == 0 && pte & GUARD_MARKER != 0
    } else {
        false
    }
}

/// Clone a page table for fork (COW — mark all user pages read-only in both parent and child)
pub fn clone_page_table() -> Result<usize, ()> {
    let parent_pml4_addr = read_cr3();

    // Allocate new PML4 for child
    let child_pml4_frame = frame_allocator::allocate_frame().ok_or(())?;
    let child_pml4 = unsafe { table_at(child_pml4_frame.addr) };
    child_pml4.zero();

    // Copy kernel mappings (upper half) directly — they share kernel space
    let parent_pml4 = unsafe { table_at(parent_pml4_addr) };
    for i in 256..512 {
        child_pml4.entries[i] = parent_pml4.entries[i];
    }

    // For user-space mappings (lower half), deep copy with COW
    for i in 0..256 {
        let pml4e = parent_pml4.entries[i];
        if pml4e & flags::PRESENT == 0 {
            continue;
        }

        // Allocate new PDPT for child
        let child_pdpt_frame = frame_allocator::allocate_frame().ok_or(())?;
        let child_pdpt = unsafe { table_at(child_pdpt_frame.addr) };
        child_pdpt.zero();

        let parent_pdpt = unsafe { table_at((pml4e & flags::ADDR_MASK) as usize) };

        for j in 0..ENTRIES_PER_TABLE {
            let pdpte = parent_pdpt.entries[j];
            if pdpte & flags::PRESENT == 0 {
                continue;
            }
            if pdpte & flags::HUGE_PAGE != 0 {
                // 1GB huge page: share with COW (remove writable)
                let cow_entry = pdpte & !flags::WRITABLE;
                child_pdpt.entries[j] = cow_entry;
                parent_pdpt.entries[j] = cow_entry;
                continue;
            }

            // Allocate new PD for child
            let child_pd_frame = frame_allocator::allocate_frame().ok_or(())?;
            let child_pd = unsafe { table_at(child_pd_frame.addr) };
            child_pd.zero();

            let parent_pd = unsafe { table_at((pdpte & flags::ADDR_MASK) as usize) };

            for k in 0..ENTRIES_PER_TABLE {
                let pde = parent_pd.entries[k];
                if pde & flags::PRESENT == 0 {
                    continue;
                }
                if pde & flags::HUGE_PAGE != 0 {
                    // 2MB huge page: share with COW
                    let cow_entry = pde & !flags::WRITABLE;
                    child_pd.entries[k] = cow_entry;
                    parent_pd.entries[k] = cow_entry;
                    continue;
                }

                // Allocate new PT for child
                let child_pt_frame = frame_allocator::allocate_frame().ok_or(())?;
                let child_pt = unsafe { table_at(child_pt_frame.addr) };
                child_pt.zero();

                let parent_pt = unsafe { table_at((pde & flags::ADDR_MASK) as usize) };

                for l in 0..ENTRIES_PER_TABLE {
                    let pte = parent_pt.entries[l];
                    if pte & flags::PRESENT == 0 {
                        // Copy guard markers and non-present entries (e.g., demand stubs) through
                        if pte != 0 {
                            child_pt.entries[l] = pte;
                        }
                        continue;
                    }

                    // Mark both parent and child as COW:
                    //   - clear WRITABLE so writes will fault
                    //   - set COW_BIT so the fault handler knows this is a COW page
                    //     (not a genuinely read-only page)
                    let cow_entry = if pte & flags::WRITABLE != 0 {
                        // Was writable → make COW: clear WRITABLE, set COW_BIT
                        (pte & !flags::WRITABLE) | COW_BIT
                    } else {
                        // Was already read-only → keep as-is, no COW_BIT needed
                        pte
                    };
                    child_pt.entries[l] = cow_entry;
                    parent_pt.entries[l] = cow_entry;
                    // Note: TLB flush is deferred — caller does flush_tlb_all() after clone

                    // Increment reference count on the physical frame
                    let phys = (pte & flags::ADDR_MASK) as usize;
                    frame_allocator::inc_refcount(PhysFrame::from_addr(phys));
                }

                child_pd.entries[k] = (child_pt_frame.addr as u64)
                    | flags::PRESENT
                    | flags::WRITABLE
                    | (pde & flags::USER_ACCESSIBLE);
            }

            child_pdpt.entries[j] = (child_pd_frame.addr as u64)
                | flags::PRESENT
                | flags::WRITABLE
                | (pdpte & flags::USER_ACCESSIBLE);
        }

        child_pml4.entries[i] = (child_pdpt_frame.addr as u64)
            | flags::PRESENT
            | flags::WRITABLE
            | (pml4e & flags::USER_ACCESSIBLE);
    }

    // All parent writable pages are now marked COW (read-only + COW_BIT).
    // Flush the entire TLB so the CPU sees the new read-only mappings for
    // the parent.  The child has not been scheduled yet, so its TLB is empty.
    // Use a SeqCst fence first so all page-table writes are globally visible
    // before the CR3 reload.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    flush_tlb_all();

    Ok(child_pml4_frame.addr)
}

/// Create a fresh user address space: a new PML4 with the kernel half
/// (entries 256..511) mapped identically to the current kernel.
/// Returns the physical address of the new PML4.
pub fn create_user_address_space() -> Result<usize, ()> {
    let kernel_pml4_addr = read_cr3();
    let kernel_pml4 = unsafe { table_at(kernel_pml4_addr) };

    let user_pml4_frame = frame_allocator::allocate_frame().ok_or(())?;
    let user_pml4 = unsafe { table_at(user_pml4_frame.addr) };
    user_pml4.zero();

    // Copy kernel half (upper 256 entries)
    for i in 256..512 {
        user_pml4.entries[i] = kernel_pml4.entries[i];
    }

    Ok(user_pml4_frame.addr)
}

/// Free a user address space page table. Frees all user-half page table
/// frames and the physical pages they point to (unless reference count > 1).
/// Does NOT free the kernel half.
pub fn free_user_address_space(pml4_addr: usize) {
    let pml4 = unsafe { table_at(pml4_addr) };

    // Only free user half (entries 0..255)
    for i in 0..256 {
        let pml4e = pml4.entries[i];
        if pml4e & flags::PRESENT == 0 {
            continue;
        }

        let pdpt_addr = (pml4e & flags::ADDR_MASK) as usize;
        let pdpt = unsafe { table_at(pdpt_addr) };

        for j in 0..ENTRIES_PER_TABLE {
            let pdpte = pdpt.entries[j];
            if pdpte & flags::PRESENT == 0 {
                continue;
            }
            if pdpte & flags::HUGE_PAGE != 0 {
                continue;
            } // Don't free shared huge pages

            let pd_addr = (pdpte & flags::ADDR_MASK) as usize;
            let pd = unsafe { table_at(pd_addr) };

            for k in 0..ENTRIES_PER_TABLE {
                let pde = pd.entries[k];
                if pde & flags::PRESENT == 0 {
                    continue;
                }
                if pde & flags::HUGE_PAGE != 0 {
                    continue;
                }

                let pt_addr = (pde & flags::ADDR_MASK) as usize;
                let pt = unsafe { table_at(pt_addr) };

                // Free all mapped physical pages (via refcount)
                for l in 0..ENTRIES_PER_TABLE {
                    let pte = pt.entries[l];
                    if pte & flags::PRESENT != 0 {
                        let phys = (pte & flags::ADDR_MASK) as usize;
                        frame_allocator::dec_refcount(PhysFrame::from_addr(phys));
                    }
                }

                // Free the PT frame itself
                frame_allocator::deallocate_frame(PhysFrame::from_addr(pt_addr));
            }

            // Free the PD frame
            frame_allocator::deallocate_frame(PhysFrame::from_addr(pd_addr));
        }

        // Free the PDPT frame
        frame_allocator::deallocate_frame(PhysFrame::from_addr(pdpt_addr));
    }

    // Free the PML4 frame itself
    frame_allocator::deallocate_frame(PhysFrame::from_addr(pml4_addr));
}

/// Gather page table statistics by walking the entire table hierarchy
pub fn page_table_stats() -> PageTableStats {
    let pml4_addr = read_cr3();
    page_table_stats_for(pml4_addr)
}

/// Gather page table statistics for a specific PML4
pub fn page_table_stats_for(pml4_addr: usize) -> PageTableStats {
    let mut stats = PageTableStats::default();
    stats.table_frames = 1; // PML4 itself

    let pml4 = unsafe { table_at(pml4_addr) };
    for i in 0..ENTRIES_PER_TABLE {
        let pml4e = pml4.entries[i];
        if pml4e & flags::PRESENT == 0 {
            continue;
        }

        stats.table_frames += 1;
        let pdpt = unsafe { table_at((pml4e & flags::ADDR_MASK) as usize) };

        for j in 0..ENTRIES_PER_TABLE {
            let pdpte = pdpt.entries[j];
            if pdpte & flags::PRESENT == 0 {
                continue;
            }
            if pdpte & flags::HUGE_PAGE != 0 {
                // 1GB page = 512 * 2MB pages conceptually
                stats.mapped_2m_pages += 512;
                continue;
            }

            stats.table_frames += 1;
            let pd = unsafe { table_at((pdpte & flags::ADDR_MASK) as usize) };

            for k in 0..ENTRIES_PER_TABLE {
                let pde = pd.entries[k];
                if pde & flags::PRESENT == 0 {
                    continue;
                }
                if pde & flags::HUGE_PAGE != 0 {
                    stats.mapped_2m_pages += 1;
                    continue;
                }

                stats.table_frames += 1;
                let pt = unsafe { table_at((pde & flags::ADDR_MASK) as usize) };

                for l in 0..ENTRIES_PER_TABLE {
                    let pte = pt.entries[l];
                    if pte & flags::PRESENT != 0 {
                        stats.mapped_4k_pages += 1;
                    } else if pte & GUARD_MARKER != 0 {
                        stats.guard_pages += 1;
                    }
                }
            }
        }
    }

    stats
}

/// Map a range of virtual addresses to a range of physical addresses.
/// Both must be page-aligned. `count` is the number of 4KB pages to map.
pub fn map_range(
    virt_start: usize,
    phys_start: usize,
    count: usize,
    page_flags: u64,
) -> Result<(), ()> {
    for i in 0..count {
        let virt = virt_start + i * FRAME_SIZE;
        let phys = phys_start + i * FRAME_SIZE;
        map_page(virt, phys, page_flags)?;
    }
    Ok(())
}

/// Unmap a range of virtual addresses and free the physical frames.
pub fn unmap_range_free(virt_start: usize, count: usize) {
    for i in 0..count {
        let virt = virt_start + i * FRAME_SIZE;
        unmap_page_free(virt);
    }
}

/// Software COW bit — stored in available PTE bit 9 (same bit as GUARD_MARKER).
///
/// We reuse bit 9 for COW marking.  A page is COW when:
///   - PRESENT is set (page is mapped)
///   - WRITABLE is clear (write will fault)
///   - COW_BIT is set (distinguish from genuinely read-only pages)
///
/// On a write fault to a COW page:
///   - If refcount == 1: promote in-place (set WRITABLE, clear COW_BIT)
///   - If refcount > 1: copy physical frame, remap writable, dec old refcount
pub const COW_BIT: u64 = 1 << 9; // software bit, in available PTE bits

/// Mark an already-mapped page as copy-on-write.
/// Clears WRITABLE, sets COW_BIT, flushes TLB.
pub fn mark_page_cow(virt_addr: usize) -> Result<(), ()> {
    let virt_aligned = virt_addr & !0xFFF;
    let pml4_addr = read_cr3();
    let pml4 = unsafe { table_at(pml4_addr) };

    let pml4e = pml4.entries[pml4_index(virt_aligned)];
    if pml4e & flags::PRESENT == 0 {
        return Err(());
    }
    let pdpt = unsafe { table_at((pml4e & flags::ADDR_MASK) as usize) };

    let pdpte = pdpt.entries[pdpt_index(virt_aligned)];
    if pdpte & flags::PRESENT == 0 {
        return Err(());
    }
    if pdpte & flags::HUGE_PAGE != 0 {
        return Err(());
    }
    let pd = unsafe { table_at((pdpte & flags::ADDR_MASK) as usize) };

    let pde = pd.entries[pd_index(virt_aligned)];
    if pde & flags::PRESENT == 0 {
        return Err(());
    }
    if pde & flags::HUGE_PAGE != 0 {
        return Err(());
    }
    let pt = unsafe { table_at((pde & flags::ADDR_MASK) as usize) };

    let pte = pt.entries[pt_index(virt_aligned)];
    if pte & flags::PRESENT == 0 {
        return Err(());
    }

    let phys_addr = pte & flags::ADDR_MASK;
    // Keep all flags except WRITABLE, add COW_BIT
    let new_flags = (pte & flags::FLAGS_MASK & !flags::WRITABLE) | COW_BIT | flags::PRESENT;
    pt.entries[pt_index(virt_aligned)] = phys_addr | new_flags;
    flush_tlb(virt_aligned);
    Ok(())
}

/// Mark a page as COW in a specific page table (used by fork).
pub fn mark_page_cow_in(pml4_addr: usize, virt_addr: usize) -> Result<(), ()> {
    let virt_aligned = virt_addr & !0xFFF;
    let pml4 = unsafe { table_at(pml4_addr) };

    let pml4e = pml4.entries[pml4_index(virt_aligned)];
    if pml4e & flags::PRESENT == 0 {
        return Err(());
    }
    let pdpt = unsafe { table_at((pml4e & flags::ADDR_MASK) as usize) };

    let pdpte = pdpt.entries[pdpt_index(virt_aligned)];
    if pdpte & flags::PRESENT == 0 {
        return Err(());
    }
    if pdpte & flags::HUGE_PAGE != 0 {
        return Err(());
    }
    let pd = unsafe { table_at((pdpte & flags::ADDR_MASK) as usize) };

    let pde = pd.entries[pd_index(virt_aligned)];
    if pde & flags::PRESENT == 0 {
        return Err(());
    }
    if pde & flags::HUGE_PAGE != 0 {
        return Err(());
    }
    let pt = unsafe { table_at((pde & flags::ADDR_MASK) as usize) };

    let pte = pt.entries[pt_index(virt_aligned)];
    if pte & flags::PRESENT == 0 {
        return Err(());
    }

    let phys_addr = pte & flags::ADDR_MASK;
    let new_flags = (pte & flags::FLAGS_MASK & !flags::WRITABLE) | COW_BIT | flags::PRESENT;
    pt.entries[pt_index(virt_aligned)] = phys_addr | new_flags;
    flush_tlb(virt_aligned);
    Ok(())
}

/// Handle a COW write fault at `fault_addr` in the page table at `pml4_addr`.
/// Returns true if handled.
fn handle_cow_in_table(pml4_addr: usize, fault_addr: usize) -> bool {
    let virt_aligned = fault_addr & !0xFFF;
    let pml4 = unsafe { table_at(pml4_addr) };

    let pml4e = pml4.entries[pml4_index(virt_aligned)];
    if pml4e & flags::PRESENT == 0 {
        return false;
    }
    let pdpt = unsafe { table_at((pml4e & flags::ADDR_MASK) as usize) };

    let pdpte = pdpt.entries[pdpt_index(virt_aligned)];
    if pdpte & flags::PRESENT == 0 {
        return false;
    }
    if pdpte & flags::HUGE_PAGE != 0 {
        return false;
    }
    let pd = unsafe { table_at((pdpte & flags::ADDR_MASK) as usize) };

    let pde = pd.entries[pd_index(virt_aligned)];
    if pde & flags::PRESENT == 0 {
        return false;
    }
    if pde & flags::HUGE_PAGE != 0 {
        return false;
    }
    let pt = unsafe { table_at((pde & flags::ADDR_MASK) as usize) };

    let pte = pt.entries[pt_index(virt_aligned)];
    // Must be present and have COW_BIT set
    if pte & flags::PRESENT == 0 {
        return false;
    }
    if pte & COW_BIT == 0 {
        return false;
    }

    let old_phys_aligned = (pte & flags::ADDR_MASK) as usize;
    let old_frame = PhysFrame::from_addr(old_phys_aligned);
    let refcount = frame_allocator::FRAME_ALLOCATOR.lock().refcount(old_frame);

    if refcount <= 1 {
        // Sole owner — promote in-place: set WRITABLE, clear COW_BIT
        let new_flags = (pte & flags::FLAGS_MASK & !COW_BIT) | flags::WRITABLE;
        pt.entries[pt_index(virt_aligned)] = (old_phys_aligned as u64) | new_flags;
        flush_tlb(virt_aligned);
        serial_println!("  Paging: COW promote {:#x} (sole owner)", fault_addr);
        return true;
    }

    // Shared — must copy
    if let Some(new_frame) = frame_allocator::allocate_frame() {
        unsafe {
            core::ptr::copy_nonoverlapping(
                old_phys_aligned as *const u8,
                new_frame.addr as *mut u8,
                FRAME_SIZE,
            );
        }
        // Build new flags: keep USER_ACCESSIBLE / NO_EXECUTE, set WRITABLE, clear COW_BIT
        let new_flags =
            (pte & (flags::USER_ACCESSIBLE | flags::NO_EXECUTE)) | flags::WRITABLE | flags::PRESENT;
        pt.entries[pt_index(virt_aligned)] = (new_frame.addr as u64) | new_flags;
        flush_tlb(virt_aligned);
        // Release reference on old frame
        frame_allocator::dec_refcount(old_frame);
        serial_println!(
            "  Paging: COW copy {:#x} -> new frame {:#x}",
            fault_addr,
            new_frame.addr
        );
        return true;
    }

    false
}

/// Handle a page fault for demand paging / COW.
///
/// Priority:
///   1. Guard-page check — report stack overflow, do NOT handle.
///   2. COW write fault — present + write + COW_BIT set → copy or promote.
///   3. mmap demand fault — look up VMA in the global mmap table for this PID;
///      let the mmap subsystem allocate the physical page and map it.
///   4. Plain demand fault (no VMA) — allocate a zero page (kernel / early boot).
///   5. Unhandled — return false; caller delivers SIGSEGV or halts.
///
/// Returns true if the fault was handled (page is now accessible).
pub fn handle_page_fault(fault_addr: usize, error_code: u64) -> bool {
    // Bit 1 of error code: 0 = read, 1 = write
    let is_write = error_code & 0x2 != 0;
    // Bit 0: 0 = not present, 1 = protection violation
    let is_present = error_code & 0x1 != 0;
    // Bit 4: instruction fetch
    let _is_instr_fetch = error_code & 0x10 != 0;
    // Bit 3: reserved PTE bits set — fatal, cannot handle
    let is_reserved = error_code & 0x8 != 0;

    if is_reserved {
        serial_println!(
            "  Paging: RESERVED BIT in PTE at {:#x} — fatal PF",
            fault_addr
        );
        return false;
    }

    // --- Guard page detection ---
    if is_guard_page(fault_addr) {
        serial_println!(
            "  Paging: GUARD PAGE hit at {:#x} — stack overflow!",
            fault_addr
        );
        return false;
    }

    let pml4_addr = read_cr3();

    // --- COW write fault (present page, write, COW_BIT set) ---
    if is_write && is_present {
        if handle_cow_in_table(pml4_addr, fault_addr) {
            return true;
        }
        // If page is present + write but no COW_BIT → genuine protection violation
        // Fall through to segfault (return false).
        serial_println!("  Paging: PROT VIOLATION write at {:#x}", fault_addr);
        return false;
    }

    // --- Demand paging (page not present) ---
    if !is_present {
        let pid = crate::process::getpid();

        // Try mmap subsystem first — it knows about VMA layout and file backing
        if crate::memory::mmap::handle_fault(fault_addr, error_code, pid) {
            return true;
        }

        // No VMA covers this address for user space — potential segfault.
        // For kernel addresses (early boot demand), still allocate a zero page.
        let is_user = error_code & 0x4 != 0;
        if !is_user {
            // Kernel demand page (e.g., kernel heap expansion before mmap is set up)
            if let Some(frame) = frame_allocator::allocate_frame() {
                unsafe {
                    core::ptr::write_bytes(frame.addr as *mut u8, 0, FRAME_SIZE);
                }
                let page_flags = flags::WRITABLE;
                if map_page(fault_addr, frame.addr, page_flags).is_ok() {
                    serial_println!(
                        "  Paging: kernel demand {:#x} -> {:#x}",
                        fault_addr,
                        frame.addr
                    );
                    return true;
                }
                // Map failed — free frame we just allocated
                frame_allocator::deallocate_frame(PhysFrame::from_addr(frame.addr));
            }
        }
        // User-space address with no VMA → segfault (return false)
        serial_println!("  Paging: SEGFAULT demand {:#x} pid={}", fault_addr, pid);
        return false;
    }

    false
}

/// Initialize paging subsystem.
pub fn init() {
    let cr3 = read_cr3();
    serial_println!("  Paging: CR3 = {:#x}", cr3);

    if let Some(phys) = translate(0xb8000) {
        serial_println!(
            "  Paging: VGA {:#x} -> {:#x} (identity mapped)",
            0xb8000,
            phys
        );
    } else {
        serial_println!("  Paging: WARNING — VGA buffer not mapped!");
    }
}
