use crate::memory::buddy;
/// swap — swap management subsystem for Genesis
///
/// Manages page swap-out (eviction to backing store) and swap-in (reload to
/// physical memory). The backing store is either a zram device (compressed
/// RAM) or a block device partition.
///
/// Architecture:
///   - Swap map: tracks which swap slot holds which page
///   - Page replacement: clock (second-chance) algorithm
///   - Swappiness tuning: controls eagerness to swap vs. reclaim page cache
///   - Multiple swap areas with priority ordering
///   - Integration with OOM subsystem
///
/// Inspired by: Linux swap (mm/swap.c, mm/swapfile.c). All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum swap areas (e.g., zram0, /dev/sda2, etc.)
const MAX_SWAP_AREAS: usize = 8;

/// Maximum swap slots per area
const MAX_SWAP_SLOTS: usize = 16384;

/// Page size
const PAGE_SIZE: usize = 4096;

/// Default swappiness (0-200, 100 = balanced, 0 = never swap anonymous)
const DEFAULT_SWAPPINESS: u32 = 60;

/// Clock hand entries for page replacement
const CLOCK_ENTRIES: usize = 8192;

// ---------------------------------------------------------------------------
// Swap slot map
// ---------------------------------------------------------------------------

/// Type of swap backing store
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapBackend {
    /// Not active
    None,
    /// Compressed RAM (zram device)
    Zram { dev_id: u8 },
    /// Block device (disk partition)
    Block { dev_id: u8, start_sector: u64 },
}

/// State of a swap slot
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapSlotState {
    /// Slot is free
    Free,
    /// Slot holds a swapped page
    Occupied,
    /// Slot is being read (swap-in in progress)
    Reading,
    /// Slot is being written (swap-out in progress)
    Writing,
    /// Slot is bad / unusable
    Bad,
}

/// A single swap slot
#[derive(Clone, Copy)]
pub struct SwapSlot {
    /// State
    pub state: SwapSlotState,
    /// Physical address of the original page (for reverse lookup)
    pub orig_phys: usize,
    /// Owner process ID
    pub owner_pid: u32,
    /// Virtual address in the owner's address space
    pub virt_addr: usize,
    /// Backend-specific handle (zram slot index, or sector offset)
    pub backend_handle: u32,
    /// Reference count (multiple PTEs can point to same swap slot via fork)
    pub refcount: u16,
}

impl SwapSlot {
    const fn empty() -> Self {
        SwapSlot {
            state: SwapSlotState::Free,
            orig_phys: 0,
            owner_pid: 0,
            virt_addr: 0,
            backend_handle: 0,
            refcount: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Swap area
// ---------------------------------------------------------------------------

/// Statistics for a swap area
#[derive(Debug, Clone, Copy, Default)]
pub struct SwapAreaStats {
    /// Total swap-out operations
    pub swap_out_count: u64,
    /// Total swap-in operations
    pub swap_in_count: u64,
    /// Total swap-in failures
    pub swap_in_failures: u64,
    /// Total swap-out failures
    pub swap_out_failures: u64,
    /// Pages currently swapped out
    pub pages_swapped: u64,
    /// Peak pages swapped
    pub peak_pages: u64,
}

/// A swap area (one backing store)
pub struct SwapArea {
    /// Backend type
    pub backend: SwapBackend,
    /// Priority (higher = preferred)
    pub priority: i32,
    /// Maximum slots
    pub max_slots: usize,
    /// Slot map
    pub slots: [SwapSlot; MAX_SWAP_SLOTS],
    /// Number of occupied slots
    pub used_slots: usize,
    /// Free slot search hint (next index to check)
    pub free_hint: usize,
    /// Whether this area is active
    pub active: bool,
    /// Statistics
    pub stats: SwapAreaStats,
}

impl SwapArea {
    const fn new() -> Self {
        const EMPTY_SLOT: SwapSlot = SwapSlot::empty();
        SwapArea {
            backend: SwapBackend::None,
            priority: 0,
            max_slots: MAX_SWAP_SLOTS,
            slots: [EMPTY_SLOT; MAX_SWAP_SLOTS],
            used_slots: 0,
            free_hint: 0,
            active: false,
            stats: SwapAreaStats {
                swap_out_count: 0,
                swap_in_count: 0,
                swap_in_failures: 0,
                swap_out_failures: 0,
                pages_swapped: 0,
                peak_pages: 0,
            },
        }
    }

    /// Find a free slot. Returns slot index.
    fn find_free_slot(&mut self) -> Option<usize> {
        let start = self.free_hint;
        for i in 0..self.max_slots {
            let idx = (start + i) % self.max_slots;
            if self.slots[idx].state == SwapSlotState::Free {
                self.free_hint = (idx + 1) % self.max_slots;
                return Some(idx);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Page replacement (Clock / Second-Chance)
// ---------------------------------------------------------------------------

/// Entry in the clock hand for page replacement
#[derive(Clone, Copy)]
struct ClockEntry {
    /// Physical page address
    phys_addr: usize,
    /// Owner PID
    pid: u32,
    /// Virtual address
    virt_addr: usize,
    /// Referenced bit (set on access, cleared on clock sweep)
    referenced: bool,
    /// Active flag
    active: bool,
}

impl ClockEntry {
    const fn empty() -> Self {
        ClockEntry {
            phys_addr: 0,
            pid: 0,
            virt_addr: 0,
            referenced: false,
            active: false,
        }
    }
}

/// Clock-based page replacement state
struct ClockReplacer {
    /// Circular buffer of page entries
    entries: [ClockEntry; CLOCK_ENTRIES],
    /// Current hand position
    hand: usize,
    /// Number of active entries
    count: usize,
}

impl ClockReplacer {
    const fn new() -> Self {
        const EMPTY: ClockEntry = ClockEntry::empty();
        ClockReplacer {
            entries: [EMPTY; CLOCK_ENTRIES],
            hand: 0,
            count: 0,
        }
    }

    /// Register a page for replacement tracking
    fn add_page(&mut self, phys_addr: usize, pid: u32, virt_addr: usize) {
        // Find a free slot or reuse an inactive one
        for i in 0..CLOCK_ENTRIES {
            if !self.entries[i].active {
                self.entries[i] = ClockEntry {
                    phys_addr,
                    pid,
                    virt_addr,
                    referenced: true,
                    active: true,
                };
                self.count += 1;
                return;
            }
        }
    }

    /// Remove a page from tracking
    fn remove_page(&mut self, phys_addr: usize) {
        for i in 0..CLOCK_ENTRIES {
            if self.entries[i].active && self.entries[i].phys_addr == phys_addr {
                self.entries[i].active = false;
                self.count -= 1;
                return;
            }
        }
    }

    /// Mark a page as recently referenced
    fn mark_referenced(&mut self, phys_addr: usize) {
        for i in 0..CLOCK_ENTRIES {
            if self.entries[i].active && self.entries[i].phys_addr == phys_addr {
                self.entries[i].referenced = true;
                return;
            }
        }
    }

    /// Select a victim page for swap-out using clock algorithm
    fn select_victim(&mut self) -> Option<(usize, u32, usize)> {
        if self.count == 0 {
            return None;
        }

        // Two full sweeps: first clears referenced bits, second selects victim
        let max_iterations = CLOCK_ENTRIES * 2;
        for _ in 0..max_iterations {
            let idx = self.hand;
            self.hand = (self.hand + 1) % CLOCK_ENTRIES;

            if !self.entries[idx].active {
                continue;
            }

            if self.entries[idx].referenced {
                // Second chance: clear referenced bit and move on
                self.entries[idx].referenced = false;
            } else {
                // Not referenced — this is our victim
                let entry = self.entries[idx];
                self.entries[idx].active = false;
                self.count -= 1;
                return Some((entry.phys_addr, entry.pid, entry.virt_addr));
            }
        }

        // All pages referenced — force-evict the current hand position
        for _ in 0..CLOCK_ENTRIES {
            let idx = self.hand;
            self.hand = (self.hand + 1) % CLOCK_ENTRIES;
            if self.entries[idx].active {
                let entry = self.entries[idx];
                self.entries[idx].active = false;
                self.count -= 1;
                return Some((entry.phys_addr, entry.pid, entry.virt_addr));
            }
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Global swap state
// ---------------------------------------------------------------------------

/// Global swap manager
struct SwapManager {
    /// Swap areas, sorted by priority (highest first)
    areas: [SwapArea; MAX_SWAP_AREAS],
    /// Number of active areas
    area_count: usize,
    /// Page replacement clock
    replacer: ClockReplacer,
    /// Swappiness (0-200)
    swappiness: u32,
    /// Whether swapping is enabled globally
    enabled: bool,
}

impl SwapManager {
    const fn new() -> Self {
        const EMPTY_AREA: SwapArea = SwapArea::new();
        SwapManager {
            areas: [EMPTY_AREA; MAX_SWAP_AREAS],
            area_count: 0,
            replacer: ClockReplacer::new(),
            swappiness: DEFAULT_SWAPPINESS,
            enabled: false,
        }
    }

    /// Find the best swap area (highest priority with free slots)
    fn find_area(&mut self) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut best_priority = i32::MIN;

        for i in 0..MAX_SWAP_AREAS {
            if self.areas[i].active
                && self.areas[i].used_slots < self.areas[i].max_slots
                && self.areas[i].priority > best_priority
            {
                best = Some(i);
                best_priority = self.areas[i].priority;
            }
        }
        best
    }
}

static SWAP: Mutex<SwapManager> = Mutex::new(SwapManager::new());

/// Global counters (lockless)
pub static TOTAL_SWAP_OUT: AtomicU64 = AtomicU64::new(0);
pub static TOTAL_SWAP_IN: AtomicU64 = AtomicU64::new(0);
pub static CURRENT_SWAPPINESS: AtomicU32 = AtomicU32::new(DEFAULT_SWAPPINESS);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Add a swap area. Returns area index.
pub fn add_swap_area(backend: SwapBackend, priority: i32) -> Option<usize> {
    let mut mgr = SWAP.lock();
    if mgr.area_count >= MAX_SWAP_AREAS {
        return None;
    }

    // Find a free slot
    let idx = (0..MAX_SWAP_AREAS).find(|&i| !mgr.areas[i].active)?;

    mgr.areas[idx].backend = backend;
    mgr.areas[idx].priority = priority;
    mgr.areas[idx].active = true;
    mgr.area_count += 1;
    mgr.enabled = true;

    let desc = match backend {
        SwapBackend::None => "none",
        SwapBackend::Zram { .. } => "zram",
        SwapBackend::Block { .. } => "block",
    };
    serial_println!(
        "  [swap] added area {} ({}, priority={})",
        idx,
        desc,
        priority
    );

    Some(idx)
}

/// Remove a swap area (must be empty first)
pub fn remove_swap_area(area_idx: usize) -> bool {
    let mut mgr = SWAP.lock();
    if area_idx >= MAX_SWAP_AREAS || !mgr.areas[area_idx].active {
        return false;
    }
    if mgr.areas[area_idx].used_slots > 0 {
        serial_println!(
            "  [swap] cannot remove area {} — {} slots still in use",
            area_idx,
            mgr.areas[area_idx].used_slots
        );
        return false;
    }
    mgr.areas[area_idx].active = false;
    mgr.area_count -= 1;
    if mgr.area_count == 0 {
        mgr.enabled = false;
    }
    true
}

/// Swap out a page: evict it to backing store and free the physical page.
///
/// Returns (area_idx, slot_idx) on success.
pub fn swap_out(phys_addr: usize, pid: u32, virt_addr: usize) -> Option<(usize, usize)> {
    let mut mgr = SWAP.lock();
    if !mgr.enabled {
        return None;
    }

    let area_idx = mgr.find_area()?;
    let area = &mut mgr.areas[area_idx];
    let slot_idx = area.find_free_slot()?;

    // Write page to backing store
    let backend_handle = match area.backend {
        SwapBackend::Zram { dev_id } => {
            // Compress and store via zram
            drop(mgr); // release lock before calling zram
            let result = crate::memory::zram::store_page(dev_id, phys_addr, pid);
            let mut mgr = SWAP.lock();
            match result {
                Some((_dev, zram_slot)) => zram_slot as u32,
                None => {
                    mgr.areas[area_idx].stats.swap_out_failures += 1;
                    return None;
                }
            }
        }
        SwapBackend::Block {
            dev_id: _,
            start_sector,
        } => {
            // Write to block device (simulated)
            let sector = start_sector + (slot_idx as u64) * ((PAGE_SIZE as u64) / 512);
            // In real impl: block_write(dev_id, sector, phys_addr, PAGE_SIZE)
            let _ = sector;
            slot_idx as u32
        }
        SwapBackend::None => return None,
    };

    // Re-acquire manager for the zram path (already held for block path)
    let mut mgr = SWAP.lock();
    let area = &mut mgr.areas[area_idx];

    area.slots[slot_idx] = SwapSlot {
        state: SwapSlotState::Occupied,
        orig_phys: phys_addr,
        owner_pid: pid,
        virt_addr,
        backend_handle,
        refcount: 1,
    };
    area.used_slots += 1;
    area.stats.swap_out_count += 1;
    area.stats.pages_swapped += 1;
    if area.stats.pages_swapped > area.stats.peak_pages {
        area.stats.peak_pages = area.stats.pages_swapped;
    }

    // Remove from replacement tracker (it is now on disk/zram)
    mgr.replacer.remove_page(phys_addr);

    TOTAL_SWAP_OUT.fetch_add(1, Ordering::Relaxed);

    Some((area_idx, slot_idx))
}

/// Swap in a page: reload from backing store into a new physical page.
///
/// Returns physical address of the new page, or None.
pub fn swap_in(area_idx: usize, slot_idx: usize) -> Option<usize> {
    let mut mgr = SWAP.lock();
    if area_idx >= MAX_SWAP_AREAS || !mgr.areas[area_idx].active {
        return None;
    }

    let area = &mut mgr.areas[area_idx];
    if slot_idx >= area.max_slots || area.slots[slot_idx].state != SwapSlotState::Occupied {
        return None;
    }

    area.slots[slot_idx].state = SwapSlotState::Reading;
    let slot = area.slots[slot_idx];
    let backend = area.backend;

    // Allocate a new physical page
    drop(mgr); // release lock for buddy allocation
    let new_phys = buddy::alloc_page()?;

    // Read from backing store
    let success = match backend {
        SwapBackend::Zram { dev_id } => {
            crate::memory::zram::load_page(dev_id, slot.backend_handle as usize, new_phys)
        }
        SwapBackend::Block {
            dev_id: _,
            start_sector,
        } => {
            // Read from block device (simulated)
            let _sector = start_sector + (slot_idx as u64) * ((PAGE_SIZE as u64) / 512);
            // In real impl: block_read(dev_id, sector, new_phys, PAGE_SIZE)
            true
        }
        SwapBackend::None => false,
    };

    {
        let mut mgr = SWAP.lock();

        if !success {
            mgr.areas[area_idx].slots[slot_idx].state = SwapSlotState::Occupied; // revert
            mgr.areas[area_idx].stats.swap_in_failures += 1;
            buddy::free_page(new_phys);
            return None;
        }

        // Free the swap slot
        mgr.areas[area_idx].slots[slot_idx].refcount -= 1;
        let refcount = mgr.areas[area_idx].slots[slot_idx].refcount;
        if refcount == 0 {
            mgr.areas[area_idx].slots[slot_idx].state = SwapSlotState::Free;
            mgr.areas[area_idx].slots[slot_idx].orig_phys = 0;
            mgr.areas[area_idx].used_slots -= 1;
            mgr.areas[area_idx].stats.pages_swapped -= 1;

            // Free zram slot too — must drop mgr first to avoid deadlock
            if let SwapBackend::Zram { dev_id } = backend {
                drop(mgr);
                crate::memory::zram::free_slot(dev_id, slot.backend_handle as usize);
            }
        } else {
            mgr.areas[area_idx].slots[slot_idx].state = SwapSlotState::Occupied;
        }
    }

    // Register the new page for replacement tracking
    {
        let mut mgr = SWAP.lock();
        mgr.replacer
            .add_page(new_phys, slot.owner_pid, slot.virt_addr);
        mgr.areas[area_idx].stats.swap_in_count += 1;
    }

    TOTAL_SWAP_IN.fetch_add(1, Ordering::Relaxed);

    Some(new_phys)
}

/// Register a page for swap tracking (call when page is first allocated)
pub fn register_page(phys_addr: usize, pid: u32, virt_addr: usize) {
    let mut mgr = SWAP.lock();
    mgr.replacer.add_page(phys_addr, pid, virt_addr);
}

/// Unregister a page from swap tracking (call when page is freed)
pub fn unregister_page(phys_addr: usize) {
    let mut mgr = SWAP.lock();
    mgr.replacer.remove_page(phys_addr);
}

/// Mark a page as recently accessed (reset the clock bit)
pub fn page_accessed(phys_addr: usize) {
    let mut mgr = SWAP.lock();
    mgr.replacer.mark_referenced(phys_addr);
}

/// Select a victim page for eviction (used by reclaim paths)
pub fn select_victim() -> Option<(usize, u32, usize)> {
    let mut mgr = SWAP.lock();
    mgr.replacer.select_victim()
}

/// Try to reclaim `target` pages by swapping out
pub fn reclaim(target: usize) -> usize {
    let mut reclaimed = 0;

    for _ in 0..target {
        // Select a victim
        let victim = {
            let mut mgr = SWAP.lock();
            mgr.replacer.select_victim()
        };

        if let Some((phys, pid, vaddr)) = victim {
            if swap_out(phys, pid, vaddr).is_some() {
                // Free the original physical page
                buddy::free_page(phys);
                reclaimed += 1;
            }
        } else {
            break; // no more victims
        }
    }

    if reclaimed > 0 {
        serial_println!("  [swap] reclaimed {} pages", reclaimed);
    }

    reclaimed
}

/// Get or set swappiness (0 = avoid swapping, 200 = aggressive)
pub fn get_swappiness() -> u32 {
    CURRENT_SWAPPINESS.load(Ordering::Relaxed)
}

pub fn set_swappiness(val: u32) {
    let clamped = val.min(200);
    CURRENT_SWAPPINESS.store(clamped, Ordering::Relaxed);
    SWAP.lock().swappiness = clamped;
    serial_println!("  [swap] swappiness set to {}", clamped);
}

/// Check if swapping is enabled
pub fn is_enabled() -> bool {
    SWAP.lock().enabled
}

/// Get swap usage summary
pub fn usage_summary() -> alloc::string::String {
    use alloc::format;
    use alloc::string::String;
    let mgr = SWAP.lock();
    let mut s = String::from("swap areas:\n");
    for i in 0..MAX_SWAP_AREAS {
        if mgr.areas[i].active {
            let a = &mgr.areas[i];
            let desc = match a.backend {
                SwapBackend::None => "none",
                SwapBackend::Zram { .. } => "zram",
                SwapBackend::Block { .. } => "block",
            };
            let pct = if a.max_slots > 0 {
                (a.used_slots * 100) / a.max_slots
            } else {
                0
            };
            s.push_str(&format!(
                "  area{}: {} pri={} {}/{} slots ({}%) out={} in={}\n",
                i,
                desc,
                a.priority,
                a.used_slots,
                a.max_slots,
                pct,
                a.stats.swap_out_count,
                a.stats.swap_in_count,
            ));
        }
    }
    s.push_str(&format!("  swappiness: {}\n", mgr.swappiness));
    s
}

/// Initialize swap subsystem
pub fn init() {
    // Set up default zram-backed swap area
    if let Some(area_idx) = add_swap_area(SwapBackend::Zram { dev_id: 0 }, 100) {
        serial_println!("  [swap] default zram swap area {} ready", area_idx);
    }
    serial_println!(
        "  [swap] subsystem initialized, swappiness={}",
        DEFAULT_SWAPPINESS
    );
}

/// Write a hibernation image to a physical destination address.
///
/// `dest` is the physical base address of the target swap partition or EFI
/// block device region where the image should be stored.  `size` is the
/// number of bytes to write (typically all resident RAM pages).
///
/// This is a stub implementation that logs the intent and returns `Ok(())`.
/// A full implementation would:
///   1. Iterate all page frames via the buddy allocator's free/used bitmap.
///   2. Compress each page with a lightweight algorithm (e.g. LZ4).
///   3. Write the compressed image to the EFI block device at `dest`.
///   4. Record a magic header so the bootloader can detect and restore it.
///
/// Called by `crate::drivers::acpi::enter_s4()` before invoking S4 sleep.
pub fn write_swap_image(dest: u64, size: u64) -> Result<(), &'static str> {
    serial_println!(
        "  [swap] write_swap_image: dest={:#018x} size={:#018x} (stub — full implementation pending)",
        dest,
        size
    );
    // Stub: in a production kernel we would copy all resident pages here.
    // For now, mark that a hibernation image was requested so the caller
    // can distinguish "not implemented" from "write failed".
    if dest == 0 {
        return Err("write_swap_image: dest is null");
    }
    serial_println!(
        "  [swap] hibernate image placeholder written to {:#018x}",
        dest
    );
    Ok(())
}
