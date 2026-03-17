pub mod balloon;
pub mod buddy;
pub mod cma;
pub mod compaction;
pub mod dma;
/// Memory management for Hoags Kernel Genesis
///
/// Subsystems:
///   1. Frame allocator — tracks physical 4KB page frames via bitmap
///   2. Paging — 4-level x86_64 page table management (with COW + huge pages)
///   3. Heap — kernel heap allocator for alloc (Vec, Box, String)
///   4. Guard — hardened memory protection (redzones, poisoning, canaries)
///   5. Buddy — power-of-2 page allocation with coalescing
///   6. Slab — fixed-size object caching with per-CPU magazines
///   7. Vmalloc — non-contiguous virtual memory allocator
///   8. Page cache — file-backed page caching with LRU eviction
///   9. OOM — out-of-memory killer with scoring and reclaim
///  10. Swap — page swap-out/in with clock replacement
///  11. CMA — contiguous memory allocator for DMA
///  12. Zram — compressed RAM block device (LZ4-lite)
///  13. Shared memory — shmget/shmat/shmdt semantics
///  14. Mmap — memory-mapped file support
///  15. Stats — global memory statistics tracking
///  16. NUMA — NUMA topology with pfn-range registry and SLIT distance table
///  17. VMStat — detailed VM page-level statistics (/proc/vmstat style)
///  18. Huge pages — transparent huge page (THP) support
///  19. Hotplug — memory hot-plug / unplug (128 MB sections)
pub mod frame_allocator;
pub mod guard;
pub mod heap;
pub mod hotplug;
pub mod ksm;
pub mod madvise;
pub mod mempool;
pub mod migrate;
pub mod mincore;
pub mod mlock;
pub mod mmap;
pub mod msync;
pub mod numa;
pub mod oom;
pub mod page_cache;
pub mod paging;
pub mod percpu;
pub mod reclaim;
pub mod shm;
pub mod slab;
pub mod stats;
pub mod swap;
pub mod thp;
pub mod vmalloc;
pub mod vmstat;
pub mod zram;

use crate::{boot_protocol, kprintln};
use frame_allocator::FRAME_ALLOCATOR;

extern "C" {
    static _kernel_end: u8;
}

/// Initialize all memory subsystems.
///
/// Called during kernel boot after GDT and IDT are set up.
/// Order matters: frame allocator -> paging -> heap -> buddy -> slab -> rest.
pub fn init() {
    let kernel_end = unsafe { &_kernel_end as *const u8 as usize };

    // Round up to next frame boundary
    let mut kernel_end_aligned =
        (kernel_end + frame_allocator::FRAME_SIZE - 1) & !(frame_allocator::FRAME_SIZE - 1);

    if let Some(info) = boot_protocol::boot_info() {
        if info.kernel_physical_end > 0 {
            let kernel_end_from_boot = info.kernel_physical_end as usize;
            let aligned_from_boot = (kernel_end_from_boot + frame_allocator::FRAME_SIZE - 1)
                & !(frame_allocator::FRAME_SIZE - 1);
            if aligned_from_boot > kernel_end_aligned {
                kernel_end_aligned = aligned_from_boot;
            }
        }
    }

    kprintln!("  Kernel ends at: {:#x}", kernel_end);
    kprintln!("  Frame alloc starts at: {:#x}", kernel_end_aligned);

    // Step 1: Physical frame allocator
    if let Some(info) = boot_protocol::boot_info() {
        let regions = info.memory_regions();
        if regions.is_empty() {
            frame_allocator::init(kernel_end_aligned);
        } else {
            frame_allocator::init_from_memory_map(kernel_end_aligned, regions);
        }
    } else {
        frame_allocator::init(kernel_end_aligned);
    }
    {
        let alloc = FRAME_ALLOCATOR.lock();
        kprintln!(
            "  Physical memory: {} frames free, {} frames used ({} MB total)",
            alloc.free_count(),
            alloc.used_count(),
            frame_allocator::MAX_MEMORY / (1024 * 1024)
        );
    }

    // Step 2: Page table management
    paging::init();
    kprintln!("  Page tables initialized");

    // Step 3: Kernel heap
    heap::init();
    kprintln!(
        "  Kernel heap: {} KB at {:#x}",
        heap::HEAP_SIZE / 1024,
        heap::HEAP_START
    );

    // Step 4: Buddy allocator (power-of-2 page allocation)
    let buddy_start = kernel_end_aligned + frame_allocator::MAX_MEMORY;
    let buddy_end = buddy_start + buddy::MAX_MEMORY;
    buddy::init(buddy_start, buddy_end);

    // Step 5: SLAB allocator (fixed-size object caching)
    slab::init();

    // Step 6: vmalloc (non-contiguous virtual memory)
    vmalloc::init();

    // Step 7: Page cache (file-backed page caching)
    page_cache::init();

    // Step 8: OOM killer
    oom::init();

    // Step 9: Memory statistics tracking
    stats::init();

    // Step 10: NUMA topology
    numa::init();

    // Step 11: VM statistics
    vmstat::init();

    // Step 12: Transparent huge pages
    thp::init();

    // Step 13: Shared memory
    shm::init();

    // Step 14: Memory-mapped files
    mmap::init();

    // Step 15: Balloon driver (VM memory ballooning)
    balloon::init();

    // Step 16: KSM (Kernel Same-page Merging)
    ksm::init();

    // Step 17: Memory compaction
    compaction::init();

    // Step 18: Page reclaim (LRU, kswapd)
    reclaim::init();

    // Register memory pressure shrinkers so that reclaim can free memory from
    // subsystems without the reclaim engine needing to know their internals.
    // Shrinkers are called in registration order (slab first — cheapest to reap).
    reclaim::register_shrinker(&slab::SLAB_SHRINKER);

    // Step 19: Per-CPU allocator
    percpu::init();

    // Step 20: Memory pools
    mempool::init();

    // Step 21: DMA allocator
    dma::init();

    // Step 22: Page migration
    migrate::init();

    // Step 23: Memory advisory (madvise)
    madvise::init();

    // Step 24: Memory locking (mlock / mlockall)
    mlock::init();

    // Step 25: Memory sync (msync)
    msync::init();

    // Step 26: Page residency query (mincore)
    mincore::init();

    // Step 27: Memory hot-plug / unplug (128 MB sections)
    hotplug::init();

    kprintln!("  Memory subsystems fully initialized");
}
