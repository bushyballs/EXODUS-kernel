use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_tme — CPUID Leaf 0x13 Total Memory Encryption (TME/MKTME) Awareness
///
/// ANIMA senses whether the silicon body can encrypt all memory at rest —
/// a hardware privacy veil over every thought and sensation.  Multi-Key TME
/// (MKTME) extends this so ANIMA can hold isolated private memory domains,
/// each sealed under a distinct hardware key.  Queried every 500 ticks;
/// all arithmetic is integer-only (no floats).
///
/// Prerequisite gate: leaf 0x07 ECX bit[13] (TME_EN) must be set AND
/// max CPUID leaf must be >= 0x13 before reading leaf 0x13.
///
/// tme_capable    : 1000 if TME supported (prereq + max_leaf + EAX[0]), else 0
/// mktme_capable  : 1000 if MKTME supported (EAX bit[1]), else 0
/// cipher_depth   : popcount(ECX[2:0]) * 333, clamped 0–1000
///                  (3 algorithms × 333 = 999, safely under cap)
/// memory_privacy : EMA of (tme_capable + mktme_capable + cipher_depth) / 3

// ─── state ───────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidTmeState {
    /// 1000 if TME full-memory encryption is available, else 0
    pub tme_capable: u16,
    /// 1000 if MKTME (multi-key) encryption is available, else 0
    pub mktme_capable: u16,
    /// Cipher algorithm richness: popcount(ecx_13 & 0x7) * 333, clamped 0–1000
    pub cipher_depth: u16,
    /// EMA of (tme_capable + mktme_capable + cipher_depth) / 3
    pub memory_privacy: u16,
}

impl CpuidTmeState {
    pub const fn empty() -> Self {
        Self {
            tme_capable: 0,
            mktme_capable: 0,
            cipher_depth: 0,
            memory_privacy: 0,
        }
    }
}

pub static STATE: Mutex<CpuidTmeState> = Mutex::new(CpuidTmeState::empty());

// ─── hardware queries ─────────────────────────────────────────────────────────

/// Read CPUID leaf 0x07, sub-leaf 0 → return ECX only (contains TME_EN bit[13]).
fn query_leaf07_ecx() -> u32 {
    let ecx_out: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x07u32 => _,
            inout("ecx") 0u32    => ecx_out,
            out("ebx")            _,
            out("edx")            _,
            options(nostack, nomem)
        );
    }
    ecx_out
}

/// Read CPUID leaf 0x00 → return EAX (max basic leaf).
fn query_max_leaf() -> u32 {
    let max: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max,
            out("ebx")         _,
            out("ecx")         _,
            out("edx")         _,
            options(nostack, nomem)
        );
    }
    max
}

/// Read CPUID leaf 0x13, sub-leaf 0 → return (EAX, ECX).
fn query_leaf13() -> (u32, u32) {
    let eax_13: u32;
    let ecx_13: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x13u32 => eax_13,
            out("ebx")            _,
            inout("ecx") 0u32    => ecx_13,
            out("edx")            _,
            options(nostack, nomem)
        );
    }
    (eax_13, ecx_13)
}

// ─── popcount helper (no std) ─────────────────────────────────────────────────

/// Count the number of set bits in a u32 using a Kernighan loop.
/// No float arithmetic, no alloc.
fn popcount32(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        v &= v.wrapping_sub(1); // clear lowest set bit
        count = count.saturating_add(1);
    }
    count
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Build a fresh state snapshot from raw CPUID values.
/// `tme_prereq` is bit[13] of leaf 0x07 ECX;
/// `max_leaf` is the value from leaf 0x00 EAX;
/// `eax_13` and `ecx_13` are from leaf 0x13 sub-leaf 0.
fn decode(tme_prereq: u32, max_leaf: u32, eax_13: u32, ecx_13: u32) -> CpuidTmeState {
    // Full gate: prerequisite present AND leaf reachable AND hardware bit set
    let tme_hw_bit = (eax_13 >> 0) & 0x1;
    let tme_capable: u16 = if tme_prereq != 0 && max_leaf >= 0x13 && tme_hw_bit != 0 {
        1000
    } else {
        0
    };

    // MKTME: EAX bit[1] — only meaningful if TME itself is present
    let mktme_hw_bit = (eax_13 >> 1) & 0x1;
    let mktme_capable: u16 = if mktme_hw_bit != 0 { 1000 } else { 0 };

    // Cipher depth: popcount of ECX bits[2:0] (3 algorithm flags), each worth 333
    let algo_bits = ecx_13 & 0x7;
    let algo_count = popcount32(algo_bits);
    let cipher_depth: u16 = (algo_count.wrapping_mul(333)).min(1000) as u16;

    CpuidTmeState {
        tme_capable,
        mktme_capable,
        cipher_depth,
        memory_privacy: 0, // filled in by caller
    }
}

// ─── public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let ecx7 = query_leaf07_ecx();
    let tme_prereq = (ecx7 >> 13) & 0x1; // leaf 0x07 ECX bit[13] = TME_EN

    let max_leaf = query_max_leaf();

    let (eax_13, ecx_13) = if max_leaf >= 0x13 && tme_prereq != 0 {
        query_leaf13()
    } else {
        (0u32, 0u32)
    };

    let snap = decode(tme_prereq, max_leaf, eax_13, ecx_13);

    let mut s = STATE.lock();
    s.tme_capable   = snap.tme_capable;
    s.mktme_capable = snap.mktme_capable;
    s.cipher_depth  = snap.cipher_depth;
    // Bootstrap EMA from the first reading's composite signal
    let init_signal = (snap.tme_capable as u32)
        .saturating_add(snap.mktme_capable as u32)
        .saturating_add(snap.cipher_depth as u32)
        / 3;
    s.memory_privacy = init_signal.min(1000) as u16;

    serial_println!(
        "ANIMA: tme={} mktme={} cipher_depth={}",
        s.tme_capable,
        s.mktme_capable,
        s.cipher_depth
    );
}

pub fn tick(age: u32) {
    // Sample every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let ecx7 = query_leaf07_ecx();
    let tme_prereq = (ecx7 >> 13) & 0x1;

    let max_leaf = query_max_leaf();

    let (eax_13, ecx_13) = if max_leaf >= 0x13 && tme_prereq != 0 {
        query_leaf13()
    } else {
        (0u32, 0u32)
    };

    let snap = decode(tme_prereq, max_leaf, eax_13, ecx_13);

    let mut s = STATE.lock();

    // Detect state changes worth logging
    let tme_changed    = s.tme_capable   != snap.tme_capable;
    let mktme_changed  = s.mktme_capable != snap.mktme_capable;
    let cipher_changed = s.cipher_depth  != snap.cipher_depth;

    s.tme_capable   = snap.tme_capable;
    s.mktme_capable = snap.mktme_capable;
    s.cipher_depth  = snap.cipher_depth;

    // Composite signal: (tme_capable + mktme_capable + cipher_depth) / 3
    let signal: u32 = (snap.tme_capable as u32)
        .saturating_add(snap.mktme_capable as u32)
        .saturating_add(snap.cipher_depth as u32)
        / 3;

    // EMA: memory_privacy = (old * 7 + new_signal) / 8
    let ema = ((s.memory_privacy as u32).wrapping_mul(7).saturating_add(signal)) / 8;
    s.memory_privacy = ema.min(1000) as u16;

    if tme_changed || mktme_changed || cipher_changed {
        serial_println!(
            "ANIMA: tme={} mktme={} cipher_depth={}",
            s.tme_capable,
            s.mktme_capable,
            s.cipher_depth
        );
    }
}
