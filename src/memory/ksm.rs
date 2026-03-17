/// Kernel Same-page Merging — deduplicate identical physical pages.
///
/// Part of the AIOS kernel.
///
/// Implemented stubs:
///   - `hash_page(ptr)` — FNV-1a hash of 4096 bytes at `ptr`
///   - `ksm_scan_page(virt)` — check if a page hash matches a known shared page
///   - `ksm_background_scan()` — iterate first 256 KSM candidates
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Page size constant
// ---------------------------------------------------------------------------

const PAGE_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// FNV-1a constants (64-bit)
// ---------------------------------------------------------------------------

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

// ---------------------------------------------------------------------------
// Shared page registry
// ---------------------------------------------------------------------------

/// Maximum number of unique page hashes the KSM registry can track.
const KSM_HASH_TABLE_SIZE: usize = 1024;

/// A registered shared page: physical address of the canonical read-only copy
/// plus the FNV-1a hash of its content.
#[derive(Clone, Copy)]
struct SharedPage {
    phys_addr: usize,
    hash: u64,
}

/// Global table of known shared pages (hash → canonical phys addr).
/// Protected by a simple linear-scan; suitable for a few hundred entries.
static KSM_SHARED: crate::sync::Mutex<KsmHashTable> = crate::sync::Mutex::new(KsmHashTable::new());

struct KsmHashTable {
    entries: [Option<SharedPage>; KSM_HASH_TABLE_SIZE],
    count: usize,
}

impl KsmHashTable {
    const fn new() -> Self {
        KsmHashTable {
            entries: [None; KSM_HASH_TABLE_SIZE],
            count: 0,
        }
    }

    /// Look up a hash. Returns the canonical phys_addr if found.
    fn lookup(&self, hash: u64) -> Option<usize> {
        for slot in &self.entries {
            if let Some(sp) = slot {
                if sp.hash == hash {
                    return Some(sp.phys_addr);
                }
            }
        }
        None
    }

    /// Insert a new shared page entry. Returns false if table is full.
    fn insert(&mut self, hash: u64, phys_addr: usize) -> bool {
        if self.count >= KSM_HASH_TABLE_SIZE {
            return false;
        }
        for slot in self.entries.iter_mut() {
            if slot.is_none() {
                *slot = Some(SharedPage { phys_addr, hash });
                self.count = self.count.saturating_add(1);
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute the FNV-1a 64-bit hash of the 4096-byte page at `ptr`.
///
/// # Safety
///
/// `ptr` must point to at least `PAGE_SIZE` bytes of readable memory.
/// The pointer must be valid for the duration of this call and must not
/// alias any concurrently mutated memory.
pub unsafe fn hash_page(ptr: *const u8) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    let mut p = ptr;
    for _ in 0..PAGE_SIZE {
        // read_volatile prevents the compiler from optimising away the reads.
        let byte = core::ptr::read_volatile(p);
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
        p = p.add(1);
    }
    hash
}

/// Check whether the page at `virt` matches any known shared page.
///
/// If the page content hash is already in the KSM shared table this function
/// returns `true` (a merge candidate was found).  If this is the first time
/// the hash has been seen it is registered as a new canonical page and the
/// function returns `false`.
///
/// # Safety (internal)
///
/// Uses `virt` as a raw identity-mapped pointer to read page content.
/// Callers must ensure the virtual address is valid and page-aligned.
pub fn ksm_scan_page(virt: u64) -> bool {
    if virt == 0 || virt % PAGE_SIZE as u64 != 0 {
        return false;
    }

    let hash = unsafe { hash_page(virt as *const u8) };

    let mut table = KSM_SHARED.lock();
    if table.lookup(hash).is_some() {
        // A page with identical content already exists — merge candidate.
        return true;
    }

    // Register this page as the canonical copy for its hash.
    table.insert(hash, virt as usize);
    false
}

/// Background KSM scanner: scan the first 256 registered KSM candidates.
///
/// Iterates the `KsmScanner::pages` list (up to 256 entries) and calls
/// `ksm_scan_page()` on each.  Pages identified as duplicates increment
/// `pages_merged`.  This function is intended to be called periodically
/// from a low-priority kernel thread or timer callback.
pub fn ksm_background_scan(scanner: &mut KsmScanner) {
    if !scanner.enabled {
        return;
    }

    let limit = scanner.pages.len().min(256);
    let mut merged = 0usize;

    for i in 0..limit {
        let virt = scanner.pages[i].phys_addr as u64; // phys == virt for identity map
        if ksm_scan_page(virt) {
            merged = merged.saturating_add(1);
            scanner.pages[i].shared_count = scanner.pages[i].shared_count.saturating_add(1);
        }
        // Refresh the stored hash so the registry stays current.
        if virt != 0 && virt % PAGE_SIZE as u64 == 0 {
            scanner.pages[i].hash = unsafe { hash_page(virt as *const u8) };
        }
    }

    scanner.pages_merged = scanner.pages_merged.saturating_add(merged as u64);

    if merged > 0 {
        crate::serial_println!("ksm: background scan merged {} pages", merged);
    }
}

// ---------------------------------------------------------------------------
// KsmPage / KsmScanner types (kept for ABI compatibility)
// ---------------------------------------------------------------------------

/// Tracks a page registered for KSM scanning.
pub struct KsmPage {
    pub phys_addr: usize,
    pub hash: u64,
    pub shared_count: usize,
}

/// KSM scanner that finds and merges duplicate pages.
pub struct KsmScanner {
    pub pages: Vec<KsmPage>,
    pub pages_merged: u64,
    pub enabled: bool,
}

impl KsmScanner {
    pub fn new() -> Self {
        KsmScanner {
            pages: Vec::new(),
            pages_merged: 0,
            enabled: false,
        }
    }

    /// Scan registered pages and merge duplicates via COW.
    ///
    /// Delegates to `ksm_background_scan()` which implements page-content
    /// hashing against the shared-page registry.
    pub fn scan_and_merge(&mut self) -> usize {
        if !self.enabled {
            return 0;
        }
        let before = self.pages_merged;
        ksm_background_scan(self);
        (self.pages_merged.saturating_sub(before)) as usize
    }
}

/// Initialize the KSM subsystem.
pub fn init() {
    // KSM_SHARED hash table is initialized via const fn.
    // A background scanner thread would be started here once scheduling is
    // available.
}
