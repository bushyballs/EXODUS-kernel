/*
 * Genesis OS — Page Fault Handler
 *
 * Handles page faults with support for:
 * - Copy-on-Write (COW)
 * - Demand paging (lazy allocation)
 * - Swap-in from page cache
 * - Stack growth
 * - Error handling (segfaults)
 */

use super::*;
use super::pmm;
use super::vmm;
use super::page_cache;

/// Page fault error code bits
const PF_PRESENT: u64 = 1 << 0;  // Page was present
const PF_WRITE: u64 = 1 << 1;    // Write access
const PF_USER: u64 = 1 << 2;     // User mode access
const PF_RESERVED: u64 = 1 << 3; // Reserved bits set
const PF_INSTR: u64 = 1 << 4;    // Instruction fetch

/// Page fault handler (called from interrupt handler)
///
/// Returns true if fault was handled, false if segfault
pub unsafe fn handle_page_fault(fault_addr: u64, error_code: u64) -> bool {
    let virt = VirtAddr::new(fault_addr);
    let cr3 = PhysAddr::new(crate::cpu::read_cr3());

    // Check if it's a COW fault
    if error_code & PF_WRITE != 0 && error_code & PF_PRESENT != 0 {
        if handle_cow_fault(cr3, virt).is_ok() {
            return true;
        }
    }

    // Check if it's a demand paging fault (page not present)
    if error_code & PF_PRESENT == 0 {
        if handle_demand_paging(cr3, virt, error_code).is_ok() {
            return true;
        }
    }

    // Check if it's stack growth
    if error_code & PF_USER != 0 && is_stack_growth(virt) {
        if handle_stack_growth(cr3, virt).is_ok() {
            return true;
        }
    }

    // Unhandled fault - this is a segfault
    false
}

/// Handle Copy-on-Write fault
///
/// When a write occurs to a COW page:
/// 1. Check if refcount is 1 (we're the only owner) - just make it writable
/// 2. Otherwise, allocate new page, copy contents, update mapping
fn handle_cow_fault(pml4_phys: PhysAddr, virt: VirtAddr) -> Result<(), &'static str> {
    let (pml4_idx, pdpt_idx, pd_idx, pt_idx) = virt_to_indices(virt.as_u64());

    // Walk page tables to find the entry
    let pml4_virt = pmm::phys_to_virt(pml4_phys);
    let pml4 = unsafe { &*(pml4_virt.as_u64() as *const vmm::PageTable) };
    let pml4_entry = pml4.get_entry(pml4_idx);
    if !pml4_entry.is_present() {
        return Err("Page not mapped");
    }

    let pdpt_virt = pmm::phys_to_virt(pml4_entry.phys_addr());
    let pdpt = unsafe { &*(pdpt_virt.as_u64() as *const vmm::PageTable) };
    let pdpt_entry = pdpt.get_entry(pdpt_idx);
    if !pdpt_entry.is_present() {
        return Err("Page not mapped");
    }

    let pd_virt = pmm::phys_to_virt(pdpt_entry.phys_addr());
    let pd = unsafe { &*(pd_virt.as_u64() as *const vmm::PageTable) };
    let pd_entry = pd.get_entry(pd_idx);
    if !pd_entry.is_present() {
        return Err("Page not mapped");
    }

    let pt_virt = pmm::phys_to_virt(pd_entry.phys_addr());
    let pt = unsafe { &mut *(pt_virt.as_u64() as *mut vmm::PageTable) };

    let mut entry = pt.get_entry(pt_idx);
    if !entry.is_present() {
        return Err("Page not mapped");
    }

    if !entry.is_cow() {
        return Err("Not a COW page");
    }

    let old_phys = entry.phys_addr();
    let refcount = pmm::get_refcount(old_phys);

    if refcount == 1 {
        // We're the only owner - just make it writable
        entry.set_flags(PTE_WRITABLE);
        entry.clear_flags(PTE_COW);
        pt.set_entry(pt_idx, entry);
    } else {
        // Need to copy the page
        let new_phys = pmm::alloc_page().ok_or("Out of memory")?;

        // Copy page contents
        unsafe { pmm::copy_page(old_phys, new_phys); }

        // Update page table entry
        entry.set_addr(new_phys);
        entry.set_flags(PTE_WRITABLE);
        entry.clear_flags(PTE_COW);
        pt.set_entry(pt_idx, entry);

        // Decrement refcount on old page
        pmm::free_page(old_phys);
    }

    // Invalidate TLB for this page
    unsafe { crate::cpu::invlpg(round_down_page(virt.as_u64())); }

    Ok(())
}

/// Handle demand paging fault
///
/// Allocates a page on first access (lazy allocation).
/// Also handles swap-in from page cache if needed.
fn handle_demand_paging(
    pml4_phys: PhysAddr,
    virt: VirtAddr,
    error_code: u64,
) -> Result<(), &'static str> {
    // Check if this is a valid virtual address for demand paging
    if !is_valid_demand_page(virt) {
        return Err("Invalid address for demand paging");
    }

    // Check if page is in page cache (swapped out)
    if let Some(cached_phys) = page_cache::lookup(virt) {
        // Swap in from cache
        let flags = if error_code & PF_USER != 0 {
            PTE_PRESENT | PTE_WRITABLE | PTE_USER
        } else {
            PTE_PRESENT | PTE_WRITABLE
        };

        unsafe { vmm::map_page(pml4_phys, virt, cached_phys, flags)?; }
        page_cache::remove(virt);

        return Ok(());
    }

    // Allocate new zero page
    let new_phys = pmm::alloc_page().ok_or("Out of memory")?;
    unsafe { pmm::zero_page(new_phys); }

    // Determine flags based on access type
    let mut flags = PTE_PRESENT | PTE_WRITABLE;

    if error_code & PF_USER != 0 {
        flags |= PTE_USER;
    }

    // Map the page
    unsafe { vmm::map_page(pml4_phys, virt, new_phys, flags)?; }

    Ok(())
}

/// Handle stack growth
fn handle_stack_growth(pml4_phys: PhysAddr, virt: VirtAddr) -> Result<(), &'static str> {
    // Get current task's stack limits
    // For now, allow growth up to 8MB below stack top
    const MAX_STACK_SIZE: u64 = 8 * 1024 * 1024;

    unsafe {
        let cpu = crate::percpu::current_cpu();
        let stack_top = (*cpu).user_stack;

        if stack_top == 0 {
            return Err("No user stack");
        }

        let stack_bottom = stack_top - MAX_STACK_SIZE;

        if virt.as_u64() < stack_bottom || virt.as_u64() >= stack_top {
            return Err("Stack overflow");
        }
    }

    // Allocate page for stack
    let new_phys = pmm::alloc_page().ok_or("Out of memory")?;
    unsafe { pmm::zero_page(new_phys); }

    let flags = PTE_PRESENT | PTE_WRITABLE | PTE_USER;
    unsafe { vmm::map_page(pml4_phys, virt, new_phys, flags)?; }

    Ok(())
}

/// Check if address is valid for demand paging
fn is_valid_demand_page(virt: VirtAddr) -> bool {
    // User space heap region
    const USER_HEAP_START: u64 = 0x0000_0001_0000_0000;
    const USER_HEAP_END: u64 = 0x0000_7000_0000_0000;

    let addr = virt.as_u64();
    if virt.is_user() {
        addr >= USER_HEAP_START && addr < USER_HEAP_END
    } else {
        // Kernel heap
        addr >= KERNEL_HEAP_START && addr < KERNEL_HEAP_END
    }
}

/// Check if fault is due to stack growth
fn is_stack_growth(virt: VirtAddr) -> bool {
    if !virt.is_user() {
        return false;
    }

    // Check if address is within reasonable distance of current stack pointer
    unsafe {
        let cpu = crate::percpu::current_cpu();
        let stack_top = (*cpu).user_stack;

        if stack_top == 0 {
            return false;
        }

        const MAX_STACK_SIZE: u64 = 8 * 1024 * 1024;
        let stack_bottom = stack_top.saturating_sub(MAX_STACK_SIZE);

        virt.as_u64() >= stack_bottom && virt.as_u64() < stack_top
    }
}

/// Page fault statistics
pub struct PageFaultStats {
    pub total_faults: u64,
    pub cow_faults: u64,
    pub demand_faults: u64,
    pub stack_faults: u64,
    pub segfaults: u64,
}

use core::sync::atomic::AtomicU64;

static TOTAL_FAULTS: AtomicU64 = AtomicU64::new(0);
static COW_FAULTS: AtomicU64 = AtomicU64::new(0);
static DEMAND_FAULTS: AtomicU64 = AtomicU64::new(0);
static STACK_FAULTS: AtomicU64 = AtomicU64::new(0);
static SEGFAULTS: AtomicU64 = AtomicU64::new(0);

/// Record page fault statistics
pub fn record_fault(fault_type: &str) {
    use core::sync::atomic::Ordering;

    TOTAL_FAULTS.fetch_add(1, Ordering::Relaxed);

    match fault_type {
        "cow" => { COW_FAULTS.fetch_add(1, Ordering::Relaxed); }
        "demand" => { DEMAND_FAULTS.fetch_add(1, Ordering::Relaxed); }
        "stack" => { STACK_FAULTS.fetch_add(1, Ordering::Relaxed); }
        "segfault" => { SEGFAULTS.fetch_add(1, Ordering::Relaxed); }
        _ => {}
    }
}

/// Get page fault statistics
pub fn stats() -> PageFaultStats {
    use core::sync::atomic::Ordering;

    PageFaultStats {
        total_faults: TOTAL_FAULTS.load(Ordering::Relaxed),
        cow_faults: COW_FAULTS.load(Ordering::Relaxed),
        demand_faults: DEMAND_FAULTS.load(Ordering::Relaxed),
        stack_faults: STACK_FAULTS.load(Ordering::Relaxed),
        segfaults: SEGFAULTS.load(Ordering::Relaxed),
    }
}

/// Enhanced page fault handler with statistics
pub unsafe fn handle_page_fault_with_stats(fault_addr: u64, error_code: u64) -> bool {
    let virt = VirtAddr::new(fault_addr);
    let cr3 = PhysAddr::new(crate::cpu::read_cr3());

    // Check if it's a COW fault
    if error_code & PF_WRITE != 0 && error_code & PF_PRESENT != 0 {
        if handle_cow_fault(cr3, virt).is_ok() {
            record_fault("cow");
            return true;
        }
    }

    // Check if it's a demand paging fault
    if error_code & PF_PRESENT == 0 {
        if handle_demand_paging(cr3, virt, error_code).is_ok() {
            record_fault("demand");
            return true;
        }
    }

    // Check if it's stack growth
    if error_code & PF_USER != 0 && is_stack_growth(virt) {
        if handle_stack_growth(cr3, virt).is_ok() {
            record_fault("stack");
            return true;
        }
    }

    // Segfault
    record_fault("segfault");
    false
}
