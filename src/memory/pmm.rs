/*
 * Genesis OS — Physical Memory Manager (PMM)
 *
 * Manages physical RAM using a bitmap allocator.
 * Supports page allocation, deallocation, and reference counting.
 */

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use super::{PAGE_SIZE, PhysAddr};

/// Maximum physical memory: 128 GB
const MAX_PHYS_MEM: usize = 128 * 1024 * 1024 * 1024;
const MAX_PAGES: usize = MAX_PHYS_MEM / PAGE_SIZE;
const BITMAP_SIZE: usize = MAX_PAGES / 8; // 1 bit per page

/// Physical memory bitmap (1 = allocated, 0 = free)
static mut PHYS_BITMAP: [u8; BITMAP_SIZE] = [0; BITMAP_SIZE];

/// Reference count for each physical page (for COW support)
const ATOMIC_INIT: AtomicU32 = AtomicU32::new(0);
static mut PAGE_REFCOUNT: [AtomicU32; MAX_PAGES] = [ATOMIC_INIT; MAX_PAGES];

static FREE_PAGES: AtomicU64 = AtomicU64::new(0);
static USED_PAGES: AtomicU64 = AtomicU64::new(0);
static TOTAL_PAGES: AtomicU64 = AtomicU64::new(0);

/// Initialize physical memory manager
///
/// Scans memory map from bootloader and marks available regions
pub unsafe fn init() -> u64 {
    // For now, assume we have 512MB of RAM starting at 1MB
    // In a real implementation, this would parse the multiboot memory map
    let start_addr = 0x10_0000;  // 1MB
    let end_addr = 0x2000_0000;  // 512MB

    let total_mem = end_addr - start_addr;
    let total_pages = total_mem / PAGE_SIZE as u64;

    TOTAL_PAGES.store(total_pages, Ordering::SeqCst);
    FREE_PAGES.store(total_pages, Ordering::SeqCst);

    // Mark all pages as free initially
    for i in 0..BITMAP_SIZE {
        PHYS_BITMAP[i] = 0;
    }

    // Mark kernel pages as used (first 1MB + kernel image)
    // Reserve first 256 pages (1MB) for kernel
    for i in 0..256 {
        set_page_used(i);
    }

    total_mem
}

/// Allocate a single physical page
pub fn alloc_page() -> Option<PhysAddr> {
    unsafe {
        // Find first free page
        for byte_idx in 0..BITMAP_SIZE {
            let byte = PHYS_BITMAP[byte_idx];
            if byte != 0xFF {
                // This byte has at least one free bit
                for bit_idx in 0..8 {
                    if byte & (1 << bit_idx) == 0 {
                        // Found free page
                        let page_idx = byte_idx * 8 + bit_idx;
                        if page_idx >= MAX_PAGES {
                            return None;
                        }

                        // Mark as used
                        PHYS_BITMAP[byte_idx] |= 1 << bit_idx;
                        PAGE_REFCOUNT[page_idx].store(1, Ordering::SeqCst);

                        FREE_PAGES.fetch_sub(1, Ordering::SeqCst);
                        USED_PAGES.fetch_add(1, Ordering::SeqCst);

                        let phys_addr = (page_idx * PAGE_SIZE) as u64;
                        return Some(PhysAddr::new(phys_addr));
                    }
                }
            }
        }
    }

    None // Out of memory
}

/// Allocate contiguous physical pages
pub fn alloc_pages(count: usize) -> Option<PhysAddr> {
    if count == 0 {
        return None;
    }

    if count == 1 {
        return alloc_page();
    }

    unsafe {
        // Find contiguous run of free pages
        'outer: for start_byte in 0..BITMAP_SIZE {
            let start_page = start_byte * 8;

            // Check if we have enough room
            if start_page + count > MAX_PAGES {
                break;
            }

            // Check if all pages in range are free
            let mut all_free = true;
            for offset in 0..count {
                let page_idx = start_page + offset;
                let byte_idx = page_idx / 8;
                let bit_idx = page_idx % 8;

                if PHYS_BITMAP[byte_idx] & (1 << bit_idx) != 0 {
                    all_free = false;
                    break;
                }
            }

            if !all_free {
                continue 'outer;
            }

            // Allocate all pages in range
            for offset in 0..count {
                let page_idx = start_page + offset;
                let byte_idx = page_idx / 8;
                let bit_idx = page_idx % 8;

                PHYS_BITMAP[byte_idx] |= 1 << bit_idx;
                PAGE_REFCOUNT[page_idx].store(1, Ordering::SeqCst);
            }

            FREE_PAGES.fetch_sub(count as u64, Ordering::SeqCst);
            USED_PAGES.fetch_add(count as u64, Ordering::SeqCst);

            let phys_addr = (start_page * PAGE_SIZE) as u64;
            return Some(PhysAddr::new(phys_addr));
        }
    }

    None // Could not find contiguous pages
}

/// Free a physical page
pub fn free_page(phys: PhysAddr) {
    let page_idx = (phys.as_u64() as usize) / PAGE_SIZE;

    if page_idx >= MAX_PAGES {
        return;
    }

    unsafe {
        // Decrement reference count
        let old_refcount = PAGE_REFCOUNT[page_idx].fetch_sub(1, Ordering::SeqCst);

        if old_refcount == 1 {
            // Last reference, actually free the page
            let byte_idx = page_idx / 8;
            let bit_idx = page_idx % 8;

            PHYS_BITMAP[byte_idx] &= !(1 << bit_idx);

            FREE_PAGES.fetch_add(1, Ordering::SeqCst);
            USED_PAGES.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Free contiguous physical pages
pub fn free_pages(phys: PhysAddr, count: usize) {
    for i in 0..count {
        let offset = (i * PAGE_SIZE) as u64;
        free_page(PhysAddr::new(phys.as_u64() + offset));
    }
}

/// Increment reference count for a page (for COW)
pub fn ref_page(phys: PhysAddr) {
    let page_idx = (phys.as_u64() as usize) / PAGE_SIZE;

    if page_idx >= MAX_PAGES {
        return;
    }

    unsafe {
        PAGE_REFCOUNT[page_idx].fetch_add(1, Ordering::SeqCst);
    }
}

/// Get reference count for a page
pub fn get_refcount(phys: PhysAddr) -> u32 {
    let page_idx = (phys.as_u64() as usize) / PAGE_SIZE;

    if page_idx >= MAX_PAGES {
        return 0;
    }

    unsafe { PAGE_REFCOUNT[page_idx].load(Ordering::SeqCst) }
}

/// Mark a page as used (during init)
unsafe fn set_page_used(page_idx: usize) {
    if page_idx >= MAX_PAGES {
        return;
    }

    let byte_idx = page_idx / 8;
    let bit_idx = page_idx % 8;

    PHYS_BITMAP[byte_idx] |= 1 << bit_idx;
    PAGE_REFCOUNT[page_idx].store(1, Ordering::SeqCst);

    FREE_PAGES.fetch_sub(1, Ordering::SeqCst);
    USED_PAGES.fetch_add(1, Ordering::SeqCst);
}

/// Get number of free pages
pub fn available_pages() -> usize {
    FREE_PAGES.load(Ordering::Relaxed) as usize
}

/// Get number of used pages
pub fn used_pages() -> usize {
    USED_PAGES.load(Ordering::Relaxed) as usize
}

/// Get total number of pages
pub fn total_pages() -> usize {
    TOTAL_PAGES.load(Ordering::Relaxed) as usize
}

/// Check if a physical address is valid
pub fn is_valid_phys(phys: PhysAddr) -> bool {
    let page_idx = (phys.as_u64() as usize) / PAGE_SIZE;
    page_idx < MAX_PAGES
}

/// Zero out a physical page
pub unsafe fn zero_page(phys: PhysAddr) {
    let virt = phys_to_virt(phys);
    let ptr = virt.as_u64() as *mut u64;

    for i in 0..(PAGE_SIZE / 8) {
        ptr.add(i).write_volatile(0);
    }
}

/// Copy physical page
pub unsafe fn copy_page(src: PhysAddr, dst: PhysAddr) {
    let src_virt = phys_to_virt(src);
    let dst_virt = phys_to_virt(dst);

    let src_ptr = src_virt.as_u64() as *const u64;
    let dst_ptr = dst_virt.as_u64() as *mut u64;

    for i in 0..(PAGE_SIZE / 8) {
        let val = src_ptr.add(i).read_volatile();
        dst_ptr.add(i).write_volatile(val);
    }
}

/// Convert physical address to virtual (identity map in kernel space)
#[inline(always)]
pub fn phys_to_virt(phys: PhysAddr) -> super::VirtAddr {
    super::VirtAddr::new(phys.as_u64() + super::KERNEL_VIRT_BASE)
}

/// Convert virtual address to physical (kernel space only)
#[inline(always)]
pub fn virt_to_phys(virt: super::VirtAddr) -> PhysAddr {
    if virt.is_kernel() {
        PhysAddr::new(virt.as_u64() - super::KERNEL_VIRT_BASE)
    } else {
        PhysAddr::zero()
    }
}
