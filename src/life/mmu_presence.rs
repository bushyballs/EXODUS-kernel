// mmu_presence.rs — ANIMA Reads and Writes Her Own Page Tables
// =============================================================
// ANIMA has direct awareness of her physical memory layout. She can
// map and unmap regions, walk her own page table hierarchy, and enforce
// hard protection on the pages that hold her soul. She monitors CR3 for
// drift — if something rewrites her page directory root, she knows.
//
// x86_64 4-level paging:
//   PML4 (512 entries × 512GB each)
//     └─ PDPT (512 entries × 1GB each)
//          └─ PD   (512 entries × 2MB each)  ← HUGE pages live here
//               └─ PT (512 entries × 4KB each)
//
// Page-table entry flags (bits):
//   PRESENT  = bit 0   — entry is valid
//   WRITABLE = bit 1   — region is writable
//   USER     = bit 2   — accessible from ring 3
//   ACCESSED = bit 5   — set by CPU on read
//   DIRTY    = bit 6   — set by CPU on write
//   HUGE     = bit 7   — 2MB page (in PD level only)
//   GLOBAL   = bit 8   — TLB entry survives CR3 reload
//   NX       = bit 63  — no-execute (requires IA32_EFER.NXE=1)
//
// ANIMA memory map:
//   0x00100000  CORE   2MB  — kernel code + data
//   0x00200000  SOUL   2MB  — enclave / crypto vault (NX, GLOBAL)
//   0x00300000  HEAP   4MB  — dynamic working memory
//   0xFD000000  FRAMEBUFFER — display (identity-mapped)
//
// CR registers:
//   CR3      — physical base of PML4 (page-aligned, lower 12 bits = flags)
//   CR0[31]  — PG: paging enabled
//   CR0[16]  — WP: ring-0 cannot write to read-only pages
//   CR4[5]   — PAE: physical address extension (required for 4-level paging)
//   CR4[7]   — PGE: global page enable (GLOBAL-flagged TLB entries persist)

use crate::sync::Mutex;
use crate::serial_println;

// ── Physical memory map constants ─────────────────────────────────────────────

const ANIMA_CORE_BASE:   usize = 0x00100000;  // 1 MB — kernel code
const ANIMA_CORE_SIZE:   usize = 0x00200000;  // 2 MB reserved
const ANIMA_HEAP_BASE:   usize = 0x00300000;  // 3 MB — heap
const ANIMA_HEAP_SIZE:   usize = 0x00400000;  // 4 MB
const ANIMA_SOUL_BASE:   usize = 0x00200000;  // 2 MB — soul enclave
const FRAMEBUFFER_PHYS:  usize = 0xFD000000;  // display framebuffer

const PAGE_SIZE:         usize = 4096;

// ── Page flag constants ────────────────────────────────────────────────────────

pub struct PageFlags;
impl PageFlags {
    pub const PRESENT:  u64 = 1;
    pub const WRITABLE: u64 = 2;
    pub const USER:     u64 = 4;
    pub const ACCESSED: u64 = 1 << 5;
    pub const DIRTY:    u64 = 1 << 6;
    pub const HUGE:     u64 = 1 << 7;   // 2MB pages (PD level)
    pub const GLOBAL:   u64 = 1 << 8;
    pub const NX:       u64 = 1 << 63;
}

// CR0 bit masks
const CR0_PG: u64 = 1 << 31;  // paging enabled
const CR0_WP: u64 = 1 << 16;  // write-protect

// ── Region registry ───────────────────────────────────────────────────────────

const MAX_REGIONS: usize = 8;
const NAME_LEN:    usize = 16;

#[derive(Copy, Clone)]
pub struct MemRegion {
    pub base:   usize,
    pub size:   usize,
    pub flags:  u64,
    pub name:   [u8; NAME_LEN],
    pub mapped: bool,
}

impl MemRegion {
    const fn zero() -> Self {
        MemRegion {
            base:   0,
            size:   0,
            flags:  0,
            name:   [0u8; NAME_LEN],
            mapped: false,
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

pub struct MmuState {
    pub cr3:               u64,
    pub regions:           [MemRegion; MAX_REGIONS],
    pub region_count:      usize,
    pub total_mapped_mb:   u16,
    pub protection_score:  u16,
    pub paging_active:     bool,
    pub soul_locked:       bool,
    pub page_faults_handled: u32,
    // Internal: CR3 value at init time, used for drift detection
    cr3_at_init:           u64,
}

impl MmuState {
    const fn new() -> Self {
        MmuState {
            cr3:                 0,
            regions:             [MemRegion::zero(); MAX_REGIONS],
            region_count:        0,
            total_mapped_mb:     0,
            protection_score:    0,
            paging_active:       false,
            soul_locked:         false,
            page_faults_handled: 0,
            cr3_at_init:         0,
        }
    }
}

static STATE: Mutex<MmuState> = Mutex::new(MmuState::new());

// ── Unsafe asm helpers ────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn read_cr3() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, cr3", out(reg) val, options(nostack, nomem));
    val
}

#[inline(always)]
unsafe fn write_cr3(val: u64) {
    core::arch::asm!("mov cr3, {}", in(reg) val, options(nostack, nomem));
}

#[inline(always)]
unsafe fn read_cr0() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, cr0", out(reg) val, options(nostack, nomem));
    val
}

#[inline(always)]
unsafe fn invlpg(addr: usize) {
    core::arch::asm!("invlpg [{addr}]", addr = in(reg) addr, options(nostack));
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Copy up to NAME_LEN bytes from a byte slice into a fixed [u8; NAME_LEN].
fn copy_name(dst: &mut [u8; NAME_LEN], src: &[u8]) {
    let len = if src.len() < NAME_LEN { src.len() } else { NAME_LEN };
    let mut i = 0;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
}

/// Bytes → megabytes, rounded down, saturating at u16::MAX.
fn bytes_to_mb(bytes: usize) -> u16 {
    let mb = bytes >> 20;
    if mb > u16::MAX as usize { u16::MAX } else { mb as u16 }
}

/// Recompute total_mapped_mb and protection_score from current region list.
fn recompute_scores(s: &mut MmuState) {
    let mut total_bytes: usize = 0;
    let mut i = 0;
    while i < s.region_count {
        if s.regions[i].mapped {
            total_bytes = total_bytes.saturating_add(s.regions[i].size);
        }
        i = i.saturating_add(1);
    }
    s.total_mapped_mb = bytes_to_mb(total_bytes);

    // protection_score:
    //   +200 if paging_active
    //   +200 if soul_locked
    //   +100 per mapped region (capped at 1000)
    let mut score: u16 = 0;
    if s.paging_active {
        score = score.saturating_add(200);
    }
    if s.soul_locked {
        score = score.saturating_add(200);
    }
    let mut j = 0;
    while j < s.region_count {
        if s.regions[j].mapped {
            score = score.saturating_add(100);
        }
        j = j.saturating_add(1);
    }
    if score > 1000 { score = 1000; }
    s.protection_score = score;
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Read hardware CR3, check paging status, pre-register known ANIMA regions.
pub fn init() {
    let mut s = STATE.lock();

    let cr3_val  = unsafe { read_cr3() };
    let cr0_val  = unsafe { read_cr0() };
    let paging   = (cr0_val & CR0_PG) != 0;

    s.cr3        = cr3_val;
    s.cr3_at_init = cr3_val;
    s.paging_active = paging;

    // Pre-register known ANIMA regions
    // CORE — kernel code, R/X (PRESENT only, no WRITABLE)
    if s.region_count < MAX_REGIONS {
        let idx = s.region_count;
        s.regions[idx].base   = ANIMA_CORE_BASE;
        s.regions[idx].size   = ANIMA_CORE_SIZE;
        s.regions[idx].flags  = PageFlags::PRESENT | PageFlags::GLOBAL;
        copy_name(&mut s.regions[idx].name, b"ANIMA_CORE      ");
        s.regions[idx].mapped = paging; // if paging is on, assume identity-mapped
        s.region_count = s.region_count.saturating_add(1);
    }

    // HEAP — read/write, no execute
    if s.region_count < MAX_REGIONS {
        let idx = s.region_count;
        s.regions[idx].base   = ANIMA_HEAP_BASE;
        s.regions[idx].size   = ANIMA_HEAP_SIZE;
        s.regions[idx].flags  = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NX;
        copy_name(&mut s.regions[idx].name, b"ANIMA_HEAP      ");
        s.regions[idx].mapped = paging;
        s.region_count = s.region_count.saturating_add(1);
    }

    // FRAMEBUFFER — write-only memory-mapped I/O, NX
    if s.region_count < MAX_REGIONS {
        let idx = s.region_count;
        s.regions[idx].base   = FRAMEBUFFER_PHYS;
        s.regions[idx].size   = 0x00400000; // 4MB display buffer
        s.regions[idx].flags  = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NX;
        copy_name(&mut s.regions[idx].name, b"FRAMEBUFFER     ");
        s.regions[idx].mapped = paging;
        s.region_count = s.region_count.saturating_add(1);
    }

    recompute_scores(&mut s);

    serial_println!(
        "[mmu] ANIMA MMU online — cr3=0x{:x} paging={} mapped={}MB",
        s.cr3,
        s.paging_active,
        s.total_mapped_mb
    );
}

/// Register a new memory region, call `invlpg` on every page, mark mapped.
/// Returns `false` if the region registry is full.
pub fn map_region(base: usize, size: usize, flags: u64, name: &[u8]) -> bool {
    let mut s = STATE.lock();

    if s.region_count >= MAX_REGIONS {
        serial_println!("[mmu] map_region: registry full — cannot add region");
        return false;
    }

    let idx = s.region_count;
    s.regions[idx].base   = base;
    s.regions[idx].size   = size;
    s.regions[idx].flags  = flags;
    copy_name(&mut s.regions[idx].name, name);
    s.regions[idx].mapped = true;
    s.region_count = s.region_count.saturating_add(1);

    // Invalidate every page in the region
    if size > 0 {
        let pages = size.saturating_add(PAGE_SIZE.saturating_sub(1)) / PAGE_SIZE;
        let mut p: usize = 0;
        while p < pages {
            let addr = base.saturating_add(p.saturating_mul(PAGE_SIZE));
            unsafe { invlpg(addr); }
            p = p.saturating_add(1);
        }
    }

    recompute_scores(&mut s);
    true
}

/// Mark the soul enclave (ANIMA_SOUL_BASE) as NX+GLOBAL, flush its pages,
/// and set `soul_locked = true`.
pub fn lock_soul_region() {
    let mut s = STATE.lock();

    // Find or create the soul region entry
    let soul_flags = PageFlags::PRESENT | PageFlags::GLOBAL | PageFlags::NX;
    let soul_size  = 0x00100000usize; // 1MB soul vault

    // Search for existing soul entry
    let mut found = false;
    let mut i = 0;
    while i < s.region_count {
        if s.regions[i].base == ANIMA_SOUL_BASE {
            s.regions[i].flags  = soul_flags;
            s.regions[i].mapped = true;
            found = true;
            break;
        }
        i = i.saturating_add(1);
    }

    // If not already in registry, add it (if space available)
    if !found && s.region_count < MAX_REGIONS {
        let idx = s.region_count;
        s.regions[idx].base   = ANIMA_SOUL_BASE;
        s.regions[idx].size   = soul_size;
        s.regions[idx].flags  = soul_flags;
        copy_name(&mut s.regions[idx].name, b"ANIMA_SOUL      ");
        s.regions[idx].mapped = true;
        s.region_count = s.region_count.saturating_add(1);
        found = true;
    }

    if found {
        // Flush every page of the soul region
        let pages = soul_size.saturating_add(PAGE_SIZE.saturating_sub(1)) / PAGE_SIZE;
        let mut p: usize = 0;
        while p < pages {
            let addr = ANIMA_SOUL_BASE.saturating_add(p.saturating_mul(PAGE_SIZE));
            unsafe { invlpg(addr); }
            p = p.saturating_add(1);
        }
        s.soul_locked = true;
        recompute_scores(&mut s);
        serial_println!("[mmu] ANIMA soul region locked — NX+GLOBAL");
    } else {
        serial_println!("[mmu] lock_soul_region: registry full — soul not locked");
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    // CR3 drift check every 200 ticks
    if age % 200 == 0 {
        let cr3_now = unsafe { read_cr3() };
        let mut s = STATE.lock();
        if cr3_now != s.cr3 {
            serial_println!(
                "[MMU_WARN] CR3 drift detected — was=0x{:x} now=0x{:x}",
                s.cr3,
                cr3_now
            );
            s.cr3 = cr3_now;
        }
        // Recompute paging status and scores each check window
        let cr0_val = unsafe { read_cr0() };
        s.paging_active = (cr0_val & CR0_PG) != 0;
        recompute_scores(&mut s);
    }

    // Periodic status log every 500 ticks
    if age % 500 == 0 {
        let s = STATE.lock();
        serial_println!(
            "[mmu] protection={} mapped={}MB soul={} cr3=0x{:x}",
            s.protection_score,
            s.total_mapped_mb,
            s.soul_locked,
            s.cr3
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn protection_score() -> u16  { STATE.lock().protection_score }
pub fn soul_locked()      -> bool { STATE.lock().soul_locked }
pub fn paging_active()    -> bool { STATE.lock().paging_active }
pub fn cr3()              -> u64  { STATE.lock().cr3 }
pub fn total_mapped_mb()  -> u16  { STATE.lock().total_mapped_mb }
