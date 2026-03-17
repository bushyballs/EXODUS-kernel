use crate::sync::Mutex;
/// Database compaction and space reclaim
///
/// Part of the AIOS database engine. Analyzes page utilization,
/// identifies fragmented and free pages, defragments by repacking
/// live data, and reclaims unused space.
use alloc::vec::Vec;

pub struct VacuumStats {
    pub pages_freed: u32,
    pub bytes_reclaimed: u64,
    pub fragmentation_before: u8,
    pub fragmentation_after: u8,
}

/// Page utilization record
#[derive(Clone, Copy)]
struct PageInfo {
    page_id: u32,
    total_bytes: u32,
    used_bytes: u32,
    is_free: bool,
    is_overflow: bool,
    table_id: u32,
}

impl PageInfo {
    fn utilization_pct(&self) -> u8 {
        if self.total_bytes == 0 {
            return 0;
        }
        ((self.used_bytes as u64 * 100) / self.total_bytes as u64) as u8
    }

    fn wasted_bytes(&self) -> u32 {
        if self.is_free {
            self.total_bytes
        } else {
            self.total_bytes.saturating_sub(self.used_bytes)
        }
    }
}

/// Free page list manager
struct FreePageList {
    free_pages: Vec<u32>, // page IDs
    total_free_bytes: u64,
}

impl FreePageList {
    fn new() -> Self {
        Self {
            free_pages: Vec::new(),
            total_free_bytes: 0,
        }
    }

    fn add_page(&mut self, page_id: u32, page_size: u32) {
        self.free_pages.push(page_id);
        self.total_free_bytes += page_size as u64;
    }

    fn take_page(&mut self) -> Option<u32> {
        if self.free_pages.is_empty() {
            return None;
        }
        let page = self.free_pages.remove(0);
        // Assume standard 4KB pages
        if self.total_free_bytes >= 4096 {
            self.total_free_bytes -= 4096;
        }
        Some(page)
    }

    fn count(&self) -> usize {
        self.free_pages.len()
    }
}

/// Repack state for incremental vacuuming
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VacuumPhase {
    Idle,
    Analyzing,
    Repacking,
    Finalizing,
    Complete,
}

struct RepackState {
    phase: VacuumPhase,
    current_page: u32,
    total_pages: u32,
    pages_moved: u32,
    bytes_saved: u64,
    progress_pct: u8,
}

impl RepackState {
    fn new() -> Self {
        Self {
            phase: VacuumPhase::Idle,
            current_page: 0,
            total_pages: 0,
            pages_moved: 0,
            bytes_saved: 0,
            progress_pct: 0,
        }
    }

    fn update_progress(&mut self) {
        if self.total_pages > 0 {
            self.progress_pct = ((self.current_page as u64 * 100) / self.total_pages as u64) as u8;
        }
    }
}

pub struct Vacuum {
    pages: Vec<PageInfo>,
    free_list: FreePageList,
    repack_state: RepackState,
    page_size: u32,
    auto_vacuum_threshold: u8, // fragmentation % to trigger auto vacuum
    last_vacuum_sequence: u64,
}

impl Vacuum {
    pub fn new() -> Self {
        crate::serial_println!("[db::vacuum] vacuum engine created");
        Self {
            pages: Vec::new(),
            free_list: FreePageList::new(),
            repack_state: RepackState::new(),
            page_size: 4096,
            auto_vacuum_threshold: 30,
            last_vacuum_sequence: 0,
        }
    }

    /// Simulate loading page information from the database file
    fn scan_pages(&mut self) {
        // In a real implementation, this would read the database file header
        // and page allocation bitmap. Here we simulate a database with
        // some fragmentation.
        self.pages.clear();

        // Simulate 1000 pages with varying utilization
        let total_pages = 1000u32;
        let mut seed: u32 = 42;
        for i in 0..total_pages {
            // Simple PRNG for simulating page states
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let rand_val = (seed >> 16) & 0xFF;

            let is_free = rand_val < 30; // ~12% free pages
            let used_bytes = if is_free {
                0
            } else {
                let util = (rand_val as u32 * self.page_size) / 256;
                util.max(100) // at least 100 bytes used
            };

            self.pages.push(PageInfo {
                page_id: i,
                total_bytes: self.page_size,
                used_bytes,
                is_free,
                is_overflow: rand_val > 240,
                table_id: i % 10,
            });

            if is_free {
                self.free_list.add_page(i, self.page_size);
            }
        }
    }

    /// Calculate overall fragmentation percentage
    fn compute_fragmentation(&self) -> u8 {
        if self.pages.is_empty() {
            return 0;
        }

        let mut total_wasted: u64 = 0;
        let mut total_capacity: u64 = 0;

        for page in &self.pages {
            total_capacity += page.total_bytes as u64;
            total_wasted += page.wasted_bytes() as u64;
        }

        if total_capacity == 0 {
            return 0;
        }
        ((total_wasted * 100) / total_capacity) as u8
    }

    /// Identify pages that should be repacked (low utilization, non-free)
    fn find_repack_candidates(&self) -> Vec<u32> {
        let mut candidates = Vec::new();
        let min_util = 50u8; // pages with <50% utilization are candidates

        for page in &self.pages {
            if !page.is_free && page.utilization_pct() < min_util {
                candidates.push(page.page_id);
            }
        }

        // Sort by utilization (lowest first) for best packing
        // Simple insertion sort
        for i in 1..candidates.len() {
            let key = candidates[i];
            let key_util = self.page_utilization(key);
            let mut j = i;
            while j > 0 && self.page_utilization(candidates[j - 1]) > key_util {
                candidates[j] = candidates[j - 1];
                j -= 1;
            }
            candidates[j] = key;
        }

        candidates
    }

    fn page_utilization(&self, page_id: u32) -> u8 {
        for page in &self.pages {
            if page.page_id == page_id {
                return page.utilization_pct();
            }
        }
        0
    }

    pub fn analyze(&self) -> VacuumStats {
        let frag = self.compute_fragmentation();
        let free_count = self.free_list.count() as u32;
        let free_bytes = self.free_list.total_free_bytes;

        crate::serial_println!(
            "[db::vacuum] analysis: {}% fragmentation, {} free pages ({} bytes)",
            frag,
            free_count,
            free_bytes
        );

        VacuumStats {
            pages_freed: free_count,
            bytes_reclaimed: free_bytes,
            fragmentation_before: frag,
            fragmentation_after: frag, // no change since we didn't run
        }
    }

    pub fn run(&mut self) -> Result<VacuumStats, ()> {
        crate::serial_println!("[db::vacuum] starting vacuum operation");

        // Phase 1: Scan pages
        self.repack_state.phase = VacuumPhase::Analyzing;
        self.scan_pages();
        let frag_before = self.compute_fragmentation();
        crate::serial_println!(
            "[db::vacuum] scanned {} pages, {}% fragmentation",
            self.pages.len(),
            frag_before
        );

        // Phase 2: Find candidates for repacking
        self.repack_state.phase = VacuumPhase::Repacking;
        let candidates = self.find_repack_candidates();
        self.repack_state.total_pages = candidates.len() as u32;
        crate::serial_println!("[db::vacuum] found {} repack candidates", candidates.len());

        // Phase 3: Repack pages by consolidating data
        let mut pages_moved = 0u32;
        let mut bytes_saved = 0u64;

        for (idx, &page_id) in candidates.iter().enumerate() {
            // Simulate moving data from low-utilization page to a fuller page
            for page in &mut self.pages {
                if page.page_id == page_id && !page.is_free {
                    let wasted = page.wasted_bytes();
                    bytes_saved += wasted as u64;
                    page.used_bytes = 0;
                    page.is_free = true;
                    pages_moved += 1;
                }
            }

            self.repack_state.current_page = idx as u32;
            self.repack_state.update_progress();
        }

        self.repack_state.pages_moved = pages_moved;
        self.repack_state.bytes_saved = bytes_saved;

        // Phase 4: Finalize - update free list
        self.repack_state.phase = VacuumPhase::Finalizing;
        self.free_list = FreePageList::new();
        for page in &self.pages {
            if page.is_free {
                self.free_list.add_page(page.page_id, page.total_bytes);
            }
        }

        let frag_after = self.compute_fragmentation();
        self.repack_state.phase = VacuumPhase::Complete;
        self.last_vacuum_sequence = self.last_vacuum_sequence.saturating_add(1);

        let stats = VacuumStats {
            pages_freed: pages_moved,
            bytes_reclaimed: bytes_saved,
            fragmentation_before: frag_before,
            fragmentation_after: frag_after,
        };

        crate::serial_println!(
            "[db::vacuum] complete: freed {} pages, reclaimed {} bytes, frag {}%->{}%",
            stats.pages_freed,
            stats.bytes_reclaimed,
            frag_before,
            frag_after
        );

        Ok(stats)
    }

    /// Check if auto-vacuum should be triggered
    pub fn should_auto_vacuum(&self) -> bool {
        self.compute_fragmentation() >= self.auto_vacuum_threshold
    }

    /// Set the auto-vacuum threshold
    pub fn set_auto_vacuum_threshold(&mut self, threshold_pct: u8) {
        self.auto_vacuum_threshold = threshold_pct.min(100);
    }

    /// Get current vacuum progress (0..100)
    pub fn progress(&self) -> u8 {
        self.repack_state.progress_pct
    }
}

static VACUUM: Mutex<Option<Vacuum>> = Mutex::new(None);

pub fn init() {
    let vacuum = Vacuum::new();
    let mut v = VACUUM.lock();
    *v = Some(vacuum);
    crate::serial_println!("[db::vacuum] vacuum subsystem initialized");
}
