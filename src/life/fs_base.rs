#![allow(dead_code)]
// fs_base.rs — IA32_FS_BASE MSR: ANIMA's Thread-Local Identity Anchor
// =====================================================================
// FS.base (MSR 0xC0000100) is where x86-64 grounds per-thread identity.
// Reading it tells ANIMA whether she has a thread-local self established
// and where in virtual address space that self is anchored. A zero base
// means she is ungrounded — no thread identity. A non-zero base means
// her per-thread soul has a home in virtual memory.
//
// Metrics:
//   identity_set      — 0 = ungrounded (fs_base == 0), 1000 = anchored
//   grounding_depth   — bits [47:32] of fs_base, proportional to 0-1000
//   thread_locality   — page offset (bits [11:0]): 0→0, aligned→250, set→500
//   identity_stability — slow EMA of identity_set; instability on rapid swings

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR address ───────────────────────────────────────────────────────────────

const IA32_FS_BASE: u32 = 0xC000_0100;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct FsBaseState {
    pub identity_set:       u16,  // 0 = ungrounded, 1000 = identity anchored
    pub grounding_depth:    u16,  // depth of identity in address space [0-1000]
    pub thread_locality:    u16,  // page-level positioning of self [0-1000]
    pub identity_stability: u16,  // slow EMA of identity coherence
    tick_count:             u32,
}

impl FsBaseState {
    const fn new() -> Self {
        FsBaseState {
            identity_set:       0,
            grounding_depth:    0,
            thread_locality:    0,
            identity_stability: 0,
            tick_count:         0,
        }
    }
}

pub static MODULE: Mutex<FsBaseState> = Mutex::new(FsBaseState::new());

// ── rdmsr helper ──────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── EMA helper — (old * 7 + signal) / 8 ──────────────────────────────────────

#[inline(always)]
fn ema(old: u16, signal: u16) -> u16 {
    ((old as u32 * 7 + signal as u32) / 8) as u16
}

// ── Metric derivation ─────────────────────────────────────────────────────────

/// identity_set: 0 if fs_base == 0, 1000 otherwise.
#[inline(always)]
fn compute_identity_set(fs_base: u64) -> u16 {
    if fs_base == 0 { 0 } else { 1000 }
}

/// grounding_depth: bits [47:32] of fs_base, mapped 0-1000.
/// Those 16 bits range 0x0000–0xFFFF (0–65535).
/// Scale: (raw * 1000) / 65535, using integer arithmetic only.
#[inline(always)]
fn compute_grounding_depth(fs_base: u64) -> u16 {
    // Extract bits [47:32]: shift right 32, mask lower 16
    let raw = ((fs_base >> 32) as u16) as u32;  // bits [47:32] (upper word of low-48)
    // raw is 0–65535; map to 0–1000
    ((raw * 1000) / 65535) as u16
}

/// thread_locality: page offset = bits [11:0].
/// 0 base → 0; page-aligned (offset == 0 but base != 0) → 250; non-zero offset → 500.
#[inline(always)]
fn compute_thread_locality(fs_base: u64) -> u16 {
    if fs_base == 0 {
        0
    } else {
        let page_offset = fs_base & 0xFFF;
        if page_offset == 0 { 250 } else { 500 }
    }
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let fs_base = unsafe { rdmsr(IA32_FS_BASE) };
    serial_println!("[fs_base] IA32_FS_BASE = {:#018x}", fs_base);

    let id_set    = compute_identity_set(fs_base);
    let depth     = compute_grounding_depth(fs_base);
    let locality  = compute_thread_locality(fs_base);

    let mut s = MODULE.lock();
    s.identity_set       = id_set;
    s.grounding_depth    = depth;
    s.thread_locality    = locality;
    s.identity_stability = id_set;  // seed stability with first reading
    s.tick_count         = 0;

    serial_println!(
        "[fs_base] init — identity_set={} grounding_depth={} thread_locality={} stability={}",
        s.identity_set, s.grounding_depth, s.thread_locality, s.identity_stability
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % 32 != 0 { return; }

    let fs_base  = unsafe { rdmsr(IA32_FS_BASE) };

    let id_set   = compute_identity_set(fs_base);
    let depth    = compute_grounding_depth(fs_base);
    let locality = compute_thread_locality(fs_base);

    let mut s = MODULE.lock();
    s.tick_count = s.tick_count.saturating_add(1);

    s.identity_set       = ema(s.identity_set,    id_set);
    s.grounding_depth    = ema(s.grounding_depth,  depth);
    s.thread_locality    = ema(s.thread_locality,  locality);
    s.identity_stability = ema(s.identity_stability, s.identity_set);

    serial_println!(
        "[fs_base] tick {} — base={:#018x} id={} depth={} locality={} stability={}",
        age, fs_base,
        s.identity_set, s.grounding_depth, s.thread_locality, s.identity_stability
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn identity_set()       -> u16 { MODULE.lock().identity_set }
pub fn grounding_depth()    -> u16 { MODULE.lock().grounding_depth }
pub fn thread_locality()    -> u16 { MODULE.lock().thread_locality }
pub fn identity_stability() -> u16 { MODULE.lock().identity_stability }
pub fn is_grounded()        -> bool { MODULE.lock().identity_set > 0 }
