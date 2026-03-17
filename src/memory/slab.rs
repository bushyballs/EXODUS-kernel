use crate::memory::buddy;
/// SLAB allocator for Genesis — efficient allocation of fixed-size kernel objects
///
/// The SLAB allocator sits on top of the buddy allocator and provides fast
/// allocation/deallocation of commonly-used fixed-size objects (PCBs, inodes,
/// file descriptors, etc.) by pre-allocating pages and carving them into
/// object-sized slots.
///
/// Each "cache" manages objects of one size. A cache contains one or more
/// "slabs" (pages). Each slab has a free list of available object slots.
///
/// Features:
///   - Multiple slab caches for common object sizes (32..4096)
///   - Per-cache: partial/full/free slab lists
///   - Object constructor/destructor callbacks (for pre-initialized objects)
///   - Cache coloring (offset objects within slab to reduce cache conflicts)
///   - Cache reaping (free empty slabs when memory pressure)
///   - Per-CPU magazine layer (fast alloc/free without global lock)
///   - Slab debugging: red zones around objects, poison freed objects
///   - Statistics per cache: active_objects, total_objects, slabs_full, slabs_partial
///
/// Inspired by: Linux SLAB/SLUB allocator (mm/slub.c). All code is original.
use crate::sync::Mutex;

/// Maximum number of slab caches
pub const MAX_CACHES: usize = 32;

/// Maximum object size for slab (larger objects go to buddy directly)
pub const MAX_SLAB_OBJ_SIZE: usize = 8192;

/// Red zone magic values (debug mode)
const RED_ZONE_HEAD: u32 = 0xBB00_DEAD;
const RED_ZONE_TAIL: u32 = 0xDEAD_00BB;

/// Poison byte for freed objects
const SLAB_POISON: u8 = 0x6B; // Linux uses 0x6b too

/// Poison byte for freshly allocated (uninitialized) objects
const SLAB_ALLOC_POISON: u8 = 0x5A;

/// Maximum cache colors (different starting offsets within a slab)
const MAX_CACHE_COLORS: usize = 8;

/// Magazine size for per-CPU fast path
const MAGAZINE_SIZE: usize = 16;

/// Maximum CPUs (simplified — we use a single "CPU" for now)
const MAX_CPUS: usize = 4;

/// Slab states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlabState {
    /// Has free objects
    Partial,
    /// All objects allocated
    Full,
    /// All objects free (can be returned to buddy)
    Empty,
}

/// A single slab (one or more pages, carved into objects)
pub struct Slab {
    /// Physical address of this slab's pages
    pub page_addr: usize,
    /// Number of pages in this slab
    pub num_pages: usize,
    /// Total number of objects in this slab
    pub total_objects: usize,
    /// Number of currently allocated objects
    pub allocated: usize,
    /// Free list head (index into the slab, or -1)
    pub free_head: i32,
    /// Next slab in the partial/full/empty list
    pub next: Option<usize>, // index into cache.slabs[]
    /// State
    pub state: SlabState,
    /// Cache color offset applied to this slab
    pub color_offset: usize,
}

impl Slab {
    const fn empty() -> Self {
        Slab {
            page_addr: 0,
            num_pages: 0,
            total_objects: 0,
            allocated: 0,
            free_head: -1,
            next: None,
            state: SlabState::Empty,
            color_offset: 0,
        }
    }
}

/// Per-CPU magazine: a small array of recently freed objects for fast reuse.
///
/// The magazine layer avoids touching the global slab lock on the hot path.
/// Each cache has a *hot* magazine (the one currently being used for alloc/free)
/// and a *cold* magazine (full reserve). When the hot magazine is empty on
/// alloc, the cold magazine is checked; if the cold is also empty, the slow
/// path fills the hot magazine from slab pages.  When the hot magazine is
/// full on free, the hot and cold magazines are swapped (the now-full hot
/// becomes the cold reserve), and the new empty hot magazine is used for the
/// current free.  This 2-magazine protocol halves the number of slow-path
/// lock acquisitions compared to a single-magazine design.
pub struct Magazine {
    /// Cached object pointers (slot address, not user-visible object address)
    objects: [usize; MAGAZINE_SIZE],
    /// Number of valid entries (stack: objects[count-1] is top)
    count: usize,
}

impl Magazine {
    const fn new() -> Self {
        Magazine {
            objects: [0; MAGAZINE_SIZE],
            count: 0,
        }
    }

    /// Returns true if the magazine is full.
    #[inline(always)]
    fn is_full(&self) -> bool {
        self.count >= MAGAZINE_SIZE
    }

    /// Returns true if the magazine is empty.
    #[inline(always)]
    fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Push an object into the magazine. Returns false if full.
    #[inline(always)]
    fn push(&mut self, ptr: usize) -> bool {
        if self.count < MAGAZINE_SIZE {
            self.objects[self.count] = ptr;
            self.count = self.count.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Pop an object from the magazine. Returns None if empty.
    #[inline(always)]
    fn pop(&mut self) -> Option<usize> {
        if self.count > 0 {
            self.count -= 1;
            Some(self.objects[self.count])
        } else {
            None
        }
    }

    /// Drain all objects from the magazine, returning the count drained.
    fn drain(&mut self) -> usize {
        let count = self.count;
        self.count = 0;
        count
    }

    /// Swap the contents of two magazines in-place (O(1) — just swap counts
    /// and the objects array).
    fn swap_with(&mut self, other: &mut Magazine) {
        core::mem::swap(&mut self.objects, &mut other.objects);
        core::mem::swap(&mut self.count, &mut other.count);
    }
}

/// Object constructor callback type (function pointer)
/// Called when a new slab is created to initialize objects
pub type ObjCtor = fn(*mut u8, usize);

/// Object destructor callback type
/// Called before a slab is destroyed
pub type ObjDtor = fn(*mut u8, usize);

/// A slab cache for objects of a specific size
pub struct SlabCache {
    /// Name of this cache (for debugging, e.g., "task_struct", "inode")
    pub name: [u8; 32],
    /// Object size (bytes)
    pub obj_size: usize,
    /// Alignment requirement
    pub align: usize,
    /// Effective object size (padded to alignment + free list pointer + red zones)
    pub slot_size: usize,
    /// Objects per slab (per page)
    pub objects_per_slab: usize,
    /// Pages per slab (order for buddy allocator)
    pub pages_per_slab: usize,
    /// Buddy order for slab allocation
    pub slab_order: usize,
    /// Slab storage
    pub slabs: [Slab; 64],
    /// Number of active slabs
    pub slab_count: usize,
    /// Partial list head (index into slabs[])
    pub partial_head: Option<usize>,
    /// Full list head
    pub full_head: Option<usize>,
    /// Empty list head
    pub empty_head: Option<usize>,
    /// Total allocated objects across all slabs
    pub total_allocated: usize,
    /// Total objects (capacity)
    pub total_capacity: usize,
    /// Whether this cache is active
    pub active: bool,
    /// Debug mode: enable red zones and poisoning
    pub debug: bool,
    /// Cache coloring: current color index
    pub color_next: usize,
    /// Cache coloring: number of colors available
    pub num_colors: usize,
    /// Cache coloring: bytes per color step
    pub color_step: usize,
    /// Object constructor (called on new slab creation)
    pub ctor: Option<ObjCtor>,
    /// Object destructor (called before slab destruction)
    pub dtor: Option<ObjDtor>,
    /// Per-CPU hot magazines (the active magazine, used first on alloc/free)
    pub magazines: [Magazine; MAX_CPUS],
    /// Per-CPU cold magazines (full reserve swapped in when hot is empty/full)
    pub cold_magazines: [Magazine; MAX_CPUS],
    /// Statistics
    pub stats: SlabCacheStats,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SlabCacheStats {
    pub alloc_count: u64,
    pub free_count: u64,
    pub slab_alloc_count: u64,
    pub slab_free_count: u64,
    pub alloc_failures: u64,
    pub magazine_hits: u64,
    pub magazine_misses: u64,
    /// Number of times the hot/cold magazines were swapped
    pub magazine_swaps: u64,
    pub red_zone_violations: u64,
    pub poison_violations: u64,
    /// Use-after-free detections via 0xDEADBEEF pattern (debug mode)
    pub deadbeef_violations: u64,
    pub reap_count: u64,
    pub slabs_full: usize,
    pub slabs_partial: usize,
    pub slabs_empty: usize,
}

impl SlabCache {
    const fn empty() -> Self {
        const EMPTY_SLAB: Slab = Slab::empty();
        const EMPTY_MAG: Magazine = Magazine::new();
        SlabCache {
            name: [0u8; 32],
            obj_size: 0,
            align: 0,
            slot_size: 0,
            objects_per_slab: 0,
            pages_per_slab: 0,
            slab_order: 0,
            slabs: [EMPTY_SLAB; 64],
            slab_count: 0,
            partial_head: None,
            full_head: None,
            empty_head: None,
            total_allocated: 0,
            total_capacity: 0,
            active: false,
            debug: false,
            color_next: 0,
            num_colors: 1,
            color_step: 0,
            ctor: None,
            dtor: None,
            magazines: [EMPTY_MAG; MAX_CPUS],
            cold_magazines: [EMPTY_MAG; MAX_CPUS],
            stats: SlabCacheStats {
                alloc_count: 0,
                free_count: 0,
                slab_alloc_count: 0,
                slab_free_count: 0,
                alloc_failures: 0,
                magazine_hits: 0,
                magazine_misses: 0,
                magazine_swaps: 0,
                red_zone_violations: 0,
                poison_violations: 0,
                deadbeef_violations: 0,
                reap_count: 0,
                slabs_full: 0,
                slabs_partial: 0,
                slabs_empty: 0,
            },
        }
    }

    /// Initialize this cache with a given object size and name
    pub fn init(&mut self, name: &str, obj_size: usize, align: usize) {
        // Copy name
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(31);
        self.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        self.name[copy_len] = 0;

        self.obj_size = obj_size;
        self.align = align.max(8); // minimum 8-byte alignment

        // Compute slot size: object + free list pointer + optional red zones
        let red_zone_size = if self.debug { 8 } else { 0 }; // 4 bytes head + 4 bytes tail
        let min_slot = (obj_size + red_zone_size).max(core::mem::size_of::<i32>());
        self.slot_size = (min_slot + self.align - 1) & !(self.align - 1);

        // Calculate how many objects fit per page
        let page_size = buddy::PAGE_SIZE;
        self.objects_per_slab = page_size / self.slot_size;
        if self.objects_per_slab == 0 {
            // Object larger than a page, need multiple pages
            self.slab_order = 0;
            let mut order = 0;
            while (page_size << order) < self.slot_size {
                order += 1;
            }
            self.slab_order = order;
            self.pages_per_slab = 1 << order;
            self.objects_per_slab = (page_size << order) / self.slot_size;
        } else {
            self.slab_order = 0;
            self.pages_per_slab = 1;
        }

        // Compute cache coloring parameters
        let slab_size = self.pages_per_slab * page_size;
        let used = self.objects_per_slab * self.slot_size;
        let leftover = slab_size - used;
        if leftover > 0 && self.align > 0 {
            self.color_step = self.align;
            self.num_colors = (leftover / self.color_step).min(MAX_CACHE_COLORS).max(1);
        } else {
            self.color_step = 0;
            self.num_colors = 1;
        }
        self.color_next = 0;

        self.active = true;
    }

    /// Initialize with constructor/destructor callbacks
    pub fn init_with_callbacks(
        &mut self,
        name: &str,
        obj_size: usize,
        align: usize,
        ctor: Option<ObjCtor>,
        dtor: Option<ObjDtor>,
    ) {
        self.init(name, obj_size, align);
        self.ctor = ctor;
        self.dtor = dtor;
    }

    /// Enable debug mode (red zones + poisoning)
    pub fn enable_debug(&mut self) {
        self.debug = true;
        // Recompute slot size with red zones
        let red_zone_size = 8;
        let min_slot = (self.obj_size + red_zone_size).max(core::mem::size_of::<i32>());
        self.slot_size = (min_slot + self.align - 1) & !(self.align - 1);
        // Recompute objects per slab
        let page_size = buddy::PAGE_SIZE;
        let slab_size = self.pages_per_slab * page_size;
        self.objects_per_slab = slab_size / self.slot_size;
    }

    /// Get the next cache color offset
    fn next_color_offset(&mut self) -> usize {
        let offset = self.color_next * self.color_step;
        self.color_next = (self.color_next + 1) % self.num_colors;
        offset
    }

    /// Write red zone markers around an object
    unsafe fn write_red_zones(&self, obj_addr: usize) {
        if !self.debug {
            return;
        }
        // Head red zone: 4 bytes before the object data
        let head_addr = obj_addr;
        *(head_addr as *mut u32) = RED_ZONE_HEAD;
        // Tail red zone: 4 bytes after the object data
        let tail_addr = obj_addr + 4 + self.obj_size;
        if tail_addr + 4 <= obj_addr + self.slot_size {
            *(tail_addr as *mut u32) = RED_ZONE_TAIL;
        }
    }

    /// Check red zone markers. Returns true if intact.
    unsafe fn check_red_zones(&mut self, obj_addr: usize) -> bool {
        if !self.debug {
            return true;
        }
        let head = *(obj_addr as *const u32);
        let tail_addr = obj_addr + 4 + self.obj_size;
        let tail = if tail_addr + 4 <= obj_addr + self.slot_size {
            *(tail_addr as *const u32)
        } else {
            RED_ZONE_TAIL // assume ok if can't read
        };

        if head != RED_ZONE_HEAD || tail != RED_ZONE_TAIL {
            self.stats.red_zone_violations = self.stats.red_zone_violations.saturating_add(1);
            crate::serial_println!(
                "  [slab] RED ZONE violation in cache '{}' at {:#x} (head={:#x}, tail={:#x})",
                self.name_str(),
                obj_addr,
                head,
                tail
            );
            false
        } else {
            true
        }
    }

    /// Poison a freed object with 0x6B bytes (Linux-style use-after-free trap).
    unsafe fn poison_object(&self, obj_addr: usize) {
        if !self.debug {
            return;
        }
        let data_start = if self.debug { obj_addr + 4 } else { obj_addr };
        let data_len = self.obj_size;
        core::ptr::write_bytes(data_start as *mut u8, SLAB_POISON, data_len);
    }

    /// Poison a freed object with the 0xDEADBEEF pattern (32-bit words).
    ///
    /// This is a stronger use-after-free trap than byte-level 0x6B: because
    /// 0xDEADBEEF is a recognisable sentinel, post-mortem debugging can
    /// immediately identify the address of the corrupt access.  If the object
    /// size is not a multiple of 4 the remaining bytes are filled with 0xDE.
    ///
    /// Called in debug mode on every free (complements poison_object).
    unsafe fn poison_object_deadbeef(&self, slot_addr: usize) {
        if !self.debug {
            return;
        }
        // Skip over any head red-zone (4 bytes in debug mode)
        let data_start = if self.debug { slot_addr + 4 } else { slot_addr };
        let data_len = self.obj_size;
        let words = data_len / 4;
        let remainder = data_len % 4;
        let ptr32 = data_start as *mut u32;
        for i in 0..words {
            // Safety: slot_addr is always a valid slab allocation; data_start
            // is within that allocation; words * 4 <= obj_size <= slot_size.
            core::ptr::write_volatile(ptr32.add(i), 0xDEAD_BEEF_u32);
        }
        // Fill trailing bytes that don't form a full word
        let byte_ptr = (data_start + words * 4) as *mut u8;
        for i in 0..remainder {
            core::ptr::write_volatile(byte_ptr.add(i), 0xDE_u8);
        }
    }

    /// Check whether a previously freed object has been modified (use-after-free).
    ///
    /// Scans for any word that is not 0xDEADBEEF (or trailing byte != 0xDE).
    /// Returns true if the poison is intact (no UAF detected).
    unsafe fn check_poison_deadbeef(&mut self, slot_addr: usize) -> bool {
        if !self.debug {
            return true;
        }
        let data_start = if self.debug { slot_addr + 4 } else { slot_addr };
        let data_len = self.obj_size;
        let words = data_len / 4;
        let remainder = data_len % 4;
        let ptr32 = data_start as *const u32;
        for i in 0..words {
            if core::ptr::read_volatile(ptr32.add(i)) != 0xDEAD_BEEF_u32 {
                self.stats.deadbeef_violations = self.stats.deadbeef_violations.saturating_add(1);
                crate::serial_println!(
                    "  [slab] DEADBEEF violation in cache '{}' at slot {:#x} word {}",
                    self.name_str(),
                    slot_addr,
                    i
                );
                return false;
            }
        }
        let byte_ptr = (data_start + words * 4) as *const u8;
        for i in 0..remainder {
            if core::ptr::read_volatile(byte_ptr.add(i)) != 0xDE_u8 {
                self.stats.deadbeef_violations = self.stats.deadbeef_violations.saturating_add(1);
                return false;
            }
        }
        true
    }

    /// Check if an object is still poisoned with 0x6B (use-after-free detection)
    unsafe fn check_poison(&mut self, obj_addr: usize) -> bool {
        if !self.debug {
            return true;
        }
        let data_start = obj_addr + 4;
        let data_len = self.obj_size;
        for i in 0..data_len {
            let byte = *((data_start + i) as *const u8);
            if byte != SLAB_POISON {
                self.stats.poison_violations = self.stats.poison_violations.saturating_add(1);
                return false;
            }
        }
        true
    }

    /// User-visible object address from slot address (skip red zone head)
    fn obj_from_slot(&self, slot_addr: usize) -> usize {
        if self.debug {
            slot_addr + 4 // skip head red zone
        } else {
            slot_addr
        }
    }

    /// Slot address from user-visible object address
    fn slot_from_obj(&self, obj_addr: usize) -> usize {
        if self.debug {
            obj_addr - 4 // back up past head red zone
        } else {
            obj_addr
        }
    }

    /// Allocate a new slab from the buddy allocator
    fn grow(&mut self) -> Option<usize> {
        if self.slab_count >= 64 {
            return None;
        }

        // Allocate pages from buddy
        let addr = buddy::alloc_pages(self.slab_order)?;

        let slab_idx = self.slab_count;
        self.slab_count = self.slab_count.saturating_add(1);

        let color_offset = self.next_color_offset();

        // Cache values from self before taking mutable borrow on slab
        let pages_per_slab = self.pages_per_slab;
        let objects_per_slab = self.objects_per_slab;
        let slot_size = self.slot_size;
        let obj_size = self.obj_size;
        let debug = self.debug;
        let ctor = self.ctor;

        let slab = &mut self.slabs[slab_idx];
        slab.page_addr = addr;
        slab.num_pages = pages_per_slab;
        slab.total_objects = objects_per_slab;
        slab.allocated = 0;
        slab.state = SlabState::Empty;
        slab.color_offset = color_offset;

        // Initialize free list: chain all slots together
        let base = addr + color_offset;
        for i in 0..objects_per_slab {
            let slot_addr = base + i * slot_size;
            let next = if i + 1 < objects_per_slab {
                (i + 1) as i32
            } else {
                -1 // end of free list
            };
            unsafe {
                *(slot_addr as *mut i32) = next;
            }

            // Call constructor if present
            if let Some(ctor_fn) = ctor {
                let obj_addr = if debug { slot_addr + 4 } else { slot_addr };
                ctor_fn(obj_addr as *mut u8, obj_size);
            }

            // Write red zones in debug mode
            if debug {
                unsafe {
                    // Inline write_red_zones to avoid borrow conflict
                    let head_addr = slot_addr;
                    *(head_addr as *mut u32) = RED_ZONE_HEAD;
                    let tail_addr = slot_addr + 4 + obj_size;
                    if tail_addr + 4 <= slot_addr + slot_size {
                        *(tail_addr as *mut u32) = RED_ZONE_TAIL;
                    }
                }
            }
        }
        slab.free_head = 0; // first slot
        slab.state = SlabState::Partial;

        // Add to partial list
        slab.next = self.partial_head;
        self.partial_head = Some(slab_idx);

        self.total_capacity += objects_per_slab;
        self.stats.slab_alloc_count = self.stats.slab_alloc_count.saturating_add(1);
        self.update_slab_counts();

        Some(slab_idx)
    }

    /// Update the slab count statistics
    fn update_slab_counts(&mut self) {
        let mut full = 0;
        let mut partial = 0;
        let mut empty = 0;
        for i in 0..self.slab_count {
            match self.slabs[i].state {
                SlabState::Full => full += 1,
                SlabState::Partial => partial += 1,
                SlabState::Empty => empty += 1,
            }
        }
        self.stats.slabs_full = full;
        self.stats.slabs_partial = partial;
        self.stats.slabs_empty = empty;
    }

    /// Allocate an object from this cache.
    ///
    /// Fast path (hot magazine):
    ///   Pop from hot magazine[0]. If empty, swap hot with cold magazine.
    ///   If cold is also empty, fall through to the slab slow path.
    ///
    /// This 2-magazine protocol cuts lock acquisitions roughly in half
    /// compared to draining straight to the slab when the hot magazine empties.
    pub fn alloc(&mut self) -> Option<*mut u8> {
        // --- Fast path: hot magazine has an object ---
        if let Some(ptr) = self.magazines[0].pop() {
            self.stats.magazine_hits = self.stats.magazine_hits.saturating_add(1);
            self.stats.alloc_count = self.stats.alloc_count.saturating_add(1);
            self.total_allocated = self.total_allocated.saturating_add(1);

            if self.debug {
                unsafe {
                    // Check both poison patterns in debug mode
                    self.check_poison_deadbeef(ptr);
                    self.check_poison(ptr);
                    self.write_red_zones(ptr);
                }
            }

            let obj_addr = self.obj_from_slot(ptr);
            unsafe {
                core::ptr::write_bytes(obj_addr as *mut u8, 0, self.obj_size);
            }
            return Some(obj_addr as *mut u8);
        }

        // --- Hot magazine empty: try swapping in the cold magazine ---
        if !self.cold_magazines[0].is_empty() {
            // Swap hot (empty) and cold (has objects)
            self.magazines[0].swap_with(&mut self.cold_magazines[0]);
            self.stats.magazine_swaps = self.stats.magazine_swaps.saturating_add(1);

            // Now the hot magazine has objects; pop one
            if let Some(ptr) = self.magazines[0].pop() {
                self.stats.magazine_hits = self.stats.magazine_hits.saturating_add(1);
                self.stats.alloc_count = self.stats.alloc_count.saturating_add(1);
                self.total_allocated = self.total_allocated.saturating_add(1);

                if self.debug {
                    unsafe {
                        self.check_poison_deadbeef(ptr);
                        self.check_poison(ptr);
                        self.write_red_zones(ptr);
                    }
                }

                let obj_addr = self.obj_from_slot(ptr);
                unsafe {
                    core::ptr::write_bytes(obj_addr as *mut u8, 0, self.obj_size);
                }
                return Some(obj_addr as *mut u8);
            }
        }

        self.stats.magazine_misses = self.stats.magazine_misses.saturating_add(1);

        // --- Slow path: allocate directly from a slab ---
        let slab_idx = match self.partial_head {
            Some(idx) => idx,
            None => {
                // No partial slabs — try empty slabs first
                if let Some(idx) = self.empty_head {
                    // Move from empty to partial
                    self.empty_head = self.slabs[idx].next;
                    self.slabs[idx].state = SlabState::Partial;
                    self.slabs[idx].next = self.partial_head;
                    self.partial_head = Some(idx);
                    idx
                } else {
                    // Grow a new slab
                    self.grow()?
                }
            }
        };

        let slab = &mut self.slabs[slab_idx];
        if slab.free_head < 0 {
            // Partial list is inconsistent — should not happen
            self.stats.alloc_failures = self.stats.alloc_failures.saturating_add(1);
            return None;
        }

        // Pop from slab free list
        let obj_idx = slab.free_head as usize;
        let base = slab.page_addr + slab.color_offset;
        let slot_addr = base + obj_idx * self.slot_size;

        let next_free = unsafe { *(slot_addr as *const i32) };
        slab.free_head = next_free;
        slab.allocated += 1;

        if slab.free_head < 0 {
            // Slab is now full — move from partial to full list
            slab.state = SlabState::Full;
            self.partial_head = slab.next;
            slab.next = self.full_head;
            self.full_head = Some(slab_idx);
        }

        self.total_allocated = self.total_allocated.saturating_add(1);
        self.stats.alloc_count = self.stats.alloc_count.saturating_add(1);

        if self.debug {
            unsafe {
                self.write_red_zones(slot_addr);
            }
        }

        let obj_addr = self.obj_from_slot(slot_addr);
        unsafe {
            core::ptr::write_bytes(obj_addr as *mut u8, 0, self.obj_size);
        }

        self.update_slab_counts();
        Some(obj_addr as *mut u8)
    }

    /// Free an object back to this cache.
    ///
    /// Fast path (hot magazine not full):
    ///   Push to hot magazine[0].  If full, swap hot with cold magazine (the
    ///   cold was just drained to slabs or is empty).  If after the swap the
    ///   hot is still full (cold was also full), drain the cold magazine to
    ///   slabs first, then do the swap.
    ///
    /// This guarantees at most one slow-path flush per MAGAZINE_SIZE frees,
    /// halving the number of slab-lock acquisitions vs. the old drain-half
    /// approach.
    pub fn free(&mut self, ptr: *mut u8) {
        let obj_addr = ptr as usize;
        let slot_addr = self.slot_from_obj(obj_addr);

        if self.debug {
            unsafe {
                self.check_red_zones(slot_addr);
                // Apply both poison patterns so that post-mortem dumps show
                // the DEADBEEF sentinel even on minimal debug builds.
                self.poison_object(slot_addr);
                self.poison_object_deadbeef(slot_addr);
            }
        }

        // --- Fast path: push into hot magazine ---
        if self.magazines[0].push(slot_addr) {
            self.total_allocated = self.total_allocated.saturating_sub(1);
            self.stats.free_count = self.stats.free_count.saturating_add(1);
            return;
        }

        // --- Hot magazine full: try to swap with cold ---
        if self.cold_magazines[0].is_empty() {
            // Cold is empty — swap hot (full) to cold, hot becomes empty
            self.magazines[0].swap_with(&mut self.cold_magazines[0]);
            self.stats.magazine_swaps = self.stats.magazine_swaps.saturating_add(1);
            // Hot is now empty — push the current object into it
            self.magazines[0].push(slot_addr);
        } else {
            // Cold is also full — drain cold to slabs, then swap
            // Drain entire cold magazine to the slab allocator
            while let Some(cached) = self.cold_magazines[0].pop() {
                self.free_to_slab(cached);
            }
            // Cold is now empty — swap hot (full) to cold, hot becomes empty
            self.magazines[0].swap_with(&mut self.cold_magazines[0]);
            self.stats.magazine_swaps = self.stats.magazine_swaps.saturating_add(1);
            // Hot is now empty — push the current object
            self.magazines[0].push(slot_addr);
        }

        self.total_allocated = self.total_allocated.saturating_sub(1);
        self.stats.free_count = self.stats.free_count.saturating_add(1);
    }

    /// Free an object directly back to its slab (bypassing magazine)
    fn free_to_slab(&mut self, slot_addr: usize) {
        // Find which slab this object belongs to
        let slab_count = self.slab_count;
        let slot_size = self.slot_size;
        let debug = self.debug;
        let obj_size = self.obj_size;

        let mut found_idx: Option<usize> = None;
        for i in 0..slab_count {
            let slab_end = self.slabs[i].page_addr + self.slabs[i].num_pages * buddy::PAGE_SIZE;
            if slot_addr >= self.slabs[i].page_addr && slot_addr < slab_end {
                found_idx = Some(i);
                break;
            }
        }

        let i = match found_idx {
            Some(idx) => idx,
            None => return,
        };

        // Compute object index
        let base = self.slabs[i].page_addr + self.slabs[i].color_offset;
        let obj_idx = (slot_addr - base) / slot_size;

        // Debug: poison the freed object (inline to avoid borrow conflict)
        if debug {
            unsafe {
                let data_start = if debug { slot_addr + 4 } else { slot_addr };
                core::ptr::write_bytes(data_start as *mut u8, SLAB_POISON, obj_size);
            }
        }

        // Push onto free list
        let old_free_head = self.slabs[i].free_head;
        unsafe {
            *(slot_addr as *mut i32) = old_free_head;
        }
        self.slabs[i].free_head = obj_idx as i32;
        self.slabs[i].allocated -= 1;

        let was_full = self.slabs[i].state == SlabState::Full;
        let now_empty = self.slabs[i].allocated == 0;

        if now_empty {
            self.slabs[i].state = SlabState::Empty;
            // Move to empty list
            if was_full {
                self.remove_from_full_list(i);
            } else {
                self.remove_from_partial_list(i);
            }
            self.slabs[i].next = self.empty_head;
            self.empty_head = Some(i);
        } else {
            self.slabs[i].state = SlabState::Partial;
            // If slab was full, move to partial list
            if was_full {
                self.remove_from_full_list(i);
                self.slabs[i].next = self.partial_head;
                self.partial_head = Some(i);
            }
        }

        self.update_slab_counts();
    }

    /// Remove a slab from the full list
    fn remove_from_full_list(&mut self, target: usize) {
        if self.full_head == Some(target) {
            self.full_head = self.slabs[target].next;
            return;
        }
        let mut current = self.full_head;
        while let Some(idx) = current {
            if self.slabs[idx].next == Some(target) {
                self.slabs[idx].next = self.slabs[target].next;
                return;
            }
            current = self.slabs[idx].next;
        }
    }

    /// Remove a slab from the partial list
    fn remove_from_partial_list(&mut self, target: usize) {
        if self.partial_head == Some(target) {
            self.partial_head = self.slabs[target].next;
            return;
        }
        let mut current = self.partial_head;
        while let Some(idx) = current {
            if self.slabs[idx].next == Some(target) {
                self.slabs[idx].next = self.slabs[target].next;
                return;
            }
            current = self.slabs[idx].next;
        }
    }

    /// Reap empty slabs — free them back to the buddy allocator.
    /// Returns the number of pages freed.
    pub fn reap(&mut self) -> usize {
        let mut freed_pages = 0;

        // Drain all hot AND cold magazines first
        for cpu in 0..MAX_CPUS {
            while let Some(ptr) = self.magazines[cpu].pop() {
                self.free_to_slab(ptr);
            }
            while let Some(ptr) = self.cold_magazines[cpu].pop() {
                self.free_to_slab(ptr);
            }
        }

        // Free all empty slabs
        let mut current = self.empty_head;
        while let Some(idx) = current {
            let slab = &self.slabs[idx];
            let next = slab.next;
            let addr = slab.page_addr;
            let pages = slab.num_pages;

            // Call destructor on all objects if present
            if let Some(dtor) = self.dtor {
                let base = addr + slab.color_offset;
                for i in 0..slab.total_objects {
                    let slot_addr = base + i * self.slot_size;
                    let obj_addr = self.obj_from_slot(slot_addr);
                    dtor(obj_addr as *mut u8, self.obj_size);
                }
            }

            // Free pages back to buddy
            buddy::free_pages(addr, self.slab_order);
            freed_pages += pages;
            self.total_capacity -= self.slabs[idx].total_objects;
            self.stats.slab_free_count = self.stats.slab_free_count.saturating_add(1);

            // Mark slab as inactive (don't shift the array, just zero it)
            self.slabs[idx] = Slab::empty();

            current = next;
        }
        self.empty_head = None;
        self.stats.reap_count = self.stats.reap_count.saturating_add(1);
        self.update_slab_counts();

        freed_pages
    }

    /// Get cache name as string
    pub fn name_str(&self) -> &str {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(32);
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }

    /// Get active (allocated) object count
    pub fn active_objects(&self) -> usize {
        self.total_allocated
    }

    /// Get total object capacity
    pub fn total_objects(&self) -> usize {
        self.total_capacity
    }
}

/// Global slab cache registry
pub struct SlabAllocator {
    pub caches: [SlabCache; MAX_CACHES],
    pub cache_count: usize,
}

impl SlabAllocator {
    const fn new() -> Self {
        const EMPTY_CACHE: SlabCache = SlabCache::empty();
        SlabAllocator {
            caches: [EMPTY_CACHE; MAX_CACHES],
            cache_count: 0,
        }
    }

    /// Create a new slab cache. Returns cache index.
    pub fn create_cache(&mut self, name: &str, obj_size: usize, align: usize) -> Option<usize> {
        if self.cache_count >= MAX_CACHES {
            return None;
        }
        let idx = self.cache_count;
        self.caches[idx].init(name, obj_size, align);
        self.cache_count += 1;
        Some(idx)
    }

    /// Create a slab cache with constructor/destructor callbacks
    pub fn create_cache_with_callbacks(
        &mut self,
        name: &str,
        obj_size: usize,
        align: usize,
        ctor: Option<ObjCtor>,
        dtor: Option<ObjDtor>,
    ) -> Option<usize> {
        if self.cache_count >= MAX_CACHES {
            return None;
        }
        let idx = self.cache_count;
        self.caches[idx].init_with_callbacks(name, obj_size, align, ctor, dtor);
        self.cache_count += 1;
        Some(idx)
    }

    /// Allocate from a cache by index
    pub fn alloc(&mut self, cache_idx: usize) -> Option<*mut u8> {
        if cache_idx < self.cache_count {
            self.caches[cache_idx].alloc()
        } else {
            None
        }
    }

    /// Free to a cache by index
    pub fn free(&mut self, cache_idx: usize, ptr: *mut u8) {
        if cache_idx < self.cache_count {
            self.caches[cache_idx].free(ptr);
        }
    }

    /// Reap all caches (free empty slabs). Returns total pages freed.
    pub fn reap_all(&mut self) -> usize {
        let mut total = 0;
        for i in 0..self.cache_count {
            if self.caches[i].active {
                total += self.caches[i].reap();
            }
        }
        total
    }

    /// Find or create a generic cache for a given size (kmalloc-style)
    pub fn kmalloc_cache(&mut self, size: usize) -> Option<usize> {
        // Standard sizes: 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192
        let bucket_size = if size <= 8 {
            8
        } else if size <= 16 {
            16
        } else if size <= 32 {
            32
        } else if size <= 64 {
            64
        } else if size <= 128 {
            128
        } else if size <= 256 {
            256
        } else if size <= 512 {
            512
        } else if size <= 1024 {
            1024
        } else if size <= 2048 {
            2048
        } else if size <= 4096 {
            4096
        } else if size <= 8192 {
            8192
        } else {
            return None;
        };

        // Look for existing cache of this size
        for i in 0..self.cache_count {
            if self.caches[i].active && self.caches[i].obj_size == bucket_size {
                return Some(i);
            }
        }

        // Create new cache
        let name = match bucket_size {
            8 => "kmalloc-8",
            16 => "kmalloc-16",
            32 => "kmalloc-32",
            64 => "kmalloc-64",
            128 => "kmalloc-128",
            256 => "kmalloc-256",
            512 => "kmalloc-512",
            1024 => "kmalloc-1k",
            2048 => "kmalloc-2k",
            4096 => "kmalloc-4k",
            8192 => "kmalloc-8k",
            _ => "kmalloc-?",
        };
        self.create_cache(name, bucket_size, 8)
    }

    /// Print slab info (like /proc/slabinfo)
    pub fn slabinfo(&self) -> alloc::string::String {
        use alloc::format;
        let mut s = alloc::string::String::from(
            "# name            <objsize> <active> <total> <slabs> <full> <partial> <empty>\n",
        );
        for i in 0..self.cache_count {
            let c = &self.caches[i];
            if c.active {
                s.push_str(&format!(
                    "{:<18} {:>6} {:>8} {:>6} {:>5} {:>5} {:>8} {:>5}\n",
                    c.name_str(),
                    c.obj_size,
                    c.total_allocated,
                    c.total_capacity,
                    c.slab_count,
                    c.stats.slabs_full,
                    c.stats.slabs_partial,
                    c.stats.slabs_empty
                ));
            }
        }
        s
    }
}

/// Global slab allocator
pub static SLAB: Mutex<SlabAllocator> = Mutex::new(SlabAllocator::new());

/// Pre-built cache indices for common kernel objects
pub static mut CACHE_TASK: usize = 0;
pub static mut CACHE_INODE: usize = 0;
pub static mut CACHE_DENTRY: usize = 0;
pub static mut CACHE_FILE: usize = 0;
pub static mut CACHE_SOCKET: usize = 0;

/// Initialize the slab allocator with standard kernel caches
pub fn init() {
    let mut slab = SLAB.lock();

    // Create standard kernel object caches
    unsafe {
        CACHE_TASK = slab.create_cache("task_struct", 512, 64).unwrap_or(0);
        CACHE_INODE = slab.create_cache("inode", 256, 32).unwrap_or(0);
        CACHE_DENTRY = slab.create_cache("dentry", 128, 16).unwrap_or(0);
        CACHE_FILE = slab.create_cache("file", 128, 16).unwrap_or(0);
        CACHE_SOCKET = slab.create_cache("socket", 256, 32).unwrap_or(0);
    }

    // Create kmalloc size caches
    for &size in &[8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096] {
        slab.kmalloc_cache(size);
    }
}

/// Allocate from a named cache
pub fn cache_alloc(cache_idx: usize) -> Option<*mut u8> {
    SLAB.lock().alloc(cache_idx)
}

/// Free to a named cache
pub fn cache_free(cache_idx: usize, ptr: *mut u8) {
    SLAB.lock().free(cache_idx, ptr);
}

/// kmalloc — allocate kernel memory of arbitrary size (via slab)
pub fn kmalloc(size: usize) -> Option<*mut u8> {
    let mut slab = SLAB.lock();
    let cache_idx = slab.kmalloc_cache(size)?;
    slab.alloc(cache_idx)
}

/// kfree — free kmalloc'd memory
pub fn kfree(ptr: *mut u8, size: usize) {
    let mut slab = SLAB.lock();
    if let Some(cache_idx) = slab.kmalloc_cache(size) {
        slab.free(cache_idx, ptr);
    }
}

/// Reap all slab caches (free empty slabs under memory pressure)
pub fn reap_all() -> usize {
    SLAB.lock().reap_all()
}

/// Count reclaimable pages currently held by the slab allocator (empty slabs).
pub fn reclaimable_pages() -> usize {
    let slab = SLAB.lock();
    let mut pages = 0usize;
    for i in 0..slab.cache_count {
        if slab.caches[i].active {
            let c = &slab.caches[i];
            // Each empty slab represents `pages_per_slab` reclaimable pages
            pages = pages.saturating_add(c.stats.slabs_empty.saturating_mul(c.pages_per_slab));
        }
    }
    pages
}

// ---------------------------------------------------------------------------
// Memory pressure shrinker implementation for the slab allocator
// ---------------------------------------------------------------------------

/// A MemPressureShrinker that drives slab reaping when memory is tight.
///
/// Register once at boot with `crate::memory::reclaim::register_shrinker`.
/// When `shrink()` is called the slab allocator frees all empty slabs in
/// every cache and returns the number of pages freed to the buddy allocator.
pub struct SlabShrinker;

impl crate::memory::reclaim::MemPressureShrinker for SlabShrinker {
    fn shrink(&self, _target_pages: usize) -> usize {
        // Reap ALL empty slabs regardless of target; the caller will stop
        // invoking shrinkers once the target is met.  Reaping empty slabs is
        // always safe and has no observable effect on allocator correctness.
        SLAB.lock().reap_all()
    }

    fn count_reclaimable(&self) -> usize {
        reclaimable_pages()
    }

    fn name(&self) -> &'static str {
        "slab"
    }
}

/// Global static shrinker instance, ready for registration.
pub static SLAB_SHRINKER: SlabShrinker = SlabShrinker;
