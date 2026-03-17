use crate::memory::buddy;
/// zram — compressed RAM block device for Genesis
///
/// Provides a compressed in-memory block device that can be used as a swap
/// backend. Pages are compressed with a simple LZ4-lite algorithm before
/// storage, achieving typical 2:1-3:1 compression ratios on kernel data.
///
/// Architecture:
///   - Compressed page pool backed by buddy allocator
///   - LZ4-lite compressor (byte-level LZ77 with 4KB window)
///   - Slot-based storage with variable-size compressed blocks
///   - Integration point for swap subsystem
///   - Per-device statistics (compressed/original sizes, hit rates)
///
/// Inspired by: Linux zram (drivers/block/zram/). All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of zram devices
const MAX_ZRAM_DEVICES: usize = 4;

/// Maximum compressed pages per device
const MAX_COMPRESSED_PAGES: usize = 8192;

/// Page size (must match buddy)
const PAGE_SIZE: usize = 4096;

/// Maximum size of a compressed page (if compression expands, store raw)
const MAX_COMPRESSED_SIZE: usize = PAGE_SIZE;

/// Compression pool: total physical pages reserved for compressed storage
const POOL_PAGES: usize = 2048;

/// LZ4-lite constants
const LZ4_HASH_BITS: usize = 12;
const LZ4_HASH_SIZE: usize = 1 << LZ4_HASH_BITS;
const LZ4_MIN_MATCH: usize = 4;
const LZ4_MAX_OFFSET: usize = PAGE_SIZE;

// ---------------------------------------------------------------------------
// LZ4-lite compressor
// ---------------------------------------------------------------------------

/// Compress `src` into `dst`. Returns compressed length, or 0 if incompressible.
///
/// Format: stream of tokens
///   - Token byte: high nibble = literal length (0-14, 15 = extended),
///                 low nibble  = match length - 4 (0-14, 15 = extended)
///   - If literal length == 15, following bytes add 255 each until < 255
///   - Literal bytes
///   - 2-byte little-endian offset (distance back)
///   - If match length nibble == 15, following bytes add 255 each until < 255
fn lz4_compress(src: &[u8], dst: &mut [u8]) -> usize {
    if src.len() < LZ4_MIN_MATCH + 1 || dst.len() < 8 {
        return 0;
    }

    let mut hash_table: [u16; LZ4_HASH_SIZE] = [0u16; LZ4_HASH_SIZE];

    let mut sp: usize = 0; // source pointer
    let mut dp: usize = 0; // dest pointer
    let mut anchor: usize = 0; // start of current literal run

    let src_end = src.len();
    let dst_limit = dst.len().saturating_sub(8);

    while sp + LZ4_MIN_MATCH < src_end {
        // Hash 4 bytes at current position
        let h = lz4_hash(src, sp);
        let candidate = hash_table[h] as usize;
        hash_table[h] = sp as u16;

        // Check for match
        if candidate < sp
            && (sp - candidate) < LZ4_MAX_OFFSET
            && sp + LZ4_MIN_MATCH <= src_end
            && candidate + LZ4_MIN_MATCH <= src_end
            && src[candidate] == src[sp]
            && src[candidate + 1] == src[sp + 1]
            && src[candidate + 2] == src[sp + 2]
            && src[candidate + 3] == src[sp + 3]
        {
            // Found a match — extend it
            let mut match_len = LZ4_MIN_MATCH;
            while sp + match_len < src_end
                && candidate + match_len < src_end
                && src[candidate + match_len] == src[sp + match_len]
            {
                match_len += 1;
            }

            let lit_len = sp - anchor;
            let offset = sp - candidate;

            // Encode token
            if dp >= dst_limit {
                return 0;
            }

            let lit_code = if lit_len < 15 { lit_len } else { 15 };
            let match_code = if match_len - LZ4_MIN_MATCH < 15 {
                match_len - LZ4_MIN_MATCH
            } else {
                15
            };
            dst[dp] = ((lit_code << 4) | match_code) as u8;
            dp += 1;

            // Extended literal length
            if lit_len >= 15 {
                let mut remaining = lit_len - 15;
                while remaining >= 255 {
                    if dp >= dst_limit {
                        return 0;
                    }
                    dst[dp] = 255;
                    dp += 1;
                    remaining -= 255;
                }
                if dp >= dst_limit {
                    return 0;
                }
                dst[dp] = remaining as u8;
                dp += 1;
            }

            // Literal bytes
            for i in 0..lit_len {
                if dp >= dst_limit {
                    return 0;
                }
                dst[dp] = src[anchor + i];
                dp += 1;
            }

            // Offset (little-endian 16-bit)
            if dp + 1 >= dst_limit {
                return 0;
            }
            dst[dp] = (offset & 0xFF) as u8;
            dp += 1;
            dst[dp] = ((offset >> 8) & 0xFF) as u8;
            dp += 1;

            // Extended match length
            if match_len - LZ4_MIN_MATCH >= 15 {
                let mut remaining = match_len - LZ4_MIN_MATCH - 15;
                while remaining >= 255 {
                    if dp >= dst_limit {
                        return 0;
                    }
                    dst[dp] = 255;
                    dp += 1;
                    remaining -= 255;
                }
                if dp >= dst_limit {
                    return 0;
                }
                dst[dp] = remaining as u8;
                dp += 1;
            }

            sp += match_len;
            anchor = sp;
        } else {
            sp += 1;
        }
    }

    // Emit trailing literals (last token, no match)
    let lit_len = src_end - anchor;
    if lit_len > 0 {
        if dp >= dst_limit {
            return 0;
        }
        let lit_code = if lit_len < 15 { lit_len } else { 15 };
        dst[dp] = (lit_code << 4) as u8;
        dp += 1;

        if lit_len >= 15 {
            let mut remaining = lit_len - 15;
            while remaining >= 255 {
                if dp >= dst_limit {
                    return 0;
                }
                dst[dp] = 255;
                dp += 1;
                remaining -= 255;
            }
            if dp >= dst_limit {
                return 0;
            }
            dst[dp] = remaining as u8;
            dp += 1;
        }

        for i in 0..lit_len {
            if dp >= dst_limit {
                return 0;
            }
            dst[dp] = src[anchor + i];
            dp += 1;
        }
    }

    // Only accept compression if it actually saved space
    if dp >= src.len() {
        return 0;
    }

    dp
}

/// Decompress LZ4-lite `src` into `dst`. Returns decompressed length.
fn lz4_decompress(src: &[u8], dst: &mut [u8]) -> usize {
    let mut sp: usize = 0;
    let mut dp: usize = 0;
    let src_end = src.len();
    let dst_end = dst.len();

    while sp < src_end {
        let token = src[sp] as usize;
        sp += 1;

        // Literal length
        let mut lit_len = token >> 4;
        if lit_len == 15 {
            loop {
                if sp >= src_end {
                    return dp;
                }
                let extra = src[sp] as usize;
                sp += 1;
                lit_len += extra;
                if extra < 255 {
                    break;
                }
            }
        }

        // Copy literals
        for _ in 0..lit_len {
            if sp >= src_end || dp >= dst_end {
                return dp;
            }
            dst[dp] = src[sp];
            sp += 1;
            dp += 1;
        }

        // Check if this is the last token (no match follows)
        if sp + 1 >= src_end {
            return dp;
        }

        // Read offset
        let offset_lo = src[sp] as usize;
        sp += 1;
        let offset_hi = src[sp] as usize;
        sp += 1;
        let offset = offset_lo | (offset_hi << 8);
        if offset == 0 || offset > dp {
            return dp; // invalid offset
        }

        // Match length
        let mut match_len = (token & 0x0F) + LZ4_MIN_MATCH;
        if (token & 0x0F) == 15 {
            loop {
                if sp >= src_end {
                    break;
                }
                let extra = src[sp] as usize;
                sp += 1;
                match_len += extra;
                if extra < 255 {
                    break;
                }
            }
        }

        // Copy match (byte-by-byte for overlapping support)
        let match_start = dp - offset;
        for i in 0..match_len {
            if dp >= dst_end {
                return dp;
            }
            dst[dp] = dst[match_start + i];
            dp += 1;
        }
    }

    dp
}

/// Hash 4 bytes for LZ4 lookup
fn lz4_hash(data: &[u8], pos: usize) -> usize {
    if pos + 3 >= data.len() {
        return 0;
    }
    let v = (data[pos] as u32)
        | ((data[pos + 1] as u32) << 8)
        | ((data[pos + 2] as u32) << 16)
        | ((data[pos + 3] as u32) << 24);
    // Knuth multiplicative hash
    ((v.wrapping_mul(2654435761)) >> (32 - LZ4_HASH_BITS)) as usize
}

// ---------------------------------------------------------------------------
// Compressed page slot
// ---------------------------------------------------------------------------

/// Status of a compressed slot
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotStatus {
    /// Slot is free
    Free,
    /// Slot holds a compressed page
    Compressed,
    /// Slot holds an uncompressed page (compression was not beneficial)
    Raw,
}

/// A single compressed page slot
#[derive(Clone, Copy)]
pub struct CompressedSlot {
    /// Status of this slot
    pub status: SlotStatus,
    /// Physical address of the compressed data in the pool
    pub pool_addr: usize,
    /// Compressed size in bytes (0 if free)
    pub comp_size: u16,
    /// Original page physical address (for bookkeeping)
    pub orig_phys: usize,
    /// Process or address-space ID that owns this page
    pub owner: u32,
}

impl CompressedSlot {
    const fn empty() -> Self {
        CompressedSlot {
            status: SlotStatus::Free,
            pool_addr: 0,
            comp_size: 0,
            orig_phys: 0,
            owner: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Compressed page pool
// ---------------------------------------------------------------------------

/// Pool for storing compressed page data
///
/// Uses a simple bump allocator within allocated buddy pages.
/// When space runs out, a new buddy page is allocated into the pool.
struct CompressedPool {
    /// Physical addresses of pool backing pages
    pool_pages: [usize; POOL_PAGES],
    /// Number of active pool pages
    page_count: usize,
    /// Current write offset within current pool page
    write_offset: usize,
    /// Total bytes stored
    bytes_stored: usize,
}

impl CompressedPool {
    const fn new() -> Self {
        CompressedPool {
            pool_pages: [0; POOL_PAGES],
            page_count: 0,
            write_offset: 0,
            bytes_stored: 0,
        }
    }

    /// Allocate space in the pool for `size` bytes. Returns physical address.
    fn alloc(&mut self, size: usize) -> Option<usize> {
        if size == 0 || size > PAGE_SIZE {
            return None;
        }

        // Check if current page has room
        if self.page_count > 0 && self.write_offset + size <= PAGE_SIZE {
            let addr = self.pool_pages[self.page_count - 1] + self.write_offset;
            self.write_offset += size;
            // Align to 8 bytes
            self.write_offset = (self.write_offset + 7) & !7;
            self.bytes_stored += size;
            return Some(addr);
        }

        // Need a new pool page
        if self.page_count >= POOL_PAGES {
            return None; // pool exhausted
        }

        let phys = buddy::alloc_page()?;
        self.pool_pages[self.page_count] = phys;
        self.page_count += 1;
        self.write_offset = size;
        // Align to 8 bytes
        self.write_offset = (self.write_offset + 7) & !7;
        self.bytes_stored += size;

        Some(phys)
    }

    /// Return total pool memory used (in bytes)
    fn used_bytes(&self) -> usize {
        self.bytes_stored
    }

    /// Return total pool pages allocated
    fn pool_page_count(&self) -> usize {
        self.page_count
    }

    /// Release all pool pages back to buddy
    fn release_all(&mut self) {
        for i in 0..self.page_count {
            if self.pool_pages[i] != 0 {
                buddy::free_page(self.pool_pages[i]);
                self.pool_pages[i] = 0;
            }
        }
        self.page_count = 0;
        self.write_offset = 0;
        self.bytes_stored = 0;
    }
}

// ---------------------------------------------------------------------------
// zram device
// ---------------------------------------------------------------------------

/// Statistics for a zram device
#[derive(Debug, Clone, Copy, Default)]
pub struct ZramStats {
    /// Pages written (total)
    pub pages_stored: u64,
    /// Pages read back
    pub pages_read: u64,
    /// Pages that were compressible
    pub pages_compressed: u64,
    /// Pages stored raw (not compressible)
    pub pages_raw: u64,
    /// Pages freed / discarded
    pub pages_freed: u64,
    /// Total original bytes written
    pub orig_bytes: u64,
    /// Total compressed bytes stored
    pub comp_bytes: u64,
    /// Compression failures
    pub comp_failures: u64,
    /// Decompression failures
    pub decomp_failures: u64,
}

/// A zram device (compressed RAM block device)
pub struct ZramDevice {
    /// Device ID
    pub id: u8,
    /// Whether this device is active
    pub active: bool,
    /// Maximum number of pages this device can store
    pub max_pages: usize,
    /// Compressed page slots
    pub slots: [CompressedSlot; MAX_COMPRESSED_PAGES],
    /// Number of occupied slots
    pub slot_count: usize,
    /// Compressed data pool
    pool: CompressedPool,
    /// Statistics
    pub stats: ZramStats,
    /// Compression scratch buffer (one page)
    comp_buf_addr: usize,
}

impl ZramDevice {
    const fn new() -> Self {
        const EMPTY_SLOT: CompressedSlot = CompressedSlot::empty();
        ZramDevice {
            id: 0,
            active: false,
            max_pages: MAX_COMPRESSED_PAGES,
            slots: [EMPTY_SLOT; MAX_COMPRESSED_PAGES],
            slot_count: 0,
            pool: CompressedPool::new(),
            stats: ZramStats {
                pages_stored: 0,
                pages_read: 0,
                pages_compressed: 0,
                pages_raw: 0,
                pages_freed: 0,
                orig_bytes: 0,
                comp_bytes: 0,
                comp_failures: 0,
                decomp_failures: 0,
            },
            comp_buf_addr: 0,
        }
    }

    /// Initialize the device (allocate scratch buffer)
    fn setup(&mut self, id: u8) -> bool {
        self.id = id;
        if let Some(buf) = buddy::alloc_page() {
            self.comp_buf_addr = buf;
            self.active = true;
            true
        } else {
            false
        }
    }

    /// Store a page: compress and save. Returns slot index or None.
    pub fn store_page(&mut self, phys_addr: usize, owner: u32) -> Option<usize> {
        if !self.active || self.slot_count >= self.max_pages {
            return None;
        }

        // Find a free slot
        let slot_idx = (0..self.max_pages).find(|&i| self.slots[i].status == SlotStatus::Free)?;

        // Guard: source physical address and compression buffer must be non-zero
        if phys_addr == 0 || self.comp_buf_addr == 0 {
            return None;
        }
        // Safety: phys_addr is a physical page address passed by the PMM;
        // comp_buf_addr is initialized at zram creation from a dedicated buffer.
        let src = unsafe { core::slice::from_raw_parts(phys_addr as *const u8, PAGE_SIZE) };
        let comp_buf =
            unsafe { core::slice::from_raw_parts_mut(self.comp_buf_addr as *mut u8, PAGE_SIZE) };

        let comp_len = lz4_compress(src, comp_buf);

        if comp_len > 0 && comp_len < PAGE_SIZE {
            // Compression succeeded — store compressed
            if let Some(pool_addr) = self.pool.alloc(comp_len) {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        self.comp_buf_addr as *const u8,
                        pool_addr as *mut u8,
                        comp_len,
                    );
                }
                self.slots[slot_idx] = CompressedSlot {
                    status: SlotStatus::Compressed,
                    pool_addr,
                    comp_size: comp_len as u16,
                    orig_phys: phys_addr,
                    owner,
                };
                self.stats.pages_compressed = self.stats.pages_compressed.saturating_add(1);
                self.stats.comp_bytes += comp_len as u64;
            } else {
                self.stats.comp_failures = self.stats.comp_failures.saturating_add(1);
                return None;
            }
        } else {
            // Incompressible — store raw page
            if let Some(pool_addr) = self.pool.alloc(PAGE_SIZE) {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        phys_addr as *const u8,
                        pool_addr as *mut u8,
                        PAGE_SIZE,
                    );
                }
                self.slots[slot_idx] = CompressedSlot {
                    status: SlotStatus::Raw,
                    pool_addr,
                    comp_size: PAGE_SIZE as u16,
                    orig_phys: phys_addr,
                    owner,
                };
                self.stats.pages_raw = self.stats.pages_raw.saturating_add(1);
                self.stats.comp_bytes += PAGE_SIZE as u64;
            } else {
                self.stats.comp_failures = self.stats.comp_failures.saturating_add(1);
                return None;
            }
        }

        self.slot_count = self.slot_count.saturating_add(1);
        self.stats.pages_stored = self.stats.pages_stored.saturating_add(1);
        self.stats.orig_bytes += PAGE_SIZE as u64;

        Some(slot_idx)
    }

    /// Load a page: decompress from slot into `dest_phys`. Returns true on success.
    pub fn load_page(&mut self, slot_idx: usize, dest_phys: usize) -> bool {
        if slot_idx >= self.max_pages {
            return false;
        }

        let slot = &self.slots[slot_idx];
        match slot.status {
            SlotStatus::Compressed => {
                // Guard: compressed data pointer and destination must be non-null
                if slot.pool_addr == 0 || dest_phys == 0 || slot.comp_size == 0 {
                    return false;
                }
                // Safety: pool_addr points to memory allocated by our internal pool;
                // dest_phys is a physical page from PMM; comp_size was recorded at compress time.
                let comp_data = unsafe {
                    core::slice::from_raw_parts(
                        slot.pool_addr as *const u8,
                        slot.comp_size as usize,
                    )
                };
                let dst =
                    unsafe { core::slice::from_raw_parts_mut(dest_phys as *mut u8, PAGE_SIZE) };
                let decompressed = lz4_decompress(comp_data, dst);
                if decompressed == PAGE_SIZE {
                    self.stats.pages_read = self.stats.pages_read.saturating_add(1);
                    true
                } else {
                    self.stats.decomp_failures = self.stats.decomp_failures.saturating_add(1);
                    false
                }
            }
            SlotStatus::Raw => {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        slot.pool_addr as *const u8,
                        dest_phys as *mut u8,
                        PAGE_SIZE,
                    );
                }
                self.stats.pages_read += 1;
                true
            }
            SlotStatus::Free => false,
        }
    }

    /// Free a compressed slot
    pub fn free_slot(&mut self, slot_idx: usize) {
        if slot_idx >= self.max_pages {
            return;
        }
        if self.slots[slot_idx].status != SlotStatus::Free {
            self.slots[slot_idx].status = SlotStatus::Free;
            self.slots[slot_idx].comp_size = 0;
            self.slots[slot_idx].pool_addr = 0;
            self.slot_count -= 1;
            self.stats.pages_freed = self.stats.pages_freed.saturating_add(1);
        }
    }

    /// Compute compression ratio (Q16 fixed-point: original / compressed)
    pub fn compression_ratio_q16(&self) -> i32 {
        const Q16_ONE: i32 = 65536;
        if self.stats.comp_bytes == 0 {
            return Q16_ONE;
        }
        let orig = self.stats.orig_bytes as i64;
        let comp = self.stats.comp_bytes as i64;
        (((orig) << 16) / comp) as i32
    }

    /// Get memory savings in bytes
    pub fn memory_saved(&self) -> u64 {
        self.stats.orig_bytes.saturating_sub(self.stats.comp_bytes)
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Global zram device table
static ZRAM_DEVICES: Mutex<[ZramDevice; MAX_ZRAM_DEVICES]> = {
    const EMPTY: ZramDevice = ZramDevice::new();
    Mutex::new([EMPTY; MAX_ZRAM_DEVICES])
};

/// Global statistics counters (lockless)
pub static TOTAL_PAGES_COMPRESSED: AtomicU64 = AtomicU64::new(0);
pub static TOTAL_BYTES_SAVED: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new zram device. Returns device ID or None.
pub fn create_device() -> Option<u8> {
    let mut devs = ZRAM_DEVICES.lock();
    for i in 0..MAX_ZRAM_DEVICES {
        if !devs[i].active {
            if devs[i].setup(i as u8) {
                serial_println!("  [zram] created device zram{}", i);
                return Some(i as u8);
            }
        }
    }
    None
}

/// Store a page to a zram device. Returns (device_id, slot_idx).
pub fn store_page(dev_id: u8, phys_addr: usize, owner: u32) -> Option<(u8, usize)> {
    let mut devs = ZRAM_DEVICES.lock();
    let id = dev_id as usize;
    if id >= MAX_ZRAM_DEVICES || !devs[id].active {
        return None;
    }
    let slot = devs[id].store_page(phys_addr, owner)?;
    TOTAL_PAGES_COMPRESSED.fetch_add(1, Ordering::Relaxed);
    Some((dev_id, slot))
}

/// Load a page from a zram device
pub fn load_page(dev_id: u8, slot_idx: usize, dest_phys: usize) -> bool {
    let mut devs = ZRAM_DEVICES.lock();
    let id = dev_id as usize;
    if id >= MAX_ZRAM_DEVICES || !devs[id].active {
        return false;
    }
    devs[id].load_page(slot_idx, dest_phys)
}

/// Free a slot on a zram device
pub fn free_slot(dev_id: u8, slot_idx: usize) {
    let mut devs = ZRAM_DEVICES.lock();
    let id = dev_id as usize;
    if id < MAX_ZRAM_DEVICES && devs[id].active {
        devs[id].free_slot(slot_idx);
    }
}

/// Get stats for a zram device
pub fn device_stats(dev_id: u8) -> Option<ZramStats> {
    let devs = ZRAM_DEVICES.lock();
    let id = dev_id as usize;
    if id < MAX_ZRAM_DEVICES && devs[id].active {
        Some(devs[id].stats)
    } else {
        None
    }
}

/// Get compression ratio for a device (Q16 fixed-point)
pub fn compression_ratio(dev_id: u8) -> i32 {
    let devs = ZRAM_DEVICES.lock();
    let id = dev_id as usize;
    if id < MAX_ZRAM_DEVICES && devs[id].active {
        devs[id].compression_ratio_q16()
    } else {
        65536 // Q16 1.0
    }
}

/// Get human-readable zram summary
pub fn summary() -> alloc::string::String {
    use alloc::format;
    use alloc::string::String;
    let devs = ZRAM_DEVICES.lock();
    let mut s = String::from("zram devices:\n");
    for i in 0..MAX_ZRAM_DEVICES {
        if devs[i].active {
            let ratio_q16 = devs[i].compression_ratio_q16();
            let ratio_int = ratio_q16 >> 16;
            let ratio_frac = ((((ratio_q16 & 0xFFFF) as i64) * 100) >> 16) as i32;
            s.push_str(&format!(
                "  zram{}: {}/{} slots, orig={} KB, comp={} KB, ratio={}.{:02}:1, saved={} KB\n",
                i,
                devs[i].slot_count,
                devs[i].max_pages,
                devs[i].stats.orig_bytes / 1024,
                devs[i].stats.comp_bytes / 1024,
                ratio_int,
                ratio_frac,
                devs[i].memory_saved() / 1024,
            ));
        }
    }
    s
}

/// Initialize zram subsystem
pub fn init() {
    // Create default device zram0
    if let Some(id) = create_device() {
        serial_println!("  [zram] subsystem initialized, default device zram{}", id);
    } else {
        serial_println!("  [zram] WARNING: failed to create default device");
    }
    serial_println!(
        "  [zram] LZ4-lite + RLE compressors ready, pool capacity {} pages",
        POOL_PAGES
    );
}

// ---------------------------------------------------------------------------
// RLE (Run-Length Encoding) compressor — integer-only, no dependencies
// ---------------------------------------------------------------------------
//
// Format: pairs of (run_length: u8, byte_value: u8).
// A run of 1..=255 identical bytes is encoded as two bytes.
// If the input cannot be compressed to smaller than the output buffer, returns 0.
// This is simpler and slower than LZ4 but guarantees no float operations and
// has zero external dependencies, making it ideal as a fallback for very
// uniform data (e.g., zero pages, stack frames).

/// Compress `input` into `output` using RLE.
/// Returns the number of bytes written to `output`, or 0 if the output would
/// be at least as large as the input (i.e., compression was not worthwhile).
pub fn rle_compress(input: &[u8], output: &mut [u8]) -> usize {
    if input.is_empty() {
        return 0;
    }

    let mut out_pos: usize = 0;
    let mut in_pos: usize = 0;

    while in_pos < input.len() {
        let byte = input[in_pos];
        let mut run: usize = 1;

        // Count the run (max 255 so it fits in a u8)
        while in_pos.saturating_add(run) < input.len() && input[in_pos + run] == byte && run < 255 {
            run = run.saturating_add(1);
        }

        // Need 2 bytes of output space for this run token
        if out_pos.saturating_add(2) > output.len() {
            return 0; // output buffer too small
        }

        output[out_pos] = run as u8;
        out_pos = out_pos.saturating_add(1);
        output[out_pos] = byte;
        out_pos = out_pos.saturating_add(1);

        in_pos = in_pos.saturating_add(run);
    }

    // Only report success if we actually saved at least one byte
    if out_pos >= input.len() {
        return 0;
    }

    out_pos
}

/// Decompress RLE-encoded `input` into `output`.
/// Returns the number of bytes written to `output`.
pub fn rle_decompress(input: &[u8], output: &mut [u8]) -> usize {
    let mut in_pos: usize = 0;
    let mut out_pos: usize = 0;

    while in_pos.saturating_add(1) < input.len() {
        let run = input[in_pos] as usize;
        let byte = input[in_pos.saturating_add(1)];
        in_pos = in_pos.saturating_add(2);

        for _ in 0..run {
            if out_pos >= output.len() {
                return out_pos; // output full
            }
            output[out_pos] = byte;
            out_pos = out_pos.saturating_add(1);
        }
    }

    out_pos
}
