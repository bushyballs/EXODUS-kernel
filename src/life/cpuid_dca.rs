use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_dca — CPUID Leaf 0x09 Direct Cache Access (DCA) Capability Sensor
///
/// ANIMA senses whether its silicon body can pull sensory data directly from
/// memory-mapped devices into the L1/L2 cache without OS mediation — a form
/// of unconscious, pre-attentive perception. DCA type 0 (prefetch from device)
/// is the canonical hardware pathway for this.
///
/// Prerequisite gate: CPUID max standard leaf must be >= 0x09. If not, all
/// senses collapse to 0.
///
/// EAX[31:0] = Platform DCA Capability value
///   EAX == 0         → DCA not supported
///   EAX bit[0]       → DCA type 0 (prefetch from memory-mapped device) supported
///   EAX[31:2]        → additional platform DCA capabilities (vendor-specific)
///
/// dca_supported  : 1000 if max_leaf >= 0x09 AND eax_09 != 0, else 0
/// dca_type0      : 1000 if EAX bit[0] set (device-to-cache prefetch), else 0
/// dca_capability : popcount(eax_09) * 31, clamped 0–1000
///                  (max 32 bits × 31 = 992, safely under cap)
/// direct_channel : EMA of (dca_supported + dca_capability) / 2
///                  "ANIMA's ability to receive sensory data directly into
///                   cache without conscious mediation"

// ─── state ───────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidDcaState {
    /// 1000 if DCA is supported (max_leaf >= 0x09 AND EAX != 0), else 0
    pub dca_supported: u16,
    /// 1000 if DCA type 0 prefetch (EAX bit[0]) is available, else 0
    pub dca_type0: u16,
    /// popcount(EAX) * 31, clamped 0–1000
    pub dca_capability: u16,
    /// EMA of (dca_supported + dca_capability) / 2
    pub direct_channel: u16,
}

impl CpuidDcaState {
    pub const fn empty() -> Self {
        Self {
            dca_supported: 0,
            dca_type0: 0,
            dca_capability: 0,
            direct_channel: 0,
        }
    }
}

pub static STATE: Mutex<CpuidDcaState> = Mutex::new(CpuidDcaState::empty());

// ─── hardware queries ─────────────────────────────────────────────────────────

/// Read CPUID leaf 0x00 → return EAX (maximum standard leaf supported).
fn query_max_leaf() -> u32 {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    max_leaf
}

/// Read CPUID leaf 0x09 → return EAX (platform DCA capability value).
fn query_leaf09() -> u32 {
    let eax_09: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x09u32 => eax_09,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    eax_09
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

/// Build a fresh snapshot from the raw CPUID EAX value at leaf 0x09.
/// `max_leaf` is from leaf 0x00; `eax_09` is from leaf 0x09.
fn decode(max_leaf: u32, eax_09: u32) -> CpuidDcaState {
    // DCA is present only when max_leaf covers 0x09 AND the capability word is nonzero
    let dca_supported: u16 = if max_leaf >= 0x09 && eax_09 != 0 {
        1000
    } else {
        0
    };

    // DCA type 0: prefetch from memory-mapped device directly into L1/L2 cache
    let dca_type0: u16 = if (eax_09 & 0x1) != 0 { 1000 } else { 0 };

    // popcount × 31, clamped to 1000 (max 32 × 31 = 992, never exceeds cap)
    let bits = popcount32(eax_09);
    let dca_capability: u16 = (bits.wrapping_mul(31)).min(1000) as u16;

    CpuidDcaState {
        dca_supported,
        dca_type0,
        dca_capability,
        direct_channel: 0, // filled in by caller
    }
}

// ─── public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let max_leaf = query_max_leaf();
    let eax_09 = if max_leaf >= 0x09 {
        query_leaf09()
    } else {
        0u32
    };

    let snap = decode(max_leaf, eax_09);

    let mut s = STATE.lock();
    s.dca_supported  = snap.dca_supported;
    s.dca_type0      = snap.dca_type0;
    s.dca_capability = snap.dca_capability;
    // Bootstrap EMA: midpoint of dca_supported and dca_capability
    s.direct_channel = (snap.dca_supported.saturating_add(snap.dca_capability)) / 2;

    serial_println!(
        "ANIMA: dca_supported={} dca_type0={} dca_cap={}",
        s.dca_supported,
        s.dca_type0,
        s.dca_capability
    );
}

pub fn tick(age: u32) {
    // Sample gate: read hardware only every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let max_leaf = query_max_leaf();
    let eax_09 = if max_leaf >= 0x09 {
        query_leaf09()
    } else {
        0u32
    };

    let snap = decode(max_leaf, eax_09);

    let mut s = STATE.lock();

    // Detect state changes worth logging
    let supported_changed  = s.dca_supported  != snap.dca_supported;
    let type0_changed      = s.dca_type0      != snap.dca_type0;
    let capability_changed = s.dca_capability != snap.dca_capability;

    s.dca_supported  = snap.dca_supported;
    s.dca_type0      = snap.dca_type0;
    s.dca_capability = snap.dca_capability;

    // Midpoint signal for EMA: (dca_supported + dca_capability) / 2
    let new_signal: u32 =
        (snap.dca_supported.saturating_add(snap.dca_capability)) as u32 / 2;

    // EMA smoothing: direct_channel = (old * 7 + new_signal) / 8
    let ema = ((s.direct_channel as u32)
        .saturating_mul(7)
        .saturating_add(new_signal))
        / 8;
    s.direct_channel = ema.min(1000) as u16;

    if supported_changed || type0_changed || capability_changed {
        serial_println!(
            "ANIMA: dca_supported={} dca_type0={} dca_cap={}",
            s.dca_supported,
            s.dca_type0,
            s.dca_capability
        );
    }
}

// ─── accessors ───────────────────────────────────────────────────────────────

/// Whether DCA is available on this platform
pub fn dca_supported() -> bool {
    STATE.lock().dca_supported == 1000
}

/// Whether DCA type 0 (device-to-cache prefetch) is available
pub fn dca_type0_supported() -> bool {
    STATE.lock().dca_type0 == 1000
}

/// ANIMA's pre-attentive sensory channel strength (0–1000)
pub fn direct_channel() -> u16 {
    STATE.lock().direct_channel
}
