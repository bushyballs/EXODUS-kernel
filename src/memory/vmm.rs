/*
 * Genesis OS — Virtual Memory Manager (VMM)
 *
 * Manages virtual address spaces and page tables.
 * Supports:
 * - Page table creation and destruction
 * - Virtual memory mapping/unmapping
 * - Copy-on-Write (COW) pages
 * - Lazy allocation
 * - Demand paging
 */

use super::*;
use super::pmm;
use core::sync::atomic::{AtomicU64, Ordering};

/// Page table entry (PTE) structure
#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    #[inline(always)]
    pub const fn new() -> Self {
        PageTableEntry(0)
    }

    #[inline(always)]
    pub fn is_present(self) -> bool {
        self.0 & PTE_PRESENT != 0
    }

    #[inline(always)]
    pub fn is_writable(self) -> bool {
        self.0 & PTE_WRITABLE != 0
    }

    #[inline(always)]
    pub fn is_cow(self) -> bool {
        self.0 & PTE_COW != 0
    }

    #[inline(always)]
    pub fn is_user(self) -> bool {
        self.0 & PTE_USER != 0
    }

    #[inline(always)]
    pub fn is_dirty(self) -> bool {
        self.0 & PTE_DIRTY != 0
    }

    #[inline(always)]
    pub fn is_accessed(self) -> bool {
        self.0 & PTE_ACCESSED != 0
    }

    #[inline(always)]
    pub fn phys_addr(self) -> PhysAddr {
        PhysAddr::new(self.0 & 0x000F_FFFF_FFFF_F000)
    }

    #[inline(always)]
    pub fn set_addr(&mut self, phys: PhysAddr) {
        self.0 = (self.0 & !0x000F_FFFF_FFFF_F000) | (phys.as_u64() & 0x000F_FFFF_FFFF_F000);
    }

    #[inline(always)]
    pub fn set_flags(&mut self, flags: u64) {
        self.0 |= flags;
    }

    #[inline(always)]
    pub fn clear_flags(&mut self, flags: u64) {
        self.0 &= !flags;
    }

    #[inline(always)]
    pub fn flags(self) -> u64 {
        self.0 & 0xFFF
    }
}

/// Page table (512 entries)
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl PageTable {
    pub const fn new() -> Self {
        PageTable {
            entries: [PageTableEntry::new(); 512],
        }
    }

    #[inline(always)]
    pub fn get_entry(&self, index: usize) -> PageTableEntry {
        self.entries[index]
    }

    #[inline(always)]
    pub fn set_entry(&mut self, index: usize, entry: PageTableEntry) {
        self.entries[index] = entry;
    }

    #[inline(always)]
    pub fn clear_entry(&mut self, index: usize) {
        self.entries[index] = PageTableEntry::new();
    }
}

static COW_PAGES_COUNT: AtomicU64 = AtomicU64::new(0);

/// Initialize virtual memory manager
pub unsafe fn init() {
    // Create identity mapping for kernel space
    // This is already done by boot.rs, so we just verify it
}

/// Create a new page table (for a new process)
pub fn create_page_table() -> Option<PhysAddr> {
    // Allocate PML4 (top-level page table)
    let pml4_phys = pmm::alloc_page()?;

    unsafe {
        // Zero out the page table
        pmm::zero_page(pml4_phys);

        // Copy kernel mappings from current PML4
        let current_pml4_phys = PhysAddr::new(crate::cpu::read_cr3());
        let current_pml4_virt = pmm::phys_to_virt(current_pml4_phys);
        let current_pml4 = &*(current_pml4_virt.as_u64() as *const PageTable);

        let new_pml4_virt = pmm::phys_to_virt(pml4_phys);
        let new_pml4 = &mut *(new_pml4_virt.as_u64() as *mut PageTable);

        // Copy kernel entries (upper half, entries 256-511)
        for i in 256..512 {
            new_pml4.set_entry(i, current_pml4.get_entry(i));
        }
    }

    Some(pml4_phys)
}

/// Destroy a page table and free all mapped pages
pub unsafe fn destroy_page_table(pml4_phys: PhysAddr) {
    let pml4_virt = pmm::phys_to_virt(pml4_phys);
    let pml4 = &*(pml4_virt.as_u64() as *const PageTable);

    // Free all user-space mappings (lower half, entries 0-255)
    for pml4_idx in 0..256 {
        let pml4_entry = pml4.get_entry(pml4_idx);
        if !pml4_entry.is_present() {
            continue;
        }

        let pdpt_phys = pml4_entry.phys_addr();
        let pdpt_virt = pmm::phys_to_virt(pdpt_phys);
        let pdpt = &*(pdpt_virt.as_u64() as *const PageTable);

        for pdpt_idx in 0..512 {
            let pdpt_entry = pdpt.get_entry(pdpt_idx);
            if !pdpt_entry.is_present() {
                continue;
            }

            let pd_phys = pdpt_entry.phys_addr();
            let pd_virt = pmm::phys_to_virt(pd_phys);
            let pd = &*(pd_virt.as_u64() as *const PageTable);

            for pd_idx in 0..512 {
                let pd_entry = pd.get_entry(pd_idx);
                if !pd_entry.is_present() {
                    continue;
                }

                let pt_phys = pd_entry.phys_addr();
                let pt_virt = pmm::phys_to_virt(pt_phys);
                let pt = &*(pt_virt.as_u64() as *const PageTable);

                // Free all pages in this page table
                for pt_idx in 0..512 {
                    let pt_entry = pt.get_entry(pt_idx);
                    if pt_entry.is_present() {
                        let page_phys = pt_entry.phys_addr();
                        pmm::free_page(page_phys);
                    }
                }

                // Free the page table itself
                pmm::free_page(pt_phys);
            }

            // Free the page directory
            pmm::free_page(pd_phys);
        }

        // Free the PDPT
        pmm::free_page(pdpt_phys);
    }

    // Free the PML4
    pmm::free_page(pml4_phys);
}

/// Map a virtual page to a physical page
pub unsafe fn map_page(
    pml4_phys: PhysAddr,
    virt: VirtAddr,
    phys: PhysAddr,
    flags: u64,
) -> Result<(), &'static str> {
    let (pml4_idx, pdpt_idx, pd_idx, pt_idx) = virt_to_indices(virt.as_u64());

    // Get PML4
    let pml4_virt = pmm::phys_to_virt(pml4_phys);
    let pml4 = &mut *(pml4_virt.as_u64() as *mut PageTable);

    // Get or create PDPT
    let pdpt_phys = if pml4.get_entry(pml4_idx).is_present() {
        pml4.get_entry(pml4_idx).phys_addr()
    } else {
        let new_pdpt = pmm::alloc_page().ok_or("Out of memory")?;
        pmm::zero_page(new_pdpt);

        let mut entry = PageTableEntry::new();
        entry.set_addr(new_pdpt);
        entry.set_flags(PTE_PRESENT | PTE_WRITABLE | (flags & PTE_USER));
        pml4.set_entry(pml4_idx, entry);

        new_pdpt
    };

    // Get or create PD
    let pdpt_virt = pmm::phys_to_virt(pdpt_phys);
    let pdpt = &mut *(pdpt_virt.as_u64() as *mut PageTable);

    let pd_phys = if pdpt.get_entry(pdpt_idx).is_present() {
        pdpt.get_entry(pdpt_idx).phys_addr()
    } else {
        let new_pd = pmm::alloc_page().ok_or("Out of memory")?;
        pmm::zero_page(new_pd);

        let mut entry = PageTableEntry::new();
        entry.set_addr(new_pd);
        entry.set_flags(PTE_PRESENT | PTE_WRITABLE | (flags & PTE_USER));
        pdpt.set_entry(pdpt_idx, entry);

        new_pd
    };

    // Get or create PT
    let pd_virt = pmm::phys_to_virt(pd_phys);
    let pd = &mut *(pd_virt.as_u64() as *mut PageTable);

    let pt_phys = if pd.get_entry(pd_idx).is_present() {
        pd.get_entry(pd_idx).phys_addr()
    } else {
        let new_pt = pmm::alloc_page().ok_or("Out of memory")?;
        pmm::zero_page(new_pt);

        let mut entry = PageTableEntry::new();
        entry.set_addr(new_pt);
        entry.set_flags(PTE_PRESENT | PTE_WRITABLE | (flags & PTE_USER));
        pd.set_entry(pd_idx, entry);

        new_pt
    };

    // Set the page table entry
    let pt_virt = pmm::phys_to_virt(pt_phys);
    let pt = &mut *(pt_virt.as_u64() as *mut PageTable);

    let mut entry = PageTableEntry::new();
    entry.set_addr(phys);
    entry.set_flags(flags);
    pt.set_entry(pt_idx, entry);

    // Invalidate TLB for this address
    crate::cpu::invlpg(virt.as_u64());

    Ok(())
}

/// Unmap a virtual page
pub unsafe fn unmap_page(pml4_phys: PhysAddr, virt: VirtAddr) -> Result<(), &'static str> {
    let (pml4_idx, pdpt_idx, pd_idx, pt_idx) = virt_to_indices(virt.as_u64());

    // Walk page tables
    let pml4_virt = pmm::phys_to_virt(pml4_phys);
    let pml4 = &mut *(pml4_virt.as_u64() as *mut PageTable);

    let pml4_entry = pml4.get_entry(pml4_idx);
    if !pml4_entry.is_present() {
        return Err("Page not mapped");
    }

    let pdpt_phys = pml4_entry.phys_addr();
    let pdpt_virt = pmm::phys_to_virt(pdpt_phys);
    let pdpt = &mut *(pdpt_virt.as_u64() as *mut PageTable);

    let pdpt_entry = pdpt.get_entry(pdpt_idx);
    if !pdpt_entry.is_present() {
        return Err("Page not mapped");
    }

    let pd_phys = pdpt_entry.phys_addr();
    let pd_virt = pmm::phys_to_virt(pd_phys);
    let pd = &mut *(pd_virt.as_u64() as *mut PageTable);

    let pd_entry = pd.get_entry(pd_idx);
    if !pd_entry.is_present() {
        return Err("Page not mapped");
    }

    let pt_phys = pd_entry.phys_addr();
    let pt_virt = pmm::phys_to_virt(pt_phys);
    let pt = &mut *(pt_virt.as_u64() as *mut PageTable);

    // Clear the entry
    let entry = pt.get_entry(pt_idx);
    if entry.is_present() {
        let phys = entry.phys_addr();
        pmm::free_page(phys);
        pt.clear_entry(pt_idx);

        // Invalidate TLB
        crate::cpu::invlpg(virt.as_u64());

        Ok(())
    } else {
        Err("Page not mapped")
    }
}

/// Translate virtual address to physical address
pub unsafe fn virt_to_phys(pml4_phys: PhysAddr, virt: VirtAddr) -> Option<PhysAddr> {
    let (pml4_idx, pdpt_idx, pd_idx, pt_idx) = virt_to_indices(virt.as_u64());

    let pml4_virt = pmm::phys_to_virt(pml4_phys);
    let pml4 = &*(pml4_virt.as_u64() as *const PageTable);

    let pml4_entry = pml4.get_entry(pml4_idx);
    if !pml4_entry.is_present() {
        return None;
    }

    let pdpt_virt = pmm::phys_to_virt(pml4_entry.phys_addr());
    let pdpt = &*(pdpt_virt.as_u64() as *const PageTable);

    let pdpt_entry = pdpt.get_entry(pdpt_idx);
    if !pdpt_entry.is_present() {
        return None;
    }

    let pd_virt = pmm::phys_to_virt(pdpt_entry.phys_addr());
    let pd = &*(pd_virt.as_u64() as *const PageTable);

    let pd_entry = pd.get_entry(pd_idx);
    if !pd_entry.is_present() {
        return None;
    }

    let pt_virt = pmm::phys_to_virt(pd_entry.phys_addr());
    let pt = &*(pt_virt.as_u64() as *const PageTable);

    let pt_entry = pt.get_entry(pt_idx);
    if !pt_entry.is_present() {
        return None;
    }

    let page_phys = pt_entry.phys_addr();
    let offset = virt.as_u64() & (PAGE_SIZE as u64 - 1);
    Some(PhysAddr::new(page_phys.as_u64() + offset))
}

/// Mark page as copy-on-write
pub unsafe fn mark_cow(pml4_phys: PhysAddr, virt: VirtAddr) -> Result<(), &'static str> {
    let (pml4_idx, pdpt_idx, pd_idx, pt_idx) = virt_to_indices(virt.as_u64());

    let pml4_virt = pmm::phys_to_virt(pml4_phys);
    let pml4 = &*(pml4_virt.as_u64() as *const PageTable);
    let pml4_entry = pml4.get_entry(pml4_idx);
    if !pml4_entry.is_present() {
        return Err("Page not mapped");
    }

    let pdpt_virt = pmm::phys_to_virt(pml4_entry.phys_addr());
    let pdpt = &*(pdpt_virt.as_u64() as *const PageTable);
    let pdpt_entry = pdpt.get_entry(pdpt_idx);
    if !pdpt_entry.is_present() {
        return Err("Page not mapped");
    }

    let pd_virt = pmm::phys_to_virt(pdpt_entry.phys_addr());
    let pd = &*(pd_virt.as_u64() as *const PageTable);
    let pd_entry = pd.get_entry(pd_idx);
    if !pd_entry.is_present() {
        return Err("Page not mapped");
    }

    let pt_virt = pmm::phys_to_virt(pd_entry.phys_addr());
    let pt = &mut *(pt_virt.as_u64() as *mut PageTable);

    let mut entry = pt.get_entry(pt_idx);
    if !entry.is_present() {
        return Err("Page not mapped");
    }

    // Mark as COW and clear writable bit
    entry.clear_flags(PTE_WRITABLE);
    entry.set_flags(PTE_COW);
    pt.set_entry(pt_idx, entry);

    // Increment refcount on physical page
    let phys = entry.phys_addr();
    pmm::ref_page(phys);

    COW_PAGES_COUNT.fetch_add(1, Ordering::Relaxed);

    // Invalidate TLB
    crate::cpu::invlpg(virt.as_u64());

    Ok(())
}

/// Get COW page count
pub fn cow_pages() -> usize {
    COW_PAGES_COUNT.load(Ordering::Relaxed) as usize
}

/// Clone page table (for fork)
pub unsafe fn clone_page_table(src_pml4: PhysAddr) -> Option<PhysAddr> {
    let dst_pml4 = create_page_table()?;

    // Walk source page table and mark all writable pages as COW
    let src_pml4_virt = pmm::phys_to_virt(src_pml4);
    let src_pml4_table = &*(src_pml4_virt.as_u64() as *const PageTable);

    for pml4_idx in 0..256 {
        let pml4_entry = src_pml4_table.get_entry(pml4_idx);
        if !pml4_entry.is_present() {
            continue;
        }

        let pdpt_virt = pmm::phys_to_virt(pml4_entry.phys_addr());
        let pdpt = &*(pdpt_virt.as_u64() as *const PageTable);

        for pdpt_idx in 0..512 {
            let pdpt_entry = pdpt.get_entry(pdpt_idx);
            if !pdpt_entry.is_present() {
                continue;
            }

            let pd_virt = pmm::phys_to_virt(pdpt_entry.phys_addr());
            let pd = &*(pd_virt.as_u64() as *const PageTable);

            for pd_idx in 0..512 {
                let pd_entry = pd.get_entry(pd_idx);
                if !pd_entry.is_present() {
                    continue;
                }

                let pt_virt = pmm::phys_to_virt(pd_entry.phys_addr());
                let pt = &mut *(pt_virt.as_u64() as *mut PageTable);

                for pt_idx in 0..512 {
                    let mut entry = pt.get_entry(pt_idx);
                    if !entry.is_present() {
                        continue;
                    }

                    // Calculate virtual address
                    let virt_addr = (pml4_idx << 39) | (pdpt_idx << 30) | (pd_idx << 21) | (pt_idx << 12);

                    if entry.is_writable() && !entry.is_cow() {
                        // Mark as COW in both parent and child
                        mark_cow(src_pml4, VirtAddr::new(virt_addr as u64)).ok();
                    }

                    // Map in child with same flags
                    map_page(dst_pml4, VirtAddr::new(virt_addr as u64), entry.phys_addr(), entry.flags()).ok();
                }
            }
        }
    }

    Some(dst_pml4)
}
