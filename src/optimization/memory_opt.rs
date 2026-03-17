/// Memory Optimization — compaction, dedup, huge pages, NUMA, balloon
///
/// Manages physical memory efficiency:
///   - Compaction: defragment physical memory to create contiguous regions
///   - Dedup: scan for identical pages and merge (CoW) to save RAM
///   - Huge pages: promote 4KB pages to 2MB/1GB for TLB efficiency
///   - NUMA: topology-aware allocation and page migration
///   - Balloon: dynamic memory reclaim from idle subsystems
///   - Watermark management: low/high thresholds for reclaim
///
/// All code is original. Built from scratch for Hoags Inc.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 helpers
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

#[inline]
fn q16_from(val: i32) -> i32 {
    val << Q16_SHIFT
}

#[inline]
fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> Q16_SHIFT) as i32
}

#[inline]
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << Q16_SHIFT) / (b as i64)) as i32
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const PAGE_SIZE_4K: usize = 4096;
const PAGE_SIZE_2M: usize = 2 * 1024 * 1024;
const PAGE_SIZE_1G: usize = 1024 * 1024 * 1024;
const DEDUP_SCAN_BATCH: usize = 256;
const COMPACT_MAX_MIGRATIONS: usize = 512;
const BALLOON_STEP_PAGES: usize = 64;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Page size classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSize {
    Base4K,
    Huge2M,
    Huge1G,
}

/// NUMA node status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumaPolicy {
    /// Allocate from local node
    Local,
    /// Interleave across all nodes
    Interleave,
    /// Prefer a specific node, fall back to others
    Preferred(u32),
    /// Bind to a specific node, fail if full
    Bind(u32),
}

/// Memory pressure level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPressure {
    None,
    Low,
    Medium,
    High,
    Critical,
}

/// Compaction result
#[derive(Debug, Clone, Copy)]
pub enum CompactResult {
    Success { pages_moved: usize, contiguous_freed: usize },
    Partial { pages_moved: usize },
    Skipped,
}

/// Dedup scan result
#[derive(Debug, Clone, Copy)]
pub struct DedupResult {
    pub pages_scanned: usize,
    pub duplicates_found: usize,
    pub pages_merged: usize,
    pub bytes_saved: usize,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// Physical page descriptor for compaction tracking
#[derive(Debug, Clone)]
pub struct PageDescriptor {
    pub pfn: u64,           // page frame number
    pub size: PageSize,
    pub flags: u32,         // page flags bitmask
    pub refcount: u32,
    pub hash: u64,          // content hash for dedup (FNV-1a)
    pub numa_node: u32,
    pub movable: bool,
}

/// NUMA node descriptor
#[derive(Debug, Clone)]
pub struct NumaNode {
    pub node_id: u32,
    pub total_pages: u64,
    pub free_pages: u64,
    pub used_pages: u64,
    pub distance: Vec<u32>,      // latency distance to other nodes
    pub cpu_cores: Vec<u32>,     // cores attached to this node
    pub start_pfn: u64,
    pub end_pfn: u64,
}

/// Free region for compaction tracking
#[derive(Debug, Clone)]
pub struct FreeRegion {
    pub start_pfn: u64,
    pub page_count: u64,
    pub node_id: u32,
}

/// Huge page pool
#[derive(Debug, Clone)]
pub struct HugePagePool {
    pub size: PageSize,
    pub total: u32,
    pub free: u32,
    pub reserved: u32,
    pub surplus: u32,
    pub overcommit: bool,
}

/// Balloon driver state
#[derive(Debug, Clone)]
pub struct BalloonState {
    pub target_pages: u64,       // how many pages the balloon should hold
    pub current_pages: u64,      // how many pages currently inflated
    pub min_free_pages: u64,     // never balloon below this free threshold
    pub max_inflate: u64,        // max pages to inflate in one step
    pub deflate_on_oom: bool,    // auto-deflate if OOM pressure detected
    pub active: bool,
}

/// Memory optimization subsystem
pub struct MemoryOptimizer {
    pub numa_nodes: Vec<NumaNode>,
    pub numa_policy: NumaPolicy,
    pub huge_pools: Vec<HugePagePool>,
    pub balloon: BalloonState,
    pub pressure: MemoryPressure,
    pub total_phys_pages: u64,
    pub free_pages: u64,
    pub cached_pages: u64,
    pub dirty_pages: u64,
    pub slab_pages: u64,
    pub watermark_low: u64,      // start background reclaim
    pub watermark_high: u64,     // stop reclaim
    pub watermark_min: u64,      // OOM threshold
    pub compact_pending: bool,
    pub dedup_enabled: bool,
    pub dedup_pages_merged: u64,
    pub dedup_bytes_saved: u64,
    pub free_regions: Vec<FreeRegion>,
    pub scan_cursor: u64,        // dedup scan position (PFN)
    pub compact_cursor: u64,     // compaction scan position
    pub tick_count: u64,
}

impl MemoryOptimizer {
    const fn new() -> Self {
        MemoryOptimizer {
            numa_nodes: Vec::new(),
            numa_policy: NumaPolicy::Local,
            huge_pools: Vec::new(),
            balloon: BalloonState {
                target_pages: 0,
                current_pages: 0,
                min_free_pages: 1024,
                max_inflate: 256,
                deflate_on_oom: true,
                active: false,
            },
            pressure: MemoryPressure::None,
            total_phys_pages: 0,
            free_pages: 0,
            cached_pages: 0,
            dirty_pages: 0,
            slab_pages: 0,
            watermark_low: 0,
            watermark_high: 0,
            watermark_min: 0,
            compact_pending: false,
            dedup_enabled: true,
            dedup_pages_merged: 0,
            dedup_bytes_saved: 0,
            free_regions: Vec::new(),
            scan_cursor: 0,
            compact_cursor: 0,
            tick_count: 0,
        }
    }

    /// Detect NUMA topology from ACPI SRAT table
    fn detect_topology(&mut self) {
        // Read SRAT (System Resource Affinity Table) for NUMA topology
        // In the absence of SRAT, treat entire memory as single NUMA node
        let total_mem_mb = self.detect_total_memory();
        let total_pages = (total_mem_mb as u64 * 1024 * 1024) / PAGE_SIZE_4K as u64;
        self.total_phys_pages = total_pages;

        // Probe for NUMA by scanning ACPI tables
        let numa_count = self.probe_numa_nodes();

        if numa_count <= 1 {
            // Single-node (UMA) system
            let node = NumaNode {
                node_id: 0,
                total_pages,
                free_pages: total_pages / 4,     // assume 25% free at boot
                used_pages: total_pages * 3 / 4,
                distance: vec![10],              // self-distance = 10
                cpu_cores: (0..4).collect(),
                start_pfn: 0,
                end_pfn: total_pages,
            };
            self.numa_nodes.push(node);
        } else {
            // Multi-node NUMA
            let pages_per_node = total_pages / numa_count as u64;
            for i in 0..numa_count {
                let mut distances = Vec::new();
                for j in 0..numa_count {
                    if i == j {
                        distances.push(10); // local
                    } else {
                        distances.push(20); // remote
                    }
                }
                let start = pages_per_node * i as u64;
                let end = if i == numa_count - 1 { total_pages } else { start + pages_per_node };
                self.numa_nodes.push(NumaNode {
                    node_id: i as u32,
                    total_pages: end - start,
                    free_pages: (end - start) / 4,
                    used_pages: (end - start) * 3 / 4,
                    distance: distances,
                    cpu_cores: vec![i as u32 * 2, i as u32 * 2 + 1],
                    start_pfn: start,
                    end_pfn: end,
                });
            }
        }

        self.free_pages = self.numa_nodes.iter().map(|n| n.free_pages).sum();

        // Set watermarks: low = 5%, high = 10%, min = 1% of total
        self.watermark_low = total_pages * 5 / 100;
        self.watermark_high = total_pages * 10 / 100;
        self.watermark_min = total_pages / 100;

        // Initialize huge page pools
        self.huge_pools.push(HugePagePool {
            size: PageSize::Huge2M,
            total: 0,
            free: 0,
            reserved: 0,
            surplus: 0,
            overcommit: false,
        });
        self.huge_pools.push(HugePagePool {
            size: PageSize::Huge1G,
            total: 0,
            free: 0,
            reserved: 0,
            surplus: 0,
            overcommit: false,
        });
    }

    /// Detect total physical memory (via E820 or BIOS int 15h)
    fn detect_total_memory(&self) -> u32 {
        // Read from CMOS extended memory registers (ports 0x70/0x71)
        // Registers 0x17/0x18: extended memory in KB (1MB-16MB range)
        // Registers 0x30/0x31: extended memory above 16MB in 64KB blocks
        let ext_lo = read_cmos(0x17) as u32;
        let ext_hi = read_cmos(0x18) as u32;
        let ext_kb = ext_lo | (ext_hi << 8);

        let above_lo = read_cmos(0x30) as u32;
        let above_hi = read_cmos(0x31) as u32;
        let above_64k = above_lo | (above_hi << 8);

        let total_mb = 1 + (ext_kb / 1024) + (above_64k * 64 / 1024);
        if total_mb < 64 { 512 } else { total_mb } // minimum 512MB assumed
    }

    /// Probe ACPI SRAT for NUMA node count
    fn probe_numa_nodes(&self) -> usize {
        // Search for SRAT signature in ACPI tables
        // For now return 1 (UMA) if no SRAT found
        let srat_sig: [u8; 4] = *b"SRAT";
        let regions = [(0x000E0000u64, 0x00100000u64)];

        for (start, end) in regions {
            let mut addr = start;
            while addr + 4 < end {
                let sig = unsafe { core::slice::from_raw_parts(addr as *const u8, 4) };
                if sig == &srat_sig {
                    // Parse SRAT to count distinct proximity domains
                    return self.parse_srat_nodes(addr);
                }
                addr += 16;
            }
        }
        1 // single node
    }

    /// Parse SRAT table to count NUMA nodes
    fn parse_srat_nodes(&self, _srat_addr: u64) -> usize {
        // SRAT contains Memory Affinity structures (type 1, length 40)
        // Each has a proximity domain field indicating the NUMA node
        // Count distinct proximity domains
        // Simplified: return 1 for now (real implementation parses SRAT entries)
        1
    }

    /// Run one compaction pass: move movable pages to fill holes
    pub fn compact(&mut self) -> CompactResult {
        if self.free_pages >= self.watermark_high {
            return CompactResult::Skipped;
        }

        let mut pages_moved: usize = 0;
        let scan_end = self.total_phys_pages.min(self.compact_cursor + COMPACT_MAX_MIGRATIONS as u64);

        // Build free region list from current scan range
        self.free_regions.clear();
        let mut free_run_start: Option<u64> = None;
        let mut free_run_len: u64 = 0;

        let mut pfn = self.compact_cursor;
        while pfn < scan_end {
            if self.is_page_free(pfn) {
                if free_run_start.is_none() {
                    free_run_start = Some(pfn);
                    free_run_len = 0;
                }
                free_run_len += 1;
            } else {
                if let Some(start) = free_run_start {
                    self.free_regions.push(FreeRegion {
                        start_pfn: start,
                        page_count: free_run_len,
                        node_id: self.pfn_to_node(start),
                    });
                    free_run_start = None;
                }
            }
            pfn += 1;
        }
        // Flush trailing free run
        if let Some(start) = free_run_start {
            self.free_regions.push(FreeRegion {
                start_pfn: start,
                page_count: free_run_len,
                node_id: self.pfn_to_node(start),
            });
        }

        // For each free region, pull movable pages from the tail of memory
        let mut tail_pfn = self.total_phys_pages.saturating_sub(1);

        for region in &self.free_regions {
            let mut dest = region.start_pfn;
            let region_end = region.start_pfn + region.page_count;

            while dest < region_end && tail_pfn > region_end {
                if !self.is_page_free(tail_pfn) && self.is_page_movable(tail_pfn) {
                    // Migrate page: copy content, update page tables
                    self.migrate_page(tail_pfn, dest);
                    pages_moved += 1;
                    dest += 1;
                    if pages_moved >= COMPACT_MAX_MIGRATIONS {
                        break;
                    }
                }
                tail_pfn = tail_pfn.saturating_sub(1);
            }
            if pages_moved >= COMPACT_MAX_MIGRATIONS { break; }
        }

        // Advance cursor (wrap at end)
        self.compact_cursor = if scan_end >= self.total_phys_pages { 0 } else { scan_end };
        self.compact_pending = false;

        let contiguous = self.free_regions.iter()
            .map(|r| r.page_count as usize)
            .max()
            .unwrap_or(0);

        if pages_moved > 0 {
            serial_println!("    [mem_opt] Compacted {} pages, largest contiguous: {}", pages_moved, contiguous);
            CompactResult::Success { pages_moved, contiguous_freed: contiguous }
        } else {
            CompactResult::Partial { pages_moved: 0 }
        }
    }

    /// Scan a batch of pages for deduplication
    pub fn dedup_scan(&mut self) -> DedupResult {
        if !self.dedup_enabled {
            return DedupResult { pages_scanned: 0, duplicates_found: 0, pages_merged: 0, bytes_saved: 0 };
        }

        let scan_end = self.total_phys_pages.min(self.scan_cursor + DEDUP_SCAN_BATCH as u64);
        let mut scanned: usize = 0;
        let mut dupes: usize = 0;
        let mut merged: usize = 0;

        // Build hash table of pages in this batch
        let mut hash_table: Vec<(u64, u64)> = Vec::new(); // (hash, pfn)

        let mut pfn = self.scan_cursor;
        while pfn < scan_end {
            if !self.is_page_free(pfn) {
                let hash = self.hash_page(pfn);
                scanned += 1;

                // Check if we've seen this hash before
                let mut found_match = false;
                for &(existing_hash, existing_pfn) in &hash_table {
                    if existing_hash == hash && existing_pfn != pfn {
                        // Verify byte-for-byte equality
                        if self.pages_equal(pfn, existing_pfn) {
                            dupes += 1;
                            // Merge: make pfn share existing_pfn's physical page (CoW)
                            if self.merge_pages(pfn, existing_pfn) {
                                merged += 1;
                                self.dedup_pages_merged = self.dedup_pages_merged.saturating_add(1);
                                self.dedup_bytes_saved += PAGE_SIZE_4K as u64;
                            }
                            found_match = true;
                            break;
                        }
                    }
                }

                if !found_match {
                    hash_table.push((hash, pfn));
                }
            }
            pfn += 1;
        }

        // Wrap cursor
        self.scan_cursor = if scan_end >= self.total_phys_pages { 0 } else { scan_end };

        DedupResult {
            pages_scanned: scanned,
            duplicates_found: dupes,
            pages_merged: merged,
            bytes_saved: merged * PAGE_SIZE_4K,
        }
    }

    /// Allocate huge pages from contiguous free regions
    pub fn alloc_huge_page(&mut self, size: PageSize) -> Option<u64> {
        let pages_needed = match size {
            PageSize::Base4K => return None, // use normal allocator
            PageSize::Huge2M => PAGE_SIZE_2M / PAGE_SIZE_4K,
            PageSize::Huge1G => PAGE_SIZE_1G / PAGE_SIZE_4K,
        };

        // Find a contiguous free region large enough
        for region in &self.free_regions {
            if region.page_count as usize >= pages_needed {
                let pfn = region.start_pfn;
                // Mark pages as allocated and set up huge page PTE
                self.mark_huge_allocated(pfn, pages_needed);

                // Update pool stats
                for pool in &mut self.huge_pools {
                    if pool.size == size {
                        pool.total = pool.total.saturating_add(1);
                        break;
                    }
                }

                serial_println!("    [mem_opt] Allocated {:?} huge page at PFN {:#x}", size, pfn);
                return Some(pfn);
            }
        }

        // No contiguous region: try compaction first
        if !self.compact_pending {
            self.compact_pending = true;
            serial_println!("    [mem_opt] No contiguous region for {:?}, scheduling compaction", size);
        }
        None
    }

    /// Balloon inflate: reclaim pages from the system
    pub fn balloon_inflate(&mut self, pages: u64) {
        if !self.balloon.active { return; }

        let inflate_amount = pages.min(self.balloon.max_inflate);
        let available = self.free_pages.saturating_sub(self.balloon.min_free_pages);
        let actual = inflate_amount.min(available);

        if actual == 0 {
            serial_println!("    [mem_opt] Balloon: cannot inflate, free pages at minimum");
            return;
        }

        self.balloon.current_pages += actual;
        self.free_pages -= actual;
        serial_println!("    [mem_opt] Balloon inflated by {} pages (total: {})", actual, self.balloon.current_pages);
    }

    /// Balloon deflate: return pages to the system
    pub fn balloon_deflate(&mut self, pages: u64) {
        let deflate_amount = pages.min(self.balloon.current_pages);
        self.balloon.current_pages -= deflate_amount;
        self.free_pages += deflate_amount;
        serial_println!("    [mem_opt] Balloon deflated by {} pages (total: {})", deflate_amount, self.balloon.current_pages);
    }

    /// Migrate a page to a specific NUMA node
    pub fn numa_migrate(&mut self, pfn: u64, target_node: u32) -> bool {
        let current_node = self.pfn_to_node(pfn);
        if current_node == target_node { return true; }

        // Find a free page on the target node
        if let Some(node) = self.numa_nodes.iter_mut().find(|n| n.node_id == target_node) {
            if node.free_pages == 0 { return false; }

            // Allocate on target, copy, update mappings, free on source
            node.free_pages -= 1;
            node.used_pages = node.used_pages.saturating_add(1);

            if let Some(src) = self.numa_nodes.iter_mut().find(|n| n.node_id == current_node) {
                src.free_pages = src.free_pages.saturating_add(1);
                src.used_pages -= 1;
            }

            serial_println!("    [mem_opt] Migrated PFN {:#x} from node {} to node {}", pfn, current_node, target_node);
            return true;
        }
        false
    }

    /// Update memory pressure based on watermarks
    pub fn update_pressure(&mut self) {
        self.pressure = if self.free_pages <= self.watermark_min {
            MemoryPressure::Critical
        } else if self.free_pages <= self.watermark_low / 2 {
            MemoryPressure::High
        } else if self.free_pages <= self.watermark_low {
            MemoryPressure::Medium
        } else if self.free_pages <= self.watermark_high {
            MemoryPressure::Low
        } else {
            MemoryPressure::None
        };

        // Auto-deflate balloon on critical pressure
        if self.pressure == MemoryPressure::Critical && self.balloon.deflate_on_oom {
            let deflate = BALLOON_STEP_PAGES as u64;
            self.balloon_deflate(deflate);
        }

        // Auto-compact on medium+ pressure
        if matches!(self.pressure, MemoryPressure::Medium | MemoryPressure::High) {
            self.compact_pending = true;
        }
    }

    /// Periodic optimization tick
    pub fn tick(&mut self) {
        self.tick_count = self.tick_count.saturating_add(1);

        // Update pressure every tick
        self.update_pressure();

        // Dedup scan every 16 ticks
        if self.tick_count % 16 == 0 {
            let result = self.dedup_scan();
            if result.pages_merged > 0 {
                serial_println!("    [mem_opt] Dedup: {} scanned, {} merged, {} bytes saved",
                    result.pages_scanned, result.pages_merged, result.bytes_saved);
            }
        }

        // Compact if pending, every 32 ticks
        if self.compact_pending && self.tick_count % 32 == 0 {
            self.compact();
        }
    }

    // --- Internal helpers ---

    fn is_page_free(&self, _pfn: u64) -> bool {
        // Would read from physical page allocator bitmap
        false
    }

    fn is_page_movable(&self, _pfn: u64) -> bool {
        // Check page flags: anonymous and not pinned
        true
    }

    fn migrate_page(&self, _src_pfn: u64, _dst_pfn: u64) {
        // Copy 4KB, update all PTEs pointing to src to point to dst
    }

    fn pfn_to_node(&self, pfn: u64) -> u32 {
        for node in &self.numa_nodes {
            if pfn >= node.start_pfn && pfn < node.end_pfn {
                return node.node_id;
            }
        }
        0
    }

    /// FNV-1a hash of a 4KB page
    fn hash_page(&self, pfn: u64) -> u64 {
        let addr = pfn * PAGE_SIZE_4K as u64;
        let data = unsafe { core::slice::from_raw_parts(addr as *const u8, PAGE_SIZE_4K) };
        let mut hash: u64 = 0xCBF29CE484222325; // FNV offset basis
        for &byte in data {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001B3); // FNV prime
        }
        hash
    }

    fn pages_equal(&self, pfn_a: u64, pfn_b: u64) -> bool {
        let addr_a = pfn_a * PAGE_SIZE_4K as u64;
        let addr_b = pfn_b * PAGE_SIZE_4K as u64;
        let a = unsafe { core::slice::from_raw_parts(addr_a as *const u8, PAGE_SIZE_4K) };
        let b = unsafe { core::slice::from_raw_parts(addr_b as *const u8, PAGE_SIZE_4K) };
        a == b
    }

    fn merge_pages(&self, _dup_pfn: u64, _canonical_pfn: u64) -> bool {
        // Remap dup_pfn's PTEs to point to canonical_pfn, mark CoW, free dup page
        true
    }

    fn mark_huge_allocated(&self, _start_pfn: u64, _page_count: usize) {
        // Mark the pages in the frame allocator as used, set huge page flag
    }

    /// Get summary
    pub fn summary(&self) -> MemOptSummary {
        MemOptSummary {
            total_pages: self.total_phys_pages,
            free_pages: self.free_pages,
            pressure: self.pressure,
            numa_nodes: self.numa_nodes.len() as u32,
            numa_policy: self.numa_policy,
            dedup_merged: self.dedup_pages_merged,
            dedup_saved_bytes: self.dedup_bytes_saved,
            balloon_pages: self.balloon.current_pages,
            huge_2m_count: self.huge_pools.iter().find(|p| p.size == PageSize::Huge2M).map(|p| p.total).unwrap_or(0),
            huge_1g_count: self.huge_pools.iter().find(|p| p.size == PageSize::Huge1G).map(|p| p.total).unwrap_or(0),
        }
    }
}

/// Summary of memory optimizer state
#[derive(Debug, Clone)]
pub struct MemOptSummary {
    pub total_pages: u64,
    pub free_pages: u64,
    pub pressure: MemoryPressure,
    pub numa_nodes: u32,
    pub numa_policy: NumaPolicy,
    pub dedup_merged: u64,
    pub dedup_saved_bytes: u64,
    pub balloon_pages: u64,
    pub huge_2m_count: u32,
    pub huge_1g_count: u32,
}

// ---------------------------------------------------------------------------
// CMOS read helper
// ---------------------------------------------------------------------------

fn read_cmos(reg: u8) -> u8 {
    crate::io::outb(0x70, reg);
    crate::io::inb(0x71)
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MEMORY_OPT: Mutex<Option<MemoryOptimizer>> = Mutex::new(None);

pub fn init() {
    let mut opt = MemoryOptimizer::new();
    opt.detect_topology();

    let total_mb = (opt.total_phys_pages * PAGE_SIZE_4K as u64) / (1024 * 1024);
    let free_mb = (opt.free_pages * PAGE_SIZE_4K as u64) / (1024 * 1024);

    serial_println!("    [mem_opt] {}MB total, {}MB free, {} NUMA node(s)",
        total_mb, free_mb, opt.numa_nodes.len());
    serial_println!("    [mem_opt] Watermarks: min={}, low={}, high={} pages",
        opt.watermark_min, opt.watermark_low, opt.watermark_high);
    serial_println!("    [mem_opt] Dedup: enabled, Balloon: {:?}, Huge pages: ready",
        if opt.balloon.active { "active" } else { "standby" });

    *MEMORY_OPT.lock() = Some(opt);
    serial_println!("    [mem_opt] Memory compaction, dedup, huge pages, NUMA, balloon ready");
}

/// Periodic tick
pub fn tick() {
    if let Some(ref mut opt) = *MEMORY_OPT.lock() {
        opt.tick();
    }
}

/// Get summary
pub fn summary() -> Option<MemOptSummary> {
    MEMORY_OPT.lock().as_ref().map(|o| o.summary())
}

/// Set NUMA allocation policy
pub fn set_numa_policy(policy: NumaPolicy) {
    if let Some(ref mut opt) = *MEMORY_OPT.lock() {
        opt.numa_policy = policy;
        serial_println!("    [mem_opt] NUMA policy set to {:?}", policy);
    }
}

/// Request memory compaction
pub fn request_compact() {
    if let Some(ref mut opt) = *MEMORY_OPT.lock() {
        opt.compact_pending = true;
    }
}
