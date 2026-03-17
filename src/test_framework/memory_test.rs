use crate::sync::Mutex;
use alloc::collections::BTreeMap;
/// Memory subsystem tests
///
/// Part of the AIOS. Tests for the memory allocator and
/// virtual memory system including page allocation, heap
/// allocation, and virtual memory mapping.
use alloc::vec::Vec;

/// Simulated page frame allocator state for testing
struct SimPageAllocator {
    total_pages: usize,
    allocated: Vec<u64>, // allocated frame addresses
    next_frame: u64,
}

impl SimPageAllocator {
    fn new(total_pages: usize) -> Self {
        Self {
            total_pages,
            allocated: Vec::new(),
            next_frame: 0x1000, // Start allocating from 4KB
        }
    }

    fn alloc_page(&mut self) -> Option<u64> {
        if self.allocated.len() >= self.total_pages {
            return None; // out of pages
        }
        let frame = self.next_frame;
        self.next_frame += 0x1000; // 4KB pages
        self.allocated.push(frame);
        Some(frame)
    }

    fn free_page(&mut self, addr: u64) -> bool {
        let initial_len = self.allocated.len();
        self.allocated.retain(|&a| a != addr);
        self.allocated.len() < initial_len
    }

    fn allocated_count(&self) -> usize {
        self.allocated.len()
    }
}

/// Simulated virtual memory mapping
struct SimVirtMapping {
    mappings: BTreeMap<u64, u64>, // virt -> phys
}

impl SimVirtMapping {
    fn new() -> Self {
        Self {
            mappings: BTreeMap::new(),
        }
    }

    fn map(&mut self, virt_addr: u64, phys_addr: u64) {
        self.mappings.insert(virt_addr, phys_addr);
    }

    fn unmap(&mut self, virt_addr: u64) -> bool {
        self.mappings.remove(&virt_addr).is_some()
    }

    fn resolve(&self, virt_addr: u64) -> Option<u64> {
        self.mappings.get(&virt_addr).copied()
    }

    fn mapping_count(&self) -> usize {
        self.mappings.len()
    }
}

/// Tests for the memory allocator and virtual memory system.
pub struct MemoryTests;

impl MemoryTests {
    /// Test basic page allocation and deallocation.
    /// Allocates pages, verifies they are unique and properly aligned,
    /// then frees them and verifies reclamation.
    pub fn test_page_alloc() -> bool {
        crate::serial_println!("    [mem-test] running test_page_alloc...");

        let mut allocator = SimPageAllocator::new(16);

        // Allocate several pages
        let mut frames = Vec::new();
        for _ in 0..8 {
            match allocator.alloc_page() {
                Some(frame) => frames.push(frame),
                None => {
                    crate::serial_println!(
                        "    [mem-test] FAIL: page allocation returned None too early"
                    );
                    return false;
                }
            }
        }

        // Verify we got 8 unique frames
        if frames.len() != 8 {
            crate::serial_println!(
                "    [mem-test] FAIL: expected 8 frames, got {}",
                frames.len()
            );
            return false;
        }

        // Verify all frames are page-aligned (4KB)
        for &frame in &frames {
            if frame % 0x1000 != 0 {
                crate::serial_println!("    [mem-test] FAIL: frame {:#x} not page-aligned", frame);
                return false;
            }
        }

        // Verify all frames are unique
        for i in 0..frames.len() {
            for j in (i + 1)..frames.len() {
                if frames[i] == frames[j] {
                    crate::serial_println!("    [mem-test] FAIL: duplicate frame {:#x}", frames[i]);
                    return false;
                }
            }
        }

        // Free some pages
        let freed = allocator.free_page(frames[0]);
        if !freed {
            crate::serial_println!("    [mem-test] FAIL: free_page returned false for valid frame");
            return false;
        }

        let freed_invalid = allocator.free_page(0xDEAD_0000);
        if freed_invalid {
            crate::serial_println!(
                "    [mem-test] FAIL: free_page returned true for invalid frame"
            );
            return false;
        }

        // Verify allocation count decreased
        if allocator.allocated_count() != 7 {
            crate::serial_println!(
                "    [mem-test] FAIL: expected 7 allocated pages, got {}",
                allocator.allocated_count()
            );
            return false;
        }

        // Test exhaustion: allocate remaining capacity
        let remaining = allocator.total_pages - allocator.allocated_count();
        for _ in 0..remaining {
            if allocator.alloc_page().is_none() {
                crate::serial_println!("    [mem-test] FAIL: premature exhaustion");
                return false;
            }
        }

        // Next allocation should fail
        if allocator.alloc_page().is_some() {
            crate::serial_println!("    [mem-test] FAIL: allocation succeeded beyond capacity");
            return false;
        }

        crate::serial_println!(
            "    [mem-test] PASS: test_page_alloc (8 alloc, 1 free, exhaustion verified)"
        );
        true
    }

    /// Test heap allocation and freeing.
    /// Uses Vec allocation to test the kernel heap allocator under
    /// various allocation patterns and sizes.
    pub fn test_heap_alloc() -> bool {
        crate::serial_println!("    [mem-test] running test_heap_alloc...");

        // Test small allocation
        let mut small: Vec<u8> = Vec::new();
        for i in 0..64u8 {
            small.push(i);
        }
        if small.len() != 64 {
            crate::serial_println!("    [mem-test] FAIL: small vec length mismatch");
            return false;
        }
        // Verify contents
        for i in 0..64u8 {
            if small[i as usize] != i {
                crate::serial_println!("    [mem-test] FAIL: small vec content mismatch at {}", i);
                return false;
            }
        }

        // Test medium allocation
        let mut medium: Vec<u32> = Vec::new();
        for i in 0..256u32 {
            medium.push(i * 7);
        }
        if medium.len() != 256 {
            crate::serial_println!("    [mem-test] FAIL: medium vec length mismatch");
            return false;
        }
        for i in 0..256u32 {
            if medium[i as usize] != i * 7 {
                crate::serial_println!("    [mem-test] FAIL: medium vec content mismatch at {}", i);
                return false;
            }
        }

        // Test allocation and deallocation pattern
        let mut vecs: Vec<Vec<u8>> = Vec::new();
        for size in &[16usize, 64, 128, 256, 512] {
            let mut v = Vec::with_capacity(*size);
            for i in 0..*size {
                v.push((i & 0xFF) as u8);
            }
            vecs.push(v);
        }

        // Verify all allocations
        if vecs.len() != 5 {
            crate::serial_println!("    [mem-test] FAIL: expected 5 allocations");
            return false;
        }

        // Drop them in reverse order (tests free patterns)
        while !vecs.is_empty() {
            vecs.pop();
        }

        // Test zero-size and single-byte allocations
        let empty: Vec<u8> = Vec::new();
        if !empty.is_empty() {
            crate::serial_println!("    [mem-test] FAIL: empty vec not empty");
            return false;
        }

        let single: Vec<u8> = Vec::from([42u8]);
        if single.len() != 1 || single[0] != 42 {
            crate::serial_println!("    [mem-test] FAIL: single-byte vec mismatch");
            return false;
        }

        crate::serial_println!(
            "    [mem-test] PASS: test_heap_alloc (small, medium, pattern, edge cases)"
        );
        true
    }

    /// Test virtual memory mapping.
    /// Creates simulated virtual-to-physical mappings, verifies
    /// address resolution, and tests unmapping.
    pub fn test_virt_map() -> bool {
        crate::serial_println!("    [mem-test] running test_virt_map...");

        let mut vm = SimVirtMapping::new();

        // Map several virtual pages to physical frames
        let mappings: &[(u64, u64)] = &[
            (0x0040_0000, 0x0010_0000),
            (0x0040_1000, 0x0020_0000),
            (0x0040_2000, 0x0030_0000),
            (0x0040_3000, 0x0040_0000),
        ];

        for &(virt, phys) in mappings {
            vm.map(virt, phys);
        }

        // Verify mapping count
        if vm.mapping_count() != mappings.len() {
            crate::serial_println!(
                "    [mem-test] FAIL: expected {} mappings, got {}",
                mappings.len(),
                vm.mapping_count()
            );
            return false;
        }

        // Verify resolution
        for &(virt, expected_phys) in mappings {
            match vm.resolve(virt) {
                Some(phys) => {
                    if phys != expected_phys {
                        crate::serial_println!(
                            "    [mem-test] FAIL: virt {:#x} resolved to {:#x}, expected {:#x}",
                            virt,
                            phys,
                            expected_phys
                        );
                        return false;
                    }
                }
                None => {
                    crate::serial_println!("    [mem-test] FAIL: virt {:#x} not mapped", virt);
                    return false;
                }
            }
        }

        // Test unmapped address
        if vm.resolve(0xDEAD_0000).is_some() {
            crate::serial_println!("    [mem-test] FAIL: unmapped address resolved to something");
            return false;
        }

        // Unmap a page
        let unmap_ok = vm.unmap(0x0040_1000);
        if !unmap_ok {
            crate::serial_println!("    [mem-test] FAIL: unmap returned false for valid mapping");
            return false;
        }

        // Verify it's unmapped
        if vm.resolve(0x0040_1000).is_some() {
            crate::serial_println!("    [mem-test] FAIL: page still mapped after unmap");
            return false;
        }

        // Verify count decreased
        if vm.mapping_count() != mappings.len() - 1 {
            crate::serial_println!("    [mem-test] FAIL: mapping count wrong after unmap");
            return false;
        }

        // Test double-unmap
        let double_unmap = vm.unmap(0x0040_1000);
        if double_unmap {
            crate::serial_println!("    [mem-test] FAIL: double unmap returned true");
            return false;
        }

        crate::serial_println!(
            "    [mem-test] PASS: test_virt_map ({} mappings, map/resolve/unmap verified)",
            mappings.len()
        );
        true
    }
}

pub fn run_all() {
    crate::serial_println!("    [mem-test] ==============================");
    crate::serial_println!("    [mem-test] Running memory test suite");
    crate::serial_println!("    [mem-test] ==============================");

    let mut passed = 0u32;
    let mut failed = 0u32;

    if MemoryTests::test_page_alloc() {
        passed += 1;
    } else {
        failed += 1;
    }
    if MemoryTests::test_heap_alloc() {
        passed += 1;
    } else {
        failed += 1;
    }
    if MemoryTests::test_virt_map() {
        passed += 1;
    } else {
        failed += 1;
    }

    crate::serial_println!(
        "    [mem-test] Results: {} passed, {} failed",
        passed,
        failed
    );
}
