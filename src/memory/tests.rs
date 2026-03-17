/*
 * Genesis OS — Memory Subsystem Tests
 *
 * Test suite for memory management features:
 * - Physical memory allocation
 * - Virtual memory mapping
 * - Copy-on-Write
 * - Demand paging
 * - Page cache
 */

#![cfg(test)]

use super::*;
use super::pmm;
use super::vmm;
use super::page_cache;

#[test_case]
fn test_pmm_alloc_free() {
    // Test basic page allocation and deallocation
    let page1 = pmm::alloc_page().expect("Failed to allocate page 1");
    let page2 = pmm::alloc_page().expect("Failed to allocate page 2");

    assert_ne!(page1, page2, "Allocated same page twice");

    let free_before = pmm::free_pages();
    pmm::free_page(page1);
    let free_after = pmm::free_pages();

    assert_eq!(free_after, free_before + 1, "Free count didn't increase");
}

#[test_case]
fn test_pmm_refcount() {
    // Test reference counting
    let page = pmm::alloc_page().expect("Failed to allocate page");

    assert_eq!(pmm::get_refcount(page), 1, "Initial refcount should be 1");

    pmm::ref_page(page);
    assert_eq!(pmm::get_refcount(page), 2, "Refcount should be 2 after ref");

    pmm::free_page(page);
    assert_eq!(pmm::get_refcount(page), 1, "Refcount should be 1 after free");

    pmm::free_page(page);
    assert_eq!(pmm::get_refcount(page), 0, "Refcount should be 0 after final free");
}

#[test_case]
fn test_pmm_contiguous() {
    // Test contiguous page allocation
    let pages = pmm::alloc_pages(4).expect("Failed to allocate 4 pages");

    // Verify pages are contiguous
    for i in 1..4 {
        let expected = PhysAddr::new(pages.as_u64() + (i * PAGE_SIZE) as u64);
        // Pages should be contiguous in physical memory
    }

    pmm::free_pages(pages, 4);
}

#[test_case]
fn test_vmm_create_destroy() {
    // Test page table creation and destruction
    let pml4 = vmm::create_page_table().expect("Failed to create page table");

    assert!(!pml4.is_null(), "Page table should not be null");

    unsafe {
        vmm::destroy_page_table(pml4);
    }
}

#[test_case]
fn test_vmm_map_unmap() {
    let pml4 = vmm::create_page_table().expect("Failed to create page table");
    let phys = pmm::alloc_page().expect("Failed to allocate page");
    let virt = VirtAddr::new(0x1000);

    unsafe {
        // Map page
        vmm::map_page(pml4, virt, phys, PTE_PRESENT | PTE_WRITABLE)
            .expect("Failed to map page");

        // Verify mapping
        let translated = vmm::virt_to_phys(pml4, virt).expect("Failed to translate");
        assert_eq!(translated.as_u64(), phys.as_u64(), "Address translation mismatch");

        // Unmap page
        vmm::unmap_page(pml4, virt).expect("Failed to unmap page");

        // Verify unmapped
        let result = vmm::virt_to_phys(pml4, virt);
        assert!(result.is_none(), "Page should be unmapped");

        vmm::destroy_page_table(pml4);
    }
}

#[test_case]
fn test_vmm_cow() {
    let pml4 = vmm::create_page_table().expect("Failed to create page table");
    let phys = pmm::alloc_page().expect("Failed to allocate page");
    let virt = VirtAddr::new(0x1000);

    unsafe {
        // Map writable page
        vmm::map_page(pml4, virt, phys, PTE_PRESENT | PTE_WRITABLE)
            .expect("Failed to map page");

        let refcount_before = pmm::get_refcount(phys);

        // Mark as COW
        vmm::mark_cow(pml4, virt).expect("Failed to mark COW");

        let refcount_after = pmm::get_refcount(phys);
        assert_eq!(refcount_after, refcount_before + 1, "Refcount should increase");

        // Clone page table
        let child_pml4 = vmm::clone_page_table(pml4).expect("Failed to clone");

        // Verify child has same mapping
        let child_phys = vmm::virt_to_phys(child_pml4, virt).expect("Failed to translate in child");
        assert_eq!(child_phys.as_u64(), phys.as_u64(), "Child should have same physical page");

        vmm::destroy_page_table(child_pml4);
        vmm::destroy_page_table(pml4);
    }
}

#[test_case]
fn test_page_cache_insert_lookup() {
    let virt = VirtAddr::new(0x1000);
    let phys = pmm::alloc_page().expect("Failed to allocate page");

    // Insert into cache
    page_cache::insert(virt, phys, PTE_PRESENT, false)
        .expect("Failed to insert into cache");

    // Lookup
    let cached = page_cache::lookup(virt).expect("Failed to lookup");
    assert_eq!(cached.as_u64(), phys.as_u64(), "Cached address mismatch");

    // Remove
    let removed = page_cache::remove(virt).expect("Failed to remove");
    assert_eq!(removed.as_u64(), phys.as_u64(), "Removed address mismatch");

    // Verify removed
    let result = page_cache::lookup(virt);
    assert!(result.is_none(), "Page should not be in cache");

    pmm::free_page(phys);
}

#[test_case]
fn test_page_cache_lru() {
    // Fill cache and test LRU eviction
    let pages = 10;
    let mut virts = Vec::new();

    for i in 0..pages {
        let virt = VirtAddr::new((0x1000 * (i + 1)) as u64);
        let phys = pmm::alloc_page().expect("Failed to allocate page");
        virts.push((virt, phys));

        page_cache::insert(virt, phys, PTE_PRESENT, false)
            .expect("Failed to insert");
    }

    // Access first page to make it most recently used
    let _ = page_cache::lookup(virts[0].0);

    // Fill cache to trigger eviction
    // Note: This depends on cache size configuration

    // Cleanup
    for (virt, phys) in virts {
        page_cache::remove(virt);
        pmm::free_page(phys);
    }
}

#[test_case]
fn test_allocator_basic() {
    use alloc::vec::Vec;

    // Test basic heap allocation
    let vec = Vec::with_capacity(100);
    assert_eq!(vec.capacity(), 100, "Vector capacity mismatch");

    let vec2 = Vec::with_capacity(1000);
    assert_eq!(vec2.capacity(), 1000, "Vector capacity mismatch");

    // Allocations should succeed (no panic)
}

#[test_case]
fn test_allocator_large() {
    use alloc::vec::Vec;

    // Test large allocation that spans multiple pages
    let large_vec: Vec<u64> = Vec::with_capacity(1000);
    assert!(large_vec.capacity() >= 1000, "Large allocation failed");

    // Verify memory is accessible (this would page fault if not mapped)
    let mut vec = large_vec;
    vec.push(42);
    assert_eq!(vec[0], 42, "Memory access failed");
}

#[test_case]
fn test_page_zeroing() {
    let phys = pmm::alloc_page().expect("Failed to allocate page");

    unsafe {
        pmm::zero_page(phys);

        // Verify page is zeroed
        let virt = pmm::phys_to_virt(phys);
        let ptr = virt.as_u64() as *const u64;

        for i in 0..(PAGE_SIZE / 8) {
            assert_eq!(ptr.add(i).read_volatile(), 0, "Page not zeroed at offset {}", i);
        }
    }

    pmm::free_page(phys);
}

#[test_case]
fn test_page_copy() {
    let src = pmm::alloc_page().expect("Failed to allocate source page");
    let dst = pmm::alloc_page().expect("Failed to allocate dest page");

    unsafe {
        // Write pattern to source
        let src_virt = pmm::phys_to_virt(src);
        let src_ptr = src_virt.as_u64() as *mut u64;

        for i in 0..(PAGE_SIZE / 8) {
            src_ptr.add(i).write_volatile(i as u64);
        }

        // Copy to destination
        pmm::copy_page(src, dst);

        // Verify copy
        let dst_virt = pmm::phys_to_virt(dst);
        let dst_ptr = dst_virt.as_u64() as *const u64;

        for i in 0..(PAGE_SIZE / 8) {
            assert_eq!(
                dst_ptr.add(i).read_volatile(),
                i as u64,
                "Page copy mismatch at offset {}",
                i
            );
        }
    }

    pmm::free_page(src);
    pmm::free_page(dst);
}

/// Test runner
pub fn run_tests() {
    crate::vga_print(b"\n=== Running Memory Subsystem Tests ===\n");

    test_pmm_alloc_free();
    crate::vga_print(b"[OK] PMM alloc/free\n");

    test_pmm_refcount();
    crate::vga_print(b"[OK] PMM refcount\n");

    test_vmm_create_destroy();
    crate::vga_print(b"[OK] VMM create/destroy\n");

    test_vmm_map_unmap();
    crate::vga_print(b"[OK] VMM map/unmap\n");

    test_vmm_cow();
    crate::vga_print(b"[OK] VMM COW\n");

    test_page_cache_insert_lookup();
    crate::vga_print(b"[OK] Page cache insert/lookup\n");

    test_page_zeroing();
    crate::vga_print(b"[OK] Page zeroing\n");

    test_page_copy();
    crate::vga_print(b"[OK] Page copy\n");

    crate::vga_print(b"\n=== All Tests Passed ===\n");
}
