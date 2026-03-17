/*
 * Genesis OS — Kernel Heap Allocator
 *
 * Simple bump allocator for kernel heap.
 * In production, this would be replaced with a more sophisticated
 * allocator like slab or buddy.
 */

use super::*;
use super::pmm;
use super::vmm;
use core::sync::atomic::{AtomicU64, Ordering};
use core::alloc::{GlobalAlloc, Layout};

/// Kernel heap boundaries
const HEAP_START: u64 = KERNEL_HEAP_START;
const HEAP_SIZE: u64 = KERNEL_HEAP_END - KERNEL_HEAP_START;

static HEAP_NEXT: AtomicU64 = AtomicU64::new(HEAP_START);
static HEAP_ALLOCATED: AtomicU64 = AtomicU64::new(0);

/// Initialize kernel heap allocator
pub unsafe fn init() {
    HEAP_NEXT.store(HEAP_START, Ordering::SeqCst);
    HEAP_ALLOCATED.store(0, Ordering::SeqCst);
}

/// Simple bump allocator (allocations only grow, no deallocation)
pub struct BumpAllocator;

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align();

        // Align next pointer
        let mut next = HEAP_NEXT.load(Ordering::SeqCst);
        let aligned = (next + align as u64 - 1) & !(align as u64 - 1);

        // Check if we have space
        if aligned + size as u64 > HEAP_START + HEAP_SIZE {
            return core::ptr::null_mut();
        }

        // Allocate
        let old = HEAP_NEXT.compare_exchange(
            next,
            aligned + size as u64,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );

        match old {
            Ok(_) => {
                HEAP_ALLOCATED.fetch_add(size as u64, Ordering::Relaxed);

                // Map pages if needed (lazy allocation)
                let start_page = round_down_page(aligned);
                let end_page = round_up_page(aligned + size as u64);
                let cr3 = PhysAddr::new(crate::cpu::read_cr3());

                for page_addr in (start_page..end_page).step_by(PAGE_SIZE) {
                    let virt = VirtAddr::new(page_addr);

                    // Check if already mapped
                    if vmm::virt_to_phys(cr3, virt).is_none() {
                        // Allocate and map new page
                        if let Some(phys) = pmm::alloc_page() {
                            pmm::zero_page(phys);
                            let _ = vmm::map_page(
                                cr3,
                                virt,
                                phys,
                                PTE_PRESENT | PTE_WRITABLE | PTE_NO_EXEC,
                            );
                        } else {
                            return core::ptr::null_mut();
                        }
                    }
                }

                aligned as *mut u8
            }
            Err(_) => {
                // Retry
                self.alloc(layout)
            }
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Bump allocator doesn't support deallocation
        // In a real kernel, we'd use a proper allocator here
    }
}

/// Global allocator instance
#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

/// Get heap statistics
pub struct HeapStats {
    pub total_size: u64,
    pub allocated: u64,
    pub free: u64,
}

pub fn stats() -> HeapStats {
    let next = HEAP_NEXT.load(Ordering::Relaxed);
    let allocated = HEAP_ALLOCATED.load(Ordering::Relaxed);

    HeapStats {
        total_size: HEAP_SIZE,
        allocated,
        free: HEAP_SIZE.saturating_sub(next - HEAP_START),
    }
}
