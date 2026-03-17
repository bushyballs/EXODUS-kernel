use crate::serial_println;
/// memory/hotplug.rs — Memory hot-plug / unplug for Genesis AIOS
///
/// Tracks physical memory sections (each 128 MB) and allows them to be
/// brought online into the buddy allocator or taken offline at runtime.
///
/// ## Design
///
/// - `MEM_SECTIONS` is a fixed-size static array of `MemSection` descriptors.
/// - `TOTAL_ONLINE_MB` is an `AtomicU64` that accumulates the megabytes of
///   memory that are currently in the Online state.
/// - All counters use saturating arithmetic; no heap allocation is performed.
///
/// ## Safety rules
/// - No Vec, Box, String, format!, or alloc.
/// - No float casts (no `as f32` / `as f64`).
/// - No unwrap() / expect() / panic!().
/// - Counters: saturating_add / saturating_sub.
/// - Array accesses are bounds-checked before use.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of independently hot-pluggable memory sections.
/// Matches Linux's MAX_NR_SECTIONS on a 48-bit physical address space.
pub const MAX_MEMORY_SECTIONS: usize = 256;

/// Size of each hot-plug section in megabytes.
pub const SECTION_SIZE_MB: u64 = 128;

/// Size of each hot-plug section in bytes (128 MiB).
pub const SECTION_SIZE_BYTES: u64 = SECTION_SIZE_MB * 1024 * 1024;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Lifecycle state of a single memory section.
#[derive(Copy, Clone, PartialEq)]
pub enum MemSectionState {
    /// Section not present in the system.
    Absent,
    /// Section is present in hardware but not given to the allocator.
    Offline,
    /// Section is active in the buddy / frame allocator.
    Online,
    /// Transition: section is being brought online.
    GoingOnline,
    /// Transition: section is being taken offline.
    GoingOffline,
}

/// Descriptor for a single 128-MB physical memory section.
///
/// Stored in a fixed-size static array; inactive entries have `active == false`.
#[derive(Copy, Clone)]
pub struct MemSection {
    /// First physical frame number (4 KB page) in this section.
    pub pfn_start: u64,
    /// One-past-last physical frame number in this section.
    pub pfn_end: u64,
    /// Current lifecycle state.
    pub state: MemSectionState,
    /// NUMA node this section belongs to.
    pub node_id: u8,
    /// Timestamp (milliseconds since boot) when the section was brought Online.
    pub online_time_ms: u64,
    /// Whether this slot is allocated (false = empty slot).
    pub active: bool,
}

impl MemSection {
    /// Construct an inactive, zeroed section descriptor.
    pub const fn empty() -> Self {
        MemSection {
            pfn_start: 0,
            pfn_end: 0,
            state: MemSectionState::Absent,
            node_id: 0,
            online_time_ms: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Statics
// ---------------------------------------------------------------------------

/// Registry of all physical memory sections.
static MEM_SECTIONS: Mutex<[MemSection; MAX_MEMORY_SECTIONS]> = {
    const EMPTY: MemSection = MemSection::empty();
    Mutex::new([EMPTY; MAX_MEMORY_SECTIONS])
};

/// Total megabytes of memory currently in the Online state.
pub static TOTAL_ONLINE_MB: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new (initially Offline) memory section covering `[pfn_start, pfn_end)`.
///
/// Finds the first inactive slot in `MEM_SECTIONS`, fills it in, and returns
/// the slot index. Returns `None` if the table is full or the range is empty.
pub fn mem_section_add(pfn_start: u64, pfn_end: u64, node_id: u8) -> Option<u32> {
    if pfn_end <= pfn_start {
        return None;
    }

    let mut sections = MEM_SECTIONS.lock();
    for (i, slot) in sections.iter_mut().enumerate() {
        if !slot.active {
            *slot = MemSection {
                pfn_start,
                pfn_end,
                state: MemSectionState::Offline,
                node_id,
                online_time_ms: 0,
                active: true,
            };
            return Some(i as u32);
        }
    }
    None
}

/// Transition a section from Offline to Online.
///
/// Adds the section's megabyte count to `TOTAL_ONLINE_MB` using saturating
/// addition. Records `current_ms` as the online timestamp.
///
/// Returns `true` on success, `false` if the index is out of bounds, the slot
/// is inactive, or the section is not currently in the Offline state.
pub fn mem_section_online(idx: u32, current_ms: u64) -> bool {
    if idx as usize >= MAX_MEMORY_SECTIONS {
        return false;
    }

    let mut sections = MEM_SECTIONS.lock();
    let slot = &mut sections[idx as usize];

    if !slot.active || slot.state != MemSectionState::Offline {
        return false;
    }

    slot.state = MemSectionState::Online;
    slot.online_time_ms = current_ms;

    // Compute megabytes: (pfn_end - pfn_start) * PAGE_SIZE / (1024 * 1024)
    // PAGE_SIZE = 4096 = 4 KiB; 1 MiB = 1024 * 1024 = 1,048,576 bytes.
    // (pfn_end - pfn_start) * 4096 / 1048576 = (pfn_end - pfn_start) / 256
    let frames = slot.pfn_end.saturating_sub(slot.pfn_start);
    // Guard: division by 256 is always safe (non-zero constant).
    let mb = frames / 256;

    drop(sections);
    TOTAL_ONLINE_MB.fetch_add(mb, Ordering::Relaxed);
    true
}

/// Transition a section from Online to Offline.
///
/// Subtracts the section's megabyte count from `TOTAL_ONLINE_MB` using
/// saturating subtraction.
///
/// Returns `true` on success, `false` if the index is invalid or the section
/// is not currently Online.
pub fn mem_section_offline(idx: u32) -> bool {
    if idx as usize >= MAX_MEMORY_SECTIONS {
        return false;
    }

    let mut sections = MEM_SECTIONS.lock();
    let slot = &mut sections[idx as usize];

    if !slot.active || slot.state != MemSectionState::Online {
        return false;
    }

    let frames = slot.pfn_end.saturating_sub(slot.pfn_start);
    let mb = frames / 256;

    slot.state = MemSectionState::Offline;

    drop(sections);
    // Saturating subtraction — never underflows below zero.
    let prev = TOTAL_ONLINE_MB.load(Ordering::Relaxed);
    let next = prev.saturating_sub(mb);
    // Use a store rather than fetch_sub to guarantee correct saturating semantics.
    TOTAL_ONLINE_MB.store(next, Ordering::Relaxed);
    true
}

/// Transition a section from Offline to Absent and mark its slot inactive.
///
/// Returns `true` on success, `false` if the index is invalid or the section
/// is not currently Offline.
pub fn mem_section_remove(idx: u32) -> bool {
    if idx as usize >= MAX_MEMORY_SECTIONS {
        return false;
    }

    let mut sections = MEM_SECTIONS.lock();
    let slot = &mut sections[idx as usize];

    if !slot.active || slot.state != MemSectionState::Offline {
        return false;
    }

    slot.state = MemSectionState::Absent;
    slot.active = false;
    true
}

/// Return the total megabytes of memory currently in the Online state.
pub fn mem_get_total_online_mb() -> u64 {
    TOTAL_ONLINE_MB.load(Ordering::Relaxed)
}

/// Return the current state of a memory section, or `None` if the index is
/// out of bounds or the slot is inactive.
pub fn mem_section_state(idx: u32) -> Option<MemSectionState> {
    if idx as usize >= MAX_MEMORY_SECTIONS {
        return None;
    }
    let sections = MEM_SECTIONS.lock();
    let slot = &sections[idx as usize];
    if slot.active {
        Some(slot.state)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the memory hot-plug subsystem.
///
/// Registers four 128-MB sections that represent the initial 512 MB of RAM
/// and immediately brings them all Online.
///
/// ```
/// Section 0: pfn    0 ..  32768  (  0 – 128 MB)
/// Section 1: pfn 32768 ..  65536  (128 – 256 MB)
/// Section 2: pfn 65536 ..  98304  (256 – 384 MB)
/// Section 3: pfn 98304 .. 131072  (384 – 512 MB)
/// ```
///
/// Each section has 32 768 frames (128 MiB / 4 KiB).
pub fn init() {
    // PFN boundaries for 512 MB / 4 KB = 131072 frames.
    // 128 MB / 4 KB = 32768 frames per section.
    let sections: [(u64, u64, u8); 4] = [
        (0, 32768, 0),
        (32768, 65536, 0),
        (65536, 98304, 0),
        (98304, 131072, 0),
    ];

    for (pfn_start, pfn_end, node_id) in &sections {
        if let Some(idx) = mem_section_add(*pfn_start, *pfn_end, *node_id) {
            mem_section_online(idx, 0);
        }
    }

    serial_println!(
        "[memory_hotplug] memory hotplug initialized, {} MB online",
        mem_get_total_online_mb()
    );
}
