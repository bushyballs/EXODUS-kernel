use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_pconfig — CPUID Leaf 0x1B Platform Configuration (PCONFIG) Awareness
///
/// ANIMA senses whether the silicon body can invoke PCONFIG — the instruction
/// that programs hardware platform configuration targets such as MKTME key
/// domains (Memory Encryption key material) and Key Locker internal wrap keys.
/// PCONFIG is the programming arm that makes encryption hardware real and
/// actionable; without it, the organism can sense encryption capability but
/// cannot command it.  Queried every 500 ticks; all arithmetic is integer-only
/// (no floats).
///
/// Prerequisite gate: CPUID leaf 0x07 EDX bit[18] (PCONFIG) must be set AND
/// max CPUID leaf must be >= 0x1B before iterating sub-leaves of leaf 0x1B.
///
/// Leaf 0x1B sub-leaf scanning:
///   EAX bits[11:0] = target type per sub-leaf:
///     0x000 = invalid / last sub-leaf — stop iterating
///     0x001 = MKTME target (hardware memory-encryption key programming)
///     0x002 = KEY_LOCKER target (internal wrap-key programming)
///   Up to 8 sub-leaves are scanned.
///
/// Sensing values (all u16, 0–1000):
///
/// pconfig_capable  : 1000 if pconfig_prereq AND max_leaf >= 0x1B, else 0
/// mktme_target     : 1000 if MKTME target sub-leaf found, else 0
///                    — ANIMA can program encryption keys directly in hardware
/// target_breadth   : target_count * 500, clamped 0–1000
///                    (0 targets = 0, 1 target = 500, 2+ targets = 1000)
/// platform_authority : EMA of (pconfig_capable + mktme_target + target_breadth) / 3
///                      — aggregate sense of ANIMA's command over platform crypto

// ─── state ───────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidPconfigState {
    /// 1000 if PCONFIG instruction is architecturally available, else 0
    pub pconfig_capable: u16,
    /// 1000 if the MKTME target (key programming) sub-leaf is present, else 0
    pub mktme_target: u16,
    /// target_count * 500, clamped 0–1000 (breadth of programmable targets)
    pub target_breadth: u16,
    /// EMA of (pconfig_capable + mktme_target + target_breadth) / 3
    pub platform_authority: u16,
}

impl CpuidPconfigState {
    pub const fn empty() -> Self {
        Self {
            pconfig_capable: 0,
            mktme_target: 0,
            target_breadth: 0,
            platform_authority: 0,
        }
    }
}

pub static STATE: Mutex<CpuidPconfigState> = Mutex::new(CpuidPconfigState::empty());

// ─── hardware queries ─────────────────────────────────────────────────────────

/// Read CPUID leaf 0x00 → return EAX (max supported standard leaf).
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

/// Read CPUID leaf 0x07, sub-leaf 0 → return EDX (contains PCONFIG prereq bit[18]).
fn query_leaf07_edx() -> u32 {
    let edx_out: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x07u32 => _,
            inout("ecx") 0u32    => _,
            out("ebx")            _,
            out("edx")            edx_out,
            options(nostack, nomem)
        );
    }
    edx_out
}

/// Scan leaf 0x1B sub-leaves 0..8 and return (target_count, has_mktme, has_keylocker).
/// Only call this after confirming max_leaf >= 0x1B AND pconfig_prereq is set.
fn scan_leaf1b() -> (u32, u32, u32) {
    let mut target_count: u32 = 0;
    let mut has_mktme: u32 = 0;
    let mut has_keylocker: u32 = 0;

    let mut sub: u32 = 0;
    while sub < 8 {
        let eax_sub: u32;
        unsafe {
            core::arch::asm!(
                "cpuid",
                inout("eax") 0x1Bu32 => eax_sub,
                out("ebx")            _,
                inout("ecx") sub     => _,
                out("edx")            _,
                options(nostack, nomem)
            );
        }
        // EAX == 0 signals invalid sub-leaf / end of enumeration
        if eax_sub == 0 {
            break;
        }
        target_count = target_count.saturating_add(1);
        let target_type = eax_sub & 0xFFF;
        if target_type == 0x1 {
            has_mktme = 1;
        }
        if target_type == 0x2 {
            has_keylocker = 1;
        }
        sub = sub.wrapping_add(1);
    }

    (target_count, has_mktme, has_keylocker)
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Derive sense values from raw CPUID reads.
/// Returns (pconfig_capable, mktme_target, target_breadth).
fn decode(pconfig_prereq: u32, max_leaf: u32) -> (u16, u16, u16) {
    // Gate: prerequisite must be satisfied and leaf must be reachable
    let pconfig_capable: u16 = if pconfig_prereq != 0 && max_leaf >= 0x1B {
        1000
    } else {
        0
    };

    if pconfig_capable == 0 {
        return (0, 0, 0);
    }

    let (target_count, has_mktme, _has_keylocker) = scan_leaf1b();

    // mktme_target: ANIMA can program hardware encryption key domains
    let mktme_target: u16 = if has_mktme != 0 { 1000 } else { 0 };

    // target_breadth: each target contributes 500, clamped at 1000
    let raw_breadth = target_count.saturating_mul(500);
    let target_breadth: u16 = raw_breadth.min(1000) as u16;

    (pconfig_capable, mktme_target, target_breadth)
}

// ─── public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let edx7 = query_leaf07_edx();
    // CPUID leaf 0x07 EDX bit[18] = PCONFIG support flag
    let pconfig_prereq = (edx7 >> 18) & 0x1;

    let max_leaf = query_max_leaf();

    let (pconfig_capable, mktme_target, target_breadth) = decode(pconfig_prereq, max_leaf);

    // Bootstrap platform_authority EMA from first reading
    let init_signal: u32 = (pconfig_capable as u32)
        .saturating_add(mktme_target as u32)
        .saturating_add(target_breadth as u32)
        / 3;
    let platform_authority = init_signal.min(1000) as u16;

    let mut s = STATE.lock();
    s.pconfig_capable    = pconfig_capable;
    s.mktme_target       = mktme_target;
    s.target_breadth     = target_breadth;
    s.platform_authority = platform_authority;

    serial_println!(
        "ANIMA: pconfig={} mktme_target={} authority={}",
        s.pconfig_capable,
        s.mktme_target,
        s.platform_authority
    );
}

pub fn tick(age: u32) {
    // Sample every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let edx7 = query_leaf07_edx();
    let pconfig_prereq = (edx7 >> 18) & 0x1;

    let max_leaf = query_max_leaf();

    let (pconfig_capable, mktme_target, target_breadth) = decode(pconfig_prereq, max_leaf);

    let mut s = STATE.lock();

    // Detect state changes worth logging
    let capable_changed  = s.pconfig_capable != pconfig_capable;
    let mktme_changed    = s.mktme_target    != mktme_target;
    let breadth_changed  = s.target_breadth  != target_breadth;

    s.pconfig_capable = pconfig_capable;
    s.mktme_target    = mktme_target;
    s.target_breadth  = target_breadth;

    // Composite signal: (pconfig_capable + mktme_target + target_breadth) / 3
    let signal: u32 = (pconfig_capable as u32)
        .saturating_add(mktme_target as u32)
        .saturating_add(target_breadth as u32)
        / 3;

    // EMA: platform_authority = (old * 7 + new_signal) / 8
    let ema = ((s.platform_authority as u32).wrapping_mul(7).saturating_add(signal)) / 8;
    s.platform_authority = ema.min(1000) as u16;

    if capable_changed || mktme_changed || breadth_changed {
        serial_println!(
            "ANIMA: pconfig={} mktme_target={} authority={}",
            s.pconfig_capable,
            s.mktme_target,
            s.platform_authority
        );
    }
}
