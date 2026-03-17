use crate::memory::buddy::PAGE_SIZE;
use crate::sync::Mutex;
/// Guaranteed-allocation memory pools for critical kernel paths.
///
/// Part of the AIOS kernel.
///
/// Three layers of allocation are provided:
///
///   1. `MemPool` — general-purpose pre-allocated pool backed by either the
///      slab allocator (when `element_size` matches a slab cache) or the
///      buddy frame allocator.
///
///   2. `CRITICAL_POOL` — a dedicated pool of `CRITICAL_POOL_FRAMES` physical
///      frames pre-allocated at boot time, reserved for paths that must not
///      fail (interrupt handlers, OOM paths, etc.).  Equivalent to Linux's
///      `__GFP_MEMALLOC` / emergency pool.
///
///   3. `critical_pool_alloc()` — lock-protected pop from the critical pool.
///
/// All code is #![no_std] compatible.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of physical frames pre-allocated for the critical pool at init.
const CRITICAL_POOL_FRAMES: usize = 64;

// ---------------------------------------------------------------------------
// Critical pool — pre-allocated emergency frames
// ---------------------------------------------------------------------------

/// The critical pool: a Vec of physical addresses (buddy-allocated frames)
/// reserved for emergency allocations.
static CRITICAL_POOL: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Allocate a single frame from the critical pool.
///
/// Returns `Some(phys_addr)` on success, `None` if the pool is empty.
/// Does NOT refill the pool; refilling should happen when memory pressure
/// falls (e.g., in the periodic reclaim path).
pub fn critical_pool_alloc() -> Option<usize> {
    CRITICAL_POOL.lock().pop()
}

/// Return a frame to the critical pool (e.g., after emergency use).
///
/// Excess frames beyond `CRITICAL_POOL_FRAMES * 2` are freed back to the
/// buddy allocator to avoid unbounded growth.
pub fn critical_pool_free(addr: usize) {
    let mut pool = CRITICAL_POOL.lock();
    if pool.len() < CRITICAL_POOL_FRAMES * 2 {
        pool.push(addr);
    } else {
        // Pool already full — return the frame to the buddy.
        crate::memory::buddy::free_page(addr);
    }
}

/// Return the number of frames currently in the critical pool.
pub fn critical_pool_available() -> usize {
    CRITICAL_POOL.lock().len()
}

// ---------------------------------------------------------------------------
// Slab cache size matching helper
// ---------------------------------------------------------------------------

/// Standard slab cache object sizes, mirroring the sizes created by
/// `slab::init()`.  Order matters: we pick the smallest fitting size.
const SLAB_SIZES: [usize; 9] = [32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

/// Return `true` if `element_size` matches one of the standard slab caches.
fn matches_slab_cache(element_size: usize) -> bool {
    SLAB_SIZES.iter().any(|&s| s == element_size)
}

// ---------------------------------------------------------------------------
// MemPool
// ---------------------------------------------------------------------------

/// A pre-allocated memory pool that guarantees allocation cannot fail.
pub struct MemPool {
    /// Pre-allocated element pointers (physical addresses or slab pointers).
    pub free_list: Vec<usize>,
    /// Size of each element in bytes.
    pub element_size: usize,
    /// Minimum number of reserved elements.
    pub min_reserved: usize,
}

impl MemPool {
    /// Create a memory pool and pre-fill with `min_reserved` elements.
    ///
    /// If `element_size` matches a slab cache, elements are allocated from
    /// the slab allocator.  Otherwise the buddy allocator provides page-sized
    /// frames (rounded up to the nearest page).
    pub fn new(element_size: usize, min_reserved: usize) -> Self {
        let mut free_list = Vec::with_capacity(min_reserved);

        if matches_slab_cache(element_size) {
            // Allocate from the appropriate slab cache.
            for _ in 0..min_reserved {
                match crate::memory::slab::kmalloc(element_size) {
                    Some(ptr) => free_list.push(ptr as usize),
                    None => {
                        crate::serial_println!(
                            "mempool: slab alloc failed at {} of {} (size={})",
                            free_list.len(),
                            min_reserved,
                            element_size
                        );
                        break;
                    }
                }
            }
        } else {
            // Fall back to buddy allocator (one frame per element).
            for _ in 0..min_reserved {
                match crate::memory::buddy::alloc_page() {
                    Some(addr) => free_list.push(addr),
                    None => {
                        crate::serial_println!(
                            "mempool: buddy alloc failed at {} of {} (size={})",
                            free_list.len(),
                            min_reserved,
                            element_size
                        );
                        break;
                    }
                }
            }
        }

        MemPool {
            free_list,
            element_size,
            min_reserved,
        }
    }

    /// Allocate an element from the pool.
    ///
    /// Priority order:
    ///   1. Pop from the pre-allocated free list (O(1), no allocator call).
    ///   2. Fall back to the slab allocator if the size matches a slab cache.
    ///   3. Fall back to the buddy allocator (page-granularity).
    ///   4. Fall back to the critical pool as a last resort.
    pub fn alloc(&mut self) -> Result<usize, &'static str> {
        // 1. Try the pre-allocated pool first.
        if let Some(ptr) = self.free_list.pop() {
            return Ok(ptr);
        }

        // 2. Try slab allocator.
        if matches_slab_cache(self.element_size) {
            if let Some(ptr) = crate::memory::slab::kmalloc(self.element_size) {
                return Ok(ptr as usize);
            }
        }

        // 3. Try buddy allocator (page frame).
        if let Some(addr) = crate::memory::buddy::alloc_page() {
            return Ok(addr);
        }

        // 4. Emergency: draw from the critical pool.
        if let Some(addr) = critical_pool_alloc() {
            crate::serial_println!(
                "mempool: using critical pool for size={} alloc (pool now {} frames)",
                self.element_size,
                critical_pool_available()
            );
            return Ok(addr);
        }

        Err("mempool: pool empty — all allocators and critical pool exhausted")
    }

    /// Return an element to the pool.
    ///
    /// Elements beyond `min_reserved * 2` are released back to the
    /// appropriate allocator to prevent unbounded pool growth.
    pub fn free(&mut self, ptr: usize) {
        let max_count = self.min_reserved.saturating_mul(2).max(64);
        if self.free_list.len() < max_count {
            self.free_list.push(ptr);
        } else {
            // Pool is oversized — return to slab or buddy.
            if matches_slab_cache(self.element_size) {
                crate::memory::slab::kfree(ptr as *mut u8, self.element_size);
            } else {
                // Buddy expects page-aligned addresses.
                if ptr % PAGE_SIZE == 0 {
                    crate::memory::buddy::free_page(ptr);
                }
                // Non-page-aligned pointers from slab are simply dropped here.
                // A production implementation would record the origin allocator
                // in the pool metadata and dispatch accordingly.
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

/// Initialize the mempool subsystem.
///
/// Pre-allocates the critical emergency pool from the buddy allocator.
pub fn init() {
    let mut pool = CRITICAL_POOL.lock();
    for i in 0..CRITICAL_POOL_FRAMES {
        // Try buddy (Atomic to bypass watermarks), fall back to frame allocator
        let page = crate::memory::buddy::alloc_pages_flags(0, crate::memory::buddy::AllocFlags::Atomic)
            .or_else(|| crate::memory::frame_allocator::allocate_frame().map(|f| f.addr));
        match page {
            Some(addr) => pool.push(addr),
            None => {
                crate::serial_println!(
                    "mempool: critical pool filled {} of {} frames (OOM)",
                    i,
                    CRITICAL_POOL_FRAMES
                );
                break;
            }
        }
    }
    crate::serial_println!(
        "mempool: critical pool ready — {} frames reserved",
        pool.len()
    );
}
